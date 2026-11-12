# Modules

A module is a vertical slice of one feature: its backend and its interface, side by side, behind one contract. This chapter states the shape of a module, what it may depend on, and how it is switched on and off.

## The shape of a module

r[modules.vertical-slice]

A module lives in `modules/<id>/`:

```text
modules/<id>/
  rust/      crate unidtk-mod-<id>: the backend half
  ui/        widget.js + metadata.json + icon.svg: the interface half
  helper/    optional second binary (vpn-helper.exe for vpn)
```

The two halves of one feature are never split across modules. A module without a backend has only `ui/` (today that is `clock`). Built-in modules ship inside the application resources; there is no user `mods/` directory.

The modules are `clock`, `explorer`, `glyph-bar`, `micrographics`, `mp3-player`, `telegram`, and `vpn`. Each one is described in [modules/](../modules/README.md).

r[modules.compiled-crates]

Modules are compiled crates, linked into the application. There are no dynamic plugins (DLLs loaded at run time), no Lua, and no WASM: those add an ABI or a second runtime, and every module is trusted code anyway.

## What a module may depend on

r[modules.contract-only]

A module crate depends on `unidtk-api` and never on the kernel crate. Tauri host types (`AppHandle`, `Emitter`, `async_runtime`) are allowed, because a module that owns its own runtime needs them. `lint_modules_integrity()` in `toolchain/build.rs` fails the build when this is broken.

r[modules.no-module-to-module]

Modules do not call each other, and Cargo must not allow one module crate to depend on another. They talk only through the contract's events: `unidtk_api::emit`, `subscribe`, and `state`.

r[modules.command-table]

A module declares each of its commands once, in a `unidtk_api::module_commands!` table in its own crate: the access, the name, a summary, the arguments and the body. The table generates the module's `COMMANDS` and its `dispatch_command`, which the module returns from `Module::commands` and `Module::dispatch`, so a declared command always has a handler. The kernel's registry maps each name to its module and refuses a name that two modules, or a module and the kernel, both claim. Every door serves a module command through `ipc::serve_module` in `core/kernel/ipc.rs`, on a thread that is never the main thread; the Tauri door reaches it from the kernel's `invoke_handler`, so a module has no `#[tauri::command]`. An argument is read by its Rust name in camelCase, then in snake_case. A read that only the interface needs is `read_ui`, so the socket surface does not grow with it ([ipc.socket-read-only](ipc.md#r-ipc.socket-read-only)).

r[modules.idempotent]

Commands must be safe to repeat. The interface, the CLI, and tests call them from different contexts, and a retry must not double an effect.

## The contract

The contract crate is `core/api` (package `unidtk-api`). It must not depend on `tauri`, `redb`, or Win32.

- `Module` trait: `id`, `dependencies`, `starts_after`, `init(ModuleContext)`, `begin_shutdown`, `shutdown(budget)`, `suspend`, `resume(ModuleContext)`, `commands`, `dispatch`.
- Commands: `CommandSpec` and the `module_commands!` table (`unidtk_api::commands`).
- Services: `log`, `emit`, `subscribe`, `state`, and `paths`.
- Storage: `ModuleContext::store(StoreSpec)` returns a `ModuleDb`. See [Storage](storage.md).

## Switching a module on and off

r[modules.lifecycle]

The kernel's `Registry` (`core/kernel/registry.rs`) holds every module of a run and is the only place that starts, switches and stops them. It starts the modules in an order where each comes after the ids its `starts_after` names; an unknown id or a cycle is a startup error. On exit it calls `begin_shutdown` on every module, which only raises exit latches and never waits, and then `shutdown` in reverse start order. A zero budget means "do not wait": the caller is the main thread.

`set_module_enabled {id, enabled}` is the only command that switches a module in the running application (`apply_module_enabled` in `core/kernel/main.rs`). It switches the module first and writes the stored list of disabled modules only after that succeeds, so a failure leaves both unchanged. Switches are serialized by `MODULE_SWITCH`; the exit takes the same lock, and a switch after the exit began is refused.

By default `suspend` calls `shutdown` with a 1500 ms budget, and `resume` calls `init` with a fresh context. The application keeps running across a switch, so neither may set a global exit latch. A module switched off at startup is still built and initialized with `enabled` false, and must start nothing heavy; its `resume` starts what `init` skipped.

r[modules.states]

A module is in one of three states (`moduleState` in `core/ui/menu.js`): `off`; `background`, where the backend keeps running and the card is hidden (only for modules whose metadata says `backend: true`); and `desk`, where the card is on the desk. A module without a backend has only `off` and `desk`.

r[modules.metadata]

`metadata.json` declares how the menu and the desk treat a module: `backend: true` allows the background state, `settingsView` names the view that "Configure…" opens, `tints` and `lookOptions` give its share of the theme, `container` and `embeddable` decide nesting, `defaultPlacement` (the card's position and settings) and `defaultParent` (the container it starts in) make up the default desk layout, which the page assembles in `getDefaultWidgetConfig` (`core/ui/widgets-api.js`) for the first run and "Reset positions", and `motion.open` names the selector in its desk card whose click opens a surface with no other effect (empty means the card), for the motion budget of `udtk doctor -simulate`. `lint_module_contracts()` in `toolchain/build.rs` checks the theme slots, the nesting fields, that a `defaultPlacement` has numeric `x`, `y`, `w` and `h` and a `defaultParent` names a container module, and that a `motion.open` class is named in the module's `ui/*.js`, and fails the build on an error.
