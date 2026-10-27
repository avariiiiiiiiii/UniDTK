use chrono::{Datelike, Timelike};
use gpui::{
    canvas, div, point, prelude::*, px, rgb, rgba, Background, Context, FontWeight, IntoElement,
    MouseButton, Path, Pixels, Point, Render, Window,
};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

pub struct ClockWidget {
    pub time_str: String,
    pub minute: u32,
    pub second: u32,
    pub day: u32,
    pub month: u32,
    pub year: i32,
    pub month_name: String,
    pub days_in_month: u32,
    pub first_weekday_sun0: usize,
    pub is_expanded: bool,
}

impl ClockWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut widget = Self {
            time_str: "00:00".to_string(),
            minute: 0,
            second: 0,
            day: 1,
            month: 1,
            year: 2026,
            month_name: "ОКТЯБРЬ".to_string(),
            days_in_month: 31,
            first_weekday_sun0: 0,
            is_expanded: false,
        };
        widget.sync_time();
        widget
    }

    pub fn sync_time(&mut self) {
        let now = chrono::Local::now();
        self.time_str = now.format("%H:%M").to_string();
        self.minute = now.minute();
        self.second = now.second();
        self.day = now.day();
        self.month = now.month();
        self.year = now.year();

        self.days_in_month = match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if (self.year % 4 == 0 && self.year % 100 != 0) || (self.year % 400 == 0) {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        };

        if let Some(first_date) = chrono::NaiveDate::from_ymd_opt(self.year, self.month, 1) {
            self.first_weekday_sun0 = first_date.weekday().num_days_from_sunday() as usize;
        }

        self.month_name = match self.month {
            1 => "ЯНВАРЬ",
            2 => "ФЕВРАЛЬ",
            3 => "МАРТ",
            4 => "АПРЕЛЬ",
            5 => "МАЙ",
            6 => "ИЮНЬ",
            7 => "ИЮЛЬ",
            8 => "АВГУСТ",
            9 => "СЕНТЯБРЬ",
            10 => "ОКТЯБРЬ",
            11 => "НОЯБРЬ",
            _ => "ДЕКАБРЬ",
        }
        .to_string();
    }

    pub fn toggle_expand(&mut self) {
        self.is_expanded = !self.is_expanded;
    }

    pub fn collapse(&mut self) {
        self.is_expanded = false;
    }
}

impl DesktopWidgetContent for ClockWidget {
    fn id(&self) -> &'static str {
        "clock"
    }
}

impl Render for ClockWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_expanded = self.is_expanded;
        let minute = self.minute;
        let second = self.second;
        let time_str = self.time_str.clone();

        if is_expanded {
            // ── EXPANDED CLOCK DASHBOARD (420px x 280px) ──
            let month_header = format!("{} {}", self.month_name, self.year);
            let today = self.day;
            let days_count = self.days_in_month;
            let empty_lead = self.first_weekday_sun0;

            div()
                .w(px(420.0))
                .p_5()
                .rounded_3xl()
                .bg(rgba(0x0f0f14fa))
                .border_1()
                .border_color(rgba(0xffffff26))
                .shadow_2xl()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    // Top header with close button
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .w_full()
                        .pb_2()
                        .border_b_1()
                        .border_color(rgba(0xffffff15))
                        .child(
                            div()
                                .font(font_dot())
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::nothing_red())
                                .child("NOTHING OS // DASHBOARD"),
                        )
                        .child(
                            div()
                                .size(px(24.0))
                                .rounded_full()
                                .bg(rgba(0xffffff10))
                                .hover(|s| s.bg(rgba(0xffffff25)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.collapse();
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgba(0xffffffbb))
                                        .child("✕"),
                                ),
                        ),
                )
                .child(
                    // Main content: Left Clock Face + Right Calendar & Net
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(16.0))
                        .child(
                            // Left: 140px Rings Clock
                            div()
                                .size(px(140.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(render_concentric_rings(minute, second, time_str, px(130.0))),
                        )
                        .child(
                            // Right: Calendar + Internet Status
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .child(
                                    // Calendar Card
                                    div()
                                        .p_3()
                                        .rounded_2xl()
                                        .bg(rgba(0xffffff08))
                                        .border_1()
                                        .border_color(rgba(0xffffff10))
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .font(font_dot())
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme::nothing_red())
                                                .text_center()
                                                .child(month_header),
                                        )
                                        .child(
                                            // Calendar Grid (7 columns: S M T W T F S)
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(2.0))
                                                .child(
                                                    // Day headers
                                                    div()
                                                        .flex()
                                                        .justify_between()
                                                        .text_xs()
                                                        .text_color(rgba(0xffffff4d))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(div().w(px(22.0)).text_center().child("S"))
                                                        .child(div().w(px(22.0)).text_center().child("M"))
                                                        .child(div().w(px(22.0)).text_center().child("T"))
                                                        .child(div().w(px(22.0)).text_center().child("W"))
                                                        .child(div().w(px(22.0)).text_center().child("T"))
                                                        .child(div().w(px(22.0)).text_center().child("F"))
                                                        .child(div().w(px(22.0)).text_center().child("S")),
                                                )
                                                .child({
                                                    let mut grid = div().flex().flex_wrap().w_full();
                                                    for _ in 0..empty_lead {
                                                        grid = grid.child(div().w(px(22.0)).h(px(18.0)));
                                                    }
                                                    for d in 1..=days_count {
                                                        let is_today = d == today;
                                                        let day_cell = if is_today {
                                                            div()
                                                                .w(px(22.0))
                                                                .h(px(18.0))
                                                                .rounded(px(4.0))
                                                                .bg(rgb(0xffffff))
                                                                .text_color(rgb(0x000000))
                                                                .text_xs()
                                                                .font_weight(FontWeight::BOLD)
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(format!("{d}"))
                                                        } else {
                                                            div()
                                                                .w(px(22.0))
                                                                .h(px(18.0))
                                                                .text_color(rgba(0xffffff99))
                                                                .text_xs()
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(format!("{d}"))
                                                        };
                                                        grid = grid.child(day_cell);
                                                    }
                                                    grid
                                                }),
                                        ),
                                )
                                .child(
                                    // Internet Status Card
                                    div()
                                        .px_3()
                                        .py_2()
                                        .rounded_xl()
                                        .bg(rgba(0xffffff08))
                                        .border_1()
                                        .border_color(rgba(0xffffff10))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .size_2()
                                                        .rounded_full()
                                                        .bg(rgb(0x00ff88)),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgba(0xffffffcc))
                                                        .child("Интернет"),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x00ff88))
                                                .child("Подключено"),
                                        ),
                                ),
                        ),
                )
        } else {
            // ── COLLAPSED 210x190 NOTHING OS CLOCK CARD (Matching Tauri exactly) ──
            div()
                .w(px(210.0))
                .h(px(190.0))
                .rounded_2xl()
                .bg(rgba(0x0f121ec7))
                .border_1()
                .border_color(rgba(0xffffff1f))
                .hover(|s| s.border_color(rgba(0xffffff38)))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.toggle_expand();
                        cx.notify();
                    }),
                )
                .child(render_concentric_rings(minute, second, time_str, px(120.0)))
        }
    }
}

/// Renders the authentic Nothing OS Concentric Rings:
/// - Outer Ring (120px): Track `rgba(255,255,255,0.05)`, active 90° white arc rotated by minute * 6°
/// - Inner Ring (84px / 70%): Track `rgba(255,255,255,0.05)`, active 90° Nothing Red (#ff3b30) arc rotated by sec * 6°
/// - Center: Digital time in Ndot 55 dot-matrix typography (19.2px)
pub fn render_concentric_rings(minute: u32, second: u32, time_str: String, dial_size: Pixels) -> impl IntoElement {
    let font_mono = font_dot();

    div()
        .relative()
        .size(dial_size)
        .flex()
        .items_center()
        .justify_center()
        .child(
            // Hardware-accelerated canvas for Lyon-tessellated concentric arcs
            canvas(
                move |_bounds, _window, _cx| (),
                move |bounds, (), window, _cx| {
                    let cx = bounds.origin.x + bounds.size.width / 2.0;
                    let cy = bounds.origin.y + bounds.size.height / 2.0;
                    let center = point(cx, cy);

                    let outer_radius = bounds.size.width * 0.50; // 60px
                    let inner_radius = bounds.size.width * 0.35; // 42px (70%)
                    let thickness = px(4.0);

                    let track_color = rgba(0xffffff0d); // rgba(255, 255, 255, 0.05)
                    let white_color = rgba(0xffffffff);
                    let red_color = rgba(0xff3b30ff); // Nothing Red #ff3b30

                    // 1. Outer full track circle (120px diameter)
                    draw_circle_arc(center, outer_radius, thickness, 0.0, std::f32::consts::TAU, track_color, window);

                    // 2. Outer minute arc: 90° arc segment (border-top equivalent) rotated by (minute * 6°)
                    let min_angle = -std::f32::consts::FRAC_PI_2 + (minute as f32 * 6.0).to_radians();
                    let half_arc = std::f32::consts::FRAC_PI_4;
                    draw_circle_arc(center, outer_radius, thickness, min_angle - half_arc, min_angle + half_arc, white_color, window);

                    // 3. Inner full track circle (84px diameter)
                    draw_circle_arc(center, inner_radius, thickness, 0.0, std::f32::consts::TAU, track_color, window);

                    // 4. Inner second arc: 90° arc segment rotated by (second * 6°) in Nothing Red (#ff3b30)
                    let sec_angle = -std::f32::consts::FRAC_PI_2 + (second as f32 * 6.0).to_radians();
                    draw_circle_arc(center, inner_radius, thickness, sec_angle - half_arc, sec_angle + half_arc, red_color, window);
                },
            )
            .size(dial_size),
        )
        .child(
            // Center Ndot 55 Digital Time (19.2px)
            div()
                .absolute()
                .font(font_mono)
                .text_size(px(19.2))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xffffff))
                .child(time_str),
        )
}

fn draw_circle_arc(
    center: Point<Pixels>,
    radius: Pixels,
    thickness: Pixels,
    start_angle: f32,
    end_angle: f32,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let r_out = (radius + thickness / 2.0).to_f64() as f32;
    let r_in = (radius - thickness / 2.0).to_f64() as f32;
    let angle_delta = end_angle - start_angle;
    let steps = ((angle_delta.abs() * 32.0 / std::f32::consts::TAU).ceil() as usize).max(4);

    let mut path = Path::new(center);
    let st_solid = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));

    for i in 0..steps {
        let a1 = start_angle + angle_delta * (i as f32 / steps as f32);
        let a2 = start_angle + angle_delta * ((i + 1) as f32 / steps as f32);

        let p1 = point(center.x + px(r_out * a1.cos()), center.y + px(r_out * a1.sin()));
        let p2 = point(center.x + px(r_in * a1.cos()), center.y + px(r_in * a1.sin()));
        let p3 = point(center.x + px(r_out * a2.cos()), center.y + px(r_out * a2.sin()));
        let p4 = point(center.x + px(r_in * a2.cos()), center.y + px(r_in * a2.sin()));

        path.push_triangle((p1, p2, p3), st_solid);
        path.push_triangle((p2, p4, p3), st_solid);
    }

    window.paint_path(path, color);
}
