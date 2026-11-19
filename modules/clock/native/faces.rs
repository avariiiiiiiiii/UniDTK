//! The clock's thirteen faces, drawn as `modules/clock/ui/widget.js` builds them and
//! `clock.css` / `gooey.css` style them. A face is laid out in the content box of its
//! `.clock-container` (the tile's, or the dashboard's 160 x 160 `.dash-clock`, or a frame
//! between the two while it flies).
//!
//! Hands, rings and ellipses are shapes rather than rotated boxes: a hand turns about the
//! dial's centre, which a box would only do round its own, and a `border-radius: 50%` on a
//! box that is not square is an ellipse, which Rune's corner radius is not.

use rune::el::{col, el, text, El};
use rune::{Color, LineCap, Shape, TextStyle};

use unidtk_shell::widget::{LocalTime as ClockTime, Material, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Digital,
    Gooey,
    DigitalStacked,
    DigitalVertical,
    Analogue,
    AnalogueDot,
    AnalogueBezel,
    AnalogueCmf,
    Rings,
    MinimalLines,
    GridSecond,
    NothingWatchPro,
    Gothic,
    /// A name the page does not know: `updateClockFace` leaves the container empty.
    Unknown,
}

impl Face {
    pub fn parse(name: &str) -> Face {
        match name {
            "digital" => Face::Digital,
            "gooey" => Face::Gooey,
            "digital-stacked" => Face::DigitalStacked,
            "digital-vertical" => Face::DigitalVertical,
            "analogue" => Face::Analogue,
            "analogue-dot" => Face::AnalogueDot,
            "analogue-bezel" => Face::AnalogueBezel,
            "analogue-cmf" => Face::AnalogueCmf,
            "rings" => Face::Rings,
            "minimal-lines" => Face::MinimalLines,
            "grid-second" => Face::GridSecond,
            "nothing-watch-pro" => Face::NothingWatchPro,
            "gothic" => Face::Gothic,
            _ => Face::Unknown,
        }
    }

    /// Faces that show seconds change every second; the rest once a minute.
    pub fn ticks_each_second(self) -> bool {
        matches!(
            self,
            Face::Rings | Face::DigitalVertical | Face::Analogue | Face::AnalogueBezel | Face::AnalogueCmf | Face::GridSecond | Face::NothingWatchPro
        )
    }
}

/// The content box of a `.clock-container`, in the card's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What every face paints with: `--fg-rgb`, `--bg-elev-rgb`, `--theme-accent`, the fonts.
struct Ink<'a> {
    theme: &'a Theme,
    fg: Color,
    elev: Color,
    accent: Color,
    /// `.dash-clock` sizes some text differently (`gooey`'s date).
    in_dashboard: bool,
}

fn display(theme: &Theme, size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).family(Theme::family(theme.font_display)).lh_normal()
}

fn body(theme: &Theme, size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).family(Theme::family(theme.font_body)).lh_normal()
}

/// `.clock-digital`: the display font with 2 px of letter spacing; under flat material a
/// `0 0 20px rgba(255, 255, 255, 0.15)` glow, which jelly turns off.
fn clock_digital(ink: &Ink<'_>, content: String, size: f32, colour: Color) -> El {
    let style = display(ink.theme, size).tracking(2.0);
    let glyphs = text(content.clone()).font(style).color(colour).single_line();
    if ink.theme.material != Material::Flat {
        return glyphs;
    }
    el().child(
        el().absolute(0.0, 0.0).w_full().h_full().blur(10.0)
            .child(text(content).font(style).color(Color::WHITE.alpha(0.15)).single_line()),
    )
    .child(glyphs)
}

/// An element of `w` x `h` centred on `(cx, cy)` whose child is centred in it.
fn centred(cx: f32, cy: f32, w: f32, h: f32, child: El) -> El {
    col().absolute(cx - w / 2.0, cy - h / 2.0).size(w, h).items_center().justify_center().child(child)
}

fn ellipse_points(cx: f32, cy: f32, rx: f32, ry: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..steps)
        .map(|step| {
            let angle = std::f32::consts::TAU * step as f32 / steps as f32;
            [cx + rx * angle.sin(), cy - ry * angle.cos()]
        })
        .collect()
}

/// A `.clock-analogue` dial: an ellipse of 85 % of the box (at most 200 px a side) with a
/// border drawn inside its edge and an optional fill under it (CSS paints a background under
/// the border). Its absolutely placed children (hands, dots, texts) resolve their percentages
/// against the padding box, the dial less its border on each side.
struct Dial {
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    border: f32,
}

impl Dial {
    fn in_frame(frame: Frame, border: f32) -> Dial {
        Dial { cx: frame.x + frame.w / 2.0, cy: frame.y + frame.h / 2.0, w: (frame.w * 0.85).min(200.0), h: (frame.h * 0.85).min(200.0), border }
    }

    /// The padding box's left edge, top edge, width and height.
    fn left(&self) -> f32 {
        self.cx - self.w / 2.0 + self.border
    }

    fn top(&self) -> f32 {
        self.cy - self.h / 2.0 + self.border
    }

    fn inner_w(&self) -> f32 {
        self.w - 2.0 * self.border
    }

    fn inner_h(&self) -> f32 {
        self.h - 2.0 * self.border
    }

    fn body(&self, shape: Shape, border_colour: Color, fill: Option<Color>) -> Shape {
        let shape = match fill {
            Some(fill) => shape.fill(ellipse_points(self.cx, self.cy, self.w / 2.0, self.h / 2.0, 96), fill),
            None => shape,
        };
        if self.border > 0.0 {
            let points = ellipse_points(self.cx, self.cy, (self.w - self.border) / 2.0, (self.h - self.border) / 2.0, 96);
            shape.stroke_with(points, true, self.border, LineCap::Butt, border_colour)
        } else {
            shape
        }
    }

    /// `.hand`: a bar of `width` standing on the centre, `share` of the padding box's height
    /// long, turned `degrees` clockwise round its foot; its 4 px corners round both ends.
    fn hand(&self, shape: Shape, degrees: f32, width: f32, share: f32, colour: Color) -> Shape {
        let length = self.inner_h() * share;
        let (sin, cos) = degrees.to_radians().sin_cos();
        let at = |distance: f32| [self.cx + distance * sin, self.cy - distance * cos];
        let inset = (width / 2.0).min(length / 2.0);
        shape.line(at(inset), at(length - inset), width, LineCap::Round, colour)
    }

    fn dot(&self, shape: Shape, x: f32, y: f32, diameter: f32, colour: Color) -> Shape {
        shape.fill(ellipse_points(x, y, diameter / 2.0, diameter / 2.0, 24), colour)
    }
}

fn hand_angles(time: ClockTime) -> (f32, f32, f32) {
    ((time.h % 12) as f32 * 30.0 + time.m as f32 * 0.5, time.m as f32 * 6.0, time.s as f32 * 6.0)
}

fn two(value: u32) -> String {
    format!("{value:02}")
}

const RING: f32 = 120.0;
const RING_WIDTH: f32 = 4.0;

/// `.clock-rings .ring`: a circle with a 4 px border, faint all round and coloured on the top
/// side. With four equal border widths and a 50 % radius the top side is the quarter between
/// the 45-degree joins; the ring turns by whole steps of 6 degrees.
fn ring(shape: Shape, centre: [f32; 2], diameter: f32, turn: f32, track: Color, colour: Color) -> Shape {
    let r = diameter / 2.0 - RING_WIDTH / 2.0;
    let arc: Vec<[f32; 2]> = (0..=30)
        .map(|step| {
            let angle = (turn - 45.0 + 3.0 * step as f32).to_radians();
            [centre[0] + r * angle.sin(), centre[1] - r * angle.cos()]
        })
        .collect();
    shape
        .stroke_with(ellipse_points(centre[0], centre[1], r, r, 72), true, RING_WIDTH, LineCap::Butt, track)
        .stroke_with(arc, false, RING_WIDTH, LineCap::Butt, colour)
}

/// Draws `face` in `frame`. `fg` is the card's ink (`--fg-rgb`). While the card opens or
/// folds, `frame` is the one the morph's progress gives between the tile's container and the
/// dashboard's (`View::fly`), so the face flies (`r[motion.shared-flight]`).
pub fn face(face: Face, time: ClockTime, frame: Frame, theme: &Theme, fg: Color, in_dashboard: bool) -> El {
    let ink = Ink { theme, fg, elev: theme.bg_elevated, accent: theme.accent, in_dashboard };
    let (cx, cy) = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
    let hm = format!("{}:{}", two(time.h), two(time.m));
    let mut out = el().key("clock-face").absolute(0.0, 0.0).w_full().h_full();
    let (hour, minute, second) = hand_angles(time);
    match face {
        Face::Unknown => {}
        Face::Digital => out = out.child(centred(cx, cy, frame.w + 200.0, frame.h, clock_digital(&ink, hm, 56.0, fg))),
        Face::Rings => {
            let track = ink.elev.alpha(0.05);
            let c = [cx, cy];
            let shape = ring(Shape::new(), c, RING, (time.m * 6) as f32, track, fg);
            let shape = ring(shape, c, RING * 0.7, (time.s * 6) as f32, track, ink.accent);
            out = out
                .child(el().absolute(0.0, 0.0).w_full().h_full().shape(shape))
                .child(centred(cx, cy, RING + 120.0, RING, text(hm).font(display(theme, 19.2)).color(fg).single_line()));
        }
        Face::DigitalStacked => {
            // `line-height: 0.9` on 3.5rem; the minutes at 40 %.
            let line = 56.0 * 0.9;
            let stack = col()
                .items_center()
                .child(el().h(line).child(text(two(time.h)).font(display(theme, 56.0).lh(line)).color(fg).single_line()))
                .child(el().h(line).child(text(two(time.m)).font(display(theme, 56.0).lh(line)).color(fg.alpha(0.4)).single_line()));
            out = out.child(centred(cx, cy, frame.w, frame.h + 40.0, stack));
        }
        Face::DigitalVertical => {
            let big = display(theme, 35.2).lh(35.2);
            let stack = col()
                .gap(4.0)
                .child(text(two(time.h)).font(big).color(fg).single_line())
                .child(text(two(time.m)).font(big).color(fg).single_line())
                .child(text(two(time.s)).font(display(theme, 16.0).lh(16.0)).color(ink.accent).single_line().self_end());
            out = out.child(centred(cx, cy, frame.w, frame.h + 60.0, stack));
        }
        Face::Analogue | Face::AnalogueCmf => {
            let dial = Dial::in_frame(frame, 1.5);
            let orange = rune::color::hex("#ff6e00").unwrap_or(ink.accent);
            let (second_colour, centre_colour) = if face == Face::AnalogueCmf { (orange, orange) } else { (ink.accent, ink.elev) };
            let shape = dial.body(Shape::new(), ink.elev.alpha(0.15), None);
            let shape = dial.hand(shape, minute, 3.0, 0.38, ink.elev.alpha(0.7));
            let shape = dial.hand(shape, hour, 4.0, 0.25, ink.elev);
            let shape = dial.hand(shape, second, 1.5, 0.42, second_colour);
            let shape = dial.dot(shape, dial.cx, dial.cy, 6.0, centre_colour);
            out = out.child(el().absolute(0.0, 0.0).w_full().h_full().shape(shape));
        }
        Face::AnalogueBezel => {
            let dial = Dial::in_frame(frame, 4.0);
            let shape = dial.body(Shape::new(), ink.elev.alpha(0.1), Some(ink.elev.alpha(0.02)));
            let shape = dial.hand(shape, minute, 4.0, 0.38, ink.elev);
            let shape = dial.hand(shape, hour, 6.0, 0.25, ink.elev);
            let shape = dial.hand(shape, second, 1.5, 0.42, ink.accent);
            let shape = dial.dot(shape, dial.cx, dial.cy, 6.0, ink.elev);
            out = out.child(el().absolute(0.0, 0.0).w_full().h_full().shape(shape));
        }
        Face::MinimalLines => {
            let dial = Dial::in_frame(frame, 0.0);
            let shape = dial.hand(Shape::new(), minute, 6.0, 0.40, fg);
            let shape = dial.hand(shape, hour, 6.0, 0.25, fg);
            out = out.child(el().absolute(0.0, 0.0).w_full().h_full().shape(shape));
        }
        Face::AnalogueDot => {
            let dial = Dial::in_frame(frame, 1.0);
            let grey = rune::color::hex("#333333").unwrap_or(Color::BLACK);
            let shape = dial.body(Shape::new(), ink.elev.alpha(0.05), Some(ink.elev.alpha(0.03)));
            // `::before`: `inset: 5%` of the padding box, a disc of rgba(170, 170, 170, 0.8).
            let disc = shape.fill(ellipse_points(dial.cx, dial.cy, dial.inner_w() * 0.45, dial.inner_h() * 0.45, 96), Color { a: 0.8, ..rune::color::hex("#aaaaaa").unwrap_or(Color::BLACK) });
            // Over the texts (z-index 5): the red dot (6), the minute (7) and hour (8) hands and
            // the centre (10), in that order.
            // The red dot's box: top at 38 %, right edge at 25 % from the right.
            let shape = dial.dot(Shape::new(), dial.left() + dial.inner_w() * 0.75 - 3.0, dial.top() + dial.inner_h() * 0.38 + 3.0, 6.0, ink.accent);
            let shape = dial.hand(shape, minute, 2.0, 0.38, fg.alpha(0.33));
            let shape = dial.hand(shape, hour, 4.0, 0.28, fg.alpha(0.2));
            let shape = dial.dot(shape, dial.cx, dial.cy, 8.0, fg.alpha(0.2));
            let stats = col()
                .child(text("СБ 23").font(body(theme, 8.0, 600.0).lh(11.2)).color(grey).single_line())
                .child(text("156 ккал").font(body(theme, 8.0, 600.0).lh(11.2)).color(grey).single_line())
                .child(text("6 240 шагов").font(body(theme, 8.0, 600.0).lh(11.2)).color(grey).single_line());
            let stats_h = 3.0 * 11.2;
            out = out
                .child(el().absolute(0.0, 0.0).w_full().h_full().shape(disc))
                // `.dot-digital-overlay` centred at 38 % of the dial's height.
                .child(centred(dial.cx, dial.top() + dial.inner_h() * 0.38, dial.w, 20.0, clock_digital(&ink, hm, 11.2, grey)))
                // `.dot-stats-overlay`: bottom at 22 %, centred as a block.
                .child(centred(dial.cx, dial.top() + dial.inner_h() * 0.78 - stats_h / 2.0, dial.w, stats_h, stats))
                .child(el().absolute(0.0, 0.0).w_full().h_full().shape(shape));
        }
        Face::GridSecond => {
            // The digits sit in the container; the 140 px dashed dial is centred over them, and
            // its 4 x 8 px hand at the top turns round its own foot, as `rotate()` with
            // `transform-origin: bottom center` turns it.
            // `bottom: 95%` of the padding box inside the 2 px dashed border.
            let side = 140.0;
            let (left, top) = (cx - side / 2.0, cy - side / 2.0);
            let foot = [cx, top + 2.0 + (side - 4.0) * 0.05];
            let (sin, cos) = second.to_radians().sin_cos();
            let tip = [foot[0] + 8.0 * sin, foot[1] - 8.0 * cos];
            let inset = 2.0;
            let hand = Shape::new().line(
                [foot[0] + inset * sin, foot[1] - inset * cos],
                [tip[0] - inset * sin, tip[1] + inset * cos],
                4.0,
                LineCap::Round,
                fg,
            );
            out = out
                .child(centred(cx, cy, frame.w + 200.0, frame.h, clock_digital(&ink, hm, 40.0, fg)))
                .child(el().absolute(left, top).size(side, side).radius(side / 2.0).border(2.0, ink.elev.alpha(0.1)).border_dash(6.0, 6.0))
                .child(el().absolute(0.0, 0.0).w_full().h_full().shape(hand));
        }
        Face::NothingWatchPro => {
            let dial = Dial::in_frame(frame, 2.0);
            let face_shape = dial.body(Shape::new(), ink.elev.alpha(0.13), Some(theme.scrim));
            let shape = dial.hand(Shape::new(), hour, 3.0, 0.20, fg);
            let shape = dial.hand(shape, minute, 3.0, 0.30, fg);
            let shape = dial.dot(shape, dial.left() + dial.inner_w() * 0.8 - 3.0, dial.top() + dial.inner_h() * 0.5 + 3.0, 6.0, ink.accent);
            let hms = format!("{}:{}:{}", two(time.h), two(time.m), two(time.s));
            // Both texts are absolute without a left offset: they keep their static place at
            // the dial's left content edge. In document order they paint before the hands
            // (z-index 3 and 2) and the red dot (6), which go over them.
            let label_line = 9.6 * Theme::normal_line(theme.font_display);
            out = out
                .child(el().absolute(0.0, 0.0).w_full().h_full().shape(face_shape))
                .child(el().absolute(dial.left(), dial.top() + dial.inner_h() * 0.2).child(clock_digital(&ink, hms, 19.2, fg)))
                .child(
                    el().absolute(dial.left(), dial.top() + dial.inner_h() * 0.85 - label_line)
                        .child(text("CMF BY NOTHING").font(display(theme, 9.6)).color(ink.accent).single_line()),
                )
                .child(el().absolute(0.0, 0.0).w_full().h_full().shape(shape));
        }
        Face::Gooey => out = out.child(gooey(&ink, time, frame)),
        Face::Gothic => out = out.child(gothic(&ink, time, frame)),
    }
    out
}

/// `gooey.js`: the date over four stepped digits in the accent colour, the digits a 278 x 80
/// SVG as wide as the container. Each digit is its own element keyed by its value: a digit
/// that changes enters as `gooey-clock-settle` does (over the transient token, from 55 % and
/// 96 %, on the standard easing), and the old one leaves at once, as `updateGooeyClock` replaces its ink.
fn gooey(ink: &Ink<'_>, time: ClockTime, frame: Frame) -> El {
    let scale = frame.w.min(560.0) / 278.0;
    let digits = format!("{}{}", two(time.h), two(time.m));
    let settle = ink.theme.motion().presence(
        rune::Transition {
            insertion: rune::Effect::opacity(0.55).combined(rune::Effect::scale(0.96)),
            removal: rune::Effect::IDENTITY,
            ..rune::Transition::identity()
        },
        unidtk_shell::widget::Token::Transient,
        unidtk_shell::motion::STANDARD,
        None,
    );
    let digits_h = 80.0 * scale;
    let mut row_el = el().size(278.0 * scale, digits_h);
    for (slot, digit) in digits.chars().enumerate() {
        let place = |p: [f32; 2]| [(p[0] + 2.0) * scale, (p[1] + 2.0) * scale];
        let place_all = |points: Vec<[f32; 2]>| points.into_iter().map(place).collect::<Vec<_>>();
        let mut shape = Shape::new();
        match digit {
            '0' => {
                let rings = svg_path(GOOEY_ZERO);
                if let [outer, inner, ..] = &rings[..] {
                    for half in ring_halves(&outer.0, &inner.0, 32.0) {
                        shape = shape.fill(place_all(half), ink.accent);
                    }
                }
                shape = shape.fill(place_all(ellipse_points(32.0, 38.0, 6.2, 6.2, 24)), ink.accent);
            }
            '1' => {
                for (points, _) in svg_path(GOOEY_ONE) {
                    shape = shape.fill(place_all(points), ink.accent);
                }
            }
            other => {
                let index = other.to_digit(10).unwrap_or(2) as usize;
                for (points, closed) in svg_path(GOOEY_STROKES[index - 2]) {
                    shape = shape.stroke_with(place_all(points), closed, 14.0 * scale, LineCap::Round, ink.accent);
                }
            }
        }
        row_el = row_el.child(
            el().key(format!("gooey:{slot}:{digit}")).absolute(slot as f32 * 70.0 * scale, 0.0).size(70.0 * scale, digits_h)
                .shape(shape).transition(settle),
        );
    }
    let size = if ink.in_dashboard { 13.0 } else { 16.0 };
    // `.gooey-clock-date { line-height: 1.25 }`.
    let total = size * 1.25 + 12.0 + digits_h;
    col()
        .absolute(frame.x, frame.y + (frame.h - total) / 2.0)
        .w(frame.w)
        .items_center()
        .gap(12.0)
        .child(text(short_date(time, false)).font(TextStyle::new(size, 500.0).family(rune::Family::Named("Segoe UI")).lh(size * 1.25)).color(ink.accent).single_line())
        .child(row_el)
}

/// `toLocaleDateString('ru-RU', { weekday: 'short', day, month: 'short' })` with the dots
/// removed: "вс, 5 нояб" for `gooey`'s `day: 'numeric'`, "вс, 05 нояб" for `gothic`'s
/// `day: '2-digit'`.
pub fn short_date(time: ClockTime, two_digit_day: bool) -> String {
    const DAYS: [&str; 7] = ["вс", "пн", "вт", "ср", "чт", "пт", "сб"];
    const MONTHS: [&str; 12] = ["янв", "февр", "мар", "апр", "мая", "июн", "июл", "авг", "сент", "окт", "нояб", "дек"];
    let weekday = chrono::NaiveDate::from_ymd_opt(time.year, time.month, time.day).map_or(0, |date| {
        use chrono::Datelike;
        date.weekday().num_days_from_sunday() as usize
    });
    let day = if two_digit_day { two(time.day) } else { time.day.to_string() };
    format!("{}, {} {}", DAYS[weekday], day, MONTHS[(time.month.clamp(1, 12) - 1) as usize])
}

/// The `gothic` face as the page draws it: its SVG and texts have no styles in `clock.css`,
/// so the paths take SVG's default black fill and no stroke, the SVG fills the container's
/// width, and the time and the date follow it in the flow.
fn gothic(ink: &Ink<'_>, time: ClockTime, frame: Frame) -> El {
    let scale = frame.w / 190.0;
    let place_all = |points: Vec<[f32; 2]>| points.into_iter().map(|p| [p[0] * scale, p[1] * scale]).collect::<Vec<_>>();
    let mut shape = Shape::new();
    for path in GOTHIC_PATHS {
        for (points, _) in svg_path(path) {
            if points.len() >= 3 {
                shape = shape.fill(place_all(points), Color::BLACK);
            }
        }
    }
    shape = shape.fill(place_all(ellipse_points(95.0, 105.0, 2.5, 2.5, 16)), Color::BLACK);
    let date = short_date(time, true);
    let height = 170.0 * scale + 56.0 * Theme::normal_line(ink.theme.font_display) + 16.0 * Theme::normal_line(ink.theme.font_body);
    col()
        .absolute(frame.x, frame.y + (frame.h - height) / 2.0)
        .w(frame.w)
        .child(el().size(frame.w, 170.0 * scale).shape(shape))
        .child(clock_digital(ink, format!("{}:{}", two(time.h), two(time.m)), 56.0, ink.fg))
        .child(text(date).font(body(ink.theme, 16.0, 400.0)).color(ink.fg).single_line())
}

const GOOEY_ZERO: &str = "M20 2H44Q50 2 50 8Q50 14 57 14Q64 14 64 21V55Q64 62 57 62Q50 62 50 68Q50 74 44 74H20Q14 74 14 68Q14 62 7 62Q0 62 0 55V21Q0 14 7 14Q14 14 14 8Q14 2 20 2Z M20 17Q14 17 14 23V53Q14 59 20 59H44Q50 59 50 53V23Q50 17 44 17Z";
const GOOEY_ONE: &str = "M31 2Q38 2 38 9V56Q38 62 45 62H47Q54 62 54 68Q54 74 47 74H17Q10 74 10 68Q10 62 17 62H19Q24 62 24 56V33Q24 29 19 29Q10 29 10 22Q10 16 17 16Q24 16 24 9Q24 2 31 2Z";
const GOOEY_STROKES: [&str; 8] = [
    "M9 19Q9 9 20 9H44Q55 9 55 19V24Q55 34 43 36L25 40Q12 43 9 55L9 67H55",
    "M9 9H44Q55 9 55 20V24Q55 38 41 38H24M41 38Q55 38 55 51V56Q55 67 44 67H9",
    "M17 9L9 40Q8 46 16 46H55M46 9V67",
    "M55 9H9V34H43Q55 34 55 46V55Q55 67 43 67H9",
    "M53 9H23Q9 9 9 24V54Q9 67 22 67H43Q55 67 55 54V48Q55 35 43 35H9",
    "M9 9H55V20Q55 29 44 33Q33 38 32 49L30 67",
    "M23 9H41Q54 9 54 21V25Q54 38 41 38H23Q10 38 10 25V21Q10 9 23 9Z M23 38H41Q55 38 55 51V55Q55 67 41 67H23Q9 67 9 55V51Q9 38 23 38Z",
    "M11 67H41Q55 67 55 52V22Q55 9 42 9H21Q9 9 9 22V28Q9 41 21 41H55",
];
const GOTHIC_PATHS: [&str; 6] = [
    "M24 151V69C24 38 51 19 95 7C139 19 166 38 166 69V151",
    "M34 147V72C34 47 56 31 95 19C134 31 156 47 156 72V147",
    "M18 151H172M35 158H155",
    "M24 69C43 67 57 57 64 39M166 69C147 67 133 57 126 39",
    "M95 5L90 16L95 12L100 16Z",
    "M55 105H78M112 105H135",
];

/// The absolute SVG path commands the faces use (`M L H V Q C Z`) as flattened subpaths,
/// each with whether it was closed.
pub fn svg_path(data: &str) -> Vec<(Vec<[f32; 2]>, bool)> {
    let mut tokens = Vec::new();
    let mut number = String::new();
    for c in data.chars() {
        if c.is_ascii_alphabetic() {
            if !number.is_empty() {
                tokens.push(std::mem::take(&mut number));
            }
            tokens.push(c.to_string());
        } else if c == ' ' || c == ',' {
            if !number.is_empty() {
                tokens.push(std::mem::take(&mut number));
            }
        } else if c == '-' && !number.is_empty() {
            tokens.push(std::mem::take(&mut number));
            number.push(c);
        } else {
            number.push(c);
        }
    }
    if !number.is_empty() {
        tokens.push(number);
    }
    let mut out: Vec<(Vec<[f32; 2]>, bool)> = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    let mut at = [0.0f32, 0.0];
    let mut command = 'M';
    let mut index = 0;
    let take = |index: &mut usize| -> f32 {
        let value = tokens.get(*index).and_then(|t| t.parse().ok()).unwrap_or(0.0);
        *index += 1;
        value
    };
    while index < tokens.len() {
        if let Some(letter) = tokens[index].chars().next().filter(char::is_ascii_alphabetic) {
            command = letter;
            index += 1;
            if command == 'Z' {
                if current.len() > 1 {
                    out.push((std::mem::take(&mut current), true));
                }
                continue;
            }
        }
        match command {
            'M' => {
                if current.len() > 1 {
                    out.push((std::mem::take(&mut current), false));
                }
                current.clear();
                at = [take(&mut index), take(&mut index)];
                current.push(at);
                command = 'L';
            }
            'L' => {
                at = [take(&mut index), take(&mut index)];
                current.push(at);
            }
            'H' => {
                at = [take(&mut index), at[1]];
                current.push(at);
            }
            'V' => {
                at = [at[0], take(&mut index)];
                current.push(at);
            }
            'Q' => {
                let (c, end) = ([take(&mut index), take(&mut index)], [take(&mut index), take(&mut index)]);
                let start = at;
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    current.push([u * u * start[0] + 2.0 * u * t * c[0] + t * t * end[0], u * u * start[1] + 2.0 * u * t * c[1] + t * t * end[1]]);
                }
                at = end;
            }
            'C' => {
                let (c1, c2, end) = ([take(&mut index), take(&mut index)], [take(&mut index), take(&mut index)], [take(&mut index), take(&mut index)]);
                let start = at;
                for step in 1..=20 {
                    let t = step as f32 / 20.0;
                    let u = 1.0 - t;
                    let b = |a: f32, b: f32, c: f32, d: f32| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
                    current.push([b(start[0], c1[0], c2[0], end[0]), b(start[1], c1[1], c2[1], end[1])]);
                }
                at = end;
            }
            _ => index += 1,
        }
    }
    if current.len() > 1 {
        out.push((current, false));
    }
    out
}

/// Sutherland-Hodgman: the part of a closed polygon on one side of the vertical line `x = cut`.
fn clip_side(points: &[[f32; 2]], cut: f32, left: bool) -> Vec<[f32; 2]> {
    let inside = |p: [f32; 2]| if left { p[0] <= cut } else { p[0] >= cut };
    let mut out = Vec::new();
    for (i, &current) in points.iter().enumerate() {
        let previous = points[(i + points.len() - 1) % points.len()];
        let crossing = || {
            let t = (cut - previous[0]) / (current[0] - previous[0]);
            [cut, previous[1] + t * (current[1] - previous[1])]
        };
        match (inside(previous), inside(current)) {
            (true, true) => out.push(current),
            (true, false) => out.push(crossing()),
            (false, true) => {
                out.push(crossing());
                out.push(current);
            }
            (false, false) => {}
        }
    }
    out.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4);
    out
}

/// A clipped ring's boundary without its edge on the cut, from one cut point round to the other.
fn open_at_cut(points: Vec<[f32; 2]>, cut: f32) -> Vec<[f32; 2]> {
    let on_cut = |p: [f32; 2]| (p[0] - cut).abs() < 1e-3;
    let n = points.len();
    let Some(k) = (0..n).find(|&k| on_cut(points[k]) && on_cut(points[(k + 1) % n])) else { return points };
    (1..=n).map(|step| points[(k + step) % n]).collect()
}

/// An outline with a hole (`fill-rule="evenodd"`), as two simple polygons split at `x = cut`,
/// which must cross both the outline and the hole: each half walks the outline from one cut
/// point to the other and comes back along the hole.
pub fn ring_halves(outer: &[[f32; 2]], inner: &[[f32; 2]], cut: f32) -> Vec<Vec<[f32; 2]>> {
    let trim = |ring: &[[f32; 2]]| {
        let mut ring = ring.to_vec();
        if ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        ring
    };
    let (outer, inner) = (trim(outer), trim(inner));
    [true, false]
        .into_iter()
        .map(|left| {
            let rim = open_at_cut(clip_side(&outer, cut, left), cut);
            let mut hole = open_at_cut(clip_side(&inner, cut, left), cut);
            let (Some(&rim_end), Some(&hole_start)) = (rim.last(), hole.first()) else { return rim };
            let distance = |a: [f32; 2], b: [f32; 2]| (a[1] - b[1]).abs() + (a[0] - b[0]).abs();
            if distance(rim_end, hole_start) > distance(rim_end, *hole.last().unwrap_or(&hole_start)) {
                hole.reverse();
            }
            rim.into_iter().chain(hole).collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_area(points: &[[f32; 2]]) -> f32 {
        points.iter().zip(points.iter().cycle().skip(1)).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f32>() / 2.0
    }

    #[test]
    fn every_face_name_the_page_offers_is_known() {
        // The switcher's list in `widget.js`.
        let names = ["digital", "gooey", "digital-stacked", "digital-vertical", "analogue", "analogue-dot", "analogue-bezel", "analogue-cmf", "rings", "minimal-lines", "grid-second", "nothing-watch-pro", "gothic"];
        for name in names {
            assert_ne!(Face::parse(name), Face::Unknown, "{name}");
        }
        assert_eq!(Face::parse("Rings"), Face::Unknown);
    }

    #[test]
    fn svg_paths_flatten_with_their_subpaths() {
        let zero = svg_path(GOOEY_ZERO);
        assert_eq!(zero.len(), 2);
        assert!(zero.iter().all(|(_, closed)| *closed));
        assert_eq!(zero[0].0[0], [20.0, 2.0]);
        let three = svg_path(GOOEY_STROKES[1]);
        assert_eq!(three.len(), 2, "M inside a path starts a subpath");
        assert_eq!(three[1].0[0], [41.0, 38.0]);
        let arch = svg_path(GOTHIC_PATHS[0]);
        assert_eq!(arch[0].0.last(), Some(&[166.0, 151.0]));
    }

    #[test]
    fn a_ring_splits_into_two_halves_that_cover_it() {
        let outer = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let inner = [[3.0, 3.0], [3.0, 7.0], [7.0, 7.0], [7.0, 3.0]];
        let halves = ring_halves(&outer, &inner, 5.0);
        assert_eq!(halves.len(), 2);
        for half in &halves {
            assert!((signed_area(half).abs() - 42.0).abs() < 1e-3, "{half:?}");
        }
        let zero = svg_path(GOOEY_ZERO);
        let area: f32 = ring_halves(&zero[0].0, &zero[1].0, 32.0).iter().map(|half| signed_area(half).abs()).sum();
        let whole = signed_area(&zero[0].0).abs() - signed_area(&zero[1].0).abs();
        assert!((area - whole).abs() < 1.0, "{area} vs {whole}");
    }

    #[test]
    fn the_short_date_is_the_pages() {
        let time = ClockTime { year: 2026, month: 11, day: 15, ..Default::default() };
        assert_eq!(short_date(time, false), "вс, 15 нояб");
        let early = ClockTime { day: 5, ..time };
        assert_eq!(short_date(early, false), "чт, 5 нояб");
        assert_eq!(short_date(early, true), "чт, 05 нояб");
    }

    #[test]
    fn faces_with_seconds_tick_every_second() {
        assert!(Face::Rings.ticks_each_second() && Face::NothingWatchPro.ticks_each_second());
        assert!(!Face::Digital.ticks_each_second() && !Face::Gooey.ticks_each_second() && !Face::AnalogueDot.ticks_each_second());
    }
}
