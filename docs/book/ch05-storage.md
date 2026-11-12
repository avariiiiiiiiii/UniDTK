# Chapter 5. Storage

UniDTK keeps everything it remembers in two embedded databases, both files under the user's local application data folder. This chapter shows which file holds what, how the kernel creates the tables, and how a module gets a place in the database without ever touching the database engine. It also covers the separate files a module may declare for itself, and the log that the kernel writes for every line the app prints. After it, you can read a `ModuleDb` call in a module and follow it down to a redb transaction in the kernel.

## 5.1 Two files, two jobs

UniDTK uses two database files. Both sit in `%LOCALAPPDATA%\UniDTK`. The kernel computes the paths in two functions, one per file.

Filename: core/kernel/unidtk_db.rs (abridged)

```rust
/// Canonical path: `%LOCALAPPDATA%\UniDTK\unidtk.redb`.
pub fn default_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK").join("unidtk.redb")
}
```

Filename: core/kernel/log_store.rs (abridged)

```rust
/// Canonical path: `%LOCALAPPDATA%\UniDTK\unidtk_logs.redb`.
pub fn default_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK").join("unidtk_logs.redb")
}
```

The two files have different jobs, and the rule [storage.two-files](../reference/storage.md#r-storage.two-files) says so:

| File | Holds | Why it is separate |
| :--- | :--- | :--- |
| `unidtk.redb` | user data: widget layout, open explorer tabs, folders, the music library, the list of switched-off modules | this is the data the user cares about |
| `unidtk_logs.redb` | the log of backend and frontend lines | log writes come in bursts; a separate file keeps them from waiting on a user-data transaction |

The `dirs` crate asks Windows for the known folder, rather than reading the `%LOCALAPPDATA%` variable, because scheduled tasks do not guarantee that variable (`core/api/src/paths.rs`). The rule that every layout lives in one place is [install.paths-in-one-place](../reference/install.md#r-install.paths-in-one-place).

## 5.2 What redb gives the kernel

redb is an embedded key-value database written in pure Rust, and the project uses no other database. Three of its properties shape the code. A table is typed, so `TableDefinition<&str, &[u8]>` has string keys and byte values. A file has one writer at a time, and the module store code notes that redb locks a file exclusively. Each write transaction chooses a durability: `Durability::Immediate` waits for the disk before the commit returns, and `Durability::Eventual` does not. The kernel sets durability per transaction, a trade between safety and speed.

## 5.3 The kernel's own tables

The kernel defines its tables as constants and creates them when the file opens. The tables are in `core/kernel/unidtk_db.rs`.

Filename: core/kernel/unidtk_db.rs (abridged)

```rust
const WIDGET_SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("widget_settings");
const FOLDERS: TableDefinition<&str, &[u8]> = TableDefinition::new("folders");

impl UniDtkDatabase {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        // ... create the parent folder, open or create the file ...
        let txn = db.begin_write().map_err(|e| format!("Failed to begin write: {e}"))?;
        {
            let _ = txn.open_table(WIDGET_SETTINGS).map_err(|e| e.to_string())?;
            let _ = txn.open_table(FOLDERS).map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| format!("Failed to commit: {e}"))?;
        Ok(Self { db: Arc::new(db) })
    }
}
```

Opening a table inside a write transaction creates it if it is missing. The kernel does this once, at open, so later reads never fail with a missing-table error. The commit at the end makes the creation durable.

Two values share the `widget_settings` table, under their own keys. The widget layout is stored under the key `config`. The list of switched-off modules is stored under the key `disabled_modules`. The functions `get_widget_config` and `get_disabled_modules` read those keys, and `set_disabled_modules` writes the second one. The switch path in section 4.6 of [Chapter 4](ch04-modules.md) calls the write.

Note: the music library is not in these tables. The player's data lives in the module key slice described next. The kernel moved the old library tables into that slice once, in `library_migration.rs`, and the player has no table of its own in the kernel file.

## 5.4 The module key slice

A module gets its own part of the shared database, called a key slice. All module keys sit in one table, and the kernel adds a prefix to each key so that the slices do not overlap.

Filename: core/kernel/module_db.rs (abridged)

```rust
/// Key slice of all modules: `mods/<slug>/<key>` → bytes.
pub(crate) const MODULE_KV: TableDefinition<&str, &[u8]> = TableDefinition::new("module_kv");

/// Prefix that the core prepends to a module's keys.
pub(crate) fn scope_prefix(id: &ModuleId) -> String {
    format!("mods/{}/", id.storage_slug())
}

impl RedbModuleDb {
    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.scope, key)
    }
}
```

The slug replaces each `-` with `_` (`ModuleId::storage_slug`), so `mp3-player` gets the prefix `mods/mp3_player/`. The module never sees the prefix. If glyph-bar stores a key `settings/volume`, the table holds `mods/glyph_bar/settings/volume`, and glyph-bar reads back `settings/volume`.

The kernel builds the module's store in `module_context`, which was shown in [Chapter 4](ch04-modules.md). The store is `RedbModuleDb`, constructed with the raw database and the module id. The raw database is passed to the store constructor inside the kernel and is not exposed to the module.

Each write is one transaction. Here is the write path of the shared store.

Filename: core/kernel/module_db.rs (abridged)

```rust
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
```

The shared store uses `Durability::Immediate`, because its data is user data. The constructor sets that default. The comment on `RedbModuleDb::new` says that commits are synchronous by default.

## 5.5 The ModuleDb trait

A module never sees `RedbModuleDb`. It sees the trait `ModuleDb` from the contract crate, and the trait is the whole surface.

Filename: core/api/src/lib.rs (abridged)

```rust
pub trait ModuleDb: Send + Sync {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), String>;
    fn kv_remove(&self, key: &str) -> Result<(), String>;
    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String>;
    fn kv_batch(&self, ops: Vec<DbOp>) -> Result<(), String> { /* one op at a time */ }
    fn kv_count(&self, prefix: &str) -> Result<u64, String> { /* ... */ }
}
```

The values are bytes. The contract does not know the format, so the module chooses it. The comment in the trait says that the player stores its library as bincode through `kv_batch` and `kv_list`. The keys that `kv_list` returns include the requested prefix but not the `mods/<id>/` part, so a key returned from a list can be passed straight to `kv_remove`.

`kv_batch` is the call for an operation that changes several keys. Its redb implementation runs all the changes in one transaction, so either all of them apply or none does. The library code uses it, for example in `modules/mp3-player/rust/library/db.rs`.

Warning: the default body of `kv_batch` in the trait is not atomic. It applies the operations one after another, so a failure in the middle leaves the earlier ones applied. A module that needs atomicity depends on the redb implementation, and a test double that uses the default body does not give it.

## 5.6 Separate files: StoreSpec and StoreKind

Some data does not belong in the shared database. Writes are frequent and long, or the data can be rebuilt. For those cases a module declares a store, and the kernel decides where the file lives and how durable it is.

Filename: core/api/src/lib.rs (abridged)

```rust
pub enum StoreKind {
    /// Slice of the shared database (`mods/<id>/<key>`).
    Shared,
    /// Own file. User data: ACID, included in backups.
    OwnedDurable,
    /// Own file. Derived, rebuildable data: no fsync per commit.
    OwnedCache,
}

pub struct StoreSpec {
    pub name: String,
    pub kind: StoreKind,
    pub legacy_files: Vec<String>,
    pub legacy_tables: Vec<(&'static str, String)>,
}
```

The module names a store and a kind. It never builds a path. The kernel maps the declaration to a file in `core/kernel/module_store.rs`:

| Kind | Path under the data folder | Commit durability |
| :--- | :--- | :--- |
| `Shared` | the module key slice in `unidtk.redb` | `Immediate` |
| `OwnedDurable` | `mods/<slug>/<name>.redb` | `Immediate` |
| `OwnedCache` | `cache/<slug>/<name>.redb` | `Eventual` |

Two modules use this. The `micrographics` module, which also feeds the weather widget, declares a cache for its weather data, in `modules/micrographics/rust/micrographics.rs`:

Filename: modules/micrographics/rust/micrographics.rs (abridged)

```rust
match ctx.store(unidtk_api::StoreSpec::cache("weather")) {
    Ok(store) => { /* read the last reading */ }
    Err(_) => { /* no store: start without it */ }
}
```

The music player declares the AI DJ data the same way, with `StoreSpec::cache("ai_dj")` in `modules/mp3-player/rust/commands.rs`.

`ModuleContext::store` does two things. It checks the name, which may contain only letters, digits, `_` and `-`, and at most 64 characters. Then it dispatches on the kind. A `Shared` store returns the same slice as `db()`, so declaring one adds nothing. Any other kind goes to the factory that the kernel gave the context.

The factory is `RedbStoreFactory`. redb locks a file exclusively, so each path must have exactly one open handle. The factory keeps that handle in a map keyed by path, and a second `store` call for the same file gets the handle that is already open.

Legacy data is moved on the first open. A `StoreSpec` may list the old file names it used to live under, and the factory renames or copies the first one it finds into the new path. A spec may also list old redb tables, and the factory rewrites their rows as prefixed keys, then removes the old tables. The operation is idempotent, so it runs on every open, not once.

Note: `ModuleContext::new_diagnostic` installs `InMemoryStoreFactory`, so the doctor never writes a store file.

## 5.7 Why a module never gets the raw database

The rule [storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db) says that a module never receives the database engine. The contract is built so that the engine cannot leak through it. `core/api/Cargo.toml` lists two dependencies, `serde_json` and `dirs`, and it does not list redb. A module that wanted to open a redb file would have to add the dependency itself, and then it would also need the path, which the contract does not give out.

The kernel decides where the files live, so moving the data folder changes no module. The paths come from one function, `data_dir()` in `core/api/src/paths.rs`.

If the shared database fails to open, the kernel gives modules `NoopModuleDb`. Reads return nothing, and writes return the error `module db unavailable (no redb)`. A module that depends on its store must handle that error, and a module that only reads its settings can start with defaults.

## 5.8 The log store

The log is the third thing the kernel keeps. It is one table of entries in `unidtk_logs.redb`, and it is written in batches by a thread of its own.

Filename: core/kernel/log_store.rs (abridged)

```rust
const LOGS: TableDefinition<u64, &[u8]> = TableDefinition::new("logs");
const COUNTERS: TableDefinition<&str, u64> = TableDefinition::new("log_counters");

pub struct LogEntry {
    pub id: u64,
    pub ts: i64,            // unix millis
    pub level: String,
    pub source: String,     // "backend" | "frontend"
    pub module: String,
    pub message: String,
    pub meta: Option<String>,
}
```

The path from a line of text to the table has four steps.

1. A producer calls `unidtk_api::log`, or the page calls the `frontend_log` command. Both reach the kernel's logging functions.
2. `enqueue` in `core/kernel/logging.rs` puts the entry on a bounded queue of 4096 entries. A `WARN` or `ERROR` entry waits for room in the queue, so it is never dropped. An `INFO` or `DEBUG` entry is dropped when the queue is full, and the drop is counted.
3. A single flusher thread takes entries from the queue. It collects up to 2048 of them, or stops collecting after a 200 millisecond window, whichever comes first.
4. The flusher writes the batch in one transaction with `Durability::Eventual`, and it appends the same lines to the plain-text file `unidtk.log` in the logs folder.

One commit covers many lines, and the commit does not wait for the disk. The idle flusher blocks on a receive call, so an idle app wakes for no log lines. The `udtk logs` command reads the same table through the socket command `logs`. The retention policy `LogRetention` defaults to 7 days and 5000 entries, and `sweep` applies it. The queue and file details are in [dev/logging.md](../dev/logging.md), and the file layout is in [dev/storage.md](../dev/storage.md).

Warning: a module must write its messages through `unidtk_api::log`, never to a file of its own. A line written to a private file never reaches `udtk logs`, and it is not in the log the user reads when something fails.

## 5.9 Summary

The kernel owns two files. `unidtk.redb` holds user data in typed tables, and the kernel creates them when it opens the file. A module reaches its part of that file through `ModuleDb`, and the kernel prefixes every key with `mods/<slug>/`, so no module can name another module's keys. A module that needs its own file declares a `StoreSpec`, and the kernel picks the path and the durability. `unidtk_logs.redb` takes the log through a bounded queue and writes it in batches. The next chapter follows the other side of the interface: the page that the kernel's data is shown in. Continue with [Chapter 6. The page](ch06-the-page.md).
