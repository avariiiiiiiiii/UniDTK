//! The changes a host other than the page makes to the desk layout: `desk_position_set` for
//! one widget's position, and `desk_apply` for a list of named operations applied together
//! (positions, hiding, nesting, instances, the theme).
//!
//! The layout is a single JSON blob (`widget_settings/config`) that the page writes whole,
//! from memory. A native host must not do the same: it would erase whatever it does not
//! draw (module settings, folders, the theme). It names what it changes; the kernel applies
//! that to the stored blob in one write transaction and publishes the whole blob, so every
//! host sees the same desk. `r[desk.position-merge]`.

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

/// This run of the kernel. Revisions start from the clock, which can step back between two
/// runs; a host that sees a new `instance` takes the next layout whatever its revision.
fn instance() -> &'static str {
    static INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| format!("{:x}-{:x}", std::process::id(), now_ms()))
}

/// Tells every subscriber (the page, a native host) the layout changed, with the whole blob:
/// a state event keeps only its newest frame, so a frame that carried part of the layout
/// would make the rest look empty. The caller holds the revision lock, so frames leave in
/// the order the changes were committed.
fn publish(config: &Value, rev: u64) {
    unidtk_api::emit(EVENT, json!({ "rev": rev, "instance": instance(), "config": config }));
}

/// The layout as stored, with the revision it is current at, migrated as the page migrates it
/// on load (the stored blob is healed by the next change, not by this read). Read under the
/// same lock as the changes, so a frame with a higher `rev` is always newer than this answer.
pub fn snapshot(db: &crate::unidtk_db::UniDtkDatabase) -> Result<Value, String> {
    let mut rev = lock_rev();
    let mut config = db.get_widget_config()?;
    if let Some(config) = config.as_mut() {
        migrate(config);
    }
    Ok(json!({ "rev": current(&mut rev), "instance": instance(), "config": config }))
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


/// The layout fixes the page makes on load (`migrateWidgetConfig`), so a desk the page has
/// not opened since an upgrade is the same desk in the native host: the player's old id
/// `yt-music`, the satellites it used to have, and the one-time move of the VPN panel into
/// the clock. True when something changed.
pub fn migrate(config: &mut Value) -> bool {
    let Some(root) = config.as_object_mut() else { return false };
    let mut changed = false;
    if let Some(positions) = root.get_mut("positions").and_then(Value::as_object_mut) {
        if let Some(old) = positions.remove("yt-music") {
            positions.entry("mp3-player").or_insert(old);
            changed = true;
        }
        for stale in ["yt-music-playlists", "yt-music-menu"] {
            changed |= positions.remove(stale).is_some();
        }
    }
    if let Some(hidden) = root.get_mut("hidden").and_then(Value::as_array_mut) {
        for id in hidden.iter_mut() {
            if id == "yt-music" {
                *id = json!("mp3-player");
                changed = true;
            }
        }
    }
    let vpn_nested = root.get("migrations").and_then(|m| m.get("vpnNested")).and_then(Value::as_bool).unwrap_or(false);
    if !vpn_nested {
        let migrations = root.entry("migrations").or_insert_with(|| json!({}));
        if let Some(migrations) = migrations.as_object_mut() {
            migrations.insert("vpnNested".into(), json!(true));
        }
        let nested = root.entry("nested").or_insert_with(|| json!({}));
        if let Some(nested) = nested.as_object_mut() {
            nested.insert("clock".into(), json!(["vpn"]));
        }
        if let Some(hidden) = root.get_mut("hidden").and_then(Value::as_array_mut) {
            hidden.retain(|id| id != "vpn");
        }
        changed = true;
    }
    changed
}

/// The most operations one `desk_apply` takes, and the largest theme record it stores.
const MAX_OPS: usize = 256;
const MAX_THEME_BYTES: usize = 64 * 1024;

fn widget_id(value: Option<&Value>, what: &str) -> Result<String, String> {
    let id = value.and_then(Value::as_str).ok_or_else(|| format!("{what} requires an id"))?;
    if id.is_empty()
        || id.len() > MAX_ID_LEN
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(format!("invalid widget id '{id}'"));
    }
    Ok(id.to_string())
}

fn object_entry<'a>(root: &'a mut Map<String, Value>, key: &str, empty: Value) -> Result<&'a mut Value, String> {
    let entry = root.entry(key.to_string()).or_insert(empty.clone());
    if std::mem::discriminant(entry) != std::mem::discriminant(&empty) {
        return Err(format!("the stored layout has a `{key}` of the wrong shape"));
    }
    Ok(entry)
}

fn bounded(value: &Value, what: &str) -> Result<(), String> {
    let size = serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len());
    if size > MAX_THEME_BYTES {
        return Err(format!("{what} is {size} bytes; the limit is {MAX_THEME_BYTES}"));
    }
    Ok(())
}

/// One validated change of the layout. Each names what it changes, so two writers never
/// overwrite each other's keys; the structure lists (`hidden`, `nested`, `instances`) change
/// one id at a time, as the page's own helpers change them.
fn apply_op(root: &mut Map<String, Value>, op: &Map<String, Value>) -> Result<(), String> {
    let kind = op.get("op").and_then(Value::as_str).ok_or("every operation needs an `op`")?;
    match kind {
        "position" => {
            let mut fields = op.clone();
            fields.remove("op");
            let patch = parse_patch(&fields)?;
            let positions = object_entry(root, "positions", json!({}))?.as_object_mut().expect("checked");
            let entry = positions.entry(patch.id.clone()).or_insert_with(|| json!({}));
            let entry = entry.as_object_mut().ok_or_else(|| format!("the stored position of '{}' is not an object", patch.id))?;
            entry.extend(patch.fields);
        }
        "hide" | "show" => {
            let id = widget_id(op.get("id"), kind)?;
            let hidden = object_entry(root, "hidden", json!([]))?.as_array_mut().expect("checked");
            hidden.retain(|existing| existing != id.as_str());
            if kind == "hide" {
                hidden.push(json!(id));
            }
        }
        "nest" | "eject" => {
            // `moveNested`: the child leaves every host, then joins the new one.
            let id = widget_id(op.get("id"), kind)?;
            let host = if kind == "nest" { Some(widget_id(op.get("host"), "nest")?) } else { None };
            if host.as_deref() == Some(id.as_str()) {
                return Err("a widget cannot be nested in itself".into());
            }
            let nested = object_entry(root, "nested", json!({}))?.as_object_mut().expect("checked");
            for children in nested.values_mut() {
                if let Some(list) = children.as_array_mut() {
                    list.retain(|child| child != id.as_str());
                }
            }
            nested.retain(|_, children| children.as_array().is_some_and(|list| !list.is_empty()));
            if let Some(host) = host {
                let list = nested.entry(host).or_insert_with(|| json!([]));
                if let Some(list) = list.as_array_mut() {
                    list.push(json!(id));
                }
            }
        }
        "add_instance" => {
            let id = widget_id(op.get("id"), kind)?;
            let widget_type = widget_id(op.get("type"), kind)?;
            let patch = match op.get("position").and_then(Value::as_object) {
                Some(position) => {
                    let mut fields = position.clone();
                    fields.insert("id".into(), json!(id));
                    Some(parse_patch(&fields)?)
                }
                None => None,
            };
            let instances = object_entry(root, "instances", json!([]))?.as_array_mut().expect("checked");
            if instances.iter().any(|item| item["instanceId"] == id.as_str()) {
                return Err(format!("instance '{id}' exists"));
            }
            instances.insert(0, json!({ "instanceId": id, "widgetType": widget_type }));
            if let Some(patch) = patch {
                let positions = object_entry(root, "positions", json!({}))?.as_object_mut().expect("checked");
                positions.insert(id, Value::Object(patch.fields));
            }
        }
        "place" => {
            // "Исходная раскладка": a plain top-left place, its corner pin dropped; the
            // settings stored beside the geometry stay.
            let mut fields = op.clone();
            fields.remove("op");
            let patch = parse_patch(&fields)?;
            let positions = object_entry(root, "positions", json!({}))?.as_object_mut().expect("checked");
            let entry = positions.entry(patch.id.clone()).or_insert_with(|| json!({}));
            let entry = entry.as_object_mut().ok_or_else(|| format!("the stored position of '{}' is not an object", patch.id))?;
            for key in ["anchor", "ax", "ay"] {
                entry.remove(key);
            }
            entry.extend(patch.fields);
        }
        "settings" => {
            // A widget's own settings stored beside its geometry (a Telegram dock's
            // `expanded`, a player's `aiDj`): named keys merged in, never a geometry field.
            let id = widget_id(op.get("id"), kind)?;
            let mut fields = op.clone();
            fields.remove("op");
            fields.remove("id");
            if fields.is_empty() {
                return Err("settings needs at least one field".into());
            }
            for key in fields.keys() {
                if matches!(key.as_str(), "x" | "y" | "w" | "h" | "ax" | "ay" | "anchor") {
                    return Err(format!("'{key}' is a position field: use the position operation"));
                }
                if key.is_empty() || key.len() > 32 || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(format!("invalid setting name '{key}'"));
                }
            }
            bounded(&Value::Object(fields.clone()), "settings")?;
            let positions = object_entry(root, "positions", json!({}))?.as_object_mut().expect("checked");
            let entry = positions.entry(id.clone()).or_insert_with(|| json!({}));
            let entry = entry.as_object_mut().ok_or_else(|| format!("the stored position of '{id}' is not an object"))?;
            entry.extend(fields);
        }
        "forget" => {
            let id = widget_id(op.get("id"), kind)?;
            if let Some(positions) = root.get_mut("positions").and_then(Value::as_object_mut) {
                positions.remove(&id);
            }
        }
        "set_position" => {
            // An undo puts a whole record back, the widget's settings with it.
            let id = widget_id(op.get("id"), kind)?;
            let record = op.get("record").filter(|r| r.is_object()).ok_or("set_position requires a record")?;
            bounded(record, "the position record")?;
            let positions = object_entry(root, "positions", json!({}))?.as_object_mut().expect("checked");
            positions.insert(id, record.clone());
        }
        "structure" => {
            // Arrange mode's "Отменить": the lists as the session found them.
            for (key, shape) in [("hidden", json!([])), ("instances", json!([])), ("nested", json!({}))] {
                let value = op.get(key).cloned().unwrap_or(shape.clone());
                if std::mem::discriminant(&value) != std::mem::discriminant(&shape) {
                    return Err(format!("structure: `{key}` has the wrong shape"));
                }
                bounded(&value, key)?;
                root.insert(key.into(), value);
            }
        }
        "positions" => {
            // The undo of a layout reset: the whole map as it was. Sent with `baseRev`.
            let map = op.get("positions").filter(|p| p.is_object()).ok_or("positions requires an object")?;
            bounded(map, "the positions")?;
            root.insert("positions".into(), map.clone());
        }
        "remove_instance" => {
            let id = widget_id(op.get("id"), kind)?;
            let instances = object_entry(root, "instances", json!([]))?.as_array_mut().expect("checked");
            instances.retain(|item| item["instanceId"] != id.as_str());
            if let Some(positions) = root.get_mut("positions").and_then(Value::as_object_mut) {
                for key in [id.clone(), format!("{id}-playlists"), format!("{id}-menu")] {
                    positions.remove(&key);
                }
            }
        }
        "arrange_side" => match op.get("side").and_then(Value::as_str) {
            Some(side @ ("left" | "right")) => {
                root.insert("arrangeSide".into(), json!(side));
            }
            _ => return Err("arrange_side requires side: left or right".into()),
        },
        "theme" => {
            // The host sends the theme it normalized (`normalizeTheme`); the kernel keeps it
            // as a record and only bounds it.
            let theme = op.get("theme").filter(|t| t.is_object()).ok_or("theme requires an object")?;
            bounded(theme, "the theme")?;
            root.insert("theme".into(), theme.clone());
        }
        "color_overrides" => {
            let id = widget_id(op.get("id"), kind)?;
            let overrides = object_entry(root, "colorOverrides", json!({}))?.as_object_mut().expect("checked");
            match op.get("value") {
                Some(value) if value.is_object() => {
                    bounded(value, "the colour overrides")?;
                    overrides.insert(id, value.clone());
                }
                Some(Value::Null) | None => {
                    overrides.remove(&id);
                }
                Some(_) => return Err("color_overrides value must be an object or null".into()),
            }
        }
        "custom_presets" => {
            let list = op.get("presets").filter(|p| p.is_array()).ok_or("custom_presets requires an array")?;
            bounded(list, "the custom themes")?;
            root.insert("customPresets".into(), list.clone());
        }
        other => return Err(format!("unknown operation '{other}'")),
    }
    Ok(())
}

/// The layout with `ops` applied in order, all or nothing. Nothing stored yet starts from the
/// modules' first-run layout, the same the page seeds (`getDefaultWidgetConfig`), so a desk
/// that only ever ran native gets a complete layout on its first change.
pub fn apply_ops(stored: Option<Value>, ops: &[Value]) -> Result<Value, String> {
    if ops.is_empty() || ops.len() > MAX_OPS {
        return Err(format!("desk_apply takes 1 to {MAX_OPS} operations"));
    }
    let mut config = stored.unwrap_or_else(crate::module_table::default_layout);
    migrate(&mut config);
    let root = config.as_object_mut().ok_or("the stored layout is not a JSON object")?;
    for op in ops {
        let op = op.as_object().ok_or("every operation is an object")?;
        apply_op(root, op)?;
    }
    Ok(config)
}

/// `desk_apply {ops, baseRev?}` for both doors. With `baseRev`, a layout that changed since
/// that revision is refused (`conflict`): the caller computed its operations from a layout
/// it no longer has (an arrange snapshot, an undo) and reads it again first.
pub fn apply(db: &crate::unidtk_db::UniDtkDatabase, args: &Map<String, Value>) -> Result<Value, String> {
    let ops = args.get("ops").and_then(Value::as_array).ok_or("desk_apply requires {ops: [...]}")?;
    let base = args.get("baseRev").and_then(Value::as_u64);
    let mut rev = lock_rev();
    let current_rev = current(&mut rev);
    if let Some(base) = base.filter(|base| *base != current_rev) {
        return Err(format!("conflict: the layout is at revision {current_rev}, not {base}"));
    }
    let merged = db.update_widget_config(|stored| apply_ops(stored, ops))?;
    let rev = bump(&mut rev);
    publish(&merged, rev);
    Ok(json!({ "ok": true, "rev": rev }))
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

    fn ops(value: Value) -> Vec<Value> {
        value.as_array().unwrap().clone()
    }

    #[test]
    fn operations_change_only_what_they_name() {
        let stored = json!({
            "positions": { "clock": { "x": 1, "tint": "lilac" }, "vpn-1": { "x": 3 }, "vpn-1-menu": {} },
            "hidden": ["glyph-bar"],
            "nested": { "clock": ["vpn"] },
            "instances": [{ "instanceId": "vpn-1", "widgetType": "vpn" }],
            "migrations": { "vpnNested": true },
            "theme": { "id": "sea-glass" },
        });
        let merged = apply_ops(Some(stored), &ops(json!([
            { "op": "position", "id": "clock", "x": 9, "anchor": "tr" },
            { "op": "hide", "id": "telegram" },
            { "op": "show", "id": "glyph-bar" },
            { "op": "nest", "id": "vpn", "host": "folder_a" },
            { "op": "remove_instance", "id": "vpn-1" },
            { "op": "add_instance", "id": "vpn-2", "type": "vpn", "position": { "x": 5, "y": 6 } },
            { "op": "arrange_side", "side": "left" },
            { "op": "color_overrides", "id": "sea-glass", "value": { "accent": "#e6cf94" } },
            { "op": "place", "id": "clock", "x": 20, "y": 30 },
            { "op": "forget", "id": "vpn-1-menu" },
            { "op": "add_instance", "id": "folder_b-1", "type": "folder_b" },
        ]))).unwrap();
        assert_eq!(merged["positions"]["clock"], json!({ "x": 20, "y": 30, "tint": "lilac" }), "place drops the corner pin");
        assert_eq!(merged["instances"][0], json!({ "instanceId": "folder_b-1", "widgetType": "folder_b" }));
        let restored = apply_ops(Some(merged.clone()), &ops(json!([
            { "op": "set_position", "id": "clock", "record": { "x": 1, "version": "rings" } },
            { "op": "positions", "positions": { "only": { "x": 0 } } },
        ]))).unwrap();
        assert_eq!(restored["positions"], json!({ "only": { "x": 0 } }));
        let merged = apply_ops(Some(json!({
            "positions": { "clock": { "x": 1, "tint": "lilac" }, "vpn-1": { "x": 3 }, "vpn-1-menu": {} },
            "hidden": ["glyph-bar"], "nested": { "clock": ["vpn"] },
            "instances": [{ "instanceId": "vpn-1", "widgetType": "vpn" }],
            "migrations": { "vpnNested": true }, "theme": { "id": "sea-glass" },
        })), &ops(json!([
            { "op": "position", "id": "clock", "x": 9, "anchor": "tr" },
            { "op": "hide", "id": "telegram" },
            { "op": "show", "id": "glyph-bar" },
            { "op": "nest", "id": "vpn", "host": "folder_a" },
            { "op": "remove_instance", "id": "vpn-1" },
            { "op": "add_instance", "id": "vpn-2", "type": "vpn", "position": { "x": 5, "y": 6 } },
            { "op": "arrange_side", "side": "left" },
            { "op": "color_overrides", "id": "sea-glass", "value": { "accent": "#e6cf94" } },
        ]))).unwrap();
        assert_eq!(merged["positions"]["clock"], json!({ "x": 9, "tint": "lilac", "anchor": "tr" }));
        assert_eq!(merged["hidden"], json!(["telegram"]));
        assert_eq!(merged["nested"], json!({ "folder_a": ["vpn"] }), "moveNested drops the emptied host");
        assert_eq!(merged["instances"], json!([{ "instanceId": "vpn-2", "widgetType": "vpn" }]));
        assert!(merged["positions"].get("vpn-1").is_none() && merged["positions"].get("vpn-1-menu").is_none());
        assert_eq!(merged["positions"]["vpn-2"], json!({ "x": 5, "y": 6 }));
        assert_eq!((merged["arrangeSide"].as_str(), &merged["theme"]), (Some("left"), &json!({ "id": "sea-glass" })));
        assert_eq!(merged["colorOverrides"]["sea-glass"]["accent"], "#e6cf94");
    }

    #[test]
    fn a_bad_operation_refuses_the_whole_list() {
        let stored = json!({ "positions": {}, "migrations": { "vpnNested": true } });
        for bad in [
            json!([]),
            json!([{ "op": "teleport", "id": "clock" }]),
            json!([{ "op": "position", "id": "clock", "x": 1 }, { "op": "hide" }]),
            json!([{ "op": "nest", "id": "clock", "host": "clock" }]),
            json!([{ "op": "arrange_side", "side": "up" }]),
            json!([{ "op": "theme", "theme": "dark" }]),
            json!([{ "op": "theme", "theme": { "blob": "x".repeat(70_000) } }]),
        ] {
            assert!(apply_ops(Some(stored.clone()), &ops(bad.clone())).is_err(), "{bad}");
        }
    }

    #[test]
    fn nothing_stored_starts_from_the_first_run_layout() {
        let merged = apply_ops(None, &ops(json!([{ "op": "hide", "id": "telegram" }]))).unwrap();
        assert_eq!(merged["positions"]["clock"]["version"], "rings");
        assert_eq!(merged["nested"]["clock"], json!(["vpn"]));
        assert_eq!(merged["hidden"], json!(["telegram"]));
    }

    #[test]
    fn an_old_layout_is_migrated_as_the_page_migrates_it() {
        let mut old = json!({
            "positions": { "yt-music": { "x": 1 }, "yt-music-menu": {}, "clock": {} },
            "hidden": ["yt-music", "vpn"],
        });
        assert!(migrate(&mut old));
        assert_eq!(old["positions"]["mp3-player"], json!({ "x": 1 }));
        assert!(old["positions"].get("yt-music").is_none() && old["positions"].get("yt-music-menu").is_none());
        assert_eq!(old["hidden"], json!(["mp3-player"]));
        assert_eq!(old["nested"]["clock"], json!(["vpn"]));
        assert!(!migrate(&mut old), "idempotent");
    }

    #[test]
    fn a_change_from_a_stale_revision_is_refused() {
        let dir = std::env::temp_dir().join(format!("test_desk_apply_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::unidtk_db::UniDtkDatabase::open(dir.join("layout.redb")).unwrap();
        let hide = |base: Option<u64>| {
            let mut args = json!({ "ops": [{ "op": "hide", "id": "telegram" }] });
            if let Some(base) = base {
                args["baseRev"] = json!(base);
            }
            apply(&db, args.as_object().unwrap())
        };
        let first = snapshot(&db).unwrap()["rev"].as_u64().unwrap();
        let reply = hide(Some(first)).unwrap();
        assert!(hide(Some(first)).unwrap_err().starts_with("conflict"), "the layout moved on");
        hide(Some(reply["rev"].as_u64().unwrap())).unwrap();
        hide(None).unwrap();
        let after = snapshot(&db).unwrap();
        assert_eq!(after["config"]["hidden"], json!(["telegram"]));
        assert!(after["instance"].as_str().is_some_and(|i| !i.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
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
    fn a_widgets_settings_merge_beside_its_geometry_and_never_replace_it() {
        let merged = apply_ops(Some(json!({ "positions": { "telegram": { "x": 1, "w": 70 } } })), &ops(json!([
            { "op": "settings", "id": "telegram", "expanded": true, "showSettings": false },
        ]))).unwrap();
        assert_eq!(merged["positions"]["telegram"], json!({ "x": 1, "w": 70, "expanded": true, "showSettings": false }));
        for refused in [
            json!([{ "op": "settings", "id": "telegram", "x": 5 }]),
            json!([{ "op": "settings", "id": "telegram" }]),
            json!([{ "op": "settings", "id": "telegram", "bad key": 1 }]),
        ] {
            assert!(apply_ops(Some(json!({ "positions": {} })), &ops(refused.clone())).is_err(), "{refused}");
        }
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
