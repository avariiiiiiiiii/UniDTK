use gpui::{
    div, prelude::*, px, rgb, rgba, Context, FontWeight, IntoElement, MouseButton, Render, Window,
};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

#[derive(Clone, Debug)]
pub struct RailContact {
    pub initials: &'static str,
    pub unread: u32,
    pub bg_color: u32,
}

pub struct TelegramRailWidget {
    pub contacts: Vec<RailContact>,
}

impl TelegramRailWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            contacts: vec![
                RailContact { initials: "8", unread: 8, bg_color: 0x3d4450 },
                RailContact { initials: "У1", unread: 0, bg_color: 0x22262e },
                RailContact { initials: "◆ К", unread: 0, bg_color: 0x1f232b },
                RailContact { initials: "TOP", unread: 0, bg_color: 0x4a4035 },
                RailContact { initials: "99", unread: 99, bg_color: 0x3a3f4a },
                RailContact { initials: "99", unread: 99, bg_color: 0x2d323b },
                RailContact { initials: "99", unread: 99, bg_color: 0x3e424c },
                RailContact { initials: "🎓", unread: 0, bg_color: 0x252830 },
                RailContact { initials: "99", unread: 99, bg_color: 0x343842 },
                RailContact { initials: "MIMA", unread: 99, bg_color: 0x484236 },
                RailContact { initials: "99", unread: 99, bg_color: 0x292d36 },
                RailContact { initials: "🌸", unread: 0, bg_color: 0x4a3640 },
            ],
        }
    }
}

impl DesktopWidgetContent for TelegramRailWidget {
    fn id(&self) -> &'static str {
        "telegram-rail"
    }
}

impl Render for TelegramRailWidget {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let red = theme::nothing_red();
        let contacts = self.contacts.clone();

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(16.0))
            .child(
                // Vertical Rail Capsule (56px wide)
                div()
                    .w(px(56.0))
                    .py_3()
                    .px_1p5()
                    .rounded_3xl()
                    .bg(rgb(0x16181e))
                    .border_1()
                    .border_color(rgba(0xffffff15))
                    .shadow_2xl()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        // Top toggle handle bar
                        div()
                            .w(px(20.0))
                            .h(px(3.0))
                            .rounded_full()
                            .bg(rgba(0xffffff40))
                            .mb_1(),
                    )
                    .children(contacts.into_iter().map(|c| {
                        let has_badge = c.unread > 0;
                        div()
                            .relative()
                            .size(px(40.0))
                            .rounded_full()
                            .bg(rgb(c.bg_color))
                            .border_1()
                            .border_color(rgba(0xffffff10))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.border_color(rgba(0xffffff30)))
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xffffff))
                                    .child(c.initials),
                            )
                            .children(if has_badge {
                                Some(
                                    div()
                                        .absolute()
                                        .bottom_neg_0p5()
                                        .right_neg_0p5()
                                        .px_1()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(red)
                                        .border_1()
                                        .border_color(rgb(0x16181e))
                                        .font(font_dot())
                                        .text_size(px(9.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xffffff))
                                        .child(if c.unread > 99 { "99+".to_string() } else { c.unread.to_string() }),
                                )
                            } else {
                                None
                            })
                    })),
            )
            .child(
                // Bottom Red Action Button
                div()
                    .size(px(46.0))
                    .rounded_2xl()
                    .bg(red)
                    .shadow_lg()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.opacity(0.9)),
            )
    }
}
