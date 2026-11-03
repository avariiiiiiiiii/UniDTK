# Storage

UniDTK keeps its data in redb, an embedded key-value database written in pure Rust. This chapter states which files exist and who may open them. How a read or a write travels through the code is explained in [dev/storage.md](../dev/storage.md).

r[storage.redb-only]

All persistent state is in redb. SQLite and other C/FFI databases (`rusqlite`, `libsqlite3-sys`) are not used.

r[storage.two-files]

There are two database files, both in `%LOCALAPPDATA%\UniDTK`:

- `unidtk.redb`: user data — tracks, playlists, history, AI DJ data, configuration, widget settings;
- `unidtk_logs.redb`: the log. It is a separate file so that a burst of log writes never waits on, or blocks, a user-data transaction.

r[storage.no-raw-db]

A module never receives the raw database engine. State that must live in the shared database goes through `ModuleDb`, for which the kernel allocates the key slice `mods/<id>/…`. Anything else a module keeps in its own data files, declared through `StoreSpec`, or in memory. Because only the kernel opens the files, moving them affects no module.

r[storage.log-queue]

Log lines go through an asynchronous queue and are committed in batches, one commit per batch. `WARN` and `ERROR` lines are never dropped. Both the backend and the frontend write to the same log, and `udtk logs` reads it. The mechanism is in [dev/logging.md](../dev/logging.md).
