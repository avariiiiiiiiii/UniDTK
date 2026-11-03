# IPC and the Two Doors

The kernel can be asked to do things through two doors. This chapter states what each door may do. The `udtk` side of the protocol, step by step, is in [the udtk book](../udtk/README.md).

r[ipc.one-contract]

There is one command contract with two doors. Every command is declared once in `core/kernel/command_inventory.rs`, which both `unidtk.exe` and `udtk.exe` compile, so the names served and the names the CLI knows cannot drift apart.

- **The interface door** is how the page in the window talks to the kernel: Tauri `invoke`, or an authenticated WebSocket.
- **The socket door** is TCP on `127.0.0.1:26540`, used by `udtk`.

r[ipc.socket-read-only]

The socket door is read-only by design. A command declared as `write` (writing the log, changing the volume, any VPN, Telegram, or explorer operation) is refused there with the reason "changes state, and write commands stay on the UI door". That refusal is correct behavior, not a fault. From the CLI, writes are tested indirectly: through `udtk gui` on the page, or by their effects in `udtk logs`.

The reason is who can reach each door. Any local process can open a TCP socket on localhost; only the page inside the application holds the interface door. Keeping writes behind the interface door means a stray local process can look, but not act.

r[ipc.cdp-always-on]

The WebView2 DevTools protocol (CDP) is always enabled in the shipped application, and its port is published in the reply to the socket's `ping`, never guessed. `udtk gui`, `udtk shot`, and `udtk cdp-list` use it.
