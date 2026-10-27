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
            GetAsyncKeyState, keybd_event, KEYEVENTF_KEYUP, GetKeyboardLayout,
            GetKeyboardState, GetKeyState, ToUnicodeEx, VK_CONTROL, VK_D, VK_E, VK_ESCAPE, VK_LWIN,
            VK_MENU, VK_R, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
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

fn default_hotkeys() -> Vec<HotkeyDefinition> {
    vec![
        HotkeyDefinition {
            id: "win_e".to_string(),
            name: "Shell Explorer".to_string(),
            description: "Open in-shell file manager overlay".to_string(),
            key: "Win+E".to_string(),
            vk_code: VK_E.0 as u32,
            win_required: true,
            ctrl_required: false,
            alt_required: false,
            shift_required: false,
            swallow: true,
            enabled: true,
        },
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
        },
    ]
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

/// Класс foreground-окна на момент нажатия Escape.
#[derive(Clone, Copy)]
enum ForegroundClass {
    /// Наше окно: клавиша доходит до DOM нативно, копия задвоила бы событие.
    Shell,
    /// Рабочий стол: наше окно-обои — ребёнок WorkerW, foreground ему не
    /// достаётся, клавиатуру отдаёт только хук. Это штатный контекст оболочки.
    Desktop,
    /// Чужое приложение: юзер ушёл из оболочки.
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
    // Оригинальный failsafe: Esc всегда снимает перехват текстового ввода,
    // иначе после закрытия поля хук продолжит глотать печать под чужим окном.
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
    let pressed = |key: i32| (GetAsyncKeyState(key) as u16 & 0x8000) != 0;
    let win = pressed(VK_LWIN.0 as i32) || pressed(VK_RWIN.0 as i32);
    let ctrl = pressed(VK_CONTROL.0 as i32);
    let alt = pressed(VK_MENU.0 as i32);
    let shift = pressed(VK_SHIFT.0 as i32);

    // Global shortcuts run before text routing; callbacks execute off the hook.
    let hotkey = get_registry().try_read().ok().and_then(|keys| keys.iter().find(|key|
        key.enabled && key.vk_code == vk && key.win_required == win
        && key.ctrl_required == ctrl && key.alt_required == alt
        && key.shift_required == shift).cloned());
    if let Some(key) = hotkey {
        if ACTIONS.get().is_some_and(|tx| tx.try_send(key.id.clone()).is_ok()) {
            if key.id == "win_e" {
                keybd_event(0xE8, 0, Default::default(), 0);
                keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
            }
            if key.swallow {
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
                if id == "win_e" {
                    crate::log_line("hotkeys: Win+E triggered, opening shell explorer");
                    crate::request_wallpaper_visible(app);
                    unidtk_api::emit("explorer-open", serde_json::json!(""));
                } else {
                    unidtk_api::emit("hotkey-pressed", serde_json::json!(id));
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

            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), h_instance, 0) {
                Ok(_hook) => {
                    crate::log_line("hotkeys: WH_KEYBOARD_LL dispatcher installed");
                }
                Err(err) => {
                    crate::log_line(format!("hotkeys: WH_KEYBOARD_LL hook failed: {err}"));
                    return;
                }
            }

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

#[cfg(test)]
thread_local! { static TEST_FOREGROUND: std::cell::Cell<Option<ForegroundClass>> = const { std::cell::Cell::new(None) }; }

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
}
