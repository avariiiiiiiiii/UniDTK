//! ACID user-data store (`unidtk.redb`) — UniDTK's own redb database.
//!
//! Lives in the core: the core owns the store, modules only receive a handle
//! to it. What the core itself persists here:
//! - `widget_settings` — the widget layout/config JSON (frontend reads and
//!   writes it through the `get_widget_config` / `save_widget_config` commands);
//! - `explorer_tabs` — the explorer's open tabs.
//!
//! The music library lives in the same file inside the mp3-player key slice of
//! the module KV table (`module_db.rs`); the kernel moves the old `tracks` /
//! `playlists` tables there once (`library_migration.rs`) and never hands the
//! raw engine to a module. Logs live in the separate
//! [`crate::log_store::LogStore`] (`unidtk_logs.redb`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

// --- Widgets ---
const WIDGET_SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("widget_settings");
const EXPLORER_TABS: TableDefinition<&str, &[u8]> = TableDefinition::new("explorer_tabs");
const FOLDERS: TableDefinition<&str, &[u8]> = TableDefinition::new("folders");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderItem {
    pub id: String,
    pub name: String,
    pub target: String,
    pub kind: String, // "url", "file", "folder"
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderRecord {
    pub id: String,
    pub name: String,
    pub icon_svg: String,
    #[serde(default)]
    pub items: Vec<FolderItem>,
}

/// The core's own database.
pub struct UniDtkDatabase {
    db: Arc<Database>,
}

impl UniDtkDatabase {
    /// Open or create the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
        }

        // Automatic legacy migration if canonical file doesn't exist
        if !path.exists() && path == Self::default_path() {
            let legacy_candidates = [
                path.with_file_name("shell.redb"),
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Shell")
                    .join("shell.redb"),
            ];
            for old_path in &legacy_candidates {
                if old_path.exists() {
                    let _ = std::fs::copy(old_path, path);
                    break;
                }
            }
        }

        let db = if path.exists() {
            Database::open(path).map_err(|e| format!("Failed to open redb: {e}"))?
        } else {
            Database::create(path).map_err(|e| format!("Failed to create redb: {e}"))?
        };

        let txn = db.begin_write().map_err(|e| format!("Failed to begin write: {e}"))?;
        {
            let _ = txn.open_table(WIDGET_SETTINGS).map_err(|e| e.to_string())?;
            let _ = txn.open_table(EXPLORER_TABS).map_err(|e| e.to_string())?;
            let _ = txn.open_table(FOLDERS).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| format!("Failed to commit: {e}"))?;

        Ok(Self { db: Arc::new(db) })
    }

    /// Canonical path: `%LOCALAPPDATA%\UniDTK\unidtk.redb`.
    pub fn default_path() -> PathBuf {
        let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("UniDTK").join("unidtk.redb")
    }

    /// Raw engine handle for kernel-side plumbing only ([`crate::module_db`]
    /// and the one-shot [`crate::library_migration`]). Never given to modules.
    pub fn raw_db(&self) -> Arc<Database> {
        self.db.clone()
    }

    // ─── Widget config ───────────────────────────────────────────────────────

    pub fn get_widget_config(&self) -> Result<Option<serde_json::Value>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(WIDGET_SETTINGS).map_err(|e| e.to_string())?;
        match table.get("config").map_err(|e| e.to_string())? {
            Some(value) => {
                let bytes = value.value();
                let val: serde_json::Value = serde_json::from_slice(bytes)
                    .map_err(|e| format!("Failed to deserialize widget config: {e}"))?;
                Ok(Some(val))
            }
            None => Ok(None),
        }
    }

    pub fn set_widget_config(&self, config: &serde_json::Value) -> Result<(), String> {
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = txn.open_table(WIDGET_SETTINGS).map_err(|e| e.to_string())?;
            let bytes = serde_json::to_vec(config).map_err(|e| e.to_string())?;
            table.insert("config", bytes.as_slice()).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ─── Explorer tabs ───────────────────────────────────────────────────────

    pub fn get_explorer_tabs(&self) -> Result<Vec<(String, String)>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(EXPLORER_TABS).map_err(|e| e.to_string())?;
        match table.get("tabs").map_err(|e| e.to_string())? {
            Some(value) => {
                let bytes = value.value();
                bincode::deserialize(bytes).map_err(|e| format!("Failed to deserialize tabs: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    pub fn set_explorer_tabs(&self, tabs: &[(String, String)]) -> Result<(), String> {
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = txn.open_table(EXPLORER_TABS).map_err(|e| e.to_string())?;
            let bytes = bincode::serialize(tabs).map_err(|e| e.to_string())?;
            table.insert("tabs", bytes.as_slice()).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ─── Explorer sorting ─────────────────────────────────────────────────────

    pub fn get_explorer_sort(&self) -> Result<(String, bool), String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(EXPLORER_TABS).map_err(|e| e.to_string())?;
        match table.get("sort").map_err(|e| e.to_string())? {
            Some(value) => {
                let bytes = value.value();
                match bincode::deserialize(bytes) {
                    Ok(val) => Ok(val),
                    Err(e) => {
                        eprintln!("WARN: Failed to deserialize sort, falling back to default: {e}");
                        Ok(("name".to_string(), true))
                    }
                }
            }
            None => Ok(("name".to_string(), true)),
        }
    }

    pub fn set_explorer_sort(&self, column: &str, ascending: bool) -> Result<(), String> {
        let safe_column = match column {
            "name" | "size" | "type" | "modified" => column,
            _ => "name",
        };
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = txn.open_table(EXPLORER_TABS).map_err(|e| e.to_string())?;
            let bytes = bincode::serialize(&(safe_column.to_string(), ascending)).map_err(|e| e.to_string())?;
            table.insert("sort", bytes.as_slice()).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ─── Disabled modules ─────────────────────────────────────────────────────
    // A separate key in `widget_settings` (not part of the whole layout JSON
    // blob), so writing the layout never clobbers the disabled list.

    pub fn get_disabled_modules(&self) -> Result<Vec<String>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(WIDGET_SETTINGS).map_err(|e| e.to_string())?;
        match table.get("disabled_modules").map_err(|e| e.to_string())? {
            Some(value) => {
                let bytes = value.value();
                bincode::deserialize(bytes)
                    .map_err(|e| format!("Failed to deserialize disabled modules: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    pub fn set_disabled_modules(&self, ids: &[String]) -> Result<(), String> {
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = txn.open_table(WIDGET_SETTINGS).map_err(|e| e.to_string())?;
            let bytes = bincode::serialize(ids).map_err(|e| e.to_string())?;
            table
                .insert("disabled_modules", bytes.as_slice())
                .map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ─── Folders ─────────────────────────────────────────────────────────────

    pub fn list_folders(&self) -> Result<Vec<FolderRecord>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(FOLDERS).map_err(|e| e.to_string())?;
        let mut result = Vec::new();
        for item in table.iter().map_err(|e| e.to_string())? {
            let (_key, val) = item.map_err(|e| e.to_string())?;
            if let Ok(record) = serde_json::from_slice::<FolderRecord>(val.value()) {
                result.push(record);
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    #[allow(dead_code)]
    pub fn get_folder(&self, id: &str) -> Result<Option<FolderRecord>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(FOLDERS).map_err(|e| e.to_string())?;
        match table.get(id).map_err(|e| e.to_string())? {
            Some(val) => {
                let rec = serde_json::from_slice(val.value()).map_err(|e| e.to_string())?;
                Ok(Some(rec))
            }
            None => Ok(None),
        }
    }

    pub fn save_folder(&self, folder: &FolderRecord) -> Result<(), String> {
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = txn.open_table(FOLDERS).map_err(|e| e.to_string())?;
            let bytes = serde_json::to_vec(folder).map_err(|e| e.to_string())?;
            table.insert(folder.id.as_str(), bytes.as_slice()).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_folder(&self, id: &str) -> Result<bool, String> {
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        let existed = {
            let mut table = txn.open_table(FOLDERS).map_err(|e| e.to_string())?;
            let res = table.remove(id).map_err(|e| e.to_string())?;
            res.is_some()
        };
        txn.commit().map_err(|e| e.to_string())?;
        Ok(existed)
    }

    /// Point shortcut targets that still live under the pre-rename data root
    /// (`%LOCALAPPDATA%\Shell\folders`) at the same file under the current one.
    /// Only targets whose file exists at the new root move, so nothing is
    /// pointed at a missing file. Returns how many targets were rewritten.
    pub fn rebase_legacy_shortcut_targets(&self, old_root: &Path, new_root: &Path) -> Result<usize, String> {
        let old_prefix = format!("{}{}", old_root.display(), std::path::MAIN_SEPARATOR).to_lowercase();
        let mut moved = 0;
        for mut folder in self.list_folders()? {
            let mut changed = false;
            for item in &mut folder.items {
                // Paths on Windows compare case-insensitively.
                let Some(rest) = item.target.get(old_prefix.len()..) else { continue };
                if item.target[..old_prefix.len()].to_lowercase() != old_prefix {
                    continue;
                }
                let rebased = new_root.join(rest);
                if rebased.exists() {
                    item.target = rebased.display().to_string();
                    changed = true;
                    moved += 1;
                }
            }
            if changed {
                self.save_folder(&folder)?;
            }
        }
        Ok(moved)
    }

    /// Auto-migrate legacy folders from a directory on disk if redb `folders` table is empty.
    pub fn auto_migrate_disk_folders(&self, folders_dir: &Path) -> Result<usize, String> {
        if !folders_dir.is_dir() {
            return Ok(0);
        }
        let current = self.list_folders()?;
        if !current.is_empty() {
            return Ok(0);
        }

        let mut count = 0;
        let read_dir = std::fs::read_dir(folders_dir).map_err(|e| e.to_string())?;
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let folder_id = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            let mut folder_name = folder_id.clone();
            let meta_path = path.join("metadata.json");
            if meta_path.is_file() {
                if let Ok(data) = std::fs::read_to_string(&meta_path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                        if let Some(n) = val.get("name").and_then(|v| v.as_str()) {
                            folder_name = n.to_string();
                        }
                    }
                }
            }

            let mut icon_svg = String::new();
            let icon_path = path.join("icon.svg");
            if icon_path.is_file() {
                if let Ok(data) = std::fs::read_to_string(&icon_path) {
                    icon_svg = data;
                }
            }

            let shortcuts_path = path.join("shortcuts");
            let mut items = Vec::new();
            if shortcuts_path.is_dir() {
                if let Ok(shortcut_entries) = std::fs::read_dir(&shortcuts_path) {
                    for sentry in shortcut_entries.flatten() {
                        let spath = sentry.path();
                        let sname = match spath.file_name().and_then(|n| n.to_str()) {
                            Some(n) => n.to_string(),
                            None => continue,
                        };
                        if sname.starts_with('.') || sname.eq_ignore_ascii_case("readme.txt") {
                            continue;
                        }
                        let is_dir = spath.is_dir();
                        let ext = spath.extension().and_then(|e| e.to_str()).unwrap_or("");
                        let display_name = if is_dir {
                            sname.clone()
                        } else {
                            spath.file_stem().and_then(|s| s.to_str()).unwrap_or(&sname).to_string()
                        };
                        let kind = if is_dir {
                            "folder"
                        } else if ext.eq_ignore_ascii_case("url") {
                            "url"
                        } else {
                            "file"
                        }.to_string();

                        items.push(FolderItem {
                            id: sname,
                            name: display_name,
                            target: spath.to_string_lossy().to_string(),
                            kind,
                        });
                    }
                }
            }

            self.save_folder(&FolderRecord {
                id: folder_id,
                name: folder_name,
                icon_svg,
                items,
            })?;
            count += 1;
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_migrate_disk_folders() {
        let base_dir = std::env::temp_dir().join(format!("test_auto_migrate_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_dir);
        let disk_folders = base_dir.join("folders");
        let fld = disk_folders.join("my_apps");
        let sc = fld.join("shortcuts");
        std::fs::create_dir_all(&sc).unwrap();

        std::fs::write(fld.join("metadata.json"), r#"{"name":"My Apps"}"#).unwrap();
        std::fs::write(fld.join("icon.svg"), "<svg>icon</svg>").unwrap();
        std::fs::write(sc.join("app.lnk"), "dummy lnk").unwrap();
        std::fs::write(sc.join("readme.txt"), "skip me").unwrap();

        let db = UniDtkDatabase::open(base_dir.join("test.redb")).unwrap();
        let migrated = db.auto_migrate_disk_folders(&disk_folders).unwrap();
        assert_eq!(migrated, 1);

        let folders = db.list_folders().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "my_apps");
        assert_eq!(folders[0].name, "My Apps");
        assert_eq!(folders[0].items.len(), 1);
        assert_eq!(folders[0].items[0].name, "app");

        // Calling again when not empty returns 0 without re-importing
        let second = db.auto_migrate_disk_folders(&disk_folders).unwrap();
        assert_eq!(second, 0);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn legacy_shortcut_targets_move_to_the_new_root() {
        let base = std::env::temp_dir().join(format!("test_rebase_shortcuts_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old_root = base.join("Shell").join("folders");
        let new_root = base.join("UniDTK").join("folders");
        std::fs::create_dir_all(new_root.join("games").join("shortcuts")).unwrap();
        std::fs::write(new_root.join("games").join("shortcuts").join("Steam.lnk"), "lnk").unwrap();

        let db = UniDtkDatabase::open(base.join("test.redb")).unwrap();
        let item = |id: &str, target: String| FolderItem {
            id: id.into(),
            name: id.into(),
            target,
            kind: "file".into(),
        };
        db.save_folder(&FolderRecord {
            id: "games".into(),
            name: "Игры".into(),
            icon_svg: String::new(),
            items: vec![
                item("steam", old_root.join("games").join("shortcuts").join("Steam.lnk").display().to_string()),
                // Gone from the new root: left alone rather than pointed at nothing.
                item("gone", old_root.join("games").join("shortcuts").join("Gone.lnk").display().to_string()),
                item("web", "https://example.com".into()),
            ],
        })
        .unwrap();

        assert_eq!(db.rebase_legacy_shortcut_targets(&old_root, &new_root).unwrap(), 1);
        let items = &db.list_folders().unwrap()[0].items;
        assert_eq!(
            items[0].target,
            new_root.join("games").join("shortcuts").join("Steam.lnk").display().to_string()
        );
        assert!(items[1].target.starts_with(&old_root.display().to_string()));
        assert_eq!(items[2].target, "https://example.com");
        // Idempotent.
        assert_eq!(db.rebase_legacy_shortcut_targets(&old_root, &new_root).unwrap(), 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_folders_crud() {
        let dir = std::env::temp_dir().join(format!("test_unidtk_db_folders_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let db = UniDtkDatabase::open(dir.join("test.redb")).unwrap();
        assert_eq!(db.list_folders().unwrap().len(), 0);

        let folder = FolderRecord {
            id: "test_fld".to_string(),
            name: "Work".to_string(),
            icon_svg: "<svg></svg>".to_string(),
            items: vec![FolderItem {
                id: "item1".to_string(),
                name: "Docs".to_string(),
                target: "https://docs.google.com".to_string(),
                kind: "url".to_string(),
            }],
        };

        db.save_folder(&folder).unwrap();
        assert_eq!(db.get_folder("test_fld").unwrap().unwrap(), folder);
        assert_eq!(db.list_folders().unwrap().len(), 1);

        assert!(db.delete_folder("test_fld").unwrap());
        assert!(db.get_folder("test_fld").unwrap().is_none());
        assert_eq!(db.list_folders().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_database() {
        let dir = std::env::temp_dir().join(format!("test_unidtk_db_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let db = UniDtkDatabase::open(dir.join("test.redb")).unwrap();

        let config = serde_json::json!({ "positions": { "clock": { "x": 100, "y": 200 } } });
        assert!(db.get_widget_config().unwrap().is_none());
        db.set_widget_config(&config).unwrap();
        assert_eq!(db.get_widget_config().unwrap().unwrap(), config);

        let tabs = vec![
            ("C:\\Music".to_string(), "Music".to_string()),
            ("C:\\Games".to_string(), "Games".to_string()),
        ];
        db.set_explorer_tabs(&tabs).unwrap();
        assert_eq!(db.get_explorer_tabs().unwrap(), tabs);

        assert_eq!(db.get_explorer_sort().unwrap(), ("name".to_string(), true));
        db.set_explorer_sort("modified", false).unwrap();
        assert_eq!(db.get_explorer_sort().unwrap(), ("modified".to_string(), false));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_data_and_log_handles_are_independent() {
        let dir = std::env::temp_dir().join(format!("test_unidtk_db_split_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let data = UniDtkDatabase::open(dir.join("unidtk.redb")).unwrap();
        let logs = crate::log_store::LogStore::open(dir.join("unidtk_logs.redb")).unwrap();

        let config = serde_json::json!({ "positions": {} });
        data.set_widget_config(&config).unwrap();
        logs.insert_batch(&[crate::log_store::LogEntry {
            id: 1,
            ts: 1,
            level: "INFO".to_string(),
            source: "backend".to_string(),
            module: "test".to_string(),
            message: "probe".to_string(),
            meta: None,
        }])
        .unwrap();

        assert_eq!(data.get_widget_config().unwrap().unwrap(), config);
        assert_eq!(logs.count(), 1);

        // The two stores must never share a file.
        assert_ne!(
            UniDtkDatabase::default_path(),
            crate::log_store::LogStore::default_path()
        );
        assert_eq!(
            UniDtkDatabase::default_path().file_name().unwrap(),
            "unidtk.redb"
        );
        assert_eq!(
            crate::log_store::LogStore::default_path().file_name().unwrap(),
            "unidtk_logs.redb"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
