# Logging: One Queue, Batched Commits, Two Sinks

This chapter explains how a log line travels from the code that writes it to the database file on disk. It shows where the lines wait, when they are committed, what is dropped under load and what never is, and how old lines are removed.

The first version of this work was a task note: replace the plain `shell.log` file with a redb table, and follow a reference implementation from another project. The system that is in the repository now is different in some ways, and the differences are listed at the end of this chapter.

## What you will understand by the end

- Which code paths produce log lines: the kernel, modules, the frontend, and the panic hook.
- Why the producers never touch the disk. They only put an entry on a queue.
- Why one writer thread commits entries in batches, and what a batch costs.
- Why WARN and ERROR entries block when the queue is full, while INFO entries are dropped.
- Where the database and the text file are, and how the retention limits remove old entries.

## Files involved

| File | Role |
| :--- | :--- |
| [core/kernel/logging.rs](../../core/kernel/logging.rs) | The queue, the writer thread, the batch logic, and the text file sink (287 lines). |
| [core/kernel/log_store.rs](../../core/kernel/log_store.rs) | The redb store: the table layout, batch insert, listing, clearing, counters, retention (475 lines). |
| [core/kernel/main.rs](../../core/kernel/main.rs) | `log_line`, startup order, the Tauri commands `frontend_log`, `log_list`, `log_stats`, `log_clear`. |
| [core/kernel/panic_hook.rs](../../core/kernel/panic_hook.rs) | Panic reports, written with a synchronous flush. |
| [core/kernel/ipc.rs](../../core/kernel/ipc.rs) | The socket door: `frontend_log` and `log_stats` for the CLI. |
| [core/api/src/lib.rs](../../core/api/src/lib.rs) | `unidtk_api::log`, the function modules call. |
| [core/ui/unidtk-core.js](../../core/ui/unidtk-core.js) | `rlog()`, the frontend's logging function. |
| [opt/cli/main.rs](../../opt/cli/main.rs) | The `udtk logs` command. |

On disk there are two files:

- `%LOCALAPPDATA%\UniDTK\unidtk_logs.redb`: the database. It is the source for the log views.
- `%LOCALAPPDATA%\UniDTK\logs\unidtk.log`: a plain-text copy, one line per entry.

---

## Step 1: Who writes log lines

There are four producers, and they all end up at the same function, `enqueue`.

**The kernel** calls `log_line(message)` in [core/kernel/main.rs](../../core/kernel/main.rs). After the logging pipeline is ready, `log_line` calls `logging::log_backend("unidtk", message)`.

**Modules** call `unidtk_api::log(message)`. This function forwards the text to a hook that the kernel installs at startup, with `set_logger(|message| log_line(message))`. So a module's line takes the same path as a kernel line.

**The frontend** calls `rlog()` in [core/ui/unidtk-core.js](../../core/ui/unidtk-core.js). `rlog` writes to the browser console and calls the Tauri command `frontend_log`:

```rust
#[tauri::command(async)]
fn frontend_log(message: String) {
    crate::logging::log_frontend(&message);
}
```

The socket door has the same operation, `handle_frontend_log`, for clients that talk to the kernel over the socket instead of through Tauri.

**The panic hook** calls `log_backend("panic", summary)` and then `logging::flush()`, so the report is written even if the process is about to die.

One detail matters for the frontend. `log_frontend` always sets the level to `INFO`. A frontend line cannot be a WARN or an ERROR, so it can be dropped under load. The source is recorded as `frontend`, and the module as `frontend`.

---

## Step 2: The startup window

The database is opened in a thread at startup, so the window before the pipeline is ready has to be handled. The order is:

1. `log_line` is called before `logging::init`. Then it writes straight to `unidtk.log`, because there is no queue yet:

   ```rust
   if !crate::logging::is_ready() {
       append_log_file(message);
   }
   crate::logging::log_backend("unidtk", message);
   ```

2. A bootstrap thread opens the store at `LogStore::default_path()` and calls `logging::init(store, Some(logs_dir/unidtk.log))`.
3. If the database cannot be opened, the app keeps running with the text file only. The startup log then says `setup: log redb open failed (file log only)`.

`init` sets the entry ID counter above the highest ID already in the database, so a new process never overwrites entries written by the previous one:

```rust
if let Ok(max_id) = store.max_id() {
    LOG_ID_COUNTER.fetch_max(max_id + 1, Ordering::SeqCst);
}
```

---

## Step 3: The queue and the two kinds of entries

Producers put entries into a bounded channel with room for 4,096 entries (`QUEUE_CAPACITY`). The send depends on the level:

```rust
let critical = matches!(entry.level.as_str(), "WARN" | "ERROR");
if critical {
    // Blocking send: critical lines survive even a saturated queue.
    let _ = queue.tx.send(entry);
} else if let Err(TrySendError::Full(_)) = queue.tx.try_send(entry) {
    DROPPED.fetch_add(1, Ordering::Relaxed);
}
```

- **WARN and ERROR** use a blocking send. If the queue is full, the producer waits until the writer makes room. Such a line is never lost, but a producer can be slowed down by a stalled disk.
- **INFO and DEBUG** use a non-blocking send. If the queue is full, the entry is dropped and the `DROPPED` counter is increased. The counter is reported in the statistics as `dropped`.

The queue lets a producer return at once, without waiting for the disk. The price is that a line written just before a crash may not be in the database yet. The `flush()` function, used by the panic hook and at shutdown, drains the queue on the spot.

---

## Step 4: The writer thread and batching

One thread, `unidtk-log-flusher`, owns the disk writes. Its loop is short:

```rust
fn flusher_loop(rx: Receiver<LogEntry>) {
    let _ = sweep();                     // retention, once at startup
    loop {
        let first = match rx.recv() { ... };   // blocks: zero wakeups while idle
        let mut batch = Vec::with_capacity(64);
        batch.push(first);
        let deadline = Instant::now() + Duration::from_millis(BATCH_WINDOW_MS);  // 200 ms
        while batch.len() < BATCH_MAX {                                          // 2048
            let now = Instant::now();
            if now >= deadline { break; }
            match rx.recv_timeout(deadline - now) {
                Ok(entry) => batch.push(entry),
                Err(_) => break,
            }
        }
        write_batch(&batch);
    }
}
```

When the first entry arrives, the thread does not write it at once. It waits up to 200 milliseconds, or until 2,048 entries are collected, and then writes everything in one batch. A busy moment with many lines therefore costs one commit, not one commit per line. An idle app costs nothing, because `recv()` blocks until something is queued.

---

## Step 5: One transaction per batch

`write_batch` calls `insert_batch`, and that function opens one write transaction for the whole batch:

```rust
let mut txn = self.db.begin_write()?;
txn.set_durability(redb::Durability::Eventual);
{
    let mut table = txn.open_table(LOGS)?;
    let mut counters = txn.open_table(COUNTERS)?;
    for entry in entries {
        let bytes = serde_json::to_vec(entry)?;
        table.insert(entry.id, bytes.as_slice())?;
        bump(&mut counters, "total", 1)?;
        bump(&mut counters, &format!("level:{}", entry.level), 1)?;
        bump(&mut counters, &format!("source:{}", entry.source), 1)?;
    }
}
txn.commit()?;
```

The layout is two tables:

- `logs`: key is the entry ID (`u64`), value is the entry as JSON bytes (`&[u8]`).
- `log_counters`: key is a string, value is a count. The keys are `total`, `level:<LEVEL>`, and `source:<source>`. They are kept up to date in the same transaction as the entries, so the statistics never disagree with the rows.

`Durability::Eventual` tells redb not to wait for the disk to confirm each commit. A commit returns sooner, and the most recent batches can be lost in a crash. That trade-off is accepted for logs.

A `LogEntry` has these fields: `id`, `ts` (Unix time in milliseconds), `level`, `source` (`backend` or `frontend`), `module`, `message`, and `meta` (optional, currently always empty for the logging paths).

---

## Step 6: The text sink

After the batch is in the database, `append_file` writes the same entries to `unidtk.log`:

Each line has this shape, where the message is the entry's text:

```text
[YYYY-MM-DD HH:MM:SS.mmm] <message>
[YYYY-MM-DD HH:MM:SS.mmm] [frontend] <message>
```

The timestamp is local time with milliseconds. Frontend lines get a `[frontend] ` prefix. The file is flushed after every batch. The text file is the fallback for the startup window and for the case where the database cannot be opened, and it is the easiest file to read by hand.

---

## Step 7: Where the database lives

`LogStore::default_path()` returns `%LOCALAPPDATA%\UniDTK\unidtk_logs.redb`. The architecture keeps logs in their own file, separate from the user data file `unidtk.redb` ([docs/architecture.md](../architecture.md)). The store is opened with `LogStore::open`, which creates the folder and the file if they are missing. The tables are created on the first write, so an empty store has no tables.

---

## Step 8: Retention

Two limits apply, and the default values are in `LogRetention`:

```rust
impl Default for LogRetention {
    fn default() -> Self {
        Self { max_age_days: 7, max_entries: 5000 }
    }
}
```

`sweep` runs in two places:

- once when the writer thread starts, and
- after a batch, if the number of stored entries is above 5,000 (`count()` is an O(1) lookup of the table length).

A sweep does four things in order. It reads all entries and marks any with a timestamp older than seven days. It then counts the survivors, and if there are more than 5,000, it marks the oldest excess entries. It removes the marked entries in one transaction. Finally, it rebuilds the counters from the surviving rows, in the same transaction, so the counters cannot drift. The function returns the number of removed rows.

---

## Step 9: Reading the log

| Interface | Operation | Notes |
| :--- | :--- | :--- |
| Tauri command `log_list(limit, level, source)` | Returns entries, newest first | Default limit is 200. |
| Tauri command `log_stats` | Counters: `total`, `level:*`, `source:*`, and `dropped` | `dropped` is the in-memory count of INFO and DEBUG entries lost to a full queue. |
| Tauri command `log_clear` | Removes all entries and resets the counters | Returns the number of removed rows. |
| Socket door `log_stats` and `frontend_log` | The same data for the CLI | |
| CLI `udtk logs [--limit N] [--since ID] [--level LEVEL] [--source SOURCE]` | Prints entries | The CLI default limit is 50, not 200. |

---

## Step 10: What differs from the original task

The task note set goals, and this is what was built against them:

| Task note said | What the code does |
| :--- | :--- |
| Replace the plain `shell.log` file with redb. | The redb table is the main store. `unidtk.log` remains as a second sink. |
| Choose between a synchronous insert and a writer thread. | A bounded queue with one writer thread, batched commits, and a blocking send for WARN and ERROR. |
| Follow the reference implementation's table and retention. | The table layout (`u64` key, JSON value, counters) follows that pattern. The queue and batching are added. |
| Rotate by size or time. | Retention is by age (7 days) and by entry count (5,000). Size is not used. |
| Frontend `rlog()` may pass a level. | `log_frontend` always uses `INFO`. A frontend level is not implemented. |
| Module name taken from the caller. | Kernel lines use the module name `unidtk`, panics use `panic`, frontend lines use `frontend`. Modules are not recorded by name. |

Unit tests exist in `log_store.rs`: `test_log_entry_serde_roundtrip`, `test_log_insert_list_clear`, `test_log_sweep_retention`, and `test_get_since_and_max_id`. They were not run for this rewrite.

---

## Summary

- Every producer, from kernel to frontend, puts an entry on one queue and returns.
- One thread writes the queue in batches of up to 2,048 entries, within a 200-millisecond window, in one transaction each.
- WARN and ERROR are never dropped. INFO and DEBUG are dropped and counted when the queue is full.
- The database is `%LOCALAPPDATA%\UniDTK\unidtk_logs.redb`, with the tables `logs` and `log_counters`. A text copy is written to `logs\unidtk.log`.
- Old entries are removed after 7 days, and the store is kept at 5,000 entries or fewer.
