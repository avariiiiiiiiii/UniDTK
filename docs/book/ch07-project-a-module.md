# Chapter 7. Project: Building a Module

This chapter is an exercise. The module `uptime` does not exist in the repository. It shows how long the PC has been running, and it adds the module step by step. Every step copies a real pattern from `glyph-bar`, the smallest module with a backend, and names that file. The design has one read-only command, `get_uptime`, and no background thread. Its metadata has no `backend` flag, so the menu offers only "on the desk" and "hidden", and the kernel needs no switch command.

## 7.0 The plan

The work touches the crate, the registry, the kernel, and the page. Sections 7.1 to 7.3 make the crate and register it. Sections 7.4 to 7.5 add the kernel parts. Sections 7.6 and 7.7 add the page. The `tauri.conf.json` file does not change, as section 7.8 explains.

## 7.1 The crate

Open `modules/glyph-bar/rust/Cargo.toml` and copy its shape. Its `[lib]` section names the crate, and its only project dependency is `unidtk-api`, reached by a relative path. Nothing points at the kernel, which is the rule [modules.contract-only](../reference/modules.md#r-modules.contract-only).

Filename: modules/uptime/rust/Cargo.toml

```toml
[package]
name = "unidtk-mod-uptime"

[lib]
name = "unidtk_mod_uptime"
path = "uptime.rs"

[dependencies]
unidtk-api = { path = "../../../core/api" }
serde_json = "1"

[target.'cfg(windows)'.dependencies.windows]
version = "0.58"
features = ["Win32_System_SystemInformation"]
```

The Windows crate sits in a `cfg(windows)` section, as in `glyph-bar`. The feature name is unverified, because the exercise is not built. If the build reports `GetTickCount64` as unresolved, change this line to the feature that gates it.

## 7.2 The backend

The backend follows the layout of `modules/glyph-bar/rust/glyph_bar.rs`: a struct, a factory, a router named `ipc_call`, and the `Module` impl.

Filename: modules/uptime/rust/uptime.rs

```rust
//! Uptime: seconds since the PC booted, from the Windows tick counter.
//! No background thread: the value is read when asked.

pub struct UptimeModule;

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(UptimeModule)
}

/// `GetTickCount64` counts milliseconds since the system started.
#[cfg(windows)]
pub fn uptime_seconds() -> Result<u64, String> {
    let ms = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    Ok(ms / 1000)
}

#[cfg(not(windows))]
pub fn uptime_seconds() -> Result<u64, String> {
    Err("uptime is available on Windows only".to_string())
}

/// Answers the commands this module owns, as `glyph_bar::ipc_call` does.
pub fn ipc_call(
    cmd: &str,
    _args: &serde_json::Value,
    _host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        "get_uptime" => Some(uptime_seconds().map(|seconds| serde_json::json!(seconds))),
        _ => None,
    }
}

impl unidtk_api::Module for UptimeModule {
    fn id(&self) -> &'static str {
        "uptime"
    }

    fn init(&self, _ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        Ok(())
    }
}
```

`ipc_call` returns `None` for a name it does not own, and the router reports that as a build inconsistency, so each inventory name needs an arm. The name `"get_uptime"` must equal the inventory name of step 6. `uptime_seconds` is a plain `pub fn`, not a Tauri command, because a library crate must not mark a command `pub` (see [modules.commands-in-core](../reference/modules.md#r-modules.commands-in-core)). The kernel wraps it in step 5. The read is idempotent, as [modules.idempotent](../reference/modules.md#r-modules.idempotent) requires. `init` does nothing, because `uptime` has no thread; `glyph-bar` starts its thread there.

## 7.3 The workspace and the registry

The kernel package, `toolchain/Cargo.toml`, lists each module crate by path. Add the new one next to `glyph-bar`.

Filename: toolchain/Cargo.toml (abridged)

```toml
unidtk-mod-glyph-bar = { path = "../modules/glyph-bar/rust" }
unidtk-mod-uptime = { path = "../modules/uptime/rust" }
```

The registry, `core/kernel/registry.rs`, holds a factory for each module and the list of ids. Add one line to each.

Filename: core/kernel/registry.rs (abridged)

```rust
// Inside all_modules(): one factory line per module.
unidtk_mod_glyph_bar::create_module(),
unidtk_mod_uptime::create_module(),

// Inside ALL_MODULE_IDS: one id per module.
"glyph-bar", "uptime",
```

The build lint `lint_modules_integrity` searches for `"uptime"` here and for `unidtk-mod-uptime` in `toolchain/Cargo.toml`.

Warning: the kernel's startup does not call `all_modules()`; only the CLI does, for `udtk doctor`. A module listed here but not in the setup block of step 5 is checked by the doctor and never started by the app.

## 7.4 The kernel: import, startup, and wrapper

The kernel binary is `core/kernel/main.rs`. It needs three additions. The import goes with the other module imports at the top.

Filename: core/kernel/main.rs (abridged)

```rust
use unidtk_mod_glyph_bar as glyph_bar;
use unidtk_mod_uptime as uptime;
```

The startup block goes in `setup`, after the `glyph-bar` block, and it follows the same three steps: construct, initialize, register. The uptime module has no backend, so the user cannot switch it off, and its `enabled` flag is always true.

Filename: core/kernel/main.rs (abridged)

```rust
let glyph_bar_module = glyph_bar::GlyphBarModule::new();
unidtk_api::Module::init(&glyph_bar_module, module_context(app.handle(), "glyph-bar", !disabled_mods.contains("glyph-bar")))?;
app.manage(glyph_bar_module);

let uptime_module = uptime::UptimeModule;
unidtk_api::Module::init(&uptime_module, module_context(app.handle(), "uptime", !disabled_mods.contains("uptime")))?;
app.manage(uptime_module);
log_line("setup: uptime module ready");
```

The Tauri wrapper is the third addition. Tauri commands must be declared in the kernel, where `generate_handler!` lives.

Filename: core/kernel/main.rs (abridged)

```rust
#[tauri::command(async)]
fn get_uptime() -> Result<u64, String> {
    uptime::uptime_seconds()
}
```

Add `get_uptime,` to the `generate_handler!` list after `set_system_mute,`, because that list is the set of commands the page may call.

Warning: the command name must match in four places: the page's `invoke("get_uptime")`, the wrapper, the inventory declaration in step 6, and the router's arm in step 2. The tests `frontend_invoke_names_are_registered` and `every_manifest_ui_command_is_registered` in `core/kernel/main.rs` fail when the names disagree.

## 7.5 The command inventory and the socket route

The command inventory, `core/kernel/command_inventory.rs`, is the one place that declares what can be asked. It is compiled into both the application and the CLI.

Filename: core/kernel/command_inventory.rs (abridged)

```rust
pub const GET_UPTIME: CommandSpec =
    CommandSpec::read_only("get_uptime", "Seconds since the PC booted", "");
```

Add `&GET_UPTIME,` to the `ALL` list in the same file. `read_only` serves the command on both doors, the socket the CLI uses and the page. The test `socket_surface_is_read_only` requires every socket command to be read-only. A command that changes the machine would need `CommandSpec::write`, and it would stay off the socket.

The socket routes are in `core/kernel/ipc.rs`. A read-only command needs an entry in `ROUTES`, the table of handlers, and an entry in `MODULE_ROUTES`, the table of module answerers.

Filename: core/kernel/ipc.rs (abridged)

```rust
use unidtk_mod_uptime as uptime;

static ROUTES: &[(&str, Handler)] = &[
    (crate::command_inventory::GET_UPTIME.name, handle_get_uptime),
    // ...
];

fn handle_get_uptime(server: &IpcServer, args: &Args) -> Result<serde_json::Value, String> {
    call_module(server, "uptime", crate::command_inventory::GET_UPTIME.name, args)
}

static MODULE_ROUTES: &[ModuleRoute] = &[
    ("uptime", uptime::ipc_call),
    // ...
];
```
## 7.6 The page: metadata, icon, widget, and style

The page half lives in `modules/uptime/ui/`. The metadata comes first. Its `specialType` is the field that matters, because the core's mount branch reads it. Without it the card would get only the generic shortcut view.

Filename: modules/uptime/ui/metadata.json

```json
{
  "name": "Uptime",
  "description": "Time since the PC started",
  "specialType": "uptime"
}
```

Filename: modules/uptime/ui/icon.svg

```xml
<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
  <circle cx="12" cy="12" r="9"/>
  <path d="M12 7v5l3 2"/>
</svg>
```

The widget follows `glyph-bar/ui/widget.js`: it marks the card, appends its own element, and starts its work. The timer is the one new part, and it stops itself because the core has no hook for stopping it.

Filename: modules/uptime/ui/widget.js

```js
import { invoke } from '@unidtk/bridge';

const TICK_MS = 1000;

/** 93784 -> "1d 02:03:04", 3723 -> "01:02:03". */
function formatUptime(total) {
  const pad = (n) => String(n).padStart(2, "0");
  const days = Math.floor(total / 86400);
  const hms = `${pad(Math.floor((total % 86400) / 3600))}:${pad(Math.floor((total % 3600) / 60))}:${pad(total % 60)}`;
  return days > 0 ? `${days}d ${hms}` : hms;
}

export function renderUptime(card) {
  card.classList.add("no-header", "uptime-widget");
  card.insertAdjacentHTML("beforeend", `<div class="uptime-value">--:--:--</div>`);
  const value = card.querySelector(".uptime-value");

  const tick = async () => {
    // The core does not stop this timer when the card is removed.
    if (!card.isConnected) {
      clearInterval(timer);
      return;
    }
    try {
      value.textContent = formatUptime(Number(await invoke("get_uptime")));
    } catch (_err) {
      value.textContent = "--:--:--";
    }
  };
  const timer = setInterval(tick, TICK_MS);
  tick();
}
```

The core calls `renderUptime(card, widgetId)`, and the function ignores the second argument. The `try` keeps a failed call from becoming an unhandled rejection.

The style uses tokens for color, as the rule [theme.tokens-only](../reference/desk.md#r-theme.tokens-only) requires. `--fg-rgb` is defined in `core/ui/styles.css`.

Filename: modules/uptime/ui/uptime.css

```css
.widget-card.uptime-widget .uptime-value {
  padding: 8px 12px;
  white-space: nowrap;
  color: rgba(var(--fg-rgb), 0.9);
  font-variant-numeric: tabular-nums;
}
```

## 7.7 The mount, the click rule, and the stylesheet

The page needs three small edits in shared files. The first is a mount branch in `buildWidgetCard` in `core/ui/unidtk-core.js`, next to the `glyph-bar` branch.

Filename: core/ui/unidtk-core.js (abridged)

```js
} else if (w.specialType === "uptime") {
  import("../modules/uptime/widget.js").then((m) => m.renderUptime(card, w.id)).catch(() => {});
}
```

The second is the click exclusion. A condition in the same function skips the generic click handler for the built-in types, and `uptime` must join that list. Without it, a click on the card runs the generic toggle, which the uptime widget does not want.

Filename: core/ui/unidtk-core.js (abridged)

```js
if (w.specialType !== "mp3-player" && w.specialType !== "glyph-bar" && w.specialType !== "telegram"
    && w.specialType !== "micrographics" && w.specialType !== "uptime") {
  // the generic click handler: toggles the shortcut view
}
```

The third is the stylesheet line in `core/ui/index.html`, next to the glyph-bar line. Without it the widget appears unstyled, because a module's CSS is never loaded by itself.

Filename: core/ui/index.html (abridged)

```html
<link rel="stylesheet" href="../modules/uptime/uptime.css" />
```

## 7.8 The build-time checks

The build checks of [Chapter 4](ch04-modules.md) run before the kernel compiles. For `uptime` they pass: the ids and crate names are in place, `widget.js` imports only the bridge, and no `simulate` block is required.

Nothing in `tauri.conf.json` changes, because its resources section copies the whole `dist/modules/` folder.

## 7.9 Build, install, and check

Build and install with `udtk build`, the only release build command. It stops the running app, builds, installs into `C:\Program Files\UniDTK`, and starts the app again. Then check the result.

1. `udtk doctor -modules` runs the module checks. `uptime` declares no dependencies, and its dry-run `init` has a two-second limit. Confirm that no check for `uptime` fails.
2. `udtk invoke get_uptime` returns the seconds since boot. Run it twice, a few seconds apart, and the second value is larger.
3. `udtk logs --source frontend --limit 30` shows page lines, where a failed mount appears. `udtk logs --level ERROR` shows backend errors.
4. `udtk shot` saves a screenshot and prints its path. The uptime card should show a time such as `01:02:03`, not `--:--:--`.

Note: if the invoke works and the card does not, the fault is in the page; if the invoke fails, it is in the kernel or the module.

## 7.10 What the exercise left out

The exercise has no switch, event, or `simulate` block. A module with a thread, such as the volume module, needs the extra steps of [Chapter 4](ch04-modules.md).

Summary: a module is a crate, a few registry lines, a startup block, a command declared once and routed twice, and a folder of page files. Each part has a counterpart in `glyph-bar`. Continue with [Chapter 8. Themes and motion](ch08-themes-and-motion.md).
