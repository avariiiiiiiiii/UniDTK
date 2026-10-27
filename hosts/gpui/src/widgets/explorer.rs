use gpui::{
    div, prelude::*, px, rgb, rgba, Context, FontWeight, IntoElement, MouseButton, Render, Window,
};
use windows::core::w;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetLogicalDrives};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

#[derive(Clone, Debug)]
pub struct DriveEntry {
    pub letter: String,
    pub label: String,
    pub used_gb: f32,
    pub total_gb: f32,
}

pub struct ExplorerWidget {
    pub drives: Vec<DriveEntry>,
}

impl ExplorerWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            drives: scan_drives(),
        }
    }

    pub fn refresh_drives(&mut self) {
        self.drives = scan_drives();
    }
}

fn scan_drives() -> Vec<DriveEntry> {
    let mut result = Vec::new();
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26u32 {
            if (mask >> i) & 1 == 1 {
                let letter_char = (b'A' + i as u8) as char;
                let letter_str = letter_char.to_string();
                let path_wide: Vec<u16> = format!("{}:\\", letter_str)
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();

                let mut free_bytes = 0u64;
                let mut total_bytes = 0u64;
                let mut total_free = 0u64;
                let ok = GetDiskFreeSpaceExW(
                    windows::core::PCWSTR(path_wide.as_ptr()),
                    Some(&mut free_bytes),
                    Some(&mut total_bytes),
                    Some(&mut total_free),
                );

                if ok.is_ok() && total_bytes > 0 {
                    let total_gb = (total_bytes as f32) / (1024.0 * 1024.0 * 1024.0);
                    let free_gb = (free_bytes as f32) / (1024.0 * 1024.0 * 1024.0);
                    let used_gb = (total_gb - free_gb).max(0.0);
                    let label = if letter_char == 'C' {
                        "SYSTEM NVME".to_string()
                    } else {
                        "DATA DRIVE".to_string()
                    };

                    result.push(DriveEntry {
                        letter: format!("{}:", letter_char),
                        label,
                        used_gb,
                        total_gb,
                    });
                }
            }
        }
    }

    if result.is_empty() {
        result.push(DriveEntry {
            letter: "C:".to_string(),
            label: "SYSTEM DISK".to_string(),
            used_gb: 218.0,
            total_gb: 512.0,
        });
    }

    result
}

fn launch_explorer(target: &str) {
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            windows::core::PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOW,
        );
    }
}

impl DesktopWidgetContent for ExplorerWidget {
    fn id(&self) -> &'static str {
        "explorer"
    }
}

impl Render for ExplorerWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drives = self.drives.clone();

        div()
            .flex()
            .flex_col()
            .w(px(320.0))
            .p_6()
            .rounded_3xl()
            .bg(theme::bg_glass())
            .border_1()
            .border_color(theme::border_glass())
            .shadow_lg()
            .child(
                // Header: Indicator dot, title, and quick Explorer button
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
                                    .bg(theme::text_primary()),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("STORAGE & FILES"),
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
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, _cx| {
                                    launch_explorer("explorer.exe");
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_primary())
                                    .child("OPEN"),
                            ),
                    ),
            )
            .child(
                // Drive entries with used capacity bars
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .my_2()
                    .children(drives.into_iter().map(|drive| {
                        let letter = drive.letter.clone();
                        let used_ratio = if drive.total_gb > 0.0 {
                            (drive.used_gb / drive.total_gb).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let is_high = used_ratio > 0.85;

                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _event, _window, _cx| {
                                    launch_explorer(&format!("{letter}\\"));
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .font(font_dot())
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::text_primary())
                                            .child(format!("{} {}", drive.letter, drive.label)),
                                    )
                                    .child(
                                        div()
                                            .font(font_dot())
                                            .text_xs()
                                            .text_color(theme::text_secondary())
                                            .child(format!("{:.0} / {:.0} GB", drive.used_gb, drive.total_gb)),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgba(0xffffff20))
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .bg(if is_high {
                                                theme::nothing_red()
                                            } else {
                                                rgba(0xffffffdd)
                                            })
                                            .w(px(used_ratio * 272.0)),
                                    ),
                            )
                    })),
            )
            .child(
                // Quick folders row: Downloads, Documents, Desktop
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
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .hover(|s| s.bg(rgba(0xffffff15)))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, _cx| {
                                    if let Some(user) = std::env::var_os("USERPROFILE") {
                                        let p = format!("{}\\Downloads", user.to_string_lossy());
                                        launch_explorer(&p);
                                    }
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child("DOWNLOADS"),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .hover(|s| s.bg(rgba(0xffffff15)))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, _cx| {
                                    if let Some(user) = std::env::var_os("USERPROFILE") {
                                        let p = format!("{}\\Documents", user.to_string_lossy());
                                        launch_explorer(&p);
                                    }
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child("DOCS"),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .hover(|s| s.bg(rgba(0xffffff15)))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, _cx| {
                                    if let Some(user) = std::env::var_os("USERPROFILE") {
                                        let p = format!("{}\\Desktop", user.to_string_lossy());
                                        launch_explorer(&p);
                                    }
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child("DESKTOP"),
                            ),
                    ),
            )
    }
}
