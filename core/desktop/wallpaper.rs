//! The backdrop a card blurs (`backdrop-filter: blur(var(--theme-blur))`, both materials). The host is a
//! transparent child of the desktop: what lies under it is the wallpaper, which DWM composes and
//! the window cannot read. So the host reads the wallpaper itself — the file, its placement
//! (`WallpaperStyle`, `TileWallpaper`) and the desktop colour round it — lays it out on the
//! primary display as Windows does, and blurs a quarter-size copy once, on a worker. A card
//! draws its part of that copy under its own glass, so the blur costs nothing per frame and has
//! no limit on how many cards wear it. The page's `backdrop-filter` could only blur what the
//! page itself drew; this blurs the real wallpaper.

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

/// The copy is a quarter of the display's pixels on each side: the blur hides the difference.
pub const DOWNSCALE: f32 = 4.0;

/// How Windows places the wallpaper (`HKCU\Control Panel\Desktop`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Center,
    Tile,
    Stretch,
    Fit,
    Fill,
    Span,
}

impl Placement {
    /// `WallpaperStyle` and `TileWallpaper` as the registry stores them.
    pub fn from_registry(style: &str, tile: &str) -> Placement {
        match (style.trim(), tile.trim()) {
            (_, "1") => Placement::Tile,
            ("2", _) => Placement::Stretch,
            ("6", _) => Placement::Fit,
            ("10", _) => Placement::Fill,
            ("22", _) => Placement::Span,
            _ => Placement::Center,
        }
    }
}

/// What the desk's backdrop is made from.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub path: String,
    pub placement: Placement,
    /// The desktop colour round a centred or fitted picture.
    pub colour: [u8; 3],
}

/// The wallpaper laid out on a `width` x `height` display, as Windows draws it.
pub fn lay_out(picture: &DynamicImage, placement: Placement, colour: [u8; 3], (width, height): (u32, u32)) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(width, height, Rgba([colour[0], colour[1], colour[2], 255]));
    let (pw, ph) = picture.dimensions();
    if pw == 0 || ph == 0 || width == 0 || height == 0 {
        return out;
    }
    let place = |out: &mut RgbaImage, scaled: &RgbaImage, x: i64, y: i64| image::imageops::overlay(out, scaled, x, y);
    match placement {
        Placement::Stretch => return picture.resize_exact(width, height, FilterType::Triangle).to_rgba8(),
        Placement::Fill | Placement::Span => {
            let scaled = picture.resize_to_fill(width, height, FilterType::Triangle).to_rgba8();
            place(&mut out, &scaled, 0, 0);
        }
        Placement::Fit => {
            let scaled = picture.resize(width, height, FilterType::Triangle).to_rgba8();
            place(&mut out, &scaled, i64::from((width - scaled.width()) / 2), i64::from((height - scaled.height()) / 2));
        }
        Placement::Center => {
            let rgba = picture.to_rgba8();
            place(&mut out, &rgba, (i64::from(width) - i64::from(pw)) / 2, (i64::from(height) - i64::from(ph)) / 2);
        }
        Placement::Tile => {
            let rgba = picture.to_rgba8();
            for y in (0..height).step_by(ph as usize) {
                for x in (0..width).step_by(pw as usize) {
                    place(&mut out, &rgba, i64::from(x), i64::from(y));
                }
            }
        }
    }
    out
}

/// A Gaussian of standard deviation `sigma` pixels, as three box blurs (the CSS `blur()`
/// function's Gaussian, close enough at this size), on each channel.
pub fn blur(image: &RgbaImage, sigma: f32) -> RgbaImage {
    if sigma <= 0.0 {
        return image.clone();
    }
    // Box widths for three passes (Kovesi's "fast almost-Gaussian").
    let n = 3.0f32;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let m = ((12.0 * sigma * sigma - n * (lower * lower) as f32 - 4.0 * n * lower as f32 - 3.0 * n) / (-4.0 * lower as f32 - 4.0)).round() as i32;
    let (w, h) = image.dimensions();
    let mut data: Vec<[f32; 4]> = image.pixels().map(|p| p.0.map(f32::from)).collect();
    for pass in 0..3 {
        let radius = ((if pass < m { lower } else { upper }) - 1).max(0) as usize / 2;
        data = box_pass(&data, w as usize, h as usize, radius, true);
        data = box_pass(&data, w as usize, h as usize, radius, false);
    }
    RgbaImage::from_fn(w, h, |x, y| Rgba(data[(y * w + x) as usize].map(|v| v.round().clamp(0.0, 255.0) as u8)))
}

/// One running-sum box blur along rows (`horizontal`) or columns, edges clamped.
fn box_pass(data: &[[f32; 4]], w: usize, h: usize, radius: usize, horizontal: bool) -> Vec<[f32; 4]> {
    if radius == 0 {
        return data.to_vec();
    }
    let mut out = vec![[0.0; 4]; data.len()];
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let span = (2 * radius + 1) as f32;
    for line in 0..lines {
        let mut sum = [0.0f32; 4];
        for i in 0..=2 * radius {
            let px = data[at(line, i.saturating_sub(radius).min(len - 1))];
            for c in 0..4 {
                sum[c] += px[c];
            }
        }
        for i in 0..len {
            out[at(line, i)] = sum.map(|v| v / span);
            let leaving = data[at(line, i.saturating_sub(radius))];
            let entering = data[at(line, (i + radius + 1).min(len - 1))];
            for c in 0..4 {
                sum[c] += entering[c] - leaving[c];
            }
        }
    }
    out
}

/// The blurred backdrop of a display `size` physical pixels large, at a quarter of it, with
/// `sigma` in physical pixels; `None` when the wallpaper cannot be read (a colour only, a
/// missing file): the cards then show their glass alone.
pub fn make(source: &Source, size: (u32, u32), sigma: f32) -> Option<RgbaImage> {
    let small = ((size.0 as f32 / DOWNSCALE).round().max(1.0) as u32, (size.1 as f32 / DOWNSCALE).round().max(1.0) as u32);
    let laid_out = if source.path.is_empty() {
        RgbaImage::from_pixel(small.0, small.1, Rgba([source.colour[0], source.colour[1], source.colour[2], 255]))
    } else {
        let picture = image::open(&source.path).ok()?;
        // Laid out at full size first, as Windows lays it out, then reduced.
        let full = lay_out(&picture, source.placement, source.colour, size);
        image::imageops::resize(&full, small.0, small.1, FilterType::Triangle)
    };
    Some(blur(&laid_out, sigma / DOWNSCALE))
}

/// Windows' current wallpaper and how it is placed.
pub fn current() -> Source {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut buffer = [0u16; 1024];
    let path = unsafe {
        SystemParametersInfoW(SPI_GETDESKWALLPAPER, buffer.len() as u32, Some(buffer.as_mut_ptr().cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
    }
    .map(|_| String::from_utf16_lossy(&buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(0)]))
    .unwrap_or_default();
    let style = registry_string(r"Control Panel\Desktop", "WallpaperStyle").unwrap_or_default();
    let tile = registry_string(r"Control Panel\Desktop", "TileWallpaper").unwrap_or_default();
    let colour = registry_string(r"Control Panel\Colors", "Background")
        .and_then(|text| {
            let parts: Vec<u8> = text.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            (parts.len() == 3).then(|| [parts[0], parts[1], parts[2]])
        })
        .unwrap_or([0, 0, 0]);
    Source { path, placement: Placement::from_registry(&style, &tile), colour }
}

fn registry_string(key: &str, value: &str) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let mut buffer = [0u16; 512];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size))
    };
    status.is_ok().then(|| String::from_utf16_lossy(&buffer[..(size as usize / 2).saturating_sub(1)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_names_its_placements() {
        assert_eq!(Placement::from_registry("10", "0"), Placement::Fill);
        assert_eq!(Placement::from_registry("6", "0"), Placement::Fit);
        assert_eq!(Placement::from_registry("0", "1"), Placement::Tile);
        assert_eq!(Placement::from_registry("22", "0"), Placement::Span);
        assert_eq!(Placement::from_registry("", ""), Placement::Center);
    }

    #[test]
    fn a_fitted_picture_is_centred_on_the_desktop_colour() {
        let picture = DynamicImage::ImageRgba8(RgbaImage::from_pixel(40, 10, Rgba([255, 0, 0, 255])));
        let out = lay_out(&picture, Placement::Fit, [0, 0, 255], (40, 40));
        assert_eq!(out.get_pixel(20, 20).0, [255, 0, 0, 255]);
        assert_eq!(out.get_pixel(20, 2).0, [0, 0, 255, 255], "the band above is the desktop colour");
    }

    #[test]
    fn a_blur_keeps_a_flat_colour_and_spreads_an_edge() {
        let flat = RgbaImage::from_pixel(16, 16, Rgba([10, 20, 30, 255]));
        assert_eq!(blur(&flat, 3.0).get_pixel(8, 8).0, [10, 20, 30, 255]);
        let edge = RgbaImage::from_fn(32, 4, |x, _| if x < 16 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) });
        let soft = blur(&edge, 3.0);
        let middle = soft.get_pixel(16, 2).0[0];
        assert!(middle > 60 && middle < 200, "{middle}");
        assert_eq!(soft.get_pixel(0, 2).0[0], 0);
    }
}
