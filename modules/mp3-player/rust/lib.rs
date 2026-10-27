pub mod ai;
pub mod audio;
pub mod commands;
pub mod ffi;
mod json_util;
pub mod library;
pub mod playback;
pub mod sync;

// ── Socket surface (`unidtk_api::commands::ModuleIpcFn`) ───────────────────────
// The kernel registers this under the module id, so its router never names
// `AppState` itself.

fn state_from<'a>(
    host: Option<unidtk_api::commands::HostHandle<'a>>,
) -> Result<tauri::State<'a, commands::AppState>, String> {
    let app = host
        .and_then(|h| h.downcast_ref::<tauri::AppHandle>())
        .ok_or_else(|| "host handle unavailable".to_string())?;
    tauri::Manager::try_state::<commands::AppState>(app)
        .ok_or_else(|| "music core not ready yet".to_string())
}

fn json_of<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

pub fn ipc_call(
    cmd: &str,
    args: &serde_json::Value,
    host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        "player_status" => {
            Some(state_from(host).and_then(|state| json_of(commands::player_status(&state))))
        }
        "ai_dj_status" => {
            Some(state_from(host).and_then(|state| json_of(commands::ai_dj_status(&state))))
        }
        "ai_dj_model_download" => {
            // Команда write, ходит только через дверь интерфейса; на сокет
            // её не ставим (это arm вызывается только упомянутыми путями).
            // Состояние не нужно: загрузчик одиночный и сам находит модель.
            Some(commands::ai_dj_model_download())
        }
        "get_submodules" => {
            Some(state_from(host).and_then(|state| json_of(commands::get_submodules(&state))))
        }
        "list_tracks" => Some(state_from(host).and_then(|state| {
            // Same arguments and the same shape as the Tauri command: it returns the
            // array itself. The socket version used to ignore search/sort and answer
            // `{count, items}`, which would have silently broken library search the
            // moment this command moved to the new door.
            let search = args
                .get("search")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let sort_key = args
                .get("sort_key")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let sort_dir = args
                .get("sort_dir")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let tracks = tauri::async_runtime::block_on(commands::list_tracks(
                &state, search, sort_key, sort_dir,
            ))?;
            json_of(tracks)
        })),
        "list_playlists" => Some(state_from(host).and_then(|state| {
            let playlists = tauri::async_runtime::block_on(commands::list_playlists(&state))?;
            json_of(playlists)
        })),
        _ => None,
    }
}

pub use commands::{create_module, Mp3PlayerModule, MP3_PLAYER_DEPENDENCIES};
