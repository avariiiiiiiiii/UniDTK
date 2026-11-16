//! The settings menu (`r[kernel.menu-in-core]`), as `core/ui/menu.js` and `menu.css` build
//! it: the side panel on the right of the desk with "Настройки", the "Стол" and "Вид" tabs,
//! the VPN view reached from the VPN widget, and the footer's "Выйти из UniDTK"; the
//! three-line button at the top right that opens it; the sheets that open over it for one
//! widget or one folder. The panes live in `menu_desk.rs`, `menu_look.rs` and `menu_vpn.rs`.
//!
//! The menu talks to the kernel through [`MenuHost`]: writes to the desk go as `desk_apply`
//! operations (`r[desk.position-merge]`), other commands (`set_module_enabled`, folders, the
//! Start menu, VPN) as requests whose answers come back to the UI thread.

use std::collections::HashSet;

use rune::el::{col, el, row, text, El};
use rune::{Color, Shadow, TextEdit, Tx};
use serde_json::Value;

use crate::controls::{self, ButtonKind, Look};
use crate::desk::Layout;
use crate::motion::{Token, EXPO_OUT, STANDARD, SURFACE_EXIT};
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
    /// Drops the preview: the stored theme shows again.
    fn end_preview(&mut self);
    /// `desk_apply` with these operations; a failure is reported in a toast.
    fn apply(&mut self, ops: Vec<Value>, tx: &mut Tx);
    /// Any kernel command; `then` runs on the UI thread with the answer.
    fn call(&mut self, cmd: &str, args: Value, then: Box<dyn FnOnce(&mut Self, Result<Value, String>, &mut Tx) + Send>);
    /// Enters arrange mode; the menu reopens when it ends.
    fn enter_arrange(&mut self, tx: &mut Tx);
    /// SVG markup as an image `size` points square, tinted where it says `currentColor`.
    fn svg(&self, markup: &str, size: f32, colour: Color) -> Option<rune::Pixels>;
    /// The Windows icon of a shortcut target, once the kernel has sent it; asking for one
    /// the host does not have queues the request.
    fn shortcut_icon(&self, target: &str) -> Option<rune::Pixels>;
    /// The system colour dialog, on a worker; `then` runs on the UI thread with the choice.
    fn pick_color(&mut self, initial: [u8; 3], then: Box<dyn FnOnce(&mut Self, [u8; 3], &mut Tx) + Send>);
    fn canvas(&self) -> (f32, f32);
    fn arranging(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Desk,
    Look,
    Vpn,
}

/// A value the menu asked the kernel for.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loaded<T> {
    pub fn ready(&self) -> Option<&T> {
        if let Loaded::Ready(value) = self { Some(value) } else { None }
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
    pub view: View,
    pub sheet: Option<Sheet>,
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
    pub vpn: crate::menu_vpn::VpnPane,
    /// The widget row the pointer is over: its card is highlighted on the desk.
    pub highlight: Option<String>,
}

impl Default for Menu {
    fn default() -> Self {
        Menu {
            shown: false,
            open: false,
            closing_until: None,
            view: View::Desk,
            sheet: None,
            fine_open: false,
            preset_name: TextEdit::new(""),
            previous_colors: None,
            busy: HashSet::new(),
            disabled_modules: Loaded::Loading,
            folders: Loaded::Loading,
            missing: HashSet::new(),
            creating_folder: false,
            new_folder_name: TextEdit::new(""),
            new_folder_icon: "folder",
            start_for: None,
            start_list: Loaded::Loading,
            start_search: TextEdit::new(""),
            folder_names: Default::default(),
            item_names: Default::default(),
            vpn: Default::default(),
            highlight: None,
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
    let view = if state.menu_ref().view == View::Vpn { View::Desk } else { state.menu_ref().view };
    let menu = state.menu();
    menu.shown = true;
    menu.open = true;
    menu.closing_until = None;
    show_view(state, view, tx);
}

/// `closeMenu`: slides out, then leaves the tree.
pub fn close<S: MenuHost>(state: &mut S, _tx: &mut Tx) {
    if !state.menu_ref().open {
        return;
    }
    let motion = state.layout().map(|layout| layout.theme.motion());
    let duration = motion.map_or(0.3, |m| m.duration(Token::Motion));
    let menu = state.menu();
    menu.open = false;
    menu.sheet = None;
    menu.highlight = None;
    menu.closing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs_f32(duration));
}

/// `handleMenuEscape`: an open sheet closes first, VPN steps back to "Стол", anything else
/// closes the menu. False when the menu is closed.
pub fn escape<S: MenuHost>(state: &mut S, tx: &mut Tx) -> bool {
    if !state.menu_ref().open {
        return false;
    }
    if state.menu_ref().sheet.is_some() {
        state.menu().sheet = None;
    } else if state.menu_ref().view == View::Vpn {
        show_view(state, View::Desk, tx);
    } else {
        close(state, tx);
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
        return None;
    }
    Some(until)
}

/// `showView`: a pane reads what it shows from the kernel each time it is shown.
pub fn show_view<S: MenuHost>(state: &mut S, view: View, tx: &mut Tx) {
    state.menu().view = view;
    match view {
        View::Desk => crate::menu_desk::load(state, tx),
        View::Look => {}
        View::Vpn => crate::menu_vpn::load(state, tx),
    }
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
                Err(error) => error_toast(state, &format!("Не получилось: {error}")),
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

/// The panel's paint in each material (`.menu-container`, and the jelly panel recipe).
fn panel_paint(panel: El, theme: &Theme, look: &Look) -> El {
    let hi = Color::WHITE;
    match theme.material {
        Material::Flat => {
            let panel = panel.fill(look.surface.alpha(0.97)).border(1.0, look.elev.alpha(0.12));
            // `--theme-surface-shadow`: the computed one when the theme has a glass shadow.
            if theme.glass_shadow > 0.0 {
                panel
                    .box_shadow(Shadow { y: 1.0, color: look.elev.alpha(theme.glass_rim * 0.4), inset: true, ..Shadow::default() })
                    .box_shadow(Shadow { y: 24.0, blur: 70.0, spread: -6.0, color: look.scrim.alpha(theme.glass_shadow * 0.65), ..Shadow::default() })
                    .box_shadow(Shadow { y: 12.0, blur: 30.0, spread: -4.0, color: theme.bg_base.alpha(theme.glass_shadow * 0.35), ..Shadow::default() })
            } else {
                panel
                    .box_shadow(Shadow { y: 28.0, blur: 90.0, color: Color::BLACK.alpha(0.8), ..Shadow::default() })
                    .box_shadow(Shadow { y: 1.0, color: look.elev.alpha(0.08), inset: true, ..Shadow::default() })
            }
        }
        Material::Jelly => {
            let tint = theme.tint("panel", "calm", "lilac");
            let offset = theme.shadow_offset;
            panel
                .fill(mix_oklch(tint, theme.surface_tint, hi))
                .radial_gradient([0.18, 0.08], [1.2 * 420.0, 0.95 * 700.0], [(0.0, hi.alpha(theme.glass_sheen * 0.7)), (0.58, hi.alpha(0.0))])
                .border(1.0, hi.alpha(0.75))
                .box_shadow(Shadow { y: 1.5, color: hi.alpha(theme.glass_rim * 0.9), inset: true, ..Shadow::default() })
                .box_shadow(Shadow { y: offset * 2.0, blur: offset * 5.0, color: scaled(tint, 0.4, 1.0, 0.02, theme.glass_shadow * 0.4), ..Shadow::default() })
        }
    }
}

/// The three-line button of the top bar (`#openMenu`, `.action-btn`): 36 px, radius 11, at
/// 18 px from the top right; it lifts a pixel on hover. Hidden while arranging.
pub fn opener<S: MenuHost>(state: &S, theme: &Theme, canvas: (f32, f32)) -> Option<El> {
    if state.arranging() {
        return None;
    }
    let motion = theme.motion();
    let ease = motion.tween(motion.duration(Token::Transient), STANDARD);
    let lines = [([3.0, 12.0], [21.0, 12.0]), ([3.0, 6.0], [21.0, 6.0]), ([3.0, 18.0], [21.0, 18.0])];
    let ink = theme.fg.alpha(0.45);
    Some(
        col().key("hit:menu-button").label("Меню UniDTK")
            .absolute(canvas.0 - 18.0 - 36.0, 18.0).size(36.0, 36.0).radius(11.0)
            .fill(theme.bg_base.alpha(0.78)).border(1.0, theme.bg_elevated.alpha(0.08))
            .hover(theme.bg_elevated.alpha(0.1)).hover_motion(ease, ease)
            .items_center().justify_center()
            .child(el().size(18.0, 18.0).shape(controls::glyph(&lines, ink, 2.0)))
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
    let look = Look::panel(theme);
    let open = state.menu_ref().open;
    let width = (canvas.0 - 24.0).min(420.0);
    let motion = theme.motion();
    let slide = motion.tween(motion.duration(Token::Motion), if open { EXPO_OUT } else { SURFACE_EXIT });
    let fade = motion.tween(motion.duration(Token::Transient), STANDARD);
    let view = state.menu_ref().view;

    let close_cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
    let header = row().w_full().justify_between().items_center().pl(20.0).pr(16.0).pt(16.0).pb(8.0)
        .child(text("Настройки").font(look.font(17.0, 600.0)).color(look.fg).single_line())
        .child(controls::icon_button(&look, "hit:menu-close".into(), controls::glyph(&close_cross, look.fg.alpha(0.75), 2.0), |state: &mut S, tx: &mut Tx| close(state, tx)));
    let tabs = (view != View::Vpn).then(|| {
        el().w_full().px(16.0).pb(8.0).child(controls::tabs(
            &look,
            &[("desk", "Стол"), ("look", "Вид")],
            if view == View::Look { "look" } else { "desk" },
            |state: &mut S, id: &str, tx: &mut Tx| show_view(state, if id == "look" { View::Look } else { View::Desk }, tx),
        ))
    });
    let pane = match view {
        View::Desk => crate::menu_desk::pane(state, &look),
        View::Look => crate::menu_look::pane(state, &look),
        View::Vpn => crate::menu_vpn::pane(state, &look),
    };
    let body = col().key("menu-body").w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).px(16.0).pt(4.0).pb(16.0).child(pane);
    let quit = controls::button(&look, "hit:menu-quit".into(), "Выйти из UniDTK", ButtonKind::Quiet, |state: &mut S, _tx: &mut Tx| {
        run_once(state, "menu-quit", "quit_app", serde_json::json!({}), |_, _, _| {});
    });
    let footer = el().w_full().px(16.0).pt(10.0).pb(14.0).border_sides([1.0, 0.0, 0.0, 0.0], look.elev.alpha(0.1)).child(row().child(quit));

    let mut panel = col().key("hit:menu").label("Настройки")
        .absolute(canvas.0 - 12.0 - width, 12.0).w(width).h(canvas.1 - 24.0)
        .radius(theme.radius_surface)
        .offset(if open { 0.0 } else { width + 24.0 }, 0.0).offset_motion(slide)
        .opacity(if open { 1.0 } else { 0.0 }).opacity_motion(fade)
        .child(header);
    panel = panel_paint(panel, theme, &look);
    if let Some(tabs) = tabs {
        panel = panel.child(tabs);
    }
    panel = panel.child(body).child(footer);
    if let Some(sheet) = state.menu_ref().sheet.clone() {
        panel = panel.child(crate::menu_desk::sheet(state, &look, theme, &sheet, width, canvas.1 - 24.0));
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
pub fn sheet_frame<S: MenuHost>(look: &Look, theme: &Theme, title: &str, width: f32, height: f32, body: El) -> El {
    let close_cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
    let frame = col().key("hit:menu-sheet").absolute(0.0, 0.0).w(width).h(height).radius(theme.radius_surface)
        .transition(rune::Transition::opacity())
        .child(
            row().w_full().justify_between().items_center().pl(20.0).pr(16.0).pt(18.0).pb(8.0)
                .child(text(title.to_string()).font(look.font(15.0, 600.0)).color(look.fg).single_line())
                .child(controls::icon_button(look, "hit:sheet-close".into(), controls::glyph(&close_cross, look.fg.alpha(0.75), 2.0), |state: &mut S, _tx: &mut Tx| {
                    state.menu().sheet = None;
                })),
        )
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).px(20.0).pb(20.0).child(body));
    panel_paint(frame, theme, look)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_counts_decline_as_russian_does() {
        assert_eq!(shortcut_count(1), "1 ярлык");
        assert_eq!(shortcut_count(3), "3 ярлыка");
        assert_eq!(shortcut_count(11), "11 ярлыков");
        assert_eq!(shortcut_count(22), "22 ярлыка");
        assert_eq!(shortcut_count(0), "0 ярлыков");
    }
}
