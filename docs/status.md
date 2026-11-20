# UniDTK status

This is the first file an agent reads. It says what is true today and where to read next. What happened on a given day belongs to the [session log](sessions.md), and the rules of the system belong to the [reference](reference/README.md).

## Current state

- **Focus:** the native desktop host on Rune source code ([plan](dev/plan-rune-host.md)), on the branch `rune-transition`. The kernel cleanup ([plan](dev/plan-kernel-cleanup.md)) is done there (session 138). **G0 is closed again** (session 141): the user saw a title-bar strip across the top of the host window, which the `wgpu` readback behind `udtk shot --host` cannot show; the fix (`no_redirection_bitmap`, `core/rune/UPSTREAM.md` item 9) is built, and `PrintWindow` of the shell window (as DWM composes it) shows the card with no strip. Slices 1–5 (`6c4af20`, `46ca3be`, `fcd9058`, `34d3bdb`, `f2de4ea`) shipped child-window occlusion fixes, offscreen `wgpu` PNG readback, zero-wake WS, kernel `HostSupervisor` lifecycle ownership (`desk.host`, stdin WS credentials, implicit-exit fix on `Web -> Native`), keyboard/wheel forwarding, kernel-proxied inspector reads (`host_ping`, `host_tree`, `host_frames`, `host_hit`), and host readiness that follows the committed element tree (382/382 tests green). Measured live on the interactive desktop before the fix (session 141): attached under Progman, `udtk host tree`, `shot --host` and `doctor --host --simulate` pass; over a 5-minute idle window `unidtk-desktop.exe` used `0.005 %` of one core, `0` Rune `timer_wakes`, `67.9 MB` Working Set (`68.7 MB` private), `0` UniDTK WebView2 processes. G1 is under way (session 142, `3227b9b`, 416/416 tests): the host draws the kernel's layout with the stored theme in full (flat and jelly), and the Clock with all thirteen faces and its dashboard; the Glyph Bar is native and accepts wheel and touchpad input (sessions 144–145). The full G1 measurements are pending. G2 is under way (session 143, `a73fde1`, 463/463 tests; a close from a fully open surface no longer waits for a timer wake: 28 frames from 22 ms instead of 3 frames after 760 ms): the kernel's menu, folders, arrange mode and the VPN panel are native; widgets come from the modules' `native/` crates through the widget contract ([shell.md](reference/shell.md)), one morph serves every surface and any press reverses it mid-flight; the VPN settings pane now registers from its native module (session 146). Remaining module-specific rules are listed in [ideas-and-debt.md](dev/ideas-and-debt.md). Production default stays `web`. Win+E opens Explorer and minimises windows; physical check open: three real sign-outs (#326).
- **Production:** `C:\Program Files\UniDTK` (Tauri v2 + WebView2, plus `unidtk-desktop.exe`), built by `udtk build -j 1`.

| Subsystem / Metric | Value / Status | Technology stack | Primary document |
| :--- | :---: | :--- | :--- |
| **Active branch** | `rune-transition` in the `UniDTK-rune` worktree | Git | [Rune host plan](dev/plan-rune-host.md) |
| **Design tokens / themes** | 🟢 Two built-in presets: Classic (flat) + Sea Glass (jelly), schema v4: colors, fonts, geometry, material, palette + tintMap, optics | CSS custom properties, `data-material`, `unidtk-theme-changed`, OKLCH ink, pre-paint from the `paint` cache | [desk.md](reference/desk.md), session 125 |
| **Core (kernel)** | 🟢 Stable; on `rune-transition` module commands come from the modules' tables ([modules.command-table](reference/modules.md#r-modules.command-table)), 377 passed (1 doctest ignored) | Headless Rust, `unidtk-api`, TCP IPC `127.0.0.1:26540`, `unidtk-desktop` inspector | [kernel.md](reference/kernel.md) |
| **Storage (DB)** | 🟢 Stable | Clean `redb` transactions (`unidtk.redb`, `unidtk_logs.redb`) | [storage.md](reference/storage.md) |
| **CLI bridge (`udtk`)** | 🟢 Stable | Rust CLI, CDP WebSocket, TCP IPC; `doctor -simulate` measures a motion budget (real click/Escape, frame and task ≤ 50 ms) | [udtk book](udtk/ch11-doctor-the-diagnostic-loop.md), session 136 |
| **Production GUI** | 🟢 Stable | Tauri v2, WebView2 (Working Set of the process tree: 571 MB after a test run, session 136) | [the page](book/ch06-the-page.md) |
| **Native VPN (G2)** | The VPN module owns its settings pane and the server picker follows the subscription revision. After picker navigation, the host used 109.42 MB Working Set with 4.71 ms frame CPU p95. Kernel IPC RTT was 1 ms (session 148). | Rune and the kernel UI door | [VPN](modules/vpn.md) |
| **Motion** | 🟡 Telegram over budget (#536); surfaces grow out of their object (container morph, session 137) | Worst frame on a quiet machine (session 137): menu/folder/clock 17–34 ms; Telegram open 88–450 ms (task 91–438 ms), over the 50 ms budget. Folders, the clock and the Telegram dock avatars go through one engine ([shared-flight.js](../core/ui/shared-flight.js)); the explorer morphs, the menu slide and popups do not yet (#535). Earlier (session 136): Telegram open click 68 → 25 ms. A history change during the open patches only the changed rows (session 136 follow-up; fixture-proven, not yet seen live). Open: `doctor -simulate` budget is flaky under machine load, Telegram long tasks of 60–137 ms in some runs are unexplained | [surfaces](reference/surfaces.md), sessions 136, 137 |
| **Module: Clock** | 🟢 Stable | WebView2, Nothing Ndot 55 typography, on-demand timer (1 Hz) | [clock](modules/clock.md) |
| **Module: Glyph Bar** | 🟢 Stable | 10 LED segments, WASAPI master volume | [glyph-bar](modules/glyph-bar.md) |
| **Build / Release** | 🟢 `udtk build` ~11.5 min | cargo thin-LTO `jobs=3`, NSIS `zlib`, installer **126 MB** (models → AppData) | [build](udtk/ch14-build-step-by-step.md), session 116 |
| **Module: MP3 Player** | 🟢 Stable | libmpv FFI raw-dylib, WASAPI loopback, AI DJ, arcs; model weights in `%LOCALAPPDATA%\UniDTK\models` (downloaded, session 116; default source: the published ONNX export on Hugging Face, pinned to a commit, session 134) | [mp3-player](modules/mp3-player.md) |
| **Module: Micrographics** | 🟢 Stable | WinRT geolocation, adaptive precipitation/weather grid; place name via BDC→Nominatim chain (session 116, `b08fbae`) | [micrographics](modules/micrographics.md) |
| **Module: Telegram** | 🟢 Stable | TDLib C FFI, ByeDPI socks5, reactions, forum threads; pinned messages in groups — 15 s budget + cooldown (session 115, `5ab1e7a`) | [telegram](modules/telegram.md) |
| **Module: VPN** | 🟢 Settings writes and cache invalidation share the subscription revision. Stale picker requests are rejected before starting a connection (sessions 147–148). | Sing-box TUN, native `vpn-helper.exe` (GUI subsystem) | [vpn](modules/vpn.md) |
| **Module: Explorer** | 🟢 Stable | Tabs, Win32 Shell API, cross-view sorting | [explorer](modules/explorer.md) |

## Where to read

Load only what the task needs. The whole book is listed in [SUMMARY.md](SUMMARY.md).

| Your task | Read |
| :--- | :--- |
| Any task | this file, then [AGENTS.md](../AGENTS.md) |
| Change a contract of the kernel, a module, or storage | the matching file in [reference/](reference/README.md) |
| Learn how the system works, from the start | [the book](book/ch00-introduction.md) |
| Use or extend the CLI | [the `udtk` book](udtk/README.md) |
| Work inside a module | [modules/](modules/README.md) |
| Build, test, internals of logging and storage | [dev/](dev/README.md) |
| Port a widget to the native host | [the Rune host plan](dev/plan-rune-host.md) |
| Why something was decided, a past failure | [sessions.md](sessions.md), [history/](history/README.md) |

## Codebase map

```
UniDTK Repository
├── core/                       # Core system (invariants that cannot be turned off)
│   ├── api/                    # unidtk-api: stable contract (Module trait, ModuleDb, StoreSpec)
│   ├── desktop/                # unidtk-shell + unidtk-desktop: native Rune desk, widget contract, inspector (26541)
│   ├── kernel/                 # unidtk: microkernel, IPC (26540), tray, redb stores, WorkerW attachment
│   ├── rune/                   # Local Rune engine source tree (workspace crates)
│   └── ui/                     # WebView2 frontend: menu.js, surface-manager.js, motion.js, styles.css
│
├── modules/                    # Vertical feature slices (can be turned off)
│   ├── clock/                  # Clock: ui/ (WebView2) + native/ (Rune desk) + tests/
│   ├── glyph-bar/              # Nothing OS LED volume indicator on WASAPI
│   ├── mp3-player/             # Player: rust/ (libmpv FFI) + ui/ (AI DJ graph, arcs)
│   ├── micrographics/          # Ambient infographics: rust/ (WinRT geo) + ui/ (grid)
│   ├── telegram/               # Messenger: rust/ (TDLib FFI) + ui/ (chats, threads)
│   ├── vpn/                    # VPN: rust/ + helper/ (vpn-helper.exe) + ui/ + native/
│   └── explorer/               # Explorer: ui/ (tabs, navigation)
│
├── opt/                        # Auxiliary native utilities
│   └── cli/                    # udtk.exe: the canonical developer CLI tool
│
├── toolchain/                  # Build infrastructure
│   ├── Cargo.toml              # Single Cargo manifest for release builds
│   └── windows/hooks.nsh       # NSIS installer hooks for Program Files
│
└── scheduler-tasks/            # Task Scheduler XML templates (UniDTK, Singbox, VpnStop)
```

## Updating the documentation

1. Links are relative Markdown links. `udtk doc --check` must pass.
2. Every new page is listed in [SUMMARY.md](SUMMARY.md).
3. Each session is recorded in [sessions.md](sessions.md) with a date and a commit SHA.
4. This file changes when a gate, a metric, or the document map changes.
5. A file in [reference/](reference/README.md) changes only when a contract changes. A rule's text lives there and nowhere else.

The writing standard is in [dev/doc-style.md](dev/doc-style.md).
