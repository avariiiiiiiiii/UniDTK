# UniDTK — Instructions for AI Agents (AGENTS.md)

> **Purpose of this document:** The working protocol for all AI agents: the mandatory 10-step cycle, live debugging through the CLI, and pointers to the rules in the `UniDTK` repository (Unified Desktop Täglich Kit).

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
   - **Study [`docs/status.md`](docs/status.md) mandatorily** — the single entry point: the current state and the table that says which document to read for which task. Reading the historical log `docs/sessions.md` in full at the start is forbidden; when context is needed, read only the latest sessions via a line slice.
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
    - **Documentation record:**
      1. **Session log ([`docs/sessions.md`](docs/sessions.md)):** Every session is recorded under the current date, strictly following the 5-paragraph standard:
         - *Paragraph 1 (Context and symptoms):* The essence of the task or the symptoms of the problem;
         - *Paragraph 2 (Root cause):* The proven primary cause at the level of Win32, redb, message queues, or DOM;
         - *Paragraph 3 (Architecture decision):* What was done conceptually and which rules (cited as `r[id]`) were kept;
         - *Paragraph 4 (Changed files):* A list of direct relative links to all created and changed files;
         - *Paragraph 5 (Verification and metrics):* Results of `udtk doctor`, IPC ping RTT (ms), Working Set RAM (MB), log status, and the `udtk shot` screenshot.
         - Add a line to the express index at the top of `docs/sessions.md` with the date and the Git commit SHA.
      2. **Status ([`docs/status.md`](docs/status.md)):** Update the current state and metrics, and the reading table when a document appears.
      3. **Reference ([`docs/reference/`](docs/reference/README.md)):** Change the chapter that owns a contract **only** if the task changed that contract, in the same commit. Day-by-day debugging does not go there.
      4. Teaching text, module pages, and the `udtk` book are corrected when the mechanism they describe changes. `udtk doc --check` must pass before the commit.

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
* `udtk path [status|ensure|remove]` — checks, registers or removes `udtk.exe` in the user's Windows `PATH`.
* `udtk doc` — renders the book (the pages in `docs/SUMMARY.md`) to HTML in the cache and opens it, as `rustup doc` opens the Rust documentation.
* `udtk doc --check` — from a checkout: broken links and anchors, pages missing from `SUMMARY.md`, unknown commands and rule identifiers, and code comments that name a missing document.

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
* Dev entry without a CLI command: `cd toolchain && cargo tauri dev`.

**Release build order (verified in practice).** Only `udtk build` builds and deploys. The application must be stopped before the files are replaced (otherwise `os error 32` without a file name); `udtk build` performs the stop itself. The build is heavy: thin LTO is memory-hungry, so the thread count is set in `.cargo/config.toml` (`[build] jobs`), and the compiler stack is `RUST_MIN_STACK=134217728`; `-j N` overrides via `CARGO_BUILD_JOBS`. Tests: `cargo test --release --workspace -j 1` (`-j 2` brings down two LTO builds at once, `0xc000012d`). On an **empty** `target/` the build is impossible without a workaround: `build.rs` requires `../target/release/udtk.exe` and `vpn-helper.exe` (fixed with empty placeholder files, which cargo then overwrites).

**The CLI in depth** (one contract, two doors, own protocol, transport flags `tauri`/`shadow`/`ws`) — [the `udtk` book](docs/udtk/README.md); the history of the migration is [docs/history/rfcs/udtk-migration.md](docs/history/rfcs/udtk-migration.md).

### 3.4 Visual and DOM control via CDP:
* `udtk gui tree` — a compact semantic tree of the interactive elements of WebView2.
* `udtk gui click:<target> | type:<target>:<text> | press:<key>` — physical input emulation with a receipt, a log delta, and a refusal when the target is covered.
* `udtk gui js:<expr>` — runs arbitrary JavaScript in the context of the active WebView2.
* `udtk shot [path] [--json]` — takes a screenshot of the screen via CDP (by default saves to Pictures/Screenshots with a local timestamp and prints the absolute path).
* `udtk cdp-list` — lists the active pages and CDP targets.
* `udtk help --json` — the machine-readable manifest of the CLI bridge commands.

---

## 4. Architecture rules

The rules live in [the reference](docs/reference/README.md), each under a stable identifier; this list only points at them. Read the chapter before touching what it governs, and cite the identifier in code comments and the session log.

| Area | Rules | Chapter |
| :--- | :--- | :--- |
| Kernel and startup | `kernel.core-invisible`, `kernel.menu-in-core`, `kernel.stable-api`, `kernel.no-blocking-main-thread`, `kernel.main-thread-once`, `repo.flat-root` | [kernel.md](docs/reference/kernel.md) |
| Modules | `modules.vertical-slice`, `modules.compiled-crates`, `modules.contract-only`, `modules.no-module-to-module`, `modules.commands-in-core`, `modules.idempotent`, `modules.lifecycle`, `modules.states`, `modules.metadata` | [modules.md](docs/reference/modules.md) |
| Storage | `storage.redb-only`, `storage.two-files`, `storage.no-raw-db`, `storage.log-queue` | [storage.md](docs/reference/storage.md) |
| IPC | `ipc.one-contract`, `ipc.socket-read-only`, `ipc.cdp-always-on` | [ipc.md](docs/reference/ipc.md) |
| Install | `install.program-files-read-only`, `install.data-in-localappdata`, `install.paths-in-one-place`, `install.no-absolute-paths`, `install.one-build-command`, `install.native-tasks`, `install.embed-or-bundle` | [install.md](docs/reference/install.md) |
| Desk and themes | `desk.positions`, `desk.relayout-not-reset`, `desk.folders`, `desk.no-blocking-dialogs`, `desk.nesting`, `theme.tokens-only`, `theme.module-contracts`, `theme.contrast`, `theme.per-theme-colors` | [desk.md](docs/reference/desk.md) |
| Motion | `motion.canonical-tokens`, `motion.reduced-motion`, `motion.no-hardcoded-ms`, `motion.clip-morph`, `motion.reversal`, `motion.lifo-stack`, `motion.widget-integrity`, `motion.shared-flight` | [motion.md](docs/reference/motion.md) |
| Documentation | `docs.english-low-level`, `docs.one-place` | [README.md](docs/reference/README.md) |

## 5. Documentation

The book is [README.md](README.md) and the pages in [docs/SUMMARY.md](docs/SUMMARY.md). Each kind of text has one home (`docs.one-place`): teaching in `docs/book/`, rules in `docs/reference/`, the CLI in `docs/udtk/`, modules in `docs/modules/`, internals and boards in `docs/dev/`, and the frozen record in `docs/sessions.md` and `docs/history/`. The writing standard is [docs/dev/doc-style.md](docs/dev/doc-style.md). A reference chapter stays under 350 lines, so it can be read in one tool call.

Session log entries, all documentation, and code comments are written in English.
