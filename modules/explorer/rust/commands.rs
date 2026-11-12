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

/// Explorer module: the search index, and its slice of the database, where the
/// open tabs and the sort live. Its commands read both from here.
pub struct ExplorerModule {
    search: SharedSearch,
    store: std::sync::OnceLock<Arc<dyn unidtk_api::ModuleDb>>,
}

impl ExplorerModule {
    pub fn new() -> Self {
        Self {
            search: create_shared_search(),
            store: std::sync::OnceLock::new(),
        }
    }

    fn store(&self) -> Result<&Arc<dyn unidtk_api::ModuleDb>, String> {
        self.store.get().ok_or_else(|| "explorer is not ready".to_string())
    }

    /// The open tabs as `(path, title)` pairs; none stored is no tabs.
    fn tabs(&self) -> Result<Vec<(String, String)>, String> {
        match self.store()?.kv_get(KEY_TABS)? {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("explorer tabs: {e}")),
            None => Ok(Vec::new()),
        }
    }

    fn set_tabs(&self, tabs: &[(String, String)]) -> Result<(), String> {
        let bytes = serde_json::to_vec(tabs).map_err(|e| e.to_string())?;
        self.store()?.kv_set(KEY_TABS, &bytes)
    }

    /// The sort column and direction. A missing or unreadable value is the
    /// default, by name ascending, as it always was.
    fn sort(&self) -> Result<(String, bool), String> {
        let stored = self.store()?.kv_get(KEY_SORT)?;
        Ok(stored
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| ("name".to_string(), true)))
    }

    fn set_sort(&self, column: &str, ascending: bool) -> Result<(), String> {
        let column = match column {
            "name" | "size" | "type" | "modified" => column,
            _ => "name",
        };
        let bytes = serde_json::to_vec(&(column, ascending)).map_err(|e| e.to_string())?;
        self.store()?.kv_set(KEY_SORT, &bytes)
    }
}

/// Keys in the module's slice; the kernel moved the old values here once
/// (`core/kernel/explorer_migration.rs`), so the names must stay.
const KEY_TABS: &str = "tabs";
const KEY_SORT: &str = "sort";

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
        // The tabs and sort are readable while the module is switched off too.
        let _ = self.store.set(ctx.db());
        if !ctx.enabled() {
            unidtk_api::log("explorer: disabled — search engine stays empty");
        }
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        Ok(())
    }

    fn commands(&self) -> &'static [unidtk_api::commands::CommandSpec] {
        Self::COMMANDS
    }

    fn dispatch(&self, cmd: &str, args: &serde_json::Value) -> Option<Result<serde_json::Value, String>> {
        self.dispatch_command(cmd, args)
    }

    /// Win+E opens the overlay, but only on the desktop or over the shell: with a
    /// browser in front, Windows opens its own Explorer.
    fn hotkeys(&self) -> &'static [unidtk_api::HotkeySpec] {
        &[unidtk_api::HotkeySpec {
            id: "win_e",
            name: "Shell Explorer",
            description: "Open in-shell file manager overlay",
            key: "Win+E",
            vk_code: 0x45, // VK_E
            win: true,
            ctrl: false,
            alt: false,
            shift: false,
            swallow: true,
            desktop_only: true,
            event: "explorer-open",
            shows_desktop: true,
        }]
    }
}

// The answers keep the shapes of the Tauri commands of the same names.

/// Runs an async command body on the blocking thread the kernel dispatches on.
fn block<F: std::future::Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

unidtk_api::module_commands! {
    impl ExplorerModule {
        read_ui "explorer_tabs_get" "Open Explorer tabs" |this| this.tabs();
        write "explorer_tabs_set" "Store the open Explorer tabs" |this, tabs: Vec<(String, String)>| this.set_tabs(&tabs);
        read_only "explorer_sort_get" "Explorer sort column and direction" |this| this.sort();
        write "explorer_sort_set" "Set the Explorer sort column and direction" |this, column: String, ascending: bool| this.set_sort(&column, ascending);
        read_ui "scan_directory" "Entries of a folder" |this, path: String| block(crate::commands::scan_directory(path));
        read_ui "scan_directory_full" "Entries of a folder with sizes and details" |this, path: String| block(crate::commands::scan_directory_full(path));
        read_only "get_drives" "Mounted drives" |this| block(crate::commands::get_drives());
        read_only "get_user_places" "User shell folders" |this| crate::commands::get_user_places();
        read_ui "path_is_directory" "Whether a path is a folder" |this, path: String| crate::commands::path_is_directory(path);
        read_ui "get_raw_drives" "Drives without a file system" |this| crate::commands::get_raw_drives();
        read_ui "get_unmounted_volumes" "Volumes without a drive letter" |this| crate::commands::get_unmounted_volumes();
        read_ui "is_drive_removable" "Whether a drive is removable" |this, drive: String| crate::commands::is_drive_removable(drive);
        read_ui "get_portable_devices" "Portable devices" |this| crate::commands::get_portable_devices();
        read_ui "get_portable_contents" "Contents of a portable device" |this, device_id: String| crate::commands::get_portable_contents(device_id);
        write "copy_to_clipboard" "Put paths on the clipboard, to copy or cut" |this, paths: Vec<String>, cut: bool| crate::commands::copy_to_clipboard(paths, cut);
        read_ui "paste_from_clipboard" "Paths on the clipboard" |this| crate::commands::paste_from_clipboard();
        write "clear_clipboard" "Empty the clipboard" |this| crate::commands::clear_clipboard();
        read_ui "file_exists" "Whether a path exists" |this, path: String| crate::commands::file_exists(path);
        read_ui "get_file_info" "Size, dates and attributes of a path" |this, path: String| crate::commands::get_file_info(path);
        read_ui "format_file_size" "A byte count for people" |this, bytes: u64| Ok(crate::commands::format_file_size(bytes));
        read_ui "get_file_type_name" "The Windows name of a file type" |this, extension: String| Ok(crate::commands::get_file_type_name(extension));
        read_ui "is_valid_filename" "Whether a file name is allowed" |this, name: String| crate::commands::is_valid_filename(name);
        write "delete_to_recycle_bin" "Move a path to the Recycle Bin" |this, path: String| crate::commands::delete_to_recycle_bin(path);
        read_ui "get_folder_size" "Total size of a folder" |this, path: String| crate::commands::get_folder_size(path);
        read_ui "sort_files" "Sort entries by a column" |this, files: Vec<crate::fs::FileItem>, column: String, ascending: bool| crate::commands::sort_files(files, column, ascending);
        read_ui "search_files" "Search the index" |this, query: String| Ok(crate::commands::search_files(&this.search, query));
        write "build_search_index" "Index a folder for search" |this, root: String| { crate::commands::build_search_index(&this.search, root); Ok(()) };
        write "remove_from_search_index" "Drop a path from the search index" |this, path: String| { crate::commands::remove_from_search_index(&this.search, path); Ok(()) };
        write "clear_search_index" "Empty the search index" |this| { crate::commands::clear_search_index(&this.search); Ok(()) };
        read_ui "get_search_suggestions" "Names that start with a prefix" |this, prefix: String, limit: usize| Ok(crate::commands::get_search_suggestions(&this.search, prefix, limit));
        read_ui "hash_file" "MD5 and SHA-256 of a file" |this, path: String| crate::commands::hash_file(path);
        read_ui "hash_file_md5" "MD5 of a file" |this, path: String| crate::commands::hash_file_md5(path);
        read_ui "hash_file_sha256" "SHA-256 of a file" |this, path: String| crate::commands::hash_file_sha256(path);
        read_ui "get_archive_info" "Entries of an archive" |this, path: String| crate::commands::get_archive_info(path);
        write "extract_archive" "Extract an archive into a folder" |this, archive_path: String, dest_dir: String| crate::commands::extract_archive(archive_path, dest_dir);
        write "extract_archive_entry" "Extract one entry of an archive" |this, archive_path: String, entry_name: String, dest_dir: String| crate::commands::extract_archive_entry(archive_path, entry_name, dest_dir);
        write "create_zip_archive" "Pack files into a ZIP archive" |this, files: Vec<String>, archive_path: String| crate::commands::create_zip_archive(files, archive_path);
        read_ui "is_zip_file" "Whether a file is a ZIP archive" |this, path: String| Ok(crate::commands::is_zip_file(path));
        read_ui "get_file_preview" "Preview of a file" |this, path: String| Ok(crate::commands::get_file_preview(path));
        read_ui "get_text_preview" "The first bytes of a text file" |this, path: String, max_bytes: usize| Ok(crate::commands::get_text_preview(path, max_bytes));
        read_ui "get_file_icon" "Icon of a file type" |this, extension: String| Ok(crate::commands::get_file_icon(extension));
        write "open_path_default" "Open a path with its default application" |this, path: String| crate::commands::open_path_default(path);
        write "create_directory" "Create a folder" |this, path: String| crate::commands::create_directory(path);
        write "create_empty_file" "Create an empty file" |this, path: String| crate::commands::create_empty_file(path);
        write "rename_path" "Rename a path" |this, from: String, to: String| crate::commands::rename_path(from, to);
        write "copy_path" "Copy a path" |this, from: String, to: String| crate::commands::copy_path(from, to);
        write "open_terminal" "Open a terminal in a folder" |this, path: String| crate::commands::open_terminal(path);
        write "setup_registry_override" "Make UniDTK the default file manager" |this| crate::registry::install_as_default_file_manager();
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
                // The name Explorer shows, in the user's language; the English
                // name only when the shell does not answer.
                let name = crate::drives::shell_display_name(&s).unwrap_or_else(|| name.to_string());
                places.push(UserPlace {
                    name,
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
                name: crate::drives::shell_display_name(crate::drives::RECYCLE_BIN)
                    .unwrap_or_else(|| "Recycle Bin".to_string()),
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
    let mut engine = search.lock().unwrap();
    engine.search_checked(&query)
}

/// Replaces the index with the tree under `root`. The UI calls this when search
/// opens for a folder, so the index only ever covers one folder at a time.
pub fn build_search_index(search: &SharedSearch, root: String) {
    let mut engine = search.lock().unwrap();
    engine.clear();
    engine.index_tree(Path::new(&root), crate::search::SEARCH_MAX_DEPTH);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn module_with_store() -> ExplorerModule {
        let module = ExplorerModule::new();
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        assert!(module.store.set(store).is_ok());
        module
    }

    #[test]
    fn tabs_and_sort_round_trip_through_the_commands() {
        let module = module_with_store();
        let call = |cmd: &str, args: serde_json::Value| module.dispatch_command(cmd, &args).unwrap().unwrap();
        assert_eq!(call("explorer_tabs_get", json!({})), json!([]));
        assert_eq!(call("explorer_sort_get", json!({})), json!(["name", true]));

        call("explorer_tabs_set", json!({ "tabs": [["C:/Music", "Music"]] }));
        assert_eq!(call("explorer_tabs_get", json!({})), json!([["C:/Music", "Music"]]));
        call("explorer_sort_set", json!({ "column": "modified", "ascending": false }));
        assert_eq!(call("explorer_sort_get", json!({})), json!(["modified", false]));
        call("explorer_sort_set", json!({ "column": "nonsense", "ascending": true }));
        assert_eq!(call("explorer_sort_get", json!({})), json!(["name", true]));
    }

    #[test]
    fn the_commands_wait_for_init() {
        let module = ExplorerModule::new();
        let reply = module.dispatch_command("explorer_tabs_get", &json!({})).unwrap();
        assert_eq!(reply, Err("explorer is not ready".to_string()));
    }
}
