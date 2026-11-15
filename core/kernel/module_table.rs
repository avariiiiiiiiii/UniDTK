//! The module table `toolchain/build.rs` generates from `modules/*/ui/metadata.json` and
//! `icon.svg` (`OUT_DIR/modules.json`). The kernel seeds a first desk layout from it, and the
//! native host (which includes this file) draws its catalogue from it; the page reads the same
//! files at runtime. `r[modules.metadata]`.

use std::sync::OnceLock;

use serde_json::{json, Map, Value};

pub struct ModuleInfo {
    pub id: String,
    pub meta: Value,
    /// `icon.svg` as text; the host's menu draws it (G2), the kernel only carries it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub icon: String,
}

const TABLE: &str = include_str!(concat!(env!("OUT_DIR"), "/modules.json"));

/// Every module with a readable `metadata.json`, sorted by id; `explorer` included.
pub fn modules() -> &'static [ModuleInfo] {
    static MODULES: OnceLock<Vec<ModuleInfo>> = OnceLock::new();
    MODULES.get_or_init(|| parse(TABLE))
}

fn parse(table: &str) -> Vec<ModuleInfo> {
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

/// The modules the desk offers: the page's `readModuleMetadata` leaves `explorer` out.
pub fn desk_modules() -> impl Iterator<Item = &'static ModuleInfo> {
    modules().iter().filter(|module| module.id != "explorer")
}

/// `getDefaultWidgetConfig`: the first layout, from each module's `defaultPlacement` and
/// `defaultParent`, with the one-time VPN nesting already marked done.
pub fn default_layout() -> Value {
    let mut positions = Map::new();
    let mut nested: Map<String, Value> = Map::new();
    for module in desk_modules() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_every_module_with_its_metadata_and_icon() {
        let ids: Vec<&str> = modules().iter().map(|m| m.id.as_str()).collect();
        for id in ["clock", "glyph-bar", "micrographics", "mp3-player", "telegram", "vpn"] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        let clock = modules().iter().find(|m| m.id == "clock").unwrap();
        assert_eq!(clock.meta["name"], "Часы");
        assert!(clock.icon.contains("<svg"), "the clock's icon.svg is in the table");
        assert!(desk_modules().all(|m| m.id != "explorer"));
    }

    #[test]
    fn the_default_layout_is_the_pages_first_run() {
        let layout = default_layout();
        assert_eq!(layout["positions"]["clock"]["version"], "rings");
        assert_eq!(layout["nested"]["clock"], json!(["vpn"]), "vpn's defaultParent");
        assert_eq!(layout["migrations"]["vpnNested"], true);
        assert_eq!(layout["hidden"], json!([]));
    }
}
