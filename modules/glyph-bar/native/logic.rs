//! The Glyph Bar's rules, as `modules/glyph-bar/ui/widget.js` states them: which LEDs light for a
//! level, what a press, a drag and a wheel step ask the system for, and the strip's geometry.
//! Pure functions, so the page's quirks (a drag never reaches 10 %, the wheel mutes at 0) are
//! pinned by tests rather than by the drawing.

pub const SEGMENTS: usize = 10;
pub const GAP: f32 = 3.0;
pub const SIDE_DEFAULT: f32 = 10.0;
pub const SIDE_MIN: f32 = 6.0;
pub const SIDE_MAX: f32 = 64.0;

/// The system volume as the kernel reports it (`{level, muted}`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub level: u8,
    pub muted: bool,
}

/// What a gesture asks the kernel to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    Volume(u8),
    Mute(bool),
}

/// `glyphBarVolumeForIndex`: the level of a click on LED `index` (bottom is 0, the mute LED).
pub fn level_for_index(index: usize) -> u8 {
    if index == 0 { 0 } else { ((index + 1) * 10).min(100) as u8 }
}

/// `glyphBarLitCount`: how many LEDs light from the bottom. A level of 1-14 % lights one.
pub fn lit(volume: Volume) -> usize {
    if volume.muted || volume.level == 0 {
        0
    } else {
        (((volume.level as f32) / 10.0).round() as usize).clamp(1, SEGMENTS)
    }
}

/// The strip's height for a square side: ten squares and nine gaps.
pub fn height_for_side(side: f32) -> f32 {
    side.max(SIDE_MIN).round() * SEGMENTS as f32 + GAP * (SEGMENTS as f32 - 1.0)
}

/// The side a saved width means: a width outside 6-64 resets to 10, it is not clamped
/// (`renderGlyphBar`).
pub fn side_for_width(width: f32) -> f32 {
    let side = width.round();
    if (SIDE_MIN..=SIDE_MAX).contains(&side) { side } else { SIDE_DEFAULT }
}

/// `glyphBarIndexAt`: the LED under a pointer `from_bottom` points above the bar's bottom edge.
/// The pitch is the bar's height over ten, not the LED's real pitch (`side + 3`), as the page has
/// it, so the index drifts 0.3 points a LED at the default size.
pub fn index_at(from_bottom: f32, height: f32) -> usize {
    let pitch = height / SEGMENTS as f32;
    ((from_bottom / pitch).floor().max(0.0) as usize).min(SEGMENTS - 1)
}

/// A click on LED `index`: the bottom one toggles mute, the others set `(index + 1) * 10`.
pub fn press(index: usize, now: Volume) -> (Volume, Ask) {
    if index == 0 {
        let muted = !now.muted;
        (Volume { muted, ..now }, Ask::Mute(muted))
    } else {
        let level = level_for_index(index);
        (Volume { level, muted: false }, Ask::Volume(level))
    }
}

/// A drag onto LED `index`: `(index + 1) * 10`, so LED 0 is 10 %, not mute.
pub fn drag_to(index: usize) -> (Volume, Ask) {
    let level = ((index + 1) * 10) as u8;
    (Volume { level, muted: false }, Ask::Volume(level))
}

/// `nudgeSystemVolume`: a wheel step of `delta` points. Muted, a step up unmutes at
/// `max(10, delta)` (not at the level it had), a step down does nothing; reaching 0 mutes and
/// keeps the level.
pub fn nudge(delta: i32, now: Volume) -> Option<(Volume, Ask)> {
    if now.muted && delta < 0 {
        return None;
    }
    let next = if now.muted && delta > 0 { delta.clamp(10, 100) } else { (now.level as i32 + delta).clamp(0, 100) };
    Some(if next == 0 {
        (Volume { muted: true, ..now }, Ask::Mute(true))
    } else {
        (Volume { level: next as u8, muted: false }, Ask::Volume(next as u8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: fn(u8, bool) -> Volume = |level, muted| Volume { level, muted };

    #[test]
    fn leds_light_by_rounded_tens_and_a_low_level_still_lights_one() {
        assert_eq!(lit(V(0, false)), 0);
        assert_eq!(lit(V(1, false)), 1);
        assert_eq!(lit(V(14, false)), 1);
        assert_eq!(lit(V(15, false)), 2);
        assert_eq!(lit(V(100, false)), 10);
        assert_eq!(lit(V(70, true)), 0, "muted lights none");
    }

    #[test]
    fn a_click_sets_the_leds_ten_and_the_bottom_led_toggles_mute() {
        assert_eq!(press(1, V(50, false)), (V(20, false), Ask::Volume(20)));
        assert_eq!(press(9, V(50, true)), (V(100, false), Ask::Volume(100)));
        assert_eq!(press(0, V(50, false)), (V(50, true), Ask::Mute(true)));
        assert_eq!(press(0, V(50, true)), (V(50, false), Ask::Mute(false)));
    }

    #[test]
    fn a_drag_never_reaches_mute_and_its_lowest_mark_is_ten_percent() {
        assert_eq!(drag_to(0), (V(10, false), Ask::Volume(10)));
        assert_eq!(drag_to(9), (V(100, false), Ask::Volume(100)));
    }

    #[test]
    fn the_wheel_mutes_at_zero_and_unmutes_at_ten() {
        assert_eq!(nudge(10, V(50, false)), Some((V(60, false), Ask::Volume(60))));
        assert_eq!(nudge(10, V(95, false)), Some((V(100, false), Ask::Volume(100))));
        assert_eq!(nudge(-10, V(10, false)), Some((V(10, true), Ask::Mute(true))), "level kept, muted");
        assert_eq!(nudge(10, V(70, true)), Some((V(10, false), Ask::Volume(10))));
        assert_eq!(nudge(30, V(70, true)), Some((V(30, false), Ask::Volume(30))));
        assert_eq!(nudge(-10, V(70, true)), None);
    }

    #[test]
    fn geometry_follows_the_page() {
        assert_eq!(height_for_side(10.0), 127.0);
        assert_eq!(side_for_width(56.0), 56.0);
        assert_eq!(side_for_width(70.0), SIDE_DEFAULT, "out of range resets, not clamps");
        assert_eq!(side_for_width(4.0), SIDE_DEFAULT);
        // 127 points high: the pitch is 12.7, the real LED pitch 13.
        assert_eq!(index_at(0.0, 127.0), 0);
        assert_eq!(index_at(12.6, 127.0), 0);
        assert_eq!(index_at(12.8, 127.0), 1);
        assert_eq!(index_at(500.0, 127.0), 9);
        assert_eq!(index_at(-20.0, 127.0), 0);
    }
}
