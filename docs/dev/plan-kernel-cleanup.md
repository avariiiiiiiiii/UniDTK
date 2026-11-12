# Plan: a Kernel That Knows No Module

This page is the working plan for removing module knowledge from the kernel (issues #326, #327, #330, #331). It is independent of the UI host: every step ships into the current Tauri build. When the plan is done the page moves to `docs/history/`.

The order is from small and isolated to large: Win+E first, then shutdown, then the registry and the command table, then module data and events.

## Step 1. Win+E only on the desktop (#327)

**Cause.** `keyboard_hook_proc` in `core/kernel/hotkeys.rs` matches the hotkey table and sends the action (`ACTIONS.try_send`) before it looks at the foreground class. The class (`ForegroundClass`, from `GetForegroundWindow`) is already computed earlier in the same call, but the table match ignores it. `win_e` is declared with `swallow: true`, so Win+E is consumed in every application, and the action thread opens the overlay.

**Change.**

1. `HotkeyDefinition` gets a field `scope: HotkeyScope` with the values `Desktop` and `Global`; the default table gives `win_e` the scope `Desktop`. Hotkeys are not stored anywhere (the table lives in memory, built from `default_hotkeys()`), so no compatibility for old records is needed.
2. The hook checks the scope before `ACTIONS.try_send`, before the Start-menu suppression (`keybd_event(0xE8)`) and before `remember_key`. A `Desktop` hotkey with a foreign foreground goes to `CallNextHookEx`, so Windows opens its own Explorer. Nothing is simulated.
3. The hook stays non-blocking: only `try_read`, `try_send` and `try_lock`. Windows removes a low-level hook that exceeds `LowLevelHooksTimeout`.

**Tests first.** In the `tests` module of `hotkeys.rs`: with a desktop foreground, Win+E is consumed and one action is queued; with a foreign foreground, it is passed on, no action is queued, and no owned-key record is left.

**Done when.** With a browser in front, Win+E opens the Windows Explorer and not the overlay; on the empty desktop it opens the overlay.

## Step 2. Shutdown (#326)

### 2a. A second shutdown waits for the first

**Cause, from the code.** `app_shutdown` returns at once when `SHUTDOWN_DONE` is already set (`core/kernel/main.rs`). If a tray quit is running and the session ends, `WM_ENDSESSION` in `core/kernel/shutdown.rs` returns while the VPN is still stopping, and Windows may end the process with the TUN adapter up.

**Change.** The first caller runs the shutdown; every later caller waits for its completion signal with its own budget. The `RunEvent::ExitRequested` path keeps its budget of zero, because it runs on the main thread and must not block it.

**Test.** Two threads call shutdown; the second returns only after the first finished, or after its budget.

### 2b. The tao panic

**Known.** After `shutdown: begin` the tao event loop panics on the main thread (`runner.rs:371`, "cannot move state from Destroyed") and `shutdown: done` never appears.

**Cause, proven.** The panic names the line, and tao 0.35.3's source explains it without symbols. tao's hidden window "Tao Thread Event Target" is a top-level `WS_POPUP`, so it receives `WM_ENDSESSION`; on `wParam = TRUE` its procedure (`thread_event_target_callback`, `event_loop.rs:2384`) calls `loop_destroyed()`, which moves the runner to `Destroyed`. The process does not end there: Windows ends it after every window has answered, and the listener in `core/kernel/shutdown.rs` is still stopping the VPN on its own thread. The main thread keeps dispatching, and the next event moves the runner out of `Destroyed`, which tao answers with the panic. The panic kills the process in the middle of the shutdown.

**Reproduced.** `WM_QUERYENDSESSION` and then `WM_ENDSESSION` with `ENDSESSION_LOGOFF`, sent to every top-level window of the running process (`EnumWindows`): the log shows `shutdown: WM_ENDSESSION received`, `shutdown: begin`, `vpn: shutdown requested`, then the same panic 40 ms after the message, and no `shutdown: done`.

**Fix.** `shutdown::guard_event_loop`, called from `setup` on the main thread, subclasses tao's event-target window and answers a committed `WM_ENDSESSION` itself, without passing it on. tao never learns of the session end and its loop keeps running until Windows ends the process; the shutdown is the listener's. A cancelled session end (`wParam = FALSE`) passes through, as tao does nothing with it. The test `the_event_loop_never_sees_a_committed_session_end` in `shutdown.rs` fails without the subclass.

**Done when.** The synthetic session end logs `shutdown: done` and no panic; then three real sign-outs log `shutdown: done`, `crash.log` gains no record, and the TUN adapter is down after the next sign-in (the user's physical step).

## Step 3. One registry and one command table (#330)

**Cause.** `registry::all_modules()` exists, but `main.rs` ignores it: `setup` builds every module by hand, stores runtimes by concrete type with `app.manage` (more than 40 commands take `tauri::State<VpnRuntime>`, `State<TelegramRuntime>` and the like), `apply_module_enabled` switches by a `match`, and `app_shutdown` stops modules by hand in an order that carries meaning.

### 3a. The registry drives start, switch and stop

1. The registry is built in `setup` **before** `set_emitter` and before any module's `init`: a list of `Arc<dyn Module>` in start order, read-only afterwards. Until 3b the kernel still registers the commands' typed state (`VpnRuntime` and the like), so it builds the modules from their concrete types and keeps a clone of each; after 3b it builds them from `all_modules()`.
2. **Start order** comes from a new trait method `Module::starts_after() -> &'static [&'static str]` (module ids; empty by default), sorted topologically; a cycle or an unknown id is a startup error. The existing `Module::dependencies()` is a different thing — the module's system dependencies (binaries, DLLs) — and is not used for ordering. Telegram declares `vpn` (it reads the VPN state on start). The mp3 player keeps its background start, because a synchronous start delayed the wallpaper attach.
3. **Switch.** `apply_module_enabled` calls `suspend` or `resume` through the registry. The lazy start of `mp3-player` moves into its own `resume` ([modules.idempotent](../reference/modules.md#r-modules.idempotent)).
4. **Switch against stop.** Shutdown sets a `shutting_down` flag under `MODULE_SWITCH`; a switch that takes the lock afterwards refuses. The background mp3 start publishes the module under the same lock and checks both the flag and the stored disabled list first, which closes today's race where a player switched off during its start keeps running.
5. **Stop** runs in two passes from the registry: `Module::begin_shutdown()` on every module (a new trait method with an empty default, idempotent: the process-wide latches of Telegram and the player), then `shutdown(budget)` in reverse start order. A budget of zero means "do not wait" (the `ExitRequested` path); the VPN waits for its elevated helper only with a non-zero budget. A test pins the order: stopping the VPN does not bring the Telegram proxy back.

### 3b. Module commands go through the modules

Not all 176 Tauri commands in `main.rs` belong to modules: kernel commands such as `quit_app` (also called by `udtk stop`), `attach_wallpaper`, `open_explorer`, `set_desktop_input_focus`, `refresh_display_layout`, `session_info`, `weather_location_request` stay kernel commands. Only module commands move.

1. **One place per command.** A module declares each of its commands once, in a `unidtk_api::module_commands!` table in its own crate: the access (`read`, `read_ui`, `write`), the name, a summary, the argument names with their types, and the body. The macro generates both the module's `COMMANDS` (the specs) and its dispatch, so a spec without a handler, or a handler without a spec, cannot be written. The CLI already links every module crate (it calls `registry::all_modules()`), so its offline manifest reads the modules' lists as well; `core/kernel/command_inventory.rs` keeps the kernel's own commands. `MODULE_ROUTES`, the per-command `handle_vpn_*` functions in `core/kernel/ipc.rs` and each module's `ipc_call` are deleted.
2. `Module::commands()` and `Module::dispatch(name, args) -> Option<Result<Value, String>>`. The module holds its runtime itself, so the kernel stops managing typed module state. The `Registry` maps every command name to its module and refuses a name claimed twice, by two modules or by a module and the kernel, as a startup error.
3. The door is checked in the dispatch path by the spec, as `check_door` does today, so a write command never runs from the socket. A read that the socket did not serve before is declared `read_ui`, so the socket surface does not grow ([ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only)).
4. **The Tauri door.** The kernel wraps the handler that `generate_handler!` builds for its own commands in its own `invoke_handler`: a name that the registry maps to a module is served from the module's dispatch on the blocking pool ([kernel.no-blocking-main-thread](../reference/kernel.md#r-kernel.no-blocking-main-thread)), and every other name goes to the generated handler. An argument is read by its Rust name in camelCase, which is what Tauri asked of the interface, or in snake_case, which a tool may send; nested payloads are deserialized as they are.
5. **JavaScript does not change.** Command names and argument shapes stay those of the Tauri commands they replace, so `invoke`, the WebSocket path, the shadow comparison and `udtk gui js:` recipes are untouched.
6. **Tests.** `frontend_invoke_names_are_registered` and `every_manifest_ui_command_is_registered` in `main.rs` read the kernel's handler list and the modules' tables together. A test in `core/api` pins the argument lookup (camelCase first, snake_case second, a missing argument as `null`).
7. The default desk layout comes from module metadata ([modules.metadata](../reference/modules.md#r-modules.metadata)), not from `main.rs`: `defaultPlacement` and `defaultParent` in each `metadata.json`, assembled by the page in `getDefaultWidgetConfig`. The kernel stops seeding a layout; the page already saves the default into an empty store.

**Done when.** The socket door lists the same commands as before and the manifest gains the interface's module commands; every module switches off and on twice; a cold start with a module stored as disabled leaves it off; the frontend log shows no failed call.

## Step 4. Module data and events leave the kernel (#331)

1. **State events.** `STATE_EVENTS` in `core/kernel/events.rs` is a delivery property of the broker (a ring of one, the newest value). It becomes a `coalesce` flag in each module's event declarations, a compile-time constant. The broker collects the flags into a `OnceLock` table before `set_emitter`, so the first frame of a state event is never stored as a stream.
2. **Explorer tabs.** `explorer_tabs` (`tabs` and `sort`) in `core/kernel/unidtk_db.rs` moves to the Explorer module through `ModuleDb`, which stores under `mods/explorer/` in `module_kv` — a different table and key. So the move is a one-time migration: copy both values and write a marker in one transaction, then stop opening the old table. A test on a database with old records checks that tabs and sort survive. `FOLDERS` stays: desk folders belong to the kernel ([desk.folders](../reference/desk.md#r-desk.folders)).
3. **The player migration stays.** `core/kernel/library_migration.rs` works on the raw database on purpose (modules never get it, [storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db)), and an installation restored from an old backup still needs it. Only its comments are corrected: they name `shell.redb`, the file is `unidtk.redb`.
4. **The weather permission window** (`core/kernel/weather_location.rs`) is a WebView window and leaves with the WebView. It is not moved.
5. **The Explorer hotkey.** The "Shell Explorer" entry in `hotkeys.rs` becomes a hotkey contributed by the Explorer module.

**Done when.** A search of `core/kernel` finds no module id, table, or event name outside `registry.rs` and the two migrations.

## Verification of every step

`udtk build`, `udtk doctor`, `udtk doctor -ping` (RTT), `udtk logs --level ERROR`, `udtk logs --source frontend --limit 30`, `udtk shot`, `cargo test --release --workspace -j 1`, `udtk doc --check`. Each step is one commit with the test count in its body.
