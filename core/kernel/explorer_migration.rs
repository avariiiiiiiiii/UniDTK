//! One-time move of the Explorer's tabs and sort out of the kernel's table.
//!
//! They lived in `explorer_tabs` (`tabs` and `sort`, bincode) in `unidtk.redb`, read and
//! written by kernel commands. The Explorer module now keeps them itself, in its slice of
//! the module key-value table (`mods/explorer/tabs` and `mods/explorer/sort`, JSON), so
//! they are copied there once. The copy and its marker are written in one transaction:
//! an interruption leaves either nothing or everything. Like `library_migration.rs`, this
//! works on the raw database on purpose, which a module never gets
//! (r[storage.no-raw-db]); the old table stays as it was, for an older build.

use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition, TableHandle};

use crate::module_db::MODULE_KV;

const LEGACY_EXPLORER_TABS: TableDefinition<&str, &[u8]> = TableDefinition::new("explorer_tabs");
const MARKER: &str = "mods/explorer/migrated-from-kernel";

/// Copies the old tabs and sort into the Explorer's slice once. Returns whether this
/// call copied anything.
pub fn migrate_if_needed(db: &Arc<Database>) -> Result<bool, String> {
    let txn = db.begin_write().map_err(|e| e.to_string())?;
    let copied = {
        let mut kv = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
        if kv.get(MARKER).map_err(|e| e.to_string())?.is_some() {
            return Ok(false);
        }
        let mut copied = false;
        // Opened only when it exists: in a write transaction, opening creates it.
        let has_legacy = txn
            .list_tables()
            .map_err(|e| e.to_string())?
            .any(|table| table.name() == LEGACY_EXPLORER_TABS.name());
        if has_legacy {
            let legacy = txn.open_table(LEGACY_EXPLORER_TABS).map_err(|e| e.to_string())?;
            if let Some(bytes) = legacy.get("tabs").map_err(|e| e.to_string())? {
                let tabs: Vec<(String, String)> = bincode::deserialize(bytes.value())
                    .map_err(|e| format!("explorer tabs: {e}"))?;
                let json = serde_json::to_vec(&tabs).map_err(|e| e.to_string())?;
                kv.insert("mods/explorer/tabs", json.as_slice()).map_err(|e| e.to_string())?;
                copied = true;
            }
            // An unreadable sort fell back to the default before, so it is not copied.
            let sort = legacy
                .get("sort")
                .map_err(|e| e.to_string())?
                .and_then(|bytes| bincode::deserialize::<(String, bool)>(bytes.value()).ok());
            if let Some(sort) = sort {
                let json = serde_json::to_vec(&sort).map_err(|e| e.to_string())?;
                kv.insert("mods/explorer/sort", json.as_slice()).map_err(|e| e.to_string())?;
                copied = true;
            }
        }
        kv.insert(MARKER, b"1".as_slice()).map_err(|e| e.to_string())?;
        copied
    };
    txn.commit().map_err(|e| e.to_string())?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(db: &Database, key: &str) -> Option<serde_json::Value> {
        let txn = db.begin_read().unwrap();
        let table = txn.open_table(MODULE_KV).unwrap();
        let value = table.get(key).unwrap();
        value.map(|bytes| serde_json::from_slice(bytes.value()).unwrap())
    }

    #[test]
    fn old_tabs_and_sort_move_once_into_the_explorer_slice() {
        let dir = std::env::temp_dir().join(format!("unidtk-explorer-migration-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::create(dir.join("old.redb")).unwrap());
        let tabs = vec![("C:\\Music".to_string(), "Music".to_string())];
        {
            let txn = db.begin_write().unwrap();
            {
                let mut legacy = txn.open_table(LEGACY_EXPLORER_TABS).unwrap();
                legacy.insert("tabs", bincode::serialize(&tabs).unwrap().as_slice()).unwrap();
                legacy
                    .insert("sort", bincode::serialize(&("modified".to_string(), false)).unwrap().as_slice())
                    .unwrap();
            }
            txn.commit().unwrap();
        }

        assert!(migrate_if_needed(&db).unwrap(), "the first run copies");
        assert_eq!(kv(&db, "mods/explorer/tabs"), Some(serde_json::json!([["C:\\Music", "Music"]])));
        assert_eq!(kv(&db, "mods/explorer/sort"), Some(serde_json::json!(["modified", false])));
        assert!(!migrate_if_needed(&db).unwrap(), "the second run finds the marker");

        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_database_without_the_old_table_only_gets_the_marker() {
        let dir = std::env::temp_dir().join(format!("unidtk-explorer-migration-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::create(dir.join("new.redb")).unwrap());
        assert!(!migrate_if_needed(&db).unwrap());
        assert_eq!(kv(&db, "mods/explorer/tabs"), None);
        assert!(!migrate_if_needed(&db).unwrap());
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
