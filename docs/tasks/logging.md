# Task: Unified logging system (redb) — backend + frontend

## Context
- Repo: `C:/Users/admin/Documents/GitHub/unidtk`
- Architecture docs/architecture.md §8: single `LOGS` table in redb; backend writes all commands/errors, frontend writes console/errors via invoke; rotation (cap by size/time); export via CLI (`shell logs`).
- Reference implementation (READ THIS FIRST): `C:/Users/admin/Documents/GitHub/noto/NOTO/backend/src/logging.rs` and `C:/Users/admin/Documents/GitHub/noto/NOTO/backend/src/commands/logs.rs`. It shows: redb `logs` table (key u64 id, value JSON), async writer thread with mpsc channel + batch flush + retention sweep, `log::Log` impl, Tauri commands log_batch/log_list/log_clear/log_stats.

## Current state in shell
- `backend/code/music/src/shell_db.rs` — redb DB (ShellDb) with tables: widget_positions, widget_settings, explorer_tabs, telegram_*, vpn_*, tracks, playlists, ai_*. Has `TableDefinition` imports (redb).
- `backend/code/main.rs` — has `log_line(msg)` writing to `shell.log` file next to exe (std::fs append). There's a `frontend_log` command already (`fn frontend_log(message: String)` at ~line 176) that calls `log_line`.
- Tauri commands registered in `main.rs` invoke_handler (~line 752).

## Goal
Replace the file-based `shell.log` logging with a redb-backed unified log system:

1. **Add `LOGS` table** to `backend/code/music/src/shell_db.rs`:
   - `const LOGS: TableDefinition<u64, &[u8]> = TableDefinition::new("logs");`
   - Register in the table-init section (find where other tables are opened in init/write txn).
   - Add methods: `list_logs(limit, level, source)`, `insert_logs(&[LogEntry])`, `clear_logs()`, `log_stats()` — follow the noto pattern but adapt to ShellDb's API (check how shell_db opens tables: it uses `self.db.begin_read()` / `begin_write()` and `txn.open_table(...)`).

2. **LogEntry struct** — mirror noto: `{ id: u64, ts: i64 (unix millis), level: String, source: String ("backend"|"frontend"), module: String, message: String, meta: Option<String> }`. Put it in shell_db.rs (or a new `logging.rs` in backend/code/music/src).

3. **Backend logging**:
   - Keep `log_line(msg)` but ALSO write to redb LOGS (level=INFO, source=backend, module from caller if easy, else "shell").
   - To avoid blocking on every log call, use a channel + writer thread like noto, OR keep it simple: synchronous insert (logs are low-frequency in this app). Choose synchronous insert for simplicity unless volume is high. Make `log_line` async-safe (it's called from many places; check main.rs usages).
   - Add a `backend_log` Tauri command or reuse existing pattern.

4. **Frontend logging**:
   - Existing `frontend_log` command: make it write to redb LOGS (source=frontend) instead of (or in addition to) file.
   - Add `log_list`, `log_clear`, `log_stats` Tauri commands (mirror noto commands/logs.rs), registered in main.rs invoke_handler.
   - Frontend `rlog()` in `frontend/shell-core.js` calls `invoke("frontend_log", ...)` — keep that working; maybe extend to include level/module.

5. **Rotation**: implement retention in redb (like noto: sweep older than N days or beyond max entries). Call sweep on insert or via timer.

6. **Keep shell.log file** for compatibility (dev convenience) OR remove — decide based on what still reads it (check main.rs `log_line` usages, VBS scripts, docs). Prefer: keep file append as well, but make redb the primary store. Update `frontend_log` to also push into redb.

## Files to touch
- `backend/code/music/src/shell_db.rs` (LOGS table + methods + LogEntry)
- `backend/code/main.rs` (log_line → redb, new commands, register)
- `backend/code/music/src/lib.rs` (export new module if created)
- Maybe new `backend/code/music/src/logging.rs`
- Frontend: `frontend/shell-core.js` rlog (optional level), `frontend/boot-log.js` if it logs
- Update `docs/architecture.md` §8 if needed (mark done)

## Validation
- `cargo check` in backend/toolchain compiles.
- Test: `cargo test` in backend/code/music if there are tests; add a small unit test for LogEntry serialize + insert/list roundtrip in shell_db.
- Manual: app starts, `frontend_log` writes to redb, `log_list` returns entries.

## Deliverable
- Report: changed files, validation, how to view logs (command names), any deviation from noto pattern.
