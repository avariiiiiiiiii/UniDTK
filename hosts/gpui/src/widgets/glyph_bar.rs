use gpui::{
    div, prelude::*, px, rgba, Context, FontWeight, IntoElement, MouseButton, Render,
    ScrollDelta, ScrollWheelEvent, Window,
};
use unidtk_mod_glyph_bar::{get_system_volume, set_system_mute, set_system_volume};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

pub struct GlyphBarWidget {
    pub volume: u8,
    pub muted: bool,
}

impl GlyphBarWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let (volume, muted) = match get_system_volume() {
            Ok(v) => (v.level, v.muted),
            Err(_) => (50, false),
        };

        Self { volume, muted }
    }

    /// Syncs state with Windows WASAPI master volume. Returns true if changed.
    pub fn sync_from_system(&mut self) -> bool {
        if let Ok(sys) = get_system_volume() {
            if self.volume != sys.level || self.muted != sys.muted {
                self.volume = sys.level;
                self.muted = sys.muted;
                return true;
            }
        }
        false
    }

    pub fn set_volume(&mut self, level: u8) {
        let clamped = level.min(100);
        self.volume = clamped;
        if clamped > 0 {
            self.muted = false;
        }
        let _ = set_system_volume(clamped);
    }

    pub fn toggle_mute(&mut self) {
        let new_mute = !self.muted;
        self.muted = new_mute;
        let _ = set_system_mute(new_mute);
    }

    pub fn adjust_volume(&mut self, delta: i8) {
        let current = self.volume as i16;
        let next = (current + delta as i16).clamp(0, 100) as u8;
        self.set_volume(next);
    }
}

impl DesktopWidgetContent for GlyphBarWidget {
    fn id(&self) -> &'static str {
        "glyph-bar"
    }
}

impl Render for GlyphBarWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let volume = self.volume;
        let muted = self.muted;

        // Number of active LED segments (0..10)
        let active_segments = if muted {
            0
        } else {
            ((volume as f32 / 10.0).round() as usize).min(10)
        };

        div()
            .flex()
            .flex_col()
            .w(px(260.0))
            .p_6()
            .rounded_3xl()
            .bg(theme::bg_glass())
            .border_1()
            .border_color(theme::border_glass())
            .shadow_lg()
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                let dy = match event.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => p.y.to_f64() as f32,
                };
                if dy.abs() > 0.05 {
                    let step = if dy > 0.0 { 5 } else { -5 };
                    this.adjust_volume(step);
                    cx.notify();
                }
            }))
            .child(
                // Header: Title & Mute/Volume status badge
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .mb_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                // Red/Green Nothing glyph dot
                                div()
                                    .size_2()
                                    .rounded_full()
                                    .bg(if muted {
                                        theme::nothing_red()
                                    } else {
                                        theme::accent_green()
                                    }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("GLYPH BAR"),
                            ),
                    )
                    .child(
                        // Mute button / status pill
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(if muted {
                                rgba(0xd7192133)
                            } else {
                                rgba(0xffffff15)
                            })
                            .border_1()
                            .border_color(if muted {
                                theme::nothing_red()
                            } else {
                                theme::border_glass()
                            })
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_mute();
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if muted {
                                        theme::nothing_red()
                                    } else {
                                        theme::text_primary()
                                    })
                                    .child(if muted {
                                        "MUTED"
                                    } else {
                                        "ACTIVE"
                                    }),
                            ),
                    ),
            )
            .child(
                // 10-Segment LED Meter (Nothing OS Phone Glyph Bar style)
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .py_2()
                    .px_1()
                    .children((0..10).map(|i| {
                        let is_lit = i < active_segments;
                        let target_vol = ((i + 1) * 10) as u8;

                        div()
                            .w(px(16.0))
                            .h(px(34.0))
                            .rounded_md()
                            .bg(if is_lit {
                                theme::glyph_on()
                            } else {
                                theme::glyph_off()
                            })
                            .border_1()
                            .border_color(if is_lit {
                                rgba(0xffffff40)
                            } else {
                                rgba(0xffffff0d)
                            })
                            .shadow_sm()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_volume(target_vol);
                                    cx.notify();
                                }),
                            )
                    })),
            )
            .child(
                // Footer: Master level percentage and WASAPI status
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .mt_3()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border_glass())
                    .child(
                        div()
                            .font(font_dot())
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child("WASAPI MASTER"),
                    )
                    .child(
                        div()
                            .font(font_dot())
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if muted {
                                theme::nothing_red()
                            } else {
                                theme::text_primary()
                            })
                            .child(format!("{}%", volume)),
                    ),
            )
    }
}
