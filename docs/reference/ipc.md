# IPC and the Two Doors

The kernel can be asked to do things through two doors. This chapter states what each door may do. The `udtk` side of the protocol, step by step, is in [the udtk book](../udtk/README.md).

r[ipc.one-contract]

There is one command contract with two doors. Every command is declared once: a kernel command in `core/kernel/command_inventory.rs`, a module command in its module's table ([modules.command-table](modules.md#r-modules.command-table)). Both `unidtk.exe` and `udtk.exe` compile both, so the names served and the names the CLI knows cannot drift apart. Every door checks a command against the same spec, and a module command takes the same path, `serve_module` in `core/kernel/ipc.rs`, through every door.

- **The interface door** is how the page in the window talks to the kernel: Tauri `invoke`, or an authenticated WebSocket.
- **The socket door** is TCP on `127.0.0.1:26540`, used by `udtk`.

r[ipc.socket-read-only]

The socket door is read-only by design. A command declared as `write` (writing the log, changing the volume, any VPN, Telegram, or explorer operation) is refused there with the reason "changes state, and write commands stay on the UI door". That refusal is correct behavior, not a fault. From the CLI, writes are tested indirectly: through `udtk gui` on the page, or by their effects in `udtk logs`.

The reason is who can reach each door. Any local process can open a TCP socket on localhost; only the page inside the application holds the interface door. Keeping writes behind the interface door means a stray local process can look, but not act.

r[ipc.cdp-always-on]

The WebView2 DevTools protocol (CDP) is always enabled in the shipped application, and its port is published in the reply to the socket's `ping`, never guessed. `udtk gui`, `udtk shot`, and `udtk cdp-list` use it.

r[ipc.host-inspector]

The experimental native desktop host (`unidtk-desktop`, [the Rune host plan](../dev/plan-rune-host.md)) has a standalone inspector on `127.0.0.1:26541`: one newline-terminated JSON request (`{"cmd", "args"}`) and reply (`{"v":1,"ok", "result" | "error"}`) per connection. The shared protocol is `core/desktop/protocol.rs`. At most four workers serve requests, bounded to 16 KiB and a three-second receive deadline; replies have a write timeout and the CLI bounds them to 12 MiB.

Open reads are `ping`, `tree` (rectangles in window points), `frames` (bounded CPU frame samples, process CPU/memory, and Rune surface counters), `png` (base64 PNG bytes, at most 8 MiB), and `hit`. `png` refuses a path: only the calling CLI saves files. Capture currently uses `PrintWindow(PW_RENDERFULLCONTENT)` with opaque alpha; GPU readback remains a G0 requirement. Captures are serialized and limited to 16 megapixels.

Writes (`click`, `quit`) require the current run's token. The host binds first, creates an OS-random token under `%LOCALAPPDATA%\UniDTK\host-inspector.token` with a protected owner/SYSTEM DACL before writing any secret bytes, and removes only its own token on graceful exit. A duplicate launch fails before touching it. Input is serialized; clicks reject an uncovered-region miss, refuse covered screen targets unless forced, and wait for the G0 card's Rune handler to acknowledge them. A receipt labels forced Win32 posting explicitly. `quit` requests Rune's normal application lifecycle after replying.

Native CLI selection is explicit: `udtk host`, `udtk gui --host`, `udtk shot --host`, and `udtk doctor --host`. Native failure never falls back to CDP. The endpoint is still standalone: kernel-owned child startup, authenticated endpoint discovery and keyboard routing remain open in the plan. The ordinary CLI commands continue to target WebView2.
