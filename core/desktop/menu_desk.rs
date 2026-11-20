//! The menu's "Стол" pane (`renderDesk`, `renderFolders` in `menu.js`): "Расставить" and
//! "Исходная раскладка", one row per module with its state and its sheet (on the desk, hidden
//! but working, switched off, inside another widget), and the folders with their sheet
//! (rename, delete, shortcuts, "Из Пуска", "Файл…", "Папку…", "Вставить").

use std::collections::HashSet;

use rune::el::{col, el, row, text, El};
use rune::{Color, Tx};
use serde_json::{json, Value};

use crate::controls::{self, ButtonKind, Look};
use crate::menu::{self, Loaded, MenuHost, Sheet};
use crate::theme::Theme;

/// `FOLDER_ICONS` of `folders.js`, with their labels (`ICON_LABELS`).
pub const FOLDER_ICONS: [(&str, &str, &str); 9] = [
    ("folder", "Папка", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>"#),
    ("terminal", "Терминал", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>"#),
    ("gamepad", "Игры", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="12" x2="10" y2="12"/><line x1="8" y1="10" x2="8" y2="14"/><line x1="15" y1="13" x2="15.01" y2="13"/><line x1="18" y1="11" x2="18.01" y2="11"/><rect x="2" y="6" width="20" height="12" rx="6"/></svg>"#),
    ("code", "Код", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>"#),
    ("globe", "Сайты", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>"#),
    ("music", "Музыка", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>"#),
    ("tools", "Инструменты", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>"#),
    ("book", "Книги", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></svg>"#),
    ("star", "Избранное", r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>"#),
];

pub fn folder_icon(key: &str) -> &'static str {
    FOLDER_ICONS.iter().find(|(k, ..)| *k == key).map_or(FOLDER_ICONS[0].2, |(.., svg)| svg)
}

/// A module the menu lists: the page's `getWidgets` without the folders.
struct Module {
    id: String,
    name: String,
    description: String,
    icon: String,
    backend: bool,
}

fn modules() -> Vec<Module> {
    crate::catalog::desk_modules()
        .map(|m| Module {
            id: m.id.clone(),
            name: m.meta["name"].as_str().unwrap_or(&m.id).to_string(),
            description: m.meta["description"].as_str().unwrap_or_default().to_string(),
            icon: m.icon.clone(),
            backend: m.meta["backend"].as_bool().unwrap_or(false),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModuleState {
    Desk,
    Background,
    Off,
}

/// `moduleState`.
fn module_state(module: &Module, hidden: &[String], disabled: &HashSet<String>) -> ModuleState {
    if module.backend && disabled.contains(&module.id) {
        ModuleState::Off
    } else if !hidden.iter().any(|h| *h == module.id) {
        ModuleState::Desk
    } else if module.backend {
        ModuleState::Background
    } else {
        ModuleState::Off
    }
}

/// `stateNoun`.
fn state_noun(module: &Module, state: ModuleState) -> &'static str {
    match state {
        ModuleState::Desk => "на столе",
        ModuleState::Background => "скрыт, работает",
        ModuleState::Off if !module.backend => "скрыт",
        ModuleState::Off => "выключен",
    }
}

/// `nestingHost`: the widget a module sits in.
fn nesting_host<S: MenuHost>(state: &S, id: &str) -> Option<String> {
    let layout = state.layout()?;
    let host = layout.nested.iter().find(|(_, children)| children.iter().any(|c| c == id))?.0.clone();
    let name = modules().into_iter().find(|m| m.id == host).map(|m| m.name)
        .or_else(|| state.menu_ref().folders.ready().and_then(|f| f.iter().find(|f| f["id"] == host.as_str()).and_then(|f| f["name"].as_str().map(str::to_string))))
        .unwrap_or(host);
    Some(name)
}

/// What the pane reads from the kernel when it is shown.
pub fn load<S: MenuHost>(state: &mut S, _tx: &mut Tx) {
    load_lists(state);
}

/// The switched-off modules and the folders: the pane shows them, the desk draws from them.
pub fn load_lists<S: MenuHost>(state: &mut S) {
    state.call("get_disabled_modules", json!({}), Box::new(|state: &mut S, result, _tx: &mut Tx| {
        state.menu().disabled_modules = match result {
            Ok(list) => Loaded::Ready(list.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()),
            Err(error) => Loaded::Failed(error),
        };
    }));
    load_folders(state);
}

pub fn load_folders<S: MenuHost>(state: &mut S) {
    state.call("get_folders", json!({}), Box::new(|state: &mut S, result, _tx: &mut Tx| match result {
        Ok(folders) => {
            let folders: Vec<Value> = folders.as_array().cloned().unwrap_or_default();
            let targets: Vec<Value> = folders.iter().flat_map(|f| f["items"].as_array().cloned().unwrap_or_default()).map(|item| item["target"].clone()).collect();
            state.menu().folders = Loaded::Ready(folders);
            state.call("missing_targets", json!({ "targets": targets }), Box::new(move |state: &mut S, result, _tx: &mut Tx| {
                // A failed check marks nothing missing, as `missingTargets` does.
                let flags: Vec<bool> = result.ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                state.menu().missing = targets.iter().zip(flags).filter(|(_, gone)| *gone).filter_map(|(t, _)| t.as_str().map(str::to_string)).collect();
            }));
        }
        Err(error) => state.menu().folders = Loaded::Failed(error),
    }));
}

fn icon_el<S: MenuHost>(state: &S, svg: &str, size: f32, colour: Color) -> El {
    match state.svg(svg, size, colour) {
        Some(pixels) => el().size(size, size).pixels(&pixels),
        None => el().size(size, size),
    }
}

fn empty(look: &Look, message: &str, error: bool) -> El {
    el().px(4.0).py(16.0).child(
        text(message.to_string()).font(look.font(13.0, 400.0).lh(13.0 * 1.5))
            .color(if error { look.status_error } else { look.fg.alpha(0.78) }).wrap(),
    )
}

/// `.menu-row`: icon, name and sub-line, then the row's controls.
fn list_row(look: &Look, key: String, icon: El, name: &str, sub: &str, sub_warn: bool, controls: Vec<El>) -> El {
    let mut line = row().key(key).w_full().min_h(44.0).px(8.0).py(6.0).gap(10.0).radius(12.0).items_center()
        .hover(look.elev.alpha(0.07))
        .child(col().size(28.0, 28.0).items_center().justify_center().child(icon))
        .child(
            col().grow(1.0).min_w(0.0).gap(2.0)
                .child(text(name.to_string()).font(look.font(13.0, 400.0)).color(look.fg).single_line().truncate())
                .child(text(sub.to_string()).font(look.font(12.0, 400.0)).color(if sub_warn { look.status_warn } else { look.fg.alpha(0.72) }).single_line()),
        );
    for control in controls {
        line = line.child(control);
    }
    line
}

pub fn pane<S: MenuHost>(state: &S, look: &Look) -> El {
    // `.menu-actions { margin: 8px 0 }`: its bottom margin collapses into the heading's 18 px.
    let mut pane = col().w_full().child(
        row().gap(8.0).pt(8.0).flex_wrap()
            .child(controls::button(look, "hit:arrange".into(), "Расставить", ButtonKind::Primary, |state: &mut S, tx: &mut Tx| {
                menu::close(state, tx);
                state.enter_arrange(tx);
            }))
            .child(controls::button(look, "hit:reset-positions".into(), "Исходная раскладка", ButtonKind::Normal, |state: &mut S, tx: &mut Tx| reset_positions(state, tx))),
    );

    pane = pane.child(controls::heading(look, "Виджеты"));
    let hidden = state.layout().map(|l| l.hidden.clone()).unwrap_or_default();
    match state.menu_ref().disabled_modules.clone() {
        Loaded::Loading => pane = pane.child(empty(look, "Загрузка…", false)),
        Loaded::Failed(error) => {
            pane = pane.child(
                col().gap(10.0).child(empty(look, &format!("Не удалось загрузить список виджетов: {error}"), true))
                    .child(row().child(controls::button(look, "hit:retry-widgets".into(), "Повторить", ButtonKind::Normal, |state: &mut S, tx: &mut Tx| load(state, tx)))),
            );
        }
        Loaded::Ready(disabled) => {
            let mut list = col().gap(2.0);
            for module in modules() {
                let sub = match nesting_host(state, &module.id) {
                    Some(host) => format!("внутри «{host}»"),
                    None => state_noun(&module, module_state(&module, &hidden, &disabled)).to_string(),
                };
                let icon = icon_el(state, &module.icon, 20.0, look.fg.alpha(0.85));
                let id = module.id.clone();
                let hover_id = module.id.clone();
                let open_key = format!("hit:widget-open:{id}");
                let rect_key = open_key.clone();
                let open = controls::button(look, open_key.clone(), "Настроить…", ButtonKind::Normal, move |state: &mut S, tx: &mut Tx| {
                    menu::open_sheet(state, Sheet::Widget(id.clone()), &open_key, tx);
                })
                .on_layout_tx(move |state: &mut S, rect: [f32; 4], tx: &mut Tx| menu::remember_rect(state, &rect_key, rect, tx));
                list = list.child(
                    list_row(look, format!("hit:widget-row:{}", module.id), icon, &module.name, &sub, false, vec![open])
                        .on_hover_change(move |state: &mut S, hovered: bool, _tx: &mut Tx| {
                            state.menu().highlight = hovered.then(|| hover_id.clone());
                        }),
                );
            }
            pane = pane.child(list);
        }
    }

    pane = pane.child(controls::heading(look, "Папки"));
    pane.child(crate::menu_folders::section(state, look))
}

/// "Исходная раскладка": every widget with a default goes back to it (geometry only: the
/// settings stored beside it stay); folders and instances only come back on screen. Undo puts
/// the old positions back if nothing changed the layout since.
fn reset_positions<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    let Some(layout) = state.layout() else { return };
    let before = layout.config.get("positions").cloned().unwrap_or(json!({}));
    let defaults = crate::catalog::default_layout()["positions"].clone();
    let canvas = state.desk_size();
    let (max_x, max_y) = ((canvas.0 - 120.0).max(0.0) as f64, (canvas.1 - 64.0).max(0.0) as f64);
    let mut ops = Vec::new();
    for (id, pos) in before.as_object().into_iter().flatten() {
        let mut op = json!({ "op": "place", "id": id });
        match defaults.get(id) {
            Some(def) => {
                op["x"] = def["x"].clone();
                op["y"] = def["y"].clone();
                for side in ["w", "h"] {
                    if def.get(side).is_some_and(|v| v.as_f64().is_some_and(|n| n > 0.0)) {
                        op[side] = def[side].clone();
                    }
                }
            }
            None => {
                if let Some(x) = pos["x"].as_f64() {
                    op["x"] = json!(x.clamp(0.0, max_x));
                }
                if let Some(y) = pos["y"].as_f64() {
                    op["y"] = json!(y.clamp(0.0, max_y));
                }
            }
        }
        ops.push(op);
    }
    if !ops.is_empty() {
        state.apply(ops, tx);
    }
    menu::undo_toast(state, "Раскладка сброшена".into(), Box::new(move |state: &mut S, tx: &mut Tx| {
        state.apply(vec![json!({ "op": "positions", "positions": before })], tx);
    }));
}

/// `setModuleState`: switching a backend module off stops it and keeps its desk choice, so
/// switching it on restores that; hiding only takes the card off the desk.
fn set_module_state<S: MenuHost>(state: &mut S, id: &str, next: ModuleState, tx: &mut Tx) {
    let Some(module) = modules().into_iter().find(|m| m.id == id) else { return };
    let disabled = state.menu_ref().disabled_modules.ready().cloned().unwrap_or_default();
    let off = next == ModuleState::Off;
    if module.backend && off != disabled.contains(id) {
        let id_owned = id.to_string();
        menu::run_once(state, &format!("module:{id}"), "set_module_enabled", json!({ "id": id, "enabled": !off }), move |state: &mut S, _, _| {
            if let Loaded::Ready(set) = &mut state.menu().disabled_modules {
                if off { set.insert(id_owned.clone()); } else { set.remove(&id_owned); }
            }
        });
    }
    if !module.backend || !off {
        let op = if next == ModuleState::Desk { "show" } else { "hide" };
        state.apply(vec![json!({ "op": op, "id": id })], tx);
    }
}

/// The sheet over the menu for one widget or one folder.
pub fn sheet<S: MenuHost>(state: &S, look: &Look, theme: &Theme, sheet: &Sheet, width: f32, height: f32) -> El {
    match sheet {
        Sheet::Widget(id) => widget_sheet(state, look, theme, id, width, height),
        Sheet::Folder(id) => crate::menu_folders::sheet(state, look, theme, id, width, height),
    }
}

fn widget_sheet<S: MenuHost>(state: &S, look: &Look, theme: &Theme, id: &str, width: f32, height: f32) -> El {
    let Some(module) = modules().into_iter().find(|m| m.id == id) else {
        return menu::sheet_frame::<S>(state, look, theme, id, width, height, col());
    };
    let hidden = state.layout().map(|l| l.hidden.clone()).unwrap_or_default();
    let disabled = state.menu_ref().disabled_modules.ready().cloned().unwrap_or_default();
    let current = module_state(&module, &hidden, &disabled);
    let host = nesting_host(state, id);
    let options: Vec<(&str, &str)> = if module.backend {
        vec![("desk", "Включить"), ("background", "Скрыть"), ("off", "Выключить")]
    } else {
        vec![("desk", "Включить"), ("off", "Скрыть")]
    };
    let checked = match current {
        ModuleState::Desk => "desk",
        ModuleState::Background => "background",
        ModuleState::Off => "off",
    };
    let id_owned = id.to_string();
    let mut body = col().w_full()
        .child(controls::note(look, &module.description))
        .child(controls::pills(look, &format!("hit:state:{id}"), &options, checked, move |state: &mut S, value: &str, tx: &mut Tx| {
            let next = match value {
                "desk" => ModuleState::Desk,
                "background" => ModuleState::Background,
                _ => ModuleState::Off,
            };
            set_module_state(state, &id_owned, next, tx);
        }))
        .child(controls::note(look, &match &host {
            Some(host) => format!("Сейчас внутри «{host}»."),
            None => format!("Сейчас {}.", state_noun(&module, current)),
        }));
    let mut actions = row().gap(8.0).py(8.0).flex_wrap();
    if host.is_some() {
        let id = id.to_string();
        actions = actions.child(controls::button(look, "hit:eject".into(), "Вынуть на стол", ButtonKind::Normal, move |state: &mut S, tx: &mut Tx| {
            state.apply(vec![json!({ "op": "eject", "id": id })], tx);
        }));
    }
    if state.settings_factory(id).is_some() {
        let id = id.to_string();
        actions = actions.child(controls::button(look, "hit:module-settings".into(), &format!("Настроить {}…", module.name), ButtonKind::Normal, move |state: &mut S, tx: &mut Tx| {
            menu::open_module_settings(state, &id, tx);
        }));
    }
    body = body.child(actions);
    menu::sheet_frame::<S>(state, look, theme, &module.name, width, height, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_states_follow_the_page() {
        let backend = Module { id: "example-backend".into(), name: String::new(), description: String::new(), icon: String::new(), backend: true };
        let plain = Module { id: "example".into(), name: String::new(), description: String::new(), icon: String::new(), backend: false };
        let none = HashSet::new();
        let off: HashSet<String> = [backend.id.clone()].into();
        assert_eq!(module_state(&backend, &[], &none), ModuleState::Desk);
        assert_eq!(module_state(&backend, &[backend.id.clone()], &none), ModuleState::Background);
        assert_eq!(module_state(&backend, &[], &off), ModuleState::Off);
        assert_eq!(module_state(&plain, &[plain.id.clone()], &none), ModuleState::Off);
        assert_eq!(state_noun(&plain, ModuleState::Off), "скрыт");
        assert_eq!(state_noun(&backend, ModuleState::Off), "выключен");
    }

    #[test]
    fn every_folder_icon_has_a_label() {
        assert_eq!(FOLDER_ICONS.len(), 9);
        assert!(folder_icon("nope").contains("M22 19a2"), "unknown keys fall back to the folder");
    }
}
