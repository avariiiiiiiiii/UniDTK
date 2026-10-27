use crate::{
    json_util::{int_field, string_field, thumbnail_url},
    library::config::ConfigStore,
    library::db::{file_size, manual_playlist_id, now_ts, stable_hash, Database, DownloadedTrack, Playlist},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::Semaphore,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTask {
    pub id: String,
    pub playlist_id: String,
    pub label: String,
    pub source_url: String,
    pub status: String,
    pub progress_pct: Option<f64>,
    pub current: Option<String>,
    pub downloaded: i64,
    pub message: Option<String>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Clone)]
pub struct SyncManager {
    tasks: Arc<Mutex<HashMap<String, SyncTask>>>,
    db: Database,
    config: ConfigStore,
    semaphore: Arc<Semaphore>,
}

struct YtDlpRun {
    downloaded: i64,
    warning: Option<String>,
}

#[derive(Debug, Clone)]
struct PlaylistSnapshot {
    title: Option<String>,
    thumbnail_url: Option<String>,
    entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Clone)]
struct PlaylistEntry {
    id: String,
    title: Option<String>,
    artist: Option<String>,
    duration_s: Option<i64>,
    thumbnail_url: Option<String>,
    url: String,
    position: i64,
}

impl SyncManager {
    pub fn new(db: Database, config: ConfigStore) -> Self {
        let parallelism = config.get().sync_parallelism.max(1);
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            db,
            config,
            semaphore: Arc::new(Semaphore::new(parallelism)),
        }
    }

    pub fn start_url(
        &self,
        url: String,
        name: Option<String>,
    ) -> Result<String, String> {
        let url = url.trim().to_string();
        if url.is_empty() {
            return Err("Paste a YouTube or YouTube Music playlist URL first".to_string());
        }

        let playlist_id = manual_playlist_id(&url);
        let label = name
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| short_source_label(&url));
        self.db.upsert_playlist(&playlist_id, &label, &url)?;
        self.start_playlist(
            Playlist {
                id: playlist_id,
                name: label,
                source_url: url,
                last_sync: None,
                track_count: 0,
                thumbnail_path: None,
            },
        )
    }

    pub fn start_existing_playlist(
        &self,
        playlist_id: String,
    ) -> Result<String, String> {
        let playlist = self
            .db
            .get_playlist(&playlist_id)?
            .ok_or_else(|| format!("Playlist not found: {playlist_id}"))?;
        self.start_playlist(playlist)
    }

    pub fn start_all(&self) -> Result<Vec<String>, String> {
        let playlists = self.db.list_playlists()?;
        playlists
            .into_iter()
            .filter(|playlist| !playlist.source_url.trim().is_empty())
            .map(|playlist| self.start_playlist(playlist))
            .collect()
    }

    /// Queue sync only for playlists that have never been synced or whose
    /// `last_sync` is older than `max_age_secs`. Used by the background
    /// auto-sync loop so restarts within the interval don't re-scan everything.
    pub fn start_stale(
        &self,
        max_age_secs: i64,
    ) -> Result<Vec<String>, String> {
        let now = now_ts();
        let playlists = self.db.list_playlists()?;
        let mut ids = Vec::new();
        for playlist in playlists {
            if playlist.source_url.trim().is_empty() {
                continue;
            }
            let stale = match playlist.last_sync {
                Some(ts) => now.saturating_sub(ts) >= max_age_secs,
                None => true,
            };
            if !stale {
                continue;
            }
            ids.push(self.start_playlist(playlist)?);
        }
        Ok(ids)
    }

    pub fn tasks(&self) -> Vec<SyncTask> {
        let mut tasks: Vec<_> = self
            .tasks
            .lock()
            .expect("sync task lock poisoned")
            .values()
            .cloned()
            .collect();
        tasks.sort_by(|a, b| b.queued_at.cmp(&a.queued_at));
        tasks
    }

    async fn run_task(self, task_id: String) {
        let permit = match self.semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                self.update(&task_id, |task| {
                    task.status = "error".to_string();
                    task.message = Some(format!("Sync queue closed: {error}"));
                    task.finished_at = Some(now_ts());
                });
                return;
            }
        };
        let _permit = permit;

        self.update(&task_id, |task| {
            task.status = "downloading".to_string();
            task.started_at = Some(now_ts());
            task.message = None;
        });

        let task = match self.get_task(&task_id) {
            Some(task) => task,
            None => return,
        };
        let result = self.run_yt_dlp(&task).await;
        match result {
            Ok(run) => {
                if let Err(error) = self.db.mark_playlist_synced(&task.playlist_id) {
                    self.update(&task_id, |task| {
                        task.status = "error".to_string();
                        task.message = Some(error);
                        task.finished_at = Some(now_ts());
                    });
                    return;
                }
                self.update(&task_id, |task| {
                    task.status = "done".to_string();
                    task.progress_pct = Some(100.0);
                    task.current = None;
                    task.downloaded = run.downloaded;
                    task.message = Some(if let Some(warning) = run.warning {
                        if run.downloaded == 0 {
                            format!("No new downloads; yt-dlp warning: {warning}")
                        } else {
                            format!(
                                "Downloaded {} track(s); yt-dlp warning: {warning}",
                                run.downloaded
                            )
                        }
                    } else if run.downloaded == 0 {
                        "No new downloads; archive is up to date".to_string()
                    } else {
                        format!("Downloaded {} track(s)", run.downloaded)
                    });
                    task.finished_at = Some(now_ts());
                });
            }
            Err(error) => {
                self.update(&task_id, |task| {
                    task.status = "error".to_string();
                    task.message = Some(error);
                    task.finished_at = Some(now_ts());
                });
            }
        }
    }

    async fn run_yt_dlp(&self, task: &SyncTask) -> Result<YtDlpRun, String> {
        match self.fetch_playlist_snapshot(&task.source_url).await {
            Ok(snapshot) if !snapshot.entries.is_empty() => {
                self.run_incremental_snapshot(task, snapshot).await
            }
            Ok(_) if is_playlist_source(&task.source_url) => Err(
                "yt-dlp metadata scan returned an empty playlist; full playlist download was not started"
                    .to_string(),
            ),
            Ok(_) => {
                self.run_yt_dlp_download(task, None, &HashMap::new())
                    .await
            }
            Err(error) => {
                if is_playlist_source(&task.source_url) {
                    return Err(format!(
                        "yt-dlp metadata scan failed; full playlist download was not started: {error}"
                    ));
                }
                self.update(&task.id, |task| {
                    task.message = Some(format!("Metadata scan skipped: {error}"));
                });
                self.run_yt_dlp_download(task, None, &HashMap::new())
                    .await
            }
        }
    }

    async fn run_incremental_snapshot(
        &self,
        task: &SyncTask,
        snapshot: PlaylistSnapshot,
    ) -> Result<YtDlpRun, String> {
        let config = self.config.get();
        let library_dir = PathBuf::from(&config.library_dir);
        fs::create_dir_all(&library_dir).map_err(|e| {
            format!(
                "Failed to create library directory {}: {e}",
                library_dir.display()
            )
        })?;

        let playlist_title = if snapshot.entries.len() == 1 {
            snapshot.entries[0]
                .title
                .as_deref()
                .or(snapshot.title.as_deref())
        } else {
            snapshot.title.as_deref()
        };
        self.db.set_playlist_metadata(
            &task.playlist_id,
            playlist_title,
            snapshot.thumbnail_url.as_deref(),
        )?;

        self.update(&task.id, |task| {
            task.status = "checking".to_string();
            task.current = Some("Checking local library".to_string());
            task.progress_pct = Some(0.0);
        });

        let current_ids = snapshot
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        self.db
            .prune_playlist_tracks(&task.playlist_id, &current_ids)?;

        let mut missing = Vec::new();
        let mut positions = HashMap::<String, i64>::new();
        let mut recovered = 0_i64;
        let total = snapshot.entries.len().max(1) as f64;
        // Single recursive scan of the library; map track-id -> path.
        // Replaces the previous O(N_entries × N_files) per-track walk.
        let library_index = build_library_index(&library_dir);

        for (index, entry) in snapshot.entries.iter().enumerate() {
            positions.insert(entry.id.clone(), entry.position);

            if let Some(track) = self.db.get_track(&entry.id)? {
                let file_exists = Path::new(&track.path).exists();
                if file_exists {
                    self.db
                        .link_track_to_playlist(&task.playlist_id, entry.position, &entry.id)?;
                    self.update(&task.id, |task| {
                        task.current = entry.title.clone();
                        task.progress_pct = Some(((index + 1) as f64 / total) * 20.0);
                    });
                    continue;
                } else if let Some(path) = library_index.get(&entry.id).cloned() {
                    // File relocated within library folder: update path and continue
                    let path_ref = Path::new(&path);
                    let downloaded = DownloadedTrack {
                        id: track.id.clone(),
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        album: track.album.clone(),
                        duration_s: track.duration_s,
                        ext: path_ref
                            .extension()
                            .map(|v| v.to_string_lossy().to_string()),
                        filesize: file_size(path_ref),
                        thumbnail_path: track.thumbnail_path.clone(),
                        path,
                    };
                    self.db.upsert_track(&task.playlist_id, entry.position, &downloaded)?;
                    continue;
                }
                // File physically missing from disk and not found in library_index:
                // Fall through so it is added to `missing` and downloaded!
            }

            if let Some(path) = library_index.get(&entry.id).cloned() {
                let path_ref = Path::new(&path);
                let track = DownloadedTrack {
                    id: entry.id.clone(),
                    title: entry.title.clone().unwrap_or_else(|| entry.id.clone()),
                    artist: entry.artist.clone(),
                    album: snapshot.title.clone(),
                    duration_s: entry.duration_s,
                    ext: path_ref
                        .extension()
                        .map(|value| value.to_string_lossy().to_string()),
                    filesize: file_size(path_ref),
                    thumbnail_path: find_thumbnail_for(path_ref)
                        .or_else(|| entry.thumbnail_url.clone()),
                    path,
                };
                self.db
                    .upsert_track(&task.playlist_id, entry.position, &track)?;
                recovered += 1;
                self.update(&task.id, |task| {
                    task.downloaded = recovered;
                    task.current = Some(track.title.clone());
                    task.progress_pct = Some(((index + 1) as f64 / total) * 20.0);
                });
                continue;
            }

            missing.push(entry.clone());
        }

        if missing.is_empty() {
            return Ok(YtDlpRun {
                downloaded: recovered,
                warning: None,
            });
        }

        self.update(&task.id, |task| {
            task.status = "downloading".to_string();
            task.current = Some(format!(
                "Downloading {} missing of {}",
                missing.len(),
                snapshot.entries.len()
            ));
            task.message = Some("Existing tracks were linked without re-downloading".to_string());
            task.progress_pct = Some(20.0);
        });

        let urls = missing
            .into_iter()
            .map(|entry| entry.url)
            .collect::<Vec<_>>();
        match self
            .run_yt_dlp_download(task, Some(urls), &positions)
            .await
        {
            Ok(run) => Ok(YtDlpRun {
                downloaded: recovered + run.downloaded,
                warning: run.warning,
            }),
            Err(detail) if recovered > 0 => Ok(YtDlpRun {
                downloaded: recovered,
                warning: Some(format!(
                    "{} track(s) linked; missing tracks failed: {detail}",
                    recovered
                )),
            }),
            Err(detail) => Err(detail),
        }
    }

    async fn run_yt_dlp_download(
        &self,
        task: &SyncTask,
        urls: Option<Vec<String>>,
        known_positions: &HashMap<String, i64>,
    ) -> Result<YtDlpRun, String> {
        let config = self.config.get();
        let library_dir = PathBuf::from(&config.library_dir);
        fs::create_dir_all(&library_dir).map_err(|e| {
            format!(
                "Failed to create library directory {}: {e}",
                library_dir.display()
            )
        })?;

        let archive = self.config.archive_path();
        let output_template = library_dir
            .join("%(uploader)s")
            .join("%(title)s [%(id)s].%(ext)s")
            .to_string_lossy()
            .to_string();

        let mut command = Command::new(self.config.resolve_yt_dlp());
        apply_headless_flags(&mut command);
        command
            .arg("-f")
            .arg("bestaudio[ext=m4a]/bestaudio")
            .arg("--cookies-from-browser")
            .arg(config.cookies_browser)
            .arg("--extractor-args")
            .arg("youtubetab:skip=authcheck")
            .arg("--add-metadata")
            .arg("--embed-thumbnail")
            .arg("--embed-metadata")
            .arg("--write-thumbnail")
            .arg("--no-overwrites")
            .arg("--print-json")
            .arg("--newline")
            .arg("--progress-template")
            .arg("download:%(progress._percent_str)s")
            .arg("--ignore-errors")
            .arg("-o")
            .arg(output_template)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        if urls.is_none() {
            command.arg("--download-archive").arg(archive);
        }

        if let Some(urls) = &urls {
            command.args(urls);
        } else {
            command.arg(&task.source_url);
        }

        let mut child = command.spawn().map_err(|e| {
            format!(
                "Failed to start yt-dlp. Put yt-dlp.exe next to the app, set yt_dlp_path in config.toml, or install it in PATH: {e}"
            )
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "yt-dlp stdout was not captured".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "yt-dlp stderr was not captured".to_string())?;

        let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_capture = stderr_lines.clone();
        let stderr_manager = self.clone();
        let stderr_task_id = task.id.clone();
        let stderr_reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if let Ok(mut captured) = stderr_capture.lock() {
                    captured.push(line.clone());
                    if captured.len() > 20 {
                        captured.remove(0);
                    }
                }
                stderr_manager.update(&stderr_task_id, |task| {
                    task.message = Some(line.clone());
                });
            }
        });

        let mut downloaded = 0_i64;
        let mut position = 0_i64;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| format!("Failed reading yt-dlp output: {e}"))?
        {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(percent) = parse_progress_line(trimmed) {
                self.update(&task.id, |task| {
                    task.progress_pct = Some(percent);
                    task.status = "downloading".to_string();
                });
                continue;
            }

            if trimmed.starts_with('{') {
                let value = serde_json::from_str::<Value>(trimmed)
                    .map_err(|e| format!("yt-dlp returned invalid JSON: {e}"))?;
                if let Some((track, json_position)) = extract_track(&value, &library_dir) {
                    position = json_position
                        .or_else(|| known_positions.get(&track.id).copied())
                        .unwrap_or(position + 1);
                    self.db.upsert_track(&task.playlist_id, position, &track)?;
                    downloaded += 1;
                    self.update(&task.id, |task| {
                        task.downloaded = downloaded;
                        task.current = Some(track.title.clone());
                        task.progress_pct = Some(100.0);
                    });
                }
            } else if trimmed.contains("has already been recorded in the archive") {
                self.update(&task.id, |task| {
                    task.message = Some("Already in archive".to_string());
                });
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| format!("Failed waiting for yt-dlp: {e}"))?;
        let _ = stderr_reader.await;
        if !status.success() {
            let detail = format_yt_dlp_failure(&status.to_string(), &stderr_lines);
            if downloaded > 0 {
                return Ok(YtDlpRun {
                    downloaded,
                    warning: Some(detail),
                });
            }
            // Часть записей недоступна в источнике (удалены/приватны/регион):
            // yt-dlp выходит с кодом 1, хотя скачивать больше нечего — это
            // пропуск, а не сбой синхронизации.
            let (errors, unavailable) = yt_dlp_error_stats(&stderr_lines);
            if errors > 0 && errors == unavailable {
                return Ok(YtDlpRun {
                    downloaded,
                    warning: Some(format!(
                        "{unavailable} unavailable video(s) in the source were skipped"
                    )),
                });
            }
            return Err(detail);
        }

        Ok(YtDlpRun {
            downloaded,
            warning: None,
        })
    }

    async fn fetch_playlist_snapshot(&self, source_url: &str) -> Result<PlaylistSnapshot, String> {
        let config = self.config.get();
        let mut command = Command::new(self.config.resolve_yt_dlp());
        apply_headless_flags(&mut command);
        let output = command
            .arg("--cookies-from-browser")
            .arg(config.cookies_browser)
            .arg("--extractor-args")
            .arg("youtubetab:skip=authcheck")
            .arg("--flat-playlist")
            .arg("--dump-single-json")
            .arg("--ignore-errors")
            .arg("--no-warnings")
            .arg(source_url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("Failed to scan playlist metadata with yt-dlp: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                format!("yt-dlp metadata scan exited with {}", output.status)
            } else {
                format!(
                    "yt-dlp metadata scan exited with {}: {detail}",
                    output.status
                )
            });
        }

        let json = stdout
            .lines()
            .find(|line| line.trim_start().starts_with('{'))
            .ok_or_else(|| "yt-dlp metadata scan did not return JSON".to_string())?;
        let value = serde_json::from_str::<Value>(json)
            .map_err(|e| format!("yt-dlp metadata JSON was invalid: {e}"))?;
        Ok(extract_playlist_snapshot(&value, source_url))
    }

    fn start_playlist(&self, playlist: Playlist) -> Result<String, String> {
        if let Some(existing_id) = self.active_task_for_playlist(&playlist.id) {
            return Ok(existing_id);
        }

        let task_id = format!("sync:{}:{}", now_ts(), stable_hash(&playlist.source_url));
        let task = SyncTask {
            id: task_id.clone(),
            playlist_id: playlist.id,
            label: playlist.name,
            source_url: playlist.source_url,
            status: "queued".to_string(),
            progress_pct: None,
            current: None,
            downloaded: 0,
            message: None,
            queued_at: now_ts(),
            started_at: None,
            finished_at: None,
        };
        self.tasks
            .lock()
            .expect("sync task lock poisoned")
            .insert(task_id.clone(), task);
        self.emit();

        let manager = self.clone();
        let run_id = task_id.clone();
        tauri::async_runtime::spawn(async move {
            manager.run_task(run_id).await;
        });

        Ok(task_id)
    }

    fn active_task_for_playlist(&self, playlist_id: &str) -> Option<String> {
        self.tasks
            .lock()
            .expect("sync task lock poisoned")
            .values()
            .find(|task| {
                task.playlist_id == playlist_id
                    && matches!(task.status.as_str(), "queued" | "checking" | "downloading")
            })
            .map(|task| task.id.clone())
    }

    fn get_task(&self, task_id: &str) -> Option<SyncTask> {
        self.tasks
            .lock()
            .expect("sync task lock poisoned")
            .get(task_id)
            .cloned()
    }

    fn update<F>(&self, task_id: &str, update: F)
    where
        F: FnOnce(&mut SyncTask),
    {
        if let Some(task) = self
            .tasks
            .lock()
            .expect("sync task lock poisoned")
            .get_mut(task_id)
        {
            update(task);
        }
        self.emit();
    }

    fn emit(&self) {
        unidtk_api::emit(
            "sync-tasks",
            serde_json::to_value(self.tasks()).unwrap_or(serde_json::Value::Null),
        );
    }
}

// yt-dlp refuses to work reliably once its build is >90 days old (YouTube
// keeps changing the signature/cipher logic it depends on), and the sync UI
// has no human in the loop to notice the warning and click "Update yt-dlp".
// So every shell that builds an AppState gets this for free at startup.
const AUTO_UPDATE_INTERVAL_SECS: u64 = 20 * 60 * 60;

fn ytdlp_marker_path(config: &ConfigStore) -> PathBuf {
    config.path().with_file_name(".ytdlp_last_update")
}

/// Время (unix seconds) последней авто-проверки yt-dlp — для статуса подмодуля
/// в реестре настроек.
pub fn ytdlp_last_update(config: &ConfigStore) -> Option<u64> {
    fs::read_to_string(ytdlp_marker_path(config))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// Best-effort, throttled background self-update. Runs at most once per
/// AUTO_UPDATE_INTERVAL_SECS so we don't hit yt-dlp's GitHub release check
/// (and add startup latency) on every single launch.
pub async fn spawn_auto_update(config: ConfigStore) {
    if !config.get().auto_update_enabled {
        return;
    }
    let marker = ytdlp_marker_path(&config);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Ok(text) = fs::read_to_string(&marker) {
        if let Ok(last) = text.trim().parse::<u64>() {
            if now.saturating_sub(last) < AUTO_UPDATE_INTERVAL_SECS {
                return;
            }
        }
    }
    let _ = update_yt_dlp(config).await;
    let _ = fs::write(&marker, now.to_string());
}

/// Delay before the first auto-sync pass so yt-dlp update / app attach settle.
const AUTO_SYNC_STARTUP_DELAY_SECS: u64 = 90;

/// Periodically re-sync every local playlist against its YouTube/YTM source:
/// new tracks download, removed tracks are pruned from the playlist links.
/// Controlled by `auto_sync_enabled` / `auto_sync_interval_hours` in config.toml.
pub async fn spawn_auto_playlist_sync(
    sync: SyncManager,
    config: ConfigStore,
) {
    tokio::time::sleep(Duration::from_secs(AUTO_SYNC_STARTUP_DELAY_SECS)).await;

    loop {
        let cfg = config.get();
        if cfg.auto_sync_enabled {
            let max_age_secs = (cfg.auto_sync_interval_hours * 3600.0).round() as i64;
            match sync.start_stale(max_age_secs.max(1)) {
                Ok(ids) if !ids.is_empty() => {
                    unidtk_api::emit(
                        "sync-auto",
                        serde_json::json!({
                            "queued": ids.len(),
                            "taskIds": ids,
                        }),
                    );
                }
                Ok(_) => {}
                Err(_) => {
                    // Soft-fail: next interval retries. Avoid panicking the loop.
                }
            }
        }

        let sleep_secs = (config.get().auto_sync_interval_hours * 3600.0)
            .clamp(15.0 * 60.0, 7.0 * 24.0 * 3600.0) as u64;
        tokio::time::sleep(Duration::from_secs(sleep_secs.max(60))).await;
    }
}

/// Hide the Windows console that console-subsystem tools (yt-dlp.exe) flash
/// when spawned from a GUI host. Without CREATE_NO_WINDOW auto-sync pops a
/// black terminal for every playlist scan/download.
fn apply_headless_flags(command: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = command;
}

pub async fn update_yt_dlp(config: ConfigStore) -> Result<String, String> {
    let mut command = Command::new(config.resolve_yt_dlp());
    apply_headless_flags(&mut command);
    let output = command
        .arg("-U")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to run yt-dlp -U: {e}"))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text.trim().to_string())
    } else {
        Err(format!("yt-dlp -U failed: {}", text.trim()))
    }
}

fn parse_progress_line(line: &str) -> Option<f64> {
    let value = line.strip_prefix("download:")?.trim();
    let number = value
        .trim_end_matches('%')
        .trim()
        .split_whitespace()
        .next()?;
    number.parse::<f64>().ok()
}

/// Сколько строк-ошибок yt-dlp и сколько из них — «запись недоступна в
/// источнике» (удалена/приватна/регион). Такие пропуски не должны помечать
/// синк как сбой: качать больше нечего, плейлист просто короче источника.
fn yt_dlp_error_stats(stderr_lines: &Arc<Mutex<Vec<String>>>) -> (usize, usize) {
    let Ok(guard) = stderr_lines.lock() else {
        return (0, 0);
    };
    let mut errors = 0usize;
    let mut unavailable = 0usize;
    for line in guard.iter() {
        if !line.contains("ERROR:") {
            continue;
        }
        errors += 1;
        let lower = line.to_ascii_lowercase();
        if lower.contains("video unavailable")
            || lower.contains("this video is not available")
            || lower.contains("private video")
            || lower.contains("members-only")
            || lower.contains("not available in your country")
            || lower.contains("has already been recorded in the archive")
        {
            unavailable += 1;
        }
    }
    (errors, unavailable)
}

fn format_yt_dlp_failure(status: &str, stderr_lines: &Arc<Mutex<Vec<String>>>) -> String {
    let lines = stderr_lines
        .lock()
        .map(|captured| {
            let mut tail: Vec<_> = captured.iter().rev().take(4).cloned().collect();
            tail.reverse();
            tail
        })
        .unwrap_or_default();

    if lines.is_empty() {
        format!("yt-dlp exited with status {status}")
    } else {
        format!("yt-dlp exited with status {status}: {}", lines.join(" | "))
    }
}

fn is_playlist_source(source_url: &str) -> bool {
    let lower = source_url.to_ascii_lowercase();
    (lower.contains("youtube.com/playlist") || lower.contains("music.youtube.com/playlist"))
        && lower.contains("list=")
}

fn extract_playlist_snapshot(value: &Value, source_url: &str) -> PlaylistSnapshot {
    let title = string_field(value, &["playlist_title", "playlist", "title"]);
    let thumbnail_url = thumbnail_url(value);
    let mut entries = Vec::new();

    if let Some(items) = value.get("entries").and_then(Value::as_array) {
        for (index, item) in items.iter().enumerate() {
            if let Some(entry) = extract_playlist_entry(item, source_url, (index + 1) as i64) {
                entries.push(entry);
            }
        }
    } else if let Some(entry) = extract_playlist_entry(value, source_url, 1) {
        entries.push(entry);
    }

    PlaylistSnapshot {
        title,
        thumbnail_url,
        entries,
    }
}

fn extract_playlist_entry(
    value: &Value,
    source_url: &str,
    fallback_position: i64,
) -> Option<PlaylistEntry> {
    let id = string_field(value, &["id", "display_id", "url"])?;
    let id = id
        .trim()
        .trim_start_matches("https://music.youtube.com/watch?v=")
        .trim_start_matches("https://www.youtube.com/watch?v=")
        .trim_start_matches("https://youtube.com/watch?v=")
        .split(['&', '#'])
        .next()
        .unwrap_or(&id)
        .to_string();
    if id.is_empty() {
        return None;
    }
    let title = string_field(value, &["title", "track", "fulltitle"]);
    let artist = string_field(value, &["artist", "uploader", "creator", "channel"]);
    let duration_s = int_field(value, &["duration"]).or_else(|| {
        value
            .get("duration")
            .and_then(Value::as_f64)
            .map(|seconds| seconds.round() as i64)
    });
    let position =
        int_field(value, &["playlist_index", "playlist_autonumber"]).unwrap_or(fallback_position);
    let url = string_field(value, &["webpage_url", "original_url", "url"])
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        .unwrap_or_else(|| {
            if source_url.contains("music.youtube.com") {
                format!("https://music.youtube.com/watch?v={id}")
            } else {
                format!("https://www.youtube.com/watch?v={id}")
            }
        });

    Some(PlaylistEntry {
        id,
        title,
        artist,
        duration_s,
        thumbnail_url: thumbnail_url(value),
        url,
        position,
    })
}

fn extract_track(value: &Value, library_dir: &Path) -> Option<(DownloadedTrack, Option<i64>)> {
    let id = string_field(value, &["id", "display_id"])?;
    let title = string_field(value, &["title", "track", "fulltitle"]).unwrap_or_else(|| id.clone());
    let artist = string_field(value, &["artist", "uploader", "creator", "channel"]);
    let album = string_field(value, &["album"]);
    let duration_s = int_field(value, &["duration"]).or_else(|| {
        value
            .get("duration")
            .and_then(Value::as_f64)
            .map(|seconds| seconds.round() as i64)
    });
    let requested = value
        .get("requested_downloads")
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let path = requested
        .and_then(|download| string_field(download, &["filepath", "filename", "_filename"]))
        .or_else(|| string_field(value, &["filepath", "_filename", "filename"]))
        .or_else(|| locate_download_by_id(library_dir, &id));
    let path = path?;
    let path_ref = Path::new(&path);
    let ext = path_ref
        .extension()
        .map(|value| value.to_string_lossy().to_string())
        .or_else(|| string_field(value, &["ext"]));
    let filesize = requested
        .and_then(|download| int_field(download, &["filesize", "filesize_approx"]))
        .or_else(|| int_field(value, &["filesize", "filesize_approx"]))
        .or_else(|| file_size(path_ref));
    let thumbnail_path = find_thumbnail_for(path_ref).or_else(|| thumbnail_url(value));
    let playlist_index = int_field(value, &["playlist_index"]);

    Some((
        DownloadedTrack {
            id,
            title,
            artist,
            album,
            duration_s,
            path,
            ext,
            filesize,
            thumbnail_path,
        },
        playlist_index,
    ))
}


fn locate_download_by_id(library_dir: &Path, id: &str) -> Option<String> {
    let entries = fs::read_dir(library_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = locate_download_by_id(&path, id) {
                return Some(found);
            }
        } else if path
            .file_stem()
            .map(|stem| stem.to_string_lossy().contains(&format!("[{id}]")))
            .unwrap_or(false)
        {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

/// Walk `library_dir` once and return a map from yt-dlp track id (extracted
/// from `Title [ID].ext`) to the absolute file path. Audio extensions only —
/// thumbnail siblings (.jpg/.webp) are skipped so they don't shadow the audio.
fn build_library_index(library_dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut stack = vec![library_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            match ext.as_deref() {
                Some("m4a" | "mp3" | "opus" | "ogg" | "webm" | "flac" | "aac" | "wav") => {}
                _ => continue,
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // yt-dlp template "%(title)s [%(id)s]" → grab the last [...] group.
            if let (Some(open), Some(close)) = (stem.rfind('['), stem.rfind(']')) {
                if open < close {
                    let id = &stem[open + 1..close];
                    if !id.is_empty() {
                        out.entry(id.to_string())
                            .or_insert_with(|| path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    out
}

pub fn find_thumbnail_for(audio_path: &Path) -> Option<String> {
    let dir = audio_path.parent()?;
    let mut stem = audio_path.file_stem()?.to_string_lossy().to_string();
    // Strip trailing .part or intermediate audio extension if present
    if stem.ends_with(".part") {
        stem = stem[..stem.len() - 5].to_string();
    }
    for ext in [".m4a", ".mp3", ".opus", ".ogg", ".webm", ".flac", ".aac", ".wav"] {
        if stem.to_ascii_lowercase().ends_with(ext) {
            stem = stem[..stem.len() - ext.len()].to_string();
            break;
        }
    }
    for ext in ["webp", "jpg", "jpeg", "png"] {
        let candidate = dir.join(format!("{stem}.{ext}"));
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    // Also try matching by [ID] if present in stem
    if let (Some(open), Some(close)) = (stem.rfind('['), stem.rfind(']')) {
        if open < close {
            let id = &stem[open + 1..close];
            if !id.is_empty() {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if matches!(ext.to_ascii_lowercase().as_str(), "webp" | "jpg" | "jpeg" | "png") {
                                if let Some(entry_stem) = path.file_stem().and_then(|s| s.to_str()) {
                                    if entry_stem.contains(&format!("[{id}]")) {
                                        return Some(path.to_string_lossy().to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn short_source_label(url: &str) -> String {
    if url.len() <= 54 {
        url.to_string()
    } else {
        format!("{}...", &url[..54])
    }
}
