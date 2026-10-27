# AI DJ Research — Audio Similarity Graph & Track Transition Path

## Overview
Research document for implementing AI DJ feature in Shell's audio player widget.
Goal: build a graph of track similarity and construct optimal transition paths between tracks.
**Implementation: strictly in Rust, embedded directly into Shell.**

---

## Key Findings: Small Neural Networks for Music Analysis

### 1. Discogs-MAST (Audio Spectrogram Transformer)
- **Source**: https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e
- **Architecture**: ASTForAudioClassification (Audio Spectrogram Transformer)
- **Parameters**: 129M (base), 73M (tiny), 519M (large)
- **License**: CC-BY-NC-SA-4.0
- **Trained on**: Discogs music database (genre, style, mood, instrument tags)
- **Output**: 416-dimensional embedding vector per 30s audio chunk
- **Use case**: Music auto-tagging, similarity search, genre classification
- **Rust integration**: Export to ONNX → use `ort` (ONNX Runtime Rust bindings)

### 2. MusicNN
- **Source**: https://huggingface.co/oriyonay/musicnn-pytorch
- **Architecture**: VGG-like CNN for audio
- **Parameters**: ~8M (very small!)
- **License**: Apache-2.0
- **Trained on**: Million Song Dataset, MagnaTagATune
- **Output**: Tag predictions (genre, mood, instrumentation)
- **Rust integration**: Convert to ONNX or use `tch-rs` (PyTorch C++ bindings)

### 3. Music Sentiment Analysis
- **Source**: https://huggingface.co/nikarpoff/music-sentiment-analysis-happy-sad-13M
- **Parameters**: 13M (happy/sad), 8M (relaxing/energetic)
- **Use case**: Emotion/mood detection for track selection
- **Rust integration**: ONNX export → `ort`

### 4. Genre Classification Models
- **Source**: https://huggingface.co/dima806/music_genres_classification
- **Architecture**: DistilHuBERT (distilled HuBERT)
- **Parameters**: ~6M
- **Use case**: Genre classification for similarity grouping

---

## Rust-Native Audio Analysis Libraries

### 1. fundsp
- **Source**: https://github.com/SamiPerttu/fundsp
- **Description**: Functional audio DSP library in Rust
- **Features**: Filters, oscillators, FFT, spectral processing
- **Use case**: Building blocks for custom audio analysis

### 2. rustfft
- **Crates.io**: rustfft
- **Description**: Fast Fourier Transform in pure Rust
- **Use case**: Spectral analysis, feature extraction

### 3. mel_spec
- **Crates.io**: mel_spec
- **Description**: Mel spectrogram computation
- **Use case**: Preprocessing for neural network input

### 4. ort (ONNX Runtime)
- **Crates.io**: ort
- **Description**: Rust bindings for ONNX Runtime
- **Use case**: Run exported ML models (discogs-maest, musicnn) in Rust

### 5. whisper-rs / whisper.cpp
- **Source**: https://github.com/tazz4843/whisper-rs
- **Description**: Speech recognition (can be adapted for music)
- **Use case**: Vocal detection, speech/music separation

---

## Proposed Architecture (Rust-Native)

### Phase 1: Feature Extraction Pipeline
```
Audio File → Decode (symphonia) → Mel Spectrogram (mel_spec) → 
→ ONNX Model (discogs-maest) → 416-dim Embedding Vector
```

**Components:**
- **Audio decoding**: `symphonia` (pure Rust, supports MP3/FLAC/OGG/WAV)
- **Spectrogram**: `mel_spec` or custom with `rustfft`
- **ML inference**: `ort` with exported discogs-maest ONNX model

### Phase 2: Similarity Graph Construction
```rust
struct TrackEmbedding {
    track_id: String,
    vector: Vec<f32>,  // 416-dim from discogs-maest
}

struct SimilarityGraph {
    nodes: Vec<TrackEmbedding>,
    edges: Vec<(usize, usize, f32)>,  // (from, to, similarity)
}

impl SimilarityGraph {
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 { ... }
    fn build_from_embeddings(tracks: Vec<TrackEmbedding>) -> Self { ... }
}
```

### Phase 3: Transition Path Algorithm
```rust
struct TransitionPath {
    graph: SimilarityGraph,
}

impl TransitionPath {
    fn find_next_track(&self, current: usize, energy_curve: EnergyCurve) -> usize {
        // Combine:
        // 1. Cosine similarity of embeddings
        // 2. BPM compatibility (harmonic mixing)
        // 3. Key compatibility (Camelot wheel)
        // 4. Energy curve matching
    }
    
    fn dijkstra_path(&self, start: usize, goal: usize) -> Vec<usize> { ... }
}
```

### Phase 4: AI DJ Runtime
```rust
struct AiDj {
    graph: SimilarityGraph,
    current_track: Option<usize>,
    energy_curve: EnergyCurve,
}

impl AiDj {
    fn next_track(&mut self) -> Option<Track> {
        // 1. Find optimal next track from graph
        // 2. Apply crossfade at beat-matched point
        // 3. Update energy curve
    }
}
```

---

## Implementation Plan

### Step 1: ONNX Model Export
1. Download discogs-maest from Hugging Face
2. Export to ONNX format (Python one-liner):
   ```python
   from optimum.onnxruntime import ORTModelForAudioClassification
   model = ORTModelForAudioClassification.from_pretrained("mtg-upf/discogs-maest-30s-pw-129e")
   model.save_pretrained("./models/discogs-maest-onnx")
   ```

### Step 2: Rust Audio Pipeline
1. Add dependencies to `Cargo.toml`:
   ```toml
   [dependencies]
   symphonia = "0.5"
   mel_spec = "0.2"
   ort = "2.0"
   rustfft = "6.0"
   ndarray = "0.15"
   ```

### Step 3: Embedding Extraction
```rust
pub fn extract_embedding(audio_path: &Path) -> Result<Vec<f32>, Error> {
    // 1. Decode audio with symphonia
    // 2. Compute mel spectrogram
    // 3. Run through ONNX model
    // 4. Return 416-dim vector
}
```

### Step 4: Graph + Pathfinding
```rust
pub struct MusicGraph {
    embeddings: HashMap<String, Vec<f32>>,
}

impl MusicGraph {
    pub fn add_track(&mut self, id: String, embedding: Vec<f32>);
    pub fn find_similar(&self, id: &str, top_k: usize) -> Vec<(String, f32)>;
    pub fn build_transition_path(&self, from: &str, to: &str) -> Vec<String>;
}
```

### Step 5: Integration with player.rs
```rust
// In player.rs
impl PlayerManager {
    pub fn enable_ai_dj(&mut self) {
        // Load pre-computed embeddings from SQLite
        // Build similarity graph
        // Set up auto-advance with transition path
    }
}
```

---

## Model Size & Performance

| Model | Parameters | Size (ONNX) | Inference Time* | Quality |
|-------|-----------|-------------|-----------------|---------|
| discogs-maest-73e | 73M | ~300MB | ~200ms | Good |
| discogs-maest-129e | 129M | ~500MB | ~400ms | Better |
| musicnn | 8M | ~30MB | ~50ms | Basic |
| music-sentiment | 13M | ~50MB | ~100ms | Mood only |

*Approximate on CPU, will be faster on GPU

**Selected**: `discogs-maest-129e` — only ~200MB larger than 73e, significantly better embeddings.
- 129M parameters, 416-dim output
- ONNX export for Rust inference via `ort`
- ~400ms inference on CPU per 30s chunk

---

## Storage Strategy — redb

[redb](https://github.com/cberner/redb) — pure Rust embedded key-value database.
Perfect for storing binary embeddings, no SQL migrations needed.

```toml
# Cargo.toml
redb = "2.1"
```

```rust
// redb schema for AI DJ embeddings
use redb::{Database, ReadableTable, TableDefinition};

const EMBEDDINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("embeddings");
const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

// Store embedding as raw bytes: 416 x f32 = 1664 bytes
// Store metadata as JSON: { bpm, key, duration_ms, analyzed_at }
```

**Advantages over SQLite for this use case:**
- No SQL schema migrations
- Native Rust types, zero-copy reads
- Single file storage (~16MB for 10K tracks)
- ACID transactions
- Better performance for key-value lookups

**Storage size:**
- 416-dim f32 vector = 1.6 KB per track
- 10,000 tracks = ~16 MB (very manageable)
- Metadata ~200 bytes per track

---

## References

### Models
- discogs-maest: https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e
- musicnn: https://huggingface.co/oriyonay/musicnn-pytorch
- music-sentiment: https://huggingface.co/nikarpoff/music-sentiment-analysis-happy-sad-13M

### Rust Libraries
- symphonia: https://github.com/pdeljanov/Symphonia
- ort: https://github.com/pykeio/ort
- rustfft: https://github.com/ejmahler/RustFFT
- mel_spec: https://crates.io/crates/mel_spec
- fundsp: https://github.com/SamiPerttu/fundsp

### Tools
- optimum (ONNX export): https://huggingface.co/docs/optimum/
- onnxruntime: https://onnxruntime.ai/

---

## Next Steps
1. [ ] Download and export discogs-maest to ONNX
2. [ ] Create Rust module `ai_dj.rs` in backend/code/music/src/
3. [ ] Implement embedding extraction pipeline
4. [ ] Build similarity graph data structure
5. [ ] Implement transition path algorithm (Dijkstra + heuristics)
6. [ ] Add SQLite storage for embeddings
7. [ ] Integrate with player.rs for auto-advance
8. [ ] Add UI toggle in player settings (bottom of circle)
