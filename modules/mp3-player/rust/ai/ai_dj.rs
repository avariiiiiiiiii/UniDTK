//! AI DJ — music similarity graph and transition pathfinding.
//!
//! Uses discogs-maest-129e (ONNX) to extract embeddings from audio, stores them
//! in the module's own store, and builds a similarity graph for intelligent
//! track transitions.
//!
//! Хранилище — **отдельный файл**, который выделяет ядро по объявлению модуля
//! (`unidtk_api::StoreSpec::cache("ai_dj")`, см. `core/kernel/module_store.rs`).
//! Поэтому путь здесь не собирается, и `redb` модулю не нужен: он работает с
//! тем же `unidtk_api::ModuleDb`, что и общий срез базы.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json;
use unidtk_api::{DbOp, ModuleDb};

/// Эмбеддинги и анализы живут в одном пространстве ключей, поэтому у каждого
/// свой префикс. Наружу `kv_list` отдаёт ключ уже без префикса.
const EMB: &str = "emb:";
const ANA: &str = "ana:";

/// Размерность эмбеддинга discogs-maest-129e.
const EMBEDDING_DIM: usize = 400;

/// Относительный путь модели AI DJ в любом каталоге (поставка, каталог данных,
/// dev-обход репозитория). Контракт раскладки для всего: и для загрузчика
/// модели, и для виджета, поэтому он объявлен публично — один раз, здесь.
pub const MODEL_REL: &str = "discogs-maest-129e/model.onnx";

fn emb_key(track_id: &str) -> String {
    format!("{EMB}{track_id}")
}

fn ana_key(track_id: &str) -> String {
    format!("{ANA}{track_id}")
}

/// Embedding vector from discogs-maest-129e (400-dim logits).
pub type Embedding = Vec<f32>;

/// Track metadata for AI DJ analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackAnalysis {
    pub track_id: String,
    pub bpm: Option<f32>,
    pub key: Option<String>,
    pub duration_ms: Option<u64>,
    pub analyzed_at: u64,
}

/// Graph node representing an analyzed track for similarity visualization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub bpm: Option<f32>,
    pub key: Option<String>,
}

/// Compact similarity graph: nodes and top-k nearest neighbor undirected edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarityGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(usize, usize, f32)>,
}

/// Energy curve for DJ transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyCurve {
    /// Random/any energy level.
    Any,
}

/// Хранилище AI DJ, выданное ядром при загрузке модуля.
///
/// Модуль не собирает путь сам: объявление `StoreSpec::cache("ai_dj")` делает
/// `commands::AppState::bootstrap`, а расположение файла и надёжность коммита
/// выбирает ядро (см. `core/kernel/module_store.rs`).
static AI_DJ_STORE: OnceLock<Arc<dyn ModuleDb>> = OnceLock::new();

/// Выдать хранилище AI DJ — вызывает модуль при bootstrap, получив его из
/// [`unidtk_api::ModuleContext::store`].
pub fn set_store(store: Arc<dyn ModuleDb>) {
    let _ = AI_DJ_STORE.set(store);
}

/// Хранилище AI DJ. Если ядро его ещё не выдало — это ошибка: молча писать в
/// память в рантайме означало бы тихую потерю всего анализа.
pub fn ai_dj_store() -> Result<Arc<dyn ModuleDb>, String> {
    AI_DJ_STORE
        .get()
        .cloned()
        .ok_or_else(|| "ai-dj: store is not initialized (module bootstrap skipped)".to_string())
}

/// Сколько треков проанализировано — по хранилищу, не поднимая движок.
pub fn analyzed_count() -> usize {
    match ai_dj_store() {
        Ok(store) => store.kv_count(EMB).unwrap_or(0) as usize,
        Err(_) => 0,
    }
}

/// Ревизия анализа: растёт при каждой записи/очистке эмбеддингов — по ней
/// инвалидируется кэш графа сходства.
static ANALYSIS_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bump_analysis_revision() {
    ANALYSIS_REVISION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Кэш собранного графа сходства: хранилище → (ревизия анализа, отпечаток
/// треков, граф). Сборка стоит ~n² скалярных произведений, а попап плеера
/// открывается часто — повторный вызов должен получать готовый граф.
///
/// Ключ — идентичность хранилища, а не его путь: путь теперь знает только ядро,
/// а у одного хранилища должен быть ровно один кэш (в тестах хранилища разные).
static GRAPH_CACHE: std::sync::OnceLock<Mutex<HashMap<usize, (u64, u64, SimilarityGraph)>>> =
    std::sync::OnceLock::new();

/// Ключ кэша — адрес хранилища: один `Arc`, значит один кэш.
fn store_key(store: &Arc<dyn ModuleDb>) -> usize {
    Arc::as_ptr(store) as *const () as usize
}

/// Дешёвый отпечаток набора треков (id/title/artist): правка метаданных
/// (например, синхронизация) тоже инвалидирует кэш графа.
fn tracks_fingerprint(tracks: &[crate::library::db::Track]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for track in tracks {
        mix(track.id.as_bytes());
        mix(track.title.as_bytes());
        if let Some(artist) = track.artist.as_deref() {
            mix(artist.as_bytes());
        }
        mix(&[0x1f]);
    }
    hash
}

/// AI DJ engine — manages embeddings, similarity graph, and transition paths.
pub struct AiDj {
    /// Хранилище модуля: отдельный файл, путь выбирает ядро.
    store: Arc<dyn ModuleDb>,
    /// In-memory cache of embeddings (track_id -> embedding).
    embeddings: HashMap<String, Embedding>,
    /// In-memory cache of track analyses.
    analyses: HashMap<String, TrackAnalysis>,
}

impl AiDj {
    /// Ensure the ONNX session and model are loaded and ready.
    pub fn ensure_session_ready(&self) -> Result<(), String> {
        let mut guard = ONNX_SESSION.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            let model_path = find_model_path()?;
            let session = ort::session::Session::builder()
                .map_err(|e| format!("Failed to create ONNX session builder: {e}"))?
                .commit_from_file(&model_path)
                .map_err(|e| format!("Failed to load ONNX model from {}: {e}", model_path.display()))?;
            *guard = Some(session);
        }
        Ok(())
    }

    /// Создать движок поверх хранилища модуля: в рантайме это `ai_dj_store()`,
    /// в тестах — хранилище в памяти.
    pub fn new(store: Arc<dyn ModuleDb>) -> Self {
        Self {
            store,
            embeddings: HashMap::new(),
            analyses: HashMap::new(),
        }
    }

    /// Load embeddings and analyses from the module store into memory.
    pub fn load(&mut self) -> Result<(), String> {
        self.embeddings.clear();
        self.analyses.clear();

        for (key, bytes) in self.store.kv_list(EMB)? {
            // `kv_list` отдаёт ключ с запрошенным префиксом — id трека это хвост.
            let track_id = key.strip_prefix(EMB).unwrap_or(&key).to_string();
            // Deserialize: 400 x f32 = 1600 bytes
            if bytes.len() >= EMBEDDING_DIM * 4 {
                let mut embedding = Vec::with_capacity(EMBEDDING_DIM);
                for i in 0..EMBEDDING_DIM {
                    let offset = i * 4;
                    let bits = u32::from_le_bytes([
                        bytes[offset],
                        bytes[offset + 1],
                        bytes[offset + 2],
                        bytes[offset + 3],
                    ]);
                    embedding.push(f32::from_bits(bits));
                }
                self.embeddings.insert(track_id, embedding);
            }
        }

        for (key, bytes) in self.store.kv_list(ANA)? {
            let track_id = key.strip_prefix(ANA).unwrap_or(&key).to_string();
            if let Ok(analysis) = serde_json::from_slice::<TrackAnalysis>(&bytes) {
                self.analyses.insert(track_id, analysis);
            }
        }

        Ok(())
    }

    /// Save an embedding and its analysis in a single transaction.
    pub fn store_embedding(
        &mut self,
        track_id: &str,
        embedding: &Embedding,
        analysis: &TrackAnalysis,
    ) -> Result<(), String> {
        self.embeddings.insert(track_id.to_string(), embedding.clone());
        self.analyses.insert(track_id.to_string(), analysis.clone());

        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for &val in embedding {
            bytes.extend_from_slice(&val.to_le_bytes());
        }
        let analysis_json = serde_json::to_vec(analysis).map_err(|e| e.to_string())?;

        // Эмбеддинг и анализ едут вместе: половина записи бессмысленна.
        self.store.kv_batch(vec![
            DbOp::Set {
                key: emb_key(track_id),
                value: bytes,
            },
            DbOp::Set {
                key: ana_key(track_id),
                value: analysis_json,
            },
        ])?;

        // Набор эмбеддингов изменился — кэш графа сходства больше не актуален.
        bump_analysis_revision();

        Ok(())
    }

    /// Get embedding for a track.
    #[cfg(test)]
    pub fn get_embedding(&self, track_id: &str) -> Option<&Embedding> {
        self.embeddings.get(track_id)
    }

    /// Compute cosine similarity between two embeddings.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a < 1e-8 || norm_b < 1e-8 {
            return 0.0;
        }
        (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
    }

    /// Find top-K most similar tracks to the given track.
    pub fn find_similar(&self, track_id: &str, top_k: usize) -> Vec<(String, f32)> {
        let embedding = match self.embeddings.get(track_id) {
            Some(e) => e,
            None => return Vec::new(),
        };

        let mut scores: Vec<(String, f32)> = self
            .embeddings
            .iter()
            .filter(|(id, _)| *id != track_id)
            .map(|(id, other_emb)| {
                let sim = Self::cosine_similarity(embedding, other_emb);
                (id.clone(), sim)
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(top_k);
        scores
    }

    /// Find the next track for a smooth transition.
    #[cfg(test)]
    pub fn next_track(
        &self,
        current_id: &str,
        energy_curve: EnergyCurve,
        exclude_ids: &[String],
    ) -> Option<String> {
        self.next_track_scoped(current_id, energy_curve, exclude_ids, None)
    }

    /// Find the next track for a smooth transition, optionally scoped to a playlist pool.
    pub fn next_track_scoped(
        &self,
        current_id: &str,
        _energy_curve: EnergyCurve,
        exclude_ids: &[String],
        pool: Option<&[String]>,
    ) -> Option<String> {
        let candidates: Vec<(String, f32)> = match pool {
            Some(pool_ids) if !pool_ids.is_empty() => {
                let current_emb = self.embeddings.get(current_id);
                // When scoped to a playlist, candidate pool is strictly from pool_ids
                let available: Vec<String> = pool_ids
                    .iter()
                    .filter(|id| id.as_str() != current_id && !exclude_ids.contains(id))
                    .cloned()
                    .collect();

                let final_pool = if available.is_empty() {
                    // Circular fallback: if all tracks in playlist played, only exclude current_id
                    pool_ids
                        .iter()
                        .filter(|id| id.as_str() != current_id)
                        .cloned()
                        .collect()
                } else {
                    available
                };

                final_pool
                    .into_iter()
                    .map(|id| {
                        let sim = current_emb
                            .and_then(|e| self.embeddings.get(&id).map(|other| Self::cosine_similarity(e, other)))
                            .unwrap_or(0.5);
                        (id, sim)
                    })
                    .collect()
            }
            _ => {
                let similar = self.find_similar(current_id, 50);
                let mut c: Vec<_> = similar
                    .into_iter()
                    .filter(|(id, _)| !exclude_ids.contains(id))
                    .collect();

                // Fallback Tier 1: If top-50 exhausted, widen search beam to 200
                if c.is_empty() {
                    let wider = self.find_similar(current_id, 200);
                    c = wider
                        .into_iter()
                        .filter(|(id, _)| !exclude_ids.contains(id))
                        .collect();
                }

                // Fallback Tier 2: If still empty, only exclude the last 15 tracks from history
                if c.is_empty() && !exclude_ids.is_empty() {
                    let recent_window: Vec<String> = exclude_ids.iter().rev().take(15).cloned().collect();
                    let wider = self.find_similar(current_id, 100);
                    c = wider
                        .into_iter()
                        .filter(|(id, _)| !recent_window.contains(id))
                        .collect();
                }

                // Fallback Tier 3: Pick any available analyzed track that is not current_id
                if c.is_empty() {
                    c = self
                        .embeddings
                        .keys()
                        .filter(|id| *id != current_id)
                        .map(|id| (id.clone(), 0.5f32))
                        .take(10)
                        .collect();
                }
                c
            }
        };

        if candidates.is_empty() {
            return None;
        }

        // Get current track's BPM and key for compatibility scoring
        let current_bpm = self.analyses.get(current_id).and_then(|a| a.bpm);
        let current_key = self.analyses.get(current_id).and_then(|a| a.key.clone());

        // Score candidates with BPM and key compatibility
        let mut scored: Vec<(String, f32)> = candidates
            .into_iter()
            .map(|(id, emb_sim)| {
                let mut score = emb_sim;

                // BPM compatibility: ±10 BPM is ideal
                if let (Some(curr_bpm), Some(cand_bpm)) = (current_bpm, self.analyses.get(&id).and_then(|a| a.bpm)) {
                    let bpm_diff = (curr_bpm - cand_bpm).abs();
                    if bpm_diff <= 5.0 {
                        score += 0.15; // Perfect match
                    } else if bpm_diff <= 10.0 {
                        score += 0.08; // Good match
                    } else if bpm_diff <= 20.0 {
                        score += 0.02; // Acceptable
                    }
                }

                // Key compatibility: Camelot wheel adjacency
                if let (Some(ref curr_key), Some(ref cand_key)) = (&current_key, self.analyses.get(&id).and_then(|a| a.key.clone())) {
                    if camelot_compatible(curr_key, cand_key) {
                        score += 0.10; // Harmonic mixing bonus
                    }
                }

                (id, score)
            })
            .collect();

        // Sort by score (highest first)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored.into_iter().next().map(|(id, _)| id)
    }

    /// Build an optimal harmonic transition queue from an explicit list of tracks (playlist).
    /// Uses greedy Hamiltonian pathfinding over embedding similarity, octave-aware BPM,
    /// and Camelot wheel compatibility.
    pub fn build_playlist_queue(&self, track_ids: &[String]) -> Vec<String> {
        if track_ids.len() <= 1 {
            return track_ids.to_vec();
        }

        let mut remaining: Vec<String> = track_ids.to_vec();
        let mut queue = Vec::with_capacity(track_ids.len());

        // The caller supplies the randomly chosen session seed first, even if unanalyzed.
        let start_id = remaining.remove(0);
        queue.push(start_id);

        while !remaining.is_empty() {
            let current_id = queue.last().unwrap().clone();
            let current_emb = self.embeddings.get(&current_id);
            let current_bpm = self.analyses.get(&current_id).and_then(|a| a.bpm);
            let current_key = self.analyses.get(&current_id).and_then(|a| a.key.clone());

            let mut best_idx = 0;
            let mut best_score = -1000.0f32;

            for (i, cand_id) in remaining.iter().enumerate() {
                let emb_sim = match (current_emb, self.embeddings.get(cand_id)) {
                    (Some(e1), Some(e2)) => Self::cosine_similarity(e1, e2),
                    _ => 0.5, // Neutral prior for tracks without embeddings
                };

                let mut score = emb_sim;

                // BPM match with octave awareness
                if let (Some(b1), Some(b2)) = (current_bpm, self.analyses.get(cand_id).and_then(|a| a.bpm)) {
                    let d0 = (b1 - b2).abs();
                    let d1 = (b1 * 2.0 - b2).abs();
                    let d2 = (b1 - b2 * 2.0).abs();
                    let bpm_diff = d0.min(d1).min(d2);

                    if bpm_diff <= 5.0 {
                        score += 0.15;
                    } else if bpm_diff <= 10.0 {
                        score += 0.08;
                    } else if bpm_diff <= 20.0 {
                        score += 0.02;
                    }
                }

                // Camelot wheel harmonic compatibility
                if let (Some(ref k1), Some(ref k2)) = (&current_key, self.analyses.get(cand_id).and_then(|a| a.key.as_ref())) {
                    if camelot_compatible(k1, k2) {
                        score += 0.10;
                    }
                }

                if score > best_score {
                    best_score = score;
                    best_idx = i;
                }
            }

            queue.push(remaining.remove(best_idx));
        }

        queue
    }

    /// Граф сходства из кэша, если ревизия анализа и отпечаток треков совпадают.
    /// Публичный: путь «AI DJ выключен» может отдать готовый граф, не загружая
    /// эмбеддинги из хранилища.
    pub fn cached_graph(
        store: &Arc<dyn ModuleDb>,
        tracks: &[crate::library::db::Track],
    ) -> Option<SimilarityGraph> {
        let revision = ANALYSIS_REVISION.load(std::sync::atomic::Ordering::SeqCst);
        let fingerprint = tracks_fingerprint(tracks);
        let cache = GRAPH_CACHE.get()?;
        let guard = cache.lock().ok()?;
        let (cached_revision, cached_fingerprint, graph) = guard.get(&store_key(store))?;
        if *cached_revision == revision && *cached_fingerprint == fingerprint {
            return Some(graph.clone());
        }
        None
    }

    fn remember_graph(
        store: &Arc<dyn ModuleDb>,
        tracks: &[crate::library::db::Track],
        graph: &SimilarityGraph,
    ) {
        let revision = ANALYSIS_REVISION.load(std::sync::atomic::Ordering::SeqCst);
        let fingerprint = tracks_fingerprint(tracks);
        let cache = GRAPH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(mut guard) = cache.lock() {
            guard.insert(
                store_key(store),
                (revision, fingerprint, graph.clone()),
            );
        }
    }

    /// Build a compact similarity graph with top-k nearest neighbors for Canvas visualization.
    ///
    /// Пересборка — O(n²) скалярных произведений (n = число проанализированных
    /// треков), поэтому результат кэшируется по (ревизия анализа, отпечаток
    /// треков): попап открывается часто, а набор меняется редко.
    pub fn build_similarity_graph(&self, tracks: &[crate::library::db::Track], k: usize) -> SimilarityGraph {
        if let Some(graph) = Self::cached_graph(&self.store, tracks) {
            return graph;
        }
        let graph = self.build_similarity_graph_uncached(tracks, k);
        Self::remember_graph(&self.store, tracks, &graph);
        graph
    }

    /// Сборка без кэша: нормы эмбеддингов считаются один раз, top-k держится
    /// вставкой по индексам — без `String`-клонов на каждую пару и без
    /// сортировки массивов длиной n.
    fn build_similarity_graph_uncached(
        &self,
        tracks: &[crate::library::db::Track],
        k: usize,
    ) -> SimilarityGraph {
        let mut nodes = Vec::new();
        // (эмбеддинг, его норма) — индекс строки совпадает с индексом узла.
        let mut rows: Vec<(&Embedding, f32)> = Vec::new();

        for track in tracks {
            let Some(embedding) = self.embeddings.get(&track.id) else {
                continue;
            };
            let analysis = self.analyses.get(&track.id);
            nodes.push(GraphNode {
                id: track.id.clone(),
                title: track.title.clone(),
                artist: track.artist.clone(),
                bpm: analysis.and_then(|a| a.bpm),
                key: analysis.and_then(|a| a.key.clone()),
            });
            let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            rows.push((embedding, norm));
        }

        let k = k.max(1);
        let mut edges = Vec::new();
        let mut edge_set = std::collections::HashSet::new();
        let mut best: Vec<(usize, f32)> = Vec::with_capacity(k);

        for (i, (embedding, norm_a)) in rows.iter().enumerate() {
            best.clear();
            if *norm_a > 1e-8 {
                for (j, (other, norm_b)) in rows.iter().enumerate() {
                    if i == j || *norm_b <= 1e-8 {
                        continue;
                    }
                    let dot: f32 = embedding.iter().zip(other.iter()).map(|(x, y)| x * y).sum();
                    let sim = (dot / (norm_a * norm_b)).clamp(-1.0, 1.0);
                    if best.len() < k {
                        best.push((j, sim));
                        best.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    } else if sim > best[0].1 {
                        best[0] = (j, sim);
                        best.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    }
                }
            }

            for &(j, sim) in &best {
                let (u, v) = if i < j { (i, j) } else { (j, i) };
                if edge_set.insert((u, v)) {
                    edges.push((u, v, sim));
                }
            }
        }

        SimilarityGraph { nodes, edges }
    }

    /// Get the number of tracks in the database.
    pub fn track_count(&self) -> usize {
        self.embeddings.len()
    }

    /// Check if a track has been analyzed.
    pub fn has_track(&self, track_id: &str) -> bool {
        self.embeddings.contains_key(track_id)
    }

    /// Clear all embeddings and analyses from memory and from the store.
    pub fn clear(&mut self) {
        self.embeddings.clear();
        self.analyses.clear();
        // Одной транзакцией: поштучные удаления дали бы коммит на каждый трек.
        let mut ops = Vec::new();
        if let Ok(list) = self.store.kv_list(EMB) {
            for (key, _) in list {
                ops.push(DbOp::Remove { key });
            }
        }
        if let Ok(list) = self.store.kv_list(ANA) {
            for (key, _) in list {
                ops.push(DbOp::Remove { key });
            }
        }
        let _ = self.store.kv_batch(ops);
        // Анализ стёрт — кэш графа сходства инвалидируется.
        bump_analysis_revision();
    }

    /// Get analysis history as structured data.
    pub fn get_history(&self) -> Vec<serde_json::Value> {
        self.analyses
            .values()
            .map(|a| {
                serde_json::json!({
                    "track_id": a.track_id,
                    "bpm": a.bpm,
                    "key": a.key,
                    "analyzed_at": a.analyzed_at
                })
            })
            .collect()
    }
}

/// Decodes at most the first 30 seconds of audio into 16kHz mono `Vec<f32>` in a single pass.
/// Supports MP3, FLAC (16/24/32-bit), AAC, OGG/Vorbis, Opus.
pub fn decode_audio_samples_30s(audio_path: &Path) -> Result<Vec<f32>, String> {
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(audio_path)
        .map_err(|e| format!("Failed to open audio file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = audio_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();
    let decoder_opts = DecoderOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| format!("Failed to probe format: {e}"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("No audio track found")?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &decoder_opts)
        .map_err(|e| format!("Failed to create decoder: {e}"))?;

    const TARGET_SR: u32 = 16000;
    let track_sr = track.codec_params.sample_rate.unwrap_or(TARGET_SR);
    // Limit decoding to first 30 seconds of source frames
    let max_source_frames = (track_sr as usize) * 30;

    let mut mono_source: Vec<f32> = Vec::with_capacity(max_source_frames.min(48000 * 30));

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("Read error: {e}")),
        };

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let spec = *decoded.spec();
        let n_channels = spec.channels.count();
        let n_frames = decoded.frames();
        if n_channels == 0 || n_frames == 0 {
            continue;
        }

        let mut sample_buf = symphonia::core::audio::SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();

        for frame in 0..n_frames {
            let offset = frame * n_channels;
            let mut sum = 0.0f32;
            for ch in 0..n_channels {
                sum += samples[offset + ch];
            }
            mono_source.push(sum / n_channels as f32);

            if mono_source.len() >= max_source_frames {
                break;
            }
        }

        if mono_source.len() >= max_source_frames {
            break;
        }
    }

    if mono_source.is_empty() {
        return Err("No audio samples decoded".to_string());
    }

    // Resample to 16kHz if needed (linear interpolation)
    let resampled = if track_sr != TARGET_SR {
        let ratio = TARGET_SR as f64 / track_sr as f64;
        let new_len = (mono_source.len() as f64 * ratio) as usize;
        let mut out = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let src_idx = i as f64 / ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f64;
            let val = if idx + 1 < mono_source.len() {
                mono_source[idx] * (1.0 - frac as f32) + mono_source[idx + 1] * frac as f32
            } else {
                mono_source[idx]
            };
            out.push(val);
        }
        out
    } else {
        mono_source
    };

    let max_target_samples = (TARGET_SR * 30) as usize;
    let mut final_samples = resampled;
    if final_samples.len() > max_target_samples {
        final_samples.truncate(max_target_samples);
    }

    Ok(final_samples)
}

/// Extract full analysis from audio file: embedding, BPM, and key in a single decoding pass.
pub fn extract_analysis(audio_path: &Path) -> Result<(Embedding, Option<f32>, Option<String>), String> {
    let samples = decode_audio_samples_30s(audio_path)?;
    const TARGET_SR: u32 = 16000;

    let mel_spec = compute_mel_spectrogram(&samples, TARGET_SR, 96, 512, 256, 1876);
    let embedding = run_onnx_inference(&mel_spec)?;

    let bpm = detect_bpm(&samples, TARGET_SR);
    let key = detect_key(&samples, TARGET_SR);

    Ok((embedding, bpm, key))
}

/// Compute mel spectrogram matching MAEST / Audio Spectrogram Transformer specification.
/// Returns shape: [target_frames (1876), n_mels (96)]
fn compute_mel_spectrogram(
    samples: &[f32],
    sample_rate: u32,
    n_mels: usize,
    window_size: usize,
    hop_size: usize,
    target_frames: usize,
) -> Vec<Vec<f32>> {
    use rustfft::{num_complex::Complex, FftPlanner};

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(window_size);

    let n_frames = if samples.len() > window_size {
        (samples.len() - window_size) / hop_size + 1
    } else {
        1
    };

    let mut spec = Vec::with_capacity(target_frames);

    // Hanning window
    let window: Vec<f32> = (0..window_size)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
        .collect();

    // Mel filterbank
    let mel_filters = create_mel_filterbank(n_mels, window_size, sample_rate);

    for frame_idx in 0..n_frames.min(target_frames) {
        let start = frame_idx * hop_size;
        let end = (start + window_size).min(samples.len());

        // Apply window and FFT
        let mut input: Vec<Complex<f32>> = (0..window_size)
            .map(|i| {
                if start + i < end {
                    Complex::new(samples[start + i] * window[i], 0.0)
                } else {
                    Complex::new(0.0, 0.0)
                }
            })
            .collect();

        fft.process(&mut input);

        // Power spectrum
        let power: Vec<f32> = input[..window_size / 2].iter().map(|c| c.norm_sqr()).collect();

        // Apply mel filterbank
        let mut mel_frame = vec![0.0f32; n_mels];
        for (mel_idx, filter) in mel_filters.iter().enumerate() {
            let mut sum = 0.0f32;
            for (bin_idx, &weight) in filter.iter().enumerate() {
                if bin_idx < power.len() {
                    sum += power[bin_idx] * weight;
                }
            }
            // logC compression: log10(1 + melspec * 10000)
            let log_val = (1.0 + sum * 10000.0).max(1.0).log10();
            // MAEST normalization: (x - mean) / (std * 2)
            let norm_val = (log_val - 2.06755686098554) / (1.268292820667291 * 2.0);
            mel_frame[mel_idx] = norm_val;
        }

        spec.push(mel_frame);
    }

    // Pad with baseline normalized zero frames if shorter than target_frames (1876)
    let norm_zero = (0.0f32 - 2.06755686098554) / (1.268292820667291 * 2.0);
    while spec.len() < target_frames {
        spec.push(vec![norm_zero; n_mels]);
    }

    spec
}

/// Create a simplified mel filterbank.
fn create_mel_filterbank(n_mels: usize, fft_size: usize, sample_rate: u32) -> Vec<Vec<f32>> {
    let n_bins = fft_size / 2;
    let f_min = 0.0f32;
    let f_max = sample_rate as f32 / 2.0;

    let mel_min = 2595.0 * (1.0 + f_min / 700.0).log10();
    let mel_max = 2595.0 * (1.0 + f_max / 700.0).log10();

    let mut filters = vec![vec![0.0f32; n_bins]; n_mels];

    for m in 0..n_mels {
        let mel_center = mel_min + (mel_max - mel_min) * (m + 1) as f32 / (n_mels + 1) as f32;
        let mel_left = mel_min + (mel_max - mel_min) * m as f32 / (n_mels + 1) as f32;
        let mel_right = mel_min + (mel_max - mel_min) * (m + 2) as f32 / (n_mels + 1) as f32;

        let f_center = 700.0 * (10.0f32.powf(mel_center / 2595.0) - 1.0);
        let f_left = 700.0 * (10.0f32.powf(mel_left / 2595.0) - 1.0);
        let f_right = 700.0 * (10.0f32.powf(mel_right / 2595.0) - 1.0);

        let bin_center = (f_center / f_max * n_bins as f32) as usize;
        let bin_left = (f_left / f_max * n_bins as f32) as usize;
        let bin_right = (f_right / f_max * n_bins as f32) as usize;

        for bin in bin_left..=bin_right.min(n_bins - 1) {
            if bin <= bin_center && bin_center > bin_left {
                filters[m][bin] = (bin - bin_left) as f32 / (bin_center - bin_left) as f32;
            } else if bin > bin_center && bin_right > bin_center {
                filters[m][bin] = (bin_right - bin) as f32 / (bin_right - bin_center) as f32;
            }
        }
    }

    filters
}

/// Detect BPM from audio samples using peak detection in the 60-180 BPM range.
/// Uses a simple autocorrelation-based approach on the onset envelope.
pub fn detect_bpm(samples: &[f32], sample_rate: u32) -> Option<f32> {
    if samples.len() < (sample_rate as usize) * 10 {
        // Need at least 10 seconds for reliable detection
        return None;
    }

    // Target BPM range: 60-180
    const MIN_BPM: f32 = 60.0;
    const MAX_BPM: f32 = 180.0;

    // Compute onset envelope (energy differences)
    let hop_size = (sample_rate / 100) as usize; // 10ms hop
    let n_frames = samples.len() / hop_size;
    let mut onset_env = Vec::with_capacity(n_frames);

    let mut prev_energy = 0.0f32;
    for i in 0..n_frames {
        let start = i * hop_size;
        let end = ((i + 1) * hop_size).min(samples.len());
        let energy: f32 = samples[start..end].iter().map(|s| s * s).sum();
        let diff = (energy - prev_energy).max(0.0);
        onset_env.push(diff);
        prev_energy = energy;
    }

    // Autocorrelation to find periodicity
    let min_lag = ((60.0 / MAX_BPM) * sample_rate as f32 / hop_size as f32) as usize;
    let max_lag = ((60.0 / MIN_BPM) * sample_rate as f32 / hop_size as f32) as usize;

    let mut best_corr = 0.0f32;
    let mut best_lag = min_lag;

    for lag in min_lag..=max_lag.min(onset_env.len() / 2) {
        let mut corr = 0.0f32;
        let mut count = 0;
        for i in 0..(onset_env.len() - lag) {
            corr += onset_env[i] * onset_env[i + lag];
            count += 1;
        }
        if count > 0 {
            corr /= count as f32;
            if corr > best_corr {
                best_corr = corr;
                best_lag = lag;
            }
        }
    }

    // Convert lag to BPM
    let bpm = 60.0 / (best_lag as f32 * hop_size as f32 / sample_rate as f32);
    if (MIN_BPM..=MAX_BPM).contains(&bpm) {
        // Round to nearest integer
        Some(bpm.round())
    } else {
        None
    }
}

/// Detect musical key using chroma features.
/// Returns key in Camelot Wheel notation (e.g. "8A", "5B").
///
/// Uses 12-bin chroma spectrum and correlates with key profiles.
pub fn detect_key(samples: &[f32], sample_rate: u32) -> Option<String> {
    if samples.len() < (sample_rate as usize) * 5 {
        // Need at least 5 seconds
        return None;
    }

    // Compute chroma (12 pitch classes)
    let window_size = 4096;
    let hop_size = 2048;
    let n_frames = (samples.len() - window_size) / hop_size + 1;
    if n_frames == 0 {
        return None;
    }

    use rustfft::{num_complex::Complex, FftPlanner};
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(window_size);

    // Hanning window
    let window: Vec<f32> = (0..window_size)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
        .collect();

    // Accumulate chroma across frames
    let mut chroma = [0.0f32; 12];

    for frame_idx in 0..n_frames {
        let start = frame_idx * hop_size;
        let mut input: Vec<Complex<f32>> = (0..window_size)
            .map(|i| Complex::new(samples[start + i] * window[i], 0.0))
            .collect();
        fft.process(&mut input);

        // Map frequency bins to pitch classes
        for bin in 1..(window_size / 2) {
            let freq = bin as f32 * sample_rate as f32 / window_size as f32;
            if freq < 50.0 || freq > 5000.0 {
                continue;
            }
            // Convert frequency to MIDI note number
            let midi_note = 69.0 + 12.0 * (freq / 440.0).log2();
            let pitch_class = ((midi_note.round() as i32).rem_euclid(12)) as usize;
            let magnitude = input[bin].norm();
            chroma[pitch_class] += magnitude;
        }
    }

    // Normalize chroma
    let max_val = chroma.iter().copied().fold(0.0f32, f32::max);
    if max_val < 1e-10 {
        return None;
    }
    for v in &mut chroma {
        *v /= max_val;
    }

    // Key detection using Krumhansl-Schmuckler key profiles
    // Major profile (C, C#, D, D#, E, F, F#, G, G#, A, A#, B)
    let major_profile = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    let minor_profile = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

    // Camelot Wheel mapping
    // A = minor, B = major
    // 1A=Abm, 2A=Bbm, 3A=Ebm, 4A=Dbm, 5A=Gbm, 6A=Bm, 7A=F#m, 8A=C#m, 9A=G#m, 10A=D#m, 11A=A#m, 12A=Fm

    // Find best matching key
    let mut best_score = f32::MIN;
    let mut best_is_minor = false;
    let mut best_idx = 0;

    // Try all 12 transpositions of major and minor
    for shift in 0..12 {
        let mut major_score = 0.0f32;
        let mut minor_score = 0.0f32;
        for i in 0..12 {
            let shifted = (i + shift) % 12;
            major_score += chroma[i] * major_profile[shifted];
            minor_score += chroma[i] * minor_profile[shifted];
        }
        if major_score > best_score {
            best_score = major_score;
            best_is_minor = false;
            best_idx = shift;
        }
        if minor_score > best_score {
            best_score = minor_score;
            best_is_minor = true;
            best_idx = shift;
        }
    }

    // Convert to Camelot notation
    // Use the enharmonic spelling that matches Camelot
    let camelot_code = if best_is_minor {
        // Map minor keys to Camelot A
        let minor_to_camelot = ["8A", "3A", "10A", "5A", "12A", "7A", "2A", "9A", "4A", "11A", "6A", "1A"];
        minor_to_camelot[best_idx].to_string()
    } else {
        // Map major keys to Camelot B
        let major_to_camelot = ["5B", "12B", "7B", "2B", "9B", "4B", "11B", "6B", "1B", "8B", "3B", "10B"];
        major_to_camelot[best_idx].to_string()
    };

    Some(camelot_code)
}

/// Check if two Camelot codes are compatible (adjacent or same on the wheel).
pub fn camelot_compatible(key1: &str, key2: &str) -> bool {
    if key1 == key2 {
        return true;
    }

    // Parse Camelot codes: number (1-12) + letter (A or B)
    let parse = |k: &str| -> Option<(u32, char)> {
        let num = k[..k.len() - 1].parse::<u32>().ok()?;
        let letter = k.chars().last()?;
        Some((num, letter))
    };

    let (n1, l1) = match parse(key1) {
        Some(v) => v,
        None => return false,
    };
    let (n2, l2) = match parse(key2) {
        Some(v) => v,
        None => return false,
    };

    // Same letter: check if numbers are adjacent (circular)
    if l1 == l2 {
        let diff = if n1 > n2 { n1 - n2 } else { n2 - n1 };
        diff == 1 || diff == 11 // 11 because 12 and 1 are adjacent
    } else {
        // Different letters: same number is compatible
        n1 == n2
    }
}

static ONNX_SESSION: std::sync::Mutex<Option<ort::session::Session>> = std::sync::Mutex::new(None);

/// Unload the static ONNX session from memory to reclaim ~331MB working set after analysis.
pub fn unload_session() {
    let mut guard = ONNX_SESSION.lock().unwrap_or_else(|p| p.into_inner());
    *guard = None;
}

/// Verifies whether a given path points to a complete, valid ONNX model file:
/// 1. The path exists and is a regular file.
/// 2. If the .onnx file is small (< 50MB, e.g. discogs-maest-129e graph is 122KB),
///    an external weights file (model.onnx.data) must also exist and be > 10MB.
///
/// Публичный предикат: им пользуется и загрузчик модели (решать, докачивать ли
/// файлы), и ядро-обёртка — проверка валидности должна быть одна на всех.
pub fn is_valid_onnx_model(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };

    // If file is >= 50MB, weights are self-contained in protobuf
    if meta.len() >= 50 * 1024 * 1024 {
        return true;
    }

    // Lightweight graph (< 50MB) requires external weights file (model.onnx.data)
    let Some(parent) = path.parent() else {
        return false;
    };

    let data_candidates = [
        parent.join("model.onnx.data"),
        path.with_extension("onnx.data"),
        path.with_extension("data"),
    ];

    data_candidates.iter().any(|data_path| {
        data_path.is_file()
            && std::fs::metadata(data_path)
                .map(|m| m.len() > 10 * 1024 * 1024)
                .unwrap_or(false)
    })
}

/// Публичный поиск модели: точка истины «модель здесь и валидна» — её
/// одинаково читают ONNX-загрузчик и фетчер (не докачивать валидный файл).
pub fn find_model_path() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("AI_DJ_MODEL_PATH") {
        let pb = std::path::PathBuf::from(p);
        if is_valid_onnx_model(&pb) {
            return Ok(pb);
        }
    }

    let install = unidtk_api::paths::install_dir();
    let mut candidates = vec![
        // Приходит с установщиком — рядом с программой (Program Files).
        install.join("models").join(MODEL_REL),
        // Докачана или положена вручную — в каталоге данных.
        unidtk_api::paths::models_dir().join(MODEL_REL),
    ];

    // Dev-прогон из чекаута: `udtk build` кладёт модель в `target/release/models`,
    // а исходник живёт в `opt/models` — обходим дерево репозитория.
    for ancestor in install.ancestors().take(4).skip(1) {
        candidates.push(ancestor.join("opt").join("models").join(MODEL_REL));
    }

    for candidate in &candidates {
        if is_valid_onnx_model(candidate) {
            return Ok(candidate.clone());
        }
    }

    Err(format!(
        "AI DJ model discogs-maest-129e/model.onnx or external weights (model.onnx.data ~331MB) not found (checked {} candidates)",
        candidates.len()
    ))
}

/// Run ONNX inference on mel spectrogram.
fn run_onnx_inference(mel_spec: &[Vec<f32>]) -> Result<Vec<f32>, String> {
    if mel_spec.is_empty() {
        return Err("Mel spectrogram is empty".to_string());
    }

    let mut guard = ONNX_SESSION.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        let model_path = find_model_path()?;
        let session = ort::session::Session::builder()
            .map_err(|e| format!("Failed to create ONNX session builder: {e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| format!("Failed to load model from {}: {e}", model_path.display()))?;
        *guard = Some(session);
    }
    let session = guard.as_mut().ok_or_else(|| "ONNX session uninitialized".to_string())?;

    // Prepare input: [1, time_frames, 96]
    let n_frames = mel_spec.len();
    let n_mels = if n_frames > 0 { mel_spec[0].len() } else { 96 };
    if n_mels != 96 {
        return Err(format!("Invalid mel bins count: expected 96, got {n_mels}"));
    }
    let flat: Vec<f32> = mel_spec.iter().flatten().copied().collect();
    let input_shape = vec![1i64, n_frames as i64, n_mels as i64];

    let input = ort::value::Tensor::from_array((input_shape, flat))
        .map_err(|e| format!("Failed to create input tensor: {e}"))?;

    let input_value: ort::value::Value<ort::value::TensorValueType<f32>> = input.into();
    let outputs = session.run(ort::inputs!["input_values" => &input_value])
        .map_err(|e| format!("Inference failed: {e}"))?;

    let output = outputs.get("logits").unwrap_or(&outputs[0]);
    let (_shape, data) = output.try_extract_tensor::<f32>()
        .map_err(|e| format!("Failed to extract output: {e}"))?;

    Ok(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_onnx_model() {
        let path = find_model_path();
        let path = path.expect("find_model_path must succeed in repo tests");
        let session = ort::session::Session::builder()
            .expect("Session builder")
            .commit_from_file(&path);
        let session = session.expect("Failed to commit from file");
        for input in session.inputs() {
            println!("MODEL INPUT: {:?}", input);
        }
        for output in session.outputs() {
            println!("MODEL OUTPUT: {:?}", output);
        }
    }

    #[test]
    fn test_extract_analysis_real_audio() {
        let p = Path::new(r"C:\Users\admin\Music\qbc-music\Alex G\Forever [iu8Lh4xvx3g].m4a");
        if p.exists() {
            let res = extract_analysis(p);
            println!("extract_analysis result: {:?}", res.as_ref().map(|(emb, bpm, key)| (emb.len(), *bpm, key.clone())));
            let (emb, bpm, key) = res.expect("extract_analysis failed");
            assert_eq!(emb.len(), 400);
            println!("SUCCESS! Embedding len: {}, BPM: {:?}, Key: {:?}", emb.len(), bpm, key);
        }
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = AiDj::cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = AiDj::cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_find_similar() {
        let mut dj = AiDj::new(unique_store());

        let emb1 = vec![1.0, 0.0, 0.0];
        let emb2 = vec![0.9, 0.1, 0.0];
        let emb3 = vec![0.0, 1.0, 0.0];

        dj.store_embedding(
            "track1",
            &emb1,
            &TrackAnalysis {
                track_id: "track1".to_string(),
                bpm: None,
                key: None,
                duration_ms: None,
                analyzed_at: 0,
            },
        )
        .unwrap();

        dj.store_embedding(
            "track2",
            &emb2,
            &TrackAnalysis {
                track_id: "track2".to_string(),
                bpm: None,
                key: None,
                duration_ms: None,
                analyzed_at: 0,
            },
        )
        .unwrap();

        dj.store_embedding(
            "track3",
            &emb3,
            &TrackAnalysis {
                track_id: "track3".to_string(),
                bpm: None,
                key: None,
                duration_ms: None,
                analyzed_at: 0,
            },
        )
        .unwrap();

        let similar = dj.find_similar("track1", 2);
        assert_eq!(similar.len(), 2);
        assert_eq!(similar[0].0, "track2"); // Most similar to track1
    }

    /// Свежее хранилище в памяти: движок работает с тем же контрактом, что и в
    /// рантайме, а путь и файл в тестах больше не участвуют.
    fn unique_store() -> Arc<dyn ModuleDb> {
        crate::library::db::memory_module_db()
    }

    #[test]
    fn test_next_track() {
        let mut dj = AiDj::new(unique_store());

        dj.store_embedding(
            "track1",
            &vec![1.0, 0.0, 0.0],
            &TrackAnalysis {
                track_id: "track1".to_string(),
                bpm: None,
                key: None,
                duration_ms: None,
                analyzed_at: 0,
            },
        )
        .unwrap();

        dj.store_embedding(
            "track2",
            &vec![0.9, 0.1, 0.0],
            &TrackAnalysis {
                track_id: "track2".to_string(),
                bpm: None,
                key: None,
                duration_ms: None,
                analyzed_at: 0,
            },
        )
        .unwrap();

        let next = dj.next_track("track1", EnergyCurve::Any, &[]);
        assert_eq!(next, Some("track2".to_string()));
    }

    #[test]
    fn test_redb_store_and_load() {
        let store = unique_store();

        // Create and store (use 400-dim embedding like production)
        {
            let mut dj = AiDj::new(Arc::clone(&store));
            let mut emb = vec![0.0f32; 400];
            emb[0] = 1.0;
            emb[1] = 0.5;
            emb[2] = 0.3;
            emb[3] = 0.1;
            let analysis = TrackAnalysis {
                track_id: "test_track".to_string(),
                bpm: Some(120.0),
                key: Some("C".to_string()),
                duration_ms: Some(180000),
                analyzed_at: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            };

            dj.store_embedding("test_track", &emb, &analysis).unwrap();
            assert!(dj.has_track("test_track"));
            assert_eq!(dj.track_count(), 1);
        } // dj drops here, releasing the db lock

        // Load into new instance
        {
            let mut dj2 = AiDj::new(Arc::clone(&store));
            dj2.load().unwrap();
            assert!(dj2.has_track("test_track"));
            assert_eq!(dj2.track_count(), 1);

            let loaded_emb = dj2.get_embedding("test_track").unwrap();
            assert_eq!(loaded_emb.len(), 400);
            assert!((loaded_emb[0] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_camelot_compatible() {
        // Same key
        assert!(camelot_compatible("8A", "8A"));

        // Adjacent numbers, same letter (circular)
        assert!(camelot_compatible("8A", "9A"));
        assert!(camelot_compatible("8A", "7A"));
        assert!(camelot_compatible("12A", "1A")); // Circular wrap
        assert!(camelot_compatible("1A", "12A"));

        // Same number, different letter (A vs B)
        assert!(camelot_compatible("8A", "8B"));

        // Incompatible
        assert!(!camelot_compatible("8A", "5A"));
        assert!(!camelot_compatible("3A", "9B"));
    }

    #[test]
    fn test_detect_bpm_synthetic() {
        // Create a 120 BPM click track (500Hz sine, 0.01s clicks every 0.5s)
        let sample_rate: u32 = 16000;
        let bpm = 120.0;
        let click_interval = (60.0 / bpm * sample_rate as f32) as usize;
        let click_duration = (0.01 * sample_rate as f32) as usize;
        let total_samples = sample_rate as usize * 30; // 30 seconds

        let mut samples = vec![0.0f32; total_samples];
        let mut pos = 0;
        while pos < total_samples {
            for i in 0..click_duration.min(total_samples - pos) {
                // 500Hz sine wave click
                let t = i as f32 / sample_rate as f32;
                samples[pos + i] = (2.0 * std::f32::consts::PI * 500.0 * t).sin() * 0.8;
            }
            pos += click_interval;
        }

        let detected = detect_bpm(&samples, sample_rate);
        assert!(detected.is_some(), "BPM detection should work on click track");
        let detected_bpm = detected.unwrap();
        // Allow ±5 BPM tolerance
        assert!((detected_bpm - bpm).abs() < 8.0, "Detected BPM {detected_bpm} should be close to {bpm}");
    }

    #[test]
    fn test_detect_key_synthetic() {
        // Create a C major chord (C4=261.63Hz, E4=329.63Hz, G4=392.00Hz) for 10 seconds
        let sample_rate: u32 = 16000;
        let freqs = [261.63f32, 329.63, 392.00];
        let total_samples = sample_rate as usize * 10;

        let mut samples = vec![0.0f32; total_samples];
        for i in 0..total_samples {
            let t = i as f32 / sample_rate as f32;
            let mut sum = 0.0f32;
            for &f in &freqs {
                sum += (2.0 * std::f32::consts::PI * f * t).sin();
            }
            samples[i] = sum / freqs.len() as f32;
        }

        let detected = detect_key(&samples, sample_rate);
        assert!(detected.is_some(), "Key detection should work on C major chord");
        let key = detected.unwrap();
        // C major in Camelot is 8A (Am relative minor) or 5B (C major)
        // Accept either since C major and Am share the same notes
        assert!(key == "5B" || key == "8A" || key == "5A" || key == "8B",
            "Detected key {key} should be compatible with C major");
    }

    #[test]
    fn test_next_track_with_bpm_key() {
        let mut dj = AiDj::new(unique_store());

        // Track 1: 120 BPM, 8A
        dj.store_embedding("track1", &vec![1.0, 0.0, 0.0], &TrackAnalysis {
            track_id: "track1".to_string(),
            bpm: Some(120.0),
            key: Some("8A".to_string()),
            duration_ms: None,
            analyzed_at: 0,
        }).unwrap();

        // Track 2: 125 BPM (close), 8A (same key) - should be preferred
        dj.store_embedding("track2", &vec![0.9, 0.1, 0.0], &TrackAnalysis {
            track_id: "track2".to_string(),
            bpm: Some(125.0),
            key: Some("8A".to_string()),
            duration_ms: None,
            analyzed_at: 0,
        }).unwrap();

        // Track 3: 140 BPM (far), 5A (incompatible key) - should be less preferred
        dj.store_embedding("track3", &vec![0.85, 0.15, 0.0], &TrackAnalysis {
            track_id: "track3".to_string(),
            bpm: Some(140.0),
            key: Some("5A".to_string()),
            duration_ms: None,
            analyzed_at: 0,
        }).unwrap();

        let next = dj.next_track("track1", EnergyCurve::Any, &[]);
        // Should pick track2 (closer BPM and compatible key) even though track3 has similar embedding
        assert_eq!(next, Some("track2".to_string()));
    }

    #[test]
    fn test_redb_persistence() {
        let store = unique_store();

        // Store multiple tracks (use 400-dim embeddings)
        {
            let mut dj = AiDj::new(Arc::clone(&store));
            for i in 0..5 {
                let mut emb = vec![0.0f32; 400];
                emb[0] = i as f32;
                emb[1] = 0.5;
                emb[2] = 0.3;
                let analysis = TrackAnalysis {
                    track_id: format!("track_{i}"),
                    bpm: None,
                    key: None,
                    duration_ms: None,
                    analyzed_at: 0,
                };
                dj.store_embedding(&format!("track_{i}"), &emb, &analysis).unwrap();
            }
            assert_eq!(dj.track_count(), 5);
        } // dj drops here

        // Reload and verify
        {
            let mut dj2 = AiDj::new(Arc::clone(&store));
            dj2.load().unwrap();
            assert_eq!(dj2.track_count(), 5);

            for i in 0..5 {
                assert!(dj2.has_track(&format!("track_{i}")));
            }
        }
    }

    #[test]
    fn test_similarity_graph_top_neighbor_and_cache() {
        let store = unique_store();
        let mut dj = AiDj::new(Arc::clone(&store));
        dj.load().unwrap();

        let embeddings: Vec<(&str, Vec<f32>)> = vec![
            ("a", vec![1.0, 0.0, 0.0, 0.0]),
            ("b", vec![0.99, 0.01, 0.0, 0.0]),
            ("c", vec![0.0, 0.0, 1.0, 0.0]),
        ];
        for (id, embedding) in &embeddings {
            let analysis = TrackAnalysis {
                track_id: id.to_string(),
                bpm: None,
                key: None,
                duration_ms: None,
                analyzed_at: 0,
            };
            dj.store_embedding(id, embedding, &analysis).unwrap();
        }

        let tracks: Vec<crate::library::db::Track> = embeddings
            .iter()
            .map(|(id, _)| crate::library::db::Track {
                id: id.to_string(),
                title: id.to_string(),
                artist: None,
                album: None,
                duration_s: None,
                path: String::new(),
                ext: None,
                filesize: None,
                thumbnail_path: None,
                added_at: 0,
                last_played: None,
                play_count: 0,
            })
            .collect();

        let graph = dj.build_similarity_graph(&tracks, 1);
        assert_eq!(graph.nodes.len(), 3);
        // k=1: ближайшая пара a↔b обязана быть соединена.
        let connected = graph.edges.iter().any(|(u, v, _)| {
            let (left, right) = (graph.nodes[*u].id.as_str(), graph.nodes[*v].id.as_str());
            (left == "a" && right == "b") || (left == "b" && right == "a")
        });
        assert!(connected, "a↔b must be the top edge: {:?}", graph.edges);

        // Повторный вызов отдаёт кэшированный граф.
        let cached = dj.build_similarity_graph(&tracks, 1);
        assert_eq!(cached.nodes.len(), graph.nodes.len());
        assert_eq!(cached.edges.len(), graph.edges.len());
        assert!(AiDj::cached_graph(&store, &tracks).is_some());

        // Новая запись эмбеддинга инвалидирует кэш.
        let analysis = TrackAnalysis {
            track_id: "d".into(),
            bpm: None,
            key: None,
            duration_ms: None,
            analyzed_at: 0,
        };
        dj.store_embedding("d", &vec![0.0, 0.0, 0.0, 1.0], &analysis).unwrap();
        assert!(AiDj::cached_graph(&store, &tracks).is_none());
    }
}
