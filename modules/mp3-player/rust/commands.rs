//! Shared Tauri command surface for every shell that embeds the music core
//! (neuro-music desktop app, app-tauri wallpaper app, future shells).
//!
//! All hot-path commands are `async` and wrap blocking work in
//! `tauri::async_runtime::spawn_blocking` so the IPC thread is never held.
//! Each binary's `main.rs` just builds the state and lists these names in
//! `tauri::generate_handler!`.

use std::sync::{Arc, Mutex};

use crate::{
    library::config::{AppConfig, ConfigStore},
    library::db::{Database, Playlist, Track},
    playback::player::{PlayerManager, PlayerState},
    sync::sync::{
        spawn_auto_playlist_sync, spawn_auto_update, update_yt_dlp, SyncManager, SyncTask,
    },
};

#[derive(Clone)]
pub struct AppState {
    pub config: ConfigStore,
    pub db: Database,
    pub player: PlayerManager,
    pub sync: SyncManager,
    /// Захват спектра (визуализатор): подмодуль владеет им сам, ядро его больше
    /// не запускает и не останавливает.
    audio: Arc<Mutex<Option<crate::audio::AudioState>>>,
}

/// mpv IPC-пайп оболочки-обоев: имя принадлежит модулю, ядро его не знает.
const MPV_PIPE: &str = r"\\.\pipe\mpv-wallpaper";

impl AppState {
    /// Build the full state graph (config → library → player/sync/spectrum) from
    /// the context the kernel hands over. The library lives in the module's own
    /// key slice of `unidtk.redb`; the raw engine never reaches this module.
    pub fn bootstrap(ctx: unidtk_api::ModuleContext) -> Result<Self, String> {
        // События модуль публикует через контракт (`unidtk_api::emit`), поэтому
        // собственный `AppHandle` ему больше не нужен вовсе.
        let config = ConfigStore::load().map_err(|e| e.to_string())?;
        // AI DJ держит эмбеддинги в отдельном файле: своя очередь записи (redb —
        // один писатель на файл), своя надёжность коммита и возможность снести
        // файл целиком. Путь выбирает ядро — см. `core/kernel/module_store.rs`.
        let ai_dj_store = ctx.store(
            unidtk_api::StoreSpec::cache("ai_dj")
                .legacy(&["unidtk_ai_dj.redb"])
                // Прежняя схема AI DJ — две таблицы; контракт даёт плоские ключи.
                .legacy_tables(&[("embeddings", "emb:"), ("analyses", "ana:")]),
        )?;
        crate::ai::ai_dj::set_store(ai_dj_store);
        let db = Database::new(ctx.into_db());
        // Обложки из `<id>.jpg` рядом с треками: вызов при старте библиотеки
        // был потерян (docs/architecture.md §18.1 → §45.5).
        if let Err(error) = db.repair_missing_thumbnails() {
            unidtk_api::log(format!("mp3-player: thumbnail repair failed: {error}"));
        }
        // Наблюдаемость: одна строка с реальным объёмом библиотеки — по ней
        // сессия сопровождения сверяет миграцию на живой базе.
        let tracks = db.list_tracks(None).map(|tracks| tracks.len()).unwrap_or(0);
        let playlists = db
            .list_playlists()
            .map(|playlists| playlists.len())
            .unwrap_or(0);
        unidtk_api::log(format!(
            "mp3-player: library ready ({tracks} tracks / {playlists} playlists)"
        ));
        let player =
            PlayerManager::with_pipe_name(config.clone(), db.clone(), MPV_PIPE.to_string());
        // Кроссфейд — настройка подмодуля AI DJ: применяем сохранённое значение.
        let crossfade = config.get().ai_dj_crossfade_seconds;
        if let Err(error) = player.set_crossfade_duration(crossfade) {
            unidtk_api::log(format!("mp3-player: crossfade apply failed: {error}"));
        }
        let sync = SyncManager::new(db.clone(), config.clone());
        if config.get().auto_update_enabled {
            tauri::async_runtime::spawn(spawn_auto_update(config.clone()));
        }
        let state = Self {
            config,
            db,
            player,
            sync,
            audio: Arc::new(Mutex::new(None)),
        };
        let visualizer = state.config.get().visualizer_enabled;
        if let Err(error) = state.set_visualizer_enabled(visualizer) {
            unidtk_api::log(format!("mp3-player: visualizer start failed: {error}"));
        }
        Ok(state)
    }

    /// Фоновые задачи модуля (авто-синхронизация, авто-обновление): запускаются
    /// из `Module::init`, ядро в бутстрап модуля не лезет.
    pub fn start_background_jobs(&self) {
        let sync = self.sync.clone();
        let config = self.config.clone();
        tauri::async_runtime::spawn(spawn_auto_playlist_sync(sync, config));
    }

    /// Живое включение/выключение захвата спектра. Внешний лок держится на всей
    /// операции check→start→store: иначе бутстрап и тумблер могли бы поднять два
    /// захвата (два клиента WASAPI loopback).
    fn set_visualizer_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut guard = self
            .audio
            .lock()
            .map_err(|_| "visualizer lock poisoned".to_string())?;
        if IS_SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        match (enabled, guard.is_some()) {
            (true, false) => *guard = Some(crate::audio::start()),
            (false, true) => {
                if let Some(audio) = guard.as_ref() {
                    audio.request_stop();
                }
                *guard = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn visualizer_running(&self) -> bool {
        self.audio
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Остановка без join'ов: латч анализа, флаг захвата спектра, пауза
    /// воспроизведения best effort. Выход не ждёт тяжёлые потоки.
    pub fn begin_shutdown(&self) {
        begin_shutdown();
        if let Ok(guard) = self.audio.lock() {
            if let Some(audio) = guard.as_ref() {
                audio.request_stop();
            }
        }
        let _ = self.player.set_pause(true);
    }
}

/// Модуль плеера: `init` строит состояние и фоновые задачи, `shutdown` глушит
/// анализ, захват спектра и воспроизведение. Оригинал состояния живёт здесь
/// (жизненный цикл), копия — в managed-state ядра (обёртки команд).
pub struct Mp3PlayerModule {
    state: Mutex<Option<AppState>>,
}

impl Mp3PlayerModule {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }

    pub fn state(&self) -> Option<AppState> {
        self.state.lock().expect("mp3-player module lock").clone()
    }
}

pub static MP3_PLAYER_DEPENDENCIES: &[unidtk_api::manifest::ModuleDependency] = &[
    unidtk_api::manifest::ModuleDependency {
        id: "libmpv",
        description: "libmpv media playback engine DLL",
        severity: unidtk_api::manifest::DependencySeverity::Mandatory,
        kind: unidtk_api::manifest::DependencyKind::DynamicLibrary {
            file_name: "libmpv-2.dll",
            companions: &[],
            min_bytes: 10_000_000,
        },
    },
    unidtk_api::manifest::ModuleDependency {
        id: "ai-dj-model",
        description: "MAEST Discogs ONNX model weights for AI DJ analysis",
        severity: unidtk_api::manifest::DependencySeverity::Optional,
        kind: unidtk_api::manifest::DependencyKind::ModelAsset {
            relative_path: "discogs-maest-129e/model.onnx",
            min_bytes: 100_000,
            companion_files: &[("discogs-maest-129e/model.onnx.data", 10_000_000)],
        },
    },
];

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(Mp3PlayerModule::new())
}

impl unidtk_api::Module for Mp3PlayerModule {
    fn id(&self) -> &'static str {
        "mp3-player"
    }

    fn dependencies(&self) -> &'static [unidtk_api::manifest::ModuleDependency] {
        MP3_PLAYER_DEPENDENCIES
    }

    fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        if ctx.is_diagnostic() {
            let db = Database::new(ctx.into_db());
            let tracks = db
                .list_tracks(None)
                .map_err(|e| format!("mp3-player in-memory db test failed: {e}"))?;
            unidtk_api::log(format!(
                "mp3-player: diagnostic init OK (in-memory store functional, {} tracks)",
                tracks.len()
            ));
            return Ok(());
        }
        let state = AppState::bootstrap(ctx)?;
        state.start_background_jobs();
        *self.state.lock().expect("mp3-player module lock") = Some(state);
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        let mut guard = self.state.lock().map_err(|e| e.to_string())?;
        if let Some(state) = guard.take() {
            state.begin_shutdown();
        }
        Ok(())
    }
}

// --- Config -----------------------------------------------------------------

pub fn get_config(state: &AppState) -> AppConfig {
    state.config.get()
}

pub fn get_config_path(state: &AppState) -> String {
    state.config.path().to_string_lossy().to_string()
}

pub fn save_config(
    state: &AppState,
    config: AppConfig,
) -> Result<AppConfig, String> {
    state.config.update(config)?;
    Ok(state.config.get())
}

// --- Submodules (настройки для будущего меню) --------------------------------

fn toggle_setting(
    key: &str,
    title: &str,
    value: bool,
    default: bool,
    applies: &str,
) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "title": title,
        "kind": "toggle",
        "value": value,
        "default": default,
        "applies": applies,
    })
}

/// Реестр подмодулей с типизированными настройками и статусом — его рендерит
/// меню настроек (мост). Форма стабильна: у настройки `key`, `title`, `kind`
/// (`toggle|number|select`), `value`, `default`, `applies`
/// (`live|next_cycle|restart`) и, где нужно, `min`/`max`/`step`/`unit`/`options`.
pub fn get_submodules(state: &AppState) -> serde_json::Value {
    let config = state.config.get();
    let ai_dj = ai_dj_status(state);
    let sync_running = state
        .sync
        .tasks()
        .iter()
        .any(|task| task.status != "done" && task.status != "error");
    let last_sync = state
        .db
        .list_playlists()
        .unwrap_or_default()
        .iter()
        .filter_map(|playlist| playlist.last_sync)
        .max();

    serde_json::json!({
        "submodules": [
            {
                "id": "ai_dj",
                "title": "AI DJ",
                "description": "Анализ библиотеки, умная очередь и переходы",
                "settings": [
                    toggle_setting("enabled", "Включён", ai_dj.enabled, false, "live"),
                    {
                        "key": "crossfade_seconds",
                        "title": "Кроссфейд",
                        "kind": "number",
                        "value": config.ai_dj_crossfade_seconds,
                        "default": 0.0,
                        "min": 0.0,
                        "max": 10.0,
                        "step": 0.5,
                        "unit": "с",
                        "applies": "live"
                    }
                ],
                "status": {
                    "running": ai_dj.is_analyzing,
                    "detail": format!(
                        "Проанализировано {} из {}",
                        ai_dj.analyzed_count, ai_dj.track_count
                    )
                }
            },
            {
                "id": "auto_sync",
                "title": "Авто-синхронизация",
                "description": "Сверка локальных плейлистов с их YouTube/YTM источниками",
                "settings": [
                    toggle_setting("enabled", "Включена", config.auto_sync_enabled, true, "next_cycle"),
                    {
                        "key": "interval_hours",
                        "title": "Интервал",
                        "kind": "number",
                        "value": config.auto_sync_interval_hours,
                        "default": 6.0,
                        "min": 0.25,
                        "max": 168.0,
                        "step": 0.25,
                        "unit": "ч",
                        "applies": "next_cycle"
                    }
                ],
                "status": {
                    "running": sync_running,
                    "detail": last_sync
                        .map(|ts| format!("Последняя синхронизация: {ts}"))
                        .unwrap_or_else(|| "Ещё не запускалась".to_string())
                }
            },
            {
                "id": "auto_update",
                "title": "Авто-обновление yt-dlp",
                "description": "Тихая проверка свежего yt-dlp не чаще раза в ~20 часов",
                "settings": [
                    toggle_setting("enabled", "Включено", config.auto_update_enabled, true, "live")
                ],
                "status": {
                    "running": false,
                    "detail": crate::sync::sync::ytdlp_last_update(&state.config)
                        .map(|ts| format!("Последняя проверка: {ts}"))
                        .unwrap_or_else(|| "Ещё не проверялось".to_string())
                }
            },
            {
                "id": "visualizer",
                "title": "Визуализатор",
                "description": "Захват системного звука (WASAPI loopback) для дуг и полос",
                "settings": [
                    toggle_setting("enabled", "Включён", config.visualizer_enabled, true, "live")
                ],
                "status": {
                    "running": state.visualizer_running(),
                    "detail": serde_json::Value::Null
                }
            }
        ]
    })
}

/// Точечное изменение настройки подмодуля. Возвращает обновлённое значение —
/// уже нормализованное (`AppConfig::normalized` — единственный владелец
/// диапазонов), чтобы меню не показывало то, чего нет в конфиге.
pub async fn set_submodule_setting(
    state: &AppState,
    module: String,
    key: String,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let state = state.clone();
    tauri::async_runtime::spawn_blocking(move || apply_submodule_setting(&state, &module, &key, value))
        .await
        .map_err(|e| e.to_string())?
}

fn apply_submodule_setting(
    state: &AppState,
    module: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let as_bool = |value: &serde_json::Value| {
        value
            .as_bool()
            .ok_or_else(|| format!("{module}.{key}: ожидалось true/false"))
    };
    let as_f64 = |value: &serde_json::Value| {
        value
            .as_f64()
            .ok_or_else(|| format!("{module}.{key}: ожидалось число"))
    };

    let updated = match (module, key) {
        ("ai_dj", "enabled") => {
            let enabled = as_bool(&value)?;
            if enabled {
                state.player.enable_ai_dj()?;
            } else {
                state.player.disable_ai_dj()?;
            }
            let snapshot = state
                .config
                .mutate(|config| config.ai_dj_enabled = enabled)?;
            serde_json::json!(snapshot.ai_dj_enabled)
        }
        ("ai_dj", "crossfade_seconds") => {
            let seconds = as_f64(&value)?;
            let snapshot = state
                .config
                .mutate(|config| config.ai_dj_crossfade_seconds = seconds)?;
            let normalized = snapshot.ai_dj_crossfade_seconds;
            state.player.set_crossfade_duration(normalized)?;
            serde_json::json!(normalized)
        }
        ("auto_sync", "enabled") => {
            let enabled = as_bool(&value)?;
            let snapshot = state
                .config
                .mutate(|config| config.auto_sync_enabled = enabled)?;
            serde_json::json!(snapshot.auto_sync_enabled)
        }
        ("auto_sync", "interval_hours") => {
            let hours = as_f64(&value)?;
            let snapshot = state
                .config
                .mutate(|config| config.auto_sync_interval_hours = hours)?;
            serde_json::json!(snapshot.auto_sync_interval_hours)
        }
        ("auto_update", "enabled") => {
            let enabled = as_bool(&value)?;
            let snapshot = state
                .config
                .mutate(|config| config.auto_update_enabled = enabled)?;
            // Включение проверяем сразу; маркер не даст частить чаще ~20 ч.
            if enabled {
                tauri::async_runtime::spawn(spawn_auto_update(state.config.clone()));
            }
            serde_json::json!(snapshot.auto_update_enabled)
        }
        ("visualizer", "enabled") => {
            let enabled = as_bool(&value)?;
            let snapshot = state
                .config
                .mutate(|config| config.visualizer_enabled = enabled)?;
            state.set_visualizer_enabled(snapshot.visualizer_enabled)?;
            serde_json::json!(snapshot.visualizer_enabled)
        }
        _ => return Err(format!("unknown submodule setting: {module}.{key}")),
    };

    Ok(serde_json::json!({ "module": module, "key": key, "value": updated }))
}

// --- Tracks / playlists -----------------------------------------------------

pub async fn list_tracks(
    state: &AppState,
    search: Option<String>,
    sort_key: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<Track>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        db.list_tracks_sorted(search, sort_key.as_deref(), sort_dir.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn get_track(
    state: &AppState,
    track_id: String,
) -> Result<Option<Track>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.get_track(&track_id))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn list_playlists(state: &AppState) -> Result<Vec<Playlist>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.list_playlists())
        .await
        .map_err(|e| e.to_string())?
}

/// Локальное переименование плейлиста: имя хранится только в библиотеке и
/// переживает синхронизацию (sync не перезатирает не-URL имя именем-URL).
pub async fn rename_playlist(
    state: &AppState,
    playlist_id: String,
    name: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Пустое имя плейлиста".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        db.set_playlist_metadata(&playlist_id, Some(&name), None)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Удаление плейлиста: связки и запись, файлы остаются на диске.
pub async fn delete_playlist(state: &AppState, playlist_id: String) -> Result<(), String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.delete_playlist(&playlist_id))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn list_playlist_tracks(
    state: &AppState,
    playlist_id: String,
    sort_key: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<Track>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        db.list_playlist_tracks_sorted(&playlist_id, sort_key.as_deref(), sort_dir.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

// --- Sync -------------------------------------------------------------------

pub async fn sync_url(
    state: &AppState,
    url: String,
    name: Option<String>,
) -> Result<String, String> {
    let sync = state.sync.clone();
    tauri::async_runtime::spawn_blocking(move || sync.start_url(url, name))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn sync_playlist(
    state: &AppState,
    playlist_id: String,
) -> Result<String, String> {
    let sync = state.sync.clone();
    tauri::async_runtime::spawn_blocking(move || sync.start_existing_playlist(playlist_id))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn sync_all(state: &AppState) -> Result<Vec<String>, String> {
    let sync = state.sync.clone();
    tauri::async_runtime::spawn_blocking(move || sync.start_all())
        .await
        .map_err(|e| e.to_string())?
}

pub async fn sync_tasks(state: &AppState) -> Result<Vec<SyncTask>, String> {
    let sync = state.sync.clone();
    tauri::async_runtime::spawn_blocking(move || sync.tasks())
        .await
        .map_err(|e| e.to_string())
}

pub async fn update_ytdlp(state: &AppState) -> Result<String, String> {
    update_yt_dlp(state.config.clone()).await
}

// --- Player -----------------------------------------------------------------

pub async fn player_load_queue(
    state: &AppState,
    track_ids: Vec<String>,
    start_index: usize,
) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.load_queue(track_ids, start_index))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn player_load_view_queue(
    state: &AppState,
    playlist_id: Option<String>,
    search: Option<String>,
    sort_key: Option<String>,
    sort_dir: Option<String>,
    start_index: usize,
) -> Result<PlayerState, String> {
    let db = state.db.clone();
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let tracks = match playlist_id {
            Some(id) => {
                db.list_playlist_tracks_sorted(&id, sort_key.as_deref(), sort_dir.as_deref())?
            }
            None => db.list_tracks_sorted(search, sort_key.as_deref(), sort_dir.as_deref())?,
        };
        player.load_tracks(tracks, start_index)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn player_toggle_pause(state: &AppState) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.toggle_pause())
        .await
        .map_err(|e| e.to_string())?
}

pub async fn player_set_pause(
    state: &AppState,
    paused: bool,
) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.set_pause(paused))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn player_seek(
    state: &AppState,
    seconds: f64,
) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.seek(seconds))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn player_set_volume(
    state: &AppState,
    volume: f64,
) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.set_volume(volume))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn player_next(state: &AppState) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.next())
        .await
        .map_err(|e| e.to_string())?
}

pub async fn player_previous(state: &AppState) -> Result<PlayerState, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.previous())
        .await
        .map_err(|e| e.to_string())?
}

pub fn player_status(state: &AppState) -> PlayerState {
    state.player.status()
}

// --- AI DJ -----------------------------------------------------------------

pub async fn toggle_ai_dj(
    state: &AppState,
    enabled: bool,
) -> Result<bool, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if enabled {
            player.enable_ai_dj()?;
        } else {
            player.disable_ai_dj()?;
        }
        Ok(enabled)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct AiDjStatus {
    pub enabled: bool,
    /// Number of tracks in the library (from the tracks DB).
    pub track_count: usize,
    /// Number of tracks whose physical audio file exists on disk.
    pub reachable_count: usize,
    /// Number of tracks that have a stored AI-DJ embedding (analyzed).
    pub analyzed_count: usize,
    pub mode: String,
    pub is_analyzing: bool,
    pub current_analyzing_track: Option<String>,
    /// Модель AI DJ найдена и валидна (поставка или докачка) — предикат лоадера.
    pub model_present: bool,
    /// докачка модели идёт прямо сейчас (фаза `"running"`).
    pub model_downloading: bool,
    /// Скачанные байты текущей цепочки: готовые файлы + текущий `.part`.
    pub model_download_bytes_done: u64,
    /// Полный объём цепочки, когда сервер дал размеры.
    pub model_download_bytes_total: Option<u64>,
}

#[derive(Default)]
pub struct AnalysisProgress {
    pub is_analyzing: bool,
    pub current: usize,
    pub total: usize,
    pub current_track: Option<String>,
}

static ANALYSIS_PROGRESS: std::sync::RwLock<AnalysisProgress> = std::sync::RwLock::new(AnalysisProgress {
    is_analyzing: false,
    current: 0,
    total: 0,
    current_track: None,
});

pub fn ai_dj_status(state: &AppState) -> AiDjStatus {
    let all_tracks = state.db.list_tracks(None).unwrap_or_default();
    let library_count = all_tracks.len();
    let reachable_count = all_tracks.iter().filter(|t| std::path::Path::new(&t.path).exists()).count();
    let (is_analyzing, current_analyzing_track) = {
        let prog = ANALYSIS_PROGRESS.read().unwrap();
        (prog.is_analyzing, prog.current_track.clone())
    };

    let analyzed_count = {
        let ai_dj = state.player.ai_dj.lock();
        if let Ok(guard) = ai_dj {
            match guard.as_ref() {
                Some(dj) => dj.track_count(),
                // Движок не поднят — считаем по хранилищу, не открывая файл вручную.
                None => crate::ai::ai_dj::analyzed_count(),
            }
        } else {
            0
        }
    };

    let (enabled, mode) = {
        let ai_dj = state.player.ai_dj.lock();
        let enabled = ai_dj.as_ref().map(|g| g.is_some()).unwrap_or(false);
        let mode = match state.player.ai_dj_get_mode() {
            crate::playback::player::AiDjMode::Smart => "smart",
            crate::playback::player::AiDjMode::Random => "random",
            crate::playback::player::AiDjMode::Sequential => "sequential",
        }.to_string();
        (enabled, mode)
    };

    let (model_present, model_downloading, model_download_bytes_done, model_download_bytes_total) = {
        let snap = crate::ai::model_fetch::snapshot();
        (
            crate::ai::model_fetch::is_model_present(),
            snap.phase == "running",
            snap.bytes_done,
            snap.bytes_total,
        )
    };

    AiDjStatus {
        enabled,
        track_count: library_count,
        reachable_count,
        analyzed_count,
        mode,
        is_analyzing,
        current_analyzing_track,
        model_present,
        model_downloading,
        model_download_bytes_done,
        model_download_bytes_total,
    }
}

/// Докачка AI DJ модели (`discogs-maest-129e`) в каталог данных — по явному
/// действию пользователя в виджете.
///
/// Ответ немедленный: `{"result": "started" | "running" | "already_present"}`
/// плюс срез состояния докачки; сам ход идёт в фоне и публикует прогресс
/// событием `ai-dj-model`. Ошибка фазы (`phase:"error"`) в этот ответ не
/// возвращается — она живёт в событии, статусе и в `Snapshot.error`.
pub fn ai_dj_model_download() -> Result<serde_json::Value, String> {
    crate::ai::model_fetch::start_download()
}

pub fn get_crossfade_duration(state: &AppState) -> f64 {
    state.player.get_crossfade_duration().unwrap_or(0.0)
}

pub fn set_crossfade_duration(
    state: &AppState,
    seconds: f64,
) -> Result<f64, String> {
    state.player.set_crossfade_duration(seconds)?;
    state.player.get_crossfade_duration()
}

pub async fn ai_dj_undo(state: &AppState) -> Result<Option<PlayerState>, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.ai_dj_undo())
        .await
        .map_err(|e| e.to_string())?
}

pub async fn ai_dj_set_mode(
    state: &AppState,
    mode: String,
) -> Result<(), String> {
    let player = state.player.clone();
    let mode = match mode.as_str() {
        "smart" => crate::playback::player::AiDjMode::Smart,
        "random" => crate::playback::player::AiDjMode::Random,
        "sequential" => crate::playback::player::AiDjMode::Sequential,
        _ => return Err(format!("Unknown AI DJ mode: {mode}")),
    };
    tauri::async_runtime::spawn_blocking(move || player.ai_dj_set_mode(mode))
        .await
        .map_err(|e| e.to_string())?
}

pub fn ai_dj_get_mode(state: &AppState) -> String {
    let mode = state.player.ai_dj_get_mode();
    match mode {
        crate::playback::player::AiDjMode::Smart => "smart",
        crate::playback::player::AiDjMode::Random => "random",
        crate::playback::player::AiDjMode::Sequential => "sequential",
    }
    .to_string()
}

pub async fn ai_dj_build_queue(
    state: &AppState,
    track_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ai_dj = player.ai_dj.lock().map_err(|e| e.to_string())?;
        let ai_dj = ai_dj.as_ref().ok_or("AI DJ not enabled")?;
        if ai_dj.track_count() == 0 {
            return Ok(track_ids);
        }
        Ok(ai_dj.build_playlist_queue(&track_ids))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn check_track_finished(state: &AppState) -> Result<bool, String> {
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || player.check_track_finished())
        .await
        .map_err(|e| e.to_string())?
}

static IS_ANALYZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static IS_SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Латч выхода: останавливает цикл анализа, не дожидаясь его (никаких join'ов —
/// выход должен укладываться в бюджет завершения Windows).
pub fn begin_shutdown() {
    IS_SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
}

struct AnalysisGuard;
impl Drop for AnalysisGuard {
    fn drop(&mut self) {
        IS_ANALYZING.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut p) = ANALYSIS_PROGRESS.write() {
            p.is_analyzing = false;
            p.current_track = None;
        }
        crate::ai::ai_dj::unload_session();
    }
}

fn run_analysis_loop(
    player: &crate::playback::player::PlayerManager,
    tracks: &[crate::library::db::Track],
) -> Result<usize, String> {

    let total = tracks.len();
    let mut analyzed = 0;
    let mut last_emit = std::time::Instant::now();

    for (idx, track) in tracks.iter().enumerate() {
        if IS_SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        let already_has = {
            let ai_dj_lock = player.ai_dj.lock().map_err(|e| e.to_string())?;
            ai_dj_lock.as_ref().map(|dj| dj.has_track(&track.id)).unwrap_or(false)
        };
        if already_has {
            analyzed += 1;
            continue;
        }

        let path = std::path::Path::new(&track.path);
        if !path.exists() {
            continue;
        }

        let track_name = track.title.clone();

        {
            if let Ok(mut p) = ANALYSIS_PROGRESS.write() {
                p.current = idx + 1;
                p.total = total;
                p.current_track = Some(track_name.clone());
            }
        }

        if last_emit.elapsed() >= std::time::Duration::from_millis(150) {
            unidtk_api::emit("ai-dj-progress", serde_json::json!({
                "is_analyzing": true,
                "current": idx + 1,
                "total": total,
                "track": track_name,
            }));
            last_emit = std::time::Instant::now();
        }

        match crate::ai::ai_dj::extract_analysis(path) {
            Ok((embedding, bpm, key)) => {
                let mut ai_dj_lock = player.ai_dj.lock().map_err(|e| e.to_string())?;
                if let Some(dj) = ai_dj_lock.as_mut() {
                    let duration_ms = track.duration_s.map(|s| (s * 1000) as u64);
                    let analyzed_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let analysis = crate::ai::ai_dj::TrackAnalysis {
                        track_id: track.id.clone(),
                        bpm,
                        key,
                        duration_ms,
                        analyzed_at,
                    };
                    if dj.store_embedding(&track.id, &embedding, &analysis).is_ok() {
                        analyzed += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to analyze {}: {e}", track.id);
            }
        }
    }

    unidtk_api::emit("ai-dj-progress", serde_json::json!({
        "is_analyzing": false,
        "current": total,
        "total": total,
        "track": null,
    }));

    crate::ai::ai_dj::unload_session();

    Ok(analyzed)
}

pub async fn analyze_tracks(state: &AppState) -> Result<usize, String> {
    let db = state.db.clone();
    let tracks = db.list_tracks(None)?;
    if tracks.is_empty() {
        return Ok(0);
    }

    // Модель обязательна для анализа; при её отсутствии — читаемая ошибка
    // вместо подземного отказа ONNX. Докачку здесь не затеваем: она только по
    // явному действию пользователя.
    if !crate::ai::model_fetch::is_model_present() {
        return Err(
            "AI DJ: модель анализа не установлена — скачайте её кнопкой «Скачать модель» в виджете"
                .to_string(),
        );
    }

    if IS_ANALYZING
        .compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst)
        .is_err()
    {
        return Err("AI DJ: анализ аудиотеки уже выполняется в фоновом режиме".to_string());
    }

    if let Ok(mut p) = ANALYSIS_PROGRESS.write() {
        p.is_analyzing = true;
        p.current = 0;
        p.total = tracks.len();
        p.current_track = Some("Инициализация...".to_string());
    }

    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = AnalysisGuard;

        // Ensure AiDj is initialized and session is ready under short lock
        {
            let mut ai_dj_lock = player.ai_dj.lock().map_err(|e| e.to_string())?;
            if ai_dj_lock.is_none() {
                let mut new_dj =
                    crate::ai::ai_dj::AiDj::new(crate::ai::ai_dj::ai_dj_store()?);
                let _ = new_dj.load();
                *ai_dj_lock = Some(new_dj);
            }
            let ai_dj = ai_dj_lock.as_mut().ok_or("AI DJ not initialized")?;
            ai_dj.ensure_session_ready()?;
        }

        run_analysis_loop(&player, &tracks)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn reanalyze_tracks(state: &AppState) -> Result<usize, String> {
    let db = state.db.clone();
    let tracks = db.list_tracks(None)?;
    if tracks.is_empty() {
        return Ok(0);
    }

    // Как и при первом анализе: модель должна быть на месте, автодокачки
    // здесь нет — запрос повторного анализа без модели отвечает понятной
    // ошибкой, а не запускает 331 МБ в фоне.
    if !crate::ai::model_fetch::is_model_present() {
        return Err(
            "AI DJ: модель анализа не установлена — скачайте её кнопкой «Скачать модель» в виджете"
                .to_string(),
        );
    }

    if IS_ANALYZING
        .compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst)
        .is_err()
    {
        return Err("AI DJ: анализ аудиотеки уже выполняется в фоновом режиме".to_string());
    }

    if let Ok(mut p) = ANALYSIS_PROGRESS.write() {
        p.is_analyzing = true;
        p.current = 0;
        p.total = tracks.len();
        p.current_track = Some("Сброс кэша...".to_string());
    }

    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = AnalysisGuard;

        // Clear existing embeddings and ensure session ready under short lock
        {
            let mut ai_dj_lock = player.ai_dj.lock().map_err(|e| e.to_string())?;
            if let Some(ref mut dj) = *ai_dj_lock {
                dj.clear();
            } else {
                let mut new_dj =
                    crate::ai::ai_dj::AiDj::new(crate::ai::ai_dj::ai_dj_store()?);
                let _ = new_dj.load();
                *ai_dj_lock = Some(new_dj);
            }
            let ai_dj = ai_dj_lock.as_mut().ok_or("AI DJ not initialized")?;
            ai_dj.ensure_session_ready()?;
        }

        run_analysis_loop(&player, &tracks)
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn ai_dj_history(
    state: &AppState,
) -> Vec<serde_json::Value> {
    let ai_dj = state.player.ai_dj.lock();
    match ai_dj {
        Ok(guard) => match guard.as_ref() {
            Some(dj) => dj.get_history(),
            None => vec![],
        },
        Err(_) => vec![],
    }
}

pub async fn ai_dj_graph(
    state: &AppState,
) -> Result<crate::ai::ai_dj::SimilarityGraph, String> {
    let db = state.db.clone();
    let player = state.player.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // list_tracks — полное bincode-чтение библиотеки: держим его внутри
        // spawn_blocking, чтобы не блокировать async-поток.
        let tracks = db.list_tracks(None).unwrap_or_default();
        let ai_dj_lock = player.ai_dj.lock().map_err(|e| e.to_string())?;
        if let Some(dj) = ai_dj_lock.as_ref() {
            return Ok(dj.build_similarity_graph(&tracks, 3));
        }
        drop(ai_dj_lock);

        let ai_dj_store = crate::ai::ai_dj::ai_dj_store()?;
        // Кэш проверяем до загрузки эмбеддингов: попап открывается часто,
        // а анализ меняется редко.
        if let Some(graph) = crate::ai::ai_dj::AiDj::cached_graph(&ai_dj_store, &tracks) {
            return Ok(graph);
        }
        let mut dj = crate::ai::ai_dj::AiDj::new(ai_dj_store);
        let _ = dj.load();
        Ok(dj.build_similarity_graph(&tracks, 3))
    })
    .await
    .map_err(|e| e.to_string())?
}
