//! VPN's native half (`r[modules.vertical-slice]`): the panel `modules/vpn/ui/vpn-panel.js`
//! and `vpn.css` draw — the state line with its switch, the server picker with its list and
//! "ПРОВЕРИТЬ ВСЕ", and "АВТО" — as a widget of the shell's contract. It draws in its opened
//! card and, compact, in a host's nesting zone (`.nested-body > .dash-vpn` drops the margin,
//! the padding and the background). The server list is read once per run (`vpnServersCache`)
//! and `vpn_status` every 3 s while the panel is on screen. Writes (`vpn_connect`,
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
    /// `vpnServersCache`: the subscription's server tags.
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

impl Vpn {
    fn connected(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s["connected"] == true)
    }

    fn auto_on(&self) -> bool {
        self.auto_pending.unwrap_or_else(|| self.status.as_ref().is_some_and(|s| s["auto_connect"] == true || s["autoConnect"] == true))
    }

    fn load_servers(&mut self, act: &mut Act<'_>) {
        self.loading = true;
        self.failed = false;
        act.call("vpn_list_servers", json!({}), |vpn: &mut Vpn, result, act| {
            vpn.loading = false;
            match result {
                Ok(list) => {
                    let tags: Vec<String> = list.as_array().into_iter().flatten().filter_map(|s| s["tag"].as_str().map(str::to_string)).collect();
                    if vpn.selected.as_ref().map_or(true, |s| !tags.contains(s)) {
                        vpn.selected = vpn.status.as_ref().and_then(|s| s["selected_tag"].as_str()).map(str::to_string).filter(|t| tags.contains(t)).or_else(|| tags.first().cloned());
                    }
                    vpn.servers = Some(tags);
                }
                Err(error) => {
                    vpn.failed = true;
                    act.log(format!("vpn_list_servers error={error}"));
                }
            }
        });
    }

    /// `refreshVpnStatus`: a failed read changes nothing.
    fn refresh(&mut self, act: &mut Act<'_>) {
        self.polled = Some(Instant::now());
        act.call("vpn_status", json!({}), |vpn: &mut Vpn, result, _act| {
            let Ok(status) = result else { return };
            if !vpn.busy {
                if let Some(tag) = status["selected_tag"].as_str().filter(|t| !t.is_empty()) {
                    vpn.selected = Some(tag.to_string());
                }
            }
            vpn.status = Some(status);
        });
    }

    /// `runVpnToggle`.
    fn toggle(&mut self, next: bool, act: &mut Act<'_>) {
        if self.busy {
            self.refresh(act);
            return;
        }
        self.busy = true;
        self.pending = Some(next);
        let (cmd, args) = if next {
            let tag = self.servers.as_ref().and_then(|_| self.selected.clone());
            ("vpn_connect", json!({ "tag": tag }))
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
    fn connect_to(&mut self, tag: String, act: &mut Act<'_>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.selected = Some(tag.clone());
        act.call("vpn_connect", json!({ "tag": tag }), |vpn: &mut Vpn, result, act| {
            if let Err(error) = result {
                act.debug(format!("VPN: {error}"));
            }
            vpn.busy = false;
            vpn.refresh(act);
        });
    }

    /// `pingAllServers`: every server checked, the list sorted by the answer, the fastest taken.
    fn ping_all(&mut self, act: &mut Act<'_>) {
        if self.pinging {
            return;
        }
        self.pinging = true;
        act.call("vpn_ping_all", json!({}), |vpn: &mut Vpn, result, act| {
            vpn.pinging = false;
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
                            vpn.connect_to(tag, act);
                        }
                        None => act.debug("Рабочих серверов не найдено"),
                    }
                }
                Err(error) => act.debug(format!("Проверка: {error}")),
            }
        });
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
            .child(switch(v, format!("hit:vpn-toggle:{key}"), on, busy, |vpn: &mut Vpn, act| {
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
        if picker_enabled {
            button = button.on_press_tx(v.on(|vpn: &mut Vpn, act| {
                if vpn.busy {
                    return;
                }
                if vpn.failed || vpn.servers.as_ref().map_or(true, Vec::is_empty) {
                    vpn.load_servers(act);
                    return;
                }
                vpn.menu = !vpn.menu;
            }));
        }
        let mut picker = el().w_full().child(button);
        if self.menu {
            picker = picker.child(self.server_menu(v, inner));
        }

        let auto = row().w_full().pt(2.0).h(22.0).items_center().justify_between()
            .child(text("АВТО").font(accent(theme, 9.6).tracking(0.48)).color(ink.alpha(0.5)))
            .child(switch(v, format!("hit:vpn-auto:{key}"), self.auto_on(), self.auto_pending.is_some(), |vpn: &mut Vpn, act| {
                let next = !vpn.auto_on();
                vpn.set_auto(next, act);
            }));

        // `vpnPanel.addEventListener("click", e => e.stopPropagation())`: a press on the panel
        // does not fold the card it is in.
        let mut dash = v.swallow(col().key(format!("vpn-panel:{key}")).w(width).gap(8.0).radius(14.0).border(1.0, theme.bg_elevated.alpha(0.05)).border_layout()
            .child(top).child(picker).child(auto));
        let mut height = 2.0 + 28.0 + 8.0 + 28.0 + 8.0 + 22.0;
        if !nested {
            dash = dash.px(14.0).py(10.0).fill(theme.bg_elevated.alpha(0.03)).mt(14.0);
            height += 20.0 + 14.0;
        }
        (dash, height)
    }

    /// `.vpn-server-menu`: above the picker, the list at most 200 px, options fading up one
    /// after another as the page's `vpnOptIn` (22 ms apart).
    fn server_menu(&self, v: &View<'_>, width: f32) -> El {
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
                    ping = ping.on_press_tx(v.on(|vpn: &mut Vpn, act| vpn.ping_all(act)));
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
                        vpn.menu = false;
                        vpn.connect_to(choose.clone(), act);
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

    /// A panel came on screen: the list once per run, and the state now. Off screen, the list
    /// closes and the polling stops.
    fn shown(&mut self, visible: bool, act: &mut Act<'_>) {
        self.visible = visible;
        if visible {
            if self.servers.is_none() && !self.loading {
                self.load_servers(act);
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
    fn the_panel_polls_only_while_on_screen() {
        let mut vpn = Vpn::default();
        assert_eq!(vpn.wake(&LocalTime::default()), None);
        vpn.visible = true;
        assert_eq!(vpn.wake(&LocalTime::default()), Some(0.0), "never read: now");
        vpn.polled = Some(Instant::now());
        assert!(vpn.wake(&LocalTime::default()).unwrap() > 2.9);
    }
}
