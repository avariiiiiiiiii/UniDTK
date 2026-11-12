# Chapter 6. Design decisions

This chapter records the decisions behind the structure. The history of how each was reached is in [Part V](../history/rfcs/udtk-migration.md).

## 6.1 One contract, two doors

The command surface used to be written down three times: a `match` in the kernel, a list in the `commands` reply, and the Tauri command list. Adding a module command meant editing the kernel in three places. The decision was to describe the surface **once**, as data (the inventory of Chapter 2), and to make both doors read that data. The kernel's own Tauri commands stay handwritten, because they carry the Tauri injection (`State`, `AppHandle`) that only the kernel can express, and a test checks their names against the inventory. A module's commands have no Tauri wrapper: the kernel's `invoke_handler` sends them to the module's table, which declares and serves each one in a single line.

## 6.2 A manifest built into both binaries

The manifest is compiled into the kernel and into `udtk`: the kernel's commands as constants, each module's as the table in its crate. The offline CLI lists the modules' commands by building them with `registry::all_modules()` and asking each for `Module::commands()`. That is safe only because building a module starts nothing: the VPN auto-connect, TDLib and the player's library load all begin in `init`, which the CLI never calls (the same holds for `doctor -env`, which reads `Module::dependencies()`). The running kernel does not build second instances at all: its `commands` reply reads the tables of the modules in its registry.

## 6.3 Modules own their state

The kernel does not name module types in its router, and it registers no module state with Tauri. Each module keeps its own state and declares its commands in one table in its crate (`unidtk_api::module_commands!`), which generates the specs and the dispatch together. A module adds a command by adding one line to its table. No kernel code changes, on any door: the socket, the WebSocket and Tauri's `invoke` all reach the module through `serve_module` and the registry.

## 6.4 Rejected alternatives

| Alternative | Why it was rejected |
|---|---|
| A procedural macro `#[udtk_command]`, plus scanning `modules/*/rust` in `build.rs` | Two parsers of the same thing. A text scan also catches comments and disabled code |
| Generating the bodies of the Tauri wrappers | The Tauri injection is known only to the kernel, and `include!` from `OUT_DIR` breaks the IDE and incremental builds |
| Linker magic (`inventory`, `linkme`) | On MSVC, `/OPT:REF` strips unreferenced sections, so entries silently vanish |
| A long-lived CLI daemon that holds session state | It would move logic into the CLI. The CLI stays a thin client, one batch per `gui` run |
| Write commands on the socket, "right away" | It lowers the security perimeter without a threat model |
| The `help --json` answer taken from the socket | It would make the diagnostics depend on the running app |

The reference projects were used for technique, not as dependencies. `agent-browser` (Rust and CDP, element references taken from a snapshot) gave the refusal on an occluded target and the stable references. Chrome DevTools MCP gave the idea of targeted queries instead of a full snapshot after each action.

## 6.5 Owner's decisions, and the accepted risk

The owner decided four things, and they are the basis of the current design:

1. The page is moved entirely to the new protocol (the radical option). Authentication of the door is therefore a required stage, not an option.
2. Module state moves into the modules. This is the condition for a third-party developer to add a command without editing the kernel.
3. `metadata.json` is extended with the render entry points for the probe (see [Chapter 11](ch11-doctor-the-diagnostic-loop.md#chapter-11-doctor-the-diagnostic-loop)).
4. CDP stays enabled always. The port is no longer hard-coded, and the `dev` and `--debug` modes are retired.

The accepted, recorded risk: a local process of the same user can execute JavaScript in the application, and the token does not prevent it. The security boundary is the Windows user account. The plan was written with the honest limit stated, and it is repeated here so that it is not forgotten.
