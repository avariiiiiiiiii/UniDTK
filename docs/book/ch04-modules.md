# Chapter 4. Modules

A module is one feature of UniDTK: the clock, the volume strip, the VPN panel, the music player. Its backend is a Rust crate under `modules/<id>/rust/`, and its page code lives beside it under `modules/<id>/ui/`. This chapter follows one module through its life. It shows what the kernel hands a module, which functions the kernel calls and in what order, how a module is switched off and on while the app runs, how two modules exchange data without calling each other, and which build checks stop a broken module. When you finish, you can read the setup code in `core/kernel/main.rs` and know what each call means.

## 4.1 The shape of a module

Start with the folder of the `glyph-bar` module, which is the smallest module that has a backend.

Filename: modules/glyph-bar/ (abridged listing of the folder)

```text
modules/glyph-bar/
  rust/
    Cargo.toml        crate unidtk-mod-glyph-bar
    glyph_bar.rs      the backend: system volume, the Module impl
  ui/
    widget.js         the page code, exports renderGlyphBar
    metadata.json     how the menu and the desk treat the module
    glyph-bar.css     styles, linked by hand from index.html
    icon.svg          the icon shown in the menu
```

The two halves change together, so they share one folder, as the rule [modules.vertical-slice](../reference/modules.md#r-modules.vertical-slice) requires. A module without a backend has only `ui/`, as `clock` does. The backend is a separate crate whose `Cargo.toml` depends on `unidtk-api` and not on the kernel, the rule [modules.contract-only](../reference/modules.md#r-modules.contract-only). The contract crate is `core/api`.

## 4.2 The Module trait

Every backend implements one trait, `Module`, defined in the contract crate.

Filename: core/api/src/lib.rs (abridged)

```rust
pub trait Module: Send + Sync {
    fn id(&self) -> &'static str;
    fn dependencies(&self) -> &'static [manifest::ModuleDependency] { &[] }
    fn starts_after(&self) -> &'static [&'static str] { &[] }
    fn init(&self, ctx: ModuleContext) -> Result<(), String>;
    fn begin_shutdown(&self) {}
    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> { Ok(()) }
    fn suspend(&self) -> Result<(), String> {
        self.shutdown(std::time::Duration::from_millis(1500))
    }
    fn resume(&self, ctx: ModuleContext) -> Result<(), String> {
        self.init(ctx)
    }
    fn commands(&self) -> &'static [commands::CommandSpec] { &[] }
    fn dispatch(&self, _cmd: &str, _args: &Value) -> Option<Result<Value, String>> { None }
    fn state_events(&self) -> &'static [&'static str] { &[] }
    fn hotkeys(&self) -> &'static [HotkeySpec] { &[] }
}
```

Each method has one job:

- `id` returns the folder name, such as `"glyph-bar"`. The kernel and the page both use this string to find the module.
- `dependencies` lists external files the module needs, such as a DLL. The [`udtk` book](../udtk/ch15-how-a-module-joins-the-bridge.md) explains how the doctor reads it.
- `starts_after` names the modules this one must start after, and so stop before. Section 4.4 shows how the kernel orders the start by it.
- `init` starts the module. It receives a `ModuleContext` by value, so the module owns the context from then on.
- `begin_shutdown` raises the module's exit latches. On exit the kernel calls it on every module before it stops any, so that no stop can start another module's work again. It must be cheap and must not wait.
- `shutdown` stops the module's threads and processes. It receives a time budget, and the module must finish within it. A zero budget means "do not wait at all", because the caller is the main thread.
- `commands` and `dispatch` are the module's commands: what it serves, and the call that serves one. Both usually come from the module's command table, section 4.5.
- `state_events` names the events whose newest value is all that matters, such as the volume or the VPN state. The kernel's event broker keeps one frame of each, so a page that subscribes gets the current value at once.
- `hotkeys` lists key combinations the module wants, such as Win+E for the Explorer. The kernel's keyboard hook matches them and publishes the event each one names, so the kernel never learns what a key means.
- `suspend` and `resume` are the switch for a running app. Their defaults call `shutdown` and `init`, so most modules get switching for free. A module that overrides them must not set a global flag that means "the app is exiting", because the app keeps running.

The trait requires `Send + Sync`, because the kernel keeps each module in its registry, which Tauri's managed state holds and every thread can reach.

## 4.3 ModuleContext: what the kernel hands in

`init` receives everything the module may use from the kernel, in one value.
Filename: core/api/src/lib.rs (abridged)

```rust
pub struct ModuleContext {
    id: ModuleId,
    db: Arc<dyn ModuleDb>,
    host: Option<Arc<dyn Any + Send + Sync>>,
    stores: Option<Arc<dyn StoreFactory>>,
    run_mode: ModuleRunMode,
    enabled: bool,
}
```

The fields are private. The module reads them through methods:

| Method | Returns |
| :--- | :--- |
| `id()` | the module id as a `ModuleId` |
| `db()`, `into_db()` | the module's slice of the shared database, see [Chapter 5](ch05-storage.md) |
| `host::<T>()` | the kernel's opaque host object, downcast to a type the module asks for |
| `store(StoreSpec)` | a separate file for the module's data, see [Chapter 5](ch05-storage.md) |
| `is_diagnostic()`, `enabled()` | a `udtk doctor` dry run; the user has the module switched on |

The kernel builds the context in one function. Every module gets it the same way.

Filename: core/kernel/main.rs (abridged)

```rust
fn module_context(app: &tauri::AppHandle, id: &str, enabled: bool) -> unidtk_api::ModuleContext {
    let module_id = unidtk_api::ModuleId::new(id);
    let db: Arc<dyn unidtk_api::ModuleDb> = match shared_db(app) {
        Some(shared) => Arc::new(crate::module_db::RedbModuleDb::new(shared.raw_db(), &module_id)),
        None => Arc::new(NoopModuleDb),
    };
    unidtk_api::ModuleContext::new(module_id, db)
        .with_host(Arc::new(app.clone()))
        .with_store_factory(store_factory())
        .with_enabled(enabled)
}
```

The function does four things, in order. It wraps the module's key slice of the shared database. It stores the Tauri `AppHandle` as the opaque host. It attaches the one store factory of the process. It records whether the user has the module on. If the shared database failed to open, the module gets `NoopModuleDb`, whose reads return nothing and whose writes fail with a clear message. The module still starts, so one broken file does not stop the whole app.

The `enabled` flag is a promise the module keeps itself. The kernel does not stop anything when the flag is `false`. The module must read `ctx.enabled()` and skip its heavy work. The `glyph-bar` backend does exactly that.

Filename: modules/glyph-bar/rust/glyph_bar.rs (abridged)

```rust
fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
    if ctx.is_diagnostic() {
        // In diagnostics, check that WASAPI is reachable, without a background thread.
        let _ = get_system_volume();
        return Ok(());
    }
    if !ctx.enabled() {
        unidtk_api::log("glyph-bar: disabled — volume poll not started");
        return Ok(());
    }
    *self.state.lock().expect("glyph-bar module lock") = Some(start());
    Ok(())
}
```

The diagnostic branch runs under `udtk doctor`. It must not start a thread that runs forever, because the doctor process ends after its checks. The doctor marks its context as diagnostic with `ModuleContext::new_diagnostic`, which sets `ModuleRunMode::Diagnostic`.

## 4.4 How the kernel creates a module

The kernel has no plugin loader. At the top of the `setup` closure in `core/kernel/main.rs` it builds every module from the factory list `all_modules()` and hands them to a `Registry` (both in `core/kernel/registry.rs`), which is the only place that starts, switches and stops modules, and the only place that knows which module serves which command. Building a module starts nothing; `init` does.

Filename: core/kernel/main.rs (abridged)

```rust
let registry = registry::Registry::new(registry::all_modules().into_iter().map(Arc::from).collect())?;
app.manage(registry);
// ...
for module in app.state::<registry::Registry>().in_start_order() {
    let id = module.id();
    module.init(module_context(app.handle(), id, !disabled_mods.contains(id)))?;
    log_line(format!("setup: module {id} started"));
}
```

The start order comes from the modules themselves. `Module::starts_after` returns the ids a module must start after, and `Registry::new` sorts the list by it, keeping the given order where nothing constrains it. Telegram names `vpn`. An unknown id, a cycle, an id registered twice, or a command name that two modules (or a module and the kernel) both declare is a startup error. `starts_after` is not `dependencies`: the second lists files and scheduled tasks for `udtk doctor`.

`disabled_module_set` reads the switched-off list from the database, and `init` gets a context whose `enabled` flag comes from that list. The `?` after `init` matters. If `init` returns an error, the setup closure fails, and Tauri stops the start-up with that error. A module that can fail at startup must report the failure as a value it can recover from, or it must not return an error at all.

Note: a module keeps its own state (`VpnRuntime`, `TelegramRuntime`, the explorer search, the weather runtime), and its commands read it from the module, so the kernel registers no module state with Tauri. A command that arrives before the state exists answers "not ready". The music player loads its library on a thread of its own, started from its `init`, and its commands answer "music core not ready yet" until the load is done.

Warning: `all_modules()` and `ALL_MODULE_IDS` in the registry are the only lists that name the modules. The build lint checks `ALL_MODULE_IDS` against the folders on disk, see section 4.8.

## 4.5 The calls during a run

After startup, the kernel calls a module in five situations. It calls `init` once, at startup, as shown above. It calls `dispatch` when a door asks for one of the module's commands. It calls `suspend` or `resume` when the user switches the module, described in section 4.6. On exit it calls `begin_shutdown` on every module and then `shutdown` on each.

A module declares its commands in one table, with the `module_commands!` macro from `unidtk-api`. Each entry gives the access, the name, a summary, the arguments and the body.

Filename: modules/glyph-bar/rust/glyph_bar.rs

```rust
unidtk_api::module_commands! {
    impl GlyphBarModule {
        read_only "get_system_volume" "Windows master volume" |this| get_system_volume();
        write "set_system_volume" "Set the master volume, 0 to 100" |this, level: u64| set_system_volume(volume_level(level));
        write "set_system_mute" "Mute or unmute the master volume" |this, muted: bool| set_system_mute(muted);
    }
}
```

The macro generates two items on the module type: `COMMANDS`, the specs, and `dispatch_command`, which reads each argument by name and runs the body. The module returns them from `Module::commands` and `Module::dispatch`. Because one table generates both, a command cannot be declared without a handler, and the kernel does not list module commands anywhere. The access names the `CommandSpec` constructor from chapter 3: `read_only` is served on both doors, `read_ui` and `write` only on the page's. `this` is the module itself, so a body reaches the module's state as `this.runtime`.

An argument is read by its Rust name, first in camelCase (`chatId`, what the page sends) and then in snake_case (`chat_id`, what a tool may send). A missing argument reads as `null`, so an `Option` parameter is `None` and a required one fails with its name. The body must evaluate to `Result<T, String>`, and the kernel always calls `dispatch` on a blocking thread, so a body may block, and an async body is driven with `tauri::async_runtime::block_on`.

The exit path is `app_shutdown` in `core/kernel/main.rs`. It runs once (a second exit route waits for the first), and it hands the modules to the registry.

Filename: core/kernel/registry.rs (abridged)

```rust
pub fn shutdown(&self, budget: Duration) -> Vec<(&'static str, String)> {
    for module in &self.modules {
        module.begin_shutdown();
    }
    self.modules
        .iter()
        .rev()
        .filter_map(|module| module.shutdown(budget).err().map(|error| (module.id(), error)))
        .collect()
}
```

The first pass raises every exit latch, so no stop can start another module's work again. The second stops the modules in reverse start order. The order matters for Telegram and the VPN: the VPN's stop fires the state event that would start Telegram's proxy again, and Telegram, which starts after the VPN, has latched and stopped before that.

The budget is zero when `app_shutdown` runs on the main thread, and two seconds when it runs off it (`quit_app`, the session end). A zero budget means "do not wait": the VPN fires its stop and returns. With two seconds, it waits for the elevated helper to confirm that sing-box is gone.

## 4.6 Switching a module off and on

The menu switches a module through one command, `set_module_enabled`. Its path is the menu, then the page's `setModuleEnabled` wrapper, then the Tauri command, then `apply_module_enabled` in `core/kernel/main.rs`.

Filename: core/kernel/main.rs (abridged)

```rust
pub(crate) fn apply_module_enabled(app: &tauri::AppHandle, db: &UniDtkDatabase, id: &str, enabled: bool) -> Result<(), String> {
    let _switch = MODULE_SWITCH.lock().map_err(|_| "module switch lock poisoned".to_string())?;
    if is_shutting_down() {
        return Err("the application is shutting down".to_string());
    }
    let registry = app.try_state::<registry::Registry>().ok_or("modules are not started yet")?;
    let module = registry.get(id).ok_or_else(|| format!("unknown module: {id}"))?;
    let mut disabled = db.get_disabled_modules_or_reset(/* ... */)?;
    if enabled {
        module.resume(module_context(app, id, true))?;
    } else {
        module.suspend()?;
    }
    disabled.retain(|m| m != id);
    if !enabled {
        disabled.push(id.to_string());
    }
    db.set_disabled_modules(&disabled)?;
    Ok(())
}
```

The order keeps a failure from leaving the app and the saved choice in disagreement. A static lock, `MODULE_SWITCH`, serializes switches. The module changes state first, through `resume` or `suspend`. Only after that succeeds is the new list of disabled ids written. If the module fails, the stored list is unchanged, and the error goes back to the menu. The rule is [modules.lifecycle](../reference/modules.md#r-modules.lifecycle).

The exit takes the same lock before it stops the modules, so a switch that is running finishes first, and one that comes later sees the exit and refuses. On the main thread the exit only tries the lock, because a switch may need the event loop.

Every module is built at startup and initialized, with `enabled` set to `false` when the user has it switched off. A module keeps that promise itself: the music player, switched off, does not load its library, and its `resume` loads it when the user switches it on. A switch during the load is not lost: the loading thread reads the switch and the exit latch under the same lock that `suspend` and `shutdown` take.

Note: the menu asks for a switch only for modules whose `metadata.json` says `"backend": true`, the rule [modules.states](../reference/modules.md#r-modules.states).

## 4.7 Events: how modules exchange data

A module must not call another module. The rule is [modules.no-module-to-module](../reference/modules.md#r-modules.no-module-to-module), and Cargo enforces it, because no module crate lists another module in its dependencies. Modules talk through three functions of the contract: `emit`, `subscribe`, and `state`.

The VPN module publishes its connection state, and the Telegram module reacts to it. Here are the two sides.

Filename: modules/vpn/rust/vpn.rs (abridged)

```rust
unidtk_api::emit("vpn", json!({ "connected": is_connected() }));
```

Filename: modules/telegram/rust/telegram.rs (abridged)

```rust
// Read the current value, without asking the VPN module.
let connected = unidtk_api::state("vpn");
// React to every later change.
unidtk_api::subscribe("vpn", |_payload| { /* ... */ });
```

In `core/api/src/lib.rs`, a call to `emit` stores the payload as the event's last value, passes the event to the kernel's emitter, and then calls each subscribed handler with a clone of the payload, on the calling thread. The kernel's emitter sends the event to the page through Tauri and also publishes it to the WebSocket broker. `subscribe` adds a handler to the list for a name. `state` returns the stored last value, or `None` if the event was never emitted, so a module that starts later can read the current state. The comment in `lib.rs` records why: `state("vpn")` replaced a direct call from Telegram into VPN, and that call made the two modules depend on each other.

Note: the handlers run after the lock is released, so a handler may call `emit` itself. Nothing in the bus stops a cycle of events, so a handler must not re-emit the event it received.

Warning: a handler runs on the thread that called `emit`. If a handler blocks, the module that emitted is blocked too. Heavy work belongs on a thread the handler starts.

Warning: an event name is a plain string. No compiler or lint connects a subscriber to an emitter. The only contract is the spelling, so a typo fails silently, with no error and no event.

## 4.8 The build-time lints

Some module mistakes are caught before the kernel compiles. The build script `toolchain/build.rs` runs four checks at the start of `main`.

Filename: toolchain/build.rs (abridged)

```rust
fn main() {
    lint_modules_integrity();
    lint_ui_imports();
    lint_simulate_metadata();
    lint_module_contracts();
    // ... embedding, staging the frontend, Tauri build
}
```

| Lint | What it checks | Failure |
| :--- | :--- | :--- |
| `lint_modules_integrity` | every `modules/<id>/rust/Cargo.toml` has its id in `core/kernel/registry.rs` and a line `unidtk-mod-<id>` in `toolchain/Cargo.toml` | `BUILD INTEGRITY ERROR` |
| `lint_ui_imports` | no page file imports `@tauri-apps/...` directly, except the bridge `unidtk-bridge.js` | `UI IMPORT ERROR` |
| `lint_simulate_metadata` | a `simulate` entry names a real file and a function in that file | `SIMULATE METADATA ERROR` |
| `lint_module_contracts` | `metadata.json` is valid JSON; an `embeddable` names a file that contains its render function; a container is not embeddable | a panic naming the file |

The checks are text checks. `lint_modules_integrity` searches for the quoted id in the registry file, and it searches for the crate name in `Cargo.toml`. A text check cannot prove that the kernel registers the module, which is why section 4.4 warns about the lists.

Note: no lint checks that a module avoids the kernel. The kernel package has only binary targets (see `toolchain/Cargo.toml`), so a module cannot link it as a library, and nothing else enforces the rule. The `udtk` book, in [how a module joins the bridge](../udtk/ch15-how-a-module-joins-the-bridge.md), states the same gap.
## 4.9 Summary

A module is a crate with a `Module` implementation and a folder of page code. The kernel constructs it in `setup`, hands it to the registry, which orders the start by `starts_after`, passes it a `ModuleContext` through `init`, and calls `begin_shutdown` and `shutdown` at exit. A user switch goes through `apply_module_enabled`. Modules exchange data only through `emit`, `subscribe`, and `state`, and the build lints catch broken shapes early. The next chapter shows where a module's data lives. Continue with [Chapter 5. Storage](ch05-storage.md).
