//! Arrange mode (`enterArrangeMode`, `setupDragReorder`, `snapMove`, `leaveArrangeMode` in
//! `unidtk-core.js`; `.arrange-bar` in `styles.css`): the island at the right (or left)
//! edge with the hint, the side toggle, "+ Папка", "+ Виджет", "Отменить" and "Готово";
//! cards dragged with a 10 px magnet to the screen edges and to each other at a 12 px gap, Alt
//! placing freely; resize handles; "×" taking a widget off the desk. Drafts stay in the host
//! until "Готово", which pins every card to its nearest corner and writes them in one
//! `desk_apply` from the revision the session started at (`r[desk.positions]`).

use std::collections::BTreeMap;

use rune::el::{col, el, row, text, El};
use rune::{Color, Tx};
use serde_json::{json, Value};

use crate::desk::Placement;

pub const SNAP_MAGNET: f32 = 10.0;
pub const SNAP_GAP: f32 = 12.0;
const MIN_SIDE: f32 = 64.0;
const MAX_SIDE: f32 = 800.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    Corner,
}

/// One arrange session.
pub struct Arrange {
    /// The layout revision the session started at; "Готово" is refused if it moved on.
    pub base_rev: u64,
    /// The whole layout as the session found it, for "Отменить".
    pub snapshot: Value,
    /// Where each card the host draws is now, by widget id.
    pub drafts: BTreeMap<String, Placement>,
    /// Folders created during the session; "Отменить" deletes them.
    pub created_folders: Vec<String>,
    pub side_right: bool,
    pub folder_form: bool,
    pub folder_name: rune::TextEdit,
    pub folder_icon: &'static str,
    pub add_popover: bool,
    /// A card is being dragged: the island steps aside.
    pub dragging: bool,
    /// The geometry a drag started from.
    pub grab: Option<Placement>,
}

impl Arrange {
    pub fn new(base_rev: u64, snapshot: Value, drafts: BTreeMap<String, Placement>, side_right: bool) -> Arrange {
        Arrange {
            base_rev,
            snapshot,
            drafts,
            created_folders: Vec::new(),
            side_right,
            folder_form: false,
            folder_name: rune::TextEdit::new(""),
            folder_icon: "folder",
            add_popover: false,
            dragging: false,
            grab: None,
        }
    }
}

/// `snapMove`: where a dragged card settles. The nearest screen edge (12 px in) or neighbour
/// line within the 10 px magnet, else where the pointer put it; never off the screen.
pub fn snap_move(id: &str, raw: (f32, f32), size: (f32, f32), canvas: (f32, f32), others: &BTreeMap<String, Placement>, free: bool) -> (f32, f32) {
    if free {
        return (raw.0.round(), raw.1.round());
    }
    let (w, h) = size;
    let mut xs = vec![SNAP_GAP, canvas.0 - w - SNAP_GAP];
    let mut ys = vec![SNAP_GAP, canvas.1 - h - SNAP_GAP];
    for (other_id, o) in others {
        if other_id == id || o.w <= 0.0 || o.h <= 0.0 {
            continue;
        }
        xs.extend([o.x, o.x + o.w - w, o.x + o.w + SNAP_GAP, o.x - SNAP_GAP - w]);
        ys.extend([o.y, o.y + o.h - h, o.y + o.h + SNAP_GAP, o.y - SNAP_GAP - h]);
    }
    let settle = |raw: f32, lines: &[f32]| {
        let (mut best, mut distance) = (raw, SNAP_MAGNET);
        for line in lines {
            let d = (line - raw).abs();
            if d < distance {
                best = *line;
                distance = d;
            }
        }
        best.round()
    };
    let clamp = |v: f32, max: f32| if max > 0.0 { v.clamp(0.0, max) } else { 0.0 };
    (clamp(settle(raw.0, &xs), canvas.0 - w), clamp(settle(raw.1, &ys), canvas.1 - h))
}

/// The resize of a card from `start` by the pointer's travel: one side, or the corner; Alt
/// stretches symmetrically round the centre. Sides stay within 64..800 px.
pub fn resize(start: Placement, edge: Edge, delta: (f32, f32), symmetric: bool) -> Placement {
    let k = if symmetric { 2.0 } else { 1.0 };
    let clamp = |v: f32| v.round().clamp(MIN_SIDE, MAX_SIDE);
    let mut next = start;
    match edge {
        Edge::Corner => {
            next.w = clamp(start.w + delta.0 * k);
            next.h = clamp(start.h + delta.1 * k);
            if symmetric {
                next.x = start.x - (next.w - start.w) / 2.0;
                next.y = start.y - (next.h - start.h) / 2.0;
            }
        }
        Edge::Left | Edge::Right => {
            let raw = if edge == Edge::Right { delta.0 } else { -delta.0 };
            next.w = clamp(start.w + raw * k);
            let grown = next.w - start.w;
            if symmetric {
                next.x = start.x - grown / 2.0;
            } else if edge == Edge::Left {
                next.x = start.x - grown;
            }
        }
        Edge::Top | Edge::Bottom => {
            let raw = if edge == Edge::Bottom { delta.1 } else { -delta.1 };
            next.h = clamp(start.h + raw * k);
            let grown = next.h - start.h;
            if symmetric {
                next.y = start.y - grown / 2.0;
            } else if edge == Edge::Top {
                next.y = start.y - grown;
            }
        }
    }
    next
}

/// `pinToNearestCorner`: the corner nearer the card's centre, with the distance to it.
pub fn pin(at: Placement, canvas: (f32, f32)) -> Value {
    let right = at.x + at.w / 2.0 > canvas.0 / 2.0;
    let bottom = at.y + at.h / 2.0 > canvas.1 / 2.0;
    let anchor = format!("{}{}", if bottom { "b" } else { "t" }, if right { "r" } else { "l" });
    json!({
        "x": at.x.round(), "y": at.y.round(), "w": at.w.round(), "h": at.h.round(),
        "anchor": anchor,
        "ax": if right { (canvas.0 - at.x - at.w).round() } else { at.x.round() },
        "ay": if bottom { (canvas.1 - at.y - at.h).round() } else { at.y.round() },
    })
}

/// Whether Alt is down now: the kernel's hook passes Alt to Windows, so the host reads it.
pub fn alt_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 }
}

/// `.resize-handle`: 4 px strips 2 px inside the card's sides (10 px short of the corners),
/// and a 12 px corner; the corner and the top and bottom strips only on special cards.
pub fn handles(look: &crate::controls::Look, at: Placement, special: bool) -> Vec<(Edge, El)> {
    let strip = |x: f32, y: f32, w: f32, h: f32| el().absolute(x, y).size(w, h).fill(look.elev.alpha(0.05)).hover(look.elev.alpha(0.15));
    let mut out = vec![
        (Edge::Left, strip(2.0, 10.0, 4.0, (at.h - 20.0).max(0.0))),
        (Edge::Right, strip(at.w - 6.0, 10.0, 4.0, (at.h - 20.0).max(0.0))),
    ];
    if special {
        out.push((Edge::Top, strip(10.0, 2.0, (at.w - 20.0).max(0.0), 4.0)));
        out.push((Edge::Bottom, strip(10.0, at.h - 6.0, (at.w - 20.0).max(0.0), 4.0)));
        out.push((Edge::Corner, el().absolute(at.w - 14.0, at.h - 14.0).size(12.0, 12.0).radius(2.0).fill(look.elev.alpha(0.3))));
    }
    out
}

/// `.widget-instance-remove`: a 22 px round "×" 5 px inside the card's top right.
pub fn remove_button(look: &crate::controls::Look, at: Placement, key: String) -> El {
    let cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
    col().key(key).label("Убрать виджет").absolute(at.w - 27.0, 5.0).size(22.0, 22.0).radius(11.0)
        .fill(look.scrim.alpha(0.48)).border(1.0, look.elev.alpha(0.18))
        .hover(look.status_error.alpha(0.72))
        .items_center().justify_center()
        .child(el().size(12.0, 12.0).shape(crate::controls::glyph(&cross, look.fg.alpha(0.82), 2.4)))
}

/// The dots of the arrange grid: one every 32 px in 22 % ink.
pub fn grid(look: &crate::controls::Look, canvas: (f32, f32)) -> El {
    let mut shape = rune::Shape::new();
    let mut y = 16.0;
    while y < canvas.1 {
        let mut x = 16.0;
        while x < canvas.0 {
            shape = shape.fill(
                (0..6).map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 6.0;
                    [x + 1.0 * a.cos(), y + 1.0 * a.sin()]
                }).collect::<Vec<_>>(),
                look.fg.alpha(0.22),
            );
            x += 32.0;
        }
        y += 32.0;
    }
    el().absolute(0.0, 0.0).size(canvas.0, canvas.1).shape(shape)
}

/// `.arrange-bar`: 188 px wide, 12 px from the right (or left) edge, its bottom 133 px below
/// the middle so the folder form grows upward; it slides 24 px in on `expoOut`.
pub fn bar_frame(look: &crate::controls::Look, theme: &crate::theme::Theme, canvas: (f32, f32), side_right: bool, dragging: bool, children: Vec<El>) -> El {
    let motion = theme.motion();
    let mut bar = col().key("hit:arrange-bar").label("Расстановка виджетов")
        .w(188.0).padding(14.0).gap(8.0).radius(16.0)
        .fill(look.surface.alpha(0.96)).border(1.0, look.elev.alpha(0.14))
        .shadow(18.0, 50.0, Color::BLACK.alpha(0.5))
        .opacity(if dragging { 0.3 } else { 1.0 })
        .opacity_motion(motion.tween(motion.duration(crate::motion::Token::Hover), crate::motion::STANDARD))
        .transition(rune::Transition::offset(if side_right { 24.0 } else { -24.0 }, 0.0).combined(rune::Transition::opacity()));
    for child in children {
        bar = bar.child(child);
    }
    // Hung from its bottom edge: a column the height of the canvas down to that edge.
    let bottom = canvas.1 / 2.0 + 133.0;
    let x = if side_right { canvas.0 - 12.0 - 188.0 } else { 12.0 };
    col().absolute(x, 0.0).w(188.0).h(bottom).justify_end().child(bar)
}

pub fn hint(look: &crate::controls::Look) -> El {
    el().mb(4.0).child(text("Перетаскивайте виджеты\nEsc — отменить").font(look.font(12.0, 400.0).lh(12.0 * 1.4)).color(look.fg.alpha(0.78)).wrap())
}

/// The folder icons the island offers (`.arrange-folder-icons`): four of the nine.
pub const BAR_FOLDER_ICONS: [(&str, &str); 4] = [("folder", "Папка"), ("gamepad", "Игры"), ("code", "Работа"), ("globe", "Сайты")];

pub fn row_of(children: Vec<El>) -> El {
    children.into_iter().fold(row().w_full().justify_between(), |r, c| r.child(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(x: f32, y: f32, w: f32, h: f32) -> Placement {
        Placement { x, y, w, h }
    }

    #[test]
    fn a_card_snaps_to_the_screen_edge_and_to_a_neighbour() {
        let canvas = (1280.0, 720.0);
        let mut others = BTreeMap::new();
        others.insert("glyph".to_string(), place(600.0, 100.0, 50.0, 200.0));
        // 5 px from the 12 px edge line: it snaps.
        assert_eq!(snap_move("clock", (17.0, 300.0), (200.0, 190.0), canvas, &others, false).0, 12.0);
        // Right of the neighbour, 12 px apart: 600 + 50 + 12 = 662.
        assert_eq!(snap_move("clock", (668.0, 400.0), (200.0, 190.0), canvas, &others, false).0, 662.0);
        // Out of the magnet: as the pointer put it, rounded and kept on screen.
        assert_eq!(snap_move("clock", (300.4, -40.0), (200.0, 190.0), canvas, &others, false), (300.0, 0.0));
        // Alt: free.
        assert_eq!(snap_move("clock", (17.0, 300.0), (200.0, 190.0), canvas, &others, true), (17.0, 300.0));
    }

    #[test]
    fn resizing_keeps_the_far_side_or_the_centre() {
        let start = place(100.0, 100.0, 200.0, 190.0);
        let left = resize(start, Edge::Left, (-20.0, 0.0), false);
        assert_eq!((left.x, left.w), (80.0, 220.0));
        let sym = resize(start, Edge::Corner, (10.0, 10.0), true);
        assert_eq!((sym.x, sym.y, sym.w, sym.h), (90.0, 90.0, 220.0, 210.0));
        assert_eq!(resize(start, Edge::Bottom, (0.0, -500.0), false).h, 64.0);
        assert_eq!(resize(start, Edge::Right, (900.0, 0.0), false).w, 800.0);
    }

    #[test]
    fn done_pins_each_card_to_its_nearest_corner() {
        let canvas = (1280.0, 720.0);
        assert_eq!(pin(place(1000.0, 500.0, 200.0, 100.0), canvas)["anchor"], "br");
        let tr = pin(place(1000.0, 10.0, 200.0, 100.0), canvas);
        assert_eq!((tr["anchor"].as_str(), tr["ax"].as_f64(), tr["ay"].as_f64()), (Some("tr"), Some(80.0), Some(10.0)));
        assert_eq!(pin(place(10.0, 10.0, 100.0, 100.0), canvas)["anchor"], "tl");
    }
}
