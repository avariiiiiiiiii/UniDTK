//! The desk the native host draws: the layout the kernel stores, where a record lands on the
//! canvas, the local time widgets show, and the grain a jelly card wears.
//!
//! Positions are the old page's CSS pixels, which are Rune's points (1280x720 points at
//! 150 % on the development machine): every field is used as stored and never divided by the
//! physical scale. `r[desk.positions]`, `r[desk.position-merge]`. Every color, radius, font
//! and duration comes from the theme (`theme.rs`), `r[theme.tokens-only]`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::theme::Theme;

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

/// One widget's stored position fields. Module settings in the same record (`version`,
/// `expanded`, `activeChatId`, ...) are the widget's own (`Widget::sync`) and stay in the
/// kernel's blob untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub x: f64,
    pub y: f64,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub anchor: Option<Anchor>,
    pub ax: f64,
    pub ay: f64,
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
    pub fn parse(value: &Value) -> Record {
        let number = |key: &str| value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite());
        Record {
            x: number("x").unwrap_or(0.0),
            y: number("y").unwrap_or(0.0),
            w: number("w").filter(|w| *w > 0.0),
            h: number("h").filter(|h| *h > 0.0),
            anchor: value.get("anchor").and_then(Value::as_str).and_then(Anchor::parse),
            ax: number("ax").unwrap_or(0.0),
            ay: number("ay").unwrap_or(0.0),
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

/// A module's `defaultPlacement` (`metadata.json`), the record a desk without one draws, as
/// the page's first run seeds it.
pub fn default_record(module: &str) -> Option<Record> {
    let placement = crate::catalog::modules().iter().find(|m| m.id == module)?.meta.get("defaultPlacement").filter(|p| p.is_object())?;
    Some(Record::parse(placement))
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
    /// The blob as the kernel sent it, for what the menu edits (themes, colours, folders) and
    /// what each widget keeps in its record.
    pub config: Value,
}

impl Layout {
    /// A desk with nothing stored, as the page draws its first run: the modules' default
    /// layout (`getDefaultWidgetConfig`) and the default theme. The host never writes it back;
    /// the kernel seeds the same layout on the first change.
    pub fn empty() -> Layout {
        Layout::from_config(0, String::new(), &crate::catalog::default_layout())
    }

    /// A `desk-layout-changed` frame or a `desk_layout_get` answer: it names its `config`, and
    /// a `config` of `null` (nothing stored) is the first-run layout. Anything without the key
    /// (another command's answer, `{ok, rev}`) is not a layout.
    pub fn from_event(payload: &Value) -> Option<Layout> {
        let rev = payload.get("rev")?.as_u64()?;
        let instance = payload.get("instance").and_then(Value::as_str).unwrap_or_default().to_string();
        match payload.get("config")? {
            Value::Null => Some(Layout { rev, instance, ..Layout::empty() }),
            config if config.is_object() => Some(Layout::from_config(rev, instance, config)),
            _ => None,
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

    /// A widget's whole stored record (its settings too), `Null` when it has none: the
    /// module's default placement for a module that was never placed.
    pub fn record_value(&self, id: &str, widget_type: &str) -> Value {
        match self.config["positions"].get(id) {
            Some(record) => record.clone(),
            None if id == widget_type => crate::catalog::modules()
                .iter()
                .find(|m| m.id == widget_type)
                .and_then(|m| m.meta.get("defaultPlacement").cloned())
                .unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden.iter().any(|hidden| hidden == id)
    }
}

/// The local date and time widgets show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LocalTime {
    pub year: i32,
    /// 1..=12.
    pub month: u32,
    pub day: u32,
    pub h: u32,
    pub m: u32,
    pub s: u32,
    /// Nanoseconds into the second.
    pub nanos: u32,
}

impl LocalTime {
    pub fn now() -> LocalTime {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        LocalTime { year: now.year(), month: now.month(), day: now.day(), h: now.hour(), m: now.minute(), s: now.second(), nanos: now.nanosecond() % 1_000_000_000 }
    }

    /// Seconds until the next whole second, and a few milliseconds more so a wake lands after
    /// the boundary, not before it.
    pub fn to_next_second(&self) -> f64 {
        (1.0 - f64::from(self.nanos) / 1e9).max(0.0) + 0.005
    }

    /// The month's full name in the language of Windows' interface, as WebView2's
    /// `toLocaleString('default', { month: 'long' })` names it ("ноябрь"); Russian when
    /// Windows gives none.
    pub fn month_name(&self) -> String {
        use windows::Win32::Globalization::{GetLocaleInfoW, GetUserDefaultUILanguage, LOCALE_SMONTHNAME1};
        const RUSSIAN: [&str; 12] = ["январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь"];
        let month = self.month.clamp(1, 12);
        let mut buffer = [0u16; 80];
        // LOCALE_SMONTHNAME1..12 are consecutive.
        let written = unsafe { GetLocaleInfoW(u32::from(GetUserDefaultUILanguage()), LOCALE_SMONTHNAME1 + month - 1, Some(&mut buffer)) };
        match written {
            n if n > 1 => String::from_utf16_lossy(&buffer[..n as usize - 1]),
            _ => RUSSIAN[month as usize - 1].to_string(),
        }
    }

    /// Seconds until the next whole minute, likewise.
    pub fn to_next_minute(&self) -> f64 {
        (f64::from(60 - self.s) - f64::from(self.nanos) / 1e9).max(0.0) + 0.005
    }
}

/// The grain the page tiles over a jelly card, `assets/textures/grain.png`: a 96 px tile drawn
/// 64 CSS px wide, at the theme's grain strength, as an image in device pixels (Rune has no
/// tiled backgrounds). It is made once at the size of the largest surface and clipped by each
/// card, so it never stretches while a card grows.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Material;
    use serde_json::json;

    fn layout(positions: Value) -> Layout {
        Layout::from_event(&json!({ "rev": 7, "instance": "a", "config": { "positions": positions } })).unwrap()
    }

    #[test]
    fn a_module_without_a_record_takes_its_default_placement() {
        let record = default_record("clock").unwrap();
        assert_eq!((record.x, record.y, record.w, record.h), (510.0, -10.0, Some(210.0), Some(190.0)));
        assert_eq!(layout(json!({})).record_value("clock", "clock")["version"], "rings");
        assert_eq!(layout(json!({})).record_value("clock-2", "clock"), Value::Null, "an instance has no default");
    }

    #[test]
    fn an_anchored_record_keeps_its_distance_to_its_corner() {
        let clock = layout(json!({ "clock": { "anchor": "tr", "ax": 494, "ay": 0, "w": 202, "h": 192, "x": 528, "y": 0 } })).record("clock").unwrap();
        let wide = clock.place((1280.0, 720.0), (210.0, 190.0));
        assert_eq!((wide.x, wide.y, wide.w, wide.h), (584.0, 0.0, 202.0, 192.0));
        assert_eq!(clock.place((1000.0, 720.0), (210.0, 190.0)).x, 304.0);
        let bottom = layout(json!({ "clock": { "anchor": "br", "ax": 10, "ay": 20, "w": 100, "h": 50 } })).record("clock").unwrap();
        let placed = bottom.place((1280.0, 720.0), (210.0, 190.0));
        assert_eq!((placed.x, placed.y), (1170.0, 650.0));
    }

    #[test]
    fn a_plain_record_is_top_left_and_stays_inside_the_canvas() {
        let placed = layout(json!({ "clock": { "x": 5000, "y": -30 } })).record("clock").unwrap().place((1280.0, 720.0), (210.0, 190.0));
        assert_eq!((placed.x, placed.y, placed.w, placed.h), (1070.0, 0.0, 210.0, 190.0));
    }

    #[test]
    fn layouts_carry_the_whole_desk_and_nothing_else_is_a_layout() {
        assert!(Layout::from_event(&json!({ "config": {} })).is_none(), "no revision");
        assert!(Layout::from_event(&json!({ "rev": 1, "config": [] })).is_none());
        assert!(Layout::from_event(&json!({ "ok": true, "rev": 12 })).is_none(), "a desk_apply answer is not a layout");
        let reply = json!({ "rev": 9, "instance": "k1", "config": {
            "positions": { "clock": { "version": "digital" } },
            "hidden": ["telegram", 3], "nested": { "clock": ["vpn"] },
            "theme": { "id": "sea-glass", "schemaVersion": 4 },
        } });
        let layout = Layout::from_event(&reply).unwrap();
        assert_eq!((layout.rev, layout.theme.material), (9, Material::Jelly));
        assert_eq!(layout.record_value("clock", "clock")["version"], "digital");
        assert!(layout.is_hidden("telegram") && !layout.is_hidden("clock"));
        assert_eq!(layout.nested["clock"], vec!["vpn".to_string()]);
        // Nothing stored: the first-run layout, with the default theme.
        let first = Layout::from_event(&json!({ "rev": 2, "instance": "k1", "config": null })).unwrap();
        assert_eq!(first.rev, 2);
        assert_eq!(first.record_value("clock", "clock")["version"], "rings");
        assert_eq!(first.nested["clock"], vec!["vpn".to_string()]);
        assert_eq!(Layout::empty().theme, Theme::nothing_classic());
        // A new kernel run supersedes whatever revision it starts from.
        let older_run = Layout { rev: 10, instance: "k0".into(), ..layout.clone() };
        assert!(layout.supersedes(&older_run));
        let later_same_run = Layout { rev: 10, ..layout.clone() };
        assert!(!layout.supersedes(&later_same_run));
    }

    #[test]
    fn waits_land_just_after_the_boundary() {
        let time = LocalTime { s: 50, nanos: 250_000_000, ..Default::default() };
        assert!((time.to_next_second() - 0.755).abs() < 1e-9);
        assert!((time.to_next_minute() - 9.755).abs() < 1e-9);
    }
}
