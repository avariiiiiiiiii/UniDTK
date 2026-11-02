# Architecture Specification and Plan: Fixing HKCU/Folder Registry Interception and System Applets

> **Status:** Agreed with 5 independent subagents, ready for implementation  
> **Date:** 2026-09-04  
> **Target subsystems:** `backend/modules/explorer`, `backend/core`, `frontend/modules/explorer`, `frontend/core`

---

## 1. Problem Description and Symptoms

While using the shell, the user ran into unwanted system interceptions:
1. **Opening Control Panel from the Start menu:** Nothing Explorer opens instead of the Windows Control Panel window.
2. **Running `ncpa.cpl` from `Win + R`:** Nothing Explorer opens instead of the Network Connections applet.
3. **Strange tab names:** tabs appear in Explorer with names like `::{26EE0668-A00A-44D7-9371-BEB064C98683}\3\::{7007ACC7-3202-11D1-AAD2-00805FC1270E}`.
4. **Random folders:** when navigation fails, Windows Explorer falls back to opening a working or parent folder (System32, Programs), and that fallback is also intercepted.

---

## 2. Root Cause Analysis

### 2.1 The Windows Shell Namespace Hierarchy in the Registry
The registry separates the classes of shell objects:
- **`HKCR\Directory` (`HKCU\Software\Classes\Directory`):** describes **only physical directories** on disk (`C:\Users`, `D:\Projects`).
- **`HKCR\Drive` (`HKCU\Software\Classes\Drive`):** describes **only drive roots** (`C:\`, `D:\`).
- **`HKCR\Folder` (`HKCU\Software\Classes\Folder`):** the base container for **the entire Shell Namespace**. It covers both physical directories and **virtual system folders (CLSIDs)**:
  - Control Panel: `::{26EE0668-A00A-44D7-9371-BEB064C98683}`
  - Network Connections (`ncpa.cpl`): `::{7007ACC7-3202-11D1-AAD2-00805FC1270E}`
  - Printers, Devices, Fonts, MTP devices (phones), ZIP archives.

### 2.2 Where the Bug Was in the Code
In `backend/modules/explorer/src/registry.rs`:
```rust
const FOLDER_OPEN: &str = r"HKCU\Software\Classes\Folder\shell\open";
const FOLDER_OPEN_CMD: &str = r"HKCU\Software\Classes\Folder\shell\open\command";
const FOLDER_EXPLORE: &str = r"HKCU\Software\Classes\Folder\shell\explore";
const FOLDER_EXPLORE_CMD: &str = r"HKCU\Software\Classes\Folder\shell\explore\command";
```
In the function `install_as_default_file_manager()`:
```rust
set_open(FOLDER_OPEN, FOLDER_OPEN_CMD, &open_path);
set_open(FOLDER_EXPLORE, FOLDER_EXPLORE_CMD, &open_path);
```
`set_open` writes `DelegateExecute = ""` (an empty string). That disables the system COM handler of Windows Explorer (`{11dbb47c-a525-400b-9e80-a54615a090c0}`).
`backend/core/main.rs:1174-1179` calls this function on every application start, in a background thread, and overwrites the `HKCU\Software\Classes\Folder` branch.

The resulting chain of events:
1. At startup, the app writes `open` and `explore` verbs under `Folder`, with `DelegateExecute` set to an empty string.
2. When Windows opens a virtual folder (Control Panel, `ncpa.cpl`), it finds the overridden verb and runs the command instead of the system handler:
   ```cmd
   "shell.exe" open "::{26EE0668-A00A-44D7-9371-BEB064C98683}"
   ```
3. The backend accepted this argument and forwarded it to the frontend as an `explorer-open` event.
4. The frontend created a tab titled with the GUID and tried to read the virtual path with `std::fs::read_dir`. That failed with "Path does not exist".
5. The broken tab was saved to `shell.redb`, so the problem persisted across restarts.

---

## 3. Findings of 5 Independent Subagents

### Subagent 1: Win32 & Thread Safety
- **Association caching:** Explorer caches registry verbs in memory. Deleting the keys is not enough. The code must also call the Win32 API `SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None)`. Otherwise running processes keep using the old interception until reboot.
- **Key cleanup:** Use `RegDeleteTreeW` and treat `ERROR_FILE_NOT_FOUND` (code 2) as success. Remove the empty parent branch `Software\Classes\Folder\shell` with `RegDeleteKeyW`.
- **Fork bomb protection:** When system GUIDs are delegated to `explorer.exe`, prevent a loop by resetting associations first and by filtering known internal shell GUIDs.
- **COM apartments:** In `create_shortcut_native`, call `CoUninitialize()` only if `CoInitializeEx` returned a success code.

### Subagent 2: Win32 IPC & Message Loop
- **Critical hidden bug in `main.rs:883`:** In the current `single_instance` implementation, if `parse_open_arg` returns `None`, control falls through to line 883:
  ```rust
  if let Some(window) = app.get_webview_window("wallpaper") {
      let _ = window.hide();
      wallpaper::attach(...);
      let _ = window.show();
  }
  ```
  The code treats any unrecognized argument as a second launch of the wallpaper. When the app tries to ignore the argument, or when the CLI launches it, the wallpaper flickers and re-attaches under `WorkerW`.
- **Fix:** introduce a strict four-variant enum `LaunchIntent`:
  - `OpenInShell(String)`: a valid local path, the Recycle Bin, or "This PC".
  - `ForwardToNativeExplorer(String)`: a system virtual CLSID.
  - `ReattachWallpaper`: strictly an empty launch of `shell.exe` with no arguments (`argv.len() == 1`).
  - `Ignore`: other arguments and flags.

### Subagent 3: Performance & Resources
- `RegDeleteTreeW` takes under 0.5 ms on the first run and under 50 µs afterwards (when the keys are absent). Its load on the registry and CPU is negligible.
- `Command::new("explorer.exe").spawn()` starts a short-lived helper process. That process passes the DDE/COM command to the main `explorer.exe` and exits.
- Do not use the synchronous `.status()` or `.output()`, because they block the thread until the child exits (about 100 ms). `spawn()` returns as soon as the child starts.

### Subagent 4: Frontend & DOM Regressions
- **redb cleanup:** In `tabs.js:restoreTabs()`, tabs whose paths start with `::{` or `shell:` must be filtered out, and `await this._saveTabs()` must be called right away to purge the junk from the database.
- **Input guards:** `openExplorer()` in `shell-core.js` and `addTab()` in `tabs.js` must pass incoming paths through `sanitizeTabPath()`, which blocks GUIDs from getting into tabs.
- **`getFolderName` safety:** guard against `null`/`undefined`, normalize the drive letter to uppercase (`C:\`), and return readable names for system GUIDs instead of 38-character codes.
- **`navigateToSilent` resilience:** when reading a directory fails, show an empty state with an error message and fall back to "This PC". This prevents a frozen black screen.

### Subagent 5: Cold Logon & Desktop Icons
- **The main trap in the plan: the "This PC" desktop icon.** The icon is a virtual object with CLSID `{20D04FE0-3AEA-1069-A2D8-08002B30309D}`. It used to open in the Shell only because `Folder` was intercepted. If `Folder` is removed and nothing is added in its place, double-clicking the "This PC" desktop icon opens the standard Windows Explorer.
- **Fix:** register this specific CLSID in `registry.rs`:
  `HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open\command` -> `"shell.exe" open`
  and normalize its target to an empty string `""`, which the frontend shows as the drives view (`::MY_PC::`).
- **Side effect fixed:** removing `Folder` also restores native support for MTP (connected phones) and ZIP archives.

---

## 4. Step-by-Step Implementation Plan

### File 1: `backend/modules/explorer/src/registry.rs`
1. Remove the constants `FOLDER_OPEN`, `FOLDER_OPEN_CMD`, `FOLDER_EXPLORE`, `FOLDER_EXPLORE_CMD`.
2. Add constants for registering "This PC" on its own:
   ```rust
   const THIS_PC_OPEN: &str = r"HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open";
   const THIS_PC_OPEN_CMD: &str = r"HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open\command";
   ```
3. In `install_as_default_file_manager()`:
   - Remove the `set_open` calls for `FOLDER_*`.
   - Register `THIS_PC_OPEN` / `THIS_PC_OPEN_CMD` with the command `open_this_pc`.
   - Add a migration/cleanup block that removes keys left by earlier versions:
     ```rust
     let legacy_keys = [
         r"Software\Classes\Folder\shell\open",
         r"Software\Classes\Folder\shell\explore",
     ];
     for key in legacy_keys {
         let wide: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
         unsafe {
             let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()));
         }
     }
     let folder_shell: Vec<u16> = r"Software\Classes\Folder\shell".encode_utf16().chain(Some(0)).collect();
     unsafe {
         let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(folder_shell.as_ptr()));
     }
     ```
   - Add the shell notification:
     ```rust
     unsafe {
         windows::Win32::UI::Shell::SHChangeNotify(
             windows::Win32::UI::Shell::SHCNE_ASSOCCHANGED,
             windows::Win32::UI::Shell::SHCNF_IDLIST,
             None,
             None,
         );
     }
     ```
4. In `uninstall_as_default_file_manager()`:
   - Add `{20D04FE0-...}` to the list of keys to delete.
   - Call `SHChangeNotify`.

---

### File 2: `backend/modules/explorer/src/window.rs`
1. Define the enum `LaunchIntent`:
   ```rust
   #[derive(Debug, PartialEq, Eq)]
   pub enum LaunchIntent {
       OpenInShell(String),
       ForwardToNativeExplorer(String),
       ReattachWallpaper,
       Ignore,
   }
   ```
2. Implement `parse_launch_intent(args: &[String]) -> LaunchIntent`:
   - If `args` contains only the binary's own name, return `LaunchIntent::ReattachWallpaper`.
   - Recognize the `open` command and its positional arguments.
   - Check the CLSID:
     - Recycle Bin `{645FF040-...}` -> `OpenInShell("C:\\$Recycle.Bin")`.
     - This PC `{20D04FE0-...}` or Win+E `{52205FD8-...}` -> `OpenInShell(String::new())`.
     - Other `::{...}` or `shell:::{...}` -> `ForwardToNativeExplorer(arg)`.
   - Physical paths -> `OpenInShell(normalize_open_path(...))`.
   - Anything else -> `LaunchIntent::Ignore`.
3. Implement a safe launch of the system Explorer:
   ```rust
   pub fn forward_to_native_explorer(target: &str) {
       let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
       let explorer_exe = format!(r"{sysroot}\explorer.exe");
       crate::log_line(format!("explorer: delegating virtual target {target:?} to native explorer.exe"));
       let _ = std::process::Command::new(&explorer_exe)
           .arg(target)
           .current_dir(&sysroot)
           .spawn();
   }
   ```

---

### File 3: `backend/core/main.rs`
1. In the `single_instance` plugin (line 875):
   ```rust
   match explorer::window::parse_launch_intent(&argv) {
       explorer::window::LaunchIntent::OpenInShell(path) => {
           log_line(format!("single-instance: explorer open {path:?}"));
           let _ = app.emit("explorer-open", path);
       }
       explorer::window::LaunchIntent::ForwardToNativeExplorer(clsid) => {
           explorer::window::forward_to_native_explorer(&clsid);
       }
       explorer::window::LaunchIntent::ReattachWallpaper => {
           log_line("single-instance: bare launch, re-attaching wallpaper under WorkerW");
           let app_handle = app.clone();
           let _ = run_on_main_thread(&app_handle, show_and_attach_wallpaper);
       }
       explorer::window::LaunchIntent::Ignore => {
           log_line(format!("single-instance: ignored non-explorer args {argv:?}"));
       }
   }
   ```
2. In the cold-start block (line 1164):
   - Use `parse_launch_intent(&args)`. On `ForwardToNativeExplorer`, call `forward_to_native_explorer`. On `OpenInShell`, send the event to the frontend.

---

### File 4: `frontend/modules/explorer/tabs.js`
1. Add the validator `sanitizeTabPath(raw)`:
   - Check the type and strip quotes.
   - Normalize `::MY_PC::` and the CLSID `{20d04fe0-...}` to `::MY_PC::`.
   - For paths that start with `::{` or `shell:`, or that are serialization junk (`null`, `undefined`), return `null`.
   - Require a drive `^[A-Za-z]:[\\/]` or a UNC prefix `\\\\`.
2. In `restoreTabs()`:
   - Filter the saved tabs through `sanitizeTabPath`.
   - If at least one tab was dropped, call `await this._saveTabs()` right away to clean the `redb` database.
   - If the result is empty, create a `::MY_PC::` tab.
3. In `addTab()`:
   - Validate the input path: `let target = sanitizeTabPath(path) || '::MY_PC::';`.
4. In `getFolderName(path)`:
   - Add a check for `null`/`undefined`.
   - For known CLSIDs, return `'This PC'`, `'Recycle Bin'`, or `'Folder'`.

---

### File 5: `frontend/core/shell-core.js`
1. In the function `openExplorer(path)`:
   - Sanitize the incoming `path`. If an unsupported virtual CLSID is passed, do not open a broken tab for it.

---

### File 6: `frontend/modules/explorer/explorer.js`
1. In `navigateToSilent(path)`:
   - When `scanDirectory` fails, call `this.showEmpty(true, e)`. If needed, roll the active tab back to `this.showThisPC()`, so the empty screen does not hang.

---

## 5. Verification Plan and Checks

1. **Backend and CLI compilation:**
   - `shell check` (syntax and type check).
   - `shell build` (release build of the core).
2. **Registry state check via PowerShell / CMD:**
   - `reg query "HKCU\Software\Classes\Folder\shell" /s` -> the keys must be absent (error 1 / File Not Found).
   - `reg query "HKCU\Software\Classes\Directory\shell\open\command"` -> `"shell.exe" open "%1"`.
   - `reg query "HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open\command"` -> `"shell.exe" open`.
3. **Functional checks of system applets:**
   - Run `ncpa.cpl` from `Win + R` -> the native Windows Network Connections window opens.
   - Open Control Panel from the Start menu -> the native Windows Control Panel opens.
   - Double-click a regular folder on the desktop -> Nothing Explorer opens.
   - Double-click drive C: -> Nothing Explorer opens.
   - Double-click the "This PC" desktop icon -> the drives section of Nothing Explorer opens (`::MY_PC::`).
   - Press `Win + E` -> Nothing Explorer opens.
4. **Log and garbage check:**
   - `shell logs --level ERROR` -> no exceptions in the frontend or the backend.
   - `shell eval "JSON.stringify(window.nothingExplorer?.tabManager?.tabs.map(t => t.title))"` -> no tabs named `::{GUID}`.
5. **Visual audit:**
   - `shell shot` -> a screenshot of the screen through the CLI, to check the layout is correct and no widgets are broken.
