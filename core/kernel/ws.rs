//! WebSocket door for the application's own UI (S4).
//!
//! Two doors now exist with different jobs. The TCP door is for local tools
//! (`udtk invoke`, diagnostics) and stays read-only by policy. This one is for the
//! page: it is guarded by a **per-run token**, and the first frame must be a
//! handshake carrying that token — nothing is parsed as a command before it is
//! accepted, and a wrong or missing token closes the connection immediately.
//!
//! Trust boundary, stated honestly: the token keeps out other local processes that do
//! not know it, and any web page that tries to open the socket. It does **not** defend
//! against code already running inside this process — CDP can read the page's own
//! token. That is the accepted risk recorded in `docs/udtk/ch06-design-decisions.md`, and it is why
//! the token does not replace the read-only policy on the open door.
//!
//! Both doors answer through the same dispatcher (`ipc::IpcServer::dispatch`), so a
//! command cannot behave differently depending on how it arrived.

use std::net::{Ipv4Addr, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

use crate::ipc::IpcServer;

/// How long a fresh connection has to present its handshake.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(3);
/// How long a quiet *unsubscribed* connection is kept before dropping. Once
/// subscribed, peer disconnects arrive immediately via `FD_CLOSE` so idle
/// connections sleep with zero periodic timer wakes (`WSA_INFINITE`).
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

static PORT: OnceLock<u16> = OnceLock::new();
static TOKEN: OnceLock<String> = OnceLock::new();

/// The port of this run's UI door. Public knowledge on purpose — the token is not.
pub fn port() -> Option<u16> {
    PORT.get().copied()
}

/// The token of this run, handed to the page once at boot.
pub fn token() -> Option<String> {
    TOKEN.get().cloned()
}

/// Makes a fresh token. Derived from the OS-seeded hasher (`RandomState`) rather than
/// pulling in a random-number dependency: it only has to be unpredictable to other
/// local processes, and it is regenerated on every launch.
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u64(std::process::id() as u64);
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

/// Binds an ephemeral loopback port and serves connections until the process ends.
/// Returns the port, or `None` when the door could not be opened (the application
/// keeps working in that case — this door is not load-bearing yet).
pub fn start(server: IpcServer, token: String) -> Option<u16> {
    let listener = match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(error) => {
            crate::log_line(format!("ws: bind failed: {error}"));
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    let _ = PORT.set(port);
    let _ = TOKEN.set(token.clone());

    std::thread::Builder::new()
        .name("unidtk-ws".into())
        .spawn(move || {
            crate::log_line(format!(
                "ws: listening on 127.0.0.1:{port} (handshake required)"
            ));
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let server = server.clone();
                let token = token.clone();
                std::thread::spawn(move || serve(stream, server, token));
            }
        })
        .ok()?;
    Some(port)
}

fn refusal(message: &str) -> String {
    serde_json::json!({ "v": 1, "ok": false, "error": message }).to_string()
}

#[cfg(windows)]
mod waiter {
    use std::net::TcpStream;
    use std::os::windows::io::AsRawSocket;
    use std::sync::Arc;
    use std::time::Duration;

    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
    use windows::Win32::Networking::WinSock::{
        WSACloseEvent, WSACreateEvent, WSAEnumNetworkEvents, WSAEventSelect,
        WSAWaitForMultipleEvents, FD_CLOSE, FD_READ, SOCKET, WSAEVENT, WSANETWORKEVENTS,
        WSA_INFINITE, WSA_WAIT_FAILED,
    };
    use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

    struct WakeHandle(HANDLE);
    unsafe impl Send for WakeHandle {}
    unsafe impl Sync for WakeHandle {}
    impl Drop for WakeHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    pub struct EventWaiter {
        sock: SOCKET,
        sock_event: WSAEVENT,
        wake: Arc<WakeHandle>,
    }

    impl EventWaiter {
        pub fn new(stream: &TcpStream) -> Option<Self> {
            let sock = SOCKET(stream.as_raw_socket() as usize);
            let sock_event = unsafe { WSACreateEvent() }.ok()?;
            // WSAEventSelect automatically puts `sock` into non-blocking mode.
            if unsafe { WSAEventSelect(sock, sock_event, (FD_READ | FD_CLOSE) as i32) } != 0 {
                unsafe {
                    let _ = WSACloseEvent(sock_event);
                }
                return None;
            }
            let wake_handle = unsafe { CreateEventW(None, true, false, None) }.ok()?;
            Some(Self {
                sock,
                sock_event,
                wake: Arc::new(WakeHandle(wake_handle)),
            })
        }

        pub fn wake_hook(&self) -> crate::events::WakeHook {
            let wake = self.wake.clone();
            Arc::new(move || unsafe {
                let _ = SetEvent(wake.0);
            })
        }

        pub fn reset_wake(&self) {
            unsafe {
                let _ = ResetEvent(self.wake.0);
            }
        }

        pub fn wait(&self, timeout: Option<Duration>) -> bool {
            let handles = [HANDLE(self.sock_event.0 as _), self.wake.0];
            let ms = timeout
                .map(|d| d.as_millis().min((u32::MAX - 1) as u128) as u32)
                .unwrap_or(WSA_INFINITE);
            let res = unsafe { WSAWaitForMultipleEvents(&handles, false, ms, false) };
            if res == WAIT_FAILED || res.0 == WSA_WAIT_FAILED {
                return false;
            }
            if res == WAIT_OBJECT_0 {
                let mut net_events = WSANETWORKEVENTS::default();
                unsafe {
                    let _ = WSAEnumNetworkEvents(self.sock, self.sock_event, &mut net_events);
                }
            }
            true
        }
    }

    impl Drop for EventWaiter {
        fn drop(&mut self) {
            unsafe {
                let _ = WSAEventSelect(self.sock, WSAEVENT::default(), 0);
                let _ = WSACloseEvent(self.sock_event);
            }
        }
    }
}

fn send_ws_message(
    socket: &mut tungstenite::WebSocket<TcpStream>,
    msg: tungstenite::Message,
) -> bool {
    match socket.send(msg) {
        Ok(()) => true,
        Err(tungstenite::Error::Io(ref err)) if err.kind() == std::io::ErrorKind::WouldBlock => {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                match socket.flush() {
                    Ok(()) => return true,
                    Err(tungstenite::Error::Io(ref io))
                        if io.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => return false,
                }
            }
            false
        }
        Err(_) => false,
    }
}

fn send_text(socket: &mut tungstenite::WebSocket<TcpStream>, text: String) -> bool {
    send_ws_message(socket, tungstenite::Message::Text(text))
}

/// The connection's commands that run off its thread (`r[ipc.long-commands-off-thread]`).
///
/// A long command (a server check, a connect) used to run inline, so every command the
/// host sent after it waited, and the host's own five-second deadline turned them all
/// into errors: the folder list came back empty and the desk drew no folders. Now a
/// worker runs it and leaves the reply here; only the connection's thread writes the
/// socket, because a WebSocket is not shared between threads.
#[derive(Default)]
struct LongWork {
    state: std::sync::Mutex<LongState>,
}

#[derive(Default)]
struct LongState {
    /// The names running now: one of each at a time, at most [`MAX_LONG_IN_FLIGHT`].
    running: Vec<String>,
    replies: std::collections::VecDeque<String>,
}

/// Long commands one connection may run at once.
const MAX_LONG_IN_FLIGHT: usize = 4;

impl LongWork {
    fn state(&self) -> std::sync::MutexGuard<'_, LongState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn start(&self, cmd: &str) -> Result<(), String> {
        let mut state = self.state();
        if state.running.iter().any(|running| running == cmd) {
            return Err(format!("busy: '{cmd}' is already running"));
        }
        if state.running.len() >= MAX_LONG_IN_FLIGHT {
            return Err(format!("busy: {MAX_LONG_IN_FLIGHT} long commands are already running"));
        }
        state.running.push(cmd.to_string());
        Ok(())
    }

    fn finish(&self, cmd: &str, reply: String) {
        let mut state = self.state();
        state.running.retain(|running| running != cmd);
        state.replies.push_back(reply);
    }

    /// A start whose worker never ran: the name is free again, and no reply is left.
    fn abandon(&self, cmd: &str) {
        self.state().running.retain(|running| running != cmd);
    }

    fn busy(&self) -> bool {
        !self.state().running.is_empty()
    }

    fn take_replies(&self) -> Vec<String> {
        self.state().replies.drain(..).collect()
    }
}

/// Runs `request` on a worker and wakes the connection when its reply is ready.
fn spawn_long<F>(
    cmd: &str,
    request: serde_json::Value,
    dispatch: &std::sync::Arc<F>,
    work: &std::sync::Arc<LongWork>,
    wake: &crate::events::WakeHook,
) -> Result<(), String>
where
    F: Fn(&serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
{
    work.start(cmd)?;
    let (cmd, dispatch, work, wake) = (cmd.to_string(), dispatch.clone(), work.clone(), wake.clone());
    let name = format!("unidtk-ws-{cmd}");
    let spawned = std::thread::Builder::new().name(name).spawn({
        let (cmd, work) = (cmd.clone(), work.clone());
        move || {
            let reply = crate::ipc::respond(request.get("id").cloned(), dispatch(&request));
            // A connection that closed meanwhile never drains this: the reply goes with it.
            work.finish(&cmd, reply);
            wake();
        }
    });
    spawned.map(|_| ()).map_err(|error| {
        work.abandon(&cmd);
        format!("could not start '{cmd}': {error}")
    })
}

fn handle_client_text<F, L>(
    text: &str,
    socket: &mut tungstenite::WebSocket<TcpStream>,
    subscription: &crate::events::Subscription,
    dispatch: &std::sync::Arc<F>,
    is_long: &L,
    work: &std::sync::Arc<LongWork>,
    wake: &crate::events::WakeHook,
) -> bool
where
    F: Fn(&serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    L: Fn(&str) -> bool,
{
    let request: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return send_text(socket, refusal(&format!("bad json: {error}")));
        }
    };

    if request.get("type").and_then(|kind| kind.as_str()) == Some("subscribe") {
        let event = request
            .get("event")
            .and_then(|name| name.as_str())
            .unwrap_or_default();
        if event.is_empty() {
            return send_text(socket, refusal("subscribe needs an event name"));
        }
        let cursor = request.get("since").and_then(|value| value.as_u64());
        match subscription.subscribe_topic(event, cursor) {
            crate::events::SubscribeInit::Snapshot(Some(newest)) => {
                // Tagged as a snapshot: catch-up frame, not a live delivery.
                let message = serde_json::json!({
                    "v": 1,
                    "type": "event",
                    "snapshot": true,
                    "event": newest.event,
                    "seq": newest.seq,
                    "payload": newest.payload,
                })
                .to_string();
                if !send_text(socket, message) {
                    return false;
                }
            }
            crate::events::SubscribeInit::Snapshot(None) => {}
            crate::events::SubscribeInit::Replay(frames) => {
                for frame in frames {
                    let message = serde_json::json!({
                        "v": 1,
                        "type": "event",
                        "event": frame.event,
                        "seq": frame.seq,
                        "payload": frame.payload,
                    })
                    .to_string();
                    if !send_text(socket, message) {
                        return false;
                    }
                }
            }
            crate::events::SubscribeInit::Gap { since } => {
                let gap = serde_json::json!({
                    "v": 1,
                    "type": "gap",
                    "event": event,
                    "since": since,
                    "error": "the buffer no longer covers this cursor; resync the state",
                })
                .to_string();
                if !send_text(socket, gap) {
                    return false;
                }
            }
        }
        return true;
    }

    if request.get("type").and_then(|kind| kind.as_str()) == Some("unsubscribe") {
        if let Some(event) = request.get("event").and_then(|name| name.as_str()) {
            subscription.unsubscribe_topic(event);
        }
        return true;
    }

    let cmd = request.get("cmd").and_then(|cmd| cmd.as_str()).unwrap_or_default().to_string();
    if is_long(&cmd) {
        // The reply arrives later through `work`; the commands behind this one go on.
        return match spawn_long(&cmd, request.clone(), dispatch, work, wake) {
            Ok(()) => true,
            Err(refusal) => send_text(socket, crate::ipc::respond(request.get("id").cloned(), Err(refusal))),
        };
    }
    let reply = crate::ipc::respond(request.get("id").cloned(), dispatch(&request));
    send_text(socket, reply)
}

fn serve(stream: TcpStream, server: IpcServer, token: String) {
    let long = server.clone();
    serve_with_dispatch(
        stream,
        token,
        move |request| server.dispatch(request, unidtk_api::commands::Door::Ui),
        move |cmd| long.is_long(cmd),
    );
}

fn serve_with_dispatch<F, L>(stream: TcpStream, token: String, dispatch: F, is_long: L)
where
    F: Fn(&serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    L: Fn(&str) -> bool,
{
    let dispatch = std::sync::Arc::new(dispatch);
    let work = std::sync::Arc::new(LongWork::default());
    let _ = stream.set_read_timeout(Some(HANDSHAKE_DEADLINE));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut socket = match tungstenite::accept(stream) {
        Ok(socket) => socket,
        Err(error) => {
            crate::log_line(format!("ws: websocket handshake failed: {error}"));
            return;
        }
    };

    // The token comes first, on its own frame. Only after it is accepted does the
    // connection get to say a command.
    let greeting = match socket.read() {
        Ok(tungstenite::Message::Text(text)) => text,
        _ => {
            let _ = socket.close(None);
            return;
        }
    };
    let presented = serde_json::from_str::<serde_json::Value>(&greeting)
        .ok()
        .and_then(|value| {
            value
                .get("hello")
                .and_then(|hello| hello.get("token"))
                .and_then(|token| token.as_str())
                .map(str::to_string)
        });
    if presented.as_deref() != Some(token.as_str()) {
        let _ = socket.send(tungstenite::Message::Text(refusal(
            "handshake required: send {\"hello\":{\"token\":\"…\"}} as the first frame",
        )));
        let _ = socket.close(None);
        crate::log_line("ws: connection rejected (missing or wrong token)");
        return;
    }

    let ready = serde_json::json!({ "v": 1, "ok": true, "result": { "ready": true } }).to_string();
    if !send_text(&mut socket, ready) {
        return;
    }

    #[cfg(windows)]
    let Some(waiter) = waiter::EventWaiter::new(socket.get_ref()) else {
        crate::log_line("ws: failed to initialize Winsock event waiter");
        return;
    };
    #[cfg(windows)]
    let wake = waiter.wake_hook();
    #[cfg(windows)]
    let subscription = crate::events::subscribe_with_wake(wake.clone());
    let mut last_activity = std::time::Instant::now();

    'conn: loop {
        // A connection waiting for its own long command is not idle.
        if !subscription.has_topics() && !work.busy() && last_activity.elapsed() > IDLE_TIMEOUT {
            break 'conn;
        }

        // Clear the manual-reset wake event strictly BEFORE draining the queues so a
        // concurrent publish or a finished long command that calls SetEvent during/after
        // the drain cannot be lost.
        #[cfg(windows)]
        waiter.reset_wake();

        for reply in work.take_replies() {
            if !send_text(&mut socket, reply) {
                break 'conn;
            }
        }

        for item in subscription.drain() {
            let message = match item {
                crate::events::Delivery::Frame(frame) => serde_json::json!({
                    "v": 1,
                    "type": "event",
                    "event": frame.event,
                    "seq": frame.seq,
                    "payload": frame.payload,
                })
                .to_string(),
                crate::events::Delivery::Gap { event, since } => serde_json::json!({
                    "v": 1,
                    "type": "gap",
                    "event": event,
                    "since": since,
                    "error": "subscriber queue overflowed; resync state",
                })
                .to_string(),
            };
            if !send_text(&mut socket, message) {
                break 'conn;
            }
        }

        // Drain every frame already buffered in TCP / tungstenite before waiting.
        loop {
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    last_activity = std::time::Instant::now();
                    if !handle_client_text(&text, &mut socket, &subscription, &dispatch, &is_long, &work, &wake) {
                        break 'conn;
                    }
                }
                Ok(tungstenite::Message::Close(_)) => break 'conn,
                Ok(tungstenite::Message::Ping(payload)) => {
                    last_activity = std::time::Instant::now();
                    if !send_ws_message(&mut socket, tungstenite::Message::Pong(payload)) {
                        break 'conn;
                    }
                }
                Ok(_) => {
                    last_activity = std::time::Instant::now();
                }
                Err(tungstenite::Error::Io(ref error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break 'conn,
            }
        }

        let timeout = if subscription.has_topics() || work.busy() {
            None
        } else {
            let remaining = IDLE_TIMEOUT.saturating_sub(last_activity.elapsed());
            if remaining.is_zero() {
                break 'conn;
            }
            Some(remaining)
        };
        #[cfg(windows)]
        if !waiter.wait(timeout) {
            break 'conn;
        }
    }

    drop(subscription);
    #[cfg(windows)]
    drop(waiter);
    let _ = socket.close(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn ws_multi_topic_subscribe_and_overflow_gap_delivery() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "test-ws-token".to_string();
        let server_token = token.clone();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_with_dispatch(
                stream,
                server_token,
                move |req| {
                    if req.get("cmd").and_then(|v| v.as_str()) == Some("hold") {
                        let _ = entered_tx.send(());
                        let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(2));
                    }
                    Ok(serde_json::json!({ "ok": true }))
                },
                |_| false,
            );
        });

        let (mut client, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();
        match client.get_mut() {
            tungstenite::stream::MaybeTlsStream::Plain(tcp) => {
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            }
            _ => unreachable!("loopback ws is plain tcp"),
        }
        client
            .send(tungstenite::Message::Text(
                serde_json::json!({ "hello": { "token": token } }).to_string(),
            ))
            .unwrap();
        let ready: serde_json::Value =
            serde_json::from_str(&client.read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ready["ok"], true);

        client
            .send(tungstenite::Message::Text(
                serde_json::json!({ "type": "subscribe", "event": "ws-multi-a" }).to_string(),
            ))
            .unwrap();
        client
            .send(tungstenite::Message::Text(
                serde_json::json!({ "type": "subscribe", "event": "ws-overflow-gap" }).to_string(),
            ))
            .unwrap();
        // Hold the server loop inside dispatch so the 80-frame burst fills the 64-slot queue deterministically.
        client
            .send(tungstenite::Message::Text(
                serde_json::json!({ "id": 1, "cmd": "hold" }).to_string(),
            ))
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        crate::events::publish("ws-multi-a", serde_json::json!({ "done": true }));
        for i in 0..80 {
            crate::events::publish("ws-overflow-gap", serde_json::json!({ "i": i }));
        }
        let _ = release_tx.send(());

        let mut saw_multi = false;
        let mut saw_gap = false;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) && !(saw_multi && saw_gap) {
            match client.read() {
                Ok(tungstenite::Message::Text(txt)) => {
                    let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
                    if v["type"] == "gap" && v["event"] == "ws-overflow-gap" {
                        saw_gap = true;
                    }
                    if v["type"] == "event" && v["event"] == "ws-multi-a" {
                        saw_multi = true;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = client.close(None);
        drop(client);
        let _ = handle.join();
        assert!(saw_multi, "first topic delivery must remain intact");
        assert!(saw_gap, "overflow on bounded ws subscriber must deliver explicit gap frame");
    }

    #[test]
    fn ws_wakes_immediately_on_publish_and_exits_promptly_on_close() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "test-ws-wake".to_string();
        let server_token = token.clone();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_with_dispatch(stream, server_token, |_| Ok(serde_json::json!({})), |_| false);
        });

        let (mut client, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();
        match client.get_mut() {
            tungstenite::stream::MaybeTlsStream::Plain(tcp) => {
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            }
            _ => unreachable!("loopback ws is plain tcp"),
        }
        client
            .send(tungstenite::Message::Text(
                serde_json::json!({ "hello": { "token": token } }).to_string(),
            ))
            .unwrap();
        let _ = client.read().unwrap();
        client
            .send(tungstenite::Message::Text(
                serde_json::json!({ "type": "subscribe", "event": "ws-wake-ping" }).to_string(),
            ))
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));

        let mut max_elapsed = Duration::ZERO;
        for round in 1..=5 {
            // Offset across 0..50ms phase so any polling timer would hit at least one stall.
            std::thread::sleep(Duration::from_millis(round * 7));
            let t0 = Instant::now();
            crate::events::publish("ws-wake-ping", serde_json::json!({ "tick": round }));
            let msg = client.read().unwrap().into_text().unwrap();
            let elapsed = t0.elapsed();
            max_elapsed = max_elapsed.max(elapsed);
            let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
            assert_eq!(v["event"], "ws-wake-ping");
        }
        assert!(
            max_elapsed < Duration::from_millis(20),
            "event-driven ws must wake well under old 50ms poll interval (max={max_elapsed:?})"
        );

        drop(client);
        let close_start = Instant::now();
        handle.join().unwrap();
        assert!(
            close_start.elapsed() < Duration::from_millis(250),
            "ws server thread must exit immediately when peer drops"
        );
    }

    // r[ipc.long-commands-off-thread]: the 22-second server check held every later command
    // of the host, which then timed out at five seconds and drew the desk without folders.
    #[test]
    fn a_long_command_does_not_hold_the_commands_behind_it() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "test-ws-long".to_string();
        let server_token = token.clone();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_with_dispatch(
                stream,
                server_token,
                move |req| match req["cmd"].as_str() {
                    Some("slow") => {
                        let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(5));
                        Ok(serde_json::json!("slow done"))
                    }
                    _ => Ok(serde_json::json!("fast")),
                },
                |cmd| cmd == "slow",
            );
        });

        let (mut client, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();
        match client.get_mut() {
            tungstenite::stream::MaybeTlsStream::Plain(tcp) => {
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            }
            _ => unreachable!("loopback ws is plain tcp"),
        }
        let mut send = |value: serde_json::Value| client.send(tungstenite::Message::Text(value.to_string())).unwrap();
        send(serde_json::json!({ "hello": { "token": token } }));
        send(serde_json::json!({ "id": 1, "cmd": "slow" }));
        send(serde_json::json!({ "id": 2, "cmd": "slow" }));
        send(serde_json::json!({ "id": 3, "cmd": "fast" }));
        let mut read = || -> serde_json::Value {
            serde_json::from_str(&client.read().unwrap().into_text().unwrap()).unwrap()
        };
        assert_eq!(read()["result"]["ready"], true);

        // The second "slow" is refused while the first runs, and "fast" answers at once.
        let t0 = Instant::now();
        let busy = read();
        assert_eq!(busy["id"], 2);
        assert!(busy["error"].as_str().unwrap().contains("busy"), "{busy}");
        let fast = read();
        assert_eq!((fast["id"].clone(), fast["result"].clone()), (serde_json::json!(3), serde_json::json!("fast")));
        assert!(t0.elapsed() < Duration::from_secs(1), "the fast command waited for the slow one");

        release_tx.send(()).unwrap();
        let slow = read();
        assert_eq!((slow["id"].clone(), slow["result"].clone()), (serde_json::json!(1), serde_json::json!("slow done")));

        drop(client);
        handle.join().unwrap();
    }
}
