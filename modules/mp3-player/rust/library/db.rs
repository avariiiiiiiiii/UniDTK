use serde::{de::DeserializeOwned, Deserialize, Serialize};
use unidtk_api::{DbOp, ModuleDb};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

// Раскладка библиотеки внутри среза модуля. Префикс `mods/mp3_player/` к ключу
// добавляет ядро ([`ModuleDb`]) — модуль видит только эти ключи:
//   track/<id>                → bincode(Track)
//   playlist/<id>             → bincode(Playlist)
//   pt/<playlist_id>/<pos:08> → id трека (сырые utf-8 байты), порядок позиции
const TRACK_PREFIX: &str = "track/";
const PLAYLIST_PREFIX: &str = "playlist/";
const LINK_PREFIX: &str = "pt/";

#[derive(Clone)]
pub struct Database {
    db: Arc<dyn ModuleDb>,
}

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
}

#[derive(Debug, Clone)]
pub struct DownloadedTrack {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_s: Option<i64>,
    pub path: String,
    pub ext: Option<String>,
    pub filesize: Option<i64>,
    pub thumbnail_path: Option<String>,
}

impl Database {
    pub fn new(db: Arc<dyn ModuleDb>) -> Self {
        Self { db }
    }

    fn track_key(id: &str) -> String {
        format!("{TRACK_PREFIX}{id}")
    }

    fn playlist_key(id: &str) -> String {
        format!("{PLAYLIST_PREFIX}{id}")
    }

    fn link_key(playlist_id: &str, position: i64) -> String {
        format!("{LINK_PREFIX}{playlist_id}/{position:08}")
    }

    fn link_prefix(playlist_id: &str) -> String {
        format!("{LINK_PREFIX}{playlist_id}/")
    }

    fn put<T: Serialize>(&self, key: String, value: &T) -> Result<(), String> {
        let bytes = bincode::serialize(value).map_err(|e| e.to_string())?;
        self.db.kv_set(&key, &bytes)
    }

    fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, String> {
        match self.db.kv_get(key)? {
            Some(bytes) => bincode::deserialize(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    fn all<T: DeserializeOwned>(&self, prefix: &str) -> Result<Vec<T>, String> {
        let mut out = Vec::new();
        for (_, bytes) in self.db.kv_list(prefix)? {
            if let Ok(value) = bincode::deserialize::<T>(&bytes) {
                out.push(value);
            }
        }
        Ok(out)
    }

    pub fn list_tracks(&self, search: Option<String>) -> Result<Vec<Track>, String> {
        self.list_tracks_sorted(search, None, None)
    }

    pub fn list_tracks_sorted(
        &self,
        search: Option<String>,
        sort_key: Option<&str>,
        sort_dir: Option<&str>,
    ) -> Result<Vec<Track>, String> {
        let mut tracks = self.all::<Track>(TRACK_PREFIX)?;
        let search_filter = search
            .as_deref()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        if let Some(ref query) = search_filter {
            tracks.retain(|track| {
                track.title.to_lowercase().contains(query)
                    || track
                        .artist
                        .as_deref()
                        .map(|a| a.to_lowercase().contains(query))
                        .unwrap_or(false)
                    || track
                        .album
                        .as_deref()
                        .map(|a| a.to_lowercase().contains(query))
                        .unwrap_or(false)
            });
        }

        sort_tracks(&mut tracks, sort_key, sort_dir);
        Ok(tracks)
    }

    #[cfg(test)]
    pub fn list_playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>, String> {
        self.list_playlist_tracks_sorted(playlist_id, None, None)
    }

    pub fn list_playlist_tracks_sorted(
        &self,
        playlist_id: &str,
        sort_key: Option<&str>,
        sort_dir: Option<&str>,
    ) -> Result<Vec<Track>, String> {
        let mut ordered: Vec<(i64, String)> = Vec::new();
        for (key, value) in self.db.kv_list(&Self::link_prefix(playlist_id))? {
            let position = key
                .rsplit('/')
                .next()
                .and_then(|pos| pos.parse::<i64>().ok())
                .unwrap_or(0);
            if let Ok(track_id) = std::str::from_utf8(&value) {
                ordered.push((position, track_id.to_string()));
            }
        }
        ordered.sort_by_key(|(position, _)| *position);

        let mut tracks = Vec::new();
        for (_, track_id) in ordered {
            if let Some(track) = self.get::<Track>(&Self::track_key(&track_id))? {
                tracks.push(track);
            }
        }

        if let Some(key) = sort_key {
            if key != "position" && key != "custom" {
                sort_tracks(&mut tracks, Some(key), sort_dir);
            }
        }

        Ok(tracks)
    }

    pub fn get_tracks_by_ids(&self, ids: &[String]) -> Result<Vec<Track>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut map: HashMap<String, Track> = HashMap::new();
        for id in ids {
            if let Some(track) = self.get::<Track>(&Self::track_key(id))? {
                map.insert(id.clone(), track);
            }
        }
        let result = ids.iter().filter_map(|id| map.remove(id)).collect();
        Ok(result)
    }

    pub fn list_playlists(&self) -> Result<Vec<Playlist>, String> {
        let mut result = self.all::<Playlist>(PLAYLIST_PREFIX)?;
        for pl in result.iter_mut() {
            // Счётчик в записи — исторический максимум позиции (позиции приходят
            // из yt-dlp 1-based, поэтому «position + 1» завышал длину на единицу).
            // Настоящая длина плейлиста — число связок, его и показываем.
            let links = self.db.kv_list(&Self::link_prefix(&pl.id))?;
            pl.track_count = links.len() as i64;

            let needs_local = pl
                .thumbnail_path
                .as_deref()
                .map_or(true, |p| p.starts_with("http") || !Path::new(p).exists());
            if !needs_local {
                continue;
            }
            let mut found: Option<String> = None;
            for (_, value) in &links {
                let Ok(track_id) = std::str::from_utf8(value) else {
                    continue;
                };
                if let Some(track) = self.get::<Track>(&Self::track_key(track_id))? {
                    if let Some(ref thumb) = track.thumbnail_path {
                        if Path::new(thumb).exists() {
                            found = Some(thumb.clone());
                            break;
                        }
                    }
                }
            }
            if found.is_some() {
                pl.thumbnail_path = found;
            }
        }
        result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(result)
    }

    pub fn get_playlist(&self, id: &str) -> Result<Option<Playlist>, String> {
        self.get(&Self::playlist_key(id))
    }

    #[cfg(test)]
    pub fn import_library(
        &self,
        playlists: &[Playlist],
        tracks: &[Track],
        playlist_tracks: &[(String, String, i64)],
    ) -> Result<(), String> {
        let mut ops = Vec::new();
        for track in tracks {
            ops.push(DbOp::Set {
                key: Self::track_key(&track.id),
                value: bincode::serialize(track).map_err(|e| e.to_string())?,
            });
        }
        for pl in playlists {
            ops.push(DbOp::Set {
                key: Self::playlist_key(&pl.id),
                value: bincode::serialize(pl).map_err(|e| e.to_string())?,
            });
        }
        for (playlist_id, track_id, position) in playlist_tracks {
            ops.push(DbOp::Set {
                key: Self::link_key(playlist_id, *position),
                value: track_id.as_bytes().to_vec(),
            });
        }
        self.db.kv_batch(ops)
    }

    pub fn upsert_playlist(&self, id: &str, name: &str, source_url: &str) -> Result<(), String> {
        let existing: Option<Playlist> = self.get(&Self::playlist_key(id))?;

        let final_name = match existing.as_ref() {
            Some(prev) => {
                let prev_is_url = looks_like_url(&prev.name);
                let new_is_url = looks_like_url(name);
                if prev_is_url && !new_is_url {
                    name.to_string()
                } else if !prev_is_url && new_is_url {
                    prev.name.clone()
                } else {
                    name.to_string()
                }
            }
            None => name.to_string(),
        };

        let pl = Playlist {
            id: id.to_string(),
            name: final_name,
            source_url: source_url.to_string(),
            last_sync: existing.as_ref().and_then(|p| p.last_sync),
            track_count: existing.as_ref().map(|p| p.track_count).unwrap_or(0),
            thumbnail_path: existing.as_ref().and_then(|p| p.thumbnail_path.clone()),
        };

        self.put(Self::playlist_key(id), &pl)
    }

    pub fn set_playlist_metadata(
        &self,
        id: &str,
        name: Option<&str>,
        thumbnail_path: Option<&str>,
    ) -> Result<(), String> {
        let mut pl = self
            .get::<Playlist>(&Self::playlist_key(id))?
            .unwrap_or_else(|| Playlist {
                id: id.to_string(),
                name: id.to_string(),
                source_url: String::new(),
                last_sync: None,
                track_count: 0,
                thumbnail_path: None,
            });

        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            if !looks_like_url(n) {
                pl.name = n.to_string();
            }
        }
        if let Some(t) = thumbnail_path.filter(|t| !t.trim().is_empty()) {
            pl.thumbnail_path = Some(t.to_string());
        }

        self.put(Self::playlist_key(id), &pl)
    }

    /// Удалить плейлист вместе с его связками. Треки и файлы остаются в
    /// библиотеке — при повторной синхронизации они переиспользуются без
    /// повторного скачивания.
    pub fn delete_playlist(&self, playlist_id: &str) -> Result<(), String> {
        let mut ops = vec![DbOp::Remove {
            key: Self::playlist_key(playlist_id),
        }];
        for (key, _) in self.db.kv_list(&Self::link_prefix(playlist_id))? {
            ops.push(DbOp::Remove { key });
        }
        self.db.kv_batch(ops)
    }

    pub fn mark_playlist_synced(&self, id: &str) -> Result<(), String> {
        if let Some(mut pl) = self.get::<Playlist>(&Self::playlist_key(id))? {
            pl.last_sync = Some(now_ts());
            self.put(Self::playlist_key(id), &pl)?;
        }
        Ok(())
    }

    pub fn get_track(&self, id: &str) -> Result<Option<Track>, String> {
        self.get(&Self::track_key(id))
    }

    pub fn link_track_to_playlist(
        &self,
        playlist_id: &str,
        position: i64,
        track_id: &str,
    ) -> Result<(), String> {
        let mut ops = vec![DbOp::Set {
            key: Self::link_key(playlist_id, position),
            value: track_id.as_bytes().to_vec(),
        }];
        if let Some(mut pl) = self.get::<Playlist>(&Self::playlist_key(playlist_id))? {
            pl.track_count = (position + 1).max(pl.track_count);
            ops.push(DbOp::Set {
                key: Self::playlist_key(playlist_id),
                value: bincode::serialize(&pl).map_err(|e| e.to_string())?,
            });
        }
        self.db.kv_batch(ops)
    }

    pub fn prune_playlist_tracks(
        &self,
        playlist_id: &str,
        current_track_ids: &[String],
    ) -> Result<(), String> {
        let mut ops = Vec::new();
        let mut surviving_count: i64 = 0;

        for (key, value) in self.db.kv_list(&Self::link_prefix(playlist_id))? {
            if let Ok(track_id) = std::str::from_utf8(&value) {
                if current_track_ids.iter().any(|id| id == track_id) {
                    surviving_count += 1;
                } else {
                    ops.push(DbOp::Remove { key });
                }
            }
        }

        if let Some(mut pl) = self.get::<Playlist>(&Self::playlist_key(playlist_id))? {
            pl.track_count = surviving_count;
            ops.push(DbOp::Set {
                key: Self::playlist_key(playlist_id),
                value: bincode::serialize(&pl).map_err(|e| e.to_string())?,
            });
        }

        self.db.kv_batch(ops)
    }

    pub fn upsert_track(
        &self,
        playlist_id: &str,
        position: i64,
        downloaded: &DownloadedTrack,
    ) -> Result<(), String> {
        let existing: Option<Track> = self.get(&Self::track_key(&downloaded.id))?;

        let thumbnail_path = downloaded.thumbnail_path.clone().or_else(|| {
            if is_youtube_id(&downloaded.id) {
                Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", downloaded.id))
            } else {
                None
            }
        });

        let track = Track {
            id: downloaded.id.clone(),
            title: downloaded.title.clone(),
            artist: downloaded.artist.clone(),
            album: downloaded.album.clone(),
            duration_s: downloaded.duration_s,
            path: downloaded.path.clone(),
            ext: downloaded.ext.clone(),
            filesize: downloaded.filesize,
            thumbnail_path,
            added_at: existing.as_ref().map(|t| t.added_at).unwrap_or_else(now_ts),
            last_played: existing.as_ref().and_then(|t| t.last_played),
            play_count: existing.as_ref().map(|t| t.play_count).unwrap_or(0),
        };

        let mut ops = vec![DbOp::Set {
            key: Self::track_key(&downloaded.id),
            value: bincode::serialize(&track).map_err(|e| e.to_string())?,
        }];

        if !playlist_id.is_empty() {
            ops.push(DbOp::Set {
                key: Self::link_key(playlist_id, position),
                value: downloaded.id.as_bytes().to_vec(),
            });
            if let Some(mut pl) = self.get::<Playlist>(&Self::playlist_key(playlist_id))? {
                pl.track_count = (position + 1).max(pl.track_count);
                ops.push(DbOp::Set {
                    key: Self::playlist_key(playlist_id),
                    value: bincode::serialize(&pl).map_err(|e| e.to_string())?,
                });
            }
        }

        self.db.kv_batch(ops)
    }

    pub fn record_play(&self, track_id: &str) -> Result<(), String> {
        if let Some(mut track) = self.get::<Track>(&Self::track_key(track_id))? {
            track.play_count += 1;
            track.last_played = Some(now_ts());
            self.put(Self::track_key(track_id), &track)?;
        }
        Ok(())
    }

    /// Восстановление обложек по файлам `<id>.jpg` рядом с треком. Читающая
    /// фаза сначала собирает кандидатов — writer-транзакция открывается только
    /// если реально есть что чинить.
    pub fn repair_missing_thumbnails(&self) -> Result<(), String> {
        let mut repairs = Vec::new();
        for track in self.all::<Track>(TRACK_PREFIX)? {
            if track.thumbnail_path.is_some() {
                continue;
            }
            let track_path = Path::new(&track.path);
            if let Some(parent) = track_path.parent() {
                let candidate = parent.join(format!("{}.jpg", track.id));
                if candidate.exists() {
                    let mut updated = track.clone();
                    updated.thumbnail_path = Some(candidate.to_string_lossy().to_string());
                    repairs.push(updated);
                }
            }
        }
        if repairs.is_empty() {
            return Ok(());
        }

        let mut ops = Vec::new();
        for track in repairs {
            ops.push(DbOp::Set {
                key: Self::track_key(&track.id),
                value: bincode::serialize(&track).map_err(|e| e.to_string())?,
            });
        }
        self.db.kv_batch(ops)
    }
}

fn sort_tracks(tracks: &mut [Track], sort_key: Option<&str>, sort_dir: Option<&str>) {
    let desc = sort_dir
        .map(|d| d.eq_ignore_ascii_case("desc"))
        .unwrap_or(false);
    tracks.sort_by(|a, b| {
        let cmp = match sort_key.unwrap_or("title").to_ascii_lowercase().as_str() {
            "title" => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            "artist" => a
                .artist
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b.artist.as_deref().unwrap_or("").to_lowercase()),
            "album" => a
                .album
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b.album.as_deref().unwrap_or("").to_lowercase()),
            "duration" => a.duration_s.unwrap_or(0).cmp(&b.duration_s.unwrap_or(0)),
            "added_at" => a.added_at.cmp(&b.added_at),
            "last_played" => a
                .last_played
                .unwrap_or(0)
                .cmp(&b.last_played.unwrap_or(0)),
            "play_count" => a.play_count.cmp(&b.play_count),
            _ => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        };
        if desc {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

pub fn is_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Legacy id scheme: `ytmd:` prefix is kept for compatibility with playlist
/// keys already stored in `unidtk.redb` (playlist id and `playlist_tracks`
/// keys are persisted), not as a YouTube Music Desktop dependency.
pub fn manual_playlist_id(source_url: &str) -> String {
    youtube_playlist_list_id(source_url)
        .map(|id| format!("ytmd:{id}"))
        .unwrap_or_else(|| format!("url:{}", stable_hash(source_url)))
}

pub fn stable_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn file_size(path: &Path) -> Option<i64> {
    fs::metadata(path).ok().map(|m| m.len() as i64)
}

fn youtube_playlist_list_id(source_url: &str) -> Option<String> {
    let lower = source_url.to_ascii_lowercase();
    if !lower.contains("youtube.com/playlist") && !lower.contains("music.youtube.com/playlist") {
        return None;
    }

    let start = lower.find("list=")? + "list=".len();
    source_url
        .get(start..)
        .and_then(|value| value.split(['&', '#']).next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn looks_like_url(value: &str) -> bool {
    value
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("http://")
        || value
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("https://")
}

/// Тестовая реализация контракта: та же семантика ключей, но в памяти и без
/// redb — модульные тесты не должны зависеть от движка базы ядра.
#[cfg(test)]
pub fn memory_module_db() -> Arc<dyn ModuleDb> {
    Arc::new(MemoryDb::default())
}

#[cfg(test)]
#[derive(Default)]
struct MemoryDb {
    map: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[cfg(test)]
impl ModuleDb for MemoryDb {
    fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }

    fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn kv_remove(&self, key: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }

    fn kv_list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let map = self.map.lock().unwrap();
        let mut out: Vec<(String, Vec<u8>)> = map
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_db() -> Database {
        Database::new(memory_module_db())
    }

    #[test]
    fn test_track_upsert_and_query() {
        let db = temp_test_db();
        let track = DownloadedTrack {
            id: "track_1".into(),
            title: "Alpha Song".into(),
            artist: Some("Artist One".into()),
            album: Some("Album A".into()),
            duration_s: Some(180),
            path: "/path/to/1.mp3".into(),
            ext: Some("mp3".into()),
            filesize: Some(1024),
            thumbnail_path: None,
        };

        db.upsert_track("pl_1", 0, &track).unwrap();

        let loaded = db.get_track("track_1").unwrap().expect("Track should exist");
        assert_eq!(loaded.title, "Alpha Song");
        assert_eq!(loaded.artist.as_deref(), Some("Artist One"));
        assert_eq!(loaded.play_count, 0);

        db.record_play("track_1").unwrap();
        let updated = db.get_track("track_1").unwrap().unwrap();
        assert_eq!(updated.play_count, 1);
        assert!(updated.last_played.is_some());

        let search_res = db.list_tracks(Some("Alpha".into())).unwrap();
        assert_eq!(search_res.len(), 1);

        let search_none = db.list_tracks(Some("Beta".into())).unwrap();
        assert_eq!(search_none.len(), 0);
    }

    #[test]
    fn test_playlist_and_tracks_ordering() {
        let db = temp_test_db();
        db.upsert_playlist(
            "pl_main",
            "My Playlist",
            "https://music.youtube.com/playlist?list=pl_main",
        )
        .unwrap();

        let track1 = DownloadedTrack {
            id: "t1".into(),
            title: "First".into(),
            artist: Some("A".into()),
            album: None,
            duration_s: Some(100),
            path: "/1.mp3".into(),
            ext: Some("mp3".into()),
            filesize: None,
            thumbnail_path: None,
        };
        let track2 = DownloadedTrack {
            id: "t2".into(),
            title: "Second".into(),
            artist: Some("B".into()),
            album: None,
            duration_s: Some(200),
            path: "/2.mp3".into(),
            ext: Some("mp3".into()),
            filesize: None,
            thumbnail_path: None,
        };

        db.upsert_track("pl_main", 0, &track1).unwrap();
        db.upsert_track("pl_main", 1, &track2).unwrap();

        let pl_tracks = db.list_playlist_tracks("pl_main").unwrap();
        assert_eq!(pl_tracks.len(), 2);
        assert_eq!(pl_tracks[0].id, "t1");
        assert_eq!(pl_tracks[1].id, "t2");

        // Prune track 1
        db.prune_playlist_tracks("pl_main", &["t2".into()]).unwrap();
        let remaining = db.list_playlist_tracks("pl_main").unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "t2");
    }

    #[test]
    fn test_import_library() {
        let db = temp_test_db();
        let playlists = vec![Playlist {
            id: "pl_1".into(),
            name: "Рок".into(),
            source_url: "https://music.youtube.com/playlist?list=pl_1".into(),
            last_sync: None,
            track_count: 2,
            thumbnail_path: None,
        }];
        let tracks = vec![
            Track {
                id: "t1".into(),
                title: "Песня 1".into(),
                artist: Some("Исполнитель".into()),
                album: None,
                duration_s: Some(180),
                path: "C:\\Music\\1.mp3".into(),
                ext: Some("mp3".into()),
                filesize: Some(1024),
                thumbnail_path: None,
                added_at: 1000,
                last_played: Some(2000),
                play_count: 5,
            },
            Track {
                id: "t2".into(),
                title: "Песня 2".into(),
                artist: Some("Исполнитель 2".into()),
                album: None,
                duration_s: Some(200),
                path: "C:\\Music\\2.mp3".into(),
                ext: Some("mp3".into()),
                filesize: Some(2048),
                thumbnail_path: None,
                added_at: 1100,
                last_played: None,
                play_count: 0,
            },
        ];
        let links = vec![
            ("pl_1".to_string(), "t1".to_string(), 0),
            ("pl_1".to_string(), "t2".to_string(), 1),
        ];

        db.import_library(&playlists, &tracks, &links).unwrap();

        let loaded_pls = db.list_playlists().unwrap();
        assert_eq!(loaded_pls.len(), 1);
        assert_eq!(loaded_pls[0].name, "Рок");

        let loaded_tracks = db.list_tracks(None).unwrap();
        assert_eq!(loaded_tracks.len(), 2);

        let pl_tracks = db.list_playlist_tracks("pl_1").unwrap();
        assert_eq!(pl_tracks.len(), 2);
        assert_eq!(pl_tracks[0].id, "t1");
        assert_eq!(pl_tracks[1].id, "t2");
    }

    #[test]
    fn test_list_playlists_reports_link_count() {
        let db = temp_test_db();
        let playlists = vec![Playlist {
            id: "pl_1".into(),
            name: "Рок".into(),
            source_url: "https://music.youtube.com/playlist?list=pl_1".into(),
            last_sync: None,
            // Исторический (неверный) счётчик: показывать должно число связок.
            track_count: 99,
            thumbnail_path: None,
        }];
        let track = |id: &str| Track {
            id: id.into(),
            title: id.into(),
            artist: None,
            album: None,
            duration_s: None,
            path: format!("C:\\Music\\{id}.mp3"),
            ext: Some("mp3".into()),
            filesize: None,
            thumbnail_path: None,
            added_at: 0,
            last_played: None,
            play_count: 0,
        };
        let tracks = vec![track("t1"), track("t2")];
        let links = vec![
            ("pl_1".to_string(), "t1".to_string(), 1),
            ("pl_1".to_string(), "t2".to_string(), 2),
        ];

        db.import_library(&playlists, &tracks, &links).unwrap();

        let listed = db.list_playlists().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].track_count, 2);
    }

    #[test]
    fn test_delete_playlist_keeps_tracks() {
        let db = temp_test_db();
        db.upsert_playlist("pl_1", "Рок", "https://music.youtube.com/playlist?list=pl_1")
            .unwrap();
        let track = DownloadedTrack {
            id: "t1".into(),
            title: "Песня".into(),
            artist: None,
            album: None,
            duration_s: None,
            path: "C:\\Music\\1.mp3".into(),
            ext: Some("mp3".into()),
            filesize: None,
            thumbnail_path: None,
        };
        db.upsert_track("pl_1", 1, &track).unwrap();

        db.delete_playlist("pl_1").unwrap();

        assert!(db.get_playlist("pl_1").unwrap().is_none());
        assert_eq!(
            db.list_playlist_tracks_sorted("pl_1", None, None)
                .unwrap()
                .len(),
            0
        );
        // Трек остаётся в библиотеке.
        assert!(db.get_track("t1").unwrap().is_some());
        assert_eq!(db.list_tracks(None).unwrap().len(), 1);
    }

    #[test]
    fn test_youtube_helpers() {
        assert!(is_youtube_id("dQw4w9WgXcQ"));
        assert!(!is_youtube_id("invalid"));
        assert_eq!(
            youtube_playlist_list_id("https://music.youtube.com/playlist?list=PL12345"),
            Some("PL12345".into())
        );
    }
}
