//! Explicit native inspector client. A failed native command never falls through to CDP.

use std::io::Write;
use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};
use base64::Engine;
use serde_json::{json, Value};
#[path = "../../core/desktop/protocol.rs"]
mod protocol;

pub fn host_cmd(args: &[String]) -> Result<String, String> {
    let sub = args.get(1).map(String::as_str).unwrap_or("ping");
    let force = args.iter().any(|a| a == "--force");
    let reply = match sub {
        "run" => return run(),
        "ping" | "tree" | "frames" => request(json!({ "cmd": sub }))?,
        "shot" => return shot(&[vec!["shot".to_string()], args.iter().skip(2).cloned().collect()].concat()),
        "click" | "hit" => {
            let number = |i: usize| -> Result<f64, String> {
                args.get(i).and_then(|v| v.parse::<f64>().ok()).filter(|n| n.is_finite())
                    .ok_or_else(|| format!("usage: udtk host {sub} <x> <y> [--force] (points)"))
            };
            let (x, y) = (number(2)?, number(3)?);
            let mut body = json!({ "cmd": sub, "args": { "x": x, "y": y, "force": force } });
            if sub == "click" { body["args"]["token"] = json!(token()?); }
            request(body)?
        }
        "quit" => quit()?,
        _ => return Err("usage: udtk host [run|ping|tree|frames|shot [path]|hit <x> <y>|click <x> <y> [--force]|quit]".into()),
    };
    serde_json::to_string_pretty(&reply).map_err(|e| e.to_string())
}

fn run() -> Result<String, String> {
    if request(json!({ "cmd": "ping" })).is_ok() { return Err("a native host is already listening".into()); }
    let install = super::install_dir();
    let exe = install.join("unidtk-desktop.exe");
    if !exe.is_file() { return Err(format!("{} is missing; run udtk build", exe.display())); }
    let mut child = super::spawn_detached(std::process::Command::new(&exe).current_dir(&install).creation_flags(super::NO_WINDOW))
        .map_err(|e| format!("start native host: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? { return Err(format!("native host exited: {status}; see {}", unidtk_api::paths::logs_dir().join("unidtk-desktop.log").display())); }
        if let Ok(ping) = request(json!({ "cmd": "ping" })) {
            if ping["pid"] == child.id() && ping["attached"] == true { return Ok(format!("native host started: {ping}")); }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("native host did not attach within 20 seconds; inspect udtk host ping".into())
}

pub fn quit() -> Result<Value, String> { request(json!({ "cmd": "quit", "args": { "token": token()? } })) }

pub fn gui(args: &[String]) -> Result<String, String> {
    let force = args.iter().any(|a| a == "--force");
    let steps: Vec<_> = args.iter().skip(1).filter(|a| !matches!(a.as_str(), "--host" | "--json" | "--force")).collect();
    if steps.is_empty() || steps.len() > 50 { return Err("usage: udtk gui --host [--json] [--force] tree|click:<key-or-path> ... (max 50 steps)".into()); }
    let mut receipts = Vec::new();
    for step in steps {
        let result = if step.as_str() == "tree" { request(json!({ "cmd": "tree" }))? }
            else if let Some(target) = step.strip_prefix("click:") {
                request(json!({ "cmd": "click", "args": { "target": target, "force": force, "token": token()? } }))?
            } else { return Err(format!("native G0 supports tree and click:<key-or-path>; unsupported step: {step}")); };
        receipts.push(json!({ "step": step, "ok": true, "result": result }));
    }
    serde_json::to_string_pretty(&json!({ "host": "native", "receipts": receipts })).map_err(|e| e.to_string())
}

pub fn shot(args: &[String]) -> Result<String, String> {
    let mut path = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--host" | "--json" | "-j" | "-q" | "--quiet" => {},
            "--output" | "-o" => {
                index += 1;
                path = Some(args.get(index).ok_or("--output needs a path")?.clone());
            }
            option if option.starts_with('-') => return Err(format!("unknown shot option: {option}")),
            value => { if path.replace(value.to_string()).is_some() { return Err("shot accepts one output path".into()); } }
        }
        index += 1;
    }
    let mut path = path.map(std::path::PathBuf::from).unwrap_or_else(|| super::resolve_screenshot_dir().join(format!("unidtk_host_{}_{}.png", super::local_timestamp_string(), std::process::id())));
    if path.extension().is_none() { path.set_extension("png"); }
    if path.is_relative() { path = std::env::current_dir().map_err(|e| e.to_string())?.join(path); }
    let mut reply = request(json!({ "cmd": "png" }))?;
    let encoded = reply["base64"].as_str().ok_or("PNG reply has no image bytes")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| format!("invalid PNG encoding: {e}"))?;
    if bytes.len() > protocol::MAX_PNG || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Err("invalid PNG payload".into()); }
    // The client owns the path. An unauthenticated inspector request never writes a file.
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Err(error) = file.write_all(&bytes) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(error.to_string());
    }
    reply.as_object_mut().unwrap().remove("base64");
    reply["path"] = json!(path.display().to_string());
    if args.iter().any(|arg| arg == "-q" || arg == "--quiet") { return Ok(path.display().to_string()); }
    serde_json::to_string_pretty(&reply).map_err(|e| e.to_string())
}

/// Safe simulation changes only the experimental test card. Forced posting is explicit
/// in the report; no desktop pointer is moved and no production command is invoked.
pub fn doctor(args: &[String]) -> Result<String, String> {
    if args.iter().skip(1).any(|a| !matches!(a.as_str(), "--host" | "-ping" | "--ping" | "-simulate" | "--simulate")) {
        return Err("usage: udtk doctor --host [-ping|-simulate]".into());
    }
    let started = Instant::now();
    let ping = request(json!({ "cmd": "ping" }))?;
    let mut out = format!("UniDTK native host doctor\n  [OK] IPC ping ({} ms): {ping}\n", started.elapsed().as_millis());
    if args.iter().any(|a| a == "-simulate" || a == "--simulate") {
        if ping["ready"] != true { return Err(format!("{out}  [FAIL] first native frame has not been drawn")); }
        for cmd in ["click", "quit"] {
            match request(json!({ "cmd": cmd })) {
                Err(error) if error.contains("requires its per-run token") => out.push_str(&format!("  [OK] unauthenticated {cmd} refused\n")),
                other => return Err(format!("unexpected unauthenticated {cmd} result: {other:?}")),
            }
        }
        match request(json!({ "cmd": "png", "args": { "path": "forbidden.png" } })) {
            Err(error) if error.contains("save the image in the client") => out.push_str("  [OK] host-side PNG path refused\n"),
            other => return Err(format!("unexpected PNG path result: {other:?}")),
        }
        let tree = request(json!({ "cmd": "tree" }))?;
        if !tree.as_array().is_some_and(|nodes| nodes.iter().any(|n| n["key"] == "g0-card")) { return Err("test card missing from element tree".into()); }
        let receipt = request(json!({ "cmd": "click", "args": { "target": "g0-card", "force": true, "token": token()? } }))?;
        if receipt["handled"] != true || receipt["input_count"].as_u64() <= receipt["before"].as_u64() { return Err("Rune did not acknowledge the card press".into()); }
        out.push_str(&format!("  [OK] card press acknowledged (forced Win32 posting): {receipt}\n"));
        let hit = request(json!({ "cmd": "hit", "args": { "x": 1, "y": 1 } }))?;
        if hit["host_is_child"] != true { return Err(format!("native window lost its WS_CHILD style: {hit}")); }
        if hit["in_region"] != false { return Err(format!("blank desktop is intercepted: {hit}")); }
        out.push_str("  [OK] blank desktop point is outside the native input region\n");
        let original_token = token()?;
        let exe = super::install_dir().join("unidtk-desktop.exe");
        let mut duplicate = super::spawn_detached(std::process::Command::new(&exe).creation_flags(super::NO_WINDOW)).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = duplicate.try_wait().map_err(|e| e.to_string())? {
                if status.success() { return Err("duplicate host reported success".into()); }
                break;
            }
            if Instant::now() >= deadline { return Err("duplicate host did not refuse within five seconds".into()); }
            std::thread::sleep(Duration::from_millis(25));
        }
        if token()? != original_token || request(json!({ "cmd": "ping" }))?["pid"] != ping["pid"] { return Err("duplicate host changed the live token or endpoint".into()); }
        out.push_str("  [OK] duplicate start refused; live token and process preserved\n");
    }
    out.push_str(&format!("  [OK] metrics: {}\n", request(json!({ "cmd": "frames" }))?));
    out.push_str("Native inspector checks passed; G0 kernel ownership and GPU readback remain open.");
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub port: u16,
    pub token: String,
}

#[derive(serde::Deserialize)]
struct InspectorMetadata {
    pid: u32,
    port: u16,
    token: String,
    #[allow(dead_code)]
    supervised: bool,
}

fn read_metadata(path: &std::path::Path) -> Result<InspectorMetadata, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {e} (is the host running?)", path.display()))?;
    serde_json::from_str::<InspectorMetadata>(&raw)
        .map_err(|e| format!("{}: bad metadata json: {e}", path.display()))
}

pub(crate) fn probe_kernel_ping() -> Option<Value> {
    use std::io::BufRead;
    let addr: std::net::SocketAddr = super::IPC.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
    writeln!(stream, r#"{{"cmd":"ping"}}"#).ok()?;
    let _ = stream.flush();
    let mut line = String::new();
    if std::io::BufReader::new(stream).read_line(&mut line).ok()? == 0 {
        return None;
    }
    let reply: Value = serde_json::from_str(line.trim()).ok()?;
    if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Some(reply.get("result").cloned().unwrap_or(Value::Null))
    } else {
        None
    }
}

pub(crate) fn resolve_endpoint_in(
    data_dir: &std::path::Path,
    kernel_ping: Option<&Value>,
) -> Result<Endpoint, String> {
    if let Some(ping) = kernel_ping {
        if let Some(host) = ping.get("host") {
            if host.get("mode").and_then(|m| m.as_str()) == Some("native") {
                let pid = host
                    .get("pid")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| "native host mode is active without a host PID".to_string())?;
                let port = host
                    .get("inspectorPort")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u16::try_from(v).ok())
                    .ok_or_else(|| {
                        "native host has not registered its inspector port yet".to_string()
                    })?;
                let meta_path = data_dir.join(format!("host-inspector.{pid}.json"));
                let meta = read_metadata(&meta_path)?;
                if meta.pid != pid || meta.port != port {
                    return Err(format!(
                        "{}: metadata mismatch (pid={}, port={}) vs ping (pid={pid}, port={port})",
                        meta_path.display(),
                        meta.pid,
                        meta.port
                    ));
                }
                return Ok(Endpoint {
                    port,
                    token: meta.token,
                });
            }
        }
    }

    let standalone = read_metadata(&data_dir.join("host-standalone.json"))?;
    Ok(Endpoint {
        port: standalone.port,
        token: standalone.token,
    })
}

fn resolve_endpoint() -> Result<Endpoint, String> {
    let ping = probe_kernel_ping();
    resolve_endpoint_in(&unidtk_api::paths::data_dir_view(), ping.as_ref())
}

fn token() -> Result<String, String> {
    resolve_endpoint().map(|ep| ep.token)
}

fn request(body: Value) -> Result<Value, String> {
    let ep = resolve_endpoint()?;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], ep.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .map_err(|e| format!("native inspector unavailable at {address}: {e}"))?;
    stream.set_write_timeout(Some(protocol::IO_DEADLINE)).map_err(|e| e.to_string())?;
    let message = body.to_string();
    if message.len() > protocol::MAX_REQUEST { return Err("native request exceeds 16 KiB".into()); }
    writeln!(stream, "{message}").map_err(|e| e.to_string())?;
    let bytes = protocol::read_line(&mut stream, protocol::MAX_REPLY)?;
    let reply: Value = serde_json::from_slice(&bytes).map_err(|e| format!("bad inspector reply: {e}"))?;
    if reply["v"] != protocol::VERSION { return Err("incompatible native inspector protocol".into()); }
    if reply["ok"] == true { Ok(reply["result"].clone()) }
    else { Err(reply["error"].as_str().unwrap_or("inspector error").to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_resolves_host_port_from_ping_or_standalone_and_routes_native_checks() {
        let dir = std::env::temp_dir().join(format!(
            "unidtk-cli-host-resolve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("host-standalone.json"),
            r#"{"pid":101,"port":26541,"token":"standalone-tok","supervised":false}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("host-inspector.777.json"),
            r#"{"pid":777,"port":54321,"token":"sup-tok","supervised":true}"#,
        )
        .unwrap();

        let sup_ping = json!({
            "host": { "mode": "native", "pid": 777, "generation": 2, "inspectorPort": 54321, "attached": true, "ready": true }
        });
        let ep = resolve_endpoint_in(&dir, Some(&sup_ping)).expect("supervised ping resolution");
        assert_eq!(ep.port, 54321);
        assert_eq!(ep.token, "sup-tok");

        let pending_ping = json!({
            "host": { "mode": "native", "pid": 777, "generation": 2, "inspectorPort": null, "attached": false, "ready": false }
        });
        assert!(
            resolve_endpoint_in(&dir, Some(&pending_ping)).is_err(),
            "native mode without inspectorPort must not fall back to stale standalone metadata"
        );

        let standalone = resolve_endpoint_in(&dir, None).expect("offline standalone fallback");
        assert_eq!(standalone.port, 26541);
        assert_eq!(standalone.token, "standalone-tok");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
