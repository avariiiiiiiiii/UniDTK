use std::sync::{Mutex, atomic::AtomicBool};

use windows::core::w;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::WindowsAndMessaging;

use crate::input::{start_input_forwarding, stop_input_forwarding};

#[derive(Clone, Copy)]
pub struct AttachOptions {
    pub transparent: bool,
    pub forward_mouse: bool,
    pub forward_keyboard: bool,
}

/// Physical-pixel bounds of the primary monitor (the one whose top-left is at
/// the desktop origin). Under PerMonitorV2 DPI awareness this is in the same
/// physical coordinate space as `GetWindowRect(WorkerW)`, so the two stay
/// consistent — unlike `SM_C*SCREEN`, which is virtualized to the thread's DPI
/// context and leaves a gap at the screen edge on mixed-DPI multi-monitor setups.
fn primary_monitor_rect() -> Option<RECT> {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            Some(mi.rcMonitor)
        } else {
            None
        }
    }
}

extern "system" fn enum_window(window: HWND, ref_worker_w: LPARAM) -> BOOL {
    unsafe {
        let mut class_name = [0u16; 256];
        let len = WindowsAndMessaging::GetClassNameW(window, &mut class_name);
        if len == 0 {
            return BOOL(1);
        }
        let class_string = String::from_utf16_lossy(&class_name[..len as usize]);

        let shell_dll_def_view = WindowsAndMessaging::FindWindowExW(
            window,
            HWND::default(),
            w!("SHELLDLL_DefView"),
            None,
        )
        .unwrap_or_default();

        if !HWND::is_invalid(&shell_dll_def_view) {
            let mut w_rect = RECT::default();
            let _ = WindowsAndMessaging::GetWindowRect(window, &mut w_rect);
            crate::log_line(format!(
                "enum_window: found SHELLDLL_DefView in hwnd=0x{:X} cls={} rect=({},{},{},{})",
                window.0 as usize, class_string, w_rect.left, w_rect.top, w_rect.right, w_rect.bottom
            ));

            let worker_w = WindowsAndMessaging::FindWindowExW(
                HWND::default(),
                window,
                w!("WorkerW"),
                None,
            )
            .unwrap_or_default();

            if !HWND::is_invalid(&worker_w) {
                let mut rect = RECT::default();
                let _ = WindowsAndMessaging::GetWindowRect(worker_w, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                crate::log_line(format!(
                    "enum_window: sibling WorkerW hwnd=0x{:X} rect=({},{},{},{}) size={}x{}",
                    worker_w.0 as usize, rect.left, rect.top, rect.right, rect.bottom, width, height
                ));
                if width >= 640 && height >= 480 {
                    *(ref_worker_w.0 as *mut HWND) = worker_w;
                    return BOOL(0);
                }
            } else {
                crate::log_line(format!("enum_window: no sibling WorkerW after hwnd=0x{:X}", window.0 as usize));
            }
        }

        BOOL(1)
    }
}

fn toggle_transparent(hwnd: HWND, on: bool) {
    unsafe {
        let styles = WindowsAndMessaging::GetWindowLongA(hwnd, WindowsAndMessaging::GWL_EXSTYLE);
        if on {
            WindowsAndMessaging::SetWindowLongA(
                hwnd,
                WindowsAndMessaging::GWL_EXSTYLE,
                styles | WindowsAndMessaging::WS_EX_LAYERED.0 as i32,
            );
            let _ = WindowsAndMessaging::SetLayeredWindowAttributes(
                hwnd,
                COLORREF::default(),
                255,
                WindowsAndMessaging::LWA_ALPHA,
            );
        } else {
            WindowsAndMessaging::SetWindowLongA(
                hwnd,
                WindowsAndMessaging::GWL_EXSTYLE,
                styles & !WindowsAndMessaging::WS_EX_LAYERED.0 as i32,
            );
        }
    }
}

/// Restyle as a child of WorkerW.
fn make_child_window(hwnd: HWND) {
    unsafe {
        let style = WindowsAndMessaging::GetWindowLongA(hwnd, WindowsAndMessaging::GWL_STYLE);
        let next_style = (style
            & !(WindowsAndMessaging::WS_POPUP.0 as i32
                | WindowsAndMessaging::WS_CAPTION.0 as i32
                | WindowsAndMessaging::WS_THICKFRAME.0 as i32
                | WindowsAndMessaging::WS_BORDER.0 as i32
                | WindowsAndMessaging::WS_DLGFRAME.0 as i32
                | WindowsAndMessaging::WS_SYSMENU.0 as i32))
            | WindowsAndMessaging::WS_CHILD.0 as i32
            | WindowsAndMessaging::WS_VISIBLE.0 as i32;

        let ex_style = WindowsAndMessaging::GetWindowLongA(hwnd, WindowsAndMessaging::GWL_EXSTYLE);
        let next_ex_style = ex_style
            & !(WindowsAndMessaging::WS_EX_WINDOWEDGE.0 as i32
                | WindowsAndMessaging::WS_EX_CLIENTEDGE.0 as i32
                | WindowsAndMessaging::WS_EX_STATICEDGE.0 as i32
                | WindowsAndMessaging::WS_EX_DLGMODALFRAME.0 as i32);

        WindowsAndMessaging::SetWindowLongA(hwnd, WindowsAndMessaging::GWL_STYLE, next_style);
        WindowsAndMessaging::SetWindowLongA(hwnd, WindowsAndMessaging::GWL_EXSTYLE, next_ex_style);

        let _ = WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            WindowsAndMessaging::SWP_FRAMECHANGED
                | WindowsAndMessaging::SWP_NOACTIVATE
                | WindowsAndMessaging::SWP_NOMOVE
                | WindowsAndMessaging::SWP_NOSIZE
                | WindowsAndMessaging::SWP_NOZORDER,
        );
    }
}

/// Resolve on a worker thread; the UI only validates and reparents.
pub fn resolve_desktop_host(allow_progman_fallback: bool) -> Result<(isize, u32), String> {
    let host = find_worker_w_internal(true, allow_progman_fallback)?;
    let mut pid = 0;
    unsafe { WindowsAndMessaging::GetWindowThreadProcessId(host, Some(&mut pid)); }
    Ok((host.0 as isize, pid))
}

fn find_worker_w_internal(verbose: bool, allow_progman_fallback: bool) -> Result<HWND, String> {
    unsafe {
        let mut progman = WindowsAndMessaging::FindWindowW(w!("Progman"), None).unwrap_or_default();
        if HWND::is_invalid(&progman) || progman.0.is_null() {
            progman = WindowsAndMessaging::GetShellWindow();
        }
        if HWND::is_invalid(&progman) || progman.0.is_null() {
            progman = WindowsAndMessaging::FindWindowW(w!("Progman"), w!("Program Manager"))
                .unwrap_or_default();
        }

        if !HWND::is_invalid(&progman) && !progman.0.is_null() {
            let mut res = 0usize;
            // Windows 10/11: 0x052C with (0xD, 1) or (0xD, 0) splits desktop into WorkerW.
            // SMTO_ABORTIFHUNG ensures we do not block if Explorer is unresponsive during logon.
            let flags = WindowsAndMessaging::SMTO_NORMAL | WindowsAndMessaging::SMTO_ABORTIFHUNG;
            let _ = WindowsAndMessaging::SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0x0000000D),
                LPARAM(0x00000001),
                flags,
                100,
                Some(&mut res),
            );
            let _ = WindowsAndMessaging::SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0x0000000D),
                LPARAM(0x00000000),
                flags,
                100,
                Some(&mut res),
            );
            let _ = WindowsAndMessaging::SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0),
                LPARAM(0),
                flags,
                100,
                Some(&mut res),
            );
        } else if !verbose {
            return Err("progman not found yet".into());
        }

        if verbose {
            crate::log_line(format!("find_worker_w: progman=0x{:X}", progman.0 as usize));
        }

        let mut worker_w = HWND::default();
        let _ = WindowsAndMessaging::EnumWindows(
            Some(enum_window),
            LPARAM(&mut worker_w as *mut HWND as isize),
        );

        if !HWND::is_invalid(&worker_w) && !worker_w.0.is_null() {
            if verbose {
                crate::log_line(format!("find_worker_w: matched sibling WorkerW=0x{:X}", worker_w.0 as usize));
            }
            return Ok(worker_w);
        }

        // Fallback 1: check any top-level WorkerW that does NOT contain SHELLDLL_DefView
        let mut top_worker_w = WindowsAndMessaging::FindWindowExW(
            HWND::default(),
            HWND::default(),
            w!("WorkerW"),
            None,
        )
        .unwrap_or_default();

        while !HWND::is_invalid(&top_worker_w) && !top_worker_w.0.is_null() {
            let def_view = WindowsAndMessaging::FindWindowExW(
                top_worker_w,
                HWND::default(),
                w!("SHELLDLL_DefView"),
                None,
            )
            .unwrap_or_default();

            if HWND::is_invalid(&def_view) || def_view.0.is_null() {
                let mut rect = RECT::default();
                let _ = WindowsAndMessaging::GetWindowRect(top_worker_w, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width >= 640 && height >= 480 {
                    if verbose {
                        crate::log_line(format!(
                            "find_worker_w: top-level WorkerW=0x{:X} size={}x{}",
                            top_worker_w.0 as usize, width, height
                        ));
                    }
                    return Ok(top_worker_w);
                }
            }

            top_worker_w = WindowsAndMessaging::FindWindowExW(
                HWND::default(),
                top_worker_w,
                w!("WorkerW"),
                None,
            )
            .unwrap_or_default();
        }

        // Fallback 2: If Progman contains SHELLDLL_DefView and no WorkerW was created,
        // Progman itself is the desktop host
        if allow_progman_fallback && !HWND::is_invalid(&progman) && !progman.0.is_null() {
            let def_view = WindowsAndMessaging::FindWindowExW(
                progman,
                HWND::default(),
                w!("SHELLDLL_DefView"),
                None,
            )
            .unwrap_or_default();
            if !HWND::is_invalid(&def_view) && !def_view.0.is_null() {
                if verbose {
                    crate::log_line(format!("find_worker_w: using Progman=0x{:X} as fallback desktop host", progman.0 as usize));
                }
                return Ok(progman);
            }
        }

        if verbose {
            crate::log_line("find_worker_w: desktop host WorkerW not found");
        }
        Err("desktop host not ready yet".into())
    }
}

pub fn attach_to_host(hwnd_raw: isize, opts: AttachOptions, prepared: (isize, u32)) -> Result<(), String> {
    let hwnd = HWND(hwnd_raw as _);
    unsafe {
        let host_w = HWND(prepared.0 as _);
        let mut pid = 0;
        WindowsAndMessaging::GetWindowThreadProcessId(host_w, Some(&mut pid));
        if !WindowsAndMessaging::IsWindow(host_w).as_bool() || pid != prepared.1 {
            return Err("desktop host changed during preparation".into());
        }

        // Snapshot the original wallpaper the first time we attach so we can
        // restore it on exit. Doing it here (inside the WorkerW reparent)
        // guarantees explorer is already fully up.
        save_original_wallpaper();

        let mut host_class_buf = [0u16; 256];
        let host_class_len = WindowsAndMessaging::GetClassNameW(host_w, &mut host_class_buf);
        let host_class = String::from_utf16_lossy(&host_class_buf[..host_class_len.max(0) as usize]);
        if host_class != "WorkerW" && host_class != "Progman" {
            return Err("desktop host class changed during preparation".into());
        }
        let is_progman_fallback = host_class == "Progman";

        let mut ww_rect = RECT::default();
        WindowsAndMessaging::GetWindowRect(host_w, &mut ww_rect)
            .map_err(|e| format!("desktop host geometry unavailable: {e}"))?;
        if ww_rect.right - ww_rect.left < 640 || ww_rect.bottom - ww_rect.top < 480 {
            return Err("desktop host geometry not ready".into());
        }
        crate::log_line(format!(
            "attach: desktop host class={} HWND=0x{:X} rect=({},{},{},{})",
            host_class, host_w.0 as usize, ww_rect.left, ww_rect.top, ww_rect.right, ww_rect.bottom,
        ));

        // Reparent under the selected desktop host. Callers keep the window
        // hidden until this attachment and its final placement both succeed.
        WindowsAndMessaging::SetParent(hwnd, host_w).map_err(|e| e.to_string())?;
        make_child_window(hwnd);

        if opts.transparent {
            toggle_transparent(hwnd, true);
        }

        // Position child at primary monitor's location within WorkerW.
        // WorkerW origin is at (ww_rect.left, ww_rect.top); child coords are
        // relative to it. Use the primary monitor's real physical bounds rather
        // than SM_C*SCREEN so size/offset stay in the same coordinate space as
        // the WorkerW rect and no uncovered strip is left at the screen edges.
        let primary = primary_monitor_rect().unwrap_or(RECT {
            left: 0,
            top: 0,
            right: WindowsAndMessaging::GetSystemMetrics(WindowsAndMessaging::SM_CXSCREEN),
            bottom: WindowsAndMessaging::GetSystemMetrics(WindowsAndMessaging::SM_CYSCREEN),
        });
        let child_x = primary.left - ww_rect.left;
        let child_y = primary.top - ww_rect.top;
        let child_w = primary.right - primary.left;
        let child_h = primary.bottom - primary.top;
        let _ = WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            child_x,
            child_y,
            child_w,
            child_h,
            WindowsAndMessaging::SWP_NOZORDER | WindowsAndMessaging::SWP_NOACTIVATE,
        );

        // Force WRY/WebView2 to re-fit its DirectComposition surface to the full
        // client rect after the reparent + restyle + layered-attribute changes.
        // Without a post-attach resize the composition surface can stay offset by
        // the old window frame, intermittently leaving an opaque strip along the
        // top edge. A 1px shrink-then-restore guarantees two WM_SIZE events whose
        // final state is the correct size, so the surface always covers (0,0).
        let _ = WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            child_x,
            child_y,
            child_w,
            child_h - 1,
            WindowsAndMessaging::SWP_NOZORDER | WindowsAndMessaging::SWP_NOACTIVATE,
        );
        let position_flags = if is_progman_fallback {
            // Progman also owns Explorer's SHELLDLL_DefView. HWND_BOTTOM puts
            // this child behind the desktop host's own painted surface, hiding
            // the application. Keep the transparent surface above Progman so
            // its widgets stay visible; the normal WorkerW path keeps its order.
            WindowsAndMessaging::SWP_NOACTIVATE | WindowsAndMessaging::SWP_SHOWWINDOW
        } else {
            WindowsAndMessaging::SWP_NOZORDER
                | WindowsAndMessaging::SWP_NOACTIVATE
                | WindowsAndMessaging::SWP_SHOWWINDOW
        };
        WindowsAndMessaging::SetWindowPos(
            hwnd,
            WindowsAndMessaging::HWND_TOP,
            child_x,
            child_y,
            child_w,
            child_h,
            position_flags,
        )
        .map_err(|e| format!("position desktop child: {e}"))?;
        let _ = WindowsAndMessaging::ShowWindow(hwnd, WindowsAndMessaging::SW_SHOW);

        let mut final_rect = RECT::default();
        let _ = WindowsAndMessaging::GetWindowRect(hwnd, &mut final_rect);
        crate::log_line(format!(
            "attach: placed child at ({},{}) {}x{}, actual=({},{},{},{})",
            child_x,
            child_y,
            child_w,
            child_h,
            final_rect.left,
            final_rect.top,
            final_rect.right,
            final_rect.bottom,
        ));
    }

    stop_input_forwarding(hwnd);
    start_input_forwarding(hwnd, opts.forward_mouse, opts.forward_keyboard);
    Ok(())
}

pub fn detach(hwnd_raw: isize, opts: AttachOptions) -> Result<(), String> {
    let hwnd = HWND(hwnd_raw as _);
    unsafe {
        WindowsAndMessaging::SetParent(hwnd, None).map_err(|e| e.to_string())?;
        if opts.transparent {
            toggle_transparent(hwnd, false);
        }
    }
    stop_input_forwarding(hwnd);
    Ok(())
}

/// Cached original wallpaper path restored on shutdown. Kept in a static
/// so `reset()` can restore (not clear) the wallpaper when the app exits.
static ORIGINAL_WALLPAPER: Mutex<Option<String>> = Mutex::new(None);
static ORIGINAL_WALLPAPER_SAVED: AtomicBool = AtomicBool::new(false);

/// Snapshot the current wallpaper into `ORIGINAL_WALLPAPER` the first time we
/// touch the desktop. Safe to call repeatedly — only the first call stores the
/// path. Returns the cached path.
pub fn save_original_wallpaper() -> Option<String> {
    use std::sync::atomic::Ordering;

    if ORIGINAL_WALLPAPER_SAVED.load(Ordering::SeqCst) {
        return ORIGINAL_WALLPAPER.lock().unwrap().clone();
    }

    let path = get_current_wallpaper_path();
    if !path.is_empty() {
        *ORIGINAL_WALLPAPER.lock().unwrap() = Some(path.clone());
    }
    ORIGINAL_WALLPAPER_SAVED.store(true, Ordering::SeqCst);
    ORIGINAL_WALLPAPER.lock().unwrap().clone()
}

/// Restore (or clear) the desktop wallpaper. On app exit we restore the
/// original image the user had *before* UniDTK started. We deliberately skip `SPIF_SENDCHANGE` when the
/// original path is empty or already the current wallpaper — that message is
/// what forces explorer to repaint the desktop and briefly hide the icons.
pub fn reset() -> Result<(), String> {
    let original = ORIGINAL_WALLPAPER.lock().unwrap().clone();

    // If we have a saved original and it is a real file, restore it.
    // Otherwise just leave whatever is there (do NOT call SPI_SETDESKWALLPAPER
    // with a null path — that is what clears the wallpaper and forces a
    // full desktop repaint that blanks the icons).
    if let Some(path) = original {
        if !path.is_empty() {
            return set_wallpaper(&path);
        }
    }

    Ok(())
}

/// Set the desktop wallpaper to `path` without broadcasting WM_SETTINGCHANGE
/// unless the path actually differs from the current wallpaper. The broadcast
/// is what triggers explorer to re-paint the desktop tree and briefly hide
/// the icons, so avoiding it when nothing changed keeps the icons visible.
fn set_wallpaper(path: &str) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;

    let current = get_current_wallpaper_path();

    if current.eq_ignore_ascii_case(path) {
        // Already the current wallpaper — nothing to do, no broadcast sent.
        return Ok(());
    }

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as *mut _),
            windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Read the current wallpaper path via SPI_GETDESKWALLPAPER. Returns an
/// empty string on failure / no wallpaper set.
fn get_current_wallpaper_path() -> String {
    use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    let mut buf = vec![0u16; 512];
    unsafe {
        let _ = SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETDESKWALLPAPER,
            (buf.len() * std::mem::size_of::<u16>()) as u32,
            Some(buf.as_mut_ptr() as *mut _),
            windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE,
        );
    }
    let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..nul])
}

pub fn update_geometry(hwnd_raw: isize) -> Result<(), String> {
    let hwnd = HWND(hwnd_raw as _);
    unsafe {
        // Geometry follows the host we are already attached to. Searching here
        // would issue blocking Explorer messages during every resize.
        let host_w = WindowsAndMessaging::GetParent(hwnd)
            .map_err(|e| format!("desktop parent unavailable: {e}"))?;
        if !WindowsAndMessaging::IsWindow(host_w).as_bool() {
            return Err("desktop parent no longer exists".into());
        }

        let mut ww_rect = RECT::default();
        let _ = WindowsAndMessaging::GetWindowRect(host_w, &mut ww_rect);

        let primary = primary_monitor_rect().unwrap_or(RECT {
            left: 0,
            top: 0,
            right: WindowsAndMessaging::GetSystemMetrics(WindowsAndMessaging::SM_CXSCREEN),
            bottom: WindowsAndMessaging::GetSystemMetrics(WindowsAndMessaging::SM_CYSCREEN),
        });
        let child_x = primary.left - ww_rect.left;
        let child_y = primary.top - ww_rect.top;
        let child_w = primary.right - primary.left;
        let child_h = primary.bottom - primary.top;

        crate::log_line(format!(
            "display update: child=({},{}) {}x{}, worker=({},{},{},{}), primary=({},{},{},{})",
            child_x, child_y, child_w, child_h,
            ww_rect.left, ww_rect.top, ww_rect.right, ww_rect.bottom,
            primary.left, primary.top, primary.right, primary.bottom
        ));

        let _ = WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            child_x,
            child_y,
            child_w,
            child_h,
            WindowsAndMessaging::SWP_NOZORDER | WindowsAndMessaging::SWP_NOACTIVATE,
        );

        let _ = WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            child_x,
            child_y,
            child_w,
            child_h - 1,
            WindowsAndMessaging::SWP_NOZORDER | WindowsAndMessaging::SWP_NOACTIVATE,
        );
        let _ = WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            child_x,
            child_y,
            child_w,
            child_h,
            WindowsAndMessaging::SWP_NOZORDER | WindowsAndMessaging::SWP_NOACTIVATE,
        );
    }
    Ok(())
}

