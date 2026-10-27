//! Declarative description of the command surface.
//!
//! The whole point of this module is that "what the application can be asked to do"
//! exists in exactly one place, as data. Three hand-written lists used to describe
//! it: the `match` in `core/kernel/ipc.rs`, the reply of the `commands` command, and
//! the `#[tauri::command]` / `generate_handler!` pair. They drifted apart, and adding
//! a module command meant editing the kernel in three places.
//!
//! Rules of the road:
//! - this module holds **types only** — no `tauri`, no `redb`, no Win32, so both the
//!   application and the CLI can compile the inventory;
//! - a command is described by data, never by silence: whether it changes state is a
//!   field, not the mere absence of an entry;
//! - handlers are deliberately *not* part of the description: a handler needs the
//!   kernel's typed state, so it belongs to the kernel. The description is what
//!   travels.

/// Whether a command only reads, or is able to change something.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    ReadOnly,
    Write,
}

/// A door a command may be served through.
///
/// A door describes **who is asking**, not which pipe carries the bytes. Two exist:
///
/// - [`Door::Socket`] — the `udtk` bridge over the local line protocol. It is
///   loopback-only and unauthenticated, so nothing that changes state is served there:
///   any local process could drive it.
/// - [`Door::Ui`] — the application's own door. The UI is what asks here, and its
///   transport is being replaced (Tauri IPC today, the token-authenticated WebSocket
///   next) without touching this policy: the question is who is asking, not what the
///   pipe is made of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Door {
    Socket,
    Ui,
}

impl Door {
    /// Stable name, used in error messages and in the CLI's manifest output.
    pub fn name(&self) -> &'static str {
        match self {
            Door::Socket => "socket",
            Door::Ui => "ui",
        }
    }
}

/// How a repeated request must be treated.
///
/// A door that can retry (or a client that reconnects) must know whether asking twice
/// equals asking once. This is data, not a convention, so the transport can refuse to
/// auto-retry the dangerous ones instead of hoping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Idempotency {
    /// Repeating is harmless: asking twice equals asking once.
    Safe,
    /// A repeat may duplicate an effect; never retry automatically, deduplicate by
    /// request id instead.
    Unsafe,
}

/// One command, described.
#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    /// Human hint for the arguments, e.g. `"[--limit N]"`; empty when there are none.
    pub args: &'static str,
    pub access: Access,
    pub doors: &'static [Door],
    pub idempotency: Idempotency,
}

impl CommandSpec {
    /// A command that only reports state. It may be served through both doors.
    pub const fn read_only(name: &'static str, summary: &'static str, args: &'static str) -> Self {
        Self {
            name,
            summary,
            args,
            access: Access::ReadOnly,
            doors: &[Door::Socket, Door::Ui],
            // Reading twice changes nothing, so a door is free to retry it.
            idempotency: Idempotency::Safe,
        }
    }

    /// A read that only the **tool door** serves: introspection of the kernel itself —
    /// liveness, the journal readers, the manifest.
    ///
    /// Declaring these as [`CommandSpec::read_only`] would claim the UI door for commands
    /// the interface never calls and the kernel registers nowhere, and the manifest is
    /// exactly where such a claim is expensive: it is what the CLI, the acceptance checks
    /// and the migration are read against.
    pub const fn read_only_cli(
        name: &'static str,
        summary: &'static str,
        args: &'static str,
    ) -> Self {
        Self {
            name,
            summary,
            args,
            access: Access::ReadOnly,
            doors: &[Door::Socket],
            // Reading twice changes nothing, so a door is free to retry it.
            idempotency: Idempotency::Safe,
        }
    }

    /// A command that changes state. It is **never** served over the socket: that
    /// door is unauthenticated, so any local process could drive it.
    ///
    /// Marked as not retry-safe by default — the conservative side for a write.
    pub const fn write(name: &'static str, summary: &'static str, args: &'static str) -> Self {
        Self {
            name,
            summary,
            args,
            access: Access::Write,
            doors: &[Door::Ui],
            idempotency: Idempotency::Unsafe,
        }
    }

    pub fn is_write(&self) -> bool {
        self.access == Access::Write
    }

    pub fn on_socket(&self) -> bool {
        self.doors.contains(&Door::Socket)
    }

    pub fn is_retry_safe(&self) -> bool {
        self.idempotency == Idempotency::Safe
    }
}

/// Opaque host handle passed to a module's command handler.
///
/// The kernel passes its `tauri::AppHandle`, but the contract stays free of Tauri
/// types: a module that needs the host downcasts it, a module that does not (like
/// `glyph-bar`) ignores the parameter entirely. This is what lets modules answer for
/// their own commands without the kernel naming their state types.
pub type HostHandle<'a> = &'a (dyn std::any::Any + Send + Sync);

/// Signature of a module's command handler, as registered by the kernel.
///
/// Returns `None` when the module does not know the command — the caller then reports
/// it as a build inconsistency rather than guessing.
pub type ModuleIpcFn = fn(
    cmd: &str,
    args: &serde_json::Value,
    host: Option<HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_commands_stay_off_the_socket() {
        let spec = CommandSpec::write("vpn_connect", "Connect to a server", "<tag>");
        assert!(spec.is_write());
        assert!(!spec.on_socket(), "write commands must not be exposed on the socket");
    }

    #[test]
    fn write_commands_are_served_on_the_ui_door() {
        // The other half of the policy: staying off the socket must not mean being
        // unreachable — the UI is exactly who may drive a state change.
        let spec = CommandSpec::write("vpn_connect", "Connect to a server", "<tag>");
        assert!(
            spec.doors.contains(&Door::Ui),
            "a write command the UI cannot call would be a command that cannot run at all"
        );
    }

    #[test]
    fn read_only_commands_are_available_on_both_doors() {
        let spec = CommandSpec::read_only("vpn_status", "VPN runtime status", "");
        assert!(!spec.is_write());
        assert!(spec.on_socket());
    }

    #[test]
    fn a_cli_only_read_does_not_claim_the_ui_door() {
        let spec = CommandSpec::read_only_cli("ping", "Liveness probe", "");
        assert!(spec.on_socket());
        assert!(
            !spec.doors.contains(&Door::Ui),
            "the tool door's own surface must not claim a UI command that does not exist"
        );
    }

    #[test]
    fn every_door_has_a_stable_name() {
        assert_eq!(Door::Socket.name(), "socket");
        assert_eq!(Door::Ui.name(), "ui");
    }
}
