//! The host's own attach under the desktop (the kernel keeps its copy in
//! `core/kernel/wallpaper.rs` for the WebView window until G5) and the screen reads the
//! transparency probe compares.

use std::sync::Mutex;

use windows::core::w;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, GetPixel,
    MonitorFromPoint, ReleaseDC, SelectObject, CAPTUREBLT, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetWindowLongW, GetWindowRect, SendMessageTimeoutW,
    SetLayeredWindowAttributes, SetParent, SetWindowLongW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
    HWND_TOP, LWA_ALPHA, SMTO_ABORTIFHUNG, SMTO_NORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_SHOWNOACTIVATE, WS_CAPTION, WS_CHILD, WS_EX_LAYERED, WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
};

static HOST: Mutex<Option<String>> = Mutex::new(None);

pub fn note(line: &str) {
    let path = std::env::temp_dir().join("unidtk-desktop.log");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, format!("{stamp} {line}\n").as_bytes()));
}

pub fn host_description() -> String {
    HOST.lock().ok().and_then(|host| host.clone()).unwrap_or_else(|| "none".into())
}

fn primary_rect() -> RECT {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcMonitor
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
        }
    }
}

pub fn primary_size() -> (i32, i32) {
    let rect = primary_rect();
    (rect.right - rect.left, rect.bottom - rect.top)
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

extern "system" fn find_sibling_worker(window: HWND, found: LPARAM) -> BOOL {
    unsafe {
        let def_view = FindWindowExW(window, HWND::default(), w!("SHELLDLL_DefView"), None).unwrap_or_default();
        if !def_view.0.is_null() {
            let worker = FindWindowExW(HWND::default(), window, w!("WorkerW"), None).unwrap_or_default();
            if !worker.0.is_null() {
                *(found.0 as *mut HWND) = worker;
                return BOOL(0);
            }
        }
        BOOL(1)
    }
}

/// The desktop host: the WorkerW behind the icons on the classic layout, Progman itself
/// on the newer Windows 11 one, where `SHELLDLL_DefView` and the wallpaper's WorkerW are
/// both children of Progman.
fn desktop_host() -> Result<HWND, String> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).map_err(|e| format!("Progman: {e}"))?;
        let mut result = 0usize;
        // Asks Explorer to split the wallpaper into its own WorkerW, as the kernel does.
        let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(1), SMTO_NORMAL | SMTO_ABORTIFHUNG, 100, Some(&mut result));
        let mut worker = HWND::default();
        let _ = EnumWindows(Some(find_sibling_worker), LPARAM(&mut worker as *mut HWND as isize));
        if !worker.0.is_null() {
            return Ok(worker);
        }
        let def_view = FindWindowExW(progman, HWND::default(), w!("SHELLDLL_DefView"), None).unwrap_or_default();
        if !def_view.0.is_null() {
            return Ok(progman);
        }
        Err("no desktop host".into())
    }
}

/// Makes `hwnd` a full-screen child of the desktop host, above its other children.
/// With `UNIDTK_PROBE_LAYERED=1` it also becomes a layered window at full alpha, as the
/// kernel makes the WebView window.
pub fn attach(hwnd: isize) -> Result<(), String> {
    let hwnd = HWND(hwnd as _);
    let host = desktop_host()?;
    let layered = std::env::var("UNIDTK_PROBE_LAYERED").is_ok_and(|value| value == "1");
    if let Ok(mut slot) = HOST.lock() {
        *slot = Some(format!("{} 0x{:X}{}", class_of(host), host.0 as usize, if layered { " layered" } else { "" }));
    }
    unsafe {
        SetParent(hwnd, host).map_err(|e| format!("SetParent: {e}"))?;
        let style = GetWindowLongW(hwnd, GWL_STYLE);
        let style = (style & !((WS_POPUP.0 | WS_CAPTION.0 | WS_THICKFRAME.0) as i32)) | (WS_CHILD.0 | WS_VISIBLE.0) as i32;
        SetWindowLongW(hwnd, GWL_STYLE, style);
        if layered {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as i32);
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF::default(), 255, LWA_ALPHA);
        }
        let mut host_rect = RECT::default();
        let _ = GetWindowRect(host, &mut host_rect);
        let primary = primary_rect();
        SetWindowPos(
            hwnd,
            HWND_TOP,
            primary.left - host_rect.left,
            primary.top - host_rect.top,
            primary.right - primary.left,
            primary.bottom - primary.top,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .map_err(|e| format!("SetWindowPos: {e}"))?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(())
}

/// The composed screen colour at each point, as `[r, g, b]`. `CAPTUREBLT` includes
/// layered windows, which a plain screen copy leaves out.
pub fn read_pixels(points: &[(&'static str, i32, i32)]) -> Vec<[u8; 3]> {
    unsafe {
        let screen = GetDC(HWND::default());
        let memory = CreateCompatibleDC(screen);
        let (width, height) = primary_size();
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(memory, bitmap);
        let _ = BitBlt(memory, 0, 0, width, height, screen, 0, 0, SRCCOPY | CAPTUREBLT);
        let pixels = points
            .iter()
            .map(|(_, x, y)| {
                let color = GetPixel(memory, *x, *y).0;
                [(color & 0xFF) as u8, ((color >> 8) & 0xFF) as u8, ((color >> 16) & 0xFF) as u8]
            })
            .collect();
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(HWND::default(), screen);
        pixels
    }
}
