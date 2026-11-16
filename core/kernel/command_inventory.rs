//! The command inventory: one place that knows what the application can be asked.
//!
//! This file is compiled into **both** the application and the `udtk` bridge (the CLI
//! includes it with `#[path]`, the same way it already includes `registry.rs`), so the
//! offline manifest and the served surface cannot disagree about names.
//!
//! It is deliberately free of kernel-only types (`tauri`, `redb`, Win32) — that is
//! what makes the shared compilation possible.
//!
//! Scope: [`ALL`] describes the kernel's own commands. Each module declares its
//! commands with its handlers, in the `unidtk_api::module_commands!` table of its own
//! crate, and [`manifest`] lists both: the kernel's, then the modules'.

use unidtk_api::commands::CommandSpec;

// ── Kernel-owned infrastructure ────────────────────────────────────────────────

pub const PING: CommandSpec =
    CommandSpec::read_only_cli("ping", "Liveness probe; also reports the CDP port", "");
pub const LOGS: CommandSpec = CommandSpec::read_only_cli(
    "logs",
    "Read journal records from redb",
    "[--limit N] [--since ID] [--level LEVEL] [--source SOURCE]",
);
pub const LOG_STATS: CommandSpec =
    CommandSpec::read_only("log_stats", "Journal statistics (levels, sources, total)", "");
pub const LOG_MAX_ID: CommandSpec =
    CommandSpec::read_only_cli("log_max_id", "Highest journal record id", "");
pub const LOG_DELTA: CommandSpec = CommandSpec::read_only_cli(
    "log_delta",
    "Records after a given id plus the new high-water mark, in one round-trip",
    "{since, limit}",
);
pub const GET_WIDGET_CONFIG: CommandSpec =
    CommandSpec::read_only("get_widget_config", "Widget layout configuration", "");
pub const GET_DISABLED_MODULES: CommandSpec =
    CommandSpec::read_only("get_disabled_modules", "Which modules are disabled (backend off)", "");
pub const SET_MODULE_ENABLED: CommandSpec =
    CommandSpec::write("set_module_enabled", "Turn a module on or off in the running app", "{id, enabled}");
pub const COMMANDS: CommandSpec =
    CommandSpec::read_only_cli("commands", "List the command surface", "");

/// Appends a frontend record to the journal — the command the interface actually calls.
///
/// Kept off the socket: an unauthenticated door that can write log records lets any
/// local process forge or erase evidence.
///
/// The name has to be the one the kernel registers (`frontend_log`). While it read
/// `log`, this entry promised a command that no door served, and the interface's real
/// call looked undeclared — the gate test in `main.rs` now compares the manifest against
/// the handler list so that cannot come back.
pub const FRONTEND_LOG: CommandSpec =
    CommandSpec::write("frontend_log", "Append a frontend record to the journal", "{message, level?}");

pub const WEATHER_LOCATION_REQUEST: CommandSpec =
    CommandSpec::write("weather_location_request", "Request Windows location permission for weather", "");
pub const DESK_HOST_GET: CommandSpec =
    CommandSpec::read_only("desk_host_get", "Active desktop host mode (web or native)", "");
pub const DESK_HOST_SET: CommandSpec =
    CommandSpec::write("desk_host_set", "Switch desktop host mode between web and native", "{mode}");
pub const DESK_POSITION_SET: CommandSpec = CommandSpec::write(
    "desk_position_set",
    "Merge the position fields of one widget into the desk layout and publish it",
    "{id, x?, y?, ax?, ay?, w?, h?, anchor?, version?}",
);
pub const DESK_APPLY: CommandSpec = CommandSpec::write(
    "desk_apply",
    "Apply a list of named desk operations (position, place, forget, hide, show, nest, eject, instances, theme) in one transaction and publish the layout",
    "{ops: [{op, ...}], baseRev?}",
);
// The desk's folders, shortcuts and quitting, served on the interface door for the native
// host as well as through Tauri for the page (r[ipc.one-contract]).
pub const GET_FOLDERS: CommandSpec = CommandSpec::read_ui("get_folders", "The desktop folders with their shortcuts", "");
pub const SAVE_FOLDER: CommandSpec = CommandSpec::write("save_folder", "Create or replace one desktop folder record", "{folder}");
pub const DELETE_FOLDER: CommandSpec = CommandSpec::write("delete_folder", "Delete one desktop folder record", "{id}");
pub const MISSING_TARGETS: CommandSpec = CommandSpec::read_ui("missing_targets", "Which shortcut targets no longer exist", "{targets}");
pub const PICK_SHORTCUT_TARGET: CommandSpec = CommandSpec::write("pick_shortcut_target", "Ask the user for a file or folder in the system dialog and describe it", "{folder}");
pub const CLIPBOARD_SHORTCUT_TARGETS: CommandSpec = CommandSpec::read_ui("clipboard_shortcut_targets", "The files or the text on the clipboard, for a folder's paste", "");
pub const DESCRIBE_SHORTCUT_TARGET: CommandSpec = CommandSpec::read_ui("describe_shortcut_target", "A shortcut draft (name, kind) for a path or a link", "{target}");
pub const SHORTCUT_ICON: CommandSpec = CommandSpec::read_ui("shortcut_icon", "The icon of a shortcut target as BGRA pixels", "{target, size?}");
pub const START_MENU_LIST: CommandSpec = CommandSpec::read_ui("start_menu_list", "The programs of the Start menu", "");
pub const LAUNCH_SHORTCUT: CommandSpec = CommandSpec::write("launch_shortcut", "Run a shortcut's target with its arguments", "{target, args?}");
pub const OPEN_EXPLORER: CommandSpec = CommandSpec::write("open_explorer", "Open the shell's Explorer at a path", "{path?}");
pub const QUIT_APP: CommandSpec = CommandSpec::write("quit_app", "Quit UniDTK", "");
pub const DESK_LAYOUT_GET: CommandSpec = CommandSpec::read_only(
    "desk_layout_get",
    "The stored desk layout with the revision that the desk-layout-changed events continue from",
    "",
);
pub const HOST_REGISTER: CommandSpec = CommandSpec::write(
    "host_register",
    "Register supervised native desktop host state",
    "{pid, generation, instanceNonce, inspectorPort, attached, ready}",
);
pub const SET_DESKTOP_INPUT_FOCUS: CommandSpec = CommandSpec::write(
    "set_desktop_input_focus",
    "Report focused text-input state from the active desktop host",
    "{focused}",
);
pub const HOST_PING: CommandSpec = CommandSpec::read_only_cli(
    "host_ping",
    "Read native desktop host status via kernel proxy",
    "",
);
pub const HOST_TREE: CommandSpec = CommandSpec::read_only_cli(
    "host_tree",
    "Read native desktop element tree via kernel proxy",
    "",
);
pub const HOST_FRAMES: CommandSpec = CommandSpec::read_only_cli(
    "host_frames",
    "Read native desktop frame metrics via kernel proxy",
    "",
);
pub const HOST_HIT: CommandSpec = CommandSpec::read_only_cli(
    "host_hit",
    "Test native desktop hit region at (x, y) via kernel proxy",
    "{x, y}",
);

/// Read-only health probes `doctor -simulate` runs against the live app.
///
/// Kept next to the commands they call: a probe that names a command the
/// inventory does not know is a lie by construction. Adding a new health
/// probe is one line here; `run_simulation` iterates this table instead of
/// hard-coding `{"cmd": …}` literals.
///
/// The CLI binary (`udtk`) includes this file with `#[path]` and reads these
/// items; the kernel binary does not, hence the `dead_code` allowance.
#[allow(dead_code)]
pub struct SimulateProbe {
    pub label: &'static str,
    pub command: &'static str,
    pub args: &'static str,
}

/// Liveness first: a dead IPC answers nothing else.
#[allow(dead_code)]
pub const SIMULATE_PING: &str = PING.name;
/// Journal health.
#[allow(dead_code)]
pub const SIMULATE_LOG_STATS: &str = LOG_STATS.name;
/// Frontend exceptions audit.
#[allow(dead_code)]
pub const SIMULATE_LOGS: &str = LOGS.name;
/// Widget layout validation.
#[allow(dead_code)]
pub const SIMULATE_WIDGET_CONFIG: &str = GET_WIDGET_CONFIG.name;

/// Safe read-only probes of core modules. `vpn_status`/`telegram_status` are
/// cheap snapshots, never writes, so `simulate` stays non-destructive.
#[allow(dead_code)]
pub static SIMULATE_PROBES: &[SimulateProbe] = &[
    SimulateProbe {
        label: "Module VPN",
        command: "vpn_status",
        args: "{}",
    },
    SimulateProbe {
        label: "Module Telegram",
        command: "telegram_status",
        args: "{}",
    },
];

/// Everything the inventory knows, in declaration order.
pub static ALL: &[&CommandSpec] = &[
    &PING,
    &FRONTEND_LOG,
    &LOGS,
    &LOG_STATS,
    &LOG_MAX_ID,
    &LOG_DELTA,
    &GET_WIDGET_CONFIG,
    &GET_DISABLED_MODULES,
    &SET_MODULE_ENABLED,
    &COMMANDS,
    &WEATHER_LOCATION_REQUEST,
    &DESK_HOST_GET,
    &DESK_HOST_SET,
    &DESK_POSITION_SET,
    &DESK_APPLY,
    &GET_FOLDERS,
    &SAVE_FOLDER,
    &DELETE_FOLDER,
    &MISSING_TARGETS,
    &PICK_SHORTCUT_TARGET,
    &CLIPBOARD_SHORTCUT_TARGETS,
    &DESCRIBE_SHORTCUT_TARGET,
    &SHORTCUT_ICON,
    &START_MENU_LIST,
    &LAUNCH_SHORTCUT,
    &OPEN_EXPLORER,
    &QUIT_APP,
    &DESK_LAYOUT_GET,
    &HOST_REGISTER,
    &SET_DESKTOP_INPUT_FOCUS,
    &HOST_PING,
    &HOST_TREE,
    &HOST_FRAMES,
    &HOST_HIT,
];

/// Every command of the application: the kernel's, then each module's.
pub fn manifest() -> Vec<&'static CommandSpec> {
    ALL.iter()
        .copied()
        .chain(crate::registry::all_module_commands().into_iter().map(|(_, spec)| spec))
        .collect()
}

/// The subset actually served over the socket (the CLI's door).
#[allow(dead_code)]
pub fn socket_commands() -> Vec<&'static CommandSpec> {
    manifest().into_iter().filter(|spec| spec.on_socket()).collect()
}

/// Look a kernel command up by name; a module's command is the registry's.
pub fn find(name: &str) -> Option<&'static CommandSpec> {
    ALL.iter().copied().find(|spec| spec.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_described() {
        let manifest = manifest();
        for (index, spec) in manifest.iter().enumerate() {
            assert!(!spec.name.is_empty(), "command #{index} has no name");
            assert!(
                !spec.summary.is_empty(),
                "command '{}' has no summary — a command without a description is undocumented by construction",
                spec.name
            );
            let duplicates = manifest
                .iter()
                .filter(|other| other.name == spec.name)
                .count();
            assert_eq!(duplicates, 1, "command '{}' is declared twice", spec.name);
        }
    }

    #[test]
    fn socket_surface_is_read_only() {
        for spec in socket_commands() {
            assert!(
                !spec.is_write(),
                "write command '{}' must not be reachable over the unauthenticated socket",
                spec.name
            );
        }
    }

    /// A door that may retry must only serve commands where a repeat is harmless. If a
    /// non-idempotent command is ever exposed on the socket, this fails rather than
    /// letting a retry duplicate its effect.
    #[test]
    fn socket_surface_is_retry_safe() {
        for spec in socket_commands() {
            assert!(
                spec.is_retry_safe(),
                "command '{}' is served on a retryable door but is not marked retry-safe",
                spec.name
            );
        }
    }

    #[test]
    fn write_commands_are_marked_not_hidden() {
        // The policy is data, not silence: this write says so, and stays off the socket.
        let log = find("frontend_log").expect("frontend_log must be declared");
        assert!(log.is_write());
        assert!(!log.on_socket());
    }

    #[test]
    fn simulate_probes_name_real_socket_commands() {
        for probe in super::SIMULATE_PROBES {
            let spec = manifest().into_iter().find(|spec| spec.name == probe.command).unwrap_or_else(|| {
                panic!(
                    "simulate probe '{}' names '{}', which the inventory does not know",
                    probe.label, probe.command
                )
            });
            assert!(
                spec.on_socket(),
                "simulate probe '{}' calls '{}', which the socket door does not serve",
                probe.label,
                probe.command
            );
            assert!(
                !spec.is_write(),
                "simulate probe '{}' calls '{}', which changes state — simulate must stay read-only",
                probe.label,
                probe.command
            );
            let args: serde_json::Value = serde_json::from_str(probe.args).unwrap_or_else(|_| {
                panic!(
                    "simulate probe '{}' carries args that are not JSON: {}",
                    probe.label, probe.args
                )
            });
            assert!(
                args.is_object(),
                "simulate probe '{}' args must be a JSON object",
                probe.label
            );
        }
        for name in [
            super::SIMULATE_PING,
            super::SIMULATE_LOG_STATS,
            super::SIMULATE_LOGS,
            super::SIMULATE_WIDGET_CONFIG,
        ] {
            let spec = find(name).unwrap_or_else(|| {
                panic!("simulate step names '{name}', which the inventory does not know")
            });
            assert!(
                spec.on_socket(),
                "simulate step '{name}' is not served on the socket door"
            );
        }
    }
}
