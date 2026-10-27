//! Rotated log storage: a dedicated `unidtk_logs.redb` database.
//!
//! Logs are high-frequency rotated data; keeping them in their own file avoids
//! bloating the ACID data store (`unidtk.redb`) and lets the writer relax
//! durability. Tables are created lazily on the first write.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};

const LOGS: TableDefinition<u64, &[u8]> = TableDefinition::new("logs");
const COUNTERS: TableDefinition<&str, u64> = TableDefinition::new("log_counters");

/// A single log entry stored in redb.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub ts: i64, // unix millis
    pub level: String,
    pub source: String, // "backend" | "frontend"
    pub module: String,
    pub message: String,
    pub meta: Option<String>,
}

/// Retention policy for automatic log cleanup.
#[derive(Debug, Clone, Copy)]
pub struct LogRetention {
    pub max_age_days: i64,
    pub max_entries: u64,
}

impl Default for LogRetention {
    fn default() -> Self {
        Self {
            max_age_days: 7,
            max_entries: 5000,
        }
    }
}

/// Dedicated redb store for Shell logs.
pub struct LogStore {
    db: Arc<Database>,
}

impl LogStore {
    /// Open or create the log database. Tables stay absent until the first write.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create dir: {e}"))?;
        }
        // Automatic legacy migration if canonical file doesn't exist
        if !path.exists() && path == Self::default_path() {
            let legacy_candidates = [
                path.with_file_name("shell_logs.redb"),
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Shell")
                    .join("shell_logs.redb"),
            ];
            for old_path in &legacy_candidates {
                if old_path.exists() {
                    let _ = std::fs::copy(old_path, path);
                    break;
                }
            }
        }

        let db = if path.exists() {
            Database::open(path).map_err(|e| format!("Failed to open log redb: {e}"))?
        } else {
            Database::create(path).map_err(|e| format!("Failed to create log redb: {e}"))?
        };
        Ok(Self { db: Arc::new(db) })
    }

    /// Canonical path: `%LOCALAPPDATA%\UniDTK\unidtk_logs.redb`.
    pub fn default_path() -> PathBuf {
        let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("UniDTK").join("unidtk_logs.redb")
    }

    /// Stored entry count (O(1) B-tree length; 0 before the first write).
    pub fn count(&self) -> u64 {
        let Ok(txn) = self.db.begin_read() else {
            return 0;
        };
        match txn.open_table(LOGS) {
            Ok(table) => table.len().unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Insert a batch in one transaction with relaxed durability.
    pub fn insert_batch(&self, entries: &[LogEntry]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut txn = self.db.begin_write().map_err(|e| e.to_string())?;
        txn.set_durability(redb::Durability::Eventual);
        {
            let mut table = txn.open_table(LOGS).map_err(|e| e.to_string())?;
            let mut counters = txn.open_table(COUNTERS).map_err(|e| e.to_string())?;
            for entry in entries {
                let bytes = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
                table
                    .insert(entry.id, bytes.as_slice())
                    .map_err(|e| e.to_string())?;
                bump(&mut counters, "total", 1)?;
                bump(&mut counters, &format!("level:{}", entry.level), 1)?;
                bump(&mut counters, &format!("source:{}", entry.source), 1)?;
            }
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// List entries newest-first with optional level/source filters.
    pub fn list(
        &self,
        limit: usize,
        level_filter: Option<&str>,
        source_filter: Option<&str>,
    ) -> Result<Vec<LogEntry>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = match txn.open_table(LOGS) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };
        let mut entries: Vec<LogEntry> = Vec::with_capacity(limit.min(1000));
        let range = table.range::<u64>(..).map_err(|e| e.to_string())?;
        for item in range.rev() {
            if entries.len() >= limit {
                break;
            }
            let (_, v) = item.map_err(|e| e.to_string())?;
            let entry: LogEntry = match serde_json::from_slice(v.value()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if let Some(lf) = level_filter {
                if !lf.is_empty() && entry.level.to_lowercase() != *lf {
                    continue;
                }
            }
            if let Some(sf) = source_filter {
                if !sf.is_empty() && entry.source != *sf {
                    continue;
                }
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Entries with `id > since_id`, ascending (oldest to newest).
    pub fn get_since(&self, since_id: u64, limit: Option<usize>) -> Result<Vec<LogEntry>, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = match txn.open_table(LOGS) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };
        let limit = limit.unwrap_or(1000);
        let mut entries = Vec::with_capacity(limit.min(128));
        let range = table
            .range::<u64>((std::ops::Bound::Excluded(since_id), std::ops::Bound::Unbounded))
            .map_err(|e| e.to_string())?;
        for item in range.take(limit) {
            let (_, v) = item.map_err(|e| e.to_string())?;
            if let Ok(entry) = serde_json::from_slice::<LogEntry>(v.value()) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Remove all entries (and counters); returns how many were removed.
    pub fn clear(&self) -> Result<u64, String> {
        let count = self.count();
        let mut txn = self.db.begin_write().map_err(|e| e.to_string())?;
        txn.set_durability(redb::Durability::Immediate);
        {
            let mut table = txn.open_table(LOGS).map_err(|e| e.to_string())?;
            let ids: Vec<u64> = table
                .range::<u64>(..)
                .map_err(|e| e.to_string())?
                .map(|r| r.map(|(k, _)| k.value()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            for id in ids {
                table.remove(id).map_err(|e| e.to_string())?;
            }
            let mut counters = txn.open_table(COUNTERS).map_err(|e| e.to_string())?;
            let keys: Vec<String> = counters
                .range::<&str>(..)
                .map_err(|e| e.to_string())?
                .map(|r| r.map(|(k, _)| k.value().to_string()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            for key in keys {
                counters.remove(key.as_str()).map_err(|e| e.to_string())?;
            }
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }

    /// Highest stored id (0 when empty) — seeds a fresh process's id counter.
    pub fn max_id(&self) -> Result<u64, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = match txn.open_table(LOGS) {
            Ok(t) => t,
            Err(_) => return Ok(0),
        };
        let max = table
            .range::<u64>(..)
            .map_err(|e| e.to_string())?
            .next_back()
            .transpose()
            .map_err(|e| e.to_string())?
            .map(|(k, _)| k.value())
            .unwrap_or(0);
        Ok(max)
    }

    /// Counters-based statistics (no table scan): totals by level/source.
    pub fn stats(&self) -> Result<serde_json::Value, String> {
        let txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let counters = match txn.open_table(COUNTERS) {
            Ok(t) => t,
            Err(_) => {
                return Ok(serde_json::json!({
                    "total": 0,
                    "by_level": {},
                    "by_source": {},
                }))
            }
        };
        let mut by_level = serde_json::Map::new();
        let mut by_source = serde_json::Map::new();
        let mut total = 0u64;
        for item in counters.iter().map_err(|e| e.to_string())? {
            let (k, v) = item.map_err(|e| e.to_string())?;
            let key = k.value();
            let value = v.value();
            if key == "total" {
                total = value;
            } else if let Some(level) = key.strip_prefix("level:") {
                by_level.insert(level.to_string(), serde_json::json!(value));
            } else if let Some(source) = key.strip_prefix("source:") {
                by_source.insert(source.to_string(), serde_json::json!(value));
            }
        }
        Ok(serde_json::json!({
            "total": total,
            "by_level": by_level,
            "by_source": by_source,
        }))
    }

    /// Retention sweep: drop entries older than `max_age_days` and everything
    /// beyond `max_entries` (oldest first). Rebuilds counters in the same txn.
    pub fn sweep(&self, retention: LogRetention) -> Result<u64, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let cutoff = now - retention.max_age_days * 86_400_000;
        let mut to_remove: HashSet<u64> = HashSet::new();

        {
            let txn = self.db.begin_read().map_err(|e| e.to_string())?;
            let table = match txn.open_table(LOGS) {
                Ok(t) => t,
                Err(_) => return Ok(0),
            };
            let mut remaining_ids = Vec::new();
            for item in table.range::<u64>(..).map_err(|e| e.to_string())? {
                let (k, v) = item.map_err(|e| e.to_string())?;
                let id = k.value();
                let too_old = serde_json::from_slice::<LogEntry>(v.value())
                    .map(|e| e.ts < cutoff)
                    .unwrap_or(false);
                if too_old {
                    to_remove.insert(id);
                } else {
                    remaining_ids.push(id);
                }
            }
            if remaining_ids.len() > retention.max_entries as usize {
                let excess = remaining_ids.len() - retention.max_entries as usize;
                for &id in &remaining_ids[..excess] {
                    to_remove.insert(id);
                }
            }
        }

        if to_remove.is_empty() {
            return Ok(0);
        }
        let removed = to_remove.len() as u64;

        let mut txn = self.db.begin_write().map_err(|e| e.to_string())?;
        txn.set_durability(redb::Durability::Eventual);
        {
            let mut table = txn.open_table(LOGS).map_err(|e| e.to_string())?;
            for id in &to_remove {
                table.remove(*id).map_err(|e| e.to_string())?;
            }

            // Rebuild counters from the surviving entries (bounded by cap).
            let mut counts: HashMap<String, u64> = HashMap::new();
            let mut total = 0u64;
            for item in table.iter().map_err(|e| e.to_string())? {
                let (_, v) = item.map_err(|e| e.to_string())?;
                if let Ok(entry) = serde_json::from_slice::<LogEntry>(v.value()) {
                    total += 1;
                    *counts.entry(format!("level:{}", entry.level)).or_insert(0) += 1;
                    *counts.entry(format!("source:{}", entry.source)).or_insert(0) += 1;
                }
            }
            let mut counters = txn.open_table(COUNTERS).map_err(|e| e.to_string())?;
            let keys: Vec<String> = counters
                .range::<&str>(..)
                .map_err(|e| e.to_string())?
                .map(|r| r.map(|(k, _)| k.value().to_string()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            for key in keys {
                counters.remove(key.as_str()).map_err(|e| e.to_string())?;
            }
            counters
                .insert("total", total)
                .map_err(|e| e.to_string())?;
            for (key, value) in counts {
                counters
                    .insert(key.as_str(), value)
                    .map_err(|e| e.to_string())?;
            }
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(removed)
    }
}

fn bump(table: &mut redb::Table<'_, &str, u64>, key: &str, delta: u64) -> Result<(), String> {
    let current = table
        .get(key)
        .map_err(|e| e.to_string())?
        .map(|v| v.value())
        .unwrap_or(0);
    table
        .insert(key, current.saturating_add(delta))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, ts: i64, level: &str, source: &str, message: &str) -> LogEntry {
        LogEntry {
            id,
            ts,
            level: level.to_string(),
            source: source.to_string(),
            module: "test".to_string(),
            message: message.to_string(),
            meta: None,
        }
    }

    #[test]
    fn test_log_entry_serde_roundtrip() {
        let e = LogEntry {
            id: 1,
            ts: 1234567890,
            level: "INFO".to_string(),
            source: "backend".to_string(),
            module: "unidtk".to_string(),
            message: "hello world".to_string(),
            meta: Some("{\"k\":1}".to_string()),
        };
        let bytes = serde_json::to_vec(&e).unwrap();
        let back: LogEntry = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.id, 1);
        assert_eq!(back.level, "INFO");
        assert_eq!(back.message, "hello world");
        assert_eq!(back.meta.as_deref(), Some("{\"k\":1}"));
    }

    #[test]
    fn test_log_insert_list_clear() {
        let dir = std::env::temp_dir().join(format!("test_shell_logs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let store = LogStore::open(dir.join("logs.redb")).unwrap();
        store
            .insert_batch(&[entry(1, 1000, "INFO", "backend", "first"), entry(2, 2000, "ERROR", "frontend", "second")])
            .unwrap();

        let all = store.list(10, None, None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, 2);
        assert_eq!(all[1].id, 1);

        let front = store.list(10, None, Some("frontend")).unwrap();
        assert_eq!(front.len(), 1);
        assert_eq!(front[0].message, "second");

        let stats = store.stats().unwrap();
        assert_eq!(stats["total"], 2);
        assert_eq!(stats["by_level"]["ERROR"], 1);

        let removed = store.clear().unwrap();
        assert_eq!(removed, 2);
        assert_eq!(store.list(10, None, None).unwrap().len(), 0);
        assert_eq!(store.stats().unwrap()["total"], 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_log_sweep_retention() {
        let dir = std::env::temp_dir().join(format!("test_shell_logs_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let store = LogStore::open(dir.join("logs.redb")).unwrap();
        let fresh_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        store
            .insert_batch(&[entry(1, 1, "INFO", "backend", "old"), entry(2, fresh_ts, "INFO", "backend", "fresh")])
            .unwrap();

        let removed = store
            .sweep(LogRetention {
                max_age_days: 7,
                max_entries: 1000,
            })
            .unwrap();
        assert_eq!(removed, 1);

        let rest = store.list(10, None, None).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, 2);
        assert_eq!(store.stats().unwrap()["total"], 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_get_since_and_max_id() {
        let dir = std::env::temp_dir().join(format!("test_shell_logs_since_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let store = LogStore::open(dir.join("logs.redb")).unwrap();
        assert_eq!(store.max_id().unwrap(), 0);

        let entries: Vec<LogEntry> = (1..=5)
            .map(|i| entry(i, 1000 + i as i64, "INFO", "backend", &format!("msg {i}")))
            .collect();
        store.insert_batch(&entries).unwrap();
        assert_eq!(store.max_id().unwrap(), 5);

        let delta = store.get_since(2, None).unwrap();
        assert_eq!(delta.len(), 3);
        assert_eq!(delta[0].id, 3);
        assert_eq!(delta[2].id, 5);

        assert_eq!(store.get_since(5, None).unwrap().len(), 0);

        let delta_lim = store.get_since(1, Some(2)).unwrap();
        assert_eq!(delta_lim.len(), 2);
        assert_eq!(delta_lim[0].id, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
