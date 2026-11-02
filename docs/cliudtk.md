# The `udtk` book: the UniDTK CLI bridge

This is the book about `udtk`, the command-line bridge of **UniDTK** (Unified Desktop Täglich Kit). It teaches the mechanism step by step: what runs on the machine, which channel carries each request, which code decides whether a request is allowed, and why it is built that way. It follows the documentation principle in [architecture.md](architecture.md): English, low level, like the Rust Book. The entry point of the whole book is [README.md](../README.md). `udtk doc` renders that book and its linked pages into the browser.

> **Source of truth.** The code. Every fact in this book was checked against the files named in it on 2026-11-02. Where the code and an older note disagree, this book follows the code and says so in [Chapter 18](#chapter-18-known-gaps-and-open-decisions). Session records and acceptance reports from the migration are kept, condensed, in [Part V: History](#part-v-history).

## Contents

**Part I. Foundations**

1. [What `udtk` is](#chapter-1-what-udtk-is)
2. [How a command travels](#chapter-2-how-a-command-travels)
3. [The two doors, and why writes stay on the UI door](#chapter-3-the-two-doors-and-why-writes-stay-on-the-ui-door)
4. [The WebSocket door](#chapter-4-the-websocket-door)
5. [Controlling the WebView over CDP](#chapter-5-controlling-the-webview-over-cdp)
6. [Design decisions](#chapter-6-design-decisions)

**Part II. Commands**

7. [The command map](#chapter-7-the-command-map)
8. [`invoke` and `logs`](#chapter-8-invoke-and-logs)
9. [`gui`: driving the page](#chapter-9-gui-driving-the-page)
10. [`shot` and `cdp-list`](#chapter-10-shot-and-cdp-list)
11. [`doctor`: the diagnostic loop](#chapter-11-doctor-the-diagnostic-loop)
12. [`tasks`, `autostart`, and `path`](#chapter-12-tasks-autostart-and-path)
13. [`run`, `stop`, `doc`, `help`, and the splash](#chapter-13-run-stop-doc-help-and-the-splash)
14. [`build`, step by step](#chapter-14-build-step-by-step)

**Part III. Rules for modules and the install**

15. [How a module joins the bridge](#chapter-15-how-a-module-joins-the-bridge)
16. [Install layout and data](#chapter-16-install-layout-and-data)

**Part IV. Verification and open items**

17. [How to verify a change](#chapter-17-how-to-verify-a-change)
18. [Known gaps and open decisions](#chapter-18-known-gaps-and-open-decisions)

**Part V. History**

- [H1. Sessions 58 to 60 (17.09.2026)](#h1-sessions-58-to-60-17092026)
- [H2. The plan and its critique (former 10.1 to 10.8)](#h2-the-plan-and-its-critique-former-101-to-108)
- [H3. Slices S0 to S5 (former 10.9 to 10.22)](#h3-slices-s0-to-s5-former-109-to-1022)
- [H4. Migration of the page to the new door (former 10.23 to 10.35)](#h4-migration-of-the-page-to-the-new-door-former-1023-to-1035)
- [H5. Corrections made in the rewrite of 2026-11-02](#h5-corrections-made-in-the-rewrite-of-2026-11-02)

---

## Part I. Foundations

## Chapter 1. What `udtk` is

`udtk` is a small native program. It does not talk to the user's screen, and it does not contain the application's logic. It is a **client**. It asks the running UniDTK application questions over a local socket, and it drives the application's WebView through the browser debugging protocol (CDP). It also performs the operations that belong to the machine rather than to the app: building, installing, scheduled tasks, and the PATH.

### 1.1 One project, three binaries

The whole Rust side is one Cargo project in `toolchain/`. It declares three binaries in [toolchain/Cargo.toml](../toolchain/Cargo.toml):

```toml
[[bin]]
name = "unidtk"
path = "../core/kernel/main.rs"

[[bin]]
name = "udtk"
path = "../opt/cli/main.rs"

[[bin]]
name = "vpn-helper"
path = "../modules/vpn/helper/main.rs"
```

| Binary | Source | Role |
|---|---|---|
| `unidtk.exe` | [core/kernel/main.rs](../core/kernel/main.rs) | The application: kernel, WebView, IPC server, modules |
| `udtk.exe` | [opt/cli/main.rs](../opt/cli/main.rs) | This CLI |
| `vpn-helper.exe` | [modules/vpn/helper/main.rs](../modules/vpn/helper/main.rs) | A GUI-subsystem helper for the VPN, run as a scheduled task |

Nothing else is needed at run time. There are no script wrappers, no Node.js, and no external runtime.

### 1.2 Where the binaries live

`udtk` finds the application by looking **next to itself**. The function `install_dir()` in [core/api/src/paths.rs](../core/api/src/paths.rs) returns the directory of the running executable. In a shipped install that is `C:\Program Files\UniDTK`, where `unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, and `sing-box.exe` sit together. In a development run it is `target\release` of the checkout.

Two commands use a different, fixed location on purpose. `udtk path` and `udtk tasks register` always point at `C:\Program Files\UniDTK`, even when the CLI runs from a checkout, so that a development build never registers itself in the PATH or in Task Scheduler. Section 12 explains this.

### 1.3 Exit codes

| Code | Meaning | Where it comes from |
|---|---|---|
| `0` | Success. For `doctor`: every check passed | `main()` prints the output and returns |
| `1` | Execution error: the IPC socket is unreachable, the remote command failed, or `doctor` found a failure | Every command except the argument errors below |
| `2` | Invalid arguments: an unknown command, or an unknown `doctor` flag | `main()` for an unknown command, `doctor_cmd` for a flag |

An unknown command prints `error: unknown command: <name>` and then the help text to stderr. An unknown `doctor` flag is checked against the whitelist `ALLOWED_DOCTOR_FLAGS` in [opt/cli/main.rs](../opt/cli/main.rs), and anything not on it is rejected with exit 2. Nothing is silently ignored.

### 1.4 Commands that were removed

Removing a command is a change of contract, so the old names are rejected rather than redirected. None of these names is an alias any more.

| Old name | What happens now | Use instead |
|---|---|---|
| `udtk ping`, `udtk status` | `unknown command`, exit 2 | `udtk doctor -ping` |
| `udtk check` | `unknown command`, exit 2 | `udtk doctor -check` |
| `udtk commands` | `unknown command`, exit 2 | `udtk help --json` |
| `udtk dev` | `unknown command`, exit 2 (there is no arm for it in the dispatcher) | `cd toolchain && cargo tauri dev` |
| `udtk run --debug`, `udtk run -d` | Refused with an error: CDP is always on in release builds | `udtk gui`, `udtk shot` |
| `-h`, `--help` | Not recognised; `udtk -h` gives exit 2 | `udtk help` |
| `-cargo`, `-sim`, `-app`, `-ipc` | Unrecognised `doctor` flags | `-check`, `-simulate`, `-ping` |

---

## Chapter 2. How a command travels

Two channels connect `udtk` to the running app. Each has its own job, and this chapter follows the first one, the socket, from the first character typed to the printed answer.

### 2.1 The two channels

| Channel | Address | Who opens it | What it carries |
|---|---|---|---|
| Local socket (TCP, one JSON line per request) | `127.0.0.1:26540` | The kernel, in [core/kernel/ipc.rs](../core/kernel/ipc.rs) (`IPC_PORT`) | Every `udtk` command except the screenshots and the page control |
| CDP (HTTP and WebSocket) | A port the app chooses. Usually `9222`, otherwise a free port. The app publishes it in the `ping` reply as `cdpPort` | WebView2, always enabled in release | `gui`, `shot`, `cdp-list`, `stop`, and the page probes of `doctor -simulate` |

A third channel, the page's own WebSocket door, is used by the interface, not by `udtk`. [Chapter 4](#chapter-4-the-websocket-door) covers it.

### 2.2 The journey of `udtk invoke vpn_status`

Take the simplest command that reaches a module. Each numbered step below names the function that does it.

1. **The process starts.** `main()` in [opt/cli/main.rs](../opt/cli/main.rs) collects the arguments. With none, it prints the splash. With `invoke`, it calls `invoke_cmd`.
2. **The request is built.** `invoke_cmd` takes `vpn_status` as the command and `{}` as the default arguments, and calls `ipc(json!({"cmd": ..., "args": ...}))`.
3. **The envelope is numbered.** `ipc()` takes the next value of a process-wide counter (`AtomicU64`, starting at 1). It adds `"v": 1` (the protocol version) and `"id": <n>` to the request. The line is the JSON text followed by `\n`.
4. **The socket opens.** `TcpStream::connect_timeout` connects to `127.0.0.1:26540` with a **500 ms** connect timeout. The read and write timeouts are **2500 ms**. If the connection is refused, the error is `IPC server not reachable at 127.0.0.1:26540: ... (is the app running?)`, and the exit code is 1.
5. **The kernel accepts.** The thread `unidtk-ipc` (started in `IpcServer::start`) accepts connections. Each connection gets its own thread, `handle_conn`.
6. **The in-flight limit is checked.** `InflightGuard::acquire` allows at most **8** requests at a time (`MAX_INFLIGHT`). A ninth one gets the reply `busy: too many ipc requests in flight` and is closed.
7. **One line is read.** `handle_conn` sets a **1500 ms** read timeout on the server side and reads exactly one line. The protocol is one request per connection.
8. **The line is routed.** `route()` parses the JSON. A parse error gives `bad json: ...`. Otherwise the request goes to `dispatch(&req, Door::Socket)`. [Chapter 3](#chapter-3-the-two-doors-and-why-writes-stay-on-the-ui-door) explains the door.
9. **The inventory is consulted.** `dispatch` calls `check_door(cmd, door)`. It looks the name up in the inventory (see 2.5). An unknown name gives `unknown cmd: <name>`. A name the door does not serve gives a refusal with a reason.
10. **The route is found.** `dispatch` looks the name up in the static table `ROUTES` (see 2.6). For `vpn_status` the handler is `handle_vpn_status`.
11. **The module is called by its id.** `handle_vpn_status` calls `call_module(server, "vpn", "vpn_status", args)`. `call_module` finds `"vpn"` in `MODULE_ROUTES` and calls `vpn::ipc_call`. The kernel passes the module an opaque handle to the application, typed `&(dyn Any + Send + Sync)`. The module downcasts it to fetch its own state. The kernel never names the module's state type.
12. **The reply is built.** `respond()` wraps the outcome as `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`. It adds `"v": 1`, and it echoes `"id"` when the request had one.
13. **The reply is written.** `handle_conn` writes the reply with a trailing newline and closes the connection.
14. **The client checks the reply.** `ipc()` reads one line and parses it. If the reply carries an `id`, it must equal the request's id. If it carries a `v`, it must equal 1. Otherwise the error is `protocol mismatch: the app speaks vN, this bridge speaks v1 ... update them together`. On `ok: true` the client pretty-prints `result` and exits 0. On `ok: false` it prints the `error` and exits 1.

The envelope is optional on the server side. An older bridge that sends only `{cmd, args}` still works, and the reply simply has no `id` to echo.

### 2.3 Wire format at a glance

```text
client -> app   {"cmd":"vpn_status","args":{},"v":1,"id":3}\n
app -> client   {"ok":true,"result":{...},"v":1,"id":3}\n
```

A failure looks the same, with `"ok":false` and `"error":"..."` in place of `result`. The protocol number is `PROTOCOL = 1` in [core/kernel/ipc.rs](../core/kernel/ipc.rs). The same number is reported in the `ping` reply as `proto`, so a client can compare its own version with the app's before it trusts an answer.

### 2.4 Transport failures and what they mean

| Symptom | Cause | What to do |
|---|---|---|
| `IPC server not reachable at 127.0.0.1:26540 ... (is the app running?)` | Nothing listens on 26540. The app is not running, or it failed to bind the port | `udtk run`, or `schtasks /run /tn UniDTK` |
| `busy: too many ipc requests in flight` | Eight requests are already being served | Retry. This is back-pressure, not a fault |
| `protocol mismatch: ...` | The CLI and the app are different versions | Update both together (`udtk build` installs both) |
| `protocol: response id N does not match request id M` | A reply belonged to another request | Usually a stale or foreign process on the port; restart the app |
| The command stops responding after 2.5 s | The client read timeout is 2500 ms. The server may still be running the command | Re-check the state before retrying. Commands that change state must not be retried blindly (see 3.5) |

### 2.5 The inventory: one list of what can be asked

The file [core/kernel/command_inventory.rs](../core/kernel/command_inventory.rs) describes every command the application can be asked. It is compiled **twice**: into the application and into `udtk`, which includes it with `#[path]`. The offline manifest that `udtk help --json` prints and the surface the kernel serves are therefore built from the same data, and they cannot drift apart.

Each command is a `CommandSpec` value, defined in [core/api/src/commands.rs](../core/api/src/commands.rs). A spec has a `name`, a `summary`, a list of `args`, an `access` (`ReadOnly` or `Write`), a list of `doors` (`Socket`, `Ui`), and an `idempotency` (`Safe` or `Unsafe`). Three constructors set these fields:

| Constructor | Access | Doors | Idempotency | Count in the inventory |
|---|---|---|---|---|
| `read_only(...)` | ReadOnly | Socket and UI | Safe | 20 |
| `read_only_cli(...)` | ReadOnly | Socket only (the tool's own surface) | Safe | 5 |
| `write(...)` | Write | UI only | Unsafe | 12 |

The inventory holds **37** commands (the constant `ALL`, in declaration order). Twenty-five of them are served on the socket, and thirty-two on the UI door. The 12 writes never reach the socket by construction. Each name is written once: the route table takes the name from the constant (`COMMAND.name`), not from a repeated string.

Inventory rules that the tests enforce are listed in [Chapter 15](#chapter-15-how-a-module-joins-the-bridge).

### 2.6 The route table and the module routes

A name in the inventory is not enough to serve it. The kernel also needs a handler. There are two tables in [core/kernel/ipc.rs](../core/kernel/ipc.rs):

- **`ROUTES`**: a static slice of `(name, handler)`, where each handler is a kernel function with the type `fn(&IpcServer, &Args) -> Result<Value, String>`. The kernel owns handlers that need kernel state, such as the log (`handle_logs`, `handle_log_delta`), the widget configuration, and the module-enable switch. Module commands are thin: each handler calls `call_module`.
- **`MODULE_ROUTES`**: a static slice of `(module_id, ipc_call)`. It lists the six modules that answer for their own commands: `glyph-bar`, `explorer`, `vpn`, `telegram`, `mp3-player`, and `micrographics`. The kernel addresses a module by id and never names its state type. A test, `router_does_not_name_module_state_types`, reads the source of `ipc.rs` and fails if a module state type name appears there.

`dispatch` looks the name up in `ROUTES`. If the inventory knows a command but no route exists, the reply is `command '<name>' is declared in the inventory but has no handler`. The route tests exist to make that impossible to ship: `every_command_has_a_handler`, `every_socket_command_has_a_handler`, `every_handler_is_declared_in_the_inventory`, and `route_names_are_unique`. They are tests, not build-time checks, because `generate_handler!` is a macro that `build.rs` cannot read (see [Chapter 15](#chapter-15-how-a-module-joins-the-bridge)).

---

## Chapter 3. The two doors, and why writes stay on the UI door

### 3.1 Who is asking

A **door** is a rule about **who is asking**, not about which pipe carries the bytes. The enum `Door` in [core/api/src/commands.rs](../core/api/src/commands.rs) has two values:

- **`Door::Socket`**: the `udtk` bridge over the local TCP line protocol. This socket is loopback-only and **unauthenticated**. Any process of the same user can connect to it.
- **`Door::Ui`**: the application's own interface. It is authenticated by the token of [Chapter 4](#chapter-4-the-websocket-door). During the migration its transport changed from Tauri IPC to a WebSocket. The door did not change with it, because the question is who is asking, not what the pipe is made of.

The door is passed into `dispatch(req, door)` by the caller. The TCP path passes `Door::Socket`, and the WebSocket path passes `Door::Ui`. A command therefore behaves the same way on either door, and only the set of allowed commands depends on the door.

### 3.2 The policy, as data

The policy is the `doors` field of each spec, not a rule written into `dispatch`:

| Spec | Served on the socket | Served on the UI door | What a request from the other door receives |
|---|---|---|---|
| Read on both doors | yes | yes | Served |
| `read_only_cli` (for example `ping`, `logs`, `commands`) | yes | no | `served by the tool door only` |
| `write` (for example `vpn_connect`) | no | yes | `changes state, and write commands stay on the UI door` |
| Not in the inventory | no | no | `unknown cmd: <name>` |

`check_door` in [core/kernel/ipc.rs](../core/kernel/ipc.rs) produces these three refusal messages. It is a free function on purpose: an `IpcServer` cannot be created in a unit test, because it needs a live `AppHandle`, while the policy is pure data and can be tested directly. The tests `a_write_command_is_refused_on_the_socket_and_answered_on_the_ui`, `a_cli_only_read_is_not_offered_to_the_ui`, `a_read_command_is_served_on_both_doors`, and `an_unknown_command_is_refused_on_every_door` cover the three cases.

### 3.3 Why writes stay on the UI door

The socket accepts any local process of the same user, without a password. If it could change state, then any program on the machine could connect a VPN, send a Telegram message, change the volume, or write to the journal and erase evidence. The inventory states this rule in its own comments. The `frontend_log` entry says that a socket that can write log records "lets any local process forge or erase evidence". The VPN entries say that the rule is kept "by rule rather than by accident".

The UI door is different. Its requests come only from the page, and the page proved that it holds the per-run token before any command is parsed. So the interface may change state. The policy is the same for the Tauri and the WebSocket transports, because it is attached to the door, not to the transport.

### 3.4 Consequences for `udtk`

- `udtk invoke vpn_connect` is refused with `command 'vpn_connect' is not available on the socket door: it changes state, and write commands stay on the UI door`, and it exits 1.
- `udtk` cannot write state through the socket. To observe a write from the outside, use `udtk gui js:...` (which runs in the page through CDP) or read back the consequence with a read command. Chapter 9 shows how.
- A read command that the CLI calls internally, such as `ping`, can be served only on the socket. Asking for it on the UI door is refused.
- `udtk gui js:<expr>` does not use the WebSocket door. It evaluates the expression in the page through CDP (Chapter 5), which needs no token.

### 3.5 Retry safety

A request can be repeated, either by a client that reconnects or by a human who presses the button again. The contract records whether repetition is harmless. `Idempotency::Safe` means that asking twice equals asking once. Every read is `Safe`. Every write is `Unsafe`, because it may duplicate an effect. The field is published in both manifests as `retry_safe`, so a client can see it. The test `socket_surface_is_retry_safe` in [core/kernel/command_inventory.rs](../core/kernel/command_inventory.rs) checks that every socket command is `Safe`.

Honest note: the code records the class, but it does not yet deduplicate writes by request id, and it does not refuse automatic retries of writes. The request-id deduplication cache the plan describes is not in [core/kernel/ipc.rs](../core/kernel/ipc.rs) or [core/kernel/ws.rs](../core/kernel/ws.rs) as of this rewrite. See [Chapter 18](#chapter-18-known-gaps-and-open-decisions).

---

## Chapter 4. The WebSocket door

### 4.1 Why a second door

The socket of Chapter 2 is read-only by policy. The page, however, must change state: play music, connect the VPN, move a folder. The page therefore gets its own door. It is a WebSocket server on the kernel, in [core/kernel/ws.rs](../core/kernel/ws.rs). It uses the same dispatcher as the socket, but it is protected by a secret that only the page knows. Because it is protected, it serves the UI door.

### 4.2 The token, and where it is handed out

The token is made once per launch by `ws::new_token()`. It is 32 hexadecimal characters, built from two values of the standard library's OS-seeded hasher (`RandomState`) plus the process id and the time. No new dependency is needed. The token only has to be unpredictable to other local processes, and it changes on every launch.

The token reaches the page in **one** place: the Tauri command `session_info` in [core/kernel/main.rs](../core/kernel/main.rs). It returns `{"v": 1, "wsPort": ..., "token": ...}`. The page asks for it through the facade, [core/ui/unidtk-bridge.js](../core/ui/unidtk-bridge.js), which calls `session_info` through Tauri and never through the public `invoke`. The token is not written to a file, to a URL, or to the command line.

The socket's `ping` reply publishes the **port** of the door as `wsPort`, but never the token. The comment in `handle_ping` explains why: that reply travels over the unauthenticated socket.

### 4.3 The handshake

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

### 4.4 Requests after the handshake

After `ready`, every text frame is one of three things:

- `{"type":"subscribe","event":"<name>"[, "since": N]}`: an event subscription (4.5).
- `{"type":"unsubscribe","event":"<name>"}`: removes the event from this connection.
- Any other JSON: a command. It is dispatched with `Door::Ui`, and the reply is `respond(id, dispatch(...))`, which carries the `id` back.

A connection that sends nothing for **120 seconds** (`IDLE_TIMEOUT`) is dropped, so a forgotten page cannot hold a slot forever. The loop reads with a **50 ms** poll (`READ_POLL`), and between reads it pushes any pending event frames. The socket stays a single object owned by one thread, so no lock guards it.

### 4.5 Events

Events are pushed by the **broker** in [core/kernel/events.rs](../core/kernel/events.rs). Every event published through the contract emitter gets a global, monotonic sequence number `seq`. The emitter is set once in `main.rs` (`unidtk_api::set_emitter`). That one closure feeds Tauri and the broker, so both doors carry the same events with the same numbers. Each event keeps a **ring of 64 frames** (`RING_CAPACITY`).

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

### 4.6 The client side: the facade and the transport flag

The page never calls Tauri or the WebSocket directly. It imports one module, `@unidtk/bridge`, which is [core/ui/unidtk-bridge.js](../core/ui/unidtk-bridge.js) (the facade). An import map and a build lint enforce this, as Chapter 15 explains.

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

### 4.7 Boundary: what the token protects, and what it does not

The token keeps out any local process that does not know it, and any web page that tries to open the socket. It does **not** protect against code already running inside the same process. The CDP protocol can read the page's own variables, and the token is one of them. That is the accepted risk of [Chapter 6](#chapter-6-design-decisions). It is also why the token does not justify widening the socket's policy.

---

## Chapter 5. Controlling the WebView over CDP

`udtk gui`, `udtk shot`, `udtk cdp-list`, and `udtk stop` talk to the WebView2 engine through the Chrome DevTools Protocol (CDP). CDP is a JSON protocol over WebSocket that the browser engine exposes on a local port. This chapter explains how the CLI finds the right window and drives it safely.

### 5.1 Always on, and the port is discovered

WebView2 is started with a remote-debugging port. [core/kernel/main.rs](../core/kernel/main.rs) decides the port in `pick_cdp_port()`:

1. Try `127.0.0.1:9222`. If it can be bound, use it.
2. Otherwise bind port 0 and let the operating system choose a free port.

The port is stored once (`CDP_PORT`) and published over the socket as `cdpPort` in the `ping` reply. The CLI never guesses. A stale or foreign process may hold 9222, and a blind guess would drive the wrong window. In one recorded probe, 9222 was held by a foreign listener and the app took port 54060, and `udtk eval` worked through it (see [H3](#h3-slices-s0-to-s5-former-109-to-1022)).

The browser arguments are assembled by `merge_browser_args()`. It keeps whatever `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` already holds, **except** an inherited `--remote-debugging-port=` or `--remote-allow-origins=`, and then appends the kernel's own arguments and `--remote-debugging-port=<port>`. The kernel no longer overwrites what the caller set. `--remote-allow-origins` is not set anywhere in the code: the old wildcard was removed (session records in [H3](#h3-slices-s0-to-s5-former-109-to-1022)).

### 5.2 Finding the page, and refusing foreign windows

The CLI asks `cdpPort` of the app, through `ping`, and caches the answer for the process (`cdp_port()`). If the app is older and does not publish a port, the CLI falls back to 9222, because such a kernel always binds it. If the app does not answer at all, the error is `cannot determine the CDP port: app is not answering on 127.0.0.1:26540`.

Then `cdp_ws_url()` in [opt/cli/main.rs](../opt/cli/main.rs):

1. Reads the target list over plain HTTP (`GET /json`, with `Host: 127.0.0.1`).
2. Chooses the page whose URL is `http://tauri.localhost/` (the wallpaper document). A transient window, such as the location-permission host, is also a page, and must never be chosen by list order.
3. **Refuses** any target whose URL does not start with `http://tauri.localhost`, with `CDP target is not the UniDTK page (url: ...); refusing to drive a foreign window`.
4. Rewrites a `ws://localhost/...` address to `ws://127.0.0.1:<port>/...`, which WebView2 sometimes reports without a port.

### 5.3 One session per batch

The `Cdp` structure in [opt/cli/main.rs](../opt/cli/main.rs) holds **one** WebSocket and a counter of request ids. `udtk gui` opens it once and runs all its steps over that connection. Before this design, every call opened a new WebSocket, so one click cost five handshakes.

`Cdp::call` sends `{"id", "method", "params"}` and reads frames until it finds the reply with the same id. Frames without a matching id are progress events, and they are ignored on purpose. Each call has a 10 second deadline (`CDP response timeout for <method>`). A CDP error becomes `CDP error: <message>`.

`Cdp::evaluate` calls `Runtime.evaluate` with `returnByValue` and `awaitPromise`. A JavaScript exception becomes `JS Exception: <text>`.

### 5.4 Input conventions

Mouse input is `Input.dispatchMouseEvent` in three steps: moved, pressed, released. The coordinates are **CSS pixels** straight from `getBoundingClientRect()`. CDP takes them **1:1**. Multiplying by `devicePixelRatio` would miss the target by a factor of 1.25 to 1.5, and the comment in `mouse_click` says so.

Keyboard input is `Input.dispatchKeyEvent`, and the `press:` step emulates a **US keyboard**, independent of the system layout. The `code` is physical (`KeyA`, `Digit1`), and `vk` is the virtual key code. Only DOM `key` names are accepted (`Escape`, `ArrowLeft`, `Enter`). Aliases such as `Esc` or `Left` are refused, so each capability has one name. Text is entered with the `type:` step (`Input.insertText`), not with `press:`.

### 5.5 The page harness

The page defines `window.__UNIDTK_HARNESS__`, implemented in [core/ui/harness.js](../core/ui/harness.js). The CLI uses three of its functions:

- `getSemanticTree()`: a list of the interactive elements, each with a numeric `id`, a `ref`, an accessible name, a `point` (the centre in CSS pixels), and an `occluded` flag with the `covering` element when something sits on top.
- `getErrors(clear)`: the page errors collected since the last call. With `true`, the buffer is cleared.
- `resolveTarget(target)`: maps a target string to a node. The order in the code is: a numeric id, then a stable reference `r<N>` (the `data-udtk-ref` attribute, which survives re-rendering), then `@testid`, and then the other locators.

A node that has neither a test id nor an element id is marked `fragile: true`. The harness skips anything inside `.simulation-sandbox`, so that a diagnostic render is never offered as a target.

### 5.6 Security boundary

Honest statement, unchanged since the design decision of [Chapter 6](#chapter-6-design-decisions): CDP is always on in release. Any local process of the same Windows user can connect to it and run JavaScript in the page. The boundary is the **Windows user account**, not a token and not a flag. Two things do close the web-page class: the CDP HTTP endpoint checks the `Host` header (only IP or localhost is accepted), and it sends no CORS headers. These two statements come from the recorded live probes of slice S0, not from code that can be read here. The CSP of the page allows `ws://127.0.0.1:*` in `connect-src` ([toolchain/tauri.conf.json](../toolchain/tauri.conf.json)), so the page can reach the WebSocket door.

---

## Chapter 6. Design decisions

This chapter records the decisions behind the structure. The history of how each was reached is in [Part V](#part-v-history).

### 6.1 One contract, two doors

The command surface used to be written down three times: a `match` in the kernel, a list in the `commands` reply, and the Tauri command list. Adding a module command meant editing the kernel in three places. The decision was to describe the surface **once**, as data (the inventory of Chapter 2), and to make both doors read that data. The Tauri wrappers stay handwritten, because they carry the Tauri injection (`State`, `AppHandle`) that only the kernel can express. Their names are checked against the inventory by a test.

### 6.2 A manifest built into both binaries

The manifest is a static build artifact, compiled into the kernel and into `udtk`. It must not be produced by instantiating the modules. The reason is concrete: building the list from `registry::all_modules()` would create second instances of the modules. That means a repeated VPN auto-connect, a second TDLib, and a race with the deferred start of the player. The CLI does call `registry::all_modules()` in `doctor -env`, but only to read `Module::dependencies()`, which does not start anything.

### 6.3 Modules own their state

The kernel no longer names module types in its router. Each module answers for its own commands through `ipc_call`, and fetches its state from the host handle itself. A third-party module can add a command in three steps: declare it in the inventory, implement it in `ipc_call`, and add one line to `MODULE_ROUTES`. No kernel code changes. This is true for the socket and the UI door. For the Tauri wrappers it stays true only while they exist.

### 6.4 Rejected alternatives

| Alternative | Why it was rejected |
|---|---|
| A procedural macro `#[udtk_command]`, plus scanning `modules/*/rust` in `build.rs` | Two parsers of the same thing. A text scan also catches comments and disabled code |
| Generating the bodies of the Tauri wrappers | The Tauri injection is known only to the kernel, and `include!` from `OUT_DIR` breaks the IDE and incremental builds |
| Linker magic (`inventory`, `linkme`) | On MSVC, `/OPT:REF` strips unreferenced sections, so entries silently vanish |
| A long-lived CLI daemon that holds session state | It would move logic into the CLI. The CLI stays a thin client, one batch per `gui` run |
| Write commands on the socket, "right away" | It lowers the security perimeter without a threat model |
| The `help --json` answer taken from the socket | It would make the diagnostics depend on the running app |

The reference projects were used for technique, not as dependencies. `agent-browser` (Rust and CDP, element references taken from a snapshot) gave the refusal on an occluded target and the stable references. Chrome DevTools MCP gave the idea of targeted queries instead of a full snapshot after each action.

### 6.5 Owner's decisions, and the accepted risk

The owner decided four things, and they are the basis of the current design:

1. The page is moved entirely to the new protocol (the radical option). Authentication of the door is therefore a required stage, not an option.
2. Module state moves into the modules. This is the condition for a third-party developer to add a command without editing the kernel.
3. `metadata.json` is extended with the render entry points for the probe (see [Chapter 11](#chapter-11-doctor-the-diagnostic-loop)).
4. CDP stays enabled always. The port is no longer hard-coded, and the `dev` and `--debug` modes are retired.

The accepted, recorded risk: a local process of the same user can execute JavaScript in the application, and the token does not prevent it. The security boundary is the Windows user account. The plan was written with the honest limit stated, and it is repeated here so that it is not forgotten.

---

## Part II. Commands

## Chapter 7. The command map

| Command | Purpose | Needs the running app |
|---|---|---|
| `udtk` (no arguments) | Prints the splash and the command map | No |
| `udtk doctor [flags]` | Diagnostics: compiler, dependencies, kernel, modules, IPC, simulation | Partly (`-ping`, `-simulate`) |
| `udtk invoke <cmd> [json]` | Calls one socket command | Yes |
| `udtk logs [filters]` | Reads journal records through the socket | Yes |
| `udtk gui [--json] [--force] <step>...` | Runs a batch of steps in the page over one CDP session | Yes, plus CDP |
| `udtk shot [path] [--json] [-q]` | Saves a PNG of the window through CDP | Yes, plus CDP |
| `udtk cdp-list` | Lists the CDP targets | Yes, plus CDP |
| `udtk tasks [status\|register\|unregister\|cleanup-old]` | Manages the Windows scheduled tasks | No |
| `udtk autostart [status\|enable\|disable]` | Controls the logon task `UniDTK` | No |
| `udtk path [status\|ensure]` | Checks or adds the install directory to the User PATH | No |
| `udtk build [-j N]` | The one release build: stop, build, install, verify, restart | No (it stops and restarts the app) |
| `udtk stop` | Quits the app gracefully, then forces it, so that its files can be replaced | Optional |
| `udtk run [args...]` | Starts the app that sits next to the CLI | No |
| `udtk doc` | Renders the book and opens it | No |
| `udtk help [--json]` | Prints the help text, or the machine-readable manifest | No |

The help text of `udtk help` and the manifest of `udtk help --json` are generated from [opt/cli/main.rs](../opt/cli/main.rs). In the manifest, `cli_commands` lists the 14 commands of `udtk` itself, and `app_commands` lists the 37 entries of the inventory, each with its `access`, its `doors`, and its `retry_safe`. The manifest also states `"protocol": 1`. The `cli_commands` array is written by hand in `help_cmd` (see [Chapter 18](#chapter-18-known-gaps-and-open-decisions)).

---

## Chapter 8. `invoke` and `logs`

### 8.1 `udtk invoke`

```bash
udtk invoke <cmd> [json-args]
```

`json-args` is a JSON object. The reply is printed pretty. Examples:

```bash
udtk invoke ping
udtk invoke vpn_status
udtk invoke logs '{"limit":10,"level":"ERROR"}'
```

| Failure | Message | Exit |
|---|---|---|
| No command given | `usage: udtk invoke <cmd> [json-args]` | 1 |
| Arguments are not JSON | `bad json args: ...` | 1 |
| Name not in the inventory | `unknown cmd: <name>` | 1 |
| A write over the socket | `command '<name>' is not available on the socket door: it changes state, and write commands stay on the UI door` | 1 |
| A UI-only read over the socket | `command '<name>' is not available on the socket door: it is served by the UI door only` | 1 |
| The module does not handle the command | `module '<id>' is registered but does not handle '<cmd>'`, or the module's own error text | 1 |

### 8.2 `udtk logs`

```bash
udtk logs [--limit N] [--since ID] [--level LEVEL] [--source SOURCE]
```

The default limit is **50**. `--since` returns the records after a journal id. `--level` and `--source` are passed as strings, for example `--level ERROR` or `--source frontend`. The command calls the socket command `logs` (its name comes from the inventory constant `SIMULATE_LOGS`, so a rename cannot leave `logs` talking to a dead name). The journal itself is `unidtk_logs.redb`, read by [core/kernel/log_store.rs](../core/kernel/log_store.rs).

Examples:

```bash
udtk logs --level ERROR          # expect [] when the app is healthy
udtk logs --source frontend --limit 30
```

`udtk logs` answers only as far as the journal goes. A page error that happened in the page itself is collected by the page harness and read with `getErrors`, which `gui` does for every step (see [Chapter 9](#chapter-9-gui-driving-the-page)).

---

## Chapter 9. `gui`: driving the page

```bash
udtk gui [--json] [--force] <step>...
```

`gui` runs a batch of steps in the page. It is the replacement for three older commands, `dom`, `eval`, and `act`, which were removed. The batch has at most **50** steps, and it opens one CDP session (Chapter 5) for all of them.

### 9.1 The steps

| Step | What it does |
|---|---|
| `js:<expr>` | Evaluates JavaScript in the page and returns the value |
| `click:<target>` | Moves the mouse to the centre of the target, presses, and releases |
| `type:<target>:<text>` | Clicks the target to focus it, then inserts the text (`Input.insertText`) |
| `press:<key>` or `press:<mod>+<key>` | Presses one key. Modifiers are `ctrl`, `shift`, `alt`, `meta` |
| `tree` | Returns the semantic tree of the page |
| `wait:<ms>` | Pauses, at most 10 000 ms |

The `<target>` is resolved by the page's own harness (Chapter 5, 5.5): a numeric id from the tree, a stable reference `r<N>`, a test id written `@name`, an accessible name, or a selector.

Examples:

```bash
udtk gui js:6*7
udtk gui tree
udtk gui click:r12
udtk gui type:@search:hello press:Enter
udtk gui press:ctrl+a
udtk gui js:window.__UNIDTK_BRIDGE__.getTransportState()
```

### 9.2 The receipt

Each step produces a receipt, so the caller never has to trust that a step worked. The fields the code writes are `step`, `kind`, `ok`, `target`, `hit_point`, `covering`, `refused`, `text`, `key`, `value`, `error`, `log_delta`, `ui_errors`, and `duration_ms`.

- `log_delta` holds the journal records written since the previous step. It is read with the socket command `log_delta`, one round trip per step.
- `ui_errors` holds the page errors collected since the previous step.
- `ok` is derived from what happened, not from whether a message was sent. A click on an occluded target has `ok: false`, not `ok: true`.

`--json` prints `{"ok": ..., "steps": [...]}`. The text mode prints one line per step. Any failed step gives exit 1.

### 9.3 Occlusion is a refusal

Before a click, the harness reports whether the centre of the target is covered by another element. If it is, the step is **refused**, not sent:

```text
ok: false, refused: "occluded", covering: {tag, id, testid, ref}
```

The receipt names the covering element. `--force` lifts the refusal and clicks anyway. This rule came from a live finding: the old `act` read the flag and sent the click anyway, and returned `status: ok` while the press landed on another element (see [H3](#h3-slices-s0-to-s5-former-109-to-1022)).

### 9.4 Key refusals

A key that cannot be emulated is refused before anything is sent, so that no `keyUp` is left without its `keyDown`:

| `refused` value | Cause |
|---|---|
| `unknown-key` | The name is not a canonical DOM key name (for example `Esc`, `Left`) |
| `non-ascii` | A non-ASCII character was given to `press:` (use `type:` for text) |
| `unknown-modifier` | The modifier is not `ctrl`, `shift`, `alt`, or `meta` |
| `empty-key` | Nothing follows the modifiers |

Keys that used to be inert through the CLI, such as `Delete`, `Backspace`, `F2`, and `F5`, now act on the page. Do not run them against live data in a check.

### 9.5 Failure modes

| Message | Meaning |
|---|---|
| `too many steps in one batch (max 50)` | Split the batch |
| `CDP not reachable on <port>: ... (is the app running?)` | The app is not running, or CDP is not open |
| `CDP target is not the UniDTK page ...` | The first page is not the app's page; the CLI refuses to drive it |
| `target not found in the active UI: <target>` | No element matches; use `tree` to see the names |
| `unknown step kind: <kind>` | A step was not understood. In PowerShell 5.1 a multi-line `js:` argument is split at line breaks and double quotes, and each piece becomes a step. Write one line, with single quotes inside |

---

## Chapter 10. `shot` and `cdp-list`

```bash
udtk shot [path] [--json|-j] [-q|--quiet] [-o|--output <path>]
udtk cdp-list
```

`shot` asks CDP for a PNG (`Page.captureScreenshot`, from the surface, optimised for speed). The default file is `unidtk_YYYY-MM-DD_HH-MM-SS.png` in the **Screenshots** folder of the user, found through `SHGetKnownFolderPath`. If that folder cannot be created, it tries `Pictures\Screenshots`, then `Pictures`, and then the system temporary directory. A name that already exists gets `_1` to `_100` appended. A path without an extension gets `.png`. A relative path is taken from the current directory.

Before writing, the file is checked: it must start with the PNG signature and be at least **1024 bytes**. Otherwise the error is `corrupted screenshot received (size: N bytes, expected valid PNG)`. If writing to the chosen place fails, the file goes to the temporary directory, and the output gives the path that was actually used.

`-q` prints only the absolute path. `--json` prints `{"ok": true, "path": ..., "bytes": N}`.

When CDP is not reachable, the error carries a hint, and the hint tells two cases apart: `UniDTK application is not running. Start it via 'udtk run' or 'schtasks /run /tn UniDTK'` when the socket also fails, and `UniDTK core is running, but WebView2 CDP (port N) is not responding.` when only CDP fails.

`cdp-list` prints the JSON list of CDP targets from `GET /json`.

---

## Chapter 11. `doctor`: the diagnostic loop

`doctor` is one entry point for health. It reads declarations from the modules rather than holding its own lists. Each section is described below.

### 11.1 Flags and which sections run

```text
udtk doctor [-kernel] [-modules] [-ping] [-env] [-check] [-quick] [-simulate] [-all]
```

Each flag has a long form with two dashes (`--kernel`, and so on). The whitelist is `ALLOWED_DOCTOR_FLAGS`, and any other flag is refused with exit 2.

| Flag | Section | What it does |
|---|---|---|
| `-check` | Compiler and build | Runs `cargo check` on the toolchain. This takes tens of seconds |
| `-env` | Dependencies | Presence of the files that modules declare (DLLs, executables, model weights, scheduled tasks), and the `.env` file |
| `-kernel` | Kernel | Five dry checks, in memory (see 11.3) |
| `-modules` | Modules | Four phases per module (see 11.4) |
| `-ping` | Runtime and IPC | Connects to the socket, asks `ping`, and reads the journal statistics |
| `-quick` | Shortcut | `env`, `kernel`, `modules`, and `ping`. No compilation |
| `-simulate` | User simulation | Probes of the live app and of the rendering functions (11.6) |
| `-all` | Everything | The full audit. This is the default |

Selection rules, as coded in `doctor_cmd`:

- If a selective flag is given and neither `-all` nor `-quick` is, **only** the chosen sections run.
- With `-quick`, the sections are `env`, `kernel`, `modules`, and `ping`.
- Otherwise (no flag, or `-all`), the sections are `compiler`, `env`, `kernel`, `modules`, and `ping`. `-simulate` runs only when asked for.

### 11.2 Compiler and environment

`-env` iterates the modules of the registry and, for each declared dependency, checks its presence:

- `DynamicLibrary` (for example `libmpv-2.dll`, `tdjson.dll`), resolved with `paths::resolve_binary`, together with its companion files.
- `Executable` (for example `sing-box.exe`, `vpn-helper.exe`, and `ciadpi.exe` for the Telegram proxy, which is Optional and gives a WARN, not a FAIL).
- `ModelAsset` (the model graph and its external weights, under `opt/models`).
- `WindowsTask` (the task must be registered in Task Scheduler).

The `.env` file is looked up and reported. The scheduled tasks are checked for state, and a task that a module declares but that `udtk tasks` does not manage gives `[WARN] task '<name>' declared by module '<id>' is not managed by 'udtk tasks'`. The severity comes from the manifest: a `Mandatory` miss is a FAIL, and an `Optional` miss is a WARN.

Limit of this section: `-env` checks **presence only**. It does not validate that a DLL is a valid x64 PE file. The PE check (`validate_pe_x64`) runs in the module phases (11.4), not here.

### 11.3 Kernel checks

Five checks that run in memory, with no disk writes. They are built on the contract in `unidtk-api`:

1. `kernel:in_memory_db`: the `ModuleDb` contract on an in-memory store (create, read, update, prefix listing, count, delete).
2. `kernel:event_bus`: the event bus loops back a published value to `state`.
3. `kernel:system_paths`: resolves the install directory, the data directory **without creating it**, the logs directory, and the cache directory.
4. `kernel:task_templates`: substitutes `@INSTALL_DIR@` and the user name into the task XML, and checks that no placeholder remains.
5. `kernel:capture_logger`: the thread-local log capture buffer is isolated.

### 11.4 Module phases

Modules come from the registry, not from a list written in the CLI. For each module the run has four phases:

1. **Declarations.** For each dependency, the carrier is checked. A `DynamicLibrary` is resolved, and `validate_pe_x64` checks the `MZ` header, the PE signature, the machine type `0x8664` (x64), and the minimum size. An `Executable` is checked for presence and, if it declares probe arguments, run with a **1500 ms** watchdog (`run_executable_probe`, started with `CREATE_NO_WINDOW`, with its output captured). A `ModelAsset` is checked for its size and companion files. A `WindowsTask` is checked for registration.
2. **Dry init.** The module is initialised with `ModuleContext::new_diagnostic(id, InMemoryModuleDb)`, in a separate thread, under `catch_unwind` and `recv_timeout(2000 ms)`. Logs are captured, so that they survive a panic. On timeout the thread is left detached, without a blocking join.
3. **Shutdown.** The modules are shut down in reverse order, with a **1500 ms** budget each.
4. **Orphans.** A directory `modules/*/rust/Cargo.toml` that the registry does not know gives `orphan Rust module ... missing from registry!` and fails the check.

No data directory is created during the run. `paths::data_dir_view()` is used, not `data_dir()`.

### 11.5 Runtime and IPC

`-ping` connects to `127.0.0.1:26540` with a 500 ms timeout, sends `ping`, and reads the answer. On success it prints `[OK] IPC ping (N ms): app is running` and then the journal statistics from `log_stats`. If the app does not answer, the output is `[FAIL] IPC ping: app is not running (...)`.

### 11.6 Simulation

`-simulate` checks the live application. The code runs these steps, in order:

1. `IPC Ping`, timed in milliseconds.
2. Journal health: `log_stats`.
3. `Frontend Error Audit`: the journal records from the sources `frontend` or `webview` at level ERROR. Any such record is a FAIL. The first three are printed.
4. Widget configuration: `get_widget_config` is read and checked for positions. An empty layout is a WARN.
5. Read-only probes of the VPN and Telegram runtimes: `vpn_status` and `telegram_status`. Each one prints `[OK]`, or `[INFO] ... (idle or offline)` if it does not answer.
6. A **CDP run** in a hidden sandbox. For each module that declares a `simulate` entry in its `metadata.json`, the probe imports the module's file, renders the function into an offscreen `div` with the class `simulation-sandbox`, and times it. The sandbox deliberately has **no** `widget-card` class, so the layout code and the harness never see it.
7. An interaction on the **offscreen copy** (`interaction:clickSandboxCopy`), and an audit of cards that show `Failed to fetch`.

Without a running app, the CDP part degrades to `[INFO] CDP simulation unavailable (...)` and does not fail. If no module declares a `simulate` entry, the output is a WARN. The probe is read-only: it does not call `save_widget_config`, and it does not click the live widgets. Earlier versions clicked the live clock widget. That defect is recorded in [H3](#h3-slices-s0-to-s5-former-109-to-1022).

### 11.7 Result

The last line is one of two:

```text
[✓] UniDTK doctor: ALL CHECKS PASSED                     (exit 0)
[!] UniDTK doctor: FAILED (N issues detected)            (exit 1)
```

A failure in a module phase is counted and reported as `[FAIL]` or `[WARN]`. It does not stop the process. The count includes failures from each section. The `-simulate` section counts as one failure when any of its checks fails. The rule matters: an earlier version printed `ALL CHECKS PASSED` while the frontend probes were broken.

---

## Chapter 12. `tasks`, `autostart`, and `path`

### 12.1 Scheduled tasks

The three background tasks are defined by templates in [scheduler-tasks/](../scheduler-tasks/). Each template contains the placeholder `@INSTALL_DIR@`, which registration replaces. The table below is taken from the files and from the `SCHEDULER_TASKS` table in [opt/cli/main.rs](../opt/cli/main.rs).

| Task | Template | Runs | Run level | Hidden |
|---|---|---|---|---|
| `UniDTK` | [UniDTKTask.xml](../scheduler-tasks/UniDTKTask.xml) | `@INSTALL_DIR@\unidtk.exe` | `LeastPrivilege`, for the Users group (`S-1-5-32-545`) | `false` |
| `UniDTKSingbox` | [UniDTKSingboxTask.xml](../scheduler-tasks/UniDTKSingboxTask.xml) | `@INSTALL_DIR@\vpn-helper.exe` | `HighestAvailable` | `true` |
| `UniDTKVpnStop` | [UniDTKVpnStopTask.xml](../scheduler-tasks/UniDTKVpnStopTask.xml) | `@INSTALL_DIR@\vpn-helper.exe` | `HighestAvailable` | `true` |

The `Hidden` column differs from the rule in [AGENTS.md](../AGENTS.md) (section 6 of the architecture rules), which says that every background task is hidden. The file `UniDTKTask.xml` says `false`. See [Chapter 18](#chapter-18-known-gaps-and-open-decisions).

`udtk tasks` has these subcommands:

- `udtk tasks status` (or `list`): the state of each task.
- `udtk tasks register [all|<name>]`: writes the XML and registers the task. `all` is the default. Registration needs `vpn-helper.exe` in `C:\Program Files\UniDTK`, and it fails with `vpn-helper.exe is not next to the app` otherwise. The template is read from the install directory first, and from the checkout if that is missing. `@INSTALL_DIR@` and the user name are substituted. The temporary XML is written as **UTF-16 LE with a byte-order mark**, because `schtasks` expects it. The registration first runs `schtasks /create` directly. Only if that fails does it retry through `Start-Process schtasks.exe ... -Verb RunAs -Wait` (elevation). The code comment says that UAC is off on the author's machine, so the elevated retry succeeds without a dialog.
- `udtk tasks unregister <all|name>`: removes the task.
- `udtk tasks cleanup-old`: removes the obsolete tasks `NothingWallpaper`, `NothingWallpaperSingbox`, `NothingWallpaperVpnStop`, and `NothingWallpaperXray`, if they exist.

Failures are reported in the exit code. The installer runs `udtk tasks register all` and checks its status. A swallowed failure once looked like a successful install while the autostart task was missing. The hook is in [toolchain/windows/hooks.nsh](../toolchain/windows/hooks.nsh). The uninstall hook runs `udtk tasks unregister all`.

### 12.2 Autostart

`udtk autostart status` reports the state of the `UniDTK` task. `udtk autostart enable` (or `on`) registers it. `udtk autostart disable` (or `off`) removes it. It is a wrapper over the same task.

### 12.3 The PATH

`udtk path status` (or `check`) reports whether `C:\Program Files\UniDTK` is in the User PATH, and whether `udtk.exe` is there. `udtk path ensure` (or `add`) adds that one directory to the User PATH. It refuses if `udtk.exe` is not in that directory. The CLI writes **no copy** of itself anywhere: two copies would drift in version. The installer runs `udtk path ensure` as part of the install.

---

## Chapter 13. `run`, `stop`, `doc`, `help`, and the splash

### 13.1 `udtk run`

```bash
udtk run [args...]
```

`run` starts **only** `unidtk.exe` from the directory of the CLI (`install_dir()`). It loads the `.env` file, sets the working directory to that directory, forwards the arguments, and starts the process with `CREATE_NO_WINDOW`. It does not wait for the app. It prints `launched <path>` and returns. If `unidtk.exe` is not next to the CLI, the error is `unidtk.exe not found next to udtk.exe (...). Run 'udtk build' or install the app first.` `run` never searches the repository for another binary. A stale copy once started from a deleted `target` directory, and that is why the rule exists.

`--debug` and `-d` are refused with `--debug was removed: CDP is always on in release builds`.

### 13.2 `udtk stop`

```bash
udtk stop
```

`stop` makes the app's files replaceable. It runs in three steps:

1. If CDP is reachable, it asks the page to quit through its harness (`h.invoke('quit_app')`). The command `quit_app` is defined in [core/kernel/main.rs](../core/kernel/main.rs). In that case it allows **3500 ms** for the quit.
2. It then polls for processes whose executable lies under the install directory, excluding itself. The total wait is the grace period plus **2500 ms**. The poll uses PowerShell and `Win32_Process`.
3. If processes remain after the deadline, it runs `taskkill /F` on each. Finally it runs `schtasks /end` for the elevated tasks (`UniDTKSingbox`, `UniDTKVpnStop`).

It prints `stopped`. It does not check the result after the forced kill.

### 13.3 `udtk doc`

```bash
udtk doc
```

`doc` renders [README.md](../README.md), [AGENTS.md](../AGENTS.md), and every page in `docs/` into `%LOCALAPPDATA%\UniDTK\cache\book`, and opens `index.html` in Explorer. The source is the installed `book` folder if it exists, and otherwise the checkout. The output goes to the cache because `Program Files` is read-only.

### 13.4 `udtk help`

`udtk help` prints the text help. `udtk help --json` prints the manifest of Chapter 7: `protocol`, `cli_commands`, `app_commands`, and a note. The offline manifest works when the app is not running, because the inventory is compiled into the CLI.

### 13.5 The splash

`udtk` with no arguments prints the splash, from [opt/cli/splash.rs](../opt/cli/splash.rs) and the generated art in [opt/cli/splash_art.rs](../opt/cli/splash_art.rs). The art is generated, so it must not be edited by hand. The generator is not in the repository. The sessions record says it lives next to the design skill, outside the repository, under the name `make_splash.py`, and that the repository keeps only the output.

How the layout is chosen:

- The width comes from `COLUMNS`, and the height from `LINES`.
- The emblem is the largest of three precomputed sizes that fits **both** the width (a threshold computed from the art's geometry) **and** the row budget. The budget is `clamp(height - 2, 10, 28)` (`rows_budget`). Narrower or shorter windows fall back to smaller emblems, then to a stacked layout, then to text.
- Colour is used only on a terminal. It is disabled by `NO_COLOR` and by `TERM=dumb`, and it is forced by `FORCE_COLOR` or `CLICOLOR_FORCE`.
- On Windows, the console is set to VT mode and to code page 65001, and the result is **verified**. If that fails, the output is plain text instead of raw escape sequences.
- The right column is aligned to the computed width of the emblem, not to a constant. Otherwise the lines drift.

The command map at the bottom wraps at narrow widths, with an indent.

---

## Chapter 14. `build`, step by step

`udtk build` is the only way to make a release. It builds the three binaries and the NSIS installer in one tauri pass, installs the installer silently into `C:\Program Files\UniDTK`, proves the install by hashes, restarts the app, and checks it over IPC. Do not run `cargo build` by hand, and do not copy binaries into `Program Files`. Both skip the installer, and the scheduled tasks and the PATH are registered by the installer.

```bash
udtk build          # thread count from .cargo/config.toml
udtk build -j 1    # one compiler thread, for a machine low on memory
```

The `-j N` (or `--jobs=N`) argument sets `CARGO_BUILD_JOBS` for the build. `.cargo/config.toml` sets `[build] jobs = 2` and the `RUST_MIN_STACK` stack size for the compiler. The lessons behind those numbers are in 14.3.

### 14.1 Preconditions

- The `udtk.exe` that runs the command must **not** be in `Program Files`. If it is, the command refuses with `udtk build cannot self-build from Program Files (ACL). Run it from the checkout's target\release\udtk.exe`. The reason is that the directory is read-only for the user.
- The working copy must be a checkout with a `toolchain/` directory. Otherwise the command fails with `toolchain not found (run from repo root)`.

### 14.2 The steps

Each step below is a stage in `build_cmd` in [opt/cli/main.rs](../opt/cli/main.rs).

**Step 0. Stop the app.** `build_cmd` calls `stop_cmd()`, which is described in 13.2. The running app holds DLLs such as `tdjson.dll` and `libmpv-2.dll`. Replacing a file in use gives `os error 32`, so the app must be stopped first.

**Step 1. Set the running CLI aside.** The `udtk.exe` that runs the build is in `target\release`, and cargo must relink it. Windows will not overwrite a running executable. So the command renames its own file to `udtk.exe.held`. It deletes any old `.held` file first.

**Step 2. Build with tauri.** `run_cargo_tauri(["build", "--bundles", "nsis"], jobs)` runs `cargo tauri build --bundles nsis` in the `toolchain` directory. Tauri builds the three binaries and the NSIS installer in one pass. The environment is the `.env` file, and `CARGO_BUILD_JOBS` is set if `-j` was given. If the build fails, the command renames `udtk.exe.held` back to `udtk.exe`, so the CLI stays usable, and it returns the error.

**Step 3. Remove the set-aside copy.** After a successful build, cargo has written a fresh `udtk.exe` at the canonical path. The `.held` copy is deleted, so that the installer packs the new CLI and not the old one.

**Step 4. Copy the runtime library.** `opt/libmpv/libmpv-2.dll` is copied to `target\release`. It is compared by SHA-256 first, and an unchanged copy is left alone, because a large DLL is often held for a moment by a virus scanner. If the source is missing and no copy exists, the command fails with `... is missing and target/release has no copy`.

**Step 5. Prune stale resources.** `prune_stale_resources` removes entries in `target\release\modules` and `target\release\folders` that no longer exist in the sources. A module removed from the sources would otherwise show a second widget card. Each removal is logged as `pruned stale resource: ...`.

**Step 6. Copy the model weights.** If `opt/models/discogs-maest-129e` exists, its `model.onnx` and `model.onnx.data` are copied into `target\release\models\discogs-maest-129e`.

**Step 7. Find the installer.** `find_installer` looks for a file ending in `-setup.exe` in `target\release\bundle\nsis`. None found gives `installer not found under target/release/bundle/nsis`.

**Step 8. Install silently.** `run_installer_and_verify` starts the installer with `/S`, with no window.

- If the installer cannot start because another process holds the file (`os error 32`, typical right after a build, when a scanner reads a fresh 400 MB file), the command retries **up to 5 times, 3 seconds apart**.
- If the installer returns `740` (`ERROR_ELEVATION_REQUIRED`, because a per-machine install needs administrator rights), the command runs the installer again with `Start-Process ... -Verb RunAs -Wait`. On the author's machine UAC is off, so this succeeds without a prompt.
- Because NSIS can skip a locked file and still exit 0, the command does not trust the exit code alone. It waits up to **60 seconds** until `C:\Program Files\UniDTK\unidtk.exe` exists with the same size as `target\release\unidtk.exe`. Otherwise it fails with `installer ran but installed unidtk.exe is missing or wrong size`.

**Step 9. Verify by hashes.** For `udtk.exe`, `vpn-helper.exe`, and `libmpv-2.dll`, the command computes the SHA-256 of the installed copy and of the copy in `target\release`, using `certutil -hashfile`. A difference, or an empty hash, fails with `<name> did not update after install`. This is the proof that the installed files are the new ones.

**Step 10. Restart and check over IPC.** The command starts the task with `schtasks /run /tn UniDTK`. Task Scheduler refuses to start the task from a shell that runs at medium integrity (a token filtered by UAC). When that happens, the command starts `C:\Program Files\UniDTK\unidtk.exe` directly, with the same working directory, and notes it in the output: `restart: task start denied (medium-integrity shell) — spawned the app directly`. Then it sends `ping` every 400 ms for up to **30 seconds**. If the app does not answer, the command fails with `app did not answer on IPC within 30s after install`.

**Step 11. Report.** On success the output starts with `build to install to start to verify OK`, and then it appends the journal's ERROR records (`logs --level ERROR`). An empty list is the expected result.

### 14.3 Lessons behind the steps

These are the failures that shaped the steps. They are facts from the recorded sessions, and they are the reason for the stop-first rule.

- **Memory.** A full build with thin LTO, on the author's 15.7 GB machine, reached the system's allocation limit. The page file grew to 48 GB, and the compiler failed with `LLVM out of memory`. The fix in `.cargo/config.toml` is a low job count and a large stack. For the tests, `cargo test --release --workspace -j 2` brought down two LTO builds at once with `0xc000012d`, so the tests run with `-j 1`. These are the author's recorded limits, not a general rule.
- **Held files.** The installed app once held `tdjson.dll` and the OpenSSL DLLs **from the repository's `target\release`**, because the path to the Telegram library was stored in the settings. The consequences were an `os error 32` without a file name, and a broken Telegram after the folder was renamed. The step 0 stop and the install path prevent both.
- **Cold build.** `build.rs` requires the resources of `tauri.conf.json` (`../target/release/udtk.exe`, `vpn-helper.exe`) to exist. On an empty `target/` the build cannot succeed, because it needs the program it builds. The recorded workaround is zero-length placeholder files, which cargo overwrites. The recorded fix is to accept missing resources at build time and check them at packaging.
- **Quiet success.** A cargo run piped through a PowerShell `Select-Object` was cut off early, and the artifact silently stayed old. It was caught twice as "build succeeded, binary is old". Run cargo directly, and read the exit status.
- **Commit `cd1949b`** (`fix(cli): leave an unchanged libmpv copy alone`) is the reason for the hash check in step 4.

---

## Part III. Rules for modules and the install

## Chapter 15. How a module joins the bridge

A module is a vertical slice, `modules/<id>/{rust,ui}`. Its back half, `rust/`, is a crate. Its front half, `ui/`, is the page code. The bridge reads a module in four places. Each has a rule, and most of the rules are enforced by a build lint or a test.

### 15.1 Dependencies are data

A module declares its external needs in `Module::dependencies()`, as a static list of `ModuleDependency` values. The types are in [core/api/src/manifest.rs](../core/api/src/manifest.rs):

```rust
fn dependencies(&self) -> &'static [manifest::ModuleDependency] { &[] }
```

- `DependencySeverity::{Mandatory, Optional}`: a missing mandatory dependency is a FAIL, and an optional one is a WARN.
- `DependencyKind::{DynamicLibrary, Executable, ModelAsset, WindowsTask}`: what the dependency is.

Diagnostics read this list. A module has no diagnostic code of its own. The old method `diagnostic_checks` was removed from the `Module` trait.

### 15.2 The registry

A module is registered in [core/kernel/registry.rs](../core/kernel/registry.rs): in `all_modules()`, and in the constant `ALL_MODULE_IDS`. The current list is `glyph-bar`, `explorer`, `vpn`, `telegram`, `mp3-player`, and `micrographics`. The module must also be a dependency in [toolchain/Cargo.toml](../toolchain/Cargo.toml). The kernel and the CLI both use the registry, so there is no second list. (The CLI includes `registry.rs` with `#[path]`.)

### 15.3 Commands of a module

A module answers for its commands through `ipc_call`, with the signature `ModuleIpcFn`:

```rust
fn(cmd: &str, args: &serde_json::Value, host: Option<HostHandle<'_>>)
    -> Option<Result<serde_json::Value, String>>
```

It returns `None` when the command is not its own. The kernel then reports the module as registered but not handling the command, which is a build inconsistency and not a silent answer. To add a command, the module:

1. declares it in [command_inventory.rs](../core/kernel/command_inventory.rs), with `read_only`, `read_only_cli`, or `write`,
2. implements it in its own `ipc_call`, with the name taken from the inventory constant,
3. gets one line in `MODULE_ROUTES`, and, if the kernel must route it, one handler in `ROUTES`.

A write must also be listed in the facade's `WS_WRITES`, and a read in `WS_COMMANDS`, once it is verified. Chapter 4 explains why.

### 15.4 The build lints

Lints run in [toolchain/build.rs](../toolchain/build.rs) at compile time. They are cheap, and they stop a broken build before it starts:

| Lint | What it checks | Error text |
|---|---|---|
| `lint_modules_integrity` | Every `modules/*/rust/Cargo.toml` is in the registry and in `Cargo.toml` | `BUILD INTEGRITY ERROR: Rust module '<id>' ... is missing from core/kernel/registry.rs` |
| `lint_ui_imports` | No UI file imports Tauri directly or through the vendor path. Only the facade may | `UI IMPORT ERROR: ... imports Tauri directly; the interface must go through the bridge` |
| `lint_module_contracts` | Each module's `ui/metadata.json` is well formed: an `embeddable` entry names a file that contains its render function, a container is not embeddable, and tints and packs have ids and labels | `<file>: embeddable.render <name> is not in <module>` (and similar) |
| `lint_simulate_metadata` | Each `simulate` entry names an existing file that defines the function, at least one entry exists, and the production CLI has no hard-coded render names (the list is `ENTRY_POINTS`, six names including `renderMicrographics`) | (metadata check) |

The rule for modules is narrower than it was. A module must not depend on the kernel crate. It must depend on `unidtk-api`. Tauri host types are allowed, because four of the five modules already use them, and the contract itself gives the host handle (see the decision recorded in [Chapter 6](#chapter-6-design-decisions)). **No lint in `build.rs` enforces this rule as of this rewrite.** The plan planned such a check in `lint_modules_integrity`, and the code of that function does not contain it (see [Chapter 18](#chapter-18-known-gaps-and-open-decisions)).

### 15.5 Guarantees that are tests, not build checks

Some guarantees cannot be checked by a build script, because `generate_handler!` is a macro, and a build script cannot read it. These are tests:

- `every_socket_command_has_a_handler` and `every_command_has_a_handler`: every declared command has a route.
- `every_manifest_ui_command_is_registered`: every command declared for the UI door is registered in the kernel's `generate_handler!`.
- `frontend_invoke_names_are_registered`: every literal name in `invoke("...")` in the interface is a registered command. Plugin commands, such as `plugin:fs|...`, are skipped.
- `frontend_listen_names_exist_in_rust`: every literal name in `listen("...")` is a string literal in the Rust code. This proves the name exists, not that the event is emitted on every path. A text analysis cannot be stricter, because some names are built dynamically.
- `simulate_probes_name_real_socket_commands`: the probes of `doctor -simulate` name real read-only socket commands.

Each of these was checked with a negative test: a deliberately broken name made it fail, and the restored name made it pass.

---

## Chapter 16. Install layout and data

The rule is that the program and its data are kept apart, so that an update never overwrites user state. The list of the program's files below is the one stated in [architecture.md](architecture.md) (section 4.8). The path logic that this book cites is in code. The installer's file list was not read for this rewrite.

| What | Where | Why |
|---|---|---|
| Program: `unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, `sing-box.exe`, DLLs, `models/`, `modules/`, `scheduler-tasks/` | `C:\Program Files\UniDTK` | Read-only for the user. The installer writes it |
| Data: `logs/`, `cache/`, `music/` | `%LOCALAPPDATA%\UniDTK` (`logs_dir()`, `cache_dir()`, `music_dir()` in [core/api/src/paths.rs](../core/api/src/paths.rs)) | The application writes here, never next to itself |
| Databases: `unidtk.redb`, `unidtk_logs.redb` | `%LOCALAPPDATA%\UniDTK` | Described in [architecture.md](architecture.md) |

The `UniDTK` task runs `unidtk.exe` from the install directory. The task templates contain the placeholder `@INSTALL_DIR@`, so that no absolute path is stored in the repository. The installer, not the first run of the app, registers the tasks and the PATH.

The application does not write into its own directory. The code in `paths.rs` resolves the paths, and the modules declare a store rather than building paths themselves. The layout is therefore changed in one place.

---

## Part IV. Verification and open items

## Chapter 17. How to verify a change

The rule in [AGENTS.md](../AGENTS.md) is that a change is not done when it compiles. It is done when it has been run and checked through the bridge. The steps below use `udtk` only. Do not call `cargo` or `node` directly. The two exceptions are the build itself, which is `udtk build`, and the tests, which `udtk doctor -check` runs.

1. **Build and install:** `udtk build`. It must end with `build to install to start to verify OK` and an empty ERROR list.
2. **Health:** `udtk doctor -quick` and then `udtk doctor -all`. Both must exit 0.
3. **Journal:** `udtk logs --level ERROR` (expect `[]`) and `udtk logs --source frontend --limit 30`.
4. **Page:** `udtk gui tree`, and `udtk gui js:window.__UNIDTK_HARNESS__.getErrors(true)`. The page must have no errors.
5. **Probe:** `udtk doctor -simulate` with the app running. It must print `ALL CHECKS PASSED`.
6. **Visual:** `udtk shot`. The screenshot is the mandatory check of the layout after any change to the interface or the kernel. Open the file and look at it.
7. **Behaviour of a write:** change the state through the page (`udtk gui click:...`, or `js:` through the page's door), then read the consequence back with a read command.

For a change to the transport, add the `shadow` comparison of Chapter 4: set `sessionStorage["unidtk.transport"]` to `shadow` through `udtk gui js:...`, run the scenario, and read `getTransportState()`. The divergence count must be zero. Then reset the key and check that the transport is `tauri` again.

---

## Chapter 18. Known gaps and open decisions

Each item was checked against the code on 2026-11-02. Items marked *not re-verified* are taken from the sessions and were not re-read for this rewrite.

**Migration of the page (see [H4](#h4-migration-of-the-page-to-the-new-door-former-1023-to-1035)).**

1. Four writes are still served only by Tauri: `set_module_enabled`, `ai_dj_model_download`, `telegram_set_archived`, and `explorer_sort_set`. They are not in `WS_WRITES`.
2. Read commands that are not in `WS_COMMANDS`: `get_system_volume`, `get_disabled_modules`, `telegram_chats_archive`, `telegram_messages_around`, `explorer_sort_get`, and the kernel's own readers (`log_stats`, `log_max_id`, `log_delta`, `logs`). The volume reaches the page as an event, not as a call.
3. Request-id deduplication for writes, and a refusal of automatic retries of writes, are part of the plan. They are **not** implemented in [ipc.rs](../core/kernel/ipc.rs) or [ws.rs](../core/kernel/ws.rs). Until they are, a write that times out on the client may still run on the server.
4. The default transport is still `tauri`. `shadow` and `ws` are opt-in, through `sessionStorage`.
5. The contract's emitter is the only path to Tauri for events in the kernel's own code. A search of [core/kernel/input.rs](../core/kernel/input.rs) and [core/kernel/main.rs](../core/kernel/main.rs) finds no other direct `emit`. Modules were not searched again for this rewrite (*not re-verified*).

**Diagnostics and the install.**

5. `doctor -env` checks presence only. It does not validate the PE format. The PE check lives in the module phases (11.4).
5a. The rule that a module must not depend on the kernel crate has no enforcing lint in `toolchain/build.rs`. The plan said `lint_modules_integrity` would fail on such a dependency. The function reads the registry and `Cargo.toml` only, and it does not check module dependencies on the kernel. The rule currently rests on review.
6. `SCHEDULER_TASKS` and `OBSOLETE_TASKS` are hard-coded in the CLI on purpose. They describe the installation, not the modules. A drift against a module's manifest is reported as a WARN.
7. The `cli_commands` array of `help --json` is written by hand in `help_cmd`. The application half comes from the inventory.
8. The `UniDTK` task has `Hidden` set to `false` in [UniDTKTask.xml](../scheduler-tasks/UniDTKTask.xml), while the architecture rule says that every background task is hidden. The code and the rule disagree. Decide which is right, then change one of them.
9. The splash generator `make_splash.py` is not in the repository. The sessions say it lives outside it. The art in `splash_art.rs` cannot be regenerated from this repository alone.

**Stop and update.**

10. The question of who stops a running instance during an update is open. The uninstaller's `PREUNINSTALL` removes the autostart task on upgrade too, and the old process holds ports 26540 and 9222 until it exits. Today `udtk build` stops the app itself (Chapter 14).

**Records and counts.**

11. The old live checklist in this book used `cargo build --release --bins` and `cargo test --workspace`. The rule in [AGENTS.md](../AGENTS.md) forbids a direct release build, and the test count changed. Chapter 17 replaces the checklist. The counts in [Part V](#part-v-history) are the ones recorded at the time, and they are not re-run.
12. The sessions record and [AGENTS.md](../AGENTS.md) still refer to the older section numbers of this book. The History chapter keeps those labels in its headings, so that the references still point at the right text.

---

## Part V. History

This part keeps the session records of the design, the slices, and the migration, condensed. Each heading keeps the former section number in brackets, so that references from [sessions.md](sessions.md) and [AGENTS.md](../AGENTS.md) still resolve. The numbers, commit hashes, and recorded results are kept. The narrative of each step is shortened to its outcome.

### H1. Sessions 58 to 60 (17.09.2026)

**Commit `c3c7070` (`feat(kernel,cli)`).**

- The hard-coded `udtk status` (and `udtk ping`) was removed. The aggregating function `status_snapshot()` in the kernel was eliminated, so that a new module would not force kernel edits.
- `doctor` was rewritten around a strict flag whitelist. `--app` was replaced by `-ping`. The redundant synonyms were removed later: only `-x`/`--x` pairs remain. The duplicate `udtk check` was removed in favour of `udtk doctor -check`.
- A declarative manifest (`core/api/src/manifest.rs`) and `Module::dependencies()` were added. `diagnostic_checks` was removed from the modules. `ModuleRunMode`, `ModuleContext::new_diagnostic`, `InMemoryModuleDb`, `capture_logs`, `capture_logs_unwind`, `CheckStatus`, and `CheckResult` were introduced.
- `core/kernel/registry.rs` and the build lint `lint_modules_integrity()` were added.
- The CLI gained `validate_pe_x64`, `run_executable_probe`, `run_module_checks` (four phases), `run_kernel_checks`, and `run_ping_check`.
- The splash was redesigned: `UniDTK` in mixed case, `DTK` in the colours of the German flag, a titanium Ouroboros, an isolated Saturn.

**Commit `afdd2e4` (`fix(cli,ui)`).**

- `side_by_side` computes the emblem width dynamically, so the right column is aligned to one axis. A regression test was added.
- The paint clipping of the library popup was removed (`contain: none !important` for `.music-widget`), so the popup no longer clips the header and the close button.

**Later.** The shell name was removed from the CLI and from the paths in session 131, together with the `Shell*` tasks and the `%LOCALAPPDATA%\Shell` cleanup. Only the Win32 API `shell32` and the environment key `SHELL_DEBUG` remain on purpose.

### H2. The plan and its critique (former 10.1 to 10.8)

**The problem (10.1).** The command surface was written three times (the router `match`, the `commands` reply, and the Tauri list), and a fourth list lived in `doctor`. Each of them drifted. The plan set out to remove that drift.

**Recorded decisions (10.2).** A single `udtk gui` command, which replaces `dom`, `eval`, and `act`. One contract with two doors. The manifest as a build artifact, linked into both binaries. A module declares its metadata declaratively. The handwritten Tauri wrappers stay in the kernel. One routing table. No linker magic. Separation of layers: business logic in the modules, transport and inventory in the kernel, the CLI as a thin client.

**Module decisions (10.3).** Accepted: a module owns its runtime state, and the kernel stops naming module types. `metadata.json` is extended with the render entry points. Open at that time: the fate of plugins and assets (`fs`, `opener`, `path`, `convertFileSrc`), the stop during an update, and resource budgets for `gui`.

**Stages (10.4).** Stage 0: secure the debug door. Remove `--remote-allow-origins=*` in three places (`core/kernel/main.rs`, `dev --debug`, `run --debug`), stop overwriting `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, and discover the port. Stage 1: protocol envelope v1, with a version, an id, limits, and a handshake. Stage 2: a command manifest. Stage 3: one routing table. Stage 4: the CLI reads the manifest. Stage 5: `udtk gui` with one session per batch, a log delta per step, and a receipt per step. Stage 6: `simulate` rewritten on the registry, without a live click. Stage 7: the front end moves to the new protocol, one command at a time, behind a flag. Stage 8: documents.

**Acceptance criteria (10.5).** Seven measurable criteria. (1) The manifest matches the routing table, by a test. (2) `help --json`, the manifest, and the runtime answer of `commands` agree on the set of names. (3) Every name in the interface's `invoke` calls is in the manifest. (4) Offline, `help --json` and `doctor -modules` are green. (5) `gui`: one CDP handshake per batch and at most two IPC connections per batch. (6) `simulate` is read-only. (7) Regressions: `cargo test --workspace` green, `doctor -all` without errors, and `shot` shows no layout change.

**Risks (10.6).** Security: CDP always on, with the boundary at the Windows user account. Version compatibility: an old `udtk.exe` and a new kernel diverge, so a protocol version is needed. Resources of a persistent process. Threads and COM: commands that touch COM or the clipboard must not run on socket threads, because those threads do not initialise COM. Shutdown: a request in flight during `app.exit(0)` could leave a torn redb transaction, so a closing gate and a drain of in-flight requests are needed. Module life cycle: an init timeout leaves the module alive without `shutdown`. Update: `PREUNINSTALL` removes the autostart on upgrade.

**Outcome of the critique (10.7).** Rejected from the first version: a proc macro with source scanning, generated Tauri wrappers, building the table from `registry::all_modules()`, write commands on the socket, and a socket-based single source of truth for `help --json`. Accepted: the manifest as a build artifact, the envelope with version and capabilities, the distinction "no such command" against "not ready", read-only by default on the socket, `gui` with one session and one snapshot per batch with `log_delta`, the rewritten `simulate`, an inventory of the Tauri dependencies before the front-end migration, and measurable criteria with resource budgets.

**Consensus after dialogue (10.8).** Two rounds with a subagent, with live probes.

- *10.8.1.* "The kernel stops naming module types" is not achievable in full at stage 3. The rule "a module depends only on `unidtk-api`" is obsolete, because four of the five modules declare `tauri`. The owner narrowed the rule: a module does not depend on the kernel crate, and Tauri host types are permitted.
- *10.8.2.* The check "spec without a handler" cannot be done at build time. The guarantee is a test, and it must be non-degenerate: removing a handler must turn the test red.
- *10.8.3.* The slices: S0 (CDP and discovery), S1 (`gui`), S2 (manifest, route, and CLI), S3 (decoupling of types in `ipc.rs`), S4 (front end behind a flag), and S5 (`simulate` and acceptance). The gate on each slice was `cargo test --workspace`, `doctor -all`, `doctor -check`, and `shot`.
- *10.8.4.* Borrow techniques from `agent-browser`, not its daemon. In order: refusal on an occluded target with the name of the covering element; references instead of indexes (`data-udtk-ref`); an annotated screenshot when it is cheaper than a tree; no daemon.
- *10.8.5.* Found by the probes: a contradiction about DPI in the older test notes (CDP takes CSS pixels 1:1, and multiplying by the device pixel ratio misses); the harness selected elements by class and exposed positional ids, which are not stable; and an `occluded` flag that was detected but ignored, so that a click was sent and the receipt said `ok`. The fix was to derive `ok` from the hit test.

### H3. Slices S0 to S5 (former 10.9 to 10.22)

**Slice S0 (former 10.9).** Changed: `--remote-allow-origins=*` removed. `pick_cdp_port()`, `merge_browser_args()`, and `cdp_port()` added. `ping` returns `cdpPort`. The write `save_widget_config` removed from the route. `dev --debug` and `run --debug` lost the wildcard, and the CLI asks the app for the port instead of hard-coding 9222. Live results: `ping` gave `cdpPort: 9222`. A WebSocket with `Origin: http://evil.test` was rejected, where before it was accepted with `101`. A client without an Origin header worked. With 9222 held by a foreign listener, the app took 54060 and `udtk eval` worked through it. `save_widget_config` over IPC gave `unknown cmd`. `doctor -simulate` without the app gave `FAILED (1 issue detected)`, exit 1. `cargo test -p unidtk --bin udtk`: 10 of 10. `doctor -quick`: all checks passed.

*Remarks of the independent reviewer (GPT-5.6 Sol), accepted for the next slices:* idempotency and request repetition (a `gui` reconnect or a timeout after execution can repeat a side effect, so commands need a class and deduplication by request id); events were not replayed after resubscription (`Bus::last` keeps one value, so a cursor, `resumeFrom`, a gap signal, and a resync are needed); a TOCTOU window in the port choice (a re-verifiable owner marker is needed); a "fourth list" risk (the spec and the handler must be declared together); the inventory must not call the module factories; and the phrase "build-time guarantee of a handler" was wrong, because it is a test.

*Environment found on the way:* a running app holds `tdjson.dll`, `libcrypto`, `libssl`, and `z.dll` from `target\release`, so the release build fails with `os error 32`. Stop the app, build, and start it again.

**Slice S1 (former 10.10).** `udtk gui` added: one CDP session for the batch, steps `js`, `click`, `type`, `press`, `tree`, `wait`, up to 50 steps, a receipt per step, `--json`, and `--force`. `dom`, `act`, and `eval` removed. `harness.js` gained `data-udtk-ref` and the `covering` field, and lost the class selectors. The kernel gained `log_delta`. Live: a batch of two `js` steps with receipts. `tree` returned 29 nodes, all with a `ref`, none with a class locator. A click on `r1` with the menu open was refused with `refused: "occluded"` and `covering: div#menuOverlay (r30)`. `--force` clicked anyway. `cargo test -p unidtk --bin udtk`: 10 of 10. `doctor -quick` passed.

**Slice S2, partial (former 10.11).** `core/api/src/commands.rs` added the types `CommandSpec`, `Access`, and `Door`, so that a write cannot reach the socket by construction. `core/kernel/command_inventory.rs` added 21 commands at that time, compiled into both binaries. `ipc.rs` got a guard before the route. `help --json` returned `protocol: 1`, `cli_commands`, and `app_commands`. Live: `invoke commands` gave 21 records. `invoke log` was refused by policy. `invoke save_widget_config` gave `unknown cmd`. Honest limits left for S3: the handlers were still in a `match`, and module commands were still listed in the kernel.

**Slice S1 addendum (former 10.12).** The reviewer's remarks on idempotency, events, and the port TOCTOU were moved to S4.

**Slice S3, step 1 (former 10.13).** The `match` became a route table, `ROUTES`, with handlers as functions and names taken from inventory constants. `route()` checks the inventory first. Tests: `every_socket_command_has_a_handler`, `every_handler_is_declared_in_the_inventory`, and `route_names_are_unique`. Live: `ping`, `vpn_status`, `log_max_id`, and `log_delta` answered, `log` was refused by policy, `gui js:6*7` gave 42. `cargo test --workspace`: 0 failures, 15 suites green.

**Slice S3, step 2 (former 10.14).** Modules answer for their own commands. The contract gained `HostHandle` and `ModuleIpcFn`, and the kernel holds `MODULE_ROUTES`, addressing a module by id. The modules: `glyph-bar` (`volume_get`), `explorer` (`explorer_drives`, `explorer_places`), `vpn` (`vpn_status`, `vpn_settings`, `vpn_list_servers`), `telegram` (`telegram_status`, `telegram_chats`), and `mp3-player` (`player_status`, `ai_dj_status`, `list_tracks`, `list_playlists`, `get_submodules`). The gate became an automatic test, `router_does_not_name_module_state_types`. Live: all 17 socket commands at that time answered with exit 0. Adding a module command no longer needs a kernel edit.

**Slice S4, step 1 (former 10.15).** Protocol envelope v1: `PROTOCOL = 1`, one function `respond(id, outcome)`, an echoed `id`, and an optional envelope so that the old bridge keeps working. `ping` reports `proto` and `capabilities`. The CLI numbers its requests and checks the reply. Live: a raw request with `"id":42` came back with `"id":42`. `cargo test --workspace`: 0 failures.

**Slice S4, step 2 (former 10.16).** The WebSocket door, `ws.rs`, with a handshake, a per-launch token, a hard timeout, and `wsPort` in `ping` (the token is not reported there). The token reaches the page only through `session_info`. Live: a WebSocket without a handshake was refused with the handshake message. `ping` gave `wsPort: 51090`. The token read through the page was 32 hex characters. The handshake with the token gave `ready`. A `volume_get` after the handshake answered with `id: 7`. The boundary was stated: the token keeps out other processes and web pages, not code inside the process.

**Slice S4, step 3 (former 10.17).** Three Explorer files imported Tauri by a **relative** path, which bypasses the import map. They were switched to the bare specifier `@tauri-apps/api/core`, and a build lint, `lint_ui_imports()`, was added. Negative test: a bypass put back made the build fail with `UI IMPORT ERROR`. The interface tree had 29 nodes and no errors. The Explorer panel itself was not opened in this pass, and the book says so: running it live was left for the next session.

**Slice S5, step 1 (former 10.18).** The defect: `doctor -simulate` clicked the **live** clock widget twice, which called the real `toggleWidget`, collapsed a widget the user had opened, and raced the animation timer. The fix: the click is made on an offscreen copy in the sandbox (`interaction:clickSandboxCopy`). The sandbox lost the class `widget-card`, got `contain: strict`, and is skipped by `getSemanticTree`. The dead `log` calls were removed. Live: `doctor -simulate` gave exit 0. All five renders were `OK`. The tree still had 29 nodes, and the sandbox was not in it. Page errors: 0.

**Acceptance criteria 3 and 4 (former 10.19).** Criterion 3: the test `frontend_invoke_names_are_registered` was added, and it was proved by a negative test (a call to a command that does not exist made it fail). Criterion 4: with the app stopped, `help --json` gave `protocol: 1`, 13 CLI commands, and 21 application commands, and `doctor -modules` exited 0. `cargo test --workspace`: 15 suites, 0 failures, 0 warnings.

**Migration preparation (former 10.20).** An inventory of the four categories of Tauri dependency: commands, events, plugins (`fs`, `opener`, `path`), and assets (`convertFileSrc`). Commands and events are checked by tests. `frontend_listen_names_exist_in_rust` was first written with 10 false positives, because some names are passed on a separate line or are built dynamically. Each was checked in the code, and the criterion was weakened to "the name is alive". Negative test: a renamed listener made the test fail.

**Idempotency as data (former 10.21).** `Idempotency::{Safe, Unsafe}` was added to the contract, set by the constructors, and published as `retry_safe`. Honest note: the invariant held trivially, because the socket had only reads.

**Plan for the migration (former 10.22).** Four discrepancies were found in the code before the migration: the dispatcher applied the socket policy to both doors; the WebSocket had no push and no reconnection; some events bypassed the contract emitter; and some response shapes differed between the doors (`list_tracks`, `get_widget_config`). The steps that followed are in [H4](#h4-migration-of-the-page-to-the-new-door-former-1023-to-1035).

### H4. Migration of the page to the new door (former 10.23 to 10.35)

**Steps 1 and 2 (former 10.23).** Baseline recorded: `cargo test --workspace` with 0 failures, `doctor -quick` with 0, the tree with 29 nodes, and 0 page errors. The facade `core/ui/unidtk-bridge.js` was created, and 13 files were switched to the single import `@unidtk/bridge` (20 import lines). The real Tauri functions stayed private inside the facade. `session_info` always goes through Tauri. The transport is read once. `WS_COMMANDS` was empty. The lint `lint_ui_imports()` covers all UI modules. Live: 29 nodes, 0 errors, and `getTransportState()` gave `{transport: "tauri", migratedCommands: [], sessionInfoCalls: 0}`.

**Steps 3 to 5 (former 10.24).** The transport flag, the WebSocket client inside the facade (one memoised session, the handshake as the first frame, pending requests keyed by id, and separate timeouts). Two blockers were found and fixed. (1) The page's CSP `connect-src` allowed only the TCP door, so `ws://127.0.0.1:*` was added. (2) The first shadow run gave 16 divergences out of 17, all of them key order. The comparison became structural. The handshake timeout was raised to 8 seconds, because a cold page timed out. The first three commands were `player_status`, `vpn_status`, and `telegram_status`. `shadow`: 19 comparisons, 0 divergences. `ws`: 18 commands answered by the socket door. Rollback to `tauri`: verified.

**Step 7, first slice (former 10.25).** Six commands: `player_status`, `vpn_status`, `vpn_list_servers`, `vpn_settings`, `telegram_status`, `ai_dj_status`. A per-command counter, `perCommand`, was added, because "23 comparisons, 0 divergences" was not proof when only two commands had been called. `shadow`: 54 comparisons, 0 divergences. `ws`: 25 calls.

**The seventh command, and two shadow errors (former 10.26).** `get_widget_config` was unified: the socket now returns `null` where there is no configuration, like Tauri. The shadow found: (1) 14 refusals from a 4-second timeout, during a library scan. The timeout was raised to 15 seconds, because the kernel serves one connection in order. (2) A race in the client's `ensureSocket`, which cached the socket and not the promise, so two calls during the handshake opened two connections. The promise is cached now. `shadow`: 34 comparisons, 0 divergences. `ws`: 30 calls.

**The whole read surface (former 10.27).** Three contracts were unified. `telegram_chats` returns an array, as Tauri does. `get_drives` and `get_user_places` take the names that both sides already use. `list_tracks` and `list_playlists` accept `search`, `sort_key`, and `sort_dir`, and return an array. Without this, search and sorting would have stopped working in WS mode only. `shadow`: 49 comparisons, 0 divergences. `ws` with a live scenario of the menu and Explorer: 31 calls, 29 nodes, 0 errors.

**Events (former 10.28).** `core/kernel/events.rs` was created: a global `seq`, a ring of 64 frames per event, state events that keep only the newest frame, an explicit `gap`, and `try_send` delivery. The broker is fed from the same emitter that feeds Tauri, so both doors carry the same events with the same numbers. The push protocol: `subscribe` and `unsubscribe`, the frame `{v, type:"event", event, seq, payload}`, and `gap` on a cursor the ring no longer covers. Three bugs of the instrument were fixed: comparison by position gave false divergences (state events compare by convergence of values, not by order); one comparator per `listen` drained the same buffer twice; and a broker test was flaky. Live: a raw client got snapshot `seq: 1`, and a volume change pushed `seq: 2`. `shadow` (event): 34 comparisons, 0 divergences. The glyph bar, on `ws`, showed 2 segments at 20 percent and 9 at 90 percent after an event delivered by the new door.

**All events through the contract (former 10.29).** Events moved from direct `AppHandle::emit` to `unidtk_api::emit`: the kernel's `explorer-open`, `display-changed`, and `hotkey-pressed`; the Telegram events, through one helper, `TelegramRuntime::emit`; and the player's `audio-bands`, `ai-dj-progress`, `sync-tasks`, and `sync-auto`. The player lost its `AppHandle` parameters (about fifteen of them), and `cargo check` gave zero warnings. The WS writer merges state frames to the newest one per pass. Live: `explorer-open` took the tree from 29 to 39 nodes, with the path visible. A module event, `telegram-connection-state`, arrived with `{"state":"Ready"}` and a real `seq`. The events that play only during playback were verified through the common path, not by observation. `cargo check --release --bins`: 0 warnings.

**Migration complete (former 10.30).** The interface works over the socket: 13 commands and 17 events went through the new door at that time. The remaining Tauri-only calls were `get_folders`, `scan_directory`, `explorer_tabs_get`, `frontend_log`, `telegram_get_config`, and `telegram_debug`. The default stayed `tauri`. Shadow comparison: 65 checks, 0 divergences. Two protocol improvements came out of it: the catch-up frame at subscription is marked `"snapshot": true`, and the comparison is by delivery, with a ring of the last 8 live payloads per event, because the module sends events in batches.

Side findings: a cold build from scratch was impossible (the placeholder workaround, see 14.3), and the installed app held DLLs from the repository (see 14.3).

**Stage 7 closed (former 10.31).** The dispatcher applied the socket policy to both doors. The fix made the door the dispatcher's context: `dispatch(req, door)`, with `Socket` for TCP and `Ui` for WS. The second door was renamed from `Tauri` to `Ui`, because the name must describe who is asking. The three refusals were given distinct reasons. A new kind of command, `read_only_cli`, was added for the kernel's own introspection. Three manifest defects were fixed: the journal write was declared as `log`, while the kernel registers `frontend_log`; the volume was declared as `volume_get`, while the kernel has `get_system_volume`; and the CLI commands were presented as available to the interface. A live probe found that `frontend_log` over WS had no route, although the Tauri twin existed. The test had checked routes only for the socket subset. New guards: `every_command_has_a_handler`, `every_manifest_ui_command_is_registered`, and three policy-refusal tests. Live: a write over the socket was refused, the same write over WS landed in the journal (`udtk logs --source frontend` showed it), `get_system_volume` over WS answered, `ping` over WS was refused, and the old name `volume_get` gave `unknown cmd`. `cargo test --release --workspace`: 0 failures, 40 tests in the kernel, zero warnings.

A note on the tool: PowerShell 5.1 breaks a multi-line `js:` argument, as described in 9.5.

**The first write from the interface (former 10.32).** A trap was found before the first write. `shadow` calls both doors, so for a write the **action** would run twice. Writes therefore have their own list, `WS_WRITES`. They go to the socket only in `ws` mode, and they are verified by their consequence, with a following read. The pilot was `frontend_log`, whose consequence is a journal record. Live: in `shadow`, one journal entry and no double execution. In `ws`, one entry. The interface had 29 nodes and no errors.

**The first module writes (former 10.33).** `set_system_volume` and `set_system_mute` were declared as writes, and migrated. The recipe that followed for each write has four steps: declare the write in the manifest, add a branch in the module's `ipc_call` with the name from the manifest, add a route in the kernel that calls the module, and add the name to `WS_WRITES` and nowhere else. Live: over the socket, refused. Over WS, `set_system_volume {level:30}` answered `{"level":30,"muted":false}`, and an independent read over the socket confirmed it. The original state was restored through the same door and confirmed by a read. An observation: a non-zero volume clears mute, and that was the same on both doors. `cargo test --release --workspace`: 0 failures, 40 in the kernel, zero warnings.

**VPN writes (former 10.34).** `vpn_connect`, `vpn_disconnect`, `vpn_ping_all`, and `vpn_set_auto_connect` were declared as writes and migrated. `vpn_ping_all` is a write although it answers with measurements, because it takes a global in-progress guard and stores results, so the state it leaves behind is observable. `Access` describes what a command does, not what it replies. Live: all four refused over the socket. `vpn_set_auto_connect {enabled:true}` over WS answered `auto_connect: true`, and a read over the socket confirmed it. The original value was restored. The manifest at that time had 27 commands (20 on the socket) and 7 writes, and the runtime matched it. Lesson: `cargo test --release --workspace -j 2` brought down two LTO builds with `0xc000012d`, so the tests run with `-j 1`.

**Doctor without hard-coding (former 10.35).** The fourth list, in `doctor`, is gone. `-env` iterates the manifests of the modules, with the severity from the manifest. It found `telegram:byedpi` (`ciadpi.exe`, Optional), which the old list had not declared. `-simulate` takes its six probes from the inventory (`SIMULATE_PROBES`), and its render functions from each module's `metadata.json` (`simulate: {module, function, sandbox}`, present in six modules: `clock`, `glyph-bar`, `micrographics`, `mp3-player`, `telegram`, `vpn`). The explorer module has no section, because it renders differently, and the absence of a section means that it takes no part in the probe. A build lint, `lint_simulate_metadata`, checks the entries. Tests: `simulate_targets_come_from_metadata`, `env_section_lists_manifest_dependencies`, and `simulate_probes_name_real_socket_commands`. Verified: `cargo test --workspace -j 1` green (CLI 17 of 17, kernel 41 of 41). `cargo check --bins`: zero warnings. `doctor -env` gave `ALL CHECKS PASSED`. `doctor -unknown` exited with 2. The live `-simulate` with CDP was not run in that session, and the next session with the app up had to confirm the probes.

### H5. Corrections made in the rewrite of 2026-11-02

The rewrite was checked line by line against the code. These are the statements of the previous text that the code does not support, and how the book now states them:

| Previous text | What the code says | Now |
|---|---|---|
| `udtk dev [--debug]` is a command | There is no arm for `dev` in the dispatcher | Listed as removed (1.4) |
| `udtk run [args] [--debug]` | `--debug` is refused with an error | Documented as refused (13.1) |
| `udtk build`: `cargo tauri build` then `cargo build --release --bins`, then copies | One `cargo tauri build --bundles nsis`, then the installer, the hash check, and the restart | Chapter 14 rewritten step by step |
| The PATH and the tasks use the directory of the CLI | `udtk path` and `udtk tasks register` use `C:\Program Files\UniDTK` | Corrected (1.2, 12.1, 12.3) |
| 21 application commands | 37 in the inventory: 12 writes, 5 CLI-only reads, 20 reads on both doors | Corrected (2.5, 7) |
| `help --json` lists 13 CLI commands | The code lists 14 (`doc` and `stop` were added) | Corrected (7) |
| The splash budget is `min(28, height-2)` | `clamp(height-2, 10, 28)` | Corrected (13.5) |
| The splash tiers have fixed widths 98, 86, 78, 46, 29 | The widths are computed from the art's geometry, and the height from the budget | Described as computed (13.5) |
| The handshake deadline is 3 seconds, and the client is also 3 seconds | Server: 3 seconds. Client: 8 seconds for the handshake and 15 seconds per command | Both stated (4.3, 4.6) |
| The `doctor -simulate` steps include a live click on the clock widget, and a marker in the log | The live click was removed (sandbox copy). The code shows no log marker | Corrected (11.6). The marker is not described |
| `lint_modules_integrity` fails on a module dependency on the kernel crate | The function checks the registry and `Cargo.toml` membership only. `lint_module_contracts` checks UI metadata | Corrected (15.4, 18.5a) |
| Task registration runs through elevation (`Start-Process ... -Verb RunAs`) | It first runs `schtasks /create` directly, and elevates only on failure | Corrected (12.1) |
| `udtk gui js:` goes through the page's own door | It goes through CDP `Runtime.evaluate` | Corrected (3.4) |
| `doctor -env` validates PE | It checks presence only | Corrected (11.2) |
| The live checklist uses `cargo build --release --bins` | The rule of AGENTS.md is `udtk build` only | Replaced (Chapter 17) |
| `make_splash.py --repo <repo> --check` is in the repository | The generator is not in the repository | Flagged (13.5, 18.9) |
| The build runs with `-j 1` | `.cargo/config.toml` sets `jobs = 2` | Both stated (14, 14.3) |

