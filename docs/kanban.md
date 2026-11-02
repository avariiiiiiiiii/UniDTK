---

kanban-plugin: board

---

## ✅ Done

- [x] **Explorer animations brought to the canon** — merged (febeef4): `both`→`backwards`, duplicate keyframes removed, navigateTo timing fixed, tabs no longer blink, canon easing instead of `0.22`
- [x] **Telegram topics: topic column** — merged (a6f6dc9): the chat list collapses into a rail, topics get their own column, the thread does not shrink, message_thread_id in fetch/send
- [x] **VPN scrollbar** — merged (0f94917): custom scrollbar for the server list + tg-forward picker
- [x] **AI DJ "0 tracks"** — merged (711b588): track_count = library, + analyzed_count
- [x] **mpv doesn't play** — merged (ed88bfc): invalid `--audio-fallback` removed, try_wait after spawn, errors shown in UI, resolve_mpv finds MPV Player
- [x] **Explorer overlay is symmetric** — merged: 260ms spring on both sides, `:not(.visible)` close removed
- [x] **renderer.js split** — merged (1258682): renderer.js = 9 lines (bootstrap), all logic in core.js + widgets/<id>/widget.js; all 9 files pass node --check OK
- [x] **Decision: xray → sing-box (one process)** — agent: fold the proxy role into the already used sing-box, remove xray (Medium, vpn.rs)
- [x] **Logging system (project)** — unified DB (redb) + logs from frontend and backend (in design, see Core)
- [x] **ESC → core** — global Escape handled only in core.js (`collapseAllShellWidgets`); duplicate keydown removed from the telegram module; local keydown handlers on input fields remain
- [x] **libmpv instead of mpv.exe** — in-process player (raw-dylib FFI): real pause/seek/volume/time-pos/EOF; libmpv-2.dll in resources
- [x] **Unified logging system (redb)** — LOGS table, backend+frontend via a channel, rotation, commands log_list/log_clear/log_stats (modeled on noto)
- [x] **AI DJ track counter** — UI re-reads `ai_dj_status` after AI DJ is enabled (it used to stay at the default `0 tracks`); `track_count` = library (637 tracks in qbc-music)

- [x] **Core/modules layout** — backend/core + backend/modules/*, frontend/core + frontend/widgets + frontend/explorer; Cargo paths + tauri resources updated
- [x] **Module IPC bridge** — IpcServer + IpcModule trait; telegram/vpn/volume/explorer ipc.rs; `shell tg/vpn/vol`, `shell doctor`, `shell build|dev|run`
- [x] **Player: EOF auto-advance in the backend** — mpv event loop (MPV_EVENT_END_FILE), end of queue → stop; audio: replaygain=no, gapless=no, volume-max, audio-buffer
- [x] **TG: updateConnectionState** — connection_state in status, event to the frontend, automatic re-proxy when stuck, forced refresh when online
- [x] **TG: account switching** — statusLoaded fix (frontend re-reads after switch); verified via CLI
- [x] **Explorer: tabs in redb** — explorer_tabs_get/set, restore-first, no duplicates on startup
- [x] **CLI bridge** — shell tg/vpn/vol/explorer/player/logs, `shell doctor`, `shell build|dev|run` (replaces VBS)
- [x] **VPN fallback** — if the saved server is not in the subscription, the first one is picked (instead of "server not found")
## 🟡 In Progress

- [ ] **Evaluate sing-box as a DLL (gomobile/C-ABI)** — research: compile the Go core into sing-box.dll, check on Windows

## 🧱 Core

> **Principle: the core = NOT what the user sees.** The core is invisible infrastructure that everything visible runs on: the frame (windows/overlays/layout), the widget system (loading/placement/drag/resize), the command registry + event bus, the unified DB (redb), the service layer (FS/network/audio), the module loader. Test: "if I turn it off, does the shell stop being a shell?"

- [ ] **Command registry + event bus** — single facade: `invoke` / `events` / `db`
- [ ] **Stable API for modules** — contract: commands, events, DB, permissions (grants)
- [ ] **System services as a platform** — FS, Telegram, VPN, music, wallpaper, volume/audio/input
- [ ] **Module manager** — manifests, enable/disable, hot-reload, sandbox
- [ ] **CLI** — `shell build|dev|run|mods|theme|ai|doctor` (replaces VBS, bridge for AI and modules)
- [ ] **Logging into the unified DB** — `LOGS` table in redb; the backend writes (all commands/errors) AND the frontend writes (console.log/errors via invoke); rotation (cap by size/time), export via CLI (`shell logs`); does not block, but is diagnosable
- [ ] **Settings menu** — replaces the move button: widget placement, adding/creating, styles, animations

## 🧩 Modules

> **Principle: everything visible = a module.** Explorer, Telegram, VPN, widgets, glyph bar — all of these are modules the user can turn off. In the future: taskbar and start menu are modules too.

- [ ] **Telegram → multi-messenger** — in the future the widget may be not only Telegram but also WhatsApp, Discord, Slack (one messenger module with backends)
- [ ] **Explorer → module** — the custom explorer can be turned off like Telegram
- [ ] **Glyph bar → module** — volume, can be turned off
- [ ] **VPN → module** — can be turned off
- [ ] **Future: taskbar, start menu, terminal** — as modules
- [ ] **Module format** — folder `mods/<id>/` + `mod.toml` (id, version, permissions, triggers) + code
- [ ] **Lua modules** — fast backend with no rebuild/restart, hot-reload, works in a release build (mlua, vendored Lua 5.4 — embedded in the binary, not a separate process)
- [ ] **Rust modules** — a full backend, but with automatic rebuilding
- [ ] **JS widgets** — the module's frontend (the `widgets/<id>/widget.js` pattern is already in place)
- [ ] **Mini-scripts** — automation without a widget (optional)
- [ ] **Precompiling Lua to bytecode** — "accepted → compiled in the background → loaded like a native one after reload" (bytecode cache)

## 🌉 Bridges

> **The settings menu = a BRIDGE, not a module and not the core.** It cannot be turned off. The menu decides: which modules are enabled, how animations work, which styles apply, how the AI works and which AI is connected. It is the shell's "control panel".

- [x] **Settings menu = bridge** — decides modules/animations/styles/AI; cannot be turned off
- [ ] **Module bridge** — module ↔ core: `invoke`/`events`/`db` with grants
- [ ] **CLI bridge** — CLI ↔ core: the same contract (UI/CLI/AI/Lua — all clients of one API)
- [ ] **AI bridge (MCP)** — built-in AI reads/writes modules, configs, styles, tokens via CLI/API
- [ ] **Managing binaries as services** — libmpv/yt-dlp/sing-box: spawn + health-check + restart, hidden behind the API

## 🔌 API (Contracts)

- [ ] **Commands** — `invoke(cmd, args)`, allowlist by grants; module commands — `mod_<id>_<cmd>` in the shared registry
- [ ] **Events** — `events.subscribe(type, fn)` / `events.emit(type, payload)`
- [ ] **State** — `db.get/set` in redb, module namespace `mods/<id>/...`
- [ ] **UI contract** — the module ships its own JS widget
- [ ] **API versioning** — v1, compatibility across core updates

## ⚙️ Tools

- [ ] **mpv → libmpv** — LGPL DLL, embedded; requires rewriting the music backend
- [ ] **sing-box: fold xray into it** — implementation (vpn.rs, remove ShellXray, one process)
- [ ] **yt-dlp** — stays a binary (Python), bundled (Unlicense — free)
- [ ] **Installer = package installer** — MSI/NSIS carries libmpv + sing-box + yt-dlp inside
- [ ] **`shell doctor`** — checks presence/versions of binaries, delivery from release assets
- [ ] **Python as a library (research)** — RustPython (lightweight, embeddable) vs pyo3 (CPython, ~30-50MB)

## 🎨 Styles and Animations (Theme)

- [ ] **Theme tokens** — a single object/variables (`--dur-*`, `--ease-*`, `--r-*`) instead of hardcoded values
- [ ] **Theme source** — a table in redb (e.g. `THEME`); JS picks it up at startup
- [ ] **Unified animation base** — all animations go through tokens; sliders for the user
- [ ] **prefers-reduced-motion** — project-wide gap (mandatory for open source)

## 🤖 Built-in AI

- [ ] **AI module editor** — creates/edits Lua/JS modules (option A: only modules/configs/styles, NOT the core)
- [ ] **AI settings editor** — changes styles, animations, tokens via API/CLI
- [ ] **AI bridge (MCP contract)** — tools: read, write, invoke, list mods/theme
- [ ] **AI edits the core** — NO (breaks updates); the core changes only via PR

## 💰 Business Model (future)

- [ ] **Turnkey manual setup** — a paid service: configuring the shell for a specific user (modules, styles, animations, AI)
- [ ] **Own AI hub** — analogous to opencode/hermes: an open-source product + its own AI base/subscription; the shell's built-in AI connects to this hub
- [ ] **Module ecosystem** — ability to publish/sell modules (marketplace later)

## 🧹 Tech Debt

- [ ] **Dead Telegram code** — `activeTopicThreadId`, the `data-topic-id` branch, `hasBackend` (assigned to a separate agent)
- [ ] **Repository hygiene** — `Cargo.lock`, `Server - SSH.cmd` (uncommitted artifacts), `.obsidian/` (decide: commit or gitignore)

## 📌 Notes

- **Boundary rule:** core = whatever the shell cannot do without (via PR/me); modules = everything that can be turned off (any user via the API)
- **Lua is not a separate binary:** mlua compiles the interpreter into shell.exe (vendored), zero external dependencies
- **WASM — dropped:** an extra layer; the Lua sandbox + native Rust already solve isolation
- **Bootstrap:** a Cargo toolchain inside = ~2GB, WebView is a system component — can't "eat" it; the sweet spot is the built-in Lua/Rust layer


%% kanban:settings
```
{"kanban-plugin":"board","list-heading-label":"","tag-colors":[],"hide-tags-display":false,"show-relative-date":true}
```
%%
