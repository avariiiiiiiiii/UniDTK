//! VPN's native half (`r[modules.vertical-slice]`): the panel `modules/vpn/ui/vpn-panel.js`
//! and `vpn.css` draw — the state line with its switch, the server picker with its list and
//! "ПРОВЕРИТЬ ВСЕ", and "АВТО" — as a widget of the shell's contract. It draws in its opened
//! card and, compact, in a host's nesting zone (`.nested-body > .dash-vpn` drops the margin,
//! the padding and the background). The server list follows the subscription revision from
//! `vpn_status`, polled every 3 s while the panel is on screen. Writes (`vpn_connect`,
//! `vpn_disconnect`, `vpn_set_auto_connect`) go through the interface door.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rune::el::{col, el, row, text, El};
use rune::{Color, TextStyle};
use serde_json::{json, Value};
use unidtk_shell::widget::{Act, Init, LocalTime, Surface, SurfaceSize, Theme, View, Widget, WidgetType};

mod settings;

pub static WIDGET: WidgetType = WidgetType { id: "vpn", events: &[], make, settings: Some(settings::make) };

fn make(_: &Init<'_>) -> Box<dyn Widget> {
    Box::new(Vpn::default())
}

/// The page's poll interval while a panel is on screen.
const POLL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Ping {
    ok: bool,
    ms: Option<u64>,
}

#[derive(Default)]
struct Vpn {
    subscription_revision: Option<u64>,
    /// The server tags for that revision.
    servers: Option<Vec<String>>,
    loading: bool,
    /// The last read failed: the picker offers "Ошибка подписки · повторить".
    failed: bool,
    /// The hidden `<select>`'s value: the server a connect asks for.
    selected: Option<String>,
    status: Option<Value>,
    busy: bool,
    /// The switch as the person set it while the command runs.
    pending: Option<bool>,
    pinging: bool,
    auto_pending: Option<bool>,
    pings: Option<HashMap<String, Ping>>,
    /// The server list is open.
    menu: bool,
    visible: bool,
    polled: Option<Instant>,
}

/// `repairMojibake`: a UTF-8 tag read as Windows-1251 is turned back, when it decodes cleanly.
pub fn display_text(raw: &str) -> String {
    // WHATWG windows-1251 for 0x80..=0xFF (0x98 is U+0098).
    const HIGH: &str = "ЂЃ‚ѓ„…†‡€‰Љ‹ЊЌЋЏђ‘’“”•–—\u{98}™љ›њќћџ\u{a0}ЎўЈ¤Ґ¦§Ё©Є«¬\u{ad}®Ї°±Ііґµ¶·ё№є»јЅѕїАБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдежзийклмнопрстуфхцчшщъыьэюя";
    let chars: Vec<char> = raw.chars().collect();
    let suspicious = chars.windows(2).any(|pair| matches!(pair[0], 'Р' | 'С') && ('\u{400}'..='\u{4ff}').contains(&pair[1]));
    if !suspicious {
        return raw.to_string();
    }
    let mut bytes = Vec::with_capacity(chars.len());
    for ch in chars {
        let byte = if (ch as u32) < 0x80 { Some(ch as u8) } else { HIGH.chars().position(|c| c == ch).map(|i| 0x80 + i as u8) };
        match byte {
            Some(byte) => bytes.push(byte),
            None => return raw.to_string(),
        }
    }
    match String::from_utf8(bytes) {
        Ok(repaired) if !repaired.contains('\u{fffd}') => repaired,
        _ => raw.to_string(),
    }
}

/// `formatSelectedVpnServer`.
fn server_line(status: &Value) -> String {
    // The refusal comes first: without it the switch just flips back and the
    // panel gives no reason (`#538`). A connect that failed has no selected tag
    // to name, so the bare reason is the whole line.
    if let Some(error) = status_error(status) {
        return match status["selected_tag"].as_str().filter(|t| !t.is_empty()) {
            Some(tag) => format!("{} · {error}", display_text(tag)),
            None => error,
        };
    }
    let Some(tag) = status["selected_tag"].as_str().filter(|t| !t.is_empty()) else { return "—".into() };
    let name = display_text(tag);
    let mut line = if let Some(ms) = status["proxyLatencyMs"].as_f64() {
        format!("{name} · прокси: {ms} мс")
    } else if status["proxyError"].as_str().is_some_and(|e| !e.is_empty()) || status["proxyError"] == true {
        format!("{name} · прокси: ошибка")
    } else if status["connecting"] == true {
        format!("{name} · прокси: …")
    } else {
        name
    };
    if status["connected"] == true && status["ipUnverified"] == true {
        line.push_str(" · IP: ?");
    }
    if let Some(skipped) = status["skippedServers"].as_u64().filter(|n| *n > 0) {
        line.push_str(&format!(" · пропущено в подписке: {skipped}"));
    }
    line
}

/// The reason the last connect or disconnect gave, when it gave one.
///
/// The kernel has carried this in `vpn_status.error` all along, and nothing drew
/// it: a refused connect (Happ's TUN holding the address, a dead server, a
/// subscription that would not resolve) flipped the switch back and said nothing,
/// so "the VPN does not work" was the only thing the panel showed (`#538`).
fn status_error(status: &Value) -> Option<String> {
    status["error"]
        .as_str()
        .map(str::trim)
        .filter(|error| !error.is_empty())
        .map(str::to_string)
}

fn listed_tag<'a>(status: &'a Value, servers: &[String]) -> Option<&'a str> {
    status["selected_tag"].as_str()
        .filter(|tag| servers.iter().any(|server| server.as_str() == *tag))
}

impl Vpn {
    fn connected(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s["connected"] == true)
    }

    fn auto_on(&self) -> bool {
        self.auto_pending.unwrap_or_else(|| self.status.as_ref().is_some_and(|s| s["auto_connect"] == true || s["autoConnect"] == true))
    }

    fn load_servers(&mut self, act: &mut Act<'_>) {
        let Some(revision) = self.subscription_revision else { return };
        if self.loading || !self.visible {
            return;
        }
        self.loading = true;
        self.failed = false;
        act.call("vpn_list_servers", json!({ "subscriptionRevision": revision }), move |vpn: &mut Vpn, result, act| {
            if let Some(error) = vpn.receive_servers(revision, result) {
                act.log(format!("vpn_list_servers error={error}"));
            }
        });
    }

    fn receive_servers(&mut self, revision: u64, result: Result<Value, String>) -> Option<String> {
        if self.subscription_revision != Some(revision) {
            return None;
        }
        self.loading = false;
        match result {
            Ok(list) => {
                let tags: Vec<String> = list.as_array().into_iter().flatten()
                    .filter_map(|server| server["tag"].as_str().map(str::to_string))
                    .collect();
                if self.selected.as_ref().map_or(true, |tag| !tags.contains(tag)) {
                    self.selected = self.status.as_ref().and_then(|status| listed_tag(status, &tags))
                        .map(str::to_string).or_else(|| tags.first().cloned());
                }
                self.servers = Some(tags);
                None
            }
            Err(error) => {
                self.failed = true;
                Some(error)
            }
        }
    }

    /// `refreshVpnStatus` also checks whether the visible picker needs a new list.
    fn refresh(&mut self, act: &mut Act<'_>) {
        self.polled = Some(Instant::now());
        act.call("vpn_status", json!({}), |vpn: &mut Vpn, result, act| {
            if vpn.receive_status(result) {
                vpn.load_servers(act);
            }
        });
    }

    fn receive_status(&mut self, result: Result<Value, String>) -> bool {
        let Ok(status) = result else {
            self.failed |= self.subscription_revision.is_none();
            return false;
        };
        let revision = status["subscription_revision"].as_u64();
        if let Some(revision) = revision {
            match self.subscription_revision {
                Some(previous) if previous > revision => return false,
                Some(previous) if previous == revision => {}
                _ => {
                    self.subscription_revision = Some(revision);
                    self.servers = None;
                    self.selected = None;
                    self.pings = None;
                    self.loading = false;
                    self.failed = false;
                    self.pinging = false;
                    self.menu = false;
                }
            }
            if !self.busy {
                if let Some(tag) = self.servers.as_deref().and_then(|tags| listed_tag(&status, tags)) {
                    self.selected = Some(tag.to_string());
                }
            }
        } else {
            self.failed |= self.subscription_revision.is_none();
        }
        self.status = Some(status);
        revision.is_some() && self.visible && self.servers.is_none() && !self.loading && !self.failed
    }

    /// `runVpnToggle`.
    fn toggle(&mut self, next: bool, act: &mut Act<'_>) {
        if self.busy {
            self.refresh(act);
            return;
        }
        if next && self.subscription_revision.is_none() {
            return;
        }
        self.busy = true;
        self.pending = Some(next);
        let (cmd, args) = if next {
            let tag = self.servers.as_ref().and_then(|_| self.selected.clone());
            ("vpn_connect", json!({ "tag": tag, "subscriptionRevision": self.subscription_revision }))
        } else {
            ("vpn_disconnect", json!({}))
        };
        act.call(cmd, args, |vpn: &mut Vpn, result, act| {
            if let Err(error) = result {
                act.debug(format!("VPN: {error}"));
            }
            vpn.busy = false;
            vpn.pending = None;
            vpn.refresh(act);
        });
    }

    /// `connectTo`: a server from the list, or the fastest after a check.
    fn connect_to(&mut self, tag: String, revision: u64, act: &mut Act<'_>) {
        if self.busy || self.subscription_revision != Some(revision) {
            return;
        }
        self.busy = true;
        self.selected = Some(tag.clone());
        act.call("vpn_connect", json!({ "tag": tag, "subscriptionRevision": revision }), |vpn: &mut Vpn, result, act| {
            if let Err(error) = result {
                act.debug(format!("VPN: {error}"));
            }
            vpn.busy = false;
            vpn.refresh(act);
        });
    }

    /// `pingAllServers`: every server checked, the list sorted by the answer, the fastest taken.
    fn ping_all(&mut self, revision: u64, act: &mut Act<'_>) {
        if self.pinging || !self.menu_is_current(revision) {
            return;
        }
        self.pinging = true;
        act.call("vpn_ping_all", json!({ "subscriptionRevision": revision }), move |vpn: &mut Vpn, result, act| {
            if !vpn.finish_ping(revision) {
                return;
            }
            match result {
                Ok(list) => {
                    let rows: Vec<(String, Ping)> = list.as_array().into_iter().flatten().filter_map(|r| {
                        let tag = r["tag"].as_str()?.to_string();
                        Some((tag, Ping { ok: r["ok"] == true, ms: r["latencyMs"].as_f64().filter(|m| m.is_finite()).map(|m| m.round() as u64) }))
                    }).collect();
                    let best = rows.iter().filter(|(_, p)| p.ok && p.ms.is_some()).min_by_key(|(_, p)| p.ms).map(|(tag, p)| (tag.clone(), p.ms.unwrap_or(0)));
                    vpn.pings = Some(rows.into_iter().collect());
                    match best {
                        Some((tag, ms)) => {
                            act.debug(format!("⚡ {} · {ms} мс", display_text(&tag)));
                            vpn.connect_to(tag, revision, act);
                        }
                        None => act.debug("Рабочих серверов не найдено"),
                    }
                }
                Err(error) => act.debug(format!("Проверка: {error}")),
            }
        });
    }

    fn finish_ping(&mut self, revision: u64) -> bool {
        if self.subscription_revision != Some(revision) {
            return false;
        }
        self.pinging = false;
        true
    }

    fn menu_is_current(&self, revision: u64) -> bool {
        self.menu && self.visible && self.subscription_revision == Some(revision)
    }

    fn set_auto(&mut self, next: bool, act: &mut Act<'_>) {
        self.auto_pending = Some(next);
        act.call("vpn_set_auto_connect", json!({ "enabled": next }), |vpn: &mut Vpn, result, act| {
            if let Err(error) = result {
                act.debug(format!("VPN: {error}"));
            }
            vpn.auto_pending = None;
            vpn.refresh(act);
        });
    }

    /// The servers in the list's order: by the last check (answered fastest first, then
    /// failed, then unchecked), else as the subscription lists them.
    fn ordered(&self) -> Vec<String> {
        let mut servers = self.servers.clone().unwrap_or_default();
        if let Some(pings) = &self.pings {
            let rank = |tag: &String| match pings.get(tag) {
                Some(Ping { ok: true, ms: Some(ms) }) => (0, *ms),
                Some(_) => (1, 0),
                None => (2, 0),
            };
            servers.sort_by_key(rank);
        }
        servers
    }

    /// The panel and its height, `width` wide; `nested` is the compact form inside a host.
    fn panel(&self, v: &View<'_>, width: f32, nested: bool) -> (El, f32) {
        let theme = v.theme;
        let ink = v.ink;
        let key = format!("{}:{}", if nested { "nested" } else { "card" }, v.id);
        let on = self.pending.unwrap_or_else(|| self.connected());
        let status = self.status.as_ref();
        let label = match status {
            Some(s) if s["connecting"] == true => "VPN: подключение…",
            Some(s) if s["connected"] == true => "VPN: включён",
            _ => "VPN: выключен",
        };
        let shield = v.svg(r#"<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2 4 6v6c0 5 3.4 8.7 8 10 4.6-1.3 8-5 8-10V6l-8-4Z" /></svg>"#, 16.0, if self.connected() { theme.status_ok } else { ink.alpha(0.4) });
        let icon = col().size(28.0, 28.0).radius(14.0).items_center().justify_center()
            .fill(if self.connected() { theme.status_ok.alpha(0.1) } else { theme.bg_elevated.alpha(0.06) })
            .child(shield);
        let line = status.map(server_line).unwrap_or_else(|| "—".into());
        let busy = self.busy;
        let top = row().w_full().h(28.0).gap(10.0).items_center()
            .child(icon)
            .child(
                col().grow(1.0).min_w(0.0)
                    .child(text(label.to_uppercase()).font(accent(theme, 10.4)).color(ink.alpha(0.8)).single_line())
                    .child(text(line).font(body(theme, 10.4)).color(ink.alpha(0.4)).single_line().truncate()),
            )
            .child(switch(v, format!("hit:vpn-toggle:{key}"), on, busy || (!on && self.subscription_revision.is_none()), |vpn: &mut Vpn, act| {
                let next = !vpn.pending.unwrap_or_else(|| vpn.connected());
                vpn.toggle(next, act);
            }));

        // The picker: the chosen server, "Загрузка…", or the subscription error with a retry.
        let (picker_label, picker_enabled) = if self.loading {
            ("Загрузка…".to_string(), false)
        } else if self.failed {
            ("Ошибка подписки · повторить".to_string(), true)
        } else if let Some(servers) = &self.servers {
            let shown = self.selected.as_ref().filter(|s| servers.contains(s)).or(servers.first());
            (shown.map_or_else(|| "Выберите сервер".to_string(), |s| display_text(s)), !busy)
        } else {
            ("Загрузка…".to_string(), false)
        };
        let inner = width - if nested { 2.0 } else { 30.0 };
        let mut button = row().key(format!("hit:vpn-server:{key}")).w_full().min_h(28.0).pl(8.0).pr(28.0).py(5.0).radius(8.0).items_center()
            .fill(theme.bg_elevated.alpha(0.05)).border(1.0, theme.bg_elevated.alpha(0.08)).border_layout()
            .opacity(if picker_enabled { 1.0 } else { 0.55 })
            .child(text(picker_label).font(body(theme, 10.4)).color(ink).single_line().truncate())
            .child(el().absolute(inner - 19.0, 4.0).child(text("⌄").font(body(theme, 12.8)).color(ink.alpha(0.8))));
        let revision = self.subscription_revision;
        if picker_enabled {
            button = button.on_press_tx(v.on(move |vpn: &mut Vpn, act| {
                if vpn.busy || !vpn.visible || vpn.subscription_revision != revision {
                    return;
                }
                if vpn.failed || vpn.servers.as_ref().map_or(true, Vec::is_empty) {
                    if revision.is_some() {
                        vpn.load_servers(act);
                    } else {
                        vpn.failed = false;
                        vpn.refresh(act);
                    }
                    return;
                }
                vpn.menu = !vpn.menu;
            }));
        }
        let mut picker = el().w_full().child(button);
        if let Some(revision) = revision.filter(|_| self.menu) {
            picker = picker.child(self.server_menu(v, inner, revision));
        }

        // The reason a connect or a disconnect gave, when it gave one (`#538`): the
        // kernel has carried `vpn_status.error` all along and no panel drew it, so a
        // refused connect flipped the switch back in silence. Two lines at most,
        // broken anywhere (the message names an adapter as often as a server), so
        // the row can never grow past the space the panel reserved for it.
        let error = status.and_then(status_error);
        let error_style = body(theme, 10.4);
        let error_width = width - if nested { 0.0 } else { 28.0 };
        let error_height = error.as_deref().map_or(0.0, |message| {
            let (_, measured) = rune::text::measure(message, &[], error_style, Some(error_width.max(1.0)));
            measured.min(2.0 * 12.6)
        });
        let notice = error.as_ref().map(|message| {
            el().w_full().child(
                text(message)
                    .font(error_style)
                    .color(theme.status_error)
                    .max_lines(2)
                    .wrap_anywhere(),
            )
        });

        let auto = row().w_full().pt(2.0).h(22.0).items_center().justify_between()
            .child(text("АВТО").font(accent(theme, 9.6).tracking(0.48)).color(ink.alpha(0.5)))
            .child(switch(v, format!("hit:vpn-auto:{key}"), self.auto_on(), self.auto_pending.is_some(), |vpn: &mut Vpn, act| {
                let next = !vpn.auto_on();
                vpn.set_auto(next, act);
            }));

        // `vpnPanel.addEventListener("click", e => e.stopPropagation())`: a press on the panel
        // does not fold the card it is in.
        let mut dash = v.swallow(col().key(format!("vpn-panel:{key}")).w(width).gap(8.0).radius(14.0).border(1.0, theme.bg_elevated.alpha(0.05)).border_layout()
            .child(top).child(picker));
        if let Some(notice) = notice {
            dash = dash.child(notice);
        }
        dash = dash.child(auto);
        let mut height = 2.0 + 28.0 + 8.0 + 28.0 + 8.0 + 22.0;
        if error.is_some() {
            height += 8.0 + error_height;
        }
        if !nested {
            dash = dash.px(14.0).py(10.0).fill(theme.bg_elevated.alpha(0.03)).mt(14.0);
            height += 20.0 + 14.0;
        }
        (dash, height)
    }

    /// `.vpn-server-menu`: above the picker, the list at most 200 px, options fading up one
    /// after another as the page's `vpnOptIn` (22 ms apart).
    fn server_menu(&self, v: &View<'_>, width: f32, revision: u64) -> El {
        let theme = v.theme;
        let ink = v.ink;
        let pinging = self.pinging;
        let head = row().w_full().px(4.0).pt(2.0).pb(6.0).mb(4.0).items_center().justify_between()
            .border_sides([0.0, 0.0, 1.0, 0.0], theme.bg_elevated.alpha(0.07))
            .child(text("СЕРВЕРЫ").font(accent(theme, 9.6).tracking(1.34)).color(ink.alpha(0.5)))
            .child({
                let mut ping = el().key("hit:vpn-ping").px(9.0).py(3.0).radius(999.0)
                    .fill(theme.bg_elevated.alpha(0.06)).border(1.0, theme.bg_elevated.alpha(if pinging { 0.12 } else { 0.18 })).border_layout()
                    .hover(theme.bg_elevated.alpha(0.14))
                    .child(text(if pinging { "ПРОВЕРКА…" } else { "ПРОВЕРИТЬ ВСЕ" }).font(accent(theme, 9.6).tracking(0.96)).color(if pinging { ink.alpha(0.6) } else { ink }));
                if !pinging {
                    ping = ping.on_press_tx(v.on(move |vpn: &mut Vpn, act| vpn.ping_all(revision, act)));
                }
                ping
            });
        let servers = self.ordered();
        let list_h = (servers.len() as f32 * 32.0 - 2.0).clamp(0.0, 200.0);
        let mut list = col().w_full().gap(2.0);
        for (index, tag) in servers.into_iter().enumerate() {
            let selected = self.selected.as_deref() == Some(tag.as_str());
            let badge = match self.pings.as_ref().and_then(|p| p.get(&tag)) {
                Some(Ping { ok: true, ms: Some(ms) }) => {
                    let colour = if *ms < 300 { theme.status_ok } else { theme.status_warn };
                    row().items_end()
                        .child(text(ms.to_string()).font(accent(theme, 9.6)).color(colour))
                        .child(text("мс").font(accent(theme, 8.0)).color(colour.alpha(0.55)).ml(1.0))
                }
                Some(_) => row().child(text("ошибка").font(accent(theme, 9.6)).color(theme.status_error)),
                None => row().w(14.0).justify_end().child(el().size(4.0, 4.0).radius(2.0).fill(theme.bg_elevated.alpha(0.2))),
            };
            let choose = tag.clone();
            list = list.child(
                row().key(format!("hit:vpn-option:{index}")).w_full().min_h(30.0).px(9.0).py(6.0).gap(8.0).radius(8.0).items_center().justify_between()
                    .fill(if selected { theme.status_ok.alpha(0.14) } else { Color::TRANSPARENT })
                    .hover(theme.bg_elevated.alpha(0.08))
                    .child(text(display_text(&tag)).font(body(theme, 10.4)).color(ink).single_line().truncate().grow(1.0).min_w(0.0))
                    .child(badge)
                    .transition(rune::Transition::offset(0.0, 6.0).combined(rune::Transition::opacity()))
                    .enter_delay(if v.motion.reduced { 0.0 } else { index as f32 * 0.022 })
                    .on_press_tx(v.on(move |vpn: &mut Vpn, act| {
                        if vpn.menu_is_current(revision) {
                            vpn.menu = false;
                            vpn.connect_to(choose.clone(), revision, act);
                        }
                    })),
            );
        }
        // `bottom: calc(100% + 6px)`: its height is the head's 32.5, the list's and 14 of
        // padding and border.
        let height = 6.0 + 32.5 + list_h + 6.0 + 2.0;
        // 12 px, the card radius under jelly (popups take the card radius).
        let radius = if theme.material == unidtk_shell::widget::Material::Jelly { theme.radius_card } else { 12.0 };
        col().key("hit:vpn-menu").absolute(0.0, -(height + 6.0)).w(width).h(height).padding(6.0).radius(radius)
            .fill(theme.surface.alpha(0.96)).border(1.0, theme.bg_elevated.alpha(0.1)).border_layout().shadow(20.0, 48.0, Color::BLACK.alpha(0.5))
            .child(head)
            .child(el().w_full().h(list_h).scroll(rune::ScrollSpec { stick_bottom: false }).child(list))
    }
}

fn accent(theme: &Theme, size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).family(Theme::family(theme.font_display)).lh_normal()
}

fn body(theme: &Theme, size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).family(Theme::family(theme.font_body)).lh_normal()
}

/// `.vpn-toggle`: 34 x 20, the knob 14 px 3 px in, green when on.
fn switch(v: &View<'_>, key: String, on: bool, disabled: bool, press: impl Fn(&mut Vpn, &mut Act<'_>) + 'static) -> El {
    let theme = v.theme;
    // `transition: transform var(--dur-hover) ease`.
    let ease = v.motion.tween(v.motion.duration(unidtk_shell::widget::Token::Hover), unidtk_shell::motion::EASE);
    let mut toggle = el().key(key).size(34.0, 20.0).radius(10.0)
        .fill(if on { theme.status_ok } else { v.ink.alpha(0.22) })
        .child(el().absolute(3.0, 3.0).size(14.0, 14.0).radius(7.0).fill(theme.bg_elevated).offset(if on { 14.0 } else { 0.0 }, 0.0).offset_motion(ease))
        .opacity(if disabled { 0.6 } else { 1.0 });
    if !disabled {
        toggle = toggle.on_press_tx(v.on(press));
    }
    toggle
}

impl Widget for Vpn {
    fn content(&self, v: &View<'_>) -> Option<El> {
        Some(self.panel(v, v.frame.w - 2.0 - 32.0, false).0)
    }

    fn surface(&self, v: &View<'_>) -> Option<Surface> {
        Some(Surface { size: SurfaceSize::Body(self.panel(v, v.frame.w - 2.0 - 32.0, false).1), nest: None })
    }

    fn compact(&self, v: &View<'_>) -> Option<(El, f32)> {
        Some(self.panel(v, v.frame.w, true))
    }

    fn wake(&self, _now: &LocalTime) -> Option<f64> {
        if !self.visible {
            return None;
        }
        let due = self.polled.map_or(Duration::ZERO, |at| (at + POLL).saturating_duration_since(Instant::now()));
        Some(due.as_secs_f64())
    }

    fn tick(&mut self, _now: &LocalTime, act: &mut Act<'_>) -> bool {
        if self.visible && self.polled.map_or(true, |at| at.elapsed() >= POLL) {
            self.refresh(act);
        }
        false
    }

    /// Check for subscription changes on open. Off screen, close the menu and stop polling.
    fn shown(&mut self, visible: bool, act: &mut Act<'_>) {
        self.visible = visible;
        if visible {
            if self.servers.is_none() {
                self.failed = false;
            }
            self.refresh(act);
        } else {
            self.menu = false;
        }
    }

    fn escape(&mut self, _act: &mut Act<'_>) -> bool {
        std::mem::take(&mut self.menu)
    }

    fn outside(&mut self, _act: &mut Act<'_>) -> bool {
        std::mem::take(&mut self.menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_read_as_windows_1251_is_turned_back() {
        // "Германия" in UTF-8, decoded as Windows-1251.
        let broken: String = "Германия".bytes().map(|b| if b < 0x80 { b as char } else {
            "ЂЃ‚ѓ„…†‡€‰Љ‹ЊЌЋЏђ‘’“”•–—\u{98}™љ›њќћџ\u{a0}ЎўЈ¤Ґ¦§Ё©Є«¬\u{ad}®Ї°±Ііґµ¶·ё№є»јЅѕїАБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдежзийклмнопрстуфхцчшщъыьэюя".chars().nth((b - 0x80) as usize).unwrap()
        }).collect();
        assert_ne!(broken, "Германия");
        assert_eq!(display_text(&broken), "Германия");
        assert_eq!(display_text("Россия 🇷🇺"), "Россия 🇷🇺", "clean Cyrillic stays");
        assert_eq!(display_text("nl-ams-01"), "nl-ams-01");
    }

    #[test]
    fn the_server_line_names_the_proxy_and_the_skipped() {
        assert_eq!(server_line(&json!({})), "—");
        assert_eq!(server_line(&json!({ "selected_tag": "de", "proxyLatencyMs": 120 })), "de · прокси: 120 мс");
        assert_eq!(server_line(&json!({ "selected_tag": "de", "connecting": true })), "de · прокси: …");
        assert_eq!(server_line(&json!({ "selected_tag": "de", "connected": true, "ipUnverified": true, "skippedServers": 2 })), "de · IP: ? · пропущено в подписке: 2");
    }

    #[test]
    fn a_refused_connect_says_why_on_the_line() {
        // #538: the reason `vpn_status.error` carries used to reach no panel.
        let reason = "Happ VPN/TUN is active; disconnect Happ before starting wallpaper VPN";
        assert_eq!(server_line(&json!({ "error": reason })), reason);
        assert_eq!(server_line(&json!({ "selected_tag": "de", "error": reason })), format!("de · {reason}"));
        assert_eq!(status_error(&json!({ "error": "   " })), None);
        assert_eq!(status_error(&json!({ "error": null })), None);
        assert_eq!(status_error(&json!({})), None);
        // A live tunnel reports no error, so nothing is drawn.
        assert_eq!(status_error(&json!({ "connected": true, "error": null })), None);
    }

    #[test]
    fn the_panel_polls_only_while_on_screen() {
        let mut vpn = Vpn::default();
        assert_eq!(vpn.wake(&LocalTime::default()), None);
        vpn.visible = true;
        assert_eq!(vpn.wake(&LocalTime::default()), Some(0.0), "never read: now");
        vpn.polled = Some(Instant::now());
        assert!(vpn.wake(&LocalTime::default()).unwrap() > 2.9);
    }

    #[test]
    fn a_changed_subscription_reloads_the_visible_picker_and_discards_old_replies() {
        let mut vpn = Vpn { visible: true, ..Vpn::default() };
        assert!(vpn.receive_status(Ok(json!({ "subscription_revision": 1 }))));
        vpn.receive_servers(1, Ok(json!([{ "tag": "old" }])));
        vpn.menu = true;
        vpn.pinging = true;
        vpn.pings = Some(HashMap::from([("old".into(), Ping { ok: true, ms: Some(20) })]));
        assert!(vpn.menu_is_current(1));

        assert!(vpn.receive_status(Ok(json!({
            "subscription_revision": 2, "selected_tag": "old", "connected": true,
        }))));
        assert!(vpn.servers.is_none());
        assert!(vpn.selected.is_none());
        assert!(vpn.pings.is_none());
        assert!(!vpn.menu);
        assert!(!vpn.pinging);

        vpn.loading = true;
        vpn.pinging = true;
        assert!(!vpn.receive_status(Ok(json!({ "subscription_revision": 1, "connected": false }))));
        assert!(vpn.connected());
        vpn.receive_servers(1, Ok(json!([{ "tag": "old" }])));
        vpn.receive_servers(1, Err("old request failed".into()));
        assert!(vpn.loading);
        assert!(vpn.servers.is_none());
        assert!(!vpn.failed);
        assert!(!vpn.finish_ping(1));
        assert!(vpn.pinging);

        vpn.receive_servers(2, Ok(json!([{ "tag": "new" }])));
        assert!(!vpn.loading);
        assert_eq!(vpn.selected.as_deref(), Some("new"));
        vpn.menu = true;
        assert!(!vpn.menu_is_current(1));
        assert!(vpn.menu_is_current(2));
        assert!(vpn.finish_ping(2));
        assert!(!vpn.pinging);
        assert!(!vpn.receive_status(Ok(json!({ "subscription_revision": 2, "selected_tag": "old" }))));
        assert!(vpn.menu);
        assert_eq!(vpn.selected.as_deref(), Some("new"));
    }

    #[test]
    fn a_cold_picker_recovers_at_revision_zero_and_defers_loading_when_hidden() {
        let mut vpn = Vpn { visible: true, ..Vpn::default() };
        assert!(!vpn.receive_status(Err("status unavailable".into())));
        assert!(vpn.failed);
        assert!(!vpn.receive_status(Ok(json!({ "subscription_revision": null }))));
        assert!(vpn.failed);
        assert!(vpn.receive_status(Ok(json!({ "subscription_revision": 0 }))));
        assert!(!vpn.failed);
        vpn.receive_servers(0, Err("list unavailable".into()));
        assert!(!vpn.receive_status(Ok(json!({ "subscription_revision": 0 }))));
        assert!(vpn.failed);

        vpn.visible = false;
        assert!(!vpn.receive_status(Ok(json!({ "subscription_revision": 1 }))));
        assert!(!vpn.failed);
        assert!(vpn.servers.is_none());
        vpn.visible = true;
        assert!(vpn.receive_status(Ok(json!({ "subscription_revision": 1 }))));
    }
}
