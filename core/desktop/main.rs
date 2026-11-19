#![windows_subsystem = "windows"]
//! `unidtk-desktop`: the native desk. The desk itself is the `unidtk-shell` library; this
//! binary embeds the module table and registers the widgets of the modules that have a native
//! half (`modules/<id>/native`, `r[shell.widget-contract]`). `toolchain/build.rs` fails the
//! build when a native half is missing from this list.

#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

static WIDGETS: &[&unidtk_shell::widget::WidgetType] = &[&unidtk_native_clock::WIDGET, &unidtk_native_glyph_bar::WIDGET, &unidtk_native_vpn::WIDGET];

fn main() {
    unidtk_shell::main(include_str!(concat!(env!("OUT_DIR"), "/modules.json")), WIDGETS);
}
