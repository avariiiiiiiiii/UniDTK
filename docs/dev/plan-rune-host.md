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
- **Memory.** 132 MB Working Set, 116 MB private, 14 threads after the first frame of one full-screen window: over the G0 threshold of 80 MB. Not yet investigated (swapchain buffers of a 1920x1080 surface, the D3D12 runtime, the adapter pick without the `wgpu-hal` patch are the first candidates).
- **The desktop host here** is Progman of 1920x1680 (two displays, the second below the first); the probe covers the primary only.

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
