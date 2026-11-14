//! Event-driven zero-wake WebSocket client connecting `unidtk-desktop` to the
//! kernel UI door (`ws.rs`). Uses `WSAEventSelect` + Win32 manual-reset Event so
//! idle waits consume 0 timers and shutdown wakes instantaneously.

use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Networking::WinSock::{
    WSACloseEvent, WSACreateEvent, WSAEnumNetworkEvents, WSAEventSelect,
    WSAWaitForMultipleEvents, FD_CLOSE, FD_READ, SOCKET, WSAEVENT, WSANETWORKEVENTS,
    WSA_INFINITE, WSA_WAIT_FAILED,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};

use std::os::windows::io::AsRawSocket;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRegistrationInfo {
    pub pid: u32,
    pub generation: u64,
    pub instance_nonce: String,
    pub inspector_port: u16,
    pub attached: bool,
    pub ready: bool,
}

struct RegistrationState {
    info: HostRegistrationInfo,
    dirty: bool,
    focus_pending: Option<bool>,
}

type SharedRegistration = Arc<(Mutex<RegistrationState>, OwnedEvent)>;

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

pub struct WsClientHandle {
    stopping: Arc<AtomicBool>,
    stop_event: Arc<OwnedEvent>,
    registration: Option<SharedRegistration>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WsClientHandle {
    pub fn update_registration(&self, inspector_port: u16, attached: bool, ready: bool) {
        let Some((state, ev)) = self.registration.as_deref() else { return };
        if let Ok(mut guard) = state.lock() {
            if guard.info.inspector_port == inspector_port
                && guard.info.attached == attached
                && guard.info.ready == ready
            {
                return;
            }
            guard.info.inspector_port = inspector_port;
            guard.info.attached = attached;
            guard.info.ready = ready;
            guard.dirty = true;
        }
        ev.signal();
    }

    pub fn report_input_focus(&self, focused: bool) {
        let Some((state, ev)) = self.registration.as_deref() else { return };
        if let Ok(mut guard) = state.lock() {
            guard.focus_pending = Some(focused);
        }
        ev.signal();
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
    connect_internal(port, token, None)
}

pub fn connect_with_registration(
    port: u16,
    token: String,
    initial: HostRegistrationInfo,
) -> Result<WsClientHandle, String> {
    let reg = Arc::new((
        Mutex::new(RegistrationState {
            info: initial,
            dirty: true,
            focus_pending: None,
        }),
        OwnedEvent::new_manual_reset()?,
    ));
    connect_internal(port, token, Some(reg))
}

fn connect_internal(
    port: u16,
    token: String,
    registration: Option<SharedRegistration>,
) -> Result<WsClientHandle, String> {
    let stopping = Arc::new(AtomicBool::new(false));
    let stop_event = Arc::new(OwnedEvent::new_manual_reset()?);

    let stop_flag = stopping.clone();
    let stop_sig = stop_event.clone();
    let reg_clone = registration.clone();
    let thread = std::thread::Builder::new()
        .name("host-ws-client".into())
        .spawn(move || {
            let mut backoff_ms = 250u64;
            while !stop_flag.load(Ordering::Acquire) {
                match run_session(port, &token, &stop_flag, &stop_sig, reg_clone.as_ref()) {
                    Ok(()) => {
                        backoff_ms = 250;
                    }
                    Err(err) => {
                        if stop_flag.load(Ordering::Acquire) {
                            break;
                        }
                        crate::desktop_host::note(&format!("ws_client: {err}"));
                        backoff_ms = (backoff_ms * 2).min(5000);
                    }
                }
                if stop_flag.load(Ordering::Acquire) {
                    break;
                }
                let wait = unsafe { WaitForSingleObject(stop_sig.0, backoff_ms as u32) };
                if wait == WAIT_OBJECT_0 {
                    break;
                }
            }
        })
        .map_err(|e| format!("spawn host-ws-client: {e}"))?;

    Ok(WsClientHandle {
        stopping,
        stop_event,
        registration,
        thread: Some(thread),
    })
}

fn flush_pending_state(
    ws: &mut tungstenite::WebSocket<TcpStream>,
    registration: Option<&SharedRegistration>,
    force_register: bool,
    next_id: &mut u64,
) -> Result<(), String> {
    let Some((state, _)) = registration.map(|r| (&r.0, &r.1)) else {
        return Ok(());
    };
    let (reg_info, focus_update) = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        let send_reg = force_register || guard.dirty;
        guard.dirty = false;
        let info = send_reg.then(|| guard.info.clone());
        let focus = guard.focus_pending.take();
        (info, focus)
    };
    if let Some(info) = reg_info {
        let id = *next_id;
        *next_id += 1;
        let payload = serde_json::json!({
            "id": id,
            "cmd": "host_register",
            "args": {
                "pid": info.pid,
                "generation": info.generation,
                "instanceNonce": info.instance_nonce,
                "inspectorPort": info.inspector_port,
                "attached": info.attached,
                "ready": info.ready,
            }
        });
        ws.send(tungstenite::Message::Text(payload.to_string()))
            .map_err(|e| format!("host_register send: {e}"))?;
    }
    if let Some(focused) = focus_update {
        let id = *next_id;
        *next_id += 1;
        let payload = serde_json::json!({
            "id": id,
            "cmd": "set_desktop_input_focus",
            "args": { "focused": focused }
        });
        ws.send(tungstenite::Message::Text(payload.to_string()))
            .map_err(|e| format!("set_desktop_input_focus send: {e}"))?;
    }
    Ok(())
}

fn run_session(
    port: u16,
    token: &str,
    stopping: &AtomicBool,
    stop_event: &OwnedEvent,
    registration: Option<&SharedRegistration>,
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

    ws.send(tungstenite::Message::Text(
        serde_json::json!({ "hello": { "token": token } }).to_string(),
    ))
    .map_err(|e| format!("hello send: {e}"))?;

    let ready_txt = match ws.read() {
        Ok(tungstenite::Message::Text(t)) => t,
        Ok(_) => return Err("unexpected non-text ready frame".into()),
        Err(e) => return Err(format!("hello read: {e}")),
    };
    let ready_json: serde_json::Value =
        serde_json::from_str(&ready_txt).map_err(|e| format!("bad ready json: {e}"))?;
    if ready_json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Err(format!("ws hello rejected: {ready_json}"));
    }

    // Subscribe to host change events so the connection exercises event-driven wakeups cleanly.
    ws.send(tungstenite::Message::Text(
        serde_json::json!({ "type": "subscribe", "event": "desk-host-changed" }).to_string(),
    ))
    .map_err(|e| format!("subscribe send: {e}"))?;

    let mut next_id = 1u64;
    if let Some((_, ev)) = registration.map(|r| (&r.0, &r.1)) {
        ev.reset();
    }
    flush_pending_state(&mut ws, registration, true, &mut next_id)?;

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
    if unsafe { WSAEventSelect(sock, sock_event, (FD_READ | FD_CLOSE) as i32) } != 0 {
        return Err("WSAEventSelect failed".into());
    }

    let mut handles = vec![HANDLE(sock_event.0 as _), stop_event.0];
    if let Some((_, ev)) = registration.map(|r| (&r.0, &r.1)) {
        handles.push(ev.0);
    }
    while !stopping.load(Ordering::Acquire) {
        loop {
            match ws.read() {
                Ok(tungstenite::Message::Close(_)) => return Ok(()),
                Ok(tungstenite::Message::Ping(data)) => {
                    if ws.send(tungstenite::Message::Pong(data)).is_err() {
                        return Ok(());
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref err)) if err.kind() == ErrorKind::WouldBlock => {
                    break;
                }
                Err(_) => return Ok(()),
            }
        }

        let res = unsafe { WSAWaitForMultipleEvents(&handles, false, WSA_INFINITE, false) };
        if res.0 == WAIT_OBJECT_0.0 + 1 {
            let _ = ws.close(None);
            return Ok(());
        }
        if res.0 == WAIT_OBJECT_0.0 + 2 {
            if let Some((_, ev)) = registration.map(|r| (&r.0, &r.1)) {
                // Reset manual-reset event before draining state so concurrent updates re-signal it.
                ev.reset();
            }
            flush_pending_state(&mut ws, registration, false, &mut next_id)?;
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
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn ws_client_authenticates_and_exits_immediately_on_stop() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (hello_tx, hello_rx) = std::sync::mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let hello: serde_json::Value =
                serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
            let _ = hello_tx.send(hello);
            ws.send(tungstenite::Message::Text(
                serde_json::json!({ "v": 1, "ok": true, "result": { "ready": true } }).to_string(),
            ))
            .unwrap();
            let _ = ws.read();
        });

        let client = connect_loop(port, "client-secret".into()).unwrap();
        let hello = hello_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(hello["hello"]["token"], "client-secret");

        let t0 = Instant::now();
        client.stop();
        assert!(
            t0.elapsed() < Duration::from_millis(300),
            "ws client must wake immediately from WSA_INFINITE on stop"
        );
        let _ = server_thread.join();
    }

    #[test]
    fn ws_client_sends_host_register_on_connect_and_wakes_on_state_update_without_timers() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (frame_tx, frame_rx) = std::sync::mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let hello = ws.read().unwrap().into_text().unwrap();
            let _ = frame_tx.send(hello);
            ws.send(tungstenite::Message::Text(
                serde_json::json!({ "v": 1, "ok": true, "result": { "ready": true } }).to_string(),
            ))
            .unwrap();
            while let Ok(msg) = ws.read() {
                if let tungstenite::Message::Text(txt) = msg {
                    let _ = frame_tx.send(txt.clone());
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                        if let Some(id) = val.get("id").cloned() {
                            let _ = ws.send(tungstenite::Message::Text(
                                serde_json::json!({ "v": 1, "id": id, "ok": true, "result": { "ok": true } })
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        });

        let client = connect_with_registration(
            port,
            "client-secret".into(),
            HostRegistrationInfo {
                pid: 4242,
                generation: 9,
                instance_nonce: "nonce-xyz".into(),
                inspector_port: 51111,
                attached: false,
                ready: false,
            },
        )
        .unwrap();

        let _hello: serde_json::Value = serde_json::from_str(&frame_rx.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        let _sub: serde_json::Value = serde_json::from_str(&frame_rx.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        let reg1: serde_json::Value = serde_json::from_str(&frame_rx.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        assert_eq!(reg1["cmd"], "host_register");
        assert_eq!(reg1["args"]["pid"], 4242);
        assert_eq!(reg1["args"]["generation"], 9);
        assert_eq!(reg1["args"]["instanceNonce"], "nonce-xyz");
        assert_eq!(reg1["args"]["inspectorPort"], 51111);
        assert_eq!(reg1["args"]["attached"], false);
        assert_eq!(reg1["args"]["ready"], false);

        let t_upd = Instant::now();
        client.update_registration(51111, true, true);
        let reg2: serde_json::Value = serde_json::from_str(&frame_rx.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        assert!(t_upd.elapsed() < Duration::from_millis(300), "registration wake must be event-driven");
        assert_eq!(reg2["cmd"], "host_register");
        assert_eq!(reg2["args"]["attached"], true);
        assert_eq!(reg2["args"]["ready"], true);

        client.stop();
        let _ = server_thread.join();
    }
}
