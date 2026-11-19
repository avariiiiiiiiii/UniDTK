//! One card on the desk, for every widget alike (`buildWidgetCard`, `.widget-card` in
//! `styles.css`): its chrome in both materials, the clip the surface morph shows
//! (`surface.rs`), the jelly wash and grain, the header of a header card with its icon flying
//! into the surface (`r[motion.shared-flight]`), the body or the bare content the widget draws,
//! the nesting zone, the outlines, and arrange mode's chrome. `r[theme.tokens-only]`.

use rune::el::{col, el, row, text, El};
use rune::{Color, Pixels, Shadow, TextStyle, Tx};

use crate::desk::Placement;
use crate::surface::Pose;
use crate::theme::{derived, mix_oklch, scaled, Material, Theme};
use crate::Host;

/// `.widget-card` without a stored size: 340 wide, the 64 px header high.
pub const CARD: (f32, f32) = (340.0, 64.0);
/// `toggleWidget`: a header card opens 400 wide and at most 500 high.
pub const OPEN_WIDTH: f32 = 400.0;
pub const OPEN_MAX: f32 = 500.0;
/// The body inside an opened header card: the 1 px border and 16 px each side, under the
/// 64 px header and the body's 2 px.
pub const BODY_X: f32 = 17.0;
pub const BODY_Y: f32 = 67.0;
pub const BODY_W: f32 = OPEN_WIDTH - 2.0 - 32.0;

fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color, inset: bool) -> Shadow {
    Shadow { x, y, blur, spread, color, inset }
}

/// The card's resting radius: the strip radius for a strip card (the page's
/// `.telegram-widget:not(.expanded)`, `.glyph-bar-widget`, which only Sea Glass wears; Nothing
/// Classic wears it here too, so the slider shows in both), the card radius otherwise.
pub fn tile_radius(theme: &Theme, strip: bool) -> f32 {
    if strip { theme.radius_strip } else { theme.radius_card }
}

/// The surface radius a card opens to (`.widget-card.expanded`).
pub fn surface_radius(theme: &Theme) -> f32 {
    if theme.material == Material::Jelly { theme.radius_surface } else { theme.radius_card }
}

/// The card's own paint (body, gradients, border, shadows) as the material and the card's
/// state choose: `.widget-card`, the jelly block and `.widget-card.expanded`.
/// `editing` is arrange mode: a flat card's border turns 14 % (`body.edit-mode .widget-card`).
pub fn paint(card: El, theme: &Theme, tint: Color, radius: f32, expanded: bool, editing: bool) -> El {
    let hi = Color::WHITE;
    match (theme.material, expanded) {
        (Material::Jelly, false) => {
            let offset = theme.shadow_offset;
            card.radius(radius)
                .fill(Color { a: theme.glass_opacity, ..tint })
                .linear_gradient(155f32.to_radians(), [(0.42, hi.alpha(0.0)), (1.0, derived(tint, -0.07, 1.5, 0.02, theme.glass_depth * 0.75))])
                .box_shadow(shadow(0.0, 1.5, 0.0, 0.0, hi.alpha(theme.glass_rim * 0.9), true))
                .box_shadow(shadow(1.5, 0.0, 0.0, 0.0, hi.alpha(theme.glass_rim * 0.35), true))
                .box_shadow(shadow(-2.0, -3.0, 6.0, 0.0, derived(tint, -0.2, 1.4, 0.0, theme.glass_depth * 0.45), true))
                .box_shadow(shadow(offset * 0.6, offset, offset * 1.6, -offset * 0.2, scaled(tint, 0.5, 0.5, 0.0, theme.glass_shadow * 0.6), false))
                .box_shadow(shadow(1.0, 2.0, 3.0, 0.0, theme.scrim.alpha(theme.glass_shadow * 0.2), false))
        }
        // An expanded jelly card keeps its dock body (the wash fades in over it), takes the
        // surface radius and a white hairline, and loses its shadows
        // (`.widget-card.expanded { box-shadow: none !important }`).
        (Material::Jelly, true) => card
            .radius(radius)
            .fill(Color { a: theme.glass_opacity, ..tint })
            .linear_gradient(155f32.to_radians(), [(0.42, hi.alpha(0.0)), (1.0, derived(tint, -0.07, 1.5, 0.02, theme.glass_depth * 0.75))])
            .border(1.0, hi.alpha(0.8)),
        (Material::Flat, false) => {
            // `.widget-card:hover { border-color: transparent }`, on the transient token.
            let motion = theme.motion();
            let ease = motion.tween(motion.duration(crate::motion::Token::Transient), crate::motion::STANDARD);
            // `transition: border-color var(--dur-transient) ...`: arrange mode's border eases in
            // (Rune moves a paint change on a spring; one without bounce over the same time).
            let mut card = card.radius(radius).fill(Color { a: theme.glass_opacity, ..theme.bg_base })
                .border(1.0, theme.bg_elevated.alpha(if editing { 0.14 } else { 0.12 }))
                .token_motion(rune::Spring::new(motion.duration(crate::motion::Token::Transient), 0.0))
                .hover_border(Color::TRANSPARENT).hover_motion(ease, ease);
            if theme.glass_sheen > 0.0 {
                card = card.linear_gradient(
                    145f32.to_radians(),
                    [
                        (0.0, theme.bg_elevated.alpha(theme.glass_sheen * 0.35)),
                        (0.4, theme.bg_base.alpha(theme.glass_opacity * 0.82)),
                        (1.0, theme.bg_base.alpha(theme.glass_opacity)),
                    ],
                );
            }
            if theme.glass_shadow > 0.0 || theme.glass_rim > 0.0 {
                // The page's flat shadows are fixed; "Сдвиг тени" moves them down and right here.
                let (rim, drop, offset) = (theme.glass_rim, theme.glass_shadow, theme.shadow_offset);
                card = card
                    .box_shadow(shadow(0.0, 1.0, 1.0, 0.0, theme.bg_elevated.alpha(rim * 0.75), true))
                    .box_shadow(shadow(0.0, 2.0, 6.0, -1.0, theme.bg_elevated.alpha(rim * 0.35), true))
                    .box_shadow(shadow(0.0, -1.0, 1.0, 0.0, theme.bg_elevated.alpha(rim * 0.2), true))
                    .box_shadow(shadow(offset * 0.6, 4.0 + offset, 12.0, -2.0, theme.scrim.alpha(drop * 0.12), false))
                    .box_shadow(shadow(offset * 0.6, 16.0 + offset, 36.0, -4.0, theme.bg_base.alpha(drop * 0.4), false))
                    .box_shadow(shadow(offset * 0.6, 28.0 + offset, 60.0, -10.0, theme.scrim.alpha(drop * 0.15), false));
            }
            if theme.glass_depth > 0.0 {
                // "Тёмная кромка": the jelly body's inner shade at the lower right, as an inset
                // shadow on the dark glass.
                card = card.box_shadow(shadow(-2.0, -3.0, 6.0, 0.0, theme.scrim.alpha(theme.glass_depth * 0.45), true));
            }
            card
        }
        // "Оттенок раскрытых окон": a quarter of the slider's share of the card's tint in the surface,
        // so the dark panel keeps its contrast (`r[theme.contrast]`); at 0 it is the page's flat surface.
        (Material::Flat, true) => card.radius(radius).fill(mix_oklch(tint, theme.surface_tint * 0.25, theme.surface).alpha(0.96)).border(1.0, theme.bg_elevated.alpha(0.15)),
    }
}

fn style(theme: &Theme, size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).family(Theme::family(theme.font_body)).lh_normal()
}

/// A header card's header (`.widget-header`): 13 x 16 padding, the 38 px icon, the name and
/// description, the chevron that turns a quarter when open. A tile at most 150 px wide shows
/// its icon alone, centred. `icon` false leaves the icon's place empty (it is flying).
#[derive(Clone, Copy)]
pub struct Header<'a> {
    pub name: &'a str,
    pub desc: &'a str,
    pub icon: &'a str,
    pub ink: Color,
    pub width: f32,
    pub expanded: bool,
}

/// Where the header's icon sits in a header `width` wide.
pub fn header_icon_at(width: f32, expanded: bool) -> (f32, f32) {
    if width <= 150.0 && !expanded { ((width - 38.0) / 2.0, 13.0) } else { (16.0, 13.0) }
}

fn header_icon(host: &Host, theme: &Theme, icon: &str, ink: Color) -> El {
    col().size(38.0, 38.0).radius(11.0).items_center().justify_center()
        .fill(theme.bg_elevated.alpha(0.06)).border(1.0, theme.bg_elevated.alpha(0.06))
        .child(crate::shell::svg_icon(host, icon, 20.0, ink.alpha(0.7)))
}

fn header(host: &Host, theme: &Theme, h: &Header<'_>, show_icon: bool) -> El {
    let motion = theme.motion();
    let compact = h.width <= 150.0 && !h.expanded;
    let icon = if show_icon { header_icon(host, theme, h.icon, h.ink) } else { el().size(38.0, 38.0) };
    // `.widget-header:hover` washes it 3 %; `:active` 6 %.
    let mut head = row().w(h.width).h(64.0).gap(12.0).items_center()
        .hover(theme.bg_elevated.alpha(0.03)).press(theme.bg_elevated.alpha(0.06))
        .hover_motion(motion.tween(motion.duration(crate::motion::Token::Hover), crate::motion::STANDARD), motion.tween(motion.duration(crate::motion::Token::Hover), crate::motion::STANDARD))
        .child(icon);
    if compact {
        return head.justify_center();
    }
    let chevron = [([9.0, 18.0], [15.0, 12.0]), ([15.0, 12.0], [9.0, 6.0])];
    // `transition: transform var(--dur-layout) var(--ease-spring)`.
    let turn = motion.tween(motion.duration(crate::motion::Token::Layout), crate::motion::SPRING);
    head = head.px(16.0)
        .child(
            col().grow(1.0).min_w(0.0)
                .child(text(h.name.to_string()).font(style(theme, 13.76, 500.0)).color(h.ink).single_line().truncate())
                .child(text(h.desc.to_string()).font(style(theme, 10.56, 400.0)).color(h.ink.alpha(0.38)).single_line().truncate().mt(1.0)),
        )
        .child(
            el().size(16.0, 16.0)
                .shape(crate::controls::glyph(&chevron, h.ink.alpha(if h.expanded { 0.5 } else { 0.22 }), 2.5))
                .rotate(if h.expanded { std::f32::consts::FRAC_PI_2 } else { 0.0 })
                .rotate_motion(turn),
        );
    head
}

/// Everything one card needs to be drawn.
pub struct Frame<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub theme: &'a Theme,
    pub tint: Color,
    pub strip: bool,
    /// The tile on the canvas, and the box it opens into.
    pub tile: Placement,
    pub frame: Placement,
    /// The morph's pose while its surface is open or folding; `None` at rest on the desk.
    pub pose: Option<Pose>,
    pub grain: &'a Pixels,
    pub grain_size: (f32, f32),
    /// A header card's header; `None` for a bare card.
    pub header: Option<Header<'a>>,
    /// What the widget draws, in the box's coordinates (a header card's body is placed under
    /// the header by the caller).
    pub content: Option<El>,
    /// The nesting zone and where it starts, in the box's coordinates.
    pub nest: Option<(El, [f32; 3])>,
    /// The outline it wears (the menu's highlight, a nesting mark) and its offset.
    pub outline: Option<(Color, f32)>,
    /// Drawn over the tile in its coordinates: arrange mode's handles, "×" and the widget's
    /// tools.
    pub overlay: Vec<El>,
    /// Arrange mode is dragging this card (`.widget-card.dragging`).
    pub dragged: bool,
    /// Arrange mode is on.
    pub editing: bool,
    /// The blurred wallpaper a flat card shows under its glass, and the canvas it covers.
    pub backdrop: Option<(&'a Pixels, (f32, f32))>,
    /// Wires the card's own element: its press, its drag, a file drop.
    pub window: Box<dyn FnOnce(El) -> El + 'a>,
    /// Wires a header card's header: a folder opens from it.
    pub head: Box<dyn FnOnce(El) -> El + 'a>,
}

/// The card: an element the size of its box at the box's place, holding the visible window.
pub fn view(host: &Host, f: Frame<'_>) -> El {
    let theme = f.theme;
    let open = f.pose.is_some();
    let tile_in_box = Placement { x: f.tile.x - f.frame.x, y: f.tile.y - f.frame.y, w: f.tile.w, h: f.tile.h };
    let (window, radius) = match f.pose {
        Some(pose) => (pose.clip.window(f.frame), pose.clip.radius),
        None => (tile_in_box, tile_radius(theme, f.strip)),
    };
    let progress = f.pose.map_or(0.0, |p| p.progress);
    let shift = |element: El, x: f32, y: f32| element.absolute(x - window.x, y - window.y);

    // Inside the clip: the jelly sheen, wash and grain, the content, the header, the zone.
    let mut inside = el().absolute(0.0, 0.0).size(window.w, window.h).clip_rounded(radius);
    let jelly = theme.material == Material::Jelly;
    let painted_box = if open { (f.frame.w, f.frame.h) } else { (window.w, window.h) };
    let origin = if open { (0.0, 0.0) } else { (window.x, window.y) };
    if jelly {
        let hi = Color::WHITE;
        inside = inside.child(
            shift(el().size(painted_box.0, painted_box.1), origin.0, origin.1)
                .radial_gradient([0.18, 0.08], [1.2 * painted_box.0, 0.95 * painted_box.1], [(0.0, hi.alpha(theme.glass_sheen * 0.7)), (0.58, hi.alpha(0.0))]),
        );
        if let Some(pose) = f.pose {
            // `::after`: the surface wash, under the grain.
            inside = inside.child(
                shift(el().size(f.frame.w, f.frame.h), 0.0, 0.0)
                    .fill(mix_oklch(f.tint, theme.surface_tint, hi))
                    .radial_gradient([0.15, 0.0], [1.2 * f.frame.w, 0.7 * f.frame.h], [(0.0, hi.alpha(theme.glass_sheen * 0.55)), (0.6, hi.alpha(0.0))])
                    .child(el().absolute(0.0, 0.0).w(f.frame.w).h(1.5).fill(hi.alpha(theme.glass_rim * 0.9)))
                    .opacity(pose.wash),
            );
        }
        inside = inside.child(shift(el().size(f.grain_size.0, f.grain_size.1).pixels(f.grain), 0.0, 0.0));
    } else if theme.glass_grain > 0.0 {
        // The page draws grain on Sea Glass only; "Зерно" works on flat glass here.
        inside = inside.child(shift(el().size(f.grain_size.0, f.grain_size.1).pixels(f.grain), 0.0, 0.0));
    }
    if let Some(content) = f.content {
        inside = inside.child(shift(el().size(f.frame.w, f.frame.h), 0.0, 0.0).child(content));
    }
    let mut flying_icon = None;
    let wire_head = f.head;
    if let Some(h) = &f.header {
        if open {
            // The card is laid out at its box from the first frame: the header sits at the
            // box's top, and only its icon flies from the tile into it.
            inside = inside.child(wire_head(shift(el(), 0.0, 0.0).child(header(host, theme, &Header { width: f.frame.w, ..*h }, false))));
            let from = header_icon_at(f.tile.w, false);
            let to = header_icon_at(f.frame.w, true);
            let at = (tile_in_box.x + from.0 + (to.0 - (tile_in_box.x + from.0)) * progress, tile_in_box.y + from.1 + (to.1 - (tile_in_box.y + from.1)) * progress);
            flying_icon = Some(shift(header_icon(host, theme, h.icon, h.ink), at.0, at.1));
        } else {
            inside = inside.child(wire_head(el().child(header(host, theme, &Header { width: window.w, ..*h }, true))));
        }
    }
    if let Some(icon) = flying_icon {
        inside = inside.child(icon);
    }
    if let Some((zone, [x, y, w])) = f.nest {
        inside = inside.child(shift(el().w(w), x, y).child(zone));
    }

    let mut card = if open {
        // The painted box under the clip; its shadows are gone while expanded.
        el().key(format!("card:{}", f.id)).label(f.label.to_string())
            .absolute(window.x, window.y).size(window.w, window.h).clip_rounded(radius)
            .child(shift(paint(el().size(f.frame.w, f.frame.h), theme, f.tint, surface_radius(theme), true, f.editing), 0.0, 0.0))
            .child(inside)
    } else {
        paint(el().key(format!("card:{}", f.id)).label(f.label.to_string()).absolute(window.x, window.y).size(window.w, window.h), theme, f.tint, radius, false, f.editing)
            .child(inside)
    };
    card = (f.window)(card);
    if f.dragged {
        // `.widget-card.dragging`: lifted, 88 %, a deeper shadow.
        card = card.opacity(0.88).shadow(24.0, 60.0, Color::BLACK.alpha(0.35));
    }
    if let Some((colour, offset)) = f.outline {
        card = card.outline(2.0, offset, colour);
    }
    for part in f.overlay {
        card = card.child(part);
    }
    // The card's whole box, keyed by its id: Rune knows a card by its key, not by its place
    // among the desk's children, which changes as a card opens over the others or arrange
    // mode adds its grid; an unkeyed wrapper would be taken for its neighbour and fly there.
    // Its rect is what takes the pointer (`widget:` in the window region).
    let mut out = el().key(format!("widget:{}", f.id)).absolute(f.frame.x, f.frame.y).size(f.frame.w, f.frame.h);
    // `.widget-card.jelly.expanded { backdrop-filter: none }`: an open Sea Glass card is a wash over
    // its dock body, not glass over the wallpaper.
    if let Some((backdrop, canvas)) = f.backdrop.filter(|_| !(open && jelly)) {
        // `backdrop-filter`: the wallpaper under the card, blurred, under its translucent fill.
        out = out.child(
            el().absolute(window.x, window.y).size(window.w, window.h).clip_rounded(radius)
                .child(el().absolute(-(f.frame.x + window.x), -(f.frame.y + window.y)).size(canvas.0, canvas.1).pixels(backdrop)),
        );
    }
    out.child(card)
}

/// `toggleWidget`'s box: `size` centred on the tile, pushed inside the window with a 10 px
/// margin, and never so far that the tile would fall outside it.
pub fn surface_box(tile: Placement, (w, h): (f32, f32), canvas: (f32, f32)) -> Placement {
    let margin = 10.0;
    let mut cx = tile.x + tile.w / 2.0;
    let mut cy = tile.y + tile.h / 2.0;
    if cx - w / 2.0 < margin {
        cx += margin - (cx - w / 2.0);
    } else if cx + w / 2.0 > canvas.0 - margin {
        cx -= cx + w / 2.0 - (canvas.0 - margin);
    }
    if cy - h / 2.0 < margin {
        cy += margin - (cy - h / 2.0);
    } else if cy + h / 2.0 > canvas.1 - margin {
        cy -= cy + h / 2.0 - (canvas.1 - margin);
    }
    let x = (tile.x + tile.w - w).max((cx - w / 2.0).min(tile.x));
    let y = (tile.y + tile.h - h).max((cy - h / 2.0).min(tile.y));
    Placement { x, y, w, h }
}

/// A press handler that toggles card `id`.
pub fn toggle(id: &str) -> impl Fn(&mut Host, &mut Tx) + 'static {
    let id = id.to_string();
    move |host: &mut Host, tx: &mut Tx| host.toggle_card(&id, tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_grows_round_the_tile_and_stays_on_screen() {
        let canvas = (1280.0, 720.0);
        let middle = surface_box(Placement { x: 500.0, y: 300.0, w: 200.0, h: 100.0 }, (400.0, 420.0), canvas);
        assert_eq!((middle.x, middle.y), (400.0, 140.0));
        let corner = surface_box(Placement { x: 1200.0, y: 0.0, w: 70.0, h: 70.0 }, (400.0, 420.0), canvas);
        assert_eq!((corner.x, corner.y), (870.0, 0.0), "the tile stays inside the box");
    }

    #[test]
    fn a_strip_wears_the_strip_radius_in_both_materials() {
        let sea = Theme::normalize(&serde_json::json!({ "id": "sea-glass" }));
        let classic = Theme::normalize(&serde_json::json!({ "id": "nothing-classic" }));
        assert_eq!(tile_radius(&sea, true), sea.radius_strip);
        assert_eq!(tile_radius(&classic, true), classic.radius_strip, "the slider reaches flat glass too");
        assert_ne!(classic.radius_strip, classic.radius_card);
        assert_eq!(tile_radius(&sea, false), sea.radius_card);
        assert_eq!(tile_radius(&classic, false), classic.radius_card);
    }
}
