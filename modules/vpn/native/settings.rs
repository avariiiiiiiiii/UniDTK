//! VPN's pane in the native settings menu (r[shell.widget-contract]).

use std::rc::Rc;

use rune::el::{col, el, row, text, El};
use rune::TextEdit;
use serde_json::{json, Value};
use unidtk_shell::controls::{self, ButtonKind, Look};
use unidtk_shell::settings::{Act, Settings, View};

const HOME_FIELDS: [(&str, &str, &str); 7] = [
    ("host", "Адрес сервера", "IP или домен"),
    ("port", "Порт", "443"),
    ("uuid", "UUID пользователя", "xxxxxxxx-xxxx-…"),
    ("public_key", "Открытый ключ", "Reality public key"),
    ("short_id", "Короткий ID", "Reality short id"),
    ("sni", "Имя сервера (SNI)", "example.com"),
    ("subnet", "Домашняя подсеть", DEFAULT_SUBNET),
];
const DEFAULT_PORT: u16 = 443;
const DEFAULT_SUBNET: &str = "10.10.0.0/16";

pub(super) fn make() -> Box<dyn Settings> {
    Box::new(VpnPane::default())
}

#[derive(Default, PartialEq, Eq)]
enum Load {
    #[default]
    Loading,
    Failed,
    Ready,
}

#[derive(Default)]
struct VpnPane {
    load: Load,
    connected: Option<bool>,
    saving: bool,
    subscription: TextEdit,
    auto_connect: bool,
    /// The settings last acknowledged by the kernel. The switch only changes `enabled`.
    saved_home: Value,
    home: [TextEdit; HOME_FIELDS.len()],
}

enum Change {
    Subscription(String),
    AutoConnect(bool),
    Home {
        config: Value,
        message: &'static str,
    },
}

impl VpnPane {
    fn fill(&mut self, settings: &Value) {
        self.subscription =
            TextEdit::new(settings["subscription_url"].as_str().unwrap_or_default());
        self.auto_connect = settings["auto_connect"] == true;
        let home = &settings["home_server"];
        let port = home["port"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|&n| n > 0);
        let subnet = home["subnet"].as_str().filter(|s| !s.is_empty());
        self.saved_home = home
            .as_object()
            .cloned()
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        self.saved_home["port"] = json!(port.unwrap_or(DEFAULT_PORT));
        self.saved_home["subnet"] = json!(subnet.unwrap_or(DEFAULT_SUBNET));
        self.home = HOME_FIELDS.map(|(id, ..)| {
            if id == "port" {
                TextEdit::new(port.unwrap_or(DEFAULT_PORT).to_string())
            } else {
                TextEdit::new(self.saved_home[id].as_str().unwrap_or_default())
            }
        });
    }

    fn home_enabled(&self) -> bool {
        self.saved_home["enabled"] == true
    }

    fn home_config(&self) -> Result<Value, &'static str> {
        let field = |i: usize| self.home[i].text.trim();
        let port = if field(1).is_empty() {
            DEFAULT_PORT
        } else {
            field(1)
                .parse::<u16>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or("Порт должен быть числом от 1 до 65535")?
        };
        let subnet = if field(6).is_empty() {
            DEFAULT_SUBNET
        } else {
            field(6)
        };
        Ok(json!({
            "enabled": self.home_enabled(),
            "host": field(0),
            "port": port,
            "uuid": field(2),
            "public_key": field(3),
            "short_id": field(4),
            "sni": field(5),
            "subnet": subnet,
        }))
    }

    fn home_toggle(&self) -> Change {
        let enabled = !self.home_enabled();
        let mut config = self.saved_home.clone();
        config["enabled"] = json!(enabled);
        let message = if enabled {
            "Домашний сервер включён"
        } else {
            "Домашний сервер выключен"
        };
        Change::Home { config, message }
    }

    fn save_subscription(&mut self, act: &mut Act) {
        self.save(
            Change::Subscription(self.subscription.text.trim().to_string()),
            act,
        );
    }

    fn save_home(&mut self, act: &mut Act) {
        if self.saving || !self.home_enabled() {
            return;
        }
        match self.home_config() {
            Ok(config) => self.save(
                Change::Home {
                    config,
                    message: "Настройки сервера сохранены",
                },
                act,
            ),
            Err(error) => act.error(error),
        }
    }

    fn save(&mut self, change: Change, act: &mut Act) {
        // Enter submits the text input even if its save button is disabled.
        if self.load != Load::Ready || self.saving {
            return;
        }
        self.saving = true;
        let (cmd, args) = match &change {
            Change::Subscription(url) => ("vpn_set_subscription_url", json!({ "url": url })),
            Change::AutoConnect(enabled) => ("vpn_set_auto_connect", json!({ "enabled": enabled })),
            Change::Home { config, .. } => {
                ("vpn_set_home_server_config", json!({ "config": config }))
            }
        };
        act.call(cmd, args, move |pane: &mut Self, result, act| {
            pane.finish_save(change, result, act)
        });
    }

    fn finish_save(&mut self, change: Change, result: Result<Value, String>, act: &mut Act) {
        self.saving = false;
        if let Err(error) = result {
            act.error(format!("Не удалось подтвердить сохранение: {error}"));
            return;
        }
        match change {
            // The backend invalidates its cache here. A later list request fetches the servers.
            Change::Subscription(_) => act.toast("Подписка сохранена"),
            Change::AutoConnect(enabled) => self.auto_connect = enabled,
            Change::Home { config, message } => {
                self.saved_home = config;
                act.toast(message);
            }
        }
    }
}

impl Settings for VpnPane {
    fn load(&mut self, act: &mut Act) {
        self.load = Load::Loading;
        self.connected = None;
        act.call("vpn_status", json!({}), |pane: &mut Self, result, _| {
            pane.connected = result.ok().and_then(|status| status["connected"].as_bool());
        });
        act.call("vpn_settings", json!({}), |pane: &mut Self, result, _| {
            pane.load = match result {
                Ok(settings) if settings.is_object() => {
                    pane.fill(&settings);
                    Load::Ready
                }
                _ => Load::Failed,
            };
        });
    }

    fn pane(&self, view: &View<'_>) -> El {
        let look = view.look;
        match self.load {
            Load::Loading => {
                return el().px(4.0).py(16.0).child(
                    text("Загрузка…")
                        .font(look.font(13.0, 400.0))
                        .color(look.fg.alpha(0.78)),
                );
            }
            Load::Failed => {
                return col()
                    .px(4.0)
                    .py(16.0)
                    .gap(10.0)
                    .child(
                        text("Не удалось загрузить настройки VPN.")
                            .font(look.font(13.0, 400.0))
                            .color(look.status_error)
                            .wrap(),
                    )
                    .child(row().child(controls::button(
                        look,
                        "hit:vpn-retry".into(),
                        "Повторить",
                        ButtonKind::Normal,
                        view.on(Self::load),
                    )));
            }
            Load::Ready => {}
        }
        let pending_look;
        let look = if self.saving {
            pending_look = Look {
                busy: Rc::new(
                    ["vpn-sub-save", "vpn-auto", "vpn-home", "vpn-home-save"]
                        .map(str::to_owned)
                        .into(),
                ),
                ..look.clone()
            };
            &pending_look
        } else {
            look
        };
        let status_label = match self.connected {
            None => "Состояние неизвестно",
            Some(true) => "Подключено",
            Some(false) => "Отключено",
        };
        let status_colour = if self.connected == Some(true) {
            look.status_ok
        } else {
            look.fg.alpha(0.75)
        };
        let status_pill = el()
            .px(8.0)
            .py(2.0)
            .radius(999.0)
            .fill(look.elev.alpha(0.1))
            .child(
                text(status_label)
                    .font(look.font(11.0, 500.0))
                    .color(status_colour)
                    .single_line(),
            );
        let subscription = input(
            view,
            "hit:vpn-sub".into(),
            "Ссылка на подписку",
            &self.subscription,
            "https://…",
            |pane| &mut pane.subscription,
            Self::save_subscription,
        );
        let auto = controls::switch(
            look,
            "hit:vpn-auto".into(),
            self.auto_connect,
            view.on(|pane: &mut Self, act| pane.save(Change::AutoConnect(!pane.auto_connect), act)),
        )
        .role("switch")
        .label("Подключаться при запуске")
        .checked(self.auto_connect);
        let home_on = self.home_enabled();
        let home = controls::switch(
            look,
            "hit:vpn-home".into(),
            home_on,
            view.on(|pane: &mut Self, act| pane.save(pane.home_toggle(), act)),
        )
        .role("switch")
        .label("Прямое подключение к своему серверу")
        .checked(home_on);
        let mut pane = col()
            .w_full()
            .child(
                row()
                    .items_center()
                    .gap(8.0)
                    .child(controls::heading(look, "VPN"))
                    .child(status_pill),
            )
            .child(
                col()
                    .gap(6.0)
                    .py(10.0)
                    .child(controls::field_label(look, "Ссылка на подписку", ""))
                    .child(subscription)
                    .child(row().child(controls::button(
                        look,
                        "hit:vpn-sub-save".into(),
                        "Сохранить подписку",
                        ButtonKind::Primary,
                        view.on(Self::save_subscription),
                    ))),
            )
            .child(switch_row(
                look,
                "Подключаться при запуске",
                "VPN включится сам, когда запустится UniDTK",
                auto,
            ))
            .child(controls::heading(look, "Домашний сервер"))
            .child(switch_row(
                look,
                "Прямое подключение к своему серверу",
                "Отдельно от подписки, протокол VLESS Reality",
                home,
            ));
        if home_on {
            let mut form = col().gap(8.0).py(8.0);
            for (index, (id, label, placeholder)) in HOME_FIELDS.iter().enumerate() {
                form = form.child(
                    col()
                        .gap(6.0)
                        .pt(10.0)
                        .child(controls::field_label(look, label, ""))
                        .child(input(
                            view,
                            format!("hit:vpn-home-{id}"),
                            label,
                            &self.home[index],
                            placeholder,
                            move |pane| &mut pane.home[index],
                            Self::save_home,
                        )),
                );
            }
            form = form.child(row().child(controls::button(
                look,
                "hit:vpn-home-save".into(),
                "Сохранить сервер",
                ButtonKind::Primary,
                view.on(Self::save_home),
            )));
            pane = pane.child(form);
        }
        pane
    }
}

/// Enter submits the form, as on the page.
fn input(
    view: &View<'_>,
    key: String,
    label: &str,
    edit: &TextEdit,
    placeholder: &str,
    field: impl Fn(&mut VpnPane) -> &mut TextEdit + 'static,
    submit: fn(&mut VpnPane, &mut Act),
) -> El {
    let look = view.look;
    el().w_full()
        .min_h(32.0)
        .px(10.0)
        .py(6.0)
        .radius(9.0)
        .border(1.0, look.elev.alpha(0.16))
        .border_layout()
        .fill(look.well.alpha(0.25))
        .child(
            controls::text_field(edit, look.font(13.0, 400.0), look.fg)
                .single_line()
                .key(key)
                .label(label.to_string())
                .placeholder(placeholder.to_string(), look.fg.alpha(0.55))
                .on_edit(view.edit(field))
                .on_submit(view.on(submit)),
        )
}

fn switch_row(look: &Look, name: &str, sub: &str, switch: El) -> El {
    row()
        .w_full()
        .min_h(44.0)
        .px(8.0)
        .py(6.0)
        .gap(10.0)
        .radius(12.0)
        .items_center()
        .hover(look.elev.alpha(0.07))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(name.to_string())
                        .font(look.font(13.0, 400.0))
                        .color(look.fg)
                        .single_line(),
                )
                .child(
                    text(sub.to_string())
                        .font(look.font(12.0, 400.0))
                        .color(look.fg.alpha(0.72))
                        .single_line(),
                ),
        )
        .child(switch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_home_server_gets_the_pages_defaults() {
        let mut pane = VpnPane::default();
        pane.fill(&json!({ "subscription_url": "https://x", "auto_connect": true }));
        assert_eq!(pane.subscription.text, "https://x");
        assert!(pane.auto_connect);
        assert!(!pane.home_enabled());
        let config = pane.home_config().unwrap();
        assert_eq!(config["port"], 443);
        assert_eq!(config["subnet"], "10.10.0.0/16");
    }

    #[test]
    fn home_port_accepts_the_tcp_range_and_defaults_only_when_blank() {
        let mut pane = VpnPane::default();
        for (text, port) in [("  ", 443), ("1", 1), ("65535", 65535)] {
            pane.home[1] = TextEdit::new(text);
            assert_eq!(pane.home_config().unwrap()["port"], port);
        }
        for text in ["0", "65536", "abc", "-1"] {
            pane.home[1] = TextEdit::new(text);
            assert!(pane.home_config().is_err(), "{text}");
        }
    }

    #[test]
    fn home_switch_saves_confirmed_fields_and_waits_for_acknowledgement() {
        let mut pane = VpnPane {
            load: Load::Ready,
            ..VpnPane::default()
        };
        pane.fill(
            &json!({ "home_server": { "enabled": true, "port": 8443, "host": "example.test" } }),
        );
        pane.home[1] = TextEdit::new("65536");
        let Change::Home { config, .. } = pane.home_toggle() else {
            panic!("expected home switch")
        };
        assert_eq!(config["enabled"], false);
        assert_eq!(config["port"], 8443);
        assert_eq!(config["host"], "example.test");

        let mut act = Act::default();
        pane.save(pane.home_toggle(), &mut act);
        assert!(pane.saving);
        assert!(pane.home_enabled());
        pane.finish_save(pane.home_toggle(), Err("rejected".into()), &mut act);
        assert!(!pane.saving);
        assert!(pane.home_enabled());
        pane.finish_save(pane.home_toggle(), Ok(Value::Null), &mut act);
        assert!(!pane.home_enabled());
        assert_eq!(pane.home[1].text, "65536");
    }
}
