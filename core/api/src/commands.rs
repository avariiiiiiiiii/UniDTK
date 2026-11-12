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
//! - a module's command is declared once, together with its handler, in the module's
//!   [`module_commands!`](crate::module_commands) table, which generates both the
//!   module's specs and its dispatch; the kernel's own commands are described in
//!   `core/kernel/command_inventory.rs`.

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
    /// A read that only the interface asks for. It stays off the socket, so the tool
    /// door does not grow with every getter a widget needs (a file preview read through
    /// the unauthenticated socket would hand any local process the user's files).
    pub const fn read_ui(name: &'static str, summary: &'static str, args: &'static str) -> Self {
        Self {
            name,
            summary,
            args,
            access: Access::ReadOnly,
            doors: &[Door::Ui],
            idempotency: Idempotency::Safe,
        }
    }

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

/// Reads one argument of a command by its Rust parameter name.
///
/// The interface sends the name in camelCase, because Tauri asked it for that while the
/// commands were Tauri functions; a tool may send it in snake_case. A missing argument
/// reads as `null`, so an `Option` parameter becomes `None` and a required one fails
/// with its own name in the message.
pub fn arg<T: serde::de::DeserializeOwned>(args: &serde_json::Value, name: &str) -> Result<T, String> {
    let camel = camel_case(name);
    let value = args
        .get(&camel)
        .or_else(|| args.get(name))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|e| format!("argument '{camel}': {e}"))
}

/// `chat_id` → `chatId`: the name the interface uses for a parameter.
pub fn camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Turns a command's result into the reply value.
pub fn reply<T: serde::Serialize>(result: Result<T, String>) -> Result<serde_json::Value, String> {
    result.and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
}

/// Declares a module's commands, each in one place.
///
/// Every entry gives the access (`read_only`, `read_ui` or `write`: the [`CommandSpec`]
/// constructor of that name), the command's name and summary, then the module and the
/// arguments as closure parameters, and a body that evaluates to `Result<T, String>`
/// for a serializable `T`. The macro generates the module's `COMMANDS` and its
/// `dispatch_command`, so a spec and its handler cannot drift apart. The arguments are
/// read with [`arg`].
///
/// ```ignore
/// unidtk_api::module_commands! {
///     impl VpnModule {
///         read_only "vpn_status" "VPN runtime status" |this| Ok(status(&this.runtime));
///         write "vpn_connect" "Connect to a server" |this, tag: Option<String>| connect(&this.runtime, tag);
///     }
/// }
/// ```
#[macro_export]
macro_rules! module_commands {
    (
        impl $module:ty {
            $(
                $access:ident $name:literal $summary:literal
                |$this:ident $(, $arg:ident : $argty:ty)* $(,)?| $body:expr;
            )*
        }
    ) => {
        impl $module {
            /// This module's commands, as the kernel's inventory lists them.
            pub const COMMANDS: &'static [$crate::commands::CommandSpec] = &[
                $(
                    $crate::commands::CommandSpec::$access(
                        $name,
                        $summary,
                        concat!($(stringify!($arg), " ",)*),
                    ),
                )*
            ];

            /// Runs one of [`Self::COMMANDS`]; `None` for a name this module does not own.
            pub fn dispatch_command(
                &self,
                cmd: &str,
                args: &$crate::__serde_json::Value,
            ) -> Option<Result<$crate::__serde_json::Value, String>> {
                match cmd {
                    $(
                        $name => Some((|| -> Result<$crate::__serde_json::Value, String> {
                            #[allow(unused_variables)]
                            let $this = self;
                            $( let $arg: $argty = $crate::commands::arg(args, stringify!($arg))?; )*
                            $crate::commands::reply($body)
                        })()),
                    )*
                    _ => None,
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_is_read_in_camel_case_then_snake_case() {
        let args = serde_json::json!({ "chatId": "7", "thread_id": 3 });
        assert_eq!(arg::<String>(&args, "chat_id").unwrap(), "7");
        assert_eq!(arg::<i64>(&args, "thread_id").unwrap(), 3);
        assert_eq!(camel_case("custom_emoji_id"), "customEmojiId");
        assert_eq!(camel_case("path"), "path");
    }

    #[test]
    fn a_missing_argument_is_none_or_names_itself() {
        let args = serde_json::json!({});
        assert_eq!(arg::<Option<String>>(&args, "tag").unwrap(), None);
        let error = arg::<String>(&args, "chat_id").unwrap_err();
        assert!(error.contains("chatId"), "the error names the argument: {error}");
    }

    struct Probe(u32);

    crate::module_commands! {
        impl Probe {
            read_only "probe_value" "The value" |this| Ok(this.0);
            write "probe_add" "Adds to the value" |this, amount: u32, label: Option<String>| Ok(format!("{}{}", label.unwrap_or_default(), this.0 + amount));
        }
    }

    #[test]
    fn the_table_declares_and_dispatches_the_same_commands() {
        let names: Vec<_> = Probe::COMMANDS.iter().map(|spec| spec.name).collect();
        assert_eq!(names, ["probe_value", "probe_add"]);
        assert!(Probe::COMMANDS[0].on_socket() && Probe::COMMANDS[1].is_write());
        assert_eq!(Probe::COMMANDS[1].args, "amount label ");
        let probe = Probe(2);
        assert_eq!(probe.dispatch_command("probe_value", &serde_json::json!({})), Some(Ok(serde_json::json!(2))));
        assert_eq!(
            probe.dispatch_command("probe_add", &serde_json::json!({ "amount": 3, "label": "n=" })),
            Some(Ok(serde_json::json!("n=5")))
        );
        assert!(probe.dispatch_command("probe_add", &serde_json::json!({})).unwrap().is_err());
        assert_eq!(probe.dispatch_command("other", &serde_json::json!({})), None);
    }

    #[test]
    fn an_interface_read_stays_off_the_socket() {
        let spec = CommandSpec::read_ui("get_file_preview", "Preview a file", "");
        assert!(!spec.is_write() && spec.is_retry_safe());
        assert_eq!(spec.doors, &[Door::Ui]);
    }

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
