//! The theme as the page stores it: a JSON record that `normalizeTheme` completes from a
//! built-in preset, the per-theme colour overrides, the user's own presets and the first-run
//! roll of personal Sea Glass colours (`core/ui/theme.js`). The menu edits these records and
//! writes them back through `desk_apply`, so the page reads exactly what it would have written
//! itself; `theme.rs` turns a normalized record into the colours the host paints with.

use serde_json::{json, Map, Value};

pub const SCHEMA_VERSION: u64 = 4;
pub const USER_COLOR_PRESET: &str = "sea-glass";
pub const SLOTS: [&str; 8] = ["neutral", "blue", "aqua", "mint", "lilac", "peach", "magenta", "coral"];

const HEX_FIELDS: [&str; 5] = ["accent", "statusOk", "statusWarn", "statusError", "statusInfo"];
const RGB_FIELDS: [&str; 7] = ["bgBase", "fgRgb", "onAccentRgb", "bgElevRgb", "scrimRgb", "surfaceRgb", "surfaceDeepRgb"];
/// `NUM_FIELDS`: field, min, max, rounded.
const NUM_FIELDS: [(&str, f64, f64, bool); 15] = [
    ("glassOpacity", 0.0, 1.0, false),
    ("glassRim", 0.0, 1.0, false),
    ("glassSheen", 0.0, 1.0, false),
    ("glassShadow", 0.0, 1.0, false),
    ("blur", 0.0, 48.0, true),
    ("radiusCard", 0.0, 32.0, true),
    ("radiusSurface", 0.0, 48.0, true),
    ("motionScale", 0.0, 3.0, false),
    ("glassGrain", 0.0, 1.0, false),
    ("glassDepth", 0.0, 1.0, false),
    ("inkDepth", 0.15, 0.38, false),
    ("surfaceTint", 0.0, 1.0, false),
    ("shadowOffset", 0.0, 24.0, true),
    ("radiusStrip", 0.0, 999.0, true),
    // Not in the page's table, kept here so the order above matches it.
    ("", 0.0, 0.0, false),
];
const FONT_FIELDS: [(&str, &[&str]); 3] = [
    ("fontDisplay", &["'Ndot', monospace", "'Geist', sans-serif", "'Segoe UI', 'Geist', sans-serif", "system-ui, sans-serif"]),
    ("fontBody", &["'Geist', sans-serif", "'Segoe UI', 'Geist', sans-serif", "system-ui, sans-serif", "'Ndot', monospace"]),
    ("fontMono", &["'Consolas', 'Courier New', monospace", "'Cascadia Code', 'Consolas', monospace", "ui-monospace, monospace"]),
];
const ENUM_FIELDS: [(&str, &[&str]); 2] = [("material", &["flat", "jelly"]), ("mgLayout", &["panel", "pills"])];
/// Every colour a user can edit besides the palette and tint map (`COLOR_FIELDS`).
pub const COLOR_FIELDS: [&str; 13] = [
    "accent", "statusOk", "statusWarn", "statusError", "statusInfo",
    "bgBase", "fgRgb", "onAccentRgb", "bgElevRgb", "scrimRgb", "surfaceRgb", "surfaceDeepRgb", "inkDepth",
];
/// Presets retired for the Nothing Classic baseline; a record naming one falls back to it.
const RETIRED: [&str; 4] = ["pure-oled", "cyber-amber", "frost-glass", "monochrome-slate"];

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

fn palette_json() -> Value {
    Value::Object(SEA_PALETTE.iter().map(|(k, v)| (k.to_string(), json!(v))).collect())
}

fn sizes_json() -> Value {
    json!({
        "clock": { "w": 207, "h": 187 },
        "mp3-player": { "w": 250, "h": 250 },
        "telegram": { "w": 55, "h": 593 },
        "glyph-bar": { "w": 14, "h": 167 },
        "folder": { "w": 67, "h": 67 },
    })
}

/// `BUILTIN_PRESETS`, in the page's order: Nothing Classic first, then Sea Glass.
pub fn builtins() -> [Value; 2] {
    [
        json!({
            "id": "nothing-dark", "name": "Nothing Classic", "isBuiltin": true, "schemaVersion": SCHEMA_VERSION,
            "accent": "#d71921", "bgBase": "15, 18, 30", "fgRgb": "255, 255, 255", "onAccentRgb": "255, 255, 255",
            "bgElevRgb": "255, 255, 255", "scrimRgb": "0, 0, 0", "surfaceRgb": "18, 18, 24", "surfaceDeepRgb": "12, 12, 16",
            "statusOk": "#34c759", "statusWarn": "#ffd60a", "statusError": "#ff3b30", "statusInfo": "#0a84ff",
            "glassOpacity": 0.78, "glassRim": 0, "glassSheen": 0, "glassShadow": 0, "blur": 16,
            "radiusCard": 16, "radiusSurface": 24, "motionScale": 1.0, "motionProfile": "balanced",
            "fontDisplay": "'Ndot', monospace", "fontBody": "'Geist', sans-serif",
            "fontMono": "'Consolas', 'Courier New', monospace",
            "sizes": sizes_json(), "material": "flat", "palette": palette_json(),
            "tintMap": { "panel": "lilac", "folder": "mint" },
            "glassGrain": 0, "glassDepth": 0, "inkDepth": 0.3, "shadowOffset": 0, "radiusStrip": 24,
            "surfaceTint": 0.35, "mgLayout": "panel", "avatarColor": false,
        }),
        json!({
            "id": "sea-glass", "name": "Sea Glass", "isBuiltin": true, "schemaVersion": SCHEMA_VERSION,
            "accent": "#4a5fc1", "bgBase": "242, 244, 247", "fgRgb": "30, 34, 48", "onAccentRgb": "255, 255, 255",
            "bgElevRgb": "255, 255, 255", "scrimRgb": "40, 48, 70", "surfaceRgb": "242, 244, 247", "surfaceDeepRgb": "226, 230, 238",
            "statusOk": "#1a7f37", "statusWarn": "#9a6700", "statusError": "#c4402a", "statusInfo": "#0969da",
            "glassOpacity": 0.9, "glassRim": 0.85, "glassSheen": 0.6, "glassShadow": 0.6, "blur": 0,
            "radiusCard": 20, "radiusSurface": 30, "motionScale": 1.0, "motionProfile": "balanced",
            "fontDisplay": "'Ndot', monospace", "fontBody": "'Segoe UI', 'Geist', sans-serif",
            "fontMono": "'Consolas', 'Courier New', monospace",
            "sizes": sizes_json(), "material": "jelly", "palette": palette_json(),
            "tintMap": { "panel": "lilac", "folder": "mint" },
            "glassGrain": 0.22, "glassDepth": 0.6, "inkDepth": 0.3, "shadowOffset": 10, "radiusStrip": 999,
            "surfaceTint": 0.35, "mgLayout": "pills", "avatarColor": true,
        }),
    ]
}

pub fn builtin(id: &str) -> Option<Value> {
    builtins().into_iter().find(|preset| preset["id"] == id)
}

pub fn is_retired(record: &Value) -> bool {
    record.get("id").and_then(Value::as_str).is_some_and(|id| RETIRED.contains(&id))
}

/// `normalizeHex`: `#rgb` or `#rrggbb`, lower case, six digits.
pub fn normalize_hex(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 3 | 6) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digits = if digits.len() == 3 { digits.chars().flat_map(|c| [c, c]).collect() } else { digits.to_string() };
    Some(format!("#{}", digits.to_ascii_lowercase()))
}

/// `isRgbString` and its re-join: `"r, g, b"` with each part at most 255.
pub fn normalize_rgb(value: &Value) -> Option<String> {
    let parts: Vec<u32> = value.as_str()?.split(',').map(|part| part.trim().parse().ok()).collect::<Option<_>>()?;
    match parts[..] {
        [r, g, b] if r <= 255 && g <= 255 && b <= 255 => Some(format!("{r}, {g}, {b}")),
        _ => None,
    }
}

/// `hexToRgb`.
pub fn hex_to_rgb(hex: &str) -> String {
    let digits = hex.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    if digits.len() >= 6 { format!("{}, {}, {}", byte(0), byte(2), byte(4)) } else { "255, 255, 255".into() }
}

fn valid_tint_key(key: &str) -> bool {
    (1..=64).contains(&key.len()) && key.chars().all(|c| c.is_alphanumeric() || "_.:-".contains(c))
}

/// `normalizeTheme`: any stored record onto the full schema. The base is the built-in preset
/// with the same id, Nothing Classic otherwise; a built-in stored under an older schema takes
/// the current built-in style and keeps only its sizes.
pub fn normalize(raw: &Value) -> Value {
    let record = raw.as_object();
    let builtin_base = record.and_then(|r| r.get("id")).and_then(Value::as_str).and_then(builtin);
    let is_builtin_id = builtin_base.is_some();
    let base = builtin_base.unwrap_or_else(|| builtins()[0].clone());
    let mut theme = base.as_object().cloned().unwrap_or_default();
    let legacy = is_builtin_id && record.and_then(|r| r.get("schemaVersion")).and_then(Value::as_f64).is_none_or(|v| v < SCHEMA_VERSION as f64);
    if let (Some(record), false) = (record, legacy) {
        for field in HEX_FIELDS {
            if let Some(hex) = record.get(field).and_then(normalize_hex) {
                theme.insert(field.into(), json!(hex));
            }
        }
        for field in RGB_FIELDS {
            if let Some(rgb) = record.get(field).and_then(normalize_rgb) {
                theme.insert(field.into(), json!(rgb));
            }
        }
        for (field, min, max, round) in NUM_FIELDS.iter().filter(|(field, ..)| !field.is_empty()) {
            if let Some(n) = record.get(*field).and_then(Value::as_f64).filter(|n| n.is_finite()) {
                let n = n.clamp(*min, *max);
                theme.insert(field.to_string(), if *round { json!(n.round() as i64) } else { json!(n) });
            }
        }
        for (field, options) in FONT_FIELDS.iter().chain(ENUM_FIELDS.iter()) {
            if let Some(value) = record.get(*field).and_then(Value::as_str).filter(|v| options.contains(v)) {
                theme.insert(field.to_string(), json!(value));
            }
        }
        if let Some(avatar) = record.get("avatarColor").and_then(Value::as_bool) {
            theme.insert("avatarColor".into(), json!(avatar));
        }
        let mut palette = base["palette"].as_object().cloned().unwrap_or_default();
        if let Some(stored) = record.get("palette").and_then(Value::as_object) {
            for slot in SLOTS {
                if let Some(hex) = stored.get(slot).and_then(normalize_hex) {
                    palette.insert(slot.into(), json!(hex));
                }
            }
        }
        theme.insert("palette".into(), Value::Object(palette));
        let mut tints = base["tintMap"].as_object().cloned().unwrap_or_default();
        if let Some(stored) = record.get("tintMap").and_then(Value::as_object) {
            for (key, slot) in stored {
                if valid_tint_key(key) && slot.as_str().is_some_and(|s| SLOTS.contains(&s)) {
                    tints.insert(key.clone(), slot.clone());
                }
            }
        }
        theme.insert("tintMap".into(), Value::Object(tints));
        if let Some(profile) = record.get("motionProfile").and_then(Value::as_str).filter(|p| ["responsive", "balanced", "fluid"].contains(p)) {
            theme.insert("motionProfile".into(), json!(profile));
        }
        if let Some(id) = record.get("id").and_then(Value::as_str).filter(|id| (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c))) {
            theme.insert("id".into(), json!(id));
        }
        if let Some(name) = record.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
            theme.insert("name".into(), json!(name.chars().take(32).collect::<String>()));
        }
    }
    if let Some(record) = record {
        let mut sizes = base["sizes"].as_object().cloned().unwrap_or_default();
        if let Some(stored) = record.get("sizes").and_then(Value::as_object) {
            for (id, size) in stored {
                let side = |key: &str| size.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).map(|n| n.clamp(8.0, 2000.0).round() as i64);
                if let (Some(w), Some(h), true) = (side("w"), side("h"), (1..=64).contains(&id.len())) {
                    sizes.insert(id.clone(), json!({ "w": w, "h": h }));
                }
            }
        }
        theme.insert("sizes".into(), Value::Object(sizes));
    }
    let accent = theme.get("accent").and_then(Value::as_str).unwrap_or("#d71921").to_string();
    theme.insert("accentRgb".into(), json!(hex_to_rgb(&accent)));
    let id = theme.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    theme.insert("isBuiltin".into(), json!(builtin(&id).is_some()));
    theme.insert("schemaVersion".into(), json!(SCHEMA_VERSION));
    Value::Object(theme)
}

/// `colorsOf`: the colour part of a theme, what is remembered per theme id.
pub fn colors_of(theme: &Value) -> Value {
    let mut colors = Map::new();
    colors.insert("palette".into(), theme.get("palette").cloned().unwrap_or(json!({})));
    colors.insert("tintMap".into(), theme.get("tintMap").cloned().unwrap_or(json!({})));
    for field in COLOR_FIELDS {
        if let Some(value) = theme.get(field) {
            colors.insert(field.into(), value.clone());
        }
    }
    Value::Object(colors)
}

/// `withColorOverrides`: the preset with the user's colours for its id.
pub fn with_color_overrides(preset: &Value, overrides: &Value) -> Value {
    let mut theme = preset.clone();
    let id = preset.get("id").and_then(Value::as_str).unwrap_or_default();
    let Some(own) = overrides.get(id).and_then(Value::as_object) else { return theme };
    let Some(target) = theme.as_object_mut() else { return theme };
    for field in COLOR_FIELDS {
        if let Some(value) = own.get(field) {
            target.insert(field.into(), value.clone());
        }
    }
    for key in ["palette", "tintMap"] {
        let mut merged = target.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
        if let Some(extra) = own.get(key).and_then(Value::as_object) {
            merged.extend(extra.clone());
        }
        target.insert(key.into(), Value::Object(merged));
    }
    theme
}

/// The tint targets the theme knows (`tintRegistry`): the core's own two, then each module's
/// `tints`, as `(id, default slot, role)`.
pub fn tint_targets<'a>(modules: impl Iterator<Item = &'a Value>) -> Vec<(String, String, String)> {
    let mut targets = vec![
        ("panel".to_string(), "lilac".to_string(), "calm".to_string()),
        ("folder".to_string(), "mint".to_string(), "folder".to_string()),
    ];
    for meta in modules {
        for tint in meta.get("tints").and_then(Value::as_array).into_iter().flatten() {
            let (Some(id), Some(default)) = (tint.get("id").and_then(Value::as_str), tint.get("default").and_then(Value::as_str)) else { continue };
            if !valid_tint_key(id) || !SLOTS.contains(&default) {
                continue;
            }
            let role = tint.get("role").and_then(Value::as_str).unwrap_or_default().to_string();
            match targets.iter_mut().find(|(existing, ..)| existing == id) {
                Some(entry) => *entry = (id.to_string(), default.to_string(), role),
                None => targets.push((id.to_string(), default.to_string(), role)),
            }
        }
    }
    targets
}

/// `mulberry32`, the page's seeded generator.
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = (self.0 ^ (self.0 >> 15)).wrapping_mul(1 | self.0);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

const ROLL_L: f32 = 0.86;
const ROLL_C: f32 = 0.08;
const ROLL_MIN_GAP: f64 = 35.0;
const OLIVE_BAND: (f64, f64) = (90.0, 125.0);
const ROLL_ROLES: [&str; 7] = ["folder", "calm", "social", "player", "calendar", "weather", "sky"];

/// `pastelHex`: the hue at the roll's lightness, chroma cut to fit sRGB by bisection.
fn pastel_hex(hue_deg: f64) -> String {
    let h = (hue_deg.to_radians()) as f32;
    let fits = |c: f32| crate::theme::linear_in_gamut(ROLL_L, c, h);
    let mut chroma = ROLL_C;
    if !fits(chroma) {
        let (mut lo, mut hi) = (0.0f32, ROLL_C);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            if fits(mid) { lo = mid } else { hi = mid }
        }
        chroma = lo;
    }
    crate::theme::oklch_hex(ROLL_L, chroma, h)
}

fn circular_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

/// `rollColors(seed)`: seven pastels of equal lightness at random, well separated hues
/// (the olive band excluded), coral kept, dealt over the seven coloured roles.
pub fn roll_colors(seed: u32, targets: &[(String, String, String)]) -> Value {
    let mut rand = Mulberry32(seed);
    let span = 360.0 - (OLIVE_BAND.1 - OLIVE_BAND.0);
    let coral_hue = crate::theme::hue_degrees("#f7a38c");
    let chroma_slots: Vec<&str> = SLOTS.iter().copied().filter(|s| *s != "neutral").collect();
    let mut hues: Vec<f64> = Vec::new();
    while hues.len() < chroma_slots.len() - 1 {
        let mut h = rand.next() * span;
        if h >= OLIVE_BAND.0 {
            h += OLIVE_BAND.1 - OLIVE_BAND.0;
        }
        if std::iter::once(coral_hue).chain(hues.iter().copied()).all(|x| circular_gap(x, h) >= ROLL_MIN_GAP) {
            hues.push(h);
        } else if !hues.is_empty() && rand.next() < 0.02 {
            hues.clear();
        }
    }
    let mut palette = Map::new();
    palette.insert("neutral".into(), json!("#f2f4f7"));
    palette.insert("coral".into(), json!(pastel_hex(coral_hue)));
    for (slot, hue) in chroma_slots.iter().filter(|s| **s != "coral").zip(&hues) {
        palette.insert(slot.to_string(), json!(pastel_hex(*hue)));
    }
    let mut deck: Vec<&str> = chroma_slots.clone();
    for i in (1..deck.len()).rev() {
        let j = (rand.next() * (i + 1) as f64).floor() as usize;
        deck.swap(i, j);
    }
    let mut tint_map = Map::new();
    for (role, slot) in ROLL_ROLES.iter().zip(&deck) {
        tint_map.insert(format!("role:{role}"), json!(slot));
    }
    for (target, _, role) in targets {
        if let Some(slot) = tint_map.get(&format!("role:{role}")).cloned() {
            tint_map.insert(target.clone(), slot);
        }
    }
    json!({ "palette": palette, "tintMap": tint_map })
}

/// A random seed, as `newColorSeed` takes one from the platform's generator.
pub fn new_color_seed() -> u32 {
    let mut bytes = [0u8; 4];
    if getrandom(&mut bytes) {
        u32::from_le_bytes(bytes)
    } else {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos())
    }
}

fn getrandom(bytes: &mut [u8]) -> bool {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    unsafe { BCryptGenRandom(None, bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_ok() }
}

/// Widget ids whose sizes presets never manage (`SIZE_SKIP_IDS`), and simulation ids.
fn size_managed(id: &str) -> bool {
    !["micrographics", "vpn", "glyph", "audio-circle"].contains(&id) && !id.starts_with("simulation")
}

/// `migrateSizesToPreset` / `resetSizesToPreset`: the position records whose size moves to
/// `new_theme`'s defaults, as `(id, w, h)`. Without `old_theme` (the reset) every known record
/// moves; with it (a preset change) only records that still hold the old preset's default,
/// or no size at all. Folders (any id the sizes do not name) take the `folder` size.
pub fn size_moves(positions: &Value, new_theme: &Value, old_theme: Option<&Value>) -> Vec<(String, i64, i64)> {
    let Some(positions) = positions.as_object() else { return Vec::new() };
    let sizes = new_theme.get("sizes").and_then(Value::as_object).cloned().unwrap_or_default();
    let old_sizes = old_theme.and_then(|t| t.get("sizes")).and_then(Value::as_object).cloned().unwrap_or_default();
    let size_of = |sizes: &Map<String, Value>, key: &str| sizes.get(key).and_then(|s| Some((s.get("w")?.as_i64()?, s.get("h")?.as_i64()?)));
    let mut moves = Vec::new();
    let mut consider = |id: &str, key: &str| {
        let (Some(next), Some(current)) = (size_of(&sizes, key), positions.get(id)) else { return };
        let current_size = (current.get("w").and_then(Value::as_f64), current.get("h").and_then(Value::as_f64));
        let moves_now = match (old_theme, current_size) {
            (None, _) => true,
            (Some(_), (None, _) | (_, None)) => true,
            (Some(_), (Some(w), Some(h))) => size_of(&old_sizes, key)
                .is_some_and(|old| w == old.0 as f64 && h == old.1 as f64 && (w != next.0 as f64 || h != next.1 as f64)),
        };
        if moves_now {
            moves.push((id.to_string(), next.0, next.1));
        }
    };
    for key in sizes.keys() {
        if key != "folder" && size_managed(key) && positions.contains_key(key) {
            consider(key, key);
        }
    }
    if sizes.contains_key("folder") {
        for id in positions.keys() {
            if !sizes.contains_key(id) && size_managed(id) {
                consider(id, "folder");
            }
        }
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_completes_a_record_from_its_builtin() {
        let theme = normalize(&json!({ "id": "sea-glass", "schemaVersion": 4, "accent": "#ABC", "radiusCard": 50.4, "fgRgb": "1,2, 3", "palette": { "lilac": "#CCCBFF", "bad": "#000" } }));
        assert_eq!(theme["accent"], "#aabbcc");
        assert_eq!(theme["accentRgb"], "170, 187, 204");
        assert_eq!(theme["radiusCard"], 32);
        assert_eq!(theme["fgRgb"], "1, 2, 3");
        assert_eq!(theme["palette"]["lilac"], "#cccbff");
        assert!(theme["palette"].get("bad").is_none());
        assert_eq!((theme["material"].as_str(), theme["mgLayout"].as_str(), theme["isBuiltin"].as_bool()), (Some("jelly"), Some("pills"), Some(true)));
        let custom = normalize(&json!({ "id": "custom-1", "name": "  Моя  ", "schemaVersion": 4 }));
        assert_eq!((custom["material"].as_str(), custom["name"].as_str(), custom["isBuiltin"].as_bool()), (Some("flat"), Some("Моя"), Some(false)));
        let legacy = normalize(&json!({ "id": "sea-glass", "schemaVersion": 3, "accent": "#000000", "sizes": { "clock": { "w": 300, "h": 280 } } }));
        assert_eq!(legacy["accent"], "#4a5fc1");
        assert_eq!(legacy["sizes"]["clock"], json!({ "w": 300, "h": 280 }));
    }

    #[test]
    fn overrides_carry_a_themes_colours_only() {
        let theme = normalize(&json!({ "id": "sea-glass", "schemaVersion": 4, "accent": "#e6cf94", "radiusCard": 12 }));
        let colors = colors_of(&theme);
        assert_eq!(colors["accent"], "#e6cf94");
        assert!(colors.get("radiusCard").is_none());
        let back = with_color_overrides(&builtins()[1], &json!({ "sea-glass": colors }));
        assert_eq!((back["accent"].as_str(), back["radiusCard"].as_i64()), (Some("#e6cf94"), Some(20)));
    }

    #[test]
    fn mulberry32_matches_the_page() {
        // The first numbers of `mulberry32(1)`, computed with JavaScript's `Math.imul` and
        // `>>>` semantics.
        let mut rand = Mulberry32(1);
        let first: Vec<String> = (0..3).map(|_| format!("{:.10}", rand.next())).collect();
        assert_eq!(first, ["0.6270739406", "0.0027357212", "0.5274470400"]);
    }

    #[test]
    fn a_roll_deals_seven_separated_pastels_over_the_roles() {
        let targets = tint_targets(std::iter::once(&json!({ "tints": [{ "id": "clock", "default": "lilac", "role": "calm" }] })));
        let roll = roll_colors(42, &targets);
        let palette = roll["palette"].as_object().unwrap();
        assert_eq!(palette.len(), 8);
        assert_eq!(palette["neutral"], "#f2f4f7");
        let roles: Vec<&str> = ROLL_ROLES.iter().map(|r| roll["tintMap"][format!("role:{r}")].as_str().unwrap()).collect();
        let mut dealt = roles.clone();
        dealt.sort_unstable();
        dealt.dedup();
        assert_eq!(dealt.len(), 7, "every coloured slot dealt once: {roles:?}");
        assert_eq!(roll["tintMap"]["clock"], roll["tintMap"]["role:calm"]);
        assert_eq!(roll_colors(42, &targets), roll, "pure for a seed");
    }

    #[test]
    fn sizes_follow_a_preset_only_where_the_user_kept_the_old_default() {
        let positions = json!({
            "clock": { "w": 207, "h": 187 }, "telegram": { "w": 70, "h": 600 }, "mp3-player": { "x": 1 },
            "folder_a": { "w": 67, "h": 67 }, "micrographics": { "w": 1, "h": 1 },
        });
        let mut bigger = builtins()[0].clone();
        bigger["sizes"]["clock"] = json!({ "w": 300, "h": 280 });
        bigger["sizes"]["folder"] = json!({ "w": 80, "h": 80 });
        let moves = size_moves(&positions, &bigger, Some(&builtins()[0]));
        assert!(moves.contains(&("clock".into(), 300, 280)));
        assert!(moves.contains(&("mp3-player".into(), 250, 250)), "no size yet");
        assert!(moves.contains(&("folder_a".into(), 80, 80)));
        assert!(!moves.iter().any(|(id, ..)| id == "telegram" || id == "micrographics"));
        let reset = size_moves(&positions, &builtins()[0], None);
        assert!(reset.contains(&("telegram".into(), 55, 593)), "the reset moves every known record");
    }
}
