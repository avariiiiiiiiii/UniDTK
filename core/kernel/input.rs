use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};

use windows::{
    core::s,
    Win32::{
        Devices::HumanInterfaceDevice::{
            HID_USAGE_GENERIC_MOUSE, HID_USAGE_PAGE_GENERIC,
        },
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::ScreenToClient,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Input, Input::KeyboardAndMouse, WindowsAndMessaging},
    },
};

use windows::Win32::UI::Input::RAWINPUT;

struct WindowContext {
    window_handle: isize,
    forward_mouse_input: bool,
    #[allow(dead_code)]
    forward_keyboard_input: bool,
    /// Last WebView/child HWND that received a click (keyboard target when
    /// GetFocus() is empty because WorkerW never activates our process).
    last_key_target: isize,
}

static WINDOWS: OnceLock<Mutex<Vec<WindowContext>>> = OnceLock::new();
static RAW_INPUT_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

static LEFT_BUTTON_HELD: AtomicBool = AtomicBool::new(false);
static RIGHT_BUTTON_HELD: AtomicBool = AtomicBool::new(false);
static MIDDLE_BUTTON_HELD: AtomicBool = AtomicBool::new(false);

const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_SHIFT: usize = 0x0004;
const MK_CONTROL: usize = 0x0008;
const MK_MBUTTON: usize = 0x0010;

#[inline]
fn matches_utf16(slice: &[u16], target: &str) -> bool {
    if slice.len() != target.len() {
        return false;
    }
    target.encode_utf16().eq(slice.iter().copied())
}

#[inline]
pub(crate) fn is_desktop_class(hwnd: HWND) -> bool {
    let mut class_name = [0u16; 64];
    let len = unsafe { WindowsAndMessaging::GetClassNameW(hwnd, &mut class_name) } as usize;
    if len == 0 {
        return false;
    }
    let s = &class_name[..len];
    matches_utf16(s, "WorkerW")
        || matches_utf16(s, "Progman")
        || matches_utf16(s, "SysListView32")
        || matches_utf16(s, "SHELLDLL_DefView")
}

fn get_current_mouse_wparam() -> usize {
    let mut flags = 0usize;
    unsafe {
        if (KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0
            || LEFT_BUTTON_HELD.load(Ordering::SeqCst)
        {
            flags |= MK_LBUTTON;
        }
        if (KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_RBUTTON.0 as i32) as u16 & 0x8000) != 0
            || RIGHT_BUTTON_HELD.load(Ordering::SeqCst)
        {
            flags |= MK_RBUTTON;
        }
        if (KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_MBUTTON.0 as i32) as u16 & 0x8000) != 0
            || MIDDLE_BUTTON_HELD.load(Ordering::SeqCst)
        {
            flags |= MK_MBUTTON;
        }
        if (KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_SHIFT.0 as i32) as u16 & 0x8000) != 0 {
            flags |= MK_SHIFT;
        }
        if (KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 {
            flags |= MK_CONTROL;
        }
    }
    flags
}

/// Point Chromium's caret at `target` without joining the desktop input queue.
///
/// Earlier we used AttachThreadInput+SetFocus so the OS would deliver real
/// keyboard messages into the WorkerW child. That worked — but combined with
/// our raw-input WM_CHAR injection every glyph landed twice (or thrice). Text
/// entry now comes only from raw-input synthesis (see send_keyboard_input);
/// this helper only places focus for caret/selection.
fn force_focus(target: HWND) {
    unsafe {
        if HWND::is_invalid(&target) {
            return;
        }
        let _ = KeyboardAndMouse::SetFocus(target);
    }
}

fn make_wparam(low: usize, high: usize) -> WPARAM {
    let combined = (high << 16) | low;
    WPARAM(combined)
}

fn make_lparam(low: isize, high: isize) -> LPARAM {
    let combined = (high << 16) | low;
    LPARAM(combined)
}

fn send_mouse_input(msg: u32, point: POINT, w_param: WPARAM) {
    unsafe {
        let hwnd_under_cursor = WindowsAndMessaging::WindowFromPoint(point);
        let is_desktop = is_desktop_class(hwnd_under_cursor);

        let Some(windows) = WINDOWS.get() else { return; };
        let mut windows_lock = match windows.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        // Collect focus updates after the pass so we can mutate last_key_target
        // without holding an immutable borrow across the loop.
        let mut focus_updates: Vec<(isize, isize)> = Vec::new();

        for window in windows_lock.iter() {
            if window.forward_mouse_input {
                let win_hwnd = HWND(window.window_handle as _);
                let is_own_window = hwnd_under_cursor == win_hwnd
                    || WindowsAndMessaging::IsChild(win_hwnd, hwnd_under_cursor)
                        .as_bool();
                if !is_desktop && !is_own_window {
                    if msg == WindowsAndMessaging::WM_LBUTTONDOWN {
                        crate::hotkeys::reset_input_focus();
                    }
                    continue;
                }

                let target = child_window_at_point(win_hwnd, point);
                if HWND::is_invalid(&target) {
                    continue;
                }
                let mut client_point = point.clone();
                // Race: the HWND we just resolved (often a transient Chromium
                // surface created by overlay widgets like the music playlist
                // popup) can be destroyed before we get here. Don't unwrap —
                // with panic=abort in release that kills the whole process,
                // and the user perceives the desktop as frozen.
                if ScreenToClient(target, &mut client_point).as_bool() == false {
                    continue;
                }

                let l_param = make_lparam(client_point.x as isize, client_point.y as isize);

                let _ = WindowsAndMessaging::PostMessageA(target, msg, w_param, l_param);

                // A forwarded click must also place caret focus inside our
                // process so the WebView knows which control receives the
                // raw-input WM_CHAR stream (see send_keyboard_input).
                if msg == WindowsAndMessaging::WM_LBUTTONDOWN {
                    force_focus(target);
                    focus_updates.push((window.window_handle, target.0 as isize));
                }
            }
        }

        for (root, target) in focus_updates {
            if let Some(w) = windows_lock
                .iter_mut()
                .find(|w| w.window_handle == root)
            {
                w.last_key_target = target;
            }
        }
    }
}

pub fn get_last_key_target() -> Option<HWND> {
    // Called from the global keyboard hook: contention must let input through.
    let windows = WINDOWS.get()?.try_lock().ok()?;
    for w in windows.iter() {
        if w.last_key_target != 0 {
            let hwnd = HWND(w.last_key_target as _);
            let root = HWND(w.window_handle as _);
            if unsafe { WindowsAndMessaging::IsWindow(hwnd).as_bool()
                && WindowsAndMessaging::IsWindow(root).as_bool()
                && (hwnd == root || WindowsAndMessaging::IsChild(root, hwnd).as_bool()) } {
                return Some(hwnd);
            }
        }
    }
    None
}

pub fn is_key_target(target: HWND) -> bool {
    let Some(windows) = WINDOWS.get().and_then(|windows| windows.try_lock().ok()) else { return false };
    windows.iter().any(|window| unsafe {
        let root = HWND(window.window_handle as _);
        let mut pid = 0;
        WindowsAndMessaging::GetWindowThreadProcessId(root, Some(&mut pid));
        pid == std::process::id() && WindowsAndMessaging::IsWindow(root).as_bool()
            && WindowsAndMessaging::IsWindow(target).as_bool()
            && (target == root || WindowsAndMessaging::IsChild(root, target).as_bool())
    })
}

/// Forward a vertical mouse-wheel notch to the webview. A posted
/// `WM_MOUSEWHEEL` is ignored by Chromium (unlike the posted button/move
/// messages, wheel input is consumed via a different path that checks real
/// input state), so instead the notch is delivered as a Tauri event —
/// `renderer.js` listens for `wallpaper-wheel` and scrolls the DOM element
/// under the cursor. Coordinates are converted to client space here; the
/// frontend divides by devicePixelRatio.
fn send_mouse_wheel(point: POINT, wheel_delta: i16) {
    unsafe {
        let hwnd_under_cursor = WindowsAndMessaging::WindowFromPoint(point);
        let is_desktop = is_desktop_class(hwnd_under_cursor);

        let Some(windows) = WINDOWS.get() else { return; };
        let windows_lock = match windows.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        for window in windows_lock.iter() {
            if window.forward_mouse_input {
                let win_hwnd = HWND(window.window_handle as _);
                let is_own_window = hwnd_under_cursor == win_hwnd
                    || WindowsAndMessaging::IsChild(win_hwnd, hwnd_under_cursor)
                        .as_bool();
                if !is_desktop && !is_own_window {
                    continue;
                }

                let mut client_point = point;
                if !ScreenToClient(win_hwnd, &mut client_point).as_bool() {
                    continue;
                }

                unidtk_api::emit(
                    "wallpaper-wheel",
                    serde_json::json!({
                        "x": client_point.x,
                        "y": client_point.y,
                        "delta": wheel_delta,
                    }),
                );
            }
        }
    }
}

fn child_window_at_point(root: HWND, screen_point: POINT) -> HWND {
    unsafe {
        let mut client_point = screen_point;
        if !ScreenToClient(root, &mut client_point).as_bool() {
            return root;
        }

        let child = WindowsAndMessaging::ChildWindowFromPointEx(
            root,
            client_point,
            WindowsAndMessaging::CWP_SKIPDISABLED
                | WindowsAndMessaging::CWP_SKIPINVISIBLE
                | WindowsAndMessaging::CWP_SKIPTRANSPARENT,
        );

        if HWND::is_invalid(&child) || child == root {
            root
        } else {
            child_window_at_point(child, screen_point)
        }
    }
}

fn handle_mouse_input(raw_data: &RAWINPUT) {
    let mouse_data = unsafe { raw_data.data.mouse };
    let mouse_button_flags = unsafe { mouse_data.Anonymous.Anonymous.usButtonFlags } as u32;

    let mut point = POINT::default();

    unsafe {
        if WindowsAndMessaging::GetCursorPos(&mut point).is_err() {
            return;
        }
    }

    // Update held states on button events
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_1_DOWN != 0 {
        LEFT_BUTTON_HELD.store(true, Ordering::SeqCst);
    }
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_1_UP != 0 {
        LEFT_BUTTON_HELD.store(false, Ordering::SeqCst);
    }
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_2_DOWN != 0 {
        RIGHT_BUTTON_HELD.store(true, Ordering::SeqCst);
    }
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_2_UP != 0 {
        RIGHT_BUTTON_HELD.store(false, Ordering::SeqCst);
    }
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_3_DOWN != 0 {
        MIDDLE_BUTTON_HELD.store(true, Ordering::SeqCst);
    }
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_3_UP != 0 {
        MIDDLE_BUTTON_HELD.store(false, Ordering::SeqCst);
    }

    let current_wparam = get_current_mouse_wparam();

    // Always send WM_MOUSEMOVE first so that Chromium can properly
    // hit-test which element is under the cursor before processing
    // any button events. When left button is held, current_wparam includes MK_LBUTTON,
    // which allows Chromium to continue scrollbar thumb dragging and selection.
    send_mouse_input(
        WindowsAndMessaging::WM_MOUSEMOVE,
        point,
        WPARAM(current_wparam),
    );

    if mouse_button_flags == 0 {
        // Pure mouse move, already handled above
        return;
    }

    // Vertical wheel is not taken from raw input: a precision touchpad's
    // two-finger scroll never shows up here (the touchpad is a HID digitizer,
    // not a mouse). Every wheel — mouse notch or touchpad — comes from the
    // low-level hook instead (start_wheel_hook), so there is one path only.
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_WHEEL != 0 {
        return;
    }

    // Horizontal wheel is not used by any widget; ignore it.
    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_HWHEEL != 0 {
        return;
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_1_DOWN != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_LBUTTONDOWN,
            point,
            WPARAM(current_wparam | MK_LBUTTON),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_1_UP != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_LBUTTONUP,
            point,
            WPARAM(current_wparam & !MK_LBUTTON),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_2_DOWN != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_RBUTTONDOWN,
            point,
            WPARAM(current_wparam | MK_RBUTTON),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_2_UP != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_RBUTTONUP,
            point,
            WPARAM(current_wparam & !MK_RBUTTON),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_3_DOWN != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_MBUTTONDOWN,
            point,
            WPARAM(current_wparam | MK_MBUTTON),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_3_UP != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_MBUTTONUP,
            point,
            WPARAM(current_wparam & !MK_MBUTTON),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_4_DOWN != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_XBUTTONDOWN,
            point,
            make_wparam(current_wparam, WindowsAndMessaging::XBUTTON1 as usize),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_4_UP != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_XBUTTONUP,
            point,
            make_wparam(current_wparam, WindowsAndMessaging::XBUTTON1 as usize),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_5_DOWN != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_XBUTTONDOWN,
            point,
            make_wparam(current_wparam, WindowsAndMessaging::XBUTTON2 as usize),
        );
    }

    if mouse_button_flags & WindowsAndMessaging::RI_MOUSE_BUTTON_5_UP != 0 {
        send_mouse_input(
            WindowsAndMessaging::WM_XBUTTONUP,
            point,
            make_wparam(current_wparam, WindowsAndMessaging::XBUTTON2 as usize),
        );
    }
}





fn read_input(l_param: LPARAM) {
    let mut raw_data = RAWINPUT {
        ..Default::default()
    };
    let mut raw_data_size = size_of::<RAWINPUT>() as u32;
    let raw_data_header_size = size_of::<Input::RAWINPUTHEADER>() as u32;

    let raw_input_data = unsafe {
        Input::GetRawInputData(
            Input::HRAWINPUT(l_param.0 as _),
            Input::RID_INPUT,
            Some(&mut raw_data as *mut _ as *mut _),
            &mut raw_data_size,
            raw_data_header_size,
        )
    };

    if raw_input_data == u32::MAX {
        return;
    }

    let device_type = Input::RID_DEVICE_INFO_TYPE(raw_data.header.dwType);

    if device_type == Input::RIM_TYPEMOUSE {
        handle_mouse_input(&raw_data);
        return;
    }
}

unsafe extern "system" fn handle_window_message(
    hwnd: HWND,
    msg: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    // Handle raw input messages
    if msg == WindowsAndMessaging::WM_INPUT {
        read_input(l_param);
    }

    if msg == WindowsAndMessaging::WM_DISPLAYCHANGE
        || msg == WindowsAndMessaging::WM_SETTINGCHANGE
        || msg == WindowsAndMessaging::WM_DPICHANGED
    {
        crate::log_line(format!("input: window received display/setting change msg=0x{msg:X}"));
        crate::trigger_display_change();
    }

    static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
    let taskbar_msg = *TASKBAR_CREATED.get_or_init(|| {
        WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!("TaskbarCreated"))
    });
    if taskbar_msg != 0 && msg == taskbar_msg {
        crate::log_line("input: received TaskbarCreated message (Explorer restarted), re-attaching desktop");
        if let Some(app) = crate::APP_HANDLE.get() {
            crate::request_wallpaper_visible(app);
        }
    }

    if msg == WindowsAndMessaging::WM_CLOSE {
        println!("Raw input window closing");
    }

    // Call the default window procedure
    WindowsAndMessaging::DefWindowProcA(hwnd, msg, w_param, l_param)
}

// ── Wheel via WH_MOUSE_LL ─────────────────────────────────────────────────────
// The low-level hook sees every WM_MOUSEWHEEL the system produces, including
// the ones Windows synthesizes from touchpad scroll gestures (fine-grained
// deltas, not only multiples of 120). The hook never swallows input and only
// hands the event to a worker thread, so it cannot stall the input queue.

static WHEEL_TX: OnceLock<std::sync::mpsc::Sender<(POINT, i16)>> = OnceLock::new();
static WHEEL_HOOK: std::sync::Once = std::sync::Once::new();

unsafe extern "system" fn wheel_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WindowsAndMessaging::WM_MOUSEWHEEL {
        let info = &*(lparam.0 as *const WindowsAndMessaging::MSLLHOOKSTRUCT);
        let delta = (info.mouseData >> 16) as u16 as i16;
        if let Some(tx) = WHEEL_TX.get() {
            let _ = tx.send((info.pt, delta));
        }
    }
    WindowsAndMessaging::CallNextHookEx(None, code, wparam, lparam)
}

fn start_wheel_hook() {
    WHEEL_HOOK.call_once(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(POINT, i16)>();
        let _ = WHEEL_TX.set(tx);
        std::thread::spawn(move || {
            for (point, delta) in rx {
                send_mouse_wheel(point, delta);
            }
        });
        std::thread::spawn(|| unsafe {
            let h_instance = match GetModuleHandleW(None) {
                Ok(h) => HINSTANCE(h.0),
                Err(_) => HINSTANCE::default(),
            };
            match WindowsAndMessaging::SetWindowsHookExW(WindowsAndMessaging::WH_MOUSE_LL, Some(wheel_hook_proc), h_instance, 0) {
                Ok(_) => crate::log_line("input: WH_MOUSE_LL wheel hook installed"),
                Err(err) => {
                    crate::log_line(format!("input: WH_MOUSE_LL wheel hook failed: {err}"));
                    return;
                }
            }
            let mut msg = WindowsAndMessaging::MSG::default();
            while WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0).into() {
                let _ = WindowsAndMessaging::TranslateMessage(&msg);
                WindowsAndMessaging::DispatchMessageW(&msg);
            }
        });
    });
}

pub fn start_input_forwarding(hwnd: HWND, forward_mouse_input: bool, forward_keyboard_input: bool) {
    // If we are not forwarding mouse or keyboard input, return
    if !forward_mouse_input && !forward_keyboard_input {
        return;
    }

    if forward_mouse_input {
        start_wheel_hook();
    }

    let hwnd_raw = hwnd.0 as isize;
    let windows = WINDOWS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut windows_lock) = windows.lock() {
        // attach() can race (fallback + frontend + single-instance). Never keep
        // duplicate targets — each extra entry multiplies every key/mouse post.
        windows_lock.retain(|window| window.window_handle != hwnd_raw);
        windows_lock.push(WindowContext {
            window_handle: hwnd_raw,
            forward_mouse_input,
            forward_keyboard_input,
            last_key_target: hwnd_raw,
        });
    }

    if RAW_INPUT_WINDOW.load(Ordering::SeqCst) != 0 {
        return;
    }

    unsafe {
        // Create a window to receive raw input messages
        let h_instance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return,
        };

        // Create a window class for the raw input window
        let wnd_class = WindowsAndMessaging::WNDCLASSA {
            lpfnWndProc: Some(handle_window_message),
            hInstance: HINSTANCE::from(h_instance),
            lpszClassName: s!("RawInputWindowClass"),
            ..WindowsAndMessaging::WNDCLASSA::default()
        };

        // Register the window class
        WindowsAndMessaging::RegisterClassA(&wnd_class);

        // Create the raw input window
        let raw_input_window = match WindowsAndMessaging::CreateWindowExA(
            WindowsAndMessaging::WINDOW_EX_STYLE::default(),
            wnd_class.lpszClassName,
            s!("RawInputWindow"),
            WindowsAndMessaging::WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            h_instance,
            None,
        ) {
            Ok(w) => w,
            Err(_) => return,
        };

        RAW_INPUT_WINDOW.store(raw_input_window.0 as isize, Ordering::SeqCst);

        // Input devices to forward: mouse only (keyboard & hotkeys handled by hotkeys.rs)
        let inputs: [Input::RAWINPUTDEVICE; 1] = [
            Input::RAWINPUTDEVICE {
                hwndTarget: raw_input_window,
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_MOUSE,
                dwFlags: Input::RIDEV_INPUTSINK,
            },
        ];

        // Register the input devices
        let _ = Input::RegisterRawInputDevices(&inputs, size_of::<Input::RAWINPUTDEVICE>() as u32);
    }
}

pub fn stop_input_forwarding(hwnd: HWND) {
    let hwnd_raw = hwnd.0 as isize;
    if let Some(windows) = WINDOWS.get() {
        if let Ok(mut windows_lock) = windows.lock() {
            windows_lock.retain(|window| window.window_handle != hwnd_raw);

            if windows_lock.is_empty() {
                let prev = RAW_INPUT_WINDOW.swap(0, Ordering::SeqCst);
                if prev != 0 {
                    unsafe {
                        let _ = WindowsAndMessaging::DestroyWindow(HWND(prev as _));
                    }
                }
            }
        }
    }
}
