# IPC and the Two Doors

The kernel can be asked to do things through two doors. This chapter states what each door may do. The `udtk` side of the protocol, step by step, is in [the udtk book](../udtk/README.md).

r[ipc.one-contract]

There is one command contract with two doors. Every command is declared once: a kernel command in `core/kernel/command_inventory.rs`, a module command in its module's table ([modules.command-table](modules.md#r-modules.command-table)). Both `unidtk.exe` and `udtk.exe` compile both, so the names served and the names the CLI knows cannot drift apart. Every door checks a command against the same spec, and a module command takes the same path, `serve_module` in `core/kernel/ipc.rs`, through every door.

- **The interface door** is how the page in the window talks to the kernel: Tauri `invoke`, or an authenticated WebSocket.
- **The socket door** is TCP on `127.0.0.1:26540`, used by `udtk`.

r[ipc.socket-read-only]

The socket door is read-only by design. A command declared as `write` (writing the log, changing the volume, any VPN, Telegram, or explorer operation) is refused there with the reason "changes state, and write commands stay on the UI door". That refusal is correct behavior, not a fault. From the CLI, writes are tested indirectly: through `udtk gui` on the page, or by their effects in `udtk logs`.

The reason is who can reach each door. Any local process can open a TCP socket on localhost; only the page inside the application holds the interface door. Keeping writes behind the interface door means a stray local process can look, but not act.

r[ipc.long-commands-off-thread]

A command that can take seconds (network, a child process) is declared `long` in its table (`write(long) "vpn_ping_all" …`, `CommandSpec::long`), and both manifests publish the flag. The WebSocket door runs an ordinary command inline, in the order it arrived, and a `long` one on a worker thread: its reply comes back later, carrying its request id, while the commands behind it are answered at once. Only the connection's thread writes the socket. One connection runs at most four long commands, and one of each name at a time; another is refused with `busy`. The host gives a long command its own deadline (`Outbox::request_within`, `Act::call_within`) instead of the default five seconds, and a request that timed out before it left is taken out of the outbox.

The reason: one 22-second server check on the host's single connection held every command behind it, the host's five-second deadline turned them into errors, and the desk drew no folders until the menu was opened again (session 151).

r[ipc.cdp-always-on]

The WebView2 DevTools protocol (CDP) is always enabled in the shipped application, and its port is published in the reply to the socket's `ping`, never guessed. `udtk gui`, `udtk shot`, and `udtk cdp-list` use it.

r[ipc.host-inspector]

The native desktop host (`unidtk-desktop`, [the Rune host plan](../dev/plan-rune-host.md)) runs an internal inspector listener speaking newline-terminated JSON (`{"v":1, "cmd", "args"}` -> `{"v":1, "ok", "result" | "error"}`) defined in `core/desktop/protocol.rs`. At most four normal workers serve requests (16 KiB request cap, 3 s read deadline), plus one reserved priority slot that serves only `ping` and `quit` (500 ms read deadline); excess requests receive `busy`. Replies have a write timeout and the CLI bounds them to 12 MiB.

- **Supervised vs standalone binding.** Under `--supervised`, `unidtk-desktop.exe` binds an ephemeral loopback port (`127.0.0.1:0`), writes per-PID protected metadata `%LOCALAPPDATA%\UniDTK\host-inspector.<pid>.json` (atomic temp+replace with an explicit owner-SID + `SY` DACL, never `OW`), and registers `{pid, generation, instanceNonce, inspectorPort, attached, ready}` with `unidtk.exe` over the authenticated WebSocket door (`host_register`). When launched standalone (`udtk host run`), it binds `127.0.0.1:26541` with ephemeral fallback on `AddrInUse` and also writes `%LOCALAPPDATA%\UniDTK\host-standalone.json`. A named mutex (`Local\UniDTK.DesktopHost`) enforces one host instance with `WAIT_ABANDONED` recovery.
- **Open reads and GPU PNG capture.** Open inspector reads are `ping`, `tree` (rectangles in window points), `frames` (bounded CPU frame samples, process CPU/memory, and Rune `SurfaceCounts`), `hit`, and `png` (base64 PNG bytes, at most 8 MiB and 16 MP enforced by `readback_staging_layout` before GPU allocation). `png` renders an offscreen GPU readback (`capture: "wgpu-readback"`) with true per-pixel alpha and refuses a host-side path: only the calling CLI writes image files. The kernel's `ping` publishes `"host": { "mode", "pid", "generation", "inspectorPort", "attached", "ready" }`, and lock-free read-only CLI proxies (`host_ping`, `host_tree`, `host_frames`, `host_hit`) are declared in `command_inventory.rs` on the socket door (`Door::Socket`). `ready`, `tree`, and `click` gate on a committed Rune element tree (`committed > 0`, including cold covered boots) and a non-empty hit region rather than waiting for an uncovered swapchain present.
- **Writes and input forwarding.** Inspector writes (`click`, `quit`, and `desk`, which forwards one `desk_position_set` to the kernel through the host's own link) require the current run's token from the matching metadata file. Input is serialized; clicks reject an uncovered-region miss, refuse covered screen targets unless `--force` is passed, wait for the Rune handler acknowledgement plus a short post-input commit barrier (`tree_seq`), and return the updated count. In supervised native mode, text focus changes report over WebSocket (`set_desktop_input_focus`), and the kernel's keyboard hook forwards lossless `InjectKey` and `FocusLost` messages over the supervised stdin control pipe. `udtk host` and `--host` always target the native host without falling back to CDP (reporting the active `"web"` host mode clearly when unlaunched); in `"native"` host mode, `udtk shot`, `udtk doctor -ping`, `udtk doctor -simulate`, and `udtk stop` route cleanly to the native host without requiring WebView2.
