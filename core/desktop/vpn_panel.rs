//! The VPN panel (`modules/vpn/ui/vpn-panel.js`, `vpn.css`): the state line with its switch,
//! the server picker with its list and "ПРОВЕРИТЬ ВСЕ", and "АВТО". It draws in an opened VPN
//! card and in a host's nesting zone (`.nested-body > .dash-vpn` drops the margin, the padding
//! and the background). Every panel shows one state: the server list is read once per run, as
//! the page's `vpnServersCache`, and `vpn_status` every 3 s while a panel is on screen. Writes
//! (`vpn_connect`, `vpn_disconnect`, `vpn_set_auto_connect`) go through the interface door.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rune::el::{col, el, row, text, El};
use rune::{Color, TextStyle, Tx};
use serde_json::{json, Value};

use crate::menu::MenuHost;
use crate::shell::Host;
use crate::theme::Theme;

/// The page's poll interval for every panel on screen.
pub const POLL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ping {
    pub ok: bool,
    pub ms: Option<u64>,
}

#[derive(Default)]
pub struct VpnPanel {
    /// `vpnServersCache`: the subscription's server tags.
    pub servers: Option<Vec<String>>,
    pub loading: bool,
    /// The last read failed: the picker offers "Ошибка подписки · повторить".
    pub failed: bool,
    /// The hidden `<select>`'s value: the server a connect asks for.
    pub selected: Option<String>,
    pub status: Option<Value>,
    pub busy: bool,
    /// The switch as the person set it while the command runs.
    pub pending: Option<bool>,
    pub pinging: bool,
    pub auto_pending: Option<bool>,
    pub pings: Option<HashMap<String, Ping>>,
    /// The panel whose server list is open.
    pub menu: Option<String>,
    pub polled: Option<Instant>,
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

/// A panel came on screen: the list once per run, and the state now.
pub fn shown(state: &mut Host) {
    if state.vpn.servers.is_none() && !state.vpn.loading {
        load_servers(state);
    }
    refresh(state);
}

pub fn load_servers(state: &mut Host) {
    state.vpn.loading = true;
    state.vpn.failed = false;
    state.call("vpn_list_servers", json!({}), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        state.vpn.loading = false;
        match result {
            Ok(list) => {
                let tags: Vec<String> = list.as_array().into_iter().flatten().filter_map(|s| s["tag"].as_str().map(str::to_string)).collect();
                if state.vpn.selected.as_ref().map_or(true, |s| !tags.contains(s)) {
                    state.vpn.selected = state.vpn.status.as_ref().and_then(|s| s["selected_tag"].as_str()).map(str::to_string).filter(|t| tags.contains(t)).or_else(|| tags.first().cloned());
                }
                state.vpn.servers = Some(tags);
            }
            Err(error) => {
                state.vpn.failed = true;
                crate::shell::log_frontend(state, &format!("vpn_list_servers error={error}"));
            }
        }
    }));
}

/// `refreshVpnStatus`: a failed read changes nothing.
pub fn refresh(state: &mut Host) {
    state.vpn.polled = Some(Instant::now());
    state.call("vpn_status", json!({}), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        let Ok(status) = result else { return };
        if !state.vpn.busy {
            if let Some(tag) = status["selected_tag"].as_str().filter(|t| !t.is_empty()) {
                state.vpn.selected = Some(tag.to_string());
            }
        }
        state.vpn.status = Some(status);
    }));
}

fn connected(state: &Host) -> bool {
    state.vpn.status.as_ref().is_some_and(|s| s["connected"] == true)
}

/// `runVpnToggle`.
fn toggle(state: &mut Host, next: bool) {
    if state.vpn.busy {
        refresh(state);
        return;
    }
    state.vpn.busy = true;
    state.vpn.pending = Some(next);
    let (cmd, args) = if next {
        let tag = state.vpn.servers.as_ref().and_then(|_| state.vpn.selected.clone());
        ("vpn_connect", json!({ "tag": tag }))
    } else {
        ("vpn_disconnect", json!({}))
    };
    state.call(cmd, args, Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        if let Err(error) = result {
            state.show_debug(format!("VPN: {error}"));
        }
        state.vpn.busy = false;
        state.vpn.pending = None;
        refresh(state);
    }));
}

/// `connectTo`: a server from the list, or the fastest after a check.
fn connect_to(state: &mut Host, tag: String) {
    if state.vpn.busy {
        return;
    }
    state.vpn.busy = true;
    state.vpn.selected = Some(tag.clone());
    state.call("vpn_connect", json!({ "tag": tag }), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        if let Err(error) = result {
            state.show_debug(format!("VPN: {error}"));
        }
        state.vpn.busy = false;
        refresh(state);
    }));
}

/// `pingAllServers`: every server checked, the list sorted by the answer, the fastest taken.
fn ping_all(state: &mut Host) {
    if state.vpn.pinging {
        return;
    }
    state.vpn.pinging = true;
    state.call("vpn_ping_all", json!({}), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        state.vpn.pinging = false;
        match result {
            Ok(list) => {
                let rows: Vec<(String, Ping)> = list.as_array().into_iter().flatten().filter_map(|r| {
                    let tag = r["tag"].as_str()?.to_string();
                    Some((tag, Ping { ok: r["ok"] == true, ms: r["latencyMs"].as_f64().filter(|m| m.is_finite()).map(|m| m.round() as u64) }))
                }).collect();
                let best = rows.iter().filter(|(_, p)| p.ok && p.ms.is_some()).min_by_key(|(_, p)| p.ms).map(|(tag, p)| (tag.clone(), p.ms.unwrap_or(0)));
                state.vpn.pings = Some(rows.into_iter().collect());
                match best {
                    Some((tag, ms)) => {
                        state.show_debug(format!("⚡ {} · {ms} мс", display_text(&tag)));
                        connect_to(state, tag);
                    }
                    None => state.show_debug("Рабочих серверов не найдено".into()),
                }
            }
            Err(error) => state.show_debug(format!("Проверка: {error}")),
        }
    }));
}

fn set_auto(state: &mut Host, next: bool) {
    state.vpn.auto_pending = Some(next);
    state.call("vpn_set_auto_connect", json!({ "enabled": next }), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        if let Err(error) = result {
            state.show_debug(format!("VPN: {error}"));
        }
        state.vpn.auto_pending = None;
        refresh(state);
    }));
}

/// The servers in the list's order: by the last check (answered fastest first, then failed,
/// then unchecked), else as the subscription lists them.
fn ordered(state: &Host) -> Vec<String> {
    let mut servers = state.vpn.servers.clone().unwrap_or_default();
    if let Some(pings) = &state.vpn.pings {
        let rank = |tag: &String| match pings.get(tag) {
            Some(Ping { ok: true, ms: Some(ms) }) => (0, *ms),
            Some(_) => (1, 0),
            None => (2, 0),
        };
        servers.sort_by_key(rank);
    }
    servers
}

fn accent(theme: &Theme, size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).family(Theme::family(theme.font_display))
}

fn body(theme: &Theme, size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).family(Theme::family(theme.font_body))
}

/// `.vpn-toggle`: 34 x 20, the knob 14 px 3 px in, green when on.
fn switch(theme: &Theme, ink: Color, key: String, on: bool, disabled: bool, press: impl Fn(&mut Host, &mut Tx) + 'static) -> El {
    let motion = theme.motion();
    let ease = motion.tween(motion.duration(crate::motion::Token::Hover), crate::motion::STANDARD);
    let mut toggle = el().key(key).size(34.0, 20.0).radius(10.0)
        .fill(if on { theme.status_ok } else { ink.alpha(0.22) })
        .child(el().absolute(3.0, 3.0).size(14.0, 14.0).radius(7.0).fill(theme.bg_elevated).offset(if on { 14.0 } else { 0.0 }, 0.0).offset_motion(ease))
        .opacity(if disabled { 0.6 } else { 1.0 });
    if !disabled {
        toggle = toggle.on_press_tx(press);
    }
    toggle
}

/// The panel and its height. `key` tells two panels on screen apart; `nested` is the compact
/// form inside a host (no margin, padding or background).
pub fn panel(state: &Host, theme: &Theme, ink: Color, key: &str, width: f32, nested: bool) -> (El, f32) {
    let on = state.vpn.pending.unwrap_or_else(|| connected(state));
    let status = state.vpn.status.as_ref();
    let label = match status {
        Some(s) if s["connecting"] == true => "VPN: подключение…",
        Some(s) if s["connected"] == true => "VPN: включён",
        _ => "VPN: выключен",
    };
    let shield = crate::shell::svg_icon(state, r#"<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2 4 6v6c0 5 3.4 8.7 8 10 4.6-1.3 8-5 8-10V6l-8-4Z" /></svg>"#, 16.0, if connected(state) { theme.status_ok } else { ink.alpha(0.4) });
    let icon = col().size(28.0, 28.0).radius(14.0).items_center().justify_center()
        .fill(if connected(state) { theme.status_ok.alpha(0.1) } else { theme.bg_elevated.alpha(0.06) })
        .child(shield);
    let line = status.map(server_line).unwrap_or_else(|| "—".into());
    let busy = state.vpn.busy;
    let top = row().w_full().h(28.0).gap(10.0).items_center()
        .child(icon)
        .child(
            col().grow(1.0).min_w(0.0)
                .child(text(label.to_uppercase()).font(accent(theme, 10.4)).color(ink.alpha(0.8)).single_line())
                .child(text(line).font(body(theme, 10.4)).color(ink.alpha(0.4)).single_line().truncate()),
        )
        .child(switch(theme, ink, format!("hit:vpn-toggle:{key}"), on, busy, move |state: &mut Host, _tx: &mut Tx| {
            let next = !state.vpn.pending.unwrap_or_else(|| connected(state));
            toggle(state, next);
        }));

    // The picker: the chosen server, "Загрузка…", or the subscription error with a retry.
    let (picker_label, picker_enabled) = if state.vpn.loading {
        ("Загрузка…".to_string(), false)
    } else if state.vpn.failed {
        ("Ошибка подписки · повторить".to_string(), true)
    } else if let Some(servers) = &state.vpn.servers {
        let shown = state.vpn.selected.as_ref().filter(|s| servers.contains(s)).or(servers.first());
        (shown.map_or_else(|| "Выберите сервер".to_string(), |s| display_text(s)), !busy)
    } else {
        ("Загрузка…".to_string(), false)
    };
    let menu_key = key.to_string();
    let mut button = row().key(format!("hit:vpn-server:{key}")).w_full().min_h(28.0).pl(8.0).pr(28.0).py(5.0).radius(8.0).items_center()
        .fill(theme.bg_elevated.alpha(0.05)).border(1.0, theme.bg_elevated.alpha(0.08))
        .opacity(if picker_enabled { 1.0 } else { 0.55 })
        .child(text(picker_label).font(body(theme, 10.4)).color(ink).single_line().truncate())
        .child(el().absolute(width - (if nested { 2.0 } else { 30.0 }) - 19.0, 4.0).child(text("⌄").font(body(theme, 12.8)).color(ink.alpha(0.8))));
    if picker_enabled {
        button = button.on_press_tx(move |state: &mut Host, _tx: &mut Tx| {
            if state.vpn.busy {
                return;
            }
            if state.vpn.failed || state.vpn.servers.as_ref().map_or(true, Vec::is_empty) {
                load_servers(state);
                return;
            }
            state.vpn.menu = if state.vpn.menu.as_deref() == Some(menu_key.as_str()) { None } else { Some(menu_key.clone()) };
        });
    }
    let mut picker = el().w_full().child(button);
    if state.vpn.menu.as_deref() == Some(key) {
        picker = picker.child(server_menu(state, theme, ink, width - if nested { 2.0 } else { 30.0 }));
    }

    let auto_on = state.vpn.auto_pending.unwrap_or_else(|| status.is_some_and(|s| s["auto_connect"] == true || s["autoConnect"] == true));
    let auto = row().w_full().pt(2.0).h(22.0).items_center().justify_between()
        .child(text("АВТО").font(accent(theme, 9.6).tracking(0.48)).color(ink.alpha(0.5)))
        .child(switch(theme, ink, format!("hit:vpn-auto:{key}"), auto_on, state.vpn.auto_pending.is_some(), |state: &mut Host, _tx: &mut Tx| {
            let next = !state.vpn.auto_pending.unwrap_or_else(|| state.vpn.status.as_ref().is_some_and(|s| s["auto_connect"] == true || s["autoConnect"] == true));
            set_auto(state, next);
        }));

    // `vpnPanel.addEventListener("click", e => e.stopPropagation())`: a press on the panel
    // does not fold the card it is in.
    let mut dash = col().key(format!("vpn-panel:{key}")).w(width).gap(8.0).radius(14.0).border(1.0, theme.bg_elevated.alpha(0.05))
        .child(top).child(picker).child(auto)
        .on_press(|_: &mut Host| {});
    let mut height = 2.0 + 28.0 + 8.0 + 28.0 + 8.0 + 22.0;
    if !nested {
        dash = dash.px(14.0).py(10.0).fill(theme.bg_elevated.alpha(0.03)).mt(14.0);
        height += 20.0 + 14.0;
    }
    (dash, height)
}

/// `.vpn-server-menu`: above the picker, the list at most 200 px, options fading up one after
/// another as the page's `vpnOptIn` (22 ms apart).
fn server_menu(state: &Host, theme: &Theme, ink: Color, width: f32) -> El {
    let motion = theme.motion();
    let pinging = state.vpn.pinging;
    let head = row().w_full().px(4.0).pt(2.0).pb(6.0).mb(4.0).items_center().justify_between()
        .border_sides([0.0, 0.0, 1.0, 0.0], theme.bg_elevated.alpha(0.07))
        .child(text("СЕРВЕРЫ").font(accent(theme, 9.6).tracking(1.34)).color(ink.alpha(0.5)))
        .child({
            let mut ping = el().key("hit:vpn-ping").px(9.0).py(3.0).radius(999.0)
                .fill(theme.bg_elevated.alpha(0.06)).border(1.0, theme.bg_elevated.alpha(if pinging { 0.12 } else { 0.18 }))
                .hover(theme.bg_elevated.alpha(0.14))
                .child(text(if pinging { "ПРОВЕРКА…" } else { "ПРОВЕРИТЬ ВСЕ" }).font(accent(theme, 9.6).tracking(0.96)).color(if pinging { ink.alpha(0.6) } else { ink }));
            if !pinging {
                ping = ping.on_press_tx(|state: &mut Host, _tx: &mut Tx| ping_all(state));
            }
            ping
        });
    let servers = ordered(state);
    let list_h = (servers.len() as f32 * 32.0 - 2.0).clamp(0.0, 200.0);
    let mut list = col().w_full().gap(2.0);
    for (index, tag) in servers.into_iter().enumerate() {
        let selected = state.vpn.selected.as_deref() == Some(tag.as_str());
        let badge = match state.vpn.pings.as_ref().and_then(|p| p.get(&tag)) {
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
                .enter_delay(if motion.reduced { 0.0 } else { index as f32 * 0.022 })
                .on_press_tx(move |state: &mut Host, _tx: &mut Tx| {
                    state.vpn.menu = None;
                    connect_to(state, choose.clone());
                }),
        );
    }
    // `bottom: calc(100% + 6px)`: its height is the head's 32.5, the list's and 14 of padding
    // and border.
    let height = 6.0 + 32.5 + list_h + 6.0 + 2.0;
    col().key("hit:vpn-menu").absolute(0.0, -(height + 6.0)).w(width).h(height).padding(6.0).radius(12.0)
        .fill(theme.surface.alpha(0.96)).border(1.0, theme.bg_elevated.alpha(0.1)).shadow(20.0, 48.0, Color::BLACK.alpha(0.5))
        .child(head)
        .child(el().w_full().h(list_h).scroll(rune::ScrollSpec { stick_bottom: false }).child(list))
}

/// The poll's part: while a panel is on screen its state is read every 3 s; the time of the
/// next read.
pub fn poll(state: &mut Host, visible: bool, now: Instant) -> Option<Instant> {
    if !visible {
        state.vpn.menu = None;
        return None;
    }
    let due = state.vpn.polled.map_or(now, |at| at + POLL);
    if now >= due {
        refresh(state);
        return Some(now + POLL);
    }
    Some(due)
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
}
