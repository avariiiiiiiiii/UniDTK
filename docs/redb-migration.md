# SQLite → redb Migration Plan

## Current State

### SQLite Schema
- **tracks** (~10K-100K rows): id, title, artist, album, duration_s, path, ext, filesize, thumbnail_path, added_at, last_played, play_count
- **playlists** (~100-1000 rows): id, name, source_url, last_sync, thumbnail_path, track_count, first_track_id
- **playlist_tracks** (many-to-many): playlist_id, track_id, position
- **tracks_fts**: FTS5 full-text search index

### Dependencies
- `rusqlite` 0.32.1
- `r2d2` 0.8.10 (connection pooling)
- `r2d2_sqlite` 0.25.0

### Size Estimate (10K tracks)
- tracks: ~5MB
- playlists: ~500KB
- playlist_tracks: ~2MB
- tracks_fts: ~3MB
- **Total: ~10-15MB**

---

## Proposed redb Schema

### Table Definitions

```rust
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

// Primary tables
const TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("tracks");
const PLAYLISTS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlists");
const PLAYLIST_TRACKS: TableDefinition<&str, &[u8]> = TableDefinition::new("playlist_tracks");

// Secondary indexes (manually maintained)
const TRACKS_BY_TITLE: TableDefinition<&str, &str> = TableDefinition::new("idx_tracks_title");
const TRACKS_BY_ARTIST: TableDefinition<&str, &str> = TableDefinition::new("idx_tracks_artist");
const TRACKS_BY_LAST_PLAYED: TableDefinition<i64, &str> = TableDefinition::new("idx_tracks_last_played");
const TRACKS_BY_ADDED_AT: TableDefinition<i64, &str> = TableDefinition::new("idx_tracks_added_at");
const PLAYLIST_TRACKS_BY_POSITION: TableDefinition<&str, &str> = TableDefinition::new("idx_playlist_tracks_position");
```

### Data Structures

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_s: Option<i64>,
    pub path: String,
    pub ext: Option<String>,
    pub filesize: Option<i64>,
    pub thumbnail_path: Option<String>,
    pub added_at: i64,
    pub last_played: Option<i64>,
    pub play_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub source_url: String,
    pub last_sync: Option<i64>,
    pub track_count: i64,
    pub thumbnail_path: Option<String>,
    pub first_track_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub playlist_id: String,
    pub track_id: String,
    pub position: i64,
}
```

### Serialization: bincode

**Why bincode:**
- 3-5x smaller than JSON
- 10-50x faster serialization
- Deterministic output
- No schema needed

**Size comparison (single track):**
- JSON: ~300 bytes
- bincode: ~180 bytes

---

## Migration Plan

### Phase 1: Dual-Write (Backward Compatible)

**Goal:** Write to both SQLite and redb, read from SQLite.

**Steps:**
1. Add `redb` and `bincode` dependencies
2. Create `RedbDatabase` struct with same API as `Database`
3. On every write: write to SQLite AND redb
4. On every read: read from SQLite (source of truth)
5. Add migration status tracking

**Code changes:**
```rust
pub struct HybridDatabase {
    sqlite: Database,      // Existing SQLite
    redb: RedbDatabase,    // New redb
    migration_complete: bool,
}

impl HybridDatabase {
    pub fn new(config: &Config) -> Result<Self, String> {
        let sqlite = Database::new(config.database_path());
        sqlite.init()?;
        
        let redb = RedbDatabase::new(config.redb_path());
        redb.init()?;
        
        Ok(Self {
            sqlite,
            redb,
            migration_complete: false,
        })
    }
    
    pub fn upsert_track(&self, playlist_id: &str, position: i64, track: &DownloadedTrack) -> Result<(), String> {
        // Write to both
        self.sqlite.upsert_track(playlist_id, position, track)?;
        self.redb.upsert_track(playlist_id, position, track)?;
        Ok(())
    }
    
    pub fn list_tracks(&self, search: Option<String>) -> Result<Vec<Track>, String> {
        // Read from SQLite (source of truth)
        self.sqlite.list_tracks(search)
    }
}
```

**Timeline:** 2-3 days

---

### Phase 2: Redb as Primary

**Goal:** Switch reads to redb, keep SQLite for validation.

**Steps:**
1. Implement all read operations in redb
2. Add consistency checks (compare SQLite vs redb on reads)
3. Log any discrepancies
4. Performance benchmarking

**Code changes:**
```rust
impl HybridDatabase {
    pub fn list_tracks(&self, search: Option<String>) -> Result<Vec<Track>, String> {
        // Read from redb (new source of truth)
        let redb_tracks = self.redb.list_tracks(search)?;
        
        // Optional: validate against SQLite in background
        if !self.migration_complete {
            self.validate_consistency("list_tracks", &redb_tracks);
        }
        
        Ok(redb_tracks)
    }
}
```

**Timeline:** 1-2 days

---

### Phase 3: Remove SQLite (COMPLETED)

**Goal:** Full migration to redb.

**Status:** ✅ Fully completed and verified.
1. SQLite read/write code removed in favor of pure `redb` + `bincode`.
2. Removed `rusqlite`, `r2d2`, `r2d2_sqlite` dependencies from `Cargo.toml`.
3. All music, player, playlist, sync, log, widget, and AI DJ data are managed exclusively in `shell.redb`.

---

## Full Migration Timeline

| Phase | Duration | Status |
|---|---|---|
| 1: Unified redb schema | Completed | ✅ |
| 2: Redb primary | Completed | ✅ |
| 3: Remove SQLite | Completed | ✅ |

---

## Full-Text Search Replacement

### Option A: Simple Prefix Matching (Recommended for now)
```rust
fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    
    let txn = self.db.begin_read().map_err(|e| e.to_string())?;
    let table = txn.open_table(TRACKS).map_err(|e| e.to_string())?;
    
    for entry in table.iter().map_err(|e| e.to_string())? {
        let (_, value) = entry.map_err(|e| e.to_string())?;
        let track: Track = bincode::deserialize(value).map_err(|e| e.to_string())?;
        
        if track.title.to_lowercase().contains(&query_lower)
            || track.artist.as_ref().map(|a| a.to_lowercase()).unwrap_or_default().contains(&query_lower)
            || track.album.as_ref().map(|a| a.to_lowercase()).unwrap_or_default().contains(&query_lower)
        {
            results.push(track);
        }
    }
    
    Ok(results)
}
```

### Option b: Tantivy Integration (Future)
- Add `tantivy` dependency
- Build index on track metadata
- Full FTS5-like capabilities
- More complex but better search quality

---

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Data loss | High | Dual-write phase, backups |
| Performance regression | Medium | Benchmark before/after |
| Search quality loss | Medium | Tantivy integration |
| Migration bugs | Low | Gradual rollout, validation |

---

## Recommendation

**Proceed with migration** — benefits outweigh risks:

1. **Simpler code** — no SQL migrations, no connection pooling
2. **Better Rust integration** — pure Rust, no FFI
3. **Smaller binary** — fewer dependencies
4. **ACID transactions** — data safety maintained
5. **Good performance** — key-value access is sufficient

**Start with Phase 1** (dual-write) to minimize risk.

---

## References

- redb: https://github.com/cberner/redb
- bincode: https://github.com/bincode-org/bincode
- tantivy: https://github.com/quickwit-oss/tantivy
