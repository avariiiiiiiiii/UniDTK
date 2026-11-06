# UniDTK status

This is the first file an agent reads. It says what is true today and where to read next. What happened on a given day belongs to the [session log](sessions.md), and the rules of the system belong to the [reference](reference/README.md).

## Current state

- **Focus:** migration from WebView2 to the native GPUI host, on the branch `feature/gpui-desktop`. See [the GPUI host](reference/gpui-host.md).
- **GPUI gates:** Gate 0 (transparent window on WorkerW) and Gate 1 (design system, `clock` and `glyph-bar`) are complete.
- **Production:** `C:\Program Files\UniDTK` (Tauri v2 + WebView2), built by `udtk build`.

| Subsystem / Metric | Value / Status | Technology stack | Primary document |
| :--- | :---: | :--- | :--- |
| **Active branch** | `main` (current checkout; GPUI migration work lives on `feature/gpui-desktop`) | Git | [GPUI host](reference/gpui-host.md) |
| **Design tokens / themes** | 🟢 Two built-in presets: Classic (flat) + Sea Glass (jelly), schema v4: colors, fonts, geometry, material, palette + tintMap, optics | CSS custom properties, `data-material`, `unidtk-theme-changed`, OKLCH ink, pre-paint from the `paint` cache | [desk.md](reference/desk.md), session 125 |
| **Core (kernel)** | 🟢 Stable | Headless Rust, `unidtk-api`, TCP IPC `127.0.0.1:26540` | [kernel.md](reference/kernel.md) |
| **Storage (DB)** | 🟢 Stable | Clean `redb` transactions (`unidtk.redb`, `unidtk_logs.redb`) | [storage.md](reference/storage.md) |
| **CLI bridge (`udtk`)** | 🟢 Stable | Rust CLI, CDP WebSocket, TCP IPC; `doctor -simulate` measures a motion budget (real click/Escape, frame and task ≤ 50 ms) | [udtk book](udtk/ch11-doctor-the-diagnostic-loop.md), session 136 |
| **Production GUI** | 🟢 Stable | Tauri v2, WebView2 (Working Set of the process tree: 571 MB after a test run, session 136) | [the page](book/ch06-the-page.md) |
| **Motion** | 🟢 Budget met | Worst frame: menu/folder/clock 17 ms, Telegram 17–33 ms, no long task; Telegram open click 68 → 25 ms. Open: a history refresh during the open still rebuilds the list (80–118 ms) | [surfaces](reference/surfaces.md), session 136 |
| **Experimental GUI** | 🟡 Gate 1 ready | GPUI, DirectX 11, DirectComposition (RAM: **59.7 MB**, CPU: **0.39%**) | [GPUI host](reference/gpui-host.md) |
| **Module: Clock** | 🟢 Ready (GPUI) | GPUI, Nothing Ndot 55 typography, on-demand timer (1 Hz) | [GPUI host](reference/gpui-host.md) |
| **Module: Glyph Bar** | 🟢 Ready (GPUI) | GPUI, 10 LED segments, WASAPI master volume | [GPUI host](reference/gpui-host.md) |
| **Build / Release** | 🟢 `udtk build` ~11.5 min | cargo thin-LTO `jobs=3`, NSIS `zlib`, installer **126 MB** (models → AppData) | [build](udtk/ch14-build-step-by-step.md), session 116 |
| **Module: MP3 Player** | 🔵 GPUI queue | libmpv FFI raw-dylib, WASAPI loopback, AI DJ, arcs; model weights in `%LOCALAPPDATA%\UniDTK\models` (downloaded, session 116; default source: the published ONNX export on Hugging Face, pinned to a commit, session 134) | [mp3-player](modules/mp3-player.md) |
| **Module: Micrographics** | 🔵 GPUI queue | WinRT geolocation, adaptive precipitation/weather grid; place name via BDC→Nominatim chain (session 116, `b08fbae`) | [micrographics](modules/micrographics.md) |
| **Module: Telegram** | 🔵 GPUI queue | TDLib C FFI, ByeDPI socks5, reactions, forum threads; pinned messages in groups — 15 s budget + cooldown (session 115, `5ab1e7a`) | [telegram](modules/telegram.md) |
| **Module: VPN** | 🔵 GPUI queue | Sing-box TUN, native `vpn-helper.exe` (GUI subsystem) | [vpn](modules/vpn.md) |
| **Module: Explorer** | 🔵 GPUI queue | Tabs, Win32 Shell API, cross-view sorting | [explorer](modules/explorer.md) |

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
| Port a widget to GPUI | [the GPUI host](reference/gpui-host.md) |
| Why something was decided, a past failure | [sessions.md](sessions.md), [history/](history/README.md) |

## Codebase map

```
UniDTK Repository
├── core/                       # Core system (invariants that cannot be turned off)
│   ├── api/                    # unidtk-api: stable contract (Module trait, ModuleDb, StoreSpec)
│   ├── kernel/                 # unidtk: microkernel, IPC (26540), tray, redb stores, WorkerW attachment
│   └── ui/                     # WebView2 frontend: menu.js, surface-manager.js, motion.js, styles.css
│
├── hosts/                      # Graphical shell hosts
│   └── gpui/                   # unidtk-gpui: native GPU host on Zed GPUI (DirectX 11 + DirectComposition)
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
