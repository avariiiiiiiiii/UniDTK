# Install and Runtime Layout

This chapter states where the program lives, where its data lives, how it is built and started, and which outside binaries it carries.

## Program and data

r[install.program-files-read-only]

The installed application lives in `C:\Program Files\UniDTK` (the installer runs in `perMachine` mode): `unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, `sing-box.exe`, native DLLs, `modules/`, `scheduler-tasks/`, and the book. The directory is read-only to the application. It writes nothing next to itself.

r[install.data-in-localappdata]

Everything mutable is in `%LOCALAPPDATA%\UniDTK`: the databases, `logs/`, `cache/` (including the rendered book), `tdlib/` (the Telegram session), `models/`, and `music/`. Roaming (`%APPDATA%`) and LocalLow are not used: the configuration holds absolute paths and a browser name for cookies, so it must not travel with a roaming profile.

r[install.paths-in-one-place]

Only the contract (`unidtk_api::paths`) and the kernel know this layout. A module does not build paths itself.

r[install.no-absolute-paths]

The repository holds no hardcoded absolute paths. Task templates (`scheduler-tasks/*.xml`) contain `@INSTALL_DIR@`, which registration replaces.

## Building and installing

r[install.one-build-command]

The release is built and installed only by `udtk build [-j N]`. It stops the application, builds the three binaries and the NSIS installer in one `cargo tauri build --bundles nsis`, installs silently into Program Files, verifies the installed files by SHA-256, restarts the application, and waits for live IPC. Building with plain `cargo build` or copying binaries into Program Files is not allowed: a hand copy does not register the scheduled tasks. `udtk build` cannot run from the Program Files copy, because that directory is not writable.

Tasks and the `PATH` entry are set up by the installer (`toolchain/windows/hooks.nsh` runs `udtk tasks register all` and `udtk path ensure`), not by the first launch.

## Scheduled tasks

r[install.native-tasks]

Every task runs a native binary directly, with its arguments. `powershell.exe`, `cmd.exe`, and `.vbs` wrappers are not used, because a console-subsystem process flashes a black window. That is why the VPN stopper is its own GUI-subsystem binary, `vpn-helper.exe`, and why console helpers spawned by the CLI pass `CREATE_NO_WINDOW`. The two elevated helper tasks are also marked `<Hidden>true</Hidden>`, which hides the task itself in the Task Scheduler list; it does not affect windows.

| Task | Runs | Principal |
| :--- | :--- | :--- |
| `UniDTK` | `unidtk.exe` at logon | any user (group `S-1-5-32-545`), `LeastPrivilege` |
| `UniDTKSingbox` | `vpn-helper.exe start`, which starts sing-box with the TUN adapter | installing user, `HighestAvailable` |
| `UniDTKVpnStop` | `vpn-helper.exe stop`, which stops the VPN processes | installing user, `HighestAvailable` |

## Outside binaries

r[install.embed-or-bundle]

A component with a library API is embedded in process (libmpv, as `libmpv-2.dll`; TDLib, as `tdjson.dll`). A component without one is bundled as a separate program (sing-box, ByeDPI as `ciadpi.exe`, yt-dlp). Go and Python runtimes stay hidden inside those binaries; the user installs nothing. `udtk doctor` checks that each is present.
