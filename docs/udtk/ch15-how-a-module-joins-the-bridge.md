# Chapter 15. How a module joins the bridge

A module is a vertical slice, `modules/<id>/{rust,ui}`. Its back half, `rust/`, is a crate. Its front half, `ui/`, is the page code. The bridge reads a module in four places. Each has a rule, and most of the rules are enforced by a build lint or a test.

## 15.1 Dependencies are data

A module declares its external needs in `Module::dependencies()`, as a static list of `ModuleDependency` values. The types are in [core/api/src/manifest.rs](../../core/api/src/manifest.rs):

```rust
fn dependencies(&self) -> &'static [manifest::ModuleDependency] { &[] }
```

- `DependencySeverity::{Mandatory, Optional}`: a missing mandatory dependency is a FAIL, and an optional one is a WARN.
- `DependencyKind::{DynamicLibrary, Executable, ModelAsset, WindowsTask}`: what the dependency is.

Diagnostics read this list. A module has no diagnostic code of its own. The old method `diagnostic_checks` was removed from the `Module` trait.

## 15.2 The registry

A module is registered in [core/kernel/registry.rs](../../core/kernel/registry.rs): in `all_modules()`, and in the constant `ALL_MODULE_IDS`. The current list is `glyph-bar`, `explorer`, `vpn`, `telegram`, `mp3-player`, and `micrographics`. The module must also be a dependency in [toolchain/Cargo.toml](../../toolchain/Cargo.toml). The kernel and the CLI both use the registry, so there is no second list. (The CLI includes `registry.rs` with `#[path]`.)

## 15.3 Commands of a module

A module declares its commands in one table, with the `module_commands!` macro from [core/api/src/commands.rs](../../core/api/src/commands.rs):

```rust
unidtk_api::module_commands! {
    impl VpnModule {
        read_only "vpn_status" "VPN runtime status" |this| Ok(status(&this.runtime));
        write "vpn_connect" "Connect to a server" |this, tag: Option<String>| connect(&this.runtime, tag);
    }
}
```

Each line gives the access (`read_only`, `read_ui`, or `write`), the name, a summary, the arguments with their types, and a body that returns `Result<T, String>`. The macro generates the module's `COMMANDS` and its `dispatch_command`, which the module returns from `Module::commands` and `Module::dispatch`, so a declared command cannot lack a handler. To add a command, the module adds one line; the kernel does not change. The registry refuses at startup a name that two modules, or a module and the kernel, both declare, and the test `the_real_modules_claim_distinct_commands` fails before that.

A write must also be listed in the facade's `WS_WRITES`, and a read in `WS_COMMANDS`, once it is verified. Chapter 4 explains why.

## 15.4 The build lints

Lints run in [toolchain/build.rs](../../toolchain/build.rs) at compile time. They are cheap, and they stop a broken build before it starts:

| Lint | What it checks | Error text |
|---|---|---|
| `lint_modules_integrity` | Every `modules/*/rust/Cargo.toml` is in the registry and in `Cargo.toml` | `BUILD INTEGRITY ERROR: Rust module '<id>' ... is missing from core/kernel/registry.rs` |
| `lint_ui_imports` | No UI file imports Tauri directly or through the vendor path. Only the facade may | `UI IMPORT ERROR: ... imports Tauri directly; the interface must go through the bridge` |
| `lint_module_contracts` | Each module's `ui/metadata.json` is well formed: an `embeddable` entry names a file that contains its render function, a container is not embeddable, and tints and packs have ids and labels | `<file>: embeddable.render <name> is not in <module>` (and similar) |
| `lint_simulate_metadata` | Each `simulate` entry names an existing file that defines the function, at least one entry exists, and the production CLI has no hard-coded render names (the list is `ENTRY_POINTS`, six names including `renderMicrographics`) | (metadata check) |

The rule for modules is narrower than it was. A module must not depend on the kernel crate. It must depend on `unidtk-api`. Tauri host types are allowed, because four of the five modules already use them, and the contract itself gives the host handle (see the decision recorded in [Chapter 6](ch06-design-decisions.md#chapter-6-design-decisions)). **No lint in `build.rs` enforces this rule as of this rewrite.** The plan planned such a check in `lint_modules_integrity`, and the code of that function does not contain it (see [Chapter 18](ch18-known-gaps-and-open-decisions.md#chapter-18-known-gaps-and-open-decisions)).

## 15.5 Guarantees that are tests, not build checks

Some guarantees cannot be checked by a build script, because `generate_handler!` is a macro, and a build script cannot read it. These are tests:

- `every_socket_command_has_a_handler` and `every_command_has_a_handler`: every kernel command has a route. A module command has one by construction.
- `every_manifest_ui_command_is_registered`: every kernel command declared for the UI door is registered in the kernel's `generate_handler!`.
- `frontend_invoke_names_are_registered`: every literal name in `invoke("...")` in the interface is a kernel command or a command in a module's table. Plugin commands, such as `plugin:fs|...`, are skipped.
- `frontend_listen_names_exist_in_rust`: every literal name in `listen("...")` is a string literal in the Rust code. This proves the name exists, not that the event is emitted on every path. A text analysis cannot be stricter, because some names are built dynamically.
- `simulate_probes_name_real_socket_commands`: the probes of `doctor -simulate` name real read-only socket commands.

Each of these was checked with a negative test: a deliberately broken name made it fail, and the restored name made it pass.
