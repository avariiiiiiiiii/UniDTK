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
    // create_dir_all succeeds for a folder that exists, which made "New Folder"
    // reuse the same name forever; say so, as create_empty_file does.
    if Path::new(path).exists() {
        return Err("already exists".to_string());
    }
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

/// `FILE_ATTRIBUTE_REPARSE_POINT`: symlinks, junctions and mount points.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// True for symlinks and junctions. `Metadata::is_symlink` reports false for
/// junctions, so the reparse attribute is the only reliable test on Windows.
#[cfg(windows)]
pub fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Renaming onto an existing path would silently replace it, so the destination
/// must be free. The one exception is a case-only rename of a file onto itself
/// (`report.txt` -> `Report.txt`): both names resolve to the same volume and file
/// index, so the rename changes only the stored case.
fn check_rename_destination(from: &Path, to: &Path) -> Result<(), String> {
    if !destination_exists(to) {
        return Ok(());
    }
    if is_case_only_rename(from, to) {
        return Ok(());
    }
    Err(format!("destination already exists: {}", to.display()))
}

fn destination_exists(to: &Path) -> bool {
    // symlink_metadata also sees dangling links, which a plain exists() misses.
    std::fs::symlink_metadata(to).is_ok()
}

fn is_case_only_rename(from: &Path, to: &Path) -> bool {
    let (Some(from_name), Some(to_name)) = (from.file_name(), to.file_name()) else {
        return false;
    };
    if from.parent() != to.parent() || !from_name.to_string_lossy().eq_ignore_ascii_case(&to_name.to_string_lossy()) {
        return false;
    }
    same_file(from, to).unwrap_or(false)
}

/// Two paths name the same file when they report the same volume serial and file index.
#[cfg(windows)]
fn same_file(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (id_a, id_b) = (file_identity(a)?, file_identity(b)?);
    Ok(id_a == id_b)
}

/// Volume serial and 64-bit file index. Rust's own getters are still unstable.
#[cfg(windows)]
fn file_identity(path: &Path) -> std::io::Result<(u32, u32, u32)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
    };
    // Access 0 reads metadata only; the flag is what lets a directory open.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle() as _), &mut info) }
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

#[cfg(not(windows))]
fn same_file(a: &Path, b: &Path) -> std::io::Result<bool> {
    Ok(a == b)
}

pub fn rename_path(from: &str, to: &str) -> Result<(), String> {
    let (from, to) = (Path::new(from), Path::new(to));
    check_rename_destination(from, to)?;
    std::fs::rename(from, to).map_err(|e| format!("rename: {e}"))
}

pub fn copy_path(from: &str, to: &str) -> Result<(), String> {
    let (from, to) = (Path::new(from), Path::new(to));
    // A junction or symlink at the top level is refused outright: silently
    // copying nothing would look like success.
    let source_meta = std::fs::symlink_metadata(from).map_err(|e| format!("copy source: {e}"))?;
    if is_reparse_point(&source_meta) {
        return Err(format!("cannot copy a link or junction: {}", from.display()));
    }
    if from.is_dir() {
        ensure_not_into_itself(from, to)?;
    }
    copy_recursive(from, to)
}

/// A folder cannot be copied into its own subtree: the walk would keep finding the
/// copies it is writing. The destination may not exist yet, so the longest existing
/// ancestor is canonicalized and the missing tail is appended.
fn ensure_not_into_itself(from: &Path, to: &Path) -> Result<(), String> {
    let source = from.canonicalize().map_err(|e| format!("copy source: {e}"))?;
    let destination = canonicalize_for_create(to).map_err(|e| format!("copy destination: {e}"))?;
    if destination.starts_with(&source) {
        return Err(format!("cannot copy a folder into itself: {}", from.display()));
    }
    Ok(())
}

fn canonicalize_for_create(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();
    while std::fs::symlink_metadata(existing).is_err() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_owned());
                existing = parent;
            }
            _ => break,
        }
    }
    let mut resolved = existing.canonicalize()?;
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| format!("copy mkdir: {e}"))?;
        for entry in std::fs::read_dir(from).map_err(|e| format!("copy read_dir: {e}"))? {
            let entry = entry.map_err(|e| format!("copy entry: {e}"))?;
            let child = entry.path();
            // Links are skipped: following a junction can loop back into the tree
            // being copied, or pull in data from outside the selection.
            let child_meta = std::fs::symlink_metadata(&child).map_err(|e| format!("copy entry: {e}"))?;
            if is_reparse_point(&child_meta) {
                continue;
            }
            copy_recursive(&child, &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if destination_exists(to) {
            return Err(format!("destination already exists: {}", to.display()));
        }
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

/// A drive root (`C:\`, `C:`) or an empty path: never a target for delete.
fn is_filesystem_root(path: &str) -> bool {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    let bytes = trimmed.as_bytes();
    trimmed.is_empty() || (bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

pub fn delete_to_recycle_bin(path: &str) -> Result<(), String> {
    if is_filesystem_root(path) {
        return Err(format!("refusing to delete a drive root: {path:?}"));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creating_a_folder_that_exists_is_an_error() {
        let root = Scratch::new("mkdir-twice");
        let dir = root.0.join("New Folder");
        assert!(create_directory(path_str(&dir)).is_ok());
        assert_eq!(create_directory(path_str(&dir)), Err("already exists".to_string()));
    }

    #[test]
    fn drive_roots_are_never_deleted() {
        for root in ["C:\\", "C:", "d:/", "", "  "] {
            assert!(is_filesystem_root(root), "{root:?} is a root");
            assert!(delete_to_recycle_bin(root).is_err(), "{root:?} must be refused");
        }
        for ordinary in ["C:\\Users", "D:\\a\\b.txt", "C:\\Windows\\"] {
            assert!(!is_filesystem_root(ordinary), "{ordinary:?} is not a root");
        }
    }

    /// Scratch directory under the system temp dir, removed on drop (also on panic).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("unidtk-files-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn path_str(path: &Path) -> &str {
        path.to_str().unwrap()
    }

    #[test]
    fn rename_refuses_to_overwrite_an_existing_file() {
        let root = Scratch::new("rename-clash");
        let from = root.0.join("a.txt");
        let to = root.0.join("b.txt");
        std::fs::write(&from, "source").unwrap();
        std::fs::write(&to, "keep me").unwrap();

        let result = rename_path(path_str(&from), path_str(&to));

        assert!(result.is_err(), "rename over an existing file must fail");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "keep me");
        assert!(from.exists());
    }

    #[test]
    fn rename_allows_a_case_only_change_on_the_same_file() {
        let root = Scratch::new("rename-case");
        let from = root.0.join("report.txt");
        let to = root.0.join("Report.txt");
        std::fs::write(&from, "data").unwrap();

        rename_path(path_str(&from), path_str(&to)).expect("case-only rename must work");

        let names: Vec<String> = std::fs::read_dir(&root.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["Report.txt".to_string()]);
    }

    #[test]
    fn copy_refuses_to_overwrite_an_existing_file() {
        let root = Scratch::new("copy-clash");
        let from = root.0.join("a.txt");
        let to = root.0.join("b.txt");
        std::fs::write(&from, "source").unwrap();
        std::fs::write(&to, "keep me").unwrap();

        let result = copy_path(path_str(&from), path_str(&to));

        assert!(result.is_err(), "copy over an existing file must fail");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "keep me");
    }

    #[test]
    fn copy_refuses_a_folder_into_its_own_subtree() {
        let root = Scratch::new("copy-subtree");
        let src = root.0.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub").join("f.txt"), "x").unwrap();

        let result = copy_path(path_str(&src), path_str(&src.join("sub").join("copy")));

        assert!(result.is_err(), "copying a folder into itself must fail");
        assert!(!src.join("sub").join("copy").exists());
    }

    #[test]
    fn copy_creates_missing_parent_folders_for_the_destination() {
        let root = Scratch::new("copy-nested");
        let src = root.0.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("f.txt"), "x").unwrap();
        let dest = root.0.join("new").join("deeper").join("dst");

        copy_path(path_str(&src), path_str(&dest)).expect("copy into a new nested folder must work");

        assert_eq!(std::fs::read_to_string(dest.join("f.txt")).unwrap(), "x");
    }

    #[cfg(windows)]
    #[test]
    fn copy_skips_junctions_instead_of_following_them() {
        let root = Scratch::new("copy-junction");
        let src = root.0.join("src");
        let outside = root.0.join("outside");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(src.join("real.txt"), "real").unwrap();
        std::fs::write(outside.join("secret.txt"), "outside").unwrap();
        let junction = src.join("link");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", path_str(&junction), path_str(&outside)])
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J must create the junction");

        let dest = root.0.join("dst");
        copy_path(path_str(&src), path_str(&dest)).unwrap();

        assert!(dest.join("real.txt").exists());
        assert!(!dest.join("link").exists(), "junction must not be copied");
        std::fs::remove_dir(&junction).unwrap();
    }
}
