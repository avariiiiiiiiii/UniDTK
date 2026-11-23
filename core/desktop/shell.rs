//! The desk the host draws and what a person does on it: the cards the kernel's layout places
//! (`desk_cards.rs`), each module's widget behind the contract (`widget.rs`), the one surface
//! morph (`surface.rs`), the theme they are painted with (the stored one, or a draft while the
//! menu edits it), the menu (`menu.rs`), arrange mode (`arrange.rs`) and the toast. `host.rs`
//! owns the window, the kernel link and the inspector; this file owns the state they serve.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rune::el::{col, el, text, text_input, El};
use rune::{Color, Image, ImageCache, Pixels, TextEdit, Tx};
use serde_json::{json, Value};

use crate::arrange::{self, Arrange, Edge};
use crate::card::{self, Header, BODY_W, BODY_X, BODY_Y, OPEN_MAX, OPEN_WIDTH};
use crate::controls::{self, ButtonKind, Look};
use crate::desk::{Layout, LocalTime, Placement};
use crate::desk_cards::{self, Card, Desk};
use crate::inspector::Inspector;
use crate::menu::{self, Menu, MenuHost};
use crate::surface::{Morph, Pose};
use crate::theme::{Material, Theme};
use crate::toast::Toasts;
use crate::widget::{Act, Init, Op, Reply, SurfaceSize, View, Widget, WidgetType};

/// `.app-shell { padding: 28px }`: the widgets' canvas (`#widgets`) lies 28 points inside the
/// window on every side. Records are in the canvas's coordinates and anchor to its corners.
pub(crate) const DESK_INSET: f32 = 28.0;
/// Wheel units in one notch (`WHEEL_DELTA`), as `wheelNotches` counts them.
const WHEEL_NOTCH: f32 = 120.0;

/// `wheelNotches`: the whole notches in `carry + delta`, and what is left over for the next turn.
fn take_notches(carry: f32, delta: f32) -> (i32, f32) {
    let travel = carry + delta;
    let notches = (travel / WHEEL_NOTCH).trunc();
    (notches as i32, travel - notches * WHEEL_NOTCH)
}

/// One widget on the desk: its type, the widget, and a generation that tells it from a widget
/// made later under the same id (an answer for the old one is dropped).
pub(crate) struct Slot {
    gen: u64,
    ty: &'static WidgetType,
    widget: Box<dyn Widget>,
    /// It is on screen as a surface (its own, or its host's).
    visible: bool,
    /// When its next wake is due.
    due: Option<Instant>,
}

enum StartupData {
    AwaitingWindow,
    Waiting(Instant),
    Released,
}

/// The desk: the app state Rune's handlers receive.
pub struct Host {
    pub(crate) inspector: Arc<Inspector>,
    /// The latest desk received from the kernel, or the startup fallback.
    pub(crate) layout: Option<Layout>,
    /// Until the layout and both lists are available, or the one startup deadline passes.
    startup_data: StartupData,
    show_requested: bool,
    pub(crate) time: LocalTime,
    pub(crate) canvas: (f32, f32),
    /// Physical pixels per point; images are made in device pixels.
    pub(crate) scale: f32,
    pub(crate) grain: Pixels,
    /// What `grain` was made for: width and height in pixels, and the strength's bits.
    pub(crate) grain_made_for: Option<(u32, u32, u32)>,
    /// The widget types the binary registered.
    pub(crate) types: &'static [&'static WidgetType],
    pub(crate) widgets: BTreeMap<String, Slot>,
    /// The last payload of each kernel event the types subscribed to. A state event's catch-up
    /// comes once, at subscribe, usually before the layout has made any widget; a widget made
    /// later is given it from here.
    sticky: HashMap<String, Value>,
    /// The wheel travel a widget has not yet turned into a whole notch.
    wheel_carry: HashMap<String, f32>,
    next_gen: u64,
    /// What the widgets were last made from: the layout's run and revision, the switched-off.
    synced: Option<(String, u64, Vec<String>)>,
    /// The card open as its surface; one at a time (`r[motion.widget-integrity]`).
    pub(crate) open: Option<String>,
    /// The surfaces open or folding, the last drawn on top.
    pub(crate) morphs: Vec<Morph>,
    /// A morph ran on the last poll: the next one draws its end.
    morphing: bool,
    pub(crate) motion: crate::motion::Motion,
    pub(crate) menu: Menu,
    pub(crate) toasts: Toasts<Host>,
    /// The theme record shown while the menu edits it, before the kernel has it.
    pub(crate) draft: Option<Value>,
    pub(crate) arrange: Option<Arrange>,
    /// After arrange mode ends, the desk takes no press for one motion step.
    pub(crate) settling_until: Option<Instant>,
    /// Where arrange mode left the cards, held until the layout that stores it (or the
    /// snapshot Cancel restores) arrives, so a card does not go back to its old record in
    /// between and then move again; the revision is the one `desk_apply` answered with.
    pub(crate) left_drafts: Option<(BTreeMap<String, Placement>, Option<u64>)>,
    /// The handle the host's own callbacks come back through.
    pub(crate) ui: Arc<OnceLock<rune::AppHandle<Host>>>,
    pub(crate) images: Option<ImageCache>,
    pub(crate) svgs: RefCell<HashMap<(String, u32), Image>>,
    pub(crate) icons: RefCell<HashMap<String, Option<Pixels>>>,
    /// Shortcut icons the view asked for that the poll has not requested yet.
    pub(crate) wanted_icons: RefCell<Vec<String>>,
    pub(crate) folder_menu: Option<desk_cards::FolderMenu>,
    pub(crate) rename: Option<desk_cards::Rename>,
    pub(crate) shortcut_drag: Option<desk_cards::ShortcutDrag>,
    /// `showDebugMessage`: the red line at the bottom, for 4 s.
    pub(crate) debug: Option<(String, Instant)>,
    /// The host under a card dragged in arrange mode, and whether it would take it.
    pub(crate) nest_mark: Option<(String, bool)>,
    /// What a text field of a dismissed widget or settings pane edits.
    pub(crate) scratch: TextEdit,
    /// The blurred wallpaper a flat card wears under its glass (`wallpaper.rs`), what it was
    /// made from, and whether a worker is making it.
    pub(crate) backdrop: Pixels,
    backdrop_for: Option<(crate::wallpaper::Source, u32)>,
    backdrop_making: bool,
}

impl Host {
    pub(crate) fn new(
        inspector: Arc<Inspector>,
        layout: Option<Layout>,
        canvas: (f32, f32),
        scale: f32,
        ui: Arc<OnceLock<rune::AppHandle<Host>>>,
        types: &'static [&'static WidgetType],
    ) -> Host {
        let motion = layout.as_ref().map_or_else(|| crate::motion::Motion::new(crate::motion::Profile::Balanced, 1.0), |l| l.theme.motion());
        let mut host = Host {
            inspector,
            startup_data: if layout.is_none() { StartupData::AwaitingWindow } else { StartupData::Released },
            show_requested: false,
            layout,
            time: LocalTime::now(),
            canvas,
            scale,
            grain: Pixels::new(),
            grain_made_for: None,
            types,
            widgets: BTreeMap::new(),
            sticky: HashMap::new(),
            wheel_carry: HashMap::new(),
            next_gen: 1,
            synced: None,
            open: None,
            morphs: Vec::new(),
            morphing: false,
            motion,
            menu: Menu::default(),
            toasts: Toasts::default(),
            draft: None,
            arrange: None,
            settling_until: None,
            left_drafts: None,
            ui,
            images: None,
            svgs: RefCell::new(HashMap::new()),
            icons: RefCell::new(HashMap::new()),
            wanted_icons: RefCell::new(Vec::new()),
            folder_menu: None,
            rename: None,
            shortcut_drag: None,
            debug: None,
            nest_mark: None,
            scratch: TextEdit::new(""),
            backdrop: Pixels::new(),
            backdrop_for: None,
            backdrop_making: false,
        };
        host.refresh_grain();
        host
    }

    pub(crate) fn start_snapshot_deadline(&mut self) {
        if matches!(self.startup_data, StartupData::AwaitingWindow) {
            self.startup_data = StartupData::Waiting(Instant::now() + Duration::from_millis(750));
        }
    }

    /// The theme the desk shows: the draft while one is being edited, else the stored one.
    pub fn theme(&self) -> Theme {
        match (&self.draft, &self.layout) {
            (Some(draft), _) => Theme::normalize(draft),
            (None, Some(layout)) => layout.theme.clone(),
            (None, None) => Theme::normalize(&Value::Null),
        }
    }

    /// Takes a layout from the kernel. True when it moves, adds or removes cards: the page sets
    /// their `left`/`top` with no transition, so the caller lands that change at once.
    pub(crate) fn accept_layout(&mut self, layout: Layout) -> bool {
        // A reply and an event can arrive in either order; the newer revision wins, and any
        // revision of a new kernel run.
        let mut moved = false;
        if self.layout.as_ref().map_or(true, |known| layout.supersedes(known)) {
            moved = self.layout.as_ref().is_none_or(|known| {
                ["positions", "hidden", "nested", "instances"].iter().any(|key| known.config.get(*key) != layout.config.get(*key))
            });
            // The kernel has the theme the draft showed: the stored one shows from now on.
            if self.draft.as_ref().is_some_and(|draft| crate::theme_record::normalize(draft) == crate::theme_record::normalize(&layout.config["theme"])) {
                self.draft = None;
            }
            let first_colours = layout.config.is_object() && layout.config["colorOverrides"].get(crate::theme_record::USER_COLOR_PRESET).is_none();
            let new_run = self.layout.as_ref().map_or(true, |known| known.instance != layout.instance);
            if new_run || self.left_drafts.as_ref().is_some_and(|(_, rev)| rev.is_some_and(|rev| layout.rev >= rev)) {
                self.left_drafts = None;
            }
            self.layout = Some(layout);
            // The theme shown (a draft still being edited wins over the stored one) times
            // every motion, the morph's as the hover's.
            self.motion = self.theme().motion();
            if new_run {
                // Only lists from this kernel run may complete the startup snapshot.
                if !matches!(self.startup_data, StartupData::Released) {
                    self.menu.disabled_modules = crate::menu::Loaded::Loading;
                    self.menu.folders = crate::menu::Loaded::Loading;
                }
                crate::menu_desk::load_lists(self);
            }
            self.refresh_grain();
            self.refresh_backdrop();
            if first_colours {
                self.deal_user_colours();
            }
        }
        moved
    }

    /// `initThemeSystem`'s first run: the user's own Sea Glass colours, dealt once and stored;
    /// applied at once when Sea Glass is the active theme.
    fn deal_user_colours(&mut self) {
        // A host without the kernel stores nothing; it draws the defaults.
        if self.inspector.outbox.lock().map_or(true, |slot| slot.is_none()) {
            return;
        }
        let Some(layout) = &self.layout else { return };
        let targets = crate::theme_record::tint_targets(crate::catalog::desk_modules().map(|m| &m.meta));
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

    fn type_of(&self, widget_type: &str) -> Option<&'static WidgetType> {
        self.types.iter().copied().find(|ty| ty.id == widget_type)
    }

    /// What the desk shows: the layout with the folders and the switched-off modules.
    pub(crate) fn desk(&self) -> Option<Desk> {
        let layout = self.layout.as_ref()?;
        let folders = self.menu.folders.ready().map_or(&[][..], Vec::as_slice);
        let empty = HashSet::new();
        let disabled = self.menu.disabled_modules.ready().unwrap_or(&empty);
        Some(desk_cards::desk(layout, folders, disabled, &|id| self.type_of(id).is_some()))
    }

    /// Makes, keeps and drops the widgets so they are the ones the desk shows, and tells each
    /// its record. The open card and the morphs of widgets gone go with them.
    fn sync_widgets(&mut self) -> Vec<String> {
        let Some(desk) = self.desk() else { return Vec::new() };
        let Some(layout) = self.layout.as_ref() else { return Vec::new() };
        let mut keep = HashSet::new();
        let mut made = Vec::new();
        for (id, entry) in &desk.widgets {
            let Some(ty) = self.type_of(&entry.widget_type) else { continue };
            keep.insert(id.clone());
            let record = layout.record_value(id, &entry.widget_type);
            match self.widgets.get_mut(id) {
                Some(slot) if std::ptr::eq(slot.ty, ty) => slot.widget.sync(&record),
                _ => {
                    let widget = (ty.make)(&Init { id, record: &record });
                    self.widgets.insert(id.clone(), Slot { gen: self.next_gen, ty, widget, visible: false, due: None });
                    self.next_gen += 1;
                    made.push(id.clone());
                }
            }
        }
        self.widgets.retain(|id, _| keep.contains(id));
        if self.open.as_ref().is_some_and(|open| desk.card(open).is_none()) {
            self.open = None;
        }
        self.morphs.retain(|m| desk.card(&m.id).is_some());
        if let Some(session) = self.arrange.as_mut() {
            session.drafts.retain(|id, _| desk.card(id).is_some());
        }
        if self.menu.highlight.as_ref().is_some_and(|id| !desk.widgets.contains_key(id)) {
            self.menu.highlight = None;
        }
        made
    }

    /// The last payloads of the events a just-made widget's type asked for, as the catch-up frame
    /// a subscribe replays (`snapshot`).
    fn replay_events(&mut self, made: &[String], tx: &mut Tx) {
        for id in made {
            let Some(ty) = self.widgets.get(id).map(|slot| slot.ty) else { continue };
            for &event in ty.events {
                if let Some(payload) = self.sticky.get(event).cloned() {
                    self.with_dyn(id, tx, |widget, act| widget.on_event(event, &payload, true, act));
                }
            }
        }
    }

    /// The widgets' canvas in the window (`#widgets`): what records are relative to.
    pub(crate) fn desk_area(&self) -> Placement {
        Placement { x: DESK_INSET, y: DESK_INSET, w: (self.canvas.0 - 2.0 * DESK_INSET).max(0.0), h: (self.canvas.1 - 2.0 * DESK_INSET).max(0.0) }
    }

    /// Each card's place in the window: its draft while arranging, its record otherwise.
    pub(crate) fn card_tiles(&self, desk: &Desk) -> Vec<Placement> {
        let Some(layout) = &self.layout else { return Vec::new() };
        let placed = desk_cards::place(layout, &self.theme(), &desk.cards, self.desk_area());
        let drafts = self.arrange.as_ref().map(|a| &a.drafts).or(self.left_drafts.as_ref().map(|(drafts, _)| drafts));
        desk.cards.iter().zip(placed).map(|(card, at)| drafts.and_then(|d| d.get(&card.id)).copied().unwrap_or(at)).collect()
    }

    /// The view a widget draws with.
    fn view_of<'a>(&'a self, id: &'a str, theme: &'a Theme, tint: Color, ink: Color, frame: Placement, tile: Placement, pose: Option<Pose>) -> View<'a> {
        View {
            id,
            theme,
            motion: &self.motion,
            now: self.time,
            canvas: self.canvas,
            tint,
            ink,
            frame,
            tile,
            progress: pose.map_or(0.0, |p| p.progress),
            open: self.open.as_deref() == Some(id),
            arranging: self.arrange.is_some(),
            host: self,
        }
    }

    /// The box a card opens into, or `None` when it does not open: a folder is 400 wide and
    /// as high as its body and 80 more, 500 at most; a widget says its own size.
    fn surface_frame(&self, desk: &Desk, card: &Card, tile: Placement, theme: &Theme) -> Option<Placement> {
        let tint = card.tint(theme);
        let ink = theme.ink_on(tint);
        let nested = desk_cards::nested_zone(self, desk, theme, ink, &card.id).map_or(0.0, |(_, h)| 10.0 + h);
        let size = if card.folder {
            (OPEN_WIDTH, (desk_cards::folder_height(theme, &card.items) + nested + 80.0).min(OPEN_MAX))
        } else {
            let slot = self.widgets.get(&card.id)?;
            let probe = Placement { x: 0.0, y: 0.0, w: OPEN_WIDTH, h: OPEN_MAX };
            let view = self.view_of(&card.id, theme, tint, ink, probe, Placement { x: 0.0, y: 0.0, w: tile.w, h: tile.h }, None);
            match slot.widget.surface(&view)?.size {
                SurfaceSize::Box(w, h) => (w, h),
                SurfaceSize::Body(body) => (OPEN_WIDTH, (body + nested + 80.0).min(OPEN_MAX)),
            }
        };
        Some(card::surface_box(tile, size, self.canvas))
    }

    /// The size the grain image covers: every tile and every surface on the desk, rounded up
    /// to 64 pt, so a card clips it and never stretches it while it grows.
    pub(crate) fn grain_size(&self) -> (f32, f32) {
        let mut size = (0.0f32, 0.0f32);
        if let Some(desk) = self.desk() {
            for tile in self.card_tiles(&desk) {
                size = (size.0.max(tile.w), size.1.max(tile.h));
            }
            if !desk.cards.is_empty() {
                size = (size.0.max(OPEN_WIDTH), size.1.max(OPEN_MAX));
            }
        }
        // The menu panel wears the same grain, as tall as the window less its margins.
        size = (size.0.max((self.canvas.0 - 24.0).min(420.0)), size.1.max(self.canvas.1 - 24.0));
        ((size.0 / 64.0).ceil() * 64.0, (size.1 / 64.0).ceil() * 64.0)
    }

    /// The grain is made again only when its size or the theme's grain strength changes.
    pub(crate) fn refresh_grain(&mut self) {
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
            self.grain.set(size.0, size.1, crate::desk::grain_rgba(size.0, size.1, self.scale, strength));
            self.grain_made_for = Some(made_for);
        }
    }

    /// The backdrop of both materials: made again on a worker when the wallpaper, its placement
    /// or the theme's blur changes.
    pub(crate) fn refresh_backdrop(&mut self) {
        let theme = self.theme();
        // Both materials wear `backdrop-filter` (`.widget-card` in `styles.css`); one being
        // made is checked again when it is done, so a blur changed meanwhile is not lost.
        if theme.blur <= 0.0 || self.backdrop_making {
            return;
        }
        let source = crate::wallpaper::current();
        let key = (source.clone(), theme.blur.to_bits());
        if self.backdrop_for.as_ref() == Some(&key) {
            return;
        }
        let Some(ui) = self.ui.get().cloned() else { return };
        self.backdrop_making = true;
        let (w, h) = crate::desktop_host::primary_size();
        let sigma = theme.blur * self.scale;
        let _ = std::thread::Builder::new().name("wallpaper-blur".into()).spawn(move || {
            let made = crate::wallpaper::make(&source, (w.max(1) as u32, h.max(1) as u32), sigma);
            ui.update(move |host: &mut Host, _tx: &mut Tx| {
                host.backdrop_making = false;
                match made {
                    Some(image) => {
                        host.backdrop.set(image.width(), image.height(), image.into_raw());
                        host.backdrop_for = Some(key);
                    }
                    None => {
                        host.backdrop.set(1, 1, vec![0, 0, 0, 0]);
                        host.backdrop_for = Some(key);
                    }
                }
                host.refresh_backdrop();
            });
        });
    }

    /// The tile and the box of a card on the desk.
    fn tile_and_frame(&self, id: &str) -> Option<(Placement, Placement)> {
        let desk = self.desk()?;
        let index = desk.cards.iter().position(|c| c.id == id)?;
        let tile = *self.card_tiles(&desk).get(index)?;
        let frame = self.surface_frame(&desk, &desk.cards[index], tile, &self.theme())?;
        Some((tile, frame))
    }

    /// `toggleWidget`'s open: another open card folds first, then this one grows out of its
    /// tile (`r[motion.widget-integrity]`, `r[motion.clip-morph]`).
    pub(crate) fn open_card(&mut self, id: &str, tx: &mut Tx) {
        let Some((tile, frame)) = self.tile_and_frame(id) else { return };
        self.folder_menu = None;
        desk_cards::finish_rename(self, true);
        let now = Instant::now();
        if let Some(other) = self.open.take() {
            if let Some(morph) = self.morphs.iter_mut().find(|m| m.id == other) {
                morph.close(now);
            }
        }
        let theme = self.theme();
        let strip = self.desk().and_then(|d| d.card(id).map(|c| c.strip)).unwrap_or(false);
        let morph = match self.morphs.iter().position(|m| m.id == id) {
            Some(index) => {
                // A close that is still running turns round from where it is.
                let mut morph = self.morphs.remove(index);
                morph.reopen(now);
                morph
            }
            None => Morph::open(id, &self.motion, frame, tile, card::tile_radius(&theme, strip), card::surface_radius(&theme), now),
        };
        self.morphs.push(morph);
        self.open = Some(id.to_string());
        tx.instant();
        crate::surface::keep_frames(tx);
        self.update_visibility(tx);
    }

    /// The close runs on `surfaceExit`, shorter when it interrupts the open.
    pub(crate) fn close_card(&mut self, tx: &mut Tx) {
        let Some(id) = self.open.take() else { return };
        self.folder_menu = None;
        desk_cards::finish_rename(self, true);
        if let Some(morph) = self.morphs.iter_mut().find(|m| m.id == id) {
            morph.close(Instant::now());
        }
        tx.instant();
        crate::surface::keep_frames(tx);
        self.update_visibility(tx);
    }

    /// A press on a card's tile or on its open surface.
    pub(crate) fn toggle_card(&mut self, id: &str, tx: &mut Tx) {
        if self.arrange.is_some() {
            return;
        }
        if self.open.as_deref() == Some(id) {
            self.close_card(tx);
        } else {
            self.open_card(id, tx);
        }
    }

    /// Tells each widget whether it is on screen now: the open card's own widget and the ones
    /// nested in it.
    fn update_visibility(&mut self, tx: &mut Tx) {
        let mut on = HashSet::new();
        if let Some(open) = &self.open {
            on.insert(open.clone());
            if let Some(desk) = self.desk() {
                for child in desk.nested.get(open).into_iter().flatten() {
                    on.insert(child.clone());
                }
            }
        }
        let changed: Vec<(String, bool)> = self.widgets.iter().filter(|(id, slot)| slot.visible != on.contains(*id)).map(|(id, _)| (id.clone(), on.contains(id))).collect();
        for (id, visible) in changed {
            if let Some(slot) = self.widgets.get_mut(&id) {
                slot.visible = visible;
                slot.due = None;
            }
            self.with_dyn(&id, tx, |widget, act| widget.shown(visible, act));
        }
    }

    /// The widgets on screen as surfaces, the open one first.
    fn visible_widgets(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.widgets.iter().filter(|(_, slot)| slot.visible).map(|(id, _)| id.clone()).collect();
        if let Some(open) = &self.open {
            ids.sort_by_key(|id| id != open);
        }
        ids
    }

    /// `showDebugMessage`.
    pub(crate) fn show_debug(&mut self, text: String, tx: &mut Tx) {
        self.debug = Some((text, Instant::now() + Duration::from_secs(4)));
        crate::surface::keep_frames(tx);
    }

    /// Whether the host wants the desk's keys: a menu, the arrange island or a surface is open.
    pub(crate) fn wants_keyboard(&self) -> bool {
        self.menu.open || self.arrange.is_some() || self.open.is_some() || self.rename.is_some() || self.folder_menu.is_some()
    }

    /// A press on the bare desk, outside the host: what is on top closes first — the folder's
    /// menu, a rename, a widget's own popup, the sheet over the menu, then the open card
    /// (`setupDesktopOutsideClickHandler`). The menu panel itself is not a surface and stays.
    pub(crate) fn pointer_outside(&mut self, tx: &mut Tx) {
        if self.folder_menu.is_some() {
            self.folder_menu = None;
            return;
        }
        if self.rename.is_some() {
            desk_cards::finish_rename(self, true);
            return;
        }
        for id in self.visible_widgets() {
            let mut closed = false;
            self.with_dyn(&id, tx, |widget, act| closed = widget.outside(act));
            if closed {
                return;
            }
        }
        if self.menu.sheet.is_some() {
            menu::close_sheet(self, tx);
        } else {
            self.close_card(tx);
        }
    }

    /// Wheel travel over the desk at window point (`x`, `y`) (`wallpaper-wheel`): the card whose
    /// tile is under it gets whole notches, and the rest of a turn waits for the next (a
    /// touchpad sends many small ones). Nothing takes it while arranging, with the menu or a
    /// surface open: there the page scrolls what is under the pointer, and nothing here scrolls.
    pub(crate) fn wheel(&mut self, x: f32, y: f32, delta: f32, touchpad: bool, tx: &mut Tx) {
        if self.arrange.is_some() || self.menu.open || self.open.is_some() {
            return;
        }
        let Some(desk) = self.desk() else { return };
        let under = desk.cards.iter().zip(self.card_tiles(&desk)).find(|(_, t)| x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h).map(|(card, _)| card.id.clone());
        let Some(id) = under else { return };
        let (notches, carry) = take_notches(self.wheel_carry.get(&id).copied().unwrap_or(0.0), delta);
        self.wheel_carry.insert(id.clone(), carry);
        if notches != 0 {
            self.with_dyn(&id, tx, |widget, act| {
                widget.wheel(notches, touchpad, act);
            });
        }
    }

    /// `enterArrangeMode`: the open card folds, the island appears, every card the desk draws
    /// becomes a draft.
    pub(crate) fn start_arrange(&mut self, tx: &mut Tx) {
        if self.layout.is_none() {
            return;
        }
        self.close_card(tx);
        let desk = self.desk();
        let tiles = desk.as_ref().map(|d| self.card_tiles(d)).unwrap_or_default();
        let Some(layout) = &self.layout else { return };
        let drafts: BTreeMap<String, Placement> = desk.iter().flat_map(|d| d.cards.iter()).zip(tiles).map(|(card, tile)| (card.id.clone(), tile)).collect();
        let side_right = layout.config["arrangeSide"].as_str() != Some("left");
        self.arrange = Some(Arrange::new(layout.rev, layout.config.clone(), drafts, side_right));
    }

    /// `leaveArrangeMode`: "Готово" pins every card still on the desk to its nearest corner
    /// and writes the drafts in one change from the session's revision; "Отменить" puts the
    /// layout back as the session found it and deletes the folders it created. The menu comes
    /// back.
    pub(crate) fn finish_arrange(&mut self, done: bool, tx: &mut Tx) {
        let Some(session) = self.arrange.take() else { return };
        self.left_drafts = Some((session.drafts.clone(), None));
        let side = json!({ "op": "arrange_side", "side": if session.side_right { "right" } else { "left" } });
        if done {
            let on_desk: HashSet<String> = self.desk().map(|d| d.cards.into_iter().map(|c| c.id).collect()).unwrap_or_default();
            let area = self.desk_area();
            let mut ops: Vec<Value> = session.drafts.iter().filter(|(id, _)| on_desk.contains(*id)).map(|(id, at)| {
                let local = Placement { x: at.x - area.x, y: at.y - area.y, ..*at };
                let mut op = arrange::pin(local, (area.w, area.h));
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
        self.settling_until = Some(Instant::now() + Duration::from_secs_f32(self.motion.duration(crate::motion::Token::Motion)));
        crate::surface::keep_frames(tx);
        if !self.menu.open {
            menu::toggle(self, tx);
        }
    }

    /// Arrange mode's one `desk_apply`. The drafts it leaves (`left_drafts`) go when the
    /// layout of the revision it answers with arrives, or at once when it fails.
    fn apply_from(&mut self, ops: Vec<Value>, base_rev: Option<u64>, _tx: &mut Tx) {
        let mut args = json!({ "ops": ops });
        if let Some(base) = base_rev {
            args["baseRev"] = json!(base);
        }
        self.call("desk_apply", args, Box::new(|state: &mut Host, result, tx: &mut Tx| {
            match result.map(|answer| answer["rev"].as_u64()) {
                Ok(Some(rev)) if state.layout.as_ref().is_none_or(|known| known.rev < rev) => {
                    if let Some((_, until)) = state.left_drafts.as_mut() {
                        *until = Some(rev);
                    }
                }
                Ok(_) => state.left_drafts = None,
                Err(error) => {
                    // The cards go back to their stored places, as the page re-renders them.
                    tx.instant();
                    state.left_drafts = None;
                    menu::error_toast(state, &format!("Не получилось: {error}"));
                }
            }
        }));
    }

    /// `createWidgetInstance`: a hidden base widget comes back instead; a singleton that is
    /// placed is refused; otherwise a new instance at `defaultWidgetPosition`.
    fn add_widget(&mut self, widget_type: &str, tx: &mut Tx) {
        let Some(layout) = &self.layout else { return };
        if layout.is_hidden(widget_type) {
            self.apply(vec![json!({ "op": "show", "id": widget_type })], tx);
            return;
        }
        let singleton = crate::catalog::desk_modules().any(|m| m.id == widget_type && m.meta["singleton"] == true);
        let placed = layout.config["instances"].as_array().is_some_and(|items| items.iter().any(|i| i["widgetType"] == widget_type))
            || layout.config["positions"].get(widget_type).is_some();
        if singleton && placed {
            return;
        }
        // The module's `instanceSize`, else `defaultWidgetPosition`'s 340 x 64.
        let size = crate::catalog::desk_modules().find(|m| m.id == widget_type).map(|m| m.meta["instanceSize"].clone()).unwrap_or_default();
        let (w, h) = (size["w"].as_f64().map_or(340.0, |w| w as f32), size["h"].as_f64().map_or(64.0, |h| h as f32));
        let id = format!("{widget_type}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
        // `defaultWidgetPosition`, in the canvas's coordinates.
        let position = json!({ "x": (self.desk_area().w - w - 28.0).max(0.0), "y": 68.0, "w": w, "h": h });
        self.apply(vec![json!({ "op": "add_instance", "id": id, "type": widget_type, "position": position })], tx);
    }

    /// Runs `f` on widget `id` if it is a `W` and still on the desk, then what it asked for.
    pub(crate) fn with_widget<W: Widget>(&mut self, id: &str, tx: &mut Tx, f: impl FnOnce(&mut W, &mut Act<'_>)) {
        self.with_dyn(id, tx, |widget, act| {
            let any: &mut dyn Any = widget;
            if let Some(widget) = any.downcast_mut::<W>() {
                f(widget, act);
            }
        });
    }

    fn with_dyn(&mut self, id: &str, tx: &mut Tx, f: impl FnOnce(&mut dyn Widget, &mut Act<'_>)) {
        let Some(slot) = self.widgets.get_mut(id) else { return };
        let gen = slot.gen;
        let mut act = Act::new(id, tx);
        f(slot.widget.as_mut(), &mut act);
        let ops = std::mem::take(&mut act.ops);
        drop(act);
        self.run_ops(id, gen, ops, tx);
    }

    /// A text field inside widget `id`, or the scratch field when it is gone.
    pub(crate) fn widget_field<W: Widget>(&mut self, id: &str, field: fn(&mut W) -> &mut TextEdit) -> &mut TextEdit {
        let found = self.widgets.get_mut(id).and_then(|slot| {
            let any: &mut dyn Any = slot.widget.as_mut();
            any.downcast_mut::<W>()
        });
        match found {
            Some(widget) => field(widget),
            None => &mut self.scratch,
        }
    }

    /// What a widget asked for in a handler, applied now that the handler is done.
    fn run_ops(&mut self, id: &str, gen: u64, ops: Vec<Op>, tx: &mut Tx) {
        for op in ops {
            match op {
                Op::Call { cmd, args, reply } => {
                    let owner = id.to_string();
                    match reply {
                        Some(reply) => self.call(&cmd, args, Box::new(move |host: &mut Host, result, tx: &mut Tx| host.reply(&owner, gen, reply, result, tx))),
                        None => {
                            let what = cmd.clone();
                            self.call(&cmd, args, Box::new(move |host: &mut Host, result, _tx: &mut Tx| {
                                if let Err(error) = result {
                                    log_frontend(host, &format!("{owner}: {what} failed: {error}"));
                                }
                            }))
                        }
                    }
                }
                Op::Save(fields) => {
                    let mut op = fields;
                    op["op"] = json!(if position_fields(&op) { "position" } else { "settings" });
                    op["id"] = json!(id);
                    self.apply(vec![op], tx);
                }
                Op::Toast { text, error } => {
                    self.toasts.show(text, error, None);
                    crate::surface::keep_frames(tx);
                }
                Op::Debug(text) => self.show_debug(text, tx),
                Op::Log(text) => log_frontend(self, &text),
                Op::Open => {
                    if self.open.as_deref() != Some(id) {
                        self.open_card(id, tx);
                    }
                }
                Op::Close => {
                    if self.open.as_deref() == Some(id) {
                        self.close_card(tx);
                    }
                }
                Op::Redraw => {}
            }
        }
    }

    fn reply(&mut self, id: &str, gen: u64, reply: Reply, result: Result<Value, String>, tx: &mut Tx) {
        let Some(slot) = self.widgets.get_mut(id).filter(|slot| slot.gen == gen) else { return };
        let mut act = Act::new(id, tx);
        reply(slot.widget.as_mut(), result, &mut act);
        let ops = std::mem::take(&mut act.ops);
        drop(act);
        self.run_ops(id, gen, ops, tx);
    }

    /// A kernel event, to every widget whose type asked for it.
    pub(crate) fn deliver_event(&mut self, event: &str, payload: &Value, snapshot: bool, tx: &mut Tx) {
        self.sticky.insert(event.to_string(), payload.clone());
        let ids: Vec<String> = self.widgets.iter().filter(|(_, slot)| slot.ty.events.contains(&event)).map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.with_dyn(&id, tx, |widget, act| widget.on_event(event, payload, snapshot, act));
        }
    }

    /// A nested widget's compact form in its host's zone, `width` wide, in the host's ink.
    pub(crate) fn compact_of(&self, child: &str, host: &str, ink: Color, width: f32) -> Option<(El, f32)> {
        let slot = self.widgets.get(child)?;
        let theme = self.theme();
        let frame = Placement { x: 0.0, y: 0.0, w: width, h: 0.0 };
        let pose = self.morphs.iter().find(|m| m.id == host).map(|m| m.pose(Instant::now()));
        let view = self.view_of(child, &theme, ink, ink, frame, frame, pose);
        slot.widget.compact(&view)
    }
}

/// The fields `desk_apply`'s `position` takes; anything else is a widget's setting.
fn position_fields(op: &Value) -> bool {
    op.as_object().is_some_and(|fields| fields.keys().all(|k| matches!(k.as_str(), "op" | "id" | "x" | "y" | "w" | "h" | "ax" | "ay" | "anchor" | "version")))
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
        self.refresh_backdrop();
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

    fn grain(&self) -> (&Pixels, (f32, f32)) {
        (&self.grain, self.grain_size())
    }

    fn draws(&self, id: &str) -> bool {
        self.type_of(id).is_some()
    }

    fn settings_factory(&self, id: &str) -> Option<crate::settings::Factory> {
        self.type_of(id)?.settings
    }

    fn svg(&self, markup: &str, size: f32, colour: Color) -> Option<Pixels> {
        let images = self.images.as_ref()?;
        // The colour, opaque, replaces `currentColor`; its alpha is the drawing's opacity.
        let hex = colour.hex();
        let tinted = markup.replace("currentColor", &hex[..7]).replacen("<svg ", &format!("<svg opacity=\"{:.3}\" ", colour.a), 1);
        let key = (tinted.clone(), size.to_bits());
        let mut svgs = self.svgs.borrow_mut();
        // Every colour a theme preview passes through is a new entry: the cache is emptied
        // when it grows past what one desk draws, so dragging a slider cannot grow it forever.
        if svgs.len() >= 512 && !svgs.contains_key(&key) {
            svgs.clear();
        }
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

    fn desk_size(&self) -> (f32, f32) {
        let area = self.desk_area();
        (area.w, area.h)
    }

    fn arranging(&self) -> bool {
        self.arrange.is_some()
    }
}

/// A card's drag in arrange mode: it follows the pointer and snaps; over a card that hosts
/// others it marks the host (`nest-target`, or `nest-refused` with the reason on release), and
/// a release there nests it (`r[desk.nesting]`).
fn drag_card(state: &mut Host, id: &str, drag: &mut rune::el::Drag, tx: &mut Tx) {
    let area = state.desk_area();
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
            session.dragged = Some(id.to_string());
        }
        rune::el::Phase::Move => {
            let Some(session) = state.arrange.as_mut() else { return };
            let Some(start) = session.grab else { return };
            // `snapMove` works in the canvas's coordinates.
            let raw = (start.x + drag.dx - area.x, start.y + drag.dy - area.y);
            let others: BTreeMap<String, Placement> = session.drafts.iter().map(|(other, at)| (other.clone(), Placement { x: at.x - area.x, y: at.y - area.y, ..*at })).collect();
            let (x, y) = arrange::snap_move(id, raw, (start.w, start.h), (area.w, area.h), &others, arrange::alt_down());
            if let Some(draft) = session.drafts.get_mut(id) {
                draft.x = x + area.x;
                draft.y = y + area.y;
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
            session.dragged = None;
            if drag.cancelled() {
                if let Some(start) = start {
                    session.drafts.insert(id.to_string(), start);
                }
                return;
            }
            let Some(host) = host else { return };
            match state.desk().map(|d| d.refusal(id, &host)) {
                Some(None) => {
                    // The draft stays where it was dropped until the layout nests the card, so
                    // it does not go back to its stored place first; a nested card is not drawn.
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

/// One card, a module's widget or a folder, at its pose.
fn card_element(state: &Host, desk: &Desk, card: &Card, tile: Placement, theme: &Theme, look: &Look, now: Instant, grain_size: (f32, f32)) -> El {
    let arranging = state.arrange.is_some();
    let morph = state.morphs.iter().find(|m| m.id == card.id);
    let pose = morph.map(|m| m.pose(now));
    let frame = morph.map_or(tile, |m| m.frame);
    let tile_in_box = Placement { x: tile.x - frame.x, y: tile.y - frame.y, w: tile.w, h: tile.h };
    let tint = card.tint(theme);
    let ink = theme.ink_on(tint);
    let open = state.open.as_deref() == Some(card.id.as_str());
    let opening = pose.is_some_and(|p| p.open && !p.done);

    // The nesting zone, shown while the card is a surface.
    let zone = pose.and_then(|_| desk_cards::nested_zone(state, desk, theme, ink, &card.id));
    let mut content = None;
    let mut nest = None;
    let mut tools = Vec::new();
    if card.folder {
        if pose.is_some() {
            let mut body = col().w(BODY_W).child(desk_cards::folder_body(state, theme, card, ink, (frame.x + BODY_X, frame.y + BODY_Y)));
            if let Some((zone, _)) = zone {
                body = body.child(el().mt(10.0).child(zone));
            }
            content = Some(body_region(body, frame, opening.then(|| card.id.clone())));
        }
    } else if let Some(slot) = state.widgets.get(&card.id) {
        let view = state.view_of(&card.id, theme, tint, ink, frame, tile_in_box, pose);
        if card.bare {
            content = slot.widget.content(&view);
            if let (Some(zone), Some(spec)) = (zone, slot.widget.surface(&view)) {
                if let Some(at) = spec.nest {
                    nest = Some((zone.0, at));
                }
            }
        } else if pose.is_some() {
            let mut body = col().w(BODY_W).children(slot.widget.content(&view));
            if let Some((zone, _)) = zone {
                body = body.child(el().mt(10.0).child(zone));
            }
            content = Some(body_region(body, frame, opening.then(|| card.id.clone())));
        }
        if arranging {
            tools = slot.widget.arrange_tools(&view);
        }
    }

    let mut overlay = Vec::new();
    if arranging {
        let (remove_id, instance) = (card.id.clone(), card.instance);
        overlay.push(arrange::remove_button(look, tile, format!("hit:remove:{}", card.id)).on_press_tx(move |state: &mut Host, tx: &mut Tx| {
            // The draft goes when the layout drops the card (`sync_widgets`), not before, so the
            // card does not jump to its stored place on the way out.
            let op = if instance { "remove_instance" } else { "hide" };
            state.apply(vec![json!({ "op": op, "id": remove_id })], tx);
        }));
        for (edge, handle) in arrange::handles(look, tile, card.special()) {
            let id = card.id.clone();
            overlay.push(handle.key(format!("hit:resize:{}:{edge:?}", card.id)).on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, _tx: &mut Tx| drag_handle(state, &id, edge, drag)));
        }
        overlay.extend(tools);
    }

    // `.nest-target` / `.nest-refused`: 2 px out; `.menu-highlight`: 4 px out.
    let outline = match &state.nest_mark {
        Some((host, ok)) if host == &card.id => Some((if *ok { theme.accent } else { theme.status_error }, 2.0)),
        _ => (state.menu.highlight.as_deref() == Some(card.id.as_str())).then_some((theme.accent, 4.0)),
    };
    let header = (!card.bare).then(|| Header { name: &card.name, desc: &card.desc, icon: &card.icon, ink, width: tile.w, expanded: open });
    let dragged = state.arrange.as_ref().is_some_and(|a| a.dragged.as_deref() == Some(card.id.as_str()));
    let id = card.id.clone();
    let head_id = card.id.clone();
    let folder_type = card.folder.then(|| card.widget_type.clone());
    let special = card.special();
    let bare = card.bare;
    card::view(state, card::Frame {
        id: &card.id,
        label: &card.name,
        theme,
        tint,
        strip: card.strip,
        clear: card.clear_flat && theme.material == Material::Flat,
        tile,
        frame,
        pose,
        grain: &state.grain,
        grain_size,
        header,
        content,
        nest,
        outline,
        overlay,
        dragged,
        editing: arranging,
        backdrop: (theme.blur > 0.0 && state.backdrop_for.is_some()).then_some((&state.backdrop, state.canvas)),
        // A module's card opens from anywhere on it, a folder from its header (`clickTarget`).
        // A morph can always be taken back: a press on a closing card opens it again from
        // where it is (`r[motion.reversal]`).
        // In arrange mode a card moves from its header, a bare card from anywhere on it
        // (`unidtk-core.js`: the header, or the clock's, player's, glyph bar's or Telegram's
        // own container), so a press inside a header card's body stays its own.
        window: Box::new(move |window: El| {
            if arranging {
                if !bare {
                    return window;
                }
                let id = id.clone();
                return window.on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, tx: &mut Tx| drag_card(state, &id, drag, tx));
            }
            let mut window = window;
            if let Some(folder) = folder_type {
                // A folder takes files dropped from Explorer, open or not (`tauri://drag-drop`).
                window = window.on_drop_files(move |state: &mut Host, files: &[std::path::PathBuf], _tx: &mut Tx| {
                    let targets: Vec<String> = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
                    log_frontend(state, &format!("folder drop: {} path(s) on {folder}", targets.len()));
                    crate::menu_folders::describe_and_add(state, folder.clone(), targets);
                });
            }
            if special {
                window = window.on_press_tx(card::toggle(&id));
            } else {
                // A press inside a card is the card's, never the desk's below it.
                window = window.on_press(|_: &mut Host| {});
            }
            window
        }),
        head: Box::new(move |head: El| {
            if arranging {
                return head.on_drag(move |state: &mut Host, drag: &mut rune::el::Drag, tx: &mut Tx| drag_card(state, &head_id, drag, tx));
            }
            if !special { head.on_press_tx(card::toggle(&head_id)) } else { head }
        }),
    })
}

/// The opened header card's body: under the header, `BODY_W` wide, scrolling. While the
/// card is still opening (`opening` names it), a press on the body does not reach what grows
/// in it: it folds the card back from where it is, so an open can always be taken back.
fn body_region(body: El, frame: Placement, opening: Option<String>) -> El {
    let region = el().absolute(BODY_X, BODY_Y).w(BODY_W).h((frame.h - BODY_Y - 17.0).max(0.0)).scroll(rune::ScrollSpec { stick_bottom: false }).child(body);
    let mut out = el().absolute(0.0, 0.0).size(frame.w, frame.h).child(region);
    if let Some(id) = opening {
        out = out.child(el().key("opening-body").absolute(BODY_X, BODY_Y).w(BODY_W).h((frame.h - BODY_Y).max(0.0)).on_press_tx(card::toggle(&id)));
    }
    out
}

pub(crate) fn view(state: &Host, cx: &mut rune::runtime::Cx<Host>) -> El {
    let mut root = el().w_full().h_full();
    if !matches!(state.startup_data, StartupData::Released) || state.layout.is_none() {
        return root;
    }
    let theme = state.theme();
    // `.menu-container :focus-visible { outline: 2px solid var(--accent); outline-offset: 2px }`,
    // for every control the host draws, instead of Rune's 3 px system ring.
    cx.focus_ring(2.0, 2.0, theme.accent);
    let look = Look::panel(&theme);
    let arranging = state.arrange.is_some();
    // Every child of the desk is keyed: Rune knows an unkeyed one by its place, and a child
    // added in front (the grid) or a card moved to the top would shift the rest.
    //
    // Under everything, the bare desk inside the window region: a shadow's reach, the part of a
    // growing surface's box the clip has not reached yet. A press there is a press on the desk
    // (`pointer_outside`), as one outside the region is through the kernel; a surface takes
    // the presses inside itself.
    root = root.child(el().key("desk").absolute(0.0, 0.0).size(state.canvas.0, state.canvas.1)
        .on_press_tx(|state: &mut Host, tx: &mut Tx| state.pointer_outside(tx)));
    if arranging {
        root = root.child(arrange::grid(&Look { fg: theme.fg, ..look.clone() }, state.desk_area()).key("arrange-grid"));
    }

    // Widgets switched off (`config.hidden`) or nested are not drawn on the desk; their records
    // stay. The surfaces draw above the cards, the one opened last on top.
    let Some(desk_now) = state.desk() else { return root };
    let tiles = state.card_tiles(&desk_now);
    let now = Instant::now();
    let grain_size = state.grain_size();
    let mut resting = Vec::new();
    let mut surfaces: Vec<(usize, El)> = Vec::new();
    for (card, tile) in desk_now.cards.iter().zip(tiles) {
        let element = card_element(state, &desk_now, card, tile, &theme, &look, now, grain_size);
        match state.morphs.iter().position(|m| m.id == card.id) {
            Some(order) => surfaces.push((order, element)),
            None => resting.push(element),
        }
    }
    surfaces.sort_by_key(|(order, _)| *order);
    root = root.children(resting).children(surfaces.into_iter().map(|(_, element)| element));
    if state.settling_until.is_some_and(|until| now < until) {
        // `body.arrange-settling`: for one motion step after arrange mode the desk takes no
        // press, so the second click of a double click on "Done" lands on nothing.
        root = root.child(el().key("hit:arrange-settling").absolute(0.0, 0.0).size(state.canvas.0, state.canvas.1).on_press(|_: &mut Host| {}));
    }
    if desk_now.cards.is_empty() && !arranging {
        // `#deskHint`: every widget is off the desk.
        root = root.child(
            el().key("desk-hint").absolute(0.0, 0.0).w(state.canvas.0).h(state.canvas.1).items_center().justify_center()
                .child(text("Все виджеты скрыты — вернуть их можно в меню").font(look.font(13.0, 400.0)).color(theme.fg.alpha(0.55))),
        );
    }

    if let Some(opener) = menu::opener(state, &theme, state.canvas) {
        root = root.child(opener);
    } else if arranging {
        // Inert at 40 % while arranging (`body.edit-mode .topbar-actions`).
        let lines = [([3.0, 12.0], [21.0, 12.0]), ([3.0, 6.0], [21.0, 6.0]), ([3.0, 18.0], [21.0, 18.0])];
        root = root.child(
            col().key("menu-button-inert").absolute(state.canvas.0 - 54.0, 18.0).size(36.0, 36.0).radius(11.0).opacity(0.4)
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
            el().key("debug-line").absolute(0.0, 0.0).w(canvas.0).h(canvas.1).items_center().justify_end().pb(70.0).child(
                el().max_w(width).px(14.0).py(10.0).radius(12.0).fill(rune::color::hex("#f87171").unwrap_or(Color::WHITE).alpha(0.95))
                    .shadow(12.0, 36.0, Color::BLACK.alpha(0.3))
                    .child(text(message.clone()).font(look.font(12.48, 600.0)).color(rune::color::hex("#111111").unwrap_or(Color::BLACK)).align(rune::el::Align::Center).wrap()),
            ),
        );
    }
    root.child(crate::toast::view(&look, &state.toasts, canvas, |state: &mut Host, tx: &mut Tx| {
        if let Some(action) = state.toasts.take_action() {
            action(state, tx);
        }
    }))
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
            .on_edit(|state: &mut Host| match state.arrange.as_mut() {
                Some(a) => &mut a.folder_name,
                None => &mut state.scratch,
            })
            .on_submit(|state: &mut Host, _tx: &mut Tx| create_arrange_folder(state));
        let icons: Vec<El> = arrange::BAR_FOLDER_ICONS.iter().map(|(key, label)| {
            let key = *key;
            let on = session.folder_icon == key;
            col().key(format!("hit:arrange-icon:{key}")).label(*label).size(32.0, 32.0).radius(9.0).items_center().justify_center()
                .border(1.0, if on { look.accent } else { look.elev.alpha(0.12) })
                .hover(look.elev.alpha(0.08))
                .child(svg_icon(state, crate::menu_desk::folder_icon(key), 18.0, look.fg.alpha(if on { 1.0 } else { 0.7 })))
                .on_press_tx(move |state: &mut Host, _tx: &mut Tx| {
                    if let Some(a) = state.arrange.as_mut() {
                        a.folder_icon = key;
                    }
                })
        }).collect();
        // `.arrange-folder-name`: 32 px, 4 x 10, a 16 % border on 20 % scrim, radius 9.
        children.push(
            col().gap(8.0)
                .child(el().w_full().min_h(32.0).px(10.0).py(4.0).radius(9.0).border(1.0, look.elev.alpha(0.16)).border_layout().fill(look.scrim.alpha(0.2)).items_center().child(name))
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

/// `.widget-add-popover`: beside the island, on the side facing the desk, 250 px wide,
/// centred on the island (`top: 50%` of the bar, `translate(.., -50%)`) and 11 px off its
/// border (`calc(100% + 12px)` from the padding box); it fades in and out 8 px on the hover
/// token, at most 420 px tall and 144 px short of the window.
fn add_popover(state: &Host, look: &Look, side_right: bool) -> El {
    let layout = state.layout.as_ref();
    let mut list = col().gap(4.0);
    for module in crate::catalog::desk_modules() {
        let singleton = module.meta["singleton"] == true;
        let placed = layout.is_some_and(|l| !l.is_hidden(&module.id) && (l.config["positions"].get(&module.id).is_some() || l.config["instances"].as_array().is_some_and(|i| i.iter().any(|x| x["widgetType"] == module.id.as_str()))));
        let disabled = singleton && placed;
        let id = module.id.clone();
        let mut option = row_el(format!("hit:add:{id}"))
            .hover(look.elev.alpha(0.07))
            .opacity(if disabled { 0.5 } else { 1.0 })
            .child(col().size(30.0, 30.0).radius(8.0).fill(look.elev.alpha(0.06)).items_center().justify_center().child(svg_icon(state, &module.icon, 18.0, look.fg.alpha(0.8))))
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
    use crate::motion::{Token, STANDARD};
    let side = if side_right { rune::Side::Left } else { rune::Side::Right };
    col().key("hit:add-popover").on_press(|_: &mut Host| {}).anchored("hit:arrange-bar", side, rune::Align::Center, 11.0)
        .w(250.0).max_h(420.0f32.min(state.canvas.1 - 144.0)).padding(8.0).radius(12.0)
        .fill(look.surface.alpha(0.94)).border(1.0, look.elev.alpha(0.12)).border_layout().shadow(18.0, 60.0, Color::BLACK.alpha(0.45))
        .transition(state.motion.presence(
            rune::Transition::offset(if side_right { 8.0 } else { -8.0 }, 0.0).combined(rune::Transition::opacity()),
            Token::Hover,
            STANDARD,
            Some(STANDARD),
        ))
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(list)
}

fn row_el(key: String) -> El {
    rune::el::row().key(key).w_full().min_h(48.0).px(8.0).py(7.0).gap(10.0).radius(8.0).items_center()
}

fn create_arrange_folder(state: &mut Host) {
    let Some(session) = state.arrange.as_mut() else { return };
    let name = session.folder_name.text.trim().to_string();
    let name = if name.is_empty() { "Новая папка".to_string() } else { name };
    let icon = crate::menu_desk::folder_icon(session.folder_icon);
    let id = format!("folder_{}", uuid_like());
    session.created_folders.push(id.clone());
    session.folder_form = false;
    session.folder_name = TextEdit::new("").max_chars(40);
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

/// Escape, in the page's order: the folder's menu and a rename, a widget's own popup, the
/// sheet over the menu, the menu (VPN steps back first), arrange mode, then the open card
/// (`collapseAllShellWidgets`). Ctrl+V pastes copied files or a link into the open folder.
pub(crate) fn on_key(state: &mut Host, event: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if event.is_command('v') && !event.mods.shift && !event.mods.alt && state.rename.is_none() && !state.menu.open {
        let folder = state.open.as_ref().and_then(|open| state.desk()?.card(open).filter(|c| c.folder).map(|c| c.widget_type.clone()));
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
    for id in state.visible_widgets() {
        let mut closed = false;
        state.with_dyn(&id, tx, |widget, act| closed = widget.escape(act));
        if closed {
            return true;
        }
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

/// Runs at the start of every frame: the widgets' wakes, the morphs (a frame each while one
/// runs, then none), the toast's end, the menu's slide-out and the arrange settle. An idle
/// desk wakes only when a widget asks (the clock once a second or once a minute).
pub(crate) fn poll(state: &mut Host, _: f64, tx: &mut Tx) -> bool {
    if matches!(state.startup_data, StartupData::AwaitingWindow) { return false; }
    if let StartupData::Waiting(deadline) = state.startup_data {
        let complete = state.layout.is_some()
            && state.menu.disabled_modules.ready().is_some()
            && state.menu.folders.ready().is_some();
        if !complete {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                tx.wake_after(remaining);
                return false;
            }
            crate::desktop_host::note("startup snapshot timed out; using available desk data");
            if state.layout.is_none() {
                state.layout = Some(Layout::empty());
                state.refresh_grain();
                state.refresh_backdrop();
            }
        }
        state.startup_data = StartupData::Released;
        tx.instant();
    }
    if !state.show_requested {
        let hwnd = state.inspector.hwnd.load(Ordering::Acquire);
        if crate::desktop_host::window_is_attached(hwnd) {
            if let Some(ui) = state.ui.get() {
                state.show_requested = true;
                ui.show();
                crate::desktop_host::note("desktop reveal requested after attachment and startup data");
            }
        }
    }
    let Some(layout) = &state.layout else { return false };
    let mut changed = false;
    // The widgets follow the desk: made, synced or dropped when the layout or the
    // switched-off modules change.
    let disabled: Vec<String> = state.menu.disabled_modules.ready().map(|set| {
        let mut list: Vec<String> = set.iter().cloned().collect();
        list.sort();
        list
    }).unwrap_or_default();
    let signature = (layout.instance.clone(), layout.rev, disabled);
    if state.synced.as_ref() != Some(&signature) {
        state.synced = Some(signature);
        let made = state.sync_widgets();
        state.replay_events(&made, tx);
        state.update_visibility(tx);
        changed = true;
    }

    state.time = LocalTime::now();
    let instant = Instant::now();
    let mut next = f64::INFINITY;
    let at = |instant: Instant, until: Instant, tx: &Tx| tx.clock() + until.saturating_duration_since(instant).as_secs_f64();

    // Each widget's wake: a tick when it is due, then its next wake, the earliest kept.
    let ids: Vec<String> = state.widgets.keys().cloned().collect();
    for id in ids {
        let due = state.widgets.get(&id).and_then(|slot| slot.due);
        if due.is_some_and(|due| instant >= due) {
            let time = state.time;
            let mut redraw = false;
            state.with_dyn(&id, tx, |widget, act| redraw = widget.tick(&time, act));
            changed |= redraw;
            if let Some(slot) = state.widgets.get_mut(&id) {
                slot.due = None;
            }
        }
        if let Some(slot) = state.widgets.get_mut(&id) {
            // Never sooner than a frame: a widget that asks for "now" does not spin the desk.
            let wanted = slot.widget.wake(&state.time).map(|seconds| instant + Duration::from_secs_f64(seconds.max(0.004)));
            slot.due = match (slot.due, wanted) {
                (Some(old), Some(new)) => Some(old.min(new)),
                (old, new) => new.or(old),
            };
            if let Some(due) = slot.due {
                next = next.min(at(instant, due, tx));
            }
        }
    }

    // The morphs: one frame after another while any runs, the closed ones dropped at their end.
    // The menu's sheet is one of them.
    let sheet_running = menu::sheet_frame_due(state, instant);
    let running = sheet_running || state.morphs.iter().any(|m| !m.pose(instant).done);
    let before = state.morphs.len();
    state.morphs.retain(|m| m.is_open() || !m.pose(instant).done);
    if running {
        tx.instant();
        next = tx.clock();
        changed = true;
    } else if state.morphing || state.morphs.len() != before {
        // The last frame: a closed card goes from its surface's box back to its tile, which a
        // spring would stretch; the clip already stands at the tile.
        tx.instant();
        changed = true;
    }
    state.morphing = running;

    let had_toast = state.toasts.current.is_some();
    if let Some(until) = state.toasts.expire(instant) {
        next = next.min(at(instant, until, tx));
    }
    changed |= had_toast && state.toasts.current.is_none();
    let was_shown = state.menu.shown;
    if let Some(until) = menu::settle(state, instant) {
        next = next.min(at(instant, until, tx));
    }
    changed |= was_shown && !state.menu.shown;
    if let Some(until) = state.settling_until {
        if instant >= until {
            state.settling_until = None;
            changed = true;
        } else {
            next = next.min(at(instant, until, tx));
        }
    }
    if let Some((_, until)) = &state.debug {
        if instant >= *until {
            state.debug = None;
            changed = true;
        } else {
            next = next.min(at(instant, *until, tx));
        }
    }
    state.inspector.wants_keyboard.store(state.wants_keyboard(), Ordering::Release);
    let wanted: Vec<String> = state.wanted_icons.borrow_mut().drain(..).collect();
    for target in wanted {
        request_icon(state, target);
    }
    if next.is_finite() {
        tx.wake_at(next);
    }
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

    #[test]
    fn a_face_is_a_position_field_and_anything_else_a_setting() {
        assert!(super::position_fields(&serde_json::json!({ "op": "x", "id": "c", "version": "rings" })));
        assert!(!super::position_fields(&serde_json::json!({ "op": "x", "id": "t", "expanded": true })));
    }

    #[test]
    fn small_wheel_travel_adds_up_to_a_notch_and_keeps_the_rest() {
        assert_eq!(super::take_notches(0.0, 120.0), (1, 0.0));
        assert_eq!(super::take_notches(0.0, -240.0), (-2, 0.0));
        // A touchpad: 30 + 30 + 30 is not yet a notch, the fourth makes one.
        let (n, carry) = super::take_notches(0.0, 30.0);
        assert_eq!((n, carry), (0, 30.0));
        let (n, carry) = super::take_notches(carry, 90.0);
        assert_eq!((n, carry), (1, 0.0));
        // Turning back cancels what is carried instead of stepping.
        assert_eq!(super::take_notches(100.0, -30.0), (0, 70.0));
    }
}
