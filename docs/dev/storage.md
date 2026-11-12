# Storage in redb: How UniDTK Keeps Its Data, and the SQLite Migration

In this document you will learn how UniDTK stores its data in redb, the embedded key-value database. We start with the files on disk and the tables inside them. Then we open a database, read from it, and write to it, and we see what a transaction is and why each write ends in one commit. After that we look at the module storage that the core hands to the mp3 player, the one-time migration that moved the library out of its old tables, and the separate log file. The last part keeps the original SQLite-to-redb plan. It is marked as history, and each claim in it is checked against the tree.

All code excerpts come from `core/kernel/unidtk_db.rs`, `core/kernel/module_db.rs` and `core/kernel/library_migration.rs` unless the text says otherwise.

---

## 1. The Files on Disk

UniDTK uses several redb files. Each has one job.

| File | Location | Owner | What it holds |
|------|----------|-------|---------------|
| `unidtk.redb` | `%LOCALAPPDATA%\UniDTK` | the core (`core/kernel/unidtk_db.rs`) | widget layout, folders, and the `mods/<id>/` slices of the modules |
| `unidtk_logs.redb` | `%LOCALAPPDATA%\UniDTK` | the log store (`core/kernel/log_store.rs`) | system and frontend log records |
| `cache/mp3_player/ai_dj.redb` | the module's cache root | the mp3 player's AI DJ | the AI DJ store, declared as a cache store |

The core gives each file its own path, and its own test checks that the two core files never coincide:

```rust
// Filename: core/kernel/unidtk_db.rs (default_path, abridged)
/// Canonical path: `%LOCALAPPDATA%\UniDTK\unidtk.redb`.
pub fn default_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK").join("unidtk.redb")
}
```

The test `test_data_and_log_handles_are_independent` asserts that `UniDtkDatabase::default_path()` ends in `unidtk.redb` and that `LogStore::default_path()` ends in `unidtk_logs.redb`, and that the two paths differ.

The AI DJ store is not in `unidtk.redb`. Its location comes from a `StoreSpec::cache("ai_dj")` declaration in `core/kernel/module_store.rs`, and the expected path is `root/cache/mp3_player/ai_dj.redb` (line 277). The same spec lists `unidtk_ai_dj.redb` as a legacy name to look for (line 314).

---

## 2. What redb Gives Us

redb is an embedded key-value database written in pure Rust. It is linked into the program as a library, so there is no server process and no C library. The properties UniDTK depends on are these:

- **Typed tables.** A table is declared once with a key type and a value type. Here every table is `TableDefinition<&str, &[u8]>`: the key is a string and the value is a byte slice. The bytes are then decoded by the code that reads them.
- **Transactions.** Every read and every write happens inside a transaction. A read transaction sees a consistent view of the file. A write transaction changes the file only when it is committed.
- **One file per store.** A database is one file. UniDTK opens several of them, one per store, as section 1 shows.

No SQLite engine remains in the tree. The old SQLite code and its crates are gone (section 9.4).

---

## 3. The Tables in `unidtk.redb`

The core declares its own tables at the top of `unidtk_db.rs`:

```rust
// Filename: core/kernel/unidtk_db.rs (table declarations)
// --- Widgets ---
const WIDGET_SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("widget_settings");
const FOLDERS: TableDefinition<&str, &[u8]> = TableDefinition::new("folders");
```

Each table holds a set of named keys. The encoding of each value is fixed by the code that writes it:

| Table | Key | Value | Encoding | Written by |
|-------|-----|-------|----------|------------|
| `widget_settings` | `config` | the widget layout | JSON (`serde_json`) | `set_widget_config` |
| `widget_settings` | `disabled_modules` | list of module ids | bincode | `set_disabled_modules` |
| `folders` | the folder `id` | a folder record with its items | JSON (`serde_json`) | `save_folder` |
| `module_kv` | `mods/<slug>/<key>` | the module's bytes | opaque to the core | `RedbModuleDb` (section 6) |

The Explorer's open tabs and sort lived in a kernel table `explorer_tabs` (bincode). They are the Explorer module's now, as JSON under `mods/explorer/tabs` and `mods/explorer/sort`: `core/kernel/explorer_migration.rs` copied the old values there once, with its marker in the same transaction, and left the old table as it was.

The `module_kv` table is declared in `module_db.rs`, and it is not created at startup. The comment on its read path says so: the table is created lazily, at the first write, and until then a read sees an empty list.

The disabled list lives under its own key, next to the layout, and not inside the layout JSON. The code says why:

```rust
// Filename: core/kernel/unidtk_db.rs (disabled modules, abridged)
// A separate key in `widget_settings` (not part of the whole layout JSON
// blob), so writing the layout never clobbers the disabled list.
```

Each key is an independent value. A write to one key cannot change another one, because each write replaces only the value under its own key.

---

## 4. Opening the Database

`UniDtkDatabase::open` creates the directory, opens or creates the file, and makes sure the three core tables exist:

```rust
// Filename: core/kernel/unidtk_db.rs (open, abridged)
pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
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
```

Step by step:

1. **Find or create the file.** `Database::open` reads an existing file. `Database::create` makes a new one.
2. **Create the tables in one write transaction.** Opening a table for writing creates it if it does not exist. Doing all three in one transaction means the file never has some of the tables and not the others.
3. **Keep the handle.** The database is wrapped in an `Arc`, so the handle can be shared. Several threads can hold it, and each of them opens its own transactions.

The block `{ ... }` around the three `open_table` calls matters. The table handles borrow the transaction, and the borrows must end before `commit()` runs (section 5.3).

---

## 5. Reads and Writes

### 5.1 A Read Is a Snapshot

A read opens a read transaction, opens the table, and asks for a key:

```rust
// Filename: core/kernel/unidtk_db.rs (get_widget_config, abridged)
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
```

`begin_read` gives a view of the file as it was when the transaction started. A write that commits later does not change what this read sees. That is what "snapshot" means here. The function returns `Ok(None)` when the key is absent, so a missing layout is not an error.

Iterating works the same way. `list_folders` iterates over the whole `folders` table inside one read transaction, decodes each value with `serde_json`, and sorts the result by name.

### 5.2 A Write Is a Transaction

A write opens a write transaction, changes the tables, and commits. Here is the folder write:

```rust
// Filename: core/kernel/unidtk_db.rs (save_folder)
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
```

The steps are these:

1. `begin_write` starts a write transaction. redb allows one write transaction at a time (a property of the redb library), so this call waits for any other write to finish.
2. `open_table` gives a mutable table bound to that transaction.
3. `insert` puts the encoded bytes under the key. Nothing is visible to other readers yet.
4. The block ends, which drops the table handle and releases its borrow of the transaction.
5. `commit` makes the change permanent and visible. It takes the transaction by value, so the transaction cannot be used afterwards.

### 5.3 Why a Write Is One Commit

A commit is the unit of change. Everything inside one write transaction becomes visible together, or nothing does. A crash or an error before `commit` leaves the file as it was before the transaction began.

The core uses this in two ways.

- **One setter, one transaction.** `set_explorer_sort` stores the column and the direction together as one value under the key `sort`, so a reader can never see a column from one write and a direction from another. `set_widget_config` and the other setters follow the same pattern.
- **Several changes, one transaction.** When a change touches several keys, all of them go into one transaction. The module KV layer does this for batches (section 6.2), and the library migration does it for its data and its schema marker. The migration comment says why:

```rust
// Filename: core/kernel/library_migration.rs (module header, abridged)
//! Данные, маркер схемы и удаление таблиц-источников разделены на транзакции
//! redb: запись среза + маркер — **одна** транзакция (обрыв не оставит
//! половинчатого состояния), удаление — вторая, только после сверки счётчиков.
```

In English, the comment says: the data of the slice and the schema marker are **one** transaction, so an interruption leaves no half-written state. The deletion of the source tables is a second transaction, and it runs only after the counts are compared. The migration section (section 7) describes it.

### 5.4 Durability

Each write transaction also has a durability setting. `RedbModuleDb::new` chooses `Durability::Immediate` for the shared store. The comment there says that the shared database holds user data, so its commits are synchronous by default (`module_db.rs`, lines 42–43). The setting is applied on every write with `txn.set_durability(self.durability)`. A dedicated module file gets the level that the kernel picks from the module's declared store kind.

---

## 6. Module Storage: The `module_kv` Slice

Modules do not get the raw database. The core hands each module a `ModuleDb`, and the core adds the module's prefix to every key.

### 6.1 The Prefix

```rust
// Filename: core/kernel/module_db.rs (abridged)
/// Срез ключей всех модулей: `mods/<slug>/<key>` → байты.
pub(crate) const MODULE_KV: TableDefinition<&str, &[u8]> = TableDefinition::new("module_kv");

/// Префикс, который ядро добавляет к ключам модуля.
pub(crate) fn scope_prefix(id: &ModuleId) -> String {
    format!("mods/{}/", id.storage_slug())
}
```

All modules share one table, `module_kv`. The prefix `mods/<slug>/` keeps their keys apart. A module writes `track/42`, and the core stores it as `mods/mp3_player/track/42`. The module never names the table, and it cannot write outside its own prefix.

The same type serves two cases. In the shared file the scope is the prefix. In a dedicated file the scope is empty, and the whole file belongs to the module. The module-level comment says that a module receives neither `raw_db()` nor the right to choose table names in the shared space.

### 6.2 Listing and Batching

Listing a prefix is a range scan. The scan starts at the prefix and stops at the first key that no longer has it:

```rust
// Filename: core/kernel/module_db.rs (kv_list, abridged)
let full_prefix = self.full_key(prefix);
let mut out = Vec::new();
for entry in table.range::<&str>(full_prefix.as_str()..).map_err(|e| e.to_string())? {
    let (key, value) = entry.map_err(|e| e.to_string())?;
    let key = key.value();
    if !key.starts_with(&full_prefix) {
        break;
    }
    out.push((key[self.scope.len()..].to_string(), value.value().to_vec()));
}
```

Because keys are sorted, the keys with the same prefix sit next to each other. The scan reads only that range. The returned keys have the module's own prefix removed but keep the prefix that was asked for. That lets a caller pass a key it got from a list straight into `kv_remove` or `kv_batch`.

A batch applies several operations in one transaction:

```rust
// Filename: core/kernel/module_db.rs (kv_batch, abridged)
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
                DbOp::Set { key, value } => { /* insert under the prefixed key */ }
                DbOp::Remove { key } => { /* remove the prefixed key */ }
            }
        }
    }
    txn.commit().map_err(|e| e.to_string())?;
    Ok(())
}
```

Every operation in the batch is either committed or not. A playlist update that adds several links and removes others therefore never leaves a playlist with half its links.

---

## 7. The Library Migration

Before the module slice existed, the mp3 library lived in its own tables inside the database file (`unidtk.redb`, named `shell.redb` before the rename): `tracks`, `playlists` and `playlist_tracks`, plus two indexes. `library_migration.rs` moves that data into the module slice, once, at startup.

The migration declares the old tables with their old names and types, because redb refuses to open a table with a different definition:

```rust
// Filename: core/kernel/library_migration.rs (legacy tables, abridged)
// Имена и типы старых таблиц один в один как в `modules/mp3-player/rust/library/db.rs`
// до перехода на контракт — иначе redb их не откроет.
const LEGACY_TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("tracks");
const LEGACY_PLAYLISTS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlists");
const LEGACY_PLAYLIST_TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlist_tracks");
```

The module header lists the steps:

1. **Back up first.** Before the database is opened, the file is copied to `unidtk.redb.pre-module-kv.bak`. An existing backup is never overwritten (`backup_before_migration`).
2. **Write the data and the schema marker in one transaction.** The new keys use the prefixes `track/`, `playlist/` and `pt/` (for playlist links), and the marker `schema` is set to `2`. The marker says the library now lives in the module slice.
3. **Delete the source tables in a second transaction.** This runs only after the counts are checked, as the module comment states.

Because step 2 is one transaction, an interruption cannot leave the new data without its marker. Because step 3 is a separate transaction, a failure there does not undo the data written in step 2, and it leaves the source tables where they were.

---

## 8. The Log Store

Logs go to their own file, `unidtk_logs.redb`, so that a noisy log stream never competes with user data. The log store declares its own tables, `logs` (a `u64` id to bytes) and `log_counters`, and `insert_batch` commits once per batch (`core/kernel/log_store.rs`, line 105). A batch of log entries is therefore one transaction, the same rule as in section 5.3.

---

## 9. The Original Plan: SQLite to redb (Historical)

The rest of this document is the original plan for the move from SQLite. It is kept for reference. The parts that were built differ from the plan, and section 10 lists the differences.

### 9.1 Current State (of the Old Shell)

#### SQLite Schema
- **tracks** (~10K-100K rows): id, title, artist, album, duration_s, path, ext, filesize, thumbnail_path, added_at, last_played, play_count
- **playlists** (~100-1000 rows): id, name, source_url, last_sync, thumbnail_path, track_count, first_track_id
- **playlist_tracks** (many-to-many): playlist_id, track_id, position
- **tracks_fts**: FTS5 full-text search index

#### Dependencies
- `rusqlite` 0.32.1
- `r2d2` 0.8.10 (connection pooling)
- `r2d2_sqlite` 0.25.0

#### Size Estimate (10K tracks)
- tracks: ~5MB
- playlists: ~500KB
- playlist_tracks: ~2MB
- tracks_fts: ~3MB
- **Total: ~10-15MB**

### 9.2 The Planned redb Schema

The plan proposed these tables and index tables. Table names and types as written in the plan:

```rust
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

// Primary tables
const TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("tracks");
const PLAYLISTS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlists");
const PLAYLIST_TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlist_tracks");

// Secondary indexes (manually maintained)
const TRACKS_BY_TITLE: TableDefinition<&str, &str> = TableDefinition::new("idx_tracks_title");
const TRACKS_BY_ARTIST: TableDefinition<&str, &str> = TableDefinition::new("idx_tracks_artist");
const TRACKS_BY_LAST_PLAYED: TableDefinition<i64, &str> = TableDefinition::new("idx_tracks_last_played");
const TRACKS_BY_ADDED_AT: TableDefinition<i64, &str> = TableDefinition::new("idx_tracks_added_at");
const PLAYLIST_TRACKS_BY_POSITION: TableDefinition<&str, &str> = TableDefinition::new("idx_playlist_tracks_position");
```

The plan also proposed these data structures:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_s: Option<i64>,
    pub path: String,
    pub ext: Option<String>,
    pub filesize: Option<i64>,
    pub thumbnail_path: Option<String>,
    pub added_at: i64,
    pub last_played: Option<i64>,
    pub play_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub source_url: String,
    pub last_sync: Option<i64>,
    pub track_count: i64,
    pub thumbnail_path: Option<String>,
    pub first_track_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub playlist_id: String,
    pub track_id: String,
    pub position: i64,
}
```

> **Status:** not built in this form. The library that shipped stores its records under the prefixes `track/`, `playlist/` and `pt/` in the module slice (section 7), and it has no secondary index tables. The index tables above were not added.

### 9.3 Serialization

**Why bincode (as planned):**
- 3-5x smaller than JSON
- 10-50x faster serialization
- Deterministic output
- No schema needed

**Size comparison (single track), plan estimates:**
- JSON: ~300 bytes
- bincode: ~180 bytes

> **Status:** the plan's estimates were not measured again for this document. The encodings that the tree uses are listed in section 3, and they are mixed: JSON for the widget layout and the folders, bincode for the explorer state and the disabled list. The module slice stores the bytes that the module chooses.

### 9.4 The Migration Phases

#### Phase 1: Dual-Write (Backward Compatible)

**Goal:** Write to both SQLite and redb, read from SQLite.

**Steps:**
1. Add `redb` and `bincode` dependencies
2. Create `RedbDatabase` struct with same API as `Database`
3. On every write: write to SQLite AND redb
4. On every read: read from SQLite (source of truth)
5. Add migration status tracking

**Code changes:**

```rust
pub struct HybridDatabase {
    sqlite: Database,      // Existing SQLite
    redb: RedbDatabase,    // New redb
    migration_complete: bool,
}

impl HybridDatabase {
    pub fn new(config: &Config) -> Result<Self, String> {
        let sqlite = Database::new(config.database_path());
        sqlite.init()?;
        
        let redb = RedbDatabase::new(config.redb_path());
        redb.init()?;
        
        Ok(Self {
            sqlite,
            redb,
            migration_complete: false,
        })
    }
    
    pub fn upsert_track(&self, playlist_id: &str, position: i64, track: &DownloadedTrack) -> Result<(), String> {
        // Write to both
        self.sqlite.upsert_track(playlist_id, position, track)?;
        self.redb.upsert_track(playlist_id, position, track)?;
        Ok(())
    }
    
    pub fn list_tracks(&self, search: Option<String>) -> Result<Vec<Track>, String> {
        // Read from SQLite (source of truth)
        self.sqlite.list_tracks(search)
    }
}
```

**Timeline:** 2-3 days

---

#### Phase 2: Redb as Primary

**Goal:** Switch reads to redb, keep SQLite for validation.

**Steps:**
1. Implement all read operations in redb
2. Add consistency checks (compare SQLite vs redb on reads)
3. Log any discrepancies
4. Performance benchmarking

**Code changes:**

```rust
impl HybridDatabase {
    pub fn list_tracks(&self, search: Option<String>) -> Result<Vec<Track>, String> {
        // Read from redb (new source of truth)
        let redb_tracks = self.redb.list_tracks(search)?;
        
        // Optional: validate against SQLite in background
        if !self.migration_complete {
            self.validate_consistency("list_tracks", &redb_tracks);
        }
        
        Ok(redb_tracks)
    }
}
```

**Timeline:** 1-2 days

---

#### Phase 3: Remove SQLite (COMPLETED)

**Goal:** Full migration to redb.

**Status:** ✅ Fully completed and verified.
1. SQLite read/write code removed in favor of pure `redb` + `bincode`.
2. Removed `rusqlite`, `r2d2`, `r2d2_sqlite` dependencies from `Cargo.toml`.
3. All music, player, playlist, sync, log, widget, and AI DJ data are managed exclusively in `shell.redb`.

> **Correction (checked against the tree):** the status above says `shell.redb`. The current names are: the widget layout, explorer state and folders in `unidtk.redb`; the music library in the module slice of `unidtk.redb` (section 7); the logs in `unidtk_logs.redb` (section 8); the AI DJ in `cache/mp3_player/ai_dj.redb` (section 1). `shell.redb` is the legacy file that the migration reads.
>
> **Verified:** the status of the dependencies holds. A search of every `*.toml` and `*.rs` file outside `target/` finds no `rusqlite` and no `r2d2`.

---

### 9.5 Full Migration Timeline

| Phase | Duration | Status |
|---|---|---|
| 1: Unified redb schema | Completed | ✅ |
| 2: Redb primary | Completed | ✅ |
| 3: Remove SQLite | Completed | ✅ |

> **Status note:** Phases 1 and 2 happened in earlier history. The current tree shows only the final state (phase 3). They are not re-checked here.

---

### 9.6 Full-Text Search Replacement

#### Option A: Simple Prefix Matching (Recommended for now)

```rust
fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    
    let txn = self.db.begin_read().map_err(|e| e.to_string())?;
    let table = txn.open_table(TRACKS).map_err(|e| e.to_string())?;
    
    for entry in table.iter().map_err(|e| e.to_string())? {
        let (_, value) = entry.map_err(|e| e.to_string())?;
        let track: Track = bincode::deserialize(value).map_err(|e| e.to_string())?;
        
        if track.title.to_lowercase().contains(&query_lower)
            || track.artist.as_ref().map(|a| a.to_lowercase()).unwrap_or_default().contains(&query_lower)
            || track.album.as_ref().map(|a| a.to_lowercase()).unwrap_or_default().contains(&query_lower)
        {
            results.push(track);
        }
    }
    
    Ok(results)
}
```

> **Status:** the library implements a search of this kind, and the code does a **substring** match, not a prefix match. In `modules/mp3-player/rust/library/db.rs` (`list_tracks_sorted`), the code loads every track with `self.all::<Track>(TRACK_PREFIX)`, lowercases the query, and keeps a track when the query is contained in its title, artist or album. The whole library is read for each search. No index exists.

#### Option B: Tantivy Integration (Future)
- Add `tantivy` dependency
- Build index on track metadata
- Full FTS5-like capabilities
- More complex but better search quality

> **Status:** not started. `tantivy` appears in no `*.toml` or `*.rs` file in the tree.

---

## 10. Known Differences From the Plan

1. **Table layout.** The plan's table names (`tracks`, `playlists`, `idx_tracks_*`) and structs (`Track`, `Playlist`) are not the ones in the tree. The library keeps its records in the `mods/mp3_player/` slice of `module_kv`, under the prefixes `track/`, `playlist/` and `pt/`.
2. **Serialization.** The plan said bincode everywhere. The tree uses JSON for the widget layout and the folders, and bincode for the explorer state and the disabled list (section 3).
3. **File names.** The plan's phase 3 names `shell.redb`. The current files are listed in section 1.
4. **Search.** The plan's option A says prefix matching. The code does substring matching over a full scan (section 9.6).
5. **Index tables.** The planned index tables were not added (section 9.2).
6. **Phase 2 and the consistency check.** The plan's phase 2 compared the two databases on every read. The tree has no SQLite left to compare with, so this check does not exist in the code.

---

## 11. Risks and Mitigations (as Planned)

| Risk | Impact | Mitigation |
|------|--------|------------|
| Data loss | High | Dual-write phase, backups |
| Performance regression | Medium | Benchmark before/after |
| Search quality loss | Medium | Tantivy integration |
| Migration bugs | Low | Gradual rollout, validation |

> **Status:** the backup step exists in the code: `backup_before_migration` copies `shell.redb` before the migration opens it (section 7). The dual-write phase was not kept, because phase 3 removed SQLite.

---

## 12. Recommendation (as Written)

**Proceed with migration**, the benefits outweigh the risks:

1. **Simpler code**: no SQL migrations, no connection pooling
2. **Better Rust integration**: pure Rust, no FFI
3. **Smaller binary**: fewer dependencies
4. **ACID transactions**: data safety maintained
5. **Good performance**: key-value access is sufficient

**Start with Phase 1** (dual-write) to minimize risk.

> **Status:** the tree records phase 3 as completed (section 9.4), so the migration reached its end state. Whether each benefit above was measured is not confirmed here.

---

## 13. References

- redb: https://github.com/cberner/redb
- bincode: https://github.com/bincode-org/bincode
- tantivy: https://github.com/quickwit-oss/tantivy
