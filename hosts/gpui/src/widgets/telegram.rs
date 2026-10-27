use std::os::windows::process::CommandExt;
use std::process::Command;

use gpui::{
    div, prelude::*, px, rgb, rgba, Context, FontWeight, IntoElement, MouseButton, Render, Window,
};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Debug)]
pub struct ChatSnippet {
    pub title: String,
    pub last_message: String,
    pub unread_count: u32,
    pub time_str: String,
}

pub struct TelegramWidget {
    pub connected: bool,
    pub total_unread: u32,
    pub chats: Vec<ChatSnippet>,
}

impl TelegramWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            connected: true,
            total_unread: 4,
            chats: vec![
                ChatSnippet {
                    title: "UniDTK Community".to_string(),
                    last_message: "GPUI transition gate 2 verified!".to_string(),
                    unread_count: 3,
                    time_str: "15:10".to_string(),
                },
                ChatSnippet {
                    title: "Kernel Dev Logs".to_string(),
                    last_message: "Subclass WM_NCHITTEST synchronized".to_string(),
                    unread_count: 1,
                    time_str: "14:48".to_string(),
                },
                ChatSnippet {
                    title: "Nothing OS Discussion".to_string(),
                    last_message: "Typography & glass shaders aligned".to_string(),
                    unread_count: 0,
                    time_str: "13:20".to_string(),
                },
            ],
        }
    }

    pub fn open_telegram_app(&self) {
        let _ = Command::new("cmd")
            .args(["/c", "start", "tg://"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

impl DesktopWidgetContent for TelegramWidget {
    fn id(&self) -> &'static str {
        "telegram"
    }
}

impl Render for TelegramWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_unread = self.total_unread;
        let chats = self.chats.clone();

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
                // Header: Indicator dot, title, and Launch button
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
                                    .bg(theme::accent_green()),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("TELEGRAM"),
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
                                cx.listener(|this, _event, _window, _cx| {
                                    this.open_telegram_app();
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text_primary())
                                    .child(if total_unread > 0 {
                                        format!("{total_unread} NEW")
                                    } else {
                                        "READY".to_string()
                                    }),
                            ),
                    ),
            )
            .child(
                // Chat previews list
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .my_1()
                    .children(chats.into_iter().map(|chat| {
                        let has_unread = chat.unread_count > 0;
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .p_2()
                            .rounded_xl()
                            .hover(|s| s.bg(rgba(0xffffff10)))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, _cx| {
                                    this.open_telegram_app();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .font(font_dot())
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::text_primary())
                                            .child(chat.title),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::text_secondary())
                                            .child(chat.last_message),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_end()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font(font_dot())
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(chat.time_str),
                                    )
                                    .child(
                                        if has_unread {
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(theme::nothing_red())
                                                .font(font_dot())
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme::text_primary())
                                                .child(chat.unread_count.to_string())
                                        } else {
                                            div()
                                        },
                                    ),
                            )
                    })),
            )
            .child(
                // Bottom row: TDLib MTProto engine indicator
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border_glass())
                    .child(
                        div()
                            .font(font_dot())
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child("TDLIB MTPROTO"),
                    )
                    .child(
                        div()
                            .font(font_dot())
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::accent_green())
                            .child("DESYNC ACTIVE"),
                    ),
            )
    }
}
