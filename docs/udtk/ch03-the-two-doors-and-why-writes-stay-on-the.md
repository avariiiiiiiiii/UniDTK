# Chapter 3. The two doors, and why writes stay on the UI door

## 3.1 Who is asking

A **door** is a rule about **who is asking**, not about which pipe carries the bytes. The enum `Door` in [core/api/src/commands.rs](../../core/api/src/commands.rs) has two values:

- **`Door::Socket`**: the `udtk` bridge over the local TCP line protocol. This socket is loopback-only and **unauthenticated**. Any process of the same user can connect to it.
- **`Door::Ui`**: the application's own interface. It is authenticated by the token of [Chapter 4](ch04-the-websocket-door.md#chapter-4-the-websocket-door). During the migration its transport changed from Tauri IPC to a WebSocket. The door did not change with it, because the question is who is asking, not what the pipe is made of.

The door is passed into `dispatch(req, door)` by the caller. The TCP path passes `Door::Socket`, and the WebSocket path passes `Door::Ui`. A command therefore behaves the same way on either door, and only the set of allowed commands depends on the door.

## 3.2 The policy, as data

The policy is the `doors` field of each spec, not a rule written into `dispatch`:

| Spec | Served on the socket | Served on the UI door | What a request from the other door receives |
|---|---|---|---|
| Read on both doors | yes | yes | Served |
| `read_only_cli` (for example `ping`, `logs`, `commands`) | yes | no | `served by the tool door only` |
| `write` (for example `vpn_connect`) | no | yes | `changes state, and write commands stay on the UI door` |
| Not in the inventory | no | no | `unknown cmd: <name>` |

`check_door` in [core/kernel/ipc.rs](../../core/kernel/ipc.rs) produces these three refusal messages. It is a free function on purpose: an `IpcServer` cannot be created in a unit test, because it needs a live `AppHandle`, while the policy is pure data and can be tested directly. The tests `a_write_command_is_refused_on_the_socket_and_answered_on_the_ui`, `a_cli_only_read_is_not_offered_to_the_ui`, `a_read_command_is_served_on_both_doors`, and `an_unknown_command_is_refused_on_every_door` cover the three cases.

## 3.3 Why writes stay on the UI door

The socket accepts any local process of the same user, without a password. If it could change state, then any program on the machine could connect a VPN, send a Telegram message, change the volume, or write to the journal and erase evidence. The inventory states this rule in its own comments. The `frontend_log` entry says that a socket that can write log records "lets any local process forge or erase evidence". The VPN entries say that the rule is kept "by rule rather than by accident".

The UI door is different. Its requests come only from the page, and the page proved that it holds the per-run token before any command is parsed. So the interface may change state. The policy is the same for the Tauri and the WebSocket transports, because it is attached to the door, not to the transport.

## 3.4 Consequences for `udtk`

- `udtk invoke vpn_connect` is refused with `command 'vpn_connect' is not available on the socket door: it changes state, and write commands stay on the UI door`, and it exits 1.
- `udtk` cannot write state through the socket. To observe a write from the outside, use `udtk gui js:...` (which runs in the page through CDP) or read back the consequence with a read command. Chapter 9 shows how.
- A read command that the CLI calls internally, such as `ping`, can be served only on the socket. Asking for it on the UI door is refused.
- `udtk gui js:<expr>` does not use the WebSocket door. It evaluates the expression in the page through CDP (Chapter 5), which needs no token.

## 3.5 Retry safety

A request can be repeated, either by a client that reconnects or by a human who presses the button again. The contract records whether repetition is harmless. `Idempotency::Safe` means that asking twice equals asking once. Every read is `Safe`. Every write is `Unsafe`, because it may duplicate an effect. The field is published in both manifests as `retry_safe`, so a client can see it. The test `socket_surface_is_retry_safe` in [core/kernel/command_inventory.rs](../../core/kernel/command_inventory.rs) checks that every socket command is `Safe`.

Honest note: the code records the class, but it does not yet deduplicate writes by request id, and it does not refuse automatic retries of writes. The request-id deduplication cache the plan describes is not in [core/kernel/ipc.rs](../../core/kernel/ipc.rs) or [core/kernel/ws.rs](../../core/kernel/ws.rs) as of this rewrite. See [Chapter 18](ch18-known-gaps-and-open-decisions.md#chapter-18-known-gaps-and-open-decisions).
