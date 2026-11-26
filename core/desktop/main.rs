#![windows_subsystem = "windows"]
//! `unidtk-desktop`: the native desk. The desk itself is the `unidtk-shell` library; this
//! binary embeds the module table and registers the widgets of the modules that have a native
//! half (`modules/<id>/native`, `r[shell.widget-contract]`). `toolchain/build.rs` fails the
//! build when a native half is missing from this list.

#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(not(feature = "mp3-native"))]
static WIDGETS: &[&unidtk_shell::widget::WidgetType] = &[&unidtk_native_clock::WIDGET, &unidtk_native_glyph_bar::WIDGET, &unidtk_native_vpn::WIDGET];

// The music player joins only when its port is whole (session 152); the gate goes with it.
#[cfg(feature = "mp3-native")]
static WIDGETS: &[&unidtk_shell::widget::WidgetType] =
    &[&unidtk_native_clock::WIDGET, &unidtk_native_glyph_bar::WIDGET, &unidtk_native_vpn::WIDGET, &unidtk_native_mp3_player::WIDGET];

fn main() {
    unidtk_shell::main(include_str!(concat!(env!("OUT_DIR"), "/modules.json")), WIDGETS);
}
