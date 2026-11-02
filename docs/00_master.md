---
title: UniDTK Master Documentation & Map of Content (MOC)
type: moc
project: unidtk
version: "0.1.0"
updated_at: "2026-11-02"
status: active
format: "Obsidian Vault MOC / GitHub Markdown"
---

# UniDTK — Master Map of Content (MOC)

> **How to use this documentation:** This is the central engineering dashboard (L1 router) for AI agents and developers. It is read in 1–2 minutes, records the current verified state of the system, and routes to all specialized specifications. At the start of any task the agent **must** begin its analysis with this file (Steps 1–2 of the `AGENTS.md` protocol).

---

## 1. Express status dashboard

> [!IMPORTANT]
> **Current engineering focus:** Migration from Chromium WebView2 to the native lightweight GPU framework **GPUI** (Zed GUI), on the branch `feature/gpui-desktop`.  
> **Migration status:** **Gate 0** (transparent window on WorkerW, 0% CPU) and **Gate 1** (Nothing OS design system, Ndot 55 font, `clock` and `glyph-bar` widgets on WASAPI, dynamic hit-testing via Taffy prepaint) are complete.  
> **Completed 2026-10-20 (session 114):** Five-stage design-token migration — the WebView2 shell is fully recolored from one theme object: engine v2 (primitives only in JS, derivations in CSS `:root`), a sweep of 1189 hardcoded values, `styles.css` split into the core plus 5 per-module CSS files, canvas token cache updated on the `unidtk-theme-changed` event.  
> **Completed 2026-10-22 (session 116):** Place name in micrographics — BigDataCloud → Nominatim chain (the BDC DPI block no longer silences the panel; attribution in secondary); release pipeline sped up ~2.5×: NSIS `zlib`, 331 MB of AI DJ models removed from the installer payload (downloaded into AppData via `mp3-player/model_fetch`), `udtk build -j 3` without OOM — full build ~11.5 min, installer 126 MB.  
> **Completed 2026-10-24/25 (sessions 117–123):** Sea Glass v3 rolled back; layout restored to the 20.10 reference; theme schema v3 (fonts + geometry defaults as tokens); Sea Glass light — mint glass with gloss on all surfaces, aqua, marine micrographics, hardcode sweep; Classic without regression; switching themes does not move the layout. Process: design live via injection, build once per fact.  
> **Completed 2026-10-29 (session 131):** Customization menu redesign ([`tasks/menu-redesign.md`](tasks/menu-redesign.md)) — side panel on the right (~420 px) over the live desk, sections "Desk", "Folders", "Look" + VPN panel; extra desk buttons removed (`#editMode`, `#resetWallpaper`, `#quitApp`), one menu button left on the desk; arrange mode on the desk snapping to a 16 px grid and to corners (`anchor ∈ {tl,tr,bl,br}`); folders without AppData — added from the Start menu (`start_menu.rs`, STA worker), system dialog, Ctrl+V, icons via BGRA data URL, status of unavailable shortcuts with "Find…"; undo of actions as a 5 s toast (`toast.js`) instead of alert/confirm; legacy migration code from `Shell` removed.  
> **Completed 2026-10-28 (session 130):** Customization menu bugs — the animation profile no longer resets, one config write queue (theme survives exit without loss), menu actions do not rebuild the desk (`syncWidgets`), VPN does not overwrite settings when a load fails, default layout comes from the core, 38 shortcuts moved from the old `Shell`.  
> **Completed 2026-10-28 (session 129):** Trackpad scrolling over the shell works (confirmed by the user) — the core reads trackpad contacts via raw HID, because over WebView2 Windows hands the gesture to DirectManipulation and does not synthesize wheel events.  
> **Completed 2026-10-27 (session 128):** Mouse wheel — via `WH_MOUSE_LL`; layout no longer drifts after sleep; opening folders in Explorer without long tasks.  
> **Completed 2026-10-27 (session 127):** AI DJ switches again; morphs without flicker (clock, Telegram chat, explorer), avatars flow between rail and list, glyph-bar drags with snapping; explorer: long tasks 0.7–1.1 s → 0.12–0.17 s; menu and explorer in the Sea Glass tint; jelly radii on tokens. Open: trackpad scrolling (awaiting confirmation).  
> **Completed 2026-10-27 (session 126):** Sea Glass v4.1 — expanded cards and popups in the light tint of their own widget (`surfaceTint`, `data-surface-tint` for portals), glyph-bar with a jelly slider, each user gets a random color set (`rollColors` → `userColors` in redb, "Shuffle / Restore"), hardcoded jelly CSS moved to the tokens `--jelly-hi-rgb`/`--well-rgb`.  
> **Completed 2026-10-26 (session 125):** Sea Glass v4 — theme schema v4 (material `flat|jelly`, 8 pastel slots + `tintMap` for widgets/panels/micrographics groups, grain/edge/shadow/ink optics, migration of old records); each widget has its own pastel tint, text is deep ink of the same hue (OKLCH, ≥7:1); micrographics as pills per group; light menu/explorer/Telegram panels; the menu's "Material" section makes everything configurable; blur 0 — idle CPU 0.31%, GPU 0.4%; Classic without regression.  
> **Completed 2026-10-26 (session 124):** Physical optics of Sea Glass: token schema extended with optical scalars `glassRim`, `glassSheen`, `glassShadow`; caustic shadows and SSS gradient; hardcoded `:root[data-theme="sea-glass"]` card rules removed; micrographics legibility via a capsule and the deep sea-ink token `rgb(var(--fg-rgb))` instead of blind `#fff`; ocean accent Deep Sea Teal (`#007780`) instead of red; Classic without a single pixel of regression.  
> **Working tree, 2026-11-02 (not committed yet):** widget nesting in containers (`core/ui/nesting.js`, `container`/`embeddable` in metadata), module packs and states (`pack`, `backend`, `settingsView`; `off` / `background` / `desk`), runtime module switching without restart (`suspend`/`resume`, `set_module_enabled`), per-theme colors in `config.colorOverrides`, theme contracts (`tints`, `lookOptions`, `onAccentRgb` kept at WCAG AA by `readableOn`).  
> **Production base:** The stable release `C:\Program Files\UniDTK` (Tauri v2 + WebView2) runs normally and is built with one command, `udtk build`.

| Subsystem / Metric | Value / Status | Technology stack | Primary document |
| :--- | :---: | :--- | :--- |
| **Active branch** | `main` (current checkout; GPUI migration work lives on `feature/gpui-desktop`) | Git | [gpui_transition.md](gpui_transition.md) |
| **Design tokens / themes** | 🟢 Two built-in presets: Classic (flat) + Sea Glass (jelly), schema v4: colors, fonts, geometry, material, palette + tintMap, optics | CSS custom properties, `data-material`, `unidtk-theme-changed`, OKLCH ink, pre-paint from the `paint` cache | [sessions.md §125](sessions.md) |
| **Core (kernel)** | 🟢 Stable | Headless Rust, `unidtk-api`, TCP IPC `127.0.0.1:26540` | [architecture.md](architecture.md) |
| **Storage (DB)** | 🟢 Stable | Clean `redb` transactions (`unidtk.redb`, `unidtk_logs.redb`) | [architecture.md §1.1](architecture.md#11-the-core-is-not-what-the-user-sees) |
| **CLI bridge (`udtk`)** | 🟢 Stable | Rust CLI, CDP WebSocket, TCP IPC | [cliudtk.md](cliudtk.md) |
| **Production GUI** | 🟢 Stable | Tauri v2, WebView2 (Working Set RAM: ~420 MB) | [architecture.md](architecture.md) |
| **Experimental GUI** | 🟡 Gate 1 ready | GPUI, DirectX 11, DirectComposition (RAM: **59.7 MB**, CPU: **0.39%**) | [gpui_transition.md](gpui_transition.md) |
| **Module: Clock** | 🟢 Ready (GPUI) | GPUI, Nothing Ndot 55 typography, on-demand timer (1 Hz) | [gpui_transition.md §3](gpui_transition.md) |
| **Module: Glyph Bar** | 🟢 Ready (GPUI) | GPUI, 10 LED segments, WASAPI master volume | [gpui_transition.md §3](gpui_transition.md) |
| **Build / Release** | 🟢 `udtk build` ~11.5 min | cargo thin-LTO `jobs=3`, NSIS `zlib`, installer **126 MB** (models → AppData) | [tokens/cliudtk.md](cliudtk.md), [sessions.md §116](sessions.md) |
| **Module: MP3 Player** | 🔵 GPUI queue | libmpv FFI raw-dylib, WASAPI loopback, AI DJ, arcs; model weights in `%LOCALAPPDATA%\UniDTK\models` (downloaded, session 116) | [tasks/libmpv.md](tasks/libmpv.md) |
| **Module: Micrographics** | 🔵 GPUI queue | WinRT geolocation, adaptive precipitation/weather grid; place name via BDC→Nominatim chain (session 116, `b08fbae`) | [architecture.md](architecture.md) |
| **Module: Telegram** | 🔵 GPUI queue | TDLib C FFI, ByeDPI socks5, reactions, forum threads; pinned messages in groups — 15 s budget + cooldown (session 115, `5ab1e7a`) | [telegram-topics-and-communities.md](telegram-topics-and-communities.md) |
| **Module: VPN** | 🔵 GPUI queue | Sing-box TUN, native `vpn-helper.exe` (GUI subsystem) | [modules/vpn.md](modules/vpn.md) |
| **Module: Explorer** | 🔵 GPUI queue | Tabs, Win32 Shell API, cross-view sorting | [architecture.md](architecture.md) |

---

## 2. Documentation routing table (L1 router)

Each document in the repository has a strictly limited scope. Agents must not read all files in sequence — load the satellites strictly by task trigger:

| Document | Scope | When the agent reads it (trigger) |
| :--- | :--- | :--- |
| **[`docs/00_master.md`](00_master.md)** | Main dispatcher (MOC), current runtime, documentation map | **Always at the start of any session** (Steps 1–2). |
| **[`AGENTS.md`](../AGENTS.md)** | The project's main rules: 10-step protocol, CLI commands, Win32 rules | Before planning and executing any action. |
| **[`docs/architecture.md`](architecture.md)** | Normative architecture: core, `unidtk-api`, `redb`, WorkerW, motion tokens | When changing system contracts, adding modules, or editing the core. |
| **[`docs/sessions.md`](sessions.md)** | Day-by-day chronological log of engineering sessions (1–112+) with links to commits and files | When looking for context on past debugging cases, regressions, or open backlog. |
| **[`docs/gpui_transition.md`](gpui_transition.md)** | Strategy and roadmap for moving from WebView2 to Zed GPUI (Gates 0–5) | When working on the `feature/gpui-desktop` branch and porting widgets to GPUI. |
| **[`docs/cliudtk.md`](cliudtk.md)** | Full specification of the `udtk` command line, interface doors, CDP and IPC | When debugging the CLI, implementing new bridge commands, or automating tests. |
| **[`docs/unicoremodmotion.md`](unicoremodmotion.md)** | Canonical motion tokens: Bézier curves, durations, morph/fade physics | When building UI, widget animations, and surfaces. |
| **[`docs/tasks/menu-redesign.md`](tasks/menu-redesign.md)** | Plan for the customization menu rework: panel, desk arrangement, folders without AppData, `Shell` cleanup | When working on the menu, folders, or widget layout. |
| **[`docs/kanban.md`](kanban.md)** | Visual Kanban board for people (Obsidian Kanban) | When checking the high-level user backlog. |

---

## 3. Codebase topology (component map)

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

---

## 4. Interactive knowledge base map (tree navigator)

- 📁 **docs/**
  - 📄 **[00_master.md](00_master.md)** — 🌟 *You are here (main MOC dispatcher)*
  - 📄 **[architecture.md](architecture.md)** — *Architecture standard for the core, module isolation, and Win32*
  - 📄 **[sessions.md](sessions.md)** — *Chronological engineering session log (1–112+)*
  - 📄 **[gpui_transition.md](gpui_transition.md)** — *Strategy and tracker for the migration to Zed GPUI (Gates 0–5)*
  - 📄 **[cliudtk.md](cliudtk.md)** — *Specification of the `udtk` CLI bridge, socket doors, and CDP*
  - 📄 **[unicoremodmotion.md](unicoremodmotion.md)** — *Canonical motion and animation tokens*
  - 📄 **[kanban.md](kanban.md)** — *Kanban board for people (Obsidian Kanban)*
  - 📁 **modules/**
    - 📄 **[vpn.md](modules/vpn.md)** — *VPN and Sing-box module specification*
  - 📁 **tasks/**
    - 📄 **[libmpv.md](tasks/libmpv.md)** — *Audio engine integration over FFI libmpv-2.dll*
    - 📄 **[logging.md](tasks/logging.md)** — *Structured logging subsystem on redb*

---

## 5. Documentation update rules for agents

1. **Single link standard:** All links inside project documents must be standard relative Markdown links of the form `[Text](relative_path.md)`. `[[wikilinks]]` are forbidden outside personal notes.
2. **Breadcrumbs in every document:** Each child document must have a navigation trail back to `00_master.md` on the line under the heading:
   `> [← UniDTK Master MOC](00_master.md) | [Architecture](architecture.md) | [Session log](sessions.md)`
3. **Update at Step 10:**
   - Every completed session **must** be recorded in [`docs/sessions.md`](sessions.md), tied to a date and a Git commit SHA.
   - The express dashboard and status table in [`docs/00_master.md`](00_master.md) are updated when moving to new gates, adding documents, or changing runtime metrics.
   - The normative book [`docs/architecture.md`](architecture.md) is updated **only** when core or module architecture contracts change.
