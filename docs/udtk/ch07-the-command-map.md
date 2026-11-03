# Chapter 7. The command map

| Command | Purpose | Needs the running app |
|---|---|---|
| `udtk` (no arguments) | Prints the splash and the command map | No |
| `udtk doctor [flags]` | Diagnostics: compiler, dependencies, kernel, modules, IPC, simulation | Partly (`-ping`, `-simulate`) |
| `udtk invoke <cmd> [json]` | Calls one socket command | Yes |
| `udtk logs [filters]` | Reads journal records through the socket | Yes |
| `udtk gui [--json] [--force] <step>...` | Runs a batch of steps in the page over one CDP session | Yes, plus CDP |
| `udtk shot [path] [--json] [-q]` | Saves a PNG of the window through CDP | Yes, plus CDP |
| `udtk cdp-list` | Lists the CDP targets | Yes, plus CDP |
| `udtk tasks [status\|register\|unregister\|cleanup-old]` | Manages the Windows scheduled tasks | No |
| `udtk autostart [status\|enable\|disable]` | Controls the logon task `UniDTK` | No |
| `udtk path [status\|ensure]` | Checks or adds the install directory to the User PATH | No |
| `udtk build [-j N]` | The one release build: stop, build, install, verify, restart | No (it stops and restarts the app) |
| `udtk stop` | Quits the app gracefully, then forces it, so that its files can be replaced | Optional |
| `udtk run [args...]` | Starts the app that sits next to the CLI | No |
| `udtk doc` | Renders the book and opens it | No |
| `udtk help [--json]` | Prints the help text, or the machine-readable manifest | No |

The help text of `udtk help` and the manifest of `udtk help --json` are generated from [opt/cli/main.rs](../../opt/cli/main.rs). In the manifest, `cli_commands` lists the 14 commands of `udtk` itself, and `app_commands` lists the 37 entries of the inventory, each with its `access`, its `doors`, and its `retry_safe`. The manifest also states `"protocol": 1`. The `cli_commands` array is written by hand in `help_cmd` (see [Chapter 18](ch18-known-gaps-and-open-decisions.md#chapter-18-known-gaps-and-open-decisions)).
