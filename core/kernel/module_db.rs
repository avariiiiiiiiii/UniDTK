//! Implementation of [`unidtk_api::ModuleDb`] on top of the shared UniDTK database.
//!
//! A module receives neither `raw_db()` nor the right to invent table names in the shared
//! namespace: the core hands it a [`RedbModuleDb`], which itself prepends the
//! `mods/<id>/` prefix to every key. Everything a module writes physically lives in
//! its own key slice, and no other module can reach it.
//!
//! The first real consumer is the mp3-player library: the core gives it the slice
//! `mods/mp3_player/…` via [`unidtk_api::ModuleContext`], and the migration of the player's
//! old tables is done by `library_migration.rs`.

use std::sync::Arc;

use redb::{Database, Durability, ReadableTableMetadata, TableDefinition};
use unidtk_api::{DbOp, ModuleDb, ModuleId};

/// Key slice of all modules: `mods/<slug>/<key>` → bytes.
pub(crate) const MODULE_KV: TableDefinition<&str, &[u8]> = TableDefinition::new("module_kv");

/// Prefix that the core prepends to a module's keys.
pub(crate) fn scope_prefix(id: &ModuleId) -> String {
    format!("mods/{}/", id.storage_slug())
}

/// Storage of a single module.
///
/// One and the same implementation serves two cases: a slice of the core's shared database
/// (`scope = "mods/<id>/"`) and a module's separate file (`scope` empty, in which case all
/// keys in the file belong to the module).
pub struct RedbModuleDb {
    db: Arc<Database>,
    scope: String,
    durability: Durability,
}

impl RedbModuleDb {
    /// Create a key slice for module `id` in database `db`.
    pub fn new(db: Arc<Database>, id: &ModuleId) -> Self {
        Self {
            db,
            scope: scope_prefix(id),
            // The shared database holds user data: commits are synchronous by default.
            durability: Durability::Immediate,
        }
    }

    /// The whole store in a separate file: there is no prefix, and all keys are its own.
    /// The core picks durability from the declared [`unidtk_api::StoreKind`].
    pub fn dedicated(db: Arc<Database>, durability: Durability) -> Self {
        Self {
            db,
            scope: String::new(),
            durability,
        }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.scope, key)
    }
}

impl ModuleDb for RedbModuleDb {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
        match table.get(self.full_key(key).as_str()).map_err(|e| e.to_string())? {
            Some(value) => Ok(Some(value.value().to_vec())),
            None => Ok(None),
        }
    }

    fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let mut txn = self.db.begin_write().map_err(|e| e.to_string())?;
        txn.set_durability(self.durability);
        {
            let mut table = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
            table
                .insert(self.full_key(key).as_str(), value)
                .map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn kv_remove(&self, key: &str) -> Result<(), String> {
        let mut txn = self.db.begin_write().map_err(|e| e.to_string())?;
        txn.set_durability(self.durability);
        {
            let mut table = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
            table
                .remove(self.full_key(key).as_str())
                .map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = match txn.open_table(MODULE_KV) {
            Ok(table) => table,
            // The table is created lazily, on first write: until then the list is empty.
            Err(_) => return Ok(Vec::new()),
        };
        let full_prefix = self.full_key(prefix);
        let mut out = Vec::new();
        for entry in table.range::<&str>(full_prefix.as_str()..).map_err(|e| e.to_string())? {
            let (key, value) = entry.map_err(|e| e.to_string())?;
            let key = key.value();
            if !key.starts_with(&full_prefix) {
                break;
            }
            // The key is returned without the module prefix, but WITH the query prefix;
            // otherwise the caller could not reuse the received key in
            // `kv_remove`/`kv_batch`, and that is exactly how playlist links are cleaned up.
            out.push((key[self.scope.len()..].to_string(), value.value().to_vec()));
        }
        Ok(out)
    }

    fn kv_batch(&self, ops: Vec<DbOp>) -> Result<(), String> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut txn = self.db.begin_write().map_err(|e| e.to_string())?;
        txn.set_durability(self.durability);
        {
            let mut table = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
            for op in ops {
                match op {
                    DbOp::Set { key, value } => {
                        let physical = self.full_key(&key);
                        table
                            .insert(physical.as_str(), value.as_slice())
                            .map_err(|e| e.to_string())?;
                    }
                    DbOp::Remove { key } => {
                        let physical = self.full_key(&key);
                        table.remove(physical.as_str()).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn kv_count(&self, prefix: &str) -> Result<u64, String> {
        // Whole module file: the counter is read from the B-tree, without a scan.
        // This is not possible in the shared slice: len() would count the keys of all modules.
        if self.scope.is_empty() && prefix.is_empty() {
            let txn = self.db.begin_read().map_err(|e| e.to_string())?;
            let table = match txn.open_table(MODULE_KV) {
                Ok(table) => table,
                Err(_) => return Ok(0),
            };
            return table.len().map_err(|e| e.to_string());
        }
        Ok(self.kv_list(prefix)?.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_db(name: &str) -> (Arc<Database>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("unidtk_moddb_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.redb");
        let db = Database::create(&path).unwrap();
        (Arc::new(db), dir)
    }

    #[test]
    fn modules_are_isolated_from_each_other() {
        let (db, dir) = scratch_db("isolation");
        let vpn = RedbModuleDb::new(db.clone(), &ModuleId::new("vpn"));
        let telegram = RedbModuleDb::new(db.clone(), &ModuleId::new("telegram"));

        vpn.kv_set("last_tag", b"Poland").unwrap();
        telegram.kv_set("last_tag", b"Germany").unwrap();

        assert_eq!(vpn.kv_get("last_tag").unwrap().as_deref(), Some(&b"Poland"[..]));
        assert_eq!(
            telegram.kv_get("last_tag").unwrap().as_deref(),
            Some(&b"Germany"[..])
        );
        // The list only sees the module's own keys.
        let listed = vpn.kv_list("").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "last_tag");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_filters_by_prefix_and_strips_it() {
        let (db, dir) = scratch_db("list");
        let db_for_mod = RedbModuleDb::new(db, &ModuleId::new("mp3-player"));
        db_for_mod.kv_set("cache/a", b"1").unwrap();
        db_for_mod.kv_set("cache/b", b"2").unwrap();
        db_for_mod.kv_set("other", b"3").unwrap();

        let keys: Vec<String> = db_for_mod
            .kv_list("cache/")
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        // The module's internal prefix is stripped, the requested one is kept: the returned
        // key must be usable for `kv_remove` without re-attaching the prefix.
        assert_eq!(keys, vec!["cache/a".to_string(), "cache/b".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slug_makes_hyphenated_ids_usable() {
        let (db, dir) = scratch_db("slug");
        let db_for_mod = RedbModuleDb::new(db, &ModuleId::new("mp3-player"));
        db_for_mod.kv_set("k", b"v").unwrap();
        assert_eq!(db_for_mod.kv_get("k").unwrap().as_deref(), Some(&b"v"[..]));
        assert_eq!(db_for_mod.kv_remove("k").unwrap(), ());
        assert!(db_for_mod.kv_get("k").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_sets_and_removes_are_applied_in_one_scope() {
        let (db, dir) = scratch_db("batch");
        let scoped = RedbModuleDb::new(db, &ModuleId::new("mp3-player"));
        scoped.kv_set("keep", b"1").unwrap();
        scoped.kv_set("gone", b"x").unwrap();

        scoped
            .kv_batch(vec![
                DbOp::Set {
                    key: "a".into(),
                    value: b"2".to_vec(),
                },
                DbOp::Set {
                    key: "keep".into(),
                    value: b"3".to_vec(),
                },
                DbOp::Remove { key: "gone".into() },
            ])
            .unwrap();

        assert_eq!(scoped.kv_get("a").unwrap().as_deref(), Some(&b"2"[..]));
        assert_eq!(scoped.kv_get("keep").unwrap().as_deref(), Some(&b"3"[..]));
        assert!(scoped.kv_get("gone").unwrap().is_none());
        let keys: Vec<String> = scoped
            .kv_list("")
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, vec!["a".to_string(), "keep".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
