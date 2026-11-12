use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    OnceLock, RwLock, mpsc::{sync_channel, SyncSender},
};
use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyboardLayout,
            GetKeyboardState, GetKeyState, ToUnicodeEx, VK_CONTROL, VK_D, VK_ESCAPE, VK_LWIN,
            VK_MENU, VK_R, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            PostThreadMessageW, UnhookWindowsHookEx, WM_QUIT,
            CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW,
            GetWindowThreadProcessId, PostMessageW, SetWindowsHookExW,
            TranslateMessage, IsWindow, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_CHAR, WM_KEYDOWN,
            WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub key: String,
    pub vk_code: u32,
    pub win_required: bool,
    pub ctrl_required: bool,
    pub alt_required: bool,
    pub shift_required: bool,
    pub swallow: bool,
    pub enabled: bool,
    pub scope: HotkeyScope,
    /// What a match publishes. `None` for the kernel's own entries, which publish
    /// `hotkey-pressed` with their id; a module's entry names its own event.
    pub event: Option<String>,
    /// Bring the desktop forward before publishing.
    pub shows_desktop: bool,
}

/// Where a hotkey belongs. A `Desktop` hotkey fires only while the desktop or a
/// shell window is in front; anywhere else the key stays with Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HotkeyScope {
    Desktop,
    Global,
}

impl HotkeyScope {
    fn admits(self, foreground: ForegroundClass) -> bool {
        match self {
            HotkeyScope::Global => true,
            HotkeyScope::Desktop => !matches!(foreground, ForegroundClass::Foreign),
        }
    }
}

static HOTKEYS: OnceLock<RwLock<Vec<HotkeyDefinition>>> = OnceLock::new();
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static INPUT_FOCUSED: AtomicBool = AtomicBool::new(false);
static INPUT_GENERATION: AtomicU32 = AtomicU32::new(0);
// Escape release stays with the original target across foreground changes.
thread_local! {
    static ESC_TARGET: std::cell::Cell<Option<isize>> = const { std::cell::Cell::new(None) };
    static ESC_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The kernel's own entries; a module adds its own through [`register_module_hotkeys`].
fn default_hotkeys() -> Vec<HotkeyDefinition> {
    vec![
        HotkeyDefinition {
            id: "win_d".to_string(),
            name: "Show Desktop".to_string(),
            description: "Toggle visibility of desktop widgets".to_string(),
            key: "Win+D".to_string(),
            vk_code: VK_D.0 as u32,
            win_required: true,
            ctrl_required: false,
            alt_required: false,
            shift_required: false,
            swallow: false,
            enabled: false,
            scope: HotkeyScope::Global,
            event: None,
            shows_desktop: false,
        },
        HotkeyDefinition {
            id: "win_r".to_string(),
            name: "Shell Run".to_string(),
            description: "Open shell launcher or terminal".to_string(),
            key: "Win+R".to_string(),
            vk_code: VK_R.0 as u32,
            win_required: true,
            ctrl_required: false,
            alt_required: false,
            shift_required: false,
            swallow: false,
            enabled: false,
            scope: HotkeyScope::Global,
            event: None,
            shows_desktop: false,
        },
    ]
}

/// Adds the modules' hotkeys (`Module::hotkeys`) to the table the hook reads. An id
/// already present is left as it is, so a second call changes nothing.
pub fn register_module_hotkeys<'a>(specs: impl IntoIterator<Item = &'a unidtk_api::HotkeySpec>) {
    let Ok(mut table) = get_registry().write() else {
        return;
    };
    for spec in specs {
        if table.iter().any(|entry| entry.id == spec.id) {
            continue;
        }
        table.push(HotkeyDefinition {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            description: spec.description.to_string(),
            key: spec.key.to_string(),
            vk_code: spec.vk_code,
            win_required: spec.win,
            ctrl_required: spec.ctrl,
            alt_required: spec.alt,
            shift_required: spec.shift,
            swallow: spec.swallow,
            enabled: true,
            scope: if spec.desktop_only { HotkeyScope::Desktop } else { HotkeyScope::Global },
            event: Some(spec.event.to_string()),
            shows_desktop: spec.shows_desktop,
        });
    }
}

fn get_registry() -> &'static RwLock<Vec<HotkeyDefinition>> {
    HOTKEYS.get_or_init(|| RwLock::new(default_hotkeys()))
}

pub fn list_hotkeys() -> Vec<HotkeyDefinition> {
    get_registry().read().map(|h| h.clone()).unwrap_or_default()
}

pub fn toggle_hotkey(id: &str, enabled: bool) -> Result<(), String> {
    let mut reg = get_registry().write().map_err(|e| e.to_string())?;
    if let Some(entry) = reg.iter_mut().find(|h| h.id == id) {
        entry.enabled = enabled;
        crate::log_line(format!("hotkeys: toggled '{id}' -> {enabled}"));
        Ok(())
    } else {
        Err(format!("Hotkey '{id}' not found in registry"))
    }
}

pub fn set_input_focus(_app: &AppHandle, focused: bool) -> Result<(), String> {
    // A delayed DOM focus message cannot claim input after the user left us.
    let focused = focused && !matches!(foreground_class(), ForegroundClass::Foreign);
    INPUT_FOCUSED.store(focused, Ordering::SeqCst);
    INPUT_GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

pub fn reset_input_focus() {
    INPUT_FOCUSED.store(false, Ordering::SeqCst);
    INPUT_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Class of the foreground window at the moment Escape is pressed.
#[derive(Clone, Copy)]
enum ForegroundClass {
    /// Our own window: the key reaches the DOM natively; a copy would double the event.
    Shell,
    /// Desktop: our wallpaper window is a child of WorkerW, so it does not
    /// receive the foreground; only the hook delivers keyboard input. This is the normal shell context.
    Desktop,
    /// Another application: the user has left the shell.
    Foreign,
}

fn foreground_class() -> ForegroundClass {
    #[cfg(test)]
    if let Some(class) = TEST_FOREGROUND.with(|value| value.get()) { return class; }
    unsafe {
        let fg = GetForegroundWindow();
        if HWND::is_invalid(&fg) {
            // Unknown foreground must never grant global text interception.
            return ForegroundClass::Foreign;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid as *mut u32));
        if pid == std::process::id() {
            ForegroundClass::Shell
        } else if crate::input::is_desktop_class(fg) {
            ForegroundClass::Desktop
        } else {
            ForegroundClass::Foreign
        }
    }
}

/// Copy Escape only from desktop; foreign applications keep native input.
/// The release uses the original validated target, never a newly focused HWND.
fn forward_escape_to_shell(vk: u32, msg: u32, scan_code: isize) {
    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    // Original failsafe: Esc always releases the text-input capture,
    // otherwise after the field closes the hook would keep swallowing typing into a foreign window.
    reset_input_focus();

    let target = if is_down {
        if ESC_DOWN.with(|slot| slot.replace(true)) {
            return;
        }
        let target = if matches!(foreground_class(), ForegroundClass::Desktop) {
            crate::input::get_last_key_target().map(|target| target.0 as isize)
        } else { None };
        ESC_TARGET.with(|slot| slot.set(target));
        target
    } else {
        ESC_DOWN.with(|slot| slot.set(false));
        ESC_TARGET.with(|slot| slot.take())
    };
    if let Some(target) = target {
        let target = HWND(target as _);
        if crate::input::is_key_target(target) {
            let _ = unsafe { PostMessageW(target, msg, WPARAM(vk as usize),
                key_lparam(scan_code as u32, false, !is_down)) };
        }
    }
}

// Ownership is local to the hook thread; system modifier pairs are never owned.
#[derive(Clone, Copy)]
struct KeyOwner { target: isize, key_messages: bool, scan: u32, extended: bool, generation: u32 }
thread_local! {
    static KEY_OWNERS: std::cell::RefCell<[Option<KeyOwner>; 256]> = const {
        std::cell::RefCell::new([None; 256])
    };
}
static ACTIONS: OnceLock<SyncSender<String>> = OnceLock::new();

fn system_key(vk: u32) -> bool {
    matches!(vk, 0x10..=0x14 | 0x5B..=0x5C | 0x90..=0x91 | 0xA0..=0xA5)
}
fn key_lparam(scan: u32, extended: bool, up: bool) -> LPARAM {
    LPARAM((1usize | ((scan as usize & 0xff) << 16)
        | ((extended as usize) << 24) | if up { 3usize << 30 } else { 0 }) as isize)
}
fn remember_key(vk: u32, owner: KeyOwner) {
    KEY_OWNERS.with(|keys| keys.borrow_mut()[vk as usize] = Some(owner));
}

#[derive(Clone, Copy, Default)]
struct Modifiers { win: bool, ctrl: bool, alt: bool, shift: bool }

fn modifiers() -> Modifiers {
    #[cfg(test)]
    if let Some(held) = TEST_MODIFIERS.with(|value| value.get()) { return held; }
    let pressed = |key: i32| unsafe { (GetAsyncKeyState(key) as u16 & 0x8000) != 0 };
    Modifiers {
        win: pressed(VK_LWIN.0 as i32) || pressed(VK_RWIN.0 as i32),
        ctrl: pressed(VK_CONTROL.0 as i32),
        alt: pressed(VK_MENU.0 as i32),
        shift: pressed(VK_SHIFT.0 as i32),
    }
}

/// A Win chord that we consume must not open the Start menu on the Win release:
/// an unassigned key between the Win down and up cancels it.
fn mask_start_menu() {
    #[cfg(test)]
    { START_MASKS.with(|count| count.set(count.get() + 1)); }
    #[cfg(not(test))]
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
        keybd_event(0xE8, 0, Default::default(), 0);
        keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
    }
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 { return CallNextHookEx(None, code, wparam, lparam); }
    let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    // Our Start-mask injection and other synthetic input must not enter routing.
    if kb.flags.0 & LLKHF_INJECTED.0 != 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let msg = wparam.0 as u32;
    let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);
    let vk = kb.vkCode;
    if vk >= 256 || (!down && !up) {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let foreground = foreground_class();
    if matches!(foreground, ForegroundClass::Foreign) { reset_input_focus(); }

    // A release belongs to the original successful down, even after Alt+Tab.
    let owner = KEY_OWNERS.with(|keys| {
        let mut keys = keys.borrow_mut();
        if up { keys[vk as usize].take() } else { keys[vk as usize] }
    });
    if let Some(owner) = owner {
        if up {
            if owner.target != 0 && owner.key_messages && crate::input::is_key_target(HWND(owner.target as _)) {
                let _ = PostMessageW(HWND(owner.target as _), WM_KEYUP, WPARAM(vk as usize),
                    key_lparam(owner.scan, owner.extended, true));
            }
            return LRESULT(1);
        }
        // Never leak repeats of a swallowed down into a newly active application.
        if matches!(foreground, ForegroundClass::Desktop) && owner.target != 0
            && owner.generation == INPUT_GENERATION.load(Ordering::SeqCst)
            && INPUT_FOCUSED.load(Ordering::SeqCst) && crate::input::is_key_target(HWND(owner.target as _)) {
            if owner.key_messages {
                let repeat = key_lparam(owner.scan, owner.extended, false).0 | (1isize << 30);
                let _ = PostMessageW(HWND(owner.target as _), WM_KEYDOWN, WPARAM(vk as usize), LPARAM(repeat));
            } else {
                // Continue text translation only into the original target below.
            }
            if owner.key_messages { return LRESULT(1); }
        } else {
            return LRESULT(1);
        }
    } else if up {
        // This includes every modifier/toggle release: its down went to Windows.
        if vk == VK_ESCAPE.0 as u32 { forward_escape_to_shell(vk, msg, kb.scanCode as isize); }
        return CallNextHookEx(None, code, wparam, lparam);
    }

    if vk == VK_ESCAPE.0 as u32 {
        forward_escape_to_shell(vk, msg, kb.scanCode as isize);
        return CallNextHookEx(None, code, wparam, lparam);
    }
    if system_key(vk) { return CallNextHookEx(None, code, wparam, lparam); }
    let Modifiers { win, ctrl, alt, shift } = modifiers();

    // Global shortcuts run before text routing; callbacks execute off the hook.
    let hotkey = get_registry().try_read().ok().and_then(|keys| keys.iter().find(|key|
        key.enabled && key.vk_code == vk && key.win_required == win
        && key.ctrl_required == ctrl && key.alt_required == alt
        && key.shift_required == shift && key.scope.admits(foreground)).cloned());
    if let Some(key) = hotkey {
        if ACTIONS.get().is_some_and(|tx| tx.try_send(key.id.clone()).is_ok()) {
            if key.swallow {
                if key.win_required { mask_start_menu(); }
                remember_key(vk, KeyOwner { target: 0, key_messages: false,
                    scan: kb.scanCode, extended: kb.flags.0 & 1 != 0,
                    generation: INPUT_GENERATION.load(Ordering::SeqCst) });
                return LRESULT(1);
            }
        }
        return CallNextHookEx(None, code, wparam, lparam);
    }
    // Foreign windows always receive native input; our own foreground does too.
    if !matches!(foreground, ForegroundClass::Desktop)
        || !INPUT_FOCUSED.load(Ordering::SeqCst) || win || alt
    { return if owner.is_some() { LRESULT(1) } else { CallNextHookEx(None, code, wparam, lparam) }; }
    let target = owner.filter(|owned| owned.target != 0).map(|owned| HWND(owned.target as _))
        .or_else(crate::input::get_last_key_target);
    let Some(target) = target else {
        reset_input_focus();
        return CallNextHookEx(None, code, wparam, lparam);
    };
    if !IsWindow(target).as_bool() || !crate::input::is_key_target(target) {
        reset_input_focus();
        return CallNextHookEx(None, code, wparam, lparam);
    }
    // Unknown Ctrl chords remain with Windows. Supported editing chords are
    // delivered as key messages, while ordinary text is translated only once.
    let chord = ctrl && matches!(vk, 0x41 | 0x43 | 0x56 | 0x58 | 0x5A);
    if ctrl && !chord { return if owner.is_some() { LRESULT(1) } else { CallNextHookEx(None, code, wparam, lparam) }; }
    let navigation = matches!(vk, 0x08 | 0x09 | 0x0D | 0x2E | 0x25..=0x28 | 0x24 | 0x23);
    let key_messages = chord || navigation;
    let delivered = if key_messages {
        PostMessageW(target, WM_KEYDOWN, WPARAM(vk as usize),
            key_lparam(kb.scanCode, kb.flags.0 & 1 != 0, false)).is_ok()
    } else {
        let mut key_state = [0u8; 256];
        let _ = GetKeyboardState(&mut key_state);
        key_state[VK_SHIFT.0 as usize] = if shift { 0x80 } else { 0 };
        key_state[VK_CONTROL.0 as usize] = if ctrl { 0x80 } else { 0 };
        key_state[VK_MENU.0 as usize] = if alt { 0x80 } else { 0 };
        key_state[0x14] = (GetKeyState(0x14) as u8) & 1;
        let hkl = GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), None));
        let mut chars = [0u16; 8];
        let count = ToUnicodeEx(vk, kb.scanCode, &key_state, &mut chars, 0x04, hkl);
        count > 0 && chars[..(count as usize).min(chars.len())].iter().all(|ch|
            PostMessageW(target, WM_CHAR, WPARAM(*ch as usize), LPARAM(1)).is_ok())
    };
    if delivered {
        remember_key(vk, KeyOwner { target: target.0 as isize, key_messages,
            scan: kb.scanCode, extended: kb.flags.0 & 1 != 0,
            generation: INPUT_GENERATION.load(Ordering::SeqCst) });
        return LRESULT(1);
    }
    if owner.is_some() { LRESULT(1) } else { CallNextHookEx(None, code, wparam, lparam) }
}

pub fn init(app: AppHandle) {
    let _ = APP_HANDLE.set(app);
    let (tx, rx) = sync_channel::<String>(32);
    let _ = ACTIONS.set(tx);
    std::thread::Builder::new().name("hotkey-actions".into()).spawn(move || {
        while let Ok(id) = rx.recv() {
            if crate::is_shutting_down() { break; }
            if let Some(app) = APP_HANDLE.get() {
                let action = get_registry().read().ok().and_then(|table| {
                    table.iter().find(|entry| entry.id == id).map(|entry| (entry.event.clone(), entry.shows_desktop))
                });
                match action {
                    Some((Some(event), shows_desktop)) => {
                        crate::log_line(format!("hotkeys: {id} triggered, publishing {event}"));
                        if shows_desktop {
                            crate::request_wallpaper_visible(app);
                        }
                        unidtk_api::emit(&event, serde_json::json!(""));
                    }
                    _ => unidtk_api::emit("hotkey-pressed", serde_json::json!(id)),
                }
            }
        }
    }).expect("spawn hotkey dispatcher");

    std::thread::spawn(|| {
        unsafe {
            let h_instance = match GetModuleHandleW(None) {
                Ok(h) => HINSTANCE(h.0),
                Err(_) => HINSTANCE::default(),
            };

            let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), h_instance, 0) {
                Ok(hook) => {
                    crate::log_line("hotkeys: WH_KEYBOARD_LL dispatcher installed");
                    hook
                }
                Err(err) => {
                    crate::log_line(format!("hotkeys: WH_KEYBOARD_LL hook failed: {err}"));
                    return;
                }
            };
            HOOK_THREAD.store(windows::Win32::System::Threading::GetCurrentThreadId(), Ordering::SeqCst);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // The hook belongs to this thread, so it is released here.
            let _ = UnhookWindowsHookEx(hook);
        }
    });
}

/// Thread that owns the keyboard hook; 0 until it is installed.
static HOOK_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Ask the hook thread to leave its message loop and unhook. Does not wait.
pub fn shutdown() {
    let thread = HOOK_THREAD.swap(0, Ordering::SeqCst);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_FOREGROUND: std::cell::Cell<Option<ForegroundClass>> = const { std::cell::Cell::new(None) };
    static TEST_MODIFIERS: std::cell::Cell<Option<Modifiers>> = const { std::cell::Cell::new(None) };
    static START_MASKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod input_safety_tests {
    use super::*;
    // Exercise the real callback with stale DOM focus and native (non-injected)
    // records. No physical keys are synthesized and no hook is installed.
    #[test]
    fn foreign_input_and_system_pairs_are_never_stolen() {
        TEST_FOREGROUND.with(|slot| slot.set(Some(ForegroundClass::Foreign)));
        let callback = |vk, msg| unsafe {
            let record = KBDLLHOOKSTRUCT { vkCode: vk, scanCode: 28, ..Default::default() };
            keyboard_hook_proc(0, WPARAM(msg as usize), LPARAM(&record as *const _ as isize)).0
        };
        for vk in [0x0D, 0x41, 0x08, 0x14, 0x10, 0x11, 0x12, 0x5B, 0xA0, 0x90] {
            INPUT_FOCUSED.store(true, Ordering::SeqCst);
            assert_ne!(callback(vk, WM_KEYDOWN), 1, "stole foreign down {vk}");
            assert!(!INPUT_FOCUSED.load(Ordering::SeqCst));
            assert_ne!(callback(vk, WM_KEYUP), 1, "stole foreign up {vk}");
        }
        TEST_FOREGROUND.with(|slot| slot.set(Some(ForegroundClass::Desktop)));
        for vk in [0x14, 0x10, 0x11, 0x12, 0x5B, 0xA0, 0x90] {
            INPUT_FOCUSED.store(true, Ordering::SeqCst);
            assert_ne!(callback(vk, WM_KEYDOWN), 1);
            assert_ne!(callback(vk, WM_KEYUP), 1);
        }
        TEST_FOREGROUND.with(|slot| slot.set(Some(ForegroundClass::Foreign)));
        remember_key(0x45, KeyOwner { target: 0, key_messages: false, scan: 18,
            extended: false, generation: INPUT_GENERATION.load(Ordering::SeqCst) });
        assert_eq!(callback(0x45, WM_KEYDOWN), 1, "leaked owned repeat after switch");
        assert_eq!(callback(0x45, WM_KEYUP), 1, "lost owned release after switch");
        assert_ne!(callback(0x45, WM_KEYUP), 1, "stole unowned release");
        KEY_OWNERS.with(|keys| *keys.borrow_mut() = [None; 256]);
        reset_input_focus();
        TEST_FOREGROUND.with(|slot| slot.set(None));
    }

    // Win+E belongs to Windows outside the shell: no action, no Start mask, no owned key.
    #[test]
    fn desktop_hotkey_fires_only_on_the_desktop_or_the_shell() {
        // Win+E is the Explorer module's, not the kernel's.
        let explorer = unidtk_mod_explorer::create_module();
        register_module_hotkeys(explorer.hotkeys());
        register_module_hotkeys(explorer.hotkeys());
        assert_eq!(list_hotkeys().iter().filter(|entry| entry.id == "win_e").count(), 1, "registered once");
        let (tx, rx) = sync_channel::<String>(8);
        assert!(ACTIONS.set(tx).is_ok(), "only this test owns the action channel");
        TEST_MODIFIERS.with(|slot| slot.set(Some(Modifiers { win: true, ..Modifiers::default() })));
        let callback = |msg| unsafe {
            let record = KBDLLHOOKSTRUCT { vkCode: 0x45 /* VK_E */, scanCode: 18, ..Default::default() };
            keyboard_hook_proc(0, WPARAM(msg as usize), LPARAM(&record as *const _ as isize)).0
        };

        TEST_FOREGROUND.with(|slot| slot.set(Some(ForegroundClass::Foreign)));
        assert_ne!(callback(WM_KEYDOWN), 1, "Win+E stolen from a foreign application");
        assert!(rx.try_recv().is_err(), "overlay opened over a foreign application");
        assert_eq!(START_MASKS.with(|n| n.get()), 0, "Start menu masked for a key we passed on");
        assert_ne!(callback(WM_KEYUP), 1, "release of a passed key stolen");

        for class in [ForegroundClass::Desktop, ForegroundClass::Shell] {
            TEST_FOREGROUND.with(|slot| slot.set(Some(class)));
            assert_eq!(callback(WM_KEYDOWN), 1, "Win+E not consumed in the shell");
            assert_eq!(rx.try_recv().as_deref(), Ok("win_e"));
            assert_eq!(callback(WM_KEYUP), 1, "release of a consumed key leaked");
        }
        assert_eq!(START_MASKS.with(|n| n.get()), 2);

        KEY_OWNERS.with(|keys| *keys.borrow_mut() = [None; 256]);
        TEST_MODIFIERS.with(|slot| slot.set(None));
        TEST_FOREGROUND.with(|slot| slot.set(None));
    }
}
