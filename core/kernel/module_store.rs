//! Отдельные файловые хранилища модулей — реализация [`unidtk_api::StoreFactory`].
//!
//! Зачем модулю свой файл: redb держит **одного писателя на файл**. Модуль,
//! который пишет помногу и подолгу (плеер сохраняет эмбеддинги отдельной
//! транзакцией на каждый трек), в общей базе выстроил бы в очередь запись UI —
//! сохранение позиций виджетов, вкладок проводника, правок плейлистов. Свой файл
//! даёт свою очередь записи и свою надёжность коммита.
//!
//! Ядро владеет и путём, и хендлом: модуль объявляет [`StoreSpec`] и получает
//! обычный [`unidtk_api::ModuleDb`] — того же вида, что и общий срез. Поэтому
//! модулю не нужно знать, где физически лежат его ключи, и переезд каталогов
//! (Program Files / AppData) не задевает ни один модуль.
//!
//! Раскладка внутри каталога данных:
//!   `mods/<slug>/<name>.redb`  — данные пользователя (ACID, попадают в бэкап)
//!   `cache/<slug>/<name>.redb` — производные данные (удаляемы, без fsync)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use redb::{Database, Durability, ReadableTable, TableDefinition};
use unidtk_api::{ModuleDb, ModuleId, StoreFactory, StoreInfo, StoreKind, StoreSpec};

use crate::module_db::{RedbModuleDb, MODULE_KV};

/// Уже выданное хранилище — для [`StoreFactory::list`].
struct Opened {
    module: String,
    name: String,
    kind: StoreKind,
    path: PathBuf,
}

/// Фабрика файловых хранилищ поверх redb.
pub struct RedbStoreFactory {
    /// Каталог данных (`%LOCALAPPDATA%\UniDTK`), под ним — `mods/` и `cache/`.
    root: PathBuf,
    /// redb лочит файл эксклюзивно, поэтому на каждый путь — один общий хендл.
    /// Раньше этот кэш был вынужден жить в самом модуле (AI DJ), теперь он тут.
    handles: Mutex<HashMap<PathBuf, Arc<Database>>>,
    opened: Mutex<Vec<Opened>>,
}

impl RedbStoreFactory {
    pub fn new() -> Self {
        Self::with_root(unidtk_api::paths::data_dir())
    }

    /// Свой корень вместо каталога данных — для тестов.
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

    /// Забрать прежний файл хранилища на новое место. Ищем и в каталоге данных,
    /// и в legacy-каталоге `Shell` рядом с ним: именно там лежали старые базы.
    fn adopt_legacy(&self, target: &Path, spec: &StoreSpec) {
        if target.exists() {
            return;
        }
        let legacy_root = self.root.parent().map(|p| p.join("Shell"));
        for name in &spec.legacy_files {
            let mut candidates = vec![self.root.join(name)];
            if let Some(shell) = legacy_root.as_ref() {
                candidates.push(shell.join(name));
            }
            for old in candidates {
                if !old.is_file() {
                    continue;
                }
                if let Some(parent) = target.parent() {
                    if std::fs::create_dir_all(parent).is_err() {
                        return;
                    }
                }
                // Один том → rename; иначе копируем и убираем исходник.
                if std::fs::rename(&old, target).is_err() {
                    if std::fs::copy(&old, target).is_err() {
                        continue;
                    }
                    let _ = std::fs::remove_file(&old);
                }
                return;
            }
        }
    }

    /// Развернуть прежние таблицы модуля в плоские ключи хранилища.
    ///
    /// Пока модуль держал данные в собственных redb-таблицах, схема была его
    /// личным делом; контракт даёт одно пространство ключей, поэтому старую
    /// раскладку надо один раз переписать с префиксами. Делает это ядро, потому
    /// что redb знает только оно, а имена таблиц и префиксы объявляет модуль
    /// через [`StoreSpec::legacy_tables`].
    ///
    /// Идемпотентно: после переезда таблицы больше нет, второй вызов не находит
    /// ничего. Поэтому вызывается на каждом открытии, а не «один раз по флагу».
    fn flatten_legacy_tables(db: &Database, tables: &[(&'static str, String)]) -> Vec<String> {
        let mut moved = Vec::new();
        if tables.is_empty() {
            return moved;
        }
        let mut txn = match db.begin_write() {
            Ok(txn) => txn,
            Err(_) => return moved,
        };
        // Разовая операция: надёжность важнее скорости.
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
                // Таблицы нет — переезжать нечему.
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

        // Прежняя раскладка модуля (собственные таблицы) → плоские ключи.
        for line in Self::flatten_legacy_tables(&db, &spec.legacy_tables) {
            unidtk_api::log(format!(
                "store {}/{}: migrated legacy {}",
                id.storage_slug(),
                spec.name,
                line
            ));
        }

        // Надёжность — политика ядра по объявленному виду: производные данные
        // пересобираемы, поэтому платить fsync за каждую пачку незачем.
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

        // Прежнее имя в каталоге данных — как у shell_ai_dj.redb.
        let legacy = root.join("shell_ai_dj.redb");
        {
            let db = Database::create(&legacy).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut table = txn.open_table(crate::module_db::MODULE_KV).unwrap();
                table.insert("emb:kept", b"kept".as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }

        let spec = StoreSpec::cache("ai_dj").legacy(&["shell_ai_dj.redb"]);
        let store = factory.open(&id, &spec).unwrap();

        assert!(root.join("cache").join("mp3_player").join("ai_dj.redb").is_file());
        assert!(!legacy.exists(), "legacy file must be moved, not copied");
        // Переезд не должен терять данные.
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

        // Прежняя раскладка модуля: две его собственные таблицы в своём файле.
        // Путь — тот же, что выделит фабрика (`cache/<slug>/<name>.redb`).
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

        // Старые строки доступны уже как префиксованные ключи.
        assert_eq!(store.kv_get("emb:t1").unwrap().as_deref(), Some(&b"vec1"[..]));
        assert_eq!(store.kv_get("ana:t1").unwrap().as_deref(), Some(&b"json"[..]));
        assert_eq!(store.kv_count("emb:").unwrap(), 1);

        // Идемпотентность: повторное открытие ничего не портит.
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
