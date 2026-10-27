use crate::portable;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

pub const MY_PC_PATH: &str = "::MY_PC::";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateStyle {
    Iso,
    UsShort,
    Long,
}

impl Default for DateStyle {
    fn default() -> Self {
        DateStyle::UsShort
    }
}

pub fn filetime_to_string(filetime: i64, date_style: DateStyle, time_format_24h: bool) -> Option<String> {
    if filetime == 0 {
        return None;
    }
    let unix_time = (filetime / 10_000_000) - 11_644_473_600;
    if unix_time <= 0 {
        return None;
    }
    let dt_utc = Utc.timestamp_opt(unix_time, 0).single()?;
    let dt_local: DateTime<Local> = dt_utc.into();
    let date_fmt = match date_style {
        DateStyle::Iso => "%Y-%m-%d",
        DateStyle::UsShort => "%m/%d/%Y",
        DateStyle::Long => "%B %d, %Y",
    };
    let time_fmt = if time_format_24h { "%H:%M" } else { "%I:%M %p" };
    Some(dt_local.format(&format!("{date_fmt} {time_fmt}")).to_string())
}

fn metadata_to_item(entry: &std::fs::DirEntry, date_style: DateStyle, time_format_24h: bool) -> Option<FileItem> {
    let name = entry.file_name().to_string_lossy().to_string();
    if name.eq_ignore_ascii_case("desktop.ini") {
        return None;
    }

    let metadata = entry.metadata().ok()?;
    let full_path = entry.path();
    let is_dir = metadata.is_dir();

    #[cfg(windows)]
    let is_hidden = (name.starts_with('.') && name != "." && name != "..")
        || (metadata.file_attributes() & 0x2 != 0)  // FILE_ATTRIBUTE_HIDDEN
        || (metadata.file_attributes() & 0x4 != 0); // FILE_ATTRIBUTE_SYSTEM
    #[cfg(not(windows))]
    let is_hidden = name.starts_with('.') && name != "." && name != "..";

    let file_size = if is_dir { None } else { Some(metadata.len()) };

    let modified_time_raw = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64 * 10_000_000 + 11_644_473_600 * 10_000_000);
    let created_time_raw = metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64 * 10_000_000 + 11_644_473_600 * 10_000_000);

    let modified_time = modified_time_raw.and_then(|ft| filetime_to_string(ft, date_style, time_format_24h));
    let created_time = created_time_raw.and_then(|ft| filetime_to_string(ft, date_style, time_format_24h));

    Some(FileItem::new(
        name,
        full_path.to_string_lossy().into_owned(),
        is_dir,
        is_hidden,
        file_size,
        modified_time,
        created_time,
        modified_time_raw,
        created_time_raw,
    ))
}

#[allow(dead_code)]
pub fn get_drive_space(path: &Path) -> Option<(u64, u64)> {
    let _ = path;
    None
}

pub fn normalize_dir_path(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed.len() == 1 {
        let b = trimmed.as_bytes()[0];
        if b.is_ascii_alphabetic() {
            let drive = (b as char).to_ascii_uppercase();
            return PathBuf::from(format!("{drive}:\\"));
        }
    }
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        let first = trimmed.as_bytes()[0];
        if first.is_ascii_alphabetic() {
            let drive = (first as char).to_ascii_uppercase();
            return PathBuf::from(format!("{drive}:\\"));
        }
    }
    if trimmed.len() >= 3 && trimmed.as_bytes()[1] == b':' {
        let first = trimmed.as_bytes()[0];
        if first.is_ascii_alphabetic() {
            let rest = &trimmed[2..];
            if rest.chars().all(|c| c == '/' || c == '\\') {
                let drive = (first as char).to_ascii_uppercase();
                return PathBuf::from(format!("{drive}:\\"));
            }
        }
    }
    PathBuf::from(trimmed)
}

pub fn calculate_folder_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let initial_path = if let Some(s) = path.to_str() {
        normalize_dir_path(s)
    } else {
        path.to_path_buf()
    };
    let mut stack = vec![(initial_path, 0usize)];
    let mut count = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        if depth > 32 || count > 50_000 {
            break;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                count += 1;
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() && !ft.is_symlink() {
                        stack.push((entry.path(), depth + 1));
                    } else if ft.is_file() {
                        if let Ok(meta) = entry.metadata() {
                            total += meta.len();
                        }
                    }
                }
            }
        }
    }
    total
}

pub fn scan_directory(path: &str) -> Result<Vec<FileItem>, String> {
    if path == MY_PC_PATH || path.is_empty() {
        return Ok(Vec::new());
    }
    let dir_path = normalize_dir_path(path);
    if portable::is_portable_path(&dir_path) {
        return Ok(Vec::new());
    }
    if !dir_path.exists() {
        return Err(format!("Path does not exist: {path}"));
    }
    if !dir_path.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }
    let entries: Vec<FileItem> = std::fs::read_dir(&dir_path)
        .map_err(|e| format!("Failed to read dir {}: {e}", dir_path.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| metadata_to_item(&e, DateStyle::default(), true))
        .filter(|item| !item.name.eq_ignore_ascii_case("desktop.ini"))
        .collect();
    unidtk_api::log(format!("scan_directory: {} -> {} items", dir_path.display(), entries.len()));
    Ok(entries)
}

pub async fn scan_directory_async(path: &str) -> Result<Vec<FileItem>, String> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || scan_directory(&path))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

pub async fn scan_directory_full_async(path: &str) -> Result<Vec<FileItem>, String> {
    scan_directory_async(path).await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_hidden: bool,
    pub file_size: Option<u64>,
    pub modified_time: Option<String>,
    pub created_time: Option<String>,
    pub modified_time_raw: Option<i64>,
    pub created_time_raw: Option<i64>,
    pub total_space: Option<u64>,
    pub free_space: Option<u64>,
}

impl FileItem {
    pub fn new(
        name: String,
        path: String,
        is_dir: bool,
        is_hidden: bool,
        file_size: Option<u64>,
        modified_time: Option<String>,
        created_time: Option<String>,
        modified_time_raw: Option<i64>,
        created_time_raw: Option<i64>,
    ) -> Self {
        Self { name, path, is_dir, is_hidden, file_size, modified_time, created_time, modified_time_raw, created_time_raw, total_space: None, free_space: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_dir_path_drive_letters() {
        assert_eq!(normalize_dir_path("C:"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("c:"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("c"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("C"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("c:/"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("c://"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("D:"), PathBuf::from("D:\\"));
        assert_eq!(normalize_dir_path("C:\\"), PathBuf::from("C:\\"));
        assert_eq!(normalize_dir_path("C:\\Users"), PathBuf::from("C:\\Users"));
        assert_eq!(normalize_dir_path(""), PathBuf::from(""));
    }
}

