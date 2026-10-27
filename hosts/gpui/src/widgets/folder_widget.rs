use gpui::{
    canvas, div, point, prelude::*, px, rgb, rgba, Background, Context, FontWeight, IntoElement,
    MouseButton, Path, Pixels, Point, Render, Window,
};
use windows::core::HSTRING;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::db::{FolderItem, FolderRecord};
use crate::theme::{self, font_dot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderIconKind {
    Gamepad,
    Notes,
    Launcher,
    Tools,
    Network,
    DefaultFolder,
}

impl FolderIconKind {
    pub fn from_id(id: &str) -> Self {
        match id.to_lowercase().as_str() {
            "games" | "game" => Self::Gamepad,
            "notes" | "docs" | "documents" => Self::Notes,
            "launcher" | "apps" => Self::Launcher,
            "tools" | "utils" => Self::Tools,
            "network" | "web" => Self::Network,
            _ => Self::DefaultFolder,
        }
    }
}

pub fn open_shortcut_target(target: &str) {
    if target.is_empty() {
        return;
    }
    unsafe {
        let op = HSTRING::from("open");
        let path = HSTRING::from(target);
        let _ = ShellExecuteW(None, &op, &path, None, None, SW_SHOWNORMAL);
    }
}

pub struct FolderWidget {
    pub id: String,
    pub name: String,
    pub icon_kind: FolderIconKind,
    pub items: Vec<FolderItem>,
    pub is_expanded: bool,
}

impl FolderWidget {
    pub fn new(id: impl Into<String>, name: impl Into<String>, icon_kind: FolderIconKind, items: Vec<FolderItem>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            icon_kind,
            items,
            is_expanded: false,
        }
    }

    pub fn from_record(record: &FolderRecord) -> Self {
        let kind = FolderIconKind::from_id(&record.id);
        Self::new(record.id.clone(), record.name.clone(), kind, record.items.clone())
    }

    pub fn toggle_expand(&mut self) {
        self.is_expanded = !self.is_expanded;
    }

    pub fn collapse(&mut self) {
        self.is_expanded = false;
    }
}

impl Render for FolderWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.icon_kind;
        let is_expanded = self.is_expanded;
        let folder_name = self.name.clone();
        let items_count = self.items.len();

        if is_expanded {
            // ── EXPANDED 340px SHORTCUTS DASHBOARD ──
            div()
                .w(px(340.0))
                .p_4()
                .rounded_3xl()
                .bg(rgba(0x0f0f14f8))
                .border_1()
                .border_color(rgba(0xffffff26))
                .shadow_2xl()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    // Header row: Folder title + count badge + close button
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
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(render_vector_icon(kind, px(20.0), theme::nothing_red()))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xffffff))
                                        .child(folder_name),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_muted())
                                        .child(format!("({items_count})")),
                                ),
                        )
                        .child(
                            // Close button (X)
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
                    // Shortcuts Grid (4 columns)
                    if self.items.is_empty() {
                        div()
                            .w_full()
                            .py_6()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child("Папка пуста"),
                            )
                            .into_any_element()
                    } else {
                        let mut grid = div().flex().flex_wrap().gap(px(8.0)).w_full();
                        for item in self.items.iter() {
                            let target = item.target.clone();
                            let is_folder = item.kind == "folder";
                            let icon_color = if is_folder {
                                rgb(0xffd25a)
                            } else {
                                rgb(0xffffff)
                            };
                            let box_bg = if is_folder {
                                rgba(0xffc83c18)
                            } else {
                                rgba(0xffffff0d)
                            };
                            let box_border = if is_folder {
                                rgba(0xffc83c30)
                            } else {
                                rgba(0xffffff12)
                            };

                            let shortcut_btn = div()
                                .w(px(72.0))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(4.0))
                                .p_1()
                                .rounded_xl()
                                .hover(|s| s.bg(rgba(0xffffff10)))
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    move |_event, _window, _cx| {
                                        open_shortcut_target(&target);
                                    },
                                )
                                .child(
                                    // 38x38 Squircle Icon Box
                                    div()
                                        .size(px(38.0))
                                        .rounded_xl()
                                        .bg(box_bg)
                                        .border_1()
                                        .border_color(box_border)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            if is_folder {
                                                render_vector_icon(FolderIconKind::DefaultFolder, px(18.0), icon_color)
                                            } else {
                                                render_vector_icon(FolderIconKind::Notes, px(18.0), icon_color)
                                            },
                                        ),
                                )
                                .child(
                                    // 2-line truncated text
                                    div()
                                        .text_xs()
                                        .line_height(px(13.0))
                                        .text_color(rgba(0xffffff99))
                                        .text_center()
                                        .w(px(68.0))
                                        .child(item.name.clone()),
                                );

                            grid = grid.child(shortcut_btn);
                        }
                        grid.into_any_element()
                    },
                )
        } else {
            // ── COLLAPSED 64x64 NOTHING OS SQUIRCLE BUTTON (Matching Tauri exactly) ──
            div()
                .size(px(64.0))
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
                .child(render_vector_icon(kind, px(28.0), rgb(0xffffff)))
        }
    }
}

/// Hardware-accelerated Nothing OS Stroke Vector Icon via GPUI canvas
/// Recreates the exact SVG stroke geometry from the Tauri production shell.
pub fn render_vector_icon(kind: FolderIconKind, size: Pixels, color: gpui::Rgba) -> impl IntoElement {
    canvas(
        move |_bounds, _window, _cx| (),
        move |bounds, (), window, _cx| {
            let s = bounds.size.width.min(bounds.size.height).to_f64() as f32;
            let scale = s / 24.0;
            let ox = bounds.origin.x;
            let oy = bounds.origin.y;
            let p = |x: f32, y: f32| point(ox + px(x * scale), oy + px(y * scale));

            let stroke = px(1.4 * scale);
            let stroke_thick = px(1.7 * scale);

            match kind {
                FolderIconKind::Gamepad => {
                    // Authentic Nothing Gamepad matching Tauri SVG:
                    // <path d="M7.5 6.5h9a4 4 0 0 1 3.92 3.22l1.05 5.25a2.3 2.3 0 0 1-4.18 1.72l-.98-1.45a2 2 0 0 0-1.66-.89H9.34a2 2 0 0 0-1.66.89l-.98 1.45a2.3 2.3 0 0 1-4.18-1.72l1.05-5.25A4 4 0 0 1 7.5 6.5z" stroke-width="1.4"/>
                    let pts = [
                        (7.5, 6.5), (16.5, 6.5), (20.4, 9.7), (21.5, 15.0),
                        (17.3, 16.7), (16.3, 15.2), (14.65, 14.35), (9.34, 14.35),
                        (7.68, 15.2), (6.7, 16.7), (2.5, 15.0), (3.6, 9.7), (7.5, 6.5),
                    ];
                    for i in 0..(pts.len() - 1) {
                        draw_line(p(pts[i].0, pts[i].1), p(pts[i + 1].0, pts[i + 1].1), stroke, color, window);
                    }
                    // Cross: <path d="M6.4 11h3.2M8 9.4v3.2" stroke-linecap="round"/>
                    draw_line(p(6.4, 11.0), p(9.6, 11.0), stroke, color, window);
                    draw_line(p(8.0, 9.4), p(8.0, 12.6), stroke, color, window);
                    // Buttons: <circle cx="15.4" cy="10.6" r="0.95" fill="currentColor"/> <circle cx="17.6" cy="12.8" r="0.95"/>
                    draw_dot(p(15.4, 10.6), px(0.95 * scale), color, window);
                    draw_dot(p(17.6, 12.8), px(0.95 * scale), color, window);
                }
                FolderIconKind::Notes => {
                    // Authentic Notes Document matching Tauri SVG:
                    // <path d="M6 4H18V20H6V4Z" stroke-width="1.7"/>
                    // <path d="M8 8H16 M8 12H14 M8 16H12" stroke-width="1.7"/>
                    let corners = [(6.0, 4.0), (18.0, 4.0), (18.0, 20.0), (6.0, 20.0), (6.0, 4.0)];
                    for i in 0..4 {
                        draw_line(p(corners[i].0, corners[i].1), p(corners[i + 1].0, corners[i + 1].1), stroke_thick, color, window);
                    }
                    draw_line(p(8.0, 8.0), p(16.0, 8.0), stroke_thick, color, window);
                    draw_line(p(8.0, 12.0), p(14.0, 12.0), stroke_thick, color, window);
                    draw_line(p(8.0, 16.0), p(12.0, 16.0), stroke_thick, color, window);
                }
                FolderIconKind::Launcher => {
                    // Authentic Profile / Launcher matching Tauri SVG:
                    // <circle cx="12" cy="12" r="9"/>
                    // <circle cx="12" cy="9" r="2.5"/> <circle cx="12" cy="9" r="0.7" fill/>
                    // <path d="M7 17c0-2.8 2.2-5 5-5s5 2.2 5 5"/>
                    // <circle cx="18" cy="6" r="1.5"/> <circle cx="18" cy="6" r="0.5" fill/>
                    draw_circle_stroke(p(12.0, 12.0), px(9.0 * scale), stroke, color, window);
                    draw_circle_stroke(p(12.0, 9.0), px(2.5 * scale), stroke, color, window);
                    draw_dot(p(12.0, 9.0), px(0.7 * scale), color, window);
                    // Shoulder arch
                    let arch_pts = [(7.0, 17.0), (7.8, 14.5), (9.5, 12.8), (12.0, 12.0), (14.5, 12.8), (16.2, 14.5), (17.0, 17.0)];
                    for i in 0..(arch_pts.len() - 1) {
                        draw_line(p(arch_pts[i].0, arch_pts[i].1), p(arch_pts[i + 1].0, arch_pts[i + 1].1), stroke, color, window);
                    }
                    draw_circle_stroke(p(18.0, 6.0), px(1.5 * scale), px(1.2 * scale), color, window);
                    draw_dot(p(18.0, 6.0), px(0.5 * scale), color, window);
                }
                FolderIconKind::Tools => {
                    // Authentic 45-degree Wrench matching Tauri SVG:
                    // <path d="M14.7 6.3a1 1 0 000 1.4l1.6 1.6a1 1 0 001.4 0l3.77-3.77a6 6 0 01-7.94 7.94L6.73 20.2a2 2 0 01-2.83 0l-.1-.1a2 2 0 010-2.83l6.73-6.73A6 6 0 0114.7 6.3z"/>
                    // <circle cx="17.5" cy="6.5" r="0.7" fill/>
                    let wrench = [
                        (14.7, 6.3), (16.3, 7.9), (20.07, 4.13), (18.6, 9.5), (14.7, 12.5),
                        (6.73, 20.2), (3.9, 17.37), (11.0, 10.3), (12.2, 6.5), (14.7, 6.3),
                    ];
                    for i in 0..(wrench.len() - 1) {
                        draw_line(p(wrench[i].0, wrench[i].1), p(wrench[i + 1].0, wrench[i + 1].1), stroke, color, window);
                    }
                    draw_dot(p(17.5, 6.5), px(0.7 * scale), color, window);
                }
                FolderIconKind::Network => {
                    // Authentic 3-Node Branch Tree matching Tauri SVG:
                    // <circle cx="12" cy="5" r="2.5"/> <circle cx="5" cy="19" r="2.5"/> <circle cx="19" cy="19" r="2.5"/>
                    // <path d="M12 7.5V12 M12 12L5.5 16.8 M12 12l6.5 4.8"/>
                    draw_circle_stroke(p(12.0, 5.0), px(2.5 * scale), stroke, color, window);
                    draw_dot(p(12.0, 5.0), px(0.7 * scale), color, window);
                    draw_circle_stroke(p(5.0, 19.0), px(2.5 * scale), stroke, color, window);
                    draw_dot(p(5.0, 19.0), px(0.7 * scale), color, window);
                    draw_circle_stroke(p(19.0, 19.0), px(2.5 * scale), stroke, color, window);
                    draw_dot(p(19.0, 19.0), px(0.7 * scale), color, window);
                    draw_line(p(12.0, 7.5), p(12.0, 12.0), stroke, color, window);
                    draw_line(p(12.0, 12.0), p(6.6, 17.2), stroke, color, window);
                    draw_line(p(12.0, 12.0), p(17.4, 17.2), stroke, color, window);
                }
                FolderIconKind::DefaultFolder => {
                    let w = px(20.0);
                    let h = px(15.0);
                    let left = bounds.origin.x + px(4.0);
                    let top = bounds.origin.y + px(6.0);
                    draw_stroked_rounded_rect(point(left, top), w, h, px(3.0), stroke, color, window);
                    draw_line(point(left, top + px(3.0)), point(left + w * 0.45, top + px(3.0)), stroke, color, window);
                }
            }
        },
    )
    .size(size)
}

fn draw_line(
    from: Point<Pixels>,
    to: Point<Pixels>,
    thickness: Pixels,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let dx = (to.x - from.x).to_f64() as f32;
    let dy = (to.y - from.y).to_f64() as f32;
    let thick = thickness.to_f64() as f32;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        return;
    }
    let nx = -dy / len * (thick / 2.0);
    let ny = dx / len * (thick / 2.0);

    let p1 = point(from.x + px(nx), from.y + px(ny));
    let p2 = point(from.x - px(nx), from.y - px(ny));
    let p3 = point(to.x + px(nx), to.y + px(ny));
    let p4 = point(to.x - px(nx), to.y - px(ny));

    let mut path = Path::new(from);
    let st = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));
    path.push_triangle((p1, p2, p3), st);
    path.push_triangle((p2, p4, p3), st);
    window.paint_path(path, color);
}

fn draw_dot(
    center: Point<Pixels>,
    radius: Pixels,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let steps = 12;
    let mut path = Path::new(center);
    let st = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));
    for i in 0..steps {
        let a1 = std::f32::consts::TAU * (i as f32 / steps as f32);
        let a2 = std::f32::consts::TAU * ((i + 1) as f32 / steps as f32);
        let p1 = point(center.x + radius * a1.cos(), center.y + radius * a1.sin());
        let p2 = point(center.x + radius * a2.cos(), center.y + radius * a2.sin());
        path.push_triangle((center, p1, p2), st);
    }
    window.paint_path(path, color);
}

fn draw_circle_stroke(
    center: Point<Pixels>,
    radius: Pixels,
    thickness: Pixels,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let r_out = radius + thickness / 2.0;
    let r_in = radius - thickness / 2.0;
    let steps = 24;
    let mut path = Path::new(center);
    let st = (point(0.0, 1.0), point(0.0, 1.0), point(0.0, 1.0));

    for i in 0..steps {
        let a1 = std::f32::consts::TAU * (i as f32 / steps as f32);
        let a2 = std::f32::consts::TAU * ((i + 1) as f32 / steps as f32);
        let p1 = point(center.x + r_out * a1.cos(), center.y + r_out * a1.sin());
        let p2 = point(center.x + r_in * a1.cos(), center.y + r_in * a1.sin());
        let p3 = point(center.x + r_out * a2.cos(), center.y + r_out * a2.sin());
        let p4 = point(center.x + r_in * a2.cos(), center.y + r_in * a2.sin());
        path.push_triangle((p1, p2, p3), st);
        path.push_triangle((p2, p4, p3), st);
    }
    window.paint_path(path, color);
}

fn draw_stroked_rounded_rect(
    origin: Point<Pixels>,
    width: Pixels,
    height: Pixels,
    _radius: Pixels,
    thickness: Pixels,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let top_left = origin;
    let top_right = point(origin.x + width, origin.y);
    let bottom_left = point(origin.x, origin.y + height);
    let bottom_right = point(origin.x + width, origin.y + height);

    draw_line(top_left, top_right, thickness, color, window);
    draw_line(top_right, bottom_right, thickness, color, window);
    draw_line(bottom_right, bottom_left, thickness, color, window);
    draw_line(bottom_left, top_left, thickness, color, window);
}
