# Chapter 2. How a command travels

Two channels connect `udtk` to the running app. Each has its own job, and this chapter follows the first one, the socket, from the first character typed to the printed answer.

## 2.1 The two channels

| Channel | Address | Who opens it | What it carries |
|---|---|---|---|
| Local socket (TCP, one JSON line per request) | `127.0.0.1:26540` | The kernel, in [core/kernel/ipc.rs](../../core/kernel/ipc.rs) (`IPC_PORT`) | Every `udtk` command except the screenshots and the page control |
| CDP (HTTP and WebSocket) | A port the app chooses. Usually `9222`, otherwise a free port. The app publishes it in the `ping` reply as `cdpPort` | WebView2, always enabled in release | `gui`, `shot`, `cdp-list`, `stop`, and the page probes of `doctor -simulate` |

A third channel, the page's own WebSocket door, is used by the interface, not by `udtk`. [Chapter 4](ch04-the-websocket-door.md#chapter-4-the-websocket-door) covers it.

## 2.2 The journey of `udtk invoke vpn_status`

Take the simplest command that reaches a module. Each numbered step below names the function that does it.

1. **The process starts.** `main()` in [opt/cli/main.rs](../../opt/cli/main.rs) collects the arguments. With none, it prints the splash. With `invoke`, it calls `invoke_cmd`.
2. **The request is built.** `invoke_cmd` takes `vpn_status` as the command and `{}` as the default arguments, and calls `ipc(json!({"cmd": ..., "args": ...}))`.
3. **The envelope is numbered.** `ipc()` takes the next value of a process-wide counter (`AtomicU64`, starting at 1). It adds `"v": 1` (the protocol version) and `"id": <n>` to the request. The line is the JSON text followed by `\n`.
4. **The socket opens.** `TcpStream::connect_timeout` connects to `127.0.0.1:26540` with a **500 ms** connect timeout. The read and write timeouts are **2500 ms**. If the connection is refused, the error is `IPC server not reachable at 127.0.0.1:26540: ... (is the app running?)`, and the exit code is 1.
5. **The kernel accepts.** The thread `unidtk-ipc` (started in `IpcServer::start`) accepts connections. Each connection gets its own thread, `handle_conn`.
6. **The in-flight limit is checked.** `InflightGuard::acquire` allows at most **8** requests at a time (`MAX_INFLIGHT`). A ninth one gets the reply `busy: too many ipc requests in flight` and is closed.
7. **One line is read.** `handle_conn` sets a **1500 ms** read timeout on the server side and reads exactly one line. The protocol is one request per connection.
8. **The line is routed.** `route()` parses the JSON. A parse error gives `bad json: ...`. Otherwise the request goes to `dispatch(&req, Door::Socket)`. [Chapter 3](ch03-the-two-doors-and-why-writes-stay-on-the.md#chapter-3-the-two-doors-and-why-writes-stay-on-the-ui-door) explains the door.
9. **The inventory is consulted.** `dispatch` calls `check_door(cmd, door)`. It looks the name up in the inventory (see 2.5). An unknown name gives `unknown cmd: <name>`. A name the door does not serve gives a refusal with a reason.
10. **The route is found.** `dispatch` looks the name up in the static table `ROUTES` (see 2.6). For `vpn_status` the handler is `handle_vpn_status`.
11. **The module is called by its id.** `handle_vpn_status` calls `call_module(server, "vpn", "vpn_status", args)`. `call_module` finds `"vpn"` in `MODULE_ROUTES` and calls `vpn::ipc_call`. The kernel passes the module an opaque handle to the application, typed `&(dyn Any + Send + Sync)`. The module downcasts it to fetch its own state. The kernel never names the module's state type.
12. **The reply is built.** `respond()` wraps the outcome as `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`. It adds `"v": 1`, and it echoes `"id"` when the request had one.
13. **The reply is written.** `handle_conn` writes the reply with a trailing newline and closes the connection.
14. **The client checks the reply.** `ipc()` reads one line and parses it. If the reply carries an `id`, it must equal the request's id. If it carries a `v`, it must equal 1. Otherwise the error is `protocol mismatch: the app speaks vN, this bridge speaks v1 ... update them together`. On `ok: true` the client pretty-prints `result` and exits 0. On `ok: false` it prints the `error` and exits 1.

The envelope is optional on the server side. An older bridge that sends only `{cmd, args}` still works, and the reply simply has no `id` to echo.

## 2.3 Wire format at a glance

```text
client -> app   {"cmd":"vpn_status","args":{},"v":1,"id":3}\n
app -> client   {"ok":true,"result":{...},"v":1,"id":3}\n
```

A failure looks the same, with `"ok":false` and `"error":"..."` in place of `result`. The protocol number is `PROTOCOL = 1` in [core/kernel/ipc.rs](../../core/kernel/ipc.rs). The same number is reported in the `ping` reply as `proto`, so a client can compare its own version with the app's before it trusts an answer.

## 2.4 Transport failures and what they mean

| Symptom | Cause | What to do |
|---|---|---|
| `IPC server not reachable at 127.0.0.1:26540 ... (is the app running?)` | Nothing listens on 26540. The app is not running, or it failed to bind the port | `udtk run`, or `schtasks /run /tn UniDTK` |
| `busy: too many ipc requests in flight` | Eight requests are already being served | Retry. This is back-pressure, not a fault |
| `protocol mismatch: ...` | The CLI and the app are different versions | Update both together (`udtk build` installs both) |
| `protocol: response id N does not match request id M` | A reply belonged to another request | Usually a stale or foreign process on the port; restart the app |
| The command stops responding after 2.5 s | The client read timeout is 2500 ms. The server may still be running the command | Re-check the state before retrying. Commands that change state must not be retried blindly (see 3.5) |

## 2.5 The inventory: one list of what can be asked

The file [core/kernel/command_inventory.rs](../../core/kernel/command_inventory.rs) describes every command the application can be asked. It is compiled **twice**: into the application and into `udtk`, which includes it with `#[path]`. The offline manifest that `udtk help --json` prints and the surface the kernel serves are therefore built from the same data, and they cannot drift apart.

Each command is a `CommandSpec` value, defined in [core/api/src/commands.rs](../../core/api/src/commands.rs). A spec has a `name`, a `summary`, a list of `args`, an `access` (`ReadOnly` or `Write`), a list of `doors` (`Socket`, `Ui`), and an `idempotency` (`Safe` or `Unsafe`). Three constructors set these fields:

| Constructor | Access | Doors | Idempotency | Count in the inventory |
|---|---|---|---|---|
| `read_only(...)` | ReadOnly | Socket and UI | Safe | 20 |
| `read_only_cli(...)` | ReadOnly | Socket only (the tool's own surface) | Safe | 5 |
| `write(...)` | Write | UI only | Unsafe | 12 |

The inventory holds **37** commands (the constant `ALL`, in declaration order). Twenty-five of them are served on the socket, and thirty-two on the UI door. The 12 writes never reach the socket by construction. Each name is written once: the route table takes the name from the constant (`COMMAND.name`), not from a repeated string.

Inventory rules that the tests enforce are listed in [Chapter 15](ch15-how-a-module-joins-the-bridge.md#chapter-15-how-a-module-joins-the-bridge).

## 2.6 The route table and the module routes

A name in the inventory is not enough to serve it. The kernel also needs a handler. There are two tables in [core/kernel/ipc.rs](../../core/kernel/ipc.rs):

- **`ROUTES`**: a static slice of `(name, handler)`, where each handler is a kernel function with the type `fn(&IpcServer, &Args) -> Result<Value, String>`. The kernel owns handlers that need kernel state, such as the log (`handle_logs`, `handle_log_delta`), the widget configuration, and the module-enable switch. Module commands are thin: each handler calls `call_module`.
- **`MODULE_ROUTES`**: a static slice of `(module_id, ipc_call)`. It lists the six modules that answer for their own commands: `glyph-bar`, `explorer`, `vpn`, `telegram`, `mp3-player`, and `micrographics`. The kernel addresses a module by id and never names its state type. A test, `router_does_not_name_module_state_types`, reads the source of `ipc.rs` and fails if a module state type name appears there.

`dispatch` looks the name up in `ROUTES`. If the inventory knows a command but no route exists, the reply is `command '<name>' is declared in the inventory but has no handler`. The route tests exist to make that impossible to ship: `every_command_has_a_handler`, `every_socket_command_has_a_handler`, `every_handler_is_declared_in_the_inventory`, and `route_names_are_unique`. They are tests, not build-time checks, because `generate_handler!` is a macro that `build.rs` cannot read (see [Chapter 15](ch15-how-a-module-joins-the-bridge.md#chapter-15-how-a-module-joins-the-bridge)).
