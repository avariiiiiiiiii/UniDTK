//! The menu's building blocks as `core/ui/menu.css` draws them: buttons, icon buttons, pills,
//! the switch, the range slider, tabs, the "Тонкая настройка" disclosure, headings, notes and
//! rows. Each takes its colours from [`Look`], which is the menu panel's tokens in the active
//! theme (`r[theme.tokens-only]`); every hover and toggle runs on the page's motion tokens.

use rune::el::{col, el, row, text, El};
use rune::{Color, Family, LineCap, Shape, TextStyle, Tx};

use crate::motion::{Motion, Token, STANDARD};
use crate::theme::{ink_for, Material, Theme};

/// The tokens the menu panel paints with: `--fg-rgb` (the panel tint's ink under jelly),
/// `--bg-elev-rgb`, `--bg-surface-rgb`, `--theme-accent`, `--on-accent-rgb` made readable on
/// the accent, `--well-rgb` (the highlight under jelly, the scrim under flat), and the body font.
#[derive(Clone, Debug)]
pub struct Look {
    pub fg: Color,
    pub elev: Color,
    pub surface: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub well: Color,
    pub scrim: Color,
    pub status_ok: Color,
    pub status_warn: Color,
    pub status_error: Color,
    pub body: Family,
    pub display: Family,
    pub material: Material,
    pub motion: Motion,
    /// The actions in flight (`Menu::busy`, without `hit:`): their buttons show disabled.
    pub busy: std::rc::Rc<std::collections::HashSet<String>>,
}

/// WCAG contrast of two colours (`contrastRatio`).
fn contrast(a: Color, b: Color) -> f32 {
    let luminance = |c: Color| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    let (hi, lo) = {
        let (x, y) = (luminance(a), luminance(b));
        if x > y { (x, y) } else { (y, x) }
    };
    (hi + 0.05) / (lo + 0.05)
}

/// `readableOn`: the theme's text colour on a surface while it stays readable (4.5 : 1),
/// otherwise the better of white and near-black.
pub fn readable_on(surface: Color, preferred: Color) -> Color {
    if contrast(surface, preferred) >= 4.5 {
        return preferred;
    }
    let dark = rune::color::hex("#101014").unwrap_or(Color::BLACK);
    if contrast(surface, Color::WHITE) >= contrast(surface, dark) { Color::WHITE } else { dark }
}

impl Look {
    pub fn panel(theme: &Theme) -> Look {
        let jelly = theme.material == Material::Jelly;
        let panel_tint = theme.tint("panel", "calm", "lilac");
        Look {
            fg: if jelly { ink_for(panel_tint, theme.ink_depth) } else { theme.fg },
            elev: theme.bg_elevated,
            surface: theme.surface,
            accent: theme.accent,
            on_accent: readable_on(theme.accent, theme.on_accent),
            well: if jelly { Color::WHITE } else { theme.scrim },
            scrim: theme.scrim,
            status_ok: theme.status_ok,
            status_warn: theme.status_warn,
            status_error: theme.status_error,
            body: Theme::family(theme.font_body),
            display: Theme::family(theme.font_display),
            material: theme.material,
            motion: theme.motion(),
            busy: Default::default(),
        }
    }

    /// The body font at `size`, on the font's own lines (`line-height: normal`).
    pub fn font(&self, size: f32, weight: f32) -> TextStyle {
        TextStyle::new(size, weight).family(self.body).lh_normal()
    }

    fn hover_anim(&self) -> rune::Anim {
        self.motion.tween(self.motion.duration(Token::Hover), STANDARD)
    }
}

/// `.menu-heading`: 12 px, 600, upper case, 0.04 em apart, at 75 %.
pub fn heading(look: &Look, label: &str) -> El {
    el().pl(4.0).pt(18.0).pr(4.0).pb(8.0)
        .child(text(label.to_uppercase()).font(look.font(12.0, 600.0).tracking(0.48)).color(look.fg.alpha(0.75)))
}

/// `.menu-note`: 12 px on 1.45 lines, at 75 %.
pub fn note(look: &Look, content: &str) -> El {
    el().pl(4.0).pt(4.0).pr(4.0).pb(8.0)
        .child(text(content.to_string()).font(look.font(12.0, 400.0).lh(12.0 * 1.45)).color(look.fg.alpha(0.75)).wrap())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Normal,
    Primary,
    Quiet,
}

/// `.menu-btn`, `.menu-btn-primary`, `.menu-btn-quiet`: 32 px high, 6 x 14 padding, radius 10.
/// Hover changes the fill itself, as the page's `:hover { background }` does: 7 % to 14 %, the
/// accent to `brightness(1.08)`, a quiet one from nothing to 14 %.
pub fn button<S: 'static>(look: &Look, key: String, label: &str, kind: ButtonKind, on_press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    sized_button(look, key, label, kind, (32.0, 14.0, 6.0, 13.0), on_press)
}

/// `.menu-folder .menu-btn`: 28 px high, 4 x 10 padding, 12 px.
pub fn small_button<S: 'static>(look: &Look, key: String, label: &str, kind: ButtonKind, on_press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    sized_button(look, key, label, kind, (28.0, 10.0, 4.0, 12.0), on_press)
}

fn sized_button<S: 'static>(look: &Look, key: String, label: &str, kind: ButtonKind, (min_h, px, py, size): (f32, f32, f32, f32), on_press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let brighter = |c: Color| Color { r: (c.r * 1.08).min(1.0), g: (c.g * 1.08).min(1.0), b: (c.b * 1.08).min(1.0), a: c.a };
    let (fill, border, ink, hover) = match kind {
        ButtonKind::Normal => (look.elev.alpha(0.07), look.elev.alpha(0.16), look.fg, look.elev.alpha(0.14)),
        ButtonKind::Primary => (look.accent, look.accent, look.on_accent, brighter(look.accent)),
        ButtonKind::Quiet => (look.elev.alpha(0.0), Color::TRANSPARENT, look.fg.alpha(0.78), look.elev.alpha(0.14)),
    };
    // `.menu-btn:disabled { opacity: 0.5 }` while its action runs (`runOnce`).
    let busy = key.strip_prefix("hit:").is_some_and(|action| look.busy.contains(action));
    let button = row().key(key).min_h(min_h).px(px).py(py).radius(10.0).fill(fill).border(1.0, border).border_layout()
        .hover_fill(hover).hover_motion(look.hover_anim(), look.hover_anim())
        .items_center().justify_center()
        .child(text(label.to_string()).font(look.font(size, 500.0)).color(ink).single_line());
    if busy { button.opacity(0.5).disabled(true) } else { button.on_press_tx(on_press) }
}

/// The three-line menu glyph, the close cross and the other strokes the menu draws, in a
/// 24-unit box.
pub fn glyph(lines: &[([f32; 2], [f32; 2])], colour: Color, width: f32) -> Shape {
    lines.iter().fold(Shape::new().view_box(24.0, 24.0), |shape, (a, b)| shape.line(*a, *b, width, LineCap::Round, colour))
}

/// `.menu-icon-btn`: 32 x 32, radius 9, the glyph `size` points at 75 % turning full on hover.
/// `lines` is the glyph's strokes in a 24-unit box; `label` its `aria-label`.
pub fn icon_button<S: 'static>(look: &Look, key: String, label: &str, lines: &[([f32; 2], [f32; 2])], size: f32, on_press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    col().key(key).label(label.to_string()).size(32.0, 32.0).radius(9.0).items_center().justify_center()
        .hover(look.elev.alpha(0.12)).hover_motion(look.hover_anim(), look.hover_anim())
        .child(
            el().size(size, size)
                .child(el().absolute(0.0, 0.0).size(size, size).shape(glyph(lines, look.fg.alpha(0.75), 2.0)))
                .child(el().absolute(0.0, 0.0).size(size, size).shape(glyph(lines, look.fg, 2.0)).group_opacity(0.0, 1.0)),
        )
        .on_press_tx(on_press)
}

/// `.menu-pills`: a radio group of pills, 28 px high, the checked one on a 18 % well with an
/// accent border.
pub fn pills<S: 'static>(look: &Look, key: &str, options: &[(&str, &str)], checked: &str, choose: impl Fn(&mut S, &str, &mut Tx) + Clone + 'static) -> El {
    let mut group = row().gap(6.0).flex_wrap();
    for (value, label) in options {
        let on = *value == checked;
        let value = value.to_string();
        let choose = choose.clone();
        group = group.child(
            row().key(format!("{key}:{value}")).min_h(28.0).px(12.0).py(4.0).radius(999.0)
                .fill(if on { look.elev.alpha(0.18) } else { Color::TRANSPARENT })
                .border(1.0, if on { look.accent } else { look.elev.alpha(0.16) }).border_layout()
                .items_center()
                .child(text(label.to_string()).font(look.font(12.0, 400.0)).color(if on { look.fg } else { look.fg.alpha(0.85) }).single_line())
                .on_press_tx(move |state: &mut S, tx: &mut Tx| choose(state, &value, tx)),
        );
    }
    group
}

/// `.menu-switch`: a 40 x 24 track in 22 % ink, accent when on, with an 18 px knob that slides
/// 16 px on the transient token.
pub fn switch<S: 'static>(look: &Look, key: String, on: bool, toggle: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let slide = look.motion.tween(look.motion.duration(Token::Transient), STANDARD);
    // The track's and the knob's colours ease over the same token (`background` in the
    // transition list); Rune moves a paint change on a spring, here one without bounce.
    let paint = rune::Spring::new(look.motion.duration(Token::Transient), 0.0);
    let busy = key.strip_prefix("hit:").is_some_and(|action| look.busy.contains(action));
    let switch = el().key(key).size(40.0, 24.0).radius(12.0)
        .fill(if on { look.accent } else { look.fg.alpha(0.22) })
        .token_motion(paint)
        .child(
            el().absolute(3.0, 3.0).size(18.0, 18.0).radius(9.0)
                .fill(if on { look.on_accent } else { look.fg })
                .token_motion(paint)
                .offset(if on { 16.0 } else { 0.0 }, 0.0)
                .offset_motion(slide),
        );
    if busy { switch.opacity(0.5).disabled(true) } else { switch.on_press_tx(toggle) }
}

/// `<input type="range" class="menu-range">` with `accent-color`: Chromium's slider, a 4 px
/// track filled with the accent up to a 16 px accent thumb. Dragging reports values on the
/// step grid; `commit` runs on release, as the page commits on `change`.
pub fn slider<S: 'static>(
    look: &Look,
    key: String,
    value: f32,
    (min, max, step): (f32, f32, f32),
    preview: impl Fn(&mut S, f32, &mut Tx) + 'static,
    commit: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let fraction = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    const THUMB: f32 = 16.0;
    let snap = move |x: f32, width: f32| {
        let t = ((x - THUMB / 2.0) / (width - THUMB).max(1.0)).clamp(0.0, 1.0);
        let raw = min + t * (max - min);
        let stepped = min + ((raw - min) / step).round() * step;
        (stepped.clamp(min, max) * 1e4).round() / 1e4
    };
    let preview = std::rc::Rc::new(preview);
    let commit = std::rc::Rc::new(commit);
    let (key_preview, key_commit) = (preview.clone(), commit.clone());
    // The keys of `<input type="range">`: arrows a step, Page keys a tenth, Home and End the
    // ends; each change is committed at once, as a key's `change` event commits it.
    let keyed = move |state: &mut S, event: &rune::KeyEvent, tx: &mut Tx| {
        let next = match event.key {
            rune::Key::Left | rune::Key::Down => value - step,
            rune::Key::Right | rune::Key::Up => value + step,
            rune::Key::PageDown => value - (max - min) / 10.0,
            rune::Key::PageUp => value + (max - min) / 10.0,
            rune::Key::Home => min,
            rune::Key::End => max,
            _ => return false,
        };
        let stepped = (min + ((next - min) / step).round() * step).clamp(min, max);
        key_preview(state, (stepped * 1e4).round() / 1e4, tx);
        key_commit(state, tx);
        true
    };
    el().key(key).w_full().h(20.0).radius(10.0)
        .focusable(true).focus_ring(2.0, 2.0, look.accent).on_key(keyed)
        .child(el().absolute(0.0, 8.0).w_full().h(4.0).radius(2.0).fill(look.fg.alpha(0.18)))
        .child(
            row().absolute(0.0, 0.0).w_full().h(20.0)
                .child(el().grow(fraction).h(4.0).mt(8.0).radius(2.0).fill(look.accent))
                .child(el().size(THUMB, THUMB).mt(2.0).radius(THUMB / 2.0).fill(look.accent))
                .child(el().grow(1.0 - fraction)),
        )
        .on_drag(move |state: &mut S, drag: &mut rune::el::Drag, tx: &mut Tx| {
            let next = snap(drag.x, drag.w);
            preview(state, next, tx);
            if drag.phase == rune::el::Phase::Up {
                commit(state, tx);
            }
        })
}

/// `.menu-field-label` inside a `.menu-field` (no margin of its own): the label and its value,
/// 12 px at 78 %, the value in full ink.
pub fn field_label(look: &Look, label: &str, output: &str) -> El {
    row().w_full().justify_between()
        .child(text(label.to_string()).font(look.font(12.0, 400.0)).color(look.fg.alpha(0.78)).single_line())
        .child(text(output.to_string()).font(look.font(12.0, 400.0).features("'tnum' 1")).color(look.fg).single_line())
}

/// A `.menu-field-label` standing alone, between sections: 12 px above, 6 below.
pub fn field_heading(look: &Look, label: &str) -> El {
    el().w_full().pt(12.0).pb(6.0).child(field_label(look, label, ""))
}

/// A text field whose text, caret and selection are drawn in `ink`. Rune draws a field
/// with no colour of its own white (the host sets no window foreground), so a pasted
/// subscription link was near invisible on a light theme; every host field goes through
/// here and names the ink its surface needs (`theme.contrast`).
pub fn text_field(edit: &rune::TextEdit, style: TextStyle, ink: Color) -> El {
    rune::el::text_input(edit, style).color(ink).caret(ink, ink.alpha(0.2))
}

/// `.menu-tabs`: a well of 18 % around equal tabs, the selected one on a 16 % fill; an
/// unselected tab's label turns full ink on hover.
/// Left and Right move to the neighbouring tab, wrapping, and take the focus with them.
pub fn tabs<S: 'static>(look: &Look, tabs: &[(&str, &str)], selected: &str, choose: impl Fn(&mut S, &str, &mut Tx) + Clone + 'static) -> El {
    let mut bar = row().w_full().gap(4.0).padding(4.0).radius(12.0).fill(look.well.alpha(0.18));
    let ids: Vec<String> = tabs.iter().map(|(id, _)| id.to_string()).collect();
    for (index, (id, label)) in tabs.iter().enumerate() {
        let on = *id == selected;
        let id = id.to_string();
        let choose = choose.clone();
        let (keys_choose, ids) = (choose.clone(), ids.clone());
        bar = bar.child(
            col().key(format!("hit:tab:{id}")).grow(1.0).min_h(32.0).radius(9.0)
                .focusable(true).focus_ring(2.0, 2.0, look.accent)
                .on_key(move |state: &mut S, event: &rune::KeyEvent, tx: &mut Tx| {
                    let step = match event.key {
                        rune::Key::Left => ids.len() - 1,
                        rune::Key::Right => 1,
                        _ => return false,
                    };
                    let next = &ids[(index + step) % ids.len()];
                    keys_choose(state, next, tx);
                    tx.focus(format!("hit:tab:{next}"), true);
                    true
                })
                .fill(if on { look.elev.alpha(0.16) } else { Color::TRANSPARENT })
                .hover_group().hover_motion(look.hover_anim(), look.hover_anim())
                .items_center().justify_center()
                .child(text(label.to_string()).font(look.font(13.0, 500.0)).color(if on { look.fg } else { look.fg.alpha(0.78) }).group_color(look.fg).single_line())
                .on_press_tx(move |state: &mut S, tx: &mut Tx| choose(state, &id, tx)),
        );
    }
    bar
}

/// `.menu-details > summary`: 40 px, 500, with the `›` that turns a quarter when open.
pub fn summary<S: 'static>(look: &Look, key: String, label: &str, open: bool, toggle: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let turn = look.motion.tween(look.motion.duration(Token::Hover), STANDARD);
    row().key(key).w_full().min_h(40.0).px(4.0).py(0.0).items_center()
        .child(
            el().w(16.0).child(
                text("›").font(look.font(13.0, 500.0)).color(look.fg)
                    .rotate(if open { std::f32::consts::FRAC_PI_2 } else { 0.0 })
                    .rotate_motion(turn),
            ),
        )
        .child(text(label.to_string()).font(look.font(13.0, 500.0)).color(look.fg).single_line())
        .on_press_tx(toggle)
}
