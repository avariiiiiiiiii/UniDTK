# Comprehensive Audit and Architecture Refactoring Plan: Core, Database, IPC, and Modules (docs/coremodfixes.md)

> **Document status:** Approved architecture specification (Session 23)  
> **Execution status (Session 43):**
> - **Phase 1 (Foundation, DB/logs)**: done. `ShellDatabase` was moved into a separate crate, `backend/crates/shell-db`, so the core no longer depends on music. Logs moved to a separate `shell_logs.redb`, with an async queue and batch writes. Also added `panic_hook` + `crash.log`, wired in `mimalloc`, and kept `mem-trimmer` on purpose. Dual-Storage was implemented as "data + logs". It does not move things into `backend/core/db`. Instead, a shared crate keeps music from pulling in the core.
> - **Phase 2 (data models / AI DJ)**: partly done. `store::global()` removed the race around `raw_db()`. `shell_ai_dj.redb` is opened once (OnceLock), and its path is in `%LOCALAPPDATA%`. Unifying `Playlist` (6 or 7 fields) is not needed, because there is no second structure in this crate anymore.
> - **Phase 3 (Win32, network protection)**: done for the VPN part. A native `vpn-helper.exe` (TerminateProcess under 3 ms) replaces VBS/PowerShell. The stop is synchronous in `ExitRequested` and on quit, and there is a full `WM_QUERYENDSESSION` / `WM_ENDSESSION` handler (a hidden top-level window). The Job Object, clipboard, and message filter were not touched.
> - **Phase 4 (IPC/CLI)**: postponed (separate session).
> **Goal:** Remove the monolithic dependency inversion between the Core and the Music module. Eliminate I/O degradation and mutual deadlocks in `redb`. Protect the network gateway during PC shutdown. Stabilize the IPC bridge, and prevent Win32 resource leaks.

---

## Contents
1. [Introduction and Background of This Revision](#1-introduction-and-background-of-this-revision)
2. [Identified Root Problems (Diagnostics)](#2-identified-root-problems-diagnostics)
   - 2.1. [Architectural Inversion: The Core Held Hostage by the Music Module](#21-architectural-inversion-the-core-held-hostage-by-the-music-module)
   - 2.2. [Pseudo-Modularity via the `#[path]` Directive](#22-pseudo-modularity-via-the-path-directive)
   - 2.3. [Critical Disk I/O Degradation and Deadlock Risk in the Logger](#23-critical-disk-io-degradation-and-deadlock-risk-in-the-logger)
   - 2.4. [Split DB Schema and Broken Bincode Compatibility](#24-split-db-schema-and-broken-bincode-compatibility)
   - 2.5. [AI DJ Data Loss in the System `%TEMP%` Folder](#25-ai-dj-data-loss-in-the-system-temp-folder)
   - 2.6. [IPC Server Vulnerabilities: Process Panic and Fake Statuses](#26-ipc-server-vulnerabilities-process-panic-and-fake-statuses)
   - 2.7. [Emergency Network Blackout on PC Shutdown](#27-emergency-network-blackout-on-pc-shutdown)
   - 2.8. [Win32 Handle Leaks and Lost Session Messages](#28-win32-handle-leaks-and-lost-session-messages)
3. [Results of the Cross-Review by 5 Critic Subagents](#3-results-of-the-cross-review-by-5-critic-subagents)
   - 3.1. [Critic 1: Thread Safety and Deadlocks](#31-critic-1-thread-safety-and-deadlocks)
   - 3.2. [Critic 2: Win32 API, Hooks, and Window Integration](#32-critic-2-win32-api-hooks-and-window-integration)
   - 3.3. [Critic 3: Memory, Performance, and Dual-Storage](#33-critic-3-memory-performance-and-dual-storage)
   - 3.4. [Critic 4: Frontend, DOM, and Data Migration](#34-critic-4-frontend-dom-and-data-migration)
   - 3.5. [Critic 5: Lifecycle, Cold Start, and Network](#35-critic-5-lifecycle-cold-start-and-network)
4. [Final 4-Phase Implementation Plan](#4-final-4-phase-implementation-plan)
   - [Phase 1: Core Foundation and Dual-Storage Architecture](#phase-1-core-foundation-and-dual-storage-architecture)
   - [Phase 2: Data Model Unification and AI DJ Rescue](#phase-2-data-model-unification-and-ai-dj-rescue)
   - [Phase 3: Win32 Resilience, Native Helper, and Gateway Protection](#phase-3-win32-resilience-native-helper-and-gateway-protection)
   - [Phase 4: IPC Bridge and CLI Contract Stabilization](#phase-4-ipc-bridge-and-cli-contract-stabilization)
5. [Risk Matrix and Verification Strategy](#5-risk-matrix-and-verification-strategy)

---

## 1. Introduction and Background of This Revision

As part of a technical-debt audit, and to prepare the codebase for long-term uninterrupted operation, we ran a deep analysis of the core subsystems (`backend/core`), the storage (`backend/modules/music/src/shell_db.rs`), the links between modules (`audio`, `explorer`, `music`, `telegram`, `volume`, `vpn`, `wallpaper`), the Win32 IPC mechanisms, and the interaction with the CLI (`backend/cli`).

The audit showed that, despite the microkernel modular architecture declared in `docs/architecture.md` and `AGENTS.md`, the codebase had grown into a monolithic tangle of hidden dependencies. There are critical reliability bugs that can crash the core when a TCP socket breaks, freeze the keyboard across all of Windows, lose user data when the OS cleans temporary files, and cut the internet on the user's computer during a normal Windows session shutdown.

---

## 2. Identified Root Problems (Diagnostics)

```
                       ARCHITECTURAL INVERSION IN THE CODE:

  [ Declared model (AGENTS.md) ]               [ Actual state in code ]
 ┌─────────────────────────────────┐           ┌─────────────────────────────┐
 │       CORE (SHELL KERNEL)       │           │      qbc-music-core         │
 │  - Database shell.redb          │           │  - shell_db.rs (single DB!) │
 │  - System logger                │           │  - logging.rs (core logger!)│
 │  - TCP IPC server               │           └──────────────▲──────────────┘
 └───────────────┬─────────────────┘                          │ cargo dependency
                 │ manages                                    │ (inverted!)
 ┌───────────────▼─────────────────┐           ┌──────────────┴──────────────┐
 │     APP MODULES                 │           │        CORE (main.rs)       │
 │  music, vpn, telegram, explorer │           │  Needs the player for the DB│
 └─────────────────────────────────┘           └─────────────────────────────┘
```

### 2.1. Architectural Inversion: The Core Held Hostage by the Music Module
- **Location:** [`backend/toolchain/Cargo.toml:18`](file:///C:/Users/admin/Documents/GitHub/shell/backend/toolchain/Cargo.toml#L18), [`backend/core/main.rs:20-21`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L20-L21), [`backend/modules/music/src/shell_db.rs`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/shell_db.rs).
- **Symptom:** The core's `Cargo.toml` manifest has no `redb` or `bincode` dependencies. The entire application database (`ShellDatabase`) is physically implemented inside the music module, `qbc-music-core`.
- **Root cause:** When SQLite was dropped, the developers rewrote the player's database on `redb`. To avoid extracting a separate base crate, they simply added the widget, Explorer, log, VPN, and Telegram tables inside the music crate.
- **Consequences:** The core cannot compile without the music module. Music stopped being a removable service and became the monolithic foundation under the core.

### 2.2. Pseudo-Modularity via the `#[path]` Directive
- **Location:** [`backend/core/main.rs:3-15`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L3-L15).
- **Symptom:** The modules `audio`, `explorer`, `telegram`, `volume`, `vpn`, and `wallpaper` are not included as independent Cargo packages of the workspace. They are included through direct compiler directives:
  ```rust
  #[path = "../modules/audio/audio.rs"] mod audio;
  #[path = "../modules/telegram/telegram.rs"] mod telegram;
  #[path = "../modules/vpn/vpn.rs"] mod vpn;
  // ...
  ```
- **Consequences:**
  1. The internals of all the modules are compiled into one flat namespace, `crate::`. Modules can call each other's private functions freely, ignoring the boundaries between subsystems.
  2. A direct cyclic dependency exists between `vpn.rs` and `telegram.rs`: `telegram.rs:866` calls `crate::vpn::is_connected()` directly, and `vpn.rs:1787` calls `crate::telegram::on_vpn_state_changed()`. To work around this, the code defines a static singleton, `TELEGRAM_FOR_VPN: OnceLock<Arc<Mutex<TelegramInner>>>`.

### 2.3. Critical Disk I/O Degradation and Deadlock Risk in the Logger
- **Location:** [`backend/core/main.rs:78-83`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L78-L83), [`backend/modules/music/src/logging.rs:77-82`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/logging.rs#L77-L82), [`backend/modules/music/src/shell_db.rs:668-682`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/shell_db.rs#L668-L682).
- **Symptom:** Every call to `log_line(...)` (hundreds of calls from Win32 window loops, TDLib events, and audio threads) synchronously runs:
  ```rust
  let txn = self.db.begin_write()?;
  // insert the record
  txn.commit()?; // SYNCHRONOUS FlushFileBuffers (fsync)
  ```
- **Consequences:**
  1. On Windows, `FlushFileBuffers` takes 5 to 30 ms per write. A batch of 10 log events blocks the calling worker or UI thread for 50 to 300 ms. `commit()` does not return until the disk confirms that the data is on it.
  2. `redb` allows **only one active write transaction per database file**. If a thread already holds `begin_write()` (for example, while syncing playlists or saving widgets) and calls `log_line()` inside that block, the second `begin_write()` waits for a lock that the same thread already holds. The thread waits on itself forever: a **self-deadlock**.

### 2.4. Split DB Schema and Broken Bincode Compatibility
- **Location:** [`backend/modules/music/src/shell_db.rs:146-154`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/shell_db.rs#L146-L154) vs [`backend/modules/music/src/core/db.rs:41-48`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/core/db.rs#L41-L48).
- **Symptom:**
  - In `shell_db.rs`, the `Playlist` struct has **7 fields** (including `pub first_track_id: Option<String>`).
  - In `core/db.rs`, the `Playlist` struct has **6 fields** (the `first_track_id` field is missing).
  - Both structs write to the same `PLAYLISTS` table through `bincode::serialize`.
- **Consequences:** `bincode` does not serialize field names. It writes the fields purely by position. Reading a record that was saved by the 7-field struct through the 6-field struct produces a deserialization error. At `shell_db.rs:547`, this error is silently swallowed (`if let Ok(pl) = ...`). As a result, the user's playlists quietly disappear from the interface.
- **Track key mismatch:** In `core/db.rs:279`, the key is built with 8-digit zero padding: `{playlist_id}_{pos:08}`. In `shell_db.rs:584`, the key is saved as `{playlist_id}_{pos}`. In a byte-ordered B-tree, lexicographic order is broken: the track `"pl_10"` sorts before `"pl_2"`.

### 2.5. AI DJ Data Loss in the System `%TEMP%` Folder
- **Location:** [`backend/modules/music/src/core/player.rs:114`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/core/player.rs#L114), [`backend/modules/music/src/commands.rs:507, 532`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/music/src/commands.rs#L507).
- **Symptom:** AI DJ creates its local database at `%TEMP%\shell_ai_dj.redb`, ignoring `shell.redb` completely.
- **Consequences:** The Windows cleanup service (Storage Sense) regularly deletes files from `%TEMP%`. The computed neural embeddings, which need heavy resampling and ONNX inference for the whole music library, are destroyed for good.
- **Transaction overhead:** The method `store_embedding` (`ai_dj.rs:155-176`) runs 2 write transactions with 2 commits for each track. Processing 500 tracks produces 1,000 disk `fsync` calls!

### 2.6. IPC Server Vulnerabilities: Process Panic and Fake Statuses
- **Location:** [`backend/core/ipc.rs:69`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/ipc.rs#L69), [`backend/cli/src/main.rs:1447-1455`](file:///C:/Users/admin/Documents/GitHub/shell/backend/cli/src/main.rs#L1447-L1455).
- **Symptom:**
  1. `ipc.rs:69` calls `reader.get_mut().try_clone().unwrap()`. The release profile sets `panic = "abort"`. With that setting, a panic ends the process immediately, without unwinding. So if the client breaks the TCP connection unexpectedly (FIN/RST), or the process runs out of handles, the `.unwrap()` causes an instant panic and an **emergency crash of the whole `shell.exe` application**.
  2. The `IpcModule` trait was removed, and the commands `vpn_status` and `telegram_status` are missing from the core's IPC dispatcher.
  3. The CLI diagnostic `shell doctor --simulate` catches the `unknown cmd` error and falsely reports to the operator: `[INFO] Module VPN: (idle or offline)`.
  4. The `shell logs` command in the CLI ignores the `--level` and `--source` flags completely. It returns the unfiltered raw stream.

### 2.7. Emergency Network Blackout on PC Shutdown
- **Location:** [`backend/modules/vpn/vpn.rs:2035-2051`](file:///C:/Users/admin/Documents/GitHub/shell/backend/modules/vpn/vpn.rs#L2035-L2051), [`backend/core/main.rs:1230-1246`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L1230-L1246).
- **Symptom:** The function `vpn::shutdown()` calls `perform_stop()`, which starts the background task `ShellVpnStop` through `schtasks`. It does **not wait** for that task to finish (`wait_until_stopped()` is missing). The `shell.exe` process exits in 15 ms.
- **Catastrophic scenario:**
  1. The task `ShellVpnStop` starts a chain: `wscript.exe` $\to$ `vpn_hidden_launch.vbs` $\to$ `powershell.exe -File vpn_stop.ps1`.
  2. Initializing the .NET CLR and filtering WMI (`Get-CimInstance Win32_Process`) takes **2 to 4 seconds**.
  3. When Windows shuts down or restarts, it gives processes a 5-second limit (`WaitToKillAppTimeout`), after which it kills them. The Task Scheduler and WMI services are shut down in parallel.
  4. The stop script does not get to finish, or it fails because WMI is unavailable.
  5. The elevated `sing-box.exe` process keeps running in the system. The `wallpaper-tun` adapter (Wintun) holds the default route `0.0.0.0/0` and strict WFP filtering rules (`strict_route: true`).
  6. **Outcome:** The control shell is closed, and **the internet on the user's computer is completely blocked**.

### 2.8. Win32 Handle Leaks and Lost Session Messages
- **Location:** [`backend/core/main.rs:661-675`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L661-L675), [`backend/core/main.rs:118-132`](file:///C:/Users/admin/Documents/GitHub/shell/backend/core/main.rs#L118-L132).
- **Symptom:**
  1. The handle of the global keyboard hook `WH_KEYBOARD_LL` (`SetWindowsHookExW`) is not saved, and it is **never removed** with `UnhookWindowsHookEx`. The hook procedure `win_e_proc` calls the synchronous `log_line`. If the disk is slow for longer than `LowLevelHooksTimeout` (200 ms), this can freeze the keyboard for the whole OS.
  2. In `set_clipboard_text`, when `SetClipboardData` fails, the allocated `HGLOBAL` memory is not freed with `GlobalFree`, which leaks it.
  3. The shell window is attached as a child (`WS_CHILD`) to `WorkerW`. Windows with the `WS_CHILD` flag **do not receive** system-wide broadcast messages: `WM_QUERYENDSESSION`, `WM_ENDSESSION`, and `TaskbarCreated`. The shell is blind to OS shutdown and to Explorer crashes.

---

## 3. Results of the Cross-Review by 5 Critic Subagents

Five specialized critic subagents reviewed the original refactoring plan. Their findings removed several dangerous hidden defects from the design.

### 3.1. Critic 1: Thread Safety and Deadlocks
- **Silent crash under `panic = "abort"`:** When the application crashes, Rust destructors (`Drop`) do not run. Every log message still waiting in the in-memory `mpsc` queue is lost for good. The next start has no record of the fatal error in the logs.
  - *Resolution:* a global `std::panic::set_hook` that writes the critical stack trace synchronously to a crash file, `%LOCALAPPDATA%\Shell\crash.log`, followed by `file.sync_all()`.
- **Log queue overflow (OOM and UI freeze):** During a log storm, an unbounded channel causes OOM, and a blocking `send()` freezes the UI.
  - *Resolution:* a bounded `mpsc` buffer of **4096 elements**, with a non-blocking `try_send()` only, and an atomic loss counter `LOGS_DROPPED: AtomicU64`.
- **Durability levels in `redb`:** `Durability::Eventual` cannot be used for all data.
  - *Resolution:* strict separation. `Eventual` is allowed **only for logs**. Every user change (widgets, tabs, settings) is saved strictly with **`Durability::Immediate`**.
- **VPN ↔ Telegram state flapping:**
  - *Resolution:* switch to an asynchronous `tokio::sync::broadcast`, with a monotonic generation counter `seq: u64` attached to each event. The counter discards stale events.

### 3.2. Critic 2: Win32 API, Hooks, and Window Integration
- **`TaskbarCreated` is invisible to `WS_CHILD` windows:** The Explorer restart message is sent only to top-level windows (`HWND_BROADCAST`). The shell's child window never receives it.
  - *Resolution:* create a separate, lightweight, invisible top-level listener window (a Message-Only Listener), register the message, and call `ChangeWindowMessageFilterEx`.
- **Explorer crash loop (re-attach storm):** Repeated Explorer crashes would produce a flood of `0x052C` messages and could crash the WebView2 GPU process.
  - *Resolution:* a **1-second debounce** and a **Circuit Breaker** mechanism (at most 3 re-attach attempts in 30 seconds).
- **Separation of Windows Job Objects:** An unprivileged `shell.exe` cannot add processes started through an elevated Task Scheduler task into its own Job Object (`ERROR_ACCESS_DENIED`).
  - *Resolution:* a Job Object with the flag `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is used **only for the local child process** of Telegram, `xray.exe`. It is started with `CREATE_SUSPENDED` and with handle inheritance turned off. Elevated processes are controlled through the native scheduler.
- **RAII protection for the clipboard:**
  - *Resolution:* a double guard, `ClipboardGuard` + `HGlobalGuard`, with a `disown()` method. The retry loop uses exponential backoff, to compete with the `Win+V` service.

### 3.3. Critic 3: Memory, Performance, and Dual-Storage
- **Catastrophic database bloat from logs (Copy-on-Write B-tree bloat):** Logs are high-frequency data that are constantly rotated. In `redb`, deleted pages are not returned to the file system without `compact()`. Storing logs in the shared database would grow `shell.redb` to 500+ MB, slow the cold start, and cause a storm of page faults.
  - *Resolution:* **Dual-Storage architecture.** `shell.redb` holds only clean ACID data (configs, accounts, tracks, playlists). Logs move to a separate file, `shell_logs.redb`, with a strict rotation limit.
- **Stop the eager loading of AI DJ data:** Loading 20,000 vectors into memory at startup takes 60 to 90 MB of RAM.
  - *Resolution:* zero-copy storage of the vectors. Each is 400 `f32` values stored as raw bytes, exactly 1600 bytes (`bytemuck::cast_slice`), read on demand.
- **`mem-trimmer` is useless, and we switch to `mimalloc`:** The `SetProcessWorkingSetSize` call did not free WebView2's memory. It only trimmed the Rust process's working set, which caused micro-freezes.
  - *Resolution:* use `mimalloc` as the global Rust allocator (it fixes heap fragmentation on Windows during long uptimes). Also, a targeted V8 garbage collection in WebView2 through CDP when the session is locked (`WTS_SESSION_LOCK`).

### 3.4. Critic 4: Frontend, DOM, and Data Migration
- **Schema mismatch and the risk of losing Telegram/VPN data:** `shell_db.rs` declares truncated schemas. A naive overwrite would erase all saved account sessions.
  - *Resolution:* **Zero-Loss Migration Contract.** The source files `telegram_config.json` and `vpn_settings.json` are renamed to `.bak` only after the `redb` transaction has committed successfully and a roundtrip verification read has passed. Use canonical structs with `camelCase` names and `#[serde(default)]`.
- **Keeping input focus in `WorkerW`:** Updating statuses with `card.innerHTML = ...` is forbidden. In the desktop context, it resets the input focus of SMS codes and 2FA passwords.
  - *Resolution:* strictly targeted, in-place DOM mutations only (`classList.toggle`, `textContent`, `disabled`).

### 3.5. Critic 5: Lifecycle, Cold Start, and Network
- **Replace the heavy scripts with a native Rust helper:** VBScript and PowerShell (`vpn_stop.ps1`) take 2 to 4 seconds to run, which does not fit in the Windows shutdown limit.
  - *Resolution:* remove all `.vbs` and `.ps1` files. Add a native command, `shell vpn-helper-stop`, built on the Win32 API (`CreateToolhelp32Snapshot` / `TerminateProcess`), which terminates the processes in **under 3 ms**.
- **Intercept Windows shutdown (`WM_QUERYENDSESSION`):**
  - *Resolution:* handle the system shutdown in the listener window. Call `ShutdownBlockReasonCreate`, force the VPN to stop, and raise the process priority with `SetProcessShutdownParameters(0x280)`.
- **The blind autoconnect timer:** A 4-second timer does not wait long enough for Wi-Fi to initialize on an OS cold start (8 to 20 seconds).
  - *Resolution:* replace it with an asynchronous Network Liveness Watcher, which watches the availability of the network gateway.

---

## 4. Final 4-Phase Implementation Plan

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                               PHASE 1: FOUNDATION                               │
│  - Move ShellDatabase into backend/core/db (remove the dependency on music)     │
│  - Dual-Storage: shell.redb (ACID) + shell_logs.redb (async logs)               │
│  - Non-blocking logger: mpsc(4096) + panic hook writing crash.log               │
│  - Wire in the mimalloc allocator                                               │
├─────────────────────────────────────────────────────────────────────────────────┤
│                          PHASE 2: DATA MODELS AND AI DJ                         │
│  - Unify Playlist (metadata in JSON, 8-digit padding of track keys)             │
│  - Zero-Copy AI DJ: 1600-byte vectors (bytemuck), chunks of 25 tracks           │
│  - Safe migration of VPN and Telegram configs with .bak backups                 │
├─────────────────────────────────────────────────────────────────────────────────┤
│                      PHASE 3: WIN32 AND NETWORK PROTECTION                      │
│  - Hidden top-level listener window (TaskbarCreated, WM_QUERYENDSESSION)        │
│  - Native helper shell vpn-helper-stop (removes .vbs and .ps1)                  │
│  - Synchronous TUN adapter release in ExitRequested (blackout protection)       │
│  - Clipboard RAII guard and safe WH_KEYBOARD_LL keyboard hook                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                          PHASE 4: IPC AND CLI SERVICES                          │
│  - TCP IPC hardening: remove unwrap() and cap the line buffer                   │
│  - IpcModule contract (forward vpn_status, telegram_status)                     │
│  - Fix CLI doctor --simulate and shell logs --level/--source filtering          │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

### Phase 1: Core Foundation and Dual-Storage Architecture

#### 1.1 Moving the Database into the Core
1. Add the dependencies `redb = "2.1"` and `bincode = "1.3"` to `backend/toolchain/Cargo.toml`.
2. Create the module `backend/core/db/` (or move it into the local crate `backend/crates/shell-db`):
   - Move the table definitions and transaction methods from `backend/modules/music/src/shell_db.rs`.
   - Remove the import `use qbc_music_core::shell_db::ShellDatabase` from `backend/core/main.rs` and `backend/core/ipc.rs`.
   - Turn the `music` module into a consumer of the single database, through a reference to the core's database service.

#### 1.2 Dual-Storage and the Async Logger
1. Split the storage into two independent files:
   - `%LOCALAPPDATA%\Shell\shell.redb`: widget configs, Explorer, accounts, tracks, playlists (ACID, `Durability::Immediate`).
   - `%LOCALAPPDATA%\Shell\shell_logs.redb`: only the core and frontend system logs (rotation, a ring buffer of up to 50,000 lines).
2. Implement non-blocking log collection:
   - Channel: `tokio::sync::mpsc::channel(4096)`.
   - The producers `log_line` and `frontend_log` use `try_send`. When the channel is full, `AtomicU64 LOGS_DROPPED` is incremented.
   - A background thread flushes a batch of records to `shell_logs.redb` every 500 ms, with `Durability::Eventual`.
3. Install an emergency `std::panic::set_hook`:
   - On a panic, the thread opens `%LOCALAPPDATA%\Shell\crash.log` directly and synchronously flushes the stack trace to the physical disk.
4. Wire `mimalloc` into `backend/core/main.rs` as the `#[global_allocator]`.

---

### Phase 2: Data Model Unification and AI DJ Rescue

#### 2.1 Unifying Track and Playlist Models
1. Remove the `Playlist` structure mismatch:
   - Bring the fields to one standard (sync `shell_db.rs` and `core/db.rs`).
   - Move playlist metadata serialization to `serde_json`. JSON tolerates new fields without breaking the binary format.
2. Fix the canonical format of the track key in a playlist:
   - Exactly `{playlist_id}_{position:08}`, with leading zeros, so that numeric order is preserved in the lexicographic B-tree.

#### 2.2 Migrating AI DJ into the Database
1. Redirect the AI DJ database path from `std::env::temp_dir()` to the canonical directory `%LOCALAPPDATA%\Shell\shell.redb`.
2. Introduce zero-copy storage of the vectors:
   - Store the vector `[f32; 400]` as a raw byte slice `&[u8]` of 1600 bytes, using `bytemuck::cast_slice`.
3. Optimize the writes:
   - Stop opening and closing the database for every track.
   - Write embeddings in batches of 25 to 50 tracks, in one transaction. Force a CPU time-slice yield (`tokio::task::yield_now().await`) so the UI thread does not starve.

#### 2.3 Safe Migration of VPN and Telegram Settings
1. Implement the transactional migration contract:
   - At startup, check whether `shell.redb` already has records.
   - If the tables are empty, read the existing `vpn_settings.json`, `vpn_servers.json`, and `telegram_config.json`.
   - Write the data to `shell.redb` inside a single transaction, with roundtrip validation.
   - Rename the source JSON files to `.bak`.
2. Synchronize the schemas: support `#[serde(rename_all = "camelCase")]` and `#[serde(default)]` for all optional fields.

---

### Phase 3: Win32 Resilience, Native Helper, and Gateway Protection

#### 3.1 Invisible Top-Level Listener Window
1. Create a separate, lightweight top-level window (`WS_OVERLAPPED`, no parent, size 0x0) with the window class `"Shell_BroadcastListener"`.
2. Register the system messages:
   - `RegisterWindowMessageW("TaskbarCreated")`, with a call to `ChangeWindowMessageFilterEx` (protection against UIPI).
   - Handle `WM_QUERYENDSESSION` and `WM_ENDSESSION`.
3. On receiving `TaskbarCreated`:
   - Enable a 1000 ms debounce and a Circuit Breaker (no more than 3 attempts in 30 seconds).
   - Poll for `WorkerW` readiness strictly in a background worker thread (up to 20 attempts, 250 ms apart, with `SMTO_ABORTIFHUNG`).
   - The UI thread is brought in once, for `SetParent`, through `run_on_main_thread`.

#### 3.2 Removing the Scripts and the Native `vpn-helper-stop` Helper
1. Delete `vpn_hidden_launch.vbs` and `vpn_stop.ps1`.
2. Add the command to the CLI in `backend/cli`:
   `shell vpn-helper-stop`
   - Implementation in pure Rust through the Win32 API (`CreateToolhelp32Snapshot`, `Process32FirstW`, `Process32NextW`, `OpenProcess`, `TerminateProcess`).
   - Guaranteed termination of the `xray.exe` and `sing-box.exe` processes in **under 3 ms**.
3. Update the XML manifest of the Task Scheduler task `ShellVpnStopTask.xml`:
   - Call the compiled native binary directly, without calling PowerShell.
4. In `backend/modules/vpn/vpn.rs`, in the function `shutdown`:
   - Make the call synchronous: wait for confirmation that the processes have exited, with a 1000 ms timeout. This fully rules out the "dead TUN adapter" scenario when the app exits.

#### 3.3 Win32 Guards and Leak Protection
1. Introduce the RAII guards `ClipboardGuard` and `HGlobalGuard` for the function `set_clipboard_text`, with 10 attempts to open the clipboard.
2. Save the handle of the `WH_KEYBOARD_LL` hook in the atomic variable `HOOK_HANDLE`. Guarantee that it is removed with `UnhookWindowsHookEx` on the owning thread when `WM_QUIT` is received.
3. Remove the calls to `log_line` from the body of `win_e_proc`, and replace them with a non-blocking send through `try_send`.

---

### Phase 4: IPC Bridge and CLI Contract Stabilization

#### 4.1 Hardening the Core's TCP IPC Server
1. In `backend/core/ipc.rs:69`:
   - Remove `.try_clone().unwrap()`. Write the response directly to the original socket stream, `&mut stream`.
2. Limit the maximum size of an incoming line with `io::BufRead::take(65536)`, to protect against Slowloris attacks and memory exhaustion.
3. Limit the number of simultaneous TCP connections (a Semaphore with 16 permits).

#### 4.2 The `IpcModule` Module Contract
1. Implement the trait:
   ```rust
   pub trait IpcModule: Send + Sync {
       fn handle_ipc(&self, cmd: &str, args: &serde_json::Value) -> Option<Result<serde_json::Value, String>>;
   }
   ```
2. Register the command handlers of the VPN and Telegram modules.
3. Route the real commands `vpn_status`, `telegram_status`, and `player_status` through the IPC router.

#### 4.3 Fixing the `shell` CLI Tool
1. In `backend/cli/src/main.rs`:
   - Fix the parsing of the `shell logs` command: pass the `--level` and `--source` arguments correctly into the IPC request.
   - In `doctor --simulate`: stop masking errors for missing commands, and show the real diagnostic status of the services.

---

## 5. Risk Matrix and Verification Strategy

| Risk | Likelihood | Impact | Prevention / verification strategy |
| :--- | :--- | :--- | :--- |
| **Loss of user Telegram accounts during migration** | High | Critical | Create a `.bak` copy of the JSON files before migration. Verify that the data can be read back from `redb` before the JSON is disabled. Add a unit roundtrip test. |
| **Loss of internet connection on PC shutdown** | High | Catastrophic | Replace PowerShell with a native Win32 `TerminateProcess` (under 3 ms). Make the shutdown synchronous in `ExitRequested`. Register for `WM_QUERYENDSESSION`. |
| **Breaking existing player playlists** | Medium | High | Move playlist metadata serialization to `serde_json`. Use 8-digit track key padding. Add a regression test that loads an old database. |
| **Loss of keyboard input focus in the Telegram widget** | High | Medium | Ban the heavy re-render (`innerHTML = ""`) when statuses update. Update DOM attributes in place instead. |
| **Core panic on a network break in IPC** | High | Critical | Remove `.unwrap()` in `ipc.rs`. Run a stress test that interrupts connections, using `cargo test` or a CLI ping flood. |

### End-to-end verification protocol after the work is complete:
1. `shell check`: syntax check of the compilation, with no warnings.
2. `shell doctor --all --simulate`: end-to-end audit of the environment, modules, and logs, plus a UI simulation, with no masked errors.
3. `shell logs --level ERROR`: confirm there are no unhandled exceptions and no deserialization failures.
4. `shell shot`: a required screenshot of the desktop, to confirm that the widgets render correctly and there are no layout artifacts.
5. Record the session results in `docs/architecture.md`.
