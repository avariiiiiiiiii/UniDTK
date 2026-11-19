//! The Glyph Bar's native half (`r[modules.vertical-slice]`): the LED volume strip of
//! `modules/glyph-bar/ui` on the native desk, through the shell's widget contract
//! (`unidtk_shell::widget`). The shell places the card; this crate draws the ten LEDs (the flat
//! material) or the groove, the fill, the beads and the knob (the jelly one), and turns presses
//! and drags into `set_system_volume` and `set_system_mute`. The level comes back by the kernel's
//! `system-volume` state event, whose first frame is the catch-up a subscribe replays.

mod logic;

use logic::{Ask, Volume, GAP, SEGMENTS};
use rune::el::{el, Drag, El, Phase};
use rune::{Color, Shadow};
use serde_json::{json, Value};
use unidtk_shell::theme::{derived, oklch_in_gamut, oklch_of};
use unidtk_shell::widget::{Act, Init, Material, Theme, View, Widget, WidgetType};

pub static WIDGET: WidgetType = WidgetType { id: "glyph-bar", events: &["system-volume"], make };

fn make(_: &Init<'_>) -> Box<dyn Widget> {
    // Before the first read the page shows `{ level: 0, muted: false }`: an empty bar.
    Box::new(GlyphBar { volume: Volume { level: 0, muted: false }, grab: None })
}

struct GlyphBar {
    volume: Volume,
    /// The LED the running gesture is on, and whether it has moved off the one it began on.
    grab: Option<(usize, bool)>,
}

fn volume_of(value: &Value) -> Option<Volume> {
    Some(Volume { level: value.get("level")?.as_f64()?.clamp(0.0, 100.0) as u8, muted: value.get("muted").and_then(Value::as_bool).unwrap_or(false) })
}

fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color, inset: bool) -> Shadow {
    Shadow { x, y, blur, spread, color, inset }
}

impl GlyphBar {
    /// Shows `volume` at once and asks the kernel for it, as the page does: the answer's
    /// read-back replaces it, unless a later drag mark has moved on (`dragGlyphBarTo`).
    fn ask(&mut self, volume: Volume, ask: Ask, act: &mut Act<'_>) {
        self.volume = volume;
        match ask {
            Ask::Volume(level) => act.call::<GlyphBar>("set_system_volume", json!({ "level": level }), move |bar, result, act| {
                bar.answered(result, Some(level), "volume", act)
            }),
            Ask::Mute(muted) => act.call::<GlyphBar>("set_system_mute", json!({ "muted": muted }), |bar, result, act| bar.answered(result, None, "mute", act)),
        }
    }

    fn answered(&mut self, result: Result<Value, String>, sent: Option<u8>, what: &str, act: &mut Act<'_>) {
        match result {
            Ok(value) => {
                if sent.is_none_or(|level| self.volume.level == level) {
                    if let Some(read) = volume_of(&value) {
                        self.volume = read;
                    }
                }
            }
            Err(error) => act.debug(format!("{what}: {error}")),
        }
    }

    /// One phase of a gesture on the strip: a press on a LED is a click, a move onto another LED
    /// sets the level of the mark under the pointer (`pointerdown`, `pointermove`, `click`).
    fn gesture(&mut self, drag: &Drag, act: &mut Act<'_>) {
        let index = logic::index_at(drag.h - drag.y, drag.h);
        match drag.phase {
            Phase::Down => self.grab = Some((index, false)),
            Phase::Move => {
                if let Some((last, moved)) = self.grab {
                    if last != index {
                        self.grab = Some((index, true));
                        let (volume, ask) = logic::drag_to(index);
                        self.ask(volume, ask, act);
                    } else {
                        self.grab = Some((last, moved));
                    }
                }
            }
            Phase::Up => {
                if let Some((last, false)) = self.grab.take() {
                    if !drag.cancelled() {
                        let (volume, ask) = logic::press(last, self.volume);
                        self.ask(volume, ask, act);
                    }
                }
                self.grab = None;
            }
        }
    }
}

/// The theme's coral (`--palette-coral`): the palette slot, read as a tint nobody maps.
fn coral(theme: &Theme) -> Color {
    theme.tint("glyph-bar:palette", "none", "coral")
}

/// `rgba(255, 72, 58, 1)`, the lit red LED.
const RED: Color = Color { r: 1.0, g: 72.0 / 255.0, b: 58.0 / 255.0, a: 1.0 };

fn flat_led(theme: &Theme, index: usize, lit: usize, muted: bool, side: f32) -> El {
    let on = index < lit;
    let white = Color::WHITE;
    let led = el().w(side).h(side).radius(2.0).label(format!("Громкость {}%", level_label(index)));
    if index == 0 {
        // `.gb-seg.red`, `.red.on` and `.red.muted-only`.
        if on || muted {
            return led
                .fill(RED)
                .box_shadow(shadow(0.0, 0.0, 5.0, 0.0, theme.status_error.alpha(0.85), false))
                .box_shadow(shadow(0.0, 0.0, 12.0, 0.0, theme.status_error.alpha(0.45), false));
        }
        return led.fill(theme.status_error.alpha(0.42)).box_shadow(shadow(0.0, 0.0, 0.0, 1.0, Color { r: 1.0, g: 90.0 / 255.0, b: 80.0 / 255.0, a: 0.25 }, true));
    }
    if on {
        return led
            .fill(theme.bg_elevated.alpha(0.98))
            .box_shadow(shadow(0.0, 0.0, 5.0, 0.0, white.alpha(0.7), false))
            .box_shadow(shadow(0.0, 0.0, 12.0, 0.0, white.alpha(0.35), false))
            .box_shadow(shadow(0.0, 0.0, 0.0, 1.0, white.alpha(0.35), true));
    }
    led.fill(theme.bg_elevated.alpha(0.28)).box_shadow(shadow(0.0, 0.0, 0.0, 1.0, white.alpha(0.12), true))
}

/// The label's percent: 0 for the red LED, else `(i + 1) * 10`.
fn level_label(index: usize) -> usize {
    if index == 0 { 0 } else { (index + 1) * 10 }
}

/// `.gb-seg::before` under jelly: a bead in the middle of the square's hit area.
fn jelly_bead(theme: &Theme, index: usize, lit: usize, muted: bool, side: f32) -> El {
    let white = Color::WHITE;
    let cell = el().w(side).h(side).label(format!("Громкость {}%", level_label(index)));
    let (inset, bead) = if index == 0 && muted {
        // `.gb-seg.red.muted-only::before`: a coral pebble with a highlight and a shadow.
        let coral = coral(theme);
        let pebble = el().size(side * 0.68, side * 0.68).radius(side * 0.34)
            .linear_gradient(160f32.to_radians(), [(0.0, coral), (1.0, derived(coral, -0.12, 1.3, 0.0, 1.0))])
            .box_shadow(shadow(0.0, 2.0, 5.0, 0.0, derived(coral, -0.3, 1.0, 0.0, 0.45), false))
            .child(el().size(side * 0.68, side * 0.68).radius(side * 0.34).radial_gradient([0.3, 0.2], [0.9 * side * 0.68, 0.7 * side * 0.68], [(0.0, white.alpha(0.7)), (0.6, white.alpha(0.0))]));
        (side * 0.16, pebble)
    } else {
        let colour = if index == 0 {
            // `.gb-seg.red::before` comes after `.on::before`, so a lit red LED is coral too.
            coral(theme).alpha(0.6)
        } else if index < lit {
            white.alpha(0.5)
        } else {
            theme.fg.alpha(0.18)
        };
        let diameter = side * 0.24;
        (side * 0.38, el().size(diameter, diameter).radius(diameter / 2.0).fill(colour))
    };
    cell.child(el().absolute(inset, inset).size(side - 2.0 * inset, side - 2.0 * inset).items_center().justify_center().child(bead))
}

impl Widget for GlyphBar {
    fn on_event(&mut self, name: &str, payload: &Value, _snapshot: bool, _act: &mut Act<'_>) {
        if name == "system-volume" {
            if let Some(volume) = volume_of(payload) {
                self.volume = volume;
            }
        }
    }

    fn content(&self, v: &View<'_>) -> Option<El> {
        let theme = v.theme;
        let side = logic::side_for_width(v.tile.w);
        let (w, h) = (v.tile.w, v.tile.h);
        let lit = logic::lit(self.volume);
        let muted = self.volume.muted;
        // `justify-content: center` on a `column-reverse`: the stack's bottom edge.
        let bottom_gap = ((h - (side * SEGMENTS as f32 + GAP * (SEGMENTS as f32 - 1.0))) / 2.0).max(0.0);
        let mut strip = el().key(format!("hit:glyph-bar:{}", v.id)).absolute(v.tile.x, v.tile.y).size(w, h);
        if !v.arranging {
            strip = strip.on_drag(v.on_drag::<GlyphBar>(|bar, drag, act| bar.gesture(drag, act)));
        }

        let jelly = theme.material == Material::Jelly;
        if jelly {
            // The groove (`.gb-container::before`) and the level fill (`::after`).
            let (left, width) = (w * 0.22, w * 0.56);
            strip = strip.child(
                el().absolute(left, 0.0).size(width, h).radius(width / 2.0)
                    .fill(derived(v.tint, -0.08, 1.0, 0.0, 0.45))
                    .box_shadow(shadow(0.0, 1.0, 2.0, 0.0, theme.fg.alpha(0.14), true)),
            );
            let fill_h = (lit as f32 * (h + GAP) / SEGMENTS as f32 - GAP).max(0.0);
            strip = strip.child(
                el().absolute(left, h - fill_h).size(width, fill_h).radius(width / 2.0)
                    .linear_gradient(0.0, [(0.0, derived(v.tint, -0.26, 1.9, 0.0, 1.0)), (1.0, derived(v.tint, -0.14, 1.6, 0.0, 1.0))]),
            );
        }
        for index in 0..SEGMENTS {
            let y = h - bottom_gap - (index as f32 + 1.0) * side - index as f32 * GAP;
            let led = if jelly { jelly_bead(theme, index, lit, muted, side) } else { flat_led(theme, index, lit, muted, side) };
            strip = strip.child(led.absolute((w - side) / 2.0, y));
        }
        if jelly {
            // `.gb-knob`: one light bead on the top lit LED; hidden (small and clear) at no level.
            let size = side * 0.72;
            let has_level = !muted && lit > 0;
            let (_, c, hue) = oklch_of(v.tint);
            let rim = oklch_in_gamut(0.92, c * 0.5, hue, 1.0);
            let bottom = lit as f32 * (side + GAP) - GAP - side * 0.86;
            let mut knob = el().absolute((w - size) / 2.0, h - bottom - size).size(size, size).radius(size / 2.0)
                .radial_gradient([0.35, 0.25], [0.8 * size, 0.7 * size], [(0.0, Color::WHITE), (0.6, Color::WHITE.alpha(0.86)), (1.0, rim)])
                .box_shadow(shadow(0.0, 1.0, 3.0, 0.0, derived(v.tint, -0.4, 1.0, 0.0, 0.45), false))
                .opacity(if has_level { 1.0 } else { 0.0 })
                .scale(if has_level { 1.0 } else { 0.4 });
            // `:not(:has(+ .gb-seg.on)) ~ .gb-knob`: a coral rim while the top lit LED is the red one.
            if lit == 1 && !muted {
                knob = knob.outline(2.0, 0.0, coral(theme));
            }
            strip = strip.child(knob);
        }
        Some(el().absolute(0.0, 0.0).size(v.frame.w, v.frame.h).child(strip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_volume_reads_as_level_and_muted() {
        assert_eq!(volume_of(&json!({ "level": 42, "muted": true })), Some(Volume { level: 42, muted: true }));
        assert_eq!(volume_of(&json!({ "level": 140.0 })), Some(Volume { level: 100, muted: false }));
        assert_eq!(volume_of(&json!({ "muted": true })), None);
    }

    #[test]
    fn aria_labels_name_the_level_each_led_sets() {
        assert_eq!((level_label(0), level_label(1), level_label(9)), (0, 20, 100));
    }
}
