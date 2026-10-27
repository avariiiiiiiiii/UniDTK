//! The command inventory: one place that knows what the application can be asked.
//!
//! This file is compiled into **both** the application and the `udtk` bridge (the CLI
//! includes it with `#[path]`, the same way it already includes `registry.rs`), so the
//! offline manifest and the served surface cannot disagree about names.
//!
//! It is deliberately free of kernel-only types (`tauri`, `redb`, Win32) — that is
//! what makes the shared compilation possible.
//!
//! Scope: this inventory describes the **socket door** (what `udtk invoke` may ask
//! for). The UI's own door is `generate_handler!` in the kernel and is expected to be
//! folded into this table once the frontend moves off the Tauri transport.

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
pub const SET_DISABLED_MODULES: CommandSpec =
    CommandSpec::write("set_disabled_modules", "Set the disabled module list", "{ids}");
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
    CommandSpec::write("frontend_log", "Append a frontend record to the journal", "{message}");

// ── Modules (read-only surface) ────────────────────────────────────────────────

pub const PLAYER_STATUS: CommandSpec =
    CommandSpec::read_only("player_status", "Music player state", "");
pub const AI_DJ_STATUS: CommandSpec =
    CommandSpec::read_only("ai_dj_status", "AI DJ state", "");
pub const LIST_TRACKS: CommandSpec = CommandSpec::read_only(
    "list_tracks",
    "Library tracks",
    "{search, sort_key, sort_dir}",
);
pub const LIST_PLAYLISTS: CommandSpec =
    CommandSpec::read_only("list_playlists", "Library playlists", "");
pub const GET_SUBMODULES: CommandSpec =
    CommandSpec::read_only("get_submodules", "Music submodule state", "");
pub const GET_SYSTEM_VOLUME: CommandSpec =
    CommandSpec::read_only("get_system_volume", "Windows master volume", "");

/// The volume writes.
///
/// Declared as writes even though the effect is instantly visible and easily undone:
/// `Access` is about **who may ask**, and anything that changes the machine stays off the
/// unauthenticated socket by rule rather than by accident. They are also what makes the
/// UI door's write path testable on real state instead of on a log record.
pub const SET_SYSTEM_VOLUME: CommandSpec =
    CommandSpec::write("set_system_volume", "Set the master volume", "{level}");
pub const SET_SYSTEM_MUTE: CommandSpec =
    CommandSpec::write("set_system_mute", "Mute or unmute the master volume", "{muted}");

/// The mp3-player module's UI-door write: downloads the missing AI DJ model
/// (~331 MB) into the data directory, explicitly commanded from the widget.
///
/// Declared `write` for the same policy as the other module actions — it changes
/// the machine (writes megabytes to disk). The work is single-flight and idempotent:
/// a call against a running or already-complete download answers `running`/`already_present`
/// instead of starting a second chain.
pub const AI_DJ_MODEL_DOWNLOAD: CommandSpec = CommandSpec::write(
    "ai_dj_model_download",
    "Download the AI DJ model into the data directory",
    "",
);
pub const VPN_STATUS: CommandSpec =
    CommandSpec::read_only("vpn_status", "VPN runtime status", "");
pub const VPN_SETTINGS: CommandSpec =
    CommandSpec::read_only("vpn_settings", "VPN settings", "");

// ── VPN writes (the interface's own actions) ───────────────────────────────────

pub const VPN_CONNECT: CommandSpec =
    CommandSpec::write("vpn_connect", "Connect to a server (or the selected one)", "{tag?}");
pub const VPN_DISCONNECT: CommandSpec =
    CommandSpec::write("vpn_disconnect", "Drop the current connection", "");
/// Not a read, though it answers with measurements: it takes a global in-progress guard
/// and stores the results, so two callers cannot run it at once and the state it leaves
/// behind is observable. `Access` describes what a command *does*, not how it answers.
pub const VPN_PING_ALL: CommandSpec =
    CommandSpec::write("vpn_ping_all", "Measure latency of every server", "");
pub const VPN_SET_AUTO_CONNECT: CommandSpec =
    CommandSpec::write("vpn_set_auto_connect", "Toggle connect-on-startup", "{enabled}");
pub const VPN_LIST_SERVERS: CommandSpec =
    CommandSpec::read_only("vpn_list_servers", "VPN servers", "");
pub const TELEGRAM_STATUS: CommandSpec =
    CommandSpec::read_only("telegram_status", "Telegram runtime status", "");
pub const TELEGRAM_CHATS: CommandSpec =
    CommandSpec::read_only("telegram_chats", "Telegram chats", "");
pub const TELEGRAM_CHATS_ARCHIVE: CommandSpec =
    CommandSpec::read_only("telegram_chats_archive", "Telegram archived chats", "");
pub const TELEGRAM_SET_ARCHIVED: CommandSpec = CommandSpec::write(
    "telegram_set_archived",
    "Move a Telegram chat to or from Archive",
    "{request: {chatId, archived}}",
);
pub const TELEGRAM_MESSAGES_AROUND: CommandSpec = CommandSpec::read_only(
    "telegram_messages_around",
    "Telegram messages centered around a target message ID",
    "{chat_id, message_id, thread_id?, limit?}",
);
/// Named as the UI and the Tauri command name it, so one name works on both doors.
pub const EXPLORER_DRIVES: CommandSpec =
    CommandSpec::read_only("get_drives", "Mounted drives", "");
/// See `EXPLORER_DRIVES`: the interface asks for `get_user_places`.
pub const EXPLORER_PLACES: CommandSpec =
    CommandSpec::read_only("get_user_places", "User shell folders", "");
pub const EXPLORER_SORT_GET: CommandSpec =
    CommandSpec::read_only("explorer_sort_get", "Get explorer sort column and direction", "");
pub const EXPLORER_SORT_SET: CommandSpec = CommandSpec::write(
    "explorer_sort_set",
    "Set explorer sort column and direction",
    "{column, ascending}",
);

/// The ambient observation behind the micrographics panel.
///
/// `read_only`, not `read_only_cli`: the interface is what asks for this, every
/// time it repaints. `read_only_cli` would mean "introspection of the kernel that
/// the UI never calls" — and the refusal would be silent, because the gate test
/// that compares the manifest with `generate_handler!` only inspects commands
/// that claim the UI door.
///
/// The answer names the place it observed, which on the unauthenticated socket
/// means any local process can ask where the user is. That is accepted for a
/// current-conditions reading, and it is why the panel treats this as ambient
/// context rather than as a location the user picked.
pub const WEATHER_CURRENT: CommandSpec =
    CommandSpec::read_only("weather_current", "Ambient weather observation", "");
pub const WEATHER_LOCATION_REQUEST: CommandSpec =
    CommandSpec::write("weather_location_request", "Request Windows location permission for weather", "");

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
        command: VPN_STATUS.name,
        args: "{}",
    },
    SimulateProbe {
        label: "Module Telegram",
        command: TELEGRAM_STATUS.name,
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
    &SET_DISABLED_MODULES,
    &COMMANDS,
    &PLAYER_STATUS,
    &AI_DJ_STATUS,
    &LIST_TRACKS,
    &LIST_PLAYLISTS,
    &GET_SUBMODULES,
    &GET_SYSTEM_VOLUME,
    &SET_SYSTEM_VOLUME,
    &SET_SYSTEM_MUTE,
    &AI_DJ_MODEL_DOWNLOAD,
    &VPN_STATUS,
    &VPN_SETTINGS,
    &VPN_LIST_SERVERS,
    &VPN_CONNECT,
    &VPN_DISCONNECT,
    &VPN_PING_ALL,
    &VPN_SET_AUTO_CONNECT,
    &TELEGRAM_STATUS,
    &TELEGRAM_CHATS,
    &TELEGRAM_CHATS_ARCHIVE,
    &TELEGRAM_SET_ARCHIVED,
    &TELEGRAM_MESSAGES_AROUND,
    &EXPLORER_DRIVES,
    &EXPLORER_PLACES,
    &EXPLORER_SORT_GET,
    &EXPLORER_SORT_SET,
    &WEATHER_CURRENT,
    &WEATHER_LOCATION_REQUEST,
];

/// The subset actually served over the socket (the CLI's door).
// The bridge binary only needs `ALL`: it describes the surface, it does not serve it.
#[allow(dead_code)]
pub fn socket_commands() -> Vec<&'static CommandSpec> {
    ALL.iter().copied().filter(|spec| spec.on_socket()).collect()
}

/// Look a command up by name.
// Same: used by the kernel's router, not by the CLI.
#[allow(dead_code)]
pub fn find(name: &str) -> Option<&'static CommandSpec> {
    ALL.iter().copied().find(|spec| spec.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_described() {
        for (index, spec) in ALL.iter().enumerate() {
            assert!(!spec.name.is_empty(), "command #{index} has no name");
            assert!(
                !spec.summary.is_empty(),
                "command '{}' has no summary — a command without a description is undocumented by construction",
                spec.name
            );
            let duplicates = ALL
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
            let spec = find(probe.command).unwrap_or_else(|| {
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
