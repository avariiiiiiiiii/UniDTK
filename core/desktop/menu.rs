//! The settings menu (`r[kernel.menu-in-core]`), as `core/ui/menu.js` and `menu.css` build
//! it: the side panel on the right of the desk with "Настройки", the "Стол" and "Вид" tabs,
//! the module settings views and the footer's "Выйти из UniDTK"; the
//! three-line button at the top right that opens it; the sheets that open over it for one
//! widget or one folder. The module's own native crate draws its settings.
//!
//! The menu talks to the kernel through [`MenuHost`]: writes to the desk go as `desk_apply`
//! operations (`r[desk.position-merge]`). The kernel sends command replies back to the UI thread.

use std::collections::HashSet;

use rune::el::{col, el, row, text, El};
use rune::{Color, Shadow, TextEdit, Tx};
use serde_json::Value;

use crate::controls::{self, ButtonKind, Look};
use crate::desk::Layout;
use crate::motion::{Token, EXPO_OUT, STANDARD, SURFACE_EXIT};
use crate::settings;
use crate::theme::{mix_oklch, scaled, Material, Theme};
use crate::toast::{Action, Toasts};

/// What the menu needs from the host it lives in.
pub trait MenuHost: Sized + 'static {
    fn menu(&mut self) -> &mut Menu;
    fn menu_ref(&self) -> &Menu;
    fn layout(&self) -> Option<&Layout>;
    fn toasts(&mut self) -> &mut Toasts<Self>;
    /// The theme record the desk shows now: the draft while one is being edited.
    fn theme_record(&self) -> Value;
    /// Shows `record` without storing it (a slider being dragged, `setLiveToken`).
    fn preview_theme(&mut self, record: Value);
    /// `desk_apply` with these operations; a failure is reported in a toast.
    fn apply(&mut self, ops: Vec<Value>, tx: &mut Tx);
    /// Any kernel command; `then` runs on the UI thread with the answer.
    fn call(&mut self, cmd: &str, args: Value, then: Box<dyn FnOnce(&mut Self, Result<Value, String>, &mut Tx) + Send>);
    /// Enters arrange mode; the menu reopens when it ends.
    fn enter_arrange(&mut self, tx: &mut Tx);
    /// SVG markup as an image `size` points square, tinted where it says `currentColor`.
    fn svg(&self, markup: &str, size: f32, colour: Color) -> Option<rune::Pixels>;
    /// The desk draws module `id` (its native half is registered): the menu offers only what
    /// changes something on screen.
    fn draws(&self, id: &str) -> bool;
    fn settings_factory(&self, id: &str) -> Option<settings::Factory>;
    /// The jelly grain the desk's cards wear, and the size it covers in points.
    fn grain(&self) -> (&rune::Pixels, (f32, f32));
    /// The Windows icon of a shortcut target, once the kernel has sent it; asking for one
    /// the host does not have queues the request.
    fn shortcut_icon(&self, target: &str) -> Option<rune::Pixels>;
    /// The system colour dialog, on a worker; `then` runs on the UI thread with the choice.
    fn pick_color(&mut self, initial: [u8; 3], then: Box<dyn FnOnce(&mut Self, [u8; 3], &mut Tx) + Send>);
    fn canvas(&self) -> (f32, f32);
    /// The widgets' canvas (`#widgets`), which records are relative to.
    fn desk_size(&self) -> (f32, f32);
    fn arranging(&self) -> bool;
}

pub(crate) enum View {
    Desk,
    Look,
    ModuleSettings(settings::Session),
}

/// A value the menu asked the kernel for.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded<T> {
    Loading,
    Ready(T),
    Failed(String),
    /// A reload failed after a load had worked: the last value, and why the reload did not.
    /// The desk keeps drawing it (a busy kernel used to empty the desk of its folders), and
    /// nothing writes from it: an edit needs [`Loaded::Ready`].
    Stale(T, String),
}

impl<T> Loaded<T> {
    /// The value an edit may start from: only a current one.
    pub fn ready(&self) -> Option<&T> {
        if let Loaded::Ready(value) = self { Some(value) } else { None }
    }

    /// The value the desk draws: the current one, or the last one while a reload fails.
    pub fn shown(&self) -> Option<&T> {
        match self {
            Loaded::Ready(value) | Loaded::Stale(value, _) => Some(value),
            Loaded::Loading | Loaded::Failed(_) => None,
        }
    }

    /// What a failed load or reload leaves: the last value kept as stale, if there was one.
    pub fn failed(self, error: String) -> Self {
        match self {
            Loaded::Ready(value) | Loaded::Stale(value, _) => Loaded::Stale(value, error),
            Loaded::Loading | Loaded::Failed(_) => Loaded::Failed(error),
        }
    }

    /// A load failed, or only a stale value is left: it is worth asking again.
    pub fn needs_reload(&self) -> bool {
        matches!(self, Loaded::Failed(_) | Loaded::Stale(..))
    }
}

/// The sheet over the menu: one widget's or one folder's settings (`popup.js`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sheet {
    Widget(String),
    Folder(String),
}

pub struct Menu {
    /// The panel is in the tree: open, or sliding out.
    pub shown: bool,
    pub open: bool,
    /// When the slide-out ends and the panel leaves the tree (the page's 700 ms fallback is
    /// the transition's own end here).
    pub closing_until: Option<std::time::Instant>,
    pub(crate) view: View,
    settings_generation: u64,
    /// The sheet open, or `None` while it folds back into its row.
    pub sheet: Option<Sheet>,
    /// The sheet drawn: the open one, or the one folding, with its morph out of the row that
    /// opened it (`openSurface({ origin })`, `r[shell.one-morph]`).
    pub sheet_shown: Option<(Sheet, crate::surface::Morph)>,
    /// Where each "Настроить…" row and the panel lie in the window, for the sheet's origin.
    pub rects: std::collections::HashMap<String, [f32; 4]>,
    pub fine_open: bool,
    pub preset_name: TextEdit,
    /// `previousColors`: what "Перемешать цвета" replaced, for its undo.
    pub previous_colors: Option<Value>,
    /// Commands in flight, by control key (`runOnce`): their controls are disabled.
    pub busy: HashSet<String>,
    pub disabled_modules: Loaded<HashSet<String>>,
    pub folders: Loaded<Vec<Value>>,
    pub missing: HashSet<String>,
    pub creating_folder: bool,
    pub new_folder_name: TextEdit,
    pub new_folder_icon: &'static str,
    /// The folder whose "Из Пуска" list is open, the list, and its search.
    pub start_for: Option<String>,
    pub start_list: Loaded<Vec<Value>>,
    pub start_search: TextEdit,
    pub folder_names: std::collections::HashMap<String, TextEdit>,
    pub item_names: std::collections::HashMap<String, TextEdit>,
    /// The widget row the pointer is over: its card is highlighted on the desk.
    pub highlight: Option<String>,
    /// The body is taller than the panel: it keeps room for the scrollbar
    /// (`scrollbar-width: thin`, 11 px in Chromium), as the page's does. Set from the scroll
    /// view's own report, so it shows from the frame after the content outgrows the panel.
    pub overflow: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Default for Menu {
    fn default() -> Self {
        Menu {
            shown: false,
            open: false,
            closing_until: None,
            view: View::Desk,
            settings_generation: 0,
            sheet: None,
            sheet_shown: None,
            rects: std::collections::HashMap::new(),
            fine_open: false,
            preset_name: TextEdit::new("").max_chars(32),
            previous_colors: None,
            busy: HashSet::new(),
            disabled_modules: Loaded::Loading,
            folders: Loaded::Loading,
            missing: HashSet::new(),
            creating_folder: false,
            new_folder_name: TextEdit::new("").max_chars(40),
            new_folder_icon: "folder",
            start_for: None,
            start_list: Loaded::Loading,
            start_search: TextEdit::new(""),
            folder_names: Default::default(),
            item_names: Default::default(),
            highlight: None,
            overflow: Default::default(),
        }
    }
}

impl Menu {
    pub(crate) fn active_settings(&mut self, generation: u64) -> Option<&mut dyn settings::Settings> {
        match &mut self.view {
            View::ModuleSettings(session) if self.open && session.generation == generation => {
                Some(session.settings.as_mut())
            }
            _ => None,
        }
    }
}

/// Opens the menu, or closes it when open (`openMenu` toggles). Arrange mode keeps it shut.
pub fn toggle<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    if state.arranging() {
        return;
    }
    if state.menu_ref().open {
        close(state, tx);
        return;
    }
    let (view, tab) = if matches!(state.menu_ref().view, View::Look) {
        (View::Look, "hit:tab:look")
    } else {
        (View::Desk, "hit:tab:desk")
    };
    let menu = state.menu();
    menu.shown = true;
    menu.open = true;
    menu.closing_until = None;
    show_view(state, view, tx);
    // `panel.querySelector(#tab-…).focus()`.
    tx.focus(tab, false);
}

/// `closeMenu`: slides out, then leaves the tree.
pub fn close<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    if !state.menu_ref().open {
        return;
    }
    // The theme shown, a draft being edited included: dragging the animation length changes
    // the next close, not only the one after the slider is let go.
    let duration = Theme::normalize(&state.theme_record()).motion().duration(Token::Motion);
    let menu = state.menu();
    menu.open = false;
    // The sheet stays drawn and slides out with the panel (`settle` drops it).
    menu.sheet = None;
    menu.highlight = None;
    menu.closing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs_f32(duration));
    crate::surface::keep_frames(tx);
    // `opener.focus()` (`restoreFocus`); in arrange mode the opener is gone and nothing happens.
    tx.focus("hit:menu-button", false);
}

/// `handleMenuEscape`: close the sheet first, then step back from module settings to "Стол"
/// or close the menu. Return false when the menu is already closed.
pub fn escape<S: MenuHost>(state: &mut S, tx: &mut Tx) -> bool {
    if !state.menu_ref().open {
        return false;
    }
    if state.menu_ref().sheet.is_some() {
        close_sheet(state, tx);
    } else if matches!(state.menu_ref().view, View::ModuleSettings(_)) {
        back_to_desk(state, tx, true);
    } else {
        close(state, tx);
        // From the keyboard, so the opener shows its ring (`:focus-visible`).
        tx.focus("hit:menu-button", true);
    }
    true
}

/// The poll's part: the panel leaves the tree when its slide-out has run.
pub fn settle<S: MenuHost>(state: &mut S, now: std::time::Instant) -> Option<std::time::Instant> {
    let until = state.menu_ref().closing_until?;
    if now >= until {
        let menu = state.menu();
        menu.shown = false;
        menu.closing_until = None;
        // The sheet slid out with the panel.
        menu.sheet_shown = None;
        if matches!(menu.view, View::ModuleSettings(_)) {
            menu.view = View::Desk;
        }
        return None;
    }
    Some(until)
}

/// Opens `sheet` grown out of the element keyed `origin` (its row's "Настроить…"), or out of
/// the middle of the panel when that is not on screen. The sheet's first control takes focus
/// (`onOpened`).
pub fn open_sheet<S: MenuHost>(state: &mut S, sheet: Sheet, origin: &str, tx: &mut Tx) {
    let theme = Theme::normalize(&state.theme_record());
    let motion = theme.motion();
    let menu = state.menu();
    let now = std::time::Instant::now();
    let panel = menu.rects.get("hit:menu").copied().unwrap_or([0.0, 0.0, 420.0, 600.0]);
    let frame = crate::desk::Placement { x: 0.0, y: 0.0, w: panel[2], h: panel[3] };
    let tile = match menu.rects.get(origin) {
        Some(r) => crate::desk::Placement { x: r[0] - panel[0], y: r[1] - panel[1], w: r[2], h: r[3] },
        None => crate::desk::Placement { x: panel[2] * 0.25, y: panel[3] * 0.25, w: panel[2] * 0.5, h: panel[3] * 0.5 },
    };
    let morph = match menu.sheet_shown.take() {
        // The same sheet folding back turns round from where it is.
        Some((shown, mut morph)) if shown == sheet => {
            morph.reopen(now);
            morph
        }
        _ => crate::surface::Morph::open("menu-sheet", &motion, frame, tile, 10.0, theme.radius_surface, now),
    };
    menu.sheet = Some(sheet.clone());
    menu.sheet_shown = Some((sheet, morph));
    tx.instant();
    crate::surface::keep_frames(tx);
    tx.focus("hit:menu-sheet", false);
}

/// Notes where `key` lies, for a sheet that grows out of it; nothing on screen changes.
pub fn remember_rect<S: MenuHost>(state: &mut S, key: &str, rect: [f32; 4], tx: &mut Tx) {
    state.menu().rects.insert(key.to_string(), rect);
    tx.live_only();
}

/// Folds the sheet back into its row (`closeSurface`).
pub fn close_sheet<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    let menu = state.menu();
    menu.sheet = None;
    if let Some((_, morph)) = menu.sheet_shown.as_mut() {
        morph.close(std::time::Instant::now());
    }
    crate::surface::keep_frames(tx);
}

/// The poll's part for the sheet: true while its morph runs or has just ended, so the desk
/// draws the next frame; a folded sheet leaves the tree.
pub fn sheet_frame_due<S: MenuHost>(state: &mut S, now: std::time::Instant) -> bool {
    let menu = state.menu();
    let Some((_, morph)) = menu.sheet_shown.as_ref() else { return false };
    let pose = morph.pose(now);
    if pose.done && !morph.is_open() {
        menu.sheet_shown = None;
        return true;
    }
    !pose.done
}

/// `showView`: the desk reads its lists again each time it is shown.
pub fn show_view<S: MenuHost>(state: &mut S, view: View, tx: &mut Tx) {
    let desk = matches!(view, View::Desk);
    state.menu().view = view;
    if desk {
        crate::menu_desk::load(state, tx);
    }
}

pub fn open_module_settings<S: MenuHost>(state: &mut S, id: &str, tx: &mut Tx) {
    let menu = state.menu_ref();
    if !menu.open || !matches!(&menu.sheet, Some(Sheet::Widget(shown)) if shown == id) {
        return;
    }
    let Some(make) = state.settings_factory(id) else { return };
    close_sheet(state, tx);
    let menu = state.menu();
    menu.settings_generation += 1;
    let generation = menu.settings_generation;
    menu.view = View::ModuleSettings(settings::Session { generation, settings: make() });
    settings::run(state, generation, tx, |pane, act| pane.load(act));
    tx.focus("hit:module-settings-back", false);
}

fn back_to_desk<S: MenuHost>(state: &mut S, tx: &mut Tx, keyboard: bool) {
    if state.menu_ref().open && matches!(state.menu_ref().view, View::ModuleSettings(_)) {
        show_view(state, View::Desk, tx);
        tx.focus("hit:tab:desk", keyboard);
    }
}

fn module_settings_pane<S: MenuHost>(session: &settings::Session, look: &Look) -> El {
    let chevron = [([15.0, 18.0], [9.0, 12.0]), ([9.0, 12.0], [15.0, 6.0])];
    let back = row().key("hit:module-settings-back").label("Стол").focusable(true)
        .min_h(32.0).mt(4.0).pl(4.0).pr(8.0).py(4.0).gap(4.0).radius(9.0).items_center()
        .hover(look.elev.alpha(0.1))
        .child(el().size(14.0, 14.0).shape(controls::glyph(&chevron, look.fg.alpha(0.85), 2.0)))
        .child(text("Стол").font(look.font(13.0, 400.0)).color(look.fg.alpha(0.85)).single_line())
        .on_key(|state: &mut S, event: &rune::KeyEvent, tx: &mut Tx| {
            if event.key == rune::Key::Enter {
                back_to_desk(state, tx, true);
                return true;
            }
            false
        })
        .on_press_tx(|state: &mut S, tx: &mut Tx| back_to_desk(state, tx, false));
    let view = settings::View::new(look, session.generation);
    col().w_full().child(row().child(back)).child(session.settings.pane(&view))
}

/// `runOnce`: the control stays disabled while its command is in flight; an error becomes the
/// toast "Не получилось: …".
pub fn run_once<S: MenuHost>(
    state: &mut S,
    key: &str,
    cmd: &str,
    args: Value,
    then: impl FnOnce(&mut S, Value, &mut Tx) + Send + 'static,
) {
    run_once_as(state, key, cmd, args, "Не получилось", then);
}

/// `runOnce` whose error toast starts with `failed` ("Не переименовано", ...).
pub fn run_once_as<S: MenuHost>(
    state: &mut S,
    key: &str,
    cmd: &str,
    args: Value,
    failed: &'static str,
    then: impl FnOnce(&mut S, Value, &mut Tx) + Send + 'static,
) {
    if !state.menu().busy.insert(key.to_string()) {
        return;
    }
    let key = key.to_string();
    state.call(
        cmd,
        args,
        Box::new(move |state: &mut S, result, tx: &mut Tx| {
            state.menu().busy.remove(&key);
            match result {
                Ok(value) => then(state, value, tx),
                Err(error) => error_toast(state, &format!("{failed}: {error}")),
            }
        }),
    );
}

pub fn error_toast<S: MenuHost>(state: &mut S, message: &str) {
    state.toasts().show(message.to_string(), true, None);
}

pub fn undo_toast<S: MenuHost>(state: &mut S, message: String, undo: Action<S>) {
    state.toasts().undo(message, undo);
}

/// The panel's paint in each material (`.menu-container`, and the jelly panel recipe), `w` x
/// `h` points: the sheen is `120% 95%` of the panel itself, and a jelly panel wears the grain
/// under its content (`::before`).
fn panel_paint<S: MenuHost>(state: &S, panel: El, theme: &Theme, look: &Look, (w, h): (f32, f32)) -> El {
    let hi = Color::WHITE;
    match theme.material {
        Material::Flat => {
            let mut panel = panel.fill(look.surface.alpha(0.97)).border(1.0, look.elev.alpha(0.12)).border_layout();
            if theme.glass_grain > 0.0 {
                // Grain is Sea Glass's on the page; "Зерно" reaches the flat panel here.
                let (grain, grain_size) = state.grain();
                panel = panel.child(el().absolute(0.0, 0.0).size(w, h).clip_rounded(theme.radius_surface).child(el().size(grain_size.0, grain_size.1).pixels(grain)));
            }
            surface_shadow(panel, theme, look)
        }
        Material::Jelly => {
            let tint = theme.tint("panel", "calm", "lilac");
            let offset = theme.shadow_offset;
            let (grain, grain_size) = state.grain();
            panel
                .fill(mix_oklch(tint, theme.surface_tint, hi))
                .radial_gradient([0.18, 0.08], [1.2 * w, 0.95 * h], [(0.0, hi.alpha(theme.glass_sheen * 0.7)), (0.58, hi.alpha(0.0))])
                .child(el().absolute(0.0, 0.0).size(w, h).clip_rounded(theme.radius_surface).child(el().size(grain_size.0, grain_size.1).pixels(grain)))
                .border(1.0, hi.alpha(0.75)).border_layout()
                .box_shadow(Shadow { y: 1.5, color: hi.alpha(theme.glass_rim * 0.9), inset: true, ..Shadow::default() })
                .box_shadow(Shadow { y: offset * 2.0, blur: offset * 5.0, color: scaled(tint, 0.4, 1.0, 0.02, theme.glass_shadow * 0.4), ..Shadow::default() })
        }
    }
}

/// `--theme-surface-shadow`: the computed one when the theme has a glass shadow, else the
/// deep drop and the 8 % inner rim.
pub fn surface_shadow(element: El, theme: &Theme, look: &Look) -> El {
    if theme.glass_shadow > 0.0 {
        element
            .box_shadow(Shadow { y: 1.0, color: look.elev.alpha(theme.glass_rim * 0.4), inset: true, ..Shadow::default() })
            .box_shadow(Shadow { y: 24.0, blur: 70.0, spread: -6.0, color: look.scrim.alpha(theme.glass_shadow * 0.65), ..Shadow::default() })
            .box_shadow(Shadow { y: 12.0, blur: 30.0, spread: -4.0, color: theme.bg_base.alpha(theme.glass_shadow * 0.35), ..Shadow::default() })
    } else {
        element
            .box_shadow(Shadow { y: 28.0, blur: 90.0, color: Color::BLACK.alpha(0.8), ..Shadow::default() })
            .box_shadow(Shadow { y: 1.0, color: look.elev.alpha(0.08), inset: true, ..Shadow::default() })
    }
}

/// The three-line button of the top bar (`#openMenu`, `.action-btn`): 36 px, radius 11, at
/// 18 px from the top right. On hover (220 ms `ease`) its fill turns to 10 % elevated, its
/// border to 18 %, its glyph from 45 % to 85 % ink, and it lifts a point; a press shrinks it
/// to 92 %. Hidden while arranging.
pub fn opener<S: MenuHost>(state: &S, theme: &Theme, canvas: (f32, f32)) -> Option<El> {
    if state.arranging() {
        return None;
    }
    let motion = theme.motion();
    let ease = motion.tween(motion.duration(Token::Transient), crate::motion::EASE);
    let lines = [([3.0, 12.0], [21.0, 12.0]), ([3.0, 6.0], [21.0, 6.0]), ([3.0, 18.0], [21.0, 18.0])];
    Some(
        col().key("hit:menu-button").label("Меню UniDTK")
            .absolute(canvas.0 - 18.0 - 36.0, 18.0).size(36.0, 36.0).radius(11.0)
            .fill(theme.bg_base.alpha(0.78)).border(1.0, theme.bg_elevated.alpha(0.08))
            .hover_fill(theme.bg_elevated.alpha(0.1)).hover_border(theme.bg_elevated.alpha(0.18)).hover_lift(0.0, -1.0).hover_motion(ease, ease)
            .press_scale(0.92)
            .items_center().justify_center()
            .child(
                el().size(18.0, 18.0)
                    .child(el().absolute(0.0, 0.0).size(18.0, 18.0).shape(controls::glyph(&lines, theme.fg.alpha(0.45), 2.0)))
                    .child(el().absolute(0.0, 0.0).size(18.0, 18.0).shape(controls::glyph(&lines, theme.fg.alpha(0.85), 2.0)).group_opacity(0.0, 1.0)),
            )
            .on_press_tx(|state: &mut S, tx: &mut Tx| toggle(state, tx)),
    )
}

/// The panel: right, top and bottom 12 px, `min(420px, 100vw - 24px)` wide, sliding in from
/// past the right edge on `expoOut` and out on `surfaceExit`, its opacity on the transient
/// token.
pub fn panel<S: MenuHost>(state: &S, theme: &Theme, canvas: (f32, f32)) -> Option<El> {
    if !state.menu_ref().shown {
        return None;
    }
    let look = Look { busy: std::rc::Rc::new(state.menu_ref().busy.clone()), ..Look::panel(theme) };
    let open = state.menu_ref().open;
    let width = (canvas.0 - 24.0).min(420.0);
    let motion = theme.motion();
    let slide = motion.tween(motion.duration(Token::Motion), if open { EXPO_OUT } else { SURFACE_EXIT });
    let fade = motion.tween(motion.duration(Token::Transient), STANDARD);
    let view = &state.menu_ref().view;
    let module_settings = matches!(view, View::ModuleSettings(_));

    let close_cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
    let header = row().w_full().justify_between().items_center().pl(20.0).pr(16.0).pt(16.0).pb(8.0)
        .child(text("Настройки").font(look.font(17.0, 600.0)).color(look.fg).single_line())
        .child(controls::icon_button(&look, "hit:menu-close".into(), "Закрыть (Esc)", &close_cross, 16.0, |state: &mut S, tx: &mut Tx| close(state, tx)));
    let tabs = (!module_settings).then(|| {
        el().w_full().px(16.0).pb(8.0).child(controls::tabs(
            &look,
            &[("desk", "Стол"), ("look", "Вид")],
            if matches!(view, View::Look) { "look" } else { "desk" },
            |state: &mut S, id: &str, tx: &mut Tx| show_view(state, if id == "look" { View::Look } else { View::Desk }, tx),
        ))
    });
    let pane = match view {
        View::Desk => crate::menu_desk::pane(state, &look),
        View::Look => crate::menu_look::pane(state, &look),
        View::ModuleSettings(session) => module_settings_pane::<S>(session, &look),
    };
    let overflow = state.menu_ref().overflow.clone();
    let gutter = if overflow.get() { 11.0 } else { 0.0 };
    let body_key = if module_settings { "module-settings-body" } else { "menu-body" };
    let body = col().key(body_key).w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).pl(16.0).pr(16.0 + gutter).pt(4.0).pb(16.0)
        .on_scroll_offset(move |at: rune::ScrollPos| overflow.set(at.max[1] > 0.5))
        .child(pane);
    let body = if module_settings && !open { body.inert() } else { body };
    let quit = controls::button(&look, "hit:menu-quit".into(), "Выйти из UniDTK", ButtonKind::Quiet, |state: &mut S, _tx: &mut Tx| {
        run_once(state, "menu-quit", "quit_app", serde_json::json!({}), |_, _, _| {});
    });
    let footer = el().w_full().px(16.0).pt(10.0).pb(14.0).border_sides([1.0, 0.0, 0.0, 0.0], look.elev.alpha(0.1)).child(row().child(quit));

    let mut panel = col().key("hit:menu").label("Настройки")
        .on_press(|_: &mut S| {})
        .on_layout_tx(|state: &mut S, rect: [f32; 4], tx: &mut Tx| remember_rect(state, "hit:menu", rect, tx))
        .absolute(canvas.0 - 12.0 - width, 12.0).w(width).h(canvas.1 - 24.0)
        .radius(theme.radius_surface)
        .offset(if open { 0.0 } else { width + 24.0 }, 0.0).offset_motion(slide)
        .opacity(if open { 1.0 } else { 0.0 }).opacity_motion(fade)
        .child(header);
    panel = panel_paint(state, panel, theme, &look, (width, canvas.1 - 24.0));
    if let Some(tabs) = tabs {
        panel = panel.child(tabs);
    }
    panel = panel.child(body).child(footer);
    if let Some((sheet, morph)) = state.menu_ref().sheet_shown.as_ref() {
        // Laid out over the whole panel from the first frame; the clip grows out of the row.
        let pose = morph.pose(std::time::Instant::now());
        let window = pose.clip.window(morph.frame);
        let height = canvas.1 - 24.0;
        panel = panel.child(
            el().absolute(window.x, window.y).size(window.w, window.h).clip_rounded(pose.clip.radius)
                .child(el().absolute(-window.x, -window.y).size(width, height).child(crate::menu_desk::sheet(state, &look, theme, sheet, width, height))),
        );
    }
    Some(panel)
}

/// `shortcutCount`: "1 ярлык", "2 ярлыка", "5 ярлыков".
pub fn shortcut_count(n: usize) -> String {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        format!("{n} ярлык")
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        format!("{n} ярлыка")
    } else {
        format!("{n} ярлыков")
    }
}

/// A sheet's frame (`.menu-sheet`): the panel's paint over the whole panel, a 15 px title and
/// a close button, its body scrolling.
pub fn sheet_frame<S: MenuHost>(state: &S, look: &Look, theme: &Theme, title: &str, width: f32, height: f32, body: El) -> El {
    let close_cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
    let frame = col().key("hit:menu-sheet").absolute(0.0, 0.0).w(width).h(height).radius(theme.radius_surface)
        .child(
            row().w_full().justify_between().items_center().pl(20.0).pr(16.0).pt(18.0).pb(8.0)
                .child(text(title.to_string()).font(look.font(15.0, 600.0)).color(look.fg).single_line())
                .child(controls::icon_button(look, "hit:sheet-close".into(), "Закрыть", &close_cross, 14.0, |state: &mut S, tx: &mut Tx| close_sheet(state, tx))),
        )
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).px(20.0).pb(20.0).child(body));
    panel_paint(state, frame, theme, look, (width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A busy kernel answered the folder reload with a timeout, and the desk drew no
    // folders until the menu was opened again.
    #[test]
    fn a_failed_reload_keeps_the_last_list_for_the_desk_but_not_for_edits() {
        let stale = Loaded::Ready(vec![1, 2]).failed("timeout".into());
        assert_eq!(stale.shown(), Some(&vec![1, 2]), "the desk keeps drawing it");
        assert_eq!(stale.ready(), None, "no edit starts from a stale list");
        assert!(stale.needs_reload());
        let again = stale.failed("timeout again".into());
        assert_eq!(again, Loaded::Stale(vec![1, 2], "timeout again".into()));
        assert_eq!(Loaded::<Vec<u8>>::Loading.failed("x".into()), Loaded::Failed("x".into()));
    }

    #[test]
    fn shortcut_counts_decline_as_russian_does() {
        assert_eq!(shortcut_count(1), "1 ярлык");
        assert_eq!(shortcut_count(3), "3 ярлыка");
        assert_eq!(shortcut_count(11), "11 ярлыков");
        assert_eq!(shortcut_count(22), "22 ярлыка");
        assert_eq!(shortcut_count(0), "0 ярлыков");
    }
}
