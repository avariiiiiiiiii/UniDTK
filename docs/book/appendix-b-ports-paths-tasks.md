# Appendix B: Ports, Paths, and Tasks

This appendix lists the network ports, the files and folders, the scheduled tasks, and the environment variables that UniDTK uses. Each row names the code that defines it, so you can check a row against the source when the code changes. The rules that these facts serve are in [Install and Runtime Layout](../reference/install.md) and [IPC and the Two Doors](../reference/ipc.md).

## Ports

Every port is on the loopback address, `127.0.0.1`. Nothing listens on a network interface.

| Port | Protocol | Owner | Fixed or per run | Where it is defined |
| :--- | :--- | :--- | :--- | :--- |
| 26540 | TCP | The socket door of `unidtk.exe`, read-only | Fixed | `IPC_PORT` in `core/kernel/ipc.rs`. `udtk` uses the same address, `IPC` in `opt/cli/main.rs`. |
| OS-assigned | WebSocket | The interface door of the page, after a handshake with a per-run token | Per run. Published as `wsPort` in the ping reply. | `ws.rs` binds `LOCALHOST` with port 0, and `ipc.rs` reports `wsPort`. |
| 9222 if free, otherwise OS-assigned | CDP, the WebView2 remote debugging port | The WebView2 control. `udtk gui`, `udtk shot`, and `udtk cdp-list` use it. | Per run. Published as `cdpPort` in the ping reply. | `pick_cdp_port()` in `core/kernel/main.rs`, and `cdpPort` in `core/kernel/ipc.rs`. |
| 12908 | SOCKS5 | The local listener of sing-box. The VPN module uses it, and so does the AI DJ model download. | Fixed | `SOCKS_PORT` in `modules/vpn/rust/vpn.rs` and `modules/mp3-player/rust/ai/model_fetch.rs`. |
| 12909 | SOCKS5 | The ByeDPI instance (`ciadpi.exe`) run by the Telegram module | Fixed | `MANAGED_PROXY_PORT` in `modules/telegram/rust/telegram.rs`. |

Note: the CLI takes the CDP port from the application. It asks for `cdpPort` over the socket door, and uses the reply. The constant `CDP_FALLBACK = 9222` in `opt/cli/main.rs` is used only when the reply has no `cdpPort`, which is the case for a kernel that does not publish its port.

Note: the port of the WebSocket door changes on every run, so a script must read `wsPort` from the ping reply and must not keep a number.

## Program folder

The program folder is the folder that holds `unidtk.exe`. `unidtk_api::paths::install_dir()` returns it, and the installer places it in `C:\Program Files\UniDTK`. The application only reads from this folder. The rule is [install.program-files-read-only](../reference/install.md#r-install.program-files-read-only).

| Item | Where it is looked up | Purpose |
| :--- | :--- | :--- |
| `unidtk.exe`, `udtk.exe`, `vpn-helper.exe` | `install_dir()`, `vpn_helper_exe()` in `core/api/src/paths.rs` | The kernel, the CLI, and the elevated VPN helper |
| `sing-box.exe` | `singbox_exe()` in `core/api/src/paths.rs` | The VPN engine. There is no fallback to another copy. |
| `ciadpi.exe` | `byedpi_exe()` in `core/api/src/paths.rs`, and `resolve_binary("ciadpi.exe")` in `modules/telegram/rust/telegram.rs` | The ByeDPI proxy used by the Telegram module |
| `libmpv-2.dll` | Linked with `raw-dylib` in `modules/mp3-player/rust/ffi/libmpv.rs` | The media library. The DLL must be present when the program starts. |
| `tdjson.dll` | `TDJSON_PATH` or the configured path, in `modules/telegram/rust/telegram.rs` | The TDLib library |
| `models\discogs-maest-129e\model.onnx` | `install.join("models")` in `modules/mp3-player/rust/ai/ai_dj.rs` | The AI DJ model, when the installer places it |
| `modules\`, `scheduler-tasks\` | Installed by the installer | Module metadata and the task templates |

## Data folder

The data folder is `%LOCALAPPDATA%\UniDTK`. `unidtk_api::paths::data_dir()` returns it, and it is found through the Windows known-folder API, `dirs::data_local_dir()`, not through the environment variable. The rule is [install.data-in-localappdata](../reference/install.md#r-install.data-in-localappdata).

The subfolders are defined in `core/api/src/paths.rs`.

| Folder | Function | Contents |
| :--- | :--- | :--- |
| `cache\` | `cache_dir()` | Rebuildable data. Holds `cache\icons\` for the start menu icons, the module caches, and `cache\book\`, where `udtk doc` writes the rendered book. |
| `logs\` | `logs_dir()` | `unidtk.log`, the plain-text log, and `vpn_helper.log`, written by the elevated helper |
| `music\` | `music_dir()` | The default music library. The `library_dir` setting in the player configuration can point elsewhere. |
| `models\` | `models_dir()` | A model that was downloaded or placed by hand, as the second candidate for the AI DJ |

The files directly in the data folder are listed below. Each row names the function that creates it.

| Item | Where it is defined | Contents |
| :--- | :--- | :--- |
| `unidtk.redb` | `UniDtkDatabase::default_path()` in `core/kernel/unidtk_db.rs` | User data: tracks, playlists, history, configuration, and widget settings. Each module's `mods/<id>/` slice also lives here. |
| `unidtk_logs.redb` | `LogStore::default_path()` in `core/kernel/log_store.rs` | The log, with seven days and at most 5000 entries by default |
| `crash.log` | `crash_log_path()` in `core/kernel/panic_hook.rs` | The full report of each panic. It is written under `dirs::data_local_dir()\UniDTK`, the same folder. |
| `mods\<slug>\<name>.redb` | `StoreKind::OwnedDurable` in `core/kernel/module_store.rs` | A durable store that a module declares |
| `cache\<slug>\<name>.redb` | `StoreKind::OwnedCache` in `core/kernel/module_store.rs` | A rebuildable store. The AI DJ store is `cache\mp3_player\ai_dj.redb`. |
| `tdlib\accounts\<account>\` | `tdlib_account_dir()` in `modules/telegram/rust/telegram.rs`, under `data_dir().join("tdlib")` | The TDLib session of each account |
| `telegram_config.json` | `telegram_config_path()` in `modules/telegram/rust/telegram.rs` | The Telegram configuration |
| `vpn_settings.json`, `vpn_servers.json` | `modules/vpn/rust/vpn.rs`, lines 77 and 81 | The VPN settings and the server list |
| `singbox_config.json` | `modules/vpn/rust/vpn.rs`, line 85 | The sing-box configuration that the VPN writes and the helper reads |
| `singbox.log` | `modules/vpn/rust/vpn.rs`, line 89 | The sing-box log |
| `vpn_stop.cutoff`, `vpn_stop.done` | `modules/vpn/rust/vpn.rs`, lines 96 and 103 | Markers that coordinate the VPN stop |
| `folders\` | `core/kernel/start_menu.rs`, line 431 | The desk folder data |

## Registry

| Item | Written by | Purpose |
| :--- | :--- | :--- |
| `HKCU\Environment\Path` | `core/kernel/path_env.rs`, run by `udtk path ensure` | Puts the installed folder on the user's `PATH`, so `udtk` runs from any terminal |

## Scheduled tasks

The three templates are in `scheduler-tasks\`. The installer's hook runs `udtk tasks register all`, which reads each template, replaces `@INSTALL_DIR@` with the installation folder, and registers it. The rule is [install.native-tasks](../reference/install.md#r-install.native-tasks).

| Task name | Template | Command | Trigger | Principal and run level | Hidden |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `UniDTK` | `scheduler-tasks\UniDTKTask.xml` | `@INSTALL_DIR@\unidtk.exe` | Logon, with delay `PT0S` | Group `S-1-5-32-545` (Users), `LeastPrivilege` | `false` |
| `UniDTKSingbox` | `scheduler-tasks\UniDTKSingboxTask.xml` | `@INSTALL_DIR@\vpn-helper.exe start` | None in the template | `InteractiveToken`, `HighestAvailable` | `true` |
| `UniDTKVpnStop` | `scheduler-tasks\UniDTKVpnStopTask.xml` | `@INSTALL_DIR@\vpn-helper.exe stop` | None in the template | `InteractiveToken`, `HighestAvailable` | `true` |

The names are the ones that `opt/cli/main.rs` gives the tasks, at lines 1810 to 1825.

Note: the `start` action of the helper launches `sing-box.exe` with `singbox_config.json` from the data folder, and the `stop` action stops it. So `UniDTKSingbox` is the task that starts the TUN adapter, through the helper.

## Environment variables

The application (`unidtk.exe`) reads the following variables. A variable set in the process environment wins. The application also reads a `.env` file: `load_env_into_process()` in `core/kernel/main.rs` looks for `.env` in the program folder and up to five parent folders, and sets only the variables that are not already set.

| Variable | Read in | When absent | Purpose |
| :--- | :--- | :--- | :--- |
| `TELEGRAM_API_ID` | `core/kernel/main.rs`, `modules/telegram/rust/telegram.rs` | The value baked in at build time from the repository `.env` (`embed_dotenv()` in `toolchain/build.rs`) | The Telegram API identifier |
| `TELEGRAM_API_HASH` | Same as above | The value baked in at build time | The Telegram API hash |
| `TDJSON_PATH` | `modules/telegram/rust/telegram.rs` | The `tdjson_path` in the Telegram configuration | The location of `tdjson.dll`. It overrides the configuration. |
| `UNIDTK_TD_LOG_LEVEL` | `modules/telegram/rust/telegram.rs` | `2` | The TDLib log verbosity, from 1 to 5 |
| `AI_DJ_MODEL_PATH` | `modules/mp3-player/rust/ai/ai_dj.rs` | The model is searched for in the usual folders | The path to the ONNX model. It is used only when the file is a valid model. |
| `AI_DJ_MODEL_URL` | `modules/mp3-player/rust/ai/model_fetch.rs` | `https://huggingface.co/mtg-upf/discogs-maest-30s-pw-129e/resolve/main` | The base address of the model download |
| `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` | `core/kernel/main.rs`, `merge_browser_args()` | Empty | Extra arguments for WebView2. A `--remote-debugging-port` in it is removed, because the application sets its own port. |
| `PATH` | `core/kernel/path_env.rs` | The process `PATH` | Extended with the installed folder when the entry is registered |
| `APPDATA` | `modules/mp3-player/rust/library/config.rs` | `USERPROFILE\AppData\Roaming` | Used only to find the old `qbc-music` configuration for migration |
| `USERPROFILE`, `HOMEDRIVE`, `HOMEPATH` | `modules/mp3-player/rust/library/config.rs` | The next variable in the chain | The home folder, for the same migration |
| `ProgramFiles`, `ProgramFiles(x86)`, `LOCALAPPDATA` | `modules/mp3-player/rust/library/config.rs`, `known_binary_locations()` | No folder is searched for that root | The roots searched for an external executable, such as `mpv.exe` |
| `SystemDrive` | `modules/explorer/rust/window.rs` | `C:` | The system drive that the explorer module uses when it builds its window |

Note: `LOCALAPPDATA` is not used to find the data folder. The data folder comes from the known-folder API, as the note on `data_dir()` in `core/api/src/paths.rs` explains.

The command-line tool (`udtk.exe`) reads these variables.

| Variable | Read in | Purpose |
| :--- | :--- | :--- |
| `PROGRAMFILES`, `ProgramW6432` | `opt/cli/main.rs`, `program_files_install_dir()` | The root in which the installed folder is found |
| `USERNAME` | `opt/cli/main.rs`, `prepare_task_xml()` | The user name, read when the tasks are registered. It defaults to `admin`. |
| `PATH` | `opt/cli/main.rs`, the `path` command | Checked by `udtk path status` |
| `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE`, `TERM`, `COLUMNS`, `LINES` | `opt/cli/splash.rs` | Colour and size of the start-up splash in the terminal |

Note: the build script `toolchain/build.rs` also reads `TAURI_CONFIG` and `OUT_DIR`. Both are used while the program is compiled, so they are not run-time variables.
