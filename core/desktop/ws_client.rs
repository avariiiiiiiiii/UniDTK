//! Event-driven zero-wake WebSocket link between `unidtk-desktop` and the kernel UI door
//! (`ws.rs`). Uses `WSAEventSelect` + Win32 manual-reset events, so idle waits consume no
//! timers and shutdown wakes instantly.
//!
//! The link carries three things: subscriptions (the kernel's event frames and `gap`
//! frames come back as [`Inbound`]), replies to commands by `id`, and an outbox of commands
//! the host sends. The outbox is bounded; a command with a key replaces an unsent one with
//! the same key (a drag sends only its latest position); a *sticky* command (the host
//! registration, the input focus) is also replayed after every reconnect, so the kernel
//! never keeps an older state than the host last reported. A command sent with
//! [`Outbox::request`] is answered exactly once: by its reply, by a timeout, or as lost when
//! the link drops after it left (the kernel may or may not have run it; the caller reads the
//! state again).

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Networking::WinSock::{
    WSACloseEvent, WSACreateEvent, WSAEnumNetworkEvents, WSAEventSelect,
    WSAWaitForMultipleEvents, FD_CLOSE, FD_READ, FD_WRITE, SOCKET, WSAEVENT, WSANETWORKEVENTS,
    WSA_INFINITE, WSA_WAIT_FAILED,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};

use std::os::windows::io::AsRawSocket;

/// How long a requested command waits for its reply, unless it asks for longer
/// ([`Outbox::request_within`]).
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// What a requester hears: the reply's result or error, or why there is none.
pub type ReplyHandler = Box<dyn FnOnce(Result<Value, String>) + Send>;

struct Pending {
    /// Counted from the moment the command left (`confirm_sent`); until then, from when it
    /// was queued, so a command that never leaves still hears a timeout.
    deadline: Instant,
    timeout: Duration,
    cmd: String,
    /// The command left the outbox; a dropped link loses its reply.
    sent: bool,
    handler: ReplyHandler,
}

/// The error a requester hears when its command's deadline passed. It names the command:
/// a bare "timeout" in a folder list said nothing about which call the kernel did not answer.
pub fn timeout_error(cmd: &str, timeout: Duration) -> String {
    format!("timeout: the kernel did not answer '{cmd}' within {} s", timeout.as_secs())
}

/// Unsent commands the outbox holds; one more is refused, loudly.
const OUTBOX_CAPACITY: usize = 64;
/// A session shorter than this does not reset the reconnect back-off: a kernel that accepts
/// and drops at once must not be called every 250 ms.
const STABLE_SESSION: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRegistrationInfo {
    pub pid: u32,
    pub generation: u64,
    pub instance_nonce: String,
    pub inspector_port: u16,
    pub attached: bool,
    pub ready: bool,
}

/// What the kernel sent that is not the answer to the handshake.
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    /// An event frame; `snapshot` marks the catch-up frame of a state event.
    Event { event: String, seq: u64, payload: Value, snapshot: bool },
    /// The kernel could not replay the event from the cursor: read the state again.
    Gap { event: String },
    /// The answer to the command sent with this `id`.
    Reply { id: u64, ok: bool, body: Value },
}

/// Parses one text frame of the kernel; `None` for frames the host does not use.
pub fn parse_inbound(text: &str) -> Option<Inbound> {
    let frame: Value = serde_json::from_str(text).ok()?;
    match frame.get("type").and_then(Value::as_str) {
        Some("event") => Some(Inbound::Event {
            event: frame.get("event")?.as_str()?.to_string(),
            seq: frame.get("seq").and_then(Value::as_u64).unwrap_or(0),
            payload: frame.get("payload").cloned().unwrap_or(Value::Null),
            snapshot: frame.get("snapshot").and_then(Value::as_bool).unwrap_or(false),
        }),
        Some("gap") => Some(Inbound::Gap { event: frame.get("event")?.as_str()?.to_string() }),
        _ => {
            let id = frame.get("id")?.as_u64()?;
            let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let body = if ok { frame.get("result") } else { frame.get("error") };
            Some(Inbound::Reply { id, ok, body: body.cloned().unwrap_or(Value::Null) })
        }
    }
}

struct Queued {
    id: u64,
    key: Option<String>,
    cmd: String,
    args: Value,
}

#[derive(Default)]
struct OutboxState {
    queue: VecDeque<Queued>,
    /// Latest value per key of the commands that survive a reconnect.
    sticky: Vec<(String, String, Value)>,
}

/// The host's commands to the kernel, shared by every thread that sends one.
pub struct Outbox {
    state: Mutex<OutboxState>,
    next_id: AtomicU64,
    wake: OwnedEvent,
    pending: Mutex<HashMap<u64, Pending>>,
}

impl Outbox {
    pub fn new() -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            state: Mutex::new(OutboxState::default()),
            next_id: AtomicU64::new(1),
            wake: OwnedEvent::new_manual_reset()?,
            pending: Mutex::new(HashMap::new()),
        }))
    }

    /// Sends a command and calls `handler` once with what came of it (on the link's thread):
    /// the reply, a timeout after [`REPLY_TIMEOUT`], or lost when the link dropped after the
    /// command left. A command that never left is sent again on the next session.
    /// The handler hears every outcome, a refusal here included.
    pub fn request(&self, cmd: &str, args: Value, handler: ReplyHandler) -> Result<u64, String> {
        self.request_within(cmd, args, REPLY_TIMEOUT, handler)
    }

    /// [`Outbox::request`] for a command that can take longer than [`REPLY_TIMEOUT`]: a
    /// server check runs for seconds, and its answer arrived after the requester had been
    /// told "timeout" and stopped listening.
    pub fn request_within(&self, cmd: &str, args: Value, timeout: Duration, handler: ReplyHandler) -> Result<u64, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // A poisoned lock still holds consistent maps (every critical section is one insert
        // or remove), and dropping the handler would leave its requester waiting for ever.
        self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(
            id,
            Pending { deadline: Instant::now() + timeout, timeout, cmd: cmd.to_string(), sent: false, handler },
        );
        let queued = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.queue.len() >= OUTBOX_CAPACITY {
                false
            } else {
                state.queue.push_back(Queued { id, key: None, cmd: cmd.to_string(), args });
                true
            }
        };
        if !queued {
            let pending = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
            let error = format!("the kernel link's outbox is full ({OUTBOX_CAPACITY} commands)");
            if let Some(pending) = pending {
                (pending.handler)(Err(error.clone()));
            }
            return Err(error);
        }
        self.wake.signal();
        Ok(id)
    }

    /// A reply arrived: true when a requester was waiting for it.
    fn resolve(&self, id: u64, result: Result<Value, String>) -> bool {
        let pending = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
        match pending {
            Some(pending) => {
                (pending.handler)(result);
                true
            }
            None => false,
        }
    }

    /// Times out the requests past their deadline; returns how long until the next one is due.
    /// A timed-out command that never left is taken out of the queue: the kernel must not
    /// run it after its requester was told it failed.
    fn expire(&self) -> Option<Duration> {
        let now = Instant::now();
        let expired: Vec<(u64, Pending)> = {
            let mut pending = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids: Vec<u64> = pending.iter().filter(|(_, p)| p.deadline <= now).map(|(id, _)| *id).collect();
            ids.into_iter().filter_map(|id| pending.remove(&id).map(|p| (id, p))).collect()
        };
        let unsent: Vec<u64> = expired.iter().filter(|(_, p)| !p.sent).map(|(id, _)| *id).collect();
        if !unsent.is_empty() {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.queue.retain(|queued| !unsent.contains(&queued.id));
        }
        for (_, pending) in expired {
            (pending.handler)(Err(timeout_error(&pending.cmd, pending.timeout)));
        }
        let pending = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.values().map(|p| p.deadline.saturating_duration_since(now)).min()
    }

    /// The link dropped: every request that had left loses its reply.
    fn lose_sent(&self) {
        let lost: Vec<Pending> = {
            let Ok(mut pending) = self.pending.lock() else { return };
            let ids: Vec<u64> = pending.iter().filter(|(_, p)| p.sent).map(|(id, _)| *id).collect();
            ids.into_iter().filter_map(|id| pending.remove(&id)).collect()
        };
        for pending in lost {
            (pending.handler)(Err("lost: the link to the kernel dropped before the reply".into()));
        }
    }

    /// Queues a command and returns its `id`, which the reply carries. A `key` replaces an
    /// unsent command with the same key. A full outbox is an error, not a silent drop.
    pub fn send(&self, cmd: &str, args: Value, key: Option<&str>) -> Result<u64, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            let queued = Queued { id, key: key.map(str::to_string), cmd: cmd.to_string(), args };
            let same_key = key.and_then(|k| state.queue.iter().position(|q| q.key.as_deref() == Some(k)));
            match same_key {
                Some(index) => state.queue[index] = queued,
                None if state.queue.len() >= OUTBOX_CAPACITY => {
                    return Err(format!("the kernel link's outbox is full ({OUTBOX_CAPACITY} commands)"));
                }
                None => state.queue.push_back(queued),
            }
        }
        self.wake.signal();
        Ok(id)
    }

    /// Like [`Outbox::send`], and the latest command of this key is sent again after every
    /// reconnect.
    pub fn send_sticky(&self, key: &str, cmd: &str, args: Value) -> Result<u64, String> {
        {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            match state.sticky.iter_mut().find(|(k, ..)| k == key) {
                Some(entry) => *entry = (key.to_string(), cmd.to_string(), args.clone()),
                None => state.sticky.push((key.to_string(), cmd.to_string(), args.clone())),
            }
        }
        self.send(cmd, args, Some(key))
    }

    /// A session begins: every sticky command goes first, then what was waiting.
    fn begin_session(&self) {
        if let Ok(mut state) = self.state.lock() {
            let mut stale = std::mem::take(&mut state.queue);
            let replay: Vec<_> = state.sticky.clone();
            for (key, cmd, args) in replay {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                state.queue.push_back(Queued { id, key: Some(key), cmd, args });
            }
            for q in stale.drain(..) {
                if q.key.as_ref().is_none_or(|k| !state.queue.iter().any(|n| n.key.as_ref() == Some(k))) {
                    state.queue.push_back(q);
                }
            }
        }
    }

    /// The front command, kept in the queue until `confirm_sent` so that a failed send
    /// does not lose it.
    fn front(&self) -> Option<(u64, String, Value)> {
        let state = self.state.lock().ok()?;
        state.queue.front().map(|q| (q.id, q.cmd.clone(), q.args.clone()))
    }

    fn confirm_sent(&self, id: u64) {
        if let Ok(mut state) = self.state.lock() {
            if state.queue.front().is_some_and(|q| q.id == id) {
                state.queue.pop_front();
            }
        }
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(pending) = pending.get_mut(&id) {
                pending.sent = true;
                // The kernel's time starts now, not while the command waited for the link.
                pending.deadline = Instant::now() + pending.timeout;
            }
        }
    }
}

struct OwnedEvent(HANDLE);
unsafe impl Send for OwnedEvent {}
unsafe impl Sync for OwnedEvent {}

impl OwnedEvent {
    fn new_manual_reset() -> Result<Self, String> {
        let h = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|e| format!("CreateEventW: {e}"))?;
        Ok(Self(h))
    }

    fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }

    fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.0);
        }
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// What a link does besides sending: which events it follows and who hears what arrives.
/// The callbacks run on the link's thread and must not block.
pub struct LinkConfig {
    pub subscriptions: Vec<&'static str>,
    /// After every (re)connect, once subscribed: the place to read the state again.
    pub on_connected: Box<dyn Fn() + Send + Sync>,
    pub on_inbound: Box<dyn Fn(Inbound) + Send + Sync>,
}

impl LinkConfig {
    /// A link that follows `desk-host-changed` and discards what arrives: the kernel's
    /// registration needs nothing else.
    fn registration_only() -> Self {
        Self {
            subscriptions: vec!["desk-host-changed"],
            on_connected: Box::new(|| {}),
            on_inbound: Box::new(|_| {}),
        }
    }
}

pub struct WsClientHandle {
    stopping: Arc<AtomicBool>,
    stop_event: Arc<OwnedEvent>,
    outbox: Arc<Outbox>,
    /// What the kernel was last told about this host.
    registration: Mutex<Option<HostRegistrationInfo>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WsClientHandle {
    /// Tells the kernel who this host is; sent again after every reconnect.
    pub fn set_registration(&self, info: HostRegistrationInfo) {
        self.send_registration(&info);
        if let Ok(mut slot) = self.registration.lock() {
            *slot = Some(info);
        }
    }

    /// The host's attach state as the kernel should know it; nothing is sent when it did
    /// not change.
    pub fn update_registration(&self, inspector_port: u16, attached: bool, ready: bool) {
        let Ok(mut slot) = self.registration.lock() else { return };
        let Some(info) = slot.as_mut() else { return };
        if info.inspector_port == inspector_port && info.attached == attached && info.ready == ready {
            return;
        }
        info.inspector_port = inspector_port;
        info.attached = attached;
        info.ready = ready;
        let info = info.clone();
        drop(slot);
        self.send_registration(&info);
    }

    fn send_registration(&self, info: &HostRegistrationInfo) {
        let _ = self.outbox.send_sticky(
            "host_register",
            "host_register",
            json!({
                "pid": info.pid,
                "generation": info.generation,
                "instanceNonce": info.instance_nonce,
                "inspectorPort": info.inspector_port,
                "attached": info.attached,
                "ready": info.ready,
            }),
        );
    }

    /// Whether a text field of the desktop has the keyboard; the full state is sent after
    /// every reconnect, so the kernel's flag cannot stay stale.
    pub fn report_input_focus(&self, focused: bool) {
        let _ = self.outbox.send_sticky(
            "set_desktop_input_focus",
            "set_desktop_input_focus",
            json!({ "focused": focused }),
        );
    }

    #[allow(dead_code)] // Used by the desk model, which sends its own commands.
    pub fn outbox(&self) -> &Arc<Outbox> {
        &self.outbox
    }

    pub fn stop(mut self) {
        self.stopping.store(true, Ordering::Release);
        self.stop_event.signal();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WsClientHandle {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.stop_event.signal();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn connect_loop(port: u16, token: String) -> Result<WsClientHandle, String> {
    connect_link(port, token, Outbox::new()?, LinkConfig::registration_only())
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn connect_with_registration(
    port: u16,
    token: String,
    initial: HostRegistrationInfo,
) -> Result<WsClientHandle, String> {
    let handle = connect_loop(port, token)?;
    handle.set_registration(initial);
    Ok(handle)
}

/// Starts the link's thread. It connects, follows `config.subscriptions`, replays the
/// outbox's sticky commands, and reconnects with back-off until the handle is stopped.
pub fn connect_link(
    port: u16,
    token: String,
    outbox: Arc<Outbox>,
    config: LinkConfig,
) -> Result<WsClientHandle, String> {
    let stopping = Arc::new(AtomicBool::new(false));
    let stop_event = Arc::new(OwnedEvent::new_manual_reset()?);

    let stop_flag = stopping.clone();
    let stop_sig = stop_event.clone();
    let link_outbox = outbox.clone();
    let thread = std::thread::Builder::new()
        .name("host-ws-client".into())
        .spawn(move || {
            let mut backoff_ms = 250u64;
            while !stop_flag.load(Ordering::Acquire) {
                let started = Instant::now();
                let session = run_session(port, &token, &stop_flag, &stop_sig, &link_outbox, &config);
                link_outbox.lose_sent();
                match session {
                    Ok(()) => {}
                    Err(err) => {
                        if stop_flag.load(Ordering::Acquire) {
                            break;
                        }
                        crate::desktop_host::note(&format!("ws_client: {err}"));
                    }
                }
                if stop_flag.load(Ordering::Acquire) {
                    break;
                }
                backoff_ms = if started.elapsed() >= STABLE_SESSION { 250 } else { (backoff_ms * 2).min(5000) };
                let wait = unsafe { WaitForSingleObject(stop_sig.0, backoff_ms as u32) };
                if wait == WAIT_OBJECT_0 {
                    break;
                }
            }
        })
        .map_err(|e| format!("spawn host-ws-client: {e}"))?;

    Ok(WsClientHandle { stopping, stop_event, outbox, registration: Mutex::new(None), thread: Some(thread) })
}

/// A send on the non-blocking socket that hits `WouldBlock` has queued the frame inside
/// tungstenite; the link flushes it when the socket is writable again.
fn send_text(ws: &mut tungstenite::WebSocket<TcpStream>, text: String) -> Result<(), String> {
    match ws.send(tungstenite::Message::Text(text)) {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(format!("send: {e}")),
    }
}

fn flush(ws: &mut tungstenite::WebSocket<TcpStream>) -> Result<(), String> {
    match ws.flush() {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(tungstenite::Error::ConnectionClosed) => Ok(()),
        Err(e) => Err(format!("flush: {e}")),
    }
}

/// Sends every queued command, front first; a command leaves the queue only after its send
/// was accepted.
fn drain_outbox(ws: &mut tungstenite::WebSocket<TcpStream>, outbox: &Outbox) -> Result<(), String> {
    while let Some((id, cmd, args)) = outbox.front() {
        send_text(ws, json!({ "id": id, "cmd": cmd, "args": args }).to_string())?;
        outbox.confirm_sent(id);
    }
    flush(ws)
}

fn run_session(
    port: u16,
    token: &str,
    stopping: &AtomicBool,
    stop_event: &OwnedEvent,
    outbox: &Outbox,
    config: &LinkConfig,
) -> Result<(), String> {
    let addr = (std::net::Ipv4Addr::LOCALHOST, port)
        .to_socket_addrs()
        .map_err(|e| format!("addr: {e}"))?
        .next()
        .ok_or_else(|| "no loopback address".to_string())?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("connect {addr}: {e}"))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(3)));

    let url = format!("ws://127.0.0.1:{port}");
    let (mut ws, _) =
        tungstenite::client(url, tcp).map_err(|e| format!("ws handshake failed: {e}"))?;

    ws.send(tungstenite::Message::Text(json!({ "hello": { "token": token } }).to_string()))
        .map_err(|e| format!("hello send: {e}"))?;

    let ready_txt = match ws.read() {
        Ok(tungstenite::Message::Text(t)) => t,
        Ok(_) => return Err("unexpected non-text ready frame".into()),
        Err(e) => return Err(format!("hello read: {e}")),
    };
    let ready_json: Value = serde_json::from_str(&ready_txt).map_err(|e| format!("bad ready json: {e}"))?;
    if ready_json.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(format!("ws hello rejected: {ready_json}"));
    }

    for event in &config.subscriptions {
        ws.send(tungstenite::Message::Text(json!({ "type": "subscribe", "event": event }).to_string()))
            .map_err(|e| format!("subscribe send: {e}"))?;
    }

    outbox.wake.reset();
    outbox.begin_session();
    (config.on_connected)();
    drain_outbox(&mut ws, outbox)?;

    let sock = SOCKET(ws.get_ref().as_raw_socket() as usize);
    let sock_event = unsafe { WSACreateEvent() }.map_err(|e| format!("WSACreateEvent: {e}"))?;
    struct SockGuard(SOCKET, WSAEVENT);
    impl Drop for SockGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = WSAEventSelect(self.0, WSAEVENT::default(), 0);
                let _ = WSACloseEvent(self.1);
            }
        }
    }
    let _guard = SockGuard(sock, sock_event);
    if unsafe { WSAEventSelect(sock, sock_event, (FD_READ | FD_WRITE | FD_CLOSE) as i32) } != 0 {
        return Err("WSAEventSelect failed".into());
    }

    let handles = [HANDLE(sock_event.0 as _), stop_event.0, outbox.wake.0];
    while !stopping.load(Ordering::Acquire) {
        loop {
            match ws.read() {
                Ok(tungstenite::Message::Close(_)) => return Ok(()),
                Ok(tungstenite::Message::Ping(data)) => {
                    send_pong(&mut ws, data)?;
                }
                Ok(tungstenite::Message::Text(text)) => {
                    match parse_inbound(&text) {
                        Some(Inbound::Reply { id, ok, body }) => {
                            let result = if ok { Ok(body.clone()) } else { Err(body.as_str().map_or_else(|| body.to_string(), str::to_string)) };
                            if !outbox.resolve(id, result) {
                                (config.on_inbound)(Inbound::Reply { id, ok, body });
                            }
                        }
                        Some(inbound) => (config.on_inbound)(inbound),
                        None => {}
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref err)) if err.kind() == ErrorKind::WouldBlock => {
                    break;
                }
                Err(_) => return Ok(()),
            }
        }

        // Idle with nothing awaited, the wait has no timeout: no timer wakes the host.
        let wait = outbox.expire().map_or(WSA_INFINITE, |due| due.as_millis().clamp(1, u32::MAX as u128) as u32);
        let res = unsafe { WSAWaitForMultipleEvents(&handles, false, wait, false) };
        if res.0 == windows::Win32::Networking::WinSock::WSA_WAIT_TIMEOUT {
            continue;
        }
        if res.0 == WAIT_OBJECT_0.0 + 1 {
            let _ = ws.close(None);
            return Ok(());
        }
        if res.0 == WAIT_OBJECT_0.0 + 2 {
            // Reset the manual-reset event before draining so a concurrent send re-signals it.
            outbox.wake.reset();
            drain_outbox(&mut ws, outbox)?;
            continue;
        }
        if res.0 == WSA_WAIT_FAILED {
            return Err("WSAWaitForMultipleEvents failed".into());
        }
        if res == WAIT_OBJECT_0 {
            let mut ne = WSANETWORKEVENTS::default();
            unsafe {
                let _ = WSAEnumNetworkEvents(sock, sock_event, &mut ne);
            }
            // The socket became writable: whatever tungstenite buffered can go out now.
            flush(&mut ws)?;
        }
    }
    Ok(())
}

fn send_pong(ws: &mut tungstenite::WebSocket<TcpStream>, data: Vec<u8>) -> Result<(), String> {
    match ws.send(tungstenite::Message::Pong(data)) {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(format!("pong: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn info(attached: bool, ready: bool) -> HostRegistrationInfo {
        HostRegistrationInfo {
            pid: 4242,
            generation: 9,
            instance_nonce: "nonce-xyz".into(),
            inspector_port: 51111,
            attached,
            ready,
        }
    }

    fn ok_reply() -> String {
        json!({ "v": 1, "ok": true, "result": { "ready": true } }).to_string()
    }

    #[test]
    fn ws_client_authenticates_and_exits_immediately_on_stop() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (hello_tx, hello_rx) = mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
            let _ = hello_tx.send(hello);
            ws.send(tungstenite::Message::Text(ok_reply())).unwrap();
            let _ = ws.read();
        });

        let client = connect_loop(port, "client-secret".into()).unwrap();
        let hello = hello_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(hello["hello"]["token"], "client-secret");

        let t0 = Instant::now();
        client.stop();
        assert!(t0.elapsed() < Duration::from_millis(300), "the link must wake at once from WSA_INFINITE on stop");
        let _ = server_thread.join();
    }

    #[test]
    fn registration_is_sent_on_connect_and_a_change_wakes_the_link_without_timers() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (frame_tx, frame_rx) = mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let hello = ws.read().unwrap().into_text().unwrap();
            let _ = frame_tx.send(hello);
            ws.send(tungstenite::Message::Text(ok_reply())).unwrap();
            while let Ok(msg) = ws.read() {
                if let tungstenite::Message::Text(txt) = msg {
                    let _ = frame_tx.send(txt.clone());
                    if let Some(id) = serde_json::from_str::<Value>(&txt).ok().and_then(|v| v.get("id").cloned()) {
                        let _ = ws.send(tungstenite::Message::Text(
                            json!({ "v": 1, "id": id, "ok": true, "result": { "ok": true } }).to_string(),
                        ));
                    }
                }
            }
        });

        let client = connect_with_registration(port, "client-secret".into(), info(false, false)).unwrap();
        let recv = || serde_json::from_str::<Value>(&frame_rx.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        let _hello = recv();
        let _sub = recv();
        let reg1 = recv();
        assert_eq!(reg1["cmd"], "host_register");
        assert_eq!(reg1["args"]["pid"], 4242);
        assert_eq!(reg1["args"]["generation"], 9);
        assert_eq!(reg1["args"]["instanceNonce"], "nonce-xyz");
        assert_eq!(reg1["args"]["inspectorPort"], 51111);
        assert_eq!(reg1["args"]["attached"], false);
        assert_eq!(reg1["args"]["ready"], false);

        let t_upd = Instant::now();
        client.update_registration(51111, true, true);
        let reg2 = recv();
        assert!(t_upd.elapsed() < Duration::from_millis(300), "a registration change must be event-driven");
        assert_eq!(reg2["cmd"], "host_register");
        assert_eq!(reg2["args"]["attached"], true);
        assert_eq!(reg2["args"]["ready"], true);

        client.stop();
        let _ = server_thread.join();
    }

    #[test]
    fn inbound_frames_are_parsed_into_events_gaps_and_replies() {
        assert_eq!(
            parse_inbound(r#"{"v":1,"type":"event","event":"desk-layout-changed","seq":7,"payload":{"rev":3},"snapshot":true}"#),
            Some(Inbound::Event { event: "desk-layout-changed".into(), seq: 7, payload: json!({"rev": 3}), snapshot: true })
        );
        assert_eq!(
            parse_inbound(r#"{"type":"event","event":"system-volume","seq":1,"payload":{"level":40}}"#),
            Some(Inbound::Event { event: "system-volume".into(), seq: 1, payload: json!({"level": 40}), snapshot: false })
        );
        assert_eq!(
            parse_inbound(r#"{"type":"gap","event":"desk-layout-changed","since":2,"error":"ring"}"#),
            Some(Inbound::Gap { event: "desk-layout-changed".into() })
        );
        assert_eq!(
            parse_inbound(r#"{"v":1,"id":5,"ok":true,"result":{"rev":9}}"#),
            Some(Inbound::Reply { id: 5, ok: true, body: json!({"rev": 9}) })
        );
        assert_eq!(
            parse_inbound(r#"{"v":1,"id":6,"ok":false,"error":"nothing stored"}"#),
            Some(Inbound::Reply { id: 6, ok: false, body: json!("nothing stored") })
        );
        assert_eq!(parse_inbound("not json"), None);
        assert_eq!(parse_inbound(r#"{"hello":1}"#), None);
    }

    #[test]
    fn a_keyed_command_replaces_the_unsent_one_and_a_full_outbox_is_an_error() {
        let outbox = Outbox::new().unwrap();
        let first = outbox.send("desk_position_set", json!({"id": "clock", "x": 1}), Some("pos:clock")).unwrap();
        let second = outbox.send("desk_position_set", json!({"id": "clock", "x": 2}), Some("pos:clock")).unwrap();
        assert!(second > first);
        let (id, _, args) = outbox.front().unwrap();
        assert_eq!((id, args["x"].clone()), (second, json!(2)), "the latest position wins");
        outbox.confirm_sent(id);
        assert!(outbox.front().is_none());

        for n in 0..OUTBOX_CAPACITY {
            outbox.send("set_system_volume", json!({"level": n}), None).unwrap();
        }
        assert!(outbox.send("set_system_volume", json!({"level": 1}), None).is_err());
    }

    // A command its requester was told had timed out must not run later, and the error
    // names it; a long command gets its own deadline, counted from when it left.
    #[test]
    fn a_timed_out_unsent_command_leaves_the_queue_and_a_long_one_waits_its_own_time() {
        let outbox = Outbox::new().unwrap();
        let (tx, rx) = mpsc::channel();
        let short = tx.clone();
        outbox.request_within("get_folders", json!({}), Duration::from_millis(1), Box::new(move |r| { let _ = short.send(r); })).unwrap();
        outbox.request_within("vpn_ping_all", json!({}), Duration::from_secs(60), Box::new(move |r| { let _ = tx.send(r); })).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let next_due = outbox.expire();
        let error = rx.try_recv().expect("the short request timed out").unwrap_err();
        assert!(error.contains("get_folders") && error.starts_with("timeout:"), "{error}");
        assert!(rx.try_recv().is_err(), "the long request is still waiting");
        assert!(next_due.is_some_and(|due| due > Duration::from_secs(50)), "{next_due:?}");
        let (id, cmd, _) = outbox.front().expect("the long command is still queued");
        assert_eq!(cmd, "vpn_ping_all", "the expired command was taken out of the queue");
        outbox.confirm_sent(id);
        assert!(outbox.front().is_none());
    }

    #[test]
    fn a_failed_send_keeps_the_command_and_a_sticky_one_is_replayed_first() {
        let outbox = Outbox::new().unwrap();
        outbox.send_sticky("set_desktop_input_focus", "set_desktop_input_focus", json!({"focused": true})).unwrap();
        let (id, ..) = outbox.front().unwrap();
        // Not confirmed: still there for the next attempt.
        assert_eq!(outbox.front().unwrap().0, id);
        outbox.confirm_sent(id);
        outbox.send("set_system_mute", json!({"muted": true}), None).unwrap();
        outbox.begin_session();
        let (_, cmd, args) = outbox.front().unwrap();
        assert_eq!((cmd.as_str(), args["focused"].clone()), ("set_desktop_input_focus", json!(true)), "the focus state goes out again after a reconnect, before older commands");
    }

    #[test]
    fn the_link_subscribes_delivers_events_replies_and_sticky_state_after_a_reconnect() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen_tx, seen_rx) = mpsc::channel::<String>();
        let server_thread = std::thread::spawn(move || {
            for session in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let _hello = ws.read().unwrap();
                ws.send(tungstenite::Message::Text(ok_reply())).unwrap();
                loop {
                    let Ok(tungstenite::Message::Text(txt)) = ws.read() else { break };
                    let _ = seen_tx.send(format!("{session}:{txt}"));
                    let v: Value = serde_json::from_str(&txt).unwrap();
                    if v["cmd"] == "desk_layout_get" {
                        let _ = ws.send(tungstenite::Message::Text(json!({"v":1,"id":v["id"],"ok":true,"result":{"rev":11,"config":null}}).to_string()));
                        let _ = ws.send(tungstenite::Message::Text(json!({"v":1,"type":"event","event":"desk-layout-changed","seq":4,"payload":{"rev":12,"positions":{}}}).to_string()));
                        if session == 0 {
                            // Drop the connection: the link must come back and replay.
                            break;
                        }
                    }
                }
            }
        });

        let outbox = Outbox::new().unwrap();
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let on_outbox = outbox.clone();
        let client = connect_link(
            port,
            "t".into(),
            outbox.clone(),
            LinkConfig {
                subscriptions: vec!["desk-layout-changed"],
                on_connected: Box::new(move || {
                    let _ = on_outbox.send("desk_layout_get", json!({}), Some("snapshot:layout"));
                }),
                on_inbound: Box::new(move |inbound| {
                    let _ = inbound_tx.send(inbound);
                }),
            },
        )
        .unwrap();
        client.report_input_focus(true);

        let got = |rx: &mpsc::Receiver<Inbound>| rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(got(&inbound_rx), Inbound::Reply { ok: true, .. }));
        assert!(matches!(got(&inbound_rx), Inbound::Event { ref event, .. } if event == "desk-layout-changed"));
        // After the drop the link reconnects, subscribes and snapshots again.
        assert!(matches!(got(&inbound_rx), Inbound::Reply { ok: true, .. }));

        let mut frames = Vec::new();
        while let Ok(frame) = seen_rx.recv_timeout(Duration::from_millis(300)) {
            frames.push(frame);
        }
        let in_session = |n: u8, needle: &str| frames.iter().any(|f| f.starts_with(&format!("{n}:")) && f.contains(needle));
        for session in 0..2 {
            assert!(in_session(session, r#""type":"subscribe""#), "session {session} subscribes");
            assert!(in_session(session, "set_desktop_input_focus"), "session {session} reports the input focus");
            assert!(in_session(session, "desk_layout_get"), "session {session} reads the layout");
        }
        client.stop();
        let _ = server_thread.join();
    }
}
