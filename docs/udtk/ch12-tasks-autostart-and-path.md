# Chapter 12. `tasks`, `autostart`, and `path`

## 12.1 Scheduled tasks

The three background tasks are defined by templates in [scheduler-tasks/](../../scheduler-tasks). Each template contains the placeholder `@INSTALL_DIR@`, which registration replaces. The table below is taken from the files and from the `SCHEDULER_TASKS` table in [opt/cli/main.rs](../../opt/cli/main.rs).

| Task | Template | Runs | Run level | Hidden |
|---|---|---|---|---|
| `UniDTK` | [UniDTKTask.xml](../../scheduler-tasks/UniDTKTask.xml) | `@INSTALL_DIR@\unidtk.exe` | `LeastPrivilege`, for the Users group (`S-1-5-32-545`) | `false` |
| `UniDTKSingbox` | [UniDTKSingboxTask.xml](../../scheduler-tasks/UniDTKSingboxTask.xml) | `@INSTALL_DIR@\vpn-helper.exe` | `HighestAvailable` | `true` |
| `UniDTKVpnStop` | [UniDTKVpnStopTask.xml](../../scheduler-tasks/UniDTKVpnStopTask.xml) | `@INSTALL_DIR@\vpn-helper.exe` | `HighestAvailable` | `true` |

The `Hidden` column differs from the rule in [AGENTS.md](../../AGENTS.md) (section 6 of the architecture rules), which says that every background task is hidden. The file `UniDTKTask.xml` says `false`. See [Chapter 18](ch18-known-gaps-and-open-decisions.md#chapter-18-known-gaps-and-open-decisions).

`udtk tasks` has these subcommands:

- `udtk tasks status` (or `list`): the state of each task.
- `udtk tasks register [all|<name>]`: writes the XML and registers the task. `all` is the default. Registration needs `vpn-helper.exe` in `C:\Program Files\UniDTK`, and it fails with `vpn-helper.exe is not next to the app` otherwise. The template is read from the install directory first, and from the checkout if that is missing. `@INSTALL_DIR@` and the user name are substituted. The temporary XML is written as **UTF-16 LE with a byte-order mark**, because `schtasks` expects it. The registration first runs `schtasks /create` directly. Only if that fails does it retry through `Start-Process schtasks.exe ... -Verb RunAs -Wait` (elevation). The code comment says that UAC is off on the author's machine, so the elevated retry succeeds without a dialog.
- `udtk tasks unregister <all|name>`: removes the task.
- `udtk tasks cleanup-old`: removes the obsolete tasks `NothingWallpaper`, `NothingWallpaperSingbox`, `NothingWallpaperVpnStop`, and `NothingWallpaperXray`, if they exist.

Failures are reported in the exit code. The installer runs `udtk tasks register all` and checks its status. A swallowed failure once looked like a successful install while the autostart task was missing. The hook is in [toolchain/windows/hooks.nsh](../../toolchain/windows/hooks.nsh). The uninstall hook runs `udtk tasks unregister all`.

## 12.2 Autostart

`udtk autostart status` reports the state of the `UniDTK` task. `udtk autostart enable` (or `on`) registers it. `udtk autostart disable` (or `off`) removes it. It is a wrapper over the same task.

## 12.3 The PATH

`udtk path status` (or `check`) reports whether `C:\Program Files\UniDTK` is in the User PATH, and whether `udtk.exe` is there. `udtk path ensure` (or `add`) adds that one directory to the User PATH. It refuses if `udtk.exe` is not in that directory. The CLI writes **no copy** of itself anywhere: two copies would drift in version. The installer runs `udtk path ensure` as part of the install.
