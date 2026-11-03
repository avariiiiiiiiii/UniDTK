# Architecture Notes (History)

These sections were part of the old `docs/architecture.md`. They are frozen: they record names and decisions as they were. The current rules are in [the reference](../reference/README.md).

## Legacy names and their current form

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

## Shell bootstrap (partial)

- **Tauri** is already a library the core uses. "Eating" it means using it as the framework.
- **Cargo** can be embedded, but it is expensive (~2 GB toolchain). The sweet spot: do NOT compile inside the app; instead (a) precompile modules ahead of time, (b) use `shell build` for automatic builds.
- **WebView** is a system component (WebView2/Chromium) and cannot be "eaten". It can only be replaced with a custom renderer (wry/tao is the same thing; Servo/WebKit is huge).

## What is already done (history, for context)

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
