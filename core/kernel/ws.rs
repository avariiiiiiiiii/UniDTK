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
//! token. That is the accepted risk recorded in `docs/cliudtk.md` §10.8, and it is why
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
/// Once authenticated, how long a silent connection is kept.
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

/// How long a read waits before the loop drains the outgoing event queue. Small enough
/// that a pushed event feels immediate; the socket stays one object owned by one thread,
/// so no lock ever guards it.
const READ_POLL: Duration = Duration::from_millis(50);

fn serve(stream: TcpStream, server: IpcServer, token: String) {
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
    if socket.send(tungstenite::Message::Text(ready)).is_err() {
        return;
    }
    let _ = socket.get_ref().set_read_timeout(Some(READ_POLL));

    // Subscriptions are per connection: the broker channel and the set of event names
    // this client asked for.
    let (sender, receiver) = crossbeam_channel::bounded::<crate::events::Frame>(64);
    let mut subscriber_id: Option<u64> = None;
    let mut subscribed: Vec<String> = Vec::new();
    let mut last_activity = std::time::Instant::now();

    loop {
        // A connection that says nothing for long is dropped, so a forgotten page cannot
        // hold a slot forever.
        if last_activity.elapsed() > IDLE_TIMEOUT {
            break;
        }
        // Push whatever the broker buffered since the last pass. A state event is
        // coalesced to its newest frame: a 30 Hz spectrum would otherwise become thirty
        // messages per pass, and only the current value means anything to the client.
        let mut merged: Vec<crate::events::Frame> = Vec::new();
        while let Ok(frame) = receiver.try_recv() {
            if !subscribed.iter().any(|name| name == &frame.event) {
                continue;
            }
            if crate::events::is_state(&frame.event) {
                match merged.iter_mut().find(|kept| kept.event == frame.event) {
                    Some(slot) => *slot = frame,
                    None => merged.push(frame),
                }
            } else {
                merged.push(frame);
            }
        }
        for frame in merged {
            let message = serde_json::json!({
                "v": 1,
                "type": "event",
                "event": frame.event,
                "seq": frame.seq,
                "payload": frame.payload,
            })
            .to_string();
            if socket.send(tungstenite::Message::Text(message)).is_err() {
                break;
            }
        }

        let message = match socket.read() {
            Ok(tungstenite::Message::Text(text)) => text,
            Ok(tungstenite::Message::Close(_)) => break,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        };
        last_activity = std::time::Instant::now();

        let request: serde_json::Value = match serde_json::from_str(&message) {
            Ok(value) => value,
            Err(error) => {
                if socket
                    .send(tungstenite::Message::Text(refusal(&format!(
                        "bad json: {error}"
                    ))))
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };

        // Push protocol: subscribe / unsubscribe, numbered by the broker.
        if request.get("type").and_then(|kind| kind.as_str()) == Some("subscribe") {
            let event = request
                .get("event")
                .and_then(|name| name.as_str())
                .unwrap_or_default()
                .to_string();
            if event.is_empty() {
                let _ = socket.send(tungstenite::Message::Text(refusal(
                    "subscribe needs an event name",
                )));
                continue;
            }
            if subscriber_id.is_none() {
                subscriber_id = Some(crate::events::subscribe(sender.clone()));
            }
            if !subscribed.iter().any(|name| name == &event) {
                subscribed.push(event.clone());
            }

            // What the client missed: the newest value for a state event, or the frames
            // after its cursor. An explicit `gap` beats a silently incomplete stream.
            let cursor = request.get("since").and_then(|value| value.as_u64());
            match cursor {
                None => {
                    if let Some(newest) = crate::events::snapshot(&event) {
                        // Tagged as a snapshot: this frame is a catch-up, not a delivery,
                        // and the Tauri door has no twin for it. Without the tag a
                        // comparison pass mistakes it for a live event and reports a
                        // divergence that never happened.
                        let message = serde_json::json!({
                            "v": 1,
                            "type": "event",
                            "snapshot": true,
                            "event": newest.event,
                            "seq": newest.seq,
                            "payload": newest.payload,
                        })
                        .to_string();
                        let _ = socket.send(tungstenite::Message::Text(message));
                    }
                }
                Some(cursor) => match crate::events::replay(&event, cursor) {
                    Some(frames) => {
                        for frame in frames {
                            let message = serde_json::json!({
                                "v": 1,
                                "type": "event",
                                "event": frame.event,
                                "seq": frame.seq,
                                "payload": frame.payload,
                            })
                            .to_string();
                            if socket.send(tungstenite::Message::Text(message)).is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        let gap = serde_json::json!({
                            "v": 1,
                            "type": "gap",
                            "event": event,
                            "since": cursor,
                            "error": "the buffer no longer covers this cursor; resync the state",
                        })
                        .to_string();
                        let _ = socket.send(tungstenite::Message::Text(gap));
                    }
                },
            }
            continue;
        }

        if request.get("type").and_then(|kind| kind.as_str()) == Some("unsubscribe") {
            if let Some(event) = request.get("event").and_then(|name| name.as_str()) {
                subscribed.retain(|name| name != event);
            }
            continue;
        }

        // The UI door, not the socket door: this connection proved it holds the per-run
        // token (handshake above), which is what the page itself receives. That is what
        // lets the interface drive state changes over its own transport.
        let reply = crate::ipc::respond(
            request.get("id").cloned(),
            server.dispatch(&request, unidtk_api::commands::Door::Ui),
        );
        if socket.send(tungstenite::Message::Text(reply)).is_err() {
            break;
        }
    }

    if let Some(id) = subscriber_id {
        crate::events::unsubscribe(id);
    }
}
