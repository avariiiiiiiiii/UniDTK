//! Windows session-end listener.
//!
//! The wallpaper window is reparented under WorkerW as a `WS_CHILD`, so
//! Windows never delivers `WM_QUERYENDSESSION`/`WM_ENDSESSION` to it (those
//! broadcasts go to top-level windows only). Without a handler the app dies
//! with the session while `sing-box` keeps its TUN up — and a surviving TUN
//! with `strict_route` black-holes the user's network on the next boot.
//!
//! This module creates a hidden top-level window (**not** a message-only
//! window: those never receive broadcasts either) on a dedicated thread with
//! its own message loop, and runs the normal app-shutdown path — including a
//! bounded wait for the elevated stop helper — inside `WM_ENDSESSION`, under a
//! `ShutdownBlockReasonCreate` block reason.
//!
//! It also keeps that message away from tao's event loop (`guard_event_loop`),
//! which would otherwise stop dispatching under the running shutdown.

use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumThreadWindows, GetClassNameW,
    GetMessageW, RegisterClassW, TranslateMessage, HMENU, MSG, WINDOW_EX_STYLE, WM_ENDSESSION,
    WM_QUERYENDSESSION, WNDCLASSW, WS_OVERLAPPED,
};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Start the listener thread. Call once from `setup`.
pub fn install(app: tauri::AppHandle) {
    let _ = APP.set(app);
    std::thread::Builder::new()
        .name("unidtk-session-end".into())
        .spawn(|| unsafe { run_message_loop() })
        .expect("spawn session-end listener");
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Broadcast resume reaches this hidden top-level host, unlike WorkerW children.
        0x0218 if matches!(wparam.0, 0x0007 | 0x0012) => {
            unidtk_api::emit("system-resumed", serde_json::json!({}));
            LRESULT(1)
        }
        WM_QUERYENDSESSION => {
            // Answer immediately; the real work happens in WM_ENDSESSION after
            // Windows has committed to the shutdown, so nothing here delays the
            // OS's own decision.
            crate::log_line("shutdown: WM_QUERYENDSESSION");
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                crate::log_line("shutdown: WM_ENDSESSION received");
                let reason: PCWSTR = w!("Stopping background VPN…");
                let _ = ShutdownBlockReasonCreate(hwnd, reason);
                if let Some(app) = APP.get() {
                    // Bounded wait for the elevated stop helper; the OS gives
                    // us a few seconds thanks to the block reason.
                    crate::app_shutdown(app, true);
                }
                let _ = ShutdownBlockReasonDestroy(hwnd);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn run_message_loop() {
    let instance = match GetModuleHandleW(None) {
        Ok(module) => HINSTANCE(module.0),
        Err(_) => return,
    };
    let class_name: PCWSTR = w!("Shell_SessionEnd");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: class_name,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        crate::log_line("shutdown: RegisterClassW failed");
        return;
    }
    let _hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class_name,
        w!("Shell session-end listener"),
        WS_OVERLAPPED, // hidden: no WS_VISIBLE (and no parent, unlike the wallpaper window)
        0,
        0,
        0,
        0,
        HWND::default(),
        HMENU::default(),
        instance,
        None,
    ) {
        Ok(hwnd) => hwnd,
        Err(error) => {
            crate::log_line(format!("shutdown: CreateWindowExW failed: {error}"));
            return;
        }
    };
    crate::log_line("shutdown: session-end listener ready");
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

/// Keep a committed session end away from tao's event loop. Call once from
/// `setup`, on the main thread.
///
/// tao's hidden "Tao Thread Event Target" window is top-level, so it receives
/// `WM_ENDSESSION` too, and on `wParam = TRUE` it moves its runner to
/// `Destroyed`. The process lives on until every window has answered — this
/// listener is still stopping the VPN — and the next event the main thread
/// dispatches panics in tao (`runner.rs`: "cannot move state from Destroyed"),
/// which killed the process before `shutdown: done`. Nothing in tao needs the
/// message: the shutdown is this module's, so the main thread keeps running
/// until Windows ends the process.
pub fn guard_event_loop() {
    let guarded = guard_thread_windows(w!("Tao Thread Event Target"));
    crate::log_line(format!("shutdown: session end kept from the event loop ({guarded} window)"));
}

/// Subclasses every top-level window of the calling thread with the class
/// `class`, so that a committed `WM_ENDSESSION` never reaches its own procedure.
fn guard_thread_windows(class: PCWSTR) -> usize {
    struct Search {
        class: Vec<u16>,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        let mut name = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut name) as usize;
        if name[..len] == search.class[..] {
            search.found.push(hwnd);
        }
        BOOL(1)
    }
    let mut search = Search { class: unsafe { class.as_wide() }.to_vec(), found: Vec::new() };
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search
        .found
        .into_iter()
        .filter(|&hwnd| unsafe { SetWindowSubclass(hwnd, Some(keep_session_end), SESSION_END_SUBCLASS, 0) }.as_bool())
        .count()
}

/// Any id unique among this window's subclasses; tao uses its own.
const SESSION_END_SUBCLASS: usize = 0x5544_544B;

unsafe extern "system" fn keep_session_end(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_ENDSESSION && wparam.0 != 0 {
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, SendMessageW};

    /// Session ends that reached the stand-in for tao's event target window.
    static SEEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn event_target(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_ENDSESSION {
            SEEN.fetch_add(1, Ordering::SeqCst);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    #[test]
    fn the_event_loop_never_sees_a_committed_session_end() {
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            let class_name = w!("UniDTK Test Event Target");
            let wc = WNDCLASSW { lpfnWndProc: Some(event_target), hInstance: instance, lpszClassName: class_name, ..Default::default() };
            assert_ne!(RegisterClassW(&wc), 0);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class_name, None, WS_OVERLAPPED, 0, 0, 0, 0, HWND::default(), HMENU::default(), instance, None).unwrap();

            assert_eq!(guard_thread_windows(class_name), 1);
            // ENDSESSION_LOGOFF in lParam, as Windows sends it on sign-out.
            SendMessageW(hwnd, WM_ENDSESSION, WPARAM(1), LPARAM(0x8000_0000));
            assert_eq!(SEEN.load(Ordering::SeqCst), 0, "a committed session end reached the event loop");
            // A cancelled session end changes nothing in tao and passes through.
            SendMessageW(hwnd, WM_ENDSESSION, WPARAM(0), LPARAM(0));
            assert_eq!(SEEN.load(Ordering::SeqCst), 1);

            DestroyWindow(hwnd).unwrap();
        }
    }
}
