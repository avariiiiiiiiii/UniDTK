//! Одноразовая миграция библиотеки mp3-player из старых таблиц `shell.redb`
//! (`tracks`, `playlists`, `playlist_tracks`) в срез контракта
//! `mods/mp3_player/…` (см. `module_db.rs`).
//!
//! Плеер больше не получает сырой движок базы, поэтому перенос делает ядро —
//! ровно один раз, на старте, до публикации стора и до бутстрапа музыки.
//! Данные, маркер схемы и удаление таблиц-источников разделены на транзакции
//! redb: запись среза + маркер — **одна** транзакция (обрыв не оставит
//! половинчатого состояния), удаление — вторая, только после сверки счётчиков.
//!
//! Перед открытием базы ядро делает страховочную копию файла
//! (`shell.redb.pre-module-kv.bak`) и никогда её не перезаписывает.

use redb::{Database, ReadableTable, TableDefinition};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use unidtk_api::ModuleId;

use crate::module_db::{scope_prefix, MODULE_KV};

const MODULE_ID: &str = "mp3-player";
const SCHEMA_KEY: &str = "schema";
/// Версия раскладки: 2 — библиотека живёт в срезе модуля.
const SCHEMA_VALUE: &[u8] = b"2";

const TRACK_PREFIX: &str = "track/";
const PLAYLIST_PREFIX: &str = "playlist/";
const LINK_PREFIX: &str = "pt/";

// Имена и типы старых таблиц один в один как в `modules/mp3-player/rust/library/db.rs`
// до перехода на контракт — иначе redb их не откроет.
const LEGACY_TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("tracks");
const LEGACY_PLAYLISTS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlists");
const LEGACY_PLAYLIST_TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlist_tracks");
// Индексы по added_at/last_played в контракт не переезжают: они только
// поддерживались при записи и никогда не читались.
const LEGACY_INDEX_ADDED_AT: TableDefinition<i64, &str> =
    TableDefinition::new("idx_tracks_added_at");
const LEGACY_INDEX_LAST_PLAYED: TableDefinition<i64, &str> =
    TableDefinition::new("idx_tracks_last_played");

pub struct MigrationReport {
    pub tracks: usize,
    pub playlists: usize,
    pub links: usize,
}

struct Legacy {
    tracks: Vec<(String, Vec<u8>)>,
    playlists: Vec<(String, Vec<u8>)>,
    /// Ключи уже в формате среза: `pt/<playlist_id>/<pos>`.
    links: Vec<(String, Vec<u8>)>,
}

/// Страховочная копия `shell.redb` перед первым запуском с module-kv.
/// Копия делается до открытия базы и никогда не перезаписывается.
pub fn backup_before_migration(db_path: &Path) -> Result<Option<PathBuf>, String> {
    if !db_path.is_file() {
        return Ok(None);
    }
    let backup = db_path.with_file_name(format!(
        "{}.pre-module-kv.bak",
        db_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shell.redb")
    ));
    if backup.exists() {
        return Ok(None);
    }
    match std::fs::copy(db_path, &backup) {
        Ok(_) => Ok(Some(backup)),
        Err(e) => Err(format!(
            "copy {} -> {}: {e}",
            db_path.display(),
            backup.display()
        )),
    }
}

/// `Ok(None)` — схема уже актуальна, миграция не нужна.
/// `Ok(Some(report))` — срез создан (данные перенесены или зафиксирована
/// пустая схема на чистой установке).
pub fn migrate_if_needed(db: &Arc<Database>) -> Result<Option<MigrationReport>, String> {
    let scope = scope_prefix(&ModuleId::new(MODULE_ID));
    if schema_is_current(db, &scope)? {
        return Ok(None);
    }

    let legacy = read_legacy(db)?;
    write_slice(db, &scope, &legacy)?;
    verify_slice(db, &scope, &legacy)?;
    drop_legacy_tables(db)?;

    Ok(Some(MigrationReport {
        tracks: legacy.tracks.len(),
        playlists: legacy.playlists.len(),
        links: legacy.links.len(),
    }))
}

fn schema_is_current(db: &Arc<Database>, scope: &str) -> Result<bool, String> {
    let txn = db.begin_read().map_err(|e| e.to_string())?;
    let table = match txn.open_table(MODULE_KV) {
        Ok(table) => table,
        // Таблица создаётся лениво: до первой записи схемы нет.
        Err(_) => return Ok(false),
    };
    let marker = format!("{scope}{SCHEMA_KEY}");
    Ok(table
        .get(marker.as_str())
        .map_err(|e| e.to_string())?
        .is_some())
}

fn read_legacy(db: &Arc<Database>) -> Result<Legacy, String> {
    let txn = db.begin_read().map_err(|e| e.to_string())?;

    let mut tracks = Vec::new();
    if let Ok(table) = txn.open_table(LEGACY_TRACKS) {
        for entry in table.iter().map_err(|e| e.to_string())? {
            let (key, value) = entry.map_err(|e| e.to_string())?;
            tracks.push((key.value().to_string(), value.value().to_vec()));
        }
    }

    let mut playlists = Vec::new();
    if let Ok(table) = txn.open_table(LEGACY_PLAYLISTS) {
        for entry in table.iter().map_err(|e| e.to_string())? {
            let (key, value) = entry.map_err(|e| e.to_string())?;
            playlists.push((key.value().to_string(), value.value().to_vec()));
        }
    }

    let mut links = Vec::new();
    if let Ok(table) = txn.open_table(LEGACY_PLAYLIST_TRACKS) {
        for entry in table.iter().map_err(|e| e.to_string())? {
            let (key, value) = entry.map_err(|e| e.to_string())?;
            // Legacy-ключ: `<playlist_id>_<pos:08>`; позиция — всегда после
            // последнего подчёркивания.
            if let Some((playlist_id, position)) = key.value().rsplit_once('_') {
                if position.len() >= 8 && position.bytes().all(|b| b.is_ascii_digit()) {
                    links.push((
                        format!("{LINK_PREFIX}{playlist_id}/{position}"),
                        value.value().to_vec(),
                    ));
                }
            }
        }
    }

    Ok(Legacy {
        tracks,
        playlists,
        links,
    })
}

/// Срез модуля и маркер схемы — одной транзакцией.
fn write_slice(db: &Arc<Database>, scope: &str, legacy: &Legacy) -> Result<(), String> {
    let txn = db.begin_write().map_err(|e| e.to_string())?;
    {
        let mut table = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
        for (prefix, items) in [
            (TRACK_PREFIX, &legacy.tracks),
            (PLAYLIST_PREFIX, &legacy.playlists),
            ("", &legacy.links),
        ] {
            for (suffix, bytes) in items {
                let key = if prefix.is_empty() {
                    format!("{scope}{suffix}")
                } else {
                    format!("{scope}{prefix}{suffix}")
                };
                table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(|e| e.to_string())?;
            }
        }
        let marker = format!("{scope}{SCHEMA_KEY}");
        table
            .insert(marker.as_str(), SCHEMA_VALUE)
            .map_err(|e| e.to_string())?;
    }
    txn.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// Сверка: в срезе должно оказаться ровно столько ключей, сколько перенесли.
fn verify_slice(db: &Arc<Database>, scope: &str, legacy: &Legacy) -> Result<(), String> {
    let txn = db.begin_read().map_err(|e| e.to_string())?;
    let table = txn.open_table(MODULE_KV).map_err(|e| e.to_string())?;
    for (prefix, expected) in [
        (TRACK_PREFIX, legacy.tracks.len()),
        (PLAYLIST_PREFIX, legacy.playlists.len()),
        (LINK_PREFIX, legacy.links.len()),
    ] {
        let full = format!("{scope}{prefix}");
        let mut count = 0usize;
        for entry in table.range::<&str>(full.as_str()..).map_err(|e| e.to_string())? {
            let (key, _) = entry.map_err(|e| e.to_string())?;
            if !key.value().starts_with(&full) {
                break;
            }
            count += 1;
        }
        if count != expected {
            return Err(format!(
                "count mismatch under '{prefix}': migrated {count}, expected {expected}"
            ));
        }
    }
    Ok(())
}

fn drop_legacy_tables(db: &Arc<Database>) -> Result<(), String> {
    let txn = db.begin_write().map_err(|e| e.to_string())?;
    txn.delete_table(LEGACY_TRACKS).map_err(|e| e.to_string())?;
    txn.delete_table(LEGACY_PLAYLISTS)
        .map_err(|e| e.to_string())?;
    txn.delete_table(LEGACY_PLAYLIST_TRACKS)
        .map_err(|e| e.to_string())?;
    txn.delete_table(LEGACY_INDEX_ADDED_AT)
        .map_err(|e| e.to_string())?;
    txn.delete_table(LEGACY_INDEX_LAST_PLAYED)
        .map_err(|e| e.to_string())?;
    txn.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_db(name: &str) -> (Arc<Database>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("shell_migrate_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shell.redb");
        let db = Database::create(&path).unwrap();
        (Arc::new(db), dir)
    }

    fn seed_legacy(db: &Arc<Database>) {
        let txn = db.begin_write().unwrap();
        {
            let mut tracks = txn.open_table(LEGACY_TRACKS).unwrap();
            tracks.insert("t1", b"track-one".as_slice()).unwrap();
            tracks.insert("t2", b"track-two".as_slice()).unwrap();

            let mut playlists = txn.open_table(LEGACY_PLAYLISTS).unwrap();
            playlists.insert("pl_1", b"playlist-one".as_slice()).unwrap();

            let mut links = txn.open_table(LEGACY_PLAYLIST_TRACKS).unwrap();
            links.insert("pl_1_00000000", b"t1".as_slice()).unwrap();
            links.insert("pl_1_00000001", b"t2".as_slice()).unwrap();

            let mut added = txn.open_table(LEGACY_INDEX_ADDED_AT).unwrap();
            added.insert(1000i64, "t1").unwrap();
        }
        txn.commit().unwrap();
    }

    #[test]
    fn migrates_legacy_tables_into_module_slice_and_drops_them() {
        let (db, dir) = scratch_db("happy");
        seed_legacy(&db);

        let report = migrate_if_needed(&db).unwrap().expect("migration expected");
        assert_eq!(report.tracks, 2);
        assert_eq!(report.playlists, 1);
        assert_eq!(report.links, 2);

        let txn = db.begin_read().unwrap();
        let slice = txn.open_table(MODULE_KV).unwrap();
        assert_eq!(
            slice.get("mods/mp3_player/track/t1").unwrap().unwrap().value(),
            b"track-one"
        );
        assert_eq!(
            slice
                .get("mods/mp3_player/playlist/pl_1")
                .unwrap()
                .unwrap()
                .value(),
            b"playlist-one"
        );
        assert_eq!(
            slice
                .get("mods/mp3_player/pt/pl_1/00000001")
                .unwrap()
                .unwrap()
                .value(),
            b"t2"
        );
        assert_eq!(
            slice.get("mods/mp3_player/schema").unwrap().unwrap().value(),
            b"2"
        );
        // Старые таблицы удалены.
        assert!(txn.open_table(LEGACY_TRACKS).is_err());
        assert!(txn.open_table(LEGACY_PLAYLIST_TRACKS).is_err());
        drop(txn);

        // Повторный запуск миграцию не повторяет.
        assert!(migrate_if_needed(&db).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_install_marks_schema_without_data() {
        let (db, dir) = scratch_db("fresh");
        let report = migrate_if_needed(&db).unwrap().expect("marker expected");
        assert_eq!(report.tracks, 0);
        assert_eq!(report.playlists, 0);
        assert_eq!(report.links, 0);

        let txn = db.begin_read().unwrap();
        let slice = txn.open_table(MODULE_KV).unwrap();
        assert_eq!(
            slice.get("mods/mp3_player/schema").unwrap().unwrap().value(),
            b"2"
        );
        drop(txn);
        assert!(migrate_if_needed(&db).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_is_created_once_and_never_overwritten() {
        let (db, dir) = scratch_db("backup");
        drop(db);
        let path = dir.join("shell.redb");

        let first = backup_before_migration(&path).unwrap().expect("backup");
        assert!(first.is_file());
        std::fs::write(&first, b"do-not-overwrite").unwrap();

        assert!(backup_before_migration(&path).unwrap().is_none());
        assert_eq!(std::fs::read(&first).unwrap(), b"do-not-overwrite");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
