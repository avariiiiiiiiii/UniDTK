//! The host's own attach under the desktop (the kernel keeps its copy in
//! `core/kernel/wallpaper.rs` for the WebView window until G5) and the reads of the
//! desktop host the transparency probe compares.

use std::sync::Mutex;

use windows::core::w;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW, GetPixel,
    MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetWindowLongW, GetWindowRect, SendMessageTimeoutW,
    SetLayeredWindowAttributes, SetParent, SetWindowLongW, SetWindowPos, ShowWindow, GWL_EXSTYLE, GWL_STYLE,
    HWND_TOP, LWA_ALPHA, SMTO_ABORTIFHUNG, SMTO_NORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_SHOWNOACTIVATE, WS_CAPTION, WS_CHILD, WS_EX_LAYERED, WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
};

static HOST: Mutex<Option<String>> = Mutex::new(None);
static TEMP_FILE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Holds process-wide ownership of a named Win32 mutex for single-instance exclusion.
/// Intentionally `!Send`: Win32 mutexes must be released by the owning thread.
pub struct NamedMutexGuard(windows::Win32::Foundation::HANDLE);

impl Drop for NamedMutexGuard {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::ReleaseMutex;
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
impl NamedMutexGuard {
    pub(crate) fn into_raw_for_test(self) -> usize {
        let raw = self.0 .0 as usize;
        std::mem::forget(self);
        raw
    }
}

/// Acquires `Local\UniDTK.DesktopHost`, recovering abandoned ownership from crashes.
pub fn acquire_single_instance() -> Result<NamedMutexGuard, String> {
    acquire_named_mutex("Local\\UniDTK.DesktopHost")
}

pub fn acquire_named_mutex(name: &str) -> Result<NamedMutexGuard, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

    let wide: Vec<u16> = std::ffi::OsStr::new(name).encode_wide().chain(Some(0)).collect();
    unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(wide.as_ptr()))
            .map_err(|e| format!("create desktop mutex {name}: {e}"))?;
        let wait = WaitForSingleObject(handle, 0);
        if wait == WAIT_OBJECT_0 {
            return Ok(NamedMutexGuard(handle));
        }
        if wait == WAIT_ABANDONED {
            note(&format!("recovered abandoned desktop host mutex {name}"));
            return Ok(NamedMutexGuard(handle));
        }
        let _ = CloseHandle(handle);
        Err(format!("desktop host instance mutex {name} already held"))
    }
}

/// Resolves the current process user SID and builds a protected SDDL granting full
/// access strictly to that user and LocalSystem (`SY`). Avoids `OW`, whose rights vary
/// across elevation boundaries and atomic file replacement.
pub fn user_owner_system_sddl() -> Result<String, String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    unsafe {
        let mut raw_token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token)
            .map_err(|e| format!("OpenProcessToken: {e}"))?;
        let token = TokenHandle(raw_token);

        let mut needed = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            return Err("GetTokenInformation(TokenUser) returned zero length".into());
        }
        let mut buffer = vec![0u8; needed as usize];
        GetTokenInformation(token.0, TokenUser, Some(buffer.as_mut_ptr().cast()), needed, &mut needed)
            .map_err(|e| format!("GetTokenInformation(TokenUser): {e}"))?;

        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid_pwstr = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid_pwstr)
            .map_err(|e| format!("ConvertSidToStringSidW: {e}"))?;
        let mut len = 0usize;
        while *sid_pwstr.0.add(len) != 0 {
            len += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_pwstr.0, len));
        let _ = LocalFree(HLOCAL(sid_pwstr.0.cast()));
        Ok(format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"))
    }
}

pub fn random_token() -> Result<String, String> {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let mut bytes = [0u8; 32];
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(|e| format!("token entropy: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The DACL is protected at creation, before any secret byte exists. Only the resolved
/// `TokenUser` SID and SYSTEM can read it; atomic replacement preserves these permissions.
pub fn write_private_file_atomic(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, GENERIC_WRITE, HLOCAL};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Storage::FileSystem::{CreateFileW, MoveFileExW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create private dir: {e}"))?;
        }
    }
    let sddl = user_owner_system_sddl()?;
    let sddl_wide: Vec<u16> = std::ffi::OsStr::new(&sddl).encode_wide().chain(Some(0)).collect();
    let seq = TEMP_FILE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("tmp");
    let temporary = path.with_extension(format!("{ext}.{}.{seq}.tmp", std::process::id()));
    let wide = |path: &std::path::Path| path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let source = wide(&temporary);
    let target = wide(path);
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl_wide.as_ptr()), SDDL_REVISION_1, &mut descriptor, None)
            .map_err(|e| format!("private ACL: {e}"))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0, bInheritHandle: BOOL(0),
        };
        let created = CreateFileW(PCWSTR(source.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_READ, Some(&attributes), CREATE_NEW, FILE_ATTRIBUTE_NORMAL, None);
        let _ = LocalFree(HLOCAL(descriptor.0));
        let handle = created.map_err(|e| format!("create private file: {e}"))?;
        let mut file = std::fs::File::from_raw_handle(handle.0);
        let written = file.write_all(contents).and_then(|_| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("write private file: {error}"));
        }
        if let Err(error) = MoveFileExW(PCWSTR(source.as_ptr()), PCWSTR(target.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("publish private file: {error}"));
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn write_private_token(path: &std::path::Path, token: &str) -> Result<(), String> {
    write_private_file_atomic(path, token.as_bytes())
}

pub fn note(line: &str) {
    let directory = unidtk_api::paths::logs_dir();
    let _ = std::fs::create_dir_all(&directory);
    let path = directory.join("unidtk-desktop.log");
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
            // The kernel's rule (`core/kernel/wallpaper.rs`): a WorkerW smaller than 640x480 is
            // not the wallpaper's. Without it the host took a stray WorkerW after Win+D and
            // sat behind the kernel's window, which stayed on Progman.
            let mut rect = RECT::default();
            let _ = GetWindowRect(worker, &mut rect);
            if !worker.0.is_null() && rect.right - rect.left >= 640 && rect.bottom - rect.top >= 480 {
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
    desktop_host_for_attempt(60)
}

pub fn allow_progman_fallback(shell_tray_present: bool, attempt: usize) -> bool {
    (shell_tray_present && attempt >= 4) || attempt >= 60
}

fn has_shell_tray() -> bool {
    unsafe {
        FindWindowW(w!("Shell_TrayWnd"), None)
            .map(|hwnd| !hwnd.0.is_null())
            .unwrap_or(false)
    }
}

fn desktop_host_for_attempt(attempt: usize) -> Result<HWND, String> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).map_err(|e| format!("Progman: {e}"))?;
        let mut result = 0usize;
        let flags = SMTO_NORMAL | SMTO_ABORTIFHUNG;
        // Asks Explorer to split the wallpaper into its own WorkerW, as the kernel does.
        let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(1), flags, 100, Some(&mut result));
        let mut worker = HWND::default();
        let _ = EnumWindows(Some(find_sibling_worker), LPARAM(&mut worker as *mut HWND as isize));
        if !worker.0.is_null() {
            return Ok(worker);
        }

        // Fallback messages used by newer/restarted Explorer builds before creating sibling WorkerW.
        let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(0), flags, 100, Some(&mut result));
        let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0), LPARAM(0), flags, 100, Some(&mut result));
        let _ = EnumWindows(Some(find_sibling_worker), LPARAM(&mut worker as *mut HWND as isize));
        if !worker.0.is_null() {
            return Ok(worker);
        }

        if allow_progman_fallback(has_shell_tray(), attempt) {
            // Top-level WorkerW without SHELLDLL_DefView (>= 640x480), matching kernel fallback.
            let mut top_worker = FindWindowExW(HWND::default(), HWND::default(), w!("WorkerW"), None).unwrap_or_default();
            while !top_worker.0.is_null() {
                let inner_def = FindWindowExW(top_worker, HWND::default(), w!("SHELLDLL_DefView"), None).unwrap_or_default();
                if inner_def.0.is_null() {
                    let mut rect = RECT::default();
                    let _ = GetWindowRect(top_worker, &mut rect);
                    if rect.right - rect.left >= 640 && rect.bottom - rect.top >= 480 {
                        return Ok(top_worker);
                    }
                }
                top_worker = FindWindowExW(HWND::default(), top_worker, w!("WorkerW"), None).unwrap_or_default();
            }

            let def_view = FindWindowExW(progman, HWND::default(), w!("SHELLDLL_DefView"), None).unwrap_or_default();
            if !def_view.0.is_null() {
                return Ok(progman);
            }
        }
        Err("no desktop host".into())
    }
}

#[allow(dead_code)]
/// Worker-side discovery: no HWND on Rune's UI thread is blocked by the shell search.
pub fn discover() -> Result<isize, String> { desktop_host().map(|host| host.0 as isize) }

pub fn discover_with_attempt(attempt: usize) -> Result<isize, String> {
    desktop_host_for_attempt(attempt).map(|host| host.0 as isize)
}

pub fn window_scale(hwnd: isize) -> f64 {
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(HWND(hwnd as _)) };
    if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
}

/// HWND values can survive in our snapshot after Explorer destroys its children,
/// and Windows may reuse them. Never inspect or post input to a foreign window.
pub fn window_is_attached(hwnd: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetParent, GetWindowThreadProcessId, IsWindow};
    if hwnd == 0 { return false; }
    let window = HWND(hwnd as _);
    unsafe {
        let mut pid = 0;
        if !IsWindow(window).as_bool() || GetWindowThreadProcessId(window, Some(&mut pid)) == 0
            || pid != std::process::id() || GetWindowLongW(window, GWL_STYLE) & WS_CHILD.0 as i32 == 0 {
            return false;
        }
        let parent = GetParent(window).unwrap_or_default();
        IsWindow(parent).as_bool() && matches!(class_of(parent).as_str(), "Progman" | "WorkerW")
    }
}

/// Preserve the manual WS_CHILD style. winit's cached top-level style writer
/// cannot be used after SetParent: it would turn this HWND back into a popup.
pub fn enable_widget_input(hwnd: isize) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT;
    let hwnd = HWND(hwnd as _);
    unsafe {
        let extended = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(hwnd, GWL_EXSTYLE, extended & !(WS_EX_TRANSPARENT.0 as i32));
        if GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as i32 != 0 {
            return Err("WS_EX_TRANSPARENT was not cleared".into());
        }
    }
    Ok(())
}

/// Makes `hwnd` a full-screen child of the desktop host, above its other children.
/// With `UNIDTK_PROBE_LAYERED=1` it also becomes a layered window at full alpha, as the
/// kernel makes the WebView window.
pub fn attach(hwnd: isize) -> Result<(), String> {
    let host = desktop_host()?;
    attach_to(hwnd, host.0 as isize)
}

/// UI-thread step only: validate the worker's result, then change parent and geometry.
pub fn attach_to(hwnd: isize, host: isize) -> Result<(), String> {
    let (hwnd, host) = (HWND(hwnd as _), HWND(host as _));
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        if !IsWindow(hwnd).as_bool() || !IsWindow(host).as_bool() {
            return Err("a desktop window disappeared before attachment".into());
        }
    }
    if !matches!(class_of(host).as_str(), "Progman" | "WorkerW") {
        return Err("the discovered desktop handle was reused".into());
    }
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

/// The colour at each point of the desktop host as DWM composed it, children included,
/// as `[r, g, b]`. `PrintWindow` with `PW_RENDERFULLCONTENT` reads the host even while
/// other windows cover it, which a screen copy cannot. With `UNIDTK_PROBE_DUMP=<dir>` the
/// whole image is also written there as `<label>.bmp`.
pub fn read_pixels(points: &[(&'static str, i32, i32)], label: &str) -> Vec<[u8; 3]> {
    let Ok(host) = desktop_host() else {
        return vec![[0, 0, 0]; points.len()];
    };
    unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(host, &mut rect);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let screen = GetDC(HWND::default());
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(memory, bitmap);
        let printed = PrintWindow(host, memory, PRINT_WINDOW_FLAGS(2)).as_bool(); // PW_RENDERFULLCONTENT
        note(&format!("read {label}: PrintWindow {printed} {width}x{height}"));
        let pixels = points
            .iter()
            .map(|(_, x, y)| {
                let color = GetPixel(memory, *x - rect.left, *y - rect.top).0;
                [(color & 0xFF) as u8, ((color >> 8) & 0xFF) as u8, ((color >> 16) & 0xFF) as u8]
            })
            .collect();
        if let Ok(dir) = std::env::var("UNIDTK_PROBE_DUMP") {
            dump_bmp(memory, bitmap, width, height, &std::path::Path::new(&dir).join(format!("{label}.bmp")));
        }
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(HWND::default(), screen);
        pixels
    }
}

/// Writes a 32-bit top-down BMP of `bitmap`.
unsafe fn dump_bmp(memory: HDC, bitmap: HBITMAP, width: i32, height: i32, path: &std::path::Path) {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut data = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(memory, bitmap, 0, height as u32, Some(data.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
    if lines == 0 {
        return;
    }
    let header_size = 14 + std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    let mut file = Vec::with_capacity(header_size as usize + data.len());
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&(header_size + data.len() as u32).to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&header_size.to_le_bytes());
    let header = std::slice::from_raw_parts((&info.bmiHeader as *const BITMAPINFOHEADER).cast::<u8>(), std::mem::size_of::<BITMAPINFOHEADER>());
    file.extend_from_slice(header);
    file.extend_from_slice(&data);
    let _ = std::fs::write(path, file);
}

/// A PNG of `hwnd` as DWM composed it (`PW_RENDERFULLCONTENT`). GDI has no alpha, so the
/// clear pixels of the transparent host come out black, as in the kernel's CDP shots.
pub fn window_png(hwnd: isize) -> Result<(u32, u32, Vec<u8>), String> {
    let hwnd = HWND(hwnd as _);
    if hwnd.0.is_null() {
        return Err("the host window is not attached yet".into());
    }
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).map_err(|e| format!("GetWindowRect: {e}"))?;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let pixels = (width as i64).checked_mul(height as i64).filter(|n| width > 0 && height > 0 && *n <= 16_777_216)
            .ok_or("capture dimensions exceed 16 megapixels")? as usize;
        let screen = GetDC(HWND::default());
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        if screen.0.is_null() || memory.0.is_null() || bitmap.0.is_null() {
            if !bitmap.0.is_null() { let _ = DeleteObject(bitmap); }
            if !memory.0.is_null() { let _ = DeleteDC(memory); }
            if !screen.0.is_null() { ReleaseDC(HWND::default(), screen); }
            return Err("cannot allocate capture bitmap".into());
        }
        let old = SelectObject(memory, bitmap);
        let printed = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(2)).as_bool();
        // GetDIBits requires a bitmap that is not selected into a DC.
        SelectObject(memory, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; pixels * 4];
        let lines = GetDIBits(memory, bitmap, 0, height as u32, Some(bgra.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(HWND::default(), screen);
        if !printed || lines != height {
            return Err("PrintWindow could not read the host window".into());
        }
        for pixel in bgra.chunks_exact_mut(4) { pixel.swap(0, 2); pixel[3] = 255; }
        let bytes = encode_rgba_png(width as u32, height as u32, &bgra)?;
        Ok((width as u32, height as u32, bytes))
    }
}

/// A left click at client pixel `(x, y)`, posted to the window, so it travels winit's and
/// Rune's own input path.
pub fn click(hwnd: isize, x: i32, y: i32) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
    let hwnd = HWND(hwnd as _);
    if hwnd.0.is_null() {
        return Err("the host window is not attached yet".into());
    }
    let at = LPARAM(((y as u16 as isize) << 16) | (x as u16 as isize));
    unsafe {
        PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), at).map_err(|e| e.to_string())?;
        PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), at).map_err(|e| e.to_string())?; // MK_LBUTTON
        PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), at).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Encodes straight RGBA8 pixels (`width x height`) as PNG bytes without blocking UI.
pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "PNG dimensions overflow".to_string())?;
    if rgba.len() != expected {
        return Err(format!("RGBA buffer length {} does not match {}x{}x4", rgba.len(), width, height));
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Open kernel handles, GDI objects, and USER objects owned by this process.
pub fn process_handles() -> (u32, u32, u32) {
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };
    unsafe {
        let process = GetCurrentProcess();
        let mut handles = 0u32;
        let _ = GetProcessHandleCount(process, &mut handles);
        let gdi = GetGuiResources(process, GR_GDIOBJECTS);
        let user = GetGuiResources(process, GR_USEROBJECTS);
        (handles, gdi, user)
    }
}

/// CPU time in milliseconds, Working Set and private bytes of this process.
pub fn process_usage() -> (u64, u64, u64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let process = GetCurrentProcess();
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        let cpu_ms = (ticks(kernel) + ticks(user)) / 10_000;
        let mut counters = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let _ = GetProcessMemoryInfo(process, (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), counters.cb);
        (cpu_ms, counters.WorkingSetSize as u64, counters.PrivateUsage as u64)
    }
}

/// Limits where `hwnd` takes the pointer to `rects` (client pixels, `[x, y, w, h]`): a
/// window region, which Windows hit-tests for every process, so a click outside reaches
/// the WebView or the desktop below. `WM_NCHITTEST` returning `HTTRANSPARENT` would pass
/// a click only to windows of the host's own thread. The region also clips what the
/// window shows, so each rect carries a margin for its shadow.
pub fn set_hit_region(hwnd: isize, rects: &[[i32; 4]]) -> Result<(), String> {
    use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, SetWindowRgn, RGN_OR};
    let hwnd = HWND(hwnd as _);
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        for [x, y, w, h] in rects {
            let part = CreateRectRgn(*x, *y, x + w, y + h);
            CombineRgn(region, region, part, RGN_OR);
            let _ = DeleteObject(part);
        }
        // The system owns the region from here on; skip synchronous DWM repaint (`false`)
        // because Rune presents asynchronously through DirectComposition.
        if SetWindowRgn(hwnd, region, false) == 0 {
            let _ = DeleteObject(region);
            return Err("SetWindowRgn failed".into());
        }
    }
    Ok(())
}

/// Who takes a click at client pixel `(x, y)`, answered three ways: whether the point is in
/// the host's window region, which child of the desktop host Windows picks there (the
/// answer within the desktop, whatever covers it), and which window is on top at that point
/// on the screen (the user's answer, meaningful only while the desktop is uncovered).
pub fn takes_point(hwnd: isize, x: i32, y: i32) -> serde_json::Value {
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, GetWindowRgn, PtInRegion};
    use windows::Win32::UI::WindowsAndMessaging::{
        ChildWindowFromPointEx, GetAncestor, GetParent, WindowFromPoint, CWP_SKIPINVISIBLE, GA_ROOT,
    };
    let host = HWND(hwnd as _);
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        let kind = GetWindowRgn(host, region);
        let in_region = kind.0 != 0 && PtInRegion(region, x, y).as_bool(); // 0 is ERROR: no region
        let _ = DeleteObject(region);

        let mut origin = RECT::default();
        let _ = GetWindowRect(host, &mut origin);
        let screen = POINT { x: origin.left + x, y: origin.top + y };
        let parent = GetParent(host).unwrap_or_default();
        let mut parent_rect = RECT::default();
        let _ = GetWindowRect(parent, &mut parent_rect);
        let in_parent = POINT { x: screen.x - parent_rect.left, y: screen.y - parent_rect.top };
        let child = ChildWindowFromPointEx(parent, in_parent, CWP_SKIPINVISIBLE);
        let on_top = WindowFromPoint(screen);
        let describe = |w: HWND| format!("0x{:X} {}", w.0 as usize, class_of(w));
        serde_json::json!({
            "host_is_child": GetWindowLongW(host, GWL_STYLE) & WS_CHILD.0 as i32 != 0,
            "parent": describe(parent),
            "in_region": in_region,
            "desktop_child": describe(child),
            "desktop_child_is_host": child == host,
            "on_screen": describe(on_top),
            "on_screen_is_host": on_top == host || GetAncestor(on_top, GA_ROOT) == host,
        })
    }
}
