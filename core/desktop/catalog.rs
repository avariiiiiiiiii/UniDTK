//! The module table the host draws its catalogue from (`unidtk_api::modules`): the binary
//! embeds the file `toolchain/build.rs` generates and hands it to [`install`] before the desk
//! starts. Tests read the modules' own files instead. `r[modules.metadata]`.

use std::sync::OnceLock;

use serde_json::Value;
pub use unidtk_api::modules::ModuleInfo;

static TABLE: OnceLock<Vec<ModuleInfo>> = OnceLock::new();

/// Takes the generated table; the first call wins.
pub fn install(table: &str) {
    let _ = TABLE.set(unidtk_api::modules::parse(table));
}

/// Every module with a readable `metadata.json`, sorted by id; `explorer` included.
pub fn modules() -> &'static [ModuleInfo] {
    TABLE.get_or_init(|| {
        if cfg!(test) {
            unidtk_api::modules::read_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules"))
        } else {
            Vec::new()
        }
    })
}

/// The modules the desk offers: the page's `readModuleMetadata` leaves `explorer` out.
pub fn desk_modules() -> impl Iterator<Item = &'static ModuleInfo> {
    unidtk_api::modules::desk_modules(modules())
}

/// `getDefaultWidgetConfig`: the first layout from the modules' default placements.
pub fn default_layout() -> Value {
    unidtk_api::modules::default_layout(modules())
}
