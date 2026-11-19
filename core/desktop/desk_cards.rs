//! What the desk shows (`getWidgets`, `desiredWidgets` and `nesting.js` in the page): a card
//! for every widget a module's native half draws and for every folder, which widgets sit
//! inside which host, and where each card lands. Folders are the shell's own cards: their
//! records are the kernel's (`get_folders`, `save_folder`), their body is drawn here
//! (`renderFolderBody`), and the desk's side (hidden, instances, nesting) goes through
//! `desk_apply` (`r[desk.folders]`, `r[desk.nesting]`).

use std::collections::{BTreeMap, HashSet};

use rune::el::{col, el, row, text, text_input, Align, El};
use rune::{Color, TextStyle, Tx};
use serde_json::{json, Value};

use crate::card::BODY_W;
use crate::desk::{Layout, Placement};
use crate::menu::{self, MenuHost};
use crate::shell::Host;
use crate::theme::{Material, Theme};

/// `DRAG_START_PX`: a press that moves this far is a drag, not a click.
const DRAG_START: f32 = 6.0;

const FOLDER_FALLBACK_ICON: &str = r#"<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>"#;

/// A card the desk draws, as `getWidgets` and `desiredWidgets` describe it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// The desk id: the folder or module id, or an instance's id.
    pub id: String,
    pub widget_type: String,
    pub name: String,
    pub desc: String,
    pub icon: String,
    /// A folder's shortcuts as the kernel stores them.
    pub items: Vec<Value>,
    pub instance: bool,
    /// One of the shell's folders; otherwise a module's widget.
    pub folder: bool,
    /// `"chrome": "bare"`: no header, the widget draws the whole card (`.no-header`).
    pub bare: bool,
    /// `"radius": "strip"`: a full pill under jelly.
    pub strip: bool,
    /// The module's first tint: its id, role and default palette key.
    pub tint: (String, String, String),
}

impl Card {
    /// `--own-tint`: a module's first tint target, else the folders' shared one.
    pub fn tint(&self, theme: &Theme) -> Color {
        if self.folder {
            theme.tint("folder", "folder", "mint")
        } else {
            theme.tint(&self.tint.0, &self.tint.1, &self.tint.2)
        }
    }

    /// A module's card is resized on every side and opens from anywhere on it (`specialType`);
    /// a folder is resized in width only and opens from its header.
    pub fn special(&self) -> bool {
        !self.folder
    }
}

/// One widget of the page's list, before the desk filters it.
#[derive(Clone, Debug)]
struct Widget {
    id: String,
    widget_type: String,
    name: String,
    container: bool,
    embeddable: bool,
}

/// A widget on screen: its name, its type, whether it hosts others and whether it has a
/// compact form to be nested in.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub widget_type: String,
    pub container: bool,
    pub embeddable: bool,
}

/// What the desk shows now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desk {
    /// The cards on the desk, in the page's order.
    pub cards: Vec<Card>,
    /// `effectiveNested`: the hosts and the widgets that sit inside them now.
    pub nested: BTreeMap<String, Vec<String>>,
    /// Every widget on screen by desk id, for `canNest` and the nesting zone.
    pub widgets: BTreeMap<String, Entry>,
}

impl Desk {
    /// `canNest`: `None` when `child` may enter `host`, else the refusal the page shows.
    pub fn refusal(&self, child: &str, host: &str) -> Option<String> {
        let (Some(c), Some(h)) = (self.widgets.get(child), self.widgets.get(host)) else { return Some("Виджет не найден".into()) };
        if child == host {
            Some("Виджет нельзя вложить сам в себя".into())
        } else if !h.container {
            Some(format!("«{}» не может вмещать другие виджеты", h.name))
        } else if c.container {
            Some(format!("«{}» сам вмещает виджеты, такой нельзя вложить", c.name))
        } else if !c.embeddable {
            Some(format!("У «{}» нет компактного вида, вложить нельзя", c.name))
        } else {
            None
        }
    }

    pub fn card(&self, id: &str) -> Option<&Card> {
        self.cards.iter().find(|card| card.id == id)
    }
}

/// `getWidgets` + `desiredWidgets` + `effectiveNested`: modules and folders, minus the hidden
/// and the switched-off, plus the placed instances; the nested ones leave the desk. Only the
/// modules `drawn` names become cards: the ones with a native half.
pub fn desk(layout: &Layout, folders: &[Value], disabled: &HashSet<String>, drawn: &dyn Fn(&str) -> bool) -> Desk {
    let mut base = Vec::new();
    let mut cards_of: BTreeMap<String, Card> = BTreeMap::new();
    for module in crate::catalog::desk_modules() {
        let meta = &module.meta;
        if meta["backend"] == true && disabled.contains(&module.id) {
            continue;
        }
        let name = meta["name"].as_str().unwrap_or(&module.id).to_string();
        base.push(Widget {
            id: module.id.clone(),
            widget_type: module.id.clone(),
            name: name.clone(),
            container: meta["container"] == true,
            embeddable: meta["embeddable"].is_object() || meta["embeddable"] == true,
        });
        if drawn(&module.id) {
            let tint = &meta["tints"][0];
            let field = |key: &str, default: &str| tint[key].as_str().unwrap_or(default).to_string();
            cards_of.insert(module.id.clone(), Card {
                id: module.id.clone(),
                widget_type: module.id.clone(),
                name,
                desc: meta["description"].as_str().unwrap_or_default().to_string(),
                icon: module.icon.clone(),
                items: Vec::new(),
                instance: false,
                folder: false,
                bare: meta["chrome"] == "bare",
                strip: meta["radius"] == "strip",
                tint: (field("id", &module.id), field("role", "calm"), field("default", "lilac")),
            });
        }
    }
    for folder in folders {
        let Some(id) = folder["id"].as_str() else { continue };
        let items = folder["items"].as_array().cloned().unwrap_or_default();
        base.push(Widget { id: id.into(), widget_type: id.into(), name: folder["name"].as_str().unwrap_or_default().into(), container: true, embeddable: false });
        let icon = folder["icon_svg"].as_str().filter(|s| !s.is_empty()).unwrap_or(FOLDER_FALLBACK_ICON).to_string();
        cards_of.insert(id.into(), Card {
            id: id.into(),
            widget_type: id.into(),
            name: folder["name"].as_str().unwrap_or_default().into(),
            desc: menu::shortcut_count(items.len()),
            icon,
            items,
            instance: false,
            folder: true,
            bare: false,
            strip: false,
            tint: ("folder".into(), "folder".into(), "mint".into()),
        });
    }

    let mut render: Vec<Widget> = base.iter().filter(|w| !layout.is_hidden(&w.id)).cloned().collect();
    for instance in layout.config["instances"].as_array().into_iter().flatten() {
        let (Some(id), Some(kind)) = (instance["instanceId"].as_str(), instance["widgetType"].as_str()) else { continue };
        if layout.is_hidden(kind) {
            continue;
        }
        if let Some(of) = base.iter().find(|w| w.id == kind) {
            render.push(Widget { id: id.into(), ..of.clone() });
        }
    }

    let mut desk = Desk {
        widgets: render.iter().map(|w| (w.id.clone(), Entry { name: w.name.clone(), widget_type: w.widget_type.clone(), container: w.container, embeddable: w.embeddable })).collect(),
        ..Desk::default()
    };
    let mut taken = HashSet::new();
    for (host, children) in &layout.nested {
        if !desk.widgets.contains_key(host) {
            continue;
        }
        for child in children {
            if taken.contains(child) || desk.refusal(child, host).is_some() {
                continue;
            }
            taken.insert(child.clone());
            desk.nested.entry(host.clone()).or_default().push(child.clone());
        }
    }
    for widget in render.iter().filter(|w| !taken.contains(&w.id)) {
        if let Some(of) = cards_of.get(&widget.widget_type) {
            desk.cards.push(Card { id: widget.id.clone(), instance: widget.id != widget.widget_type, ..of.clone() });
        }
    }
    desk
}

/// Where each card lands: its record, sized by the record, else the theme's preset for its
/// type (folders share `sizes.folder`), else the page's 340 x 64. A module without a record
/// takes its default placement (`metadata.json`); anything else stacks down the left edge
/// 88 px apart, as `buildWidgetCard` places it.
/// `area` is the widgets' canvas in the window; the places returned are in the window.
pub fn place(layout: &Layout, theme: &Theme, cards: &[Card], area: Placement) -> Vec<Placement> {
    let canvas = (area.w, area.h);
    let mut offset = 28.0;
    cards.iter().map(|card| {
        let preset = theme.size_of(&card.widget_type).or_else(|| card.folder.then(|| theme.size_of("folder")).flatten());
        let fallback = preset.unwrap_or(crate::card::CARD);
        let record = layout.record(&card.id).or_else(|| if card.instance { None } else { crate::desk::default_record(&card.widget_type) });
        let at = match record {
            Some(record) => record.place(canvas, fallback),
            None => {
                let (w, h) = fallback;
                let at = Placement { x: 28.0, y: offset, w, h };
                offset += 88.0;
                at
            }
        };
        Placement { x: at.x + area.x, y: at.y + area.y, ..at }
    }).collect()
}

fn style(theme: &Theme, size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).family(Theme::family(theme.font_body)).lh_normal()
}

/// `.shortcut-name`: 0.58 rem on 1.2 lines, at most two lines 68 px wide.
fn name_style(theme: &Theme) -> TextStyle {
    style(theme, 9.28, 400.0).lh(9.28 * 1.2)
}

/// The lines a shortcut's name takes, one or two.
fn name_lines(theme: &Theme, name: &str) -> f32 {
    let (_, h) = rune::text::measure(name, &[], name_style(theme), Some(68.0));
    (h / (9.28 * 1.2)).round().clamp(1.0, 2.0)
}

/// The SVG an element draws, tinted, or an empty box until the image cache has it.
fn svg(state: &Host, markup: &str, size: f32, colour: Color) -> El {
    crate::shell::svg_icon(state, markup, size, colour)
}

/// Where a shortcut sits in the grid: four columns 3 px apart.
fn column_width() -> f32 {
    (BODY_W - 9.0) / 4.0
}

/// The rows of a folder's grid (the shortcuts and the "+" tile) and their heights.
fn grid_rows(theme: &Theme, items: &[Value]) -> Vec<f32> {
    let names: Vec<String> = items.iter().map(|it| it["name"].as_str().unwrap_or_default().to_string()).chain(std::iter::once(if items.is_empty() { "Добавить ярлык".to_string() } else { "Добавить".to_string() })).collect();
    names.chunks(4).map(|row| {
        let lines = row.iter().map(|name| name_lines(theme, name)).fold(1.0, f32::max);
        // 1 + 10 + 38 + 6 + the name + 8 + 1.
        64.0 + lines * 9.28 * 1.2
    }).collect()
}

/// The empty folder's note under the grid.
const EMPTY_NOTE: &str = "Пусто. Нажмите «+» или вставьте скопированный файл (Ctrl+V).";

fn empty_note_height(theme: &Theme) -> f32 {
    let (_, h) = rune::text::measure(EMPTY_NOTE, &[], style(theme, 11.84, 400.0).lh(11.84 * 1.6), Some(BODY_W));
    h + 20.0
}

/// The height of a folder's body: the grid, and the note when it is empty.
pub fn folder_height(theme: &Theme, items: &[Value]) -> f32 {
    let rows = grid_rows(theme, items);
    let grid = rows.iter().sum::<f32>() + 3.0 * (rows.len().saturating_sub(1)) as f32;
    grid + if items.is_empty() { empty_note_height(theme) } else { 0.0 }
}

/// What a press on a shortcut does (`openShortcut`): everything goes through the shell as
/// Explorer would open it. The page sends folders and "Explorer" to its own Explorer overlay,
/// which the native host gets at G4; until then Windows Explorer opens them.
fn launch(state: &mut Host, item: &Value) {
    let name = item["name"].as_str().unwrap_or_default();
    let target = if name.trim().eq_ignore_ascii_case("explorer") { "explorer.exe" } else { item["target"].as_str().unwrap_or_default() };
    let args = item["args"].as_str().unwrap_or_default();
    state.call("launch_shortcut", json!({ "target": target, "args": args }), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        if let Err(error) = result {
            state.show_debug(format!("open error: {error}"));
            menu::error_toast(state, &format!("Не открылось: {error}"));
        }
    }));
}

/// An action of `.folder-menu`.
#[derive(Clone, Debug, PartialEq)]
pub enum FolderAction {
    AddFile,
    AddFolder,
    Paste,
    FromStart,
    Rename(String),
    Remove(String),
    Relink(String),
}

impl FolderAction {
    fn label(&self) -> &'static str {
        match self {
            FolderAction::AddFile => "Файл или программа…",
            FolderAction::AddFolder => "Папка…",
            FolderAction::Paste => "Вставить скопированное",
            FolderAction::FromStart => "Из меню Пуск…",
            FolderAction::Rename(_) => "Переименовать",
            FolderAction::Remove(_) => "Убрать",
            FolderAction::Relink(_) => "Найти…",
        }
    }
}

/// `.folder-menu` open at a point.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderMenu {
    pub at: (f32, f32),
    pub folder: String,
    pub actions: Vec<FolderAction>,
}

/// A shortcut being renamed in place.
pub struct Rename {
    pub folder: String,
    pub item: String,
    pub edit: rune::TextEdit,
}

/// A shortcut being dragged to a new place in its folder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShortcutDrag {
    pub index: usize,
    pub dx: f32,
    pub dy: f32,
    pub moved: bool,
}

fn item_of(state: &Host, folder: &str, item: &str) -> Option<Value> {
    state.menu.folders.ready()?.iter().find(|f| f["id"] == folder)?["items"].as_array()?.iter().find(|it| it["id"] == item).cloned()
}

/// Runs a `.folder-menu` action on the folder's record.
pub fn run(state: &mut Host, folder: String, action: FolderAction, tx: &mut Tx) {
    state.folder_menu = None;
    match action {
        FolderAction::AddFile | FolderAction::AddFolder => {
            let pick_folder = action == FolderAction::AddFolder;
            menu::run_once(state, "desk-pick", "pick_shortcut_target", json!({ "folder": pick_folder }), move |state: &mut Host, draft, _tx: &mut Tx| {
                if !draft.is_null() {
                    crate::menu_folders::add_items(state, &folder, vec![draft]);
                }
            });
        }
        FolderAction::Paste => crate::menu_folders::paste(state, &folder),
        FolderAction::FromStart => crate::menu_folders::open_start_picker(state, &folder, tx),
        FolderAction::Rename(item) => {
            let Some(record) = item_of(state, &folder, &item) else { return };
            let key = format!("hit:rename:{item}");
            state.rename = Some(Rename { folder, item, edit: rune::TextEdit::new(record["name"].as_str().unwrap_or_default()) });
            tx.focus(key.clone(), false);
            tx.select_all_text(key);
        }
        FolderAction::Remove(item) => crate::menu_folders::remove_item(state, &folder, &item),
        FolderAction::Relink(item) => {
            let Some(record) = item_of(state, &folder, &item) else { return };
            let pick_folder = record["kind"] == "folder";
            menu::run_once(state, "desk-relink", "pick_shortcut_target", json!({ "folder": pick_folder }), move |state: &mut Host, draft, _tx: &mut Tx| {
                if draft.is_null() {
                    return;
                }
                crate::menu_folders::update_item(state, &folder, &item, |it| {
                    it["target"] = draft["target"].clone();
                    it["args"] = json!(draft["args"].as_str().unwrap_or_default());
                    it["kind"] = draft["kind"].clone();
                });
            });
        }
    }
}

/// Ends a rename: Enter and a press elsewhere keep the name, Escape drops it.
pub fn finish_rename(state: &mut Host, keep: bool) {
    let Some(rename) = state.rename.take() else { return };
    let name = rename.edit.text.trim().to_string();
    let current = item_of(state, &rename.folder, &rename.item);
    if keep && !name.is_empty() && current.is_some_and(|it| it["name"] != name.as_str()) {
        crate::menu_folders::update_item(state, &rename.folder, &rename.item, move |it| it["name"] = json!(name.clone()));
    }
}

/// `moveItem`.
fn move_item(state: &mut Host, folder: &str, from: usize, to: usize) {
    let Some(mut record) = state.menu.folders.ready().and_then(|f| f.iter().find(|f| f["id"] == folder).cloned()) else { return };
    let Some(items) = record["items"].as_array_mut() else { return };
    if from >= items.len() || from == to {
        return;
    }
    let item = items.remove(from);
    items.insert(to.min(items.len()), item);
    crate::menu_folders::store_quiet(state, folder, record);
}

/// The opened folder's grid (`renderFolderBody`): its shortcuts, then "+". `origin` is where
/// the grid sits in the window, for the menus it opens and the drop it finds under a drag.
pub fn folder_body(state: &Host, theme: &Theme, card: &Card, ink: Color, origin: (f32, f32)) -> El {
    let jelly_open = theme.material == Material::Jelly;
    let rows = grid_rows(theme, &card.items);
    let cw = column_width();
    let folder_id = card.widget_type.clone();
    let mut grid = col().w(BODY_W).gap(3.0);
    let mut tiles: Vec<El> = Vec::new();
    let row_top = |r: usize| rows[..r].iter().sum::<f32>() + 3.0 * r as f32;
    // Which grid cell a window point is over (the drop target of a reorder).
    let cells: Vec<(usize, [f32; 4])> = (0..card.items.len()).map(|i| {
        let (r, c) = (i / 4, i % 4);
        (i, [origin.0 + c as f32 * (cw + 3.0), origin.1 + row_top(r), cw, rows[r]])
    }).collect();
    for (index, item) in card.items.iter().enumerate() {
        let item_id = item["id"].as_str().unwrap_or_default().to_string();
        let name = item["name"].as_str().unwrap_or_default().to_string();
        let kind = item["kind"].as_str().unwrap_or("file").to_string();
        let target = item["target"].as_str().unwrap_or_default().to_string();
        let missing = state.menu.missing.contains(&target);
        let folder_look = kind == "folder";
        // `.shortcut-folder`: amber, whatever the theme.
        let amber = |hex: &str, a: f32| rune::color::hex(hex).unwrap_or(Color::WHITE).alpha(a);
        let gold = amber("#ffd25a", 0.7);
        let glyph = crate::shortcut_glyphs::icon_for(&name, &kind);
        let icon = col().size(38.0, 38.0).radius(11.0).items_center().justify_center()
            .fill(if folder_look { amber("#ffc83c", 0.08) } else { theme.bg_elevated.alpha(0.05) })
            .border(1.0, if folder_look { amber("#ffc83c", 0.12) } else { theme.bg_elevated.alpha(0.06) })
            .opacity(if missing { 0.4 } else { 1.0 })
            .child(svg(state, glyph, 22.0, if folder_look { gold } else { ink.alpha(0.6) }));
        let renaming = state.rename.as_ref().filter(|r| r.folder == folder_id && r.item == item_id);
        let label = match renaming {
            Some(rename) => el().w(68.0).px(4.0).py(2.0).radius(6.0).border(1.0, theme.accent).border_layout().fill(theme.scrim.alpha(0.4))
                .child(text_input(&rename.edit, style(theme, 9.92, 400.0)).single_line().key(format!("hit:rename:{item_id}"))
                    .on_edit(|state: &mut Host| &mut state.rename.as_mut().expect("renaming").edit)
                    .on_submit(|state: &mut Host, _tx: &mut Tx| finish_rename(state, true))),
            None => el().w(68.0).child(
                text(name.clone()).font(name_style(theme)).w(68.0).align(Align::Center).max_lines(2)
                    .color(if missing { theme.status_warn } else if jelly_open { ink.alpha(0.85) } else { ink.alpha(0.45) }),
            ),
        };
        let dragging = state.shortcut_drag.filter(|d| d.index == index && d.moved && state.open.as_deref() == Some(card.id.as_str()));
        // `.shortcut-item:hover` lifts a point and shows its border over 120 ms; `:active` shrinks.
        let ease = theme.motion().tween(theme.motion().duration(crate::motion::Token::Hover), crate::motion::EASE);
        let mut tile = col().key(format!("hit:shortcut:{}:{index}", card.id)).label(if missing { format!("{name} — недоступен") } else { name.clone() })
            .w(cw).pt(10.0).pb(8.0).px(4.0).gap(6.0).radius(13.0).border(1.0, Color::TRANSPARENT).border_layout().items_center()
            .hover(theme.bg_elevated.alpha(0.06)).hover_border(theme.bg_elevated.alpha(0.08)).hover_lift(0.0, -1.0).hover_motion(ease, ease)
            .press_scale(0.93)
            .child(icon).child(label);
        if let Some(drag) = dragging {
            tile = tile.offset(drag.dx, drag.dy).opacity(0.85).raise();
        }
        let (press_folder, press_item, menu_folder, menu_item) = (folder_id.clone(), item.clone(), folder_id.clone(), item_id.clone());
        let anchor = (cells[index].1[0], cells[index].1[1] + cells[index].1[3] + 4.0);
        let drop_cells = cells.clone();
        tile = tile
            .on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, tx: &mut Tx| {
                match drag.phase {
                    rune::el::Phase::Down => {
                        finish_rename(state, true);
                        state.folder_menu = None;
                        state.shortcut_drag = Some(ShortcutDrag { index, dx: 0.0, dy: 0.0, moved: false });
                    }
                    rune::el::Phase::Move => {
                        if let Some(d) = state.shortcut_drag.as_mut() {
                            d.moved |= drag.dx.hypot(drag.dy) >= DRAG_START;
                            if d.moved {
                                d.dx = drag.dx;
                                d.dy = drag.dy;
                            }
                        }
                    }
                    rune::el::Phase::Up => {
                        let moved = state.shortcut_drag.take().is_some_and(|d| d.moved);
                        if drag.cancelled() {
                            return;
                        }
                        if moved {
                            let [x, y] = drag.window_point();
                            let target = drop_cells.iter().find(|(i, [cx, cy, w, h])| *i != index && x >= *cx && x <= cx + w && y >= *cy && y <= cy + h).map(|(i, _)| *i);
                            if let Some(to) = target {
                                move_item(state, &press_folder, index, to);
                            }
                            return;
                        }
                        // A click: a missing target offers "Найти…" and "Убрать" instead.
                        let id = press_item["id"].as_str().unwrap_or_default().to_string();
                        let gone = state.menu.missing.contains(press_item["target"].as_str().unwrap_or_default());
                        if gone {
                            state.folder_menu = Some(FolderMenu { at: anchor, folder: press_folder.clone(), actions: vec![FolderAction::Relink(id.clone()), FolderAction::Remove(id)] });
                        } else {
                            launch(state, &press_item);
                        }
                        let _ = tx;
                    }
                }
            })
            .on_secondary_press(move |state: &mut Host, at: rune::el::PointerPress, _tx: &mut Tx| {
                finish_rename(state, true);
                let gone = state.menu.missing.contains(&target);
                let mut actions = vec![FolderAction::Rename(menu_item.clone()), FolderAction::Remove(menu_item.clone())];
                if gone {
                    actions.insert(0, FolderAction::Relink(menu_item.clone()));
                }
                state.folder_menu = Some(FolderMenu { at: (at.x, at.y), folder: menu_folder.clone(), actions });
            });
        tiles.push(tile);
    }
    // The "+" tile, its icon box dashed.
    let plus = [([12.0, 5.0], [12.0, 19.0]), ([5.0, 12.0], [19.0, 12.0])];
    let add_index = card.items.len();
    let add_anchor = (origin.0 + (add_index % 4) as f32 * (cw + 3.0), origin.1 + row_top(add_index / 4) + rows[add_index / 4] + 4.0);
    let add_folder = folder_id.clone();
    tiles.push(
        col().key(format!("hit:shortcut-add:{}", card.id)).label("Добавить ярлык").w(cw).pt(10.0).pb(8.0).px(4.0).gap(6.0).radius(13.0).border(1.0, Color::TRANSPARENT).border_layout().items_center()
            .hover(theme.bg_elevated.alpha(0.06)).hover_border(theme.bg_elevated.alpha(0.08)).hover_lift(0.0, -1.0)
            .hover_motion(theme.motion().tween(theme.motion().duration(crate::motion::Token::Hover), crate::motion::EASE), theme.motion().tween(theme.motion().duration(crate::motion::Token::Hover), crate::motion::EASE))
            .press_scale(0.93)
            .child(
                col().size(38.0, 38.0).radius(11.0).items_center().justify_center()
                    .fill(theme.bg_elevated.alpha(0.05)).border(1.0, theme.bg_elevated.alpha(0.06)).border_dash(3.0, 3.0)
                    .child(el().size(20.0, 20.0).shape(crate::controls::glyph(&plus, ink.alpha(0.6), 2.0))),
            )
            .child(el().w(68.0).child(text(if card.items.is_empty() { "Добавить ярлык" } else { "Добавить" }).font(name_style(theme)).w(68.0).align(Align::Center).max_lines(2).color(ink.alpha(if jelly_open { 0.85 } else { 0.45 }))))
            .on_press_tx(move |state: &mut Host, _tx: &mut Tx| {
                finish_rename(state, true);
                state.folder_menu = Some(FolderMenu {
                    at: add_anchor,
                    folder: add_folder.clone(),
                    actions: vec![FolderAction::AddFile, FolderAction::AddFolder, FolderAction::Paste, FolderAction::FromStart],
                });
            }),
    );
    let mut tiles = tiles.into_iter();
    for _ in 0..rows.len() {
        let mut line = row().w(BODY_W).gap(3.0).items_start();
        for tile in tiles.by_ref().take(4) {
            line = line.child(tile);
        }
        grid = grid.child(line);
    }
    if card.items.is_empty() {
        grid = grid.child(
            el().w(BODY_W).py(10.0).child(text(EMPTY_NOTE).font(style(theme, 11.84, 400.0).lh(11.84 * 1.6)).color(ink.alpha(0.3)).align(Align::Center).wrap()),
        );
    }
    grid
}

/// `.folder-menu`: at the pointer, kept inside the window, 180 px at least.
pub fn folder_menu(state: &Host, theme: &Theme, canvas: (f32, f32)) -> Option<El> {
    let open = state.folder_menu.as_ref()?;
    let look = crate::controls::Look::panel(theme);
    let width = 180.0;
    let height = 8.0 + open.actions.len() as f32 * 32.0 + 2.0;
    let x = open.at.0.min(canvas.0 - width - 4.0).max(4.0);
    let y = open.at.1.min(canvas.1 - height - 4.0).max(4.0);
    let mut list = col().key("hit:folder-menu").label("Меню ярлыка").on_press(|_: &mut Host| {}).absolute(x, y).w(width).padding(4.0).radius(theme.radius_card)
        .fill(theme.surface.alpha(0.98)).border(1.0, theme.bg_elevated.alpha(0.14)).border_layout().shadow(12.0, 36.0, Color::BLACK.alpha(0.35));
    for (index, action) in open.actions.iter().enumerate() {
        let danger = matches!(action, FolderAction::Remove(_));
        let (folder, action_run) = (open.folder.clone(), action.clone());
        list = list.child(
            row().key(format!("hit:folder-action:{index}")).w_full().min_h(32.0).px(12.0).py(6.0).radius(9.0).items_center()
                .hover(theme.bg_elevated.alpha(0.12))
                .child(text(action.label()).font(look.font(13.0, 400.0)).color(if danger { theme.status_error } else { look.fg }))
                .on_press_tx(move |state: &mut Host, tx: &mut Tx| run(state, folder.clone(), action_run.clone(), tx)),
        );
    }
    Some(list)
}

/// `mountNested`: the widgets inside a host, each with its name and "Вынуть на стол", drawn in
/// their compact form (`Widget::compact`). `None` when the host holds nothing.
pub fn nested_zone(state: &Host, desk: &Desk, theme: &Theme, ink: Color, host: &str) -> Option<(El, f32)> {
    let children = desk.nested.get(host).filter(|c| !c.is_empty())?;
    let mut zone = col().w(BODY_W).gap(8.0);
    let mut height = 0.0;
    for child in children {
        let entry = desk.widgets.get(child);
        let name = entry.map_or(child.clone(), |w| w.name.clone());
        let arrow = [([12.0, 19.0], [12.0, 5.0]), ([5.0, 12.0], [12.0, 5.0]), ([12.0, 5.0], [19.0, 12.0])];
        let eject_id = child.clone();
        let head = row().w_full().min_h(20.0).items_center().justify_between()
            .child(text(name.clone()).font(style(theme, 10.88, 400.0)).color(ink.alpha(0.6)))
            .child(
                col().key(format!("hit:eject:{child}")).label(format!("Вынуть «{name}» на стол")).size(20.0, 20.0).radius(6.0).items_center().justify_center()
                    .hover(ink.alpha(0.1))
                    .child(el().size(12.0, 12.0).shape(crate::controls::glyph(&arrow, ink.alpha(0.6), 2.4)))
                    .on_press_tx(move |state: &mut Host, tx: &mut Tx| state.apply(vec![json!({ "op": "eject", "id": eject_id })], tx)),
            );
        // A press inside stays there: it does not fold the host.
        let mut item = col().w_full().pt(6.0).px(8.0).pb(8.0).radius((theme.radius_card - 4.0).max(0.0)).fill(theme.bg_elevated.alpha(0.04)).child(head)
            .on_press(|_: &mut Host| {});
        let mut item_h = 6.0 + 20.0 + 8.0;
        if let Some((compact, compact_h)) = state.compact_of(child, host, ink, BODY_W - 16.0) {
            item = item.child(compact);
            item_h += compact_h;
        }
        zone = zone.child(item);
        height += item_h + 8.0;
    }
    Some((zone, height - 8.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(config: Value) -> Layout {
        Layout::from_event(&json!({ "rev": 1, "instance": "k", "config": config })).unwrap()
    }

    fn drawn(id: &str) -> bool {
        matches!(id, "clock" | "vpn")
    }

    fn desk(layout: &Layout, folders: &[Value], disabled: &HashSet<String>) -> Desk {
        super::desk(layout, folders, disabled, &drawn)
    }

    #[test]
    fn folders_and_vpn_are_cards_and_nesting_takes_vpn_off_the_desk() {
        let folders = [json!({ "id": "folder_a", "name": "Работа", "icon_svg": "", "items": [{ "id": "i1", "name": "x", "target": "C:\\x" }] })];
        let nested = layout(json!({ "positions": {}, "nested": { "clock": ["vpn"] } }));
        let desk_now = desk(&nested, &folders, &HashSet::new());
        assert_eq!(desk_now.cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["clock", "folder_a"]);
        assert!(desk_now.cards[0].bare, "the clock's metadata says chrome: bare");
        assert_eq!(desk_now.cards[1].desc, "1 ярлык");
        assert_eq!(desk_now.nested["clock"], vec!["vpn".to_string()]);

        let ejected = layout(json!({ "positions": {}, "nested": {} }));
        let cards = desk(&ejected, &folders, &HashSet::new()).cards;
        assert!(cards.iter().any(|c| c.id == "vpn" && !c.folder && !c.bare));

        // A switched-off VPN is neither a card nor nested.
        let off: HashSet<String> = ["vpn".to_string()].into();
        let desk_off = desk(&nested, &folders, &off);
        assert!(desk_off.nested.is_empty() && desk_off.cards.iter().all(|c| c.id != "vpn"));

        // Hidden folders stay off; instances of a folder are cards of their own.
        let hidden = layout(json!({ "hidden": ["folder_a"], "instances": [{ "instanceId": "folder_a-2", "widgetType": "folder_a" }] }));
        assert!(desk(&hidden, &folders, &HashSet::new()).cards.iter().all(|c| !c.id.starts_with("folder_a")));
        let instance = layout(json!({ "instances": [{ "instanceId": "folder_a-2", "widgetType": "folder_a" }] }));
        let cards = desk(&instance, &folders, &HashSet::new()).cards;
        assert!(cards.iter().any(|c| c.id == "folder_a-2" && c.instance && c.widget_type == "folder_a"));
    }

    #[test]
    fn nesting_follows_can_nest() {
        let folders = [json!({ "id": "f", "name": "Ф", "items": [] })];
        let d = desk(&layout(json!({ "nested": { "f": ["clock", "vpn"], "vpn": ["f"] } })), &folders, &HashSet::new());
        assert_eq!(d.nested.get("f"), Some(&vec!["vpn".to_string()]), "the clock hosts others, so it cannot be nested");
        assert!(d.nested.get("vpn").is_none(), "VPN hosts nothing");
        assert_eq!(d.refusal("vpn", "vpn").as_deref(), Some("Виджет нельзя вложить сам в себя"));
        assert_eq!(d.refusal("f", "clock").as_deref(), Some("«Ф» сам вмещает виджеты, такой нельзя вложить"));
        assert!(d.refusal("vpn", "clock").is_none());
    }

    #[test]
    fn cards_without_a_record_stack_down_the_left() {
        let folders = [json!({ "id": "a", "name": "A", "items": [] }), json!({ "id": "b", "name": "B", "items": [] })];
        let l = layout(json!({ "positions": { "b": { "x": 500, "y": 40, "w": 200 } }, "nested": { "clock": ["vpn"] } }));
        let theme = Theme::nothing_classic();
        let (w, h) = theme.size_of("folder").unwrap_or(crate::card::CARD);
        let d = desk(&l, &folders, &HashSet::new());
        let folders_only: Vec<Card> = d.cards.into_iter().filter(|c| c.folder).collect();
        // The canvas lies 28 points inside the window (`.app-shell { padding: 28px }`).
        let area = Placement { x: 28.0, y: 28.0, w: 1224.0, h: 664.0 };
        let at = place(&l, &theme, &folders_only, area);
        assert_eq!((at[0].x, at[0].y, at[0].w, at[0].h), (56.0, 56.0, w, h));
        assert_eq!((at[1].x, at[1].y, at[1].w), (528.0, 68.0, 200.0), "the record's own width");
    }

    #[test]
    fn an_empty_folder_is_one_row_and_the_note() {
        let theme = Theme::nothing_classic();
        let rows = grid_rows(&theme, &[]);
        assert_eq!(rows.len(), 1);
        let five: Vec<Value> = (0..5).map(|i| json!({ "name": format!("a{i}") })).collect();
        assert_eq!(grid_rows(&theme, &five).len(), 2, "five shortcuts and the + tile");
        assert!(folder_height(&theme, &[]) > rows[0] + 20.0);
    }
}
