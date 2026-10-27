use parking_lot::RwLock;
use std::sync::OnceLock;
use windows::core::w;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    self, FindWindowExW, FindWindowW, GetClassNameW, GetShellWindow, GetWindowRect,
    SendMessageTimeoutW, SetParent, SetWindowLongA, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
    HTCLIENT, HTTRANSPARENT, SMTO_ABORTIFHUNG, SMTO_NORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_NCHITTEST, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_DLGFRAME, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE, WS_EX_WINDOWEDGE,
    WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE,
};

#[link(name = "user32")]
extern "system" {
    fn GetCapture() -> HWND;
}

static HIT_TEST_REGIONS: OnceLock<RwLock<Vec<RECT>>> = OnceLock::new();

pub fn publish_interactive_regions(regions: Vec<RECT>) {
    let storage = HIT_TEST_REGIONS.get_or_init(|| RwLock::new(Vec::new()));
    *storage.write() = regions;
}

pub fn to_physical_rect(bounds: &gpui::Bounds<gpui::Pixels>, scale_factor: f32) -> RECT {
    let x = bounds.origin.x.to_f64() as f32;
    let y = bounds.origin.y.to_f64() as f32;
    let w = bounds.size.width.to_f64() as f32;
    let h = bounds.size.height.to_f64() as f32;

    RECT {
        left: (x * scale_factor).round() as i32,
        top: (y * scale_factor).round() as i32,
        right: ((x + w) * scale_factor).round() as i32,
        bottom: ((y + h) * scale_factor).round() as i32,
    }
}

pub fn primary_monitor_logical_size() -> (f32, f32) {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            let width = (mi.rcMonitor.right - mi.rcMonitor.left) as f32;
            let height = (mi.rcMonitor.bottom - mi.rcMonitor.top) as f32;
            use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() {
                let scale = dpi_x as f32 / 96.0;
                (width / scale, height / scale)
            } else {
                (width, height)
            }
        } else {
            (1280.0, 720.0)
        }
    }
}

pub fn primary_monitor_rect() -> Option<RECT> {
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

struct DesktopEnumState {
    worker_w: HWND,
    def_view_host: HWND,
    count: usize,
}

extern "system" fn enum_window_for_desktop(window: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut DesktopEnumState);
        let mut class_name = [0u16; 256];
        let len = GetClassNameW(window, &mut class_name);
        let class_str = if len > 0 {
            String::from_utf16_lossy(&class_name[..len as usize])
        } else {
            String::new()
        };
        eprintln!("[enum #{}] hwnd=0x{:X} cls='{}'", state.count, window.0 as usize, class_str);
        state.count += 1;

        let shell_dll_def_view = FindWindowExW(
            window,
            HWND::default(),
            w!("SHELLDLL_DefView"),
            None,
        )
        .unwrap_or_default();
        if !HWND::is_invalid(&shell_dll_def_view) && !shell_dll_def_view.0.is_null() {
            eprintln!("[enum] -> found SHELLDLL_DefView=0x{:X} inside 0x{:X}", shell_dll_def_view.0 as usize, window.0 as usize);
        }

        if !HWND::is_invalid(&shell_dll_def_view) && !shell_dll_def_view.0.is_null() {
            state.def_view_host = window;

            let sibling_worker_w = FindWindowExW(
                HWND::default(),
                window,
                w!("WorkerW"),
                None,
            )
            .unwrap_or_default();

            if !HWND::is_invalid(&sibling_worker_w) && !sibling_worker_w.0.is_null() {
                let mut rect = RECT::default();
                let _ = GetWindowRect(sibling_worker_w, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width >= 640 && height >= 480 {
                    state.worker_w = sibling_worker_w;
                    return BOOL(0); // Stop enumeration, WorkerW found
                }
            }
        }

        BOOL(1)
    }
}

pub fn attach_thread_to_input_desktop() {
    unsafe {
        use windows::Win32::System::StationsAndDesktops::{
            OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
        };
        if let Ok(input_desk) = OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(0x01FF),
        ) {
            let res = SetThreadDesktop(input_desk);
            eprintln!("[wallpaper] attach_thread_to_input_desktop: {:?}", res);
        }
    }
}

pub fn find_desktop_host() -> Result<(HWND, &'static str), String> {
    unsafe {
        use windows::Win32::System::StationsAndDesktops::{
            EnumDesktopWindows, OpenInputDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS,
        };

        let input_desk = OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(0x01FF),
        ).ok();

        let mut progman = FindWindowW(w!("Progman"), None).unwrap_or_default();
        if HWND::is_invalid(&progman) || progman.0.is_null() {
            progman = GetShellWindow();
        }
        if HWND::is_invalid(&progman) || progman.0.is_null() {
            progman = FindWindowW(w!("Progman"), w!("Program Manager")).unwrap_or_default();
        }

        if !HWND::is_invalid(&progman) && !progman.0.is_null() {
            let mut res = 0usize;
            let flags = SMTO_NORMAL | SMTO_ABORTIFHUNG;
            let _ = SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0x0000000D),
                LPARAM(0x00000001),
                flags,
                100,
                Some(&mut res),
            );
            let _ = SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0x0000000D),
                LPARAM(0x00000000),
                flags,
                100,
                Some(&mut res),
            );
            let _ = SendMessageTimeoutW(
                progman,
                0x052C,
                WPARAM(0),
                LPARAM(0),
                flags,
                100,
                Some(&mut res),
            );
        }

        let mut state = DesktopEnumState {
            worker_w: HWND::default(),
            def_view_host: HWND::default(),
            count: 0,
        };

        if let Some(desk) = input_desk {
            eprintln!("[find_desktop_host] enumerating input desktop windows...");
            let _ = EnumDesktopWindows(
                desk,
                Some(enum_window_for_desktop),
                LPARAM(&mut state as *mut DesktopEnumState as isize),
            );
        } else {
            eprintln!("[find_desktop_host] calling EnumWindows fallback...");
            let _ = WindowsAndMessaging::EnumWindows(
                Some(enum_window_for_desktop),
                LPARAM(&mut state as *mut DesktopEnumState as isize),
            );
        }

        if !HWND::is_invalid(&state.worker_w) && !state.worker_w.0.is_null() {
            return Ok((state.worker_w, "Sibling WorkerW"));
        }

        // Fallback 1: check top-level WorkerW without SHELLDLL_DefView
        let mut top_worker_w = FindWindowExW(
            HWND::default(),
            HWND::default(),
            w!("WorkerW"),
            None,
        )
        .unwrap_or_default();

        while !HWND::is_invalid(&top_worker_w) && !top_worker_w.0.is_null() {
            let shell_dll = FindWindowExW(
                top_worker_w,
                HWND::default(),
                w!("SHELLDLL_DefView"),
                None,
            )
            .unwrap_or_default();

            if HWND::is_invalid(&shell_dll) || shell_dll.0.is_null() {
                let mut rect = RECT::default();
                let _ = GetWindowRect(top_worker_w, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;
                if width >= 640 && height >= 480 {
                    return Ok((top_worker_w, "Top-level WorkerW"));
                }
            }

            top_worker_w = FindWindowExW(
                HWND::default(),
                top_worker_w,
                w!("WorkerW"),
                None,
            )
            .unwrap_or_default();
        }

        // Fallback 2: Progman containing SHELLDLL_DefView
        if !HWND::is_invalid(&state.def_view_host) && !state.def_view_host.0.is_null() {
            return Ok((state.def_view_host, "Progman Desktop Host"));
        }

        Err("No suitable desktop host found".into())
    }
}

pub fn attach_to_desktop(hwnd: HWND, host: HWND) -> Result<(), String> {
    unsafe {
        SetParent(hwnd, host).map_err(|e| format!("SetParent failed: {e}"))?;

        let style = WindowsAndMessaging::GetWindowLongA(hwnd, GWL_STYLE);
        let next_style = (style
            & !(WS_POPUP.0 as i32
                | WS_CAPTION.0 as i32
                | WS_THICKFRAME.0 as i32
                | WS_BORDER.0 as i32
                | WS_DLGFRAME.0 as i32
                | WS_SYSMENU.0 as i32))
            | WS_CHILD.0 as i32
            | WS_VISIBLE.0 as i32;

        let ex_style = WindowsAndMessaging::GetWindowLongA(hwnd, GWL_EXSTYLE);
        let next_ex_style = ex_style
            & !(WS_EX_WINDOWEDGE.0 as i32
                | WS_EX_CLIENTEDGE.0 as i32
                | WS_EX_STATICEDGE.0 as i32
                | WS_EX_DLGMODALFRAME.0 as i32);

        SetWindowLongA(hwnd, GWL_STYLE, next_style);
        SetWindowLongA(hwnd, GWL_EXSTYLE, next_ex_style);

        if let Some(rect) = primary_monitor_rect() {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                width,
                height,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        } else {
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }

        // Install hit-test subclass
        let _ = SetWindowSubclass(hwnd, Some(hit_test_subclass_proc), 101, 0);

        Ok(())
    }
}

unsafe extern "system" fn hit_test_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uid: usize,
    _ref_data: usize,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        // Drag & Capture Guard: If this window holds capture, check mouse button state
        if GetCapture() == hwnd {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture};
            let lbutton = GetAsyncKeyState(0x01) as i16; // VK_LBUTTON
            if lbutton >= 0 {
                // Left mouse button is up, release leaked capture
                let _ = ReleaseCapture();
            } else {
                return LRESULT(HTCLIENT as isize);
            }
        }

        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
        let pt = POINT { x, y };

        // Convert screen coordinates to window client coordinates
        let mut client_pt = pt;
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut client_pt);

        if let Some(storage) = HIT_TEST_REGIONS.get() {
            let regions = storage.read();
            for rect in regions.iter() {
                // Semi-open interval [left, right) x [top, bottom)
                if client_pt.x >= rect.left
                    && client_pt.x < rect.right
                    && client_pt.y >= rect.top
                    && client_pt.y < rect.bottom
                {
                    return LRESULT(HTCLIENT as isize);
                }
            }
        }

        // Outside all interactive regions: transparent to clicks, pass to desktop icons/wallpaper
        return LRESULT(HTTRANSPARENT as isize);
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
}

