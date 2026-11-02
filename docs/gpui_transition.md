# Migrating UniDTK from Chromium WebView2 to Native GPUI (Plan, Status, Roadmap)

> **Document status:** Active engineering tracker for the migration of the desktop graphics shell.  
> **Target branch:** `feature/gpui-desktop`  
> **Current stage:** Gate 0 completed, Gate 1 implemented (modular architecture, Nothing OS design system, `clock` and `glyph-bar` widgets, dynamic multi-zone hit-testing).

---

## 1. Why we are moving: WebView2 vs GPUI

The UniDTK shell was built as a lightweight alternative to resource-heavy solutions. WebView2 made it fast to build the interface in HTML/CSS/JS, but the Chromium runtime carries fundamental overhead:

| Metric / Criterion | Chromium WebView2 (current release) | GPUI Native (new architecture) | Gain / Effect |
| :--- | :--- | :--- | :--- |
| **Working Set RAM** | **400–450 MB** (6 `msedgewebview2.exe` processes) | **~50–60 MB** (1 process) | **7–8x less memory** |
| **OS process count** | 6–8 processes (broker, renderer, GPU, network) | **Exactly 1 process** | No Chromium inter-process IPC |
| **Cold start time** | 450–700 ms (WebView2 runtime initialization) | **15–30 ms** | Appears on the desktop almost instantly |
| **Rendering and GPU** | Chromium compositor -> ANGLE -> D3D11 | **DirectX 11 + DirectComposition** | Direct native swapchain, no abstraction layers |
| **Win32 window model** | Tree of intermediate windows (`Chrome_WidgetWin`) | **1 native HWND** on WorkerW | Direct integration with system icons |
| **Idle CPU** | 0.8–1.5% (Chromium background timers and IPC) | **0.0–0.4%** (on-demand rendering on a timer) | Maximum battery saving on laptops |

---

## 2. Architectural principles and isolation

1. **The core is untouched (`core/kernel`)**:
   - The core remains a clean system frame that talks through the `unidtk-api` contract.
   - The core has no dependencies on GPUI, DirectX or graphics libraries.
2. **Side-by-side coexistence**:
   - All development happens on the `feature/gpui-desktop` branch, in the `hosts/gpui` directory (crate `unidtk-gpui`).
   - The current working version in `C:\Program Files\UniDTK` (`unidtk.exe` on WebView2) stays 100% functional and is not broken.
   - The frontend sources in `modules/*/ui` are not modified, so the `udtk build` pipeline through `stage_dist()` keeps working.

---

## 3. What is done (completed and verified)

### Stage 0: Base prototype (Gate 0) — *Merged into `feature/gpui-desktop`*
- [x] Created the crate `hosts/gpui` (`unidtk-gpui`) in the Cargo workspace.
- [x] **Windows window station issue solved**: `wallpaper::attach_thread_to_input_desktop()` switches the thread to the user's interactive desktop `WinSta0\Default` before the GPUI window is created, guaranteeing that `WorkerW` is found.
- [x] **Windows desktop integration**: implemented the desktop host search algorithm (sibling `WorkerW` -> top-level `WorkerW` -> `Progman`) and re-parenting of the window (`SetParent`) with the `WS_CHILD | WS_VISIBLE` styles.
- [x] **Transparent DirectComposition SwapChain**: configured `DXGI_ALPHA_MODE_PREMULTIPLIED` with a background clear to `[0.0, 0.0, 0.0, 0.0]`. The window is 100% transparent outside the widgets.
- [x] **Basic `WM_NCHITTEST` subclass**: returns `HTTRANSPARENT (-1)` in empty zones (clicks pass through to the wallpaper and system icons) and `HTCLIENT (1)` over the card.
- [x] **Runtime resource measurements**: 59.7 MB Working Set RAM, 0.39% Idle CPU, 1 native HWND. Visual audit confirmed by the screenshot `gpui_window.png`.

### Stage 1: Modularity, Nothing OS design system and Glyph bar (Gate 1) — *Implemented*
- [x] **Nothing OS design system (`hosts/gpui/src/theme.rs`)**:
  - A single palette of color tokens (`NOTHING_RED`, `BG_GLASS`, `BORDER_GLASS`, `TEXT_PRIMARY`, `TEXT_MUTED`, `ACCENT_GREEN`, `GLYPH_ON`, `GLYPH_OFF`).
  - The built-in Nothing OS dot-matrix font `assets/fonts/Ndot 55/Ndot 55.ttf`, registered at runtime through `cx.text_system().add_fonts()`.
- [x] **Modular widget architecture (`hosts/gpui/src/widgets/mod.rs`)**:
  - Declared the `DesktopWidgetContent` trait and the object-safe `DesktopWidget` trait, with the `DesktopWidgetHandle<W>` container.
- [x] **Clock widget (`hosts/gpui/src/widgets/clock.rs`)**:
  - Extracted into the isolated struct `ClockWidget`.
  - Ndot 55 typography for the time and the seconds indicator.
- [x] **Second module — `glyph-bar` (`hosts/gpui/src/widgets/glyph_bar.rs`)**:
  - Connected the pure crate `unidtk-mod-glyph-bar`.
  - A 10-segment LED volume scale in the style of the Nothing OS Phone.
  - Interactive clicks on the LEDs: sets the volume to 10–100%.
  - Interactive click on the status badge: toggles Mute / Active.
  - Mouse wheel control over the widget (`on_scroll_wheel`): smooth adjustment in 5% steps.
  - Background reactive WASAPI polling every 120 ms, for an immediate response to volume changes made from the system.
- [x] **Dynamic multi-zone hit-testing (`hosts/gpui/src/wallpaper.rs` & `main.rs`)**:
  - Fully removed the hardcoded `RECT` rectangles.
  - Integrated with GPUI's `Prepaint` phase through `div().on_children_prepainted(...)`.
  - Correct conversion of sub-pixel logical coordinates to physical ones, taking the screen DPI into account (`scale_factor`, `floor`/`ceil`).
  - Lock-free / low-contention storage `parking_lot::RwLock<Vec<RECT>>` (read latency < 5 ns).
  - **Drag & Capture Guard**: the `GetCapture() == hwnd` check in the `WM_NCHITTEST` subclass unconditionally keeps `HTCLIENT`, preventing loss of focus during fast mouse movement.

---

## 4. What is left to do (roadmap to the full migration)

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

### Detailed description of the remaining tasks

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

## 5. Summary

The move to GPUI has successfully overcome the hardest platform risks:
- The window stably lives in the `WorkerW` hierarchy on the desktop.
- Transparent rendering through DirectComposition is fully debugged.
- Two-way hit-testing passes clicks through to Explorer and captures clicks on widgets.
- The Nothing OS design system, with the dot-matrix font and the 10-segment LED glyph bar, works with WASAPI response.
- RAM consumption is reduced from 450 MB to 59 MB.
