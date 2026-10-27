//! Async logging facade: bounded queue + one batched transaction per flush.
//!
//! Producers (any thread) build a [`LogEntry`] and enqueue it. A single flusher
//! thread batches entries and writes them to [`LogStore`] (one commit per batch,
//! `Durability::Eventual`) and to the plain-text `unidtk.log`.
//!
//! Guarantees:
//! - `WARN`/`ERROR` are never dropped: they block on a full queue.
//! - `INFO`/`DEBUG` may be dropped when the queue saturates; the loss count is
//!   exposed via [`dropped_count`] and merged into [`stats`].
//! - Idle costs zero wakeups: the flusher blocks on `recv()` until work arrives.
//! - [`flush`] drains the queue synchronously (panic hook, app shutdown).

use std::{
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};

use crate::log_store::{LogEntry, LogRetention, LogStore};

const QUEUE_CAPACITY: usize = 4096;
const BATCH_WINDOW_MS: u64 = 200;
const BATCH_MAX: usize = 2048;

/// Global atomic counter for unique log IDs.
static LOG_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
/// Entries dropped because the queue was saturated (INFO/DEBUG only).
static DROPPED: AtomicU64 = AtomicU64::new(0);

struct Queue {
    tx: Sender<LogEntry>,
    rx: Receiver<LogEntry>,
    store: Arc<LogStore>,
}

static QUEUE: OnceLock<Queue> = OnceLock::new();
static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// Initialize the global log pipeline. Call once at startup, after the log
/// store opened. `file` is the plain-text `unidtk.log` sink (optional).
pub fn init(store: Arc<LogStore>, file: Option<PathBuf>) {
    if QUEUE.get().is_some() {
        return;
    }
    // Seed the id counter above the current max so a fresh process never
    // overwrites entries committed by a previous one.
    if let Ok(max_id) = store.max_id() {
        LOG_ID_COUNTER.fetch_max(max_id + 1, Ordering::SeqCst);
    }
    init_file(file);

    let (tx, rx) = bounded::<LogEntry>(QUEUE_CAPACITY);
    let _ = QUEUE.set(Queue {
        tx,
        rx: rx.clone(),
        store,
    });

    std::thread::Builder::new()
        .name("unidtk-log-flusher".into())
        .spawn(move || flusher_loop(rx))
        .expect("spawn log flusher");
}

/// Is the pipeline ready (i.e. `init` succeeded)?
pub fn is_ready() -> bool {
    QUEUE.get().is_some()
}

/// Entries dropped due to queue saturation (INFO/DEBUG only).
pub fn dropped_count() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Append a backend log line (best-effort; no-op before [`init`]).
pub fn log_backend(module: &str, message: impl AsRef<str>) {
    enqueue(LogEntry {
        id: next_id(),
        ts: now_millis(),
        level: "INFO".to_string(),
        source: "backend".to_string(),
        module: module.to_string(),
        message: message.as_ref().to_string(),
        meta: None,
    });
}

/// Append a frontend log line (best-effort; no-op before [`init`]).
pub fn log_frontend(message: &str) {
    enqueue(LogEntry {
        id: next_id(),
        ts: now_millis(),
        level: "INFO".to_string(),
        source: "frontend".to_string(),
        module: "frontend".to_string(),
        message: message.to_string(),
        meta: None,
    });
}

/// List log entries (newest first) with optional filters.
pub fn list_entries(
    limit: usize,
    level_filter: Option<&str>,
    source_filter: Option<&str>,
) -> Result<Vec<LogEntry>, String> {
    match store() {
        Some(store) => store.list(limit, level_filter, source_filter),
        None => Ok(Vec::new()),
    }
}

/// Clear all logs.
pub fn clear_logs() -> Result<u64, String> {
    match store() {
        Some(store) => store.clear(),
        None => Ok(0),
    }
}

/// Log statistics (counters) plus the dropped-entries counter.
pub fn stats() -> Result<serde_json::Value, String> {
    let Some(store) = store() else {
        return Ok(serde_json::json!({ "total": 0 }));
    };
    let mut value = store.stats()?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("dropped".to_string(), serde_json::json!(dropped_count()));
    }
    Ok(value)
}

/// Run a retention sweep immediately (startup and cap overflow).
pub fn sweep() -> Result<u64, String> {
    match store() {
        Some(store) => store.sweep(LogRetention::default()),
        None => Ok(0),
    }
}

/// Drain the queue synchronously into both sinks. Safe to call from the panic
/// hook: it never blocks on new work and never panics.
pub fn flush() {
    let Some(queue) = QUEUE.get() else {
        return;
    };
    let mut batch = Vec::new();
    while batch.len() < QUEUE_CAPACITY * 2 {
        match queue.rx.try_recv() {
            Ok(entry) => batch.push(entry),
            Err(_) => break,
        }
    }
    if !batch.is_empty() {
        write_batch(&batch);
    }
}

// ── internals ────────────────────────────────────────────────────────────────

fn store() -> Option<Arc<LogStore>> {
    QUEUE.get().map(|q| Arc::clone(&q.store))
}

fn enqueue(entry: LogEntry) {
    let Some(queue) = QUEUE.get() else {
        return; // early startup — the file sink covers that window
    };
    let critical = matches!(entry.level.as_str(), "WARN" | "ERROR");
    if critical {
        // Blocking send: critical lines survive even a saturated queue.
        let _ = queue.tx.send(entry);
    } else if let Err(TrySendError::Full(_)) = queue.tx.try_send(entry) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

fn flusher_loop(rx: Receiver<LogEntry>) {
    // Retention is disk work: keep it off Tauri's startup/UI thread.
    let _ = sweep();
    loop {
        // Blocks with zero wakeups while idle.
        let first = match rx.recv() {
            Ok(entry) => entry,
            Err(_) => break, // channel closed (process exit)
        };
        let mut batch = Vec::with_capacity(64);
        batch.push(first);
        let deadline = Instant::now() + Duration::from_millis(BATCH_WINDOW_MS);
        while batch.len() < BATCH_MAX {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(entry) => batch.push(entry),
                Err(_) => break,
            }
        }
        write_batch(&batch);
    }
}

fn write_batch(batch: &[LogEntry]) {
    if let Some(queue) = QUEUE.get() {
        let _ = queue.store.insert_batch(batch);
        // Cap enforcement without a periodic timer: sweep only when over.
        if queue.store.count() > LogRetention::default().max_entries {
            let _ = queue.store.sweep(LogRetention::default());
        }
    }
    append_file(batch);
}

fn init_file(path: Option<PathBuf>) {
    let slot = LOG_FILE.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if guard.is_some() {
        return;
    }
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    *guard = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok();
}

fn append_file(entries: &[LogEntry]) {
    if entries.is_empty() {
        return;
    }
    let Some(slot) = LOG_FILE.get() else {
        return;
    };
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    for entry in entries {
        let timestamp = format_timestamp(entry.ts);
        let prefix = if entry.source == "frontend" {
            "[frontend] "
        } else {
            ""
        };
        let _ = writeln!(file, "[{timestamp}] {prefix}{}", entry.message);
    }
    let _ = file.flush();
}

fn format_timestamp(unix_millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(unix_millis)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string()
        })
        .unwrap_or_else(|| unix_millis.to_string())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn next_id() -> u64 {
    LOG_ID_COUNTER.fetch_add(1, Ordering::SeqCst)
}
