use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub extension: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<String>,
    pub created: Option<String>,
}

pub fn get_file_info(path: &str) -> Result<FileInfo, String> {
    let p = Path::new(path);
    let metadata = std::fs::metadata(p).map_err(|e| format!("Failed to read metadata: {}", e))?;

    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let extension = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();

    let modified = metadata.modified().ok().and_then(|t| {
        let duration = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(format!("{}", duration.as_secs()))
    });

    let created = metadata.created().ok().and_then(|t| {
        let duration = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(format!("{}", duration.as_secs()))
    });

    Ok(FileInfo {
        name,
        path: path.to_string(),
        extension,
        is_dir: metadata.is_dir(),
        size: if metadata.is_dir() { None } else { Some(metadata.len()) },
        modified,
        created,
    })
}

pub fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size_f = size as f64;
    let mut unit_index = 0;

    while size_f >= 1024.0 && unit_index < UNITS.len() - 1 {
        size_f /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", size, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size_f, UNITS[unit_index])
    }
}

pub fn get_file_type_name(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "ico" => "Image",
        "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" => "Video",
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "wma" | "m4a" => "Audio",
        "txt" | "md" | "log" | "csv" => "Text",
        "json" | "xml" | "yaml" | "yml" | "toml" => "Config",
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "java" | "c" | "cpp" | "h" | "go" | "rb" | "php" => "Source Code",
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" => "Archive",
        "pdf" => "PDF Document",
        "doc" | "docx" => "Word Document",
        "xls" | "xlsx" => "Spreadsheet",
        "ppt" | "pptx" => "Presentation",
        "exe" | "msi" | "dll" => "Executable",
        "ttf" | "otf" | "woff" | "woff2" => "Font",
        _ => "File",
    }
}

pub fn is_valid_filename(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Filename cannot be empty".to_string());
    }

    if name.len() > 255 {
        return Err("Filename too long".to_string());
    }

    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    for ch in name.chars() {
        if invalid_chars.contains(&ch) {
            return Err(format!("Invalid character: {}", ch));
        }
    }

    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];
    let upper = name.to_uppercase();
    for r in &reserved {
        if upper == *r {
            return Err(format!("Reserved name: {}", r));
        }
    }

    Ok(())
}

pub fn open_with_default(path: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let rc = ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if rc.0 as isize <= 32 {
            Err(format!("ShellExecute failed ({})", rc.0 as isize))
        } else {
            Ok(())
        }
    }
}

pub fn create_directory(path: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("create dir: {e}"))
}

pub fn create_empty_file(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.exists() {
        return Err("already exists".to_string());
    }
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create parent: {e}"))?;
        }
    }
    std::fs::File::create(p).map_err(|e| format!("create file: {e}"))?;
    Ok(())
}

pub fn rename_path(from: &str, to: &str) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|e| format!("rename: {e}"))
}

pub fn copy_path(from: &str, to: &str) -> Result<(), String> {
    copy_recursive(Path::new(from), Path::new(to))
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| format!("copy mkdir: {e}"))?;
        for entry in std::fs::read_dir(from).map_err(|e| format!("copy read_dir: {e}"))? {
            let entry = entry.map_err(|e| format!("copy entry: {e}"))?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| format!("copy parent: {e}"))?;
            }
        }
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("copy: {e}"))
    }
}

pub fn delete_to_recycle_bin(path: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, SHFILEOPSTRUCTW,
    };

    let mut from: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
        .collect();

    let mut op = SHFILEOPSTRUCTW {
        hwnd: HWND::default(),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_mut_ptr()),
        pTo: PCWSTR::null(),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0) as u16,
        fAnyOperationsAborted: false.into(),
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: PCWSTR::null(),
    };

    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        Err(format!("SHFileOperation failed ({code})"))
    } else {
        Ok(())
    }
}

#[allow(dead_code)]
pub fn unique_path(dir: &str, name: &str) -> PathBuf {
    let mut candidate = Path::new(dir).join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = Path::new(name)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for i in 2..200 {
        candidate = Path::new(dir).join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    candidate
}
