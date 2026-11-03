# Chapter 16. Install layout and data

The rule is that the program and its data are kept apart, so that an update never overwrites user state. The list of the program's files below is the one stated in [architecture.md](../reference/README.md) (section 4.8). The path logic that this book cites is in code. The installer's file list was not read for this rewrite.

| What | Where | Why |
|---|---|---|
| Program: `unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, `sing-box.exe`, DLLs, `models/`, `modules/`, `scheduler-tasks/` | `C:\Program Files\UniDTK` | Read-only for the user. The installer writes it |
| Data: `logs/`, `cache/`, `music/` | `%LOCALAPPDATA%\UniDTK` (`logs_dir()`, `cache_dir()`, `music_dir()` in [core/api/src/paths.rs](../../core/api/src/paths.rs)) | The application writes here, never next to itself |
| Databases: `unidtk.redb`, `unidtk_logs.redb` | `%LOCALAPPDATA%\UniDTK` | Described in [architecture.md](../reference/README.md) |

The `UniDTK` task runs `unidtk.exe` from the install directory. The task templates contain the placeholder `@INSTALL_DIR@`, so that no absolute path is stored in the repository. The installer, not the first run of the app, registers the tasks and the PATH.

The application does not write into its own directory. The code in `paths.rs` resolves the paths, and the modules declare a store rather than building paths themselves. The layout is therefore changed in one place.
