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
        }
    }

    pub fn font(&self, size: f32, weight: f32) -> TextStyle {
        TextStyle::new(size, weight).family(self.body)
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
pub fn button<S: 'static>(look: &Look, key: String, label: &str, kind: ButtonKind, on_press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let (fill, border, ink, hover) = match kind {
        ButtonKind::Normal => (look.elev.alpha(0.07), look.elev.alpha(0.16), look.fg, look.elev.alpha(0.07)),
        // `filter: brightness(1.08)` on hover.
        ButtonKind::Primary => (look.accent, look.accent, look.on_accent, Color::WHITE.alpha(0.08)),
        ButtonKind::Quiet => (Color::TRANSPARENT, Color::TRANSPARENT, look.fg.alpha(0.78), look.elev.alpha(0.14)),
    };
    row().key(key).min_h(32.0).px(14.0).py(6.0).radius(10.0).fill(fill).border(1.0, border)
        .hover(hover).hover_motion(look.hover_anim(), look.hover_anim())
        .items_center().justify_center()
        .child(text(label.to_string()).font(look.font(13.0, 500.0)).color(ink).single_line())
        .on_press_tx(on_press)
}

/// The three-line menu glyph, the close cross and the other strokes the menu draws, in a
/// 24-unit box.
pub fn glyph(lines: &[([f32; 2], [f32; 2])], colour: Color, width: f32) -> Shape {
    lines.iter().fold(Shape::new().view_box(24.0, 24.0), |shape, (a, b)| shape.line(*a, *b, width, LineCap::Round, colour))
}

/// `.menu-icon-btn`: 32 x 32, radius 9, the glyph at 75 % turning full on hover.
pub fn icon_button<S: 'static>(look: &Look, key: String, icon: Shape, on_press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    col().key(key).size(32.0, 32.0).radius(9.0).items_center().justify_center()
        .hover(look.elev.alpha(0.12)).hover_motion(look.hover_anim(), look.hover_anim())
        .child(el().size(18.0, 18.0).shape(icon))
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
                .border(1.0, if on { look.accent } else { look.elev.alpha(0.16) })
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
    el().key(key).size(40.0, 24.0).radius(12.0)
        .fill(if on { look.accent } else { look.fg.alpha(0.22) })
        .child(
            el().absolute(3.0, 3.0).size(18.0, 18.0).radius(9.0)
                .fill(if on { look.on_accent } else { look.fg })
                .offset(if on { 16.0 } else { 0.0 }, 0.0)
                .offset_motion(slide),
        )
        .on_press_tx(toggle)
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
    el().key(key).w_full().h(20.0)
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

/// `.menu-field-label`: the label and its value, 12 px at 78 %, the value in full ink.
pub fn field_label(look: &Look, label: &str, output: &str) -> El {
    row().w_full().justify_between().pl(0.0).pt(12.0).pr(0.0).pb(6.0)
        .child(text(label.to_string()).font(look.font(12.0, 400.0)).color(look.fg.alpha(0.78)).single_line())
        .child(text(output.to_string()).font(look.font(12.0, 400.0).features("'tnum' 1")).color(look.fg).single_line())
}

/// `.menu-tabs`: a well of 18 % around equal tabs, the selected one on a 16 % fill.
pub fn tabs<S: 'static>(look: &Look, tabs: &[(&str, &str)], selected: &str, choose: impl Fn(&mut S, &str, &mut Tx) + Clone + 'static) -> El {
    let mut bar = row().w_full().gap(4.0).padding(4.0).radius(12.0).fill(look.well.alpha(0.18));
    for (id, label) in tabs {
        let on = *id == selected;
        let id = id.to_string();
        let choose = choose.clone();
        bar = bar.child(
            col().key(format!("hit:tab:{id}")).grow(1.0).min_h(32.0).radius(9.0)
                .fill(if on { look.elev.alpha(0.16) } else { Color::TRANSPARENT })
                .items_center().justify_center()
                .child(text(label.to_string()).font(look.font(13.0, 500.0)).color(if on { look.fg } else { look.fg.alpha(0.78) }).single_line())
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
