//! UniDTK Kernel Module Registry.
//!
//! Provides canonical instantiation and inventory of all registered modules.
//! Each vertical slice module exports `create_module() -> Box<dyn unidtk_api::Module>`.
//! Compile-time integrity between disk modules and this registry is enforced by
//! `toolchain/build.rs`.

use unidtk_api::Module;

/// Factory list of all modules in the application.
pub fn all_modules() -> Vec<Box<dyn Module>> {
    vec![
        unidtk_mod_glyph_bar::create_module(),
        unidtk_mod_explorer::create_module(),
        unidtk_mod_vpn::create_module(),
        unidtk_mod_telegram::create_module(),
        unidtk_mod_mp3_player::create_module(),
        unidtk_mod_micrographics::create_module(),
    ]
}

/// Static list of module IDs for inventory checks and diagnostic assertions.
#[allow(dead_code)]
pub const ALL_MODULE_IDS: &[&str] = &[
    "glyph-bar",
    "explorer",
    "vpn",
    "telegram",
    "mp3-player",
    "micrographics",
];
