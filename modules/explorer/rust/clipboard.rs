use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
const CF_HDROP: u32 = 15;

/// What the next program that pastes these files does with them. Explorer and most file
/// managers read it from the "Preferred DropEffect" format: without it every paste is a copy.
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

fn drop_effect(cut: bool) -> u32 {
    if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY }
}

/// Attempts made on `OpenClipboard` while another process holds the clipboard.
const OPEN_CLIPBOARD_ATTEMPTS: u32 = 10;
const OPEN_CLIPBOARD_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

/// Opens the clipboard, retrying briefly: contention with another process clears
/// within milliseconds. The attempt count bounds the wait to about 200 ms.
fn open_clipboard() -> Result<(), String> {
    retry_open_clipboard(
        || unsafe { OpenClipboard(HWND(std::ptr::null_mut())) }.map_err(|e| format!("OpenClipboard failed: {e}")),
        OPEN_CLIPBOARD_ATTEMPTS,
        OPEN_CLIPBOARD_RETRY_DELAY,
    )
}

fn retry_open_clipboard<F>(mut open: F, attempts: u32, delay: std::time::Duration) -> Result<(), String>
where
    F: FnMut() -> Result<(), String>,
{
    let mut last_error = String::new();
    for attempt in 0..attempts {
        match open() {
            Ok(()) => return Ok(()),
            Err(e) => last_error = e,
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    Err(last_error)
}

/// Frees a global block that was never handed to the clipboard. Once
/// `SetClipboardData` succeeds the clipboard owns the block, and freeing it again
/// would be a double free, so the release runs only on failure.
fn release_unless_owned<F: FnOnce()>(outcome: Result<(), String>, release: F) -> Result<(), String> {
    if outcome.is_err() {
        release();
    }
    outcome
}

pub fn copy_files_to_clipboard(paths: &[PathBuf], cut: bool) -> Result<(), String> {
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
        let h_effect = match alloc_drop_effect(drop_effect(cut)) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = GlobalFree(h_global);
                return Err(error);
            }
        };
        let outcome = publish_hdrop(h_global, dropfiles_size, &buffer, h_effect);
        // The effect block is released by the function that failed to hand it over.
        release_unless_owned(outcome, || {
            let _ = GlobalFree(h_global);
        })
    }
}

unsafe fn alloc_drop_effect(effect: u32) -> Result<HGLOBAL, String> {
    let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4).map_err(|e| format!("GlobalAlloc failed: {e}"))?;
    let ptr = GlobalLock(handle);
    if ptr.is_null() {
        let _ = GlobalFree(handle);
        return Err("GlobalLock failed".into());
    }
    *(ptr as *mut u32) = effect;
    let _ = GlobalUnlock(handle);
    Ok(handle)
}

/// Fills a DROPFILES block and hands it to the clipboard, with the drop effect
/// (copy or move) beside it. Every failure returns before the clipboard takes ownership
/// of `h_global`, so the caller frees that block; the effect block is freed here when
/// the clipboard did not take it.
unsafe fn publish_hdrop(
    h_global: HGLOBAL,
    dropfiles_size: usize,
    buffer: &[u16],
    h_effect: HGLOBAL,
) -> Result<(), String> {
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

    if let Err(error) = open_clipboard() {
        let _ = GlobalFree(h_effect);
        return Err(error);
    }
    let effect_format = RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect"));
    let result = (|| -> Result<(), String> {
        if let Err(e) = EmptyClipboard() {
            let _ = GlobalFree(h_effect);
            return Err(format!("EmptyClipboard failed: {e}"));
        }
        // The effect first: if the file list then fails, emptying the clipboard again
        // drops it, and the caller still owns the file-list block.
        if effect_format == 0 {
            let _ = GlobalFree(h_effect);
            return Err("RegisterClipboardFormat failed".to_string());
        }
        if let Err(e) = SetClipboardData(effect_format, HANDLE(h_effect.0)) {
            let _ = GlobalFree(h_effect);
            return Err(format!("SetClipboardData (drop effect) failed: {e}"));
        }
        SetClipboardData(CF_HDROP, HANDLE(h_global.0))
            .map(|_| ())
            .map_err(|e| {
                let _ = EmptyClipboard();
                format!("SetClipboardData failed: {e}")
            })
    })();
    let _ = CloseClipboard();
    result
}

/// Puts plain Unicode text on the clipboard. Used by the kernel's widget copy
/// action, which runs in a window that never gets OS focus.
pub fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h_global = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)
            .map_err(|e| format!("GlobalAlloc failed: {e}"))?;
        let outcome = publish_text(h_global, &wide);
        release_unless_owned(outcome, || {
            let _ = GlobalFree(h_global);
        })
    }
}

unsafe fn publish_text(h_global: HGLOBAL, wide: &[u16]) -> Result<(), String> {
    let ptr = GlobalLock(h_global);
    if ptr.is_null() {
        return Err("GlobalLock failed".into());
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
    let _ = GlobalUnlock(h_global);

    open_clipboard()?;
    let result = EmptyClipboard()
        .map_err(|e| format!("EmptyClipboard failed: {e}"))
        .and_then(|_| {
            SetClipboardData(CF_UNICODETEXT, HANDLE(h_global.0))
                .map(|_| ())
                .map_err(|e| format!("SetClipboardData failed: {e}"))
        });
    let _ = CloseClipboard();
    result
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

const CF_UNICODETEXT: u32 = 13;

/// Text on the clipboard: a browser's link (`UniformResourceLocatorW`) first,
/// otherwise plain Unicode text. `None` when there is neither.
pub fn paste_text_from_clipboard() -> Result<Option<String>, String> {
    use windows::core::w;
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;

    unsafe {
        let url_format = RegisterClipboardFormatW(w!("UniformResourceLocatorW"));
        OpenClipboard(HWND(std::ptr::null_mut()))
            .map_err(|e| format!("OpenClipboard failed: {e}"))?;
        let mut text = None;
        for format in [url_format, CF_UNICODETEXT] {
            if format == 0 {
                continue;
            }
            let Ok(handle) = GetClipboardData(format) else { continue };
            let global = windows::Win32::Foundation::HGLOBAL(handle.0);
            let ptr = GlobalLock(global) as *const u16;
            if ptr.is_null() {
                continue;
            }
            // Another process owns this block: trust its size, not a terminator.
            let capacity = windows::Win32::System::Memory::GlobalSize(global) / 2;
            let mut len = 0usize;
            while len < capacity && *ptr.add(len) != 0 {
                len += 1;
            }
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(global);
            if !value.trim().is_empty() {
                text = Some(value.trim().to_string());
                break;
            }
        }
        let _ = CloseClipboard();
        Ok(text)
    }
}

pub fn clear_clipboard() {
    unsafe {
        if OpenClipboard(HWND(std::ptr::null_mut())).is_ok() {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_publish_releases_the_memory_block() {
        let mut freed = 0;

        let outcome = release_unless_owned(Err("SetClipboardData failed".to_string()), || freed += 1);

        assert!(outcome.is_err());
        assert_eq!(freed, 1);
    }

    #[test]
    fn successful_publish_never_frees_the_block_the_clipboard_owns() {
        let mut freed = 0;

        let outcome = release_unless_owned(Ok(()), || freed += 1);

        assert!(outcome.is_ok());
        assert_eq!(freed, 0, "freeing after SetClipboardData succeeds is a double free");
    }

    #[test]
    fn open_retries_while_another_process_holds_the_clipboard() {
        let mut calls = 0;

        let outcome = retry_open_clipboard(
            || {
                calls += 1;
                if calls < 3 {
                    Err("busy".to_string())
                } else {
                    Ok(())
                }
            },
            5,
            std::time::Duration::ZERO,
        );

        assert!(outcome.is_ok());
        assert_eq!(calls, 3);
    }

    #[test]
    fn open_gives_up_after_a_bounded_number_of_attempts() {
        let mut calls = 0;

        let outcome: Result<(), String> = retry_open_clipboard(
            || {
                calls += 1;
                Err("busy".to_string())
            },
            4,
            std::time::Duration::ZERO,
        );

        assert!(outcome.is_err());
        assert_eq!(calls, 4);
    }
}
