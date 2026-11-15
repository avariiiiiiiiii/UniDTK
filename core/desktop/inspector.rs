//! Bounded native inspector: read-only queries and authenticated input
//! (r[ipc.host-inspector]). Workers never hold the snapshot while waiting on Win32.

use std::collections::VecDeque;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use base64::Engine;
use serde_json::{json, Value};
use crate::protocol::{self, PORT};

const MAX_CLIENTS: usize = 4;
const PRIORITY_SLOTS: usize = 1;
const PRIORITY_READ_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Default)]
pub struct Snapshot {
    pub elements: Vec<Value>,
    pub committed: u64,
    pub drawn: u64,
    pub skipped: u64,
    pub unchanged: u64,
    pub encode_ms: VecDeque<f64>,
    pub update_ms: VecDeque<f64>,
    pub cpu_frame_ms: VecDeque<f64>,
    pub last_frame: Option<Instant>,
    pub region: Vec<[i32; 4]>,
    pub widget_rects: Vec<[f32; 4]>,
}

pub struct Inspector {
    pub snapshot: Mutex<Snapshot>,
    pub hwnd: AtomicIsize,
    pub input_count: AtomicU64,
    pub tree_seq: AtomicU64,
    pub window: Mutex<Option<rune::WindowHandle>>,
    pub stopping: AtomicBool,
    pub failure: Mutex<Option<String>>,
    /// The commands to the kernel (`desk` forwards a position write); set once the link exists.
    pub outbox: Mutex<Option<Arc<crate::ws_client::Outbox>>>,
    data_dir: PathBuf,
    started: Instant,
    token: String,
    supervised: AtomicBool,
    bound_port: AtomicU16,
    clients: AtomicUsize,
    priority_clients: AtomicUsize,
    listener: Mutex<Option<TcpListener>>,
    input: Mutex<()>,
    capture: Mutex<()>,
    quit: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Inspector {
    pub fn new() -> Result<Arc<Self>, String> {
        Self::new_in(unidtk_api::paths::data_dir())
    }

    pub fn new_in(data_dir: PathBuf) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            snapshot: Mutex::new(Snapshot::default()), hwnd: AtomicIsize::new(0),
            input_count: AtomicU64::new(0), tree_seq: AtomicU64::new(0), window: Mutex::new(None),
            stopping: AtomicBool::new(false), failure: Mutex::new(None), outbox: Mutex::new(None),
            data_dir,
            started: Instant::now(), token: crate::desktop_host::random_token()?,
            supervised: AtomicBool::new(false), bound_port: AtomicU16::new(0),
            clients: AtomicUsize::new(0), priority_clients: AtomicUsize::new(0),
            listener: Mutex::new(None), input: Mutex::new(()), capture: Mutex::new(()),
            quit: Mutex::new(None),
        }))
    }

    pub fn port(&self) -> u16 {
        self.bound_port.load(Ordering::Acquire)
    }

    pub(crate) fn is_ready(&self, attached: bool) -> Result<bool, String> {
        let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?;
        Ok(attached
            && !self.stopping.load(Ordering::Acquire)
            && snapshot.committed > 0
            && !snapshot.region.is_empty())
    }

    pub(crate) fn tree_snapshot(&self) -> Result<Value, String> {
        let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?;
        if snapshot.committed == 0 {
            return Err("host has not committed its initial element tree".into());
        }
        Ok(Value::Array(snapshot.elements.clone()))
    }

    pub(crate) fn wait_for_click_ack(
        &self,
        before_input: u64,
        before_tree: u64,
        ack_timeout: Duration,
        tree_grace: Duration,
    ) -> Result<u64, String> {
        let deadline = Instant::now() + ack_timeout;
        while self.input_count.load(Ordering::Acquire) == before_input {
            if Instant::now() >= deadline || self.stopping.load(Ordering::Acquire) {
                return Err("input was posted but Rune did not acknowledge a card press".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Wait briefly for the post-input commit (`tree_seq`) so callers reading `tree` right after
        // `click` see updated text without timing out on handlers that do not alter the tree.
        let tree_deadline = Instant::now() + tree_grace;
        while self.tree_seq.load(Ordering::Acquire) == before_tree
            && Instant::now() < tree_deadline
            && !self.stopping.load(Ordering::Acquire)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(self.input_count.load(Ordering::Acquire))
    }

    #[cfg(test)]
    pub(crate) fn active_normal_clients(&self) -> usize {
        self.clients.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn active_priority_clients(&self) -> usize {
        self.priority_clients.load(Ordering::Acquire)
    }

    pub fn start(self: &Arc<Self>, supervised: bool, quit: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
        let preferred = if supervised { 0 } else { PORT };
        self.start_on_port(supervised, preferred, quit)
    }

    pub(crate) fn start_on_port(
        self: &Arc<Self>,
        supervised: bool,
        preferred_port: u16,
        quit: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let listener = if supervised || preferred_port == 0 {
            TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("bind ephemeral: {e}"))?
        } else {
            match TcpListener::bind(("127.0.0.1", preferred_port)) {
                Ok(listener) => listener,
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    crate::desktop_host::note(&format!(
                        "inspector: standalone port {preferred_port} in use ({error}), falling back to ephemeral"
                    ));
                    TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("bind fallback ephemeral: {e}"))?
                }
                Err(error) => return Err(format!("bind {preferred_port}: {error}")),
            }
        };
        let port = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?.port();
        self.supervised.store(supervised, Ordering::Release);
        self.bound_port.store(port, Ordering::Release);

        // Publish per-PID (and standalone when unsupervised) metadata only after binding succeeded.
        write_metadata(&self.data_dir, std::process::id(), port, &self.token, supervised)?;

        let accepting = match listener.try_clone() {
            Ok(accepting) => accepting,
            Err(error) => { self.remove_metadata(); return Err(format!("clone listener: {error}")); }
        };
        *self.listener.lock().unwrap() = Some(listener);
        *self.quit.lock().map_err(|e| e.to_string())? = Some(Box::new(quit));
        let inspector = self.clone();
        if let Err(error) = std::thread::Builder::new().name("host-inspector".into()).spawn(move || {
            for mut stream in accepting.incoming().flatten() {
                if inspector.stopping.load(Ordering::Acquire) { break; }
                let priority_only = if inspector.clients
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_CLIENTS).then_some(n + 1))
                    .is_ok()
                {
                    false
                } else if inspector.priority_clients
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < PRIORITY_SLOTS).then_some(n + 1))
                    .is_ok()
                {
                    true
                } else {
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
                    let busy = json!({ "v": protocol::VERSION, "ok": false, "error": "busy: inspector client limit reached" });
                    let _ = writeln!(stream, "{busy}");
                    let _ = stream.flush();
                    drop(stream);
                    continue;
                };
                let worker = inspector.clone();
                if std::thread::Builder::new().name("host-request".into()).spawn(move || {
                    struct Permit {
                        inspector: Arc<Inspector>,
                        priority_only: bool,
                    }
                    impl Drop for Permit {
                        fn drop(&mut self) {
                            if self.priority_only {
                                self.inspector.priority_clients.fetch_sub(1, Ordering::AcqRel);
                            } else {
                                self.inspector.clients.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                    }
                    let permit = Permit { inspector: worker, priority_only };
                    permit.inspector.serve(stream, priority_only);
                }).is_err() {
                    if priority_only {
                        inspector.priority_clients.fetch_sub(1, Ordering::AcqRel);
                    } else {
                        inspector.clients.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            }
        }) {
            self.remove_metadata();
            self.listener.lock().unwrap().take();
            return Err(format!("inspector thread: {error}"));
        }
        Ok(())
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.remove_metadata();
        // Wake the blocking listener once; idle serving installs no polling timer.
        let port = self.bound_port.load(Ordering::Acquire);
        if port != 0 {
            let _ = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(100));
        }
        self.listener.lock().unwrap().take();
    }

    fn remove_metadata(&self) {
        let pid = std::process::id();
        let pid_path = pid_metadata_path(&self.data_dir, pid);
        if metadata_matches(&pid_path, pid, &self.token) {
            let _ = std::fs::remove_file(pid_path);
        }
        if !self.supervised.load(Ordering::Acquire) {
            let standalone = standalone_metadata_path(&self.data_dir);
            if metadata_matches(&standalone, pid, &self.token) {
                let _ = std::fs::remove_file(standalone);
            }
        }
    }

    pub fn fail(&self, error: String) {
        crate::desktop_host::note(&format!("host failed: {error}"));
        if let Ok(mut failure) = self.failure.lock() { *failure = Some(error); }
        self.request_quit();
    }

    pub(crate) fn request_quit(&self) {
        self.stopping.store(true, Ordering::Release);
        if let Ok(quit) = self.quit.lock() {
            if let Some(quit) = quit.as_ref() { quit(); }
        }
    }

    fn serve(&self, mut stream: TcpStream, priority_only: bool) {
        let _ = stream.set_write_timeout(Some(protocol::IO_DEADLINE));
        let mut quitting = false;
        let read_window = if priority_only { PRIORITY_READ_WINDOW } else { protocol::IO_DEADLINE };
        let result = protocol::read_line_within(&mut stream, protocol::MAX_REQUEST, read_window)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| format!("bad json: {e}")))
            .and_then(|request| {
                let cmd = request["cmd"].as_str().ok_or("cmd must be a string")?;
                if priority_only && !matches!(cmd, "ping" | "quit") {
                    return Err("busy: priority slot accepts only ping or quit".into());
                }
                let result = self.dispatch(&request)?;
                quitting = cmd == "quit";
                Ok(result)
            });
        let reply = match result {
            Ok(result) => json!({ "v": protocol::VERSION, "ok": true, "result": result }),
            Err(error) => json!({ "v": protocol::VERSION, "ok": false, "error": error }),
        };
        let _ = writeln!(stream, "{reply}");
        let _ = stream.flush();
        if quitting {
            crate::desktop_host::note("inspector: graceful quit");
            self.request_quit();
        }
    }

    fn dispatch(&self, request: &Value) -> Result<Value, String> {
        let cmd = request["cmd"].as_str().ok_or("cmd must be a string")?;
        let args = &request["args"];
        protocol::authorize(cmd, args["token"].as_str(), &self.token)?;
        if self.stopping.load(Ordering::Acquire) { return Err("host is stopping".into()); }
        // No request may select a host-side file on the open door.
        if cmd == "png" && args.get("path").is_some() {
            return Err("png returns image bytes; save the image in the client".into());
        }
        if cmd == "ping" {
            let hwnd = self.hwnd.load(Ordering::Acquire);
            let attached = crate::desktop_host::window_is_attached(hwnd);
            let ready = self.is_ready(attached)?;
            return Ok(json!({
                "pid": std::process::id(), "proto": protocol::VERSION,
                "port": self.port(),
                "supervised": self.supervised.load(Ordering::Acquire),
                "uptime_ms": self.started.elapsed().as_millis() as u64,
                "attached": attached, "ready": ready,
                "input_count": self.input_count.load(Ordering::Acquire),
                "state": if ready { "ready" } else if attached { "attached" } else if hwnd == 0 { "starting" } else { "detached" },
                "scale": attached.then(|| crate::desktop_host::window_scale(hwnd)),
            }));
        }
        if cmd == "frames" { return self.frames(); }
        if cmd == "desk" { return self.desk(args); }
        if cmd == "quit" { return Ok(json!({ "quitting": true })); }
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd == 0 { return Err("host is starting: the desktop window is not attached".into()); }
        if !crate::desktop_host::window_is_attached(hwnd) { return Err("native desktop window is detached; restart the host".into()); }
        match cmd {
            "tree" => self.tree_snapshot(),
            "png" => {
                let window = self.window.lock().map_err(|e| e.to_string())?.clone();
                let _capture = self.capture.try_lock().map_err(|_| "screenshot already in progress")?;
                let (width, height, bytes, method) = if let Some(w) = window {
                    let (width, height, rgba) = w.capture_rgba()?;
                    let bytes = crate::desktop_host::encode_rgba_png(width, height, &rgba)?;
                    (width, height, bytes, "wgpu-readback")
                } else {
                    let (width, height, bytes) = crate::desktop_host::window_png(hwnd)?;
                    (width, height, bytes, "dwm-printwindow")
                };
                if bytes.len() > protocol::MAX_PNG { return Err("PNG exceeds the inspector limit".into()); }
                Ok(json!({ "width": width, "height": height, "capture": method,
                    "base64": base64::engine::general_purpose::STANDARD.encode(bytes) }))
            }
            "hit" => {
                let [x, y] = coordinates(args)?;
                let scale = crate::desktop_host::window_scale(hwnd);
                Ok(crate::desktop_host::takes_point(hwnd, (x * scale).round() as i32, (y * scale).round() as i32))
            }
            "click" => self.click(hwnd, args),
            _ => unreachable!("authorize rejected unknown commands"),
        }
    }

    /// One widget's position fields for a person at the CLI, as a `desk_apply` position
    /// operation through the host's own link, which the kernel validates and merges like any
    /// other writer's (and seeds a first layout for, when nothing is stored).
    fn desk(&self, args: &Value) -> Result<Value, String> {
        let mut op = args.as_object().cloned().ok_or("desk takes an object")?;
        op.remove("token");
        op.insert("op".into(), json!("position"));
        let outbox = self.outbox.lock().map_err(|e| e.to_string())?.clone().ok_or("the host has no kernel link (it was not started by the kernel)")?;
        let id = outbox.send("desk_apply", json!({ "ops": [Value::Object(op)] }), None)?;
        Ok(json!({ "queued": id }))
    }

    fn click(&self, hwnd: isize, args: &Value) -> Result<Value, String> {
        let _input = self.input.try_lock().map_err(|_| "another input operation is in progress")?;
        let point = {
            let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?;
            if snapshot.committed == 0 { return Err("host has not committed its initial element tree".into()); }
            if let Some(target) = args["target"].as_str() {
                let matches: Vec<_> = snapshot.elements.iter().filter(|e| e["key"] == target || e["path"] == target).collect();
                if matches.len() != 1 { return Err(format!("target '{target}' matched {} elements", matches.len())); }
                let rect = &matches[0]["rect"];
                [rect[0].as_f64().ok_or("invalid target rect")? + rect[2].as_f64().ok_or("invalid target rect")? / 2.0,
                 rect[1].as_f64().ok_or("invalid target rect")? + rect[3].as_f64().ok_or("invalid target rect")? / 2.0]
            } else { coordinates(args)? }
        };
        let scale = crate::desktop_host::window_scale(hwnd);
        let (x, y) = ((point[0] * scale).round() as i32, (point[1] * scale).round() as i32);
        let hit = crate::desktop_host::takes_point(hwnd, x, y);
        if hit["in_region"] != true { return Err("point is outside the host's interactive region".into()); }
        let forced = args["force"].as_bool().unwrap_or(false);
        if !forced && hit["on_screen_is_host"] != true { return Err("target is covered; --force explicitly permits posted input to the covered host".into()); }
        let before = self.input_count.load(Ordering::Acquire);
        let before_tree = self.tree_seq.load(Ordering::Acquire);
        crate::desktop_host::click(hwnd, x, y)?;
        let input_count = self.wait_for_click_ack(
            before,
            before_tree,
            Duration::from_secs(1),
            Duration::from_millis(50),
        )?;
        Ok(json!({ "clicked": point, "forced": forced, "transport": "win32-posted",
            "before": before, "input_count": input_count, "handled": true }))
    }

    fn frames(&self) -> Result<Value, String> {
        let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?.clone();
        let stats = self.window.lock().map_err(|e| e.to_string())?.as_ref().map(|w| w.surface_stats());
        let (cpu_ms, working_set, private) = crate::desktop_host::process_usage();
        let (handles, gdi_objects, user_objects) = crate::desktop_host::process_handles();
        Ok(json!({
            "uptime_ms": self.started.elapsed().as_millis() as u64,
            "committed": snapshot.committed,
            "drawn": snapshot.drawn, "skipped": snapshot.skipped, "unchanged": snapshot.unchanged,
            "since_last_frame_ms": snapshot.last_frame.map(|at| at.elapsed().as_millis() as u64),
            "encode_ms": quantiles(snapshot.encode_ms), "update_ms": quantiles(snapshot.update_ms),
            "cpu_frame_ms": quantiles(snapshot.cpu_frame_ms),
            "surface": stats.map(|s| json!({
                "turns": s.totals.turns,
                "timer_wakes": s.totals.wakes,
                "wait_cancelled": s.totals.wait_cancelled,
                "redraws": s.totals.redraws,
                "unpaced": s.totals.unpaced,
                "presents": s.totals.presents,
                "skipped": s.totals.skipped,
                "occluded": s.totals.occluded,
                "timeouts": s.totals.timeouts,
                "outdated": s.totals.outdated,
            })),
            "cpu_ms": cpu_ms, "working_set_mb": working_set as f64 / 1048576.0, "private_mb": private as f64 / 1048576.0,
            "handles": handles, "gdi_objects": gdi_objects, "user_objects": user_objects,
        }))
    }
}

fn pid_metadata_path(dir: &std::path::Path, pid: u32) -> PathBuf {
    dir.join(format!("host-inspector.{pid}.json"))
}

fn standalone_metadata_path(dir: &std::path::Path) -> PathBuf {
    dir.join("host-standalone.json")
}

fn write_metadata(dir: &std::path::Path, pid: u32, port: u16, token: &str, supervised: bool) -> Result<(), String> {
    let payload = json!({
        "pid": pid,
        "port": port,
        "token": token,
        "supervised": supervised,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("serialize host metadata: {e}"))?;
    crate::desktop_host::write_private_file_atomic(&pid_metadata_path(dir, pid), &bytes)?;
    if !supervised {
        crate::desktop_host::write_private_file_atomic(&standalone_metadata_path(dir), &bytes)?;
    }
    Ok(())
}

fn metadata_matches(path: &std::path::Path, pid: u32, token: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else { return false; };
    let Ok(meta) = serde_json::from_str::<Value>(&raw) else { return false; };
    meta["pid"].as_u64() == Some(pid as u64) && meta["token"].as_str() == Some(token)
}

fn coordinates(args: &Value) -> Result<[f64; 2], String> {
    let x = args["x"].as_f64().ok_or("requires finite {x, y} in points")?;
    let y = args["y"].as_f64().ok_or("requires finite {x, y} in points")?;
    if !x.is_finite() || !y.is_finite() || x.abs() > 32767.0 || y.abs() > 32767.0 {
        return Err("coordinates are out of range".into());
    }
    Ok([x, y])
}

fn quantiles(samples: VecDeque<f64>) -> Value {
    let mut samples: Vec<_> = samples.into_iter().collect();
    samples.sort_by(f64::total_cmp);
    let pick = |q: f64| samples.get(((samples.len().saturating_sub(1) as f64) * q).round() as usize).copied();
    json!({ "count": samples.len(), "p50": pick(0.5), "p95": pick(0.95), "max": samples.last() })
}
