# Chapter 3. The kernel and its commands

This chapter follows one request through the kernel. A command is a name with a JSON argument, and the kernel answers with a JSON envelope. There are two ways in, the page inside the window and the `udtk` CLI, and they do not get the same rights. After reading it you can declare a new command, say where it is registered and how it is dispatched, explain why a write is refused on the socket, and publish an event that both doors can see.

## Two doors

The kernel has two entrances, and each one has a different caller. The interface door is used by the page, which runs in WebView2 inside the kernel's window. The socket door is used by `udtk`, from any local process. The rule [ipc.one-contract](../reference/ipc.md#r-ipc.one-contract) says that both doors share one command contract, and that each command is declared once.

The rule [ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only) says that the socket door serves only reads. The reason is who can reach each door. The socket is a TCP port on `127.0.0.1:26540` with no authentication, so any program on the machine can connect to it. The page's door is protected by a token that is generated for each run. A command that changes state therefore lives only behind the token.

## Declaring a command

A command is declared as data first. The type is `CommandSpec` in `core/api/src/commands.rs`. It has a name, a one-line summary, a hint for its arguments, an access level, the list of doors that may serve it, and an idempotency flag. The type holds no Tauri, redb, or Win32 code, so the CLI can compile it too.

Filename: core/api/src/commands.rs (abridged)

```rust
pub struct CommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub args: &'static str,
    pub access: Access,              // ReadOnly or Write
    pub doors: &'static [Door],      // Socket, Ui, or both
    pub idempotency: Idempotency,    // Safe or Unsafe
}

impl CommandSpec {
    pub const fn read_only(name, summary, args) -> Self { /* doors: Socket + Ui */ }
    pub const fn read_only_cli(name, summary, args) -> Self { /* doors: Socket only */ }
    pub const fn write(name, summary, args) -> Self { /* doors: Ui only, Unsafe */ }
}
```

The three constructors encode the policy. A `write` is never served on the socket, and it is marked as not safe to retry, so a client that reconnects will not repeat it. A `read_only_cli` is a read that only the CLI needs, such as the `ping` or the log readers, so it does not claim the page's door. The constructor names carry the decision, and the code does not have to repeat it.

Next, the command is listed in one inventory, `core/kernel/command_inventory.rs`. Each command is a `const`, and the constant `ALL` lists them all.

Filename: core/kernel/command_inventory.rs (abridged)

```rust
pub const PING: CommandSpec =
    CommandSpec::read_only_cli("ping", "Liveness probe; also reports the CDP port", "");

pub const SET_SYSTEM_VOLUME: CommandSpec =
    CommandSpec::write("set_system_volume", "Set the master volume", "{level}");

pub static ALL: &[&CommandSpec] = &[
    &PING,
    &SET_SYSTEM_VOLUME,
    // ...
];
```

The file says it is compiled into both binaries. `opt/cli/main.rs` includes it with a `#[path]` attribute, and so does the kernel, which declares it with `mod command_inventory;` in `core/kernel/main.rs`. The CLI therefore knows the same names the kernel serves, and a test in the file fails if a name is declared twice or has no summary.

Note: the file `core/kernel/registry.rs` is included by the CLI in the same way, but it is not the command list. It holds `all_modules()`, the list of module factories, and it is named after the modules, not after commands. Commands are in `command_inventory.rs`.

## Routing a socket request

A command also needs a handler, which is the code that does the work. On the socket door, handlers are listed in a route table in `core/kernel/ipc.rs`. Each entry pairs a name with a function. The name is taken from the inventory constant, so it is written only once.

Filename: core/kernel/ipc.rs (abridged)

```rust
type Handler = fn(&IpcServer, &Args) -> Result<serde_json::Value, String>;

static ROUTES: &[(&str, Handler)] = &[
    (crate::command_inventory::PING.name, handle_ping),
    (crate::command_inventory::LOGS.name, handle_logs),
    // ...
    (crate::command_inventory::VPN_STATUS.name, handle_vpn_status),
];

fn handle_vpn_status(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(server, "vpn", crate::command_inventory::VPN_STATUS.name, args)
}
```

Commands that belong to a module go one step further. The kernel does not know what the VPN does. The handler `handle_vpn_status()` calls `call_module()`, which looks up the module in `MODULE_ROUTES` and calls its `ipc_call` function. That function has the type `ModuleIpcFn` from `core/api/src/commands.rs`. It takes the command name, the arguments, and an opaque host handle, and it returns `None` when the module does not know the command. The module owns its own logic, and the kernel owns the names and the routing. The same division holds for Tauri commands, as [modules.commands-in-core](../reference/modules.md#r-modules.commands-in-core) states: a module exposes plain functions, and the kernel wraps them.

The page's own door works differently in one respect. The commands the page calls through Tauri's `invoke` are functions marked `#[tauri::command]` in `core/kernel/main.rs`, and they are listed in a `generate_handler!` macro inside `main()`. That is a second list, separate from the inventory. The comment in `command_inventory.rs` says it should be folded into the inventory later. Until then, a test in `main.rs` checks that every `invoke("...")` name in the page appears in the macro, so a page call cannot refer to a command that does not exist.

Warning: a command can be declared in the inventory and still have no handler. The route tests in `ipc.rs` (`every_command_has_a_handler`) fail in that case, so a missing handler is caught by `cargo test`, not discovered at runtime.

## Dispatch: the socket door

The socket door is served by `IpcServer` in `core/kernel/ipc.rs`. It listens on `127.0.0.1:26540`, and each connection gets its own thread. The function `handle_conn()` reads one line of JSON, dispatches it, writes one line back, and closes the connection. A limit of eight requests in flight protects the process: a ninth caller receives `busy: too many ipc requests in flight`.

The request is parsed in `route()`, which echoes an optional `id` field back to the caller, and then calls `dispatch()`. `dispatch()` is the same function both doors use. It takes the door as an argument, and its first step is the policy check.

Filename: core/kernel/ipc.rs (abridged, `dispatch`)

```rust
pub fn dispatch(&self, req: &serde_json::Value, door: Door) -> Result<serde_json::Value, String> {
    let cmd = req["cmd"].as_str().unwrap_or("");
    let args = req["args"].as_object().cloned().unwrap_or_default();

    check_door(cmd, door)?;           // inventory: exists, and this door serves it

    match ROUTES.iter().find(|(name, _)| *name == cmd) {
        Some((_, handler)) => handler(self, &args),
        None => Err(format!("command '{cmd}' is declared in the inventory but has no handler")),
    }
}
```

`check_door()` is a free function, so it can be tested without a running application. It looks the name up in the inventory. An unknown name gives `unknown cmd: ...`. A known name that the door does not serve gives a refusal that says why. The refusal for a write on the socket is the message `command 'set_system_volume' is not available on the socket door: it changes state, and write commands stay on the UI door`. That is correct behavior, and the CLI shows it to the user as it is.

The reply is built by `respond()`, which puts the result or the error in an envelope with the protocol version `v`, and echoes the `id` if one was sent. A successful `ping` therefore returns a line like `{"ok":true,"result":{"pong":true,...},"v":1}`.

Warning: the socket is unauthenticated. Read-only commands on it are safe to expose only because they do not change anything. A new command that changes state must be declared with `CommandSpec::write`, which keeps it off the socket by construction.

## Dispatch: the UI door

The page's WebSocket door is in `core/kernel/ws.rs`. It is separate from the socket. It listens on a port that the kernel chooses at start, bound to `127.0.0.1` with port 0, so the operating system picks a free port. The port is reported by `ping` as `wsPort`, and the token that protects it is given only to the page.

A connection must make a handshake before it can do anything. The first frame from the client must be `{"hello":{"token":"..."}}`, and it must arrive within three seconds. A wrong or missing token gets a refusal and the connection is closed. The token comes from `session_info()` in `core/kernel/main.rs`, a Tauri command that only code running inside the application can call. The comment there says the token is handed out in no other place: not over the socket, not in a file, and not on the command line.

Filename: core/kernel/ws.rs (abridged, after the handshake)

```rust
let ready = serde_json::json!({ "v": 1, "ok": true, "result": { "ready": true } }).to_string();
let _ = socket.send(tungstenite::Message::Text(ready));

// ... inside the loop, for each request that is not subscribe/unsubscribe ...
let reply = crate::ipc::respond(
    request.get("id").cloned(),
    server.dispatch(&request, unidtk_api::commands::Door::Ui),
);
let _ = socket.send(tungstenite::Message::Text(reply));
```

Once authenticated, the connection can send command requests, and each one goes to the same `dispatch()` with `Door::Ui`. A write is therefore allowed here and refused on the socket, by the same policy table. The page also uses this connection for subscriptions, which are described in the next section.

Note: the token protects against other local processes and against web pages that try to open the port. It does not protect against code that is already running inside the application, because that code can read the page's token. The file `ws.rs` states that limit in its header comment.

## The event bus

A module reports a change with `unidtk_api::emit(event, payload)`, in `core/api/src/lib.rs`. A module does not call the kernel, and it does not call other modules. It only publishes, and other parts of the system subscribe. The function does three things, in this order.

Filename: core/api/src/lib.rs (abridged, `emit`)

```rust
pub fn emit(event: &str, payload: Value) {
    let _order = EMIT_ORDER.lock()...;
    let handlers = match bus().lock() {
        Ok(mut bus) => {
            bus.last.insert(event.to_string(), payload.clone());   // remember the latest value
            bus.handlers.get(event).cloned().unwrap_or_default()   // who subscribed in-process
        }
        Err(_) => Vec::new(),
    };
    if let Some(emitter) = EMITTER.get() {
        emitter(event, payload.clone());                           // the kernel's transports
    }
    for handler in handlers {
        handler(payload.clone());                                  // run after the lock is released
    }
}
```

The function first stores the latest value of the event, so that a module can ask for the current state with `unidtk_api::state(event)` instead of calling the module that owns it. Then it calls the emitter, which the kernel installed during `setup`. Last, it runs the in-process subscribers, which a module registers with `unidtk_api::subscribe`. Those handlers run on the thread that called `emit`, after the lock is released, so a handler may emit an event of its own.

The kernel's emitter does two things. Its closure, set in the `setup` closure of `core/kernel/main.rs`, sends the event to the page through Tauri's `emit`, and it passes the same event to the event broker.

Filename: core/kernel/main.rs (abridged, in `setup`)

```rust
unidtk_api::set_emitter(move |event, payload| {
    let _ = handle.emit(event, payload.clone());
    events::publish(event, payload);
});
```

The broker is `core/kernel/events.rs`. It exists so that the WebSocket door can carry events too, with numbers. Each published event gets a global sequence number `seq`, and the last 64 frames of each event are kept in a ring. For a state event, listed in `STATE_EVENTS`, such as `system-volume` or `vpn`, the ring keeps only the newest frame, because an older value has no meaning for a client that connects late.

A page subscribes over the socket with a message such as `{"type":"subscribe","event":"vpn","since":41}`. Without `since`, the client receives the newest value of the event, marked as a snapshot. With `since`, it receives the frames after that number. If the ring no longer holds them, the client receives a `gap` message and must resynchronize, instead of a silently incomplete stream.

Warning: delivery to a subscriber is a `try_send` into a bounded channel of 64 frames. When the channel is full, the frame is dropped for that subscriber. This keeps a slow page from blocking the kernel, and it is the reason the `gap` message exists.

## Summary

A command is declared once, as a `CommandSpec` with a door policy, and listed in `command_inventory.rs`. It is routed to a handler in `ipc.rs` on the socket, or to a Tauri function on the page's door, and module commands are forwarded to the module's `ipc_call`. Every socket request passes the door check in `dispatch()` before it reaches a handler, so a write cannot be served there. Events go through `unidtk_api::emit`, which keeps the last value, notifies in-process subscribers, and hands the event to the kernel, which sends it to the page and to the numbered broker. The next chapter covers the modules that register their commands and events in this way: [Chapter 4, Modules](ch04-modules.md).
