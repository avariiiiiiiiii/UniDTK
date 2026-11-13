//! Bounded native inspector: read-only queries and authenticated input
//! (r[ipc.host-inspector]). Workers never hold the snapshot while waiting on Win32.

use std::collections::VecDeque;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use base64::Engine;
use serde_json::{json, Value};
use crate::protocol::{self, PORT};

const MAX_CLIENTS: usize = 4;

#[derive(Clone, Default)]
pub struct Snapshot {
    pub elements: Vec<Value>,
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
    pub window: Mutex<Option<rune::WindowHandle>>,
    pub stopping: AtomicBool,
    pub failure: Mutex<Option<String>>,
    started: Instant,
    token: String,
    clients: AtomicUsize,
    listener: Mutex<Option<TcpListener>>,
    input: Mutex<()>,
    capture: Mutex<()>,
    quit: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Inspector {
    pub fn new() -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            snapshot: Mutex::new(Snapshot::default()), hwnd: AtomicIsize::new(0),
            input_count: AtomicU64::new(0), window: Mutex::new(None),
            stopping: AtomicBool::new(false), failure: Mutex::new(None),
            started: Instant::now(), token: crate::desktop_host::random_token()?,
            clients: AtomicUsize::new(0), listener: Mutex::new(None), input: Mutex::new(()), capture: Mutex::new(()),
            quit: Mutex::new(None),
        }))
    }

    pub fn start(self: &Arc<Self>, quit: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
        // Binding first makes a duplicate start harmless to the current run's token.
        let listener = TcpListener::bind(("127.0.0.1", PORT)).map_err(|e| format!("bind {PORT}: {e}"))?;
        crate::desktop_host::write_private_token(&token_path(), &self.token)?;
        let accepting = match listener.try_clone() {
            Ok(accepting) => accepting,
            Err(error) => { self.remove_token(); return Err(format!("clone listener: {error}")); }
        };
        *self.listener.lock().unwrap() = Some(listener);
        *self.quit.lock().map_err(|e| e.to_string())? = Some(Box::new(quit));
        let inspector = self.clone();
        if let Err(error) = std::thread::Builder::new().name("host-inspector".into()).spawn(move || {
            for stream in accepting.incoming().flatten() {
                if inspector.stopping.load(Ordering::Acquire) { break; }
                // Reserve before spawning: even a connect storm has at most four workers.
                if inspector.clients.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_CLIENTS).then_some(n + 1)).is_err() {
                    drop(stream);
                    continue;
                }
                let worker = inspector.clone();
                if std::thread::Builder::new().name("host-request".into()).spawn(move || {
                    struct Permit(Arc<Inspector>);
                    impl Drop for Permit {
                        fn drop(&mut self) { self.0.clients.fetch_sub(1, Ordering::AcqRel); }
                    }
                    let permit = Permit(worker);
                    permit.0.serve(stream);
                }).is_err() { inspector.clients.fetch_sub(1, Ordering::AcqRel); }
            }
        }) {
            self.remove_token();
            self.listener.lock().unwrap().take();
            return Err(format!("inspector thread: {error}"));
        }
        Ok(())
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        // Keep the port reserved through cleanup: a following run must not publish
        // a token between this run's comparison and its removal.
        self.remove_token();
        // Wake the blocking listener once; idle serving installs no polling timer.
        let _ = TcpStream::connect_timeout(&([127, 0, 0, 1], PORT).into(), Duration::from_millis(100));
        self.listener.lock().unwrap().take();
    }

    fn remove_token(&self) {
        let path = token_path();
        if std::fs::read_to_string(&path).ok().as_deref() == Some(self.token.as_str()) {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn fail(&self, error: String) {
        crate::desktop_host::note(&format!("host failed: {error}"));
        if let Ok(mut failure) = self.failure.lock() { *failure = Some(error); }
        self.request_quit();
    }

    fn request_quit(&self) {
        self.stopping.store(true, Ordering::Release);
        if let Ok(quit) = self.quit.lock() {
            if let Some(quit) = quit.as_ref() { quit(); }
        }
    }

    fn serve(&self, mut stream: TcpStream) {
        let _ = stream.set_write_timeout(Some(protocol::IO_DEADLINE));
        let mut quitting = false;
        let result = protocol::read_line(&mut stream, protocol::MAX_REQUEST)
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| format!("bad json: {e}")))
            .and_then(|request| {
                let result = self.dispatch(&request)?;
                quitting = request["cmd"] == "quit";
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
            let ready = {
                let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?;
                snapshot.drawn > 0 && !snapshot.region.is_empty() && attached
                    && !self.stopping.load(Ordering::Acquire)
            };
            return Ok(json!({
                "pid": std::process::id(), "proto": protocol::VERSION,
                "uptime_ms": self.started.elapsed().as_millis() as u64,
                "attached": attached, "ready": ready,
                "input_count": self.input_count.load(Ordering::Acquire),
                "state": if ready { "ready" } else if attached { "attached" } else if hwnd == 0 { "starting" } else { "detached" },
                "scale": attached.then(|| crate::desktop_host::window_scale(hwnd)),
            }));
        }
        if cmd == "frames" { return self.frames(); }
        if cmd == "quit" { return Ok(json!({ "quitting": true })); }
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd == 0 { return Err("host is starting: the desktop window is not attached".into()); }
        if !crate::desktop_host::window_is_attached(hwnd) { return Err("native desktop window is detached; restart the host".into()); }
        match cmd {
            "tree" => {
                let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?;
                if snapshot.drawn == 0 { return Err("host has not drawn its first frame (desktop may be covered)".into()); }
                Ok(Value::Array(snapshot.elements.clone()))
            }
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

    fn click(&self, hwnd: isize, args: &Value) -> Result<Value, String> {
        let _input = self.input.try_lock().map_err(|_| "another input operation is in progress")?;
        let point = {
            let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?;
            if snapshot.drawn == 0 { return Err("host has not drawn its first frame".into()); }
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
        crate::desktop_host::click(hwnd, x, y)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.input_count.load(Ordering::Acquire) == before {
            if Instant::now() >= deadline || self.stopping.load(Ordering::Acquire) {
                return Err("input was posted but Rune did not acknowledge a card press".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(json!({ "clicked": point, "forced": forced, "transport": "win32-posted",
            "before": before, "input_count": self.input_count.load(Ordering::Acquire), "handled": true }))
    }

    fn frames(&self) -> Result<Value, String> {
        let snapshot = self.snapshot.lock().map_err(|e| e.to_string())?.clone();
        let stats = self.window.lock().map_err(|e| e.to_string())?.as_ref().map(|w| w.surface_stats());
        let (cpu_ms, working_set, private) = crate::desktop_host::process_usage();
        let (handles, gdi_objects, user_objects) = crate::desktop_host::process_handles();
        Ok(json!({
            "uptime_ms": self.started.elapsed().as_millis() as u64,
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

fn token_path() -> std::path::PathBuf { unidtk_api::paths::data_dir().join("host-inspector.token") }

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
