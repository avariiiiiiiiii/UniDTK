//! The desk the native host draws: the layout the kernel stores, where a record lands on the
//! canvas, and the Clock widget with its dashboard.
//!
//! Positions are the old page's CSS pixels, which are Rune's points (1280x720 points at
//! 150 % on the development machine): every field is used as stored and never divided by the
//! physical scale. `r[desk.positions]`, `r[desk.position-merge]`. Every color, radius, font
//! and duration comes from the theme (`theme.rs`), `r[theme.tokens-only]`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use rune::el::{col, el, row, text, Align, El};
use rune::{Color, LineCap, Pixels, Shadow, Shape, TextStyle};
use serde_json::Value;

pub use crate::faces::Face;
use crate::faces::Frame;
use crate::motion::Motion;
use crate::theme::{derived, mix_oklch, scaled, Material, Theme};

pub const CLOCK: &str = "clock";

/// A clock record that stores no size is drawn as the page draws it: `.widget-card` is
/// 340 px wide and `.clock-widget` at least 180 px high.
const CARD_FALLBACK: (f32, f32) = (340.0, 180.0);

/// `modules/clock/ui/metadata.json` `defaultPlacement`, the layout a desk without a stored
/// clock record draws, as the page's first run seeds it; a test keeps the two equal.
const CLOCK_DEFAULT: Record = Record {
    x: 510.0,
    y: -10.0,
    w: Some(210.0),
    h: Some(190.0),
    anchor: None,
    ax: 0.0,
    ay: 0.0,
    version: Some(Face::Rings),
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Anchor {
    fn parse(value: &str) -> Option<Anchor> {
        Some(match value {
            "tl" => Anchor::TopLeft,
            "tr" => Anchor::TopRight,
            "bl" => Anchor::BottomLeft,
            "br" => Anchor::BottomRight,
            _ => return None,
        })
    }
}

/// One widget's stored position fields. Module settings in the same record (`expanded`,
/// `activeChatId`, ...) are not read here and stay in the kernel's blob untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub x: f64,
    pub y: f64,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub anchor: Option<Anchor>,
    pub ax: f64,
    pub ay: f64,
    pub version: Option<Face>,
}

/// Where a widget lands on a canvas, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Record {
    fn parse(value: &Value) -> Record {
        let number = |key: &str| value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite());
        Record {
            x: number("x").unwrap_or(0.0),
            y: number("y").unwrap_or(0.0),
            w: number("w").filter(|w| *w > 0.0),
            h: number("h").filter(|h| *h > 0.0),
            anchor: value.get("anchor").and_then(Value::as_str).and_then(Anchor::parse),
            ax: number("ax").unwrap_or(0.0),
            ay: number("ay").unwrap_or(0.0),
            version: value.get("version").and_then(Value::as_str).map(Face::parse),
        }
    }

    /// The page's `placeOnCanvas`: an anchored record keeps its distance to its corner when
    /// the canvas changes, a record without an anchor is a plain top-left x/y. `fallback` is
    /// the size of a record that stores none. The result is clamped into the canvas, so a
    /// stale or default coordinate (the clock's default has `y: -10`) never hides a widget.
    pub fn place(&self, canvas: (f32, f32), fallback: (f32, f32)) -> Placement {
        let w = self.w.map_or(fallback.0, |w| w as f32);
        let h = self.h.map_or(fallback.1, |h| h as f32);
        let (x, y) = match self.anchor {
            None => (self.x as f32, self.y as f32),
            Some(anchor) => {
                let right = matches!(anchor, Anchor::TopRight | Anchor::BottomRight);
                let bottom = matches!(anchor, Anchor::BottomLeft | Anchor::BottomRight);
                (
                    if right { (canvas.0 - w - self.ax as f32).max(0.0) } else { self.ax as f32 },
                    if bottom { (canvas.1 - h - self.ay as f32).max(0.0) } else { self.ay as f32 },
                )
            }
        };
        let (max_x, max_y) = ((canvas.0 - w).max(0.0), (canvas.1 - h).max(0.0));
        Placement { x: x.clamp(0.0, max_x), y: y.clamp(0.0, max_y), w, h }
    }
}

/// The desk as the kernel stores it: `desk-layout-changed` and `desk_layout_get` both carry
/// `{rev, instance, config}` with the whole blob.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub rev: u64,
    /// The kernel run the revision belongs to; revisions of two runs do not compare.
    pub instance: String,
    records: BTreeMap<String, Record>,
    /// `config.hidden`: widgets switched off the desk.
    pub hidden: Vec<String>,
    /// `config.nested`: `{host: [child]}` as the user arranged it.
    pub nested: BTreeMap<String, Vec<String>>,
    pub theme: Theme,
    /// The blob as the kernel sent it, for what the menu edits (themes, colours, folders).
    pub config: Value,
}

impl Layout {
    /// A desk with nothing stored, as the page draws its first run: the modules' default
    /// layout (`getDefaultWidgetConfig`) and the default theme. The host never writes it back;
    /// the kernel seeds the same layout on the first change.
    pub fn empty() -> Layout {
        Layout::from_config(0, String::new(), &crate::module_table::default_layout())
    }

    /// A `desk-layout-changed` frame or a `desk_layout_get` answer. A `config` of `null`
    /// (nothing stored) is the first-run layout.
    pub fn from_event(payload: &Value) -> Option<Layout> {
        let rev = payload.get("rev")?.as_u64()?;
        let instance = payload.get("instance").and_then(Value::as_str).unwrap_or_default().to_string();
        match payload.get("config") {
            Some(Value::Null) | None => Some(Layout { rev, instance, ..Layout::empty() }),
            Some(config) if config.is_object() => Some(Layout::from_config(rev, instance, config)),
            Some(_) => None,
        }
    }

    fn from_config(rev: u64, instance: String, config: &Value) -> Layout {
        let positions = config.get("positions").and_then(Value::as_object);
        let strings = |list: &Value| list.as_array().map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        Layout {
            rev,
            instance,
            records: positions.map(|p| p.iter().map(|(id, record)| (id.clone(), Record::parse(record))).collect()).unwrap_or_default(),
            hidden: config.get("hidden").map(strings).unwrap_or_default(),
            nested: config
                .get("nested")
                .and_then(Value::as_object)
                .map(|hosts| hosts.iter().map(|(host, children)| (host.clone(), strings(children))).collect())
                .unwrap_or_default(),
            theme: Theme::normalize(config.get("theme").unwrap_or(&Value::Null)),
            config: config.clone(),
        }
    }

    /// Whether this layout replaces `known`: a newer revision of the same kernel run, or any
    /// revision of a new run (its clock may have stepped back).
    pub fn supersedes(&self, known: &Layout) -> bool {
        self.instance != known.instance || self.rev >= known.rev
    }

    /// A widget's stored record, if it has one.
    pub fn record(&self, id: &str) -> Option<Record> {
        self.records.get(id).cloned()
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|hidden| hidden == id)
    }

    /// The clock the desk draws: its stored record, else the module's default placement.
    pub fn clock(&self) -> Record {
        self.records.get(CLOCK).cloned().unwrap_or(CLOCK_DEFAULT)
    }

    /// The clock's place on the dock: its record, sized by the record, else the theme's
    /// preset size, else the page's card defaults.
    pub fn clock_place(&self, canvas: (f32, f32)) -> Placement {
        self.clock_place_with(canvas, &self.theme)
    }

    /// The same with another theme's preset sizes (a draft the menu shows).
    pub fn clock_place_with(&self, canvas: (f32, f32), theme: &Theme) -> Placement {
        let fallback = theme.size_of(CLOCK).unwrap_or(CARD_FALLBACK);
        self.clock().place(canvas, fallback)
    }
}

/// The face a record asks for: its `version`, else `digital`, as `renderClock` reads
/// `pos.version || "digital"`. Only the module's default placement names `rings`.
pub fn face_of(record: &Record) -> Face {
    record.version.unwrap_or(Face::Digital)
}

/// The local date and time the clock and the calendar show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClockTime {
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    pub day: u32,
    pub h: u32,
    pub m: u32,
    pub s: u32,
}

impl ClockTime {
    /// The local time, and the nanoseconds into the current second.
    pub fn now() -> (ClockTime, u32) {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        let time = ClockTime { year: now.year(), month: now.month(), day: now.day(), h: now.hour(), m: now.minute(), s: now.second() };
        (time, now.nanosecond() % 1_000_000_000)
    }

    /// Seconds until the face has something new to show: the next whole second for `rings`,
    /// the next minute for `digital`, plus a few milliseconds so the wake lands after the
    /// boundary, not before it.
    pub fn seconds_to_next_tick(self, face: Face, nanos: u32) -> f64 {
        let into_second = f64::from(nanos) / 1e9;
        let whole = if face.ticks_each_second() { 1.0 } else { f64::from(60 - self.s) };
        (whole - into_second).max(0.0) + 0.005
    }

    /// What a face shows has changed: every second for `rings`, every minute (or a new day)
    /// for `digital`.
    pub fn differs(self, other: ClockTime, face: Face) -> bool {
        if face.ticks_each_second() { self != other } else { ClockTime { s: 0, ..self } != ClockTime { s: 0, ..other } }
    }

    /// Whole degrees only, as the page sets `rotate(m * 6deg)` and `rotate(s * 6deg)`.
    pub fn outer_degrees(self) -> f32 {
        (self.m * 6) as f32
    }

    pub fn inner_degrees(self) -> f32 {
        (self.s * 6) as f32
    }

    /// `new Date(year, month, 1).getDay()`: the column of the 1st, Sunday first.
    fn first_weekday(self) -> usize {
        chrono::NaiveDate::from_ymd_opt(self.year, self.month, 1).map_or(0, |date| {
            use chrono::Datelike;
            date.weekday().num_days_from_sunday() as usize
        })
    }

    fn days_in_month(self) -> u32 {
        let (year, month) = if self.month == 12 { (self.year + 1, 1) } else { (self.year, self.month + 1) };
        chrono::NaiveDate::from_ymd_opt(year, month, 1)
            .and_then(|next| next.pred_opt())
            .map_or(30, |last| {
                use chrono::Datelike;
                last.day()
            })
    }

    /// `toLocaleString('default', { month: 'long' })` in the desk's Russian locale, upper-cased
    /// as `.cal-header` shows it.
    fn month_name(self) -> &'static str {
        const MONTHS: [&str; 12] = ["ЯНВАРЬ", "ФЕВРАЛЬ", "МАРТ", "АПРЕЛЬ", "МАЙ", "ИЮНЬ", "ИЮЛЬ", "АВГУСТ", "СЕНТЯБРЬ", "ОКТЯБРЬ", "НОЯБРЬ", "ДЕКАБРЬ"];
        MONTHS[(self.month.clamp(1, 12) - 1) as usize]
    }
}

fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color, inset: bool) -> Shadow {
    Shadow { x, y, blur, spread, color, inset }
}

/// The card's own paint: body, gradients, border and shadows, as the material and the
/// card's state choose (`styles.css`: `.widget-card`, the jelly block, `.widget-card.expanded`).
pub(crate) fn card_paint(card: El, theme: &Theme, tint: Color, w: f32, h: f32, expanded: bool) -> El {
    let hi = Color::WHITE;
    match (theme.material, expanded) {
        (Material::Jelly, false) => {
            let offset = theme.shadow_offset;
            card.radius(theme.radius_card)
                .fill(Color { a: theme.glass_opacity, ..tint })
                .linear_gradient(155f32.to_radians(), [(0.42, hi.alpha(0.0)), (1.0, derived(tint, -0.07, 1.5, 0.02, theme.glass_depth * 0.75))])
                .box_shadow(shadow(0.0, 1.5, 0.0, 0.0, hi.alpha(theme.glass_rim * 0.9), true))
                .box_shadow(shadow(1.5, 0.0, 0.0, 0.0, hi.alpha(theme.glass_rim * 0.35), true))
                .box_shadow(shadow(-2.0, -3.0, 6.0, 0.0, derived(tint, -0.2, 1.4, 0.0, theme.glass_depth * 0.45), true))
                .box_shadow(shadow(offset * 0.6, offset, offset * 1.6, -offset * 0.2, scaled(tint, 0.5, 0.5, 0.0, theme.glass_shadow * 0.6), false))
                .box_shadow(shadow(1.0, 2.0, 3.0, 0.0, theme.scrim.alpha(theme.glass_shadow * 0.2), false))
        }
        // An expanded jelly card keeps its dock body (the wash fades in over it, `clock_card`),
        // takes the surface radius and a white hairline, and loses its shadows
        // (`.widget-card.expanded { box-shadow: none !important }`).
        (Material::Jelly, true) => card
            .radius(theme.radius_surface)
            .fill(Color { a: theme.glass_opacity, ..tint })
            .linear_gradient(155f32.to_radians(), [(0.42, hi.alpha(0.0)), (1.0, derived(tint, -0.07, 1.5, 0.02, theme.glass_depth * 0.75))])
            .border(1.0, hi.alpha(0.8)),
        (Material::Flat, false) => {
            let mut card = card
                .radius(theme.radius_card)
                .fill(Color { a: theme.glass_opacity, ..theme.bg_base })
                .border(1.0, theme.bg_elevated.alpha(0.12));
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
                let (rim, drop) = (theme.glass_rim, theme.glass_shadow);
                card = card
                    .box_shadow(shadow(0.0, 1.0, 1.0, 0.0, theme.bg_elevated.alpha(rim * 0.75), true))
                    .box_shadow(shadow(0.0, 2.0, 6.0, -1.0, theme.bg_elevated.alpha(rim * 0.35), true))
                    .box_shadow(shadow(0.0, -1.0, 1.0, 0.0, theme.bg_elevated.alpha(rim * 0.2), true))
                    .box_shadow(shadow(0.0, 4.0, 12.0, -2.0, theme.scrim.alpha(drop * 0.12), false))
                    .box_shadow(shadow(0.0, 16.0, 36.0, -4.0, theme.bg_base.alpha(drop * 0.4), false))
                    .box_shadow(shadow(0.0, 28.0, 60.0, -10.0, theme.scrim.alpha(drop * 0.15), false));
            }
            card
        }
        (Material::Flat, true) => card
            .radius(theme.radius_card)
            .fill(theme.surface.alpha(0.96))
            .border(1.0, theme.bg_elevated.alpha(0.15)),
    }
    .size(w, h)
}

/// The grain the page tiles over a jelly card, `assets/textures/grain.png`: a 96 px tile drawn
/// 64 CSS px wide, at the theme's grain strength, as an image in device pixels (Rune has no
/// tiled backgrounds). It is made once at the size of the open dashboard and clipped by the
/// card, so it never stretches while the card grows.
pub fn grain_rgba(width: u32, height: u32, scale: f32, strength: f32) -> Vec<u8> {
    static TILE: OnceLock<(usize, Vec<u8>)> = OnceLock::new();
    let (side, tile) = TILE.get_or_init(|| {
        let decoder = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../../assets/textures/grain.png")[..]));
        let Ok(mut reader) = decoder.read_info() else { return (1, vec![0, 0]) };
        let Some(size) = reader.output_buffer_size() else { return (1, vec![0, 0]) };
        let mut buffer = vec![0; size];
        match reader.next_frame(&mut buffer) {
            Ok(frame) if frame.color_type == png::ColorType::GrayscaleAlpha && frame.width == frame.height => {
                (frame.width as usize, buffer[..frame.buffer_size()].to_vec())
            }
            _ => (1, vec![0, 0]),
        }
    });
    let period = 64.0 * scale;
    let at = |pixel: u32| (((pixel as f32 + 0.5) / period).fract() * *side as f32) as usize % side;
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row = at(y) * side;
        for x in 0..width {
            let texel = (row + at(x)) * 2;
            let alpha = (f32::from(tile[texel + 1]) * strength).round().clamp(0.0, 255.0) as u8;
            out.extend_from_slice(&[tile[texel], tile[texel], tile[texel], alpha]);
        }
    }
    out
}

fn style(theme_stack: &str, size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).family(Theme::family(theme_stack))
}

/// The dashboard's month: `.dash-calendar` as `renderClockDashboard` fills it.
fn calendar(time: ClockTime, theme: &Theme, ink: Color, width: f32) -> (El, f32) {
    const PAD: f32 = 12.0;
    const GAP: f32 = 4.0;
    const NAMES: [&str; 7] = ["В", "П", "В", "С", "Ч", "П", "С"];
    let inner = width - 2.0 * PAD - 2.0;
    let cell = (inner - 6.0 * GAP) / 7.0;
    let body = |size: f32, weight: f32| style(theme.font_body, size, weight);
    let line = 9.6 * 1.25;
    let cells: Vec<El> = (0..theme_cells(time))
        .map(|index| {
            let day = index as i64 - time.first_weekday() as i64 + 1;
            if day < 1 || day > i64::from(time.days_in_month()) {
                return el().w(cell).h(line + 4.0);
            }
            let today = day as u32 == time.day;
            let label = text(day.to_string()).font(body(9.6, if today { 700.0 } else { 400.0 })).align(Align::Center);
            let mut cell_el = col().w(cell).h(line + 4.0).items_center().justify_center();
            if today {
                cell_el = cell_el.fill(theme.bg_elevated).radius(4.0);
                cell_el.child(label.color(theme.scrim))
            } else {
                cell_el.child(label.color(ink.alpha(0.6)))
            }
        })
        .collect();
    let weeks = cells.len() / 7;
    let mut rows = col().gap(GAP).child(
        row().gap(GAP).children(NAMES.iter().map(|name| {
            col().w(cell).h(line + 4.0).items_center().child(text(*name).font(body(9.6, 600.0)).color(ink.alpha(0.3)))
        })),
    );
    let mut cells = cells.into_iter();
    for _ in 0..weeks {
        rows = rows.child(row().gap(GAP).children(cells.by_ref().take(7)));
    }
    let header = text(time.month_name()).font(style(theme.font_display, 11.2, 400.0)).color(theme.accent).align(Align::Center);
    let height = 2.0 + 2.0 * PAD + 11.2 * 1.25 + 8.0 + (weeks as f32 + 1.0) * (line + 4.0) + weeks as f32 * GAP;
    let out = col()
        .w(width)
        .h(height)
        .padding(PAD)
        .radius(16.0)
        .fill(theme.bg_elevated.alpha(0.03))
        .border(1.0, theme.bg_elevated.alpha(0.05))
        .child(col().w_full().items_center().child(header))
        .child(el().h(8.0))
        .child(rows);
    (out, height)
}

/// The grid's cell count: the blanks before the 1st and the days, whole weeks.
fn theme_cells(time: ClockTime) -> usize {
    let used = time.first_weekday() + time.days_in_month() as usize;
    used.div_ceil(7) * 7
}

/// Points on a circle of `radius` around `centre`, from `from` to `to` degrees clockwise from
/// twelve o'clock.
fn circle_points(centre: [f32; 2], radius: f32, from: f32, to: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|step| {
            let angle = (from + (to - from) * step as f32 / steps as f32).to_radians();
            [centre[0] + radius * angle.sin(), centre[1] - radius * angle.cos()]
        })
        .collect()
}

/// The Wi-Fi glyph of `.net-icon` (a 24-unit SVG: three arcs and a dot), as polylines.
fn wifi_icon(colour: Color) -> Shape {
    // Each `a r r 0 0 1` arc runs clockwise over the top of its circle between two points
    // on the same height; the centre lies below the chord.
    let arc = |from: [f32; 2], to: [f32; 2], radius: f32| {
        let half = (to[0] - from[0]) / 2.0;
        let centre = [from[0] + half, from[1] + (radius * radius - half * half).max(0.0).sqrt()];
        let sweep = (half / radius).clamp(-1.0, 1.0).asin().to_degrees();
        circle_points(centre, radius, -sweep, sweep, 24)
    };
    Shape::new()
        .view_box(24.0, 24.0)
        .stroke_with(arc([5.0, 12.55], [19.08, 12.55], 11.0), false, 2.0, LineCap::Round, colour)
        .stroke_with(arc([1.42, 9.0], [22.58, 9.0], 16.0), false, 2.0, LineCap::Round, colour)
        .stroke_with(arc([8.53, 16.11], [15.48, 16.11], 6.0), false, 2.0, LineCap::Round, colour)
        .line([12.0, 20.0], [12.01, 20.0], 2.0, LineCap::Round, colour)
}

/// `.dash-internet`: the connection badge `navigator.onLine` fills.
fn internet(online: bool, theme: &Theme, ink: Color, width: f32) -> El {
    let status = if online { theme.status_ok } else { theme.status_error };
    row()
        .w(width)
        .h(50.0)
        .padding(10.0)
        .gap(10.0)
        .items_center()
        .radius(14.0)
        .fill(theme.bg_elevated.alpha(0.03))
        .border(1.0, theme.bg_elevated.alpha(0.05))
        .child(
            col().size(28.0, 28.0).radius(14.0).fill(status.alpha(0.1)).items_center().justify_center()
                .child(el().size(16.0, 16.0).shape(wifi_icon(status))),
        )
        .child(text(if online { "В сети" } else { "Нет сети" }).font(style(theme.font_body, 11.2, 500.0)).color(ink.alpha(0.8)))
}

/// `positionExpanded` for the clock: a 400 x 420 box round the tile.
pub fn dashboard_box(tile: Placement, canvas: (f32, f32)) -> Placement {
    surface_box(tile, (400.0, 420.0), canvas)
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

pub struct ClockView<'a> {
    pub face: Face,
    pub time: ClockTime,
    pub tile: Placement,
    pub canvas: (f32, f32),
    pub theme: &'a Theme,
    pub motion: &'a Motion,
    pub grain: &'a Pixels,
    pub grain_size: (f32, f32),
    pub expanded: bool,
    /// The running open or close: its duration, as `surface-manager.js` computed it.
    pub transition: f32,
    pub online: bool,
    /// The menu's widget row is under the pointer (`.menu-highlight`).
    pub highlight: bool,
    /// Drawn over the tile in its coordinates: arrange mode's handles and "×".
    pub overlay: Vec<El>,
    /// The nesting zone under the dashboard (`mountNested`), 368 px wide.
    pub nested: Option<El>,
}

/// The clock's open and close timing (`surface-manager.js`): an open takes
/// `calculateDuration` on `expoOut`; a close takes `0.65 x T x sqrt(remaining)` on
/// `surfaceExit`, where `remaining` is how much of the open it has to undo, so a close that
/// interrupts an opening is shorter (`r[motion.reversal]`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MorphClock {
    /// When the last open started and how long it ran.
    opened: Option<(std::time::Instant, f32)>,
    /// When the running open or close ends.
    until: Option<std::time::Instant>,
}

impl MorphClock {
    pub fn open(&mut self, motion: &Motion, tile: Placement, target: Placement) -> f32 {
        let duration = motion.surface_duration((target.w, target.h), (tile.w, tile.h));
        let now = std::time::Instant::now();
        self.opened = Some((now, duration));
        self.until = Some(now + std::time::Duration::from_secs_f32(duration));
        duration
    }

    /// An open or a close is still running.
    pub fn running(&self) -> bool {
        self.until.is_some_and(|until| std::time::Instant::now() < until)
    }

    pub fn close(&mut self) -> f32 {
        let Some((started, duration)) = self.opened.take() else { return 0.0 };
        if duration <= 0.0 {
            return 0.0;
        }
        let elapsed = (started.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0);
        let remaining = crate::motion::EXPO_OUT.eval(elapsed).clamp(0.0, 1.0);
        let close = 0.65 * duration * remaining.sqrt();
        self.until = Some(std::time::Instant::now() + std::time::Duration::from_secs_f32(close));
        close
    }
}

/// The clock card, the tile on the desk and its dashboard as one surface (`r[motion.clip-morph]`):
/// the dashboard's box never moves; what animates is the visible part of it, from the tile's
/// rectangle and radius to the whole box and the surface radius, while the content keeps its
/// place in the box. The face is the one part that flies, from the tile's container to the
/// dashboard's (`r[motion.shared-flight]`). The press handler sets the transaction's curve.
pub fn clock_card(view: ClockView<'_>) -> El {
    let ClockView { face: kind, time, tile, canvas, theme, motion, grain, grain_size, expanded, transition, online, highlight, overlay, nested } = view;
    let tint = theme.tint(CLOCK, "calm", "lilac");
    let ink = theme.ink_on(tint);
    let target = dashboard_box(tile, canvas);
    // The visible window in the box's coordinates.
    let shown = if expanded {
        Placement { x: 0.0, y: 0.0, w: target.w, h: target.h }
    } else {
        Placement { x: tile.x - target.x, y: tile.y - target.y, w: tile.w, h: tile.h }
    };
    let radius = match (theme.material, expanded) {
        (Material::Jelly, true) => theme.radius_surface,
        _ => theme.radius_card,
    };

    // Inside `.clock-dash-layout`: 16 px body padding, the no-header card's 10 px top and the
    // body's 2 px, then the layout's own 10 px; columns share 368 px at 1.2 : 1 with a 20 px gap.
    let (content_x, content_w, layout_y) = (16.0, 368.0, 22.0);
    let left = (content_w - 20.0) * 1.2 / 2.2;
    let right = content_w - 20.0 - left;
    let (calendar_el, calendar_h) = calendar(time, theme, ink, right);
    let right_h = calendar_h + 15.0 + 50.0;
    let row_h = right_h.max(240.0);

    // The tile's `.clock-container` fills the card under its 10 px top padding and has 10 px of
    // its own; the dashboard's is the 180 px `.dash-clock` centred in the left column, padded
    // the same way. Both in the box's coordinates.
    let frame = if expanded {
        let (cx, cy) = (content_x + left / 2.0, layout_y + row_h / 2.0);
        Frame { x: cx - 80.0, y: cy - 80.0, w: 160.0, h: 160.0 }
    } else {
        Frame { x: shown.x + 10.0, y: shown.y + 20.0, w: (tile.w - 20.0).max(0.0), h: (tile.h - 30.0).max(0.0) }
    };

    // Content fades with the clip: in over T on `content`, out over 0.45 of the close on
    // `smooth`; the wash in over 0.3 T on `expoOut`, out over the second half of the close.
    let (content_fade, wash_fade) = if expanded {
        (motion.tween(transition, crate::motion::CONTENT), motion.tween(0.3 * transition, crate::motion::EXPO_OUT))
    } else {
        let mut wash = motion.tween(0.5 * transition, crate::motion::SURFACE_EXIT);
        wash.delay = 0.5 * transition;
        (motion.tween(0.45 * transition, crate::motion::SMOOTH), wash)
    };

    // Everything inside is placed in the box and shifted by the window's offset, so it stays
    // still while the window moves over it.
    let shift = |element: El, x: f32, y: f32| element.absolute(x - shown.x, y - shown.y);
    let mut inside = el().absolute(0.0, 0.0).size(shown.w, shown.h).clip_rounded(radius);
    if theme.material == Material::Jelly {
        let hi = Color::WHITE;
        let sheen = el().absolute(0.0, 0.0).size(shown.w, shown.h)
            .radial_gradient([0.18, 0.08], [1.2 * shown.w, 0.95 * shown.h], [(0.0, hi.alpha(theme.glass_sheen * 0.7)), (0.58, hi.alpha(0.0))]);
        // `::after`: the expanded wash.
        let wash = el().absolute(0.0, 0.0).size(shown.w, shown.h)
            .fill(mix_oklch(tint, theme.surface_tint, hi))
            .radial_gradient([0.15, 0.0], [1.2 * shown.w, 0.7 * shown.h], [(0.0, hi.alpha(theme.glass_sheen * 0.55)), (0.6, hi.alpha(0.0))])
            .child(el().absolute(0.0, 0.0).w(shown.w).h(1.5).fill(hi.alpha(theme.glass_rim * 0.9)))
            .opacity(if expanded { 1.0 } else { 0.0 })
            .opacity_motion(wash_fade);
        inside = inside
            .child(sheen)
            .child(wash)
            .child(shift(el().size(grain_size.0, grain_size.1).pixels(grain), 0.0, 0.0));
    }
    let dashboard = shift(col(), content_x + left + 20.0, layout_y + (row_h - right_h) / 2.0)
        .w(right)
        .gap(15.0)
        .child(calendar_el)
        .child(internet(online, theme, ink, right))
        .opacity(if expanded { 1.0 } else { 0.0 })
        .opacity_motion(content_fade);
    // `.nested-zone` follows `.clock-dash-layout` with its 10 px margin.
    let zone = nested.map(|zone| {
        shift(el().w(content_w), content_x, layout_y + row_h + 10.0)
            .child(zone)
            .opacity(if expanded { 1.0 } else { 0.0 })
            .opacity_motion(content_fade)
    });
    let face_layer = shift(el().size(target.w, target.h), 0.0, 0.0)
        .child(crate::faces::face(kind, time, frame, theme, ink, expanded));
    let mut window = card_paint(el().key("widget:clock").label("Clock").absolute(shown.x, shown.y), theme, tint, shown.w, shown.h, expanded)
        .child(inside.child(dashboard).children(zone).child(face_layer));
    if highlight {
        // `outline: 2px solid var(--theme-accent); outline-offset: 4px`.
        window = window.outline(2.0, 4.0, theme.accent);
    }
    for part in overlay {
        window = window.child(part);
    }
    el().absolute(target.x, target.y).size(target.w, target.h).child(window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layout(positions: Value) -> Layout {
        Layout::from_event(&json!({ "rev": 7, "instance": "a", "config": { "positions": positions } })).unwrap()
    }

    #[test]
    fn the_default_clock_is_the_modules_default_placement() {
        let metadata: Value = serde_json::from_str(include_str!("../../modules/clock/ui/metadata.json")).unwrap();
        let placement = &metadata["defaultPlacement"];
        let default = CLOCK_DEFAULT;
        assert_eq!(placement["x"].as_f64(), Some(default.x));
        assert_eq!(placement["y"].as_f64(), Some(default.y));
        assert_eq!(placement["w"].as_f64(), default.w);
        assert_eq!(placement["h"].as_f64(), default.h);
        assert_eq!(placement["version"].as_str(), Some("rings"));
    }

    #[test]
    fn an_anchored_record_keeps_its_distance_to_its_corner() {
        let clock = layout(json!({ "clock": { "anchor": "tr", "ax": 494, "ay": 0, "w": 202, "h": 192, "x": 528, "y": 0 } })).clock();
        let wide = clock.place((1280.0, 720.0), (210.0, 190.0));
        assert_eq!((wide.x, wide.y, wide.w, wide.h), (584.0, 0.0, 202.0, 192.0));
        assert_eq!(clock.place((1000.0, 720.0), (210.0, 190.0)).x, 304.0);
        let bottom = layout(json!({ "clock": { "anchor": "br", "ax": 10, "ay": 20, "w": 100, "h": 50 } })).clock();
        let placed = bottom.place((1280.0, 720.0), (210.0, 190.0));
        assert_eq!((placed.x, placed.y), (1170.0, 650.0));
    }

    #[test]
    fn a_plain_record_is_top_left_and_stays_inside_the_canvas() {
        let placed = layout(json!({ "clock": { "x": 5000, "y": -30 } })).clock().place((1280.0, 720.0), (210.0, 190.0));
        assert_eq!((placed.x, placed.y, placed.w, placed.h), (1070.0, 0.0, 210.0, 190.0));
    }

    #[test]
    fn a_record_without_a_size_takes_the_themes_then_the_pages() {
        let themed = Layout::from_event(&json!({ "rev": 1, "config": { "positions": { "clock": { "x": 0, "y": 0 } }, "theme": { "id": "sea-glass", "schemaVersion": 4 } } })).unwrap();
        assert_eq!(themed.clock_place((1280.0, 720.0)).w, 207.0);
        let mut bare = layout(json!({ "clock": { "x": 0, "y": 0 } }));
        bare.theme = Theme::normalize(&json!({ "id": "mine", "schemaVersion": 4, "sizes": {} }));
        assert_eq!(bare.clock_place((1280.0, 720.0)).w, 207.0, "custom themes start from Nothing Classic's sizes");
    }

    #[test]
    fn faces_and_their_fallbacks() {
        assert_eq!(face_of(&layout(json!({})).clock()), Face::Rings, "the module's default placement");
        assert_eq!(face_of(&layout(json!({ "clock": { "x": 10 } })).clock()), Face::Digital, "pos.version || \"digital\"");
        assert_eq!(face_of(&layout(json!({ "clock": { "version": "gooey" } })).clock()), Face::Gooey);
        assert_eq!(face_of(&layout(json!({ "clock": { "version": "neon" } })).clock()), Face::Unknown);
    }

    #[test]
    fn layouts_carry_the_whole_desk() {
        assert!(Layout::from_event(&json!({ "config": {} })).is_none(), "no revision");
        assert!(Layout::from_event(&json!({ "rev": 1, "config": [] })).is_none());
        let reply = json!({ "rev": 9, "instance": "k1", "config": {
            "positions": { "clock": { "version": "digital" } },
            "hidden": ["telegram", 3], "nested": { "clock": ["vpn"] },
            "theme": { "id": "sea-glass", "schemaVersion": 4 },
        } });
        let layout = Layout::from_event(&reply).unwrap();
        assert_eq!((layout.rev, face_of(&layout.clock()), layout.theme.material), (9, Face::Digital, Material::Jelly));
        assert!(layout.is_hidden("telegram") && !layout.is_hidden("clock"));
        assert_eq!(layout.nested["clock"], vec!["vpn".to_string()]);
        // Nothing stored: the first-run layout, with the default theme.
        let first = Layout::from_event(&json!({ "rev": 2, "instance": "k1", "config": null })).unwrap();
        assert_eq!((first.rev, face_of(&first.clock())), (2, Face::Rings));
        assert_eq!(first.nested["clock"], vec!["vpn".to_string()]);
        assert_eq!(Layout::empty().theme, Theme::nothing_classic());
        // A new kernel run supersedes whatever revision it starts from.
        let older_run = Layout { rev: 10, instance: "k0".into(), ..layout.clone() };
        assert!(layout.supersedes(&older_run));
        let later_same_run = Layout { rev: 10, ..layout.clone() };
        assert!(!layout.supersedes(&later_same_run));
    }

    #[test]
    fn the_clock_ticks_by_its_face() {
        let time = ClockTime { year: 2026, month: 11, day: 15, h: 9, m: 5, s: 59 };
        assert_eq!((time.outer_degrees(), time.inner_degrees()), (30.0, 354.0));
        let later = ClockTime { s: 10, ..time };
        assert!(later.differs(time, Face::Rings) && !later.differs(time, Face::Digital));
        assert!(ClockTime { day: 16, ..time }.differs(time, Face::Digital));
        let wait = |face, s, nanos| ClockTime { s, ..time }.seconds_to_next_tick(face, nanos);
        assert!((wait(Face::Rings, 10, 250_000_000) - 0.755).abs() < 1e-9);
        assert!((wait(Face::Digital, 50, 0) - 10.005).abs() < 1e-9);
    }

    #[test]
    fn the_calendar_month_is_laid_out_sunday_first() {
        // 1 November 2026 is a Sunday; February 2028 has 29 days and starts on a Tuesday.
        let november = ClockTime { year: 2026, month: 11, day: 15, ..Default::default() };
        assert_eq!((november.first_weekday(), november.days_in_month(), theme_cells(november)), (0, 30, 35));
        let february = ClockTime { year: 2028, month: 2, day: 1, ..Default::default() };
        assert_eq!((february.first_weekday(), february.days_in_month(), theme_cells(february)), (2, 29, 35));
        assert_eq!(ClockTime { month: 12, year: 2026, ..Default::default() }.days_in_month(), 31);
        assert_eq!(november.month_name(), "НОЯБРЬ");
    }

    #[test]
    fn the_dashboard_grows_round_the_tile_and_stays_on_screen() {
        let canvas = (1280.0, 720.0);
        let tile = Placement { x: 584.0, y: 0.0, w: 202.0, h: 192.0 };
        let open = dashboard_box(tile, canvas);
        assert_eq!((open.x, open.y, open.w, open.h), (485.0, 0.0, 400.0, 420.0));
        let corner = dashboard_box(Placement { x: 1180.0, y: 600.0, w: 100.0, h: 100.0 }, canvas);
        assert_eq!((corner.x, corner.y), (880.0, 290.0));
    }
}
