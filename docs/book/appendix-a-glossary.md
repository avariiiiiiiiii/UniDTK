# Appendix A: Glossary

This glossary defines the terms that the book and the code use. Each entry gives the meaning in the project and names the file or the rule where the term is defined. The entries are in alphabetical order. A term that is a file or a command is written in code font.

**AI DJ.** A feature of the mp3-player module. It runs the discogs-maest-129e ONNX model over tracks to get embeddings, builds a similarity graph, and finds transition paths between tracks. Source: `modules/mp3-player/rust/ai/ai_dj.rs`.

**ByeDPI.** A local SOCKS5 proxy, shipped as `ciadpi.exe`, that sends desynchronized packets to get past blocking. The Telegram module runs it on `127.0.0.1:12909` and points TDLib at it.

**CDP.** The Chrome DevTools Protocol, which WebView2 exposes on a local port that the `udtk` commands `gui`, `shot`, and `cdp-list` use. The port is chosen at launch and published in the reply to the ping.

**Crash log.** The file `crash.log` in `%LOCALAPPDATA%\UniDTK`. The panic hook appends the full report of every panic to it. See [Chapter 9](ch09-when-things-fail.md).

**Desk.** The window behind the desktop icons, where the widgets are drawn. The rules are in [The Desk, the Menu, and Themes](../reference/desk.md).

**Door.** One of the two ways into the kernel: the interface door, used by the page, and the socket door, a read-only TCP port on `127.0.0.1:26540` used by `udtk`.

**Easing.** The curve that maps time to progress in an animation. `core/ui/motion.js` names ten of them in `EASINGS`, for example `standard` and `spring`. See [Chapter 8](ch08-themes-and-motion.md).

**Folder.** A desk shortcut kind, `FolderItem`. It points to the real target, not to a copy of a `.lnk` file. Defined in [The Desk, the Menu, and Themes](../reference/desk.md).

**FOUC.** A flash of unstyled content: a frame painted with the wrong colors before the saved theme is applied. The `:root` fallbacks in `styles.css` and the pre-paint cache exist to prevent it.

**Frontend.** The JavaScript that runs inside the WebView2 page, in `core/ui` and in the `ui` half of each module.

**Kernel.** The binary `unidtk.exe`, built from `core/kernel`. It is the invisible frame: the IPC server, the event bus, the tray, the storage, and the attachment of the window to the desktop. See [The Kernel](../reference/kernel.md).

**libmpv.** The media playback library. The mp3-player module loads `libmpv-2.dll` through its FFI layer, in `modules/mp3-player/rust/ffi/libmpv.rs`.

**LIFO stack.** The stack of open surfaces, `SurfaceManager.activeStack`. Escape closes the topmost surface first. The rule is [motion.lifo-stack](../reference/motion.md#r-motion.lifo-stack).

**Module.** A vertical slice, `modules/<id>/{rust,ui}`, with a backend half and a frontend half. A module depends on `unidtk-api` and never on the kernel or on another module. See [Modules](../reference/modules.md).

**ModuleDb.** The key-value interface that a module receives. Its keys live in the module's slice `mods/<id>/` of `unidtk.redb`. A module never gets the raw database.

**Nothing Classic.** The default built-in preset, with the id `nothing-dark`. Its values are also the `:root` fallbacks in `styles.css`.

**NSIS.** Nullsoft Scriptable Install System. `cargo tauri build --bundles nsis` builds the installer, and `udtk build` installs it silently into `C:\Program Files\UniDTK`.

**Panic hook.** A function that Rust calls on every panic, before unwinding starts. UniDTK installs its hook first in `main()`, in `core/kernel/panic_hook.rs`.

**Pre-paint cache.** The `paint` object in `localStorage`, under the key `unidtk_theme_state`. A classic script in `core/ui/index.html` applies it before the first paint.

**Preset.** A complete theme record with an `id`, a `name`, and every field of the schema, which is version 4. The built-in presets are Nothing Classic and Sea Glass.

**Progman.** The Windows Program Manager window, which owns the desktop. UniDTK sends it the message `0x052C`, and Windows answers by creating a `WorkerW`.

**redb.** An embedded key-value database written in pure Rust. UniDTK keeps user data in `unidtk.redb` and the log in `unidtk_logs.redb`.

**relayoutWidgets.** The function in `core/ui/unidtk-core.js` that moves and resizes the existing widgets when the geometry changes. It never resets the widget canvas. The rule is [desk.relayout-not-reset](../reference/desk.md#r-desk.relayout-not-reset).

**Scheduled task.** A Windows Task Scheduler entry. UniDTK registers three of them from the templates in `scheduler-tasks/`: `UniDTK`, `UniDTKSingbox`, and `UniDTKVpnStop`. The rule is [install.native-tasks](../reference/install.md#r-install.native-tasks).

**Sea Glass.** The second built-in preset, with the id `sea-glass`. It uses the `jelly` material and a pastel palette, and it gives each user a personal palette on the first run.

**SOCKS5.** A proxy protocol. sing-box exposes a local SOCKS5 listener on `127.0.0.1:12908`, and the ByeDPI instance listens on `127.0.0.1:12909`.

**StoreKind.** How a module's store is kept: `Shared` in the kernel's database, `OwnedDurable` as a user-data file of its own, or `OwnedCache` as a rebuildable file of its own.

**StoreSpec.** A module's declaration of a store: a short name and a `StoreKind`. The kernel picks the file path and opens the store.

**Surface.** A transient overlay, such as a popup, a modal, or a menu, managed by `SurfaceManager`. Unlike a widget, a surface opens and closes.

**Tauri.** The Rust framework that hosts the window and the page, and provides the `invoke` commands. UniDTK uses Tauri 2.

**TDLib.** Telegram's client library, loaded from `tdjson.dll`. The telegram module uses it for the Telegram session, which is stored under `%LOCALAPPDATA%\UniDTK\tdlib`.

**Tint.** A surface that takes a palette color, such as the panel or a folder. A module declares its tints in `metadata.json`, under `tints`.

**Token.** A named value, such as `--dur-motion` or `--theme-accent`. CSS and JavaScript read it from one place, instead of using a literal. The rule is [theme.tokens-only](../reference/desk.md#r-theme.tokens-only).

**TUN.** A virtual network adapter. sing-box creates one, and with `auto_route` and `strict_route` set it carries the system's traffic through the VPN.

**udtk.** The command-line tool, built from `opt/cli`. It talks to the running application through the socket door and the CDP port.

**Undo toast.** The toast that offers Undo for five seconds after an irreversible action, in place of a dialog. Dialogs such as `alert` and `confirm` are not used.

**unidtk-api.** The contract crate, in `core/api`, with the `Module` trait, the services, `ModuleDb`, `StoreSpec`, and the paths. Its only dependencies are `serde_json` and `dirs`, so it does not depend on Tauri or on redb.

**Widget.** A permanent card on the desk, drawn by a module. A widget's position is user data. See [The Desk, the Menu, and Themes](../reference/desk.md).

**WebSocket door.** The interface door for the page. It listens on a loopback port that changes on every run, and it accepts no command until the handshake with the per-run token has succeeded.

**WebView2.** Microsoft's embedded browser control, based on Chromium. The UniDTK page runs in it.

**WorkerW.** A Windows window that holds the desktop wallpaper, behind the icons. The UniDTK window is reparented under a `WorkerW`, in `core/kernel/wallpaper.rs`.
