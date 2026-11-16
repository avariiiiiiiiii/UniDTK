//! Lightweight localhost IPC server for agent/CLI debugging (r[ipc.socket-read-only]).
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
//! A module command is served by its module, which the kernel's registry finds by
//! the command's name; a module still booting reports "not ready" instead of
//! panicking. Each connection runs on its own OS thread (never on the event loop),
//! which is also why a module may drive an async command body with
//! `tauri::async_runtime::block_on`.

use crate::log_store::LogStore;
use crate::unidtk_db::UniDtkDatabase;
use tauri::Manager;
use unidtk_api::commands::{CommandSpec, Door};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

pub const IPC_PORT: u16 = 26540;

/// How many requests are served concurrently. Each connection is a separate
/// thread, so extra requests receive `busy` instead of unbounded growth.
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
        door: Door,
    ) -> Result<serde_json::Value, String> {
        let cmd = req["cmd"].as_str().unwrap_or("");
        let args = req["args"].as_object().cloned().unwrap_or_default();

        let Some(spec) = crate::command_inventory::find(cmd) else {
            return serve_module(&self.app, cmd, &serde_json::Value::Object(args), door)
                .unwrap_or_else(|| check_door(cmd, None, door).map(|_| serde_json::Value::Null));
        };
        // The spec is authoritative for which door serves the command: a write
        // command is refused here by policy, not by being forgotten.
        check_door(cmd, Some(spec), door)?;

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
            self.dispatch(&req, Door::Socket),
        )
    }

}

fn json_value<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Serves a module's command through the door `door`, or `None` when no module
/// declares `cmd` (a kernel command, an unknown one, or modules not started yet).
///
/// The one path of a module command, whichever door it came through: the socket and
/// the WebSocket call it from `dispatch`, the Tauri door from the kernel's invoke
/// handler. The caller is never the main thread, because a module command may block.
pub fn serve_module(
    app: &tauri::AppHandle,
    cmd: &str,
    args: &serde_json::Value,
    door: Door,
) -> Option<Result<serde_json::Value, String>> {
    let registry = app.try_state::<crate::registry::Registry>()?;
    let (module, spec) = registry.command(cmd)?;
    if let Err(refusal) = check_door(cmd, Some(spec), door) {
        return Some(Err(refusal));
    }
    Some(module.dispatch(cmd, args).unwrap_or_else(|| {
        Err(format!("module '{}' declares '{cmd}' but does not handle it", module.id()))
    }))
}

/// Applies a command's door policy. `spec` is `None` for a name nobody declares.
///
/// A free function on purpose: the policy is pure data, and keeping it out of `dispatch`
/// is what makes it testable at all — an `IpcServer` needs a live `AppHandle` to exist,
/// while this needs nothing.
fn check_door(cmd: &str, spec: Option<&CommandSpec>, door: Door) -> Result<(), String> {
    match spec {
        None => Err(format!("unknown cmd: {cmd}")),
        Some(spec) if !spec.doors.contains(&door) => {
            // Say *why*, not just *no*: the refusals mean different things to a caller —
            // "this changes state", "this is the tool door's own surface", or "this
            // command is simply not yours to ask for".
            let other = if spec.doors.contains(&Door::Socket) {
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
        crate::command_inventory::SET_MODULE_ENABLED.name,
        handle_set_module_enabled,
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
        crate::command_inventory::WEATHER_LOCATION_REQUEST.name,
        handle_weather_location_request,
    ),
    (crate::command_inventory::DESK_HOST_GET.name, handle_desk_host_get),
    (crate::command_inventory::DESK_HOST_SET.name, handle_desk_host_set),
    (crate::command_inventory::DESK_POSITION_SET.name, handle_desk_position_set),
    (crate::command_inventory::DESK_LAYOUT_GET.name, handle_desk_layout_get),
    (crate::command_inventory::DESK_APPLY.name, handle_desk_apply),
    (crate::command_inventory::GET_FOLDERS.name, handle_get_folders),
    (crate::command_inventory::SAVE_FOLDER.name, handle_save_folder),
    (crate::command_inventory::DELETE_FOLDER.name, handle_delete_folder),
    (crate::command_inventory::MISSING_TARGETS.name, handle_missing_targets),
    (crate::command_inventory::PICK_SHORTCUT_TARGET.name, handle_pick_shortcut_target),
    (crate::command_inventory::CLIPBOARD_SHORTCUT_TARGETS.name, handle_clipboard_shortcut_targets),
    (crate::command_inventory::DESCRIBE_SHORTCUT_TARGET.name, handle_describe_shortcut_target),
    (crate::command_inventory::SHORTCUT_ICON.name, handle_shortcut_icon),
    (crate::command_inventory::START_MENU_LIST.name, handle_start_menu_list),
    (crate::command_inventory::LAUNCH_SHORTCUT.name, handle_launch_shortcut),
    (crate::command_inventory::OPEN_EXPLORER.name, handle_open_explorer),
    (crate::command_inventory::QUIT_APP.name, handle_quit_app),
    (crate::command_inventory::HOST_REGISTER.name, handle_host_register),
    (
        crate::command_inventory::SET_DESKTOP_INPUT_FOCUS.name,
        handle_set_desktop_input_focus,
    ),
    (crate::command_inventory::HOST_PING.name, handle_host_ping),
    (crate::command_inventory::HOST_TREE.name, handle_host_tree),
    (crate::command_inventory::HOST_FRAMES.name, handle_host_frames),
    (crate::command_inventory::HOST_HIT.name, handle_host_hit),
];

pub(crate) fn ping_payload() -> serde_json::Value {
    let snap = crate::host_supervisor::runtime_snapshot();
    serde_json::json!({
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
        "host": {
            "mode": crate::current_host_mode().as_str(),
            "pid": snap.pid,
            "generation": snap.generation,
            "inspectorPort": snap.inspector_port,
            "attached": snap.attached,
            "ready": snap.ready,
        },
    })
}

fn handle_ping(_server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    Ok(ping_payload())
}

const MAX_LOGS_LIMIT: u64 = 1000;

fn query_logs(logs: &LogStore, args: &Args) -> Result<Vec<crate::log_store::LogEntry>, String> {
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(MAX_LOGS_LIMIT) as usize;
    let level = args.get("level").and_then(|v| v.as_str());
    let source = args.get("source").and_then(|v| v.as_str());
    match args.get("since").and_then(|v| v.as_u64()) {
        Some(since) => logs.get_since_filtered(since, Some(limit), level, source),
        None => logs.list(limit, level, source),
    }
}

fn handle_logs(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let logs = server
        .logs
        .as_ref()
        .ok_or_else(|| "log store unavailable".to_string())?;
    json_value(query_logs(logs, args)?)
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
    let (max_id, entries) = logs.get_delta(since, limit)?;
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

fn handle_set_module_enabled(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    let id = args.get("id").and_then(|v| v.as_str()).ok_or("missing id")?;
    let enabled = args.get("enabled").and_then(|v| v.as_bool()).ok_or("missing enabled")?;
    crate::apply_module_enabled(&server.app, &server.db, id, enabled)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// Describes the surface of this door from the inventory itself, so the reply cannot
/// disagree with what is actually served.
fn handle_commands(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    // The running modules' tables, not `manifest()`: that one builds fresh module
    // instances, which the offline CLI needs and the running app does not.
    let registry = server.app.try_state::<crate::registry::Registry>();
    let module_commands = registry
        .iter()
        .flat_map(|registry| registry.in_start_order().iter().flat_map(|module| module.commands()));
    Ok(serde_json::json!({
        "socket": crate::command_inventory::ALL
            .iter()
            .copied()
            .chain(module_commands)
            .filter(|spec| spec.on_socket())
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
    let level = args.get("level").and_then(|value| value.as_str());
    crate::logging::log_frontend(message, level);
    Ok(serde_json::Value::Null)
}

fn handle_weather_location_request(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    crate::weather_location::request(server.app.clone())
}

fn handle_desk_host_get(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    let persisted = server.db.get_desk_host()?;
    let active = crate::current_host_mode().as_str();
    let sup = crate::host_supervisor::status();
    Ok(serde_json::json!({
        "mode": active,
        "persisted": persisted,
        "supervisorPid": sup.pid,
        "generation": sup.generation,
        "supervisorRunning": sup.running,
    }))
}

fn handle_desk_layout_get(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    crate::desk_layout::snapshot(&server.db)
}

fn handle_desk_position_set(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let patch = crate::desk_layout::parse_patch(args)?;
    crate::desk_layout::set_position(&server.db, &patch)
}

fn string_arg<'a>(args: &'a Args, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| format!("missing {key}"))
}

fn to_json<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn handle_get_folders(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    to_json(server.db.list_folders()?)
}

fn handle_save_folder(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let folder: crate::unidtk_db::FolderRecord =
        serde_json::from_value(args.get("folder").cloned().ok_or("missing folder")?).map_err(|e| format!("folder: {e}"))?;
    server.db.save_folder(&folder)?;
    Ok(serde_json::Value::Null)
}

fn handle_delete_folder(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    to_json(server.db.delete_folder(string_arg(args, "id")?)?)
}

fn handle_missing_targets(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let targets: Vec<String> = serde_json::from_value(args.get("targets").cloned().unwrap_or_default()).map_err(|e| format!("targets: {e}"))?;
    to_json(crate::start_menu::missing(&targets))
}

fn handle_pick_shortcut_target(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let folder = args.get("folder").and_then(|v| v.as_bool()).unwrap_or(false);
    to_json(crate::pick_shortcut_target_with(&server.app, folder)?)
}

fn handle_clipboard_shortcut_targets(_server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    to_json(crate::clipboard_shortcuts()?)
}

fn handle_describe_shortcut_target(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    to_json(crate::start_menu::describe(string_arg(args, "target")?.to_string())?)
}

fn handle_shortcut_icon(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let size = args.get("size").and_then(|v| v.as_u64()).map_or(48, |s| s.clamp(16, 256) as u32);
    to_json(crate::start_menu::icon(string_arg(args, "target")?.to_string(), size)?)
}

fn handle_start_menu_list(_server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    to_json(crate::start_menu::start_menu()?)
}

fn handle_launch_shortcut(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let launch_args = args.get("args").and_then(|v| v.as_str()).unwrap_or("");
    crate::start_menu::launch(string_arg(args, "target")?, launch_args)?;
    Ok(serde_json::Value::Null)
}

fn handle_open_explorer(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let path = args.get("path").and_then(|v| v.as_str()).map(str::to_string);
    crate::open_explorer_at(&server.app, path);
    Ok(serde_json::Value::Null)
}

fn handle_quit_app(server: &IpcServer, _args: &Args) -> Result<serde_json::Value, String> {
    // The reply leaves before the process does.
    let app = server.app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        crate::quit_with(&app);
    });
    Ok(serde_json::Value::Null)
}

fn handle_desk_apply(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    crate::desk_layout::apply(&server.db, args)
}

fn handle_desk_host_set(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let raw = args.get("mode").and_then(|v| v.as_str()).ok_or("missing mode")?;
    let target = crate::HostMode::parse_strict(raw)?;
    crate::switch_desk_host(&server.app, target)?;
    Ok(serde_json::json!({ "ok": true, "mode": target.as_str() }))
}

fn handle_host_register(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    let pid = args
        .get("pid")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("host_register requires valid {pid}")?;
    let generation = args
        .get("generation")
        .and_then(|v| v.as_u64())
        .ok_or("host_register requires {generation}")?;
    let instance_nonce = args
        .get("instanceNonce")
        .and_then(|v| v.as_str())
        .ok_or("host_register requires {instanceNonce}")?;
    let inspector_port = args
        .get("inspectorPort")
        .and_then(|v| v.as_u64())
        .and_then(|v| u16::try_from(v).ok())
        .ok_or("host_register requires valid {inspectorPort}")?;
    let attached = args.get("attached").and_then(|v| v.as_bool()).unwrap_or(false);
    let ready = args.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);

    crate::host_supervisor::register_host(
        pid,
        generation,
        instance_nonce,
        inspector_port,
        attached,
        ready,
    )?;
    Ok(serde_json::json!({ "ok": true }))
}

fn handle_set_desktop_input_focus(
    server: &IpcServer,
    args: &Args,
) -> Result<serde_json::Value, String> {
    let focused = args
        .get("focused")
        .and_then(|v| v.as_bool())
        .ok_or("set_desktop_input_focus requires {focused}")?;
    crate::hotkeys::set_input_focus(&server.app, focused)?;
    Ok(serde_json::Value::Null)
}

pub(crate) fn proxy_host_request(
    port: Option<u16>,
    cmd: &str,
    args: &Args,
) -> Result<serde_json::Value, String> {
    let port = match port {
        Some(p) => p,
        None => crate::host_supervisor::runtime_snapshot()
            .inspector_port
            .ok_or_else(|| "native host inspector is not registered".to_string())?,
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let timeout = std::time::Duration::from_millis(250);
    let mut stream = TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(100))
        .map_err(|e| format!("host proxy connect :{port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let payload = serde_json::json!({
        "v": 1,
        "cmd": cmd,
        "args": args,
    });
    writeln!(stream, "{payload}").map_err(|e| format!("host proxy write: {e}"))?;
    let _ = stream.flush();

    let mut line = String::new();
    let n = BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("host proxy read: {e}"))?;
    if n == 0 {
        return Err("host proxy closed without reply".into());
    }
    let reply: serde_json::Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("host proxy bad json: {e}"))?;
    if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(reply.get("result").cloned().unwrap_or(serde_json::Value::Null))
    } else {
        Err(reply
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("native host request failed")
            .to_string())
    }
}

fn handle_host_ping(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    proxy_host_request(None, "ping", args)
}

fn handle_host_tree(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    proxy_host_request(None, "tree", args)
}

fn handle_host_frames(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    proxy_host_request(None, "frames", args)
}

fn handle_host_hit(_server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    if args.get("x").and_then(|v| v.as_f64()).is_none()
        || args.get("y").and_then(|v| v.as_f64()).is_none()
    {
        return Err("host_hit requires numeric {x, y}".into());
    }
    proxy_host_request(None, "hit", args)
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
    /// own check so a failure says which door lost the command. A module's socket
    /// command is served by the module's table, which generates its handler, and the
    /// registry must find its module.
    #[test]
    fn every_socket_command_has_a_handler() {
        let module_commands = crate::registry::all_module_commands();
        for spec in crate::command_inventory::socket_commands() {
            assert!(
                ROUTES.iter().any(|(name, _)| *name == spec.name)
                    || module_commands.iter().any(|(_, module_spec)| module_spec.name == spec.name),
                "command '{}' is served on the socket but neither the kernel nor a module handles it",
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

    /// The spec of any command of the application, the kernel's or a module's.
    fn spec(name: &str) -> Option<&'static CommandSpec> {
        crate::command_inventory::manifest().into_iter().find(|spec| spec.name == name)
    }

    fn door(name: &str, door: Door) -> Result<(), String> {
        check_door(name, spec(name), door)
    }

    /// The policy that was silently wrong before: one rule was applied to both doors, so
    /// the UI's authenticated transport could not serve a state change at all — a write
    /// command was refused everywhere, which made it unreachable rather than safe.
    #[test]
    fn a_write_command_is_refused_on_the_socket_and_answered_on_the_ui() {
        for name in ["set_module_enabled", "set_system_volume", "vpn_connect", "telegram_send_message"] {
            assert!(door(name, Door::Socket).is_err(), "write command '{name}' must stay off the unauthenticated socket");
            assert!(door(name, Door::Ui).is_ok(), "write command '{name}' must be reachable from the UI, or it cannot run at all");
        }
    }

    #[test]
    fn a_read_command_is_served_on_both_doors() {
        for name in ["get_widget_config", "vpn_status", "player_status", "get_drives", "weather_current"] {
            assert!(door(name, Door::Socket).is_ok(), "'{name}' was served by the socket and must stay so");
            assert!(door(name, Door::Ui).is_ok());
        }
    }

    /// The socket surface does not grow with the interface's getters: a file preview
    /// read through the unauthenticated socket would hand any local process the user's files.
    #[test]
    fn an_interface_read_is_not_offered_to_the_socket() {
        for name in ["get_file_preview", "get_text_preview", "telegram_messages"] {
            assert!(door(name, Door::Socket).is_err(), "'{name}' must stay off the socket");
            assert!(door(name, Door::Ui).is_ok());
        }
    }

    #[test]
    fn a_cli_only_read_is_not_offered_to_the_ui() {
        let cli = crate::command_inventory::ALL
            .iter()
            .copied()
            .find(|spec| !spec.is_write() && !spec.doors.contains(&Door::Ui))
            .expect("the inventory must describe at least one read only the tool door serves");
        assert!(door(cli.name, Door::Socket).is_ok(), "a tool-door read must work on the tool door");
        assert!(
            door(cli.name, Door::Ui).is_err(),
            "claiming the UI door for '{}' would promise a command the interface cannot call",
            cli.name
        );
    }

    #[test]
    fn an_unknown_command_is_refused_on_every_door() {
        let unknown = "definitely_not_a_command";
        assert!(door(unknown, Door::Socket).is_err());
        assert!(door(unknown, Door::Ui).is_err());
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
                 modules answer their own commands through `Module::dispatch`"
            );
        }
    }

    #[test]
    fn ping_includes_host_status_block() {
        let _guard = crate::host_supervisor::test_lock();
        let ping = ping_payload();
        assert_eq!(ping["pong"], true);
        let host = ping.get("host").expect("ping payload must include host block");
        assert!(host["mode"].is_string());
        assert!(host.get("pid").is_some());
        assert!(host.get("generation").is_some());
        assert!(host.get("inspectorPort").is_some());
        assert!(host.get("attached").is_some());
        assert!(host.get("ready").is_some());
    }

    #[test]
    fn host_read_proxies_declared_in_inventory_and_forward_lock_free() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        assert!(door("host_register", Door::Socket).is_err());
        assert!(door("host_register", Door::Ui).is_ok());
        for name in ["host_ping", "host_tree", "host_frames", "host_hit"] {
            assert!(door(name, Door::Socket).is_ok(), "{name} must be on Socket");
            assert!(door(name, Door::Ui).is_err(), "{name} is cli-only");
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&mut stream).read_line(&mut line).unwrap();
                let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let reply = match req["cmd"].as_str().unwrap_or("") {
                    "ping" => serde_json::json!({ "v": 1, "ok": true, "result": { "port": port, "ready": true } }),
                    "tree" => serde_json::json!({ "v": 1, "ok": true, "result": [{ "key": "g0-card" }] }),
                    "frames" => serde_json::json!({ "v": 1, "ok": true, "result": { "drawn": 3 } }),
                    "hit" => serde_json::json!({ "v": 1, "ok": true, "result": { "x": req["args"]["x"], "in_region": true } }),
                    other => serde_json::json!({ "v": 1, "ok": false, "error": format!("unexpected {other}") }),
                };
                writeln!(stream, "{reply}").unwrap();
            }
        });

        let ping = proxy_host_request(Some(port), "ping", &Args::new()).unwrap();
        assert_eq!(ping["port"], port);
        let tree = proxy_host_request(Some(port), "tree", &Args::new()).unwrap();
        assert_eq!(tree[0]["key"], "g0-card");
        let frames = proxy_host_request(Some(port), "frames", &Args::new()).unwrap();
        assert_eq!(frames["drawn"], 3);
        let mut hit_args = Args::new();
        hit_args.insert("x".into(), serde_json::json!(24.0));
        hit_args.insert("y".into(), serde_json::json!(48.0));
        let hit = proxy_host_request(Some(port), "hit", &hit_args).unwrap();
        assert_eq!(hit["in_region"], true);
        let _ = server.join();
    }

    #[test]
    fn handle_logs_applies_level_source_with_since_and_clamps_limit() {
        let dir = std::env::temp_dir().join(format!("test_ipc_query_logs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let store = LogStore::open(dir.join("logs.redb")).unwrap();
        let entries: Vec<crate::log_store::LogEntry> = (1..=1010)
            .map(|i| crate::log_store::LogEntry {
                id: i,
                ts: 1000 + i as i64,
                level: if i % 2 == 0 { "ERROR" } else { "INFO" }.into(),
                source: if i % 3 == 0 { "frontend" } else { "backend" }.into(),
                module: "test".into(),
                message: format!("m{i}"),
                meta: None,
            })
            .collect();
        store.insert_batch(&entries).unwrap();

        // When `since` is combined with `level` and `source`, non-matching rows must be skipped.
        let mut args = Args::new();
        args.insert("since".into(), serde_json::json!(1000));
        args.insert("level".into(), serde_json::json!("ERROR"));
        args.insert("source".into(), serde_json::json!("frontend"));
        args.insert("limit".into(), serde_json::json!(10));
        let filtered = query_logs(&store, &args).unwrap();
        let filtered_ids: Vec<u64> = filtered.iter().map(|e| e.id).collect();
        assert_eq!(filtered_ids, vec![1002, 1008]);
        assert!(filtered.iter().all(|e| e.level == "ERROR" && e.source == "frontend"));

        // Huge limits must clamp to 1000 on both `since` and `list` paths.
        let mut clamped_since = Args::new();
        clamped_since.insert("since".into(), serde_json::json!(0));
        clamped_since.insert("limit".into(), serde_json::json!(5000));
        assert_eq!(query_logs(&store, &clamped_since).unwrap().len(), 1000);

        let mut clamped_list = Args::new();
        clamped_list.insert("limit".into(), serde_json::json!(5000));
        assert_eq!(query_logs(&store, &clamped_list).unwrap().len(), 1000);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
