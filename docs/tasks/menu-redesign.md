---
title: Customization menu redesign — plan
type: task
project: unidtk
updated_at: "2026-10-29"
status: done (implemented in session 131)
---

# Customization menu redesign — plan

> **Status:** done (session 131, 2026-10-29). All stages are implemented, the release is built and installed to `C:\Program Files\UniDTK`, and it is verified in the live runtime through the CLI.
> **Basis:** [menu audit](menu-audit.md) (70 problems). Bugs were fixed in session 130; the full UX redesign, positions, folders and the cleanup of the legacy Shell were completed in session 131.

## 1. User decisions

| Question | Decision |
| :--- | :--- |
| Menu form | **Side panel on the right** (~420 px) over the live desktop, instead of a full-screen modal |
| VPN | **Its own VPN panel**; the menu keeps only module on/off switches |
| X/Y fields | **Remove entirely** — placement on the desktop only |
| Migration code from `%LOCALAPPDATA%\Shell` | **Delete** (directory removed 2026-10-28, 6.72 GB freed) |
| Buttons on the desktop | **Only the menu button.** Moving → into the menu («Arrange»); «Reset wallpaper» → remove (does not work); «Exit» → menu and tray |
| Folders | A person should not have to dig into AppData — shortcuts are added without typing paths by hand |

## 2. Target UX

### 2.1 Panel
- A DOM area inside the same wallpaper window (not a second `WebviewWindow`): on the right, ~420 px. The desktop beside it stays visible and live — color, material and visibility change in front of the user.
- Hovering a widget in the list highlights it on the desktop.
- Three sections by task: **Desktop**, **Folders**, **Look**.
- Focus moves into the panel when it opens and returns to the menu button when it closes. Esc closes the panel. `role=dialog`, tabs with `tablist/tab/aria-selected`.

### 2.2 «Desktop» section
- The list of widgets; each has **one** "Show" toggle.
- The **«Arrange»** button switches to arrangement mode directly on the desktop:
  - the panel collapses into a **«Done / Cancel»** bar;
  - grid and snapping to screen edges and neighboring widgets; a widget cannot be dragged off-screen;
  - **Esc = Cancel** (folders do not expand in this mode, so Esc has one meaning);
  - dragging uses pointer events + `setPointerCapture`. Input to the window is forwarded by `PostMessage`, so there is no mouse capture, and HTML5 drag may not start.
- The X/Y fields are removed.
- **Modules** — a block at the bottom of the section: switching a module off, with the explanation «a disabled module uses no resources», and a **«Restart now»** button with a warning that the player and Telegram will restart.
- The VPN row — «Configure…» opens the VPN panel. The VPN widget may be hidden, otherwise the settings cannot be reached.

### 2.3 «Folders» section — without AppData
1. **«Add from Start»** — a list of installed programs (`.lnk` files from `%ProgramData%` and `%APPDATA%\Microsoft\Windows\Start Menu\Programs`) with search and real icons. The name and icon are filled in automatically. This is the main path for an ordinary user. UWP apps without a `.lnk` are not supported in the first version and are shown as unavailable.
2. **«Choose a file or folder…»** — the Windows system dialog instead of typing a path.
3. **Drag and drop** a file from Explorer, or a link from a browser, onto a folder on the desktop — **only if the stage 1 check passes** (see §4).
4. **Ctrl+V** in an open folder: a file from the clipboard (CF_HDROP), or a link/text. The link name comes from its domain (the page title cannot be obtained, see §4), and it can be renamed.
5. **Editing directly on the desktop:** in an open folder, a «+» tile; right-click on a shortcut → «Rename / Remove»; order is changed by dragging. Right-click and dragging work only inside an open folder; ordinary widget clicks are not affected.
6. **«+ Folder»** in arrangement mode — the folder appears on the desktop immediately, with a name field and an icon picker.
7. A shortcut points to the **real** file, program or URL, not to a `.lnk` copy in AppData.
8. A shortcut to a removed file gets an «unavailable» state with the actions **«Find…»** and **«Remove»**.
9. Russian plural forms («1 ярлык / 2 ярлыка / 5 ярлыков», i.e. 1 / 2 / 5 shortcuts), no "redb", no "FILE", and no type dropdown — the type is detected automatically.

### 2.4 «Look» section
- Two large theme cards (Classic / Sea Glass), drawn with the real material.
- **Colors:** the accent from the current theme's palette; «Shuffle» with an undo toast.
- Everything else goes into a collapsed **«Fine-tuning»** block. Labels are in Russian only, without English parentheses or jargon.

### 2.5 Common rules
- Instead of `alert`/`confirm`, one toast style (`role=status`, `aria-live`).
- Irreversible actions execute immediately, and for 5 seconds they can be **undone** from the toast. Undo restores **everything** affected: config, positions, `hidden`, `instances`, folder records.
- Enter submits forms.
- Visible focus (`:focus-visible`), text contrast ≥ 4.5:1, click targets ≥ 24 px. The CSS items of the audit are handled along the way.
- Empty states: all widgets hidden → a hint next to the menu button; empty folder → a «+» tile with a hint; Start list empty or search finds nothing → a message; load error → an error with «Retry», not a false «empty».

## 3. Data and architecture

### 3.1 Positions anchored to a corner
- A position record is extended with `anchor ∈ {tl, tr, bl, br}` and an offset. Widget fields in the same record (Telegram `activeChatId/expanded`, micrographics `mgScale/mgStyle/mgLayout`, player `version/aiDj`…) are preserved via spread.
- **Migration without shift:** a record without `anchor` is read as `tl` with the same coordinates, so nothing changes visually. The nearest corner is computed and written only on «Done», when the canvas size is known. Temporary sizes during sleep or display changes do not reach the data.
- `telegramDisplayPosition` (thresholds 1200 / 1700 px, margin 12) is replaced by ordinary `tr` anchoring and removed.
- Expanded (`.expanded`) cards are recalculated separately: `relayoutWidgets` skips them.
- The architecture §4 rule is respected: arrangement only changes the geometry of existing cards, and `widgetsCanvas` is not redrawn.

### 3.2 Folders, Start, icons
- Start menu enumeration and icons live **in the core**: a new `core/kernel/start_menu.rs`, and thin `#[tauri::command(async)]` wrappers in `main.rs` (commands are declared only where `generate_handler!` is). Folders are already in the core (`get_folders/save_folder/delete_folder`).
- Win32 work runs on a worker thread with `CoInitializeEx`, not on the UI thread (architecture §3): `IShellLinkW::GetPath` for `.lnk`, and `SHCreateItemFromParsingName` + `IShellItemImageFactory::GetImage` → BGRA. Icons are cached in `%LOCALAPPDATA%\UniDTK\cache`. There is no PNG encoder, so BGRA is returned and drawn through canvas, with no new dependency.
- The Start list loads when the section is opened, not at core startup.
- The system dialog goes **through a core command** (the Rust API of `tauri-plugin-dialog`). `lint_ui_imports` forbids UI files from importing anything except through `@unidtk/bridge`.
- Clipboard: `modules/explorer/rust/clipboard.rs` has only CF_HDROP. Add reading of `CF_UNICODETEXT` / `UniformResourceLocatorW`.
- Existing shortcut copies in `UniDTK\folders\*\shortcuts`: a one-time migration at startup expands each `.lnk` into its real target (plus arguments). If the target is not found, the record stays as it is. Copy files are deleted only after verification.
- `FolderItem` gets an icon field (or the icon is extracted from the target on the fly, with a cache).
- New reads go through the UI door. They go into `command_inventory.rs` / `WS_COMMANDS` only if they are needed from `udtk invoke`, and only after a shadow run.

### 3.3 Removing buttons and commands
- `#quitApp`: **first** switch `udtk stop` (`opt/cli/main.rs:1636`, which clicks the button through CDP) to a direct `invoke("quit_app")`, and only then remove the button and its handler (`index.html:72-76`, `unidtk-core.js:1725-1727`). The `quit_app` command and the tray item «Exit» stay. They are the fallback exit if the menu breaks.
- `#editMode`: remove the button (`index.html:58-63`). Entering arrangement from the menu becomes a direct function call instead of `document.getElementById("editMode").click()` (`menu.js:364-368`). The `body.edit-mode` class and the `editMode` checks in modules remain.
- `#resetWallpaper` / `reset_wallpaper`: remove completely — the button (`index.html:66-70`), the handler (`unidtk-core.js:1729-1731`), the command and its registration (`main.rs:232-234`, `generate_handler!`), and the tray item «Reset wallpaper». `wallpaper::reset()` on exit stays. The JS call and the `generate_handler!` entry are removed together, otherwise `frontend_invoke_names_are_registered` fails.

### 3.4 Deleting the migration from `Shell`
- Remove: `core/api/src/paths.rs` (the `old_dir` branch, the marker `.migrated-from-shell`, `migrate_entry`); `unidtk_db.rs` (the legacy candidates `shell.redb`, `rebase_legacy_shortcut_targets` and its test); `main.rs` (the `Shell\folders` fallback, the shortcut transfer call); `log_store.rs` (`shell_logs.redb`); `module_store.rs` (the `legacy_root = …/Shell` branch); the legacy name `shell_ai_dj.redb` in `modules/mp3-player/rust/commands.rs` and the test in `module_store.rs`; in `opt/cli`, `cleanup_legacy_vpn_files` and `"Shell"` in `OBSOLETE_TASKS` — to be reviewed; outdated comments.
- **Do not touch:** the `StoreSpec::legacy_files` mechanism (shared), `core/kernel/library_migration.rs` (migration of tables inside the current database, not of the `Shell` directory), and the VPN settings migrations from `data_dir()`.

### 3.5 Motion
- The panel and toasts use only the `motion.js` tokens (`DURATIONS`, `--ease-*`), with no new `ease` or `cubic-bezier`.
- WAAPI animations (`animate()`) do not obey the CSS `prefers-reduced-motion`. They rely on `currentScale` / reduced-motion from `motion.js`.

## 4. Technical risks (check before the main work)

| Risk | What is known | Check |
| :--- | :--- | :--- |
| **Dragging from the desktop** | Critic: the drop may be caught by the desktop icon layer (`SysListView32`) and create a shortcut on the desktop. Against: when scrolling with a touchpad, `WindowFromPoint` over the shell returned our window (`Chrome_RenderWidgetHostHWND`). Also, Tauri intercepts OLE drop by default (`dragDropEnabled`): file paths arrive, browser links probably do not. For both, an own `IDropTarget` is needed (feature `Win32_System_Ole`). | A log of the window under the cursor during **one manual drag by the user**, plus a check whether a shortcut appeared on the desktop. If it does not work, item 2.3.3 is dropped. |
| **System dialog behind the desktop** | The window is a child of WorkerW, and the process does not get focus. The dialog may open behind or flash in the taskbar. `set_focus` is forbidden. | Open the dialog by clicking the desktop. If there is a problem: no parent window + `SetForegroundWindow` on the dialog. |
| **Ctrl+V** | Ctrl+V chains are already forwarded (Explorer works this way). `OpenClipboard(NULL)` works without focus. | A log of `ctrlKey` in an open folder, via `udtk gui js`. |
| **Page title** | There is no HTTP client in the core, and CSP forbids external `fetch`. | Decided: the name comes from the domain. |
| **`architecture.md` limit** | 326 lines, 29.6 KB against a 30 KB limit. | Shorten new descriptions, compress old ones. |

## 5. Stages

Each stage: build with `udtk build`, a live check through the CLI (`udtk gui`, `udtk shot`), then a commit.

1. **Risk check §4** — from the user, one file dragged onto the desktop.
2. **Cleanup:** the `Shell` code, the desktop buttons, `reset_wallpaper`, the `udtk stop` path.
3. **Panel frame:** sections, toasts with undo, Russian labels, focus and keyboard.
4. **Desktop arrangement:** grid, snapping, corner anchoring, position migration.
5. **Folders:** Start with icons, the dialog, paste, drag and drop (if stage 1 passed), editing on the desktop, `.lnk` migration, unavailable shortcuts.
6. **«Look»:** theme cards, colors, collapsed fine-tuning.
7. **VPN panel.**
8. **Documentation:** `architecture.md` (§1.2 menu, §0/§2 paths, §5.5, §7), `00_master.md`, `cliudtk.md` (the `udtk stop` path, menu scenarios), `AGENTS.md` (mentions of `Shell`), the session log.

## 6. Plan critique (4 Haiku 5.5 subagents) — what was taken into account

- **UX:** a fallback exit if the menu breaks (tray), combining the panel with arrangement, gesture conflicts, empty and broken states, undo restores everything, keyboard and ARIA. The item «first fix the foundation» is already closed by session 130.
- **Win32:** z-order and OLE drop, interception of the Tauri drop, the dialog and focus, icons without a new dependency, pointer capture instead of HTML5 drag.
- **Data:** position migration without shift, preservation of widget fields, expansion of `.lnk` copies, the exact list of what to delete from `Shell` and what not to touch, `udtk stop` depends on `#quitApp`.
- **Architecture:** Start and icons in the core, commands only in the core crate, Win32 not on the UI thread, `lint_ui_imports` for the dialog, motion tokens and reduced-motion for WAAPI, the `architecture.md` limit.
