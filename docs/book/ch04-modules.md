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
    fn init(&self, ctx: ModuleContext) -> Result<(), String>;
    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> { Ok(()) }
    fn suspend(&self) -> Result<(), String> {
        self.shutdown(std::time::Duration::from_millis(1500))
    }
    fn resume(&self, ctx: ModuleContext) -> Result<(), String> {
        self.init(ctx)
    }
}
```

Each method has one job:

- `id` returns the folder name, such as `"glyph-bar"`. The kernel and the page both use this string to find the module.
- `dependencies` lists external files the module needs, such as a DLL. The [`udtk` book](../udtk/ch15-how-a-module-joins-the-bridge.md) explains how the doctor reads it.
- `init` starts the module. It receives a `ModuleContext` by value, so the module owns the context from then on.
- `shutdown` stops the module's threads and processes. It receives a time budget, and the module must finish within it.
- `suspend` and `resume` are the switch for a running app. Their defaults call `shutdown` and `init`, so most modules get switching for free. A module that overrides them must not set a global flag that means "the app is exiting", because the app keeps running.

The trait requires `Send + Sync`, because the kernel keeps each module in Tauri's managed state.

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

The kernel has no plugin loader and no loop over a list of modules at startup. Each module is named in the setup code of `core/kernel/main.rs`, inside the `setup` closure of the Tauri builder. The code for `glyph-bar` is representative.

Filename: core/kernel/main.rs (abridged)

```rust
let disabled_mods = disabled_module_set(app.handle());
// ...
let glyph_bar_module = glyph_bar::GlyphBarModule::new();
unidtk_api::Module::init(&glyph_bar_module, module_context(app.handle(), "glyph-bar", !disabled_mods.contains("glyph-bar")))?;
app.manage(glyph_bar_module);
log_line("setup: glyph-bar module started (system volume poll)");
```

`disabled_module_set` reads the switched-off list from the database, and `init` gets a context whose `enabled` flag comes from that list. The module is then registered as managed state, where other code finds it with `app.try_state::<T>()`. The `?` after `init` matters. If `init` returns an error, the setup closure fails, and Tauri stops the start-up with that error. A module that can fail at startup must report the failure as a value it can recover from, or it must not return an error at all.

Note: the setup code calls `init` before `app.manage`, because a command that arrives between the two steps would otherwise find the state "not ready" forever (the comment above the micrographics block says so). The music player is the exception. Its slow library load runs on a background thread, started by `start_music_module` in `core/kernel/main.rs`.

Warning: a module is named in several places in the kernel, and each place is a separate list. The setup code above is one. `core/kernel/registry.rs` has another, the function `all_modules()`. The CLI calls it, for `udtk doctor`, but the kernel's startup does not. The switch function, described in section 4.6, has a `match` on the id. The socket routes in `core/kernel/ipc.rs` have a third table. A new module must appear in all of them. The build lint checks only the registry, see section 4.8.

## 4.5 The calls during a run

After startup, the kernel calls a module in three situations. It calls `init` once, at startup, as shown above. It calls `suspend` or `resume` when the user switches the module, described in section 4.6. It calls `shutdown` once, when the application exits.

The exit path is `app_shutdown` in `core/kernel/main.rs`, which names each module and calls `shutdown` with a budget of 1500 milliseconds.

Filename: core/kernel/main.rs (abridged)

```rust
pub(crate) fn app_shutdown(app: &tauri::AppHandle, wait_for_vpn: bool) {
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    // ...
    if let Some(module) = app.try_state::<commands::Mp3PlayerModule>() {
        let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
    }
    if let Some(module) = app.try_state::<glyph_bar::GlyphBarModule>() {
        let _ = unidtk_api::Module::shutdown(&*module, Duration::from_millis(1500));
    }
    // ... telegram, vpn, micrographics follow in the same form
}
```

The `SHUTDOWN_DONE` flag makes the function run once. The order is deliberate: Telegram stops before VPN, because a VPN state change during exit used to race the Telegram proxy back to life.

Warning: the kernel has no list of modules that it can walk. A module that starts a thread must be added to `app_shutdown` by hand, or its thread keeps running until the process is killed.

## 4.6 Switching a module off and on

The menu switches a module through one command, `set_module_enabled`. Its path is the menu, then the page's `setModuleEnabled` wrapper, then the Tauri command, then `apply_module_enabled` in `core/kernel/main.rs`.

Filename: core/kernel/main.rs (abridged)

```rust
pub(crate) fn apply_module_enabled(app: &tauri::AppHandle, db: &UniDtkDatabase, id: &str, enabled: bool) -> Result<(), String> {
    let _switch = MODULE_SWITCH.lock().map_err(|_| "module switch lock poisoned".to_string())?;
    match id {
        "glyph-bar" => switch_module::<glyph_bar::GlyphBarModule>(app, id, enabled)?,
        // ... vpn, telegram, micrographics, mp3-player
        _ => return Err(format!("unknown module: {id}")),
    }
    let mut disabled = db.get_disabled_modules()?;
    disabled.retain(|m| m != id);
    if !enabled {
        disabled.push(id.to_string());
    }
    db.set_disabled_modules(&disabled)?;
    Ok(())
}
```

The order keeps a failure from leaving the app and the saved choice in disagreement. A static lock, `MODULE_SWITCH`, serializes switches. The module changes state first, through `resume` or `suspend`. Only after that succeeds is the new list of disabled ids written. If the module fails, the stored list is unchanged, and the error goes back to the menu. The rule is [modules.lifecycle](../reference/modules.md#r-modules.lifecycle).

Every module except the music player is built at startup, even when the user has it switched off, because it is initialized with `enabled` set to `false`. The music player is the exception. At startup it is not built at all when it is disabled. When the user switches it on and its state is missing, `apply_module_enabled` calls `start_music_module` instead of `resume`, so the module is built and started from scratch.

Note: the menu asks for a switch only for modules whose `metadata.json` says `"backend": true`, the rule [modules.states](../reference/modules.md#r-modules.states). Such a module needs a `match` arm here, or the switch fails with `unknown module`.

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

The checks are text checks. `lint_modules_integrity` searches for the quoted id in the registry file, and it searches for the crate name in `Cargo.toml`. A text check cannot prove that the kernel calls `init`, which is why section 4.4 warns about the lists.

Note: no lint checks that a module avoids the kernel. The kernel package has only binary targets (see `toolchain/Cargo.toml`), so a module cannot link it as a library, and nothing else enforces the rule. The `udtk` book, in [how a module joins the bridge](../udtk/ch15-how-a-module-joins-the-bridge.md), states the same gap.
## 4.9 Summary

A module is a crate with a `Module` implementation and a folder of page code. The kernel constructs it by name in `setup`, passes it a `ModuleContext` through `init`, and calls `shutdown` at exit. A user switch goes through `apply_module_enabled`. Modules exchange data only through `emit`, `subscribe`, and `state`, and the build lints catch broken shapes early. The next chapter shows where a module's data lives. Continue with [Chapter 5. Storage](ch05-storage.md).
