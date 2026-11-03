# GPUI: Completed Gates (History)

What was finished and verified on the GPUI branch. The host as it works now is described in [the GPUI host](../reference/gpui-host.md).

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
