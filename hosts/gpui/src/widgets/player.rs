use gpui::{
    div, prelude::*, px, rgb, rgba, Context, FontWeight, IntoElement, MouseButton, Render, Window,
};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

pub struct PlayerWidget {
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub time_pos: f64,
    pub is_playing: bool,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub spectrum_bars: [f32; 16],
}

impl PlayerWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            title: "NEURO BEATS".to_string(),
            artist: "AMBIENT FREQUENCIES".to_string(),
            duration: 214.0,
            time_pos: 42.0,
            is_playing: false,
            bass: 0.15,
            mid: 0.25,
            treble: 0.10,
            spectrum_bars: [
                0.2, 0.35, 0.5, 0.8, 0.65, 0.4, 0.3, 0.45, 0.6, 0.75, 0.5, 0.35, 0.2, 0.4, 0.3,
                0.15,
            ],
        }
    }

    pub fn toggle_play_pause(&mut self) {
        self.is_playing = !self.is_playing;
    }

    pub fn next_track(&mut self) {
        self.time_pos = 0.0;
        self.title = "DISCOGS SYNTHESIS".to_string();
        self.artist = "AI DJ ROTATION".to_string();
        self.duration = 186.0;
    }

    pub fn prev_track(&mut self) {
        self.time_pos = 0.0;
    }

    pub fn update_spectrum(&mut self, bass: f32, mid: f32, treble: f32) {
        self.bass = bass;
        self.mid = mid;
        self.treble = treble;

        for (i, bar) in self.spectrum_bars.iter_mut().enumerate() {
            let target = match i {
                0..=4 => bass * (1.0 + (i as f32 * 0.1)),
                5..=10 => mid * (1.0 + ((i - 5) as f32 * 0.08)),
                _ => treble * (1.0 + ((i - 11) as f32 * 0.12)),
            };
            *bar = (*bar * 0.65 + target * 0.35).clamp(0.08, 1.0);
        }
    }

    pub fn tick_playback(&mut self) {
        if self.is_playing {
            self.time_pos += 1.0;
            if self.time_pos > self.duration {
                self.time_pos = 0.0;
            }

            // Animate spectrum bars slightly on playback tick if no live FFT
            for (i, bar) in self.spectrum_bars.iter_mut().enumerate() {
                let jitter = (((i as f32 * 17.0 + self.time_pos as f32 * 3.0).sin() * 0.5 + 0.5)
                    * 0.6)
                    + 0.15;
                *bar = (*bar * 0.7 + jitter * 0.3).clamp(0.08, 1.0);
            }
        }
    }
}

fn format_time(secs: f64) -> String {
    let s = secs.floor() as u64;
    let m = s / 60;
    let rem_s = s % 60;
    format!("{m:02}:{rem_s:02}")
}

impl DesktopWidgetContent for PlayerWidget {
    fn id(&self) -> &'static str {
        "mp3-player"
    }
}

impl Render for PlayerWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_playing = self.is_playing;
        let progress = if self.duration > 0.0 {
            (self.time_pos / self.duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };

        div()
            .flex()
            .flex_col()
            .w(px(380.0))
            .p_6()
            .rounded_3xl()
            .bg(theme::bg_glass())
            .border_1()
            .border_color(theme::border_glass())
            .shadow_lg()
            .child(
                // Header: Indicator dot, title, and engine pill
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .mb_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size_2()
                                    .rounded_full()
                                    .bg(if is_playing {
                                        theme::nothing_red()
                                    } else {
                                        rgba(0xffffff40)
                                    }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("AUDIO CORE"),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(rgba(0xffffff15))
                            .border_1()
                            .border_color(theme::border_glass())
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_secondary())
                                    .child("MPV WASAPI"),
                            ),
                    ),
            )
            .child(
                // Track Info row
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .mb_2()
                    .child(
                        div()
                            .font(font_dot())
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::text_secondary())
                            .child(self.artist.clone()),
                    ),
            )
            .child(
                // Equalizer FFT Spectrum Visualizer (16 reactive vertical LED bars)
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .h(px(32.0))
                    .my_2()
                    .px_1()
                    .children(self.spectrum_bars.iter().enumerate().map(|(_idx, &val)| {
                        let bar_h = (val * 32.0).max(4.0);
                        let is_peak = val > 0.75;
                        div()
                            .w(px(14.0))
                            .h(px(bar_h))
                            .rounded_full()
                            .bg(if is_peak {
                                theme::nothing_red()
                            } else if is_playing {
                                rgba(0xffffffdd)
                            } else {
                                rgba(0xffffff30)
                            })
                    })),
            )
            .child(
                // Progress Bar and Time labels
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .mt_2()
                    .mb_3()
                    .child(
                        // Scrubber track
                        div()
                            .w_full()
                            .h(px(3.0))
                            .rounded_full()
                            .bg(rgba(0xffffff20))
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme::nothing_red())
                                    .w(px(progress * 332.0)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .w_full()
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(format_time(self.time_pos)),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(format_time(self.duration)),
                            ),
                    ),
            )
            .child(
                // Playback Controls Row: Prev, Play/Pause, Next
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_6()
                    .w_full()
                    .pt_1()
                    .border_t_1()
                    .border_color(theme::border_glass())
                    .child(
                        // Prev Button
                        div()
                            .cursor_pointer()
                            .p_2()
                            .rounded_full()
                            .hover(|s| s.bg(rgba(0xffffff15)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.prev_track();
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child("⏮"),
                            ),
                    )
                    .child(
                        // Play / Pause Circular Action Button
                        div()
                            .size_10()
                            .rounded_full()
                            .bg(if is_playing {
                                rgba(0xffffff25)
                            } else {
                                theme::text_primary()
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .shadow_md()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_play_pause();
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if is_playing {
                                        theme::text_primary()
                                    } else {
                                        rgb(0x000000)
                                    })
                                    .child(if is_playing { "⏸" } else { "▶" }),
                            ),
                    )
                    .child(
                        // Next Button
                        div()
                            .cursor_pointer()
                            .p_2()
                            .rounded_full()
                            .hover(|s| s.bg(rgba(0xffffff15)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.next_track();
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child("⏭"),
                            ),
                    ),
            )
    }
}
