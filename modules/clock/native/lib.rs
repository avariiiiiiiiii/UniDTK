//! The clock's native half (`r[modules.vertical-slice]`): the widget `modules/clock/ui`
//! draws on the page, for the native desk, through the shell's widget contract
//! (`unidtk_shell::widget`). The shell draws the card (`"chrome": "bare"`), runs the morph
//! into the 400 x 420 dashboard and its nesting zone; the clock draws its face, flying it
//! from the tile into the dashboard, the dashboard itself, and the face switcher of arrange
//! mode.

mod dashboard;
mod faces;

use rune::el::{col, el, El};
use rune::Shape;
use serde_json::{json, Value};
use unidtk_shell::widget::{online, Act, Init, LocalTime, Material, Placement, Surface, SurfaceSize, View, Widget, WidgetType};

pub use faces::Face;

pub static WIDGET: WidgetType = WidgetType { id: "clock", events: &[], make };

fn make(init: &Init<'_>) -> Box<dyn Widget> {
    Box::new(Clock { face: face_of(init.record), online: true })
}

/// The face a record asks for: its `version`, else `digital`, as `renderClock` reads
/// `pos.version || "digital"` (only the module's default placement names `rings`).
pub fn face_of(record: &Value) -> Face {
    record.get("version").and_then(Value::as_str).map_or(Face::Digital, Face::parse)
}

/// The switcher's order (`widget.js`).
const FACES: [&str; 13] = ["digital", "gooey", "digital-stacked", "digital-vertical", "analogue", "analogue-dot", "analogue-bezel", "analogue-cmf", "rings", "minimal-lines", "grid-second", "nothing-watch-pro", "gothic"];

/// The face after `current` in the switcher's order; a name it does not know starts it over.
pub fn next_face(current: &Value) -> &'static str {
    let name = current.as_str().unwrap_or("digital");
    FACES.iter().position(|f| *f == name).map_or(FACES[0], |i| FACES[(i + 1) % FACES.len()])
}

struct Clock {
    face: Face,
    /// `navigator.onLine`, read as the dashboard opens.
    online: bool,
}

/// The dashboard's layout in the box: `.clock-dash-layout` under the expanded card's 1 px
/// border, its 16 px body padding, the no-header card's 10 px top and the body's 2 px, with
/// the layout's own 10 px; the columns share the width at 1.2 : 1 with a 20 px gap.
struct Dash {
    content_x: f32,
    content_w: f32,
    layout_y: f32,
    left: f32,
    right: f32,
    row_h: f32,
}

impl Dash {
    fn of(v: &View<'_>) -> (Dash, El, f32) {
        let content_w = v.frame.w - 2.0 - 32.0;
        let left = (content_w - 20.0) * 1.2 / 2.2;
        let right = content_w - 20.0 - left;
        let (calendar, calendar_h) = dashboard::calendar(v.now, v.theme, v.ink, right);
        let right_h = calendar_h + 15.0 + 50.0;
        (Dash { content_x: 17.0, content_w, layout_y: 23.0, left, right, row_h: right_h.max(240.0) }, calendar, right_h)
    }

    /// `.dash-clock`: 180 px with 10 px of padding, centred in the left column.
    fn face_frame(&self) -> faces::Frame {
        let (cx, cy) = (self.content_x + self.left / 2.0, self.layout_y + self.row_h / 2.0);
        faces::Frame { x: cx - 80.0, y: cy - 80.0, w: 160.0, h: 160.0 }
    }
}

/// The tile's `.clock-container`: the card under its 10 px top padding with 10 px of its own,
/// inside a flat card's 1 px border (a jelly card has none), at least 140 px high.
fn tile_frame(v: &View<'_>) -> faces::Frame {
    let border = if v.theme.material == Material::Flat { 1.0 } else { 0.0 };
    let t = v.tile;
    faces::Frame { x: t.x + 10.0 + border, y: t.y + 20.0 + border, w: (t.w - 20.0 - 2.0 * border).max(0.0), h: (t.h - 30.0 - 2.0 * border).max(140.0) }
}

impl Widget for Clock {
    fn sync(&mut self, record: &Value) {
        self.face = face_of(record);
    }

    fn content(&self, v: &View<'_>) -> Option<El> {
        let tile = tile_frame(v);
        let moving = v.open || v.progress > 0.0;
        let mut out = el().absolute(0.0, 0.0).size(v.frame.w, v.frame.h);
        let frame = if moving {
            let (dash, calendar, right_h) = Dash::of(v);
            let to = dash.face_frame();
            let at = v.fly(Placement { x: tile.x, y: tile.y, w: tile.w, h: tile.h }, Placement { x: to.x, y: to.y, w: to.w, h: to.h });
            // The calendar and the badge sit in the box; the clip reveals them.
            out = out.child(
                col().absolute(dash.content_x + dash.left + 20.0, dash.layout_y + (dash.row_h - right_h) / 2.0)
                    .w(dash.right)
                    .gap(15.0)
                    .child(calendar)
                    .child(dashboard::internet(self.online, v.theme, v.ink, dash.right)),
            );
            faces::Frame { x: at.x, y: at.y, w: at.w, h: at.h }
        } else {
            tile
        };
        Some(out.child(faces::face(self.face, v.now, frame, v.theme, v.ink, v.progress >= 0.5)))
    }

    fn surface(&self, v: &View<'_>) -> Option<Surface> {
        let (dash, _, _) = Dash::of(&v.with_frame(Placement { w: 400.0, h: 420.0, ..v.frame }));
        Some(Surface { size: SurfaceSize::Box(400.0, 420.0), nest: Some([dash.content_x, dash.layout_y + dash.row_h + 10.0, dash.content_w]) })
    }

    /// `.version-switcher`: 28 px, 14 px inside the tile's top right, shown in arrange mode; a
    /// press steps to the next face and stores it.
    fn arrange_tools(&self, v: &View<'_>) -> Vec<El> {
        let theme = v.theme;
        // `opacity var(--dur-transient) ease` in and out with arrange mode; the hover background
        // has no transition, so it lands at once.
        let fade = v.motion.presence(rune::Transition::opacity(), unidtk_shell::widget::Token::Transient, unidtk_shell::motion::EASE, Some(unidtk_shell::motion::EASE));
        // `M21.5 2v6h-6M2.5 22v-6h6M2 12c0-4.4 3.6-8 8-8 2.2 0 4.2.9 5.7 2.3L21.5 8M22 12c0 4.4-3.6 8-8 8-2.2 0-4.2-.9-5.7-2.3L2.5 16`.
        let mut icon = Shape::new().view_box(24.0, 24.0);
        for path in faces::svg_path("M21.5 2V8H15.5M2.5 22V16H8.5M2 12C2 7.6 5.6 4 10 4C12.2 4 14.2 4.9 15.7 6.3L21.5 8M22 12C22 16.4 18.4 20 14 20C11.8 20 9.8 19.1 8.3 17.7L2.5 16") {
            icon = icon.stroke_with(path.0, path.1, 2.0, rune::LineCap::Round, v.ink);
        }
        vec![col().key(format!("hit:clock-face:{}", v.id)).label("Сменить стиль")
            .absolute(v.tile.w - 14.0 - 28.0, 14.0).size(28.0, 28.0).radius(8.0)
            .fill(theme.bg_elevated.alpha(0.1)).border(1.0, theme.bg_elevated.alpha(0.1))
            .hover_fill(theme.bg_elevated.alpha(0.2)).hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
            .transition(fade)
            .items_center().justify_center()
            .child(el().size(14.0, 14.0).shape(icon))
            .on_press_tx(v.on(|clock: &mut Clock, act: &mut Act<'_>| {
                let next = next_face(&json!(face_name(clock.face)));
                clock.face = Face::parse(next);
                act.save(json!({ "version": next }));
            }))]
    }

    fn wake(&self, now: &LocalTime) -> Option<f64> {
        Some(if self.face.ticks_each_second() { now.to_next_second() } else { now.to_next_minute() })
    }

    fn tick(&mut self, _now: &LocalTime, _act: &mut Act<'_>) -> bool {
        true
    }

    fn shown(&mut self, visible: bool, _act: &mut Act<'_>) {
        if visible {
            self.online = online();
        }
    }
}

fn face_name(face: Face) -> &'static str {
    FACES.iter().copied().find(|name| Face::parse(name) == face).unwrap_or("digital")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_and_their_fallbacks() {
        assert_eq!(face_of(&json!({ "x": 10 })), Face::Digital, "pos.version || \"digital\"");
        assert_eq!(face_of(&json!({ "version": "gooey" })), Face::Gooey);
        assert_eq!(face_of(&json!({ "version": "neon" })), Face::Unknown);
        let default: Value = serde_json::from_str(include_str!("../ui/metadata.json")).unwrap();
        assert_eq!(face_of(&default["defaultPlacement"]), Face::Rings, "the module's default placement");
        assert_eq!(default["chrome"], "bare", "the clock draws its whole card");
    }

    #[test]
    fn the_switcher_steps_through_all_thirteen_and_wraps() {
        assert_eq!(next_face(&json!("digital")), "gooey");
        assert_eq!(next_face(&json!("gothic")), "digital");
        assert_eq!(next_face(&json!("neon")), "digital");
        let mut seen = vec!["digital"];
        while seen.len() < 14 {
            seen.push(next_face(&json!(seen.last().unwrap())));
        }
        assert_eq!(seen.last(), Some(&"digital"));
        assert!(FACES.iter().all(|f| face_name(Face::parse(f)) == *f));
    }

    #[test]
    fn the_clock_wakes_by_its_face() {
        let now = LocalTime { s: 50, nanos: 0, ..Default::default() };
        let rings = Clock { face: Face::Rings, online: true };
        let digital = Clock { face: Face::Digital, online: true };
        assert!((rings.wake(&now).unwrap() - 1.005).abs() < 1e-9);
        assert!((digital.wake(&now).unwrap() - 10.005).abs() < 1e-9);
    }
}
