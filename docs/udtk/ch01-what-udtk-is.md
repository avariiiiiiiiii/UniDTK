# Chapter 1. What `udtk` is

`udtk` is a small native program. It does not talk to the user's screen, and it does not contain the application's logic. It is a **client**. It asks the running UniDTK application questions over a local socket, and it drives the application's WebView through the browser debugging protocol (CDP). It also performs the operations that belong to the machine rather than to the app: building, installing, scheduled tasks, and the PATH.

## 1.1 One project, three binaries

The whole Rust side is one Cargo project in `toolchain/`. It declares three binaries in [toolchain/Cargo.toml](../../toolchain/Cargo.toml):

```toml
[[bin]]
name = "unidtk"
path = "../core/kernel/main.rs"

[[bin]]
name = "udtk"
path = "../opt/cli/main.rs"

[[bin]]
name = "vpn-helper"
path = "../modules/vpn/helper/main.rs"
```

| Binary | Source | Role |
|---|---|---|
| `unidtk.exe` | [core/kernel/main.rs](../../core/kernel/main.rs) | The application: kernel, WebView, IPC server, modules |
| `udtk.exe` | [opt/cli/main.rs](../../opt/cli/main.rs) | This CLI |
| `vpn-helper.exe` | [modules/vpn/helper/main.rs](../../modules/vpn/helper/main.rs) | A GUI-subsystem helper for the VPN, run as a scheduled task |

Nothing else is needed at run time. There are no script wrappers, no Node.js, and no external runtime.

## 1.2 Where the binaries live

`udtk` finds the application by looking **next to itself**. The function `install_dir()` in [core/api/src/paths.rs](../../core/api/src/paths.rs) returns the directory of the running executable. In a shipped install that is `C:\Program Files\UniDTK`, where `unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, and `sing-box.exe` sit together. In a development run it is `target\release` of the checkout.

Two commands use a different, fixed location on purpose. `udtk path` and `udtk tasks register` always point at `C:\Program Files\UniDTK`, even when the CLI runs from a checkout, so that a development build never registers itself in the PATH or in Task Scheduler. Section 12 explains this.

## 1.3 Exit codes

| Code | Meaning | Where it comes from |
|---|---|---|
| `0` | Success. For `doctor`: every check passed | `main()` prints the output and returns |
| `1` | Execution error: the IPC socket is unreachable, the remote command failed, or `doctor` found a failure | Every command except the argument errors below |
| `2` | Invalid arguments: an unknown command, or an unknown `doctor` flag | `main()` for an unknown command, `doctor_cmd` for a flag |

An unknown command prints `error: unknown command: <name>` and then the help text to stderr. An unknown `doctor` flag is checked against the whitelist `ALLOWED_DOCTOR_FLAGS` in [opt/cli/main.rs](../../opt/cli/main.rs), and anything not on it is rejected with exit 2. Nothing is silently ignored.

## 1.4 Commands that were removed

Removing a command is a change of contract, so the old names are rejected rather than redirected. None of these names is an alias any more.

| Old name | What happens now | Use instead |
|---|---|---|
| `ping`, `status` | `unknown command`, exit 2 | `udtk doctor -ping` |
| `check` | `unknown command`, exit 2 | `udtk doctor -check` |
| `commands` | `unknown command`, exit 2 | `udtk help --json` |
| `dev` | `unknown command`, exit 2 (there is no arm for it in the dispatcher) | `cd toolchain && cargo tauri dev` |
| `udtk run --debug`, `udtk run -d` | Refused with an error: CDP is always on in release builds | `udtk gui`, `udtk shot` |
| `-h`, `--help` | Not recognised; exit 2 | `udtk help` |
| `-cargo`, `-sim`, `-app`, `-ipc` | Unrecognised `doctor` flags | `-check`, `-simulate`, `-ping` |
