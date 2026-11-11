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
/// Entries accepted by the queue, and entries that have reached both sinks. The
/// flusher takes entries out of the channel before it writes them, so an empty
/// channel does not mean everything is written: `flush` compares these two.
static ENQUEUED: AtomicU64 = AtomicU64::new(0);
static WRITTEN: AtomicU64 = AtomicU64::new(0);
/// The longest `flush` waits for a batch the flusher is still writing.
const FLUSH_WAIT: Duration = Duration::from_millis(1000);

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
    log_backend_at("INFO", module, message);
}

/// Append a backend log line at `level` (`INFO`, `WARN` or `ERROR`).
pub fn log_backend_at(level: &str, module: &str, message: impl AsRef<str>) {
    enqueue(LogEntry {
        id: next_id(),
        ts: now_millis(),
        level: normalize_level(Some(level)).to_string(),
        source: "backend".to_string(),
        module: module.to_string(),
        message: message.as_ref().to_string(),
        meta: None,
    });
}

/// Append a frontend log line (best-effort; no-op before [`init`]). The page
/// names the level; anything it does not recognise is recorded as `INFO`.
pub fn log_frontend(message: &str, level: Option<&str>) {
    enqueue(LogEntry {
        id: next_id(),
        ts: now_millis(),
        level: normalize_level(level).to_string(),
        source: "frontend".to_string(),
        module: "frontend".to_string(),
        message: clamp_message(message, MAX_FRONTEND_MESSAGE_BYTES),
        meta: None,
    });
}

/// The page is untrusted input to the journal: one message may not be larger
/// than this, or a single call could fill the journal and the log file.
const MAX_FRONTEND_MESSAGE_BYTES: usize = 8 * 1024;

/// Cuts a message to `max` bytes on a character boundary and says how much was left out.
fn clamp_message(message: &str, max: usize) -> String {
    if message.len() <= max {
        return message.to_string();
    }
    let mut end = max;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} more bytes]", &message[..end], message.len() - end)
}

/// One journal entry is one line of `unidtk.log`: a line break inside a message
/// (a pasted stack trace, or text imitating a log line) must not start a new line.
fn single_line(message: &str) -> String {
    message.replace("\r\n", " | ").replace(['\r', '\n'], " | ")
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
    let target = ENQUEUED.load(Ordering::SeqCst);
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
    // A batch the flusher took before this call may still be on its way to disk:
    // wait for it, but not for ever (the flusher may be gone).
    wait_until_written(&WRITTEN, target, FLUSH_WAIT);
}

/// Wait until `written` reaches `target`; false if `timeout` ran out first.
fn wait_until_written(written: &AtomicU64, target: u64, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while written.load(Ordering::SeqCst) < target {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    true
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
        if queue.tx.send(entry).is_ok() {
            ENQUEUED.fetch_add(1, Ordering::SeqCst);
        }
    } else {
        match queue.tx.try_send(entry) {
            Ok(()) => {
                ENQUEUED.fetch_add(1, Ordering::SeqCst);
            }
            Err(TrySendError::Full(_)) => {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
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
    WRITTEN.fetch_add(batch.len() as u64, Ordering::SeqCst);
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
        let _ = writeln!(file, "[{timestamp}] {prefix}{}", single_line(&entry.message));
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

/// The three levels the store keeps. `WARN` and `ERROR` are never dropped when
/// the queue is full, so a caller cannot invent a level that escapes that rule.
fn normalize_level(level: Option<&str>) -> &'static str {
    match level.map(str::to_ascii_uppercase).as_deref() {
        Some("WARN" | "WARNING") => "WARN",
        Some("ERROR") => "ERROR",
        _ => "INFO",
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_message, normalize_level, single_line, wait_until_written};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn flush_waits_for_a_batch_that_is_still_being_written() {
        let written = Arc::new(AtomicU64::new(3));
        let late = Arc::clone(&written);
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            late.store(5, Ordering::SeqCst);
        });
        assert!(wait_until_written(&written, 5, Duration::from_secs(2)));
        writer.join().unwrap();
        // Nobody writes any more: the wait ends, it does not hang the exit.
        assert!(!wait_until_written(&written, 6, Duration::from_millis(30)));
        assert!(wait_until_written(&written, 5, Duration::ZERO));
    }

    #[test]
    fn an_oversized_message_is_cut_on_a_character_and_says_so() {
        let long = "я".repeat(100);
        let cut = clamp_message(&long, 11);
        assert!(cut.starts_with(&"я".repeat(5)));
        assert!(cut.ends_with("more bytes]"));
        assert!(cut.len() < long.len());
        assert_eq!(clamp_message("short", 11), "short");
    }

    #[test]
    fn a_line_break_in_a_message_does_not_start_a_new_log_line() {
        let forged = "first\r\n[2026-01-01 00:00:00.000] setup: done\nthird";
        let line = single_line(forged);
        assert!(!line.contains('\n') && !line.contains('\r'));
        assert!(line.contains("first | [2026-01-01"));
    }

    #[test]
    fn levels_are_normalized_to_the_three_the_store_keeps() {
        assert_eq!(normalize_level(None), "INFO");
        assert_eq!(normalize_level(Some("info")), "INFO");
        assert_eq!(normalize_level(Some("WARN")), "WARN");
        assert_eq!(normalize_level(Some("warning")), "WARN");
        assert_eq!(normalize_level(Some("error")), "ERROR");
        assert_eq!(normalize_level(Some("ERROR")), "ERROR");
        assert_eq!(normalize_level(Some("debug")), "INFO");
        assert_eq!(normalize_level(Some("FATAL!")), "INFO");
    }
}
