use std::os::windows::process::CommandExt;
use std::process::Command;

use gpui::{
    div, prelude::*, px, rgba, Context, FontWeight, IntoElement, MouseButton, Render, Window,
};
use unidtk_mod_vpn::is_connected;

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct VpnWidget {
    pub connected: bool,
}

impl VpnWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            connected: is_connected(),
        }
    }

    /// Syncs tunnel status with Windows system network state. Returns true if changed.
    pub fn sync_from_system(&mut self) -> bool {
        let current = is_connected();
        if self.connected != current {
            self.connected = current;
            return true;
        }
        false
    }

    pub fn toggle_connection(&mut self) {
        let target_task = if self.connected {
            "UniDTKVpnStop"
        } else {
            "UniDTKSingbox"
        };

        let _ = Command::new("schtasks")
            .args(["/run", "/tn", target_task])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();

        // Optimistically update or wait for next poll
        self.connected = !self.connected;
    }
}

impl DesktopWidgetContent for VpnWidget {
    fn id(&self) -> &'static str {
        "vpn"
    }
}

impl Render for VpnWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = self.connected;

        div()
            .flex()
            .flex_col()
            .w(px(220.0))
            .p_6()
            .rounded_3xl()
            .bg(theme::bg_glass())
            .border_1()
            .border_color(theme::border_glass())
            .shadow_lg()
            .child(
                // Header: Status indicator dot and title
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
                                // Status Dot: Green if connected, Red if offline
                                div()
                                    .size_2()
                                    .rounded_full()
                                    .bg(if connected {
                                        theme::accent_green()
                                    } else {
                                        theme::nothing_red()
                                    }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("VPN"),
                            ),
                    )
                    .child(
                        // Action Pill Button (CONNECT / DISCONNECT)
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(if connected {
                                rgba(0x00e67626)
                            } else {
                                rgba(0xffffff15)
                            })
                            .border_1()
                            .border_color(if connected {
                                theme::accent_green()
                            } else {
                                theme::border_glass()
                            })
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_connection();
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if connected {
                                        theme::accent_green()
                                    } else {
                                        theme::text_primary()
                                    })
                                    .child(if connected {
                                        "ON"
                                    } else {
                                        "OFF"
                                    }),
                            ),
                    ),
            )
            .child(
                // Middle row: Big Status Display in Nothing OS typography
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .my_1()
                    .child(
                        div()
                            .font(font_dot())
                            .text_size(px(26.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if connected {
                                theme::text_primary()
                            } else {
                                theme::text_muted()
                            })
                            .child(if connected {
                                "SECURE"
                            } else {
                                "OFFLINE"
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(if connected {
                                "TUNNEL ENCRYPTED"
                            } else {
                                "DIRECT CONNECTION"
                            }),
                    ),
            )
            .child(
                // Bottom row: Tunnel Engine Info
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
                            .child("SING-BOX TUN"),
                    )
                    .child(
                        div()
                            .font(font_dot())
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if connected {
                                theme::accent_green()
                            } else {
                                theme::text_muted()
                            })
                            .child(if connected {
                                "ACTIVE"
                            } else {
                                "STANDBY"
                            }),
                    ),
            )
    }
}
