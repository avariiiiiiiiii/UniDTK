# UniDTK Architecture — agent specification

> **Navigation:** [← UniDTK Master MOC](00_master.md) | [Session log (1–112+)](sessions.md) | [GPUI roadmap](gpui_transition.md) | [CLI bridge](cliudtk.md)

> **Audience:** AI agents (assistants) working on this repository. **For humans:** the Kanban board `docs/kanban.md` (Obsidian). **Status:** living document — update it on every architectural decision. **Repo:** `C:/Users/admin/Documents/GitHub/shell` (Tauri + Rust backend + vanilla JS frontend).

> **⚠ Source of truth.** If this file conflicts with `AGENTS.md` and the actual code, **`AGENTS.md`** (§10 "Project applications", section *shell / unidtk*) and the source code take precedence. The normative sections below (§1–§7) are brought up to the current structure; the session history (§14+) is not rewritten.

> **Documentation principle.** Documentation is written in English and is low level, like the Rust Book: it teaches the mechanism step by step, what happens on the machine and why. The book is README.md and the links in it. `udtk doc` opens it, as `rustup doc` opens the Rust documentation.

---

## 0. Legacy-to-current map

The sections below are already in current form, but the session history and some external notes use old names. This table is a quick reference so that code is not "fixed" to match old docs.

| Legacy term | Current | Source of truth |
|---|---|---|
| `shell.exe` / binary `shell` | `unidtk.exe` + `udtk.exe` + `vpn-helper.exe` | `core/kernel/main.rs`, `opt/cli/main.rs`, `modules/vpn/helper/main.rs` |
| `backend/`, `frontend/` | `core/{api,kernel,ui}` + `modules/<id>/{rust,ui}` | repo root |
| contract `shell-api` | `unidtk-api` | `core/api` |
| crates `shell-mod-<id>` | `unidtk-mod-<id>` | `toolchain/Cargo.toml`, `core/kernel/registry.rs` |
| `shell.redb`, `shell_logs.redb` | `unidtk.redb`, `unidtk_logs.redb` (`%LOCALAPPDATA%\UniDTK`) | `core/kernel/unidtk_db.rs`, `log_store.rs` |
| Paths `frontend/…` | `core/ui/…` and `modules/<id>/ui/…` | `core/ui/*` |
| Lua / mini-script modules | none (Rust modules only) | — |
| `shell build/dev/doctor/…` | `udtk build/doctor/invoke/logs/tasks/shot/gui/…` | `opt/cli/main.rs`, `docs/cliudtk.md` |
| `C:\Program Files\Shell`, `%APPDATA%\Shell` | `C:\Program Files\UniDTK`, `%LOCALAPPDATA%\UniDTK` | `core/api/paths.rs` |
| `expo = cubic-bezier(0.16,1,0.3,1)` | `expo = cubic-bezier(0.30,0.95,0.35,1.00)` (+ `--ease-item-in` = `0.16,1,0.3,1`) | `core/ui/motion.js`, `core/ui/styles.css :root` |
| durations as ranges | exact tokens: hover 120 / transient 220 / motion 300 / layout 340 ms | `core/ui/motion.js` (DURATIONS) |

---

## 1. Architecture principles

### 1.1 The core is NOT what the user sees

The core is **invisible infrastructure** that everything visible runs on. Test for "core or module": **"If I turn this off, does the shell stop being a shell?"**

**Core (invisible, always on):**
- Frame: windows, overlays, layout, lifecycle (including attaching the window to the desktop — `wallpaper`).
- Widget system: loading, placement, drag/resize, add/remove.
- Command registry + event bus (`invoke` / `events` / `state`); physically `core/kernel/registry.rs`, `core/kernel/events.rs`.
- Storage: data in `%LOCALAPPDATA%\UniDTK\unidtk.redb` and logs in `unidtk_logs.redb` (`core/kernel/unidtk_db.rs`, `core/kernel/log_store.rs`).
- Contract `unidtk-api` (`core/api`): `Module` trait, services (`log`/`emit`/`subscribe`/`state`), `ModuleDb`/`StoreSpec`/`StoreFactory`/`ModuleContext`, and the automatically allocated part of the DB (`mods/<id>/…`).
- Service layer as a platform: FS, network.

**Everything visible = a module (can be turned off):**
- Explorer, Telegram, VPN, player (`mp3-player`), widgets, glyph bar.

> **Clarification (session 45).** Storage and logs used to live physically inside the music module, so the kernel pulled that module in only to open its own database — music was de facto core. In session 43 storage moved into the core, so "music is just another module" became true rather than a declaration. Audio capture (for the arcs) is not a core service either; it is part of the player.

**Who edits the core:** only the author / PR contributors. **Who edits modules:** any user (via API, menu, AI, or CLI).

### 1.2 The settings menu is part of the core (not a module)

The settings menu **cannot be turned off** — it is the shell's control panel. The menu decides:
- which modules are enabled or disabled;
- how animations behave (sliders, one shared base);
- which style/theme is active;
- how the AI works and **which AI is connected** (provider choice).

The control layer lives in the **core frontend**: `core/ui/menu.js` + `menu.css`, and `#menuContainer` in `core/ui/index.html` — a right-hand side panel (~420 px) with sections "Desk / Folders / Look" and the VPN view. The menu manages modules through core commands.

### 1.3 Core updates do not break modules

Modules depend on a **stable API**, not on core internals. The built-in AI also uses the same API. Rule: **the AI edits modules, configs, and styles, but NOT the core** (option A) — otherwise updates break everything.

---

## 2. Architecture layers

```
┌─────────────────────────────────────────────────────────────┐
│                        SHELL (core)                         │
│  ┌───────────────┬────────────────┬──────────────────────┐  │
│  │  Frame         │  Command reg.  │  Unified DB (redb)   │  │
│  │  windows/      │  + event bus   │  WIDGET/EXPLORER/    │  │
│  │  overlays/     │  (invoke/      │  TELEGRAM/VPN/LOGS   │  │
│  │  layout/modules│   events)      │                      │  │
│  └───────┬────────┴────────┬───────┴──────────┬───────────┘  │
│          │                 │                  │              │
│  ┌───────▼────────┐  ┌─────▼──────┐  ┌────────▼───────────┐  │
│  │ Tauri bridge   │  │ CLI bridge │  │ AI bridge (MCP)    │  │
│  │ invoke/listen  │  │ build/dev/ │  │ modules/styles/    │  │
│  │ emit + redb    │  │ mods/theme/│  │ settings/logs      │  │
│  │                │  │ ai/doctor  │  │                    │  │
│  └───────┬────────┘  └─────┬──────┘  └────────┬───────────┘  │
│          │                 │                  │              │
│  ┌───────▼─────────────────▼──────────────────▼───────────┐  │
│  │           MODULE API (stable contract)                  │  │
│  │   invoke(cmd) · events.subscribe · db.get/set · ui      │  │
│  │   + grants (permission allowlist from mod.toml)         │  │
│  └───────┬─────────────────┬──────────────────┬───────────┘  │
│          │                 │                  │              │
│  ┌───────▼──────┐  ┌───────▼──────┐  ┌────────▼───────────┐  │
│  │ Lua modules  │  │ Rust modules │  │ JS widgets         │  │
│  │ (no build,   │  │ (auto-build) │  │ (frontend/widgets/ │  │
│  │  hot-reload) │  │              │  │  <id>/widget.js)   │  │
│  └──────────────┘  └──────────────┘  └────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
   External services (bundled into the installer):
   libmpv · sing-box · yt-dlp — run as service processes
```

**Clients of one API (all equal):** UI (frontend), CLI, built-in AI, Lua modules, Rust modules. The core does not know who is calling it.

---

## 3. Modules

### 3.1 Embedding method (current)

| Method | Backend | Frontend | Rebuild | Restart | Works in release |
|---|---|---|---|---|---|
| **Rust module** (main) | Rust crate (`unidtk-mod-<id>`) | JS widget (`ui/`) | yes | yes | yes |
| **UI-only widget** | none (only `clock` today) | JS widget | yes (resources) | yes | yes |

Lua/mini-scripts are gone from the new architecture — they are legacy. Modules combine `rust/` (backend) and `ui/` (frontend) in one vertical slice `modules/<id>/`.

### 3.2 Module format (current)

```
modules/<id>/
  rust/           # crate unidtk-mod-<id> (lib.rs), pulls in only unidtk-api
  ui/             # frontend widget: widget.js + metadata.json + icon.svg
  helper/         # optional second GUI binary (e.g. vpn-helper.exe for vpn)
```

- Built-in modules live in the app resources (bundled into the release). User `mods/` directories are not part of the new contract yet (the override mechanism is legacy from the early architecture).
- A module depends only on `unidtk-api` and talks to the core through the contract (`Module`/`ModuleDb`/events `emit`/`subscribe`/`state`). Commands are registered in `core/kernel/registry.rs` (`create_module()`) and exported to Tauri as thin `#[tauri::command]` wrappers in the core. The integrity gate is `lint_modules_integrity()` in `toolchain/build.rs`.

### 3.3 Lua — **cancelled**

Lua modules (`mlua`) from the early architecture were never implemented and are absent from the current code. Modules are Rust only.

### 3.4 Rust modules (current)

- A full backend with system access. Each module is a separate crate `unidtk-mod-<id>` (a Cargo dependency that pulls in only `unidtk-api`).
- Storage goes through `ModuleContext::store(StoreSpec)` and `ModuleDb` (`mods/<id>/…`, or a separate file `mods/<slug>/<name>.redb` / `cache/<slug>/<name>.redb`); the core owns the paths.
- **We do NOT build dylib/dynamic plugins** (ABI and version problems). Modules are compiled crates.

### 3.5 Existing modules (current)

- Frontend core: `core/ui/unidtk-core.js`, `core/ui/renderer.js` (bootstrap), `core/ui/widgets-api.js`, `core/ui/motion.js`, `core/ui/theme.js`, `core/ui/surface-manager.js`, `core/ui/unidtk-bridge.js`.
- Modules (slices `modules/<id>/{rust,ui}`): `clock` (ui-only), `explorer`, `glyph-bar`, `mp3-player`, `telegram`, `vpn`.

### 3.6 Lifecycle and states

- `Module::suspend` and `Module::resume` (`core/api/src/lib.rs`) switch a module on or off in the running app. By default `suspend` calls `shutdown` with a 1500 ms budget, and `resume` calls `init` with a fresh context. The app keeps running, so these must not set global exit latches.
- `set_module_enabled {id, enabled}` is the only command for this (`apply_module_enabled` in `core/kernel/main.rs`). It switches the module first and only then writes the stored disabled list, so a failure leaves both unchanged. Switches are serialized by `MODULE_SWITCH`. A module that has been off since startup was never built, so it is built from scratch when enabled.
- Each module has one state (`moduleState` in `core/ui/menu.js`): `off`; `background` (the backend keeps running, the card is hidden — only for `backend: true`); `desk` (the card is on the desk). Modules without a backend have only `off` and `desk`.
- metadata.json: `pack {id,label}` groups modules in the menu; `backend: true` marks a module whose backend can run hidden; `settingsView` names the view opened by "Configure…" in the menu.
- `lint_module_contracts()` in `toolchain/build.rs` checks these fields at build time and fails the build on errors.

### 3.7 Nesting (`core/ui/nesting.js`)

- A widget can live inside another widget instead of on the desk. Two metadata.json fields decide it: `container` (the widget hosts others; folders always host) and `embeddable: {module, render}` (the file and export that draw the compact form, which differs from the desk card).
- `canNest(child, host)` is the single gate. It rejects a widget inside itself, a non-container host, a container as child, and a child without `embeddable`.
- One level deep: a container cannot be nested, so cycles cannot exist. There is no limit on the number of children.
- `config.nested` (`{hostId: [childId]}`) is the user's intent and is never trimmed. `effectiveNested(stored, widgets)` computes what applies now. A child whose host is absent (module off, metadata read failed) is left out and shows on the desk until the host returns. Each child is counted once; the first host wins.

---

## 4. API (stable contract)

### 4.1 Commands (RPC)

- `invoke(cmd, args)` — synchronous request/response. A module calls only what its grants allow.
- Module commands: `mod_<id>_<cmd>` in the shared registry.
- **Idempotence:** commands must be safe to repeat (UI, CLI, AI, and Lua call them from different contexts).

### 4.2 Events (pub/sub)

- `events.subscribe(type, fn)` / `events.emit(type, payload)`.
- The core publishes ("track changed", "message arrived", "widget added"); a module subscribes. A module may publish too.

### 4.3 State (data)

- `db.get/set` in redb; module namespace `mods/<id>/...`.
- Unified DB `ShellDatabase` (redb): `WIDGET_POSITIONS`, `WIDGET_SETTINGS`, `EXPLORER_TABS`, `TELEGRAM_*`, `VPN_*`, `TRACKS`, `AI_*`, `LOGS`.

### 4.4 UI contract

- A module ships its JS widget (`widget.js`) and registers commands as `mod_<id>_<cmd>`.

### 4.5 Versioning

- v1, compatible across core updates. Changes rarely.

---

## 5. Bridges

### 5.1 Tauri bridge (exists)

`invoke` / `listen` / `emit` / redb. Core ↔ frontend.

### 5.2 Module bridge

Module ↔ core: `invoke` / `events` / `db`, with grants.

### 5.3 CLI bridge

A separate native binary `udtk` (`opt/cli/main.rs`, conceptually the same API; TCP IPC `127.0.0.1:26540` + native CDP/WebSocket).

**Commands (current):**
- `udtk build [-j N]` — the only release build (stop → `cargo tauri build --bundles nsis` → install into `C:\Program Files\UniDTK` → verify SHA-256 → launch → live IPC).
- `udtk doctor [-kernel|-modules|-ping|-env|-check|-quick|-simulate|-all]` — diagnostics (compilation, DLLs, `.env`, tasks, IPC, safe simulation).
- `udtk invoke <cmd> [json]` — read-only call of core/module commands from the manifest; `udtk logs [--limit|--level|--source]`.
- `udtk tasks status|register|cleanup-old`, `udtk autostart`, `udtk path status|ensure`.
- `udtk run|stop`, `udtk shot`, `udtk gui tree|click|type|press|js`, `udtk cdp-list`, `udtk help --json`.
- `udtk doc` — renders the project book (README.md and the markdown it links to) to HTML in the cache and opens it.

Details: `docs/cliudtk.md`; the full machine-readable manifest: `udtk help --json`.

### 5.4 AI bridge (MCP) — status

Planned (a legacy claim from the early architecture). The current code has no explicit MCP bridge in the modules. The AI edits modules, configs, and styles through the same API/CLI, but NOT the core. Marked "not implemented / future".

### 5.5 Settings menu

See principle 1.2. Contracts:
- **Widget positions** (`config.positions[id]`): `x/y` plus corner anchor `anchor ∈ tl|tr|bl|br` (`ax/ay` offset). Read by `placeOnCanvas` (`unidtk-core.js`); without `anchor` it is `tl`. The corner is written on "Done" in arrange mode (snapshot → "Undo"/Esc restores everything touched).
- **Folder shortcuts** (`FolderItem`: `target`, `args`, `kind`): they reference the real target, not `.lnk` copies. Start menu, icons, and launch — `start_menu.rs` (COM in STA); editing on the desk — `folder-body.js`.
- Irreversible actions run at once; cancel is offered as a 5 s toast (`toast.js`). `alert`/`confirm` are excluded.

---

## 6. Tools (external binaries)

| Component | Now | Decision | Type | License |
|---|---|---|---|---|
| Lua | none | **not used** (no Lua modules in the new architecture; Rust modules only) | — | — |
| mpv | process | **libmpv ✅** (`libmpv-2.dll`, in-process FFI) | DLL | LGPL-2.1+ |
| xray | Go process | **fold into sing-box, remove xray** | binary | MIT |
| yt-dlp | Python process | **bundle** (used in `mp3-player`/sync) | binary | Unlicense |

**Principle:** embed what has a library API (libmpv); bundle what does not (yt-dlp, sing-box). Go/Python runtimes are **hidden inside the binaries** — the user does not install them.

**Delivery and verification:** the NSIS installer built by `udtk build` puts `libmpv-2.dll`, `tdjson.dll`, `sing-box.exe`, and `ciadpi.exe` into `C:\Program Files\UniDTK`; integrity is checked by `udtk doctor`.

### 6.1 sing-box (evaluation details)

- Already used in code (TUN router `wallpaper-tun`, task `ShellSingbox`).
- xray is a separate proxy client (VLESS/Trojan, SOCKS :12908, task `ShellXray`).
- **Decision:** embed the vless/trojan outbound in the sing-box config and remove xray entirely → one process, one task, one config. Effort: Medium (~200–350 lines in vpn.rs).
- **NOT** embedding sing-box as a Rust crate: the core is written in Go, there is no Rust crate, and gomobile/C-ABI on Windows is risky (research item).

---

## 7. Styles and animations (Theme)

- **Theme tokens:** one object/set of variables (`--dur-*`, `--ease-*`, `--r-*`, `--blur-*`, `--theme-*`, `--z-surface-*`) instead of hardcoded values. Source of truth: `core/ui/motion.js` (EASINGS/DURATIONS) and `core/ui/theme.js` (presets/sliders); CSS in `:root` (`core/ui/styles.css`) is only the fallback.
- **Theme:** `core/ui/theme.js` ships 2 built-in presets (Nothing Classic, Sea Glass) plus custom ones, schema v4 (colors, fonts, geometry, material `flat|jelly`, palette + `tintMap`, optics; old records migrate). Attributes `<html data-material|mg-layout|avatar-color>`, zero FOUC (the pre-paint applies the `paint` cache + `save_widget_config` → redb), event `unidtk-theme-changed`. Presets change only appearance: positions (x/y) are user data, sizes are preset defaults, and user edits win (migration in `selectPreset`, reset by a button in the menu). Presets and theme/animation settings are in the menu (`core/ui/menu.js`, "Look" section).
- **Theme contract (modules):** `theme.js` and the menu name no module. A module declares its part in metadata.json: `tints [{id,label,default,role}]` (a tintable surface; `default` is a palette slot; `role` groups targets that share one color when colors are shuffled) and `lookOptions` (module settings that live in the theme). `registerThemeContracts(modules)` reads them; metadata arrives after the theme is live, so new targets are painted at that point.
- **Contrast contract:** a theme declares `onAccentRgb` (the text color on the accent). `readableOn(surfaceHex, preferredRgb)` keeps it while contrast is at least 4.5:1 (WCAG AA); otherwise the better of white and near-black wins. A user-picked accent can therefore never make text unreadable.
- **Per-theme colors:** user colors are kept per theme in `config.colorOverrides` (palette, tintMap, color fields). An old `userColors` record becomes the Sea Glass entry (`loadColorOverrides`).
- **Unified animation base:** all animations use tokens; sliders in the menu (Motion Scale speed, presets, reduced-motion). `motionScale` writes `--dur-*`/`--ease-*` inline on `<html>`; CSS transitions on `var(--dur-*)` inherit the scale. Infinite/loop animations and `linear` are not scaled (spinners, pulses).
- **Canonical easings** (only these, from `motion.js` EASINGS): `standard` `(0.4,0,0.2,1)`, `spring` `(0.34,1.56,0.64,1)`, `expo`/`expoOut` `(0.30,0.95,0.35,1.00)`, `expoOutRev`/`surfaceExit` `(0.38,0.05,0.45,1.00)`, `smooth` `(0.25,0.1,0.25,1)`, `snap` `(0.18,1.04,0.22,1)`, `content` `(0.25,1,0.5,1)`, `itemIn` `(0.16,1,0.3,1)`. **Canonical durations** (only these, `DURATIONS`): hover 120 ms, transient 220 ms, motion 300 ms, layout 340 ms.
- **prefers-reduced-motion** is implemented: `core/ui/motion.js` (`initMotionSystem`, separate `userScale`/`currentScale`) plus a global `@media` rule in `styles.css` (`0.01ms !important`). It overrides the scale without overwriting the user's choice.

---

## 8. Log system (unified DB)

- Table `LOGS` in redb.
- Both the **backend** (all commands/errors) **and the frontend** (console.log/errors via invoke) write logs.
- Rotation: capped by size/time (it must not grow forever). Export via CLI (`udtk logs`).
- Purpose: diagnostics without digging through the console; the AI can read logs through the API.

---

## 9. Built-in AI

- **Option A (first iteration):** the AI edits only modules, configs, styles, tokens, and logs — safe, no rebuild. NOT the core.
- **Option B (later):** the AI may edit the Rust core and run a rebuild — a full developer agent (separate, not now).
- **MCP contract:** the same API as CLI/modules.
- **AI center (business):** an open-source product + its own AI knowledge base (analogue of opencode/hermes).

---

## 10. Business model (future)

1. **Hand-made setup** — a paid service: configuring the shell for a specific user (modules, styles, animations, AI).
2. **Own AI center** — an analogue of opencode/hermes: open source + own AI knowledge base/subscription; the built-in AI connects to it.
3. **Module ecosystem** — a module marketplace (later).

---

## 11. Ideas (future modules)

- **Terminal** — an embedded terminal as a module.
- **Taskbar** — as a module.
- **Start menu** — as a module.
- **Multi-messenger** — the Telegram widget may later support WhatsApp, Discord, Slack too. One messenger module with different backends (the architecture allows it: TDLib → common adapter).

---

## 12. WASM — decision

**WASM is not used.** Reasons:
- an extra layer (a second runtime);
- isolation is already handled by the Lua sandbox + native Rust modules;
- there is no need to run third-party untrusted code — modules are trusted.

---

## 13. Shell bootstrap (partial)

- **Tauri** is already a library the core uses. "Eating" it means using it as the framework.
- **Cargo** can be embedded, but it is expensive (~2 GB toolchain). The sweet spot: do NOT compile inside the app; instead (a) precompile modules ahead of time, (b) use `shell build` for automatic builds.
- **WebView** is a system component (WebView2/Chromium) and cannot be "eaten". It can only be replaced with a custom renderer (wry/tao is the same thing; Servo/WebKit is huge).

---

## 14. What is already done (history, for context)

- **Explorer animations → canon** (febeef4): `both` → `backwards`, duplicate keyframes removed, `navigateTo` timing fixed (rAF + reflow + token), tabs no longer flicker, canonical easing.
- **Telegram topics** (a6f6dc9): topic column (46 px rail + 200 px topics + thread), `message_thread_id` in fetch/send/older, isForum mapping.
- **VPN scrollbar** (0f94917); **Explorer overlay symmetric** (260 ms spring).
- **AI DJ "0 tracks"** (711b588): track_count = library, + analyzed_count.
- **mpv does not play** (ed88bfc): invalid `--audio-fallback` removed, try_wait after spawn, errors shown in UI, resolve_mpv finds MPV Player.
- **CLI bridge** (fa87b16): IPC server 127.0.0.1:26540 + binary `shell` (backend/cli) — player control, tracks, logs, ai_dj; CDP via a node helper for eval/screenshot. DevDebug.vbs enables remote debugging (9222).
- **libmpv embedded** (3bc28ab): the player moved from an external mpv.exe to in-process libmpv (raw-dylib FFI, no import lib). Real pause/seek/volume/time-pos/EOF. `libmpv-2.dll` bundled into resources. libmpv.rs tests (2).
- **Unified logging** (f59188b): table LOGS in redb (modeled on noto), backend + frontend write through a channel, rotation (7 days / 5000), commands log_list/log_clear/log_stats/frontend_log.
- **renderer.js split** (1258682): renderer.js = 9 lines, core.js + widgets/<id>/widget.js.
- **ESC → core** (working): a global Escape handler lives only in core.js (`collapseAllShellWidgets` → Telegram context menu, lightboxes, explorer, Telegram widgets, music, folders). The duplicate global `keydown` Escape was removed from telegram/widget.js; local keydown handlers on input fields remain in the module. The backend (`input.rs`) still emits `escape-pressed` to the core.

---

## 15. Tech debt

- Dead Telegram code: `activeTopicThreadId`, the `data-topic-id` branch, `hasBackend`.
- Repo hygiene: `Cargo.lock`, `Server - SSH.cmd` (uncommitted), `.obsidian/` (decide: commit or gitignore).
- `.tmp_analyze.py`, `.tmp_split.py` — removed (leftovers from a worker).
- The player sync does not track child `yt-dlp` processes: on exit an active download is left orphaned (not a regression; at the next change, `shutdown` should get a registry of `Child` processes and kill them).

---

## 16. Engineering session history

> [!NOTE]
> **Session log:** the full record of all engineering sessions (Sessions 1–112+), detailed post-mortems, Win32 runtime debugging, metric measurements, and change history have been moved to 📖 **[`docs/sessions.md`](sessions.md)**.

>
> `architecture.md` is maintained solely as the **normative architecture specification** of the core, the `unidtk-api` contract, the `redb` stores, and the shell's system standards.
