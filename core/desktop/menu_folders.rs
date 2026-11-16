//! The folders of the menu's "Стол" pane and the folder sheet (`renderFolders`, `folderCard`,
//! `bindStartPicker` in `menu.js`; the operations of `folders.js`): "+ Папка" with its nine
//! icons, one row per folder with "Настроить…" and its desk switch, and in the sheet the
//! name, deletion, the shortcuts (rename, relink a missing target, remove) and the ways to add
//! one. Everything that loses something is offered back as "Отменить"
//! (`r[desk.no-blocking-dialogs]`); folder records go through `save_folder` and
//! `delete_folder`, the desk's side through `desk_apply` (`r[desk.folders]`).

use rune::el::{col, el, row, text, text_input, El};
use rune::{Color, TextEdit, Tx};
use serde_json::{json, Value};

use crate::controls::{self, ButtonKind, Look};
use crate::menu::{self, shortcut_count, Loaded, MenuHost, Sheet};
use crate::menu_desk::{folder_icon, FOLDER_ICONS};
use crate::theme::Theme;

fn folder<S: MenuHost>(state: &S, id: &str) -> Option<Value> {
    state.menu_ref().folders.ready()?.iter().find(|f| f["id"] == id).cloned()
}

fn uuid() -> String {
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(4) {
        chunk.copy_from_slice(&crate::theme_record::new_color_seed().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// `store`: a folder record goes to the kernel and the list is read again.
fn store<S: MenuHost>(state: &mut S, key: &str, record: Value, then: impl FnOnce(&mut S, &mut Tx) + Send + 'static) {
    menu::run_once(state, key, "save_folder", json!({ "folder": record }), move |state: &mut S, _, tx: &mut Tx| {
        crate::menu_desk::load_folders(state);
        then(state, tx);
    });
}

fn create_folder<S: MenuHost>(state: &mut S) {
    let name = state.menu_ref().new_folder_name.text.trim().to_string();
    let name = if name.is_empty() { "Новая папка".to_string() } else { name };
    let icon = state.menu_ref().new_folder_icon;
    let record = json!({ "id": format!("folder_{}", uuid()), "name": name, "icon_svg": folder_icon(icon), "items": [] });
    store(state, "folder-create", record, move |state: &mut S, _tx: &mut Tx| {
        let menu = state.menu();
        menu.creating_folder = false;
        menu.new_folder_name = TextEdit::new("");
        state.toasts().show(format!("Папка «{name}» появилась на столе"), false, None);
    });
}

/// `deleteFolder`: deleted at once; "Отменить" brings back the record and what the desk kept
/// for it (position, hidden flag, placed instances).
fn delete_folder<S: MenuHost>(state: &mut S, id: &str, tx: &mut Tx) {
    let Some(record) = folder(state, id) else { return };
    let config = state.layout().map(|l| l.config.clone()).unwrap_or(json!({}));
    let position = config["positions"].get(id).cloned();
    let hidden = config["hidden"].as_array().is_some_and(|h| h.iter().any(|x| x == id));
    let instances: Vec<Value> = config["instances"].as_array().into_iter().flatten().filter(|it| it["widgetType"] == id).cloned().collect();
    let id = id.to_string();
    let name = record["name"].as_str().unwrap_or_default().to_string();
    menu::run_once(state, &format!("folder-delete:{id}"), "delete_folder", json!({ "id": id }), move |state: &mut S, _, tx: &mut Tx| {
        let mut ops = vec![json!({ "op": "forget", "id": id }), json!({ "op": "show", "id": id })];
        for instance in &instances {
            ops.push(json!({ "op": "remove_instance", "id": instance["instanceId"] }));
        }
        state.apply(ops, tx);
        state.menu().sheet = None;
        crate::menu_desk::load_folders(state);
        menu::undo_toast(state, format!("Папка «{name}» удалена"), Box::new(move |state: &mut S, _tx: &mut Tx| {
            let (position, instances, id) = (position.clone(), instances.clone(), id.clone());
            store(state, &format!("folder-restore:{id}"), record.clone(), move |state: &mut S, tx: &mut Tx| {
                let mut ops = Vec::new();
                if let Some(position) = position {
                    ops.push(json!({ "op": "set_position", "id": id, "record": position }));
                }
                if hidden {
                    ops.push(json!({ "op": "hide", "id": id }));
                }
                for instance in instances {
                    ops.push(json!({ "op": "add_instance", "id": instance["instanceId"], "type": instance["widgetType"] }));
                }
                if !ops.is_empty() {
                    state.apply(ops, tx);
                }
            });
        }));
    });
    let _ = tx;
}

/// `addItems`: shortcuts from drafts (name, target, args, kind), appended.
pub(crate) fn add_items<S: MenuHost>(state: &mut S, id: &str, drafts: Vec<Value>) {
    let Some(mut record) = folder(state, id) else { return };
    let fresh: Vec<Value> = drafts.into_iter().filter(|d| d["target"].as_str().is_some_and(|t| !t.is_empty())).collect();
    if fresh.is_empty() {
        return;
    }
    let folder_name = record["name"].as_str().unwrap_or_default().to_string();
    let message = if fresh.len() == 1 {
        format!("«{}» добавлен в «{folder_name}»", fresh[0]["name"].as_str().unwrap_or_default())
    } else {
        format!("{} добавлено в «{folder_name}»", shortcut_count(fresh.len()))
    };
    let items = record["items"].as_array_mut().map(std::mem::take).unwrap_or_default();
    let mut items = items;
    for draft in fresh {
        items.push(json!({
            "id": format!("item_{}", uuid()),
            "name": draft["name"],
            "target": draft["target"],
            "args": draft["args"].as_str().unwrap_or_default(),
            "kind": draft["kind"].as_str().unwrap_or("file"),
        }));
    }
    record["items"] = Value::Array(items);
    store(state, &format!("folder-add:{id}"), record, move |state: &mut S, _tx: &mut Tx| {
        state.toasts().show(message, false, None);
    });
}

pub(crate) fn remove_item<S: MenuHost>(state: &mut S, folder_id: &str, item_id: &str) {
    let Some(mut record) = folder(state, folder_id) else { return };
    let items = record["items"].as_array().cloned().unwrap_or_default();
    let Some(index) = items.iter().position(|it| it["id"] == item_id) else { return };
    let removed = items[index].clone();
    record["items"] = Value::Array(items.into_iter().filter(|it| it["id"] != item_id).collect());
    let name = removed["name"].as_str().unwrap_or_default().to_string();
    let folder_id = folder_id.to_string();
    store(state, &format!("item-remove:{item_id}"), record, move |state: &mut S, _tx: &mut Tx| {
        menu::undo_toast(state, format!("Ярлык «{name}» убран"), Box::new(move |state: &mut S, _tx: &mut Tx| {
            // Put back at the same place in the folder as it is now.
            let Some(mut current) = folder(state, &folder_id) else { return };
            let mut items = current["items"].as_array().cloned().unwrap_or_default();
            items.insert(index.min(items.len()), removed.clone());
            current["items"] = Value::Array(items);
            store(state, &format!("item-restore:{folder_id}"), current, |_, _| {});
        }));
    });
}

pub(crate) fn update_item<S: MenuHost>(state: &mut S, folder_id: &str, item_id: &str, change: impl Fn(&mut Value)) {
    let Some(mut record) = folder(state, folder_id) else { return };
    if let Some(items) = record["items"].as_array_mut() {
        for item in items.iter_mut().filter(|it| it["id"] == item_id) {
            change(item);
        }
    }
    store(state, &format!("item-update:{item_id}"), record, |_, _| {});
}

pub fn section<S: MenuHost>(state: &S, look: &Look) -> El {
    let mut section = col().w_full();
    let creating = state.menu_ref().creating_folder;
    section = section.child(row().py(8.0).child(controls::button(look, "hit:folder-new".into(), "+ Папка", ButtonKind::Normal, |state: &mut S, _tx: &mut Tx| {
        let menu = state.menu();
        menu.creating_folder = !menu.creating_folder;
    })));
    if creating {
        let name_field = text_input(&state.menu_ref().new_folder_name, look.font(13.0, 400.0))
            .single_line().key("hit:folder-new-name")
            .placeholder("Название, например «Игры»", look.fg.alpha(0.55))
            .on_edit(|state: &mut S| &mut state.menu().new_folder_name)
            .on_submit(|state: &mut S, _tx: &mut Tx| create_folder(state));
        let chosen = state.menu_ref().new_folder_icon;
        let mut icons = row().gap(6.0).flex_wrap();
        for (key, _, svg) in FOLDER_ICONS {
            let icon = match state.svg(svg, 22.0, look.fg) {
                Some(pixels) => el().size(22.0, 22.0).pixels(&pixels),
                None => el().size(22.0, 22.0),
            };
            icons = icons.child(
                col().key(format!("hit:folder-icon:{key}")).size(36.0, 36.0).radius(10.0).items_center().justify_center()
                    .border(1.0, if key == chosen { look.accent } else { look.elev.alpha(0.14) })
                    .fill(if key == chosen { look.elev.alpha(0.14) } else { Color::TRANSPARENT })
                    .child(icon)
                    .on_press_tx(move |state: &mut S, _tx: &mut Tx| state.menu().new_folder_icon = key),
            );
        }
        section = section.child(
            col().gap(8.0).py(8.0)
                .child(el().w_full().min_h(32.0).px(10.0).py(6.0).radius(9.0).border(1.0, look.elev.alpha(0.16)).fill(look.well.alpha(0.25)).child(name_field))
                .child(icons)
                .child(
                    row().gap(8.0)
                        .child(controls::button(look, "hit:folder-create".into(), "Создать", ButtonKind::Primary, |state: &mut S, _tx: &mut Tx| create_folder(state)))
                        .child(controls::button(look, "hit:folder-cancel".into(), "Отмена", ButtonKind::Normal, |state: &mut S, _tx: &mut Tx| state.menu().creating_folder = false)),
                ),
        );
    }
    match state.menu_ref().folders.clone() {
        Loaded::Loading => section.child(text("Загрузка…").font(look.font(13.0, 400.0)).color(look.fg.alpha(0.78))),
        Loaded::Failed(error) => section.child(
            col().gap(10.0).px(4.0).py(16.0)
                .child(text(format!("Не удалось загрузить папки: {error}")).font(look.font(13.0, 400.0)).color(look.status_error).wrap())
                .child(row().child(controls::button(look, "hit:retry-folders".into(), "Повторить", ButtonKind::Normal, |state: &mut S, _tx: &mut Tx| crate::menu_desk::load_folders(state)))),
        ),
        Loaded::Ready(folders) if folders.is_empty() => section.child(
            el().px(4.0).py(16.0).child(text("Папок пока нет. Папка — это значок на столе, внутри которого лежат ваши программы, файлы и сайты.").font(look.font(13.0, 400.0).lh(13.0 * 1.5)).color(look.fg.alpha(0.78)).wrap()),
        ),
        Loaded::Ready(folders) => {
            let hidden = state.layout().map(|l| l.hidden.clone()).unwrap_or_default();
            let mut list = col().gap(2.0);
            for record in folders {
                let id = record["id"].as_str().unwrap_or_default().to_string();
                let name = record["name"].as_str().unwrap_or_default().to_string();
                let svg = record["icon_svg"].as_str().filter(|s| !s.is_empty()).unwrap_or(folder_icon("folder")).to_string();
                let count = record["items"].as_array().map_or(0, Vec::len);
                let shown = !hidden.iter().any(|h| *h == id);
                let icon = match state.svg(&svg, 20.0, look.fg.alpha(0.85)) {
                    Some(pixels) => el().size(20.0, 20.0).pixels(&pixels),
                    None => el().size(20.0, 20.0),
                };
                let (open_id, toggle_id) = (id.clone(), id.clone());
                list = list.child(
                    row().key(format!("hit:folder-row:{id}")).w_full().min_h(44.0).px(8.0).py(6.0).gap(10.0).radius(12.0).items_center().hover(look.elev.alpha(0.07))
                        .child(col().size(28.0, 28.0).items_center().justify_center().child(icon))
                        .child(
                            col().grow(1.0).min_w(0.0).gap(2.0)
                                .child(text(name).font(look.font(13.0, 400.0)).color(look.fg).single_line().truncate())
                                .child(text(shortcut_count(count)).font(look.font(12.0, 400.0)).color(look.fg.alpha(0.72)).single_line()),
                        )
                        .child(controls::button(look, format!("hit:folder-open:{id}"), "Настроить…", ButtonKind::Normal, move |state: &mut S, _tx: &mut Tx| {
                            state.menu().sheet = Some(Sheet::Folder(open_id.clone()));
                        }))
                        .child(controls::switch(look, format!("hit:folder-shown:{id}"), shown, move |state: &mut S, tx: &mut Tx| {
                            let op = if shown { "hide" } else { "show" };
                            state.apply(vec![json!({ "op": op, "id": toggle_id })], tx);
                        })),
                );
            }
            section.child(list)
        }
    }
}

pub fn sheet<S: MenuHost>(state: &S, look: &Look, theme: &Theme, id: &str, width: f32, height: f32) -> El {
    let Some(record) = folder(state, id) else {
        return menu::sheet_frame::<S>(look, theme, "Папка", width, height, col());
    };
    let id = id.to_string();
    let items = record["items"].as_array().cloned().unwrap_or_default();
    let name = record["name"].as_str().unwrap_or_default().to_string();
    let svg = record["icon_svg"].as_str().filter(|s| !s.is_empty()).unwrap_or(folder_icon("folder")).to_string();
    let head_icon = match state.svg(&svg, 22.0, look.fg) {
        Some(pixels) => el().size(22.0, 22.0).pixels(&pixels),
        None => el().size(22.0, 22.0),
    };
    let (rename_id, edit_id, delete_id) = (id.clone(), id.clone(), id.clone());
    let shown_name = state.menu_ref().folder_names.get(&id).cloned().unwrap_or_else(|| TextEdit::new(name.clone()));
    let first_name = name.clone();
    let name_field = text_input(&shown_name, look.font(13.0, 600.0))
        .single_line().key(format!("hit:folder-name:{id}"))
        .on_edit(move |state: &mut S| state.menu().folder_names.entry(edit_id.clone()).or_insert_with(|| TextEdit::new(first_name.clone())))
        .on_submit(move |state: &mut S, _tx: &mut Tx| {
            // An empty name puts the old one back.
            let Some(mut record) = folder(state, &rename_id) else { return };
            let typed = state.menu().folder_names.get(&rename_id).map(|e| e.text.trim().to_string()).unwrap_or_default();
            if typed.is_empty() {
                let old = record["name"].as_str().unwrap_or_default().to_string();
                state.menu().folder_names.insert(rename_id.clone(), TextEdit::new(old));
                return;
            }
            record["name"] = json!(typed);
            let id = rename_id.clone();
            store(state, &format!("folder-rename:{id}"), record, |_, _| {});
        });
    let trash = [([3.0, 6.0], [21.0, 6.0]), ([19.0, 6.0], [19.0, 20.0]), ([5.0, 6.0], [5.0, 20.0]), ([5.0, 20.0], [19.0, 20.0]), ([8.0, 6.0], [8.0, 4.0]), ([16.0, 6.0], [16.0, 4.0]), ([8.0, 3.0], [16.0, 3.0])];
    let head = row().w_full().gap(8.0).items_center().mb(4.0)
        .child(col().w(32.0).items_center().child(head_icon))
        .child(el().grow(1.0).min_h(32.0).px(10.0).py(6.0).radius(9.0).border(1.0, look.elev.alpha(0.16)).fill(look.well.alpha(0.25)).child(name_field))
        .child(text(shortcut_count(items.len())).font(look.font(12.0, 400.0)).color(look.fg.alpha(0.72)).single_line())
        .child(controls::icon_button(look, format!("hit:folder-delete:{id}"), controls::glyph(&trash, look.fg.alpha(0.75), 2.0), move |state: &mut S, tx: &mut Tx| delete_folder(state, &delete_id, tx)));

    let mut card = col().w_full().my(10.0).padding(8.0).radius(16.0).fill(look.elev.alpha(0.05)).border(1.0, look.elev.alpha(0.08)).child(head);
    if items.is_empty() {
        card = card.child(controls::note(look, "Пусто. Добавьте программу из Пуска, файл или папку."));
    } else {
        let missing = state.menu_ref().missing.clone();
        let cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
        let mut list = col().gap(2.0);
        for item in &items {
            let item_id = item["id"].as_str().unwrap_or_default().to_string();
            let target = item["target"].as_str().unwrap_or_default().to_string();
            let gone = missing.contains(&target);
            let icon = match state.shortcut_icon(&target) {
                Some(pixels) => el().size(24.0, 24.0).pixels(&pixels),
                None => {
                    let glyph = match item["kind"].as_str() {
                        Some("url") => Some(folder_icon("globe")),
                        Some("folder") => Some(folder_icon("folder")),
                        _ => None,
                    };
                    match glyph.and_then(|svg| state.svg(svg, 20.0, look.fg.alpha(0.85))) {
                        Some(pixels) => el().size(20.0, 20.0).pixels(&pixels),
                        None => el().size(20.0, 20.0),
                    }
                }
            };
            let (edit_item, submit_item, submit_folder) = (item_id.clone(), item_id.clone(), id.clone());
            let item_name = item["name"].as_str().unwrap_or_default().to_string();
            let shown_item = state.menu_ref().item_names.get(&item_id).cloned().unwrap_or_else(|| TextEdit::new(item_name.clone()));
            let input = text_input(&shown_item, look.font(13.0, 400.0))
                .single_line().key(format!("hit:item-name:{item_id}"))
                .on_edit(move |state: &mut S| state.menu().item_names.entry(edit_item.clone()).or_insert_with(|| TextEdit::new(item_name.clone())))
                .on_submit(move |state: &mut S, _tx: &mut Tx| {
                    let typed = state.menu().item_names.get(&submit_item).map(|e| e.text.trim().to_string()).unwrap_or_default();
                    if typed.is_empty() {
                        return;
                    }
                    update_item(state, &submit_folder, &submit_item, |it| it["name"] = json!(typed.clone()));
                });
            let mut line = row().w_full().min_h(44.0).px(8.0).py(6.0).gap(10.0).radius(12.0).items_center().hover(look.elev.alpha(0.07))
                .child(col().size(28.0, 28.0).items_center().justify_center().opacity(if gone { 0.45 } else { 1.0 }).child(icon))
                .child({
                    let mut text_col = col().grow(1.0).min_w(0.0).gap(2.0)
                        .child(el().w_full().min_h(28.0).px(6.0).py(3.0).radius(9.0).hover(look.elev.alpha(0.0)).child(input));
                    if gone {
                        text_col = text_col.child(text("Недоступен — файл перемещён или удалён").font(look.font(12.0, 400.0)).color(look.status_warn).single_line());
                    }
                    text_col
                });
            if gone {
                let (relink_folder, relink_item, is_folder) = (id.clone(), item_id.clone(), item["kind"] == "folder");
                line = line.child(controls::button(look, format!("hit:item-relink:{item_id}"), "Найти…", ButtonKind::Normal, move |state: &mut S, _tx: &mut Tx| {
                    let (folder_id, item_id) = (relink_folder.clone(), relink_item.clone());
                    menu::run_once(state, &format!("relink:{item_id}"), "pick_shortcut_target", json!({ "folder": is_folder }), move |state: &mut S, draft, _tx: &mut Tx| {
                        if draft.is_null() {
                            return;
                        }
                        update_item(state, &folder_id, &item_id, |it| {
                            it["target"] = draft["target"].clone();
                            it["args"] = json!(draft["args"].as_str().unwrap_or_default());
                            it["kind"] = draft["kind"].clone();
                        });
                    });
                }));
            }
            let (remove_folder, remove_item_id) = (id.clone(), item_id.clone());
            line = line.child(controls::icon_button(look, format!("hit:item-remove:{item_id}"), controls::glyph(&cross, look.fg.alpha(0.75), 2.0), move |state: &mut S, _tx: &mut Tx| {
                remove_item(state, &remove_folder, &remove_item_id);
            }));
            list = list.child(line);
        }
        card = card.child(list);
    }

    let picker_open = state.menu_ref().start_for.as_deref() == Some(id.as_str());
    let (start_id, file_id, dir_id, paste_id) = (id.clone(), id.clone(), id.clone(), id.clone());
    card = card.child(
        row().gap(8.0).mt(6.0).mb(2.0).flex_wrap()
            .child(controls::button(look, "hit:from-start".into(), "Из Пуска", ButtonKind::Normal, move |state: &mut S, _tx: &mut Tx| {
                let menu = state.menu();
                menu.start_for = if menu.start_for.as_deref() == Some(start_id.as_str()) { None } else { Some(start_id.clone()) };
                if menu.start_for.is_some() && !matches!(menu.start_list, Loaded::Ready(_)) {
                    load_start(state);
                }
            }))
            .child(controls::button(look, "hit:pick-file".into(), "Файл…", ButtonKind::Normal, move |state: &mut S, _tx: &mut Tx| {
                let id = file_id.clone();
                menu::run_once(state, "pick-file", "pick_shortcut_target", json!({ "folder": false }), move |state: &mut S, draft, _tx: &mut Tx| {
                    if !draft.is_null() {
                        add_items(state, &id, vec![draft]);
                    }
                });
            }))
            .child(controls::button(look, "hit:pick-folder".into(), "Папку…", ButtonKind::Normal, move |state: &mut S, _tx: &mut Tx| {
                let id = dir_id.clone();
                menu::run_once(state, "pick-folder", "pick_shortcut_target", json!({ "folder": true }), move |state: &mut S, draft, _tx: &mut Tx| {
                    if !draft.is_null() {
                        add_items(state, &id, vec![draft]);
                    }
                });
            }))
            .child(controls::button(look, "hit:paste".into(), "Вставить", ButtonKind::Normal, move |state: &mut S, _tx: &mut Tx| paste(state, &paste_id))),
    );
    if picker_open {
        card = card.child(start_picker(state, look, &id));
    }
    menu::sheet_frame::<S>(look, theme, "Папка", width, height, card)
}

/// `clipboardDrafts`: copied files first, else a link or a path typed as text.
pub(crate) fn paste<S: MenuHost>(state: &mut S, id: &str) {
    let id = id.to_string();
    menu::run_once(state, "paste", "clipboard_shortcut_targets", json!({}), move |state: &mut S, clip, _tx: &mut Tx| {
        let files: Vec<String> = clip["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        let targets: Vec<String> = if files.is_empty() {
            clip["text"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(|t| vec![t.to_string()]).unwrap_or_default()
        } else {
            files
        };
        if targets.is_empty() {
            state.toasts().show("В буфере обмена нет ни файлов, ни ссылки", false, None);
            return;
        }
        describe_all(state, id, targets, Vec::new());
    });
}

/// Describes the targets one after another, then adds them together.
fn describe_all<S: MenuHost>(state: &mut S, id: String, mut targets: Vec<String>, mut drafts: Vec<Value>) {
    let Some(target) = targets.pop() else {
        drafts.reverse();
        add_items(state, &id, drafts);
        return;
    };
    state.call("describe_shortcut_target", json!({ "target": target }), Box::new(move |state: &mut S, result, _tx: &mut Tx| match result {
        Ok(draft) => {
            drafts.push(draft);
            describe_all(state, id, targets, drafts);
        }
        Err(error) => menu::error_toast(state, &format!("Не получилось: {error}")),
    }));
}

/// A folder record saved without a toast (a reorder).
pub(crate) fn store_quiet<S: MenuHost>(state: &mut S, id: &str, record: Value) {
    store(state, &format!("folder-order:{id}"), record, |_, _| {});
}

/// `describeShortcutTarget` for each path, then `addItems` (files dropped from Explorer).
pub(crate) fn describe_and_add<S: MenuHost>(state: &mut S, id: String, targets: Vec<String>) {
    let mut targets = targets;
    targets.reverse();
    describe_all(state, id, targets, Vec::new());
}

/// `openFoldersFor`: the menu on the folder's sheet with the Start list open.
pub(crate) fn open_start_picker<S: MenuHost>(state: &mut S, id: &str, tx: &mut Tx) {
    if !state.menu_ref().open {
        menu::toggle(state, tx);
    } else {
        menu::show_view(state, crate::menu::View::Desk, tx);
    }
    let menu = state.menu();
    menu.sheet = Some(Sheet::Folder(id.to_string()));
    menu.start_for = Some(id.to_string());
    if !matches!(state.menu_ref().start_list, Loaded::Ready(_)) {
        load_start(state);
    }
}

fn load_start<S: MenuHost>(state: &mut S) {
    state.menu().start_list = Loaded::Loading;
    state.call("start_menu_list", json!({}), Box::new(|state: &mut S, result, _tx: &mut Tx| {
        state.menu().start_list = match result {
            Ok(list) => Loaded::Ready(list.as_array().cloned().unwrap_or_default()),
            Err(error) => Loaded::Failed(error),
        };
    }));
}

/// `bindStartPicker`: the Start menu's programs filtered by the search, at most 200 shown.
fn start_picker<S: MenuHost>(state: &S, look: &Look, id: &str) -> El {
    let search = text_input(&state.menu_ref().start_search, look.font(13.0, 400.0))
        .single_line().key("hit:start-search")
        .placeholder("Поиск программы", look.fg.alpha(0.55))
        .on_edit(|state: &mut S| &mut state.menu().start_search);
    let mut picker = col().gap(6.0).mt(6.0)
        .child(el().w_full().min_h(32.0).px(10.0).py(6.0).radius(9.0).border(1.0, look.elev.alpha(0.16)).fill(look.well.alpha(0.25)).child(search));
    let note = |message: &str, error: bool| text(message.to_string()).font(look.font(13.0, 400.0)).color(if error { look.status_error } else { look.fg.alpha(0.78) }).wrap();
    match state.menu_ref().start_list.clone() {
        Loaded::Loading => picker.child(note("Загрузка…", false)),
        Loaded::Failed(error) => picker.child(
            col().gap(10.0).child(note(&format!("Не удалось прочитать меню Пуск: {error}"), true))
                .child(row().child(controls::button(look, "hit:start-retry".into(), "Повторить", ButtonKind::Normal, |state: &mut S, _tx: &mut Tx| load_start(state)))),
        ),
        Loaded::Ready(apps) if apps.is_empty() => picker.child(note("В меню Пуск нет программ", false)),
        Loaded::Ready(apps) => {
            let query = state.menu_ref().start_search.text.trim().to_lowercase();
            let shown: Vec<&Value> = apps.iter().filter(|a| query.is_empty() || a["name"].as_str().is_some_and(|n| n.to_lowercase().contains(&query))).take(200).collect();
            if shown.is_empty() {
                return picker.child(note("Ничего не нашлось", false));
            }
            let mut list = col().gap(2.0).max_h(280.0).scroll(rune::ScrollSpec { stick_bottom: false });
            for (index, app) in shown.into_iter().enumerate() {
                let target = app["target"].as_str().unwrap_or_default().to_string();
                let icon = match state.shortcut_icon(&target) {
                    Some(pixels) => el().size(24.0, 24.0).pixels(&pixels),
                    None => el().size(24.0, 24.0),
                };
                let (draft, id) = (json!({ "name": app["name"], "target": app["target"], "args": app["args"], "kind": app["kind"] }), id.to_string());
                list = list.child(
                    row().key(format!("hit:start-app:{index}")).w_full().min_h(36.0).px(8.0).py(4.0).gap(10.0).radius(10.0).items_center().hover(look.elev.alpha(0.1))
                        .child(col().size(28.0, 28.0).items_center().justify_center().child(icon))
                        .child(el().grow(1.0).child(text(app["name"].as_str().unwrap_or_default().to_string()).font(look.font(13.0, 400.0)).color(look.fg).single_line().truncate()))
                        .on_press_tx(move |state: &mut S, _tx: &mut Tx| add_items(state, &id, vec![draft.clone()])),
                );
            }
            picker.child(list)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uuids_look_like_crypto_random_uuids() {
        let id = super::uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
    }
}
