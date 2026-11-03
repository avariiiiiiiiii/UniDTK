# Chapter 4. The WebSocket door

## 4.1 Why a second door

The socket of Chapter 2 is read-only by policy. The page, however, must change state: play music, connect the VPN, move a folder. The page therefore gets its own door. It is a WebSocket server on the kernel, in [core/kernel/ws.rs](../../core/kernel/ws.rs). It uses the same dispatcher as the socket, but it is protected by a secret that only the page knows. Because it is protected, it serves the UI door.

## 4.2 The token, and where it is handed out

The token is made once per launch by `ws::new_token()`. It is 32 hexadecimal characters, built from two values of the standard library's OS-seeded hasher (`RandomState`) plus the process id and the time. No new dependency is needed. The token only has to be unpredictable to other local processes, and it changes on every launch.

The token reaches the page in **one** place: the Tauri command `session_info` in [core/kernel/main.rs](../../core/kernel/main.rs). It returns `{"v": 1, "wsPort": ..., "token": ...}`. The page asks for it through the facade, [core/ui/unidtk-bridge.js](../../core/ui/unidtk-bridge.js), which calls `session_info` through Tauri and never through the public `invoke`. The token is not written to a file, to a URL, or to the command line.

The socket's `ping` reply publishes the **port** of the door as `wsPort`, but never the token. The comment in `handle_ping` explains why: that reply travels over the unauthenticated socket.

## 4.3 The handshake

The door listens on an **ephemeral** loopback port (`bind((LOCALHOST, 0))`). The port is public, and the token is not. The first frame on a new connection must be a handshake:

```text
client -> app   {"hello":{"token":"<the 32 hex characters>"}}
app -> client   {"v":1,"ok":true,"result":{"ready":true}}
```

The rules, as written in `serve()`:

1. The first read has a **3 second** deadline (`HANDSHAKE_DEADLINE`). Nothing else is parsed before the handshake.
2. If the first frame is not text, or the token does not match, the server sends `{"v":1,"ok":false,"error":"handshake required: send {\"hello\":{\"token\":\"...\"}} as the first frame"}`, closes the connection, and logs `ws: connection rejected (missing or wrong token)`.
3. After a correct token, the server sends the `ready` frame.

The **3 second** value is the server's deadline. The client side of the page waits longer. The facade's `HANDSHAKE_TIMEOUT_MS` is **8000**, because on a cold page the first call could time out. Section H4 records that case.

## 4.4 Requests after the handshake

After `ready`, every text frame is one of three things:

- `{"type":"subscribe","event":"<name>"[, "since": N]}`: an event subscription (4.5).
- `{"type":"unsubscribe","event":"<name>"}`: removes the event from this connection.
- Any other JSON: a command. It is dispatched with `Door::Ui`, and the reply is `respond(id, dispatch(...))`, which carries the `id` back.

A connection that sends nothing for **120 seconds** (`IDLE_TIMEOUT`) is dropped, so a forgotten page cannot hold a slot forever. The loop reads with a **50 ms** poll (`READ_POLL`), and between reads it pushes any pending event frames. The socket stays a single object owned by one thread, so no lock guards it.

## 4.5 Events

Events are pushed by the **broker** in [core/kernel/events.rs](../../core/kernel/events.rs). Every event published through the contract emitter gets a global, monotonic sequence number `seq`. The emitter is set once in `main.rs` (`unidtk_api::set_emitter`). That one closure feeds Tauri and the broker, so both doors carry the same events with the same numbers. Each event keeps a **ring of 64 frames** (`RING_CAPACITY`).

Events come in two classes:

- **State events** (`system-volume`, `audio-bands`, `vpn`, `telegram-connection-state`, `sync-tasks`, `weather-update`, six names in `STATE_EVENTS`). Only the newest frame matters, so the ring keeps one value. The writer merges all pending frames of a state event into the newest one per pass.
- **Stream events** (for example the Telegram messages). Their order matters, so every frame is kept in the ring and delivered in order.

A subscriber gets a frame in this shape:

```text
{"v":1,"type":"event","event":"system-volume","seq":2,"payload":{...}}
```

The rules for subscribing:

- A subscription **without** a cursor receives, at once, the newest value of a state event, marked with `"snapshot": true`. The mark matters: a catch-up frame is not a live delivery, and the comparison of the migration counted it as one until it was marked.
- A subscription **with** `"since": N` receives the frames after `N` from the ring (`replay`). If the ring no longer covers `N` for a stream event, the server sends `{"v":1,"type":"gap","event":...,"since":N,"error":"the buffer no longer covers this cursor; resync the state"}`. The client must then resynchronise the state, which is safer than a silently incomplete stream.
- A slow client never blocks the kernel. The broker delivers with `try_send` into a bounded channel of 64 frames, and drops what does not fit.

## 4.6 The client side: the facade and the transport flag

The page never calls Tauri or the WebSocket directly. It imports one module, `@unidtk/bridge`, which is [core/ui/unidtk-bridge.js](../../core/ui/unidtk-bridge.js) (the facade). An import map and a build lint enforce this, as Chapter 15 explains.

The facade chooses the transport **once**, when the module loads:

- `?transport=<value>` in the page URL wins, for a one-off probe.
- Otherwise `sessionStorage["unidtk.transport"]` is read. `localStorage` is deliberately not used, so that a forgotten flag cannot survive a restart.
- The values are `tauri` (the default, the old door), `shadow` (both doors are called and compared), and `ws` (the new door is used).

Three allowlists decide what travels over the new door. A command is added to a list only after a clean `shadow` run:

| List | Count | Meaning |
|---|---|---|
| `WS_COMMANDS` | 14 | Read commands. In `shadow` they are called on both doors and the answers are compared. |
| `WS_WRITES` | 8 | Write commands. They use the new door only in `ws` mode. They are verified by their consequence, never by comparison (see below). |
| `WS_EVENTS` | 18 | Events subscribed through the socket instead of Tauri |

Why writes are not compared: `shadow` calls both doors with the same question. For a read, that is harmless. For a write, it performs the action twice. So a write is verified differently. It is switched to the new door in `ws` mode, and then a read of the state it changed confirms the result. Writes not yet migrated (`set_module_enabled`, `ai_dj_model_download`, `telegram_set_archived`, `explorer_sort_set`) are not in `WS_WRITES`, and they stay on Tauri.

The facade counts what it does. `getTransportState()` returns `transport`, `wsState` (off, connecting, ready, closed, failed), the number of shadow checks and divergences, the number of commands sent over WS, and up to three samples of divergence. The CLI reads it with `udtk gui js:window.__UNIDTK_BRIDGE__.getTransportState()`.

Comparison of answers is **structural**: key order is ignored (`sameShape` sorts the keys). Event comparison is by **delivery**, not by position: the facade keeps the last 8 live payloads of each event and counts a divergence only when the Tauri payload is absent from them.

## 4.7 Boundary: what the token protects, and what it does not

The token keeps out any local process that does not know it, and any web page that tries to open the socket. It does **not** protect against code already running inside the same process. The CDP protocol can read the page's own variables, and the token is one of them. That is the accepted risk of [Chapter 6](ch06-design-decisions.md#chapter-6-design-decisions). It is also why the token does not justify widening the socket's policy.
