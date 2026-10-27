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

use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, HMENU, MSG, WINDOW_EX_STYLE, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
    WS_OVERLAPPED,
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
