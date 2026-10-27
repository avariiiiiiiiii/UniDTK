pub mod archive;
pub mod clipboard;
pub mod commands;
pub mod drives;
pub mod files;
pub mod fs;
pub mod hashing;
pub mod portable;
pub mod preview;
pub mod registry;
pub mod search;
pub mod sorting;
pub mod window;

// ── Socket surface (`unidtk_api::commands::ModuleIpcFn`) ───────────────────────
// The kernel registers this under the module id, so its router never names this
// crate's types.

fn json_of<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

pub fn ipc_call(
    cmd: &str,
    _args: &serde_json::Value,
    _host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        "get_drives" => {
            Some(tauri::async_runtime::block_on(crate::commands::get_drives()).and_then(json_of))
        }
        "get_user_places" => Some(crate::commands::get_user_places().and_then(json_of)),
        _ => None,
    }
}

pub use commands::create_module;
