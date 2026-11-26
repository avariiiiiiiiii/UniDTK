# Plan: the Native Desktop Host on Rune

This page is the working plan for replacing the WebView2 interface with a native GPU host built from the source code of Rune (`github.com/anomalyco/rune`, MIT). It covers issue #17 and the motion work of #535 and #537. When the plan is done the page moves to `docs/history/`.

## Decisions

- Rune is the engine. The GPUI host is removed: code, chapter, branch.
- Rune is not a dependency. Its source code is copied into the tree at one upstream commit, and from then on it is our code. Third-party crates under it (`wgpu`, `winit`, `parley`, `taffy`, `accesskit`, ICU) stay ordinary crates.io dependencies.
- Only part of Rune is taken: `rune`, `rune-motion`, `rune-taffy`, `rune-staging`, `rune-fx`, `rune-light`, `rune-notify`, `rune-i18n`, `procwatch`, `rune-perf`. Not taken: `rune-kit`, `rune-mermaid`, `rune-highlights`, `rune-luau`, `rune-knobs`, `rune-mac`, `rune-ref`, `rune-register`, the examples, the site.
- The production build stays Tauri and WebView2 until the native host reaches parity. Then the web interface is deleted, with no tag and no legacy copy.
- Leaving Cargo and Tauri for plain `rustc` and blaze is a separate issue and is not part of this plan.

## Step 0. Remove GPUI

Delete `hosts/gpui/`, its workspace member in `Cargo.toml`, `docs/reference/gpui-host.md`, `docs/history/gpui-gates.md`, every link to them, the GPUI rows of `docs/status.md`, and the local branch `feature/gpui-desktop`. The session log keeps its entries: it is the record of what happened.

**Done when.** A search for `gpui` outside `docs/sessions.md` and this plan finds nothing; `udtk doc --check` passes; the workspace compiles.

## Step 1. Import Rune

Copy the ten crates from upstream commit `2b7dbf223b84b58bcfb1e5a9e92577b9244540dd` (2026-11-11) into `core/rune/<crate>/`: manifests, `src/`, the assets they embed (Inter, the precompiled DXBC uber shader), and the upstream `LICENSE`. `core/rune/UPSTREAM.md` records the commit, the list, and every change made to the copy. The import commit adds the files without making them workspace members, so it is byte-identical to upstream and builds nothing.

**Tests are not taken.** Rune has about 140 test targets and in-source test modules; 34 files need a real GPU adapter (`Gpu::headless()`), some embed fonts from the `opencode` theme, which is not taken, and under the release profile every test target is a thin-LTO link of `rune` and `wgpu`. The engine is verified at runtime through the inspector door (Step 4), not by its unit tests.

## Step 2. Fit and cut

One commit makes the copy a part of the workspace and removes what never runs on this desktop. Each cut is a whole file or directory plus its `mod` line, feature and dependency, so the diff against upstream stays readable; `core/rune/UPSTREAM.md` lists every cut.

1. **Workspace.** The root `Cargo.toml` gains the members, the `[workspace.package]` values they inherit (edition 2024, licence MIT), and Rune's `[workspace.lints]`, which only crates that opt in with `lints.workspace = true` inherit. Upstream `clippy.toml` goes next to them.
2. **Toolchain.** Rune pins `rustc 1.99.1`. First the copy is compiled on the current toolchain; only if it fails does the workspace move to a root `rust-toolchain.toml`, in its own commit, with the full test suite, `udtk build`, the build time, and `rustc --version` in the commit body.
3. **Cut.** In-source test modules and test-only files; macOS, iOS, web, Wayland and X11 code; the Metal backend and the `.metallib` files; `video/` (the player is libmpv), `recorder/`, `rewind/`, `playground/`, hot reload; the features `ogg-audio`, `metal-backend`, `dbus`, `hot`, and the dependencies only they use. `svg` and `accessibility` stay.
4. **The `wgpu-hal` patch is not taken now.** Without it the adapter pick creates a device on every GPU (1.05–1.11 s on a two-GPU laptop, by Rune's own measurement, and it wakes a discrete GPU) and FXC loads with the instance. The cost is measured at G0 on this machine; taking the patch means carrying the source of `wgpu-hal`, which is decided on those numbers.
5. **The book check.** `udtk doc --check` scans the comments under `core/`; Rune's comments cite its own `PLAN.md` sections and documents. `core/rune/` is excluded from the code scan in `opt/cli/book.rs`, with a test.

**Done when.** The workspace compiles, `cargo test --release --workspace -j 1` is green, `udtk doc --check` passes, and the size of `unidtk.exe` (55,025,664 bytes before) is unchanged, because nothing uses Rune yet.

## Step 3. Process model

Tauri runs on `tao` and Rune on `winit`; one main thread cannot run both event loops. Until the switch, the native host is a separate process.

- Binary `unidtk-desktop`, a `[[bin]]` in `toolchain/Cargo.toml` ([repo.flat-root](../reference/kernel.md#r-repo.flat-root)), source `core/desktop/main.rs`. `udtk build` adds it to the installed and SHA-256-verified files; `install.md` and the `udtk` book, which speak of three binaries, change in the same commit.
- **The kernel starts the host** after its doors are up, and hands it the WebSocket token on its standard input. A host started by hand retries the connection with back-off.
- The kernel keeps every module, the database, the low-level hooks and the tray. The host reads and changes data only through the kernel's doors ([ipc.one-contract](../reference/ipc.md#r-ipc.one-contract)); it never opens `unidtk.redb` ([storage.no-raw-db](../reference/storage.md#r-storage.no-raw-db)) and never installs a hook.
- **The host subscribes before it reads a snapshot** and drops the frames older than the snapshot by their sequence number, so no event between the two is lost.
- **One owner of the desktop.** A kernel setting `desk.host = web | native` decides which window is attached under WorkerW; the kernel skips its own wallpaper attach and `wallpaper::reset` when the native host owns the desktop. Production stays `web` until G5.
- **The host's windows are shell windows.** The kernel classifies the foreground as "shell" by its own process id (`core/kernel/hotkeys.rs`); it learns the host's process id when it starts the host, so input routing and the Win+E scope treat host windows as its own.
- **Text input.** A `WS_CHILD` of WorkerW never becomes the foreground window and gets no keyboard focus; the kernel already routes keys through its low-level hook. The host receives typed text and keys from the kernel over the door. IME composition (CJK) is not supported, as today.
- **The inspector door.** Read commands (`inspector.tree`, `inspector.png`, `inspector.frames`) are declared in `core/kernel/command_inventory.rs` and served on the socket ([ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only)). Input (`udtk gui click`, `type`, `press`) is a write and needs the handshake token, as the WebSocket door does; the token lives in a file under `%LOCALAPPDATA%\UniDTK` readable only by the user, never in `ping`. The host's port is reported by the kernel's `ping`. `udtk shot` reads the host's frame back from wgpu. `ipc.md` changes at G0, in the same commit as the door.
- At the switch the kernel drops Tauri and becomes a windowless process; whether the host then merges into it is decided at that gate.

Note: today `ping` answers without authentication and reports the CDP port, and CDP allows `Runtime.evaluate` and input — so the WebView door is writable by any local process. This is an existing hole in [ipc.socket-read-only](../reference/ipc.md#r-ipc.socket-read-only), tracked as its own issue; it disappears with CDP at G5.

## Step 4. Gates

Each gate ends with the full verification below and a screenshot comparison with production.

| Gate | Content | Done when |
| :--- | :--- | :--- |
| **G0** | **First, proof of transparency:** Rune's transparent path on Windows is a topmost layered overlay over DirectComposition (`core/rune/rune/src/overlay/`); a per-pixel transparent wgpu surface on a `WS_CHILD` of WorkerW is not proven and is the first thing built and measured, together with the z-order against `SHELLDLL_DefView` and the Progman case. Then a transparent Rune window under WorkerW with two-way hit-testing (`WM_NCHITTEST` returns `HTTRANSPARENT` outside widgets). An inspector door on the host: element tree, input, PNG, frame times, wake-ups. `udtk gui tree`, `udtk gui click`, `udtk shot` and `udtk doctor -simulate` work against it. | `udtk shot` returns a PNG of the host; the measurements below pass |
| **G1** | Clock, Glyph Bar, the 16 px desk grid, positions through the kernel ([desk.positions](../reference/desk.md#r-desk.positions)). Motion tokens on `rune-motion` and a reference chapter for the host. | Visual parity with production screenshots |
| **G2** | The central menu and settings ([kernel.menu-in-core](../reference/kernel.md#r-kernel.menu-in-core)), themes Classic and Sea Glass ([theme.tokens-only](../reference/desk.md#r-theme.tokens-only)), folders, one surface transform for every object-to-surface morph (#535). | Every setting is reachable without WebView2 |
| **G3** | Micrographics (with its own geolocation permission), VPN, the Telegram rail. | Modules work; writes go only through the interface door |
| **G4** | MP3 player (arcs, AI DJ), Explorer with a virtual list (#136) and thumbnails (#137). | A folder of 10,000 entries opens without a freeze |
| **G5** | The switch: the `UniDTK` task starts the native host; the web interface, CDP and Tauri are removed; `motion.md`, `ipc.md` and `install.md` are rewritten in the same commit. Depends on the rustc and blaze issue for the installer. | The full cycle below passes without WebView2 |

The WorkerW attach exists in the kernel today (`core/kernel/wallpaper.rs`) for the WebView window. The host gets its own copy at G0; the kernel copy is deleted at G5 with the WebView.

### G0 so far (session 138)

- `unidtk-desktop` exists as a `[[bin]]` with one mode, `--probe-transparency <report.json>`: it reads the desktop host as DWM composed it (`PrintWindow` with `PW_RENDERFULLCONTENT`, which works while other windows cover the desktop; a screen copy read the terminal in front, and black while the session was locked), attaches a full-screen transparent Rune window as a child of the desktop host (`core/desktop/desktop_host.rs`, Progman on this machine), draws one half-transparent card, reads the host again and writes the report with Rune's launch marks. `UNIDTK_PROBE_DUMP=<dir>` also writes both images.
- **Transparency: proven.** A transparent Rune window that is a `WS_CHILD` of Progman composites per pixel through DirectComposition: the wallpaper and the WebView widgets under it stay visible, and the card blends over them (the point inside it 219,219,236 -> 236,236,245, three points outside unchanged; `transparent: true` in the report). `WS_EX_LAYERED` is not needed; the layered variant gave the same images. Points are sampled in physical pixels, Rune lays out in points (1.5 at 150 %).
- **First frame.** The window attaches 0.2-0.5 s after launch and presents its first frame at 276-576 ms (Iris Xe, DX12 through DirectComposition). Before the line-end fix in `core/rune/UPSTREAM.md` (item 5) it took 57.3 s, because the precompiled uber shader was refused and FXC compiled it on every launch. A frame is `Skipped` while the desktop is covered, as Rune intends.
- **Memory: 67 MB Working Set, 58 MB private** after the first frame of one full-screen window, under the G0 threshold of 80 MB. It was 124-132 MB: `wgpu`'s default limit of 1,000,000 non-sampler bindings made `wgpu-hal`'s DX12 backend create a shader-visible view heap of that size, which the driver commits and zeroes, 61 MB of private memory with Intel's 64-byte descriptors. Rune asks for 65,536 now (`core/rune/UPSTREAM.md`, item 7). Of the 67 MB, 40 MB are images of DLLs (39 MB of them shared with other processes: the system, D3D12, the Intel driver), 25 MB private (6 MB of it the GPU upload heap, 4-5 MB the view heap). Measured by walking the working set (`QueryWorkingSet`) against the regions (`VirtualQueryEx`).
- **The desktop host here** is Progman of 1920x1680 (two displays, the second below the first); the probe covers the primary only.

## G1 plan (session 141, after five critics)

Scope decisions (revised in session 142, at the user's word that the port is a full equivalent): the Clock ports all thirteen faces as the page draws them, the broken ones included (`gothic` has no CSS, `analogue-dot` prints fixed calorie and step counts, `nothing-watch-pro` a fixed "CMF BY NOTHING"); fixing them is a separate decision. Sea Glass is the stored theme, not an approximation: every token is read from `config.theme`; only the wallpaper blur is missing, because Rune on Windows cannot blur what lies behind a child window. The arrange mode (dragging widgets) belongs to G2 with the menu; G1 builds and tests the write path through the CLI (`udtk host desk`).

**Kernel (written, compiles, not yet built).**
- `desk_position_set {id, x?, y?, ax?, ay?, w?, h?, anchor?, version?}` merges position fields of one widget into the stored layout in one redb write transaction ([`core/kernel/desk_layout.rs`](../../core/kernel/desk_layout.rs)); other keys of the record (module settings) are not accepted, a missing or broken stored layout is refused, not replaced, because a blob holding one widget would pass for a complete layout and block the page's first-run defaults.
- `desk_layout_get` answers `{rev, config}`; the state event `desk-layout-changed` carries `{rev, positions, theme}` (the whole map, never a delta, because a state event keeps only its newest frame). One lock is held across commit and publish, so frames leave in commit order, and `rev` (monotonic across restarts: it starts from the clock) lets a host that reads a snapshot and receives frames tell which is newer. The page's whole-layout save goes through the same lock and publishes too.
- While `desk.host = native` the page does not exist (the kernel destroys its window), so the host is the only writer; the stale-snapshot overwrite by the page (theme switch, clock face click, arrange Cancel all write the whole blob from memory) cannot happen. It stays a hazard of the switch moment and of any future second writer.

**Host foundations (first, they are G0 gaps the critics found).**
- Ready with an empty layout: the hit region is built from every visible widget, not from `g0-card`, and `ready` means attached, tree committed, layout known; an empty region is a state, not a failure.
- Per-monitor DPI awareness is set before the first window; the region is recomputed on a DPI or display change (`WM_DISPLAYCHANGE`, `WM_DPICHANGED`); a locked session or `OpenInputDesktop` NULL reports `attached: false` and drops input focus.
- The kernel link grows an inbound path (events to the UI thread, `gap` triggers a refetch, replies by `id`), a bounded outbound command queue with latest-wins for drag positions and absolute values for the volume (never a toggle), `WouldBlock` as flush-later, the old client stopped outside the slot lock, and a reconnect loop with back-off; a host started without WebSocket credentials is an error. Explorer arriving late is waited for in the process, not by restarting it.
- `sync_tree_snapshot` runs on commit and when the inspector asks, not on every drawn frame.
- Explorer restart: `on_taskbar_created` invalidates discovery and reattaches a surviving window. If Explorer destroyed the child, the host exits so `HostSupervisor` can start a fresh process and window.
- The hit region follows the layout revision, not only commits and drawn frames, so a moved widget is not clickable at its old place; it uses the transformed bounds of a rotated element and keeps a widget that is still painting its exit animation.
- Writes to the kernel (`set_system_volume`, `desk_position_set`) must not stall the events of the same connection: the kernel dispatches a write on a worker with a deadline, or the host writes on a second connection; the volume drag sends the band on change through the latest-wins queue.
- The input focus is sent in full on every connect and kept pending when a send fails; a host without credentials fails loudly.
- The window follows the canvas it draws on (the primary display: the layout's coordinates are canvas coordinates, and anything below the primary edge of a 1920x1680 Progman is neither painted nor hit-tested), and the layout is clamped to it.

**Desk model.**
- Positions are CSS pixels of the old page, which equal Rune points (1280x720 at 150 % on this machine); every field is used as stored, never divided by the physical scale. Anchors follow `placeOnCanvas`; no relayout while the canvas is 0; unknown ids and fields are kept verbatim; a disabled module's widget is skipped but keeps its record.
- Sizes: the stored `w`/`h`, else the module's `defaultPlacement` (the four sources disagree today: metadata 210x190 and 56x587, theme presets 207x187 and 14x167, `defaultWidgetPosition` 210x180 and 10x127). The host never creates a layout; with none stored it draws the metadata defaults, generated at build time from the `metadata.json` files that `build.rs` already parses, and the face comes from the same place, so `rings` is not lost to the page's `digital` fallback.
- The reference sentence "16 px grid" is wrong: the page snaps with a 10 px magnet and a 12 px gap. The grid belongs to the arrange mode (G2); the sentence in `desk.md` is corrected then, in the same commit as the mode.

**Rendering.**
- `AppSpec.fonts` replaces Inter, so Inter (3 weights) and Ndot 55 are both embedded; a test checks that Ndot has every glyph the faces print, because a missing one pulls a system font into the process.
- Theme tokens (Classic, Sea Glass: colors, radii, OKLCH ink) are ported from `theme.js` to Rust in `core/desktop`; the page keeps its copy until G5.
- Clock: `digital` wakes once a minute, `rings` once a second through one `wake_at` at the next whole second; the outer ring turns by `rotate` in 6-degree steps, never by a looping animation (a looping track keeps every frame a full draw). Text sizes come from a fixed set, motion by transform, so the glyph atlas does not churn.
- Glyph Bar: ten elements, flat and jelly, input through `set_system_volume` and `set_system_mute`; the wheel and drag rules of the page are copied (a step is 10 %, level 10 lights only the mute LED).
- **Open measurement:** the host is one full-screen window (2880x1620 px at 150 %), and Rune has no partial present. A 1 Hz redraw is unmeasured; if it costs more than the 0.2 % idle budget, the clock gets its own small child surface.
- **Found, not G1's to fix:** `ipc.md` says a stray local process "can look, but not act", yet the always-on CDP port lets any local process call `session_info`, take the UI token and issue UI-door writes; the plan already tracks CDP as its own issue and it disappears at G5, but the sentence in `ipc.md` is false until then, and the host holds the full UI token, not a scoped one. The supervisor holds its state lock across a blocking write to the host's pipe (a wedged host can hang the kernel's shutdown), and the key forwarder drops key-ups when its queue is full.
- Kernel side, small: the Glyph Bar's volume thread polls WASAPI 8.3 times a second and retries once a second without an audio device; it backs off to 30 s on errors. Replacing the poll with endpoint callbacks is a separate item.

**Order.** One batch, one build: the G0 title-bar fix, the kernel commands, the host foundations. Then G0 is closed again with a DWM-composed picture, and the desk model, theme, fonts, Glyph Bar and Clock follow, each verified on the desktop.

### G1 so far (session 142)

- **Desk model and link** ([`core/desktop/desk.rs`](../../core/desktop/desk.rs), [`core/desktop/host.rs`](../../core/desktop/host.rs)): the host subscribes to `desk-layout-changed`, asks `desk_layout_get` on every (re)connect and on a `gap`, keeps the newer `rev`, and places widgets as `placeOnCanvas` does. A supervised host draws nothing until the kernel has answered; a standalone one draws the modules' defaults. The hit region is built from every `widget:*` element. `udtk host desk <id> <field>=<value>...` writes one position record through the host's own link (inspector command `desk`, token-gated).
- **Theme** ([`core/desktop/theme.rs`](../../core/desktop/theme.rs)): the whole `normalizeTheme` (both presets, clamped numbers, allow-listed fonts, palette, tint map, sizes) and the colors the two materials derive (`inkFor`, OKLCH relative colors, `color-mix`). A test pins the clock's ink to the color measured on the production desk, rgb(46, 12, 120).
- **Clock** ([`modules/clock/native/faces.rs`](../../modules/clock/native/faces.rs), moved there in session 143): the tile in both materials (jelly body, sheen, depth, rims, drop shadows and the 64 px grain; flat glass with its border and computed shadows) and all thirteen faces of the page's switcher, each laid out in its `.clock-container` as `clock.css` styles it (`gooey`'s digits from its own SVG paths; `gothic` as the page draws it, black SVG fills, because `clock.css` has no rules for it). A record without `version` draws `digital`, as `renderClock` does; only the module's default placement names `rings`. A press opens the dashboard (400 x 420 round the tile, the face flying into the 160 px `.dash-clock`, the month calendar and the connection badge); a press on it, a press on the bare desk, or Escape closes it. Fonts are the theme's: Ndot and Geist embedded (Geist converted from the page's WOFF2 to TrueType), Segoe UI and Consolas from the system.
- **Differences that remain, named:** the edit-mode face switcher (G2, with the arrange mode); `gooey`'s 220 ms settle when a digit changes; `backdrop-filter` blur under a flat card (Rune cannot blur the wallpaper behind a child window); while the dashboard is open the whole canvas takes input, so the first press on a desktop icon closes the dashboard instead of selecting the icon.

## G2 moved ahead of the rest of G1 (session 142, the user's order)

In `native` nothing could be changed: no menu, no arrange mode, no folders. The user ordered the kernel's own interface (`r[kernel.menu-in-core]`) before more widgets; the Glyph Bar waits. Three read-only surveys inventoried `core/ui` (the menu with every control, the surface and motion system, arrange mode, folders, nesting, input), and five critics reviewed the plan (races, Win32 input, memory and CPU, parity with the page, cold start and the mode switch). The user decided: everything stays in the desk window (no second window), so text fields take keys forwarded by the kernel, Latin and Cyrillic only, no IME; the Explorer overlay stays in G4.

Five batches, one build each, each checked live in `native` with screenshots beside the page's:

1. **Kernel and host foundations.** `desk_apply` (named operations in one transaction, `baseRev` conflicts, first-run seeding from the generated module table, the page's migrations) and the whole blob in `desk-layout-changed {rev, instance, config}`; the native switch refused without the WebSocket door and rolled back when the host does not start; five quick crashes send the desk back to the page; `PointerOutside` from the kernel's mouse path instead of a full-canvas catcher; Escape forwarded with the host's focus intact. In the host: requests answered once (reply, timeout, or lost when the link drops), the layout with `hidden`/`nested`, the `hit:` key prefix for the input region, the inspector's tree rebuilt on commit only, `motion.rs` with the page's tokens, and the clock's dashboard as a clip morph on `expoOut` / `surfaceExit`.
2. **Controls and the menu's shell and "Вид" pane**, with a draft theme kept apart from the stored one and committed through `desk_apply`, toasts and the debug line.
3. **"Стол" pane**: modules, folders, the Start picker, the VPN pane.
4. **Arrange mode**, drafts held in the host until Done (one `desk_apply` with `baseRev`), mouse capture during drags, display changes.
5. **Folder widgets, the nesting zone with the real VPN panel, every widget's expansion, the wheel.**

Gate: every setting the page offers works in `native`; the menu's screenshots in both themes match the page's; host memory at or under 80 MB after every batch, the current 90.8 MB measured apart first.

### G2 so far (session 143): the widget contract

Batches 2-4 were written in session 142 (`992883d`). A parity audit of the kernel's interface, the menu and the clock against the page (three surveys, every finding checked in the code) found two kinds of gaps: differences of detail (listed in the session log), and a structure that could not grow. The host hard-coded its widgets: `Host` held the clock's time, the VPN panel's state and one morph; the desk model knew a `Kind::Folder` and a `Kind::Vpn`; clock instances were not drawn; every new widget meant editing the shell in five places. Five critics reviewed the contract that replaces it (extensibility, Rune's constraints, memory and cold start, crates and rules, motion parity).

- **The contract** ([shell.md](../reference/shell.md)): `core/desktop` is the library `unidtk-shell`; `unidtk-desktop` (`main.rs`) registers the modules' widget types. A module draws on the native desk through a third half, `modules/<id>/native/` (`unidtk-native-clock`, `unidtk-native-vpn`), which implements `Widget` and reads through `View`, writes through `Act`. The shell keeps widgets by desk id and generation, so an answer for a widget removed meanwhile is dropped. Chrome and strip radius are metadata (`"chrome": "bare"`, `"radius": "strip"`), checked by `build.rs`, which also checks that every native half is linked, registered, and depends on no other module.
- **One morph** (`core/desktop/surface.rs`): the shell drives the clip frame by frame with the page's curves (open on `expoOut`; close through the 0.8 keyframe on `surfaceExit` over `0.65 x T x sqrt(r)`; reversal from where the clip is; the wash's own timing). The content is not faded, as on the page; the header icon flies; a widget flies its own parts with `View::fly` (the clock's face).
- **Rune learned** `hover_fill`, `hover_border` and `hover_lift` (CSS `:hover` on the fill, the border and a translate, on the hover track), `TextEdit::max_chars` (`maxlength`), `TextStyle::lh_normal` (`line-height: normal`), `Transition::curves` (an element appears and leaves on a CSS duration and easing, not on a spring) and `WindowHandle::gpu_memory` (the allocator's report, in `udtk host frames`). The flat material's blur is the host's: it reads the wallpaper, lays it out as Windows does and blurs a quarter-size copy once ([shell.backdrop](../reference/shell.md#r-shell.backdrop)).
- **A second audit round** (the three audits checked again against the code): toasts, the arrange island, resize handles, the "+ Виджет" popover and the clock's face switcher appear and leave on the page's tokens (`Motion::presence`); the popover is centred on the island; the menu's sheet grows out of its "Настроить…" row through the one morph; a button whose command runs shows disabled; the focus ring is the page's (2 px accent, 2 px offset) and focus moves as the page moves it; a header card moves in arrange mode only by its header; the settle after arrange mode takes every press on the desk; the calendar names the month in Windows' language; the size of an added instance is the module's `instanceSize`; fields, accents and folder icons have their accessible names.
- **Cards no longer jump** (the user's report from the running build): the drafts arrange mode leaves are kept until the layout that stores them arrives, a layout that moves cards lands at once as the page's `left`/`top` do, and the last frame of a close morph is `instant`.
- **Differences that remain, named:** the Explorer overlay (G4); the wheel reaches a widget's tile only (the Glyph Bar since session 145; the player's volume and the Explorer's scrolling wait for their ports); the debug line has no global error hook; a folder or shortcut name is stored on Enter only, not on blur (Rune's fields have no blur handler yet); the colour wells open the system dialog with no live preview; the VPN port field takes any text (a non-number is stored as 443, as `saveHome` does); a module pill shows no busy state; the calendar and the minute faces are live where the page renders once and ticks every second (kept: closer to the time).

#### Structure: the page and the native desk

| Concern | The page (`core/ui`, WebView2) | The native desk (`core/desktop`, Rune) |
| :--- | :--- | :--- |
| A module's interface | `modules/<id>/ui/widget.js`, imported at run time; render functions found by name (`specialType`) | `modules/<id>/native/`, a crate linked into `unidtk-desktop`, one `WidgetType` per module, checked at build time |
| What the shell knows of a module | `metadata.json`, plus special cases by name in `unidtk-core.js` (`specialType === "clock"`, `"vpn"`, `"mp3-player"`) | `metadata.json` only; no module is named in the shell |
| State of a widget | DOM nodes and module globals (`vpnServersCache`, `window.clockInterval`) | the widget's own struct, one per desk id, made and dropped with the layout |
| Calls to the kernel | `invoke()` anywhere; a late answer for a removed card is guarded by hand (`isConnected`) | `Act::call`, answered on the widget only if it is the same one (`r[shell.widget-identity]`) |
| Timers | `setInterval` per widget (the clock every second for every face) | one wake for the desk, the earliest each widget asks for (`r[shell.wake-schedule]`) |
| The morph | `surface-manager.js` with Web Animations on `clip-path`; flights by cloned DOM stand-ins | `surface.rs` driving the clip per frame; flights by `View::fly` with the real element laid out again, not a stretched copy |
| Appearing and leaving | CSS `transition` and keyframe `animation` on the motion tokens, per selector | `Motion::presence`: Rune's presence with the token's duration and the CSS easing (`Transition::curves`), one helper for every element |
| A card moving to a new place | `left`/`top` set by the render, no transition | a layout from the kernel lands in an `instant` commit; drafts held until it arrives, so nothing moves twice |
| Styling | CSS custom properties, cascades, `:hover` | the theme record read into tokens (`theme.rs`), hover on Rune's hover track |
| The flat blur | `backdrop-filter`, which sees only what the page drew | the wallpaper itself, blurred once |
| Adding a widget | a folder under `modules/`, a `specialType` branch in the core, CSS linked by hand in `index.html` | a `native/` crate, one dependency line, one line in `main.rs`; the shell does not change |

## G4: the music player (session 152)

The port is built behind the application's `mp3-native` feature and joins the desk whole (card, library, settings, graph); the gate goes with it. Done: the backend's `player-state` state event (no 2 s poll), the crate `modules/mp3-player/native` with its event list and `logic.rs` (the page's helpers and constants, tested). Found for the shell before the card: the page's popup (320 x 330, centred over the 210 x 220 card, blurred) grows from the pressed zone (`.zone-top` or `.zone-bottom`, radius 75 px) over the card, which stays drawn under it; the native "one morph" turns the card itself into its surface, so a morph from the zone would hide the card's face outside the zone at the first frame. The contract needs a surface drawn over its unmoved tile, an open from a zone rect with its radius (the morph's origin already takes a rect and a radius), and a resize of the open surface on `--dur-layout` (the graph's 380 x 536). The zones lie under an open popup, so there is no switch between the library and the settings while one is open. Still to measure before the arcs: a sustained 30 Hz redraw of the desk (CPU, frame ms, presents per second), to decide whether the arcs need a small surface of their own.

## Measurements

Every gate records, per process (kernel, WebView2 tree, host), after a one-minute warm-up and over a five-minute idle window:

| Metric | Method | Threshold for the host |
| :--- | :--- | :--- |
| Working Set and private bytes | `Get-Process` `WorkingSet64`, `PrivateMemorySize64` | G0: ≤ 80 MB; G4: recorded against the 571 MB WebView2 tree (session 136) |
| Idle CPU | process CPU time over the window | ≤ 0.2 % |
| Wake-ups per second at idle | the host's own counter through the inspector door | 0 without animation, apart from the 1 Hz clock |
| Frame time while a surface morphs | the host's own frame clock | ≤ 8.3 ms at 120 Hz, ≤ 16.6 ms at 60 Hz |
| `unidtk-desktop.exe` size, `udtk build` time, peak commit during the build | file size, wall clock, Resource Monitor | recorded before and after each step |

## Verification of every step

`udtk build`, `udtk doctor`, `udtk doctor -ping` (RTT), `udtk logs --level ERROR`, `udtk shot`, `cargo test --release --workspace -j 1`, `udtk doc --check`. One commit per step, with the test count in its body.


### G0 supervision and remaining interactive checks (sessions 139–140)

Slices 1–4 (`6c4af20`, `46ca3be`, `fcd9058`, `34d3bdb`) completed the architecture pieces inside `unidtk` and `unidtk-desktop`:

- **Child occlusion & readback (`6c4af20`):** `core/rune/rune/src/platform/win32/occlusion.rs` handles reparented `WS_CHILD` hosts; `core/desktop` adds offscreen true-alpha `wgpu` PNG readback and key-injection routing (`InjectKey` / `FocusLost`).
- **Zero-wake WS & kernel supervisor (`46ca3be`, `fcd9058`, `34d3bdb`):** `core/kernel/ws.rs` sleeps with `WSA_INFINITE` (`EventWaiter`), `core/kernel/host_supervisor.rs` owns `unidtk-desktop.exe --supervised` inside a `KILL_ON_JOB_CLOSE` Job Object with stdin WS handshake credentials, `core/kernel/hotkeys.rs` and `core/kernel/input.rs` forward keys/wheel and treat host PIDs as shell windows without double-posting or stealing focus, and `core/kernel/main.rs` prevents implicit `RunEvent::ExitRequested` (`code: None`) from stopping the kernel when destroying `"wallpaper"` on `Web -> Native` transitions.
- **Unified IPC, CLI & docs (`fcd9058`, `34d3bdb`):** kernel `ping` publishes `"host": { mode, pid, generation, inspectorPort, attached, ready }`, read-only proxy commands (`host_ping`, `host_tree`, `host_frames`, `host_hit`) are in `core/kernel/command_inventory.rs` (19 kernel / 158 total commands), `udtk` auto-routes `--host`/`native` diagnostics (`doctor -ping`, `shot`, `stop`), and `docs/reference/ipc.md`, `install.md`, and `docs/udtk/` are updated.
- **Measured on the installed build (`34d3bdb`, `udtk build -j 1` in 32.46 min):** `unidtk-desktop.exe` `31,094,272` bytes, `unidtk.exe` `53,481,472` bytes, `udtk.exe` `35,233,280` bytes; 377/377 workspace tests green (`1` doctest ignored), 0 warnings outside `core/rune`; during a live `Web -> Native -> Web` round-trip in the agent sandbox (`3s` native window), supervised `unidtk-desktop.exe` reported `65.75 MB` Working Set (`68.52 MB` private), `6.22 ms` `cpu_frame_ms`, `0` Rune `timer_wakes`, `ping` 1–2 ms, and clean transition back to `web` (`persisted: "web"`, `0` new ERROR logs).

**G0 was reopened and is closed again (session 141).** The interactive-desktop checks ran on `WinSta0\Default` against the installed build, which contains slice 5 (`f2de4ea`, 382/382 tests): the host is attached under Progman (`attached: true`, `ready: true`), `udtk host tree` returns the `g0-card`, `udtk shot --host` returns a 1920x1080 `wgpu-readback` PNG of the half-transparent card, `udtk doctor --host --simulate` passes every check, and a 5-minute idle window gave `0.005 %` of one core, `0` Rune `timer_wakes`, `67.9 MB` Working Set (`68.7 MB` private) against the thresholds below (≤ 0.2 %, 0, ≤ 80 MB). UniDTK owns no WebView2 process in `native` mode. Production default remains `web`; the cold start of the `UniDTK` task in `native` mode is shown at G1's first run.

**Why it is reopened.** The user's screenshot showed a title-bar strip ("UniDTK desktop", an icon, a light-blue band) over the top of the host window. `udtk shot --host` reads the frame from `wgpu`, and `doctor --host --simulate` checks the door, so neither could see it; `PrintWindow` of the desktop host (as DWM composes it) and of the host window itself both show it. Closing G0 needed a composed-desktop picture without the strip, taken after the fix (`no_redirection_bitmap`, [UPSTREAM.md](../../core/rune/UPSTREAM.md) item 9). **It is taken (session 141):** `PrintWindow` of the shell window after the rebuild shows the card and no caption band, so G0 is closed again; the 393 tests are green.
