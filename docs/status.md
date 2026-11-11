# UniDTK status

This is the first file an agent reads. It says what is true today and where to read next. What happened on a given day belongs to the [session log](sessions.md), and the rules of the system belong to the [reference](reference/README.md).

## Current state

- **Focus:** a kernel that knows no module ([plan](dev/plan-kernel-cleanup.md)) and the native desktop host on Rune source code ([plan](dev/plan-rune-host.md)), on the branch `rune-transition`.
- **Production:** `C:\Program Files\UniDTK` (Tauri v2 + WebView2), built by `udtk build`.

| Subsystem / Metric | Value / Status | Technology stack | Primary document |
| :--- | :---: | :--- | :--- |
| **Active branch** | `main`; the kernel cleanup and the Rune host live on `rune-transition` | Git | [Rune host plan](dev/plan-rune-host.md) |
| **Design tokens / themes** | 🟢 Two built-in presets: Classic (flat) + Sea Glass (jelly), schema v4: colors, fonts, geometry, material, palette + tintMap, optics | CSS custom properties, `data-material`, `unidtk-theme-changed`, OKLCH ink, pre-paint from the `paint` cache | [desk.md](reference/desk.md), session 125 |
| **Core (kernel)** | 🟢 Stable | Headless Rust, `unidtk-api`, TCP IPC `127.0.0.1:26540` | [kernel.md](reference/kernel.md) |
| **Storage (DB)** | 🟢 Stable | Clean `redb` transactions (`unidtk.redb`, `unidtk_logs.redb`) | [storage.md](reference/storage.md) |
| **CLI bridge (`udtk`)** | 🟢 Stable | Rust CLI, CDP WebSocket, TCP IPC; `doctor -simulate` measures a motion budget (real click/Escape, frame and task ≤ 50 ms) | [udtk book](udtk/ch11-doctor-the-diagnostic-loop.md), session 136 |
| **Production GUI** | 🟢 Stable | Tauri v2, WebView2 (Working Set of the process tree: 571 MB after a test run, session 136) | [the page](book/ch06-the-page.md) |
| **Motion** | 🟡 Telegram over budget (#536); surfaces grow out of their object (container morph, session 137) | Worst frame on a quiet machine (session 137): menu/folder/clock 17–34 ms; Telegram open 88–450 ms (task 91–438 ms), over the 50 ms budget. Folders, the clock and the Telegram dock avatars go through one engine ([shared-flight.js](../core/ui/shared-flight.js)); the explorer morphs, the menu slide and popups do not yet (#535). Earlier (session 136): Telegram open click 68 → 25 ms. A history change during the open patches only the changed rows (session 136 follow-up; fixture-proven, not yet seen live). Open: `doctor -simulate` budget is flaky under machine load, Telegram long tasks of 60–137 ms in some runs are unexplained | [surfaces](reference/surfaces.md), sessions 136, 137 |
| **Module: Clock** | 🟢 Stable | WebView2, Nothing Ndot 55 typography, on-demand timer (1 Hz) | [clock](modules/clock.md) |
| **Module: Glyph Bar** | 🟢 Stable | 10 LED segments, WASAPI master volume | [glyph-bar](modules/glyph-bar.md) |
| **Build / Release** | 🟢 `udtk build` ~11.5 min | cargo thin-LTO `jobs=3`, NSIS `zlib`, installer **126 MB** (models → AppData) | [build](udtk/ch14-build-step-by-step.md), session 116 |
| **Module: MP3 Player** | 🟢 Stable | libmpv FFI raw-dylib, WASAPI loopback, AI DJ, arcs; model weights in `%LOCALAPPDATA%\UniDTK\models` (downloaded, session 116; default source: the published ONNX export on Hugging Face, pinned to a commit, session 134) | [mp3-player](modules/mp3-player.md) |
| **Module: Micrographics** | 🟢 Stable | WinRT geolocation, adaptive precipitation/weather grid; place name via BDC→Nominatim chain (session 116, `b08fbae`) | [micrographics](modules/micrographics.md) |
| **Module: Telegram** | 🟢 Stable | TDLib C FFI, ByeDPI socks5, reactions, forum threads; pinned messages in groups — 15 s budget + cooldown (session 115, `5ab1e7a`) | [telegram](modules/telegram.md) |
| **Module: VPN** | 🟢 Stable | Sing-box TUN, native `vpn-helper.exe` (GUI subsystem) | [vpn](modules/vpn.md) |
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
│   ├── kernel/                 # unidtk: microkernel, IPC (26540), tray, redb stores, WorkerW attachment
│   └── ui/                     # WebView2 frontend: menu.js, surface-manager.js, motion.js, styles.css
│
├── modules/                    # Vertical feature slices (can be turned off)
│   ├── clock/                  # Clock: ui/ (WebView2) + tests/
│   ├── glyph-bar/              # Nothing OS LED volume indicator on WASAPI
│   ├── mp3-player/             # Player: rust/ (libmpv FFI) + ui/ (AI DJ graph, arcs)
│   ├── micrographics/          # Ambient infographics: rust/ (WinRT geo) + ui/ (grid)
│   ├── telegram/               # Messenger: rust/ (TDLib FFI) + ui/ (chats, threads)
│   ├── vpn/                    # VPN: rust/ + helper/ (vpn-helper.exe) + ui/
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
