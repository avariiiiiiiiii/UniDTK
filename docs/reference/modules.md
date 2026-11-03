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

r[modules.commands-in-core]

`#[tauri::command]` is declared only in the crate that holds `generate_handler!`, which is the kernel. A library crate must not mark a command `pub`: the macro exports `__cmd__<name>` to the crate root, and two of them collide with error `E0255`. A module exposes plain functions; the kernel wraps them in thin commands.

r[modules.idempotent]

Commands must be safe to repeat. The interface, the CLI, and tests call them from different contexts, and a retry must not double an effect.

## The contract

The contract crate is `core/api` (package `unidtk-api`). It must not depend on `tauri`, `redb`, or Win32.

- `Module` trait: `id`, `dependencies`, `init(ModuleContext)`, `shutdown(budget)`, `suspend`, `resume(ModuleContext)`.
- Services: `log`, `emit`, `subscribe`, `state`, and `paths`.
- Storage: `ModuleContext::store(StoreSpec)` returns a `ModuleDb`. See [Storage](storage.md).

## Switching a module on and off

r[modules.lifecycle]

`set_module_enabled {id, enabled}` is the only command that switches a module in the running application (`apply_module_enabled` in `core/kernel/main.rs`). It switches the module first and writes the stored list of disabled modules only after that succeeds, so a failure leaves both unchanged. Switches are serialized by `MODULE_SWITCH`.

By default `suspend` calls `shutdown` with a 1500 ms budget, and `resume` calls `init` with a fresh context. The application keeps running across a switch, so neither may set a global exit latch. A module that was off since startup was never built, so enabling it builds it from scratch.

r[modules.states]

A module is in one of three states (`moduleState` in `core/ui/menu.js`): `off`; `background`, where the backend keeps running and the card is hidden (only for modules whose metadata says `backend: true`); and `desk`, where the card is on the desk. A module without a backend has only `off` and `desk`.

r[modules.metadata]

`metadata.json` declares how the menu and the desk treat a module: `backend: true` allows the background state, `settingsView` names the view that "Configure…" opens, `tints` and `lookOptions` give its share of the theme, and `container` and `embeddable` decide nesting. `lint_module_contracts()` in `toolchain/build.rs` checks the theme slots and the nesting fields and fails the build on an error.
