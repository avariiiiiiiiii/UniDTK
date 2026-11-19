//! The module table: every module's `metadata.json` and `icon.svg`, as `toolchain/build.rs`
//! generates it (`OUT_DIR/modules.json`, a JSON array of `{id, meta, icon}`). The kernel seeds
//! a first desk layout from it and the native host draws its catalogue from it; each binary
//! embeds the generated file and parses it with [`parse`]. `r[modules.metadata]`.

use serde_json::{json, Map, Value};

pub struct ModuleInfo {
    pub id: String,
    pub meta: Value,
    /// `icon.svg` as text.
    pub icon: String,
}

/// Every module with a readable `metadata.json`, in the table's order (sorted by id).
pub fn parse(table: &str) -> Vec<ModuleInfo> {
    let rows: Vec<Value> = serde_json::from_str(table).unwrap_or_default();
    rows.into_iter()
        .filter_map(|row| {
            Some(ModuleInfo {
                id: row.get("id")?.as_str()?.to_string(),
                meta: row.get("meta").cloned().unwrap_or(Value::Null),
                icon: row.get("icon").and_then(Value::as_str).unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// The table read from the modules' own files, as `build.rs` reads them: for tests of a crate
/// that has no build script of its own.
pub fn read_dir(modules: &std::path::Path) -> Vec<ModuleInfo> {
    let mut ids: Vec<String> = std::fs::read_dir(modules)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids.into_iter()
        .filter_map(|id| {
            let ui = modules.join(&id).join("ui");
            let meta: Value = serde_json::from_str(&std::fs::read_to_string(ui.join("metadata.json")).ok()?).ok()?;
            let icon = std::fs::read_to_string(ui.join("icon.svg")).unwrap_or_default();
            Some(ModuleInfo { id, meta, icon })
        })
        .collect()
}

/// The modules the desk offers: the page's `readModuleMetadata` leaves `explorer` out.
pub fn desk_modules(modules: &[ModuleInfo]) -> impl Iterator<Item = &ModuleInfo> {
    modules.iter().filter(|module| module.id != "explorer")
}

/// `getDefaultWidgetConfig`: the first layout, from each module's `defaultPlacement` and
/// `defaultParent`, with the one-time VPN nesting already marked done.
pub fn default_layout(modules: &[ModuleInfo]) -> Value {
    let mut positions = Map::new();
    let mut nested: Map<String, Value> = Map::new();
    for module in desk_modules(modules) {
        if let Some(placement) = module.meta.get("defaultPlacement").filter(|p| p.is_object()) {
            positions.insert(module.id.clone(), placement.clone());
        }
        if let Some(parent) = module.meta.get("defaultParent").and_then(Value::as_str) {
            let children = nested.entry(parent.to_string()).or_insert_with(|| json!([]));
            if let Some(list) = children.as_array_mut() {
                list.push(json!(module.id));
            }
        }
    }
    json!({
        "order": [],
        "align": "left",
        "positions": positions,
        "instances": [],
        "nested": nested,
        "migrations": { "vpnNested": true },
        "hidden": [],
    })
}
