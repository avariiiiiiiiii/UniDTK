//! Splash screen for a bare `udtk` invocation.
//!
//! Layout: the ouroboros emblem on the left, the block-letter wordmark on the
//! right, and the command map underneath in plain text. The emblem is coloured
//! from its own geometry (titanium serpent with crimson accent, ice planet, pale
//! rings) and modulated per cell by a photo-derived shade byte: the serpent body
//! carries the luminance of the source ouroboros painting (docs/unidtkascii.md
//! §5), Saturn and the rings stay brand-coloured. The wordmark features "Uni" in
//! chrome and "DTK" in German flag colors (Schwarz/Graphite, Rot, Gold);
//! everything is plain UTF-8 when stdout is not a console, `NO_COLOR` is set or
//! the console refused VT processing, so the output stays pipe-friendly.
//!
//! The layout adapts to the terminal in both axes: a taller window earns the XL
//! emblem, a narrow one falls back to the compact emblem, then to the emblem
//! without the block wordmark, then to text. Every tier keeps the whole splash
//! inside the visible window (no scrolling).

use crate::splash_art as art;

const RESET: &str = "\x1b[0m";
const GAP: &str = "   ";

const ACCENT: (u8, u8, u8) = (215, 25, 33);
const DIM: (u8, u8, u8) = (126, 136, 156);

// Ouroboros: sleek metallic titanium / steel
const BODY_TOP: (u8, u8, u8) = (195, 208, 222);
const BODY_BOTTOM: (u8, u8, u8) = (90, 102, 120);

// Saturn: ice planet & rings
const PLANET_TOP: (u8, u8, u8) = (198, 238, 255);
const PLANET_BOTTOM: (u8, u8, u8) = (54, 124, 176);
const RING_TOP: (u8, u8, u8) = (214, 244, 255);
const RING_BOTTOM: (u8, u8, u8) = (108, 178, 218);

// UniDTK Wordmark: "Uni" in chrome/white, "DTK" in German flag colors
const MARK_TOP: (u8, u8, u8) = (255, 255, 255);
const MARK_BOTTOM: (u8, u8, u8) = (175, 185, 200);

const GERMAN_D_TOP: (u8, u8, u8) = (95, 105, 120);    // Anthracite/graphite for Schwarz
const GERMAN_D_BOTTOM: (u8, u8, u8) = (55, 62, 72);
const GERMAN_T_TOP: (u8, u8, u8) = (255, 45, 45);     // German Rot
const GERMAN_T_BOTTOM: (u8, u8, u8) = (200, 18, 18);
const GERMAN_K_TOP: (u8, u8, u8) = (255, 215, 0);     // German Gold
const GERMAN_K_BOTTOM: (u8, u8, u8) = (212, 155, 0);

/// Commands shown under the logo, grouped the way the help text groups them.
const COMMANDS: [(&str, &str); 4] = [
    ("control", "invoke · help"),
    ("telemetry", "doctor · logs"),
    ("ui", "gui · shot · cdp-list"),
    ("system", "tasks · path · autostart · build · stop · run"),
];

/// Block wordmark width in columns (`splash_art.rs` keeps it at 46, see tests).
const WORDMARK_COLS: usize = 46;
/// Cells every command row starts at: `"  " + group + padding` == 13 columns.
const CMD_INDENT: usize = 13;

/// Minimum terminal width for each side-by-side layout. Derived from the art, so
/// a regenerated emblem can never silently overflow the window it was given.
const XL_MIN: usize = art::EMBLEM_XL_GEO.cols + GAP.len() + WORDMARK_COLS + 1;
const WIDE_MIN: usize = art::EMBLEM_WIDE_GEO.cols + GAP.len() + WORDMARK_COLS + 1;
const COMPACT_MIN: usize = art::EMBLEM_SMALL_GEO.cols + GAP.len() + WORDMARK_COLS + 1;
/// Narrowest window that can still show the emblem on its own.
const STACK_MIN: usize = art::EMBLEM_SMALL_GEO.cols + 2;

pub fn print() {
    let (tty, width, height) = console();
    let forced = std::env::var_os("FORCE_COLOR").is_some()
        || std::env::var_os("CLICOLOR_FORCE").is_some();
    let no_color = matches!(std::env::var("NO_COLOR"), Ok(v) if !v.is_empty());
    let dumb = matches!(std::env::var("TERM"), Ok(v) if v == "dumb");
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(width);
    let height = std::env::var("LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(height);
    let color = !no_color && !dumb && (tty || forced);
    let output = render(color, width, height);

    use std::io::Write;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(output.as_bytes());
    let _ = handle.flush();
}

fn push_spaces(out: &mut String, count: usize) {
    const SPACES: &str = "                                                            ";
    if count <= SPACES.len() {
        out.push_str(&SPACES[..count]);
    } else {
        for _ in 0..count {
            out.push(' ');
        }
    }
}

/// Rows we may spend so the prompt still has room: two rows of slack, and a
/// hard ceiling so a very tall window does not turn the splash into a poster.
fn rows_budget(height: usize) -> usize {
    height.saturating_sub(2).clamp(10, 28)
}

fn render(color: bool, width: usize, height: usize) -> String {
    let budget = rows_budget(height);
    let cmds = command_lines(color, width);
    let head_budget = budget.saturating_sub(cmds.len() + 1);
    let mut out = emblem_layout(color, width, head_budget);
    out.push(String::new());
    out.extend(cmds);
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Emblem (or plain text) chosen by width *and* by the row budget handed in.
fn emblem_layout(color: bool, width: usize, budget: usize) -> Vec<String> {
    if width >= XL_MIN && art::EMBLEM_XL.len() <= budget {
        return side_by_side(&art::EMBLEM_XL, &art::EMBLEM_XL_GEO, color, true);
    }
    if width >= WIDE_MIN && art::EMBLEM_WIDE.len() <= budget {
        return side_by_side(&art::EMBLEM_WIDE, &art::EMBLEM_WIDE_GEO, color, true);
    }
    if width >= COMPACT_MIN && art::EMBLEM_SMALL.len() <= budget {
        return side_by_side(&art::EMBLEM_SMALL, &art::EMBLEM_SMALL_GEO, color, true);
    }
    // Narrow window, or too short for a side-by-side row: keep the emblem and
    // shed decoration step by step until it fits.
    if width >= STACK_MIN {
        if art::EMBLEM_SMALL.len() <= budget {
            return stacked(&art::EMBLEM_SMALL, &art::EMBLEM_SMALL_GEO, color, width, budget);
        }
        // not even the emblem fits: readable text beats a cropped emblem
        return text_head(color, width);
    }
    text_head(color, width)
}

/// Emblem above the wordmark, dropping rows until it fits `budget`:
/// tagline -> spacer -> wordmark alone -> emblem with a plain title.
fn stacked(
    emblem: &[&str],
    geo: &art::Geo,
    color: bool,
    width: usize,
    budget: usize,
) -> Vec<String> {
    for tail in [7usize, 6, 5, 2] {
        // the block wordmark is 46 columns wide: below that it would wrap
        if tail >= 5 && width < WORDMARK_COLS {
            continue;
        }
        if emblem.len() + tail > budget {
            continue;
        }
        let mut out: Vec<String> = emblem
            .iter()
            .enumerate()
            .map(|(r, line)| emblem_line(line, r, emblem.len(), geo, color))
            .collect();
        match tail {
            7 => {
                out.push(String::new());
                out.extend(wordmark_column(color, true, false));
            }
            6 => {
                out.push(String::new());
                out.extend(wordmark_column(color, false, false));
            }
            5 => out.extend(wordmark_column(color, false, false)),
            _ => {
                out.push(String::new());
                out.push(paint(color, MARK_TOP, "UniDTK"));
            }
        }
        return out;
    }
    text_head(color, width)
}

/// Last resort: no emblem, no block wordmark, one readable title line.
fn text_head(color: bool, width: usize) -> Vec<String> {
    let title = "UniDTK  Unified Desktop Täglich Kit";
    if width < title.chars().count() {
        return vec![paint(color, MARK_TOP, "UniDTK")];
    }
    vec![paint(color, MARK_TOP, title)]
}

/// Command map: one row per group, wrapped when the window is too narrow.
fn command_lines(color: bool, width: usize) -> Vec<String> {
    let mut out = vec![paint(color, DIM, "COMMANDS  ")];
    for (group, cmds) in COMMANDS {
        let mut head = String::with_capacity(64);
        head.push_str("  ");
        head.push_str(&paint(color, ACCENT, group));
        push_spaces(&mut head, 11usize.saturating_sub(group.len()).max(1));

        if display_width(&head) + cmds.chars().count() <= width {
            let mut line = head;
            line.push_str(&paint(color, DIM, cmds));
            out.push(line);
            continue;
        }

        // Wrap the command list itself; the group name keeps its own first row.
        let mut line = head;
        let mut plain = String::new();
        for item in cmds.split(" · ") {
            let add = if plain.is_empty() {
                item.to_string()
            } else {
                format!(" · {item}")
            };
            if !plain.is_empty() && CMD_INDENT + plain.chars().count() + add.chars().count() > width {
                let mut done = std::mem::take(&mut line);
                done.push_str(&paint(color, DIM, &plain));
                out.push(done);
                push_spaces(&mut line, CMD_INDENT);
                plain = item.to_string();
            } else {
                plain.push_str(&add);
            }
        }
        line.push_str(&paint(color, DIM, &plain));
        out.push(line);
    }
    out
}

/// Emblem on the left, wordmark column centred against it on the right.
fn side_by_side(emblem: &[&str], geo: &art::Geo, color: bool, with_stack: bool) -> Vec<String> {
    let rows = emblem.len();
    let right = wordmark_column(color, true, with_stack);
    let pad = rows.saturating_sub(right.len()) / 2;
    let mut out = Vec::with_capacity(rows);
    let emblem_cols = emblem
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(geo.cols);

    for (r, line) in emblem.iter().enumerate() {
        let mut s = emblem_line(line, r, rows, geo, color);
        let idx = r as isize - pad as isize;
        if idx >= 0 && (idx as usize) < right.len() {
            let used = display_width(&s);
            push_spaces(&mut s, emblem_cols.saturating_sub(used));
            s.push_str(GAP);
            s.push_str(&right[idx as usize]);
        }
        out.push(s);
    }
    out
}

fn wordmark_column(color: bool, with_tagline: bool, with_stack: bool) -> Vec<String> {
    let rows = art::WORDMARK.len();
    let mut out: Vec<String> = art::WORDMARK
        .iter()
        .enumerate()
        .map(|(r, line)| render_wordmark_line(line, r, rows, color))
        .collect();
    if with_tagline {
        out.push(paint(color, DIM, "Unified Desktop Täglich Kit"));
    }
    if with_stack {
        out.push(paint(color, DIM, "Rust · Tauri v2 · WebView2 · redb"));
    }
    out
}

/// Renders a single row of the block-letter wordmark with German flag colors on DTK.
fn render_wordmark_line(line: &str, row: usize, rows: usize, color: bool) -> String {
    if !color {
        return line.to_string();
    }
    let t = grad(row, rows);
    let uni_c = lerp(MARK_TOP, MARK_BOTTOM, t);
    let d_c = lerp(GERMAN_D_TOP, GERMAN_D_BOTTOM, t);
    let t_c = lerp(GERMAN_T_TOP, GERMAN_T_BOTTOM, t);
    let k_c = lerp(GERMAN_K_TOP, GERMAN_K_BOTTOM, t);

    let mut out = String::with_capacity(128);
    let mut current_color: Option<(u8, u8, u8)> = None;

    for (col, ch) in line.chars().enumerate() {
        let c = if col < 21 {
            uni_c
        } else if col < 29 {
            d_c
        } else if col < 38 {
            t_c
        } else {
            k_c
        };
        if current_color != Some(c) {
            write_sgr(&mut out, c);
            current_color = Some(c);
        }
        out.push(ch);
    }
    out.push_str(RESET);
    out
}

/// One emblem row, coloured per character from the art geometry and modulated
/// by the generated shade byte (128 == neutral).
fn emblem_line(line: &str, row: usize, rows: usize, geo: &art::Geo, color: bool) -> String {
    if !color {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 64);
    let mut current: Option<(u8, u8, u8)> = None;
    for (col, ch) in line.chars().enumerate() {
        if ch == ' ' {
            out.push(ch);
            continue;
        }
        let mut c = emblem_color(col, row, rows, geo);
        if let Some(s) = geo.shade.get(row * geo.cols + col).copied() {
            c = modulate(c, s);
        }
        if current != Some(c) {
            write_sgr(&mut out, c);
            current = Some(c);
        }
        out.push(ch);
    }
    out.push_str(RESET);
    out
}

/// Photo-derived shade byte: 128 is exact identity, so neutral cells keep their
/// brand colour to the bit.
fn modulate(c: (u8, u8, u8), shade: u8) -> (u8, u8, u8) {
    let f = |v: u8| (((v as u32) * (shade as u32) + 64) / 128).min(255) as u8;
    (f(c.0), f(c.1), f(c.2))
}

// Precomputed tilt angles: 26.7 degrees in radians
const COS_BETA: f32 = 0.8933713;
const SIN_BETA: f32 = 0.4493189;

/// Serpent / planet / rings, decided in the generator's dot space.
///
/// The cell centre maps to dot space exactly the way `make_splash.py` lays the
/// grid out: `scale_*` generator dots per braille dot (square, equal on both
/// axes) and `top_off` cell rows removed from the top of the untrimmed block.
pub fn emblem_color(col: usize, row: usize, rows: usize, geo: &art::Geo) -> (u8, u8, u8) {
    let t = grad(row, rows);
    let dot_x = (2 * col + 1) as f32 * geo.scale_x;
    let dot_y = (4 * (row + geo.top_off) + 2) as f32 * geo.scale_y;
    let (sx, sy, sr) = art::SPHERE;
    let (dx, dy) = (dot_x - sx, dot_y - sy);

    // Planet sphere (radius check without sqrt)
    let r_sphere = sr * 1.05;
    if dx * dx + dy * dy <= r_sphere * r_sphere {
        return lerp(PLANET_TOP, PLANET_BOTTOM, t);
    }

    // Tilted coordinate frame of Saturn
    let u = dx * COS_BETA + dy * SIN_BETA;
    let v = -dx * SIN_BETA + dy * COS_BETA;

    // Rings check
    for &(a, b) in &art::RINGS {
        let u_norm = u / a;
        let v_norm = v / b;
        let rho_sq = u_norm * u_norm + v_norm * v_norm;
        if (0.72 * 0.72..=1.12 * 1.12).contains(&rho_sq) {
            return lerp(RING_TOP, RING_BOTTOM, t);
        }
    }

    // Inside Saturn celestial envelope: strictly keep ring color to prevent bleed
    let sat_u = u / 56.0;
    let sat_v = v / 18.0;
    if sat_u * sat_u + sat_v * sat_v <= 1.0 {
        return lerp(RING_TOP, RING_BOTTOM, t);
    }

    lerp(BODY_TOP, BODY_BOTTOM, t)
}

fn grad(row: usize, rows: usize) -> f32 {
    if rows <= 1 {
        0.0
    } else {
        (row as f32 / (rows - 1) as f32).clamp(0.0, 1.0)
    }
}

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

#[inline]
fn write_sgr(out: &mut String, c: (u8, u8, u8)) {
    use std::fmt::Write;
    let _ = write!(out, "\x1b[38;2;{};{};{}m", c.0, c.1, c.2);
}

fn paint(color: bool, c: (u8, u8, u8), text: &str) -> String {
    if color {
        let mut s = String::with_capacity(text.len() + 24);
        write_sgr(&mut s, c);
        s.push_str(text);
        s.push_str(RESET);
        s
    } else {
        text.to_string()
    }
}

/// Characters as the terminal counts them (braille and block letters are all
/// single-width; only the SGR escapes are invisible).
fn display_width(s: &str) -> usize {
    let mut n = 0;
    let mut esc = false;
    for ch in s.chars() {
        if esc {
            if ch == 'm' {
                esc = false;
            }
        } else if ch == '\x1b' {
            esc = true;
        } else {
            n += 1;
        }
    }
    n
}

#[cfg(windows)]
fn console() -> (bool, usize, usize) {
    use std::ffi::c_void;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5;
    const STD_INPUT_HANDLE: u32 = 0xFFFFFFF6;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor: Coord,
        attrs: u16,
        window: SmallRect,
        max_size: Coord,
    }

    extern "system" {
        fn GetStdHandle(n: u32) -> *mut c_void;
        fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
        fn SetConsoleOutputCP(wCodePageID: u32) -> i32;
        fn GetConsoleScreenBufferInfo(h: *mut c_void, info: *mut ConsoleScreenBufferInfo) -> i32;
    }

    /// Visible window in cells (the *buffer* is often ~9000 rows tall).
    unsafe fn window(h: *mut c_void) -> Option<(usize, usize)> {
        if h.is_null() || h == (-1isize as *mut c_void) {
            return None;
        }
        let mut info: ConsoleScreenBufferInfo = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(h, &mut info) == 0 {
            return None;
        }
        let w = (info.window.right - info.window.left + 1) as usize;
        let h = (info.window.bottom - info.window.top + 1) as usize;
        if w > 0 && h > 0 {
            Some((w, h))
        } else {
            None
        }
    }

    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if let Some((w, h)) = window(handle) {
            let mut mode: u32 = 0;
            if GetConsoleMode(handle, &mut mode) == 1 {
                // colour is only honest when the console really accepted VT:
                // a raw ESC on a host without it is garbage, not a gradient
                let vt = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 1;
                let cp = SetConsoleOutputCP(65001) == 1;
                if vt && cp {
                    return (true, w, h);
                }
            }
            return (false, w, h);
        }
        // stdout is redirected: the reader's window still sizes the layout
        if let Some((w, h)) = window(GetStdHandle(STD_INPUT_HANDLE)) {
            return (false, w, h);
        }
        (false, 100, 24)
    }
}

#[cfg(not(windows))]
fn console() -> (bool, usize, usize) {
    (false, 100, 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_sgr(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut esc = false;
        for ch in s.chars() {
            if esc {
                if ch == 'm' {
                    esc = false;
                }
            } else if ch == '\x1b' {
                esc = true;
            } else {
                out.push(ch);
            }
        }
        out
    }

    const ARTS: [(&[&str], &art::Geo); 3] = [
        (&art::EMBLEM_XL, &art::EMBLEM_XL_GEO),
        (&art::EMBLEM_WIDE, &art::EMBLEM_WIDE_GEO),
        (&art::EMBLEM_SMALL, &art::EMBLEM_SMALL_GEO),
    ];

    #[test]
    fn wide_layout_fits_a_standard_terminal() {
        for (color, height) in [(true, 24usize), (true, 30), (false, 24)] {
            let s = render(color, 120, height);
            let lines: Vec<&str> = s.lines().collect();
            assert!(
                lines.len() <= rows_budget(height),
                "splash is {} rows at height {height}",
                lines.len()
            );
            for l in &lines {
                assert!(display_width(l) <= 120, "line overflows: {:?}", l);
            }
            assert!(s.contains("Unified Desktop Täglich Kit"));
            assert!(s.contains("cdp-list"));
        }
    }

    #[test]
    fn every_layout_stays_inside_its_width_and_height() {
        for height in [24usize, 30] {
            for width in [120usize, 100, 98, 96, 86, 85, 78, 76, 60, 58, 46, 45, 40, 30] {
                for color in [true, false] {
                    let s = render(color, width, height);
                    let rows = s.lines().count();
                    assert!(
                        rows <= rows_budget(height),
                        "height {height} width {width}: {rows} rows"
                    );
                    for l in s.lines() {
                        assert!(
                            display_width(l) <= width,
                            "width {width}: {:?}",
                            l
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn plain_output_has_no_escapes() {
        for width in [120usize, 60, 40] {
            let s = render(false, width, 24);
            assert!(!s.contains('\x1b'), "width {width}");
            assert!(s.contains("invoke · help"), "width {width}");
        }
    }

    #[test]
    fn colour_output_is_plain_output_plus_sgr() {
        for width in [120usize, 98, 86, 78, 60, 40] {
            for height in [24usize, 30] {
                assert_eq!(
                    strip_sgr(&render(true, width, height)),
                    render(false, width, height),
                    "width {width} height {height}"
                );
            }
        }
    }

    #[test]
    fn art_rows_shade_and_geometry_agree() {
        for (art_lines, geo) in ARTS {
            assert_eq!(
                geo.shade.len(),
                geo.cols * art_lines.len(),
                "shade length vs cols*rows"
            );
            for (row, line) in art_lines.iter().enumerate() {
                assert_eq!(
                    line.chars().count(),
                    geo.cols,
                    "row {row} length vs geo.cols"
                );
                for (col, ch) in line.chars().enumerate() {
                    if ch == ' ' {
                        continue;
                    }
                    let code = ch as u32;
                    assert!(
                        (0x2801..=0x28FF).contains(&code),
                        "row {row} col {col}: {code:#x} is not a lit braille cell"
                    );
                    // neutral or a quantised step of the photo shade
                    let s = geo.shade[row * geo.cols + col];
                    assert_eq!(s % 8, 0, "unquantised shade {s}");
                }
            }
        }
    }

    #[test]
    fn photo_shade_reaches_the_serpent_body() {
        for (art_lines, geo) in ARTS {
            let mut neutral = 0;
            let mut textured = 0;
            for &s in geo.shade {
                if s == 128 {
                    neutral += 1;
                } else {
                    textured += 1;
                    assert!((80..=176).contains(&s), "shade out of range {s}");
                }
            }
            assert!(textured > 0, "no texture in the art");
            // the emblem is mostly serpent, so a decent share of it is textured
            assert!(textured * 3 > neutral, "texture too sparse");
            let _ = art_lines;
        }
    }

    #[test]
    fn emblem_geometry_separates_planet_rings_and_body() {
        for (art_lines, geo) in ARTS {
            let rows = art_lines.len();
            let mut planet = 0;
            let mut ring = 0;
            let mut body = 0;
            for row in 0..rows {
                let t = grad(row, rows);
                let planet_col = lerp(PLANET_TOP, PLANET_BOTTOM, t);
                let ring_col = lerp(RING_TOP, RING_BOTTOM, t);
                let body_col = lerp(BODY_TOP, BODY_BOTTOM, t);
                for col in 0..geo.cols {
                    let c = emblem_color(col, row, rows, geo);
                    if c == planet_col {
                        planet += 1;
                    } else if c == ring_col {
                        ring += 1;
                    } else if c == body_col {
                        body += 1;
                    }
                }
            }
            assert!(planet > 0 && ring > 0 && body > 0, "p {planet} r {ring} b {body}");
            // every cell of the block resolves to exactly one zone
            assert_eq!(planet + ring + body, geo.cols * rows);
        }
    }

    #[test]
    fn wordmark_has_correct_proportions() {
        for line in art::WORDMARK {
            assert_eq!(line.chars().count(), WORDMARK_COLS, "wordmark line length");
        }
    }

    #[test]
    fn thresholds_follow_the_art_width() {
        assert_eq!(
            WIDE_MIN,
            art::EMBLEM_WIDE_GEO.cols + GAP.len() + WORDMARK_COLS + 1
        );
        assert!(XL_MIN > WIDE_MIN);
        assert!(WIDE_MIN > COMPACT_MIN);
        assert!(COMPACT_MIN > STACK_MIN);
    }

    #[test]
    fn side_by_side_right_column_is_strictly_aligned() {
        for color in [false, true] {
            for (emblem, geo) in ARTS {
                for height in [24usize, 30] {
                    let budget = rows_budget(height)
                        .saturating_sub(command_lines(color, 200).len() + 1);
                    if emblem.len() > budget {
                        continue;
                    }
                    let right = wordmark_column(color, true, true);
                    let rows = emblem.len();
                    let pad = rows.saturating_sub(right.len()) / 2;
                    let lines = side_by_side(emblem, geo, color, true);

                    let expected_start = emblem
                        .iter()
                        .map(|line| display_width(line))
                        .max()
                        .unwrap()
                        + display_width(GAP);

                    for (idx, item) in right.iter().enumerate() {
                        let line_idx = pad + idx;
                        let full_line = &lines[line_idx];
                        assert!(
                            full_line.ends_with(item),
                            "row {idx} must end with right item"
                        );
                        let prefix = &full_line[..full_line.len() - item.len()];
                        assert_eq!(
                            display_width(prefix),
                            expected_start,
                            "row {idx} (color={color}) starts at display col {} instead of {expected_start}",
                            display_width(prefix)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn narrow_windows_wrap_the_command_map() {
        for width in [40usize, 30] {
            let lines = command_lines(false, width);
            assert!(lines.len() > COMMANDS.len(), "expected wrapping at {width}");
            for l in &lines {
                assert!(l.chars().count() <= width, "{width}: {l:?}");
            }
            let all = lines.join("\n");
            for (_, cmds) in COMMANDS {
                for item in cmds.split(" · ") {
                    assert!(all.contains(item), "{width}: lost {item}");
                }
            }
        }
    }

    #[test]
    fn tall_terminals_earn_the_bigger_emblem() {
        let tall = render(true, 120, 30);
        let short = render(true, 120, 24);
        assert!(tall.lines().count() > short.lines().count());
        // XL emblem is 21 rows: the first block (up to the blank separator) must be that tall
        let first_block = tall.lines().take_while(|l| !l.is_empty()).count();
        assert_eq!(first_block, art::EMBLEM_XL.len(), "expected the XL emblem");
    }

    #[test]
    fn short_windows_keep_the_prompt_room() {
        assert_eq!(rows_budget(24), 22);
        assert_eq!(rows_budget(30), 28);
        assert_eq!(rows_budget(10), 10);
        for height in [24usize, 30, 20] {
            for width in [120usize, 78, 60] {
                let rows = render(true, width, height).lines().count();
                assert!(rows <= rows_budget(height), "h{height} w{width}: {rows}");
            }
        }
    }
}
