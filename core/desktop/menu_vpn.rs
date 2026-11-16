//! The VPN view of the menu (`renderVpn` in `menu.js`), reached from the VPN widget's sheet:
//! the subscription link, connecting at start, and the home server (VLESS Reality). Without
//! the stored settings the form stays hidden, so a save cannot overwrite the subscription with
//! blanks.

use rune::el::{col, el, row, text, text_input, El};
use rune::{TextEdit, Tx};
use serde_json::{json, Value};

use crate::controls::{self, ButtonKind, Look};
use crate::menu::{self, Loaded, MenuHost, View};

/// The home server form: field id, label, placeholder.
const HOME_FIELDS: [(&str, &str, &str); 7] = [
    ("host", "Адрес сервера", "IP или домен"),
    ("port", "Порт", "443"),
    ("uuid", "UUID пользователя", "xxxxxxxx-xxxx-…"),
    ("public_key", "Открытый ключ", "Reality public key"),
    ("short_id", "Короткий ID", "Reality short id"),
    ("sni", "Имя сервера (SNI)", "example.com"),
    ("subnet", "Домашняя подсеть", "10.10.0.0/16"),
];

#[derive(Default)]
pub struct VpnPane {
    pub settings: Option<Loaded<Value>>,
    /// `vpn_status` is allowed to fail: the label says the state is unknown.
    pub status: Option<Value>,
    pub subscription: TextEdit,
    pub auto_connect: bool,
    pub home_enabled: bool,
    pub home: Vec<TextEdit>,
}

impl VpnPane {
    fn fill(&mut self, settings: &Value) {
        self.subscription = TextEdit::new(settings["subscription_url"].as_str().unwrap_or_default());
        self.auto_connect = settings["auto_connect"].as_bool().unwrap_or(false);
        let home = settings.get("home_server").filter(|h| h.is_object()).cloned().unwrap_or_else(|| json!({
            "enabled": false, "host": "", "port": 443, "uuid": "", "public_key": "", "short_id": "", "sni": "", "subnet": "10.10.0.0/16",
        }));
        self.home_enabled = home["enabled"].as_bool().unwrap_or(false);
        self.home = HOME_FIELDS.iter().map(|(id, ..)| {
            let value = match (*id, &home[*id]) {
                ("port", v) => v.as_u64().filter(|p| *p > 0).unwrap_or(443).to_string(),
                ("subnet", v) => v.as_str().filter(|s| !s.is_empty()).unwrap_or("10.10.0.0/16").to_string(),
                (_, v) => v.as_str().unwrap_or_default().to_string(),
            };
            TextEdit::new(value)
        }).collect();
    }

    /// `saveHome`'s config.
    fn home_config(&self) -> Value {
        let field = |i: usize| self.home.get(i).map(|e| e.text.trim().to_string()).unwrap_or_default();
        json!({
            "enabled": self.home_enabled,
            "host": field(0),
            "port": field(1).parse::<u64>().ok().filter(|p| *p > 0).unwrap_or(443),
            "uuid": field(2),
            "public_key": field(3),
            "short_id": field(4),
            "sni": field(5),
            "subnet": Some(field(6)).filter(|s| !s.is_empty()).unwrap_or_else(|| "10.10.0.0/16".into()),
        })
    }
}

pub fn load<S: MenuHost>(state: &mut S, _tx: &mut Tx) {
    state.menu().vpn.settings = Some(Loaded::Loading);
    state.menu().vpn.status = None;
    state.call("vpn_status", json!({}), Box::new(|state: &mut S, result, _tx: &mut Tx| {
        state.menu().vpn.status = result.ok();
    }));
    state.call("vpn_settings", json!({}), Box::new(|state: &mut S, result, _tx: &mut Tx| {
        let pane = &mut state.menu().vpn;
        match result {
            Ok(settings) => {
                pane.fill(&settings);
                pane.settings = Some(Loaded::Ready(settings));
            }
            Err(error) => pane.settings = Some(Loaded::Failed(error)),
        }
    }));
}

fn save_home<S: MenuHost>(state: &mut S, key: &str, toast: &'static str) {
    let config = state.menu_ref().vpn.home_config();
    menu::run_once(state, key, "vpn_set_home_server_config", json!({ "config": config }), move |state: &mut S, _, _| {
        state.toasts().show(toast, false, None);
    });
}

fn input<S: MenuHost>(look: &Look, key: String, edit: &TextEdit, placeholder: &str, field: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> El {
    el().w_full().min_h(32.0).px(10.0).py(6.0).radius(9.0)
        .border(1.0, look.elev.alpha(0.16)).fill(look.well.alpha(0.25))
        .child(text_input(edit, look.font(13.0, 400.0)).single_line().key(key).placeholder(placeholder.to_string(), look.fg.alpha(0.55)).on_edit(field))
}

pub fn pane<S: MenuHost>(state: &S, look: &Look) -> El {
    let chevron = [([15.0, 18.0], [9.0, 12.0]), ([9.0, 12.0], [15.0, 6.0])];
    let back = row().key("hit:vpn-back").min_h(32.0).mt(4.0).pl(4.0).pr(8.0).py(4.0).gap(4.0).radius(9.0).items_center()
        .hover(look.elev.alpha(0.1))
        .child(el().size(14.0, 14.0).shape(controls::glyph(&chevron, look.fg.alpha(0.85), 2.0)))
        .child(text("Стол").font(look.font(13.0, 400.0)).color(look.fg.alpha(0.85)).single_line())
        .on_press_tx(|state: &mut S, tx: &mut Tx| menu::show_view(state, View::Desk, tx));
    let mut pane = col().w_full().child(row().child(back));
    let pane_state = &state.menu_ref().vpn;
    match &pane_state.settings {
        None | Some(Loaded::Loading) => {
            return pane.child(el().px(4.0).py(16.0).child(text("Загрузка…").font(look.font(13.0, 400.0)).color(look.fg.alpha(0.78))));
        }
        Some(Loaded::Failed(_)) => {
            return pane.child(
                col().px(4.0).py(16.0).gap(10.0)
                    .child(text("Не удалось загрузить настройки VPN. Форма скрыта, чтобы не затереть сохранённую подписку.").font(look.font(13.0, 400.0).lh(13.0 * 1.5)).color(look.status_error).wrap())
                    .child(row().child(controls::button(look, "hit:vpn-retry".into(), "Повторить", ButtonKind::Normal, |state: &mut S, tx: &mut Tx| load(state, tx)))),
            );
        }
        Some(Loaded::Ready(_)) => {}
    }
    let connected = pane_state.status.as_ref().and_then(|s| s["connected"].as_bool());
    let status_label = match connected {
        None => "Состояние неизвестно",
        Some(true) => "Подключено",
        Some(false) => "Отключено",
    };
    let status_pill = el().px(8.0).py(2.0).radius(999.0).fill(look.elev.alpha(0.1))
        .child(text(status_label).font(look.font(11.0, 500.0)).color(if connected == Some(true) { look.status_ok } else { look.fg.alpha(0.75) }).single_line());
    pane = pane.child(row().items_center().gap(8.0).child(controls::heading(look, "VPN")).child(status_pill));

    pane = pane.child(
        col().gap(8.0).py(8.0)
            .child(controls::field_label(look, "Ссылка на подписку", ""))
            .child(input(look, "hit:vpn-sub".into(), &pane_state.subscription, "https://…", |state: &mut S| &mut state.menu().vpn.subscription))
            .child(row().child(controls::button(look, "hit:vpn-sub-save".into(), "Сохранить подписку", ButtonKind::Primary, |state: &mut S, _tx: &mut Tx| {
                let url = state.menu_ref().vpn.subscription.text.trim().to_string();
                menu::run_once(state, "vpn-sub", "vpn_set_subscription_url", json!({ "url": url }), |state: &mut S, _, _| {
                    state.toasts().show("Подписка сохранена, список серверов обновлён", false, None);
                });
            }))),
    );

    let auto = pane_state.auto_connect;
    pane = pane.child(switch_row(look, "Подключаться при запуске", "VPN включится сам, когда запустится UniDTK", controls::switch(look, "hit:vpn-auto".into(), auto, move |state: &mut S, _tx: &mut Tx| {
        let next = !state.menu_ref().vpn.auto_connect;
        state.menu().vpn.auto_connect = next;
        state.call("vpn_set_auto_connect", json!({ "enabled": next }), Box::new(move |state: &mut S, result, _tx: &mut Tx| {
            if let Err(error) = result {
                state.menu().vpn.auto_connect = !next;
                menu::error_toast(state, &format!("Автоподключение не сохранено: {error}"));
            }
        }));
    })));

    pane = pane.child(controls::heading(look, "Домашний сервер"));
    let home_on = pane_state.home_enabled;
    pane = pane.child(switch_row(look, "Прямое подключение к своему серверу", "Отдельно от подписки, протокол VLESS Reality", controls::switch(look, "hit:vpn-home".into(), home_on, |state: &mut S, _tx: &mut Tx| {
        // The switch saves by itself: turning the server off must not wait for a second
        // click on a form that is now hidden.
        let next = !state.menu_ref().vpn.home_enabled;
        state.menu().vpn.home_enabled = next;
        save_home(state, "vpn-home-toggle", if next { "Домашний сервер включён" } else { "Домашний сервер выключен" });
    })));
    if home_on {
        let mut form = col().gap(8.0).py(8.0);
        for (index, (id, label, placeholder)) in HOME_FIELDS.iter().enumerate() {
            form = form.child(
                col().gap(6.0).py(10.0)
                    .child(controls::field_label(look, label, ""))
                    .child(input(look, format!("hit:vpn-home-{id}"), &pane_state.home[index], placeholder, move |state: &mut S| &mut state.menu().vpn.home[index])),
            );
        }
        form = form.child(row().child(controls::button(look, "hit:vpn-home-save".into(), "Сохранить сервер", ButtonKind::Primary, |state: &mut S, _tx: &mut Tx| {
            save_home(state, "vpn-home-save", "Настройки сервера сохранены");
        })));
        pane = pane.child(form);
    }
    pane
}

/// `.menu-row` with a name, a sub-line and a switch.
fn switch_row(look: &Look, name: &str, sub: &str, switch: El) -> El {
    row().w_full().min_h(44.0).px(8.0).py(6.0).gap(10.0).radius(12.0).items_center().hover(look.elev.alpha(0.07))
        .child(
            col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(name.to_string()).font(look.font(13.0, 400.0)).color(look.fg).single_line())
                .child(text(sub.to_string()).font(look.font(12.0, 400.0)).color(look.fg.alpha(0.72)).single_line()),
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
        assert_eq!((pane.subscription.text.as_str(), pane.auto_connect, pane.home_enabled), ("https://x", true, false));
        assert_eq!(pane.home_config()["port"], 443);
        assert_eq!(pane.home_config()["subnet"], "10.10.0.0/16");
        pane.home[1] = TextEdit::new("abc");
        assert_eq!(pane.home_config()["port"], 443, "a port that is not a number is 443");
    }
}
