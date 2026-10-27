use gpui::{
    div, prelude::*, px, rgb, rgba, Context, FontWeight, IntoElement, MouseButton, Render, Window,
};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

pub struct MusicPlayerWidget {
    pub is_playing: bool,
    pub title: String,
    pub bass_pulse: f32,
    pub mid_pulse: f32,
    pub treble_pulse: f32,
    pub progress: f32,
}

impl MusicPlayerWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            is_playing: false,
            title: "---".to_string(),
            bass_pulse: 1.0,
            mid_pulse: 1.0,
            treble_pulse: 1.0,
            progress: 0.0,
        }
    }

    pub fn toggle_play(&mut self) {
        self.is_playing = !self.is_playing;
    }

    pub fn tick_animation(&mut self) {
        if self.is_playing {
            self.progress = (self.progress + 0.005) % 1.0;
            // Simulated audio reactive pulse
            let t = self.progress * std::f32::consts::TAU * 8.0;
            self.bass_pulse = 1.0 + (t.sin() * 0.08).abs();
            self.mid_pulse = 1.0 + ((t * 1.5).cos() * 0.06).abs();
            self.treble_pulse = 1.0 + ((t * 2.3).sin() * 0.05).abs();
        } else {
            self.bass_pulse = 1.0;
            self.mid_pulse = 1.0;
            self.treble_pulse = 1.0;
        }
    }
}

impl DesktopWidgetContent for MusicPlayerWidget {
    fn id(&self) -> &'static str {
        "music-player"
    }
}

impl Render for MusicPlayerWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let red = theme::nothing_red();
        let outer_scale = self.bass_pulse;

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .child(
                // Outer Concentric Reactive Ring Container (Shadow + Visualizer Arcs)
                div()
                    .size(px(160.0 * outer_scale))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .shadow_2xl()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_play();
                            cx.notify();
                        }),
                    )
                    .child(
                        // Outer Grey/Black Vinyl Disc Border
                        div()
                            .size(px(144.0))
                            .rounded_full()
                            .border_4()
                            .border_color(rgb(0x181a20))
                            .bg(rgb(0xffffff))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                // Inner Ring
                                div()
                                    .size(px(132.0))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(rgba(0x00000015))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        // Center Nothing Red Dot (ring-accent-dot)
                                        div()
                                            .size(px(10.0))
                                            .rounded_full()
                                            .bg(red),
                                    ),
                            ),
                    ),
            )
            .child(
                // Bottom Track Title Pill: "---"
                div()
                    .px_4()
                    .py_1()
                    .rounded_full()
                    .bg(rgb(0x2d323c))
                    .border_1()
                    .border_color(rgba(0xffffff15))
                    .font(font_dot())
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xffffff))
                    .child(self.title.clone()),
            )
    }
}
