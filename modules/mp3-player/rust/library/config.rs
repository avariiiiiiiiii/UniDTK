use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub library_dir: String,
    pub yt_dlp_path: String,
    pub cookies_browser: String,
    pub sync_parallelism: usize,
    pub mpv_path: String,
    /// When true, existing playlists are re-synced against their YouTube/YTM
    /// source URLs in the background (new tracks download, removed ones drop).
    #[serde(default = "default_auto_sync_enabled")]
    pub auto_sync_enabled: bool,
    /// Hours between automatic full-library playlist refreshes.
    #[serde(default = "default_auto_sync_interval_hours")]
    pub auto_sync_interval_hours: f64,
    /// AI DJ submodule: smart transition sequencing
    #[serde(default = "default_ai_dj_enabled")]
    pub ai_dj_enabled: bool,
    /// AI DJ submodule: crossfade between tracks, seconds (0 disables it).
    #[serde(default)]
    pub ai_dj_crossfade_seconds: f64,
    /// yt-dlp submodule: throttled background self-update (~20 h).
    #[serde(default = "default_auto_update_enabled")]
    pub auto_update_enabled: bool,
    /// Visualiser submodule: WASAPI loopback capture for the spectrum arcs.
    #[serde(default = "default_visualizer_enabled")]
    pub visualizer_enabled: bool,
}

fn default_ai_dj_enabled() -> bool {
    false
}

fn default_auto_update_enabled() -> bool {
    true
}

fn default_visualizer_enabled() -> bool {
    true
}

fn default_auto_sync_enabled() -> bool {
    true
}

fn default_auto_sync_interval_hours() -> f64 {
    6.0
}

#[derive(Clone)]
pub struct ConfigStore {
    path: Arc<PathBuf>,
    inner: Arc<Mutex<AppConfig>>,
}

impl ConfigStore {
    /// Build a store from the on-disk config (falls back to defaults).
    /// Panic-free: bad/missing config degrades to `AppConfig::default()`.
    #[cfg(test)]
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| {
            let path = app_config_dir().join("config.toml");
            Self {
                path: Arc::new(path),
                inner: Arc::new(Mutex::new(AppConfig::default())),
            }
        })
    }
    pub fn load() -> Result<Self, String> {
        let dir = app_config_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;

        let path = dir.join("config.toml");
        // Config migration from Roaming: the config lived there before the switch to the data directory.
        // Without this, `library_dir` would be lost and the library would appear empty.
        if !path.exists() {
            let legacy = legacy_app_config_dir().join("config.toml");
            if legacy.is_file() && fs::copy(&legacy, &path).is_ok() {
                unidtk_api::log(format!(
                    "mp3-player: config moved from {} to {}",
                    legacy.display(),
                    path.display()
                ));
            }
        }
        let default = AppConfig::default();
        let config = if path.exists() {
            parse_config(
                &fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read config {}: {e}", path.display()))?,
                default.clone(),
            )
        } else {
            default
        };

        let store = Self {
            path: Arc::new(path),
            inner: Arc::new(Mutex::new(config)),
        };
        store.save()?;
        Ok(store)
    }

    pub fn get(&self) -> AppConfig {
        self.inner.lock().expect("config lock poisoned").clone()
    }

    pub fn path(&self) -> PathBuf {
        (*self.path).clone()
    }

    pub fn update(&self, patch: AppConfig) -> Result<(), String> {
        self.mutate(move |config| *config = patch)?;
        Ok(())
    }

    /// Targeted change under the lock: both the mutation and the write to disk happen under one
    /// lock hold, so a concurrent `save_config` from the UI cannot overwrite the edit or leave
    /// on disk a state that was never in memory.
    pub fn mutate<F>(&self, apply: F) -> Result<AppConfig, String>
    where
        F: FnOnce(&mut AppConfig),
    {
        // The lock is held through the write: two mutations cannot reach the
        // disk in the opposite order of the memory.
        let mut config = self.inner.lock().expect("config lock poisoned");
        apply(&mut config);
        *config = config.clone().normalized();
        let snapshot = config.clone();
        self.write_to_disk(&snapshot)?;
        Ok(snapshot)
    }

    pub fn save(&self) -> Result<(), String> {
        let guard = self.inner.lock().expect("config lock poisoned");
        let config = guard.clone().normalized();
        self.write_to_disk(&config)
    }

    fn write_to_disk(&self, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir {}: {e}", parent.display()))?;
        }
        fs::write(&*self.path, format_config(config))
            .map_err(|e| format!("Failed to write config {}: {e}", self.path.display()))
    }

    pub fn archive_path(&self) -> PathBuf {
        PathBuf::from(self.get().library_dir).join(".archive.txt")
    }

    pub fn resolve_yt_dlp(&self) -> PathBuf {
        resolve_binary(&self.get().yt_dlp_path, "yt-dlp.exe")
    }
}

impl AppConfig {
    fn normalized(mut self) -> Self {
        if self.library_dir.trim().is_empty() {
            self.library_dir = default_library_dir().to_string_lossy().to_string();
        }
        if self.yt_dlp_path.trim().is_empty() {
            self.yt_dlp_path = "yt-dlp.exe".to_string();
        }
        if self.cookies_browser.trim().is_empty() {
            self.cookies_browser = "chrome".to_string();
        }
        if self.sync_parallelism == 0 {
            self.sync_parallelism = 3;
        }
        if self.mpv_path.trim().is_empty() {
            self.mpv_path = "mpv.exe".to_string();
        }
        if self.auto_sync_interval_hours <= 0.0 {
            self.auto_sync_interval_hours = default_auto_sync_interval_hours();
        }
        // Floor at 15 minutes so a bad config can't hammer yt-dlp every second.
        if self.auto_sync_interval_hours < 0.25 {
            self.auto_sync_interval_hours = 0.25;
        }
        // AI DJ crossfade: this is the only owner of the range (0–10 s); the submodule
        // registry declares the same bounds.
        self.ai_dj_crossfade_seconds = self.ai_dj_crossfade_seconds.clamp(0.0, 10.0);
        self
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            library_dir: default_library_dir().to_string_lossy().to_string(),
            yt_dlp_path: "yt-dlp.exe".to_string(),
            cookies_browser: "chrome".to_string(),
            sync_parallelism: 3,
            mpv_path: "mpv.exe".to_string(),
            auto_sync_enabled: default_auto_sync_enabled(),
            auto_sync_interval_hours: default_auto_sync_interval_hours(),
            ai_dj_enabled: default_ai_dj_enabled(),
            ai_dj_crossfade_seconds: 0.0,
            auto_update_enabled: default_auto_update_enabled(),
            visualizer_enabled: default_visualizer_enabled(),
        }
    }
}

fn parse_config(input: &str, mut config: AppConfig) -> AppConfig {
    for raw in input.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = clean_value(value.trim());
        match key {
            "library_dir" => config.library_dir = value,
            "yt_dlp_path" => config.yt_dlp_path = value,
            "cookies_browser" => config.cookies_browser = value,
            "sync_parallelism" => {
                if let Ok(n) = value.parse::<usize>() {
                    config.sync_parallelism = n;
                }
            }
            "mpv_path" => config.mpv_path = value,
            "auto_sync_enabled" => {
                config.auto_sync_enabled = matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "auto_sync_interval_hours" => {
                if let Ok(n) = value.parse::<f64>() {
                    config.auto_sync_interval_hours = n;
                }
            }
            "ai_dj_enabled" => {
                config.ai_dj_enabled = matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "ai_dj_crossfade_seconds" => {
                if let Ok(n) = value.parse::<f64>() {
                    config.ai_dj_crossfade_seconds = n;
                }
            }
            "auto_update_enabled" => {
                config.auto_update_enabled = matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "visualizer_enabled" => {
                config.visualizer_enabled = matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            _ => {}
        }
    }
    config.normalized()
}

fn clean_value(value: &str) -> String {
    let value = value.trim();
    // A quoted value ends at its closing quote: a '#' inside it is part of the
    // value (a folder named "C#Pop"), and only text after the quote is a comment.
    for quote in ['\'', '"'] {
        if let Some(rest) = value.strip_prefix(quote) {
            if let Some(end) = rest.find(quote) {
                return rest[..end].to_string();
            }
        }
    }
    value.split('#').next().unwrap_or(value).trim().to_string()
}

fn format_config(config: &AppConfig) -> String {
    format!(
        "# qbc music configuration\n\
         # Default library: %USERPROFILE%\\Music\\qbc-music\n\
         library_dir = '{}'\n\
         yt_dlp_path = '{}'\n\
         cookies_browser = '{}'\n\
         sync_parallelism = {}\n\
         mpv_path = '{}'\n\
         # Background refresh of local playlists against their YouTube/YTM sources\n\
         auto_sync_enabled = {}\n\
         auto_sync_interval_hours = {}\n\
         # AI DJ submodule\n\
         ai_dj_enabled = {}\n\
         ai_dj_crossfade_seconds = {}\n\
         # yt-dlp self-update submodule\n\
         auto_update_enabled = {}\n\
         # Spectrum visualiser (WASAPI loopback) submodule\n\
         visualizer_enabled = {}\n",
        toml_literal(&config.library_dir),
        toml_literal(&config.yt_dlp_path),
        toml_literal(&config.cookies_browser),
        config.sync_parallelism,
        toml_literal(&config.mpv_path),
        if config.auto_sync_enabled {
            "true"
        } else {
            "false"
        },
        config.auto_sync_interval_hours,
        if config.ai_dj_enabled { "true" } else { "false" },
        config.ai_dj_crossfade_seconds,
        if config.auto_update_enabled {
            "true"
        } else {
            "false"
        },
        if config.visualizer_enabled {
            "true"
        } else {
            "false"
        },
    )
}

fn toml_literal(value: &str) -> String {
    value.replace('\'', "''")
}

/// The config directory is the application data directory, not Roaming.
///
/// It contains absolute paths and `cookies_browser`, so this config is machine-specific and
/// must not travel with the user's roaming profile.
pub fn app_config_dir() -> PathBuf {
    unidtk_api::paths::data_dir()
}

/// The default library lives under the data directory. It can be overridden by
/// `library_dir` in the config, because music is user content.
fn default_library_dir() -> PathBuf {
    unidtk_api::paths::music_dir()
}

/// The former config directory in Roaming (`%APPDATA%\qbc-music`) — used only for migration.
fn legacy_app_config_dir() -> PathBuf {
    match env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata).join("qbc-music"),
        None => home_dir()
            .join("AppData")
            .join("Roaming")
            .join("qbc-music"),
    }
}

fn home_dir() -> PathBuf {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| {
            let drive = env::var_os("HOMEDRIVE")?;
            let path = env::var_os("HOMEPATH")?;
            Some(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn resolve_binary(configured: &str, fallback: &str) -> PathBuf {
    let configured = configured.trim();
    let configured_path = Path::new(configured);
    if configured_path.is_absolute() || configured_path.components().count() > 1 {
        return configured_path.to_path_buf();
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            let next_to_exe = parent.join(configured);
            if next_to_exe.exists() {
                return next_to_exe;
            }
            let bin_dir = parent.join("bin").join(configured);
            if bin_dir.exists() {
                return bin_dir;
            }
        }
    }

    for candidate in known_binary_locations(if configured.is_empty() {
        fallback
    } else {
        configured
    }) {
        if candidate.exists() {
            return candidate;
        }
    }

    if configured.is_empty() {
        PathBuf::from(fallback)
    } else {
        PathBuf::from(configured)
    }
}

fn known_binary_locations(name: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(program_files) = env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(program_files));
    }
    if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(program_files_x86));
    }
    if let Some(local_appdata) = env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local_appdata).join("Programs"));
    }

    let app_dirs = match name.to_ascii_lowercase().as_str() {
        "mpv.exe" | "mpv.com" => ["MPV Player", "mpv", "mpv.net"].as_slice(),
        "yt-dlp.exe" => ["yt-dlp"].as_slice(),
        _ => [].as_slice(),
    };

    let mut candidates = Vec::new();
    for root in roots {
        candidates.push(root.join(name));
        for app_dir in app_dirs {
            candidates.push(root.join(app_dir).join(name));
        }
    }
    candidates
}

#[cfg(test)]
mod clean_value_tests {
    use super::clean_value;

    #[test]
    fn hash_inside_a_quoted_value_is_kept() {
        assert_eq!(clean_value("'C:\\Music\\C#Pop'"), "C:\\Music\\C#Pop");
        assert_eq!(clean_value("\"a#b\""), "a#b");
    }

    #[test]
    fn a_comment_after_the_value_is_dropped() {
        assert_eq!(clean_value("'x' # my library"), "x");
        assert_eq!(clean_value("5 # comment"), "5");
        assert_eq!(clean_value("true"), "true");
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn concurrent_mutations_leave_the_disk_equal_to_memory() {
        let dir = std::env::temp_dir().join(format!("mp3_config_race_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let store = ConfigStore {
            path: Arc::new(path.clone()),
            inner: Arc::new(Mutex::new(AppConfig::default())),
        };

        let workers: Vec<_> = (0..8usize)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for i in 0..60usize {
                        let value = (t * 60 + i) % 9 + 1;
                        store.mutate(|config| config.sync_parallelism = value).unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        let on_disk = parse_config(&fs::read_to_string(&path).unwrap(), AppConfig::default());
        assert_eq!(on_disk.sync_parallelism, store.get().sync_parallelism);
        let _ = fs::remove_dir_all(&dir);
    }
}
