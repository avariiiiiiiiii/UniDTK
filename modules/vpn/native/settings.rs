//! VPN's live section in the native settings menu (r[shell.widget-contract]): below the
//! declared fields the menu draws itself (the subscriptions, auto-connect;
//! r[modules.settings-contract]), the state of each subscription with the device id this
//! machine sends its provider, and the home server, a record stored through the contract.

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

/// How long "Обновить" waits: every subscription is fetched again (15 s each, at once).
const REFRESH_WAIT: std::time::Duration = std::time::Duration::from_secs(25);

#[derive(Default)]
struct VpnPane {
    load: Load,
    connected: Option<bool>,
    saving: bool,
    /// The home server last acknowledged by the kernel, and its version. The switch only
    /// changes `enabled`.
    saved_home: Value,
    home_version: u64,
    home: [TextEdit; HOME_FIELDS.len()],
    /// `vpn_groups`: per subscription its host, its provider's words, its server count and
    /// the device id this machine sends it, so the person can find this device there.
    groups: Vec<Value>,
    refreshing: bool,
}

enum Change {
    Home {
        config: Value,
        message: &'static str,
    },
}

impl VpnPane {
    fn fill(&mut self, home: &Value) {
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

    fn load_groups(&mut self, act: &mut Act) {
        act.call("vpn_groups", json!({}), |pane: &mut Self, result, _| {
            if let Ok(status) = result {
                pane.groups = status["groups"].as_array().cloned().unwrap_or_default();
            }
        });
    }

    /// "Обновить": every subscription fetched again, then its state read.
    fn refresh(&mut self, act: &mut Act) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        act.call_within("vpn_refresh_servers", json!({}), REFRESH_WAIT, |pane: &mut Self, result, act| {
            pane.refreshing = false;
            if let Err(error) = result {
                act.error(format!("Подписки не обновились: {error}"));
            }
            pane.load_groups(act);
        });
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
        let Change::Home { config, .. } = &change;
        let args = json!({ "module": "vpn", "key": "home_server", "value": config, "expectedVersion": self.home_version });
        act.call("settings_set", args, move |pane: &mut Self, result, act| {
            pane.finish_save(change, result, act)
        });
    }

    fn finish_save(&mut self, change: Change, result: Result<Value, String>, act: &mut Act) {
        self.saving = false;
        let stored = match result {
            Ok(stored) => stored,
            Err(error) => {
                act.error(format!("Не удалось подтвердить сохранение: {error}"));
                return;
            }
        };
        let Change::Home { message, .. } = change;
        // The stored record, normalised by the module; the fields keep what is being typed.
        self.home_version = stored["version"].as_u64().unwrap_or(self.home_version);
        self.saved_home = stored["value"].clone();
        act.toast(message);
    }
}

impl Settings for VpnPane {
    fn load(&mut self, act: &mut Act) {
        self.load = Load::Loading;
        self.connected = None;
        act.call("vpn_status", json!({}), |pane: &mut Self, result, _| {
            pane.connected = result.ok().and_then(|status| status["connected"].as_bool());
        });
        act.call("settings_get", json!({ "module": "vpn" }), |pane: &mut Self, result, _| {
            pane.load = match result {
                Ok(settings) if settings["values"].is_object() => {
                    let home = &settings["values"]["home_server"];
                    pane.home_version = home["version"].as_u64().unwrap_or(0);
                    pane.fill(&home["value"]);
                    Load::Ready
                }
                _ => Load::Failed,
            };
        });
        self.load_groups(act);
    }

    fn sibling_changed(&mut self, key: &str, act: &mut Act) {
        if key == "subscriptions" {
            self.load_groups(act);
        }
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
                    ["vpn-home", "vpn-home-save"]
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
        // Each subscription's state: what its provider said, how many servers it gave, and
        // the device id this machine sends it.
        let mut states = col().gap(6.0).py(10.0)
            .child(row().w_full().items_center().justify_between()
                .child(controls::field_label(look, "Состояние подписок", ""))
                .child(controls::small_button(look, "hit:vpn-refresh".into(), if self.refreshing { "…" } else { "Обновить" }, ButtonKind::Normal, view.on(Self::refresh))));
        if self.groups.is_empty() {
            states = states.child(text("Подписок нет: добавьте ссылку выше.").font(look.font(12.0, 400.0)).color(look.fg.alpha(0.6)).wrap());
        }
        for (index, group) in self.groups.iter().enumerate() {
            let host = group["host"].as_str().unwrap_or_default();
            let title = group["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(host);
            let (state, alarm) = match (group["refusal"].as_str(), group["error"].as_str(), group["fetched"] == true) {
                (Some(refusal), _, _) => (refusal.to_string(), true),
                (None, Some(error), _) => (error.to_string(), true),
                (None, None, true) => (format!("серверов: {}", group["servers"].as_u64().unwrap_or(0)), false),
                (None, None, false) => ("ещё не загружена".to_string(), false),
            };
            states = states.child(
                col().key(format!("vpn-group-state:{index}")).w_full().gap(2.0).px(10.0).py(8.0).radius(9.0).fill(look.well.alpha(0.18))
                    .child(text(title.to_string()).font(look.font(13.0, 500.0)).color(look.fg).single_line().truncate())
                    .child(text(state).font(look.font(12.0, 400.0)).color(if alarm { look.status_error } else { look.fg.alpha(0.72) }).wrap())
                    .child(text(format!("Идентификатор устройства: {}", group["device_id"].as_str().unwrap_or_default()))
                        .font(look.font(11.0, 400.0)).color(look.fg.alpha(0.6)).wrap_anywhere()),
            );
        }
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
            .child(states)
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
                            matches!(*id, "uuid" | "public_key" | "short_id"),
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

/// Enter submits the form, as on the page. A `secret` field (the subscription link, the
/// home server's id and keys) shows its text but keeps it out of the element tree, which
/// `udtk host tree` reads on the socket; such a field does not copy or undo.
#[allow(clippy::too_many_arguments)]
fn input(
    view: &View<'_>,
    key: String,
    label: &str,
    edit: &TextEdit,
    placeholder: &str,
    secret: bool,
    field: impl Fn(&mut VpnPane) -> &mut TextEdit + 'static,
    submit: fn(&mut VpnPane, &mut Act),
) -> El {
    let look = view.look;
    let mut text_field = controls::text_field(edit, look.font(13.0, 400.0), look.fg)
        .single_line()
        .key(key)
        .label(label.to_string())
        .placeholder(placeholder.to_string(), look.fg.alpha(0.55))
        .on_edit(view.edit(field))
        .on_submit(view.on(submit));
    if secret {
        text_field = text_field.secure().revealed(true);
    }
    el().w_full()
        .min_h(32.0)
        .px(10.0)
        .py(6.0)
        .radius(9.0)
        .border(1.0, look.elev.alpha(0.16))
        .border_layout()
        .fill(look.well.alpha(0.25))
        .child(text_field)
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
        pane.fill(&json!({}));
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
        pane.fill(&json!({ "enabled": true, "port": 8443, "host": "example.test" }));
        pane.home[1] = TextEdit::new("65536");
        let Change::Home { config, .. } = pane.home_toggle();
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
        pane.finish_save(
            pane.home_toggle(),
            Ok(json!({ "version": 2, "value": { "enabled": false, "port": 8443, "host": "example.test" } })),
            &mut act,
        );
        assert_eq!(pane.home_version, 2);
        assert!(!pane.home_enabled());
        assert_eq!(pane.home[1].text, "65536");
    }
}
