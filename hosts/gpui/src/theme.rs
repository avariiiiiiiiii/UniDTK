#![allow(dead_code)]

use gpui::{font, rgb, rgba, App, Font, Rgba};
use std::borrow::Cow;

pub fn nothing_red() -> Rgba {
    rgb(0xd71921)
}

pub fn bg_glass() -> Rgba {
    rgba(0x121214d9)
}

pub fn border_glass() -> Rgba {
    rgba(0xffffff1c)
}

pub fn text_primary() -> Rgba {
    rgb(0xffffff)
}

pub fn text_secondary() -> Rgba {
    rgb(0xa1a1aa)
}

pub fn text_muted() -> Rgba {
    rgb(0x71717a)
}

pub fn accent_green() -> Rgba {
    rgb(0x00e676)
}

pub fn glyph_on() -> Rgba {
    rgb(0xffffff)
}

pub fn glyph_off() -> Rgba {
    rgba(0xffffff18)
}

pub fn glyph_muted_red() -> Rgba {
    rgb(0xd71921)
}

/// Embedded Ndot 55 font bytes
const NDOT_55_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Ndot 55/Ndot 55.ttf");

/// Registers the Nothing OS Ndot 55 font into the GPUI TextSystem.
pub fn init_theme(cx: &mut App) {
    let font_cow = Cow::Borrowed(NDOT_55_BYTES);
    if let Err(e) = cx.text_system().add_fonts(vec![font_cow]) {
        eprintln!("[theme] Failed to load Ndot 55 font: {e}");
    } else {
        println!("[theme] Ndot 55 font successfully loaded into GPUI TextSystem");
    }
}

/// Returns the Ndot 55 font descriptor for dot-matrix typography.
pub fn font_dot() -> Font {
    font("Ndot 55")
}
