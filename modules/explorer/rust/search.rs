use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub struct SearchEngine {
    index: HashMap<String, PathEntry>,
    max_results: usize,
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
        Self {
            index: HashMap::new(),
            max_results,
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

        self.index.insert(
            path.to_lowercase(),
            PathEntry {
                path: path.to_string(),
                name,
                is_dir,
                size,
                modified,
            },
        );
    }

    pub fn remove_path(&mut self, path: &str) {
        self.index.remove(&path.to_lowercase());
    }

    pub fn clear(&mut self) {
        self.index.clear();
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
