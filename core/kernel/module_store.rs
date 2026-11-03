//! Separate file stores of modules: the implementation of [`unidtk_api::StoreFactory`].
//!
//! Why a module gets its own file: redb keeps **one writer per file**. A module
//! that writes a lot and for long stretches (the player saves embeddings in a separate
//! transaction for each track) would, in the shared database, queue up UI writes: saving widget
//! positions, explorer tabs, playlist edits. Its own file gives it its own write queue and
//! its own commit durability.
//!
//! The core owns both the path and the handle: a module declares a [`StoreSpec`] and receives an
//! ordinary [`unidtk_api::ModuleDb`], the same kind as the shared slice. So the module
//! does not need to know where its keys physically live, and moving the directories
//! (Program Files / AppData) affects no module.
//!
//! Layout inside the data directory:
//!   `mods/<slug>/<name>.redb`  — user data (ACID, included in backups)
//!   `cache/<slug>/<name>.redb` — derived data (deletable, no fsync)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use redb::{Database, Durability, ReadableTable, TableDefinition};
use unidtk_api::{ModuleDb, ModuleId, StoreFactory, StoreInfo, StoreKind, StoreSpec};

use crate::module_db::{RedbModuleDb, MODULE_KV};

/// Store already issued — for [`StoreFactory::list`].
struct Opened {
    module: String,
    name: String,
    kind: StoreKind,
    path: PathBuf,
}

/// Factory of file stores on top of redb.
pub struct RedbStoreFactory {
    /// Data directory (`%LOCALAPPDATA%\UniDTK`); under it are `mods/` and `cache/`.
    root: PathBuf,
    /// redb locks the file exclusively, so each path gets one shared handle.
    /// This cache used to have to live in the module itself (AI DJ); now it lives here.
    handles: Mutex<HashMap<PathBuf, Arc<Database>>>,
    opened: Mutex<Vec<Opened>>,
}

impl RedbStoreFactory {
    pub fn new() -> Self {
        Self::with_root(unidtk_api::paths::data_dir())
    }

    /// Own root instead of the data directory, for tests.
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            handles: Mutex::new(HashMap::new()),
            opened: Mutex::new(Vec::new()),
        }
    }

    fn store_path(&self, id: &ModuleId, spec: &StoreSpec) -> PathBuf {
        let branch = match spec.kind {
            StoreKind::OwnedCache => "cache",
            _ => "mods",
        };
        self.root
            .join(branch)
            .join(id.storage_slug())
            .join(format!("{}.redb", spec.name))
    }

    /// Move a previous store file from the data directory to its new location.
    fn adopt_legacy(&self, target: &Path, spec: &StoreSpec) {
        if target.exists() {
            return;
        }
        for name in &spec.legacy_files {
            let old = self.root.join(name);
            if !old.is_file() {
                continue;
            }
            if let Some(parent) = target.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    return;
                }
            }
            // Same volume → rename; otherwise copy and remove the source.
            if std::fs::rename(&old, target).is_err() {
                if std::fs::copy(&old, target).is_err() {
                    continue;
                }
                let _ = std::fs::remove_file(&old);
            }
            return;
        }
    }

    /// Expand a module's previous tables into flat store keys.
    ///
    /// While the module kept its data in its own redb tables, the schema was its
    /// private business; the contract provides one key namespace, so the old
    /// layout must be rewritten once with prefixes. The core does this, because only
    /// it knows redb, while the table names and prefixes are declared by the module
    /// via [`StoreSpec::legacy_tables`].
    ///
    /// Idempotent: after the move the table no longer exists, so a second call finds
    /// nothing. Therefore it is called on every open, not "once per flag".
    fn flatten_legacy_tables(db: &Database, tables: &[(&'static str, String)]) -> Vec<String> {
        let mut moved = Vec::new();
        if tables.is_empty() {
            return moved;
        }
        let mut txn = match db.begin_write() {
            Ok(txn) => txn,
            Err(_) => return moved,
        };
        // One-off operation: durability matters more than speed.
        txn.set_durability(Durability::Immediate);
        for (table_name, prefix) in tables {
            let def: TableDefinition<&str, &[u8]> = TableDefinition::new(table_name);
            let rows: Vec<(String, Vec<u8>)> = match txn.open_table(def) {
                Ok(table) => table
                    .iter()
                    .map(|iter| {
                        iter.filter_map(|row| {
                            row.ok()
                                .map(|(key, value)| (key.value().to_string(), value.value().to_vec()))
                        })
                        .collect()
                    })
                    .unwrap_or_default(),
                // No table: there is nothing to migrate.
                Err(_) => continue,
            };
            if rows.is_empty() {
                continue;
            }
            {
                let mut kv = match txn.open_table(MODULE_KV) {
                    Ok(kv) => kv,
                    Err(_) => continue,
                };
                for (key, value) in &rows {
                    let _ = kv.insert(format!("{prefix}{key}").as_str(), value.as_slice());
                }
            }
            let _ = txn.delete_table(def);
            moved.push(format!("{table_name} → {prefix}* ({} keys)", rows.len()));
        }
        if let Err(error) = txn.commit() {
            unidtk_api::log(format!("module_store: legacy table migration failed: {error}"));
            return Vec::new();
        }
        moved
    }

    fn handle(&self, path: &Path) -> Result<Arc<Database>, String> {
        let mut guard = self.handles.lock().map_err(|e| e.to_string())?;
        if let Some(db) = guard.get(path) {
            return Ok(Arc::clone(db));
        }
        let db = if path.exists() {
            Database::open(path).map_err(|e| format!("open {}: {e}", path.display()))?
        } else {
            Database::create(path).map_err(|e| format!("create {}: {e}", path.display()))?
        };
        let db = Arc::new(db);
        guard.insert(path.to_path_buf(), Arc::clone(&db));
        Ok(db)
    }
}

impl Default for RedbStoreFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl StoreFactory for RedbStoreFactory {
    fn open(&self, id: &ModuleId, spec: &StoreSpec) -> Result<Arc<dyn ModuleDb>, String> {
        if spec.kind == StoreKind::Shared {
            return Err("Shared is served by ModuleContext::db(), not by the factory".to_string());
        }
        let path = self.store_path(id, spec);
        self.adopt_legacy(&path, spec);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let db = self.handle(&path)?;

        // The module's previous layout (its own tables) → flat keys.
        for line in Self::flatten_legacy_tables(&db, &spec.legacy_tables) {
            unidtk_api::log(format!(
                "store {}/{}: migrated legacy {}",
                id.storage_slug(),
                spec.name,
                line
            ));
        }

        // Durability is the core's policy for the declared kind: derived data
        // can be rebuilt, so paying an fsync for every batch is pointless.
        let durability = match spec.kind {
            StoreKind::OwnedCache => Durability::Eventual,
            _ => Durability::Immediate,
        };

        {
            let mut opened = self.opened.lock().map_err(|e| e.to_string())?;
            let known = opened
                .iter()
                .any(|o| o.module == id.storage_slug() && o.name == spec.name);
            if !known {
                opened.push(Opened {
                    module: id.storage_slug(),
                    name: spec.name.clone(),
                    kind: spec.kind,
                    path: path.clone(),
                });
            }
        }

        Ok(Arc::new(RedbModuleDb::dedicated(db, durability)))
    }

    fn list(&self, id: &ModuleId) -> Vec<StoreInfo> {
        let slug = id.storage_slug();
        self.opened
            .lock()
            .map(|opened| {
                opened
                    .iter()
                    .filter(|o| o.module == slug)
                    .map(|o| StoreInfo {
                        name: o.name.clone(),
                        kind: o.kind,
                        path: Some(o.path.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factory(root: &Path) -> RedbStoreFactory {
        let _ = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(root).unwrap();
        RedbStoreFactory::with_root(root.to_path_buf())
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("unidtk_store_{name}_{}", std::process::id()))
    }

    #[test]
    fn cache_store_lives_under_cache_branch_and_roundtrips() {
        let root = scratch("cache");
        let factory = factory(&root);
        let id = ModuleId::new("mp3-player");

        let store = factory
            .open(&id, &StoreSpec::cache("ai_dj"))
            .expect("open cache store");
        store.kv_set("emb:t1", b"one").unwrap();
        store.kv_set("emb:t2", b"two").unwrap();
        store.kv_batch(vec![unidtk_api::DbOp::Remove {
            key: "emb:t1".into(),
        }])
        .unwrap();

        assert_eq!(store.kv_count("emb:").unwrap(), 1);
        assert_eq!(store.kv_get("emb:t2").unwrap().as_deref(), Some(&b"two"[..]));

        let expected = root.join("cache").join("mp3_player").join("ai_dj.redb");
        assert!(expected.is_file(), "store file at {}", expected.display());
        assert_eq!(factory.list(&id).len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn durable_store_lives_under_mods_branch() {
        let root = scratch("durable");
        let factory = factory(&root);
        let id = ModuleId::new("vpn");
        factory
            .open(&id, &StoreSpec::durable("history"))
            .expect("open durable store");
        assert!(root.join("mods").join("vpn").join("history.redb").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_file_is_adopted_on_first_open() {
        let root = scratch("legacy");
        let factory = factory(&root);
        let id = ModuleId::new("mp3-player");

        // The previous name in the data directory, the same as unidtk_ai_dj.redb.
        let legacy = root.join("unidtk_ai_dj.redb");
        {
            let db = Database::create(&legacy).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut table = txn.open_table(crate::module_db::MODULE_KV).unwrap();
                table.insert("emb:kept", b"kept".as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }

        let spec = StoreSpec::cache("ai_dj").legacy(&["unidtk_ai_dj.redb"]);
        let store = factory.open(&id, &spec).unwrap();

        assert!(root.join("cache").join("mp3_player").join("ai_dj.redb").is_file());
        assert!(!legacy.exists(), "legacy file must be moved, not copied");
        // The move must not lose data.
        assert_eq!(
            store.kv_get("emb:kept").unwrap().as_deref(),
            Some(&b"kept"[..])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_tables_are_flattened_into_prefixed_keys() {
        let root = scratch("flatten");
        let factory = factory(&root);
        let id = ModuleId::new("mp3-player");

        // The module's previous layout: its two own tables in their own file.
        // The path is the same one the factory allocates (`cache/<slug>/<name>.redb`).
        let path = root.join("cache").join("mp3_player").join("ai_dj.redb");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let db = Database::create(&path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let emb: TableDefinition<&str, &[u8]> = TableDefinition::new("embeddings");
                let mut table = txn.open_table(emb).unwrap();
                table.insert("t1", b"vec1".as_slice()).unwrap();
            }
            {
                let ana: TableDefinition<&str, &[u8]> = TableDefinition::new("analyses");
                let mut table = txn.open_table(ana).unwrap();
                table.insert("t1", b"json".as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }

        let spec = StoreSpec::cache("ai_dj")
            .legacy_tables(&[("embeddings", "emb:"), ("analyses", "ana:")]);
        let store = factory.open(&id, &spec).unwrap();

        // The old rows are now available as prefixed keys.
        assert_eq!(store.kv_get("emb:t1").unwrap().as_deref(), Some(&b"vec1"[..]));
        assert_eq!(store.kv_get("ana:t1").unwrap().as_deref(), Some(&b"json"[..]));
        assert_eq!(store.kv_count("emb:").unwrap(), 1);

        // Idempotency: reopening does not break anything.
        drop(store);
        let again = factory.open(&id, &spec).unwrap();
        assert_eq!(again.kv_count("emb:").unwrap(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shared_kind_is_rejected_by_the_factory() {
        let root = scratch("shared");
        let factory = factory(&root);
        let id = ModuleId::new("vpn");
        let err = factory
            .open(&id, &StoreSpec::new("x", StoreKind::Shared))
            .err()
            .expect("Shared must not go through the factory");
        assert!(err.contains("Shared"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
