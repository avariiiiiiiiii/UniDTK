use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Hard cap on indexed paths. Past it the oldest entries are evicted first.
pub const MAX_INDEX_ENTRIES: usize = 20_000;
/// Folder levels below the indexed root that a crawl descends into.
pub const SEARCH_MAX_DEPTH: usize = 4;

pub struct SearchEngine {
    index: HashMap<String, IndexedPath>,
    /// Insertion order as (sequence, key). Entries left behind by removals or
    /// re-inserts no longer match the index and are skipped on eviction, then
    /// dropped by `compact_order` once they outnumber the live entries.
    order: VecDeque<(u64, String)>,
    next_seq: u64,
    max_results: usize,
    max_entries: usize,
}

struct IndexedPath {
    seq: u64,
    entry: PathEntry,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PathEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub score: f32,
}

impl SearchEngine {
    pub fn new(max_results: usize) -> Self {
        Self::with_limits(max_results, MAX_INDEX_ENTRIES)
    }

    pub fn with_limits(max_results: usize, max_entries: usize) -> Self {
        Self {
            index: HashMap::new(),
            order: VecDeque::new(),
            next_seq: 0,
            max_results,
            max_entries,
        }
    }

    pub fn add_path(&mut self, path: &str, is_dir: bool, size: Option<u64>, modified: Option<i64>) {
        let path_obj = Path::new(path);
        let name = path_obj
            .file_name()
            .or_else(|| path_obj.parent()?.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());

        if name.eq_ignore_ascii_case("desktop.ini") {
            return;
        }

        let key = path.to_lowercase();
        self.next_seq += 1;
        let seq = self.next_seq;
        self.order.push_back((seq, key.clone()));
        self.index.insert(
            key,
            IndexedPath {
                seq,
                entry: PathEntry {
                    path: path.to_string(),
                    name,
                    is_dir,
                    size,
                    modified,
                },
            },
        );

        while self.index.len() > self.max_entries {
            self.evict_oldest();
        }
        if self.order.len() > 2 * self.max_entries + 1 {
            self.compact_order();
        }
    }

    fn evict_oldest(&mut self) {
        while let Some((seq, key)) = self.order.pop_front() {
            if self.index.get(&key).map(|indexed| indexed.seq) == Some(seq) {
                self.index.remove(&key);
                return;
            }
        }
    }

    fn compact_order(&mut self) {
        let index = &self.index;
        self.order
            .retain(|(seq, key)| index.get(key).map(|indexed| indexed.seq) == Some(*seq));
    }

    pub fn remove_path(&mut self, path: &str) {
        self.index.remove(&path.to_lowercase());
    }

    pub fn clear(&mut self) {
        self.index.clear();
        self.order.clear();
    }

    /// Walks `root` breadth-first and indexes every entry down to `max_depth` levels.
    /// Reparse points are not followed, so a junction cannot loop the walk or pull in
    /// another volume. The walk stops at the entry cap instead of evicting the
    /// shallow entries it has already added. An unreadable root leaves the index as is.
    pub fn index_tree(&mut self, root: &Path, max_depth: usize) {
        let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
        queue.push_back((root.to_path_buf(), 0));
        while let Some((dir, depth)) = queue.pop_front() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if self.index.len() >= self.max_entries {
                    return;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if crate::files::is_reparse_point(&meta) {
                    continue;
                }
                let is_dir = meta.is_dir();
                let path = entry.path();
                self.add_path(&path.to_string_lossy(), is_dir, None, None);
                if is_dir && depth < max_depth {
                    queue.push_back((path, depth + 1));
                }
            }
        }
    }

    /// Search that also drops hits whose path no longer exists (deleted or renamed
    /// since indexing). Only the returned hits are checked, so one query costs at
    /// most `max_results` existence checks.
    pub fn search_checked(&mut self, query: &str) -> Vec<SearchResult> {
        let hits = self.search(query);
        hits.into_iter()
            .filter(|hit| {
                let alive = Path::new(&hit.path).exists();
                if !alive {
                    self.remove_path(&hit.path);
                }
                alive
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        if query.is_empty() {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let mut results: Vec<SearchResult> = self
            .index
            .values()
            .map(|indexed| &indexed.entry)
            .filter_map(|entry| {
                let name_lower = entry.name.to_lowercase();
                let path_lower = entry.path.to_lowercase();

                let score = if name_lower == query_lower {
                    1.0
                } else if name_lower.starts_with(&query_lower) {
                    0.9
                } else if name_lower.contains(&query_lower) {
                    0.7
                } else if path_lower.contains(&query_lower) {
                    0.5
                } else if fuzzy_match(&name_lower, &query_lower) {
                    0.3
                } else {
                    return None;
                };

                Some(SearchResult {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    is_dir: entry.is_dir,
                    score,
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(self.max_results);
        results
    }

    #[allow(dead_code)]
    pub fn search_with_extension(&self, query: &str, extension: &str) -> Vec<SearchResult> {
        let ext_lower = extension.to_lowercase();
        let all_results = self.search(query);

        all_results
            .into_iter()
            .filter(|r| {
                Path::new(&r.path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase() == ext_lower)
                    .unwrap_or(false)
            })
            .collect()
    }

    pub fn get_suggestions(&self, prefix: &str, limit: usize) -> Vec<String> {
        let prefix_lower = prefix.to_lowercase();
        let mut suggestions: Vec<String> = self
            .index
            .values()
            .map(|indexed| &indexed.entry)
            .filter(|entry| entry.name.to_lowercase().starts_with(&prefix_lower))
            .map(|entry| entry.name.clone())
            .collect();

        suggestions.sort();
        suggestions.dedup();
        suggestions.truncate(limit);
        suggestions
    }
}

fn fuzzy_match(text: &str, pattern: &str) -> bool {
    let mut pattern_chars = pattern.chars();
    let mut current = pattern_chars.next();

    for c in text.chars() {
        if let Some(q) = current {
            if c == q {
                current = pattern_chars.next();
            }
        } else {
            return true;
        }
    }

    current.is_none()
}

#[allow(dead_code)]
pub type SharedSearchEngine = Arc<Mutex<SearchEngine>>;

#[allow(dead_code)]
pub fn create_shared_engine(max_results: usize) -> SharedSearchEngine {
    Arc::new(Mutex::new(SearchEngine::new(max_results)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch directory under the system temp dir, removed on drop (also on panic).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("unidtk-search-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn index_evicts_the_oldest_entries_past_the_cap() {
        let mut engine = SearchEngine::with_limits(10, 3);
        for name in ["a", "b", "c", "d"] {
            engine.add_path(&format!("C:\\data\\{name}.txt"), false, None, None);
        }

        assert_eq!(engine.len(), 3);
        assert!(engine.search("a.txt").is_empty(), "oldest entry must be evicted");
        assert_eq!(engine.search("d.txt").len(), 1);
    }

    #[test]
    fn churn_on_one_path_keeps_the_eviction_queue_bounded() {
        let mut engine = SearchEngine::with_limits(10, 4);
        for _ in 0..1000 {
            engine.add_path("C:\\data\\same.txt", false, None, None);
            engine.remove_path("C:\\data\\same.txt");
        }

        assert_eq!(engine.len(), 0);
        assert!(engine.order.len() <= 2 * 4 + 1, "queue grew to {}", engine.order.len());
    }

    #[test]
    fn search_forgets_entries_whose_files_were_deleted_or_renamed() {
        let root = Scratch::new("stale");
        std::fs::write(root.0.join("kept.txt"), "k").unwrap();
        std::fs::write(root.0.join("gone.txt"), "g").unwrap();
        std::fs::write(root.0.join("old.txt"), "o").unwrap();
        let mut engine = SearchEngine::new(50);
        engine.index_tree(&root.0, 1);

        std::fs::remove_file(root.0.join("gone.txt")).unwrap();
        std::fs::rename(root.0.join("old.txt"), root.0.join("fresh.txt")).unwrap();

        assert!(engine.search_checked("gone").is_empty());
        assert!(engine.search_checked("old").is_empty());
        assert_eq!(engine.search_checked("kept").len(), 1);
        assert_eq!(engine.len(), 1, "stale entries must leave the index");
    }

    #[test]
    fn index_tree_respects_depth_and_the_entry_cap() {
        let root = Scratch::new("depth");
        std::fs::write(root.0.join("top.txt"), "t").unwrap();
        std::fs::create_dir_all(root.0.join("sub").join("deeper")).unwrap();
        std::fs::write(root.0.join("sub").join("mid.txt"), "m").unwrap();
        std::fs::write(root.0.join("sub").join("deeper").join("low.txt"), "l").unwrap();

        let mut shallow = SearchEngine::new(50);
        shallow.index_tree(&root.0, 1);
        assert!(!shallow.search("mid.txt").is_empty());
        assert!(shallow.search("low.txt").is_empty(), "depth 1 must not reach depth 2");

        let mut capped = SearchEngine::with_limits(50, 2);
        capped.index_tree(&root.0, 4);
        assert_eq!(capped.len(), 2, "crawl must stop at the entry cap");
    }

    #[cfg(windows)]
    #[test]
    fn index_tree_does_not_follow_junctions() {
        let root = Scratch::new("junction");
        let real = root.0.join("real");
        let outside = Scratch::new("junction-target");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("x.txt"), "x").unwrap();
        std::fs::write(outside.0.join("y.txt"), "y").unwrap();
        let junction = root.0.join("link");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", junction.to_str().unwrap(), outside.0.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());

        let mut engine = SearchEngine::new(50);
        engine.index_tree(&root.0, 4);

        assert!(engine.search_checked("y.txt").is_empty(), "junction target must not be indexed");
        assert_eq!(engine.search_checked("x.txt").len(), 1);
        std::fs::remove_dir(&junction).unwrap();
    }
}
