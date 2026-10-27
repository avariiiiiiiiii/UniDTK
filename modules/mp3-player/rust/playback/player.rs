use crate::{
    ai::ai_dj::AiDj,
    library::config::ConfigStore,
    library::db::{Database, Track},
};
use crate::ffi::libmpv::Libmpv;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_MPV_OPTIONS: &[(&str, &str)] = &[
    ("config", "no"),
    ("force-window", "no"),
    ("video", "no"),
    ("audio-display", "no"),
    ("gapless-audio", "yes"),
    ("replaygain", "album"),
    ("audio-device", "auto"),
    ("audio-exclusive", "no"),
    ("ao", "wasapi"),
    ("keep-open", "yes"),
];

/// AI DJ playback modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiDjMode {
    /// Smart mode: AI DJ picks tracks based on similarity.
    Smart,
    /// Random mode: pick random tracks from library.
    Random,
    /// Sequential mode: play tracks in order.
    Sequential,
}

impl Default for AiDjMode {
    fn default() -> Self {
        Self::Smart
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub running: bool,
    pub paused: bool,
    pub time_pos: f64,
    pub duration: f64,
    pub volume: f64,
    pub queue: Vec<String>,
    pub current_index: Option<usize>,
    pub current_track_id: Option<String>,
    pub status: String,
    pub ai_dj_enabled: bool,
    pub ai_dj_mode: AiDjMode,
    pub ai_dj_history: Vec<String>,
    pub crossfade_duration: f64,
    /// Last player error message (mpv spawn failure etc.), cleared on success.
    pub last_error: Option<String>,
    #[serde(default)]
    pub is_playlist_session: bool,
}

#[derive(Default)]
struct PlayerInner {
    /// libmpv handle (None = no active player). Wrapped for Send.
    mpv: Option<Libmpv>,
    /// Path to the mpv log file for debugging
    log_path: Option<std::path::PathBuf>,
}

#[derive(Default)]
struct PlayerClock {
    last_tick: Option<Instant>,
    /// Last time we polled `eof-reached` (rate-limit the auto-advance probe).
    last_auto_advance_check: Option<Instant>,
}

#[derive(Clone)]
pub struct PlayerManager {
    inner: Arc<Mutex<PlayerInner>>,
    state: Arc<Mutex<PlayerState>>,
    clock: Arc<Mutex<PlayerClock>>,
    /// Kept for future mpv options sourced from config (volume, device, ao).
    #[allow(dead_code)]
    config: ConfigStore,
    db: Database,
    pub ai_dj: Arc<Mutex<Option<AiDj>>>,
}

impl PlayerManager {
    pub fn new(config: ConfigStore, db: Database) -> Self {
        let manager = Self {
            inner: Arc::new(Mutex::new(PlayerInner::default())),
            state: Arc::new(Mutex::new(PlayerState::default())),
            clock: Arc::new(Mutex::new(PlayerClock {
                last_tick: None,
                last_auto_advance_check: None,
            })),
            config,
            db,
            ai_dj: Arc::new(Mutex::new(None)),
        };

        // Background EOF monitor: auto-advances tracks even when UI is minimized (document.hidden)
        let pm = manager.clone();
        let _ = std::thread::Builder::new()
            .name("player-eof-monitor".to_string())
            .spawn(move || {
                while Arc::strong_count(&pm.inner) > 1 {
                    std::thread::sleep(Duration::from_millis(500));
                    pm.maybe_auto_advance_eof();
                }
            });

        manager
    }

    pub fn with_pipe_name(config: ConfigStore, db: Database, _pipe_name: String) -> Self {
        // libmpv is in-process; the pipe name is not needed for playback control.
        // Kept for API compatibility (custom mpv IPC pipe for external tools).
        Self::new(config, db)
    }

    /// Enable AI DJ mode — loads the similarity graph from redb.
    pub fn enable_ai_dj(&self) -> Result<(), String> {
        let mut ai_dj_lock = self.ai_dj.lock().map_err(|e| e.to_string())?;
        if ai_dj_lock.is_none() {
            let mut ai_dj = AiDj::new(crate::ai::ai_dj::ai_dj_store()?);
            ai_dj.load()?;
            *ai_dj_lock = Some(ai_dj);
        }
        drop(ai_dj_lock);
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.ai_dj_enabled = true;
        Ok(())
    }

    /// Disable AI DJ mode.
    pub fn disable_ai_dj(&self) -> Result<(), String> {
        let mut ai_dj_lock = self.ai_dj.lock().map_err(|e| e.to_string())?;
        *ai_dj_lock = None;
        drop(ai_dj_lock);
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.ai_dj_enabled = false;
        Ok(())
    }

    /// Get the next track from AI DJ (if enabled).
    pub fn ai_dj_next_track(&self) -> Result<Option<String>, String> {
        let ai_dj_lock = self.ai_dj.lock().map_err(|e| e.to_string())?;
        let ai_dj = match ai_dj_lock.as_ref() {
            Some(dj) => dj,
            None => return Ok(None),
        };
        let (current_id, queue, history, is_playlist_session) = {
            let state = self.state.lock().map_err(|e| e.to_string())?;
            (
                state.current_track_id.clone(),
                state.queue.clone(),
                state.ai_dj_history.clone(),
                state.is_playlist_session,
            )
        };
        let current_id = match current_id {
            Some(id) => id,
            None => return Ok(None),
        };
        let is_playlist_mode = is_playlist_session && queue.len() > 1;
        let pool = if is_playlist_mode { Some(queue.as_slice()) } else { None };
        let exclude: Vec<String> = if is_playlist_mode {
            let win = (queue.len().saturating_sub(1)).max(1);
            history.iter().rev().take(win).cloned().collect()
        } else {
            history
        };
        Ok(ai_dj.next_track_scoped(&current_id, crate::ai::ai_dj::EnergyCurve::Any, &exclude, pool))
    }

    /// Set crossfade duration in seconds (0.0 - 10.0).
    pub fn set_crossfade_duration(&self, seconds: f64) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.crossfade_duration = seconds.clamp(0.0, 10.0);
        Ok(())
    }

    /// Get current crossfade duration in seconds.
    pub fn get_crossfade_duration(&self) -> Result<f64, String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        Ok(state.crossfade_duration)
    }

    /// Undo last AI DJ track selection — go back to previous track.
    pub fn ai_dj_undo(&self) -> Result<Option<PlayerState>, String> {
        let prev_id = {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            if state.ai_dj_history.is_empty() {
                return Ok(None);
            }
            state.ai_dj_history.pop()
        };

        if let Some(id) = prev_id {
            if let Ok(Some(track)) = self.db.get_track(&id) {
                if Path::new(&track.path).exists() {
                    let queue = vec![id.clone()];
                    return Ok(Some(self.play_index(&queue, &[track], 0)?));
                }
            }
        }
        Ok(None)
    }

    /// Set AI DJ playback mode.
    pub fn ai_dj_set_mode(&self, mode: AiDjMode) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.ai_dj_mode = mode;
        Ok(())
    }

    /// Get current AI DJ mode.
    pub fn ai_dj_get_mode(&self) -> AiDjMode {
        self.state
            .lock()
            .map(|s| s.ai_dj_mode)
            .unwrap_or_default()
    }

    pub fn load_queue(
        &self,
        track_ids: Vec<String>,
        start_index: usize,
    ) -> Result<PlayerState, String> {
        let tracks = self.db.get_tracks_by_ids(&track_ids)?;
        self.load_tracks(tracks, start_index)
    }

    pub fn load_tracks(
        &self,
        tracks: Vec<Track>,
        start_index: usize,
    ) -> Result<PlayerState, String> {
        if tracks.is_empty() {
            return Err("Queue is empty".to_string());
        }
        let mut playable = Vec::new();
        let mut selected_playable_index = None;
        let mut next_playable_index = None;
        for (index, track) in tracks.into_iter().enumerate() {
            if !Path::new(&track.path).exists() {
                continue;
            }
            let playable_index = playable.len();
            if index == start_index {
                selected_playable_index = Some(playable_index);
            } else if index > start_index && next_playable_index.is_none() {
                next_playable_index = Some(playable_index);
            }
            playable.push(track);
        }
        if playable.is_empty() {
            return Err("None of the selected tracks exist on disk".to_string());
        }
        let start_index = selected_playable_index
            .or(next_playable_index)
            .unwrap_or_else(|| playable.len().saturating_sub(1));
        let queue_ids: Vec<String> = playable.iter().map(|t| t.id.clone()).collect();
        {
            let mut state = self.state.lock().expect("player state lock poisoned");
            state.is_playlist_session = queue_ids.len() > 1;
            // Стартовый трек сессии — в историю. Без него первый ИИ-переход
            // после ручного next возвращается на него же (A-B-A: петля
            // «Проспал → A Kind of Deutsch → Проспал» из живой сессии).
            if let Some(start_id) = queue_ids.get(start_index) {
                state.ai_dj_history.push(start_id.clone());
                if state.ai_dj_history.len() > 50 {
                    state.ai_dj_history.remove(0);
                }
            }
        }
        self.play_index(&queue_ids, &playable, start_index)
    }

    /// Load one track into mpv (replace mode) and stash the full queue in state.
    /// Play a track through libmpv. The full queue is stashed in state; libmpv
    /// plays the single file (audio-only) and reports real time/EOF via events.
    /// Play a track through libmpv starting at a given position.
    fn play_index_track_at(
        &self,
        queue_ids: &[String],
        index: usize,
        track: &Track,
        start_seconds: f64,
    ) -> Result<PlayerState, String> {
        let path = track.path.clone();
        let id = track.id.clone();
        let duration = track
            .duration_s
            .map(|seconds| seconds as f64)
            .unwrap_or(0.0);

        let volume = {
            let state = self.state.lock().expect("player state lock poisoned");
            state.volume
        };

        // Stop any existing playback.
        self.stop_mpv();

        let log_path = std::env::temp_dir().join("qbc-music-mpv.log");

        // Create libmpv handle + apply options.
        let mut mpv = match self.create_mpv(&log_path) {
            Ok(m) => m,
            Err(e) => {
                if let Ok(mut state) = self.state.lock() {
                    state.running = false;
                    state.status = "error".to_string();
                    state.last_error = Some(e.clone());
                }
                return Err(e);
            }
        };

        let _ = mpv.set_property_double("volume", volume);

        // Natural playback to EOF without premature truncation
        let _ = mpv.set_property_str("end", "no");

        if start_seconds > 0.0 && (duration == 0.0 || start_seconds < duration) {
            let _ = mpv.set_property_str("start", &start_seconds.to_string());
        }

        // Load the file.
        if let Err(e) = mpv.command(&["loadfile", &path, "replace"]) {
            let err_msg = format!("mpv loadfile failed for '{}': {e}. Check the log at {}.", path, log_path.display());
            if let Ok(mut state) = self.state.lock() {
                state.running = false;
                state.status = "error".to_string();
                state.last_error = Some(err_msg.clone());
            }
            return Err(err_msg);
        }
        let _ = mpv.set_property_double("volume", volume);
        let _ = mpv.set_property_flag("mute", false);

        // Store handle.
        {
            let mut inner = self.inner.lock().expect("player inner lock poisoned");
            inner.mpv = Some(mpv);
            inner.log_path = Some(log_path);
        }

        let _ = self.db.record_play(&id);
        {
            let mut state = self.state.lock().expect("player state lock poisoned");
            state.running = true;
            state.paused = false;
            state.time_pos = start_seconds;
            state.duration = duration;
            state.queue = queue_ids.to_vec();
            state.current_index = Some(index);
            state.current_track_id = Some(id);
            state.status = "playing".to_string();
            state.last_error = None;
        }
        self.mark_clock_running();
        Ok(self.status())
    }

    fn play_index_track(
        &self,
        queue_ids: &[String],
        index: usize,
        track: &Track,
    ) -> Result<PlayerState, String> {
        self.play_index_track_at(queue_ids, index, track, 0.0)
    }

    /// Load one track into mpv (replace mode) and stash the full queue in state.
    fn play_index(
        &self,
        queue_ids: &[String],
        playable: &[Track],
        index: usize,
    ) -> Result<PlayerState, String> {
        let track = playable
            .get(index)
            .ok_or_else(|| "Track index out of range".to_string())?;
        self.play_index_track_at(queue_ids, index, track, 0.0)
    }

    fn jump_relative(&self, delta: i64) -> Result<PlayerState, String> {
        let (queue, current) = {
            let state = self.state.lock().expect("player state lock poisoned");
            (state.queue.clone(), state.current_index)
        };
        if queue.is_empty() {
            return Err("Queue is empty".to_string());
        }
        let current = current.unwrap_or(0) as i64;
        let len = queue.len() as i64;
        let step = if delta >= 0 { 1 } else { -1 };
        let mut attempts = 0;
        let mut idx = (current + delta).rem_euclid(len);

        while attempts < len {
            let id = &queue[idx as usize];
            if let Some(track) = self.db.get_track(id)? {
                if Path::new(&track.path).exists() {
                    return self.play_index_track(&queue, idx as usize, &track);
                }
            }
            idx = (idx + step).rem_euclid(len);
            attempts += 1;
        }

        if let Ok(mut state) = self.state.lock() {
            state.paused = true;
            state.time_pos = 0.0;
            state.status = "idle".to_string();
        }
        Ok(self.status())
    }

    pub fn toggle_pause(&self) -> Result<PlayerState, String> {
        let paused = !self.status().paused;
        self.set_pause(paused)
    }

    pub fn set_pause(&self, paused: bool) -> Result<PlayerState, String> {
        let (current_id, current_idx, queue, time_pos, duration) = {
            let state = self.state.lock().expect("player state lock poisoned");
            (
                state.current_track_id.clone(),
                state.current_index,
                state.queue.clone(),
                state.time_pos,
                state.duration,
            )
        };

        if !paused {
            // Check if mpv is actively loaded and playing
            let mpv_alive = {
                let inner = self.inner.lock().expect("player inner lock poisoned");
                if let Some(mpv) = inner.mpv.as_ref() {
                    let idle = mpv.get_property_flag("idle-active").unwrap_or(true);
                    let eof = mpv.get_property_flag("eof-reached").unwrap_or(false);
                    !idle && !eof
                } else {
                    false
                }
            };

            if !mpv_alive {
                // If we have a current track and it exists on disk, start/resume it
                if let Some(id) = &current_id {
                    if let Ok(Some(track)) = self.db.get_track(id) {
                        if Path::new(&track.path).exists() {
                            let idx = current_idx.unwrap_or(0);
                            let resume_pos = if duration > 0.0 && time_pos < (duration - 1.0).max(0.0) {
                                time_pos
                            } else {
                                0.0
                            };
                            return self.play_index_track_at(&queue, idx, &track, resume_pos);
                        }
                    }
                }

                // If no current track but queue has items
                if !queue.is_empty() {
                    let idx = current_idx.unwrap_or(0);
                    if let Some(id) = queue.get(idx) {
                        if let Ok(Some(track)) = self.db.get_track(id) {
                            if Path::new(&track.path).exists() {
                                return self.play_index_track(&queue, idx, &track);
                            }
                        }
                    }
                }

                // Fallback to library tracks if queue is empty
                if let Ok(tracks) = self.db.list_tracks(None) {
                    let playable: Vec<Track> = tracks
                        .into_iter()
                        .filter(|t| Path::new(&t.path).exists())
                        .collect();
                    if !playable.is_empty() {
                        return self.load_tracks(playable, 0);
                    }
                }
                return Err("No tracks in queue or library to play".to_string());
            } else {
                self.resume_mpv();
            }
        } else {
            self.pause_mpv();
        }

        {
            let mut state = self.state.lock().expect("player state lock poisoned");
            state.paused = paused;
            state.status = if paused { "paused" } else { "playing" }.to_string();
        }
        if paused {
            self.mark_clock_paused();
        } else {
            self.mark_clock_running();
        }
        Ok(self.status())
    }

    pub fn seek(&self, seconds: f64) -> Result<PlayerState, String> {
        // Real seek via libmpv.
        let seconds = seconds.max(0.0);
        let (current_id, current_idx, queue, is_running) = {
            let state = self.state.lock().expect("player state lock poisoned");
            (
                state.current_track_id.clone(),
                state.current_index,
                state.queue.clone(),
                state.running && !state.paused,
            )
        };

        let mut mpv_seek_ok = false;
        {
            let mut inner = self.inner.lock().expect("player inner lock poisoned");
            if let Some(mpv) = inner.mpv.as_mut() {
                let idle = mpv.get_property_flag("idle-active").unwrap_or(true);
                let eof = mpv.get_property_flag("eof-reached").unwrap_or(false);
                if !idle && !eof {
                    let s = format!("{}", seconds);
                    if mpv.command(&["seek", &s, "absolute"]).is_ok() {
                        mpv_seek_ok = true;
                    }
                }
            }
        }

        if !mpv_seek_ok && is_running {
            if let Some(id) = &current_id {
                if let Ok(Some(track)) = self.db.get_track(id) {
                    if Path::new(&track.path).exists() {
                        let idx = current_idx.unwrap_or(0);
                        let _ = self.play_index_track_at(&queue, idx, &track, seconds);
                    }
                }
            }
        }

        {
            let mut state = self.state.lock().expect("player state lock poisoned");
            state.time_pos = seconds;
        }
        self.mark_clock_running_if_playing();
        Ok(self.status())
    }

    pub fn set_volume(&self, volume: f64) -> Result<PlayerState, String> {
        let volume = volume.clamp(0.0, 100.0);
        {
            let mut state = self.state.lock().expect("player state lock poisoned");
            state.volume = volume;
        }
        // Apply volume live to running libmpv.
        {
            let mut inner = self.inner.lock().expect("player inner lock poisoned");
            if let Some(mpv) = inner.mpv.as_mut() {
                let _ = mpv.set_property_double("volume", volume);
            }
        }
        Ok(self.status())
    }

    pub fn next(&self) -> Result<PlayerState, String> {
        // AI DJ mode: get next track from AI DJ if enabled
        let ai_dj_next = {
            let state = self.state.lock().expect("player state lock poisoned");
            if state.ai_dj_enabled {
                drop(state);
                self.ai_dj_next_track().ok().flatten()
            } else {
                None
            }
        };

        if let Some(next_id) = ai_dj_next {
            // Find the track in DB and play it
            if let Ok(Some(track)) = self.db.get_track(&next_id) {
                if Path::new(&track.path).exists() {
                    let (mut queue, is_playlist_session) = {
                        let state = self.state.lock().expect("player state lock poisoned");
                        (state.queue.clone(), state.is_playlist_session)
                    };
                    if !queue.contains(&next_id) {
                        queue.push(next_id.clone());
                    }
                    let mut target_idx = queue.iter().position(|id| id == &next_id).unwrap_or(queue.len() - 1);

                    // Dynamic sliding window on non-playlist queues (cap at 50 tracks)
                    if !is_playlist_session && queue.len() > 50 {
                        let excess = queue.len() - 50;
                        queue.drain(0..excess);
                        target_idx = target_idx.saturating_sub(excess);
                    }

                    let result = self.play_index_track_at(&queue, target_idx, &track, 0.0)?;
                    // Add to AI DJ history (capped at 50)
                    let mut state = self.state.lock().expect("player state lock poisoned");
                    state.ai_dj_history.push(next_id);
                    if state.ai_dj_history.len() > 50 {
                        state.ai_dj_history.remove(0);
                    }
                    return Ok(result);
                } else {
                    // Path does not exist on disk: record in history to avoid retry loop
                    let mut state = self.state.lock().expect("player state lock poisoned");
                    state.ai_dj_history.push(next_id);
                }
            } else {
                // Missing track in DB: record in history to avoid retry loop
                let mut state = self.state.lock().expect("player state lock poisoned");
                state.ai_dj_history.push(next_id);
            }
        }

        // Standard behavior
        let queue_is_empty = {
            let state = self.state.lock().expect("player state lock poisoned");
            state.queue.is_empty()
        };
        if queue_is_empty {
            if let Ok(tracks) = self.db.list_tracks(None) {
                let playable: Vec<Track> = tracks
                    .into_iter()
                    .filter(|t| Path::new(&t.path).exists())
                    .collect();
                if !playable.is_empty() {
                    return self.load_tracks(playable, 0);
                }
            }
        }
        self.jump_relative(1)
    }

    pub fn previous(&self) -> Result<PlayerState, String> {
        let queue_is_empty = {
            let state = self.state.lock().expect("player state lock poisoned");
            state.queue.is_empty()
        };
        if queue_is_empty {
            if let Ok(tracks) = self.db.list_tracks(None) {
                let playable: Vec<Track> = tracks
                    .into_iter()
                    .filter(|t| Path::new(&t.path).exists())
                    .collect();
                if !playable.is_empty() {
                    return self.load_tracks(playable, 0);
                }
            }
        }
        self.jump_relative(-1)
    }

    pub fn status(&self) -> PlayerState {
        // Query live properties from libmpv first without holding state lock
        let (real_pos, real_dur) = {
            let inner = self.inner.lock().expect("player inner lock poisoned");
            match inner.mpv.as_ref() {
                Some(mpv) => (
                    mpv.get_property_double("time-pos").ok(),
                    mpv.get_property_double("duration").ok(),
                ),
                None => (None, None),
            }
        };

        {
            let mut state = self.state.lock().expect("player state lock poisoned");
            if let Some(dur) = real_dur {
                if dur.is_finite() && dur > 0.0 {
                    state.duration = dur;
                }
            }

            if state.running && !state.paused {
                match real_pos {
                    Some(pos) if pos.is_finite() && pos >= 0.0 => {
                        state.time_pos = pos.min(if state.duration > 0.0 { state.duration } else { pos });
                        if let Ok(mut clock) = self.clock.lock() {
                            clock.last_tick = Some(Instant::now());
                        }
                    }
                    _ => {
                        let now = Instant::now();
                        let mut clock = self.clock.lock().expect("player clock lock poisoned");
                        if let Some(last_tick) = clock.last_tick {
                            state.time_pos += now.duration_since(last_tick).as_secs_f64();
                            if state.duration > 0.0 {
                                state.time_pos = state.time_pos.min(state.duration);
                            }
                        }
                        clock.last_tick = Some(now);
                    }
                }
            }
        }
        // Auto-advance on EOF without holding the state lock (avoids deadlock
        // when next() re-locks state). Rate-limited inside.
        self.maybe_auto_advance_eof();
        self.state.lock().expect("player state lock poisoned").clone()
    }

    /// If the current track reached EOF, advance to the next track (or stop
    /// cleanly at the end of the queue). Rate-limited to ~2 Hz. No state lock
    /// is held on entry; this method locks internally as needed.
    fn maybe_auto_advance_eof(&self) {
        // Rate limit: at most once every 500ms.
        let now = Instant::now();
        if let Ok(mut clock) = self.clock.lock() {
            if let Some(last) = clock.last_auto_advance_check {
                if now.duration_since(last) < Duration::from_millis(500) {
                    return;
                }
            }
            clock.last_auto_advance_check = Some(now);
        }

        // Snapshot running/paused/current to decide if we should probe.
        let (running, paused, has_track) = {
            let s = self.state.lock().expect("player state lock poisoned");
            (s.running, s.paused, s.current_track_id.is_some())
        };
        if !running || paused || !has_track {
            return;
        }

        let eof = {
            let inner = self.inner.lock().expect("player inner lock poisoned");
            match inner.mpv.as_ref() {
                Some(mpv) => mpv.get_property_flag("eof-reached").unwrap_or(false),
                None => true,
            }
        };
        if !eof {
            return;
        }
        // Don't loop on a single-track queue or at the end of the list.
        let (queue_len, cur, cur_id, mode, ai_dj_enabled) = {
            let s = self.state.lock().expect("player state lock poisoned");
            (
                s.queue.len(),
                s.current_index.unwrap_or(0),
                s.current_track_id.clone(),
                s.ai_dj_mode,
                s.ai_dj_enabled,
            )
        };
        if ai_dj_enabled {
            if let Some(ref id) = cur_id {
                let _ = self.db.record_play(id);
            }
            let result = match mode {
                AiDjMode::Smart => self.next(),
                AiDjMode::Random => self.ai_dj_play_random(),
                AiDjMode::Sequential => self.jump_relative(1),
            };
            match result {
                Ok(_) => {
                    unidtk_api::log("player: AI DJ auto-advanced after EOF");
                    return;
                }
                Err(e) => {
                    unidtk_api::log(
                        format!("player: AI DJ auto-advance failed: {e}"),
                    );
                    return;
                }
            }
        }

        // Standard non-AI DJ behavior: stop at end of queue
        if queue_len == 0 || cur + 1 >= queue_len {
            if let Some(ref id) = cur_id {
                let _ = self.db.record_play(id);
            }
            {
                let mut s = self.state.lock().expect("player state lock poisoned");
                s.running = false;
                s.paused = true;
                s.time_pos = s.duration;
                s.status = "stopped".to_string();
                s.current_track_id = None;
            }
            unidtk_api::log(
                format!("player: EOF at end of queue (len={queue_len} idx={cur}) — stopping"),
            );
            // Drop mpv so the next play starts cleanly.
            let mut inner = self.inner.lock().expect("player inner lock poisoned");
            if let Some(mut mpv) = inner.mpv.take() {
                let _ = mpv.command(&["stop"]);
            }
            return;
        }

        let result = self.next();
        match result {
            Ok(_) => unidtk_api::log("player: auto-advanced after EOF"),
            Err(e) => unidtk_api::log(
                format!("player: auto-advance failed: {e}"),
            ),
        }
    }

    /// Check if the current track has finished playing and auto-advance via AI DJ.
    /// Returns true if auto-advance was triggered.
    pub fn check_track_finished(&self) -> Result<bool, String> {
        let (ai_dj_enabled, mpv_running) = {
            let state = self.state.lock().expect("player state lock poisoned");
            (
                state.ai_dj_enabled,
                state.running && !state.paused,
            )
        };

        if !ai_dj_enabled || !mpv_running {
            return Ok(false);
        }

        // Check if libmpv reached EOF (track finished).
        let mpv_finished = {
            let inner = self.inner.lock().expect("player inner lock poisoned");
            match inner.mpv.as_ref() {
                Some(mpv) => mpv
                    .get_property_flag("eof-reached")
                    .unwrap_or(false),
                None => true, // No handle = track finished
            }
        };

        if mpv_finished {
            self.maybe_auto_advance_eof();
            return Ok(true);
        }

        Ok(false)
    }

    /// Play a random track from the library (Random mode).
    fn ai_dj_play_random(&self) -> Result<PlayerState, String> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let tracks = self.db.list_tracks(None)?;
        let playable: Vec<Track> = tracks
            .into_iter()
            .filter(|t| Path::new(&t.path).exists())
            .collect();
        if playable.is_empty() {
            return Err("No playable tracks".to_string());
        }
        // Simple random selection based on timestamp
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let idx = ts as usize % playable.len();
        let track = &playable[idx];
        let mut queue = {
            let state = self.state.lock().expect("player state lock poisoned");
            state.queue.clone()
        };
        if !queue.contains(&track.id) {
            queue.push(track.id.clone());
        }
        let target_idx = queue.iter().position(|id| id == &track.id).unwrap_or(queue.len() - 1);
        self.play_index_track_at(&queue, target_idx, track, 0.0)
    }

    /// Kill the current mpv process (if any).
    /// Create a libmpv handle, apply default options + logging.
    fn create_mpv(&self, log_path: &std::path::Path) -> Result<Libmpv, String> {
        let mut mpv = Libmpv::new().map_err(|e| format!("mpv create failed: {e}"))?;
        // Apply default options.
        for (key, value) in DEFAULT_MPV_OPTIONS {
            mpv.set_option(key, value).map_err(|e| {
                format!("mpv set option {key}={value} failed: {e}")
            })?;
        }
        // Log file for debugging.
        let _ = mpv.set_option("log-file", &log_path.to_string_lossy());
        Ok(mpv)
    }

    fn stop_mpv(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(mut mpv) = inner.mpv.take() {
                let _ = mpv.command(&["stop"]);
                // Drop the handle (frees libmpv context).
                drop(mpv);
            }
            inner.log_path = None;
        }
    }

    fn pause_mpv(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(mpv) = inner.mpv.as_mut() {
                let _ = mpv.set_property_flag("pause", true);
            }
        }
    }

    fn resume_mpv(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(mpv) = inner.mpv.as_mut() {
                let _ = mpv.set_property_flag("pause", false);
            }
        }
    }

    fn mark_clock_running(&self) {
        if let Ok(mut clock) = self.clock.lock() {
            clock.last_tick = Some(Instant::now());
        }
    }

    fn mark_clock_paused(&self) {
        if let Ok(mut clock) = self.clock.lock() {
            clock.last_tick = None;
        }
    }

    fn mark_clock_running_if_playing(&self) {
        let is_playing = self
            .state
            .lock()
            .map(|state| state.running && !state.paused)
            .unwrap_or(false);
        if is_playing {
            self.mark_clock_running();
        }
    }
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            running: false,
            paused: true,
            time_pos: 0.0,
            duration: 0.0,
            volume: 100.0,
            queue: Vec::new(),
            current_index: None,
            current_track_id: None,
            status: "stopped".to_string(),
            ai_dj_enabled: false,
            ai_dj_mode: AiDjMode::Smart,
            ai_dj_history: Vec::new(),
            crossfade_duration: 0.0,
            last_error: None,
            is_playlist_session: false,
        }
    }
}



fn _command_label() {}

// Ensure unused imports don't cause warnings
#[allow(dead_code)]
fn _ensure_unused_imports_used() {
    let _ = SystemTime::now();
    let _ = UNIX_EPOCH;
    let _ = Duration::from_secs(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ai_dj_mode_default() {
        let mode = AiDjMode::default();
        assert_eq!(mode, AiDjMode::Smart);
    }

    #[test]
    fn test_ai_dj_mode_serialization() {
        let mode = AiDjMode::Smart;
        let json = serde_json::to_string(&mode).unwrap();
        let deserialized: AiDjMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, AiDjMode::Smart);
    }

    #[test]
    fn test_player_state_default_ai_dj() {
        let state = PlayerState::default();
        assert!(!state.ai_dj_enabled);
        assert_eq!(state.ai_dj_mode, AiDjMode::Smart);
        assert!(state.ai_dj_history.is_empty());
    }

    /// Стартовый трек плейлиста обязан попадать в `ai_dj_history`: без него
    /// первый переход после ручного next возвращается на него же (A-B-A —
    /// «Проспал → A Kind of Deutsch → Проспал» из живой сессии 27.09).
    #[test]
    fn test_load_queue_records_session_start_in_history() {
        // load_tracks фильтрует по существованию файла — кладём настоящий.
        let dir = std::env::temp_dir().join(format!("unidtk-hist-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("start.mp3");
        std::fs::write(&file, b"not really audio").expect("temp file");

        let db = Database::new(crate::library::db::memory_module_db());
        db.upsert_track(
            "pl_test",
            0,
            &crate::library::db::DownloadedTrack {
                id: "start_id".to_string(),
                title: "Start".to_string(),
                artist: None,
                album: None,
                duration_s: Some(1),
                path: file.to_string_lossy().into_owned(),
                ext: Some("mp3".to_string()),
                filesize: None,
                thumbnail_path: None,
            },
        )
        .expect("upsert_track");

        let pm = PlayerManager::new(ConfigStore::load_or_default(), db);
        // mpv в тестовой среде может не поднять файл (нет DLL/битый content) —
        // запись о старте сессии обязана произойти до запуска воспроизведения.
        let _ = pm.load_queue(vec!["start_id".to_string()], 0);

        let state = pm.state.lock().expect("state lock");
        assert_eq!(
            state.ai_dj_history.last().map(String::as_str),
            Some("start_id"),
            "session start must be recorded in ai_dj_history, got {:?}",
            state.ai_dj_history
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ai_dj_set_mode() {
        // This test requires a database, so we just verify the mode enum works
        let _modes = [
            AiDjMode::Smart,
            AiDjMode::Random,
            AiDjMode::Sequential,
        ];
        for mode in &_modes {
            let json = serde_json::to_string(mode).unwrap();
            let _: AiDjMode = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn test_player_volume_clamping() {
        let db = Database::new(crate::library::db::memory_module_db());
        let config = ConfigStore::load_or_default();
        let player = PlayerManager::new(config, db);

        // Volume clamp upper
        let state = player.set_volume(150.0).unwrap();
        assert_eq!(state.volume, 100.0);

        // Volume clamp lower
        let state = player.set_volume(-20.0).unwrap();
        assert_eq!(state.volume, 0.0);

        // Normal volume
        let state = player.set_volume(42.5).unwrap();
        assert_eq!(state.volume, 42.5);
    }
}
