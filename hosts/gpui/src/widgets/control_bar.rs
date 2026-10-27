use gpui::{
    div, prelude::*, px, rgb, rgba, Context, IntoElement, MouseButton, Render, Window,
};

use crate::widgets::DesktopWidgetContent;

pub struct ControlBarWidget;

impl ControlBarWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl DesktopWidgetContent for ControlBarWidget {
    fn id(&self) -> &'static str {
        "control-bar"
    }
}

impl Render for ControlBarWidget {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(control_btn("☰"))
            .child(control_btn("✥"))
            .child(control_btn("↺"))
            .child(control_btn("✕"))
    }
}

fn control_btn(symbol: &'static str) -> impl IntoElement {
    div()
        .size(px(36.0))
        .rounded_xl()
        .bg(rgb(0x2d323c))
        .border_1()
        .border_color(rgba(0xffffff15))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x3d4450)))
        .child(
            div()
                .text_sm()
                .text_color(rgb(0xffffff))
                .child(symbol),
        )
}
