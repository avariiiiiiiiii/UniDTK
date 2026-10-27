use crate::archive;
use crate::fs::FileItem;
use crate::hashing;
use crate::preview;
use crate::search::SearchEngine;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type SharedSearch = Arc<Mutex<SearchEngine>>;

pub fn create_shared_search() -> SharedSearch {
    Arc::new(Mutex::new(SearchEngine::new(100)))
}

/// Модуль проводника: единственное состояние — поисковый индекс, который ядро
/// кладёт в managed-state для обёрток команд.
pub struct ExplorerModule {
    search: SharedSearch,
}

impl ExplorerModule {
    pub fn new() -> Self {
        Self {
            search: create_shared_search(),
        }
    }

    pub fn search(&self) -> SharedSearch {
        self.search.clone()
    }
}

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(ExplorerModule::new())
}

impl unidtk_api::Module for ExplorerModule {
    fn id(&self) -> &'static str {
        "explorer"
    }

    fn dependencies(&self) -> &'static [unidtk_api::manifest::ModuleDependency] {
        &[]
    }

    fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        if ctx.is_diagnostic() {
            let _ = get_user_places();
            return Ok(());
        }
        if !ctx.enabled() {
            unidtk_api::log("explorer: disabled — search engine stays empty");
        }
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        Ok(())
    }
}

pub async fn scan_directory(path: String) -> Result<Vec<FileItem>, String> {
    crate::fs::scan_directory_async(&path).await
}

pub async fn scan_directory_full(path: String) -> Result<Vec<FileItem>, String> {
    crate::fs::scan_directory_full_async(&path).await
}

pub async fn get_drives() -> Result<Vec<crate::drives::DriveInfo>, String> {
    // Volume labels and capacity calls can block for a long time on stale
    // mapped/network drives.  Never run that Windows I/O on Tauri's main
    // thread: the frontend has a short timeout and can fall back to raw drive
    // letters while this worker finishes.
    tauri::async_runtime::spawn_blocking(crate::drives::get_all_drives)
        .await
        .map_err(|error| format!("drive scan worker failed: {error}"))?
}

/// Known user places for the explorer sidebar (Desktop, Downloads, …).
/// Resolved on the Rust side so the UI does not depend on path-plugin ACLs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserPlace {
    pub name: String,
    pub path: String,
    pub kind: String,
}

pub fn get_user_places() -> Result<Vec<UserPlace>, String> {
    let mut places = Vec::new();
    let push = |places: &mut Vec<UserPlace>, name: &str, kind: &str, path: Option<std::path::PathBuf>| {
        if let Some(p) = path {
            let s = p.to_string_lossy().to_string();
            if !s.is_empty() {
                places.push(UserPlace {
                    name: name.to_string(),
                    path: s,
                    kind: kind.to_string(),
                });
            }
        }
    };

    push(&mut places, "Desktop", "folder", dirs::desktop_dir());
    push(&mut places, "Downloads", "folder", dirs::download_dir());
    push(&mut places, "Documents", "folder", dirs::document_dir());
    push(&mut places, "Pictures", "folder", dirs::picture_dir());
    push(&mut places, "Music", "folder", dirs::audio_dir());
    push(&mut places, "Videos", "folder", dirs::video_dir());

    // Recycle Bin — open as a special place when possible (per-user).
    if let Some(home) = dirs::home_dir() {
        // Windows: the shell CLSID is not a normal path; point at the drive bin root
        // for C: which is the usual entry users expect to browse.
        let recycle = PathBuf::from("C:\\$Recycle.Bin");
        if recycle.exists() {
            places.push(UserPlace {
                name: "Recycle Bin".to_string(),
                path: recycle.to_string_lossy().to_string(),
                kind: "recycle".to_string(),
            });
        } else {
            let _ = home;
        }
    }

    Ok(places)
}

/// True when `path` is a directory (follows the final path metadata).
pub fn path_is_directory(path: String) -> Result<bool, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Ok(false);
    }
    Ok(p.is_dir())
}

pub fn get_raw_drives() -> Result<Vec<String>, String> {
    crate::drives::get_raw_drive_letters()
}

pub fn get_unmounted_volumes() -> Result<Vec<String>, String> {
    crate::drives::get_unmounted_volumes()
}

pub fn is_drive_removable(drive: String) -> Result<bool, String> {
    Ok(crate::drives::is_removable_drive(&drive))
}

pub fn get_portable_devices() -> Result<Vec<String>, String> {
    crate::portable::get_portable_devices()
}

pub fn get_portable_contents(device_id: String) -> Result<Vec<String>, String> {
    crate::portable::get_device_contents(&device_id)
}

pub fn copy_to_clipboard(paths: Vec<String>, cut: bool) -> Result<(), String> {
    let path_bufs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    crate::clipboard::copy_files_to_clipboard(&path_bufs, cut)
}

pub fn paste_from_clipboard() -> Result<Vec<String>, String> {
    crate::clipboard::paste_files_from_clipboard()
        .map(|v| v.iter().map(|p| p.to_string_lossy().to_string()).collect())
}

pub fn clear_clipboard() -> Result<(), String> {
    crate::clipboard::clear_clipboard();
    Ok(())
}

pub fn file_exists(path: String) -> Result<bool, String> {
    Ok(Path::new(&path).exists())
}

pub fn get_file_info(path: String) -> Result<crate::files::FileInfo, String> {
    crate::files::get_file_info(&path)
}

pub fn format_file_size(bytes: u64) -> String {
    crate::files::format_size(bytes)
}

pub fn get_file_type_name(extension: String) -> String {
    crate::files::get_file_type_name(&extension).to_string()
}

pub fn is_valid_filename(name: String) -> Result<(), String> {
    crate::files::is_valid_filename(&name)
}

pub fn delete_to_recycle_bin(path: String) -> Result<(), String> {
    crate::files::delete_to_recycle_bin(&path)
}

pub fn open_path_default(path: String) -> Result<(), String> {
    crate::files::open_with_default(&path)
}

pub fn create_directory(path: String) -> Result<(), String> {
    crate::files::create_directory(&path)
}

pub fn create_empty_file(path: String) -> Result<(), String> {
    crate::files::create_empty_file(&path)
}

pub fn rename_path(from: String, to: String) -> Result<(), String> {
    crate::files::rename_path(&from, &to)
}

pub fn copy_path(from: String, to: String) -> Result<(), String> {
    crate::files::copy_path(&from, &to)
}

pub fn open_terminal(path: String) -> Result<(), String> {
    let raw_path = path.trim();
    let target = if raw_path.is_empty() || raw_path == "::MY_PC::" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("C:\\"))
    } else {
        let p = PathBuf::from(raw_path);
        if p.is_file() {
            p.parent().map(|parent| parent.to_path_buf()).unwrap_or(p)
        } else {
            p
        }
    };

    let target_str = target.to_string_lossy().to_string();

    // 1. Try Windows Terminal (wt.exe -d <target>)
    if std::process::Command::new("wt")
        .args(["-d", &target_str])
        .spawn()
        .is_ok()
    {
        return Ok(());
    }

    // 2. Try PowerShell
    if std::process::Command::new("powershell")
        .args(["-NoExit", "-Command", &format!("Set-Location -LiteralPath '{}'", target_str.replace('\'', "''"))])
        .current_dir(&target)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }

    // 3. Fallback to Command Prompt
    std::process::Command::new("cmd")
        .args(["/k", "cd", "/d", &target_str])
        .current_dir(&target)
        .spawn()
        .map_err(|e| format!("Failed to start terminal: {e}"))?;

    Ok(())
}



pub fn get_folder_size(path: String) -> Result<u64, String> {
    Ok(crate::fs::calculate_folder_size(Path::new(&path)))
}

pub fn sort_files(
    files: Vec<FileItem>,
    column: String,
    ascending: bool,
) -> Result<Vec<FileItem>, String> {
    let sort_col = match column.as_str() {
        "name" => crate::sorting::SortColumn::Name,
        "size" => crate::sorting::SortColumn::Size,
        "modified" => crate::sorting::SortColumn::Modified,
        "created" => crate::sorting::SortColumn::Created,
        "type" => crate::sorting::SortColumn::Type,
        _ => crate::sorting::SortColumn::Name,
    };
    Ok(crate::sorting::sort_files(files, sort_col, ascending))
}

pub fn search_files(
    search: &SharedSearch,
    query: String,
) -> Vec<crate::search::SearchResult> {
    let engine = search.lock().unwrap();
    engine.search(&query)
}

pub fn add_to_search_index(
    search: &SharedSearch,
    path: String,
    is_dir: bool,
) {
    let mut engine = search.lock().unwrap();
    engine.add_path(&path, is_dir, None, None);
}

pub fn remove_from_search_index(search: &SharedSearch, path: String) {
    let mut engine = search.lock().unwrap();
    engine.remove_path(&path);
}

pub fn clear_search_index(search: &SharedSearch) {
    let mut engine = search.lock().unwrap();
    engine.clear();
}

pub fn get_search_suggestions(
    search: &SharedSearch,
    prefix: String,
    limit: usize,
) -> Vec<String> {
    let engine = search.lock().unwrap();
    engine.get_suggestions(&prefix, limit)
}

pub fn hash_file(path: String) -> Result<hashing::FileHash, String> {
    hashing::hash_file(Path::new(&path))
}

pub fn hash_file_md5(path: String) -> Result<String, String> {
    hashing::hash_file_md5(Path::new(&path))
}

pub fn hash_file_sha256(path: String) -> Result<String, String> {
    hashing::hash_file_sha256(Path::new(&path))
}

pub fn get_archive_info(path: String) -> Result<archive::ArchiveInfo, String> {
    archive::open_archive(Path::new(&path))
}

pub fn extract_archive(archive_path: String, dest_dir: String) -> Result<Vec<String>, String> {
    archive::extract_archive(Path::new(&archive_path), Path::new(&dest_dir))
}

pub fn extract_archive_entry(
    archive_path: String,
    entry_name: String,
    dest_dir: String,
) -> Result<String, String> {
    let result = archive::extract_single(
        Path::new(&archive_path),
        &entry_name,
        Path::new(&dest_dir),
    )?;
    Ok(result.to_string_lossy().to_string())
}

pub fn create_zip_archive(files: Vec<String>, archive_path: String) -> Result<(), String> {
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    archive::create_archive(&paths, Path::new(&archive_path))
}

pub fn is_zip_file(path: String) -> bool {
    archive::is_zip_file(Path::new(&path))
}

pub fn get_file_preview(path: String) -> preview::FilePreview {
    preview::get_preview_info(Path::new(&path))
}

pub fn get_text_preview(path: String, max_bytes: usize) -> Option<String> {
    preview::get_text_preview(Path::new(&path), max_bytes)
}

pub fn get_file_icon(extension: String) -> String {
    preview::get_file_icon(&extension).to_string()
}
