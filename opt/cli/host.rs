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
            let body = json!({ "cmd": sub, "args": { "x": x, "y": y, "force": force } });
            if sub == "click" { request_with_token(body)? } else { request(body)? }
        }
        "quit" => quit()?,
        "desk" => desk(&args[2..])?,
        _ => return Err("usage: udtk host [run|ping|tree|frames|shot [path]|hit <x> <y>|click <x> <y> [--force]|desk <id> <field>=<value>...|quit]".into()),
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

/// `udtk host desk clock version=digital`: one widget's position fields, written through the
/// host's kernel link (`desk_position_set`); numbers stay numbers, anything else is text.
fn desk(args: &[String]) -> Result<Value, String> {
    let id = args.first().ok_or("usage: udtk host desk <id> <field>=<value>...")?;
    let mut fields = serde_json::Map::new();
    fields.insert("id".into(), json!(id));
    for pair in &args[1..] {
        let (key, value) = pair.split_once('=').ok_or_else(|| format!("{pair}: expected <field>=<value>"))?;
        fields.insert(key.into(), value.parse::<f64>().map_or_else(|_| json!(value), |n| json!(n)));
    }
    request_with_token(json!({ "cmd": "desk", "args": fields }))
}

pub fn quit() -> Result<Value, String> {
    request_with_token(json!({ "cmd": "quit", "args": {} }))
}

pub fn gui(args: &[String]) -> Result<String, String> {
    let force = args.iter().any(|a| a == "--force");
    let steps: Vec<_> = args.iter().skip(1).filter(|a| !matches!(a.as_str(), "--host" | "--json" | "--force")).collect();
    if steps.is_empty() || steps.len() > 50 { return Err("usage: udtk gui --host [--json] [--force] tree|click:<key-or-path> ... (max 50 steps)".into()); }
    let mut receipts = Vec::new();
    for step in steps {
        let result = if step.as_str() == "tree" { request(json!({ "cmd": "tree" }))? }
            else if let Some(target) = step.strip_prefix("click:") {
                request_with_token(json!({ "cmd": "click", "args": { "target": target, "force": force } }))?
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
        if !tree.as_array().is_some_and(|nodes| nodes.iter().any(|n| n["key"] == "widget:clock")) { return Err("clock widget missing from element tree".into()); }
        let receipt = request_with_token(json!({ "cmd": "click", "args": { "target": "widget:clock", "force": true } }))?;
        if receipt["handled"] != true || receipt["input_count"].as_u64() <= receipt["before"].as_u64() { return Err("Rune did not acknowledge the clock press".into()); }
        out.push_str(&format!("  [OK] clock press acknowledged (forced Win32 posting): {receipt}
"));
        let hit = request(json!({ "cmd": "hit", "args": { "x": 1, "y": 1 } }))?;
        if hit["host_is_child"] != true { return Err(format!("native window lost its WS_CHILD style: {hit}")); }
        if hit["in_region"] != false { return Err(format!("blank desktop is intercepted: {hit}")); }
        out.push_str("  [OK] blank desktop point is outside the native input region\n");
        let original = resolve_endpoint()?;
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
        let after = resolve_endpoint()?;
        if after != original || request_on(&after, json!({ "cmd": "ping" }))?["pid"] != ping["pid"] {
            return Err("duplicate host changed the live token or endpoint".into());
        }
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
    supervised: bool,
}

enum MetadataLoad {
    Loaded(InspectorMetadata),
    NotFound(String),
}

fn read_metadata_optional(path: &std::path::Path) -> Result<MetadataLoad, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<InspectorMetadata>(&raw)
            .map(MetadataLoad::Loaded)
            .map_err(|e| format!("{}: bad metadata json: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(MetadataLoad::NotFound(format!("{}: {e} (is the host running?)", path.display())))
        }
        Err(e) => Err(format!("{}: {e} (is the host running?)", path.display())),
    }
}

fn read_metadata(path: &std::path::Path) -> Result<InspectorMetadata, String> {
    match read_metadata_optional(path)? {
        MetadataLoad::Loaded(meta) => Ok(meta),
        MetadataLoad::NotFound(msg) => Err(msg),
    }
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

fn load_standalone(
    data_dir: &std::path::Path,
    web_mode: bool,
    mut is_live_pid: impl FnMut(u32) -> bool,
) -> Result<Endpoint, String> {
    let path = data_dir.join("host-standalone.json");
    let meta = match read_metadata_optional(&path)? {
        MetadataLoad::Loaded(meta) => meta,
        MetadataLoad::NotFound(not_found_err) => {
            if web_mode {
                return Err(
                    "native desktop host is not active: kernel is running in 'web' mode and no standalone host is listening (switch mode in tray or run 'udtk host run')"
                        .into(),
                );
            }
            return Err(not_found_err);
        }
    };
    if meta.supervised {
        return Err(format!("{}: expected standalone metadata (supervised=false)", path.display()));
    }
    if !is_live_pid(meta.pid) {
        return Err(format!(
            "{}: stale standalone host metadata (pid={} is not running)",
            path.display(),
            meta.pid
        ));
    }
    Ok(Endpoint {
        port: meta.port,
        token: meta.token,
    })
}

#[cfg(test)]
pub(crate) fn resolve_endpoint_in(
    data_dir: &std::path::Path,
    kernel_ping: Option<&Value>,
) -> Result<Endpoint, String> {
    resolve_endpoint_with(data_dir, kernel_ping, |_| true)
}

pub(crate) fn resolve_endpoint_with(
    data_dir: &std::path::Path,
    kernel_ping: Option<&Value>,
    is_live_pid: impl FnMut(u32) -> bool,
) -> Result<Endpoint, String> {
    let mut web_mode = false;
    if let Some(ping) = kernel_ping {
        if let Some(host) = ping.get("host") {
            match host.get("mode").and_then(|m| m.as_str()) {
                Some("native") => {
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
                    if !meta.supervised || meta.pid != pid || meta.port != port {
                        return Err(format!(
                            "{}: metadata mismatch (pid={}, port={}, supervised={}) vs ping (pid={pid}, port={port})",
                            meta_path.display(),
                            meta.pid,
                            meta.port,
                            meta.supervised
                        ));
                    }
                    return Ok(Endpoint {
                        port,
                        token: meta.token,
                    });
                }
                Some("web") => {
                    web_mode = true;
                }
                Some(other) => {
                    return Err(format!("kernel reported unknown desktop host mode '{other}'"));
                }
                None => {}
            }
        }
    }

    load_standalone(data_dir, web_mode, is_live_pid)
}

fn pid_is_live_desktop_host(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    unsafe {
        let Ok(raw) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let handle = OwnedHandle(raw);
        let mut exit_code = 0u32;
        if GetExitCodeProcess(handle.0, &mut exit_code).is_err() || exit_code != STILL_ACTIVE {
            return false;
        }
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_err()
        {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("unidtk-desktop.exe"))
    }
}

fn resolve_endpoint() -> Result<Endpoint, String> {
    let ping = probe_kernel_ping();
    resolve_endpoint_with(&unidtk_api::paths::data_dir_view(), ping.as_ref(), pid_is_live_desktop_host)
}

fn request(body: Value) -> Result<Value, String> {
    let ep = resolve_endpoint()?;
    request_on(&ep, body)
}

fn request_with_token(mut body: Value) -> Result<Value, String> {
    let ep = resolve_endpoint()?;
    if let Some(obj) = body.as_object_mut() {
        let args = obj.entry("args").or_insert_with(|| json!({}));
        if let Some(args_obj) = args.as_object_mut() {
            args_obj.insert("token".into(), json!(ep.token));
        }
    }
    request_on(&ep, body)
}

fn request_on(ep: &Endpoint, body: Value) -> Result<Value, String> {
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

    #[test]
    fn cli_reports_clear_error_in_web_mode_without_breaking_standalone() {
        let dir = std::env::temp_dir().join(format!(
            "unidtk-cli-host-web-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let web_ping = json!({
            "host": { "mode": "web", "pid": null, "generation": 0, "inspectorPort": null, "attached": false, "ready": false }
        });

        // Case A: kernel in `web` mode and no standalone host running -> clear error instead of raw os error 2.
        let missing_err = resolve_endpoint_in(&dir, Some(&web_ping)).unwrap_err();
        assert!(
            missing_err.contains("web") && missing_err.contains("native"),
            "expected clear web-mode message, got: {missing_err}"
        );

        // Case D: offline kernel + no standalone file keeps the standalone metadata error.
        let offline_err = resolve_endpoint_in(&dir, None).unwrap_err();
        assert!(
            offline_err.contains("host-standalone.json"),
            "expected standalone metadata error when offline, got: {offline_err}"
        );

        // Case C: corrupt standalone JSON is still surfaced honestly even in `web` mode.
        let standalone_path = dir.join("host-standalone.json");
        std::fs::write(&standalone_path, "{not json").unwrap();
        let corrupt_err = resolve_endpoint_in(&dir, Some(&web_ping)).unwrap_err();
        assert!(
            corrupt_err.contains("bad metadata json"),
            "expected parse error, got: {corrupt_err}"
        );

        // Case B: valid standalone metadata still works while kernel is in `web` mode (`udtk host run` / `stop`).
        std::fs::write(
            &standalone_path,
            r#"{"pid":101,"port":26541,"token":"standalone-tok","supervised":false}"#,
        )
        .unwrap();
        let ep = resolve_endpoint_in(&dir, Some(&web_ping))
            .expect("standalone host must still resolve while kernel is in web mode");
        assert_eq!(ep.port, 26541);
        assert_eq!(ep.token, "standalone-tok");

        // Case E: dead PID makes standalone metadata report stale instead of connecting to reused port.
        let stale_err = resolve_endpoint_with(&dir, Some(&web_ping), |_| false).unwrap_err();
        assert!(stale_err.contains("stale"), "expected stale error, got: {stale_err}");

        // Case F: unknown host mode fails explicitly.
        let weird_ping = json!({ "host": { "mode": "bogus" } });
        let mode_err = resolve_endpoint_in(&dir, Some(&weird_ping)).unwrap_err();
        assert!(mode_err.contains("bogus"), "expected unknown mode error, got: {mode_err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
