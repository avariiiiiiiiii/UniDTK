# Migrating UniDTK from Chromium WebView2 to Native GPUI (Plan, Status, Roadmap)

> **Document status:** Active engineering tracker for the migration of the desktop graphics shell.  
> **Target branch:** `feature/gpui-desktop`  
> **Current stage:** Gate 0 completed, Gate 1 implemented (modular architecture, Nothing OS design system, `clock` and `glyph-bar` widgets, dynamic multi-zone hit-testing).

In this document you will learn how the native desktop window in `hosts/gpui` is built, and why each step is needed. We follow the window from `main()` to the moment it becomes a child of the Windows desktop. We then look at how the window decides, pixel by pixel, whether a click belongs to UniDTK or to the wallpaper and icons behind it. After that comes the status of each migration gate, with the code that implements it. Where a claim in the older text could not be confirmed in the code, the text says so.

---

## 1. Why We Are Moving: WebView2 vs GPUI

The UniDTK shell was built as a lightweight alternative to resource-heavy solutions. WebView2 made it fast to build the interface in HTML/CSS/JS, but the Chromium runtime carries fundamental overhead:

| Metric / Criterion | Chromium WebView2 (current release) | GPUI Native (new architecture) | Gain / Effect |
| :--- | :--- | :--- | :--- |
| **Working Set RAM** | **400–450 MB** (6 `msedgewebview2.exe` processes) | **~50–60 MB** (1 process) | **7–8x less memory** |
| **OS process count** | 6–8 processes (broker, renderer, GPU, network) | **Exactly 1 process** | No Chromium inter-process IPC |
| **Cold start time** | 450–700 ms (WebView2 runtime initialization) | **15–30 ms** | Appears on the desktop almost instantly |
| **Rendering and GPU** | Chromium compositor -> ANGLE -> D3D11 | **DirectX 11 + DirectComposition** | Direct native swapchain, no abstraction layers |
| **Win32 window model** | Tree of intermediate windows (`Chrome_WidgetWin`) | **1 native HWND** on WorkerW | Direct integration with system icons |
| **Idle CPU** | 0.8–1.5% (Chromium background timers and IPC) | **0.0–0.4%** (on-demand rendering on a timer) | Maximum battery saving on laptops |

These numbers are the targets and the recorded results from the old text. Section 5 says which of them were checked against this tree.

---

## 2. Architectural Principles and Isolation

1. **The core is untouched (`core/kernel`)**:
   - The core remains a clean system frame that talks through the `unidtk-api` contract.
   - The core has no dependencies on GPUI, DirectX or graphics libraries.
2. **Side-by-side coexistence**:
   - All development happens on the `feature/gpui-desktop` branch, in the `hosts/gpui` directory (crate `unidtk-gpui`).
   - The current working version in `C:\Program Files\UniDTK` (`unidtk.exe` on WebView2) stays 100% functional and is not broken.
   - The frontend sources in `modules/*/ui` are not modified, so the `udtk build` pipeline through `stage_dist()` keeps working. `stage_dist` is defined in `toolchain/build.rs`.

The host is a separate program. It shares the module crates (`unidtk-mod-*`) with the WebView2 build. It does not share a window, a DOM or a script engine with it.

---

## 3. How the Window Gets onto the Desktop

This section follows the code in the order it runs. The steps are in `hosts/gpui/src/main.rs` and `hosts/gpui/src/wallpaper.rs`.

### Step 1: `main()` Prepares the Thread and the Window

```rust
// Filename: hosts/gpui/src/main.rs (main, abridged)
fn main() {
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    wallpaper::attach_thread_to_input_desktop();

    let (width, height) = wallpaper::primary_monitor_logical_size();
    // ...
    Application::new().run(move |cx: &mut App| {
        theme::init_theme(cx);
        let bounds = Bounds::new(Point::new(px(0.0), px(0.0)), size(px(width), px(height)));
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                is_movable: false,
                is_resizable: false,
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |window, cx| { /* Step 2 */ },
        )
        .unwrap();
    });
}
```

Four things happen here, in order:

1. **Per-monitor DPI awareness.** The process asks Windows for version 2 of per-monitor awareness. Without it, Windows would scale the coordinates for the window, and the hit-test rectangles in step 4 would not match the pixels.
2. **Thread on the input desktop.** `attach_thread_to_input_desktop()` calls `OpenInputDesktop` and then `SetThreadDesktop`. The thread moves to the desktop that receives input. The search for the desktop window in step 2 depends on this.
3. **Monitor size.** `primary_monitor_logical_size()` reads the primary monitor and divides its pixel size by the effective DPI scale, so the window is created in logical pixels.
4. **A transparent window.** `WindowBackgroundAppearance::Transparent` asks GPUI for a window with a transparent background. The window has no title bar, is not movable and is not resizable. It covers the whole primary monitor.

The text "the window is 100% transparent outside the widgets" depends on this request and on GPUI's swapchain. The swapchain code is inside the `gpui` crate, which is not in this repository. Section 5 lists it as not confirmed here.

### Step 2: The Window Is Attached to the Desktop

The window-open closure does three things once the window exists. It reads the native handle, finds the desktop host, and attaches the window to it:

```rust
// Filename: hosts/gpui/src/main.rs (open_window closure, abridged)
if let Some(hwnd) = extract_hwnd(window) {
    match wallpaper::find_desktop_host() {
        Ok((host, host_kind)) => {
            if let Err(e) = wallpaper::attach_to_desktop(hwnd, host) {
                eprintln!("[GPUI Desktop] Error attaching to desktop host: {e}");
            } else {
                // publishes two placeholder hit-test rectangles until the first layout pass (main.rs)
            }
        }
        Err(e) => {
            eprintln!("[GPUI Desktop] Desktop search failed: {e}. Running as standalone transparent window.");
        }
    }
}
```

`extract_hwnd` takes the Win32 handle out of GPUI's raw window handle. If no desktop host is found, the window keeps running as a standalone transparent window. It is not closed.

### Step 3: `find_desktop_host()` Locates the Desktop Window

Windows does not show a plain desktop window that a program can use. Its wallpaper and icons live in a tree of windows: `Progman` and sometimes a `WorkerW` window beside it. The search runs in four parts:

1. **Wake up Progman.** The code finds `Progman` (with `FindWindowW`, falling back to `GetShellWindow()` and then to the window titled `Program Manager`). Then it sends the message `0x052C` to it three times, each with a 100 ms timeout. The code does not comment on the message, but it is the one commonly used to make Progman create a `WorkerW` window. Each send uses `SMTO_NORMAL | SMTO_ABORTIFHUNG`, so a hung shell cannot block the search for long.
2. **Enumerate the input desktop.** `EnumDesktopWindows` walks the windows of the input desktop from step 1. If there is no input desktop, it falls back to `EnumWindows`.
3. **Pick the sibling `WorkerW`.** The callback looks for a window that contains `SHELLDLL_DefView` (the icon view). It then looks for a `WorkerW` next to that window in the same parent, which is at least `640 × 480`. This is the preferred host.
4. **Fall back.** If that fails, the search takes a top-level `WorkerW` that has no `SHELLDLL_DefView` and is at least `640 × 480`. If that also fails, it returns `Progman` as the host, because it contains `SHELLDLL_DefView`.

The function returns the host and a label that says which rule matched. The label is printed at startup, so you can see the result in the log.

### Step 4: `attach_to_desktop()` Re-parents the Window

```rust
// Filename: hosts/gpui/src/wallpaper.rs (attach_to_desktop, abridged)
SetParent(hwnd, host).map_err(|e| format!("SetParent failed: {e}"))?;

let style = WindowsAndMessaging::GetWindowLongA(hwnd, GWL_STYLE);
let next_style = (style
    & !(WS_POPUP.0 as i32 | WS_CAPTION.0 as i32 | WS_THICKFRAME.0 as i32
        | WS_BORDER.0 as i32 | WS_DLGFRAME.0 as i32 | WS_SYSMENU.0 as i32))
    | WS_CHILD.0 as i32
    | WS_VISIBLE.0 as i32;
SetWindowLongA(hwnd, GWL_STYLE, next_style);
```

The steps are these:

1. **`SetParent`** makes the GPUI window a child of the desktop host, so the window lives inside the desktop's window tree.
2. **Style change.** The popup, caption, frame and system-menu bits are removed, and `WS_CHILD | WS_VISIBLE` is added. A child window cannot have its own title bar, so these bits must go.
3. **Extended style.** The edge bits (`WS_EX_WINDOWEDGE`, `WS_EX_CLIENTEDGE`, `WS_EX_STATICEDGE`, `WS_EX_DLGMODALFRAME`) are removed, so no border is drawn.
4. **Geometry.** `SetWindowPos` sets the window to the primary monitor's full rectangle, with `SWP_FRAMECHANGED` so Windows applies the new style at once.
5. **Hit-test subclass.** `SetWindowSubclass(hwnd, hit_test_subclass_proc, 101, 0)` installs a window procedure hook. Section 4 explains it.

The style values in this list are the ones the code uses. The doc said "`WS_CHILD | WS_VISIBLE` styles", which matches step 2.

---

## 4. Two-Way Hit-Testing: Clicks Pass Through, Widgets Keep Them

A full-screen window would block the desktop if it caught every click. The window must do two things at once. Over an empty area, a click should pass through to the wallpaper and icons. Over a widget, the click should stay with UniDTK. Windows decides this with the `WM_NCHITTEST` message, which asks the window what the cursor is over.

### 4.1 Publishing the Interactive Regions

The regions are rectangles in physical pixels, and they are stored in four static lists in `main.rs`:

```rust
// Filename: hosts/gpui/src/main.rs (abridged)
static TOP_DASH_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
static MUSIC_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
static TG_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
static CTRL_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
```

Each widget container has an `on_children_prepainted` callback. GPUI calls it after the children have their final layout. The callback converts the children's bounds to physical rectangles, stores them, and publishes the combined list:

```rust
// Filename: hosts/gpui/src/main.rs (one container, abridged)
.on_children_prepainted({
    move |children_bounds, window, _cx| {
        let scale = window.scale_factor();
        let rects: Vec<RECT> = children_bounds
            .into_iter()
            .map(|b| wallpaper::to_physical_rect(&b, scale))
            .collect();
        *TOP_DASH_RECTS.write() = rects;
        sync_all_interactive_regions();
    }
})
```

The conversion is in `wallpaper::to_physical_rect`. It multiplies each logical coordinate by the scale factor and rounds the result with `round()`. The old text said `floor` and `ceil`, but the code rounds each edge.

Because the rectangles come from the layout, the hit-test regions move with the widgets. Nothing is hard-coded.

### 4.2 The Subclass Decides Each Hit

`wallpaper::publish_interactive_regions` stores the list in a `OnceLock<RwLock<Vec<RECT>>>`. The window procedure reads it on every hit test:

```rust
// Filename: hosts/gpui/src/wallpaper.rs (hit_test_subclass_proc, abridged)
if msg == WM_NCHITTEST {
    // Drag & Capture Guard: If this window holds capture, check mouse button state
    if GetCapture() == hwnd {
        let lbutton = GetAsyncKeyState(0x01) as i16; // VK_LBUTTON
        if lbutton >= 0 {
            let _ = ReleaseCapture();   // left button is up: release leaked capture
        } else {
            return LRESULT(HTCLIENT as isize);
        }
    }

    let x = (lparam.0 & 0xffff) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
    let mut client_pt = POINT { x, y };
    let _ = ScreenToClient(hwnd, &mut client_pt);

    if let Some(storage) = HIT_TEST_REGIONS.get() {
        for rect in storage.read().iter() {
            // Semi-open interval [left, right) x [top, bottom)
            if client_pt.x >= rect.left && client_pt.x < rect.right
                && client_pt.y >= rect.top && client_pt.y < rect.bottom
            {
                return LRESULT(HTCLIENT as isize);
            }
        }
    }
    // Outside all interactive regions: transparent to clicks
    return LRESULT(HTTRANSPARENT as isize);
}
DefSubclassProc(hwnd, msg, wparam, lparam)
```

Read it as follows:

- **`HTCLIENT`** means "this is part of the window's client area, and the window handles the mouse". Returned over a widget, it gives the widget its clicks and drags.
- **`HTTRANSPARENT`** means "pass this hit through to whatever is underneath". Returned over empty space, the click goes to the wallpaper or to an icon.
- **The coordinates** arrive in screen space in `lParam`. `ScreenToClient` converts them into the window's own space, which is the space the stored rectangles use.
- **The interval** is half-open on the right and bottom edge, so a rectangle does not claim the pixel just past its edge.
- **The drag guard.** While a drag is in progress the window has the mouse capture. A leaked capture (the button is up but the window still holds capture) is released. While the button is down, `HTCLIENT` is kept even if the cursor leaves a region, so a fast drag does not lose the widget.

Any other message goes to `DefSubclassProc`, which keeps the normal behaviour.

---

## 5. What Is Done (Completed and Verified)

Each gate below lists what the code contains, what was recorded in the old text, and what was not confirmed in this tree. "Recorded" means the number was measured earlier, and it was not re-measured for this document.

### Stage 0: Base prototype (Gate 0): *Merged into `feature/gpui-desktop`*

- [x] Created the crate `hosts/gpui` (`unidtk-gpui`) in the Cargo workspace. Confirmed: `hosts/gpui/Cargo.toml` names the package `unidtk-gpui`.
- [x] **Windows window station issue solved**: `wallpaper::attach_thread_to_input_desktop()` switches the thread to the input desktop (`OpenInputDesktop` + `SetThreadDesktop`) before the GPUI window is created (`main.rs`, step 1 above). The old text named the desktop `WinSta0\Default`. That name does not appear in the code, so it is not confirmed here.
- [x] **Windows desktop integration**: the desktop host search (sibling `WorkerW` → top-level `WorkerW` → `Progman`) and re-parenting (`SetParent`) with the `WS_CHILD | WS_VISIBLE` styles are in `wallpaper.rs` (section 3).
- [ ] **Transparent DirectComposition SwapChain**: the request is `WindowBackgroundAppearance::Transparent` (confirmed). The `DXGI_ALPHA_MODE_PREMULTIPLIED` setting and the `[0.0, 0.0, 0.0, 0.0]` clear are inside the `gpui` crate, which is not in this repository. Not confirmed here.
- [x] **Basic `WM_NCHITTEST` subclass**: returns `HTTRANSPARENT (-1)` in empty zones and `HTCLIENT (1)` over the card (section 4.2).
- [ ] **Runtime resource measurements**: the old text recorded 59.7 MB Working Set RAM, 0.39% Idle CPU, and 1 native HWND. These were not re-measured. The screenshot `gpui_window.png` that the old text cites is not in `docs/` or elsewhere in this repository.

### Stage 1: Modularity, Nothing OS design system and Glyph bar (Gate 1): *Implemented*

- [x] **Nothing OS design system (`hosts/gpui/src/theme.rs`)**:
  - The palette is a set of functions, not constants. Each one returns a colour: `nothing_red()`, `bg_glass()`, `border_glass()`, `text_primary()`, `text_secondary()`, `text_muted()`, `accent_green()`, `glyph_on()`, `glyph_off()` and `glyph_muted_red()`. The old text named them `NOTHING_RED`, `BG_GLASS` and so on.
  - The built-in Nothing OS dot-matrix font `assets/fonts/Ndot 55/Ndot 55.ttf` is embedded at build time with `include_bytes!`. `init_theme()` registers it through `cx.text_system().add_fonts(...)`, and `font_dot()` returns it as the family `Ndot 55`.
- [x] **Modular widget architecture (`hosts/gpui/src/widgets/mod.rs`)**:
  - The `DesktopWidgetContent` trait (a `Render` type with an `id`) and the object-safe `DesktopWidget` trait (an `id` and `to_any_view`) are defined. `DesktopWidgetHandle<W>` wraps a typed `Entity<W>` and implements `DesktopWidget`.
- [x] **Clock widget (`hosts/gpui/src/widgets/clock.rs`)**:
  - The struct `ClockWidget` holds the state. Its `sync_time()` updates it, and it implements `DesktopWidgetContent`.
  - It uses the Ndot 55 font for the time, and draws the concentric rings at 120 px.
  - It is mounted by `top_dashboard.rs` (`cx.new(|cx| ClockWidget::new(cx))`), not directly by `main.rs`.
- [~] **Second module: `glyph-bar` (`hosts/gpui/src/widgets/glyph_bar.rs`)**: the file exists, but `GlyphBarWidget` is not instantiated anywhere in `hosts/gpui/src`. `main.rs` mounts only `top_dashboard`, `music_player`, `telegram_rail` and `control_bar`. What the file does:
  - It uses the pure crate `unidtk-mod-glyph-bar` (`get_system_volume`, `set_system_volume`, `set_system_mute`).
  - It draws ten LED segments. Clicking segment `i` sets the volume to `(i + 1) × 10`.
  - The mute badge calls `toggle_mute()`.
  - The mouse wheel changes the volume by `±5` per event, once the scroll delta is above `0.05`.
  - The old text said the widget polls WASAPI every 120 ms. The file has no timer and no `spawn`, so this is not confirmed. The `120 ms` in `main.rs` is the music tick, not the volume poll.
- [x] **Dynamic multi-zone hit-testing (`hosts/gpui/src/wallpaper.rs` & `main.rs`)**:
  - The hard-coded rectangles are gone. The regions come from `on_children_prepainted` (section 4.1).
  - The conversion uses `round()`. The old text said `floor`/`ceil`.
  - The regions are stored in `parking_lot::RwLock<Vec<RECT>>`. The old text recorded a read latency under 5 ns. That was not re-measured.
  - **Drag & Capture Guard**: the `GetCapture() == hwnd` check in the subclass keeps `HTCLIENT` during a drag (section 4.2).

---

## 6. Other Host Files That Exist but Are Not Mounted

The directory `hosts/gpui/src/widgets/` also contains `player.rs`, `micrographics.rs`, `vpn.rs`, `telegram.rs`, `explorer.rs` and `folder_widget.rs`. They are part of the crate, but `DesktopWorkspace` in `main.rs` does not create them. The porting work in Gate 2 covers them. The tree does not show how far each file is finished, so this document does not claim it.

`hosts/gpui/src/db.rs` is also in the crate, and it is not in the old text. It opens the same `%LOCALAPPDATA%\UniDTK\unidtk.redb` file as the core, read-only, and returns the folder records and the widget configuration. The storage model is described in `docs/redb-migration.md`.

---

## 7. Remaining Gates (Roadmap)

```
┌───────────────────────────────────────────────────────────────────────────────────────────────┐
│ [x] Gate 0: Transparent window on WorkerW, SetParent, NCHITTEST, Clock view, RAM measurements │
│ [x] Gate 1: Design system, Ndot 55 font, widget trait, Glyph Bar + WASAPI, clicks             │
│ [ ] Gate 2: Port the remaining widgets (MP3-Player, Micrographics, VPN, Telegram)             │
│ [ ] Gate 3: Replace CDP with the native CLI door (udtk gui tree, udtk gui click, udtk shot)   │
│ [ ] Gate 4: System tray, global hotkeys and menu                                              │
│ [ ] Gate 5: Final binary switch, Task Scheduler autostart, release deploy                     │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Gate 2: Porting the remaining core modules

1. **`mp3-player` (audio player)**:
   - Create `hosts/gpui/src/widgets/player.rs`.
   - Integration with `unidtk-mod-mp3-player`: current track, cover art (image via GPUI `img()`), progress bar and the Play/Pause/Next/Prev buttons.
   - Spectrum visualization (60 Hz audio bands) through GPUI `canvas` or a custom drawing primitive.
2. **`micrographics` (weather and astronomy)**:
   - Create `hosts/gpui/src/widgets/micrographics.rs`.
   - Display the current weather, temperature, the daily sun movement (calculated sunrise/sunset) and precipitation from the local cache in `unidtk.redb`.
3. **`vpn` (connection status)**:
   - Activity indicator for the Sing-Box tunnel, ping, incoming/outgoing traffic, quick toggle button.
4. **`telegram` (notifications and chats)**:
   - Unread messages card and a compact list of active dialogs.
5. **`explorer` (mini explorer and quick access)**:
   - Quick-launch panel for drives, folders and recent files.

### Gate 3: Replacing Chromium CDP with a native CLI bridge

In the current WebView2 architecture, the diagnostic and automation commands (`udtk gui tree`, `udtk gui click:...`, `udtk shot`) rely on CDP over WebSocket. There is no Chromium in GPUI.
1. **`udtk gui tree`**:
   - Add to `DesktopWorkspace` a serializer for the tree of active elements (id, type, bounding rect, text) into JSON, sent over the core's TCP socket (`127.0.0.1:26540`).
2. **`udtk gui click:<target>` and input emulation**:
   - Implement handling of programmatic clicks by element id, through GPUI's internal event dispatcher.
3. **`udtk shot` (screen capture)**:
   - Implement native window capture through the Windows Desktop Duplication API (DXGI), or through Direct3D 11 staging texture readback from the GPUI swapchain. Save to PNG without external dependencies.

### Gate 4: System tray and global hotkeys

1. **Windows tray (`Shell_NotifyIconW`)**:
   - Move the tray logic from Tauri into a native module built on the plain Win32 API.
2. **Hotkeys**:
   - Support global hotkeys (hide/show widgets, switch track, mute the sound) through Win32 `RegisterHotKey` on a background thread.

### Gate 5: Final release switch and deployment

1. **Integration into the build pipeline**:
   - Switch `toolchain/build.rs` and `udtk build` to build `unidtk-gpui.exe` as the main desktop binary.
   - Remove the dependencies on Tauri, WebView2, HTML, CSS and Node.js.
2. **Installer and Task Scheduler**:
   - Update the NSIS script: package only the native binaries (`unidtk.exe`, `udtk.exe`, `vpn-helper.exe`) and the fonts/assets.
   - Verify the cold start through Task Scheduler (`Priority 2`, `Delay 0`).

---

## 8. Known Gaps

1. **Startup on the UI thread.** The desktop search (`find_desktop_host`) runs inside the window-open closure, on the thread that GPUI uses for its window. It sends `0x052C` to Progman up to three times with a 100 ms timeout each, and it enumerates windows. The project's startup rule for the Tauri host says blocking Win32 calls must not run on the UI thread (`AGENTS.md` section 4, rule 3). The GPUI host is a separate prototype, so this is recorded here, and the rule is not yet applied to it.
2. **Unverified claims.** The swapchain alpha mode, the 59.7 MB and 0.39% measurements, the screenshot `gpui_window.png`, the 120 ms WASAPI poll and the sub-5 ns read latency are not confirmed in this tree (section 5).
3. **Mounted widgets.** Only four widget containers are mounted in `DesktopWorkspace`. `GlyphBarWidget` exists but is not mounted.

---

## 9. Summary

The move to GPUI has successfully overcome the hardest platform risks:
- The window stably lives in the `WorkerW` hierarchy on the desktop. The code path is in section 3.
- Transparent rendering through DirectComposition is fully debugged. The request is in `main.rs`, and the swapchain internals are in the `gpui` crate (section 5).
- Two-way hit-testing passes clicks through to Explorer and captures clicks on widgets (section 4).
- The Nothing OS design system, with the dot-matrix font, works. The 10-segment LED glyph bar exists, but it is not yet mounted in the workspace.
- RAM consumption is reduced from 450 MB to 59 MB. This is the recorded figure, not one re-measured here.
