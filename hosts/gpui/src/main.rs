pub mod db;
mod theme;
mod wallpaper;
mod widgets;

use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Entity, IntoElement, Point,
    Render, Timer, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, RECT};

use widgets::{
    control_bar::ControlBarWidget, music_player::MusicPlayerWidget,
    telegram_rail::TelegramRailWidget, top_dashboard::TopDashboardWidget,
};

static TOP_DASH_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
static MUSIC_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
static TG_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());
static CTRL_RECTS: parking_lot::RwLock<Vec<RECT>> = parking_lot::RwLock::new(Vec::new());

fn sync_all_interactive_regions() {
    let mut all = Vec::new();
    all.extend(TOP_DASH_RECTS.read().iter().cloned());
    all.extend(MUSIC_RECTS.read().iter().cloned());
    all.extend(TG_RECTS.read().iter().cloned());
    all.extend(CTRL_RECTS.read().iter().cloned());
    println!("[HitTest] Synced {} interactive regions: {:?}", all.len(), all);
    wallpaper::publish_interactive_regions(all);
}

struct DesktopWorkspace {
    top_dashboard: Entity<TopDashboardWidget>,
    music_player: Entity<MusicPlayerWidget>,
    telegram_rail: Entity<TelegramRailWidget>,
    control_bar: Entity<ControlBarWidget>,
}

impl DesktopWorkspace {
    fn new(cx: &mut Context<Self>) -> Self {
        let top_dashboard = cx.new(|cx| TopDashboardWidget::new(cx));
        let music_player = cx.new(|cx| MusicPlayerWidget::new(cx));
        let telegram_rail = cx.new(|cx| TelegramRailWidget::new(cx));
        let control_bar = cx.new(|cx| ControlBarWidget::new(cx));
        Self {
            top_dashboard,
            music_player,
            telegram_rail,
            control_bar,
        }
    }
}

impl Render for DesktopWorkspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Root desktop canvas: 100% transparent fullscreen surface.
        // Non-widget areas return HTTRANSPARENT via WM_NCHITTEST subclass and pass through to Windows desktop.
        div()
            .relative()
            .size_full()
            // ── TOP CENTER COMPOSITION (Micrographics + Clock) ──
            .child(
                div()
                    .w_full()
                    .pt(px(28.0))
                    .flex()
                    .justify_center()
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
                    .child(self.top_dashboard.clone()),
            )
            // ── CENTER CIRCULAR MUSIC PLAYER ──
            .child(
                div()
                    .w_full()
                    .pt(px(220.0))
                    .flex()
                    .justify_center()
                    .on_children_prepainted({
                        move |children_bounds, window, _cx| {
                            let scale = window.scale_factor();
                            let rects: Vec<RECT> = children_bounds
                                .into_iter()
                                .map(|b| wallpaper::to_physical_rect(&b, scale))
                                .collect();
                            *MUSIC_RECTS.write() = rects;
                            sync_all_interactive_regions();
                        }
                    })
                    .child(self.music_player.clone()),
            )
            // ── TOP RIGHT CONTROL ACTION BAR ──
            .child(
                div()
                    .absolute()
                    .top(px(24.0))
                    .right(px(24.0))
                    .on_children_prepainted({
                        move |children_bounds, window, _cx| {
                            let scale = window.scale_factor();
                            let rects: Vec<RECT> = children_bounds
                                .into_iter()
                                .map(|b| wallpaper::to_physical_rect(&b, scale))
                                .collect();
                            *CTRL_RECTS.write() = rects;
                            sync_all_interactive_regions();
                        }
                    })
                    .child(self.control_bar.clone()),
            )
            // ── RIGHT DOCKED TELEGRAM RAIL ──
            .child(
                div()
                    .absolute()
                    .top(px(76.0))
                    .right(px(24.0))
                    .on_children_prepainted({
                        move |children_bounds, window, _cx| {
                            let scale = window.scale_factor();
                            let rects: Vec<RECT> = children_bounds
                                .into_iter()
                                .map(|b| wallpaper::to_physical_rect(&b, scale))
                                .collect();
                            *TG_RECTS.write() = rects;
                            sync_all_interactive_regions();
                        }
                    })
                    .child(self.telegram_rail.clone()),
            )
    }
}

fn extract_hwnd(window: &Window) -> Option<HWND> {
    if let Ok(handle) = HasWindowHandle::window_handle(window) {
        match handle.as_raw() {
            RawWindowHandle::Win32(win32_handle) => {
                let hwnd_isize = win32_handle.hwnd.get();
                Some(HWND(hwnd_isize as *mut std::ffi::c_void))
            }
            _ => None,
        }
    } else {
        None
    }
}

fn main() {
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    wallpaper::attach_thread_to_input_desktop();

    let (width, height) = wallpaper::primary_monitor_logical_size();
    println!("[GPUI Desktop] Primary monitor logical resolution: {}x{}", width, height);

    Application::new().run(move |cx: &mut App| {
        // Initialize Nothing OS design system & load embedded Ndot 55 font
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
            |window, cx| {
                if let Some(hwnd) = extract_hwnd(window) {
                    println!("[GPUI Desktop] Resolved native HWND: 0x{:X}", hwnd.0 as usize);

                    // Locate Desktop Host (WorkerW or Progman fallback) and attach under desktop icons
                    match wallpaper::find_desktop_host() {
                        Ok((host, host_kind)) => {
                            println!(
                                "[GPUI Desktop] Found desktop host: 0x{:X} ({host_kind}). Attaching window...",
                                host.0 as usize
                            );
                            if let Err(e) = wallpaper::attach_to_desktop(hwnd, host) {
                                eprintln!("[GPUI Desktop] Error attaching to desktop host: {e}");
                            } else {
                                println!("[GPUI Desktop] Successfully attached GPUI window as child of {host_kind}!");
                                // Initial hit-test regions placeholder while layout computes
                                wallpaper::publish_interactive_regions(vec![
                                    RECT { left: 48, top: 48, right: 48 + 320, bottom: 48 + 180 },
                                    RECT { left: 48 + 320 + 24, top: 48, right: 48 + 320 + 24 + 260, bottom: 48 + 180 },
                                ]);
                            }
                        }
                        Err(e) => {
                            eprintln!("[GPUI Desktop] Desktop search failed: {e}. Running as standalone transparent window.");
                        }
                    }
                }

                let workspace = cx.new(|cx| DesktopWorkspace::new(cx));
                let top_dash_ref = workspace.read(cx).top_dashboard.clone();
                let music_ref = workspace.read(cx).music_player.clone();

                // Reactive 0% Idle CPU background ticker:
                // - Only animates music when playing (120ms)
                // - Aligns time sync to exact system second rollover
                window
                    .spawn(cx, async move |cx| {
                        loop {
                            let is_playing = cx
                                .update(|_window, cx| music_ref.read(cx).is_playing)
                                .unwrap_or(false);

                            if is_playing {
                                Timer::after(Duration::from_millis(120)).await;
                                let _ = cx.update(|_window, cx| {
                                    music_ref.update(cx, |m, cx| {
                                        m.tick_animation();
                                        cx.notify();
                                    });
                                    top_dash_ref.update(cx, |t, cx| {
                                        t.sync_time();
                                        t.clock.update(cx, |c, cx| {
                                            c.sync_time();
                                            cx.notify();
                                        });
                                        cx.notify();
                                    });
                                });
                            } else {
                                // 0% IDLE CPU: Sleep until the next exact second rollover
                                let millis_to_next = 1000 - (chrono::Local::now().timestamp_subsec_millis() as u64);
                                Timer::after(Duration::from_millis(millis_to_next.max(50))).await;
                                let _ = cx.update(|_window, cx| {
                                    top_dash_ref.update(cx, |t, cx| {
                                        t.sync_time();
                                        t.clock.update(cx, |c, cx| {
                                            c.sync_time();
                                            cx.notify();
                                        });
                                        cx.notify();
                                    });
                                });
                            }
                        }
                    })
                    .detach();

                workspace
            },
        )
        .unwrap();
    });
}
