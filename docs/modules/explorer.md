# The Explorer Module: From an Open Request to a Directory Listing

This chapter explains the `explorer` module step by step. It shows how the file manager window opens, how a folder path becomes a list of entries, how tabs, sorting, search, and preview are wired between the WebView and Rust, how file operations reach Win32 and the Shell, and how each step fails. After reading it you can follow any Explorer action from a click in the page to the call that performs it, and you know which Rust commands the page never calls.

The module follows the vertical-slice layout. The Rust half in `modules/explorer/rust/` does the Windows work; the UI half in `modules/explorer/ui/` owns the window state, the tabs, and the rendering. The two halves meet only through named commands ([modules.vertical-slice](../reference/modules.md#r-modules.vertical-slice)).

Every mechanism here is taken from the code. Line numbers refer to the files as they are in this repository.

## Files involved

| File | Role |
| :--- | :--- |
| [fs.rs](../../modules/explorer/rust/fs.rs) | Directory listing and the `FileItem` record. |
| [commands.rs](../../modules/explorer/rust/commands.rs) | The module, its command table, and the plain functions behind it: listing, drives, places, clipboard, operations, search index. |
| [drives.rs](../../modules/explorer/rust/drives.rs), [files.rs](../../modules/explorer/rust/files.rs) | Drive enumeration; file info, shell open, create, rename, copy, delete. |
| [search.rs](../../modules/explorer/rust/search.rs), [clipboard.rs](../../modules/explorer/rust/clipboard.rs) | The search index; the Win32 clipboard formats. |
| [hashing.rs](../../modules/explorer/rust/hashing.rs), [archive.rs](../../modules/explorer/rust/archive.rs), [preview.rs](../../modules/explorer/rust/preview.rs) | Hashes, ZIP archives, preview metadata. |
| [registry.rs](../../modules/explorer/rust/registry.rs), [window.rs](../../modules/explorer/rust/window.rs) | The per-user shell overlay; parsing of the command line that asks for a folder. |
| [explorer.js](../../modules/explorer/ui/explorer.js) | The controller `NothingExplorer`: navigation, rendering, selection, operations (2,621 lines). |
| [tabs.js](../../modules/explorer/ui/tabs.js), [preview.js](../../modules/explorer/ui/preview.js), [context-menu.js](../../modules/explorer/ui/context-menu.js) | Tab manager, preview panel, context menu. |
| [main.rs](../../core/kernel/main.rs), [unidtk-core.js](../../core/ui/unidtk-core.js) | The Tauri wrappers and `open_explorer` in the kernel; `openExplorer()` in the shell page. |

---

## Step 1: How the explorer opens

The explorer is an overlay on the desk, not a window of its own. The shell loads `explorer.js` lazily with a dynamic import, and reports a failure to the frontend log ([unidtk-core.js](../../core/ui/unidtk-core.js) line 2280).

Four entry points all end in `openExplorer(path)`:

1. **The `explorer-open` event.** The kernel's `open_explorer` command emits it through `unidtk_api::emit`. The page listens at [unidtk-core.js](../../core/ui/unidtk-core.js) line 2439.
2. **The command line and the shell verbs.** The single-instance handler and the startup code both pass argv to `parse_open_arg`, then emit the same event.
3. **A shortcut on the desk.** [folder-body.js](../../core/ui/folder-body.js) calls `openExplorer(s.path, btn)`, or `openExplorer(null, btn)` for the shortcut named "explorer".
4. **The `shell-open-explorer` and `unidtk-open-explorer` window events**, which carry an optional `detail.path`.

`parse_open_arg` in [window.rs](../../modules/explorer/rust/window.rs) understands `open <path>`, a bare drive or UNC path, and the explorer-style `/select,<file>` and `/e,<path>`. An empty result means `This PC`. The registry verbs of Step 11 pass `open "%1"`, so a folder double-clicked in Windows reaches this parser.

Filename: modules/explorer/rust/window.rs

```rust
pub fn parse_open_arg(args: &[String]) -> Option<String> {
    // ... skips "--" flags, then:
    if rest[0].eq_ignore_ascii_case("open") {
        if rest.len() >= 2 { return Some(normalize_open_path(rest[1])); }
        return Some(String::new());   // empty string means "This PC"
    }
    if looks_like_path(rest[0]) { return Some(normalize_open_path(rest[0])); }
    None
}
```

`openExplorer` shows the overlay through the surface manager, with the clicked element as the origin. If the controller is not ready yet, it waits once for `explorer-ready`. Then it adds a tab for the path, or switches to an existing tab that already has it. The controller itself is created when `explorer.js` loads: its constructor builds the tab manager, the preview panel, and the context menu, and then `init()` binds the events, restores the sort order and the tabs, and fills the sidebar.

---

## Step 2: How a directory listing is produced

Every navigation runs the same chain:

1. `navigateTo(path)` (explorer.js line 283) normalizes drive input, so `c` and `c:` both become `C:\`. It marks the navigation in the tab, so an older, slower answer is discarded later.
2. `scanDirectory(path)` (line 112) calls `invoke('scan_directory', { path })`.
3. The kernel wrapper in [main.rs](../../core/kernel/main.rs) (line 994) is an `async fn`. It calls `explorer::commands::scan_directory`, which calls `fs::scan_directory_async`.
4. `scan_directory_async` runs the blocking work on Tokio's blocking pool, so file I/O never runs on the window thread.

Filename: modules/explorer/rust/fs.rs

```rust
pub async fn scan_directory_async(path: &str) -> Result<Vec<FileItem>, String> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || scan_directory(&path)).await...
}

pub fn scan_directory(path: &str) -> Result<Vec<FileItem>, String> {
    // ... MY_PC and portable paths return an empty list
    // ... "Path does not exist", "Not a directory" errors
    let entries: Vec<FileItem> = std::fs::read_dir(&dir_path)
        .map_err(|e| format!("Failed to read dir {}: {e}", dir_path.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| metadata_to_item(&e, DateStyle::default(), true))
        .filter(|item| !item.name.eq_ignore_ascii_case("desktop.ini"))
        .collect();
    // ... logs the count, returns Ok(entries)
}
```

The listing uses `std::fs::read_dir`, and each entry's attributes come from `entry.metadata()`. The one Windows-specific step is `MetadataExt::file_attributes()`: `metadata_to_item` (fs.rs line 42) reads the hidden bit (0x2) and the system bit (0x4). No direct Win32 call is made on this path.

For each entry, `metadata_to_item`:

- drops `desktop.ini` and keeps everything else, hidden and system files included;
- stores a size for files only; folders get `None`;
- converts the modified and created times to 100-nanosecond ticks since 1601 and formats them with `filetime_to_string`, which uses local time and the format `%m/%d/%Y %H:%M`;
- keeps the raw seconds as `modified_time_raw` and `created_time_raw` for sorting;
- drops the entry without a message if `entry.metadata()` fails.

The record is `FileItem`: `name`, `path`, `is_dir`, `is_hidden`, `file_size`, the two display times, and the two raw times.

Note: `get_drives` in `commands.rs` shows why blocking Windows calls are kept off the window thread. Volume and capacity queries can take a long time on a stale network drive, so the wrapper runs them on Tauri's blocking pool. The same rule is [kernel.no-blocking-main-thread](../reference/kernel.md#r-kernel.no-blocking-main-thread).

---

## Step 3: Sorting

Sorting is done in the page. `sortItems` (explorer.js line 174) runs on every listing and every header click:

- folders always come before files;
- names compare with `Intl.Collator` set to `numeric: true`, so `file2` sorts before `file10`;
- `size`, `modified` (the raw seconds), and `type` (the extension) compare as their names say; ties fall back to the name.

`setSorting` (line 1798) sets the direction. Clicking the active column flips it. A new column starts ascending only for `name` and `type`. The pair is saved with `explorer_sort_set` after a 300 ms debounce (lines 1817-1825) and read back once at startup with `explorer_sort_get` (line 51). The kernel keeps it in `unidtk.redb` ([storage.md](../reference/storage.md)).

Rust has a `sort_files` command with a different comparator (lowercase strings). The page never calls it.

---

## Step 4: Rendering a listing

`navigateTo` stores the sorted items in the tab and calls `renderFiles` (line 1061). That function drops `desktop.ini` again, shows an empty-folder label for an empty list, and draws either the grid or the list.

Large folders are drawn in pieces so that one folder cannot stall the page:

```js
const APPEAR_LIMIT = 24;   // later items appear without the stagger animation
const RENDER_FIRST = 48;   // drawn at once
const RENDER_CHUNK = 48;   // then 48 per animation frame; a newer render cancels the rest
```

Image files use the image itself as their icon, through `convertFileSrc(item.path)` with `loading="lazy"`, so each visible image in a folder is decoded as a thumbnail. Other files get an icon chosen by extension. Double-click calls `openItem`, a click calls `selectFile`, and the right button shows the context menu.

---

## Step 5: Tabs and history

`TabManager` ([tabs.js](../../modules/explorer/ui/tabs.js)) keeps the tabs. A tab holds `id`, `path`, `title`, `history`, `historyIndex`, `items`, and `scrollTop`. The path `::MY_PC::` is the `This PC` tab.

- **Restore.** `restoreTabs()` calls `explorer_tabs_get`, which reads the saved list from the module's own slice of the database (`mods/explorer/tabs`). If nothing is saved, one `This PC` tab is opened.
- **Save.** `_saveTabs()` runs after every add, switch, close, and navigation. It sends `[path, title]` pairs with `explorer_tabs_set`. It does nothing before the restore has finished, so an early `addTab` cannot overwrite the saved list.
- **Close.** Closing the last tab opens `This PC`. Closing the active tab activates the tab that takes its index.
- **Back, forward, up.** `goBack` and `goForward` move `historyIndex`. `goUp` removes the last segment, and at a drive root it goes to `This PC`. Keys: Alt+Left, Alt+Right, Backspace.

When a tab already holds items for the path, `navigateToSilent` (line 2244) draws the cached list at once and starts a background scan. The scan replaces the list only if it is still the current navigation. `_isNavigationCurrent` (lines 279-281) checks that the tab is active and that no newer navigation has started in it. A slow answer for a folder the user has left is therefore dropped.

---

## Step 6: Search

Search has two halves. The Rust side keeps an in-memory index of one folder tree; the page sends queries and draws the results.

**Building the index.** On the first query for a folder, the page calls `build_search_index` with the tab's path (line 2352). The index covers only that folder. `index_tree` in [search.rs](../../modules/explorer/rust/search.rs) walks it breadth-first, to 4 levels below the root, stops at 20,000 entries, and does not follow junctions (`is_reparse_point`). The command holds the index mutex for the whole crawl, so a query arriving during a rebuild waits for it.

**Scoring.** `search` (line 172) compares lowercase strings. The first rule that matches gives the score:

| Score | Match |
| :--- | :--- |
| 1.0 | the name equals the query |
| 0.9 | the name starts with the query |
| 0.7 | the name contains the query |
| 0.5 | the full path contains the query |
| 0.3 | the query letters appear in the name in order |

The hits are sorted by score and cut to 100. `search_checked` then removes any hit whose path no longer exists.

**The page.** The input waits 300 ms after the last keystroke (lines 1999-2003). A query under two characters closes the results and releases the index. A sequence token lets only the newest query draw. The first 30 results are drawn, with name and path passed through `escapeHtml`. A click opens the hit's parent folder and closes the search.

`closeSearch()` (line 2364) is the only place the index is cleared (`clear_search_index`). Module shutdown does nothing, so the index lives in memory until then.

---

## Step 7: Selection, preview, and hashing

`selectFile` (line 1305) handles plain, Ctrl, and Shift clicks. Shift selects a range in the sorted order from the last clicked item. Ctrl+A selects all.

`PreviewPanel.showFile` ([preview.js](../../modules/explorer/ui/preview.js) line 12) follows the selection. For one item it shows the image (for image extensions) or a type icon, and fills size, type, and both times from the listing record. The file itself is not read. For several items it shows a synthetic entry with a count and no hash buttons.

The hash buttons call `hash_file_md5` or `hash_file_sha256` ([hashing.rs](../../modules/explorer/rust/hashing.rs)). Each opens the file and reads it in 8 KiB blocks into the digest. There is no size limit, no progress report, and no cancel. While it runs, the result line shows a counting label. A failure shows one fixed error label, and the cause is not shown. Both commands are `async` in the kernel, so they run off the window thread.

---

## Step 8: Opening files, folders, and terminals

`openItem` (line 1266) decides what a double-click means:

1. A folder, or an entry named as the Recycle Bin, is navigated to.
2. Otherwise `path_is_directory` is asked. A directory is navigated to.
3. Otherwise `openFile` asks `path_is_directory` once more, then calls `open_path_default`.

`open_path_default` ([files.rs](../../modules/explorer/rust/files.rs) line 105) calls `ShellExecuteW` with the `open` verb and `SW_SHOWNORMAL`. A return value of 32 or less is an error, reported as `ShellExecute failed (n)`. `openFile` only writes that error to the console (explorer.js lines 1572-1576).

`open_terminal` ([commands.rs](../../modules/explorer/rust/commands.rs) line 218) opens the folder of the item, or the parent of a file, or the home folder for `This PC`. It tries Windows Terminal (`wt -d`), then PowerShell with `-NoExit`, then `cmd /k`. Each attempt counts as a success if `spawn()` returned, not if the terminal works.

Copy path calls the kernel command `set_clipboard_text`, which puts the text on the clipboard. If that fails, the page uses `navigator.clipboard`. Several selected paths are joined with CRLF. Send to Telegram dispatches a `telegram-send-file` window event with the path, which the shell listens for at [unidtk-core.js](../../core/ui/unidtk-core.js) line 2453.

---

## Step 9: Clipboard and file operations

There are two clipboards. The page keeps `this.clipboard` (explorer.js line 33), which records whether the last action was a cut. The Windows clipboard receives the files only so that other programs can paste them.

**Copy and cut.** `copySelected` and `cutSelected` (lines 2520-2532) set the page clipboard and call `copy_to_clipboard`, which publishes a `CF_HDROP` block: a `DROPFILES` header and NUL-terminated wide paths, with an extra NUL at the end ([clipboard.rs](../../modules/explorer/rust/clipboard.rs) lines 54-77). Opening the Windows clipboard is retried ten times, 20 ms apart, because another process may hold it.

**Paste.** `pasteFiles` (line 2534) uses the page clipboard, and reads the Windows clipboard only when that is empty. A cut calls `rename_path`; a copy calls `copy_path`. A copy onto its own source gets a `- Copy` name, or a numbered one (`_getUniqueCopyPath`, line 2499). Failures are collected and shown in the status line.

**Delete.** `deleteSelected` (line 2406) calls `delete_to_recycle_bin` once per path. The Rust side runs `SHFileOperationW` with `FOF_ALLOWUNDO`, `FOF_NOCONFIRMATION`, and `FOF_SILENT`, so items go to the Recycle Bin without a prompt ([files.rs](../../modules/explorer/rust/files.rs) lines 310-341).

**Rename.** `renameSelected` (line 2422) places an input over the name, with the part before the last dot selected. Enter or blur commits, Escape cancels. `rename_path` refuses to replace an existing path, except a case-only change of the same file, which is recognized by volume serial and file index (files.rs lines 172-233).

**Copy.** `copy_path` (files.rs line 235) refuses a top-level junction or symbolic link and refuses a folder copied into itself. Inside a folder being copied, links are skipped without a message (lines 289-291).

**Create.** `createFolder` and `createFile` (lines 1903 and 1926) try names in turn: `New Folder`, `New Folder (2)`, and so on, and `New File.txt`, `New File (2).txt`, and so on.

Warning: `createFolder` assumes `create_directory` fails when the name is taken. It does not. `create_directory` calls `std::fs::create_dir_all` (files.rs lines 133-135), which succeeds for an existing directory. The first attempt therefore always succeeds, and the page refreshes the folder without creating anything. `createFile` is not affected: `create_empty_file` returns `already exists` (lines 137-141).

---

## Step 10: Archives

The archive code is complete in Rust and has no caller in the page.

- `open_archive` reads the ZIP central directory and returns each entry with its sizes, plus totals.
- `extract_archive` accepts an entry only through `enclosed_name()`, which rejects absolute paths and `..`, and checks that the joined path still starts with the destination. Bytes written are capped at 4 GiB, counted from what was actually produced. An entry that exceeds the cap is removed with its partial file; entries extracted before it stay on disk (archive.rs lines 76-124).
- `create_archive` writes a Deflated ZIP. A folder becomes one empty directory entry; its contents are not walked (lines 172-173).
- `is_zip_file` checks the signature `PK\x03\x04`.

---

## Step 11: The registry overlay

At startup the kernel starts a background thread that calls `install_as_default_file_manager` (main.rs lines 2530-2535). It writes per-user keys under `HKCU\Software\Classes` only, which needs no administrator rights and does not touch `HKCR`. The keys cover `Directory`, `Drive`, `Folder`, the `explore` verbs, the directory background, and the Win+E class. Each verb sets an empty `DelegateExecute`, which skips Explorer's own handler so that the command line is used:

Filename: modules/explorer/rust/registry.rs

```rust
let open_path = format!("\"{exe}\" open \"%1\"");
set_open(DIR_OPEN, DIR_OPEN_CMD, &open_path);   // writes DelegateExecute="" and the command
```

A Start menu shortcut is installed too. `uninstall_as_default_file_manager` removes the same keys, but nothing calls it.

---

## Step 12: The socket door, the contract, and the lifecycle

**The commands.** The module declares its 44 commands in one table in [commands.rs](../../modules/explorer/rust/commands.rs) (`unidtk_api::module_commands!`), with the names the page calls. Two are `read_only` and also answered on the socket: `get_drives` and `get_user_places`. The other reads are `read_ui`, so a file preview or a hash never leaves through the unauthenticated socket, and the writes are reached only through the page. The search commands read the index the module holds.

**The contract.** The crate depends on `unidtk-api`, and on Tauri only for `tauri::async_runtime::block_on`, which drives the async bodies (`scan_directory`, `get_drives`). It does not depend on the kernel. The kernel serves every command of the table on a blocking thread, never on the window thread ([modules.command-table](../reference/modules.md#r-modules.command-table)).

**Drives.** `get_all_drives` ([drives.rs](../../modules/explorer/rust/drives.rs)) reads, for each present letter, the type with `GetDriveTypeW`, the label with `GetVolumeInformationW`, and the size with `GetDiskFreeSpaceExW`. A network drive is resolved to its UNC path through `WNetGetConnectionW`, loaded from `mpr.dll` at run time. The page waits 1,500 ms for this list, then falls back to `get_raw_drives`, which returns letters only (explorer.js lines 95-145).

**Lifecycle.** `init` logs when the module is disabled and otherwise returns `Ok`; in diagnostic mode it calls `get_user_places` once. `shutdown` returns `Ok` at once and releases nothing. The kernel's registry starts and stops it with the others ([modules.lifecycle](../reference/modules.md#r-modules.lifecycle)).

**Idempotence.** [modules.idempotent](../reference/modules.md#r-modules.idempotent) asks that commands be safe to repeat. `create_directory` is, and that is exactly why the New Folder loop in Step 9 never advances.

---

## How it fails

| Operation | Where the failure goes |
| :--- | :--- |
| Listing a folder | The Rust message (`Path does not exist`, `Not a directory`, `Failed to read dir`) replaces the list in the empty-state text (explorer.js line 383). |
| One entry's metadata | The entry is dropped without a message (fs.rs). |
| Drives | After 1,500 ms the page falls back to raw drive letters. |
| Search | Logged to the console; the results panel keeps its previous content. |
| Rename, paste | Shown in the status line by `showOperationError`, with the Rust message. |
| Delete, open file | Logged to the console only. Delete is per item and continues with the rest. |
| Hash | One fixed error label, no cause. |
| Terminal | Judged by whether the process started. |

---

## Known gaps

These gaps were seen in the code while writing this chapter.

- **Names are inserted into markup without escaping.** `renderGrid` and `renderList` put `item.name` into `innerHTML` and `aria-label` (explorer.js lines 1188-1193 and 1235-1238). `escapeHtml` (line 17) is used only for search results (lines 2380-2382), although its comment says file names must never be parsed as markup. The characters that would open markup, `<`, `>`, and `"`, are on the invalid list of `is_valid_filename` (files.rs line 87), but the page does not call that function on listed names, so the code does not rely on it.
- **Cut is a move only inside this window.** `copy_files_to_clipboard` takes a `_cut` argument and never reads it (clipboard.rs line 54). The Windows clipboard receives `CF_HDROP` alone, with no drop-effect flag, so another program or window sees a copy.
- **New Folder never gets a number.** See the warning in Step 9.
- **The Properties action cannot run.** `handleContextMenuAction('properties')` calls `showProperties`, which calls `previewPanel.showProperties` (explorer.js lines 1954-1957). `preview.js` does not define that method, and the context menu has no Properties entry, so the path is unreachable today.
- **Open with opens the default verb.** `openFileWithSystem` calls `open_path_default`, which uses the `open` verb. No Open-with dialog is shown.
- **Delete and open fail silently.** Both only log to the console, and delete asks for no confirmation.
- **No text preview.** The preview panel shows images and metadata. `get_text_preview` is registered but has no caller (preview.rs line 62).
- **No drag and drop.** There are no drag or drop handlers. The `.drop-overlay` style (explorer.css line 1370) is never made visible by any code.
- **Hidden and system entries are not marked.** `is_hidden` is computed (fs.rs lines 52-57), but no UI code reads it.
- **The Recycle Bin path is fixed to drive C.** `get_user_places` uses `C:\$Recycle.Bin` (commands.rs line 115), while `normalize_open_path` uses `SystemDrive` (window.rs lines 29-30).
- **Commands with no page caller.** `sort_files`, `hash_file` (the CRC-32 variant), `get_file_preview`, `get_text_preview`, `get_file_icon`, `get_archive_info`, `is_zip_file`, `extract_archive`, `extract_archive_entry`, `create_zip_archive`, `get_folder_size`, `remove_from_search_index`, `get_search_suggestions`, `scan_directory_full`, `file_exists`, `clear_clipboard`, `is_drive_removable`, `get_unmounted_volumes`, `format_file_size`, `get_file_type_name`, `is_valid_filename`, and the portable-device commands are registered in the kernel but not called by the page. The portable-device commands are stubs that return empty lists (portable.rs).
- **Two sort orders exist.** `sorting.rs` compares names as lowercase strings; the page uses a numeric collator (explorer.js line 178). Only the page order is used.
- **`create_archive` does not add folder contents.** A folder becomes one empty entry (archive.rs lines 172-173).
- **The uninstall path has no caller.** `uninstall_as_default_file_manager` is marked `dead_code` (registry.rs lines 247-248).
