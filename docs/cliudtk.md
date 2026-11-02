# `udtk` — the UniDTK CLI bridge

Reference for the native CLI bridge of the **UniDTK** project (Unified Desktop Täglich Kit): its commands, flags, internals, the diagnostic loop, and the rules every module must follow.

> Source of truth — `opt/cli/main.rs` (+ `opt/cli/splash.rs`, `opt/cli/splash_art.rs`). This document describes the state as of 18.09.2026 (after sessions 58–60).

---

## 1. What it is and where it lives

`udtk` is the second binary of the single Cargo project `toolchain/` (alongside `unidtk` and `vpn-helper`):

```toml
[[bin]] name = "unidtk"     path = "../core/kernel/main.rs"
[[bin]] name = "udtk"       path = "../opt/cli/main.rs"
[[bin]] name = "vpn-helper" path = "../modules/vpn/helper/main.rs"
```

* Build: `cargo build --release --bins` from the repository root. Cargo places the artifacts into the single `target/release/` directory, which `.cargo/config.toml` sets.
* In a shipped install, the CLI and the app sit **next to each other**: `C:\Program Files\UniDTK\{unidtk.exe, udtk.exe, vpn-helper.exe, sing-box.exe, …}`.
* There are no external dependencies and no script wrappers. All work with the kernel and the frontend runs on native Rust.

### Transport

| Channel | Address | Who starts it | What it provides |
|---|---|---|---|
| IPC (TCP, JSON lines) | `127.0.0.1:26540` | `unidtk.exe` (kernel) | `invoke`, `logs`, `doctor -ping/-simulate`, baseline `log_max_id` |
| CDP (HTTP + WebSocket) | the app picks the port (usually `127.0.0.1:9222`), published in the IPC `ping` reply → `cdpPort` | WebView2, always enabled | `gui`, `dom`, `eval`, `shot`, `cdp-list`, the CDP part of `doctor -simulate` |

IPC timeouts: connect 500 ms, read/write 2500 ms. A request is one JSON line `{"cmd": …, "args": …}`. The reply is `{"ok": true, "result": …}` or `{"ok": false, "error": …}`.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success (for `doctor`: all checks passed) |
| `1` | Execution error / unreachable IPC / remote command failed / `doctor` failure |
| `2` | Invalid arguments: unknown command, unknown `doctor` flag |

Unknown `doctor` flags are rejected strictly (whitelist `ALLOWED_DOCTOR_FLAGS`). Nothing is "silently ignored".

---

## 2. Command summary

| Command | Purpose | Requires the running app |
|---|---|---|
| `udtk` (no arguments) | Prints the branded splash and the command map | no |
| `udtk doctor [flags]` | Comprehensive non-destructive diagnostics (compiler, dependencies, kernel, modules, IPC, simulation) | partly (`-ping`, `-simulate`) |
| `udtk invoke <cmd> [json]` | Direct call of a kernel/module IPC command | yes |
| `udtk logs [filters]` | Reads log entries from `unidtk_logs.redb` over IPC | yes |
| `udtk gui [steps]` | Drives the page through a single CDP session: `js:`, `click:`, `type:`, `press:`, `tree`, `wait:` | yes (+CDP) |
| `udtk shot [path] [--json] [-q]` | Screenshot via CDP (default `Pictures/Screenshots`) | yes (+CDP) |
| `udtk cdp-list` | Lists CDP targets/pages | yes (+CDP) |
| `udtk tasks [status\|register\|unregister\|cleanup-old]` | Manages Windows Task Scheduler tasks | no |
| `udtk autostart [status\|enable\|disable]` | Quick control of autostart (`UniDTK`) | no |
| `udtk path [status\|ensure]` | Checks/registers the CLI directory in the User PATH | no |
| `udtk build` | Full release build + resource layout | no |
| `udtk dev [--debug]` | `cargo tauri dev` (+ CDP on 9222) | no |
| `udtk run [args…] [--debug]` | Starts the app from the directory that holds the CLI itself | no |
| `udtk help [--json]` | Help text; `--json` gives the machine-readable command manifest | no |

Commands that no longer exist:

* `udtk ping` and `udtk status` are gone. There is no command, alias, or redirect for either. Both print `error: unknown command: …` plus help to stderr and exit with code 2. Ping lives only as the flag `doctor -ping`.
* `udtk check` was removed too. Use `udtk doctor -check` instead (otherwise `error: unknown command: check`, exit 2).
* `udtk commands` was removed. The machine manifest moved to the flag `udtk help --json` (otherwise `error: unknown command: commands`, exit 2).
* The aliases `-h` and `--help` were removed. Help is invoked only as `udtk help`. A bare `udtk` prints the splash, while `udtk -h` gives `unknown command`, exit 2.
* Redundant `doctor` flag synonyms were removed: `-cargo`/`--cargo` (a duplicate of `-check`) and `-sim`/`--sim` (a duplicate of `-simulate`), as well as the historical `-app`/`--app` and `-ipc`/`--ipc`. Each flag keeps exactly one `-x`/`--x` pair. Anything else gives `error: unrecognized flag '…'`, exit 2.

---

## 3. `udtk doctor` — the diagnostic loop

A single health-check entry point. There is no "live telemetry" with hardcoded kernel variables. Everything is built from declarative module manifests and polymorphic calls of the `unidtk-api` contract.

### 3.1 Flags

| Flag (aliases) | Section | What it does |
|---|---|---|
| `-check`, `--check` | compiler & build | `cargo check` in `toolchain/` |
| `-env`, `--env` | dependencies & DLLs | Presence of `libmpv-2.dll`, `tdjson.dll`, `sing-box.exe`, `vpn-helper.exe`, `ciadpi.exe` (ByeDPI, Optional, from the telegram manifest); `.env`; weights `opt/models/discogs-maest-129e/{model.onnx,model.onnx.data}`; state of scheduler tasks + detection of stale ones |
| `-kernel`, `--kernel` | kernel diagnostics | Dry emulation of the kernel subsystems (see 3.3) |
| `-modules`, `--modules` | modules diagnostics | Declarative dependencies + life cycle of all modules (see 3.4) |
| `-ping`, `--ping` | runtime & IPC | TCP poll of the kernel (500 ms) + `log_stats` telemetry |
| `-quick`, `--quick` | — | Quick run without compilation: `env + kernel + modules + ping` |
| `-simulate`, `--simulate` | user simulation | End-to-end simulation of actions and a CDP run of the rendering functions (see 3.2) |
| `-all`, `--all` | everything | Full audit (default mode) |

The full whitelist lives in `ALLOWED_DOCTOR_FLAGS`. Anything not in it (including the historical `-app`) is rejected with code 2.

### 3.2 Section selection logic

* If **any** selective flag is passed and there is no `-all`/`-quick`, exactly **the chosen** sections run.
* Otherwise, with `-quick`: `env + kernel + modules + ping`.
* Otherwise (default or `-all`): `compiler + env + kernel + modules + ping`. `-simulate` is added only when requested explicitly.

Result: `[✓] UniDTK doctor: ALL CHECKS PASSED` (exit 0) or `[!] UniDTK doctor: FAILED (N issues detected)` (exit 1). A module failure in the life-cycle phase does not bring the process down. It is reported as `[FAIL]`/`[WARN]`.

### 3.3 Kernel section (`-kernel`)

Five checks built on the `unidtk-api` contract. All of them run in memory, with no disk access:

1. `kernel:in_memory_db` — the `ModuleDb` contract on `InMemoryModuleDb`: CRUD, prefix `kv_list`, `kv_count`, deletion.
2. `kernel:event_bus` — loopback of the event bus: `emit` → `state` returns the sent value.
3. `kernel:system_paths` — resolution of `install_dir`, `data_dir_view` (without creating anything!), `logs_dir`, `cache_dir`.
4. `kernel:task_templates` — substitution of `@INSTALL_DIR@` and the user name in the task XML templates (no placeholders may remain).
5. `kernel:capture_logger` — isolation of the thread-local log-capture buffer.

### 3.4 Modules section (`-modules`)

Modules are taken from the single registry `core/kernel/registry.rs::all_modules()`, not from a hand-written `vec![Box::new(…)]` in the CLI. Each run consists of four phases.

**Phase 1 — declarative manifest `module.dependencies()`.** For each entry, the carrier is checked:

* `DynamicLibrary { file_name, companions, min_bytes }` — `resolve_binary` + `validate_pe_x64` (MZ, PE signature, `Machine == 0x8664`, minimum size) + presence of companions (for example, OpenSSL for TDLib).
* `Executable { file_name, probe_args }` — presence + an optional smoke probe (`CREATE_NO_WINDOW`, capture of stdout/stderr, a 1500 ms watchdog).
* `ModelAsset { relative_path, min_bytes, companion_files }` — the ONNX graph, its minimum size, and external weights (`.data`) with their own minimums.
* `WindowsTask { task_name, require_elevated }` — registration of the task in Task Scheduler.

**Phase 2 — dry `init`.** The module is initialized in `ModuleContext::new_diagnostic(id, InMemoryModuleDb)` in a separate thread, under `catch_unwind` + `recv_timeout(2000 ms)`. Logs are captured by `capture_logs_unwind` and are not lost even on panic. On timeout the thread stays detached, without a blocking `join`.

**Phase 3 — `shutdown` in LIFO order**, with a 1500 ms budget.

**Phase 4 — cross-check of disk and registry.** A directory `modules/*/rust/Cargo.toml` that has no counterpart among the active modules is flagged as `orphan Rust module … missing from registry!` and fails the check.

Notes on `-env` that matter for accuracy: PE validation and version checks run in the module phase. The environment section is limited to checking that files are present.

### 3.5 Simulation (`-simulate`)

1. IPC ping + writing a marker into the log.
2. Health of `unidtk_logs.redb` (`log_stats`).
3. Audit of uncaught frontend exceptions (`logs level=ERROR`, sources `frontend`/`webview`).
4. Validation of the widget grid JSON config (`get_widget_config`).
5. A safe read-only poll of the VPN and Telegram runtimes.
6. A CDP run of the rendering functions in the hidden `simulation-sandbox` DOM: `renderClock`, `initVpnPanel`, `renderMusicPlayer`, `renderGlyphBar`, `renderTelegramWidget`, with timing.
7. A click on the live clock widget (expand/collapse) and a DOM audit for cards showing `"Failed to fetch"`.

Without a running app / `CDP`, the section degrades to `[INFO] CDP simulation unavailable (…)` instead of failing.

---

## 4. Commands in detail

### `invoke`
```bash
udtk invoke <cmd> [json-args]
```
Direct call of an IPC command. `json-args` is an object, for example:
```bash
udtk invoke logs '{"limit":10,"level":"ERROR"}'
udtk invoke vpn_status
```

### `logs`
```bash
udtk logs [--limit N] [--since ID] [--level LEVEL] [--source SOURCE]
```
The default is `--limit 50`. The source (`--source frontend|backend|…`) and the level are passed as strings.

### `gui`
```bash
udtk gui [--json] [--force] <step>...
# steps:
#   js:<expr>             — arbitrary JS, returns the value
#   click:<target>        — physical click (move → press → release)
#   type:<target>:<text>  — click to focus + Input.insertText
#   press:<key>           — key press: keyDown/rawKeyDown + keyUp with key,
#                            code, windowsVirtualKeyCode and modifiers;
#                            format press:<mod>+<key> (ctrl|shift|alt|meta);
#                            printable characters carry text and are inserted once;
#                            an unknown or non-ASCII key → refused without sending
#   tree                  — semantic tree of the harness (formerly `dom`)
#   wait:<ms>             — pause, at most 10 000 ms
```

It merges the former `dom` + `eval` + `act`. The key difference from them is **one CDP session for the whole batch**. Previously every call opened a new WebSocket, so a single click cost five handshakes. Now the handshake happens once, and the steps run over that one connection in sequence (at most 50 steps per batch).

The harness itself resolves the step target. The value `r<N>` is a stable reference that survives re-rendering. After it come `@testid`, a positional `id`, an accessible name, or a CSS selector. Coordinates are CSS pixels from `getBoundingClientRect()`, and they are passed **1:1**. Multiplying by `devicePixelRatio` would miss by a factor of 1.25–1.5×.

**Occlusion is a refusal, not a "warning".** If the center of the target is covered by another element, the step returns `ok: false`, `refused: "occluded"` and `covering: {tag, id, testid, ref}`. That gives the name of the element that covers it, and the click is not sent. `--force` lifts the restriction and clicks anyway.

Every step produces a receipt: `kind`, `ok`, `value` (for `js`/`tree`), `hit_point`, `covering`, `refused` (the refusal reason: `occluded`, `unknown-key`, `non-ascii`, `unknown-modifier`, `empty-key`), `key` (for `press`, an object `{key, code, vk, text, modifiers, event}`, where `event` is the event type actually sent: `keyDown` with text, or `rawKeyDown` without), `text` (for `type`), `log_delta` (the delta of the kernel journal from `log_delta`, one IPC round trip per step), `ui_errors` (page errors, the buffer is cleared) and `duration_ms`. `--json` returns `{ok, steps:[…]}`. In text mode it prints a line-by-line report, and any failure gives exit 1.

**`press:` emulates a US keyboard** and does not depend on the system layout. `code` is always physical (`KeyA`, `Digit1`), and `vk` is the virtual key code. Canonical names are strictly the DOM `key` values (`Escape`, `ArrowLeft`, `Enter`). Aliases such as `Esc`/`Left` are refused, so that each capability has exactly one name. Russian letters are not emulated (there is neither `code` nor `vk` for them). Text is entered with the `type:` step. Keys that used to be inert through the CLI (`Delete`, `Backspace`, `F2`, `F5`) now **really act** on the page. Do not run them in checks against live data.

### `shot`
```bash
udtk shot [path] [--json|-j] [-q|--quiet] [-o|--output <path>]
```
The default is `Pictures/Screenshots/unidtk_YYYY-MM-DD_HH-MM-SS.png`, resolved through `SHGetKnownFolderPath`, with fallbacks to `Pictures/Screenshots` and `%TEMP%`. The PNG signature and a minimum size (≥1 KiB) are checked. `-q` prints only the path. When CDP is unreachable, the hint distinguishes "the app is not running" from "the kernel is up, CDP is not".

### `help`
```bash
udtk help [--json]
```
Without the flag, it prints human-readable help. With `--json`, it prints the full machine manifest of all commands (15 entries), synchronized with the dispatcher and `help()`. Both forms produce identical text for a person. Previously JSON had its own command, `commands`. Now it is a flag.

### `tasks` / `autostart` / `path`

| Task | XML template | Description | Privileges |
|---|---|---|---|
| `UniDTK` | `UniDTKTask.xml` | Shell autostart at logon | Unelevated (`InteractiveToken`) |
| `UniDTKSingbox` | `UniDTKSingboxTask.xml` | Sing-Box TUN helper | Elevated (`HighestAvailable`) |
| `UniDTKVpnStop` | `UniDTKVpnStopTask.xml` | VPN stop helper | Elevated |

* `udtk tasks register [all|UniDTK|UniDTKSingbox|UniDTKVpnStop|autostart]` substitutes `@INSTALL_DIR@` and the current user into the UTF-16 XML, and registers the task through `schtasks`. If needed, it goes through UAC (`Start-Process … -Verb RunAs`). It requires `vpn-helper.exe` next to the app.
* `udtk tasks unregister <all|name>` / `cleanup-old` (stale tasks: `NothingWallpaper*`).
* `udtk autostart [status|enable|disable]` is a wrapper over the `UniDTK` task.
* `udtk path [status|ensure]` writes exactly one directory into the User PATH: the one that contains `udtk.exe` itself. No copies of the binary are made in the data directories.

### `build` / `dev` / `run`

* `build`: `cargo tauri build` → `cargo build --release --bins` → copying `libmpv-2.dll` and the ONNX weights into `target/release` → `prune_stale_resources` (removes modules that no longer exist in the sources, so that a second widget card does not appear).
* `dev [--debug|-d]`: with `--debug`, it lays out the DLLs and models. The app enables CDP itself and picks the port itself. The CLI does not set the port and does not extend the list of allowed origins.
* `run [args…] [--debug|-d]`: starts **only** `unidtk.exe` from the directory next to the CLI (`paths::install_dir()`), loads `.env`, and sets `CREATE_NO_WINDOW`. `--debug` only sets the markers `UNIDTK_DEBUG`/`SHELL_DEBUG`. The kernel owns the WebView2 arguments (including the CDP port). There is no search for "some" binary in the repository.

---

## 5. Splash (`udtk` with no arguments)

`opt/cli/splash.rs` plus the generated `opt/cli/splash_art.rs`. **Do not edit by hand.** Regeneration is `docs/unidtkascii.md` §5, and the check is `make_splash.py --repo <repo> --check`.

* **Four tiers** by width **and** height. The thresholds derive from the art width. `≥98` columns and height `≥29` give the XL emblem `48×21` (the splash is 27 lines). `≥86` gives `36×16`. `≥78` gives the compact `28×12`. Otherwise the emblem sits above the wordmark, with a ladder of truncations. `<46` gives the `UniDTK` line instead of the block wordmark, and `<29` gives text mode. The line budget is `min(28, height−2)`. The height comes from `srWindow` (overridden by `LINES`).
* Emblem: a metallic Ouroboros (titanium `#C3D0DE→#5A6678`), an icy Saturn (`#C6EEFF→#367CB0`) with rings. A protective sky mask keeps the serpent's colors from leaking onto the rings. On top of the geometry, the serpent's body is modulated by the byte `shade` (`128` is neutral, step `8`, range `80…176`). That is brightness taken from the original Ouroboros painting (`docs/unidtkascii.md` §4.7). The planet, the rings and the background are never modulated.
* The `UniDTK` wordmark: `Uni` in chrome, `DTK` in the colors of the German flag (graphite/red/gold).
* Color is enabled only on a TTY or with `FORCE_COLOR`/`CLICOLOR_FORCE`. It is disabled by `NO_COLOR` and `TERM=dumb`. The width comes from `COLUMNS` and the height from `LINES`. On Windows, it uses `GetConsoleScreenBufferInfo` plus a **verified** enabling of VT mode and CP65001. If that fails, the output is plain text instead of raw ESC sequences.
* The right column is aligned to the **dynamically computed** maximum width of the emblem (`display_width`), not to the constant `geo.cols`. Otherwise the stack's lines "slid" to the left.
* At the bottom is the command map: `control · telemetry · ui · system`. In narrow windows the list wraps with an indent of `13` columns. Previously the `system …` line broke below `57` columns.
* Tests (13 in `splash.rs`): all tiers fit the width **and** the height (`24`/`30`, widths `120…30`). Plain output has no escape sequences and matches the colored output after SGR is stripped. Planet, ring and body zones are isolated (the sum of the zones equals the block area). The rows, the `shade` values and `geo.cols` agree, and the alphabet is braille only. `shade` is neutral and quantized. The photo texture is present. The wordmark lines are 46 characters long. The right column is strictly aligned. The command map wraps. The XL tier is exceeded on a tall window.

---

## 6. Rules for modules (contract and inventory)

1. **No shadow test code in modules.** The `diagnostic_checks` method was removed from the `Module` trait. Diagnostics are declarative.
2. **Dependencies are declared as data**, not as code:
   ```rust
   fn dependencies(&self) -> &'static [manifest::ModuleDependency] { &[] }
   ```
   The types are `DependencySeverity::{Mandatory, Optional}` and `DependencyKind::{DynamicLibrary, Executable, ModelAsset, WindowsTask}`.
3. **A single registry.** A module is registered in `core/kernel/registry.rs` (`all_modules()`, `ALL_MODULE_IDS`) and wired into `toolchain/Cargo.toml`. Both the kernel and the CLI use the registry, so there is no duplicated list.
4. **Build linter for the inventory.** `toolchain/build.rs::lint_modules_integrity()` runs at build time and scans `../modules/*/rust/Cargo.toml`. A module on disk that is outside the registry/manifest fails the build with `BUILD INTEGRITY ERROR`.
5. **Zero disk pollution.** Diagnostics do not create data directories (`paths::data_dir_view()` is used instead of `data_dir()`) and run on `InMemoryModuleDb`.
6. **Safe life cycle.** `init` in diagnostic mode runs once and starts no background threads. `shutdown(budget)` fits within its budget. For the VPN helper this is critical at Windows shutdown.
7. **Panics are not lost.** `capture_logs_unwind` returns the logs accumulated before the panic.

---

## 7. Known discrepancies (technical debt)

* The environment section of `-env` checks only the presence of DLLs and binaries, without PE validation. PE validation lives in the module phases. Keep this in mind when reading the report.

The legacy name `shell` was removed from the CLI. The `usage:` lines, the `shot`/`cdp_get` hints, doc comments, test messages and the file header now say `udtk`/UniDTK. Only `shell32` (the Win32 API) and the env key `SHELL_DEBUG` are kept on purpose. The global `__SHELL_HARNESS__`, the `Shell*` tasks in `OBSOLETE_TASKS`, and the cleanup of the `%LOCALAPPDATA%\Shell` directory were removed together with the migration from that directory (session 131).

---

## 8. What changed on 17.09.2026 (sessions 58–60)

**Commit `c3c7070` — `feat(kernel,cli)`:**

* The hardcoded `udtk status` (and `udtk ping`) was removed. The aggregating function `status_snapshot()` in `core/kernel/ipc.rs` was eliminated, so that a new module would not force kernel edits.
* `doctor` was rewritten around a strict flag whitelist. `--app` was replaced by `-ping`. Later the redundant synonyms were cleaned out: only the `-x`/`--x` pairs remain (without `-ipc`, `-cargo`, `-sim`), and the duplicate command `udtk check` was removed in favor of `udtk doctor -check`.
* A declarative manifest `core/api/src/manifest.rs` and `Module::dependencies()` were added.
* `diagnostic_checks` was removed from the modules. `ModuleRunMode`, `ModuleContext::new_diagnostic`, `InMemoryModuleDb`, `capture_logs`, `capture_logs_unwind`, `CheckStatus`/`CheckResult` were introduced.
* Added `core/kernel/registry.rs` and the build linter `lint_modules_integrity()`.
* The CLI gained `validate_pe_x64`, `run_executable_probe`, `run_module_checks` (4 phases), `run_kernel_checks`, `run_ping_check`.
* Splash: a redesign of `splash_art.rs`/`splash.rs`. `UniDTK` in mixed case, `DTK` in the colors of the German flag, a titanium Ouroboros with scales, an isolated Saturn.

**Commit `afdd2e4` — `fix(cli,ui)`:**

* `side_by_side` computes the emblem width dynamically, so the right column is strictly aligned to a single axis. A regression test for the alignment was added.
* The paint clipping of the library popup was removed (`contain: none !important` for `.music-widget`). The popup no longer clips the header and the close button.

---

## 9. Live verification checklist

After any change to the CLI, the kernel, or the modules:

```bash
cargo test --bin udtk
cargo build --release --bins

udtk doctor -quick          # env + kernel + modules + ping
udtk doctor -all            # comprehensive audit
udtk doctor -unknown        # expect exit 2 and a flag error
udtk logs --level ERROR     # expect [] or the parsed causes
udtk shot                   # mandatory visual audit of the layout
```

`cargo test --workspace` is mandatory before handing off (currently 66 passed / 0 failed). Direct calls of `cargo check` / `node --check` that bypass the CLI bridge are prohibited. End-to-end diagnostics go through `udtk doctor` (including `udtk doctor -check`).

---

## 10. Plan: one command contract, two doors, and `udtk gui`

Status: **version 3**. This version comes after the critique by six subagents and two rounds of dialogue with a seventh. The outcome of the dialogue is §10.8, which must be executed. We start with slice S0 (§10.8.3).

### 10.1 The problem

The application's command surface is duplicated **three times**. All three lists are handwritten and drift apart:

1. `core/kernel/ipc.rs::route()`: a `match cmd` with about 20 arms. The kernel knows the module types (`vpn::VpnRuntime`, `telegram::TelegramRuntime`, `music::commands::AppState`).
2. The response of the IPC command `commands`: yet another list of the same names, synchronized by hand.
3. `#[tauri::command]` (about 30 functions in `core/kernel/main.rs`) plus the single `generate_handler!`: a second, independent surface for the web interface.

Separately, `doctor -simulate` contains a **fourth** handwritten list: five render functions hardcoded as a JS string in `opt/cli/main.rs`.

Adding a command in a module requires kernel edits in three places. This is the same anti-pattern that was eliminated in `status_snapshot` and `diagnostic_checks`.

### 10.2 Recorded decisions

- **`udtk gui`**: a single page-control command that merges `dom` + `eval` + `act`.
- **One contract, two doors, the front end entirely on our protocol (the owner's decision).** The Tauri command transport is removed, and the page goes through our door. Consequence: authentication of the door is **mandatory**. A loopback socket without a password gives no protection, and the WebSocket door is reachable from any web page (WS has no CORS). The adopted scheme: the CLI (Rust) uses a named pipe with a check of the client process (`GetNamedPipeClientProcessId` + the parent chain). The WebView uses a WebSocket on loopback with a token issued on each launch, a check of the origin, and a mandatory first handshake frame. Honest boundary: this protects against web pages and other users, but **not** against malware running as the same user, since the token file and the process memory are available to it. This is a deliberately accepted risk.
- **The manifest is a static build artifact, compiled into both the kernel and the CLI.** Materializing modules just to build the list is forbidden. `registry::all_modules()` in the kernel would produce second instances: a repeated VPN auto-connect, a second TDLib, a race with the deferred bootstrap of the player.
- **A module declares its metadata declaratively**, as a `macro_rules!` list in its own crate, following the pattern of `dependencies()`. The kernel and the CLI read it through a registry of function pointers, without instantiation.
- **The handwritten `#[tauri::command]` wrappers stay in the kernel.** They carry the Tauri injection (`State`/`AppHandle`) that a module cannot express. Only the list of names and the expansion of `generate_handler!` are generated.
- **One routing table** instead of three lists, plus a build check that "every declared spec has a handler".
- **Linker magic is not used** (`inventory`/`linkme`). On MSVC, `/OPT:REF` strips unreferenced sections.
- **We borrow techniques, not dependencies.** The reference points are `agent-browser` (Rust + CDP, element references taken from a snapshot) and Chrome DevTools MCP (targeted CDP queries instead of a full snapshot after every action).
- **Separation of layers.** Business logic lives in the modules. The kernel does transport, routing, and inventory. The CLI is a thin client. Logic does not settle into the CLI.

### 10.3 Module decisions and open questions

**Accepted:**

- **A module owns its runtime state.** The kernel stops naming module types (`VpnRuntime`, `TelegramRuntime`, `AppState`). The state lives in the module, and the handler fetches it itself. Accompanying requirement: `init` must be verifiable and cancellable, not "silently lose" a repeated `OnceLock::set`.
- **`metadata.json` is extended with the render entry points** (and added for `vpn`/`explorer`, where it is currently absent). `simulate` takes the list from there.

**Open:**

1. **The fate of plugins and assets.** Commands and events move to our protocol, but `fs`/`opener`/`path` and `convertFileSrc` (serving images) stay on Tauri. Whether to move them too is a separate stage C, a large decision.
2. **Who stops the running instance during an update** (NSIS hook / CLI command / updater). Currently `PREUNINSTALL` removes autostart, and it does so on upgrade too. The old process holds ports 26540/9222.
3. **Resource budgets**: the maximum number of steps in a `gui` batch, the lifetime of a CDP session, and the permissible growth of Working Set / idle CPU.

### 10.4 Stages

**Stage 0. The debug door: preserve and secure (owner's decision: CDP always on).**
CDP on 9222 stays enabled permanently. `udtk gui`/`shot`/`dom` run on it. Instead of a gate, the surface is reduced:

- **Remove `--remote-allow-origins=*` entirely, not narrow it to an origin.** The `Origin` header is chosen by the client itself, so an "allowed WebView origin" filters nothing. Any local client will send `http://tauri.localhost`. This is a control that looks like a control but is not one. Our CLI (`tungstenite::client_with_config`) sends no `Origin` and is not affected. Verified with a probe: an upgrade without `Origin` gives `101`.
- **Three points, not one:** `core/kernel/main.rs:1477`, `opt/cli/main.rs:1203` (`dev --debug`), `opt/cli/main.rs:1236` (`run --debug`). Otherwise the hole returns from the CLI.
- **The kernel stops overwriting `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`** (`core/kernel/main.rs:1480`). Today it erases what the CLI set, so the CLI arguments are dead. It must append its own arguments to the existing value.
- **Port and target discovery is inside stage 0, not "in the future".** A target is accepted only if the listening socket belongs to `msedgewebview2.exe` whose parent is `unidtk.exe` from `install_dir()`. Otherwise it is a hard error, not a warning. Otherwise `gui` would drive the window of an old or foreign process.

**Clarification of the boundary (verified by probes on the live app).** The "web page" class is closed **not** by this flag. It is closed by the `Host` check on DevTools (only IP/`localhost` is accepted; a request with `Host: evil.test` gets `500` before the target is looked up) and by the absence of CORS on `/json`. That means DNS rebinding against CDP fails regardless of `*`. The flag only widened the door for a client that **already knows the target UUID**. The class "local process of the same user" is not closed by anything: it connects and executes JS. **The security boundary here is the Windows user account.** The authentication of our door (§10.4 stages 1/7) is protocol hygiene, not a boundary, and cannot justify widening the write surface.

**In the future (a separate task, not in this plan):** remove the `dev` and `--debug` modes together with the variables `UNIDTK_DEBUG`/`SHELL_DEBUG`. Debugging is available at all times anyway, so a separate flag loses its meaning.

**Stage 1. Protocol envelope v1.**
`{"v":1,"id":N,"cmd":…,"args":…}` → `{"v":1,"id":N,"ok":…,"result"|"error":…}` + a frame type for push events. Mandatory server-side ceilings: the maximum size of a line/message, clamps on `limit`/`since`, a limit on the outgoing queue. In `ping`: `proto`, `capabilities[]`, `min_cli`. This removes the blocker "there are no versions at all": the installed CLI and the kernel update independently.

This stage also includes the **handshake**: a mandatory first frame that carries the token and has a hard timeout. No command is executed before it. There is also an origin check for the WebView. Without this stage, the front end cannot be moved to our door.

**Stage 2. Command manifest (no proc-macro and no scanning of sources).**
`CommandSpec { name, summary, args, access, requires_state, doors }` in `core/api`. Each module declares its own set in one `macro_rules!` list (one line per command). The registry enumerates `fn commands() -> &'static [CommandSpec]` through function pointers. The kernel and the CLI get the **same** compiled manifest. Build checks: duplicate names, missing description, collision with reserved names, "spec without a handler", and orphan modules (the existing `lint_modules_integrity`, with a fix to `rerun-if-changed` on `../modules`).

**Stage 3. One routing table in the kernel.**
`ipc.rs::route()` looks up the table, and the `match` is removed. The key point is to preserve the difference between **"the command does not exist"** and **"the module is not ready"** (currently these are different texts). Readiness is a separate field, not a fact of presence in the table. The Tauri wrappers stay handwritten, but their names are checked against the manifest at build time.

**Stage 4. The CLI reads the manifest.**
`udtk invoke` and `help --json` read from the shared manifest. This works even when the app is off, because the manifest is linked into the CLI. The response of the IPC command `commands` stays as a runtime reflection of the door (it honestly distinguishes doors and rights), but it stops being the source of truth.

**Stage 5. `udtk gui`.**
One CDP session per batch. **Ownership belongs to the scope of the CLI process**, with keepalive and one automatic reconnect. The tree snapshot is **one per batch**, with stable references. Target resolution is point-wise, without another full `getSemanticTree`. A receipt for each step: `kind`, `ok`, `value` | `hit_point`/`occluded`, `log_delta`, `ui_errors`, `duration_ms`. To avoid two connections per step, a new command `log_delta {since}` returns `(max_id, entries)` in one response. There is a limit on the batch by steps and by time. `dom`/`eval`/`act` are removed (with aliases during a transition period).

**Stage 6. `simulate` on the registry (rewrite it, do not "finish" it).**
- Remove the self-ping and the duplicates of `-ping`/`logs`/`-modules`. Simulate is responsible only for what the other sections do not cover, and it **does not re-initialize modules**.
- Remove the click on the **live** clock widget. Right now it fires the real `toggleWidget`, collapses a widget the user has opened, and races the animation timer (300 ms against a 60 ms pause). Instead, work with an offscreen copy or through the harness, with state save/restore.
- The sandbox must not be visible to global queries (`.widget-card`, the harness, `relayoutWidgets`). Isolate it, exclude it from selectors, and apply `contain: strict`.
- Render entry points come from the extended `metadata.json` (§10.3, "Accepted"), not from a string in the CLI. Record explicitly that the run is read-only: `save_widget_config` is not called during the run.

**Stage 7. The front end: migration to our protocol (radical option, the owner's decision).**
- First, an **inventory** of the dependencies on Tauri, as an artifact: commands, events (17+ subscriptions), the plugins `fs`/`opener`/`path`, and `convertFileSrc`.
- Fix the bypass immediately: `modules/explorer/ui/*.js` import Tauri by a **relative path**, bypassing the importmap. The facade would not intercept them.
- The facade is the single import point for commands, events, and (for now) plugins. A lint rule forbids direct imports of `@tauri-apps/*` outside the facade.
- The door: a WebSocket on loopback + a token per launch + an origin check + a first handshake frame (stage 1). The CLI door is a named pipe with a check of the client process.
- Migration **one command at a time** behind a runtime flag, keeping the working Tauri path until the phase passes. Rollback without a rebuild.
- The door must restore subscriptions after a reconnect. In Tauri the kernel is always alive. On our channel it is not, and otherwise events are lost silently.
- Authentication comes **before** write commands are enabled, not together with them.
- Plugins and assets stay on Tauri. That is a separate stage C (§10.3, open question 1).

**Stage 8. Acceptance and documents.**
Update `AGENTS.md` (§3.1/§3.4: `dom`/`act`/`eval` → `gui`; the status of `simulate`) and `docs/architecture.md`. Acceptance is checked against §10.5.

### 10.5 Acceptance criteria (measurable)

1. **Manifest completeness**: a test in the kernel iterates over the real routing table and compares it with the manifest. A mismatch fails the test.
2. **Door consistency**: `udtk help --json` == manifest == the runtime response of `commands`, in terms of the set of names. Differences by door/rights are permissible and must be flagged explicitly.
3. **Front end**: a snapshot test that the set of names in `invoke(...)` calls from `core/ui` and `modules/*/ui` is a subset of the manifest. This closes the risk that a rename silently breaks the front end.
4. **Offline**: with the app off, `udtk help --json` and `udtk doctor -modules` are green.
5. **`gui`**: a sequence of steps in one session, a log delta for each step, the number of CDP handshakes = 1 per batch (verified by a counter), and the number of IPC connections ≤ 2 per batch.
6. **`simulate`**: read-only (`save_widget_config` not called during the run; the layout state and `activeExpandedWidgetId` match before and after). No `[INFO] CDP simulation unavailable` while the app is running.
7. **Regressions**: `cargo test --workspace` is green, `udtk doctor -all` has no errors, and `utdk shot` shows no layout changes.

### 10.6 Risks

- **Security**: CDP is always on (the owner's decision). Slice S0 removed `--remote-allow-origins=*` (three places) and introduced port discovery. Residual risk, accepted knowingly: any local process can execute JS in the shell. **The boundary here is the Windows user account**, not a token and not a flag.
- **Version compatibility**: an old `udtk.exe` from the shipped install and a new kernel diverge independently. Without a protocol version this looks like "strange failures". Addressed by stage 1.
- **Resources of a persistent process**: a connection per request + a thread per connection. A `getSemanticTree` snapshot forces a layout and a hit-test on the WebView2 UI thread (the same wallpaper). Budgets: §10.3 item 5.
- **Threads and COM**: all tauri commands are declared `(async)`. A generated synchronous wrapper would freeze the UI (AGENTS §4.3). Socket threads do not initialize COM, so commands that work through COM/the clipboard must not be called from them. This must be recorded in the contract.
- **Shutdown**: a request in `block_on` during `app.exit(0)` leaves a torn redb transaction. A closing gate and a drain of `INFLIGHT` are needed.
- **Module life cycle**: an `init` timeout leaves the module alive without `shutdown`. A repeated `init` is ignored by `OnceLock::set`, so the module silently works with a foreign context.
- **Update**: `PREUNINSTALL` removes autostart on upgrade too. Busy ports and locked files in Program Files.
- **Scope creep**: ~30 tauri commands + ~20 IPC + calls in JS. Only in stages, one module at a time.

### 10.7 Outcome of the subagent critique

**Rejected from the first version of the plan:**
- A proc-macro `#[udtk_command]` plus scanning `modules/*/rust` in `build.rs`. These are two parsers of the same thing, mutually exclusive mechanisms. A substring scan catches comments and `cfg`-disabled code.
- Generating the bodies of the `#[tauri::command]` wrappers. Not feasible: the Tauri injection (`State`/`AppHandle`) is known only to the kernel, and `include!` from `OUT_DIR` breaks the IDE, rust-analyzer, and incremental builds.
- Building the table from `registry::all_modules()`. This creates second module instances in the kernel and double initialization of VPN/TDLib/the player.
- Exposing write commands on the socket "right away". This lowers the security perimeter without a threat model.
- The criterion "the single source of truth in `help --json` through the socket". It would make diagnostics depend on the running app.

**Accepted into the plan:**
- The manifest as a build artifact linked into the kernel and the CLI. Declaration in the module, without instantiation.
- The protocol envelope with a version, id, limits, and capabilities.
- Preserving the difference between "no command" and "not ready".
- Read-only by default for the socket, with closing CDP as a separate stage.
- `gui` with one session and one snapshot per batch, plus the `log_delta` command.
- Rewriting `simulate` (live click, sandbox isolation, no repeated init).
- An inventory of the Tauri dependencies and a facade before the front-end migration.
- Measurable acceptance criteria and explicit resource budgets.

**The owner's decisions from the discussion (recorded in §10.2–10.4):**
- The front end is moved entirely to our protocol (the radical option). Door authentication is therefore a mandatory stage, not an option. The accepted boundary (protection from web pages and other users, but not from malware of the same user) is recorded as a knowingly accepted risk.
- Module state moves into the modules themselves. This is the condition for a third-party developer to add a command without editing the kernel.
- `metadata.json` is extended with render entry points.
- Work starts with stage 0, which secures the debug door. CDP stays enabled always (the owner's decision), but `--remote-allow-origins=*` is removed, the port stops being hardcoded, and the `dev`/`--debug` modes are to be retired in the long run.

---

## 10.8 Consensus after a dialogue with a subagent

A two-round dialogue (positions → objections → consensus), with live probes against the running app. The outcome is recorded in the plan as mandatory.

### 10.8.1 Kernel and module types: narrow the wording of the goal

"The kernel stops naming module types" **is not achievable in full at stage 3**, and it should not be. The Tauri surface continues to name the types until stage 7.

Facts: in `core/kernel/main.rs` there are **31** `#[tauri::command]`, **93** `State<…>` parameter positions, and **9** `app.manage(...)` sites. In `core/kernel/ipc.rs` there is a separate, smaller surface (~20 arms).

**Decision:** §10.3 applies **to the routing surface**. From stage 3, `ipc.rs::route()` looks up a table where each arm is a module's `dispatch(&self, cmd, args)`, so the state type does not leave the module. The handwritten wrappers, `generate_handler!`, and `app.manage(module.runtime())` **remain** until stage 7, and are removed together with the Tauri transport.

**Consequence to be written into §10.1 and §10.7 in plain text:** "a command can be added without editing the kernel" is true **for our door**. For the Tauri surface it becomes true only after stage 7.

**Criteria:** S3 — a grep test that "there are no module type identifiers in `ipc.rs`". S7 — "there is no `State<…>` with a module type in `main.rs`, and no `app.manage` of module state".

**The rule "a module depends only on `unidtk-api`" is factually obsolete.** `tauri` is declared by 4 of the 5 modules, and the contract itself provokes this (`ModuleContext::host::<tauri::AppHandle>()`). **Owner's decision: the rule is narrowed** to the invariant that protects updatability: "a module **does not** depend on the kernel crate. `unidtk-api` is mandatory, and Tauri host types are permitted". Application: a rewording in `docs/architecture.md` §1 and in the header of `core/api`. The check is in `toolchain/build.rs::lint_modules_integrity()`, which fails on a dependency on the kernel crate in `modules/*/rust/Cargo.toml`. Do this as part of S2.

### 10.8.2 Manifest and route: where verification is possible and where it is not

The check "spec without a handler" **cannot be done at build time**. The proc-macro and source scanning were rejected, and `generate_handler!` is opaque to `build.rs`. So:

- the routing table of the socket door is **the only handwritten enumeration of names**;
- the build guarantees only what can be derived: duplicate names, reserved names, missing descriptions, orphan modules;
- "spec without a handler" and "table == manifest" are **test** guarantees, and the test must be non-degenerate (a mutation check: remove a handler → the test turns red);
- the macro pays off not as a description of commands, but as a way to keep `generate_handler!` and the name array from drifting apart.

### 10.8.3 Slices and gates

**S0** (CDP + discovery) → **S1** (`gui`) → **S2** (manifest + route + CLI) → **S3** (decoupling of types in `ipc.rs`) → **S4** (front end, one command at a time behind a flag) → **S5** (`simulate` + acceptance).

The gate on **each** slice: `cargo test --workspace` + `udtk doctor -all` + `udtk doctor -check` + `udtk shot` (the layout has not changed). In S0 a negative check is added: `rg -n -- '--remote-allow-origins'` must be empty **and** `udtk eval 1+1`, `udtk dom`, `udtk shot`, `udtk doctor -simulate` must be green. This is the proof that our client works without an `Origin` header.

### 10.8.4 What to borrow from `agent-browser` first

Techniques matter more than snapshot frequency. Our approach (one snapshot per batch + typed receipts + log delta) is not behind. Chrome DevTools MCP made the post-action snapshot **optional**, and `agent-browser` re-snapshots the tree only on failure. The borrowing order:

1. **Refusal on an occluded target, plus a report of what exactly occluded it.** This is the most valuable and the cheapest change (see §10.8.5 item 3).
2. **References instead of indexes:** a stable `data-udtk-ref`, not a positional number from the snapshot.
3. **An annotated screenshot** with boxes and references (`udtk shot`) as a replacement for the tree snapshot, when a picture is cheaper.
4. **Do not borrow the `agent-browser` daemon.** It contradicts §10.2 ("logic does not settle in the CLI") and moves session state into the CLI. Our contract is one batch per `udtk gui` run.

### 10.8.5 Found beyond the plan (live probes, DPI contract, harness)

**1. A direct contradiction in the documentation about DPI. It will hit S1.**
`docs/agent-e2e-and-ipc-plan.md` "Taboo #9" prescribes multiplying coordinates by `devicePixelRatio`. Section §5.3 of the same document says exactly the opposite: in CDP, `Input.dispatchMouseEvent` takes CSS pixels 1:1, and multiplication gives a miss of 1.25–1.5×. The code is right. There is no multiplication, and `opt/cli/main.rs:664-688` sends the coordinates from `getBoundingClientRect()` as they are. **Fix:** reword the taboo into "a ban on mixing input conventions". CDP takes CSS pixels without scaling. Native `SendInput` takes physical pixels and requires the dpr. Per-monitor-v2 DPI awareness is mandatory.

**2. The harness and `act` break their own "Taboo #3" (only `data-testid` and roles).**
`core/ui/harness.js` builds its selection by class (`.action-btn`, `.nav-btn`, `.track-row`, …) and exposes a `selector` of the form `.class`. `udtk act` accepts an arbitrary CSS selector. In addition, the `id` in the tree is **positional** (`nextId++`), so it is not stable between snapshots. Plan §10.4, stage 5, requires "stable references". **Fix:** expose outside only the locators `{testid}` / `{role+name}` / `{ref}`, plus a flag `fragile` if no stable one exists. Keep classes only for internal discovery. Set `data-udtk-ref` (it is removed together with the node). `act` by class only with an explicit `--allow-fragile`. The gate is a JS test: "the tree output contains no class locator".

**3. `occluded` is detected, but the click is sent anyway. The receipt lies (verified on the live app).**
`udtk dom` returned **41 elements, and all 41 are marked `[OCCLUDED]`**. `#explorerContainer` (`z-index: 2000`) is open on top, and the centers of the nodes land on other elements of Explorer itself. `act` reads the flag but **does not cancel** the click. It sends the events and returns `"status": "ok"` in the JSON receipt. The warning is printed only in text mode. Automation that reads the status sees success, while the press was received by Explorer. The check exists only for show. **Fix:** the receipt's `ok` is derived from the hit-test. With `occluded: true`, refuse without `--force`. Add `covering: {id, selector, surface}` to the receipt. Do not offer nodes under an active surface as targets. **S1 acceptance:** "a click on an occluded target → a refusal receipt with the name of the covering element".

---

## 10.9 Slice S0 completed (report)

**What changed in the code:**

* `core/kernel/main.rs`: `--remote-allow-origins=*` was removed. Added `pick_cdp_port()` (9222 if it is free, otherwise an ephemeral port), `merge_browser_args()` (its own WebView2 arguments are **appended** to the inherited ones, and the inherited `--remote-debugging-port`/`--remote-allow-origins` are discarded), and `cdp_port()`. The port is published in IPC.
* `core/kernel/ipc.rs`: `ping` returns `cdpPort`. The **write command `save_widget_config` was removed** from the route (and from the `commands` response). The door again matches the declared read-only model.
* `opt/cli/main.rs`: `--remote-allow-origins=*` was removed from `dev --debug` and `run --debug`. The CDP port is no longer hardcoded: `cdp_port()` asks the app for it and **does not substitute 9222 blindly**. `cdp_ws_url()` refuses to work if the target is not `http://tauri.localhost`. `run_simulation()` returns a failure flag, and `doctor` takes it into account.

**Verified live (release build, the app was actually restarted):**

| Check | Result |
|---|---|
| `utdk invoke ping` | `{"cdpPort": 9222, "pong": true}`. Discovery works |
| WebSocket with `Origin: http://evil.test` | **rejected** (before the fix it was `101`) |
| WebSocket without `Origin` (our CLI) | `101`, works |
| 9222 occupied by a foreign listener | the app took **54060**, and `utdk eval` worked through it |
| `save_widget_config` through IPC | `unknown cmd` |
| `dom` without the running app | a clear error, "cannot determine the CDP port", without a blind 9222 |
| `doctor -simulate` without the app | `FAILED (1 issue detected)`, exit 1 (previously it could give "ALL CHECKS PASSED") |
| `cargo test -p unidtk --bin udtk` | 10/10 |
| `utdk doctor -quick` | ALL CHECKS PASSED |
| `utdk shot` | layout is fine (checked visually) |

**Remarks from the independent reviewer (GPT-5.6 Sol), accepted for the next slices:**

- **Idempotency and request repetition** (S1/S4): `id` in the envelope is correlation, not protection against repeated execution. A `gui` reconnect, or a timeout after a command has actually executed, can repeat a side effect. Idempotency classes, deduplication by request-id, and a ban on automatic retries for non-idempotent commands are needed.
- **Events are not restored by resubscription** (S4): `Bus::last` stores only the last value, so events between a disconnect and a new subscription are lost. A monotonic sequence/cursor, `resumeFrom`, a gap signal, and a mandatory resync are needed.
- **TOCTOU in port selection** (S1): between the probe `TcpListener` and the actual bind, WebView2 leaves a window. A re-verifiable owner marker (PID/parent or a unique page identifier) and a refusal in case of uncertainty are needed.
- **A fourth list** (S2): if the routing table appears alongside separate `CommandSpec`s and handlers, the promise of "one table" will not hold. The unit of declaration must carry both the spec and the handler at once, and the manifest must be a projection of that structure.
- **Separating the inventory from the factories** (S2): the CLI currently pulls in `core/kernel/registry.rs::all_modules()`, which creates modules. The command inventory must live in static data, without calling `Module::init`.
- **The wording "build-time guarantee of a handler" is wrong.** It is a test guarantee (§10.8.2 already admits this). In the S2 gate, phrase it exactly that way.

**Environment (found along the way):** the running app holds `tdjson.dll`/`libcrypto`/`libssl`/`z.dll` from `target\release`, which makes the release build fail with `os error 32`. The order for a release: stop the app → `cargo build --release --bins` → start the app. Second: **do not run cargo through a PowerShell pipeline with `Select-Object`.** The output is cut off, the build process ends early, and the artifact silently stays old. This was caught twice as "build succeeded, binary is old".

---

## 10.10 Slice S1 completed: `udtk gui`

**What changed:**

* `opt/cli/main.rs`: the `gui` command. One CDP session for the whole batch (a `Cdp` structure with monotonic `id`s; events are ignored). Steps `js:` / `click:` / `type:` / `press:` / `tree` / `wait:`, up to 50 steps. A receipt for each step (`kind`, `ok`, `value`, `hit_point`, `covering`, `refused`, `text`/`key`, `log_delta`, `ui_errors`, `duration_ms`), `--json`, `--force`. The `dom`, `act`, `eval` commands were removed.
* `core/ui/harness.js`: stable references `data-udtk-ref` (field `ref`), a `covering` field on occluded nodes, class selectors removed from the output (the `fragile` flag is used instead), and `resolveTarget` understands `r<N>`.
* `core/kernel/ipc.rs`: the command `log_delta`, which returns the journal delta and the new watermark in one round.

**Verified live:** a batch of two `js` steps with receipts; `tree` returned 29 nodes, all with a `ref`, and 0 class locators; a click on `r1` with the menu open was refused with `refused: "occluded"` and `covering: div#menuOverlay (r30)`; `--force` clicks anyway; `cargo test -p unidtk --bin udtk` 10/10; `doctor -quick` PASSED; the layout is intact per the screenshot.

---

## 10.11 Slice S2 (partial): a unified command inventory

**What changed:**

* `core/api/src/commands.rs` (new): the types `CommandSpec`, `Access::{ReadOnly, Write}`, `Door::{Socket, Tauri}`. A write command by construction does not reach the socket. This is reflected in the contract tests. The rule has become **data**, not a default.
* `core/kernel/command_inventory.rs` (new): 21 commands with name, description, arguments, access, and doors. The file **compiles into both binaries**. The kernel serves from it, and the CLI `#[path]`-includes it and describes the surface even when the app is off.
* `core/kernel/ipc.rs`: a guard before the route. `route()` first looks the command up in the inventory. Not in the inventory → `unknown cmd`. In the inventory but write → a refusal with an explanation. The `log` branch was removed from the `match` (its role now belongs to the inventory), and the response to `commands` is built from the inventory.
* `opt/cli/main.rs`: `help --json` returns `protocol: 1`, `cli_commands` (the commands of `udtk` itself), and **`app_commands`**, the application surface from the shared inventory.

**Verified live:** `invoke commands` → 21 records with `access`/`doors`. `invoke log` → `command 'log' is not available on this door (write commands stay on the UI door)`, exit 1. `invoke save_widget_config` → `unknown cmd`. `invoke ping` and `gui js:` work. `help --json` shows 13 CLI commands and 21 application commands offline. `cargo test --workspace`: 0 failures.

**The honest boundary of what was done (remains for S3):**

* The handlers still live in the kernel's `match`. The names now come from the inventory (the guard makes it authoritative), but there is no "(spec, handler)" table yet. That is S3, together with decoupling the module types.
* A full test "inventory == route" requires a live server. Right now the mismatch is caught by the guard, not by a test.
- module commands are still listed in the kernel, not declared by the modules themselves. This depends on the decision "a module owns its state", adopted for S3;
- the module dependency rule has been narrowed. It is no longer "only `unidtk-api`" but "does not depend on the kernel crate; Tauri host types are permitted". This is recorded in `AGENTS.md` §1.1 and in the header of `core/api`. The lint check in `build.rs` has not been added yet.

---

## 10.12 Slice S1 addendum: Sol's reliability remarks

Taken into work, but not yet implemented. They are moved to S4, where the real door will appear:

- **Idempotency**: commands need a class (idempotent or not) and deduplication by `request-id`. Automatic retry of non-idempotent commands is forbidden.
- **Events**: `Bus::last` stores only the last value. A sequence/cursor, `resumeFrom`, and a mandatory resync after reconnection are needed.
- **Port TOCTOU**: between the probe `TcpListener` and the WebView2 bind, a window remains. A re-verifiable owner marker is needed (PID/parent, or a unique page identifier).

---

## 10.13 Slice S3, step 1: a route table instead of `match`

**What changed** (`core/kernel/ipc.rs`): the handwritten `match cmd` with ~20 arms was replaced by a **route table**:

- `type Handler = fn(&IpcServer, &Args) -> Result<Value, String>`. Each former "arm" became a separate function (`handle_ping`, `handle_logs`, `handle_vpn_status`, …).
- `static ROUTES: &[(&str, Handler)]` is the only enumeration of handlers. **The names are taken from the inventory constants** (`command_inventory::PING.name`), not from repeated strings, so a name is written in the code once.
- `route()` = the guard against the inventory → lookup in the table → call. If a name is declared but has no handler, the answer honestly reports a build inconsistency.

**Tests that make the table meaningful** (`ipc::route_tests`):

1. `every_socket_command_has_a_handler`: every socket command in the inventory has a handler. This is the missing "non-degenerate" check. If you add a spec without a handler, the test fails instead of a silent 404.
2. `every_handler_is_declared_in_the_inventory`: the reverse direction.
3. `route_names_are_unique`: there are no duplicates.

**Verified live** (release, the app was restarted): `invoke ping` → `cdpPort`; `invoke vpn_status` → real data through the table handler; `invoke log_max_id` and `invoke log_delta` → correct answers; `invoke log` → refused by policy; `gui js:6*7` → 42; `doctor -quick` → ALL CHECKS PASSED, exit 0; `cargo test --workspace` → 0 failures, all 15 suites green.

**What remains in S3 (step 2):** the kernel still names the module types. The handlers call `with_state::<vpn::VpnRuntime>` / `TelegramRuntime` / `AppState`. Decoupling requires moving the handling into the modules themselves:

1. A contract method `Module::ipc_call(&self, cmd, args) -> Option<Result<Value, String>>`, whose default implementation returns `None`.
2. The kernel holds **module instances**. Currently it creates them in `setup`, takes the runtimes through `app.manage(...)`, and loses the objects. A registry of `Arc<dyn Module>` available to `IpcServer` is needed.
3. Each module answers for its own commands itself, fetching its state through `ModuleContext::host::<AppHandle>()`.
4. A pilot on `glyph-bar` (one command, which needs no state), then `explorer`, `vpn`, `telegram`, `mp3-player`.
5. The gate: a grep test that "there are no module type identifiers in `ipc.rs`".

Assessment: this is the largest change to the plan after S4. It must not be done "by eye". First the pilot, then the rest, each with its own gate.

---

## 10.14 Slice S3, step 2: modules answer for their own commands (decoupling done)

**Mechanism.** The contract gains `HostHandle<'a> = &'a (dyn Any + Send + Sync)` and `ModuleIpcFn`, the signature of a module's handler. The kernel holds a table `MODULE_ROUTES: &[(&str, ModuleIpcFn)]` and addresses a module **by id**, passing an opaque host. The module fetches its own state itself (`downcast_ref::<AppHandle>()` + `try_state`). The contract stays free of Tauri types, and `glyph-bar` (which has no state for `volume_get`) stayed without a dependency on `tauri`.

**What was done:**

| Module | Commands | State |
|---|---|---|
| `glyph-bar` (pilot) | `volume_get` | not needed |
| `explorer` | `explorer_drives`, `explorer_places` | not needed (`serde_json` added to the dependencies) |
| `vpn` | `vpn_status`, `vpn_settings`, `vpn_list_servers` | `VpnRuntime` via host |
| `telegram` | `telegram_status`, `telegram_chats` | `TelegramRuntime` via host |
| `mp3-player` | `player_status`, `ai_dj_status`, `list_tracks`, `list_playlists`, `get_submodules` | `AppState` via host |

In the kernel, `run_player()` and `with_state()` were removed, together with the `tauri::Manager` import. The router no longer touches module state at all.

**Gate (an automatic test, not a promise):** `ipc::route_tests::router_does_not_name_module_state_types` reads the `ipc.rs` source through `include_str!` and fails if a module state type name appears there. The names are assembled at runtime (`["Vpn","Runtime"].concat()`), so that the test's own text does not satisfy the search.

**Verified live:** all **17** socket commands answer `exit=0`: `ping`, `logs`, `log_stats`, `get_widget_config`, `commands`, `volume_get`, `explorer_drives`, `explorer_places`, `vpn_status`, `vpn_settings`, `vpn_list_servers`, `telegram_status`, `telegram_chats`, `player_status`, `ai_dj_status`, `list_playlists`, `get_submodules`. `cargo test --workspace` — 0 failures.

**What this gave in substance:** adding a module command no longer requires kernel edits. It takes three steps: declare it in the inventory, implement it in `ipc_call` of the module's own crate, and add a line to `MODULE_ROUTES`. This is exactly the goal the owner set ("a third-party developer adds a command without editing the kernel"), reached without a proc-macro and without linker magic.

**Remaining:** S4 (the front end on our door: WS + token + handshake, idempotency and deduplication, event cursor/resync) and S5 (`simulate` + acceptance and documents).

---

## 10.15 Slice S4, step 1: protocol envelope v1

This is the first and mandatory piece of S4. Without a version and a request identifier, the door can neither be versioned nor can responses be matched. A mismatch between "installed CLI" and "new kernel" would look like random failures.

**What was done:**

- `core/kernel/ipc.rs`: `PROTOCOL = 1` and a single function `respond(id, outcome)` were introduced. The response now carries `v` and, if the request sent one, an **echo of `id`**. The envelope on input is **optional**: an old bridge that sends only `{cmd, args}` keeps working. This is the condition for a painless transition.
- `ping` reports `proto` and `capabilities: ["envelope-v1","log-delta","door-policy","gui"]`.
- `opt/cli/main.rs`: `ipc()` numbers requests (`AtomicU64`), sends `v` and `id`, and checks the response. On a mismatched `id` or a foreign protocol version it gives a clear error, "update both sides", rather than degrading silently.

**Verified live:** `invoke ping` → `proto: 1` plus the capability list. A raw request `{"v":1,"id":42,"cmd":"ping"}` gets the response `{"id":42,…, "v":1}`, so the echo is confirmed. Ordinary commands (`vpn_status`, `gui js:`) work. `doctor -quick` → exit 0. `cargo test --workspace` — 0 failures, no warnings.

**Remaining in S4** (a large part, in separate steps):

1. **WS door** for the page: `tungstenite` as a server, a mandatory first handshake frame with the token and a hard timeout, and an origin check. The token is issued per launch and reaches the page through `initialization_script`, not through a URL, a file, or the command line.
2. **Idempotency**: a command class (idempotent or not), deduplication by `request-id`, and a ban on automatic retry of non-idempotent commands. This is Sol's remark.
3. **Events**: sequence/cursor, `resumeFrom`, a gap signal, and a mandatory resync. Currently `Bus::last` stores only the last value, so a connection break loses events silently.
4. **Frontend migration** one command at a time, behind a runtime flag, keeping the working Tauri path and the ability to roll back. First come a dependency inventory and a fix for the importmap bypass in `explorer/ui/*.js`.

---

## 10.16 Slice S4, step 2: the WS door with handshake

**Why a second door.** The TCP door is for local tools, and by policy it is read-only. The page needs its own door, and that door is protected by a **token per launch**. The first frame must be a handshake with this token. No command is parsed before it, and a wrong token closes the connection immediately.

**What was done:**

- `core/kernel/ws.rs` (new): a server on `tungstenite` (an ephemeral loopback port), `HANDSHAKE_DEADLINE = 3 s`, `IDLE_TIMEOUT = 120 s`. The token (32 hex characters) is derived from an OS-seeded `RandomState`, so there is no new dependency, and it is regenerated on every launch.
- `ipc.rs`: the dispatcher was moved out into `IpcServer::dispatch(&Value) -> Result<Value,String>`. **Both doors go through it**, so a command cannot behave differently depending on how it arrived.
- `ping` publishes `wsPort` (the port is public knowledge), but **not the token**. This response travels over the unauthenticated door.
- The token is given to the page in exactly one place: the command `session_info` (in `generate_handler!`), plus the helper `harness.sessionInfo()`. It is in neither a file, nor a URL, nor the command line.

**Verified live:**

| Check | Result |
|---|---|
| WS without a handshake | `{"ok":false,"error":"handshake required: send {\"hello\":{\"token\":\"…\"}} as the first frame","v":1}` |
| `ping` | `wsPort: 51090`, `capabilities` includes `ws-door` |
| token read through the page (CDP) | 32 hex characters, the port from the page matches |
| WS with the token | `{"ok":true,"result":{"ready":true},"v":1}` |
| command over WS after the handshake | `{"id":7,"ok":true,"result":{"level":50,"muted":false},"v":1}`, which is `volume_get` with an echo of `id` |

`cargo test --workspace`: 0 failures.

**The boundary, named honestly:** the token blocks other local processes that do not know it, and any web page. It does **not** protect against code that is already running inside the process. CDP reads the token straight from the page, which is exactly how it was obtained in the check above. This is the accepted risk from §10.8. That is why the token does not cancel the read-only policy of the open door.

**Remaining in S4:** idempotency/deduplication, event cursor + resync, and the frontend migration itself, one command at a time behind a runtime flag. First come a dependency inventory and a fix for the importmap bypass.

---

## 10.17 Slice S4, step 3: the importmap bypass is closed, and a regression is forbidden by the build

**Problem (found by the earlier critique, confirmed in the code):** three Explorer files imported Tauri by a **relative path**, `../../assets/vendor/@tauri-apps/api/core.js`, bypassing the document's import map. Any facade or transport swap would not intercept such calls. Half of Explorer would keep calling Tauri directly, and the divergence of "the door" would show up at runtime, not at build time.

**What was done:**

- `modules/explorer/ui/{explorer,tabs,preview}.js`: the import was switched to the bare specifier `@tauri-apps/api/core`, exactly as `core/ui/*.js` already do. The mapping for it exists in `core/ui/index.html`.
- `toolchain/build.rs`: a new lint `lint_ui_imports()`. At build time it scans `core/ui` and `modules/explorer/ui`, and it **fails the build** if an import through `assets/vendor/@tauri-apps` reappears in UI JavaScript.

**Lint check (negative test):** I temporarily put the bypass back into `tabs.js`. The build failed with `UI IMPORT ERROR: ../modules/explorer/ui\tabs.js imports Tauri through the vendor path, bypassing the import map; use the bare specifier`. After the file was restored, the build is green. This is the proof that the gate works, not just that it exists.

**Verified live:** the release was rebuilt and started. The interface tree has 29 nodes, the same as before the edit. `harness.getErrors()` is empty, and `udtk logs --level ERROR` → `[]`. `cargo test --workspace`: 0 failures.

**The honest boundary:** the Explorer panel itself was **not opened** in this pass. It is not in the tree, because it is covered by the Telegram panel. So "explorer.js runs" is confirmed only indirectly: by the build, the lint, and a clean page load without errors. An attempt to check it with a dynamic import from `http://tauri.localhost/modules/...` failed on a wrong URL. The modules are not served that way, so that test proves nothing. Running Explorer live should be done in the next session: open a folder on the desktop and check `harness.getErrors()`.

**Remaining in S4:** idempotency/deduplication, event cursor + resync, and the frontend migration itself (the facade, plus moving one command at a time behind a runtime flag).

---

## 10.18 Slice S5, step 1: `simulate` no longer touches the live interface

**The main defect closed here.** `doctor -simulate` took the **live** clock widget (`document.querySelector('.widget-card[data-id="clock"]')`) and clicked it twice with 60 ms pauses. This was the real `toggleWidget`. It collapsed a widget the user had opened and raced the animation timer (300 ms against 60 ms), while writing `interaction:clickClockToggle — ok` into the receipt. In other words, a check that "does not destroy data" was in fact changing the state of the desktop.

**What was done:**

1. **The click on the live widget was removed.** The interaction is now checked on an **offscreen copy** inside the sandbox (`interaction:clickSandboxCopy`).
2. **The sandbox is isolated.** It no longer has the class `widget-card`. That class is what made it visible to `relayoutWidgets`, to the audit of broken cards, and to the semantic tree. `contain: strict` was added. A comment in the code explains why the class was removed deliberately.
3. **The harness does not see the sandbox.** `getSemanticTree` skips candidates inside `.simulation-sandbox`, so even if buttons are rendered during the run, the agent will not be offered them as targets.
4. The dead calls `ipc({"cmd":"log"})` were removed from the simulation. The command `log` is now a write command and is not served on the socket door, so these calls did nothing anyway.

**Verified live:**

| Check | Result |
|---|---|
| `doctor -simulate` | exit 0, `ALL CHECKS PASSED` |
| interaction receipt | `interaction:clickSandboxCopy — click dispatched on the offscreen copy` (there is no live click any more) |
| rendering of all five modules | `renderClock`, `initVpnPanel`, `renderMusicPlayer`, `renderGlyphBar`, `renderTelegramWidget`, all `OK` |
| interface tree after the run | 29 nodes, as before. The sandbox is **not** in the tree |
| page errors after the run | 0 |
| screenshot | layout intact, no trace of the sandbox |
| `cargo test --workspace` | 0 failures |

**Remaining in S5:** the render entry points are still listed as a JS string in the CLI (five functions). To remove this handwritten list as well, the modules' `metadata.json` must be extended with a field for the render entry point, and files must be created for `vpn`/`explorer`, where they are missing. This is a separate step, as §10.3 records. Acceptance per §10.5 is also ahead (criteria 3 and 4: the snapshot test of the `invoke(...)` names, and the offline check of `help --json`).

---

## 10.19 Acceptance per §10.5: criteria 3 and 4 closed

**Criterion 3, "the front end does not call what does not exist".** The test `frontend_contract_tests::frontend_invoke_names_are_registered` was added. It collects all literal names from `invoke("…")` in `core/ui` and in each `modules/*/ui`, and checks them against the list read from `generate_handler!` in `core/kernel/main.rs`. An unknown name is a test failure with the exact file and name. Plugin commands (`plugin:fs|…`) are skipped, since Tauri registers them.

Why a test and not the build: `generate_handler!` is a macro, and `build.rs` cannot read it structurally (§10.8.2). This is exactly the case where "the compiler will catch it" is not true.

**The test was checked for non-degeneracy, not just declared.** I temporarily added a call `invoke('command_that_does_not_exist')` to `core/ui/widgets-api.js`. The test failed with the message

```
the interface calls commands the kernel does not register:
…/core/ui/widgets-api.js: invoke("command_that_does_not_exist")
```

After the file was restored, the test is green again. I had done the same earlier with the import lint. The gate was proven by a negative test, not by the fact of its existence.

**Criterion 4, offline.** With the app stopped, `udtk help --json` returns `protocol: 1`, the 13 commands of `utdk` itself, and **21** application commands (from the shared inventory compiled into the CLI). `udtk doctor -modules` exits with 0. Then the app was started again, and `ping` confirmed `pong/proto/wsPort`.

**Final run:** `cargo test --workspace`: **15 suites, 0 failures, 0 warnings**. Among them, the three gate tests are green: `every_socket_command_has_a_handler`, `router_does_not_name_module_state_types`, `frontend_invoke_names_are_registered`.

**Remaining per plan:** in S4, idempotency/deduplication, event cursor + resync, and the front-end migration behind a flag. In S5, move the render entry points into `metadata.json`.

---

## 10.20 Migration step: an inventory of frontend dependencies and gates for events

The frontend migration starts with an inventory: what exactly will have to be moved. The inventory is also written up as a verifiable artifact, not as a list made "by eye".

**Inventory (four categories of dependency on Tauri):**

| Category | Where it lives | State |
|---|---|---|
| **Commands** (`invoke`) | `core/ui/*.js`, `modules/explorer/ui/*.js`, `modules/vpn/ui/vpn-panel.js`, `modules/telegram/ui/widget.js`, `modules/mp3-player/ui/audio-arcs.js` | machine-readable: test `frontend_invoke_names_are_registered` |
| **Events** (`listen`) | `core/ui/unidtk-core.js`, `modules/telegram/ui/widget.js`, `modules/glyph-bar/ui/widget.js`, etc. | machine-readable: test `frontend_listen_names_exist_in_rust` |
| **Plugins** | `@tauri-apps/plugin-fs`, `plugin-opener`, `api/path`, in `core/ui/widgets-api.js`, `unidtk-core.js`, `telegram/widget.js` | from the code; the migration is a separate stage C |
| **Assets** | `convertFileSrc` in `unidtk-core.js`, `telegram/widget.js`, `explorer/*.js` | from the code; the migration is a separate stage C |

Commands and events are now **checked by tests**, so the inventory cannot go stale silently. It is updated together with the code, or it breaks the run.

**A new guard for events.** `frontend_listen_names_exist_in_rust`: every literal name from `listen("…")` in the UI must occur as a string literal in the Rust code. Otherwise a renamed or removed event leaves its listener silent forever. This is the same class of silent breakage as with commands.

**The strength of the check, named honestly:** it proves that the **name exists** on the Rust side. It does not prove that the event is actually emitted on the required path. A text analysis cannot be stricter, because names are often passed as a separate argument (`emit(app, "telegram-message-new", payload)`, with the call split across lines), and some names are built dynamically. My first version of the test gave **10 false positives** for exactly this reason (`telegram-message-new`, `audio-bands`, `wallpaper-wheel`, and others). I checked each one in the code. All of them are emitted, just written differently. So the criterion was weakened to "the name is alive", and this is recorded in the test's comment.

**Checked with a negative test:** I renamed the listener `wallpaper-wheel` in `unidtk-core.js` to `wallpaper-wheel-renamed`. The test failed, naming the file and the name. After restoration it is green.

**Result:** `cargo test --workspace`: **15 suites, 0 failures, 0 warnings**. Four gate tests are green: `every_socket_command_has_a_handler`, `router_does_not_name_module_state_types`, `frontend_invoke_names_are_registered`, `frontend_listen_names_exist_in_rust`.

---

## 10.21 Idempotency became contract data

`Idempotency::{Safe, Unsafe}` and the field `CommandSpec::idempotency` were added to `unidtk_api::commands`. The value is set by the constructors: `read_only()` → `Safe` (reading twice is harmless), `write()` → `Unsafe` (the conservative side for writes). The field is published in both manifests, in the `commands` response and in `help --json`, under the name `retry_safe`.

Why it belongs in the contract and not in a comment: a door that can reconnect and retry must **know** whether a retry is safe. Otherwise a retry after a timeout, when the command actually executed, would duplicate the effect. This is Sol's remark about retries.

**Invariant under test:** `command_inventory::tests::socket_surface_is_retry_safe`. Everything exposed on the socket door must be `Safe`.

**Honest about the strength of the check:** right now the invariant holds **trivially**. The socket door has only read-only commands, so they are all `Safe`. The value is not in today's run but in the future. As soon as the first write command reaches the door (this will happen during the frontend migration), it must be explicitly marked, or the test will fail. This is exactly the check that will be needed in the next step.

`cargo test --workspace`: 15 suites, 0 failures. `socket_surface_is_retry_safe` is green.

---

## 10.22 An executable plan for the frontend migration (following a consultation)

The plan was drawn up from the actual code. It also recorded **four discrepancies** that must be fixed before the migration. They are visible in the sources:

1. **`dispatch` applies the socket policy to both doors.** So WS cannot currently serve write commands, because the policy is tied to `spec.on_socket()`, not to the door. Before the write cutover, the dispatcher needs an explicit door context (TCP: tools, read-only; WS: the authenticated UI door).
2. **The WS door supports only request/response.** There are no push events and no reconnection at all. This has to be designed, not "switched on".
3. **Some events bypass `unidtk_api::emit`.** These are direct `AppHandle::emit` calls (`core/kernel/input.rs`, `main.rs`, and the modules). So a single broker will not see them until they are moved to a kernel-owned fan-out.
4. **The response shapes diverge.** Over the socket, `list_tracks`/`list_playlists` return `{count, items}`, while the UI expects an array. Over the socket, `get_widget_config` returns `{}` instead of an `Option`. These commands must not be enabled on WS until the contract is unified.

### Order of steps

**1. Record the starting point.** Run `cargo test --workspace`, `udtk doctor -all`, `udtk shot`, `gui tree`, `harness.getErrors(true)`, and build a visual map (all widgets, the layout, glyph-bar, mp3, VPN, Telegram, Explorer, the menu). If the baseline is already unstable, do not start the migration. Otherwise an old defect cannot be told apart from a regression.

**2. The facade `core/ui/unidtk-bridge.js`, still entirely on Tauri.** It exports `invoke`, `listen`, `convertFileSrc`, `sessionInfo`, `appDataDir`/`resolveResource`, the fs functions, `openPath`, and the diagnostic `getTransportState()`. Internally it holds the real `tauriInvoke`/`tauriListen`. `session_info` **always** goes through `tauriInvoke`. Otherwise, with WS enabled, it would recurse. A separate key `@unidtk/bridge` is added to the import map. **The key `@tauri-apps/api/core` must not be redirected**, because the vendor plugins (`plugin-fs`, `plugin-opener`) import it themselves. All UI files move to `@unidtk/bridge`, and behavior does not change.

**3. The transport switch: `sessionStorage["unidtk.transport"]`** with the values `tauri` / `shadow` / `ws`. It is read once, when the module loads. It must not change in the middle of the page's life, because some subscriptions would remain on Tauri. No key → `tauri`. Do not use `localStorage` (a forgotten flag would survive a restart), and do not use env either (no access). Rollback: `sessionStorage.setItem('unidtk.transport','tauri'); location.reload()`, or simply restart the app.

**4. The WS client and session, without moving business commands.** One memoized session per load (`session_info` exactly once), one WebSocket, hello as the first frame, a table of pending requests keyed by `id`, validation of `v`/`id`/`result`-or-`error`, and separate timeouts for connect, handshake, and command. The token is kept in memory only. Unit tests: wrong token, command before hello, timeout, out-of-order responses, socket close with pending requests, incompatible version.

**5. The first four commands: `player_status`, `vpn_status`, `telegram_status`, `get_widget_config`.** All are read-only and retry-safe. Three have ready UI calls and visible indicators. `get_widget_config` checks WS early enough (the boot path). An explicit allowlist `WS_COMMANDS` means anything unknown does not go over WS. First comes the `shadow` mode: both transports are called, the UI gets the Tauri result, and any divergence is logged. Only after a clean run comes `ws`.

**6. Events are a separate protocol, not disguised as commands.** A single `EventBroker` lives in the kernel. The event envelope carries `type: event`, the name, a global monotonic `seq`, and the payload. Events fall into two classes. *state/latest* (`system-volume`, `vpn`, `telegram-connection-state`): on subscribe and on reconnect, the last snapshot is sent. *stream/edge* (Telegram new/edited/deleted/sent, `explorer-open`): a bounded ring with `seq` and replay. `audio-bands` carries only the last sample (coalesced), otherwise a slow WebView accumulates memory. On reconnect: if the ring covers the gap, replay by `seq`. If the cursor is older, send an explicit `gap` and a mandatory domain resync. The socket's reader and writer are separated (broker → bounded channel → writer). Subscriptions live in the facade and are recreated automatically, with backoff and jitter.

**7. The remaining read-only commands, in vertical slices** (explorer → glyph-bar → vpn → telegram → mp3-player). Each goes through: inventory → `ipc_call` → route → contract test → `shadow` → allowlist.

**8. Write commands, only after the read-only commands and events are stable.** They need a door context in the dispatcher, dedup by `request-id` (a bounded cache of completed responses), a ban on automatic retry with a new id, and a ban on a WS→Tauri fallback after a write request has been sent. Order: reversible settings → player control → VPN → Telegram/Explorer.

**9. Cutover:** default `ws`. `tauri` stays as a kill switch for the release cycle, and Tauri handlers are not deleted. Transport diagnostics (counters for reconnect, pending, and gap), plus a build gate: "the UI does not import Tauri directly, except through the facade and vendor".

### Three main risks

1. **Mismatch of Tauri and WS shapes.** Protection: `shadow` + structural comparison of JSON + contract tests. A name may not be enabled without a clean shadow run.
2. **Accidentally moving plugins/assets to WS.** Protection: a separate specifier. The facade imports vendor itself. `convertFileSrc`, fs, path, and opener stay on Tauri. The lint forbids bypassing the facade.
3. **Loss and duplication of events, repetition of destructive commands.** Protection: `seq`/ring/`gap`/resync, one ordering broker, dedup by id, no automatic retries.

### What not to do

Do not move plugins and assets. Do not enable `withGlobalTauri`. Do not redirect `@tauri-apps/api/core` in the import map. Do not delete Tauri handlers before the end of the rollback period. Do not open write on TCP 26540. Do not make a single global toggle without an allowlist. Do not retry writes without dedup. Do not keep the token outside memory. Do not change WorkerW/input/CDP for the sake of the transport.

### Questions whose answers change the plan

1. Does the nearest release end with read-only commands and events, or does write (Telegram, VPN, Explorer) also need to be in it?
2. Does Telegram need guaranteed replay of every message within a launch, or is a `gap` with a full reload of chat state acceptable?
3. Do we keep `sessionStorage`/CDP as an emergency switch in production, or is a user/CLI mechanism for choosing the transport needed, one that does not require CDP access?

---

## 10.23 Migration, steps 1–2: baseline and facade (behavior unchanged)

**Step 1, baseline recorded** (`%TEMP%\unidtk-baseline`): `cargo test --workspace` with 0 failures, `doctor -quick` with 0, a screenshot, the tree with **29 nodes**, and page errors **0**. Without this it would be impossible to tell an old defect from a migration regression.

**Step 2, the facade `core/ui/unidtk-bridge.js`.** The whole interface was switched to a single import, `@unidtk/bridge`:

- the facade re-exports exactly the surface that was in use: `invoke`, `listen`, `convertFileSrc`, `appDataDir`, `resolveResource`, `BaseDirectory`, `mkdir`, `readDir`, `readTextFile`, `writeTextFile`, `openPath`, plus `sessionInfo()` and the diagnostic `getTransportState()`;
- the real Tauri functions remain private inside (`tauriInvoke`, `tauriListen`). Otherwise the future WS path would recurse into itself;
- `session_info` always goes through `tauriInvoke`, not through the public `invoke`, for the same reason;
- the transport is read **once**, when the module loads (`?transport=` for a one-off probe, otherwise `sessionStorage["unidtk.transport"]`; values `tauri`/`shadow`/`ws`, default `tauri`). `localStorage` is deliberately not used, because a forgotten flag would survive a restart;
- the token is not stored anywhere: not in storage, not in the URL, not in the log;
- `WS_COMMANDS` is empty. A command gets there only after a clean shadow run.

13 files were migrated (20 import lines): `core/ui/{boot-log,harness,widgets-api,unidtk-core,menu}.js`, `modules/{explorer/ui/{explorer,tabs,preview},vpn/ui/vpn-panel,telegram/ui/widget,glyph-bar/ui/widget,mp3-player/ui/{widget,audio-arcs}}.js`. The key `@unidtk/bridge` was added to the import map. **The key `@tauri-apps/api/core` was not redirected**, because the vendor plugins import it themselves.

**The gates were extended.** `lint_ui_imports()` in `toolchain/build.rs` now scans **all** UI modules and fails the build if anyone imports Tauri directly, other than the facade itself. Checked with a negative test: I added a direct `@tauri-apps/api/core` import to `glyph-bar/ui/widget.js`. The build failed with `UI IMPORT ERROR: … imports Tauri directly; the interface must go through the bridge`. After restoration it is green.

**Verified live (release, the app was restarted):** the tree has **29 nodes** (baseline 29), page errors are **0** (baseline 0), and `getTransportState()` → `{transport: "tauri", migratedCommands: [], sessionInfoCalls: 0}`. The screenshot matches the baseline visually. `cargo test --workspace`: 15 suites, 0 failures.

**What this gives:** the transport has become replaceable in a single file, and the build protects this. The next step (the WS client and session, §10.22 item 4) no longer touches UI files, only the facade.

**Accepted defaults** (the owner did not object): in this release, read-only commands and events; write comes later. For Telegram, a `gap` with resync is allowed instead of guaranteed replay of every message. `sessionStorage` remains the emergency switch.

---


## 10.24 Migration, steps 3–5: flag, WS client, and first command migration

**Step 3, the transport flag.** `sessionStorage["unidtk.transport"]` (`tauri`/`shadow`/`ws`) is read once, when the module loads. `?transport=` is for a one-off probe. `localStorage` is deliberately not used.

**Step 4, the WS client and session** (inside the facade, UI files untouched): one memoized session per load, one socket, the handshake as the first frame, a table of pending requests keyed by `id`, validation of `v`/`id`/`result`-or-`error`, and separate timeouts. Closing the socket rejects all in-flight promises instead of leaving them hanging. The token lives only in the module's memory.

**Two real blockers were found and fixed:**

1. **The CSP did not let WS through at all.** `connect-src` in `toolchain/tauri.conf.json` allowed only `ws://127.0.0.1:26540` (the TCP door), while the WS door is ephemeral. So the page could not connect (`ws connection failed`). `ws://127.0.0.1:*` was added. Without this, the whole migration would have hit the policy rather than the code.
2. **The comparison of responses was too strict.** The first shadow run gave **16 "divergences" out of 17**, and all of them were only **key order**. Tauri writes fields in declaration order, while the socket path goes through `serde_json::Value` with a sorted map. The comparison was replaced by a structural one (`sameShape`, key order ignored). Otherwise a real defect would have drowned in this noise.

In addition, the handshake got more headroom (8 s instead of 3, because on a cold page the first call timed out with `telegram_status: ws handshake timed out`). The socket is also warmed up when a non-tauri transport is enabled.

**Step 5, the first three commands** (`player_status`, `vpn_status`, `telegram_status`; all of them are polled by widgets): `shadow` first, then `ws`. `get_widget_config` was deliberately left aside. Its contract diverges (`{}` against a missing value), and this must be unified before it is enabled.

**Instruments, not promises:** the facade counts `shadowChecks`, `shadowDivergences`, `shadowFailures`, and `wsCalls`, and keeps up to three samples of divergence with both responses. Everything is visible through `gui js:window.__UNIDTK_BRIDGE__.getTransportState()`, without reading the console.

**Live check:**

| Check | Result |
|---|---|
| `shadow` (19 comparisons) | **0 divergences, 0 refusals** |
| `ws` | `wsState: ready`, **18 commands** answered by the socket door, 0 page errors |
| interface parity | 29 tree nodes and 0 page errors in both modes |
| screenshot | layout identical to the baseline |
| rollback (`sessionStorage → tauri`) | `transport: tauri`, `wsState: off`, 0 calls over WS, tree 29 |
| `cargo test --workspace` | 15 suites, 0 failures |
| `udtk doctor -quick` | exit 0 |

**What this means:** the transport switches with one line in `sessionStorage`. The first trio of commands really goes through our authenticated door, and the rollback to Tauri is instant and verified. The next step in the plan is events (a single `EventBroker` with `seq`, a ring, and resync), and then the remaining read-only commands as vertical slices.

---

## 10.25 Migration, step 7 (first slice): six commands on our door

**Extending the allowlist to six:** `player_status`, `vpn_status`, `vpn_list_servers`, `vpn_settings`, `telegram_status`, `ai_dj_status`. They were selected on two criteria: the name in the inventory matches the name the UI calls, and the response shape is already identical on both doors.

**Deliberately deferred** (and why, so that it does not look like forgetfulness):

- `get_widget_config`: the socket handler answers `{}` where Tauri answers a missing value. The contract has to be unified first.
- `list_tracks` / `list_playlists`: the socket returns `{count, items}`, while the UI expects an array.
- `explorer_places`: the inventory has the name `explorer_places`, while the UI calls `get_user_places`. The name must be unified first.
- `volume_get`: the UI does not call it by name (volume arrives as the `system-volume` event), so there is nothing to migrate yet.

**A counter per command.** The first attempt to check six commands gave "23 comparisons, 0 divergences". That was **not proof**: of the six commands, only two were actually called. So a per-name counter (`perCommand`) was added to the facade, and the check now runs deliberately, through the same comparison path: `m.invoke('<cmd>')` from the page.

**Final live check:**

| Mode | Result |
|---|---|
| `shadow` | **54 comparisons, 0 divergences, 0 refusals**. Coverage by name: `player_status` 48, `telegram_status` 2, `vpn_status`/`vpn_list_servers`/`vpn_settings`/`ai_dj_status` 1 each |
| `ws` | `wsState: ready`, **25 calls** over the socket, the same per-name coverage, 0 divergences |
| interface parity | 29 tree nodes, 0 page errors in both modes |
| rollback | `sessionStorage.removeItem('unidtk.transport')` → `transport: tauri`, `wsState: off`, tree 29 |

**Conclusion:** six read-only commands are served by our authenticated door, each with a proven comparison against Tauri. All of it is switched off with one key in `sessionStorage`. The next slice in the plan is events (a single `EventBroker` with `seq`, a ring, and resync), then the remaining read-only commands after the contracts are unified.

---

## 10.26 The seventh command, contract unification, and two errors found by the shadow

**Unifying the contract of `get_widget_config`.** The Tauri command returns `Result<Option<Value>>`, that is, JSON `null` when there is no configuration. The socket handler answered `{}`. The two doors diverged, which is why the command could not be migrated. Fixed: the socket handler now returns `Value::Null`. After that the command was added to the allowlist (the seventh).

**Deliberately NOT migrated: `list_tracks`/`list_playlists`, and here is why (verified in the code, not by eye).** The Tauri commands accept `search`, `sort_key`, and `sort_dir`, while the socket handlers **ignore** these arguments and also return `{count, items}` instead of an array. Moving them now would silently break search and sorting in the library. First the contract (arguments and shape), then the migration. This reason is recorded directly in the comment on `WS_COMMANDS`.

**The shadow found two real errors. Both were fixed, not muffled:**

1. **False "refusals" caused by a too-tight timeout.** The first run gave **14 refusals** (`ws timeout for 'player_status'`) on a busy backend (a library scan was running). 4 seconds was self-inflicted: the kernel serves one connection sequentially, and a busy command legitimately takes seconds. The timeout was raised to 15 s, and a comment explains why it is that value.
2. **A race in my own client.** After the timeout was raised, 2 refusals remained with a different cause: `ws handshake timed out`. The cause was in `ensureSocket`. It cached the **socket**, not the promise, so two calls arriving during the handshake each opened their own connection (a leak and a lost response). Now the promise is cached, and `onclose` resets it.

The second error is exactly why the `shadow` mode exists. Without a comparison, it would have surfaced for the user as "the widget sometimes does not update".

**Final live check (seven commands):**

| Mode | Result |
|---|---|
| `shadow` | **34 comparisons, 0 divergences, 0 refusals**. `wsCalls = 34`, so every comparison was a real call over the socket, not a skip |
| coverage | `player_status` 26, `telegram_status` 2, `get_widget_config` 2, `vpn_status`/`vpn_list_servers`/`vpn_settings`/`ai_dj_status` 1 each |
| `ws` | `wsState: ready`, **30 calls** over the socket, the same coverage, 29 tree nodes, 0 page errors |
| rollback | key reset → `transport: tauri`, `wsState: off`, tree 29 |
| `cargo test --workspace` | 15 suites, 0 failures |
| screenshot | layout in order |

**Next per the plan:** events (a single `EventBroker` with `seq`/ring/resync, because right now WS does not push events at all). Then `list_tracks`/`list_playlists` after the arguments and shape are unified, and `explorer_places` after the name is unified.

---

## 10.27 The whole UI read surface is moved to our door (13 commands)

**Three contracts were unified. Without this, the migration would have been a breakage:**

1. **`telegram_chats`**: the socket returned `{count, items}`, while Tauri returns an array. It was changed to an array. Along the way, the `--limit` argument, which Tauri does not have, was removed from the description in the inventory.
2. **`get_drives` / `get_user_places`**: in the inventory these commands were called `explorer_drives` / `explorer_places`, while the UI (and Tauri) call them differently. The names in the inventory were changed to the ones both sides already use. One name works on both doors, and no aliases were added. The constants in the route registry refer to `.name`, so the handlers were renamed automatically.
3. **`list_tracks` / `list_playlists`**: the socket handlers ignored `search`/`sort_key`/`sort_dir` and returned `{count, items}`. Now they accept the same arguments and return an array, like Tauri. This would have been the most treacherous breakage: search and sorting in the library would have silently stopped working in WS mode only.

**Final live check (13 commands):**

| Check | Result |
|---|---|
| `shadow` | **49 comparisons, 0 divergences, 0 refusals**. `wsCalls = 49`, so every comparison was a real call over the socket |
| coverage | `player_status` 30, `get_user_places` 3, `telegram_chats` 3, `get_drives` 2, `get_widget_config` 2, `telegram_status` 2, the rest 1 each |
| `ws` + live scenario (open the menu and Explorer) | `wsState: ready`, **31 calls** over the socket, among them `get_drives`, `get_user_places`, `telegram_chats`. 29 tree nodes, 0 page errors |
| rollback | key reset → `transport: tauri`, `wsState: off`, tree 29 |
| `cargo test --workspace` | 15 suites, 0 failures |
| screenshot | layout in order |

**What is now on our door:** all read-only commands that the interface calls by name: `player_status`, `ai_dj_status`, `list_tracks`, `list_playlists`, `get_submodules`, `vpn_status`, `vpn_settings`, `vpn_list_servers`, `telegram_status`, `telegram_chats`, `get_drives`, `get_user_places`, `get_widget_config`. What remains is not a matter of principle: `volume_get` (the UI does not call it by name, since volume arrives as an event), and the write commands (by the adopted decision, after the events are stabilized).

**An honest assessment:** the command migration is complete, but it gives little while events bypass the door. Half of the interface lives on `listen`. So the next step is `EventBroker` with `seq`, a ring, and resync. It is larger than all the previous steps: it is a new protocol on both sides.

---

## 10.28 Events: the broker in the kernel, push over WS, and the first event in the interface

**The kernel, `core/kernel/events.rs` (new).** Events stopped being "just Tauri emit":

- **a global `seq`**: a client can say "I have seen 41" and receive exactly what comes after;
- **a ring per event** (64 frames) for a short reconnection;
- **state events** (`system-volume`, `audio-bands`, `vpn`, …) keep only the newest frame. Replaying 120 ms volume samples to a slow WebView is pointless and expensive;
- **an explicit `gap`**: if the ring no longer covers the cursor, the client gets a signal, not a silently incomplete sequence;
- delivery through `try_send`, so a slow client never blocks the kernel.

**One connection point.** The broker is fed from the same place where the kernel registers the emitter for Tauri (`main.rs`). Both doors carry **the same events with the same number**, rather than two parallel branches that have to be kept in sync.

**WS protocol:** `{"type":"subscribe","event":"…"[, "since": N]}` / `{"type":"unsubscribe", …}`. A push frame is `{v, type:"event", event, seq, payload}`. If replay is impossible, the server sends `{type:"gap", …}`. On a subscription without a cursor, the server immediately sends the newest value of the state event. The connection is served by one thread: a read with a 50 ms timeout, and between reads the event queue is drained. The socket stays a single object without locks.

**The facade:** `listen` for events from `WS_EVENTS` goes to the socket (registering local handlers, subscribing, unsubscribing). It keeps a **`seq` cursor per event**, and on a `gap` it resubscribes without a cursor. That is the resync.

**Three bugs found by the instrument (all in my code, all fixed):**

1. **Event comparison "by position"** gave false divergences. The doors are asynchronous, WS runs ahead, and I was matching different changes (`tauri=80` against `ws=50`). For **state** events the right semantics is convergence of values, not the order of frames. Now the newest socket value is compared 250 ms after a Tauri event. Stream events will need a separate rule, which is recorded in the code.
2. **A comparator per `listen`**: several widgets listen to one event, and each registered its own comparator, draining the same buffer twice. Now there is one comparator per event.
3. **A flaky broker test** (my own): the subscriber's channel receives all events, including publications from parallel tests, while I expected my frame first. The test now looks up its frame by name.

**Live check:**

| Check | Result |
|---|---|
| raw WS client | subscription → snapshot `seq: 1`; volume change → push `seq: 2` |
| `shadow` (event) | **34 comparisons, 0 divergences, 0 refusals** |
| `ws`: does the interface update on the event | glyph bar: **2 segments at 20 %, 9 at 90 %**. The bar reacted to an event delivered by our door |
| rollback | key reset → `tauri`, 29 nodes |
| `cargo test --workspace` | 15 suites, 0 failures (including 4 broker tests) |

**Known limits:** some events are published by direct `AppHandle::emit` calls (`core/kernel/input.rs`, `main.rs`, modules) and **do not reach** the broker. They must be moved to the contract emitter one domain at a time, and this is the next step. `WS_EVENTS` currently holds one event (`system-volume`). The others are added in the same order: "compare → enable".

---

## 10.29 All events go through the contract, and the code is free of warnings

**Moved to the contract** (`unidtk_api::emit` instead of `AppHandle::emit`). Now these events are seen by both Tauri and the broker:

- kernel: `explorer-open` (three points), `display-changed` (two), `hotkey-pressed`;
- telegram: one helper, `TelegramRuntime::emit`, closed all of its events at once (connection state, chat updates, messages). More than ten places were fixed in one change;
- player: `audio-bands`, `ai-dj-progress`, `sync-tasks`, `sync-auto`.

**A side effect, but an important one: the dead scaffolding was removed, not silenced.** Once emission moved to the contract, the player module no longer needed `AppHandle`. Removed: `app` from `AppState` (the field was deleted entirely), and `app` parameters from `start_url`, `start_all`, `start_stale`, `start_existing_playlist`, `start_playlist`, `run_task`, `run_yt_dlp`, `run_incremental_snapshot`, `run_yt_dlp_download`, `update`, `spawn_auto_playlist_sync`, `analyze_tracks`, `reanalyze_tracks`, `sync_url`, `sync_playlist`, `sync_all`, `audit::start`, and from the corresponding Tauri wrappers in the kernel. That is about fifteen parameters and one structural cycle. **`cargo check` now passes without a single warning.**

**Coalescing on the wire.** `audio-bands` is published about 30 times per second. For a state event, forwarding every frame is pointless: the WS writer collapses them to the newest value per pass (`events::is_state`). For stream events, order is preserved.

**Live check after the emission was moved:**

| Check | Result |
|---|---|
| `explorer-open` through the contract (a second instance with a path) | tree **29 → 39 nodes**, the Documents path is visible in the interface, 0 page errors |
| a **module** event in the broker | `telegram-connection-state` → `{"state":"Ready"}` with a real `seq` |
| `cargo test --workspace` | 15 suites, 0 failures |
| `cargo check --release --bins` | **0 warnings** |
| `udtk doctor -quick`, `udtk logs --level ERROR` | exit 0, no errors |
| screenshot | layout in order |

**Honest limits of this check:** `audio-bands` and `ai-dj-progress` are published only during playback and analysis. They cannot be brought up live without starting music and AI analysis, so they were verified through the common emitter path, not by observation. `sync-tasks` requires a real synchronization. All of them go through the same `unidtk_api::emit` as the verified events. The user's config was not changed during these experiments. The value of `visualizer_enabled` was `true` originally, verified against a backup copy.

---

## 10.30 Migration complete: the interface on its own door (shadow → ws)

**Result.** The interface works over the socket. **13 commands** (`get_drives`, `get_user_places`, `get_widget_config`, `player_status`, `telegram_chats`, `telegram_status`, and others) and **17 events** go through the new door. The untranslated ones (`get_folders`, `scan_directory`, `explorer_tabs_get`, `frontend_log`, `telegram_get_config`, `telegram_debug`) remain on Tauri, and this is visible in the per-door counters. The tree has 29 nodes, page errors are 0, and the screenshot is fine. The default transport remains `tauri`. `shadow` and `ws` are enabled with a flag in `sessionStorage`. It is cleared together with the WebView2 session, so a forgotten flag does not survive a restart.

**Comparison (shadow): 65 checks, 0 divergences, 0 refusals.** It was the comparison, not "seems to work", that produced two protocol improvements:

1. **The snapshot at subscription is marked explicitly.** On a subscription without a cursor, the server answers with the current value of the state event. That frame has no counterpart on the Tauri door. The comparison took it for a live event and reported a divergence that did not exist. Now the frame carries `"snapshot": true`, and the check compares only live deliveries. The fix is in `ws.rs` and the facade.
2. **Comparison by delivery, not by the last value.** The first runs gave 3–4 "divergences" for Telegram. Analysis showed that both doors delivered everything. The module sends events in batches (several user status changes, several messages per second), and the comparison was setting the "payload from Tauri" against the "last socket frame". That is two different messages from one batch (`from: "Yae Miko"` against `from: ".#GS"`, different `userId`s). The instrument was wrong, not the transport. Now the socket keeps a ring of the last 8 live payloads per event, and a divergence is counted only when **this** event is absent from the delivered ones.

**Side findings during the restoration of the build (both are real defects, not one-off failures):**

- **A cold build from scratch is impossible.** `build.rs` via `tauri_build::try_build` requires the resources from `tauri.conf.json` (`../target/release/udtk.exe`, `../target/release/vpn-helper.exe`) to already exist. With an empty `target/` this is a closed loop: to build `udtk`, the script requires `udtk`. The workaround is to create zero-length placeholder files, after which cargo overwrites them with the real binaries (verified: `udtk.exe` 28 MB, `vpn-helper.exe` 331 KB). The fix is an edit to build.rs: accept missing resources at `cargo build` time, and check them only at packaging.
- **The installed app can hold files from the repository.** The app from `C:\Program Files\UniDTK` held `tdjson.dll`, `libcrypto-3-x64.dll`, `libssl-3-x64.dll`, and `z.dll` **from the repository's `target/release`**. The path to the Telegram library is stored in the settings and points into the repository. Consequences: `cargo build` fails with `os error 32` without a clear file name, and after renaming the folder (`shell` → `unidtk`) Telegram breaks. Verified by searching the process's loaded modules. Before a release build, the app must be stopped.

**A lesson about resources (it cost a system freeze).** A full build with thin-LTO on this machine hits the **memory allocation limit**. The system reached 63.4 GB out of 63.7 GB, the page file swelled to 48 GB, after which rustc failed with `LLVM out of memory` and `memory allocation failed`, and Windows stopped responding. What helped: stop the app, build on **one thread** (`-j 1`), and give the compiler a large stack (`RUST_MIN_STACK=134217728`). Without the stack it failed on the huge `windows` crate after just 16 seconds. Result: 155 dependencies plus three binaries, `Finished release profile in 13m 55s`, without a single warning.

---

## 10.31 Stage 7 closed completely: writes, the door policy, and three manifest defects

The last blocker of stage 7 was closed (found when analyzing §10.22). **The dispatcher applied the socket policy to both doors**, so write commands did not pass anywhere. On the socket that was correct. On the authenticated WS door it was wrong. Without this fix the "migration" was half-done: the interface could not switch a VPN or send a message.

**What changed:**

- **The door became the dispatcher's context.** `dispatch(req, door)`: the TCP door passes `Door::Socket`, and WS passes `Door::Ui`. The connection has proven ownership of the token, and that is what makes it the interface door. The policy is taken from the contract data (`spec.doors`), not from one rule for all. The check was moved into a separate function, `check_door`, because an `IpcServer` cannot be created in a test (it needs a live `AppHandle`), while the policy is pure data.
- **The second door was renamed: `Door::Tauri` → `Door::Ui`.** The name describes **who is asking**, not the pipe. The interface transport is exactly what changes from Tauri to WebSocket, and the policy must not change with it.
- **The three refusals have distinct reasons, and they no longer lie:** "changes state, writes stay on the UI door", "served only by the tool door" (for `ping`/`logs`/`commands`), and "UI door only".
- **A new kind of command: `read_only_cli`.** Liveness, reading the journal, and the command list itself were declared as available to the interface, although the interface does not call them and the kernel does not register them. Now the manifest does not promise this.
- **Three manifest defects were fixed.** The journal write was declared as `log`, while the kernel registers `frontend_log`. The volume was declared as `volume_get`, while the kernel has `get_system_volume`. The `glyph-bar` module waited for the old name, so renaming without editing the module would have broken volume. CLI commands were presented as available to the interface.
- **The write command of the UI door got a route.** This was found by a **live probe after the tests were already green**: `frontend_log` arrived over WS at the dispatcher and got "declared in the inventory but has no handler". A Tauri command with the same name is a *different* transport, not a replacement. The test checked routes only for the socket subset, so it stayed silent.

**Guards that would prevent this from recurring:**

| Test | What it guarantees |
|---|---|
| `every_command_has_a_handler` (extended from the socket subset to all commands) | every declared command has a route, so at least one door can serve it |
| `every_manifest_ui_command_is_registered` (new) | every manifest command for the UI door is actually registered in the kernel. Previously the **manifest was not checked against anything** |
| `a_write_command_is_refused_on_the_socket_and_answered_on_the_ui`, `a_cli_only_read_is_not_offered_to_the_ui`, `an_unknown_command_is_refused_on_every_door` (new) | three different policy refusals, not one "just in case" |
| `a_read_command_is_served_on_both_doors` (fixed) | the command is selected **by property**, not by position. The first read command is the CLI command `ping` itself, and the index-based test asserted the opposite of the manifest |

**Live check after the build:**

| Probe | Result |
|---|---|
| write `frontend_log` over the **socket** | refused: "not available on the socket door: it changes state, and write commands stay on the UI door" |
| write `frontend_log` over the **WS door** | `ok`, and the write **landed in the journal**. `udtk logs --source frontend` shows `"запись через WS-дверь: проверка"` ("write via WS door: check") as the first line |
| read `get_system_volume` over WS | `{"level":0,"muted":true}` |
| CLI command `ping` over WS | refused: "served by the tool door only" |
| the old name `volume_get` | `unknown cmd` |
| `cargo test --release --workspace` | 0 failures (40 tests in the kernel), build `Finished in 14m 13s`, zero warnings |
| interface | 29 nodes, 0 errors |

**A note on the tool (not a code defect).** `udtk gui js:<expr>` takes JS as one argument. PowerShell 5.1, however, breaks the argument at line breaks and at double quotes inside it. A multi-line script turns into a dozen "steps" (`unknown step kind: write`). The working technique is single-line JS, with **single** quotes for the strings inside.

---

## 10.32 The first write from the interface on the new door (and why a write cannot be "compared")

**A trap found before the migration.** `shadow` (the comparison) works like this: **both** doors are called and the responses are compared. For reads this is correct. For writes it is not: the doors are not compared, but **the action is executed twice**. So the rule "we migrate a command only if the comparison is clean" cannot apply to writes at all. This had to be recorded before the first write entered the list.

**How it was done.** The facade has two separate lists:

- `WS_COMMANDS`: reads. They go to the socket in both `shadow` and `ws`, and the comparison is their check.
- `WS_WRITES`: writes. They go to the socket **only in `ws`**, while in `shadow` they stay on the old door. A write is checked not by comparison, but by its **consequence**, that is, a subsequent read of the state.

This is recorded in a comment in the facade itself, so that whoever adds a write to the list next does not fall into double execution.

**The pilot, `frontend_log`.** It was chosen because its consequence is harmless and directly observable: a journal entry. Also, it is exactly the command for which the live probe found the missing route (§10.31), so the route was tested too.

**Live check:**

| Mode | What was called | `tauri frontend_log` | `ws frontend_log` | Journal entries |
|---|---|---|---|---|
| `shadow` | `frontend_log` through the facade | 16 | 0 | **1** (no double execution) |
| `ws` | the same | 0 | 32 | **1** |

The interface in socket mode: 29 nodes, 0 errors, the layout is intact (screenshot `unidtk_2026-09-20_15-19-05.png`). After the check, the transport was returned to `tauri`. By default the app runs on the old door.

**What this gives.** The mechanism for migrating writes is ready and verified on a harmless command. From here on, live writes (VPN, Telegram, Explorer) are migrated by this scheme, one at a time: declare the write in the manifest, give it a route, add it to `WS_WRITES`, and verify it by consequence. Writes that are not yet in the manifest (`vpn_connect`, `telegram_send_message`, Explorer file operations) must first be added there. The manifest still describes only part of the UI door's surface.

---

## 10.33 The first live writes: volume is declared in the manifest and migrated

The `frontend_log` pilot was a **kernel** write. The next step is a **module** write, because the path is different there. The dispatcher calls the module by the name from the manifest, and the name must match in three places.

**What went into the manifest:** `set_system_volume` and `set_system_mute`, both `Access::Write`, so by construction they are unavailable on the socket door. Volume was chosen as the first live write because the module has no state (it stays Tauri-free), and its consequence is visible and reversible.

**What this required (a recipe for the other writes):**

1. The write in the manifest (`CommandSpec::write`). This is also what removes the command from the socket door.
2. A branch in the module's `ipc_call`. The **name must match the manifest**, otherwise the dispatcher reports "declared, but there is no handler".
3. A route in the kernel (`handle_*` → `call_module`), because over WS the command arrives at the dispatcher, not at its own Tauri wrapper.
4. The name in the facade's `WS_WRITES`, and only there, not in `WS_COMMANDS`. A write cannot be "compared" (§10.32).

**Live check:**

| Probe | Result |
|---|---|
| `set_system_volume` over the **socket** | refused: "changes state, and write commands stay on the UI door" |
| `set_system_volume {level:30}` over **WS** | response `{"level":30,"muted":false}` |
| the consequence: an independent read **over the socket** | `{"level":30,"muted":false}`. The state changed, not just "a response came back" |
| restoring the original state through **the same door** | `{level:0}` → `{"muted":false}`, then `set_system_mute {muted:true}` → `{"level":0,"muted":true}`. A read confirmed the original values |
| `cargo test --release --workspace` | 0 failures (40 in the kernel), build `Finished in 14m 27s`, zero warnings |
| interface after the flag reset | transport `tauri`, three transferable writes, 29 nodes, 0 errors |

**An observation on substance:** the `set_system_volume` write with a non-zero level **clears mute**. That is how the module works: "raised the volume, so it is not muted". It behaved exactly the same on the new door, so the command's behavior does not depend on the transport. That is what had to be proven. The user's state was restored exactly.

---

## 10.34 VPN writes: declared in the manifest and migrated

The second module with writes, and the first where a write **changes network state**, not volume. The VPN panel's interface calls four: `vpn_connect`, `vpn_disconnect`, `vpn_ping_all`, `vpn_set_auto_connect`. All four are declared `Access::Write` (the socket, by construction) and migrated to the new door.

**On `vpn_ping_all`, why it is a write even though it answers with measurements.** The command takes a global "measurement already running" flag (a second call during that time is honestly refused), and it saves the results. In other words, it changes observable state. `Access` describes what the command **does**, not what it replies with. Otherwise the door policy would stop being data and become a convention. This is recorded in a comment in the manifest so that the question does not come up again.

**Live check:**

| Probe | Result |
|---|---|
| `vpn_connect`, `vpn_disconnect`, `vpn_ping_all`, `vpn_set_auto_connect` over the **socket** | all four refused: "changes state, and write commands stay on the UI door" |
| `vpn_set_auto_connect {enabled:true}` through **WS** | response `auto_connect: true` |
| the consequence: an independent read of `vpn_settings` **over the socket** | `auto_connect: true` (the setting really changed) |
| return to `{enabled:false}` through **the same door** | response `auto_connect: false`. A read confirmed the original value |
| §10.5 acceptance, criterion 2 after the extension | manifest 27 commands (20 socket) **==** runtime (20 socket); 7 writes in the manifest |
| interface | 29 nodes, 0 errors, screenshot fine; transport returned to `tauri` |

**A lesson about tests (it cost one cycle).** Building the **test** targets is also heavy. `cargo test --release --workspace -j 2` brought down two LTO builds at once with `0xc000012d` (the memory allocation limit), even though the release binary had built normally. The tests pass at `-j 1`. So the rule "one thread on this machine" applies not only to the release build.

---

## 10.35 Doctor without hardcoding: `-env` from the manifest, `-simulate` from the inventory and `metadata.json`

The remainder of §10.1 is closed. The "fourth handwritten list" lived in `doctor`. The `-env` section duplicated the names of DLLs, EXEs, and models as a string in the CLI, and `-simulate` held six IPC commands and five JS render functions. Now both sections read data, not literals.

**What changed:**

- **`doctor -env` → `run_env_section()` (`opt/cli/main.rs`).** There are no longer any lists like `["libmpv-2.dll","tdjson.dll","sing-box.exe","vpn-helper.exe"]`, or the path `opt/models/discogs-maest-129e`, in the code. The section iterates `registry::all_modules()` → `Module::dependencies()`. For each `DynamicLibrary` / `Executable` / `ModelAsset` / `WindowsTask` it does a presence check through `paths::resolve_binary` / `resolve_asset` / `schtasks`. The severity from the manifest decides FAIL against WARN. Along the way it found `telegram:byedpi` (`ciadpi.exe`, Optional), which the old list did not declare, so the old hardcode simply did not see it. The `.env` block and `scheduled tasks` remained, since they are not module dependencies. In addition, there is a cross-check: a task from the manifest that is missing from `SCHEDULER_TASKS` yields `[WARN] … is not managed by 'udtk tasks'`.
- **`doctor -simulate`, steps 1–5 → the inventory (`core/kernel/command_inventory.rs`).** New `SIMULATE_PING` / `SIMULATE_LOG_STATS` / `SIMULATE_LOGS` / `SIMULATE_WIDGET_CONFIG` (alias constants for `.name`) and a table `SIMULATE_PROBES` (`vpn_status`, `telegram_status`, both read-only). `run_simulation`, `run_ping_check`, `cdp_port`, `logs_cmd`, and the `shot` hint refer to them. Renaming a command in the inventory no longer leaves dangling literals. The test `simulate_probes_name_real_socket_commands` checks that every probe names an existing read-only socket command with a valid JSON object of arguments.
- **`doctor -simulate`, step 6 (CDP) → `metadata.json` (`simulate`).** The entry points are declared by their owners. `modules/{clock,glyph-bar,mp3-player,telegram}/ui/metadata.json` received `simulate: {module, function, sandbox}`. Files were created for `vpn` and `explorer`. Explorer has no section, since it renders differently. This is normal: the absence of a section means "does not take part in the probe". The CLI (`simulate_targets()`) collects the entries from the checkout, `toolchain/dist`, or the install directory, sorts them by id, and generates one generic harness: a loop `import('../modules/<id>/<file>')` → `sandbox.innerHTML` → `await fn(sandbox, 'simulation-<id>')`. Values from the metadata are escaped into a single-quoted JS literal. An empty set gives `[WARN]`, not a silent success.
- **Build gate (`toolchain/build.rs::lint_simulate_metadata`).** It checks that each `simulate` section names an existing JS file, that the file defines the function, and that at least one section is declared. It also checks that the production code of `opt/cli/main.rs` (before `#[cfg(test)]`) has no literals `renderClock` / `initVpnPanel` / `renderMusicPlayer` / `renderGlyphBar` / `renderTelegramWidget`. The test assertions name them on purpose, and the lint cuts off the `#[cfg(test)]` tail. There is no serde in the build dependencies: the fields are extracted with a hand-written parser.
- **CLI tests:** `simulate_targets_come_from_metadata` (the five functions are found through discovery), and `env_section_lists_manifest_dependencies` (every `mod:dep:id` from the manifest appears in the `-env` report).

**Verified:** `cargo test --workspace -j 1` is all green (CLI 17/17, kernel 41/41, including the new `simulate_probes_name_real_socket_commands`). `cargo check --bins` has zero warnings. `cargo build --bins` (debug) plus `udtk doctor -env` gives `ALL CHECKS PASSED` with line-by-line `mod:dep:id`. `udtk doctor -unknown` exits with 2. A live `-simulate` with CDP was not run, because the app was not running. The next session with the app up must confirm the parity of the probes with the previous output.

**Honest limits:** `-env` is a presence-only twin of the deep `-modules` phase. PE validation and smoke probes live there, from the same manifest. `SCHEDULER_TASKS`/`OBSOLETE_TASKS` remain hardcoded on purpose. This is the CLI's knowledge about the installation, not about the modules, and the drift against the manifest is now visible as a WARN. `help --json` for `cli_commands` is still a handwritten array. The app half comes from the inventory.
