//! The desk the host draws and what a person does on it: the widgets the kernel's layout
//! places (`desk.rs`), the theme they are painted with (the stored one, or a draft while the
//! menu edits it), the menu (`menu.rs`), arrange mode (`arrange.rs`) and the toast. `host.rs`
//! owns the window, the kernel link and the inspector; this file owns the state they serve.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rune::el::{col, el, row, text, text_input, El};
use rune::{Color, Image, ImageCache, Pixels, Tx};
use serde_json::{json, Value};

use crate::arrange::{self, Arrange, Edge};
use crate::controls::{self, ButtonKind, Look};
use crate::desk::{self, ClockTime, Layout, Placement};
use crate::desk_cards::{self, Desk, Kind};
use crate::inspector::Inspector;
use crate::menu::{self, Menu, MenuHost};
use crate::theme::Theme;
use crate::toast::Toasts;

pub(crate) struct Host {
    pub inspector: Arc<Inspector>,
    /// `None` until the kernel has described the desk: a supervised host draws nothing
    /// rather than defaults that would jump to the stored positions a moment later.
    pub layout: Option<Layout>,
    pub time: ClockTime,
    pub canvas: (f32, f32),
    /// Physical pixels per point; images are made in device pixels.
    pub scale: f32,
    pub grain: Pixels,
    /// What `grain` was made for: width and height in pixels, and the strength's bits.
    pub grain_made_for: Option<(u32, u32, u32)>,
    /// The card open as its surface (the clock's dashboard, a folder, VPN); one at a time
    /// (`r[motion.widget-integrity]`).
    pub open: Option<String>,
    /// The card opened last: it draws above the others while it closes.
    pub top: Option<String>,
    /// Timing of the open card's open and close, and the duration of the one running now.
    pub morph: desk::MorphClock,
    pub transition: f32,
    pub motion: crate::motion::Motion,
    /// `navigator.onLine` as the dashboard read it when it opened.
    pub online: bool,
    pub menu: Menu,
    pub toasts: Toasts<Host>,
    /// The theme record shown while the menu edits it, before the kernel has it.
    pub draft: Option<Value>,
    pub arrange: Option<Arrange>,
    /// After arrange mode ends, the desk takes no press for one motion step.
    pub settling_until: Option<Instant>,
    /// The handle the host's own callbacks come back through.
    pub ui: Arc<OnceLock<rune::AppHandle<Host>>>,
    pub images: Option<ImageCache>,
    pub svgs: RefCell<HashMap<(String, u32), Image>>,
    pub icons: RefCell<HashMap<String, Option<Pixels>>>,
    /// Shortcut icons the view asked for that the poll has not requested yet.
    pub wanted_icons: RefCell<Vec<String>>,
    pub vpn: crate::vpn_panel::VpnPanel,
    pub folder_menu: Option<desk_cards::FolderMenu>,
    pub rename: Option<desk_cards::Rename>,
    pub shortcut_drag: Option<desk_cards::ShortcutDrag>,
    /// `showDebugMessage`: the red line at the bottom, for 4 s.
    pub debug: Option<(String, Instant)>,
    /// The host under a card dragged in arrange mode, and whether it would take it.
    pub nest_mark: Option<(String, bool)>,
}

impl Host {
    pub fn new(inspector: Arc<Inspector>, layout: Option<Layout>, canvas: (f32, f32), scale: f32, ui: Arc<OnceLock<rune::AppHandle<Host>>>) -> Host {
        let motion = layout.as_ref().map_or_else(|| crate::motion::Motion::new(crate::motion::Profile::Balanced, 1.0), |l| l.theme.motion());
        let mut host = Host {
            inspector,
            layout,
            time: ClockTime::now().0,
            canvas,
            scale,
            grain: Pixels::new(),
            grain_made_for: None,
            open: None,
            top: None,
            morph: desk::MorphClock::default(),
            transition: 0.0,
            motion,
            online: true,
            menu: Menu::default(),
            toasts: Toasts::default(),
            draft: None,
            arrange: None,
            settling_until: None,
            ui,
            images: None,
            svgs: RefCell::new(HashMap::new()),
            icons: RefCell::new(HashMap::new()),
            wanted_icons: RefCell::new(Vec::new()),
            vpn: Default::default(),
            folder_menu: None,
            rename: None,
            shortcut_drag: None,
            debug: None,
            nest_mark: None,
        };
        host.refresh_grain();
        host
    }

    /// The theme the desk shows: the draft while one is being edited, else the stored one.
    pub fn theme(&self) -> Theme {
        match (&self.draft, &self.layout) {
            (Some(draft), _) => Theme::normalize(draft),
            (None, Some(layout)) => layout.theme.clone(),
            (None, None) => Theme::normalize(&Value::Null),
        }
    }

    pub fn accept_layout(&mut self, layout: Layout) {
        // A reply and an event can arrive in either order; the newer revision wins, and any
        // revision of a new kernel run.
        if self.layout.as_ref().map_or(true, |known| layout.supersedes(known)) {
            // The kernel has the theme the draft showed: the stored one shows from now on.
            if self.draft.as_ref().is_some_and(|draft| crate::theme_record::normalize(draft) == crate::theme_record::normalize(&layout.config["theme"])) {
                self.draft = None;
            }
            self.motion = layout.theme.motion();
            let first_colours = layout.config.is_object() && layout.config["colorOverrides"].get(crate::theme_record::USER_COLOR_PRESET).is_none();
            let new_run = self.layout.as_ref().map_or(true, |known| known.instance != layout.instance);
            self.layout = Some(layout);
            if new_run {
                // Folders and switched-off modules decide which cards the desk draws.
                crate::menu_desk::load_lists(self);
            }
            self.refresh_grain();
            if first_colours {
                self.deal_user_colours();
            }
        }
    }

    /// `initThemeSystem`'s first run: the user's own Sea Glass colours, dealt once and stored;
    /// applied at once when Sea Glass is the active theme.
    fn deal_user_colours(&mut self) {
        // A host without the kernel stores nothing; it draws the defaults.
        if self.inspector.outbox.lock().map_or(true, |slot| slot.is_none()) {
            return;
        }
        let Some(layout) = &self.layout else { return };
        let targets = crate::theme_record::tint_targets(crate::module_table::desk_modules().map(|m| &m.meta));
        let rolled = crate::theme_record::roll_colors(crate::theme_record::new_color_seed(), &targets);
        let mut sea = crate::theme_record::builtin(crate::theme_record::USER_COLOR_PRESET).unwrap_or_default();
        sea["palette"] = rolled["palette"].clone();
        sea["tintMap"] = rolled["tintMap"].clone();
        let sea = crate::theme_record::normalize(&sea);
        let colours = crate::theme_record::colors_of(&sea);
        let mut ops = vec![json!({ "op": "color_overrides", "id": crate::theme_record::USER_COLOR_PRESET, "value": colours })];
        let active = crate::theme_record::normalize(&layout.config["theme"]);
        if active["id"] == crate::theme_record::USER_COLOR_PRESET {
            let mut theme = active;
            theme["palette"] = rolled["palette"].clone();
            theme["tintMap"] = rolled["tintMap"].clone();
            ops.push(json!({ "op": "theme", "theme": crate::theme_record::normalize(&theme) }));
        }
        self.call("desk_apply", json!({ "ops": ops }), Box::new(|_: &mut Host, _, _: &mut Tx| {}));
    }

    /// The clock's place: its draft while arranging, its record otherwise.
    fn clock_tile(&self) -> Option<Placement> {
        let layout = self.layout.as_ref()?;
        if let Some(draft) = self.arrange.as_ref().and_then(|a| a.drafts.get(desk::CLOCK)) {
            return Some(*draft);
        }
        Some(layout.clock_place_with(self.canvas, &self.theme()))
    }

    /// What the desk shows: the layout with the folders and the switched-off modules.
    pub fn desk(&self) -> Option<Desk> {
        let layout = self.layout.as_ref()?;
        let folders = self.menu.folders.ready().map_or(&[][..], Vec::as_slice);
        let empty = std::collections::HashSet::new();
        let disabled = self.menu.disabled_modules.ready().unwrap_or(&empty);
        Some(desk_cards::desk(layout, folders, disabled))
    }

    /// Each card's place: its draft while arranging, its record otherwise.
    pub fn card_tiles(&self, desk: &Desk) -> Vec<Placement> {
        let Some(layout) = &self.layout else { return Vec::new() };
        let placed = desk_cards::place(layout, &self.theme(), &desk.cards, self.canvas);
        desk.cards.iter().zip(placed).map(|(card, at)| self.arrange.as_ref().and_then(|a| a.drafts.get(&card.id)).copied().unwrap_or(at)).collect()
    }

    /// The box a card opens into: 400 wide, as high as its content and 80 more, 500 at most.
    pub fn card_box(&self, desk: &Desk, card: &desk_cards::Card, tile: Placement) -> Placement {
        let theme = self.theme();
        let ink = theme.ink_on(card.tint(&theme));
        let body = match card.kind {
            Kind::Folder => desk_cards::folder_height(&theme, &card.items),
            Kind::Vpn => crate::vpn_panel::panel(self, &theme, ink, "measure", desk_cards::BODY_W, false).1,
        };
        let nested = desk_cards::nested_zone(self, desk, &theme, ink, &card.id).map_or(0.0, |(_, h)| 10.0 + h);
        desk::surface_box(tile, (desk_cards::OPEN_WIDTH, (body + nested + 80.0).min(desk_cards::OPEN_MAX)), self.canvas)
    }

    /// The size the grain image covers: every tile and every surface on the desk, rounded up
    /// to 64 pt, so a card clips it and never stretches it while it grows.
    pub fn grain_size(&self) -> (f32, f32) {
        let mut size = (0.0f32, 0.0f32);
        let mut cover = |p: Placement| size = (size.0.max(p.w), size.1.max(p.h));
        if let Some(tile) = self.clock_tile() {
            cover(tile);
            cover(desk::dashboard_box(tile, self.canvas));
        }
        if let Some(desk) = self.desk() {
            for tile in self.card_tiles(&desk) {
                cover(tile);
            }
            if !desk.cards.is_empty() {
                cover(Placement { x: 0.0, y: 0.0, w: desk_cards::OPEN_WIDTH, h: desk_cards::OPEN_MAX });
            }
        }
        ((size.0 / 64.0).ceil() * 64.0, (size.1 / 64.0).ceil() * 64.0)
    }

    /// The grain is made again only when its size or the theme's grain strength changes.
    pub fn refresh_grain(&mut self) {
        if self.layout.is_none() {
            return;
        }
        let strength = self.theme().glass_grain;
        let (w, h) = self.grain_size();
        let size = ((w * self.scale).round().max(1.0) as u32, (h * self.scale).round().max(1.0) as u32);
        let made_for = (size.0, size.1, strength.to_bits());
        // Made again when the desk needs a larger one or the strength changed, never for a
        // smaller one, so a resize in arrange mode does not remake it on every move.
        let covers = self.grain_made_for.is_some_and(|(w, h, bits)| bits == made_for.2 && w >= size.0 && h >= size.1);
        if !covers {
            self.grain.set(size.0, size.1, desk::grain_rgba(size.0, size.1, self.scale, strength));
            self.grain_made_for = Some(made_for);
        }
    }

    /// The tile and the box of a widget on the desk: the clock or a card.
    fn tile_and_box(&self, id: &str) -> Option<(Placement, Placement)> {
        if id == desk::CLOCK {
            let tile = self.clock_tile()?;
            return Some((tile, desk::dashboard_box(tile, self.canvas)));
        }
        let desk = self.desk()?;
        let index = desk.cards.iter().position(|c| c.id == id)?;
        let tile = *self.card_tiles(&desk).get(index)?;
        Some((tile, self.card_box(&desk, &desk.cards[index], tile)))
    }

    /// `toggleWidget`'s open: another open card folds first; the open runs on `expoOut` for
    /// `calculateDuration` of the growth.
    pub fn open_card(&mut self, id: &str, tx: &mut Tx) {
        let Some((tile, target)) = self.tile_and_box(id) else { return };
        self.folder_menu = None;
        desk_cards::finish_rename(self, true);
        if id == desk::CLOCK {
            self.online = crate::desktop_host::is_online();
        }
        self.transition = self.morph.open(&self.motion, tile, target);
        tx.motion_curve(self.motion.tween(self.transition, crate::motion::EXPO_OUT));
        self.open = Some(id.to_string());
        self.top = Some(id.to_string());
        if self.vpn_visible() {
            crate::vpn_panel::shown(self);
        }
    }

    /// The close runs on `surfaceExit`, shorter when it interrupts the open.
    pub fn close_card(&mut self, tx: &mut Tx) {
        if self.open.is_none() {
            return;
        }
        self.folder_menu = None;
        desk_cards::finish_rename(self, true);
        self.transition = self.morph.close();
        tx.motion_curve(self.motion.tween(self.transition, crate::motion::SURFACE_EXIT));
        self.open = None;
    }

    /// A press on a card's tile or on its open surface.
    pub fn toggle_card(&mut self, id: &str, tx: &mut Tx) {
        if self.settling_until.is_some_and(|until| Instant::now() < until) {
            return;
        }
        if self.open.as_deref() == Some(id) {
            self.close_card(tx);
        } else {
            self.open_card(id, tx);
        }
    }

    /// A VPN panel is on screen: the open card is VPN or hosts it.
    pub fn vpn_visible(&self) -> bool {
        let Some(open) = &self.open else { return false };
        let Some(desk) = self.desk() else { return false };
        let is_vpn = |id: &str| desk.widgets.get(id).is_some_and(|w| w.widget_type == "vpn");
        is_vpn(open) || desk.nested.get(open).is_some_and(|children| children.iter().any(|c| is_vpn(c)))
    }

    /// `showDebugMessage`.
    pub fn show_debug(&mut self, text: String) {
        self.debug = Some((text, Instant::now() + std::time::Duration::from_secs(4)));
    }

    /// Whether the host wants the desk's keys: a menu or the arrange island is open.
    pub fn wants_keyboard(&self) -> bool {
        self.menu.open || self.arrange.is_some() || self.open.is_some() || self.rename.is_some() || self.folder_menu.is_some()
    }

    /// A press on the bare desk, outside the host: the sheet over the menu closes first (it is
    /// a surface), then an open dashboard (`setupDesktopOutsideClickHandler`). The menu panel
    /// itself is not a surface and stays.
    pub fn pointer_outside(&mut self, tx: &mut Tx) {
        if self.folder_menu.is_some() {
            self.folder_menu = None;
        } else if self.rename.is_some() {
            desk_cards::finish_rename(self, true);
        } else if self.vpn.menu.is_some() {
            self.vpn.menu = None;
        } else if self.menu.sheet.is_some() {
            self.menu.sheet = None;
        } else {
            self.close_card(tx);
        }
    }

    /// `enterArrangeMode`: the open dashboard folds, the island appears, every card the host
    /// draws becomes a draft.
    pub fn start_arrange(&mut self, tx: &mut Tx) {
        if self.layout.is_none() {
            return;
        }
        self.close_card(tx);
        let desk = self.desk();
        let tiles = desk.as_ref().map(|d| self.card_tiles(d)).unwrap_or_default();
        let Some(layout) = &self.layout else { return };
        let mut drafts = BTreeMap::new();
        if desk.as_ref().is_some_and(|d| d.clock) {
            drafts.insert(desk::CLOCK.to_string(), layout.clock_place_with(self.canvas, &self.theme()));
        }
        for (card, tile) in desk.iter().flat_map(|d| d.cards.iter()).zip(tiles) {
            drafts.insert(card.id.clone(), tile);
        }
        let side_right = layout.config["arrangeSide"].as_str() != Some("left");
        self.arrange = Some(Arrange::new(layout.rev, layout.config.clone(), drafts, side_right));
    }

    /// `leaveArrangeMode`: "Готово" pins every card to its nearest corner and writes the
    /// drafts in one change from the session's revision; "Отменить" puts the layout back as
    /// the session found it and deletes the folders it created. The menu comes back.
    pub fn finish_arrange(&mut self, done: bool, tx: &mut Tx) {
        let Some(session) = self.arrange.take() else { return };
        let side = json!({ "op": "arrange_side", "side": if session.side_right { "right" } else { "left" } });
        if done {
            let mut ops: Vec<Value> = session.drafts.iter().map(|(id, at)| {
                let mut op = arrange::pin(*at, self.canvas);
                op["op"] = json!("position");
                op["id"] = json!(id);
                op
            }).collect();
            ops.push(side);
            self.apply_from(ops, Some(session.base_rev), tx);
        } else {
            let snapshot = &session.snapshot;
            let ops = vec![
                json!({ "op": "positions", "positions": snapshot.get("positions").cloned().unwrap_or(json!({})) }),
                json!({ "op": "structure", "hidden": snapshot.get("hidden").cloned().unwrap_or(json!([])), "instances": snapshot.get("instances").cloned().unwrap_or(json!([])), "nested": snapshot.get("nested").cloned().unwrap_or(json!({})) }),
                side,
            ];
            self.apply_from(ops, None, tx);
            for id in session.created_folders {
                self.call("delete_folder", json!({ "id": id }), Box::new(|_: &mut Host, _, _: &mut Tx| {}));
            }
        }
        self.settling_until = Some(Instant::now() + std::time::Duration::from_secs_f32(self.motion.duration(crate::motion::Token::Motion)));
        if !self.menu.open {
            menu::toggle(self, tx);
        }
    }

    fn apply_from(&mut self, ops: Vec<Value>, base_rev: Option<u64>, _tx: &mut Tx) {
        let mut args = json!({ "ops": ops });
        if let Some(base) = base_rev {
            args["baseRev"] = json!(base);
        }
        self.call("desk_apply", args, Box::new(|state: &mut Host, result, _tx: &mut Tx| {
            if let Err(error) = result {
                menu::error_toast(state, &format!("Не получилось: {error}"));
            }
        }));
    }

    /// `createWidgetInstance`: a hidden base widget comes back instead; a singleton that is
    /// placed is refused; otherwise a new instance at the type's default place.
    fn add_widget(&mut self, widget_type: &str, tx: &mut Tx) {
        let Some(layout) = &self.layout else { return };
        if layout.is_hidden(widget_type) {
            self.apply(vec![json!({ "op": "show", "id": widget_type })], tx);
            return;
        }
        let singleton = crate::module_table::desk_modules().any(|m| m.id == widget_type && m.meta["singleton"] == true);
        let placed = layout.config["instances"].as_array().is_some_and(|items| items.iter().any(|i| i["widgetType"] == widget_type))
            || layout.config["positions"].get(widget_type).is_some();
        if singleton && placed {
            return;
        }
        let (w, h) = match widget_type {
            "clock" => (210.0, 180.0),
            "mp3-player" => (210.0, 220.0),
            "glyph-bar" => (10.0, 127.0),
            "telegram" => (56.0, 520.0),
            _ => (340.0, 64.0),
        };
        let id = format!("{widget_type}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
        let position = json!({ "x": (self.canvas.0 - w - 28.0).max(0.0), "y": 68.0, "w": w, "h": h });
        self.apply(vec![json!({ "op": "add_instance", "id": id, "type": widget_type, "position": position })], tx);
    }
}

impl MenuHost for Host {
    fn menu(&mut self) -> &mut Menu {
        &mut self.menu
    }

    fn menu_ref(&self) -> &Menu {
        &self.menu
    }

    fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    fn toasts(&mut self) -> &mut Toasts<Self> {
        &mut self.toasts
    }

    fn theme_record(&self) -> Value {
        match (&self.draft, &self.layout) {
            (Some(draft), _) => crate::theme_record::normalize(draft),
            (None, Some(layout)) => crate::theme_record::normalize(&layout.config["theme"]),
            (None, None) => crate::theme_record::normalize(&Value::Null),
        }
    }

    fn preview_theme(&mut self, record: Value) {
        self.draft = Some(record);
        self.motion = self.theme().motion();
        self.refresh_grain();
    }

    fn end_preview(&mut self) {
        self.draft = None;
        self.refresh_grain();
    }

    fn apply(&mut self, ops: Vec<Value>, tx: &mut Tx) {
        self.apply_from(ops, None, tx);
    }

    fn call(&mut self, cmd: &str, args: Value, then: Box<dyn FnOnce(&mut Self, Result<Value, String>, &mut Tx) + Send>) {
        let Some(ui) = self.ui.get().cloned() else {
            menu::error_toast(self, "Не получилось: нет связи с ядром");
            return;
        };
        let outbox = self.inspector.outbox.lock().ok().and_then(|slot| slot.clone());
        let Some(outbox) = outbox else {
            // The requester still hears it: a list stops saying "Загрузка…".
            ui.update(move |state: &mut Host, tx: &mut Tx| then(state, Err("нет связи с ядром".into()), tx));
            return;
        };
        let handler: crate::ws_client::ReplyHandler = Box::new(move |result| {
            ui.update(move |state: &mut Host, tx: &mut Tx| then(state, result, tx));
        });
        // A refusal (the outbox is full) has already reached the handler.
        let _ = outbox.request(cmd, args, handler);
    }

    fn enter_arrange(&mut self, tx: &mut Tx) {
        self.start_arrange(tx);
    }

    fn svg(&self, markup: &str, size: f32, colour: Color) -> Option<Pixels> {
        let images = self.images.as_ref()?;
        // The colour, opaque, replaces `currentColor`; its alpha is the drawing's opacity.
        let hex = colour.hex();
        let tinted = markup.replace("currentColor", &hex[..7]).replacen("<svg ", &format!("<svg opacity=\"{:.3}\" ", colour.a), 1);
        let key = (tinted.clone(), size.to_bits());
        let mut svgs = self.svgs.borrow_mut();
        let image = svgs.entry(key).or_insert_with(|| images.svg(tinted.into_bytes(), size, self.scale));
        Some(image.pixels().clone())
    }

    fn shortcut_icon(&self, target: &str) -> Option<Pixels> {
        // Links have no Windows icon; `shortcutIconUrl` skips them.
        let is_url = target.split_once("://").is_some_and(|(scheme, _)| !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)));
        if target.is_empty() || is_url {
            return None;
        }
        if let Some(cached) = self.icons.borrow().get(target) {
            return cached.clone();
        }
        self.icons.borrow_mut().insert(target.to_string(), None);
        self.wanted_icons.borrow_mut().push(target.to_string());
        None
    }

    fn pick_color(&mut self, initial: [u8; 3], then: Box<dyn FnOnce(&mut Self, [u8; 3], &mut Tx) + Send>) {
        let Some(ui) = self.ui.get().cloned() else { return };
        // The system dialog runs its own loop: on a worker, so the desk keeps drawing.
        let _ = std::thread::Builder::new().name("color-dialog".into()).spawn(move || {
            if let Some(rgb) = crate::desktop_host::choose_color(initial) {
                ui.update(move |state: &mut Host, tx: &mut Tx| then(state, rgb, tx));
            }
        });
    }

    fn canvas(&self) -> (f32, f32) {
        self.canvas
    }

    fn arranging(&self) -> bool {
        self.arrange.is_some()
    }
}

/// The widget card's drag in arrange mode: it follows the pointer and snaps; over a card that
/// hosts others it marks the host (`nest-target`, or `nest-refused` with the reason on
/// release), and a release there nests it (`r[desk.nesting]`).
fn drag_card(state: &mut Host, id: &str, drag: &mut rune::el::Drag, tx: &mut Tx) {
    let canvas = state.canvas;
    let host_under = |state: &Host, point: [f32; 2]| -> Option<String> {
        let session = state.arrange.as_ref()?;
        session.drafts.iter()
            .filter(|(other, at)| other.as_str() != id && point[0] >= at.x && point[0] <= at.x + at.w && point[1] >= at.y && point[1] <= at.y + at.h)
            .map(|(other, _)| other.clone())
            .next()
    };
    match drag.phase {
        rune::el::Phase::Down => {
            let Some(session) = state.arrange.as_mut() else { return };
            session.grab = session.drafts.get(id).copied();
            session.dragging = true;
        }
        rune::el::Phase::Move => {
            let Some(session) = state.arrange.as_mut() else { return };
            let Some(start) = session.grab else { return };
            let raw = (start.x + drag.dx, start.y + drag.dy);
            let (x, y) = arrange::snap_move(id, raw, (start.w, start.h), canvas, &session.drafts, arrange::alt_down());
            if let Some(draft) = session.drafts.get_mut(id) {
                draft.x = x;
                draft.y = y;
            }
            let host = host_under(state, drag.window_point());
            let desk = state.desk();
            state.nest_mark = host.map(|host| {
                let ok = desk.as_ref().is_some_and(|d| d.refusal(id, &host).is_none());
                (host, ok)
            });
        }
        rune::el::Phase::Up => {
            let host = if drag.cancelled() { None } else { host_under(state, drag.window_point()) };
            state.nest_mark = None;
            let Some(session) = state.arrange.as_mut() else { return };
            let start = session.grab.take();
            session.dragging = false;
            if drag.cancelled() {
                if let Some(start) = start {
                    session.drafts.insert(id.to_string(), start);
                }
                return;
            }
            let Some(host) = host else { return };
            match state.desk().map(|d| d.refusal(id, &host)) {
                Some(None) => {
                    if let Some(session) = state.arrange.as_mut() {
                        session.drafts.remove(id);
                    }
                    state.apply(vec![json!({ "op": "nest", "id": id, "host": host })], tx);
                }
                Some(Some(reason)) => {
                    // Refused: the card goes back where the drag began.
                    if let (Some(session), Some(start)) = (state.arrange.as_mut(), start) {
                        session.drafts.insert(id.to_string(), start);
                    }
                    menu::error_toast(state, &reason);
                }
                None => {}
            }
        }
    }
}

fn drag_handle(state: &mut Host, id: &str, edge: Edge, drag: &mut rune::el::Drag) {
    let Some(session) = state.arrange.as_mut() else { return };
    match drag.phase {
        rune::el::Phase::Down => {
            session.grab = session.drafts.get(id).copied();
            session.dragging = true;
        }
        rune::el::Phase::Move => {
            let Some(start) = session.grab else { return };
            let next = arrange::resize(start, edge, (drag.dx, drag.dy), arrange::alt_down());
            session.drafts.insert(id.to_string(), next);
            state.refresh_grain();
        }
        rune::el::Phase::Up => {
            session.grab = None;
            session.dragging = false;
        }
    }
}

pub(crate) fn view(state: &Host, _: &mut rune::runtime::Cx<Host>) -> El {
    let mut root = el().w_full().h_full();
    let Some(layout) = &state.layout else { return root };
    let theme = state.theme();
    let look = Look::panel(&theme);
    let arranging = state.arrange.is_some();
    if arranging {
        root = root.child(arrange::grid(&Look { fg: theme.fg, ..look.clone() }, state.canvas));
    }

    // Widgets switched off (`config.hidden`) or nested are not drawn on the desk; their records
    // stay. The card opened last draws above the others.
    let Some(desk_now) = state.desk() else { return root };
    let tiles = state.card_tiles(&desk_now);
    let mut cards: Vec<(String, El)> = Vec::new();
    let outline = |id: &str| -> Option<Color> {
        match &state.nest_mark {
            Some((host, ok)) if host == id => Some(if *ok { theme.accent } else { theme.status_error }),
            _ => (state.menu.highlight.as_deref() == Some(id)).then_some(theme.accent),
        }
    };
    if desk_now.clock {
        if let Some(tile) = state.clock_tile() {
            let mut overlay = Vec::new();
            if arranging {
                overlay.push(arrange::remove_button(&look, tile, "hit:remove:clock".into()).on_press_tx(|state: &mut Host, tx: &mut Tx| {
                    state.arrange.as_mut().map(|a| a.drafts.remove(desk::CLOCK));
                    state.apply(vec![json!({ "op": "hide", "id": desk::CLOCK })], tx);
                }));
                for (edge, handle) in arrange::handles(&look, tile, true) {
                    overlay.push(handle.key(format!("hit:resize:clock:{edge:?}")).on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, _tx: &mut Tx| drag_handle(state, desk::CLOCK, edge, drag)));
                }
            }
            let clock_ink = theme.ink_on(theme.tint(desk::CLOCK, "calm", "lilac"));
            let card = desk::clock_card(desk::ClockView {
                face: desk::face_of(&layout.clock()),
                time: state.time,
                tile,
                canvas: state.canvas,
                theme: &theme,
                motion: &state.motion,
                grain: &state.grain,
                grain_size: state.grain_size(),
                expanded: state.open.as_deref() == Some(desk::CLOCK),
                transition: state.transition,
                online: state.online,
                highlight: outline(desk::CLOCK).is_some(),
                overlay,
                nested: desk_cards::nested_zone(state, &desk_now, &theme, clock_ink, desk::CLOCK).map(|(zone, _)| zone),
            });
            let card = if arranging {
                card.on_drag(|state: &mut Host, drag: &mut rune::el::Drag, tx: &mut Tx| drag_card(state, desk::CLOCK, drag, tx))
            } else {
                // A press opens the dashboard and a press on the dashboard closes it, as the
                // page's `toggleWidget` does for the header-less clock card.
                card.on_press_tx(|state: &mut Host, tx: &mut Tx| state.toggle_card(desk::CLOCK, tx))
            };
            cards.push((desk::CLOCK.to_string(), card));
        }
    }
    for (card, tile) in desk_now.cards.iter().zip(tiles) {
        let open = state.open.as_deref() == Some(card.id.as_str());
        let target = state.card_box(&desk_now, card, tile);
        let ink = theme.ink_on(card.tint(&theme));
        let mut overlay = Vec::new();
        if arranging {
            let (remove_id, instance) = (card.id.clone(), card.instance);
            overlay.push(arrange::remove_button(&look, tile, format!("hit:remove:{}", card.id)).on_press_tx(move |state: &mut Host, tx: &mut Tx| {
                state.arrange.as_mut().map(|a| a.drafts.remove(&remove_id));
                let op = if instance { "remove_instance" } else { "hide" };
                state.apply(vec![json!({ "op": op, "id": remove_id })], tx);
            }));
            for (edge, handle) in arrange::handles(&look, tile, card.special()) {
                let id = card.id.clone();
                overlay.push(handle.key(format!("hit:resize:{}:{edge:?}", card.id)).on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, _tx: &mut Tx| drag_handle(state, &id, edge, drag)));
            }
        }
        // The body is built only while the card is open or folding.
        let showing = open || state.top.as_deref() == Some(card.id.as_str());
        let body = showing.then(|| {
            let mut body = col().w(desk_cards::BODY_W);
            match card.kind {
                Kind::Folder => body = body.child(desk_cards::folder_body(state, &theme, card, ink, (target.x + 16.0, target.y + 66.0))),
                Kind::Vpn => body = body.child(crate::vpn_panel::panel(state, &theme, ink, &format!("card:{}", card.id), desk_cards::BODY_W, false).0),
            }
            if let Some((zone, _)) = desk_cards::nested_zone(state, &desk_now, &theme, ink, &card.id) {
                body = body.child(el().mt(10.0).child(zone));
            }
            body
        });
        let mut element = desk_cards::card_view(state, desk_cards::CardView {
            card,
            tile,
            target,
            theme: &theme,
            expanded: open,
            transition: state.transition,
            grain: &state.grain,
            grain_size: state.grain_size(),
            outline: outline(&card.id),
            overlay,
            body,
            arranging,
        });
        let id = card.id.clone();
        if arranging {
            element = element.on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, tx: &mut Tx| drag_card(state, &id, drag, tx));
        } else {
            // A folder takes files dropped from Explorer, open or not (`tauri://drag-drop`).
            if card.kind == Kind::Folder {
                let folder = card.widget_type.clone();
                element = element.on_drop_files(move |state: &mut Host, files: &[std::path::PathBuf], _tx: &mut Tx| {
                    let targets: Vec<String> = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
                    crate::shell::log_frontend(state, &format!("folder drop: {} path(s) on {folder}", targets.len()));
                    crate::menu_folders::describe_and_add(state, folder.clone(), targets);
                });
            }
        }
        cards.push((card.id.clone(), element));
    }
    if let Some(top) = &state.top {
        if let Some(index) = cards.iter().position(|(id, _)| id == top) {
            let card = cards.remove(index);
            cards.push(card);
        }
    }
    for (_, card) in cards {
        root = root.child(card);
    }

    if let Some(opener) = menu::opener(state, &theme, state.canvas) {
        root = root.child(opener);
    } else if arranging {
        // Inert at 40 % while arranging (`body.edit-mode .topbar-actions`).
        let lines = [([3.0, 12.0], [21.0, 12.0]), ([3.0, 6.0], [21.0, 6.0]), ([3.0, 18.0], [21.0, 18.0])];
        root = root.child(
            col().absolute(state.canvas.0 - 54.0, 18.0).size(36.0, 36.0).radius(11.0).opacity(0.4)
                .fill(theme.bg_base.alpha(0.78)).border(1.0, theme.bg_elevated.alpha(0.08)).items_center().justify_center()
                .child(el().size(18.0, 18.0).shape(controls::glyph(&lines, theme.fg.alpha(0.45), 2.0))),
        );
    }
    if let Some(bar) = arrange_bar(state, &theme) {
        root = root.child(bar);
    }
    let canvas = state.canvas;
    if let Some(panel) = menu::panel(state, &theme, canvas) {
        root = root.child(panel);
    }
    if let Some(popup) = desk_cards::folder_menu(state, &theme, canvas) {
        root = root.child(popup);
    }
    if let Some((message, _)) = &state.debug {
        // `.debug-message`: centred 70 px above the bottom, red, at most 380 px.
        let width = (canvas.0 * 0.9).min(380.0);
        root = root.child(
            el().absolute(0.0, 0.0).w(canvas.0).h(canvas.1).items_center().justify_end().pb(70.0).child(
                el().max_w(width).px(14.0).py(10.0).radius(12.0).fill(rune::color::hex("#f87171").unwrap_or(Color::WHITE).alpha(0.95))
                    .shadow(12.0, 36.0, Color::BLACK.alpha(0.3))
                    .child(text(message.clone()).font(look.font(12.48, 600.0)).color(rune::color::hex("#111111").unwrap_or(Color::BLACK)).align(rune::el::Align::Center).wrap()),
            ),
        );
    }
    if let Some(toast) = crate::toast::view(&look, &state.toasts, canvas, |state: &mut Host, tx: &mut Tx| {
        if let Some(action) = state.toasts.take_action() {
            action(state, tx);
        }
    }) {
        root = root.child(toast);
    }
    root
}

fn arrange_bar(state: &Host, theme: &Theme) -> Option<El> {
    let session = state.arrange.as_ref()?;
    let look = Look::panel(theme);
    let side_label = if session.side_right { "← На левый край" } else { "На правый край →" };
    let mut children = vec![
        arrange::hint(&look),
        controls::button(&look, "hit:arrange-side".into(), side_label, ButtonKind::Quiet, |state: &mut Host, _tx: &mut Tx| {
            if let Some(a) = state.arrange.as_mut() {
                a.side_right = !a.side_right;
            }
        }),
    ];
    if session.folder_form {
        let name = text_input(&session.folder_name, look.font(13.0, 400.0)).single_line().key("hit:arrange-folder-name")
            .placeholder("Название папки", look.fg.alpha(0.55))
            .on_edit(|state: &mut Host| &mut state.arrange.as_mut().expect("arranging").folder_name)
            .on_submit(|state: &mut Host, _tx: &mut Tx| create_arrange_folder(state));
        let icons: Vec<El> = arrange::BAR_FOLDER_ICONS.iter().map(|(key, _label)| {
            let key = *key;
            let on = session.folder_icon == key;
            col().key(format!("hit:arrange-icon:{key}")).size(32.0, 32.0).radius(9.0).items_center().justify_center()
                .border(1.0, if on { look.accent } else { Color::TRANSPARENT })
                .on_press_tx(move |state: &mut Host, _tx: &mut Tx| {
                    if let Some(a) = state.arrange.as_mut() {
                        a.folder_icon = key;
                    }
                })
        }).collect();
        children.push(
            col().gap(8.0)
                .child(el().w_full().min_h(32.0).px(10.0).py(6.0).radius(9.0).border(1.0, look.elev.alpha(0.16)).fill(look.well.alpha(0.25)).child(name))
                .child(arrange::row_of(icons))
                .child(controls::button(&look, "hit:arrange-folder-create".into(), "Создать", ButtonKind::Normal, |state: &mut Host, _tx: &mut Tx| create_arrange_folder(state))),
        );
    }
    children.push(controls::button(&look, "hit:arrange-folder".into(), "+ Папка", ButtonKind::Normal, |state: &mut Host, _tx: &mut Tx| {
        if let Some(a) = state.arrange.as_mut() {
            a.folder_form = !a.folder_form;
        }
    }));
    let mut add = col().child(controls::button(&look, "hit:arrange-add".into(), "+ Виджет", ButtonKind::Normal, |state: &mut Host, _tx: &mut Tx| {
        if let Some(a) = state.arrange.as_mut() {
            a.add_popover = !a.add_popover;
        }
    }));
    if session.add_popover {
        add = add.child(add_popover(state, &look, session.side_right));
    }
    children.push(add);
    children.push(controls::button(&look, "hit:arrange-cancel".into(), "Отменить", ButtonKind::Normal, |state: &mut Host, tx: &mut Tx| state.finish_arrange(false, tx)));
    children.push(controls::button(&look, "hit:arrange-done".into(), "Готово", ButtonKind::Primary, |state: &mut Host, tx: &mut Tx| state.finish_arrange(true, tx)));
    Some(arrange::bar_frame(&look, theme, state.canvas, session.side_right, session.dragging, children))
}

/// `.widget-add-popover`: beside the island, on the side facing the desk, 250 px wide.
fn add_popover(state: &Host, look: &Look, side_right: bool) -> El {
    let layout = state.layout.as_ref();
    let mut list = col().gap(4.0);
    for module in crate::module_table::desk_modules() {
        let singleton = module.meta["singleton"] == true;
        let placed = layout.is_some_and(|l| !l.is_hidden(&module.id) && (l.config["positions"].get(&module.id).is_some() || l.config["instances"].as_array().is_some_and(|i| i.iter().any(|x| x["widgetType"] == module.id.as_str()))));
        let disabled = singleton && placed;
        let id = module.id.clone();
        let mut option = row().key(format!("hit:add:{id}")).w_full().min_h(48.0).px(8.0).py(7.0).gap(10.0).radius(8.0).items_center()
            .hover(look.elev.alpha(0.07))
            .opacity(if disabled { 0.5 } else { 1.0 })
            .child(col().size(30.0, 30.0).radius(8.0).fill(look.elev.alpha(0.06)).items_center().justify_center())
            .child(
                col().grow(1.0).min_w(0.0).gap(1.0)
                    .child(text(module.meta["name"].as_str().unwrap_or(&module.id).to_string()).font(look.font(11.52, 400.0)).color(look.fg.alpha(0.88)).single_line().truncate())
                    .child(text(module.meta["description"].as_str().unwrap_or(&module.id).to_string()).font(look.font(9.28, 400.0)).color(look.fg.alpha(0.42)).single_line().truncate()),
            );
        if !disabled {
            option = option.on_press_tx(move |state: &mut Host, tx: &mut Tx| {
                if let Some(a) = state.arrange.as_mut() {
                    a.add_popover = false;
                }
                state.add_widget(&id, tx);
            });
        }
        list = list.child(option);
    }
    let x = if side_right { -(250.0 + 12.0) } else { 188.0 + 12.0 - 14.0 };
    col().key("hit:add-popover").absolute(x, -100.0).w(250.0).max_h(420.0).padding(8.0).radius(12.0)
        .fill(look.surface.alpha(0.94)).border(1.0, look.elev.alpha(0.12)).shadow(18.0, 60.0, Color::BLACK.alpha(0.45))
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(list)
}

fn create_arrange_folder(state: &mut Host) {
    let Some(session) = state.arrange.as_mut() else { return };
    let name = session.folder_name.text.trim().to_string();
    let name = if name.is_empty() { "Новая папка".to_string() } else { name };
    let icon = crate::menu_desk::folder_icon(session.folder_icon);
    let id = format!("folder_{}", uuid_like());
    session.created_folders.push(id.clone());
    session.folder_form = false;
    session.folder_name = rune::TextEdit::new("");
    state.call("save_folder", json!({ "folder": { "id": id, "name": name, "icon_svg": icon, "items": [] } }), Box::new(|state: &mut Host, result, _tx: &mut Tx| {
        if let Err(error) = result {
            menu::error_toast(state, &format!("Не получилось: {error}"));
        }
    }));
}

fn uuid_like() -> String {
    let n = crate::theme_record::new_color_seed();
    let m = crate::theme_record::new_color_seed();
    format!("{n:08x}-{:04x}-4{:03x}-{:04x}-{m:08x}{:04x}", n >> 16, m & 0xfff, (n & 0x3fff) | 0x8000, m >> 16)
}

/// Escape, in the page's order: the folder's menu and a rename, the sheet over the menu, the
/// menu (VPN steps back first), arrange mode, then the open card (`collapseAllShellWidgets`).
/// Ctrl+V pastes copied files or a link into the open folder.
pub(crate) fn on_key(state: &mut Host, event: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if event.is_command('v') && !event.mods.shift && !event.mods.alt && state.rename.is_none() && !state.menu.open {
        let folder = state.open.as_ref().and_then(|open| state.desk()?.card(open).filter(|c| c.kind == Kind::Folder).map(|c| c.widget_type.clone()));
        if let Some(folder) = folder {
            crate::menu_folders::paste(state, &folder);
            return true;
        }
        return false;
    }
    if event.key != rune::Key::Escape {
        return false;
    }
    if state.folder_menu.take().is_some() {
        return true;
    }
    if state.rename.is_some() {
        desk_cards::finish_rename(state, false);
        return true;
    }
    if state.vpn.menu.take().is_some() {
        return true;
    }
    if menu::escape(state, tx) {
        return true;
    }
    if state.arrange.is_some() {
        state.finish_arrange(false, tx);
        return true;
    }
    if state.open.is_some() {
        state.close_card(tx);
        return true;
    }
    false
}

/// Runs at the start of every frame: the clock's tick, the toast's end, the menu's slide-out
/// and the arrange settle each ask for one wake, so an idle desk wakes once a second at most.
pub(crate) fn poll(state: &mut Host, _: f64, tx: &mut Tx) -> bool {
    let Some(layout) = &state.layout else { return false };
    let face = desk::face_of(&layout.clock());
    let (now, nanos) = ClockTime::now();
    let mut changed = now.differs(state.time, face);
    if changed {
        state.time = now;
    }
    let mut next = tx.clock() + now.seconds_to_next_tick(face, nanos);
    let instant = Instant::now();
    let had_toast = state.toasts.current.is_some();
    if let Some(until) = state.toasts.expire(instant) {
        next = next.min(tx.clock() + until.saturating_duration_since(instant).as_secs_f64());
    }
    changed |= had_toast && state.toasts.current.is_none();
    let was_shown = state.menu.shown;
    if let Some(until) = menu::settle(state, instant) {
        next = next.min(tx.clock() + until.saturating_duration_since(instant).as_secs_f64());
    }
    changed |= was_shown && !state.menu.shown;
    if let Some(until) = state.settling_until {
        if instant >= until {
            state.settling_until = None;
        }
    }
    if let Some((_, until)) = &state.debug {
        if instant >= *until {
            state.debug = None;
            changed = true;
        } else {
            next = next.min(tx.clock() + until.saturating_duration_since(instant).as_secs_f64());
        }
    }
    let visible = state.vpn_visible();
    if let Some(due) = crate::vpn_panel::poll(state, visible, instant) {
        next = next.min(tx.clock() + due.saturating_duration_since(instant).as_secs_f64());
    }
    // The card that folded last stops being drawn above the rest once its close has run.
    if state.open.is_none() && state.top.is_some() && !state.morph.running() {
        state.top = None;
        changed = true;
    }
    state.inspector.wants_keyboard.store(state.wants_keyboard(), Ordering::Release);
    let wanted: Vec<String> = state.wanted_icons.borrow_mut().drain(..).collect();
    for target in wanted {
        request_icon(state, target);
    }
    tx.wake_at(next);
    changed
}

/// An SVG tinted and rasterized by the image cache, or an empty box of its size until it is.
pub(crate) fn svg_icon(state: &Host, markup: &str, size: f32, colour: Color) -> El {
    match state.svg(markup, size, colour) {
        Some(pixels) => el().size(size, size).pixels(&pixels),
        None => el().size(size, size),
    }
}

/// `logFrontend`: a line in the kernel's log under the frontend source.
pub(crate) fn log_frontend(state: &mut Host, message: &str) {
    state.call("frontend_log", json!({ "message": message }), Box::new(|_: &mut Host, _, _: &mut Tx| {}));
}

/// `shortcutIconUrl`: the kernel's 48 px BGRA icon, turned RGBA.
fn request_icon(state: &mut Host, target: String) {
    let key = target.clone();
    state.call("shortcut_icon", json!({ "target": target, "size": 48 }), Box::new(move |state: &mut Host, result, _tx: &mut Tx| {
        let Ok(icon) = result else { return };
        let (Some(w), Some(h), Some(bgra)) = (icon["width"].as_u64(), icon["height"].as_u64(), icon["bgra"].as_str()) else { return };
        use base64::Engine;
        let Ok(mut bytes) = base64::engine::general_purpose::STANDARD.decode(bgra) else { return };
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let pixels = Pixels::new();
        pixels.set(w as u32, h as u32, bytes);
        state.icons.borrow_mut().insert(key, Some(pixels));
    }));
}

#[cfg(test)]
mod tests {
    #[test]
    fn instance_ids_are_uuid_shaped() {
        assert_eq!(super::uuid_like().len(), 36);
    }
}
