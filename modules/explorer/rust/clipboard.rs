use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
const CF_HDROP: u32 = 15;

pub fn copy_files_to_clipboard(paths: &[PathBuf], _cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut buffer: Vec<u16> = Vec::new();
    for path in paths {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        buffer.extend(wide);
        buffer.push(0);
    }
    buffer.push(0);

    let dropfiles_size = std::mem::size_of::<DROPFILES>();
    let total_bytes = dropfiles_size + buffer.len() * 2;

    unsafe {
        let h_global = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_bytes)
            .map_err(|e| format!("GlobalAlloc failed: {e}"))?;
        let ptr = GlobalLock(h_global);
        if ptr.is_null() {
            return Err("GlobalLock failed".into());
        }

        let dropfiles = ptr as *mut DROPFILES;
        (*dropfiles).pFiles = dropfiles_size as u32;
        (*dropfiles).fWide = true.into();

        let dest = (ptr as *mut u8).add(dropfiles_size) as *mut u16;
        std::ptr::copy_nonoverlapping(buffer.as_ptr(), dest, buffer.len());

        let _ = GlobalUnlock(h_global);

        OpenClipboard(HWND(std::ptr::null_mut()))
            .map_err(|e| format!("OpenClipboard failed: {e}"))?;
        let _ = EmptyClipboard();
        if let Err(e) = SetClipboardData(CF_HDROP, windows::Win32::Foundation::HANDLE(h_global.0)) {
            let _ = CloseClipboard();
            return Err(format!("SetClipboardData failed: {e}"));
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

pub fn paste_files_from_clipboard() -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    unsafe {
        OpenClipboard(HWND(std::ptr::null_mut()))
            .map_err(|e| format!("OpenClipboard failed: {e}"))?;
        let handle = match GetClipboardData(CF_HDROP) {
            Ok(h) => h,
            Err(_) => {
                let _ = CloseClipboard();
                return Ok(files);
            }
        };
        let hdrop = HDROP(handle.0);
        let count = DragQueryFileW(hdrop, 0xFFFFFFFF, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            buf.pop(); // remove trailing null
            files.push(PathBuf::from(String::from_utf16_lossy(&buf)));
        }
        let _ = CloseClipboard();
    }
    Ok(files)
}

pub fn clear_clipboard() {
    unsafe {
        if OpenClipboard(HWND(std::ptr::null_mut())).is_ok() {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
    }
}
