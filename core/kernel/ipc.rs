//! Lightweight localhost IPC server for agent/CLI debugging (architecture §5.3).
//!
//! Listens on 127.0.0.1:26540 and speaks a tiny JSON protocol:
//!   request:  {"cmd": "ping" | "status" | "player_status" | "vpn_status" | ...,
//!              "args": {...}}
//!   response: {"ok": true, "result": ...} | {"ok": false, "error": "..."}
//!
//! Trust model: the socket is loopback-only and **unauthenticated** — any local
//! process can talk to it. The module surface is therefore read-only by design:
//! snapshots and lists, never writes. Write-capable commands (`vpn_connect`,
//! `telegram_send_message`, player control, `refresh_servers` with a
//! caller-supplied source) stay out — they would let any local process redirect
//! traffic, message as the user, or hammer the network on demand.
//!
//! Module state is resolved lazily through the Tauri `AppHandle` (`try_state`),
//! so a module that failed to initialise or is still booting reports "not ready"
//! instead of panicking. Each connection runs on its own OS thread (never on the
//! event loop), which is also why async module commands can be driven with
//! `tauri::async_runtime::block_on`.

use crate::log_store::LogStore;
use crate::unidtk_db::UniDtkDatabase;
use unidtk_mod_explorer as explorer;
use unidtk_mod_glyph_bar as glyph_bar;
use unidtk_mod_mp3_player as music;
use unidtk_mod_micrographics as micrographics;
use unidtk_mod_telegram as telegram;
use unidtk_mod_vpn as vpn;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

pub const IPC_PORT: u16 = 26540;

/// Сколько запросов обслуживаем одновременно. Каждое соединение — отдельный
/// поток, поэтому лишние получают `busy` вместо бесконтрольного роста.
const MAX_INFLIGHT: usize = 8;
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);

struct InflightGuard;

impl InflightGuard {
    fn acquire() -> Option<Self> {
        let current = INFLIGHT.fetch_add(1, Ordering::SeqCst);
        if current >= MAX_INFLIGHT {
            INFLIGHT.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self)
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct IpcServer {
    db: Arc<UniDtkDatabase>,
    logs: Option<Arc<LogStore>>,
    app: tauri::AppHandle,
}

impl IpcServer {
    pub fn new(db: Arc<UniDtkDatabase>, logs: Option<Arc<LogStore>>, app: tauri::AppHandle) -> Self {
        Self { db, logs, app }
    }

    pub fn start(self) {
        thread::Builder::new()
            .name("unidtk-ipc".into())
            .spawn(move || {
                let listener = match TcpListener::bind(("127.0.0.1", IPC_PORT)) {
                    Ok(l) => l,
                    Err(e) => {
                        crate::log_line(format!("ipc: bind failed: {e}"));
                        return;
                    }
                };
                crate::log_line(format!("ipc: listening on 127.0.0.1:{IPC_PORT}"));
                for stream in listener.incoming() {
                    match stream {
                        Ok(s) => {
                            let server = self.clone();
                            thread::spawn(move || server.handle_conn(s));
                        }
                        Err(_) => continue,
                    }
                }
            })
            .expect("spawn ipc thread");
    }

    fn handle_conn(&self, stream: TcpStream) {
        let Some(_inflight) = InflightGuard::acquire() else {
            let mut out = stream;
            let _ = writeln!(out, "{}", json_err("busy: too many ipc requests in flight"));
            let _ = out.flush();
            return;
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(1500)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(1500)));
        let mut reader = BufReader::new(stream.try_clone().unwrap_or_else(|_| stream));
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let response = self.route(&line);
        let mut out = reader.get_mut().try_clone().unwrap();
        let _ = write!(out, "{response}\n");
        let _ = out.flush();
    }

    /// Dispatches one request with no transport involved.
    ///
    /// Both doors go through here, so a command *behaves* identically whichever way it
    /// arrived — but which commands may arrive at all depends on the door, so the caller
    /// states it. Passing one policy for every door (the socket's read-only rule, applied
    /// to both) left the authenticated WebSocket unable to serve a state change, which is
    /// exactly what the UI door exists to do.
    pub fn dispatch(
        &self,
        req: &serde_json::Value,
        door: unidtk_api::commands::Door,
    ) -> Result<serde_json::Value, String> {
        let cmd = req["cmd"].as_str().unwrap_or("");
        let args = req["args"].as_object().cloned().unwrap_or_default();

        // The inventory is authoritative for what exists and which door serves it:
        // a write command is refused here by policy, not by being forgotten.
        check_door(cmd, door)?;

        match ROUTES.iter().find(|(name, _)| *name == cmd) {
            Some((_, handler)) => handler(self, &args),
            // The inventory above already confirmed that this command exists and that
            // this door serves it, so a missing route is a build inconsistency — and
            // the route tests below fail loudly if it ever happens.
            None => Err(format!(
                "command '{cmd}' is declared in the inventory but has no handler"
            )),
        }
    }

    fn route(&self, line: &str) -> String {
        let req: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => return respond(None, Err(format!("bad json: {e}"))),
        };
        // The envelope is optional on purpose: an older bridge sending only
        // `{cmd, args}` keeps working. When an id is present it is echoed back.
        respond(
            req.get("id").cloned(),
            self.dispatch(&req, unidtk_api::commands::Door::Socket),
        )
    }

}

fn json_value<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Applies the inventory's door policy to one command name.
///
/// A free function on purpose: the policy is pure data, and keeping it out of `dispatch`
/// is what makes it testable at all — an `IpcServer` needs a live `AppHandle` to exist,
/// while this needs nothing.
fn check_door(cmd: &str, door: unidtk_api::commands::Door) -> Result<(), String> {
    match crate::command_inventory::find(cmd) {
        None => Err(format!("unknown cmd: {cmd}")),
        Some(spec) if !spec.doors.contains(&door) => {
            // Say *why*, not just *no*: the refusals mean different things to a caller —
            // "this changes state", "this is the tool door's own surface", or "this
            // command is simply not yours to ask for".
            let other = if spec.doors.contains(&unidtk_api::commands::Door::Socket) {
                "it is served by the tool door only"
            } else {
                "it is served by the UI door only"
            };
            let why = if spec.is_write() {
                "it changes state, and write commands stay on the UI door"
            } else {
                other
            };
            Err(format!(
                "command '{cmd}' is not available on the {} door: {why}",
                door.name()
            ))
        }
        Some(_) => Ok(()),
    }
}

/// Wire version. Bumped when the envelope changes shape; clients compare it with
/// their own and refuse to work against an unknown one instead of guessing.
pub const PROTOCOL: u64 = 1;

/// Builds one reply. The `id` is echoed when the request carried it, so a client can
/// tell which response belongs to which request — the old one-shot shape could not.
pub(crate) fn respond(
    id: Option<serde_json::Value>,
    outcome: Result<serde_json::Value, String>,
) -> String {
    let mut body = match outcome {
        Ok(value) => serde_json::json!({ "ok": true, "result": value }),
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    };
    body["v"] = serde_json::json!(PROTOCOL);
    if let Some(id) = id {
        body["id"] = id;
    }
    body.to_string()
}

fn json_err(msg: &str) -> String {
    serde_json::json!({"ok": false, "error": msg, "v": PROTOCOL}).to_string()
}

// ── Route table ────────────────────────────────────────────────────────────────

/// Arguments of one socket request.
type Args = serde_json::Map<String, serde_json::Value>;

/// A socket handler. Handlers used to be arms of a `match`; they are functions now,
/// so the route table can reference them and a test can prove none is missing.
type Handler = fn(&IpcServer, &Args) -> Result<serde_json::Value, String>;

/// The route table. Every name comes from an inventory **constant**, so a command
/// name is written exactly once in the codebase and the inventory cannot drift away
/// from the router.
static ROUTES: &[(&str, Handler)] = &[
    (crate::command_inventory::PING.name, handle_ping),
    (crate::command_inventory::LOGS.name, handle_logs),
    (crate::command_inventory::LOG_STATS.name, handle_log_stats),
    (crate::command_inventory::LOG_MAX_ID.name, handle_log_max_id),
    (crate::command_inventory::LOG_DELTA.name, handle_log_delta),
    (
        crate::command_inventory::GET_WIDGET_CONFIG.name,
        handle_get_widget_config,
    ),
    (
        crate::command_inventory::GET_DISABLED_MODULES.name,
        handle_get_disabled_modules,
    ),
    (
        crate::command_inventory::SET_DISABLED_MODULES.name,
        handle_set_disabled_modules,
    ),
    // Served on the UI door only, and still routed: the dispatcher is what the WebSocket
    // transport uses, so a command without a route here is unreachable on that door even
    // though its Tauri twin exists.
    (
        crate::command_inventory::FRONTEND_LOG.name,
        handle_frontend_log,
    ),
    (crate::command_inventory::COMMANDS.name, handle_commands),
    (
        crate::command_inventory::PLAYER_STATUS.name,
        handle_player_status,
    ),
    (
        crate::command_inventory::AI_DJ_STATUS.name,
        handle_ai_dj_status,
    ),
    (crate::command_inventory::LIST_TRACKS.name, handle_list_tracks),
    (
        crate::command_inventory::LIST_PLAYLISTS.name,
        handle_list_playlists,
    ),
    (
        crate::command_inventory::GET_SUBMODULES.name,
        handle_get_submodules,
    ),
    (
        crate::command_inventory::GET_SYSTEM_VOLUME.name,
        handle_get_system_volume,
    ),
    // The UI door's first *module* writes: routed like the reads, refused on the socket
    // by policy (they are declared `write`), so the same command serves both transports
    // with one handler.
    (
        crate::command_inventory::SET_SYSTEM_VOLUME.name,
        handle_set_system_volume,
    ),
    (
        crate::command_inventory::SET_SYSTEM_MUTE.name,
        handle_set_system_mute,
    ),
    (
        crate::command_inventory::AI_DJ_MODEL_DOWNLOAD.name,
        handle_ai_dj_model_download,
    ),
    (crate::command_inventory::VPN_STATUS.name, handle_vpn_status),
    (
        crate::command_inventory::VPN_SETTINGS.name,
        handle_vpn_settings,
    ),
    (
        crate::command_inventory::VPN_LIST_SERVERS.name,
        handle_vpn_list_servers,
    ),
    (crate::command_inventory::VPN_CONNECT.name, handle_vpn_connect),
    (
        crate::command_inventory::VPN_DISCONNECT.name,
        handle_vpn_disconnect,
    ),
    (crate::command_inventory::VPN_PING_ALL.name, handle_vpn_ping_all),
    (
        crate::command_inventory::VPN_SET_AUTO_CONNECT.name,
        handle_vpn_set_auto_connect,
    ),
    (
        crate::command_inventory::TELEGRAM_STATUS.name,
        handle_telegram_status,
    ),
    (
        crate::command_inventory::TELEGRAM_CHATS.name,
        handle_telegram_chats,
    ),
    (
        crate::command_inventory::TELEGRAM_CHATS_ARCHIVE.name,
        handle_telegram_chats_archive,
    ),
    (
        crate::command_inventory::TELEGRAM_SET_ARCHIVED.name,
        handle_telegram_set_archived,
    ),
    (
        crate::command_inventory::TELEGRAM_MESSAGES_AROUND.name,
        handle_telegram_messages_around,
    ),
    (
        crate::command_inventory::EXPLORER_DRIVES.name,
        handle_explorer_drives,
    ),
    (
        crate::command_inventory::EXPLORER_PLACES.name,
        handle_explorer_places,
    ),
    (
        crate::command_inventory::EXPLORER_SORT_GET.name,
        handle_explorer_sort_get,
    ),
    (
        crate::command_inventory::EXPLORER_SORT_SET.name,
        handle_explorer_sort_set,
    ),
    (
        crate::command_inventory::WEATHER_CURRENT.name,
        handle_weather_current,
    ),
    (
        crate::command_inventory::WEATHER_LOCATION_REQUEST.name,
        handle_weather_location_request,
    ),
];

fn handle_ping(_server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "pong": true,
        // What this build speaks and can do. A client compares `proto` with its own
        // and refuses to guess when they differ (an installed bridge and the app are
        // replaced independently).
        "proto": PROTOCOL,
        "capabilities": ["envelope-v1", "log-delta", "door-policy", "gui", "ws-door"],
        // The bridge discovers the CDP port here instead of assuming 9222: a stale
        // instance may hold the conventional port.
        "cdpPort": crate::cdp_port(),
        // The page's own door (S4). The port is public knowledge; the token that
        // guards it is deliberately NOT reported here — this reply travels over the
        // unauthenticated tool door.
        "wsPort": crate::ws::port(),
    }))
}

fn handle_logs(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let level = args.get("level").and_then(|v| v.as_str());
    let source = args.get("source").and_then(|v| v.as_str());
    let logs = server
        .logs
        .as_ref()
        .ok_or_else(|| "log store unavailable".to_string())?;
    let entries = match args.get("since").and_then(|v| v.as_u64()) {
        Some(since) => logs.get_since(since, Some(limit))?,
        None => logs.list(limit, level, source)?,
    };
    json_value(entries)
}

fn handle_log_stats(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    server
        .logs
        .as_ref()
        .ok_or_else(|| "log store unavailable".to_string())?
        .stats()
}

fn handle_log_max_id(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    let logs = server
        .logs
        .as_ref()
        .ok_or_else(|| "log store unavailable".to_string())?;
    Ok(serde_json::json!({ "maxId": logs.max_id()? }))
}

/// One round-trip instead of two: the caller passes its baseline and gets both the
/// new high-water mark and the entries after it. `udtk gui` uses this per step.
fn handle_log_delta(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let since = args.get("since").and_then(|v| v.as_u64()).unwrap_or(0);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(200) as usize;
    let logs = server
        .logs
        .as_ref()
        .ok_or_else(|| "log store unavailable".to_string())?;
    let entries = logs.get_since(since, Some(limit))?;
    let max_id = logs.max_id().unwrap_or(since);
    Ok(serde_json::json!({ "maxId": max_id, "entries": entries }))
}

fn handle_get_widget_config(
    server: &IpcServer,
    _args: &Args,
) -> Result<serde_json::Value, String> {
    // Must match the Tauri command exactly: that one returns `Option<Value>`, i.e. JSON
    // `null` when nothing is stored. Answering `{}` here made the two doors disagree,
    // which is why this command could not be migrated until now.
    match server.db.get_widget_config() {
        Ok(Some(config)) => Ok(config),
        Ok(None) => Ok(serde_json::Value::Null),
        Err(e) => Err(e),
    }
}

fn handle_get_disabled_modules(
    server: &IpcServer,
    _args: &Args,
) -> Result<serde_json::Value, String> {
    let list = server.db.get_disabled_modules()?;
    Ok(serde_json::json!(list))
}

fn handle_set_disabled_modules(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    let ids: Vec<String> = args
        .get("ids")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    server.db.set_disabled_modules(&ids)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// Describes the surface of this door from the inventory itself, so the reply cannot
/// disagree with what is actually served.
fn handle_commands(_server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "socket": crate::command_inventory::socket_commands()
            .iter()
            .map(|spec| serde_json::json!({
                "name": spec.name,
                "args": spec.args,
                "summary": spec.summary,
                "access": match spec.access {
                    unidtk_api::commands::Access::ReadOnly => "read_only",
                    unidtk_api::commands::Access::Write => "write",
                },
                "retry_safe": spec.is_retry_safe(),
            }))
            .collect::<Vec<_>>(),
        "note": "The socket door is read-only by policy: write commands are declared in the inventory (with access=write) but are never served here.",
    }))
}

fn handle_player_status(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "mp3-player",
        crate::command_inventory::PLAYER_STATUS.name,
        args,
    )
}

fn handle_ai_dj_status(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "mp3-player",
        crate::command_inventory::AI_DJ_STATUS.name,
        args,
    )
}

fn handle_list_tracks(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "mp3-player",
        crate::command_inventory::LIST_TRACKS.name,
        args,
    )
}

fn handle_list_playlists(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "mp3-player",
        crate::command_inventory::LIST_PLAYLISTS.name,
        args,
    )
}

fn handle_get_submodules(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "mp3-player",
        crate::command_inventory::GET_SUBMODULES.name,
        args,
    )
}

fn handle_get_system_volume(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "glyph-bar",
        crate::command_inventory::GET_SYSTEM_VOLUME.name,
        args,
    )
}

fn handle_set_system_volume(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "glyph-bar",
        crate::command_inventory::SET_SYSTEM_VOLUME.name,
        args,
    )
}

fn handle_set_system_mute(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "glyph-bar",
        crate::command_inventory::SET_SYSTEM_MUTE.name,
        args,
    )
}

/// The mp3-player module's UI-door write: starts the AI DJ model download.
/// The module answers immediately (`started` / `running` / `already_present`);
/// the download itself runs in its own thread and publishes progress through
/// the `ai-dj-model` event, so this handler never blocks on the network.
fn handle_ai_dj_model_download(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "mp3-player",
        crate::command_inventory::AI_DJ_MODEL_DOWNLOAD.name,
        args,
    )
}

/// The UI door's write command: it needs a route here like any other, because a command
/// that arrives over the WebSocket is dispatched here — the Tauri command of the same
/// name is a *different* transport, not a substitute. Without this route the door
/// answered "declared in the inventory but has no handler", which the live probe caught
/// while every test was green: the tests only required routes for the *socket* subset.
fn handle_frontend_log(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let message = args
        .get("message")
        .and_then(|value| value.as_str())
        .ok_or("frontend_log requires {message}")?;
    crate::logging::log_frontend(message);
    Ok(serde_json::Value::Null)
}

/// Module→handler registry: the kernel knows a module by **id**, never by its state
/// type. A module that needs application state downcasts the opaque host itself.
///
/// So extending the socket surface means: implement `ipc_call` in the module crate,
/// declare the command in the inventory, add one line here. This router no longer
/// names any module's state type — the test below enforces that.
type ModuleRoute = (&'static str, unidtk_api::commands::ModuleIpcFn);

static MODULE_ROUTES: &[ModuleRoute] = &[
    ("glyph-bar", glyph_bar::ipc_call),
    ("explorer", explorer::ipc_call),
    ("vpn", vpn::ipc_call),
    ("telegram", telegram::ipc_call),
    ("mp3-player", music::ipc_call),
    ("micrographics", micrographics::ipc_call),
];

fn call_module(
    server: &IpcServer,
    module_id: &str,
    cmd: &str,
    args: &Args,
) -> Result<serde_json::Value, String> {
    let handler = MODULE_ROUTES
        .iter()
        .find(|(id, _)| *id == module_id)
        .map(|(_, handler)| *handler)
        .ok_or_else(|| format!("module '{module_id}' does not serve the socket door"))?;
    let host = &server.app as &(dyn std::any::Any + Send + Sync);
    match handler(cmd, &serde_json::Value::Object(args.clone()), Some(host)) {
        Some(result) => result,
        None => Err(format!(
            "module '{module_id}' is registered but does not handle '{cmd}'"
        )),
    }
}

fn handle_vpn_status(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(server, "vpn", crate::command_inventory::VPN_STATUS.name, args)
}

fn handle_vpn_settings(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "vpn",
        crate::command_inventory::VPN_SETTINGS.name,
        args,
    )
}

fn handle_vpn_list_servers(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "vpn",
        crate::command_inventory::VPN_LIST_SERVERS.name,
        args,
    )
}

/// The VPN writes. Routed exactly like the reads: the kernel still does not know the
/// module's state type, it addresses `vpn` by id, and the door policy decides who may
/// call these at all.
fn handle_vpn_connect(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "vpn",
        crate::command_inventory::VPN_CONNECT.name,
        args,
    )
}

fn handle_vpn_disconnect(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "vpn",
        crate::command_inventory::VPN_DISCONNECT.name,
        args,
    )
}

fn handle_vpn_ping_all(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "vpn",
        crate::command_inventory::VPN_PING_ALL.name,
        args,
    )
}

fn handle_vpn_set_auto_connect(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "vpn",
        crate::command_inventory::VPN_SET_AUTO_CONNECT.name,
        args,
    )
}

fn handle_telegram_status(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "telegram",
        crate::command_inventory::TELEGRAM_STATUS.name,
        args,
    )
}

fn handle_telegram_chats(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "telegram",
        crate::command_inventory::TELEGRAM_CHATS.name,
        args,
    )
}

fn handle_telegram_chats_archive(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "telegram",
        crate::command_inventory::TELEGRAM_CHATS_ARCHIVE.name,
        args,
    )
}

/// The UI door's write: a command that arrived over the WebSocket is dispatched
/// here too, so it needs a route like any other — its Tauri twin is a different
/// transport, not a substitute.
fn handle_telegram_set_archived(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "telegram",
        crate::command_inventory::TELEGRAM_SET_ARCHIVED.name,
        args,
    )
}

fn handle_telegram_messages_around(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "telegram",
        crate::command_inventory::TELEGRAM_MESSAGES_AROUND.name,
        args,
    )
}

fn handle_explorer_drives(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "explorer",
        crate::command_inventory::EXPLORER_DRIVES.name,
        args,
    )
}

fn handle_explorer_places(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "explorer",
        crate::command_inventory::EXPLORER_PLACES.name,
        args,
    )
}

fn handle_explorer_sort_get(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    let (col, asc) = server.db.get_explorer_sort()?;
    Ok(serde_json::json!({ "column": col, "ascending": asc }))
}

fn handle_explorer_sort_set(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let col = args.get("column").and_then(|v| v.as_str()).unwrap_or("name");
    let asc = args.get("ascending").and_then(|v| v.as_bool()).unwrap_or(true);
    server.db.set_explorer_sort(col, asc)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// The ambient observation behind the micrographics panel.
///
/// A pure read of the module's last fetch: the module answers it in microseconds
/// from a snapshot the background thread already took, so nothing here can be
/// stalled by a slow network.
fn handle_weather_current(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(
        server,
        "micrographics",
        crate::command_inventory::WEATHER_CURRENT.name,
        args,
    )
}

fn handle_weather_location_request(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    crate::weather_location::request(server.app.clone())
}

#[cfg(test)]
mod route_tests {
    use super::*;

    /// The test that makes the inventory worth having: declaring a command without
    /// wiring a handler is a build that cannot pass, not a silent 404 at runtime.
    ///
    /// It covers **every** declared command, not just the socket subset. That was the
    /// hole: the UI door's own writes had no route, and because only socket commands were
    /// checked, nothing failed — the door simply answered "declared in the inventory but
    /// has no handler" when the interface asked. A route is what makes a command
    /// *reachable*; which door may call it is policy, checked separately.
    #[test]
    fn every_command_has_a_handler() {
        for spec in crate::command_inventory::ALL.iter().copied() {
            assert!(
                ROUTES.iter().any(|(name, _)| *name == spec.name),
                "inventory command '{}' has no route handler, so no door can serve it",
                spec.name
            );
        }
    }

    /// The socket subset is what the CLI sees, so it must be routable too — kept as its
    /// own check so a failure says which door lost the command.
    #[test]
    fn every_socket_command_has_a_handler() {
        for spec in crate::command_inventory::socket_commands() {
            assert!(
                ROUTES.iter().any(|(name, _)| *name == spec.name),
                "inventory command '{}' is served on the socket but has no route handler",
                spec.name
            );
        }
    }

    #[test]
    fn every_handler_is_declared_in_the_inventory() {
        for (name, _) in ROUTES {
            assert!(
                crate::command_inventory::find(name).is_some(),
                "handler '{name}' is routed but missing from the inventory"
            );
        }
    }

    /// The policy that was silently wrong before: one rule was applied to both doors, so
    /// the UI's authenticated transport could not serve a state change at all — a write
    /// command was refused everywhere, which made it unreachable rather than safe.
    #[test]
    fn a_write_command_is_refused_on_the_socket_and_answered_on_the_ui() {
        use unidtk_api::commands::Door;
        let write = crate::command_inventory::ALL
            .iter()
            .copied()
            .find(|spec| spec.is_write())
            .expect("the inventory must describe at least one write command");
        assert!(
            check_door(write.name, Door::Socket).is_err(),
            "write command '{}' must stay off the unauthenticated socket",
            write.name
        );
        assert!(
            check_door(write.name, Door::Ui).is_ok(),
            "write command '{}' must be reachable from the UI, or it cannot run at all",
            write.name
        );
    }

    #[test]
    fn a_read_command_is_served_on_both_doors() {
        use unidtk_api::commands::Door;
        // Selected by the property under test, not by position: the first read in the
        // inventory is a CLI-only probe (`ping`), and picking it by index made this test
        // assert the opposite of what the manifest says.
        let read = crate::command_inventory::ALL
            .iter()
            .copied()
            .find(|spec| !spec.is_write() && spec.doors.contains(&Door::Ui))
            .expect("the inventory must describe at least one read served on both doors");
        assert!(check_door(read.name, Door::Socket).is_ok());
        assert!(check_door(read.name, Door::Ui).is_ok());
    }

    #[test]
    fn a_cli_only_read_is_not_offered_to_the_ui() {
        use unidtk_api::commands::Door;
        let cli = crate::command_inventory::ALL
            .iter()
            .copied()
            .find(|spec| !spec.is_write() && !spec.doors.contains(&Door::Ui))
            .expect("the inventory must describe at least one read only the tool door serves");
        assert!(
            check_door(cli.name, Door::Socket).is_ok(),
            "a tool-door read must work on the tool door"
        );
        assert!(
            check_door(cli.name, Door::Ui).is_err(),
            "claiming the UI door for '{}' would promise a command the interface cannot call",
            cli.name
        );
    }

    #[test]
    fn an_unknown_command_is_refused_on_every_door() {
        use unidtk_api::commands::Door;
        let unknown = "definitely_not_a_command";
        assert!(check_door(unknown, Door::Socket).is_err());
        assert!(check_door(unknown, Door::Ui).is_err());
    }

    #[test]
    fn route_names_are_unique() {
        for (name, _) in ROUTES {
            let count = ROUTES.iter().filter(|(other, _)| other == name).count();
            assert_eq!(count, 1, "route '{name}' is declared {count} times");
        }
    }

    /// Decoupling gate: the router must not know module state types. Before S3 it
    /// reached into that state right here, so adding a module meant editing the
    /// kernel; now it addresses modules by id only.
    #[test]
    fn router_does_not_name_module_state_types() {
        const SOURCE: &str = include_str!("ipc.rs");
        // Assembled at runtime on purpose: written as literals, this test's own text
        // would satisfy the search it performs.
        let needles = [
            ["Vpn", "Runtime"].concat(),
            ["Telegram", "Runtime"].concat(),
            ["App", "State"].concat(),
        ];
        for needle in needles {
            assert!(
                !SOURCE.contains(&needle),
                "core/kernel/ipc.rs must not refer to a module state type (`{needle}`); \
                 modules answer their own commands through `ipc_call`"
            );
        }
    }
}
