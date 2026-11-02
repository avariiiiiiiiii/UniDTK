---
tags:
  - architecture
  - kanban
  - shell
created: 2026-08-27
---

# 🗂️ Shell Architecture Kanban

> **For whom:** a human (you). For agents — a separate `docs/architecture.md` + `docs/tasks.md` (to be filled in later).
> **Format:** Obsidian + Kanban plugin.
> **Board status:** a living document — updated as decisions are made.

## 📌 Legend

| Status | Meaning |
|---|---|
| ✅ Done | done, merged |
| 🟡 In Progress | being worked on now |
| ⬜ Backlog | planned, taken in priority order |
| 🔍 Research | under investigation, no decision yet |
| 🚫 Blocked | waiting on an external condition (Obsidian, a decision, a dependency) |

---

## 🧱 1. Core

> What always stays: without this, the shell stops being a shell.
> Maintained by: **you / PR contributors** (not user modules).

- [ ] ⬜ **Shell frame** — windows, overlays, widget layout, module loader
- [ ] ✅ **Unified DB** — `ShellDatabase` (redb): `WIDGET_*`, `EXPLORER_*`, `TELEGRAM_*`, `VPN_*`, `TRACKS`, `AI_*` (already exists)
- [ ] 🟡 **Command registry + event bus** — single facade: `invoke` / `events` / `db`
- [ ] ⬜ **Stable API for modules** — contract: commands, events, DB, permissions (grants)
- [ ] ⬜ **System services as a platform** — FS, Telegram, VPN, music, wallpaper, volume/audio/input
- [ ] ⬜ **Module manager** — manifests, enable/disable, hot-reload, sandbox
- [ ] ⬜ **CLI** — `shell build|dev|run|mods|theme|ai|doctor` (replaces VBS, bridge for AI and modules)

## 🧩 2. Modules

> Added by any user. **Rust or Lua** — whichever the user prefers.
> Maintained by: users (via the menu/AI/CLI). The core does not know about them — only the API.

- [ ] ⬜ **Module format** — folder `mods/<id>/` + `mod.toml` (id, version, permissions, triggers) + code
- [ ] ⬜ **Lua modules** — fast backend with no rebuild/restart, hot-reload, works in a release build
- [ ] ⬜ **Rust modules** — full backend, with automatic rebuilding
- [ ] ⬜ **JS widgets** — the module's frontend (existing pattern `widgets/<id>/`)
- [ ] ⬜ **Mini-scripts** — automation without a widget (optional)
- [ ] 🔍 **What we move out of the core into modules** — AI DJ, yt-dlp/ytm, scheduler-tasks, hashing/archive/registry, Telegram features (criterion: "if it's turned off, the shell keeps working")

## 🌉 3. Bridges

> How the parts talk to each other.

- [ ] ✅ **Tauri bridge** — core ↔ frontend: `invoke` / `listen` / `emit` / unified redb (exists)
- [ ] ⬜ **Module bridge** — module ↔ core: same pattern `invoke`/`events`/`db`, but with grants
- [ ] ⬜ **CLI bridge** — CLI ↔ core: same contract (UI/CLI/AI/Lua — all clients of one API)
- [ ] ⬜ **AI bridge (MCP)** — built-in AI reads/writes modules, configs, styles, tokens via CLI/API
- [ ] 🔍 **Binaries as services** — xray, mpv/libmpv, yt-dlp: bundled into the installer, managed as service processes (not "libraries" but managed processes)

## 🔌 4. API (Contracts)

> The stable layer modules depend on. Changes rarely, versioned.

- [ ] ⬜ **Commands** — `invoke(cmd, args)`, allowlist by grants
- [ ] ⬜ **Events** — `events.subscribe(type, fn)` / `events.emit(type, payload)`
- [ ] ⬜ **State** — `db.get/set` in redb, module namespace `mods/<id>/...`
- [ ] ⬜ **UI contract** — the module ships its own JS widget and registers commands as `mod_<id>_<cmd>` in the shared registry
- [ ] ⬜ **API versioning** — v1, compatibility across core updates

## ⚙️ 5. Tools

> External dependencies, management, installation.

- [ ] 🔍 **mpv → libmpv** — decision: take libmpv (LGPL), not mpv.exe (GPL); requires rewriting the music backend
- [ ] 🔍 **yt-dlp** — stays a binary (Python), bundled into the installer (Unlicense — free)
- [ ] 🔍 **xray → sing-box (one process)** — fold the xray proxy role into the already used sing-box (vless/trojan outbound), remove xray entirely (agent: Medium, vpn.rs ~200-350 lines)
- [ ] 🔍 **sing-box as a DLL (gomobile/C-ABI)** — research: compile the Go core into sing-box.dll, check on Windows (risky, mobile toolchain)
- [ ] 🔍 **Go → Rust compiler** — research: no production tool exists (rustgo is a toy); realistically: C-ABI / WASM / rewrite
- [ ] 🔍 **Python as a library** — research: RustPython (interpreter written in Rust, embeddable) vs pyo3 (embedding CPython, heavy ~30-50MB)
- [ ] 🔍 **WASM as a universal module format** — Rust modules / Go (wazero) / everything in a WASM runtime (wasmtime), without subprocesses
- [ ] 🔍 **Shell bootstrap** — research: self-compilation (Cargo inside = 1-2GB toolchain, heavy), WebView can't be "eaten" (a system component), sweet spot = WASM
- [ ] ⬜ **Installer = package installer** — MSI/NSIS carries all binaries inside (one package, everything included)
- [ ] ⬜ **`shell doctor`** — checks presence/versions of binaries, delivery from release assets

## 🎨 6. Styles and Animations (Theme)

> A unified base, user-configurable (sliders), settings menu.

- [ ] ✅ **Explorer animations** — fixed (canon-compliant, merged)
- [ ] ⬜ **Theme tokens** — single object/variables (`--dur-*`, `--ease-*`, `--r-*`) instead of hardcoded values
- [ ] ⬜ **Theme source** — table in redb (e.g. `THEME`) or a config; JS picks it up at startup
- [ ] ⬜ **Unified animation base** — all animations via tokens; sliders for the user
- [ ] ⬜ **Settings menu** — replaces the move button: widget placement, adding/creating widgets, styles, animations
- [ ] ⬜ **prefers-reduced-motion** — project-wide gap (mandatory for open source)

## 🤖 7. Built-in AI (AI)

> An AI, shaped by a system prompt, that edits the shell for the user.

- [ ] ⬜ **AI module editor** — creates/edits Lua/JS modules (option A: only modules/configs/styles, NOT the core)
- [ ] ⬜ **AI settings editor** — changes styles, animations, tokens via API/CLI
- [ ] ⬜ **AI bridge (MCP contract)** — tools: read, write, invoke, list mods/theme
- [ ] 🚫 **AI edits the core** — NO in the first iteration (breaks updates); the core changes only via PR

## 🧹 8. Technical Debt (Tech Debt)

> Known problems, non-critical, but we keep pushing them down.

- [ ] ⬜ `telegramRuntime.activeTopicThreadId` — dead fallback (assigned to a separate agent)
- [ ] ⬜ `wireTelegramListSection` — dead `data-topic-id` branch (assigned to a separate agent)
- [ ] ⬜ `loadForumTopics` — dead `hasBackend` flag (assigned to a separate agent)
- [ ] 🟡 **renderer.js split** (7763 lines) — widget code goes to `widgets/<id>/widget.js`, the core stays (agent in progress, branch `fix/renderer-split`)
- [ ] ⬜ `.gitignore`/hygiene — `Cargo.lock`, `Server - SSH.cmd` (uncommitted artifacts)

---

## 🗺️ Architecture Diagram (ASCII)

```
┌─────────────────────────────────────────────────────────────┐
│                        SHELL (core)                          │
│  ┌───────────────┬────────────────┬──────────────────────┐  │
│  │  Window frame  │  Command list   │  Single DB (redb)    │  │
│  │  win/overlays  │  + event bus     │  WIDGET/EXPLORER/   │  │
│  │  layout/mods   │  (invoke/events) │  TELEGRAM/VPN/...   │  │
│  └───────┬────────┴────────┬────────┴──────────┬──────────┘  │
│          │                 │                   │             │
│  ┌───────▼────────┐  ┌─────▼──────┐  ┌─────────▼──────────┐  │
│  │ Tauri door     │  │ CLI door   │  │ AI door (MCP)      │  │
│  │ invoke/listen  │  │ build/dev/ │  │ mods/styles/       │  │
│  │ emit + redb    │  │ mods/theme/│  │ settings           │  │
│  │                │  │ ai/doctor  │  │                    │  │
│  └───────┬────────┘  └─────┬──────┘  └─────────┬──────────┘  │
│          │                 │                   │             │
│  ┌───────▼─────────────────▼───────────────────▼──────────┐  │
│  │           MODULE API (stable contract)                   │  │
│  │   invoke(cmd) · events.subscribe · db.get/set · ui      │  │
│  │   + grants (permission allowlist from mod.toml)         │  │
│  └───────┬─────────────────┬───────────────────┬──────────┘  │
│          │                 │                   │             │
│  ┌───────▼──────┐  ┌───────▼──────┐  ┌────────▼───────────┐  │
│  │ Lua modules    │  │ Rust modules  │  │ JS widgets          │  │
│  │ (no build,    │  │ (auto build) │  │ (frontend/widgets/  │  │
│  │  hot-reload)  │  │              │  │  <id>/widget.js)    │  │
│  └───────────────┘  └──────────────┘  └────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
   External services (bundled in the installer):
   mpv/libmpv · yt-dlp · xray  — run as managed processes
```

---

## 📌 Board Rules

1. **Core** — only via PR/me. **Modules** — any user (via the API).
2. **A core update must not break modules** — modules depend on the API, not on internals.
3. **A new module = manifest + code (Lua/Rust) + JS widget** — registered through the shared registry (`mod_<id>_<cmd>`).
4. Update statuses with every decision.
