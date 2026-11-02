# UniDTK — Instructions for AI Agents (AGENTS.md)

> **Purpose of this document:** A guide to autonomous development, the mandatory 10-step engineering protocol, architecture standards, and live debugging for all AI agents in the `UniDTK` repository (Unified Desktop Täglich Kit).

---

## 1. Core principles of autonomous work

1. **Carry the task to the end, do not stop halfway:**
   - If the user sets a complex task (optimization, refactoring, bug fix), the agent must investigate all related subsystems, make the changes, build the project, test it, and verify it in the live runtime.
   - Do not ask for confirmation of obvious intermediate steps (running tests, checking compilation, building).

2. **Ban on superficial verification ("Compiling is not the end of the work"):**
   - A successful compilation or passing a couple of unit tests **does NOT mean** the task is solved.
   - Any change to the backend, frontend, task scheduler, or IPC must go through a **full live debugging cycle** in the runtime before the task is handed over.
   - Direct calls to raw utilities such as `cargo check` or `node --check` are forbidden. All end-to-end diagnostics go through the single canonical CLI bridge (`udtk doctor`, including `udtk doctor -check`). Node.js is entirely absent from the runtime and the CLI.

---

## 2. Task execution protocol (mandatory 10-step cycle)

Every task in the `UniDTK` repository must go through the following strict protocol:

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ 1. Receive the problem from the user (record symptoms and expectations)                │
│ 2. Find the cause yourself (investigate code, logs, Win32/DOM system interfaces)       │
│ 3. Launch subagents to search for the problem (confirm the evidence independently)     │
│ 4. Make sure the root and only problem is found (rule out masking)                     │
│ 5. Discuss the plan with the user (high level: architecture, structure, no low-level)  │
│ 6. Discuss the plan with 5 subagents (harsh critique, vulnerability search, consensus) │
│ 7. Show the plan to the user again (state subagent remarks, get approval)              │
│ 8. Implement the plan (few edits — do them yourself; large refactoring — subagents)    │
│ 9. Merge branches into the main line (when isolated git branches were created, merge   │
│    them without leftovers)                                                             │
│ 10. Rebuild the release, run it, measure via CLI, and update the documentation         │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### Detailed description of the steps:

1. **Receive the problem from the user and check it against the master doc:**
   - Carefully record the symptoms, reproduction conditions, and expected result. Do not start chaotic edits without understanding the context.
   - **Study `docs/00_master.md` mandatorily** — the single entry point and the L1 documentation router. Reading the historical log `docs/sessions.md` in full at the start is forbidden; when context is needed, read only the latest sessions via a line slice.
2. **Find the cause yourself:**
   - Localize the problem area in the codebase, the `unidtk.redb` database log, the Win32 message queue, or the frontend.
3. **Launch subagents to search for the problem:**
   - Bring in independent subagents to re-check the leads found, to rule out subjective errors and bias.
4. **Make sure the root and only problem is found:**
   - Prove the cause-and-effect link with facts and logs. Make sure the root cause is removed, not a "band-aid" laid over the symptoms.
5. **Discuss the plan with the user (architecture focus):**
   - Present the solution at the architecture level: links between modules, thread safety, and system contracts. Avoid overloading the user with walls of low-level code.
6. **Discuss the plan with 5 subagents (full critique):**
   - Launch 5 independent subagents with the task of fully critiquing the proposed solution:
     - Evaluate thread safety and possible race conditions;
     - Check the stability of Win32 IPC and window message queues;
     - Check the impact on memory consumption (Working Set) and CPU;
     - Find hidden regressions in the frontend and DOM;
     - Analyze boundary conditions during a cold system start.
   - Collect the remarks, reach consensus, and correct the weak points.
7. **Show the revised plan to the user again:**
   - Briefly report which vulnerabilities the subagents found and how the plan was strengthened, and obtain the user's final approval to execute.
8. **Implement the plan:**
   - If the changes are few: implement them precisely and carefully yourself.
   - If a large refactoring is planned or several subsystems are affected: distribute the tasks to isolated subagents and git branches.
9. **Merge the branches into the main line:**
   - When branches are used, make sure there are no conflicts and merge all work into the main branch (`master`/`main`). Leave no dangling branches or junk files.
10. **Rebuild the release, install into Program Files, run, verify and measure via CLI, update the documentation:**
    - **The release build and deployment are done ONLY with the single command `udtk build`** (from the checkout, `target\release\udtk.exe`). It is forbidden to build by hand with `cargo build`, to copy binaries into `Program Files`, or to rebuild in separate phases. `udtk build` stops the application itself, in one pass `cargo tauri build --bundles nsis` builds all three binaries and the installer, silently installs it into `C:\Program Files\UniDTK`, verifies the installation by SHA-256, restarts the application, and gates on live IPC. Manual copying that bypasses the installer does not re-register the scheduler tasks and breaks the installation rules.
    - The only build command: `udtk build [-j N]` (the default thread count comes from `.cargo/config.toml`; `-j` overrides it via `CARGO_BUILD_JOBS`). The binary from which `udtk build` is launched moves itself aside (rename) during the build and is restored afterwards; self-build from `Program Files` is forbidden (ACL).
    - Canonical binaries in a dev run: `target/release/unidtk.exe`, `target/release/udtk.exe`, `target/release/vpn-helper.exe`; in the shipped product — the install directory `C:\Program Files\UniDTK` (filled by the installer; nothing is copied by hand).
    - Launch the application (via Task Scheduler `schtasks /run /tn UniDTK` or `udtk run`).
    - Run an end-to-end measurement of metrics and a log check via the CLI:
      - `udtk doctor [--simulate]` — full diagnostics with simulated user actions, without damaging data.
      - `udtk doctor -ping` — responsiveness of the core and modules (record the RTT in ms).
      - `udtk logs --level ERROR` and `udtk logs --source frontend --limit 30` — check for the absence of JS errors and rendering exceptions.
      - `udtk shot` — **MANDATORY screenshot** via the CLI for a visual layout audit on any UI/DOM change (saved automatically to Pictures/Screenshots with a timestamp, prints the absolute path; checked by the agent via `view_file`), and a check for the absence of "Failed to fetch", empty widgets, and broken styles. For backend/CLI tasks the screenshot serves as a basic sanity check of window stability.
    - **Three-level documentation record:**
      1. **Master session log (`docs/sessions.md`):** Every session must be recorded in the log under the current date, strictly following the 5-paragraph standard:
         - *Paragraph 1 (Context and symptoms):* The essence of the task or the symptoms of the problem;
         - *Paragraph 2 (Root cause):* The proven primary cause at the level of Win32, redb, message queues, or DOM;
         - *Paragraph 3 (Architecture decision):* What was done conceptually and which contracts were kept;
         - *Paragraph 4 (Changed files):* A list of direct relative links to all created and changed files (for example, `[00_master.md](docs/00_master.md)`);
         - *Paragraph 5 (Verification and metrics):* Results of `udtk doctor`, IPC ping RTT (ms), Working Set RAM (MB), log status, and the `udtk shot` screenshot.
         - Add a line to the express index at the top of `docs/sessions.md` with the date and the Git commit SHA.
      2. **Master doc for agents (`docs/00_master.md`):** Update the status dashboard (active gate, module status, metrics) and the link map when new components or documents appear.
      3. **Normative architecture (`docs/architecture.md`):** Update the specification (§1–§15) **only** if the task changed the architecture contracts of the core, modules, or storage. Dumping day-by-day debugging logs into `architecture.md` is forbidden.

---

## 3. Live debugging commands via the CLI bridge (`opt/cli`)

The project implements its own native command-line tool (binary `udtk`, source `opt/cli/main.rs`, built by the single Cargo project `toolchain` into `target/release/udtk.exe`). It uses TCP IPC (`127.0.0.1:26540`) and a native WebSocket CDP in Rust (`tungstenite`), without external scripts or third-party environments.

### 3.1 Diagnostics, compilation, and status:
* `udtk doctor [-kernel|-modules|-ping|-env|-check|-quick|-simulate|-all]` (or the built `target/release/udtk.exe doctor ...`)  
  Comprehensive diagnostics: compilation, presence of DLLs (`libmpv-2.dll`, `tdjson.dll`, `sing-box.exe`), `.env` variables, Task Scheduler task status, core IPC availability, and safe user simulation (`--simulate`).
* `udtk doctor -check` — checks backend compilation via the CLI bridge (runs a build check, takes tens of seconds, not instant).
* `udtk doctor -ping` — checks the responsiveness of the core IPC server.
* `udtk invoke <cmd> [json-args]` — direct call of a core or module command by its name from the manifest (`player_status`, `vpn_status`, `telegram_status`, `log_stats`, …).
  **`invoke` is a socket door, and it is read-only by design.** Commands declared in the contract as `write` (writing to the log, volume, future VPN/Telegram/explorer operations) are **not** served through it. A refusal with the reason "changes state, and write commands stay on the UI door" is the norm, not a fault. Writes go only through the interface door (Tauri or an authenticated WebSocket); from the CLI they are checked indirectly, via `udtk gui js:…` or by their effects (`udtk logs`). Full list — `udtk help --json`.
* `udtk logs [--limit N] [--since ID] [--level LEVEL] [--source SOURCE]` — selects records of the core and frontend system log from the `redb` database.
* `udtk path [status|ensure]` — checks and registers `udtk.exe` in the user's Windows `PATH`.
* `udtk doc` — renders the project book (README.md and the markdown it links to) to HTML in the cache and opens it, as `rustup doc` opens the Rust documentation.

### 3.2 Managing Windows Task Scheduler tasks and autostart:
* `udtk tasks status` — checks the state of all registered Windows tasks.
* `udtk tasks register [all|name]` — generates the XML and registers tasks with automatic privilege elevation via UAC:
  - `UniDTK` — automatic start of the shell at user logon (`InteractiveToken`, unelevated).
  - `UniDTKSingbox` — elevated helper for the Sing-Box TUN adapter (`HighestAvailable`).
  - `UniDTKVpnStop` — elevated helper that stops VPN processes (`HighestAvailable`).
* `udtk tasks cleanup-old` — automatically finds and deletes obsolete tasks (`NothingWallpaper*`).
* `udtk autostart [status|enable|disable]` — quick management of the logon task.

### 3.3 Lifecycle management and build:
* `udtk build [-j N]` — **the only release build command**: stops the application, in one pass `cargo tauri build --bundles nsis` builds all three binaries and the NSIS installer, silently installs it into `C:\Program Files\UniDTK`, verifies the installation by SHA-256, restarts the application, and gates on live IPC. `-j` overrides the thread count (`CARGO_BUILD_JOBS`); self-build from `Program Files` is forbidden (ACL).
* `udtk stop` — graceful stop of the application (first its own `quit_app` via CDP, then forced termination of whatever holds the DLLs, and `schtasks /end` for the elevated helpers) — so that the files can be replaced.
* `udtk run [args...]` — launches the installed application (it lives in the same directory as the CLI: in the shipped product `C:\Program Files\UniDTK`, in a dev run `target\release`). The `--debug` flag has been removed: CDP is always enabled, and the port is discovered via `udtk doctor -ping`.
* Dev entry without a CLI command: `cd toolchain && cargo tauri dev` (the `udtk dev` command has been removed).

**Release build order (verified in practice).** Only `udtk build` builds and deploys. The application must be stopped before the files are replaced (otherwise `os error 32` without a file name); `udtk build` performs the stop itself. The build is heavy: thin LTO is memory-hungry, so the thread count is set in `.cargo/config.toml` (`[build] jobs`), and the compiler stack is `RUST_MIN_STACK=134217728`; `-j N` overrides via `CARGO_BUILD_JOBS`. Tests: `cargo test --release --workspace -j 1` (`-j 2` brings down two LTO builds at once, `0xc000012d`). On an **empty** `target/` the build is impossible without a workaround: `build.rs` requires `../target/release/udtk.exe` and `vpn-helper.exe` (fixed with empty placeholder files, which cargo then overwrites).

**Interface migration status** (one contract, two doors, own protocol, transport flags `tauri`/`shadow`/`ws`) — `docs/cliudtk.md`, which also holds the acceptance criteria and reports per slice.

### 3.4 Visual and DOM control via CDP:
* `udtk gui tree` — a compact semantic tree of the interactive elements of WebView2.
* `udtk gui click:<target> | type:<target>:<text> | press:<key>` — physical input emulation with a receipt, a log delta, and a refusal when the target is covered.
* `udtk gui js:<expr>` — runs arbitrary JavaScript in the context of the active WebView2.
* `udtk shot [path] [--json]` — takes a screenshot of the screen via CDP (by default saves to Pictures/Screenshots with a local timestamp and prints the absolute path).
* `udtk cdp-list` — lists the active pages and CDP targets.
* `udtk help --json` — the machine-readable manifest of the CLI bridge commands.

---

## 4. Architecture rules (`docs/architecture.md`)

1. **Microkernel and module isolation:**
   - The core (`core/kernel`) is only the frame: the IPC server dispatcher, the event bus, the system tray, storage (`unidtk.redb`, `unidtk_logs.redb`), and the attachment of the window to the desktop (`wallpaper.rs`).
   - The module contract is `core/api` (package `unidtk-api`): the `Module` trait, services (log, events, paths), and `ModuleDb`. The contract **must not** depend on `tauri`, `redb`, or Win32.
   - Modules are vertical slices `modules/<id>/{rust,ui}`: `clock`, `explorer`, `glyph-bar`, `mp3-player`, `telegram`, `vpn`. They are isolated and independent of each other: `rust/` is the backend half, `ui/` is the frontend half of one module; splitting them across different modules is forbidden.
   - A module that is a separate crate **does not depend on the core crate**: the `unidtk-api` contract is mandatory for it, and the Tauri host types (`AppHandle`, `Emitter`, `async_runtime`) are allowed — four of the five modules already use them, and owning its own runtime presupposes this. The check is `lint_modules_integrity()` in `toolchain/build.rs`. Interaction between modules is exclusively through the contract's events (`unidtk_api::emit` / `subscribe` / `state`), not by calling each other: Cargo must not allow a module→module reference.
   - `#[tauri::command]` is declared **only in the crate where `generate_handler!` is placed** (the core). A command must not be marked `pub` in a library crate: the macro exports `__cmd__<name>` to the crate root and a collision `E0255` occurs. A module exposes plain functions; the core makes thin wrappers.

2. **`redb` storage:**
   - Complete rejection of SQLite and third-party C/FFI dependencies (`rusqlite`, `libsqlite3-sys`).
   - User data (tracks, playlists, history, AI DJ, configurations, widget settings) — in `%LOCALAPPDATA%\UniDTK\unidtk.redb` via pure Rust transactions.
   - Logs — a separate file `%LOCALAPPDATA%\UniDTK\unidtk_logs.redb` (asynchronous queue, one commit per batch, `WARN/ERROR` are not lost).
   - Modules do not receive the raw database engine (`raw_db`): state that must live in the shared database is stored via `ModuleDb`, to which the core itself allocates a key slice `mods/<id>/…` (the first consumer is the `mp3-player` library); everything else the module keeps in its own data files or in memory.

3. **Non-blocking startup and integration with `WorkerW`:**
   - It is strictly forbidden to run blocking Win32 calls (`SendMessageTimeoutW`, window search) on the Tauri main UI thread. Polling the readiness of the desktop (`Progman`/`WorkerW`) is done strictly on a background worker thread.
   - The main UI thread is used exactly once, via `run_on_main_thread`, to call `SetParent` and show the window (`window.show()`).

4. **Preserving DOM state and preventing flicker (`relayoutWidgets`):**
   - It is forbidden to perform a blunt reset of the DOM tree (`widgetsCanvas.innerHTML = ""`) on window geometry changes, moves between monitors, or `display-changed` events.
   - Resizing and anchoring are handled by the gentle function `relayoutWidgets()`, which keeps widget instances, player state, network connections, and user input.

5. **Canonical motion tokens (`core/ui/motion.js`):**
   - All animations in widgets and overlays are built exclusively on standardized tokens:
     - Easings: `standard` (`0.4, 0, 0.2, 1`), `spring` (`0.34, 1.56, 0.64, 1`), `expo`/`expoOut` (`0.30, 0.95, 0.35, 1.00`), `expoOutRev`/`surfaceExit` (`0.38, 0.05, 0.45, 1.00`), `smooth` (`0.25, 0.1, 0.25, 1`), `snap`, `content`.
     - Durations: `hover` (120 ms), `transient` (220 ms), `motion` (300 ms), `layout` (340 ms).
   - The source of truth for durations is `setMotionScale()`; the CSS values in `:root` (`core/ui/styles.css`) are only a fallback.
   - `prefers-reduced-motion` is mandatory. It is implemented so that it does not overwrite the user's scale: `userScale` (the user's choice) and `currentScale` (the effective one, `0.01` under reduced motion) are kept separate.
   - Hardcoding arbitrary `cubic-bezier` values and random timings in CSS is forbidden — all variables are taken from `:root`.

6. **Native helpers in Windows Task Scheduler (no terminal wrappers):**
   - All background tasks (`UniDTK`, `UniDTKSingbox`, `UniDTKVpnStop`, `SecondDisplayHost`) run strictly as native binaries directly, with arguments and the `<Hidden>true</Hidden>` flag.
   - It is strictly forbidden to use `powershell.exe`, `cmd.exe`, or `.vbs` to hide windows, because the Windows console subsystem is guaranteed to flash a black window on screen.
   - That is why the VPN helper (`vpn-helper.exe`, `modules/vpn/helper/main.rs` — the second binary of the `vpn` module) is a separate GUI-subsystem binary, not a VBS chain: it also stops the VPN within the Windows shutdown time limit.

7. **Flat repository root (no nested folder):**
   - The project lies directly in the repository root (`Cargo.toml`, `Cargo.lock`, `core/`, `modules/`, `opt/`, `toolchain/`, `docs/`, `assets/`, `scheduler-tasks/`, `tests/`, `target/`, `.cargo/`). There must be no intermediate wrapper folder.

8. **Program in Program Files, data in AppData:**
   - The installed application lives entirely in `C:\Program Files\UniDTK` (installer in `perMachine` mode): `unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, `sing-box.exe`, native DLLs, `models/`, `modules/`, `scheduler-tasks/`. The directory is read-only: **the application writes nothing next to itself** (log, cache, state — everything goes to the data directory).
   - Everything mutable is in `%LOCALAPPDATA%\UniDTK`: databases (`unidtk.redb`, `unidtk_logs.redb`), `logs/`, `cache/`, `tdlib/` (Telegram session), `music/`. Roaming (`%APPDATA%`) is not used at all, and neither is LocalLow: configs contain absolute paths and `cookies_browser`, so such a config must not travel with the profile.
   - Only the contract (`unidtk_api::paths`) and the core know the layout. A module does not build paths itself and does not open databases itself: it declares a store (`ModuleContext::store` + `StoreSpec`) and works with the issued `ModuleDb`. That is why moving the directories affects no module.
   - `target/` is the build cache, and the only **shipped artifact** is the installer (`target/release/bundle/`). `udtk run` launches the application from the same directory where the CLI itself lives, rather than searching for the binary in the repository.
   - The repository must contain no hardcoded absolute paths: task templates (`scheduler-tasks/*.xml`) contain `@INSTALL_DIR@`, which registration substitutes. Tasks and PATH are set up by the installer (`toolchain/windows/hooks.nsh` → `udtk tasks register all` / `udtk path ensure`), not by the first launch of the application.

9. **Two-level documentation record (master SSOT + session log):**
   - **`docs/00_master.md` (master doc for agents / MOC):** The single entry point into the repository's knowledge base. It contains the express status dashboard, runtime metrics, the L1 documentation router, and the module topology. It is updated when the active focus changes, or when modules or documents are added.
   - **`docs/sessions.md` (master session log by day):** The chronological on-board log of all engineering sessions (newest first). Each session is recorded strictly by the 5-paragraph standard:
     1. *Task context and symptoms;*
     2. *Proven root cause (Win32, DOM, redb, race conditions);*
     3. *Architecture decision and invariants;*
     4. *Changed files (direct relative links of the form `[name](path)`);*
     5. *Runtime verification (`udtk` CLI calls, IPC ping RTT, Working Set RAM, log status, `udtk shot` screenshot).*
     - An entry in the express index at the top of the file, with the date and the Git commit SHA, is mandatory.
   - **`docs/architecture.md` (normative architecture book):** A strict, unchanging standard for the core, the `unidtk-api` contract, the `redb` stores, integration with Win32 WorkerW, and the Nothing OS design system. **Dumping day-by-day debugging logs and bug fixes into `architecture.md` is strictly forbidden** — the file is kept compact (under 350 lines, under 30 KB, readable in one tool call) and is updated only when fundamental system contracts change.

Session log entries and all documentation are written in English.
