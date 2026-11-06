# Chapter 11. `doctor`: the diagnostic loop

`doctor` is one entry point for health. It reads declarations from the modules rather than holding its own lists. Each section is described below.

## 11.1 Flags and which sections run

```text
udtk doctor [-kernel] [-modules] [-ping] [-env] [-check] [-quick] [-simulate] [-all]
```

Each flag has a long form with two dashes (`--kernel`, and so on). The whitelist is `ALLOWED_DOCTOR_FLAGS`, and any other flag is refused with exit 2.

| Flag | Section | What it does |
|---|---|---|
| `-check` | Compiler and build | Runs `cargo check` on the toolchain. This takes tens of seconds |
| `-env` | Dependencies | Presence of the files that modules declare (DLLs, executables, model weights, scheduled tasks), and the `.env` file |
| `-kernel` | Kernel | Five dry checks, in memory (see 11.3) |
| `-modules` | Modules | Four phases per module (see 11.4) |
| `-ping` | Runtime and IPC | Connects to the socket, asks `ping`, and reads the journal statistics |
| `-quick` | Shortcut | `env`, `kernel`, `modules`, and `ping`. No compilation |
| `-simulate` | User simulation | Probes of the live app and of the rendering functions (11.6) |
| `-all` | Everything | The full audit. This is the default |

Selection rules, as coded in `doctor_cmd`:

- If a selective flag is given and neither `-all` nor `-quick` is, **only** the chosen sections run.
- With `-quick`, the sections are `env`, `kernel`, `modules`, and `ping`.
- Otherwise (no flag, or `-all`), the sections are `compiler`, `env`, `kernel`, `modules`, and `ping`. `-simulate` runs only when asked for.

## 11.2 Compiler and environment

`-env` iterates the modules of the registry and, for each declared dependency, checks its presence:

- `DynamicLibrary` (for example `libmpv-2.dll`, `tdjson.dll`), resolved with `paths::resolve_binary`, together with its companion files.
- `Executable` (for example `sing-box.exe`, `vpn-helper.exe`, and `ciadpi.exe` for the Telegram proxy, which is Optional and gives a WARN, not a FAIL).
- `ModelAsset` (the model graph and its external weights, under `opt/models`).
- `WindowsTask` (the task must be registered in Task Scheduler).

The `.env` file is looked up and reported. The scheduled tasks are checked for state, and a task that a module declares but that `udtk tasks` does not manage gives `[WARN] task '<name>' declared by module '<id>' is not managed by 'udtk tasks'`. The severity comes from the manifest: a `Mandatory` miss is a FAIL, and an `Optional` miss is a WARN.

Limit of this section: `-env` checks **presence only**. It does not validate that a DLL is a valid x64 PE file. The PE check (`validate_pe_x64`) runs in the module phases (11.4), not here.

## 11.3 Kernel checks

Five checks that run in memory, with no disk writes. They are built on the contract in `unidtk-api`:

1. `kernel:in_memory_db`: the `ModuleDb` contract on an in-memory store (create, read, update, prefix listing, count, delete).
2. `kernel:event_bus`: the event bus loops back a published value to `state`.
3. `kernel:system_paths`: resolves the install directory, the data directory **without creating it**, the logs directory, and the cache directory.
4. `kernel:task_templates`: substitutes `@INSTALL_DIR@` and the user name into the task XML, and checks that no placeholder remains.
5. `kernel:capture_logger`: the thread-local log capture buffer is isolated.

## 11.4 Module phases

Modules come from the registry, not from a list written in the CLI. For each module the run has four phases:

1. **Declarations.** For each dependency, the carrier is checked. A `DynamicLibrary` is resolved, and `validate_pe_x64` checks the `MZ` header, the PE signature, the machine type `0x8664` (x64), and the minimum size. An `Executable` is checked for presence and, if it declares probe arguments, run with a **1500 ms** watchdog (`run_executable_probe`, started with `CREATE_NO_WINDOW`, with its output captured). A `ModelAsset` is checked for its size and companion files. A `WindowsTask` is checked for registration.
2. **Dry init.** The module is initialised with `ModuleContext::new_diagnostic(id, InMemoryModuleDb)`, in a separate thread, under `catch_unwind` and `recv_timeout(2000 ms)`. Logs are captured, so that they survive a panic. On timeout the thread is left detached, without a blocking join.
3. **Shutdown.** The modules are shut down in reverse order, with a **1500 ms** budget each.
4. **Orphans.** A directory `modules/*/rust/Cargo.toml` that the registry does not know gives `orphan Rust module ... missing from registry!` and fails the check.

No data directory is created during the run. `paths::data_dir_view()` is used, not `data_dir()`.

## 11.5 Runtime and IPC

`-ping` connects to `127.0.0.1:26540` with a 500 ms timeout, sends `ping`, and reads the answer. On success it prints `[OK] IPC ping (N ms): app is running` and then the journal statistics from `log_stats`. If the app does not answer, the output is `[FAIL] IPC ping: app is not running (...)`.

## 11.6 Simulation

`-simulate` checks the live application. The code runs these steps, in order:

1. `IPC Ping`, timed in milliseconds.
2. Journal health: `log_stats`.
3. `Frontend Error Audit`: the journal records from the sources `frontend` or `webview` at level ERROR. Any such record is a FAIL. The first three are printed.
4. Widget configuration: `get_widget_config` is read and checked for positions. An empty layout is a WARN.
5. Read-only probes of the VPN and Telegram runtimes: `vpn_status` and `telegram_status`. Each one prints `[OK]`, or `[INFO] ... (idle or offline)` if it does not answer.
6. A **CDP run** in a hidden sandbox. For each module that declares a `simulate` entry in its `metadata.json`, the probe imports the module's file, renders the function into an offscreen `div` with the class `simulation-sandbox`, and times it. The sandbox deliberately has **no** `widget-card` class, so the layout code and the harness never see it.
7. An interaction on the **offscreen copy** (`interaction:clickSandboxCopy`), and an audit of cards that show `Failed to fetch`.
8. The **motion budget**. Each declared surface is opened with a real CDP click and closed with a real Escape, while the page records frame gaps and long tasks (`startMotionProbe` in `core/ui/harness.js`). The targets are the core's own, from `harness.motionTargets()` (the settings menu and one folder card), and every module's `motion.open` in its `metadata.json`: the selector inside its desk card whose click opens a surface and does nothing else. An empty selector means the card itself. Per target the line reports the worst frame and the longest task of the open and of the close; anything over 50 ms is a `[FAIL]`.

Without a running app, the CDP part degrades to `[INFO] CDP simulation unavailable (...)` and does not fail. If no module declares a `simulate` entry, the output is a WARN. Steps 1–7 are read-only: they do not call `save_widget_config`, and they do not click the live widgets. Earlier versions clicked the live clock widget mid-animation. That defect is recorded in [H3](../history/rfcs/udtk-migration.md#h3-slices-s0-to-s5-former-109-to-1022).

Step 8 does click the live desk, so it guards against that defect. It runs only from a quiet desk (`harness.isDeskQuiet()`: no surface, no expanded card, no settings menu); otherwise it prints `[SKIP]` and leaves your surfaces open. It waits for each open and close to settle by counting frames (`harness.waitMotionSettled`), not with a timer, so it never races the motion tokens. It stops at the first surface that Escape does not close. A module declares `motion.open` only for a click without side effects: the volume bar and the player do not, because their clicks change the volume or the playback. The build lint fails when a `motion.open` class is named in none of the module's `ui/*.js` files.

## 11.7 Result

The last line is one of two:

```text
[✓] UniDTK doctor: ALL CHECKS PASSED                     (exit 0)
[!] UniDTK doctor: FAILED (N issues detected)            (exit 1)
```

A failure in a module phase is counted and reported as `[FAIL]` or `[WARN]`. It does not stop the process. The count includes failures from each section. The `-simulate` section counts as one failure when any of its checks fails. The rule matters: an earlier version printed `ALL CHECKS PASSED` while the frontend probes were broken.
