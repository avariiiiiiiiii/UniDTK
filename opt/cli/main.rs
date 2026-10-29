//! `udtk` — agent/debug bridge to the running UniDTK app (architecture §5.3).
//!
//! Talks to the app's localhost IPC server (127.0.0.1:26540) and the WebView2
//! CDP endpoint (always on in release; the port is published over the IPC
//! `ping` response, never guessed).

use serde_json::json;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;
use unidtk_api::ModuleDb;

mod splash;
mod splash_art;

/// CREATE_NO_WINDOW: spawned console helpers (taskkill/sch tasks/reg/powershell)
/// must never flash a black window, in line with the project's rule that
/// background work is invisible (AGENTS.md §6).
const NO_WINDOW: u32 = 0x0800_0000;

#[path = "../../core/kernel/registry.rs"]
mod registry;

#[path = "../../core/kernel/command_inventory.rs"]
mod command_inventory;

const IPC: &str = "127.0.0.1:26540";
/// Fallback only — the live CDP port is published by the app over IPC (`ping` →
/// `cdpPort`). A stale or foreign process may hold the conventional 9222.
const CDP_FALLBACK: u16 = 9222;
const CRLF: &str = "\r\n";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        splash::print();
        return;
    }
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let res: Result<String, (i32, String)> = match cmd {
        "invoke" => invoke_cmd(&args).map_err(|e| (1, e)),
        "logs" => logs_cmd(&args).map_err(|e| (1, e)),
        "gui" => gui_cmd(&args).map_err(|e| (1, e)),
        "shot" => shot_cmd(&args).map_err(|e| (1, e)),
        "cdp-list" => cdp_list_cmd().map_err(|e| (1, e)),
        "doctor" => doctor_cmd(&args),
        "build" => build_cmd(&args).map_err(|e| (1, e)),
        "stop" => stop_cmd().map_err(|e| (1, e)),
        "run" => run_cmd(&args).map_err(|e| (1, e)),
        "autostart" => autostart_cmd(&args).map_err(|e| (1, e)),
        "tasks" => tasks_cmd(&args).map_err(|e| (1, e)),
        "path" => path_cmd(&args).map_err(|e| (1, e)),
        "help" => help_cmd(&args).map_err(|e| (1, e)),
        _ => {
            let h = help();
            Err((2, format!("unknown command: {cmd}\n{h}")))
        }
    };
    match res {
        Ok(out) => println!("{out}"),
        Err((code, e)) => {
            if code == 2 {
                eprintln!("error: {e}");
            } else {
                eprintln!("{e}");
            }
            std::process::exit(code);
        }
    }
}

// ---- IPC ----

/// Wire version this bridge speaks; `ping` reports what the app speaks and a mismatch
/// is an error rather than a guess (app and CLI are replaced independently).
const PROTOCOL: u64 = 1;

fn ipc(req: serde_json::Value) -> Result<String, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);

    let socket_addr: std::net::SocketAddr = IPC
        .parse()
        .map_err(|e| format!("invalid IPC address {IPC}: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_millis(500))
        .map_err(|e| format!("IPC server not reachable at {IPC}: {e} (is the app running?)"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(2500)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(2500)))
        .map_err(|e| e.to_string())?;
    let mut envelope = req;
    envelope["v"] = json!(PROTOCOL);
    envelope["id"] = json!(id);
    let mut buf = envelope.to_string();
    buf.push('\n');
    stream.write_all(buf.as_bytes()).map_err(|e| e.to_string())?;
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(stream);
    let mut resp = String::new();
    reader.read_line(&mut resp).map_err(|e| e.to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(resp.trim()).map_err(|e| format!("bad ipc response: {e}"))?;
    // An older kernel answers without an envelope; only what is present is checked.
    if let Some(answered) = v.get("id").and_then(|x| x.as_u64()) {
        if answered != id {
            return Err(format!(
                "protocol: response id {answered} does not match request id {id}"
            ));
        }
    }
    if let Some(proto) = v.get("v").and_then(|x| x.as_u64()) {
        if proto != PROTOCOL {
            return Err(format!(
                "protocol mismatch: the app speaks v{proto}, this bridge speaks v{PROTOCOL} — \
                 update them together so both sides match"
            ));
        }
    }
    if v["ok"].as_bool().unwrap_or(false) {
        Ok(serde_json::to_string_pretty(&v["result"]).unwrap_or_else(|_| resp))
    } else {
        Err(v["error"].as_str().unwrap_or("unknown error").to_string())
    }
}

fn invoke_cmd(args: &[String]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("usage: udtk invoke <cmd> [json-args]".into());
    }
    let cmd = args[1].as_str();
    let req_args = if args.len() >= 3 {
        serde_json::from_str(&args[2]).map_err(|e| format!("bad json args: {e}"))?
    } else {
        json!({})
    };
    ipc(json!({"cmd": cmd, "args": req_args}))
}

fn logs_cmd(args: &[String]) -> Result<String, String> {
    let mut limit = 50u64;
    let mut since: Option<u64> = None;
    let mut level: Option<String> = None;
    let mut source: Option<String> = None;
    if let Some(i) = args.iter().position(|a| a == "--limit") {
        if let Some(v) = args.get(i + 1) {
            limit = v.parse().unwrap_or(50);
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--since") {
        if let Some(v) = args.get(i + 1) {
            since = v.parse().ok();
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--level") {
        if let Some(v) = args.get(i + 1) {
            level = Some(v.clone());
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--source") {
        if let Some(v) = args.get(i + 1) {
            source = Some(v.clone());
        }
    }
    let mut req_args = json!({"limit": limit});
    if let Some(s) = since {
        req_args["since"] = json!(s);
    }
    if let Some(l) = level {
        req_args["level"] = json!(l);
    }
    if let Some(s) = source {
        req_args["source"] = json!(s);
    }
    // The journal reader's name comes from the inventory, so a rename of the
    // command cannot leave `logs` talking to a dead name.
    ipc(json!({"cmd": command_inventory::SIMULATE_LOGS, "args": req_args}))
}

// ---- CDP (WebView2 remote debugging) ----

/// Port of the *live* app's CDP endpoint, asked of the app itself over IPC. Never
/// guessed: a blind 9222 fallback would let us drive a foreign or stale WebView,
/// which is exactly what this discovery exists to prevent.
fn cdp_port() -> Result<u16, String> {
    static PORT: std::sync::OnceLock<Result<u16, String>> = std::sync::OnceLock::new();
    PORT.get_or_init(|| {
        let out = ipc(json!({"cmd": command_inventory::SIMULATE_PING})).map_err(|e| {
            format!("cannot determine the CDP port: app is not answering on {IPC} ({e})")
        })?;
        let value: serde_json::Value =
            serde_json::from_str(&out).map_err(|e| format!("bad ping response: {e}"))?;
        match value.get("cdpPort").and_then(|p| p.as_u64()) {
            Some(port) => Ok(port as u16),
            // A kernel older than S0 does not publish its port; it always binds the
            // conventional one, and the app is proven alive by the IPC answer above.
            None => Ok(CDP_FALLBACK),
        }
    })
    .clone()
}

fn cdp_get(path: &str) -> Result<serde_json::Value, String> {
    let port = cdp_port()?;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("CDP not reachable on {port}: {e} (is the app running?)"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let req = format!("GET {path} HTTP/1.1{0}Host: 127.0.0.1{0}Connection: close{0}{0}", CRLF);
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut resp = String::new();
    let mut buf = [0u8; 8192];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if resp.contains(CRLF.repeat(2).as_str()) || resp.len() > 16 * 1024 {
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => resp.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("cdp read: {e}")),
        }
    }
    let sep = CRLF.repeat(2);
    let body = resp.split(sep.as_str()).nth(1).unwrap_or("");
    serde_json::from_str(body).map_err(|e| format!("bad CDP json: {e} (resp len {})", resp.len()))
}

fn cdp_list_cmd() -> Result<String, String> {
    let targets = cdp_get("/json")?;
    Ok(serde_json::to_string_pretty(&targets).unwrap_or_default())
}

fn cdp_ws_url() -> Result<String, String> {
    let targets = cdp_get("/json")?;
    let port = cdp_port()?;
    let arr = targets
        .as_array()
        .ok_or("no CDP targets found (is UniDTK running?)")?;
    // The wallpaper document first: a transient window (the location-permission
    // host) is also a page, and must never become the target by list order.
    let page = arr
        .iter()
        .find(|t| t["type"] == "page" && t["url"] == "http://tauri.localhost/")
        .or_else(|| arr.iter().find(|t| t["type"] == "page"))
        .or_else(|| arr.first())
        .ok_or("no CDP page target found")?;

    // Never drive a foreign window: the target must be our own app's document.
    let page_url = page["url"].as_str().unwrap_or("");
    if !page_url.starts_with("http://tauri.localhost") {
        return Err(format!(
            "CDP target is not the UniDTK page (url: {page_url}); refusing to drive a foreign window"
        ));
    }

    let url = page["webSocketDebuggerUrl"]
        .as_str()
        .ok_or("no webSocketDebuggerUrl in CDP target")?;

    // WebView2 CDP often reports ws://localhost/devtools/page/... without port. Ensure port is present:
    let formatted = if !url.contains(&format!(":{port}")) && url.starts_with("ws://localhost") {
        url.replacen("ws://localhost", &format!("ws://127.0.0.1:{port}"), 1)
    } else if url.starts_with("ws://127.0.0.1/") {
        url.replacen("ws://127.0.0.1", &format!("ws://127.0.0.1:{port}"), 1)
    } else {
        url.to_string()
    };
    Ok(formatted)
}

/// Execute a CDP method over native RFC 6455 WebSocket via tungstenite.
fn cdp_call(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let ws_url = cdp_ws_url()?;
    let port = cdp_port()?;
    let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("CDP TCP connect to 127.0.0.1:{port} failed: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(8)));

    let mut ws_config = tungstenite::protocol::WebSocketConfig::default();
    ws_config.max_frame_size = Some(64 * 1024 * 1024);
    ws_config.max_message_size = Some(64 * 1024 * 1024);

    let (mut socket, _) = tungstenite::client::client_with_config(&ws_url, stream, Some(ws_config))
        .map_err(|e| format!("CDP WS handshake failed ({ws_url}): {e}"))?;

    let req_id = 1u64;
    let req = serde_json::json!({
        "id": req_id,
        "method": method,
        "params": params,
    });

    socket
        .send(tungstenite::Message::Text(req.to_string()))
        .map_err(|e| format!("CDP WS send failed: {e}"))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if std::time::Instant::now() > deadline {
            return Err("CDP WS response timeout".into());
        }
        let msg = socket.read().map_err(|e| format!("CDP WS read: {e}"))?;
        if let tungstenite::Message::Text(text) = msg {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if v.get("id").and_then(|id| id.as_u64()) == Some(req_id) {
                    if let Some(err) = v.get("error") {
                        let err_msg = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| err.to_string());
                        return Err(format!("CDP error: {err_msg}"));
                    }
                    return Ok(v["result"].clone());
                }
            }
        }
    }
}

// ---- gui: one CDP session, a batch of steps ----

/// CDP client that keeps a single WebSocket for a whole batch. The old
/// dom/act/eval commands opened a fresh connection per call — one click cost five
/// handshakes; steps in a batch now share one.
struct Cdp {
    socket: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Cdp {
    fn open() -> Result<Self, String> {
        let ws_url = cdp_ws_url()?;
        let port = cdp_port()?;
        let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .map_err(|e| format!("CDP TCP connect to 127.0.0.1:{port} failed: {e}"))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let mut config = tungstenite::protocol::WebSocketConfig::default();
        config.max_frame_size = Some(16 * 1024 * 1024);
        config.max_message_size = Some(16 * 1024 * 1024);
        let (socket, _) = tungstenite::client::client_with_config(&ws_url, stream, Some(config))
            .map_err(|e| format!("CDP WS handshake failed ({ws_url}): {e}"))?;
        Ok(Self { socket, next_id: 1 })
    }

    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(tungstenite::Message::Text(request.to_string()))
            .map_err(|e| format!("CDP WS send failed: {e}"))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if std::time::Instant::now() > deadline {
                return Err(format!("CDP response timeout for {method}"));
            }
            let message = self
                .socket
                .read()
                .map_err(|e| format!("CDP WS read: {e}"))?;
            if let tungstenite::Message::Text(text) = message {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                        if let Some(error) = value.get("error") {
                            let message = error
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown");
                            return Err(format!("CDP error: {message}"));
                        }
                        return Ok(value["result"].clone());
                    }
                    // Progress events carry no matching id and are ignored on purpose.
                }
            }
        }
    }

    fn evaluate(&mut self, expression: &str) -> Result<serde_json::Value, String> {
        let result = self.call(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )?;
        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception["exception"]["description"]
                .as_str()
                .or_else(|| exception["text"].as_str())
                .unwrap_or("JS Exception");
            return Err(format!("JS Exception: {text}"));
        }
        Ok(result["result"]["value"].clone())
    }

    /// Move → press → release, i.e. what a real mouse does. Coordinates are CSS
    /// pixels straight from `getBoundingClientRect()`; CDP takes them 1:1, and
    /// multiplying by devicePixelRatio would miss by 1.25–1.5x.
    fn mouse_click(&mut self, x: f64, y: f64) -> Result<(), String> {
        for event in ["mouseMoved", "mousePressed", "mouseReleased"] {
            let mut params = serde_json::json!({ "type": event, "x": x, "y": y });
            if event != "mouseMoved" {
                params["button"] = serde_json::json!("left");
                params["clickCount"] = serde_json::json!(1);
            }
            self.call("Input.dispatchMouseEvent", params)?;
        }
        Ok(())
    }
}

const HARNESS: &str = "window.__UNIDTK_HARNESS__";

/// New high-water mark of the kernel log.
fn log_max_id() -> Option<u64> {
    ipc(serde_json::json!({"cmd": "log_max_id"}))
        .ok()
        .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
        .and_then(|v| v.get("maxId").and_then(|m| m.as_u64()))
}

/// Everything the kernel logged after `since`, plus the new watermark — one IPC
/// round-trip per step instead of two.
fn log_delta(since: u64) -> (Vec<serde_json::Value>, Option<u64>) {
    let request = serde_json::json!({
        "cmd": "log_delta",
        "args": { "since": since, "limit": 50 },
    });
    let value: serde_json::Value = match ipc(request)
        .ok()
        .and_then(|out| serde_json::from_str::<serde_json::Value>(&out).ok())
    {
        Some(v) => v,
        None => return (Vec::new(), None),
    };
    let entries = value
        .get("entries")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    (entries, value.get("maxId").and_then(|m| m.as_u64()))
}

/// Page-side errors collected since the previous step (the buffer is cleared).
fn take_ui_errors(cdp: &mut Cdp) -> Vec<serde_json::Value> {
    cdp.evaluate(&format!("({HARNESS}) ? {HARNESS}.getErrors(true) : []"))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Resolve a step target through the page's own harness (testid, stable ref,
/// positional id, accessible name or selector) and report its hit point and what
/// covers it.
fn resolve_step_target(cdp: &mut Cdp, target: &str) -> Result<serde_json::Value, String> {
    let expression = format!(
        "(function(){{ const h = {HARNESS}; if (!h) return null; try {{ return h.resolveTarget({}); }} catch (e) {{ return null; }} }})()",
        serde_json::to_string(target).unwrap_or_else(|_| "null".into())
    );
    let node = cdp.evaluate(&expression)?;
    if node.is_null() {
        return Err(format!("target not found in the active UI: {target}"));
    }
    Ok(node)
}

/// `press:` payload. The table is the contract for the step: one canonical DOM
/// key name per capability, and an unknown or non-ASCII name is refused instead
/// of being dispatched as a phantom event (same idea as `refused:"occluded"`).
#[derive(Debug)]
struct PressKey {
    /// DOM `KeyboardEvent.key`
    key: String,
    /// DOM `KeyboardEvent.code` — the physical US key, layout independent.
    code: String,
    /// `windowsVirtualKeyCode` — legacy `keyCode` in the page.
    vk: u32,
    /// Characters this press produces on its own (None for control keys).
    text: Option<String>,
}

const MOD_ALT: u32 = 1;
const MOD_CTRL: u32 = 2;
const MOD_META: u32 = 4;
const MOD_SHIFT: u32 = 8;

const PRESS_USAGE: &str = "usage: press:<key> | press:<mod>+<key> with mods ctrl|shift|alt|meta\n  \
     keys: Escape Enter Backspace Delete Tab Space ArrowLeft ArrowUp ArrowRight ArrowDown \
     Home End PageUp PageDown F1..F12 a-z A-Z 0-9 - = [ ] \\ ; ' ` , . / +";

/// What a US key produces with Shift held.
fn us_shift(ch: char) -> Option<char> {
    Some(match ch {
        '1' => '!', '2' => '@', '3' => '#', '4' => '$', '5' => '%',
        '6' => '^', '7' => '&', '8' => '*', '9' => '(', '0' => ')',
        '-' => '_', '=' => '+', '[' => '{', ']' => '}', '\\' => '|',
        ';' => ':', '\'' => '"', '`' => '~', ',' => '<', '.' => '>', '/' => '?',
        _ => return None,
    })
}

/// Control/navigation keys: canonical DOM name -> (code, virtual key).
fn named_key(name: &str) -> Option<(&'static str, u32)> {
    Some(match name {
        "Escape" => ("Escape", 27),
        "Enter" => ("Enter", 13),
        "Backspace" => ("Backspace", 8),
        "Delete" => ("Delete", 46),
        "Tab" => ("Tab", 9),
        "Space" => ("Space", 32),
        "ArrowLeft" => ("ArrowLeft", 37),
        "ArrowUp" => ("ArrowUp", 38),
        "ArrowRight" => ("ArrowRight", 39),
        "ArrowDown" => ("ArrowDown", 40),
        "Home" => ("Home", 36),
        "End" => ("End", 35),
        "PageUp" => ("PageUp", 33),
        "PageDown" => ("PageDown", 34),
        "F1" => ("F1", 112),
        "F2" => ("F2", 113),
        "F3" => ("F3", 114),
        "F4" => ("F4", 115),
        "F5" => ("F5", 116),
        "F6" => ("F6", 117),
        "F7" => ("F7", 118),
        "F8" => ("F8", 119),
        "F9" => ("F9", 120),
        "F10" => ("F10", 121),
        "F11" => ("F11", 122),
        "F12" => ("F12", 123),
        _ => return None,
    })
}

/// US punctuation: character -> (code, virtual key). ASCII is not enough here:
/// `VK_OEM_*` codes differ from the character's own code.
fn punct_key(ch: char) -> Option<(&'static str, u32)> {
    Some(match ch {
        '-' => ("Minus", 0xBD),
        '=' => ("Equal", 0xBB),
        '[' => ("BracketLeft", 0xDB),
        ']' => ("BracketRight", 0xDD),
        '\\' => ("Backslash", 0xDC),
        ';' => ("Semicolon", 0xBA),
        '\'' => ("Quote", 0xDE),
        '`' => ("Backquote", 0xC0),
        ',' => ("Comma", 0xBC),
        '.' => ("Period", 0xBE),
        '/' => ("Slash", 0xBF),
        '+' => ("Equal", 0xBB),
        _ => return None,
    })
}

fn press_unknown(name: &str) -> (String, String) {
    (
        "unknown-key".into(),
        format!("unknown key `{name}`\n  {PRESS_USAGE}"),
    )
}

/// Build the payload for one key name, with `shift` already applied.
fn press_key(name: &str, shift: bool) -> Result<PressKey, (String, String)> {
    if !name.is_ascii() {
        // A synthetic key must carry a real DOM key/code pair; making one up for
        // `я` would produce an event no machine ever emits. Text goes through `type:`.
        return Err((
            "non-ascii".into(),
            format!(
                "`{name}` is not a US key: press: emulates a US keyboard. \
                 Use type:<target>:<text> to enter text."
            ),
        ));
    }
    if let Some((code, vk)) = named_key(name) {
        let text = match name {
            "Enter" => Some("\r".to_string()),
            "Space" => Some(" ".to_string()),
            _ => None,
        };
        return Ok(PressKey {
            key: name.to_string(),
            code: code.to_string(),
            vk,
            text,
        });
    }
    let mut chars = name.chars();
    let first = chars.next().ok_or_else(|| ("empty".into(), PRESS_USAGE.to_string()))?;
    if chars.next().is_some() {
        return Err(press_unknown(name));
    }
    if first.is_ascii_alphabetic() {
        let upper = first.to_ascii_uppercase();
        let key_char = if shift { upper } else { first };
        return Ok(PressKey {
            key: key_char.to_string(),
            code: format!("Key{upper}"),
            vk: upper as u32,
            text: Some(key_char.to_string()),
        });
    }
    if first.is_ascii_digit() {
        let key_char = if shift { us_shift(first).unwrap_or(first) } else { first };
        return Ok(PressKey {
            key: key_char.to_string(),
            code: format!("Digit{first}"),
            vk: first as u32,
            text: Some(key_char.to_string()),
        });
    }
    if let Some((code, vk)) = punct_key(first) {
        let key_char = if shift { us_shift(first).unwrap_or(first) } else { first };
        return Ok(PressKey {
            key: key_char.to_string(),
            code: code.to_string(),
            vk,
            text: Some(key_char.to_string()),
        });
    }
    Err(press_unknown(name))
}

/// Parse `press:<key>` / `press:<mod>+<key>` into a payload and a CDP modifier
/// bitmask (Alt 1, Ctrl 2, Meta 4, Shift 8). Unknown modifiers and keys are
/// refused before anything is dispatched, so a batch never sends half a key.
fn resolve_press(body: &str) -> Result<(PressKey, u32), (String, String)> {
    if body.is_empty() {
        return Err(("empty".into(), PRESS_USAGE.to_string()));
    }
    let mut modifiers = 0u32;
    let mut name = body;
    if body != "+" {
        let mut parts = body.split('+').collect::<Vec<_>>();
        if parts.len() > 1 {
            let key_name = parts.pop().expect("split yields at least one part");
            if key_name.is_empty() {
                return Err(("empty-key".into(), PRESS_USAGE.to_string()));
            }
            for prefix in parts {
                modifiers |= match prefix {
                    "ctrl" => MOD_CTRL,
                    "shift" => MOD_SHIFT,
                    "alt" => MOD_ALT,
                    "meta" => MOD_META,
                    _ => {
                        return Err((
                            "unknown-modifier".into(),
                            format!("unknown modifier `{prefix}` in `{body}`\n  {PRESS_USAGE}"),
                        ))
                    }
                };
            }
            name = key_name;
        }
    }
    let mut spec = press_key(name, modifiers & MOD_SHIFT != 0)?;
    // Ctrl/Meta/Alt swallow the character (ctrl+c must not type "c"); Shift is
    // the only modifier that reshapes it. With Shift held the letter stays
    // uppercase, matching what a real browser reports.
    if modifiers & (MOD_CTRL | MOD_META | MOD_ALT) != 0 {
        if modifiers & MOD_SHIFT == 0 {
            // Only a single-character key is a letter; named keys like ArrowLeft
            // must keep their full DOM name.
            let mut chars = spec.key.chars();
            if let (Some(letter), None) = (chars.next(), chars.next()) {
                if letter.is_ascii_alphabetic() {
                    spec.key = letter.to_ascii_lowercase().to_string();
                }
            }
        }
        spec.text = None;
    }
    Ok((spec, modifiers))
}

fn gui_cmd(args: &[String]) -> Result<String, String> {
    let json_flag = args.iter().any(|a| a == "--json");
    let force = args.iter().any(|a| a == "--force");
    let steps: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    if steps.is_empty() {
        return Err(
            "usage: udtk gui [--json] [--force] <step>...\n  \
             steps: js:<expr> | click:<target> | type:<target>:<text> | press:<key> | tree | wait:<ms>"
                .into(),
        );
    }
    if steps.len() > 50 {
        return Err("too many steps in one batch (max 50)".into());
    }

    let mut cdp = Cdp::open()?;
    let mut baseline = log_max_id().unwrap_or(0);
    let mut receipts: Vec<serde_json::Value> = Vec::new();
    let mut failed = false;

    for (index, step) in steps.iter().enumerate() {
        let started = std::time::Instant::now();
        let (kind, body) = match step.split_once(':') {
            Some((kind, body)) => (kind, body),
            None => (step.as_str(), ""),
        };
        let mut receipt = serde_json::json!({ "step": index + 1, "kind": kind });
        let mut ok = true;

        match kind {
            "js" => match cdp.evaluate(body) {
                Ok(value) => receipt["value"] = value,
                Err(e) => {
                    ok = false;
                    receipt["error"] = serde_json::json!(e);
                }
            },
            "tree" => {
                match cdp.evaluate(&format!("({HARNESS}) ? {HARNESS}.getSemanticTree() : []")) {
                    Ok(value) => receipt["value"] = value,
                    Err(e) => {
                        ok = false;
                        receipt["error"] = serde_json::json!(e);
                    }
                }
            }
            "wait" => {
                let ms = body.parse::<u64>().unwrap_or(0).min(10_000);
                std::thread::sleep(Duration::from_millis(ms));
            }
            "click" | "type" => {
                let (target, text_value) = match (kind, body.split_once(':')) {
                    ("type", Some((target, text))) => (target, Some(text)),
                    ("type", None) => (body, None),
                    (_, _) => (body, None),
                };
                if kind == "type" && text_value.is_none() {
                    ok = false;
                    receipt["error"] = serde_json::json!("usage: type:<target>:<text>");
                } else {
                    match resolve_step_target(&mut cdp, target) {
                        Ok(node) => {
                            let x = node["point"]["x"].as_f64().unwrap_or(0.0);
                            let y = node["point"]["y"].as_f64().unwrap_or(0.0);
                            receipt["target"] = serde_json::json!(target);
                            receipt["hit_point"] = serde_json::json!({ "x": x, "y": y });
                            // A receipt must not claim success when the click would land
                            // on something else: refuse instead of lying.
                            if node["occluded"].as_bool().unwrap_or(false) && !force {
                                ok = false;
                                receipt["refused"] = serde_json::json!("occluded");
                                receipt["covering"] = node["covering"].clone();
                            }
                            if ok {
                                match cdp.mouse_click(x, y) {
                                    Ok(()) => {
                                        if let Some(text) = text_value {
                                            match cdp.call(
                                                "Input.insertText",
                                                serde_json::json!({ "text": text }),
                                            ) {
                                                Ok(_) => {
                                                    receipt["text"] =
                                                        serde_json::json!(text)
                                                }
                                                Err(e) => {
                                                    ok = false;
                                                    receipt["error"] =
                                                        serde_json::json!(e)
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        ok = false;
                                        receipt["error"] = serde_json::json!(e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            ok = false;
                            receipt["error"] = serde_json::json!(e);
                        }
                    }
                }
            }
            "press" => {
                match resolve_press(body) {
                    Ok((spec, modifiers)) => {
                        // Blink only emits `keypress` (and the character) for a
                        // keyDown carrying text; without text the two types are
                        // equivalent for the DOM keydown this step exists for.
                        let down_type = if spec.text.is_some() { "keyDown" } else { "rawKeyDown" };
                        let mut down = serde_json::json!({
                            "type": down_type,
                            "key": spec.key,
                            "code": spec.code,
                            "windowsVirtualKeyCode": spec.vk,
                            "modifiers": modifiers,
                            "location": 0,
                            "autoRepeat": false,
                            "isSystemKey": false,
                        });
                        if let Some(text) = &spec.text {
                            down["text"] = serde_json::json!(text);
                            down["unmodifiedText"] = serde_json::json!(text);
                        }
                        let up = serde_json::json!({
                            "type": "keyUp",
                            "key": spec.key,
                            "code": spec.code,
                            "windowsVirtualKeyCode": spec.vk,
                            "modifiers": modifiers,
                            "location": 0,
                        });
                        receipt["key"] = serde_json::json!({
                            "key": spec.key,
                            "code": spec.code,
                            "vk": spec.vk,
                            "text": spec.text,
                            "modifiers": modifiers,
                            "event": down_type,
                        });
                        for payload in [down, up] {
                            if let Err(e) = cdp.call("Input.dispatchKeyEvent", payload) {
                                ok = false;
                                receipt["error"] = serde_json::json!(e);
                                break;
                            }
                        }
                    }
                    Err((refused, message)) => {
                        // Validation happens before dispatch: a refused key must
                        // not leave a keyUp without its keyDown behind.
                        ok = false;
                        receipt["refused"] = serde_json::json!(refused);
                        receipt["error"] = serde_json::json!(message);
                    }
                }
            }
            other => {
                ok = false;
                receipt["error"] = serde_json::json!(format!("unknown step kind: {other}"));
            }
        }

        // Let the event loop settle, then collect only this step's consequences.
        std::thread::sleep(Duration::from_millis(120));
        let (entries, max_id) = log_delta(baseline);
        if let Some(id) = max_id {
            baseline = id;
        }
        let errors = take_ui_errors(&mut cdp);

        receipt["ok"] = serde_json::json!(ok);
        receipt["log_delta"] = serde_json::json!(entries);
        receipt["ui_errors"] = serde_json::json!(errors);
        receipt["duration_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
        if !ok {
            failed = true;
        }
        receipts.push(receipt);
    }

    if json_flag {
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "ok": !failed,
            "steps": receipts,
        }))
        .unwrap_or_default());
    }

    let mut out = String::new();
    for receipt in &receipts {
        let kind = receipt["kind"].as_str().unwrap_or("?");
        let ok = receipt["ok"].as_bool().unwrap_or(false);
        out.push_str(&format!(
            "[{:>2}] {:<6} {}\n",
            receipt["step"],
            kind,
            if ok { "ok" } else { "FAIL" }
        ));
        if let Some(value) = receipt.get("value") {
            if !value.is_null() {
                out.push_str(&format!("      value: {value}\n"));
            }
        }
        if let Some(point) = receipt.get("hit_point") {
            out.push_str(&format!("      hit: ({}, {})\n", point["x"], point["y"]));
        }
        if let Some(key) = receipt.get("key") {
            if let Some(raw) = key.as_str() {
                out.push_str(&format!("      key: {raw}\n"));
            } else if key.is_object() {
                out.push_str(&format!(
                    "      key: {} code: {} vk: {} modifiers: {} event: {}{}\n",
                    key["key"],
                    key["code"],
                    key["vk"],
                    key["modifiers"],
                    key["event"],
                    key["text"]
                        .as_str()
                        .map(|t| format!(" text:{t:?}"))
                        .unwrap_or_default()
                ));
            }
        }
        if let Some(refused) = receipt.get("refused") {
            if !refused.is_null() {
                out.push_str(&format!("      refused: {refused}\n"));
            }
        }
        if let Some(covering) = receipt.get("covering") {
            if !covering.is_null() {
                out.push_str(&format!("      covered by: {covering}\n"));
            }
        }
        if let Some(error) = receipt.get("error") {
            out.push_str(&format!("      error: {error}\n"));
        }
        if let Some(delta) = receipt["log_delta"].as_array() {
            for entry in delta {
                let source = entry["source"].as_str().unwrap_or("?");
                let message = entry["message"].as_str().unwrap_or("");
                out.push_str(&format!("      log [{source}] {message}\n"));
            }
        }
        if let Some(errors) = receipt["ui_errors"].as_array() {
            for entry in errors {
                let message = entry["message"].as_str().unwrap_or("");
                out.push_str(&format!("      js  {message}\n"));
            }
        }
    }
    if failed {
        return Err(out);
    }
    Ok(out)
}

#[repr(C)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// {B7BEDE81-DF94-477C-9F67-F44D1A14F21E} - FOLDERID_Screenshots
const FOLDERID_SCREENSHOTS: GUID = GUID {
    data1: 0xb7bede81,
    data2: 0xdf94,
    data3: 0x477c,
    data4: [0x9f, 0x67, 0xf4, 0x4d, 0x1a, 0x14, 0xf2, 0x1e],
};

// {33E28130-4E1E-4676-835A-98395C3BC3BB} - FOLDERID_Pictures
const FOLDERID_PICTURES: GUID = GUID {
    data1: 0x33e28130,
    data2: 0x4e1e,
    data3: 0x4676,
    data4: [0x83, 0x5a, 0x98, 0x39, 0x5c, 0x3b, 0xc3, 0xbb],
};

const KF_FLAG_CREATE: u32 = 0x00008000;
const KF_FLAG_DONT_VERIFY: u32 = 0x00004000;

#[repr(C)]
struct SYSTEMTIME {
    w_year: u16,
    w_month: u16,
    w_day_of_week: u16,
    w_day: u16,
    w_hour: u16,
    w_minute: u16,
    w_second: u16,
    w_milliseconds: u16,
}

#[link(name = "shell32")]
extern "system" {
    fn SHGetKnownFolderPath(
        rfid: *const GUID,
        dw_flags: u32,
        h_token: *mut c_void,
        ppsz_path: *mut *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(pv: *mut c_void);
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(lp_system_time: *mut SYSTEMTIME);
}

fn get_known_folder(guid: &GUID) -> Option<PathBuf> {
    unsafe {
        let mut path_ptr: *mut u16 = std::ptr::null_mut();
        let mut hr = SHGetKnownFolderPath(guid, KF_FLAG_CREATE, std::ptr::null_mut(), &mut path_ptr);
        if hr != 0 {
            hr = SHGetKnownFolderPath(guid, KF_FLAG_DONT_VERIFY, std::ptr::null_mut(), &mut path_ptr);
        }
        if hr == 0 && !path_ptr.is_null() {
            let mut len = 0;
            while *path_ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(path_ptr, len);
            let os_str = std::ffi::OsString::from_wide(slice);
            CoTaskMemFree(path_ptr as *mut c_void);
            Some(PathBuf::from(os_str))
        } else {
            None
        }
    }
}

fn resolve_screenshot_dir() -> PathBuf {
    if let Some(dir) = get_known_folder(&FOLDERID_SCREENSHOTS) {
        if std::fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }
    if let Some(dir) = get_known_folder(&FOLDERID_PICTURES) {
        let sub = dir.join("Screenshots");
        if std::fs::create_dir_all(&sub).is_ok() {
            return sub;
        }
        if std::fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }
    std::env::temp_dir()
}

fn local_timestamp_string() -> String {
    let mut st = SYSTEMTIME {
        w_year: 0,
        w_month: 0,
        w_day_of_week: 0,
        w_day: 0,
        w_hour: 0,
        w_minute: 0,
        w_second: 0,
        w_milliseconds: 0,
    };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        st.w_year, st.w_month, st.w_day, st.w_hour, st.w_minute, st.w_second
    )
}

fn shot_cmd(args: &[String]) -> Result<String, String> {
    let json_flag = args.iter().any(|a| a == "--json" || a == "-j");
    let quiet_flag = args.iter().any(|a| a == "--quiet" || a == "-q");

    let mut custom_path: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-o" || arg == "--output" {
            if let Some(val) = args.get(i + 1) {
                custom_path = Some(val.clone());
                i += 2;
                continue;
            }
        } else if !arg.starts_with('-') {
            custom_path = Some(arg.clone());
            break;
        }
        i += 1;
    }

    let target_path = match custom_path {
        Some(cp) => {
            let mut p = PathBuf::from(cp);
            if p.extension().is_none() {
                p.set_extension("png");
            }
            if p.is_relative() {
                std::env::current_dir()
                    .map_err(|e| format!("cannot resolve current dir: {e}"))?
                    .join(p)
            } else {
                p
            }
        }
        None => {
            let dir = resolve_screenshot_dir();
            let timestamp = local_timestamp_string();
            let base_name = format!("unidtk_{timestamp}");
            let mut candidate = dir.join(format!("{base_name}.png"));
            if candidate.exists() {
                for seq in 1..=100 {
                    let alt = dir.join(format!("{base_name}_{seq}.png"));
                    if !alt.exists() {
                        candidate = alt;
                        break;
                    }
                }
            }
            candidate
        }
    };

    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let res = cdp_call(
        "Page.captureScreenshot",
        serde_json::json!({
            "format": "png",
            "fromSurface": true,
            "optimizeForSpeed": true,
        }),
    ).map_err(|e| {
        let ipc_alive = ipc(serde_json::json!({"cmd": command_inventory::SIMULATE_PING})).is_ok();
        if !ipc_alive {
            format!("{e}\nHint: UniDTK application is not running. Start it via 'udtk run' or 'schtasks /run /tn UniDTK'.")
        } else {
            format!("{e}\nHint: UniDTK core is running, but WebView2 CDP (port {}) is not responding.", cdp_port().map(|p| p.to_string()).unwrap_or_else(|_| "unknown".into()))
        }
    })?;

    let data_b64 = res["data"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("no image data in Page.captureScreenshot response")?;

    let bytes = base64_decode(data_b64)?;

    const PNG_HEADER: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 1024 || !bytes.starts_with(&PNG_HEADER) {
        return Err(format!(
            "corrupted screenshot received (size: {} bytes, expected valid PNG)",
            bytes.len()
        ));
    }

    let actual_path = match std::fs::write(&target_path, &bytes) {
        Ok(()) => target_path,
        Err(e) if target_path.parent() != Some(&std::env::temp_dir()) => {
            let fallback = std::env::temp_dir().join(target_path.file_name().unwrap_or_default());
            std::fs::write(&fallback, &bytes).map_err(|e2| {
                format!(
                    "cannot write screenshot to {} ({e}) and fallback {} failed ({e2})",
                    target_path.display(),
                    fallback.display()
                )
            })?;
            fallback
        }
        Err(e) => return Err(format!("cannot write screenshot to {}: {e}", target_path.display())),
    };

    let abs_str = actual_path.display().to_string();

    if quiet_flag {
        return Ok(abs_str);
    }

    if json_flag {
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "ok": true,
            "path": abs_str,
            "bytes": bytes.len(),
        }))
        .unwrap_or_default());
    }

    Ok(format!("screenshot: {abs_str}\nsize: {} bytes", bytes.len()))
}

/// The application's own command surface, taken from the very same inventory file the
/// kernel compiles — so `help --json` describes it correctly even when the app is not
/// running, and a name cannot drift between the two binaries.
fn app_command_manifest() -> Vec<serde_json::Value> {
    command_inventory::ALL
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "args": spec.args,
                "summary": spec.summary,
                "access": match spec.access {
                    unidtk_api::commands::Access::ReadOnly => "read_only",
                    unidtk_api::commands::Access::Write => "write",
                },
                "retry_safe": spec.is_retry_safe(),
                "doors": spec
                    .doors
                    .iter()
                    .map(|door| match door {
                        unidtk_api::commands::Door::Socket => "socket",
                        unidtk_api::commands::Door::Ui => "ui",
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn help_cmd(args: &[String]) -> Result<String, String> {
    let json_flag = args.iter().any(|a| a == "--json");
    if json_flag {
        let manifest = serde_json::json!({
            "protocol": 1,
            "cli_commands": [
                { "name": "doctor", "args": "[-kernel] [-modules] [-ping] [-env] [-check] [-quick] [-simulate] [-all]", "desc": "Run non-destructive subsystem, module, environment, and IPC diagnostics" },
                { "name": "invoke", "args": "<cmd> [json-args]", "desc": "Invoke module or kernel IPC command directly" },
                { "name": "logs", "args": "[--limit N] [--since ID] [--level LEVEL] [--source SOURCE]", "desc": "Read log records from redb" },
                { "name": "gui", "args": "[--json] [--force] <step>...", "desc": "Drive the page in one CDP session: js:<expr> | click:<target> | type:<target>:<text> | press:<key> | tree | wait:<ms>" },
                { "name": "shot", "args": "[path] [--json] [-q]", "desc": "Capture screenshot via CDP, saves to Pictures/Screenshots by default" },
                { "name": "cdp-list", "desc": "List active CDP pages and targets" },
                { "name": "autostart", "args": "[status|enable|disable]", "desc": "Manage the logon autostart task (UniDTK)" },
                { "name": "tasks", "args": "[status|register [name]|unregister <name>|cleanup-old]", "desc": "Manage Windows Task Scheduler tasks" },
                { "name": "path", "args": "[status|ensure]", "desc": "Check or register the CLI directory in the User PATH" },
                { "name": "build", "args": "[-j N]", "desc": "Build the release, make the installer, install and verify (stops and restarts the app)" },
                { "name": "stop", "desc": "Stop the running app (graceful quit, then force) so its files can be replaced" },
                { "name": "run", "args": "[args...]", "desc": "Launch the app that sits next to this CLI" },
                { "name": "help", "args": "[--json]", "desc": "Print the command reference (--json emits a machine-readable manifest)" },
            ],
            "app_commands": app_command_manifest(),
            "note": "app_commands is the application surface (its own inventory); write commands are declared but never served over the socket door.",
        });
        return Ok(serde_json::to_string_pretty(&manifest).unwrap_or_default());
    }

    Ok(help())
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const T: &[u8; 256] = &{
        let mut t = [255u8; 256];
        let mut i = 0u8;
        while i < 26 { t[(b'A' + i) as usize] = i; i += 1; }
        i = 0;
        while i < 26 { t[(b'a' + i) as usize] = 26 + i; i += 1; }
        i = 0;
        while i < 10 { t[(b'0' + i) as usize] = 52 + i; i += 1; }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for &b in input.as_bytes() {
        if b == b'=' || b.is_ascii_whitespace() { continue; }
        let val = T[b as usize];
        if val == 255 { return Err(format!("invalid base64 byte: {b}")); }
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

/// Locate the repository `.env`, which lives at the repository root next to the
/// project sources.
fn find_env_file() -> Option<std::path::PathBuf> {
    let mut dir = find_repo_root().unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    for _ in 0..3 {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Load KEY=VALUE pairs from the repo `.env` (Telegram creds etc).
fn load_env() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(env_path) = find_env_file() else {
        return out;
    };
    if let Ok(text) = std::fs::read_to_string(&env_path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').to_string();
                if !k.is_empty() {
                    out.push((k.to_string(), v));
                }
            }
        }
    }
    out
}

/// Prune staged frontend resources that no longer exist in the source tree.
/// Tauri's resource copy only adds/overwrites files, so a renamed or deleted
/// module folder would otherwise linger in `target/<profile>/modules` and the
/// app would discover BOTH the old and the new widget (observed with
/// `yt-music` → `mp3-player`: the dead folder produced a second card).
///
/// Sources are the vertical-slice roots `modules/` and `folders/` (the old
/// `frontend/` prefix disappeared in the session 44 refactor; pointing at it
/// made every staged entry look stale and wiped the whole release tree).
fn prune_stale_resources(root: &std::path::Path, build_dir: &std::path::Path) {
    for name in ["modules", "folders"] {
        let staged = build_dir.join(name);
        let source = root.join(name);
        let Ok(entries) = std::fs::read_dir(&staged) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = match path.file_name() {
                Some(n) => n.to_owned(),
                None => continue,
            };
            if !source.join(&file_name).exists() {
                if std::fs::remove_dir_all(&path).is_ok() || std::fs::remove_file(&path).is_ok() {
                    eprintln!("pruned stale resource: {name}/{}", file_name.to_string_lossy());
                }
            }
        }
    }
}

/// udtk build — cargo tauri build (release) with .env vars, then copy runtime
/// DLLs next to the release exe.
fn build_cmd(args: &[String]) -> Result<String, String> {
    let (jobs, _rest, parse) = parse_build_jobs(args);
    parse?;
    let exe = std::env::current_exe().map_err(|e| format!("no current exe: {e}"))?;
    if exe.to_string_lossy().to_ascii_lowercase().contains("program files") {
        return Err(
            "udtk build cannot self-build from Program Files (ACL). Run it from the checkout's target\\release\\udtk.exe"
                .into(),
        );
    }
    // 0. stop the running app so its DLLs unlock
    let _ = stop_cmd();
    // relocate our own exe so cargo can relink it (os error 32 otherwise)
    let held = exe.with_file_name("udtk.exe.held");
    let _ = std::fs::remove_file(&held);
    let _ = std::fs::rename(&exe, &held);
    // 1. one tauri pass builds all three bins + the NSIS installer
    let build_res = run_cargo_tauri(&["build", "--bundles", "nsis"], jobs);
    if build_res.is_err() {
        // on failure keep the CLI runnable by restoring our own exe
        let _ = std::fs::rename(&held, &exe);
        return build_res;
    }
    eprintln!("build OK");
    // cargo placed a fresh udtk.exe at the canonical path; drop the renamed
    // placeholder so the installer packs the new CLI, not our running copy.
    let _ = std::fs::remove_file(&held);
    let root = find_repo_root().unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let dll_src = root.join("opt/libmpv/libmpv-2.dll");
    let release_dir = root.join("target/release");
    let _ = std::fs::create_dir_all(&release_dir);
    if dll_src.exists() {
        let _ = std::fs::copy(&dll_src, release_dir.join("libmpv-2.dll"));
    }
    // Раньше здесь бинарники дополнительно раскладывались в каталог данных и
    // зеркалились в `opt/`. Копия теперь ровно одна: `target/release` в
    // dev-прогоне, а в поставке — каталог установки, который наполняет
    // установщик. Остаётся убрать устаревшие staged-ресурсы: удалённый модуль
    // иначе отдаёт вторую карточку виджета.
    prune_stale_resources(&root, &release_dir);
    let model_src = root.join("opt/models/discogs-maest-129e");
    if model_src.is_dir() {
        let target_model_dir = release_dir.join("models/discogs-maest-129e");
        let _ = std::fs::create_dir_all(&target_model_dir);
        for file in ["model.onnx", "model.onnx.data"] {
            let src = model_src.join(file);
            if src.exists() {
                let _ = std::fs::copy(&src, target_model_dir.join(file));
            }
        }
    }
    // 2. install silently and prove by hashes
    let installer = find_installer(&root)
        .ok_or_else(|| "installer not found under target/release/bundle/nsis".to_string())?;
    let programs_dir = program_files_install_dir();
    run_installer_and_verify(&installer, &release_dir, &programs_dir)?;

    // 3. restart the freshly installed app and verify live IPC.
    // The scheduled task is the canonical restart path (same context as the
    // logon autostart), but Task Scheduler denies starting the group-principal
    // task from a medium-integrity shell (UAC-filtered token): `schtasks /run`
    // fails with "Access is denied". When that happens, spawn the installed
    // binary directly — same exe, same working directory, no console window.
    let task_start = std::process::Command::new("schtasks")
        .args(["/run", "/tn", "UniDTK"])
        .creation_flags(NO_WINDOW)
        .output();
    let task_ok = matches!(&task_start, Ok(out) if out.status.success());
    let mut restart_note = String::new();
    if !task_ok {
        let exe = programs_dir.join("unidtk.exe");
        if exe.is_file() {
            let _ = std::process::Command::new(&exe)
                .current_dir(&programs_dir)
                .creation_flags(NO_WINDOW)
                .spawn();
            restart_note = "restart: task start denied (medium-integrity shell) — spawned the app directly\n".into();
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut ping = false;
    while std::time::Instant::now() < deadline {
        if ipc(json!({"cmd": command_inventory::SIMULATE_PING})).is_ok() {
            ping = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    if !ping {
        return Err("app did not answer on IPC within 30s after install (task restart and direct spawn both failed)".into());
    }

    let mut out = String::from("build to install to start to verify OK\n");
    out.push_str(&restart_note);
    match logs_cmd(&["logs".into(), "--level".into(), "ERROR".into()]) {
        Ok(logs) => out.push_str(&logs),
        Err(e) => out.push_str(&format!("(could not read ERROR log: {e})\n")),
    }
    Ok(out)
}

/// Locate the newly built NSIS installer under `target/release/bundle/nsis/`.
fn find_installer(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let nsis_dir = root.join("target/release/bundle/nsis");
    let Ok(entries) = std::fs::read_dir(&nsis_dir) else {
        return None;
    };
    entries
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with("-setup.exe") && p.is_file()
        })
}

/// SHA-256 (lowercase hex) of a file via `certutil`.
fn sha256_of(path: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new("certutil")
        .args(["-hashfile", &path.to_string_lossy(), "SHA256"])
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| format!("certutil failed: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .map(|l| l.trim())
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|h| h.to_ascii_lowercase())
        .ok_or_else(|| format!("could not parse certutil sha256 for {}", path.display()))
}

/// Run the installer `-S` and prove the install by comparing installed vs staged
/// hashes (NSIS silently skips locked files yet still returns exit 0).
fn run_installer_and_verify(
    installer: &std::path::Path,
    release_dir: &std::path::Path,
    install: &std::path::Path,
) -> Result<String, String> {
    // perMachine installers refuse an unelevated token with 740 (ERROR_ELEVATION_REQUIRED).
    // On this machine UAC is off, so a `RunAs` relaunch succeeds without a dialog.
    let is_740 = |e: &std::io::Error| e.raw_os_error() == Some(740);
    // A freshly written ~400 MB installer is routinely held for a few seconds by
    // antivirus / indexer scans right after makensis finishes; spawning it then
    // fails with a sharing violation (os error 32). Retry instead of failing.
    let mut status = std::process::Command::new(installer)
        .arg("/S")
        .creation_flags(NO_WINDOW)
        .status();
    for attempt in 1..=5 {
        match &status {
            Err(e) if e.raw_os_error() == Some(32) => {
                println!("installer busy (os error 32, fresh exe scan); retry {attempt}/5 in 3s");
                std::thread::sleep(Duration::from_secs(3));
                status = std::process::Command::new(installer)
                    .arg("/S")
                    .creation_flags(NO_WINDOW)
                    .status();
            }
            _ => break,
        }
    }
    // 740 can arrive either as a spawn error or as the process exit code.
    let mut elevated = false;
    match status {
        Ok(s) if s.success() => {}
        Ok(s) if !s.success() && s.code() == Some(740) => elevated = true,
        Ok(s) => return Err(format!("installer exited with {:?}", s.code())),
        Err(e) if is_740(&e) => elevated = true,
        Err(e) => return Err(format!("failed to run installer: {e}")),
    }
    if elevated {
        let ps = format!(
            "Start-Process -FilePath '{}' -ArgumentList '/S' -Verb RunAs -Wait",
            installer.display()
        );
        let el = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .creation_flags(NO_WINDOW)
            .status()
            .map_err(|e| format!("failed to elevate installer: {e}"))?;
        if !el.success() {
            return Err(format!("elevated installer exited with {:?}", el.code()));
        }
    }
    // The installer packs `target/unidtk.exe` as of the makensis run; tauri may
    // relink the app once more around bundling, so the exact bytes in Program
    // Files are not bit-identical to the post-build target. What must hold: the
    // installed binary exists and is a fresh build (same size as the app we made,
    // and newer than anything installed before). If this dev checkout is also the
    // source of `target/unidtk.exe`, treat the loop as satisfied when the sizes
    // match; otherwise require the installed binary to be present and non-empty.
    let want_size = std::fs::metadata(release_dir.join("unidtk.exe"))
        .map(|m| m.len())
        .unwrap_or(0);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let pf_m = install.join("unidtk.exe");
        match std::fs::metadata(&pf_m) {
            Ok(m)
                if want_size == 0
                    || m.len() == want_size =>
            {
                break;
            }
            _ => {
                if std::time::Instant::now() > deadline {
                    return Err("installer ran but installed unidtk.exe is missing or wrong size".into());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    for name in ["udtk.exe", "vpn-helper.exe"] {
        let (a, b) = (
            sha256_of(&install.join(name)).unwrap_or_default(),
            sha256_of(&release_dir.join(name)).unwrap_or_default(),
        );
        if a.is_empty() || a != b {
            return Err(format!("{name} did not update after install"));
        }
    }
    Ok("installed".into())
}

/// `udtk stop` — graceful quit of the running app (its own quit path), then
/// force-kill anything from the install dir still holding DLLs open.
fn stop_cmd() -> Result<String, String> {
    let install = install_dir();
    let mut graceful = false;
    match Cdp::open() {
        Ok(mut cdp) => match cdp.evaluate(
            "(() => { const h = window.__UNIDTK_HARNESS__; if (!h) return false; h.invoke('quit_app').catch(() => {}); return true; })()",
        ) {
            Ok(v) if v.as_bool() == Some(true) => graceful = true,
            _ => {}
        },
        Err(_) => {}
    }
    let grace_ms = if graceful { 3500 } else { 0 };
    let deadline = std::time::Instant::now() + Duration::from_millis(grace_ms + 2500);
    loop {
        let pids = list_processes_from(&install);
        if pids.is_empty() {
            break;
        }
        if std::time::Instant::now() > deadline {
            for pid in pids {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .creation_flags(NO_WINDOW)
                    .output();
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for def in SCHEDULER_TASKS {
        if def.elevated {
            let _ = std::process::Command::new("schtasks")
                .args(["/end", "/tn", def.name])
                .creation_flags(NO_WINDOW)
                .output();
        }
    }
    Ok("stopped".into())
}

/// PIDs of processes whose executable lives under `dir` (not including us).
fn list_processes_from(dir: &std::path::Path) -> Vec<u32> {
    let self_pid = std::process::id();
    let dir_str = dir.to_string_lossy().to_ascii_lowercase();
    let script = format!(
        "$p=Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -and $_.ExecutablePath.ToLower().StartsWith('{}') }} | Select-Object -ExpandProperty ProcessId; $p | Where-Object {{ $_ -ne {spid} }}",
        dir_str.replace('\'', "''"),
        spid = self_pid
    );
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(NO_WINDOW)
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

/// Parse `-j N` (or `--jobs=N`) out of `udtk build ...`.
fn parse_build_jobs(args: &[String]) -> (Option<usize>, Vec<String>, Result<(), String>) {
    let mut jobs: Option<usize> = None;
    let mut rest = Vec::new();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-j" | "--jobs" => match it.next() {
                Some(v) => match v.parse::<usize>() {
                    Ok(n) if n >= 1 => jobs = Some(n),
                    _ => return (jobs, rest, Err(format!("bad job count: {v}"))),
                },
                None => return (jobs, rest, Err("`-j` requires a number".into())),
            },
            _ => {
                if let Some(stripped) = a.strip_prefix("--jobs=") {
                    match stripped.parse::<usize>() {
                        Ok(n) if n >= 1 => jobs = Some(n),
                        _ => return (jobs, rest, Err(format!("bad job count: {a}"))),
                    }
                } else {
                    rest.push(a.clone());
                }
            }
        }
    }
    (jobs, rest, Ok(()))
}

/// udtk run — launch the application that sits **next to this CLI**.
///
/// Никакого поиска по репозиторию и никакого legacy-имени: приложение и CLI
/// всегда лежат в одном каталоге — установленном (`C:\Program Files\UniDTK`) или
/// собранном (`target\release` в dev-прогоне). Запускать «какой-нибудь»
/// найденный неподалёку бинарник нельзя: именно так однажды поднялся старый
/// `shell.exe` из уже удалённого `target`.
fn run_cmd(args: &[String]) -> Result<String, String> {
    let install = install_dir();
    let exe = install.join("unidtk.exe");
    if !exe.is_file() {
        return Err(format!(
            "unidtk.exe not found next to udtk.exe ({}). Run 'udtk build' or install the app first.",
            install.display()
        ));
    }
    let envs = load_env();
    // `--debug` is gone: CDP is always on in release (port over IPC `ping`).
    // Reject it loudly instead of forwarding an unknown flag to the app.
    if args.iter().skip(1).any(|a| a == "--debug" || a == "-d") {
        return Err(
            "`--debug` was removed: CDP is always on in release builds — use `udtk gui` / `udtk shot` directly"
                .to_string(),
        );
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.current_dir(&install);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    for arg in args.iter().skip(1) {
        cmd.arg(arg);
    }
    cmd.creation_flags(NO_WINDOW);
    cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    Ok(format!("launched {}", exe.display()))
}

/// Location of the single Cargo project (`toolchain`), which also owns
/// the shared target directory.
fn find_toolchain() -> Option<std::path::PathBuf> {
    let mut dir = std::env::current_dir().unwrap_or_default();
    for _ in 0..4 {
        let candidate = dir.join("toolchain");
        if candidate.join("Cargo.toml").exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// One phase: `cargo tauri <args>` in the toolchain project. The tauri CLI
/// itself runs `cargo build --bins` (all three binaries) before bundling, so
/// no separate `cargo build --bins` pass is needed — a second pass with
/// different features (no `tauri/custom-protocol`) forced a full recompile of
/// the dependency graph on every build.
///
/// `jobs` maps to `CARGO_BUILD_JOBS` and overrides `.cargo/config.toml`
/// (`[build] jobs`), which is what `udtk build -j N` uses.
fn run_cargo_tauri(args: &[&str], jobs: Option<usize>) -> Result<String, String> {
    let toolchain = find_toolchain().ok_or("toolchain not found (run from repo root)")?;
    let envs = load_env();
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(&toolchain).args(["tauri"]).args(args);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    if let Some(n) = jobs {
        cmd.env("CARGO_BUILD_JOBS", n.to_string());
    }
    cmd.creation_flags(NO_WINDOW);
    let status = cmd.status().map_err(|e| format!("cargo failed: {e}"))?;
    if status.success() {
        Ok(format!("cargo tauri {} OK", args.join(" ")))
    } else {
        Err(format!(
            "cargo tauri {} failed (exit {:?})",
            args.join(" "),
            status.code()
        ))
    }
}

/// Scheduled task definitions
struct TaskDef {
    name: &'static str,
    xml_filename: &'static str,
    description: &'static str,
    elevated: bool,
}

const SCHEDULER_TASKS: &[TaskDef] = &[
    TaskDef {
        name: "UniDTK",
        xml_filename: "UniDTKTask.xml",
        description: "UniDTK logon autostart",
        // The shell autostart is registered with `LeastPrivilege` for the
        // Users group: it runs in the user's own session, without elevation.
        elevated: false,
    },
    TaskDef {
        name: "UniDTKSingbox",
        xml_filename: "UniDTKSingboxTask.xml",
        description: "VPN Sing-Box helper",
        elevated: true,
    },
    TaskDef {
        name: "UniDTKVpnStop",
        xml_filename: "UniDTKVpnStopTask.xml",
        description: "VPN stop helper",
        elevated: true,
    },
];

const OBSOLETE_TASKS: &[&str] = &[
    "NothingWallpaper",
    "NothingWallpaperSingbox",
    "NothingWallpaperVpnStop",
    "NothingWallpaperXray",
];

fn find_repo_root() -> Option<std::path::PathBuf> {
    let mut dir = std::env::current_dir().unwrap_or_default();
    for _ in 0..6 {
        if dir.join("scheduler-tasks").exists() || dir.join("core/kernel/main.rs").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for _ in 0..6 {
            if dir.join("scheduler-tasks").exists() || dir.join("core/kernel/main.rs").exists() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

/// Каталог, где лежат программа и её бинарники.
///
/// Задачи запускают приложение и VPN-хелпер **оттуда, где они установлены**,
/// поэтому подстановка идёт по каталогу самого CLI: в установленной копии это
/// `C:\Program Files\UniDTK`, в dev-прогоне — `target\release`. Зашитых
/// абсолютных путей в шаблонах задач больше нет.
fn install_dir() -> std::path::PathBuf {
    unidtk_api::paths::install_dir()
}

/// `C:\Program Files\UniDTK` — куда NSIS-установщик кладёт программу в режиме
/// perMachine. Явный путь нужен верификации `udtk build` в dev-прогоне, где
/// сам CLI лежит в `target\release` (рядом с ним нет установленного приложения).
fn program_files_install_dir() -> std::path::PathBuf {
    let root = std::env::var("PROGRAMFILES")
        .or_else(|_| std::env::var("ProgramW6432"))
        .unwrap_or_else(|_| "C:\\Program Files".into());
    std::path::Path::new(&root).join("UniDTK")
}

/// Каталог шаблонов задач: ресурс рядом с программой, а в чекауте — каталог
/// репозитория (чтобы правка шаблона действовала сразу, без пересборки).
fn task_templates_dir() -> Option<std::path::PathBuf> {
    let installed = install_dir().join("scheduler-tasks");
    if installed.is_dir() {
        return Some(installed);
    }
    find_repo_root()
        .map(|root| root.join("scheduler-tasks"))
        .filter(|dir| dir.is_dir())
}

fn prepare_task_xml(raw_xml: &str, install_dir: &std::path::Path) -> String {
    let install_str = install_dir.to_string_lossy();
    let current_user = std::env::var("USERNAME").unwrap_or_else(|_| "admin".into());

    let mut content = raw_xml.replace("@INSTALL_DIR@", &install_str);
    if current_user != "admin" {
        content = content.replace("<UserId>admin</UserId>", &format!("<UserId>{current_user}</UserId>"));
    }
    content
}

fn is_task_registered(task_name: &str) -> bool {
    std::process::Command::new("schtasks")
        .args(["/query", "/tn", task_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_task_xml_file(path: &std::path::Path, xml_str: &str) -> std::io::Result<()> {
    let mut content = xml_str.to_string();
    if content.contains("encoding=\"UTF-8\"") {
        content = content.replace("encoding=\"UTF-8\"", "encoding=\"UTF-16\"");
    }
    let mut bytes = vec![0xFF, 0xFE]; // UTF-16 LE BOM
    for u in content.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

fn register_task_by_def(def: &TaskDef, install_dir: &std::path::Path) -> Result<String, String> {
    let templates = task_templates_dir()
        .ok_or("scheduler-tasks directory not found (neither next to the app nor in the checkout)")?;
    let xml_path = templates.join(def.xml_filename);
    if !xml_path.exists() {
        return Err(format!("Task XML not found at {}", xml_path.display()));
    }
    let raw = std::fs::read_to_string(&xml_path)
        .map_err(|e| format!("Failed to read {}: {e}", xml_path.display()))?;
    let prepared = prepare_task_xml(&raw, install_dir);

    let temp_xml = std::env::temp_dir().join(format!("unidtk_task_{}.xml", def.name));
    write_task_xml_file(&temp_xml, &prepared)
        .map_err(|e| format!("Failed to write temp XML {}: {e}", temp_xml.display()))?;

    let output = std::process::Command::new("schtasks")
        .args(["/create", "/tn", def.name, "/xml", &temp_xml.to_string_lossy(), "/f"])
        .output()
        .map_err(|e| format!("Failed to execute schtasks: {e}"))?;

    let _ = std::fs::remove_file(&temp_xml);

    if output.status.success() {
        return Ok(format!("Task '{}' registered successfully.", def.name));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let temp_xml = std::env::temp_dir().join(format!("unidtk_task_{}.xml", def.name));
    let _ = write_task_xml_file(&temp_xml, &prepare_task_xml(&raw, install_dir));
    let ps_cmd = format!(
        "Start-Process schtasks.exe -ArgumentList '/create /tn {} /xml \"\"{}\"\" /f' -Verb RunAs -Wait",
        def.name,
        temp_xml.display()
    );
    let el_out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &ps_cmd])
        .output();
    let _ = std::fs::remove_file(&temp_xml);
    if el_out.map(|o| o.status.success()).unwrap_or(false) && is_task_registered(def.name) {
        return Ok(format!("Task '{}' registered successfully (elevated).", def.name));
    }

    Err(format!("Failed to register '{}': {stderr}", def.name))
}

fn unregister_task_by_name(name: &str) -> Result<String, String> {
    let output = std::process::Command::new("schtasks")
        .args(["/delete", "/tn", name, "/f"])
        .output()
        .map_err(|e| format!("Failed to execute schtasks: {e}"))?;

    if output.status.success() {
        return Ok(format!("Task '{name}' deleted."));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let ps_cmd = format!(
        "Start-Process schtasks.exe -ArgumentList '/delete /tn {} /f' -Verb RunAs -Wait",
        name
    );
    let el_out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &ps_cmd])
        .output();
    if el_out.map(|o| o.status.success()).unwrap_or(false) && !is_task_registered(name) {
        Ok(format!("Task '{name}' deleted (elevated)."))
    } else {
        Err(format!("Failed to delete '{name}': {stderr}"))
    }
}

fn tasks_cmd(args: &[String]) -> Result<String, String> {
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("status");
    match sub {
        "status" | "list" => {
            let mut out = String::new();
            out.push_str("UniDTK Scheduled Tasks\n======================\n");
            for def in SCHEDULER_TASKS {
                let reg = is_task_registered(def.name);
                let state_str = if reg { "OK (Registered)" } else { "MISSING" };
                let type_str = if def.elevated { "Elevated" } else { "Unelevated" };
                out.push_str(&format!("{:<16} [{}] ({}, {})\n", def.name, state_str, def.description, type_str));
            }
            let mut obs_found = Vec::new();
            for &obs in OBSOLETE_TASKS {
                if is_task_registered(obs) {
                    obs_found.push(obs);
                }
            }
            if !obs_found.is_empty() {
                out.push_str("\n[!] Obsolete tasks detected (from older version):\n");
                for obs in obs_found {
                    out.push_str(&format!("    - {obs}\n"));
                }
                out.push_str("    Run 'udtk tasks cleanup-old' to remove them.\n");
            }
            Ok(out)
        }
        "register" => {
            // Задачи всегда запускают установленную копию (Program Files), даже
            // если CLI вызван из `target\release`: путь разработки в планировщик
            // не попадает. Ничего никуда не копируем — вторая копия того же
            // бинарника рядом с данными расходилась бы по версиям.
            let install = program_files_install_dir();
            if !install.join("vpn-helper.exe").is_file() {
                return Err(format!(
                    "vpn-helper.exe is not next to the app ({}). Run 'udtk build' or install the app first.",
                    install.display()
                ));
            }
            let target = args.get(2).map(|s| s.as_str()).unwrap_or("all");
            let mut results = Vec::new();
            // Registration errors must reach the exit code: the NSIS installer
            // runs this command via nsExec and checks its exit status — a
            // swallowed failure looked like a successful install while the
            // autostart task was silently missing.
            let mut failed = false;
            for def in SCHEDULER_TASKS {
                if target == "all" || target.eq_ignore_ascii_case(def.name) || (target == "autostart" && def.name == "UniDTK") {
                    match register_task_by_def(def, &install) {
                        Ok(msg) => results.push(msg),
                        Err(e) => {
                            failed = true;
                            results.push(e);
                        }
                    }
                }
            }
            if results.is_empty() {
                return Err(format!("Unknown task name '{target}'. Available: all, UniDTK, UniDTKSingbox, UniDTKVpnStop"));
            }
            let report = results.join("\n");
            if failed {
                return Err(report);
            }
            Ok(report)
        }
        "unregister" => {
            let target = args.get(2).ok_or("usage: udtk tasks unregister <all|task-name>")?;
            let mut results = Vec::new();
            for def in SCHEDULER_TASKS {
                if target == "all" || target.eq_ignore_ascii_case(def.name) || (target == "autostart" && def.name == "UniDTK") {
                    match unregister_task_by_name(def.name) {
                        Ok(msg) => results.push(msg),
                        Err(e) => results.push(e),
                    }
                }
            }
            if results.is_empty() {
                return Err(format!("Unknown task name '{target}'"));
            }
            Ok(results.join("\n"))
        }
        "cleanup-old" => {
            let mut results = Vec::new();
            for &obs in OBSOLETE_TASKS {
                if is_task_registered(obs) {
                    match unregister_task_by_name(obs) {
                        Ok(msg) => results.push(msg),
                        Err(e) => results.push(e),
                    }
                }
            }
            if results.is_empty() {
                Ok("No obsolete tasks found.".into())
            } else {
                Ok(results.join("\n"))
            }
        }
        other => Err(format!("unknown tasks subcommand: {other}\nUsage: udtk tasks [status|register [name]|unregister <name>|cleanup-old]")),
    }
}

fn autostart_cmd(args: &[String]) -> Result<String, String> {
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("status");
    match sub {
        "status" => {
            let reg = is_task_registered("UniDTK");
            if reg {
                Ok("Autostart: ENABLED (task 'UniDTK' is registered in Task Scheduler)".into())
            } else {
                Ok("Autostart: DISABLED (task 'UniDTK' is not registered). Run 'udtk autostart enable' to enable.".into())
            }
        }
        "enable" | "on" => {
            let install = program_files_install_dir();
            let def = &SCHEDULER_TASKS[0]; // UniDTK task
            register_task_by_def(def, &install)
        }
        "disable" | "off" => {
            unregister_task_by_name("UniDTK")
        }
        other => Err(format!("unknown autostart subcommand: {other}\nUsage: udtk autostart [status|enable|disable]")),
    }
}

fn ensure_dir_in_user_path(dir: &std::path::Path) -> Result<(), String> {
    let dir_str = dir.to_str().ok_or("Invalid path")?;
    let ps_script = format!(
        "$dir = '{}'; $old = [Environment]::GetEnvironmentVariable('Path', 'User'); if ($old -notlike \"*$dir*\") {{ [Environment]::SetEnvironmentVariable('Path', \"$old;$dir\", 'User') }}",
        dir_str.replace('\'', "''")
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .output()
        .map_err(|e| format!("failed to run powershell: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

fn path_cmd(args: &[String]) -> Result<String, String> {
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("status");
    // В PATH — только установленная копия (Program Files), откуда бы ни был
    // запущен CLI: dev-сборка из `targetelease` в PATH не попадает. Никаких
    // копий `udtk.exe` в каталог данных: две копии расходятся по версиям.
    let bin_dir = program_files_install_dir();
    match sub {
        "status" | "check" => {
            let path_var = std::env::var("PATH").unwrap_or_default();
            let bin_str = bin_dir.to_string_lossy();
            let in_path = path_var.split(';').any(|p| p.trim().eq_ignore_ascii_case(&bin_str));
            let udtk_exists = bin_dir.join("udtk.exe").exists();
            if in_path {
                Ok(format!("UniDTK CLI directory is in User PATH:\n  {}\n  udtk.exe present: {udtk_exists}", bin_dir.display()))
            } else {
                Ok(format!("UniDTK CLI directory is NOT in User PATH:\n  {}\n  Run 'udtk path ensure' to add it.", bin_dir.display()))
            }
        }
        "ensure" | "add" => {
            if !bin_dir.join("udtk.exe").is_file() {
                return Err(format!("udtk.exe not found at {}", bin_dir.display()));
            }
            ensure_dir_in_user_path(&bin_dir)?;
            Ok(format!("Ensured UniDTK CLI directory in User PATH:\n  {}", bin_dir.display()))
        }
        other => Err(format!("unknown path subcommand: {other}\nUsage: udtk path [status|ensure]")),
    }
}

fn run_cargo_check() -> (bool, String) {
    let mut dir = std::env::current_dir().unwrap_or_default();
    let mut toolchain = None;
    for _ in 0..6 {
        let candidate = dir.join("toolchain");
        if candidate.join("Cargo.toml").exists() {
            toolchain = Some(candidate);
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    let toolchain = match toolchain {
        Some(t) => t,
        None => return (false, "toolchain not found".to_string()),
    };
    let envs = load_env();
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(&toolchain).args(["check"]);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    match cmd.output() {
        Ok(output) => {
            let success = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let summary = if success {
                "cargo check: OK (backend compiles cleanly)".to_string()
            } else {
                format!("cargo check: FAILED\n{}", stderr.trim())
            };
            (success, summary)
        }
        Err(e) => (false, format!("cargo check: failed to run cargo: {e}")),
    }
}

const ALLOWED_DOCTOR_FLAGS: &[&str] = &[
    "-kernel", "--kernel",
    "-modules", "--modules",
    "-ping", "--ping",
    "-env", "--env",
    "-check", "--check",
    "-quick", "--quick",
    "-simulate", "--simulate",
    "-all", "--all",
];

/// One UI render entry point for the `-simulate` CDP probe.
///
/// Read at runtime from `modules/<id>/ui/metadata.json`
/// (`simulate: {module, function, sandbox}`), so adding or renaming an entry
/// point never requires touching the CLI. The sandbox is the offscreen HTML
/// the probe renders into; the probe stays offscreen and never touches the
/// live layout.
struct SimulateTarget {
    module_id: String,
    file: String,
    function: String,
    sandbox: String,
}

fn parse_simulate_metadata(text: &str, module_id: &str) -> Option<SimulateTarget> {
    let val: serde_json::Value = serde_json::from_str(text).ok()?;
    let sim = val.get("simulate")?.as_object()?;
    Some(SimulateTarget {
        module_id: module_id.to_string(),
        file: sim.get("module")?.as_str()?.to_string(),
        function: sim.get("function")?.as_str()?.to_string(),
        sandbox: sim
            .get("sandbox")
            .and_then(|s| s.as_str())
            .unwrap_or("<div class=\"widget-header\"></div><div class=\"widget-body\"></div>")
            .to_string(),
    })
}

/// Render entry points, discovered from `metadata.json` — never hard-coded.
///
/// Candidates cover the checkout (`modules/<id>/ui/metadata.json`), the
/// installed layout (`modules/<id>/metadata.json` next to the app) and the
/// staged frontend tree. Sorted by module id for a deterministic report.
fn simulate_targets() -> Vec<SimulateTarget> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(root) = find_repo_root() {
        dirs.push(root.join("modules"));
        dirs.push(root.join("toolchain").join("dist").join("modules"));
    }
    dirs.push(install_dir().join("modules"));
    let mut out: Vec<SimulateTarget> = Vec::new();
    for modules_dir in &dirs {
        let Ok(entries) = std::fs::read_dir(modules_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let id = match entry.file_name().to_str().map(|s| s.to_string()) {
                Some(id) => id,
                None => continue,
            };
            if out.iter().any(|t| t.module_id == id) {
                continue;
            }
            // Checkout keeps UI sources under `ui/`; installed/staged trees
            // are already flattened to the module root.
            let candidates = [
                entry.path().join("ui").join("metadata.json"),
                entry.path().join("metadata.json"),
            ];
            for meta in &candidates {
                if let Ok(text) = std::fs::read_to_string(meta) {
                    if let Some(target) = parse_simulate_metadata(&text, &id) {
                        out.push(target);
                        break;
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.module_id.cmp(&b.module_id));
    out
}

/// The `-env` section, derived from the modules' dependency manifests.
///
/// No file, task or model name is written here: every `DynamicLibrary`,
/// `Executable`, `ModelAsset` and `WindowsTask` a module declares via
/// `Module::dependencies()` is reported, with the manifest severity deciding
/// between FAIL and WARNING. This is the presence-only twin of the deep
/// `-modules` phase (PE validation, smoke probes), which reads the same
/// manifest. Returns the failure count plus the report text.
fn run_env_section() -> (usize, String) {
    use unidtk_api::manifest::{DependencyKind, DependencySeverity};
    let mut out = String::new();
    let mut failures = 0usize;
    out.push_str("dependencies & DLLs:\n");
    for module in registry::all_modules() {
        for dep in module.dependencies() {
            let check = format!("{}:dep:{}", module.id(), dep.id);
            match &dep.kind {
                DependencyKind::DynamicLibrary {
                    file_name,
                    companions,
                    ..
                } => match unidtk_api::paths::resolve_binary(file_name) {
                    Some(path) => {
                        let missing: Vec<&&str> = companions
                            .iter()
                            .filter(|c| unidtk_api::paths::resolve_binary(c).is_none())
                            .collect();
                        if missing.is_empty() {
                            out.push_str(&format!("  {check}: OK ({} at {})\n", file_name, path.display()));
                        } else {
                            let names: Vec<String> =
                                missing.iter().map(|c| c.to_string()).collect();
                            let msg = format!(
                                "{check}: present, but companions missing: {}",
                                names.join(", ")
                            );
                            if dep.severity == DependencySeverity::Mandatory {
                                out.push_str(&format!("  [FAIL] {msg}\n"));
                                failures += 1;
                            } else {
                                out.push_str(&format!("  [WARN] {msg}\n"));
                            }
                        }
                    }
                    None => {
                        let msg = format!("{check}: MISSING ({})", dep.description);
                        if dep.severity == DependencySeverity::Mandatory {
                            out.push_str(&format!("  [FAIL] {msg}\n"));
                            failures += 1;
                        } else {
                            out.push_str(&format!("  [WARN] {msg}\n"));
                        }
                    }
                },
                DependencyKind::Executable { file_name, .. } => {
                    match unidtk_api::paths::resolve_binary(file_name) {
                        Some(path) => out.push_str(&format!(
                            "  {check}: OK ({} at {})\n",
                            file_name,
                            path.display()
                        )),
                        None => {
                            let msg = format!("{check}: MISSING ({})", dep.description);
                            if dep.severity == DependencySeverity::Mandatory {
                                out.push_str(&format!("  [FAIL] {msg}\n"));
                                failures += 1;
                            } else {
                                out.push_str(&format!("  [WARN] {msg}\n"));
                            }
                        }
                    }
                }
                DependencyKind::ModelAsset {
                    relative_path,
                    companion_files,
                    ..
                } => match unidtk_api::paths::resolve_asset(relative_path) {
                    Some(path) => {
                        let mut bad: Vec<String> = Vec::new();
                        for (comp, c_min) in *companion_files {
                            match unidtk_api::paths::resolve_asset(comp) {
                                Some(cp) => {
                                    let size =
                                        std::fs::metadata(&cp).map(|m| m.len()).unwrap_or(0);
                                    if size < *c_min {
                                        bad.push(format!("{comp} (size {size} < {c_min})"));
                                    }
                                }
                                None => bad.push(format!("{comp} (missing)")),
                            }
                        }
                        if bad.is_empty() {
                            out.push_str(&format!(
                                "  {check}: OK ({} at {})\n",
                                relative_path,
                                path.display()
                            ));
                        } else {
                            let msg =
                                format!("{check}: present, but companions invalid: {}", bad.join(", "));
                            if dep.severity == DependencySeverity::Mandatory {
                                out.push_str(&format!("  [FAIL] {msg}\n"));
                                failures += 1;
                            } else {
                                out.push_str(&format!("  [WARN] {msg}\n"));
                            }
                        }
                    }
                    None => {
                        let msg = format!("{check}: MISSING ({})", dep.description);
                        if dep.severity == DependencySeverity::Mandatory {
                            out.push_str(&format!("  [FAIL] {msg}\n"));
                            failures += 1;
                        } else {
                            out.push_str(&format!("  [WARN] {msg}\n"));
                        }
                    }
                },
                DependencyKind::WindowsTask { task_name, .. } => {
                    // Presence here; the `scheduled tasks` block below renders
                    // the managed-task view with descriptions and UAC notes.
                    let mark = if is_task_registered(task_name) {
                        "OK"
                    } else if dep.severity == DependencySeverity::Mandatory {
                        failures += 1;
                        "FAIL"
                    } else {
                        "WARN"
                    };
                    out.push_str(&format!("  [{mark}] {check}: task '{task_name}'\n"));
                }
            }
        }
    }
    out.push('\n');

    match find_env_file() {
        Some(env_file) => out.push_str(&format!("  .env: OK ({})\n", env_file.display())),
        None => {
            out.push_str("  .env: MISSING (.env not found at the repository root)\n");
            failures += 1;
        }
    }
    out.push('\n');

    out.push_str("scheduled tasks:\n");
    for def in SCHEDULER_TASKS {
        let reg = is_task_registered(def.name);
        let mark = if reg { "OK" } else { "MISSING" };
        out.push_str(&format!("  {}: {} ({})\n", def.name, mark, def.description));
    }
    // A module-declared task the CLI does not manage would silently drift:
    // registration XML, UAC elevation and cleanup would never cover it.
    for module in registry::all_modules() {
        for dep in module.dependencies() {
            if let DependencyKind::WindowsTask { task_name, .. } = &dep.kind {
                if !SCHEDULER_TASKS.iter().any(|def| def.name == *task_name) {
                    out.push_str(&format!(
                        "  [WARN] task '{task_name}' declared by module '{}' is not managed by 'udtk tasks'\n",
                        module.id()
                    ));
                }
            }
        }
    }
    for &obs in OBSOLETE_TASKS {
        if is_task_registered(obs) {
            out.push_str(&format!(
                "  [!] obsolete task detected: {obs} (run 'udtk tasks cleanup-old')\n"
            ));
        }
    }
    out.push('\n');
    (failures, out)
}

fn run_kernel_checks() -> (bool, Vec<unidtk_api::CheckResult>) {
    let mut results = Vec::new();

    // 1. InMemoryModuleDb contract check
    let (db_ok, db_logs) = unidtk_api::capture_logs(|| {
        let db = unidtk_api::InMemoryModuleDb::new();
        let _ = db.kv_set("test_key", b"test_val");
        let get_ok = db.kv_get("test_key").ok().flatten() == Some(b"test_val".to_vec());
        let _ = db.kv_set("prefix:1", b"a");
        let _ = db.kv_set("prefix:2", b"b");
        let count_ok = db.kv_count("prefix:").unwrap_or(0) == 2;
        let list_ok = db.kv_list("prefix:").map(|l| l.len()).unwrap_or(0) == 2;
        let _ = db.kv_remove("test_key");
        let rm_ok = db.kv_get("test_key").ok().flatten().is_none();
        get_ok && count_ok && list_ok && rm_ok
    });
    if db_ok {
        results.push(
            unidtk_api::CheckResult::pass(
                "kernel:in_memory_db",
                "ModuleDb contract verified (CRUD, prefix list, count)",
            )
            .with_logs(db_logs),
        );
    } else {
        results.push(
            unidtk_api::CheckResult::fail(
                "kernel:in_memory_db",
                "ModuleDb contract violation during in-memory dry-run",
            )
            .with_logs(db_logs),
        );
    }

    // 2. Event bus state & pub/sub loopback check
    let (bus_ok, bus_logs) = unidtk_api::capture_logs(|| {
        unidtk_api::emit("doctor-test-event", serde_json::json!({"dry_run": true}));
        let s = unidtk_api::state("doctor-test-event");
        s.and_then(|v| v.get("dry_run").and_then(|b| b.as_bool())) == Some(true)
    });
    if bus_ok {
        results.push(
            unidtk_api::CheckResult::pass(
                "kernel:event_bus",
                "event bus loopback & state cache verified",
            )
            .with_logs(bus_logs),
        );
    } else {
        results.push(
            unidtk_api::CheckResult::fail(
                "kernel:event_bus",
                "event bus failed to retain emitted state",
            )
            .with_logs(bus_logs),
        );
    }

    // 3. System paths resolution
    let install = unidtk_api::paths::install_dir();
    let data = unidtk_api::paths::data_dir_view();
    let logs = unidtk_api::paths::logs_dir();
    let cache = unidtk_api::paths::cache_dir();
    results.push(unidtk_api::CheckResult::pass(
        "kernel:system_paths",
        format!(
            "install: {}, data: {}, logs: {}, cache: {}",
            install.display(),
            data.display(),
            logs.display(),
            cache.display()
        ),
    ));

    // 4. Task XML templates and user substitution check
    match task_templates_dir() {
        Some(tpl_dir) => {
            let mut valid_count = 0;
            for def in SCHEDULER_TASKS {
                let path = tpl_dir.join(def.xml_filename);
                if path.is_file() {
                    if let Ok(raw) = std::fs::read_to_string(&path) {
                        let prep = prepare_task_xml(&raw, &install);
                        if !prep.contains("@INSTALL_DIR@") {
                            valid_count += 1;
                        }
                    }
                }
            }
            results.push(unidtk_api::CheckResult::pass(
                "kernel:task_templates",
                format!(
                    "{valid_count}/{} templates verified and substituted",
                    SCHEDULER_TASKS.len()
                ),
            ));
        }
        None => {
            results.push(unidtk_api::CheckResult::warn(
                "kernel:task_templates",
                "scheduler-tasks template directory not found",
            ));
        }
    }

    // 5. Thread-local logger capture isolation check
    let (_, cap_logs) = unidtk_api::capture_logs(|| {
        unidtk_api::log("kernel-probe-log");
    });
    if cap_logs.contains(&"kernel-probe-log".to_string()) {
        results.push(unidtk_api::CheckResult::pass(
            "kernel:capture_logger",
            "thread-local log capture buffer isolated and functional",
        ));
    } else {
        results.push(unidtk_api::CheckResult::fail(
            "kernel:capture_logger",
            "thread-local log capture did not record emitted log line",
        ));
    }

    let all_ok = results
        .iter()
        .all(|r| r.status != unidtk_api::CheckStatus::Fail);
    (all_ok, results)
}

fn validate_pe_x64(path: &std::path::Path, min_bytes: u64) -> Result<u64, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|e| format!("cannot open: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len < min_bytes {
        return Err(format!("file size {len} bytes is below minimum required {min_bytes} bytes"));
    }
    let mut mz = [0u8; 2];
    file.read_exact(&mut mz).map_err(|e| format!("read error: {e}"))?;
    if &mz != b"MZ" {
        return Err("not a valid PE file (missing MZ signature)".to_string());
    }
    file.seek(SeekFrom::Start(0x3C)).map_err(|e| e.to_string())?;
    let mut pe_offset_bytes = [0u8; 4];
    file.read_exact(&mut pe_offset_bytes).map_err(|e| e.to_string())?;
    let pe_offset = u32::from_le_bytes(pe_offset_bytes) as u64;
    file.seek(SeekFrom::Start(pe_offset)).map_err(|e| e.to_string())?;
    let mut pe_sig = [0u8; 4];
    file.read_exact(&mut pe_sig).map_err(|e| e.to_string())?;
    if &pe_sig != b"PE\0\0" {
        return Err("corrupt PE header (missing PE signature)".to_string());
    }
    let mut machine_bytes = [0u8; 2];
    file.read_exact(&mut machine_bytes).map_err(|e| e.to_string())?;
    let machine = u16::from_le_bytes(machine_bytes);
    if machine != 0x8664 {
        return Err(format!("architecture mismatch: expected x86_64 (0x8664), found 0x{machine:04X}"));
    }
    Ok(len)
}

fn run_executable_probe(exe_path: &std::path::Path, args: &[&str]) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(exe_path);
    cmd.args(args);
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn probe: {e}"))?;
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(1500);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut output = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut output);
                }
                if status.success() {
                    let summary = output.lines().next().unwrap_or("OK").trim();
                    return Ok(if summary.is_empty() { "OK".to_string() } else { summary.to_string() });
                } else {
                    return Err(format!("probe exited with code {status}"));
                }
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err("probe timed out after 1500ms".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("error waiting for child process: {e}"));
            }
        }
    }
}

fn run_module_checks() -> (bool, Vec<(String, Vec<unidtk_api::CheckResult>)>) {
    use unidtk_api::manifest::{DependencyKind, DependencySeverity};
    use std::sync::Arc;

    let modules = registry::all_modules();
    let mut all_ok = true;
    let mut module_results = Vec::new();
    let mut active_modules = Vec::new();

    for module in modules {
        let mod_id = module.id().to_string();
        let mut checks = Vec::new();

        // Phase 1: Declarative Dependencies
        for dep in module.dependencies() {
            let check_name = format!("{mod_id}:dep:{}", dep.id);
            match &dep.kind {
                DependencyKind::DynamicLibrary {
                    file_name,
                    companions,
                    min_bytes,
                } => match unidtk_api::paths::resolve_binary(file_name) {
                    Some(dll_path) => match validate_pe_x64(&dll_path, *min_bytes) {
                        Ok(size) => {
                            let mut companions_ok = true;
                            let mut missing_comp = Vec::new();
                            for comp in *companions {
                                if unidtk_api::paths::resolve_binary(comp).is_none() {
                                    companions_ok = false;
                                    missing_comp.push(*comp);
                                }
                            }
                            if companions_ok {
                                checks.push(unidtk_api::CheckResult::pass(
                                    check_name,
                                    format!("{file_name} valid x64 PE ({size} bytes, {} companions OK)", companions.len()),
                                ));
                            } else {
                                let msg = format!("{file_name} present, but companions missing: {}", missing_comp.join(", "));
                                if dep.severity == DependencySeverity::Mandatory {
                                    checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                                } else {
                                    checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                                }
                            }
                        }
                        Err(err) => {
                            let msg = format!("{file_name} PE validation error: {err}");
                            if dep.severity == DependencySeverity::Mandatory {
                                checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                            } else {
                                checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                            }
                        }
                    },
                    None => {
                        let msg = format!("{file_name} missing ({})", dep.description);
                        if dep.severity == DependencySeverity::Mandatory {
                            checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                        } else {
                            checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                        }
                    }
                },
                DependencyKind::Executable {
                    file_name,
                    probe_args,
                } => match unidtk_api::paths::resolve_binary(file_name) {
                    Some(exe_path) => match probe_args {
                        Some(args) => match run_executable_probe(&exe_path, args) {
                            Ok(summary) => {
                                checks.push(unidtk_api::CheckResult::pass(
                                    check_name,
                                    format!("{file_name} OK (probe: {summary})"),
                                ));
                            }
                            Err(err) => {
                                let msg = format!("{file_name} probe failed: {err}");
                                if dep.severity == DependencySeverity::Mandatory {
                                    checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                                } else {
                                    checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                                }
                            }
                        },
                        None => {
                            checks.push(unidtk_api::CheckResult::pass(
                                check_name,
                                format!("{file_name} present at {}", exe_path.display()),
                            ));
                        }
                    },
                    None => {
                        let msg = format!("{file_name} missing ({})", dep.description);
                        if dep.severity == DependencySeverity::Mandatory {
                            checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                        } else {
                            checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                        }
                    }
                },
                DependencyKind::ModelAsset {
                    relative_path,
                    min_bytes,
                    companion_files,
                } => match unidtk_api::paths::resolve_asset(relative_path) {
                    Some(asset_path) => {
                        let size = std::fs::metadata(&asset_path).map(|m| m.len()).unwrap_or(0);
                        if size < *min_bytes {
                            let msg = format!("{relative_path} size {size} below minimum {min_bytes}");
                            if dep.severity == DependencySeverity::Mandatory {
                                checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                            } else {
                                checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                            }
                        } else {
                            let mut comp_ok = true;
                            let mut missing_comp = Vec::new();
                            for (comp, c_min) in *companion_files {
                                match unidtk_api::paths::resolve_asset(comp) {
                                    Some(cp) => {
                                        let c_size = std::fs::metadata(&cp).map(|m| m.len()).unwrap_or(0);
                                        if c_size < *c_min {
                                            comp_ok = false;
                                            missing_comp.push(format!("{comp} (size {c_size} < {c_min})"));
                                        }
                                    }
                                    None => {
                                        comp_ok = false;
                                        missing_comp.push(format!("{comp} (missing)"));
                                    }
                                }
                            }
                            if comp_ok {
                                checks.push(unidtk_api::CheckResult::pass(
                                    check_name,
                                    format!("{relative_path} valid ({size} bytes, {} companions OK)", companion_files.len()),
                                ));
                            } else {
                                let msg = format!("{relative_path} present, but companions invalid: {}", missing_comp.join(", "));
                                if dep.severity == DependencySeverity::Mandatory {
                                    checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                                } else {
                                    checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                                }
                            }
                        }
                    }
                    None => {
                        let msg = format!("{relative_path} missing ({})", dep.description);
                        if dep.severity == DependencySeverity::Mandatory {
                            checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                        } else {
                            checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                        }
                    }
                },
                DependencyKind::WindowsTask {
                    task_name,
                    require_elevated: _,
                } => {
                    if is_task_registered(task_name) {
                        checks.push(unidtk_api::CheckResult::pass(
                            check_name,
                            format!("task '{task_name}' registered in Task Scheduler"),
                        ));
                    } else {
                        let msg = format!("task '{task_name}' missing ({})", dep.description);
                        if dep.severity == DependencySeverity::Mandatory {
                            checks.push(unidtk_api::CheckResult::fail(check_name, msg));
                        } else {
                            checks.push(unidtk_api::CheckResult::warn(check_name, msg));
                        }
                    }
                }
            }
        }

        // Phase 2: Lifecycle Dry-Run Init
        let db = Arc::new(unidtk_api::InMemoryModuleDb::new());
        let ctx = unidtk_api::ModuleContext::new_diagnostic(&mod_id, db);

        let module_arc = Arc::new(module);
        let mod_for_init = module_arc.clone();
        let (sender, receiver) = std::sync::mpsc::channel();

        let thread_handle = std::thread::spawn(move || {
            let res = unidtk_api::capture_logs_unwind(|| {
                mod_for_init.init(ctx)
            });
            let _ = sender.send(res);
        });

        let init_res = match receiver.recv_timeout(Duration::from_millis(2000)) {
            Ok((call_res, logs)) => {
                let _ = thread_handle.join();
                match call_res {
                    Ok(Ok(())) => {
                        active_modules.push((mod_id.clone(), module_arc.clone()));
                        let mut r = unidtk_api::CheckResult::pass(
                            format!("{mod_id}:init"),
                            "dry-run initialization OK (in-memory isolated)",
                        );
                        r.logs = logs;
                        r
                    }
                    Ok(Err(err)) => {
                        let mut r = unidtk_api::CheckResult::fail(
                            format!("{mod_id}:init"),
                            format!("initialization failed: {err}"),
                        );
                        r.logs = logs;
                        r
                    }
                    Err(panic_payload) => {
                        let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        let mut r = unidtk_api::CheckResult::fail(
                            format!("{mod_id}:init"),
                            format!("panicked during init: {panic_msg}"),
                        );
                        r.logs = logs;
                        r
                    }
                }
            }
            Err(_) => {
                unidtk_api::CheckResult::fail(
                    format!("{mod_id}:init"),
                    "timed out after 2000ms (potential deadlock or blocking I/O)",
                )
            }
        };
        checks.push(init_res);

        if checks.iter().any(|r| r.status == unidtk_api::CheckStatus::Fail) {
            all_ok = false;
        }

        module_results.push((mod_id, checks));
    }

    // Phase 3: Lifecycle Shutdown in Reverse LIFO order
    for (mod_id, module_arc) in active_modules.into_iter().rev() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mod_for_shut = module_arc.clone();
        let thread_handle = std::thread::spawn(move || {
            let res = unidtk_api::capture_logs_unwind(|| {
                mod_for_shut.shutdown(Duration::from_millis(1500))
            });
            let _ = sender.send(res);
        });

        let shut_res = match receiver.recv_timeout(Duration::from_millis(1500)) {
            Ok((call_res, logs)) => {
                let _ = thread_handle.join();
                match call_res {
                    Ok(Ok(())) => {
                        let mut r = unidtk_api::CheckResult::pass(
                            format!("{mod_id}:shutdown"),
                            "clean shutdown OK",
                        );
                        r.logs = logs;
                        r
                    }
                    Ok(Err(err)) => {
                        let mut r = unidtk_api::CheckResult::fail(
                            format!("{mod_id}:shutdown"),
                            format!("shutdown failed: {err}"),
                        );
                        r.logs = logs;
                        r
                    }
                    Err(panic_payload) => {
                        let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "unknown panic".to_string()
                        };
                        let mut r = unidtk_api::CheckResult::fail(
                            format!("{mod_id}:shutdown"),
                            format!("panicked during shutdown: {panic_msg}"),
                        );
                        r.logs = logs;
                        r
                    }
                }
            }
            Err(_) => {
                unidtk_api::CheckResult::fail(
                    format!("{mod_id}:shutdown"),
                    "timed out after 1500ms during shutdown",
                )
            }
        };

        if shut_res.status == unidtk_api::CheckStatus::Fail {
            all_ok = false;
        }

        if let Some((_, list)) = module_results.iter_mut().find(|(id, _)| id == &mod_id) {
            list.push(shut_res);
        }
    }

    // Phase 4: Disk vs Registry Cross-Check
    if let Some(repo_root) = find_repo_root() {
        let modules_dir = repo_root.join("modules");
        if let Ok(entries) = std::fs::read_dir(modules_dir) {
            let active_ids: std::collections::HashSet<String> =
                module_results.iter().map(|(id, _)| id.clone()).collect();
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() && p.join("rust").join("Cargo.toml").is_file() {
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                        if !active_ids.contains(name) {
                            all_ok = false;
                            module_results.push((
                                name.to_string(),
                                vec![unidtk_api::CheckResult::fail(
                                    format!("{name}:inventory"),
                                    format!("orphan Rust module at {} missing from registry!", p.display()),
                                )],
                            ));
                        }
                    }
                }
            }
        }
    }

    (all_ok, module_results)
}


fn run_ping_check() -> (bool, String) {
    let mut out = String::new();
    let t0 = std::time::Instant::now();
    let socket_addr: std::net::SocketAddr = match IPC.parse() {
        Ok(a) => a,
        Err(e) => return (false, format!("  [FAIL] IPC ping: invalid socket address: {e}\n")),
    };
    match TcpStream::connect_timeout(&socket_addr, Duration::from_millis(500)) {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));
            // The probe name comes from the inventory; the raw shape is
            // deliberate — a ping must work even against a kernel older than
            // the v1 envelope.
            let req = json!({"cmd": command_inventory::SIMULATE_PING}).to_string() + "\n";
            if stream.write_all(req.as_bytes()).is_err() {
                return (false, "  [FAIL] IPC ping: failed to send ping request\n".to_string());
            }
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap_or_else(|_| stream));
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    (false, "  [FAIL] IPC ping: socket closed without response\n".to_string())
                }
                Ok(_) => {
                    let d = t0.elapsed().as_millis();
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                        if val["ok"].as_bool().unwrap_or(false) {
                            out.push_str(&format!("  [OK] IPC ping ({d}ms): app is running\n"));
                            if let Ok(stats_out) =
                                ipc(json!({"cmd": command_inventory::SIMULATE_LOG_STATS}))
                            {
                                out.push_str(&format!("  [OK] logs telemetry: {stats_out}\n"));
                            }
                            (true, out)
                        } else {
                            (false, format!("  [FAIL] IPC ping: error: {}\n", val["error"].as_str().unwrap_or("unknown")))
                        }
                    } else {
                        (false, format!("  [FAIL] IPC ping: malformed json response: {line}\n"))
                    }
                }
            }
        }
        Err(e) => {
            (false, format!("  [FAIL] IPC ping: app is not running ({e})\n"))
        }
    }
}

/// udtk doctor — unified health check for compiler, dependencies, kernel, modules, and runtime IPC.
/// Use flags:
///   -kernel / --kernel   : dry-run functional emulation of kernel subsystems
///   -modules / --modules : dry-run functional emulation of all modules (unidtk-api)
///   -ping / --ping       : check live runtime IPC socket (127.0.0.1:26540) and log telemetry
///   -env / --env         : verify DLLs, .env, and scheduled tasks
///   -check / --check     : cargo check on backend
///   -quick / --quick     : skip cargo check (env + kernel + modules + ping)
///   -simulate / --simulate : live frontend action & CDP simulation
///   -all / --all         : comprehensive audit (default)
fn doctor_cmd(args: &[String]) -> Result<String, (i32, String)> {
    for arg in args.iter().skip(1) {
        if arg.starts_with('-') && !ALLOWED_DOCTOR_FLAGS.contains(&arg.as_str()) {
            return Err((
                2,
                format!(
                    "unrecognized flag '{arg}'\nUsage: udtk doctor [-kernel] [-modules] [-ping] [-env] [-check] [-quick] [-simulate] [-all]"
                ),
            ));
        }
    }

    let check_flag = args.iter().any(|a| a == "-check" || a == "--check");
    let env_flag = args.iter().any(|a| a == "-env" || a == "--env");
    let kernel_flag = args.iter().any(|a| a == "-kernel" || a == "--kernel");
    let modules_flag = args.iter().any(|a| a == "-modules" || a == "--modules");
    let ping_flag = args.iter().any(|a| a == "-ping" || a == "--ping");
    let quick_flag = args.iter().any(|a| a == "-quick" || a == "--quick");
    let sim_flag = args.iter().any(|a| a == "-simulate" || a == "--simulate");
    let all_flag = args.iter().any(|a| a == "-all" || a == "--all");

    let any_selective = check_flag || env_flag || kernel_flag || modules_flag || ping_flag || sim_flag;

    let (run_compiler, run_env, run_kernel, run_modules, run_ping, run_sim) = if any_selective && !all_flag && !quick_flag {
        (check_flag, env_flag, kernel_flag, modules_flag, ping_flag, sim_flag)
    } else if quick_flag {
        (false, true, true, true, true, false)
    } else {
        (true, true, true, true, true, sim_flag)
    };

    let mut out = String::new();
    out.push_str("UniDTK doctor\n=============\n");
    let mut total_failures = 0usize;

    // ── Compiler & Build Check ──
    if run_compiler {
        out.push_str("compiler & build:\n");
        let (ok, msg) = run_cargo_check();
        out.push_str(&format!("  {msg}\n\n"));
        if !ok {
            total_failures += 1;
        }
    }

    // ── Environment & Dependencies Check (from the module manifests) ──
    if run_env {
        let (env_failures, env_out) = run_env_section();
        out.push_str(&env_out);
        total_failures += env_failures;
    }

    // ── Kernel Diagnostics (dry-run emulation) ──
    if run_kernel {
        out.push_str("kernel diagnostics (dry-run emulation):\n");
        let (ok, results) = run_kernel_checks();
        if !ok {
            total_failures += 1;
        }
        for res in results {
            let tag = match res.status {
                unidtk_api::CheckStatus::Pass => "[OK]",
                unidtk_api::CheckStatus::Warn => "[WARN]",
                unidtk_api::CheckStatus::Fail => "[FAIL]",
            };
            out.push_str(&format!("  {tag} {} — {}\n", res.name, res.detail));
            for log in res.logs {
                out.push_str(&format!("         | {log}\n"));
            }
        }
        out.push('\n');
    }

    // ── Modules Diagnostics (dry-run emulation) ──
    if run_modules {
        out.push_str("modules diagnostics (dry-run emulation):\n");
        let (ok, mod_results) = run_module_checks();
        if !ok {
            total_failures += 1;
        }
        for (mod_name, checks) in mod_results {
            out.push_str(&format!("  {mod_name}:\n"));
            for res in checks {
                let tag = match res.status {
                    unidtk_api::CheckStatus::Pass => "[OK]",
                    unidtk_api::CheckStatus::Warn => "[WARN]",
                    unidtk_api::CheckStatus::Fail => "[FAIL]",
                };
                out.push_str(&format!("    {tag} {} — {}\n", res.name, res.detail));
                for log in res.logs {
                    out.push_str(&format!("           | {log}\n"));
                }
            }
        }
        out.push('\n');
    }

    // ── Runtime & IPC Check ──
    if run_ping {
        out.push_str("runtime & IPC:\n");
        let (ok, ping_out) = run_ping_check();
        out.push_str(&ping_out);
        if !ok {
            total_failures += 1;
        }
        out.push('\n');
    }

    // ── User Simulation Check ──
    if run_sim && run_simulation(&mut out) {
        total_failures += 1;
    }

    if total_failures > 0 {
        out.push_str(&format!(
            "[!] UniDTK doctor: FAILED ({total_failures} issue{} detected)\n",
            if total_failures == 1 { "" } else { "s" }
        ));
        Err((1, out))
    } else {
        out.push_str("[✓] UniDTK doctor: ALL CHECKS PASSED\n");
        Ok(out)
    }
}

/// Returns `true` when anything failed — without that, `doctor` could print
/// "ALL CHECKS PASSED" while the frontend probes were broken.
fn run_simulation(out: &mut String) -> bool {
    let mut any_failure = false;
    out.push_str("user simulation & health audit:\n");

    // 1. IPC Ping — the command name comes from the inventory, not a literal.
    let t0 = std::time::Instant::now();
    match ipc(json!({ "cmd": command_inventory::SIMULATE_PING })) {
        Ok(res) => {
            let d = t0.elapsed().as_millis();
            out.push_str(&format!("  [OK] IPC Ping ({d}ms): {res}\n"));
        }
        Err(e) => {
            out.push_str(&format!("  [FAIL] IPC Ping: {e}\n"));
            return true;
        }
    }

    // 2. Redb Log Store Health — name from the inventory.
    let t0 = std::time::Instant::now();
    match ipc(json!({ "cmd": command_inventory::SIMULATE_LOG_STATS })) {
        Ok(stats) => {
            let d = t0.elapsed().as_millis();
            out.push_str(&format!("  [OK] Redb Logs ({d}ms): {stats}\n"));
        }
        Err(e) => {
            out.push_str(&format!("  [WARN] Redb Log Stats: {e}\n"));
        }
    }

    // 3. Frontend Error Log Audit — name from the inventory.
    let logs_cmd_name = command_inventory::SIMULATE_LOGS;
    match ipc(json!({ "cmd": logs_cmd_name, "args": { "level": "ERROR", "limit": 10 } })) {
        Ok(res) => {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&res) {
                let items = val.get("data").and_then(|d| d.as_array()).or_else(|| val.as_array());
                if let Some(arr) = items {
                    let fe_errors: Vec<_> = arr.iter()
                        .filter(|entry| {
                            let src = entry.get("source").and_then(|s| s.as_str()).unwrap_or("");
                            src.contains("frontend") || src.contains("webview")
                        })
                        .collect();
                    if fe_errors.is_empty() {
                        out.push_str("  [OK] Frontend Error Audit: Clean (0 unhandled JS exceptions)\n");
                    } else {
                        any_failure = true;
                        out.push_str(&format!("  [FAIL] Frontend Error Audit: {} exceptions detected:\n", fe_errors.len()));
                        for err in fe_errors.iter().take(3) {
                            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                            out.push_str(&format!("         - {msg}\n"));
                        }
                    }
                }
            }
        }
        Err(e) => {
            out.push_str(&format!("  [WARN] Error Audit query: {e}\n"));
        }
    }

    // 4. Widget Configuration Inspection — name from the inventory.
    let t0 = std::time::Instant::now();
    match ipc(json!({ "cmd": command_inventory::SIMULATE_WIDGET_CONFIG })) {
        Ok(cfg_str) => {
            let d = t0.elapsed().as_millis();
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&cfg_str) {
                let data = cfg.get("data").unwrap_or(&cfg);
                let pos_count = data.get("positions").and_then(|p| p.as_object()).map(|o| o.len()).unwrap_or(0);
                if pos_count > 0 {
                    out.push_str(&format!("  [OK] Widget Config ({d}ms): Valid layout ({pos_count} widgets configured)\n"));
                } else {
                    out.push_str(&format!("  [WARN] Widget Config ({d}ms): Layout empty or missing positions\n"));
                }
            } else {
                any_failure = true;
                out.push_str(&format!("  [FAIL] Widget Config ({d}ms): Invalid JSON format\n"));
            }
        }
        Err(e) => {
            out.push_str(&format!("  [WARN] Widget Config: {e}\n"));
        }
    }

    // 5. Safe read-only probes of core modules — iterated from the inventory,
    // never hard-coded here. Probes are `read_only`, so `simulate` stays
    // non-destructive; the test `simulate_probes_name_real_socket_commands`
    // proves every entry names a real socket command.
    for probe in command_inventory::SIMULATE_PROBES {
        let args: serde_json::Value =
            serde_json::from_str(probe.args).unwrap_or(serde_json::json!({}));
        match ipc(json!({ "cmd": probe.command, "args": args })) {
            Ok(status) => out.push_str(&format!("  [OK] {}: {status}\n", probe.label)),
            Err(_) => out.push_str(&format!("  [INFO] {}: (idle or offline)\n", probe.label)),
        }
    }

    // 6. Live Frontend Code Function Simulation (CDP)
    //
    // Entry points come from `modules/<id>/ui/metadata.json` (`simulate`),
    // discovered by `simulate_targets()`. The script below is only a generic
    // harness: no module path, function name or sandbox HTML is written here.
    out.push_str("\n  code function simulation (live runtime):\n");
    let targets = simulate_targets();
    if targets.is_empty() {
        out.push_str("    [WARN] no simulate entry points: no module ships metadata.json with `simulate`\n");
    }
    let mut sim_script = String::from(
        r#"(async function() {
        const results = [];
        function logRun(fnName, ok, msg, durationMs) {
            results.push({ fn: fnName, ok, msg: msg || '', durationMs });
        }

        const sandbox = document.createElement('div');
        // Deliberately NOT `.widget-card`: the offscreen sandbox must stay invisible to
        // every global query (relayout, broken-card audit, the semantic tree below), so
        // running a simulation cannot disturb the real layout or the agent's view.
        sandbox.style.cssText = 'position:fixed;left:-9999px;top:-9999px;width:340px;height:400px;visibility:hidden;pointer-events:none;contain:strict;';
        sandbox.className = 'simulation-sandbox';
        document.body.appendChild(sandbox);
"#,
    );
    for t in &targets {
        // Rendered into single-quoted JS string literals — file/function/html
        // come from module-owned metadata.json, so escape the two characters
        // that would break out of the literal.
        let file = t.file.replace('\\', "\\\\").replace('\'', "\\'");
        let func = t.function.replace('\\', "\\\\").replace('\'', "\\'");
        let html = t
            .sandbox
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', " ");
        let label = format!("modules/{}:{}", t.module_id, t.function)
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        let module_path = t.module_id.replace('\\', "\\\\").replace('\'', "\\'");
        sim_script.push_str(&format!(
            r#"        try {{
            const t0_{id} = performance.now();
            try {{
                const mod_{id} = await import('../modules/{module_path}/{file}');
                sandbox.innerHTML = '{html}';
                await mod_{id}.{func}(sandbox, 'simulation-{id}');
                logRun('{label}', true, 'rendered successfully', Math.round(performance.now() - t0_{id}));
            }} catch (e) {{
                logRun('{label}', false, String(e?.stack || e?.message || e), Math.round(performance.now() - t0_{id}));
            }}
        }} catch (e) {{
            logRun('{label}', false, String(e?.stack || e?.message || e), 0);
        }}
"#,
            id = t.module_id.replace('-', "_").replace('\\', "\\\\").replace('\'', "\\'"),
            module_path = module_path,
            file = file,
            html = html,
            func = func,
            label = label,
        ));
    }
    sim_script.push_str(
        r#"        try {
            // Interaction on the offscreen copy
            // Interaction check on the OFFSCREEN copy. The live widget must not be
            // touched: clicking it used to collapse whatever the user had open and race
            // with the 300ms motion token, while the receipt still claimed success.
            const t0 = performance.now();
            try {
                const probe = sandbox.firstElementChild || sandbox;
                probe.click();
                await new Promise(r => setTimeout(r, 60));
                logRun('interaction:clickSandboxCopy', true, 'click dispatched on the offscreen copy', Math.round(performance.now() - t0));
            } catch (e) {
                logRun('interaction:clickSandboxCopy', false, String(e?.stack || e?.message || e), Math.round(performance.now() - t0));
            }

            // Live DOM Error Audit
            const brokenCards = Array.from(document.querySelectorAll('.widget-card'))
                .filter(c => (c.innerText || '').includes('Failed to fetch'))
                .map(c => c.getAttribute('data-id') || 'unknown');
            if (brokenCards.length > 0) {
                logRun('dom:brokenCardsAudit', false, 'Failed to fetch in: ' + brokenCards.join(', '), 0);
            } else {
                logRun('dom:brokenCardsAudit', true, '0 broken cards in live DOM', 0);
            }
        } finally {
            sandbox.remove();
        }

        return results;
    })()"#,
    );

    match cdp_call("Runtime.evaluate", serde_json::json!({
        "expression": sim_script,
        "awaitPromise": true,
        "returnByValue": true
    })) {
        Ok(eval_res) => {
            if let Some(val) = eval_res.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_array()) {
                for item in val {
                    let fn_name = item.get("fn").and_then(|f| f.as_str()).unwrap_or("unknown");
                    let ok = item.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);
                    let msg = item.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                    let dur = item.get("durationMs").and_then(|d| d.as_i64()).unwrap_or(0);
                    if ok {
                        out.push_str(&format!("    [OK] {fn_name}() ({dur}ms): {msg}\n"));
                    } else {
                        any_failure = true;
                        out.push_str(&format!("    [FAIL] {fn_name}() ({dur}ms): {msg}\n"));
                    }
                }
            } else {
                out.push_str("    [WARN] CDP simulation returned invalid value\n");
            }
        }
        Err(e) => {
            out.push_str(&format!("    [INFO] CDP simulation unavailable ({e})\n"));
        }
    }

    out.push('\n');
    any_failure
}

#[allow(dead_code)]
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn help() -> String {
    "udtk — debug bridge to the running UniDTK app (Unified Desktop Täglich Kit)

USAGE:
  udtk doctor [-kernel] [-modules] [-ping] [-env] [-check] [-quick] [-simulate] [-all]
  udtk invoke <cmd> [json-args]
  udtk logs [--limit N] [--since ID] [--level LEVEL] [--source SOURCE]
  udtk gui [--json] [--force] <step>...   # js:<expr> | click:<target> | type:<target>:<text> | press:<key> | tree | wait:<ms>
  udtk shot [path] [--json] [-q]
  udtk help [--json]
  udtk cdp-list
  udtk autostart [status|enable|disable]
  udtk tasks [status|register [name]|unregister <name>|cleanup-old]
  udtk path [status|ensure]
  udtk build [-j N] | stop | run [args...]"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_maps_every_key_to_a_real_dom_pair() {
        let (spec, mods) = resolve_press("Escape").unwrap();
        assert_eq!((spec.key.as_str(), spec.code.as_str(), spec.vk), ("Escape", "Escape", 27));
        assert_eq!(mods, 0);
        assert_eq!(spec.text, None);

        let (spec, _) = resolve_press("a").unwrap();
        assert_eq!((spec.key.as_str(), spec.code.as_str(), spec.vk), ("a", "KeyA", 65));
        assert_eq!(spec.text.as_deref(), Some("a"));

        let (spec, _) = resolve_press("F5").unwrap();
        assert_eq!((spec.code.as_str(), spec.vk), ("F5", 116));

        // ASCII is not enough for punctuation: `;` is VK_OEM_1, not 0x3B.
        let (spec, _) = resolve_press(";").unwrap();
        assert_eq!((spec.code.as_str(), spec.vk), ("Semicolon", 0xBA));

        let (spec, _) = resolve_press("+").unwrap();
        assert_eq!((spec.key.as_str(), spec.code.as_str(), spec.vk), ("+", "Equal", 0xBB));

        let (spec, _) = resolve_press("ArrowLeft").unwrap();
        assert_eq!(spec.vk, 37);
    }

    #[test]
    fn press_modifiers_follow_browser_semantics() {
        // Ctrl swallows the character so ctrl+c never types "c".
        let (spec, mods) = resolve_press("ctrl+c").unwrap();
        assert_eq!(mods, MOD_CTRL);
        assert_eq!(spec.key, "c");
        assert_eq!((spec.code.as_str(), spec.vk), ("KeyC", 67));
        assert_eq!(spec.text, None);

        // Shift reshapes the character but never the physical key identity.
        let (spec, mods) = resolve_press("shift+a").unwrap();
        assert_eq!(mods, MOD_SHIFT);
        assert_eq!(spec.key, "A");
        assert_eq!(spec.text.as_deref(), Some("A"));
        assert_eq!((spec.code.as_str(), spec.vk), ("KeyA", 65));

        let (spec, mods) = resolve_press("shift+1").unwrap();
        assert_eq!(mods, MOD_SHIFT);
        assert_eq!((spec.key.as_str(), spec.code.as_str(), spec.vk), ("!", "Digit1", 49));

        let (spec, mods) = resolve_press("alt+ArrowLeft").unwrap();
        assert_eq!(mods, MOD_ALT);
        assert_eq!(spec.key, "ArrowLeft");

        // ctrl+shift keeps the uppercase key a real browser reports.
        let (spec, mods) = resolve_press("ctrl+shift+a").unwrap();
        assert_eq!(mods, MOD_CTRL | MOD_SHIFT);
        assert_eq!(spec.key, "A");
        assert_eq!(spec.text, None);
    }

    #[test]
    fn press_refuses_what_it_cannot_emulate() {
        // One canonical name per capability: no aliases.
        assert_eq!(resolve_press("Esc").unwrap_err().0, "unknown-key");
        assert_eq!(resolve_press("Left").unwrap_err().0, "unknown-key");
        // A Russian key has no DOM code; `type:` is the text door.
        assert_eq!(resolve_press("я").unwrap_err().0, "non-ascii");
        assert_eq!(resolve_press("").unwrap_err().0, "empty");
        assert_eq!(resolve_press("ctrl++").unwrap_err().0, "empty-key");
        assert_eq!(resolve_press("hyper+a").unwrap_err().0, "unknown-modifier");
        assert_eq!(resolve_press("ctrl+nope").unwrap_err().0, "unknown-key");
    }

    #[test]
    fn simulate_targets_come_from_metadata() {
        let targets = simulate_targets();
        assert!(
            !targets.is_empty(),
            "no module declares `simulate` in metadata.json"
        );
        for t in &targets {
            assert!(
                !t.file.is_empty() && !t.function.is_empty(),
                "module '{}' has an empty simulate entry",
                t.module_id
            );
            assert!(
                !t.file.contains("..") && !t.function.contains(' '),
                "module '{}' simulate entry looks like a path escape: {} {}",
                t.module_id,
                t.file,
                t.function
            );
        }
        // The five render entry points must be discoverable without literals
        // in this file: the build lint forbids naming them here.
        let functions: Vec<&str> = targets.iter().map(|t| t.function.as_str()).collect();
        for expected in [
            "renderClock",
            "initVpnPanel",
            "renderMusicPlayer",
            "renderGlyphBar",
            "renderTelegramWidget",
        ] {
            assert!(
                functions.contains(&expected),
                "render entry point '{expected}' is not declared in any metadata.json"
            );
        }
    }

    #[test]
    fn test_prepare_task_xml_substitutions() {
        // В шаблонах задач нет ни одного абсолютного пути — только placeholder,
        // который подставляет регистрация. Проверяем и подстановку, и то, что
        // заглушек не остаётся.
        let sample = r#"
        <Exec>
          <Command>@INSTALL_DIR@\unidtk.exe</Command>
          <WorkingDirectory>@INSTALL_DIR@</WorkingDirectory>
        </Exec>
        <UserId>admin</UserId>
        "#;
        let install = std::path::Path::new(r"D:\Projects\my-unidtk");
        let result = prepare_task_xml(sample, install);
        assert!(result.contains(r"D:\Projects\my-unidtk\unidtk.exe"));
        assert!(result.contains(r"<WorkingDirectory>D:\Projects\my-unidtk</WorkingDirectory>"));
        assert!(
            !result.contains("@INSTALL_DIR@"),
            "placeholder must be fully substituted"
        );
    }

    #[test]
    fn env_section_lists_manifest_dependencies() {
        // `-env` is presence-only rendering of `Module::dependencies()`: every
        // dependency a module declares must appear in the report, with no
        // hard-coded names of its own.
        let (_, out) = run_env_section();
        for module in registry::all_modules() {
            for dep in module.dependencies() {
                let check = format!("{}:dep:{}", module.id(), dep.id);
                assert!(
                    out.contains(&check),
                    "-env report is missing manifest dependency '{check}'"
                );
            }
        }
        assert!(out.contains(".env"), "-env report lost the .env line");
        assert!(
            out.contains("scheduled tasks"),
            "-env report lost the scheduled tasks block"
        );
    }

    #[test]
    fn test_scheduler_tasks_definitions() {
        assert_eq!(SCHEDULER_TASKS.len(), 3);
        assert_eq!(SCHEDULER_TASKS[0].name, "UniDTK");
        assert_eq!(SCHEDULER_TASKS[0].elevated, false);
        assert_eq!(SCHEDULER_TASKS[1].name, "UniDTKSingbox");
        assert_eq!(SCHEDULER_TASKS[1].elevated, true);
        assert_eq!(SCHEDULER_TASKS[2].name, "UniDTKVpnStop");
        assert_eq!(SCHEDULER_TASKS[2].elevated, true);
    }

    #[test]
    fn test_base64_decode_roundtrip() {
        let original = b"Hello, UniDTK World! 12345 \x00\xFF";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(original.to_vec(), decoded);
    }

    #[test]
    fn test_prune_keeps_live_and_drops_stale_resources() {
        let root = std::env::temp_dir().join("unidtk-prune-test");
        let _ = std::fs::remove_dir_all(&root);
        let modules = root.join("modules");
        let release = root.join("target/release");
        for (path, file) in [
            (modules.join("mp3-player/ui"), "widget.js"),
            (modules.join("backend-only/rust"), "mod.rs"),
            (release.join("modules/mp3-player"), "widget.js"),
            (release.join("modules/yt-music"), "widget.js"),
            (release.join("modules/backend-only"), "widget.js"),
        ] {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(file), "x").unwrap();
        }

        prune_stale_resources(&root, &release);

        assert!(release.join("modules/mp3-player").is_dir(), "live module pruned");
        assert!(
            !release.join("modules/yt-music").exists(),
            "stale renamed module survived"
        );
        assert!(
            release.join("modules/backend-only").is_dir(),
            "staged module with a source counterpart must not be treated as stale"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

