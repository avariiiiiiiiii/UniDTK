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
        crate::command_inventory::EXPLORER_SORT_GET.name,
        handle_explorer_sort_get,
    ),
    (
        crate::command_inventory::EXPLORER_SORT_SET.name,
        handle_explorer_sort_set,
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
}
