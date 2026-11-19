//! The module table `toolchain/build.rs` generates from `modules/*/ui/metadata.json` and
//! `icon.svg` (`OUT_DIR/modules.json`), parsed by `unidtk_api::modules`. The kernel seeds a
//! first desk layout from it; the native host embeds the same file and parses its own copy;
//! the page reads the same files at runtime. `r[modules.metadata]`.

use std::sync::OnceLock;

use serde_json::Value;
pub use unidtk_api::modules::ModuleInfo;

const TABLE: &str = include_str!(concat!(env!("OUT_DIR"), "/modules.json"));

/// Every module with a readable `metadata.json`, sorted by id; `explorer` included.
pub fn modules() -> &'static [ModuleInfo] {
    static MODULES: OnceLock<Vec<ModuleInfo>> = OnceLock::new();
    MODULES.get_or_init(|| unidtk_api::modules::parse(TABLE))
}

/// `getDefaultWidgetConfig`: the first layout from the modules' default placements.
pub fn default_layout() -> Value {
    unidtk_api::modules::default_layout(modules())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_table_holds_every_module_with_its_metadata_and_icon() {
        let ids: Vec<&str> = modules().iter().map(|m| m.id.as_str()).collect();
        for id in ["clock", "glyph-bar", "micrographics", "mp3-player", "telegram", "vpn"] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        let clock = modules().iter().find(|m| m.id == "clock").unwrap();
        assert_eq!(clock.meta["name"], "Часы");
        assert!(clock.icon.contains("<svg"), "the clock's icon.svg is in the table");
        assert!(unidtk_api::modules::desk_modules(modules()).all(|m| m.id != "explorer"));
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
