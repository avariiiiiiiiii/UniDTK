//! The theme the page computes in `core/ui/theme.js`, for the native host: the stored
//! `config.theme` normalized as `normalizeTheme` does (the built-in preset of the same id is
//! the base, Nothing Classic otherwise; malformed fields keep the base's value), and the
//! colors the two materials derive from it (`styles.css`). The page keeps its copy until G5.
//! `r[theme.tokens-only]`: widgets paint with these tokens and carry no colors of their own.

use std::collections::BTreeMap;

use rune::color::hex;
use rune::{Color, Family};
use serde_json::Value;

pub const SCHEMA_VERSION: u64 = 4;

pub const PALETTE_SLOTS: [&str; 8] = ["neutral", "blue", "aqua", "mint", "lilac", "peach", "magenta", "coral"];

const SEA_PALETTE: [(&str, &str); 8] = [
    ("neutral", "#f2f4f7"),
    ("blue", "#a9c2f5"),
    ("aqua", "#9ee3e0"),
    ("mint", "#b5e8b0"),
    ("lilac", "#c9b8f2"),
    ("peach", "#ffd49a"),
    ("magenta", "#f2a6d8"),
    ("coral", "#f7a38c"),
];

/// `PRESET_GEOMETRY`: the default card sizes both built-in presets carry.
const PRESET_GEOMETRY: [(&str, f32, f32); 5] = [
    ("clock", 207.0, 187.0),
    ("mp3-player", 250.0, 250.0),
    ("telegram", 55.0, 593.0),
    ("glyph-bar", 14.0, 167.0),
    ("folder", 67.0, 67.0),
];

/// The font stacks a theme may name (`FONT_DISPLAY_OPTIONS`, `FONT_BODY_OPTIONS`,
/// `FONT_MONO_OPTIONS`), and the family list Rune resolves for each. Ndot and Geist are
/// embedded (`host.rs`); Segoe UI and Consolas load from the system. A CSS generic family
/// (`monospace`, `sans-serif`, `system-ui`) is Rune's slot fallback.
const FONT_STACKS: [(&str, &str); 7] = [
    ("'Ndot', monospace", "Ndot, Ndot 55"),
    ("'Geist', sans-serif", "Geist"),
    ("'Segoe UI', 'Geist', sans-serif", "Segoe UI, Geist"),
    ("system-ui, sans-serif", "Segoe UI"),
    ("'Consolas', 'Courier New', monospace", "Consolas, Courier New"),
    ("'Cascadia Code', 'Consolas', monospace", "Cascadia Code, Consolas"),
    ("ui-monospace, monospace", "ui-monospace"),
];
const DISPLAY_FONTS: [&str; 4] = ["'Ndot', monospace", "'Geist', sans-serif", "'Segoe UI', 'Geist', sans-serif", "system-ui, sans-serif"];
const BODY_FONTS: [&str; 4] = ["'Geist', sans-serif", "'Segoe UI', 'Geist', sans-serif", "system-ui, sans-serif", "'Ndot', monospace"];
const MONO_FONTS: [&str; 3] = ["'Consolas', 'Courier New', monospace", "'Cascadia Code', 'Consolas', monospace", "ui-monospace, monospace"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// Nothing Classic: dark glass cards.
    Flat,
    /// Sea Glass: pastel pebbles with sheen, bevel, grain and a drop shadow.
    Jelly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub id: String,
    pub material: Material,
    pub accent: Color,
    pub on_accent: Color,
    pub bg_base: Color,
    pub fg: Color,
    pub bg_elevated: Color,
    pub scrim: Color,
    pub surface: Color,
    pub surface_deep: Color,
    pub status_ok: Color,
    pub status_warn: Color,
    pub status_error: Color,
    pub status_info: Color,
    pub glass_opacity: f32,
    pub glass_rim: f32,
    pub glass_sheen: f32,
    pub glass_shadow: f32,
    pub blur: f32,
    pub radius_card: f32,
    pub radius_surface: f32,
    pub motion_scale: f32,
    /// `motionProfile`: responsive, balanced or fluid (`motion.rs`).
    pub motion_profile: &'static str,
    pub glass_grain: f32,
    pub glass_depth: f32,
    pub ink_depth: f32,
    pub surface_tint: f32,
    pub shadow_offset: f32,
    pub radius_strip: f32,
    pub font_display: &'static str,
    pub font_body: &'static str,
    pub font_mono: &'static str,
    palette: BTreeMap<String, Color>,
    tint_map: BTreeMap<String, String>,
    sizes: BTreeMap<String, (f32, f32)>,
}

fn color(value: &str) -> Color {
    hex(value).unwrap_or(Color::BLACK)
}

/// `"r, g, b"` (the page's `*Rgb` tokens) as a color; `isRgbString` decides what is valid.
fn rgb_triplet(value: &str) -> Option<Color> {
    let parts: Vec<u8> = value.split(',').map(|part| part.trim().parse().ok()).collect::<Option<_>>()?;
    let [r, g, b] = parts[..] else { return None };
    hex(&format!("#{r:02x}{g:02x}{b:02x}"))
}

/// `normalizeHex`: `#rgb` or `#rrggbb` only.
fn hex_field(value: &Value) -> Option<Color> {
    let text = value.as_str()?.trim();
    let digits = text.strip_prefix('#')?;
    (matches!(digits.len(), 3 | 6) && digits.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex(text)).flatten()
}

impl Theme {
    /// Nothing Classic (`BUILTIN_PRESETS[0]`), the base of every unknown theme.
    pub fn nothing_classic() -> Theme {
        Theme {
            id: "nothing-dark".into(),
            material: Material::Flat,
            accent: color("#d71921"),
            on_accent: Color::WHITE,
            bg_base: color("#0f121e"),
            fg: Color::WHITE,
            bg_elevated: Color::WHITE,
            scrim: Color::BLACK,
            surface: color("#121218"),
            surface_deep: color("#0c0c10"),
            status_ok: color("#34c759"),
            status_warn: color("#ffd60a"),
            status_error: color("#ff3b30"),
            status_info: color("#0a84ff"),
            glass_opacity: 0.78,
            glass_rim: 0.0,
            glass_sheen: 0.0,
            glass_shadow: 0.0,
            blur: 16.0,
            radius_card: 16.0,
            radius_surface: 24.0,
            motion_scale: 1.0,
            motion_profile: "balanced",
            glass_grain: 0.0,
            glass_depth: 0.0,
            ink_depth: 0.3,
            surface_tint: 0.35,
            shadow_offset: 0.0,
            radius_strip: 24.0,
            font_display: "'Ndot', monospace",
            font_body: "'Geist', sans-serif",
            font_mono: "'Consolas', 'Courier New', monospace",
            palette: SEA_PALETTE.iter().map(|(slot, value)| (slot.to_string(), color(value))).collect(),
            tint_map: [("panel", "lilac"), ("folder", "mint")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            sizes: PRESET_GEOMETRY.iter().map(|(id, w, h)| (id.to_string(), (*w, *h))).collect(),
        }
    }

    /// Sea Glass (`BUILTIN_PRESETS[1]`).
    pub fn sea_glass() -> Theme {
        Theme {
            id: "sea-glass".into(),
            material: Material::Jelly,
            accent: color("#4a5fc1"),
            on_accent: Color::WHITE,
            bg_base: color("#f2f4f7"),
            fg: color("#1e2230"),
            bg_elevated: Color::WHITE,
            scrim: color("#283046"),
            surface: color("#f2f4f7"),
            surface_deep: color("#e2e6ee"),
            status_ok: color("#1a7f37"),
            status_warn: color("#9a6700"),
            status_error: color("#c4402a"),
            status_info: color("#0969da"),
            glass_opacity: 0.9,
            glass_rim: 0.85,
            glass_sheen: 0.6,
            glass_shadow: 0.6,
            blur: 0.0,
            radius_card: 20.0,
            radius_surface: 30.0,
            motion_scale: 1.0,
            glass_grain: 0.22,
            glass_depth: 0.6,
            ink_depth: 0.3,
            surface_tint: 0.35,
            shadow_offset: 10.0,
            radius_strip: 999.0,
            font_display: "'Ndot', monospace",
            font_body: "'Segoe UI', 'Geist', sans-serif",
            font_mono: "'Consolas', 'Courier New', monospace",
            ..Theme::nothing_classic()
        }
    }

    fn builtin(id: &str) -> Option<Theme> {
        match id {
            "nothing-dark" => Some(Theme::nothing_classic()),
            "sea-glass" => Some(Theme::sea_glass()),
            _ => None,
        }
    }

    /// `normalizeTheme`: the stored record onto the full schema. A built-in stored under an
    /// older schema takes the current built-in style and keeps only its sizes.
    pub fn normalize(raw: &Value) -> Theme {
        let Some(record) = raw.as_object() else { return Theme::nothing_classic() };
        let builtin = record.get("id").and_then(Value::as_str).and_then(Theme::builtin);
        let is_builtin = builtin.is_some();
        let mut theme = builtin.unwrap_or_else(Theme::nothing_classic);
        let legacy = is_builtin && record.get("schemaVersion").and_then(Value::as_u64).is_none_or(|v| v < SCHEMA_VERSION);
        if !legacy {
            theme.apply(record);
        }
        if let Some(sizes) = record.get("sizes").and_then(Value::as_object) {
            for (id, size) in sizes {
                let side = |key: &str| size.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).map(|n| n.clamp(8.0, 2000.0).round() as f32);
                if let (Some(w), Some(h), true) = (side("w"), side("h"), (1..=64).contains(&id.len())) {
                    theme.sizes.insert(id.clone(), (w, h));
                }
            }
        }
        theme
    }

    fn apply(&mut self, record: &serde_json::Map<String, Value>) {
        let field = |key: &str| record.get(key).unwrap_or(&Value::Null);
        for (key, slot) in [
            ("accent", &mut self.accent),
            ("statusOk", &mut self.status_ok),
            ("statusWarn", &mut self.status_warn),
            ("statusError", &mut self.status_error),
            ("statusInfo", &mut self.status_info),
        ] {
            if let Some(value) = hex_field(field(key)) {
                *slot = value;
            }
        }
        for (key, slot) in [
            ("bgBase", &mut self.bg_base),
            ("fgRgb", &mut self.fg),
            ("onAccentRgb", &mut self.on_accent),
            ("bgElevRgb", &mut self.bg_elevated),
            ("scrimRgb", &mut self.scrim),
            ("surfaceRgb", &mut self.surface),
            ("surfaceDeepRgb", &mut self.surface_deep),
        ] {
            if let Some(value) = field(key).as_str().and_then(rgb_triplet) {
                *slot = value;
            }
        }
        // `NUM_FIELDS`: [min, max, rounded].
        for (key, slot, min, max, round) in [
            ("glassOpacity", &mut self.glass_opacity, 0.0, 1.0, false),
            ("glassRim", &mut self.glass_rim, 0.0, 1.0, false),
            ("glassSheen", &mut self.glass_sheen, 0.0, 1.0, false),
            ("glassShadow", &mut self.glass_shadow, 0.0, 1.0, false),
            ("blur", &mut self.blur, 0.0, 48.0, true),
            ("radiusCard", &mut self.radius_card, 0.0, 32.0, true),
            ("radiusSurface", &mut self.radius_surface, 0.0, 48.0, true),
            ("motionScale", &mut self.motion_scale, 0.0, 3.0, false),
            ("glassGrain", &mut self.glass_grain, 0.0, 1.0, false),
            ("glassDepth", &mut self.glass_depth, 0.0, 1.0, false),
            ("inkDepth", &mut self.ink_depth, 0.15, 0.38, false),
            ("surfaceTint", &mut self.surface_tint, 0.0, 1.0, false),
            ("shadowOffset", &mut self.shadow_offset, 0.0, 24.0, true),
            ("radiusStrip", &mut self.radius_strip, 0.0, 999.0, true),
        ] {
            if let Some(n) = field(key).as_f64().filter(|n| n.is_finite()) {
                let n = n.clamp(min, max);
                *slot = if round { n.round() } else { n } as f32;
            }
        }
        for (key, slot, allowed) in [
            ("fontDisplay", &mut self.font_display, &DISPLAY_FONTS[..]),
            ("fontBody", &mut self.font_body, &BODY_FONTS[..]),
            ("fontMono", &mut self.font_mono, &MONO_FONTS[..]),
        ] {
            if let Some(stack) = field(key).as_str().and_then(|value| allowed.iter().find(|option| **option == value)) {
                *slot = stack;
            }
        }
        if let Some(profile) = field("motionProfile").as_str().and_then(|value| ["responsive", "balanced", "fluid"].into_iter().find(|p| *p == value)) {
            self.motion_profile = profile;
        }
        match field("material").as_str() {
            Some("flat") => self.material = Material::Flat,
            Some("jelly") => self.material = Material::Jelly,
            _ => {}
        }
        if let Some(palette) = field("palette").as_object() {
            for slot in PALETTE_SLOTS {
                if let Some(value) = palette.get(slot).and_then(hex_field) {
                    self.palette.insert(slot.to_string(), value);
                }
            }
        }
        if let Some(map) = field("tintMap").as_object() {
            for (target, slot) in map {
                let well_formed = (1..=64).contains(&target.len()) && target.chars().all(|c| c.is_alphanumeric() || "_.:-".contains(c));
                if let Some(slot) = slot.as_str().filter(|slot| well_formed && PALETTE_SLOTS.contains(slot)) {
                    self.tint_map.insert(target.clone(), slot.to_string());
                }
            }
        }
        if let Some(id) = field("id").as_str().filter(|id| (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c))) {
            self.id = id.to_string();
        }
    }

    /// `tintSlot`: the slot name a target wears.
    pub fn tint_slot(&self, target: &str, role: &str, default: &str) -> String {
        self.tint_map
            .get(target)
            .or_else(|| self.tint_map.get(&format!("role:{role}")))
            .map_or_else(|| default.to_string(), String::clone)
    }

    /// `tintSlot`: the palette slot of a tint target, then of its role, then the module's
    /// default.
    pub fn tint(&self, target: &str, role: &str, default: &str) -> Color {
        let slot = self
            .tint_map
            .get(target)
            .or_else(|| self.tint_map.get(&format!("role:{role}")))
            .map_or(default, String::as_str);
        self.palette.get(slot).or_else(|| self.palette.get("neutral")).copied().unwrap_or(Color::WHITE)
    }

    /// The text and stroke color of a widget with this tint: the tint's ink under jelly
    /// (`--own-ink`), the theme's foreground under flat.
    pub fn ink_on(&self, tint: Color) -> Color {
        match self.material {
            Material::Jelly => ink_for(tint, self.ink_depth),
            Material::Flat => self.fg,
        }
    }

    /// `resolvePresetSize`: the theme's default size of a widget type.
    pub fn size_of(&self, widget: &str) -> Option<(f32, f32)> {
        self.sizes.get(widget).copied()
    }

    /// CSS's `line-height: normal` of a theme font stack, as a share of the size: the first
    /// font's own metrics (Ndot's and Geist's typographic ones, which they ask for; Segoe UI's
    /// and Consolas's ascent and descent). What a laid-out height computes with; the text itself
    /// is laid out with `TextStyle::lh_normal`.
    pub fn normal_line(stack: &str) -> f32 {
        match stack {
            "'Ndot', monospace" => 1.2,
            "'Geist', sans-serif" => 1.3,
            "'Consolas', 'Courier New', monospace" | "ui-monospace, monospace" => 1.1709,
            "'Cascadia Code', 'Consolas', monospace" => 1.1719,
            _ => 1.3301,
        }
    }

    /// The Rune family list for a theme font stack.
    pub fn family(stack: &str) -> Family {
        let list = FONT_STACKS.iter().find(|(css, _)| *css == stack).map_or("Segoe UI", |(_, rune)| rune);
        if list == "ui-monospace" { Family::SystemMono } else { Family::Named(list) }
    }

    /// The motion this theme asks for (`motionProfile`, `motionScale`), with the system's
    /// reduced-motion setting.
    pub fn motion(&self) -> crate::motion::Motion {
        crate::motion::Motion::new(crate::motion::Profile::parse(self.motion_profile), self.motion_scale)
    }
}

/// OKLab of a linear sRGB color: lightness, chroma, hue in radians.
fn oklch_of(color: Color) -> (f32, f32, f32) {
    let [l, a, b, _] = color.to_oklab();
    (l, a.hypot(b), b.atan2(a))
}

/// Linear sRGB of an OKLCH color (`oklchToLinear`); components may fall outside 0..=1.
fn linear_of(l: f32, c: f32, h: f32) -> [f32; 3] {
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_93 * s3,
        -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3,
        -0.004_196_086 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
    ]
}

fn in_gamut(rgb: [f32; 3]) -> bool {
    rgb.iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
}

/// Whether an OKLCH colour fits sRGB as it is (`inGamut(oklchToLinear(...))`).
pub fn linear_in_gamut(l: f32, c: f32, h: f32) -> bool {
    in_gamut(linear_of(l, c, h))
}

/// `linearToHex(oklchToLinear(l, c, h))`: `#rrggbb`, each channel clipped and rounded.
pub fn oklch_hex(l: f32, c: f32, h: f32) -> String {
    let encode = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let [r, g, b] = linear_of(l, c, h).map(encode);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The OKLCH hue of a `#rrggbb` colour, in degrees (`hexToOklch(hex)[2]` in degrees).
pub fn hue_degrees(hex_color: &str) -> f64 {
    let (_, _, h) = oklch_of(hex(hex_color).unwrap_or(Color::BLACK));
    f64::from(h).to_degrees()
}

/// An OKLCH color with its chroma reduced until it fits sRGB, as `inkFor` does and as CSS
/// maps an `oklch()` relative color into the display gamut.
pub fn oklch_in_gamut(l: f32, c: f32, h: f32, alpha: f32) -> Color {
    let mut chroma = c.max(0.0);
    let mut rgb = linear_of(l, chroma, h);
    while !in_gamut(rgb) && chroma > 0.005 {
        chroma *= 0.92;
        rgb = linear_of(l, chroma, h);
    }
    let clip = |v: f32| v.clamp(0.0, 1.0);
    Color { r: clip(rgb[0]), g: clip(rgb[1]), b: clip(rgb[2]), a: alpha }
}

/// `inkFor`: the deep same-hue ink of a pastel.
pub fn ink_for(pastel: Color, depth: f32) -> Color {
    let (_, chroma, hue) = oklch_of(pastel);
    oklch_in_gamut(depth, (chroma * 2.0 + 0.05).min(0.16), hue, 1.0)
}

/// `oklch(from tint calc(l + dl) calc(c * kc + dc) h / alpha)`.
pub fn derived(tint: Color, dl: f32, kc: f32, dc: f32, alpha: f32) -> Color {
    let (l, c, h) = oklch_of(tint);
    oklch_in_gamut(l + dl, c * kc + dc, h, alpha)
}

/// `oklch(from tint calc(l * kl) calc(c * kc + dc) h / alpha)`.
pub fn scaled(tint: Color, kl: f32, kc: f32, dc: f32, alpha: f32) -> Color {
    let (l, c, h) = oklch_of(tint);
    oklch_in_gamut(l * kl, c * kc + dc, h, alpha)
}

/// `color-mix(in oklch, a share, b)`: hue the shorter way round, as CSS does.
pub fn mix_oklch(a: Color, share: f32, b: Color) -> Color {
    let ((la, ca, ha), (lb, cb, hb)) = (oklch_of(a), oklch_of(b));
    // An achromatic color has no hue; CSS takes the other one's.
    let (ha, hb) = match (ca < 1e-4, cb < 1e-4) {
        (true, false) => (hb, hb),
        (false, true) => (ha, ha),
        _ => (ha, hb),
    };
    let mut delta = hb - ha;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    let t = 1.0 - share;
    oklch_in_gamut(la + (lb - la) * t, ca + (cb - ca) * t, ha + delta * t, a.a + (b.a - a.a) * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn srgb(color: Color) -> [i32; 3] {
        let value = color.hex();
        [1, 3, 5].map(|i| i32::from_str_radix(&value[i..i + 2], 16).unwrap())
    }

    /// The theme stored on the development machine (the user's Sea Glass colors).
    fn stored_theme() -> Theme {
        Theme::normalize(&json!({
            "id": "sea-glass", "schemaVersion": 4, "material": "jelly",
            "accent": "#e6cf94", "inkDepth": 0.3, "glassOpacity": 0.9, "radiusCard": 20, "shadowOffset": 10,
            "palette": { "lilac": "#cccbff", "coral": "#ffc0ae", "mint": "#febcd0" },
            "tintMap": { "clock": "lilac", "role:calm": "lilac", "folder": "coral" },
            "sizes": { "clock": { "w": 207, "h": 187 } },
        }))
    }

    #[test]
    fn the_clock_ink_is_the_one_the_page_paints() {
        // Measured on the production desk (a screenshot of the web build, Sea Glass with a
        // lilac of #cccbff): the clock's ring is rgb(46, 12, 120).
        let theme = stored_theme();
        let tint = theme.tint("clock", "calm", "lilac");
        assert_eq!(srgb(tint), [0xcc, 0xcb, 0xff]);
        let ink = srgb(theme.ink_on(tint));
        assert!(ink.iter().zip([46, 12, 120]).all(|(got, want)| (got - want).abs() <= 3), "ink {ink:?}");
        assert_eq!(srgb(theme.accent), [230, 207, 148]);
    }

    #[test]
    fn the_base_is_the_builtin_of_the_same_id_else_nothing_classic() {
        assert_eq!(Theme::normalize(&json!({ "id": "sea-glass", "schemaVersion": 4 })), Theme::sea_glass());
        let custom = Theme::normalize(&json!({ "id": "mine", "schemaVersion": 4, "glassOpacity": 0.5 }));
        assert_eq!((custom.material, custom.glass_opacity, custom.id.as_str()), (Material::Flat, 0.5, "mine"));
        assert_eq!(Theme::normalize(&json!(null)), Theme::nothing_classic());
    }

    #[test]
    fn a_builtin_from_an_older_schema_keeps_only_its_sizes() {
        let legacy = Theme::normalize(&json!({ "id": "sea-glass", "schemaVersion": 3, "accent": "#000000", "sizes": { "clock": { "w": 300, "h": 280 } } }));
        assert_eq!(legacy.accent, Theme::sea_glass().accent);
        assert_eq!(legacy.size_of("clock"), Some((300.0, 280.0)));
    }

    #[test]
    fn numbers_are_clamped_and_malformed_fields_keep_the_base() {
        let theme = Theme::normalize(&json!({
            "id": "sea-glass", "schemaVersion": 4, "radiusCard": 50.4, "inkDepth": 0.9, "shadowOffset": 3.6,
            "glassOpacity": "x", "accent": "nope", "fgRgb": "1, 2", "material": "glass", "fontDisplay": "Comic Sans",
            "palette": { "lilac": 5 }, "tintMap": { "clock": "purple" },
        }));
        assert_eq!((theme.radius_card, theme.ink_depth, theme.shadow_offset), (32.0, 0.38, 4.0));
        let base = Theme::sea_glass();
        assert_eq!((theme.glass_opacity, theme.accent, theme.fg, theme.material, theme.font_display), (base.glass_opacity, base.accent, base.fg, base.material, base.font_display));
        assert_eq!(srgb(theme.tint("clock", "calm", "lilac")), [0xc9, 0xb8, 0xf2]);
    }

    #[test]
    fn a_tint_falls_back_to_its_role_then_to_the_module_default() {
        let theme = stored_theme();
        assert_eq!(srgb(theme.tint("unknown", "calm", "mint")), [0xcc, 0xcb, 0xff]);
        assert_eq!(srgb(theme.tint("unknown", "nobody", "coral")), [0xff, 0xc0, 0xae]);
    }

    #[test]
    fn flat_widgets_paint_with_the_theme_foreground() {
        let classic = Theme::nothing_classic();
        assert_eq!(classic.ink_on(color("#cccbff")), Color::WHITE);
    }

    #[test]
    fn font_stacks_map_to_rune_families() {
        assert_eq!(Theme::family("'Ndot', monospace"), Family::Named("Ndot, Ndot 55"));
        assert_eq!(Theme::family("'Segoe UI', 'Geist', sans-serif"), Family::Named("Segoe UI, Geist"));
        assert_eq!(Theme::family("ui-monospace, monospace"), Family::SystemMono);
    }

    #[test]
    fn color_mix_in_oklch_lands_between_its_ends() {
        let wash = mix_oklch(color("#cccbff"), 0.35, Color::WHITE);
        let [r, g, b] = srgb(wash);
        assert!(r > 0xcc && g > 0xcb && b >= 0xfe, "{:?}", [r, g, b]);
        assert_eq!(srgb(mix_oklch(color("#cccbff"), 1.0, Color::WHITE)), [0xcc, 0xcb, 0xff]);
    }

    #[test]
    fn a_chroma_that_does_not_fit_srgb_is_reduced_not_clipped() {
        let (l, _, _) = oklch_of(oklch_in_gamut(0.3, 0.4, 4.7, 1.0));
        assert!((l - 0.3).abs() < 0.02);
    }
}
