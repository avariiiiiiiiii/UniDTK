//! The one change to the desk layout that a host other than the page may make: the position
//! of one widget (`desk_position_set`).
//!
//! The layout is a single JSON blob (`widget_settings/config`) that the page writes whole,
//! from memory. A native host must not do the same: it would erase whatever it does not
//! draw (module settings, folders, the theme). It names one widget and the position fields it
//! changes; the kernel merges them into the stored blob in one write transaction and
//! publishes the resulting `positions` map, so every host sees the same desk.

use serde_json::{json, Map, Value};

/// Published after every change of the layout. A state: only the newest map matters.
pub const EVENT: &str = "desk-layout-changed";

const MAX_ID_LEN: usize = 64;
const MAX_COORD: f64 = 100_000.0;

/// A position patch: the widget id and the validated fields to merge.
#[derive(Debug, PartialEq)]
pub struct PositionPatch {
    pub id: String,
    pub fields: Map<String, Value>,
}

/// Validates the arguments of `desk_position_set`. Only position fields pass: module
/// settings live in the same record and are changed by their modules, not through here.
pub fn parse_patch(args: &Map<String, Value>) -> Result<PositionPatch, String> {
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .ok_or("desk_position_set requires {id}")?;
    if id.is_empty()
        || id.len() > MAX_ID_LEN
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(format!("invalid widget id '{id}'"));
    }
    let mut fields = Map::new();
    for (key, value) in args {
        match key.as_str() {
            "id" => {}
            "x" | "y" | "ax" | "ay" => {
                fields.insert(key.clone(), number(key, value, f64::NEG_INFINITY)?);
            }
            "w" | "h" => {
                fields.insert(key.clone(), number(key, value, 0.0)?);
            }
            "anchor" => match value.as_str() {
                Some(a @ ("tl" | "tr" | "bl" | "br")) => {
                    fields.insert(key.clone(), json!(a));
                }
                _ => return Err("anchor must be one of tl, tr, bl, br".into()),
            },
            "version" => match value.as_str() {
                Some(v) if !v.is_empty()
                    && v.len() <= 32
                    && v.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') =>
                {
                    fields.insert(key.clone(), json!(v));
                }
                _ => return Err("version must be 1-32 characters of a-z, 0-9, -".into()),
            },
            other => return Err(format!("'{other}' is not a position field")),
        }
    }
    if fields.is_empty() {
        return Err("desk_position_set needs at least one position field".into());
    }
    Ok(PositionPatch { id: id.to_string(), fields })
}

/// A finite number within the canvas bounds; `above` is an exclusive lower limit.
fn number(key: &str, value: &Value, above: f64) -> Result<Value, String> {
    match value.as_f64() {
        Some(n) if n.is_finite() && n.abs() <= MAX_COORD && n > above => Ok(value.clone()),
        _ => Err(format!("{key} must be a finite number within ±{MAX_COORD}{}", if above == 0.0 { " and above 0" } else { "" })),
    }
}

/// The layout with `patch` merged into `positions[id]`. Everything else in the blob stays as
/// it was. A stored blob that is not an object is refused, never replaced: overwriting it
/// would destroy what the page might still be able to read. Nothing stored is refused too:
/// the page seeds a first layout from the modules' default placements, and a blob holding
/// only this one widget would pass for a complete layout and block that seeding.
pub fn merge_position(config: Option<Value>, patch: &PositionPatch) -> Result<Value, String> {
    let mut config = config.ok_or("no layout is stored yet; a host draws the default placements until the layout is first saved")?;
    let root = config.as_object_mut().ok_or("the stored layout is not a JSON object")?;
    let positions = root
        .entry("positions")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("the stored layout has a `positions` that is not an object")?;
    let entry = positions
        .entry(patch.id.clone())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("the stored position of '{}' is not an object", patch.id))?;
    for (key, value) in &patch.fields {
        entry.insert(key.clone(), value.clone());
    }
    Ok(config)
}

/// The `positions` map of a layout, `{}` when there is none.
pub fn positions_of(config: &Value) -> Value {
    config.get("positions").cloned().unwrap_or_else(|| json!({}))
}

/// Serializes every change of the layout with its publication and numbers the changes.
///
/// redb orders the commits, the event broker orders the frames, and nothing orders the two
/// against each other: two changes could be published in the reverse of their commit order,
/// and a host that keeps the last frame would end on the older layout. One lock held across
/// commit and publish makes the order the same, and `rev` lets a host that reads a snapshot
/// and receives frames tell which is newer. It never goes backwards across a restart: it
/// starts from the clock.
static REV: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

fn lock_rev() -> std::sync::MutexGuard<'static, u64> {
    REV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The next revision, strictly above the last one and above the clock.
fn bump(rev: &mut u64) -> u64 {
    *rev = (*rev + 1).max(now_ms());
    *rev
}

/// The current revision; before the first change of this run it is the clock's.
fn current(rev: &mut u64) -> u64 {
    if *rev == 0 {
        *rev = now_ms();
    }
    *rev
}

/// Tells every subscriber (the page, a native host) the layout changed. The caller holds
/// the revision lock, so frames leave in the order the changes were committed.
fn publish(config: &Value, rev: u64) {
    unidtk_api::emit(EVENT, json!({ "rev": rev, "positions": positions_of(config) }));
}

/// The layout as stored, with the revision it is current at. Read under the same lock as the
/// changes, so a frame with a higher `rev` is always newer than this answer.
pub fn snapshot(db: &crate::unidtk_db::UniDtkDatabase) -> Result<Value, String> {
    let mut rev = lock_rev();
    let config = db.get_widget_config()?;
    Ok(json!({ "rev": current(&mut rev), "config": config }))
}

/// The page's whole-layout save, kept in step with the host's merges: same lock, and the
/// change is published.
pub fn save(db: &crate::unidtk_db::UniDtkDatabase, config: &Value) -> Result<(), String> {
    let mut rev = lock_rev();
    db.set_widget_config(config)?;
    let rev = bump(&mut rev);
    publish(config, rev);
    Ok(())
}

/// `desk_position_set` for both doors: merge in one transaction, publish after the commit,
/// answer with the widget's record as stored.
pub fn set_position(db: &crate::unidtk_db::UniDtkDatabase, patch: &PositionPatch) -> Result<Value, String> {
    let mut rev = lock_rev();
    let merged = db.update_widget_config(|stored| merge_position(stored, patch))?;
    let rev = bump(&mut rev);
    publish(&merged, rev);
    Ok(json!({ "ok": true, "rev": rev, "position": merged["positions"][patch.id.as_str()] }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_position_patch_keeps_only_position_fields() {
        let patch = parse_patch(&args(json!({"id": "clock", "x": 510, "y": -10, "anchor": "tr", "ax": 24, "ay": 8, "w": 210, "h": 190, "version": "rings"}))).unwrap();
        assert_eq!(patch.id, "clock");
        assert_eq!(patch.fields.len(), 8, "x, y, anchor, ax, ay, w, h and version");
        assert!(parse_patch(&args(json!({"id": "clock", "tint": "lilac"}))).is_err(), "module settings are not position fields");
        assert!(parse_patch(&args(json!({"id": "clock"}))).is_err(), "an empty patch changes nothing");
    }

    #[test]
    fn bad_ids_and_values_are_refused() {
        for bad in [
            json!({"x": 1}),
            json!({"id": "", "x": 1}),
            json!({"id": "a b", "x": 1}),
            json!({"id": "../x", "x": 1}),
            json!({"id": "a".repeat(65), "x": 1}),
            json!({"id": "clock", "x": "5"}),
            json!({"id": "clock", "x": 1e9}),
            json!({"id": "clock", "w": 0}),
            json!({"id": "clock", "h": -3}),
            json!({"id": "clock", "anchor": "center"}),
            json!({"id": "clock", "version": "Rings"}),
            json!({"id": "clock", "version": ""}),
        ] {
            assert!(parse_patch(&args(bad.clone())).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn a_merge_changes_one_widget_and_keeps_the_rest_of_the_layout() {
        let stored = json!({
            "theme": "sea-glass",
            "positions": {
                "clock": {"x": 1, "y": 2, "version": "digital", "tint": "lilac"},
                "glyph-bar": {"x": 9, "y": 9, "w": 56}
            },
            "unknown": [1, 2, 3]
        });
        let patch = parse_patch(&args(json!({"id": "clock", "x": 100, "anchor": "br"}))).unwrap();
        let merged = merge_position(Some(stored), &patch).unwrap();
        assert_eq!(merged["theme"], "sea-glass");
        assert_eq!(merged["unknown"], json!([1, 2, 3]));
        assert_eq!(merged["positions"]["clock"], json!({"x": 100, "y": 2, "version": "digital", "tint": "lilac", "anchor": "br"}));
        assert_eq!(merged["positions"]["glyph-bar"], json!({"x": 9, "y": 9, "w": 56}));
    }

    #[test]
    fn nothing_stored_and_a_broken_blob_are_refused_and_an_empty_layout_gets_the_widget() {
        let patch = parse_patch(&args(json!({"id": "clock", "x": 5}))).unwrap();
        assert!(merge_position(None, &patch).is_err(), "a first layout is the page's to seed");
        assert_eq!(merge_position(Some(json!({})), &patch).unwrap(), json!({"positions": {"clock": {"x": 5}}}));
        assert!(merge_position(Some(json!([1])), &patch).is_err());
        assert!(merge_position(Some(json!({"positions": 3})), &patch).is_err());
        assert!(merge_position(Some(json!({"positions": {"clock": "x"}})), &patch).is_err());
    }

    #[test]
    fn the_event_carries_the_whole_positions_map() {
        assert_eq!(positions_of(&json!({"positions": {"a": {"x": 1}}})), json!({"a": {"x": 1}}));
        assert_eq!(positions_of(&json!({})), json!({}));
    }

    #[test]
    fn revisions_only_grow_and_start_from_the_clock() {
        let mut rev = 0;
        let first = current(&mut rev);
        assert!(first >= 1_700_000_000_000, "seeded from the clock, not from 1");
        let a = bump(&mut rev);
        let b = bump(&mut rev);
        assert!(a > first && b > a);
        // Two bumps inside one millisecond still differ.
        let mut fast = now_ms() + 10_000;
        let (x, y) = (bump(&mut fast), bump(&mut fast));
        assert_eq!(y, x + 1);
    }

    #[test]
    fn a_save_and_a_merge_are_one_ordered_history_with_a_snapshot_at_the_end() {
        let dir = std::env::temp_dir().join(format!("test_desk_layout_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::unidtk_db::UniDtkDatabase::open(dir.join("layout.redb")).unwrap();
        let patch = parse_patch(&args(json!({"id": "clock", "x": 7}))).unwrap();
        assert!(set_position(&db, &patch).is_err(), "nothing stored yet");
        let before = snapshot(&db).unwrap();
        assert_eq!(before["config"], Value::Null);

        save(&db, &json!({"theme": "t", "positions": {"clock": {"x": 1, "tint": "lilac"}}})).unwrap();
        let reply = set_position(&db, &patch).unwrap();
        assert_eq!(reply["position"], json!({"x": 7, "tint": "lilac"}));
        let after = snapshot(&db).unwrap();
        assert_eq!(after["config"]["theme"], "t");
        assert_eq!(after["config"]["positions"]["clock"]["x"], 7);
        assert!(after["rev"].as_u64().unwrap() >= reply["rev"].as_u64().unwrap());
        assert!(reply["rev"].as_u64().unwrap() > before["rev"].as_u64().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
