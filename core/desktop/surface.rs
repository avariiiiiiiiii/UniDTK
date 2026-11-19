//! The one object-to-surface morph every widget opens with (`r[motion.clip-morph]`), as
//! `surface-manager.js` runs a container morph: the card is laid out at its surface's box from
//! the first frame, and what moves is the clip, from the tile's rectangle and radius to the
//! whole box and the surface radius.
//!
//! The shell drives it frame by frame from one progress value (`r[shell.one-morph]`), with the
//! page's curves, not a spring:
//! - open: the clip's insets and radius go from where they are to the box on `expoOut`, over
//!   `calculateDuration` of the growth;
//! - close: three keyframes on `surfaceExit` — from where the clip is, to 85 % of the way at
//!   0.8 (the radius already the tile's), to the tile at 1 — over `0.65 x T x sqrt(r)`, `r` the
//!   share of the way still to go, so a close that interrupts an open is shorter;
//! - a reversal starts the other leg from the clip as it is (`r[motion.reversal]`);
//! - the jelly wash (`syncSurfaceWash`): in over 0.3 of the leg on `expoOut`, out over the
//!   second half of the close on `surfaceExit`, each from where it is.
//!
//! The content is not faded: the page opens widgets with `suppressContentAnimation` and with
//! the body's transitions off, so the clip alone reveals and hides it.

use std::time::Instant;

use rune::Tx;

use crate::desk::Placement;
use crate::motion::{Motion, EXPO_OUT, SURFACE_EXIT};

/// Asks for the frame after the handler that started or turned a morph. Rune runs a frame's
/// `poll` before the input queued for it, so the poll that drives a morph sees it first on a
/// later frame, and every commit here is `instant`, so nothing else keeps the window drawing: a
/// morph started without this waits for the next timer wake (the clock's second) and then lands
/// in one frame, which is a freeze on the click.
pub fn keep_frames(tx: &mut Tx) {
    tx.wake_after(std::time::Duration::ZERO);
}

/// The clip in the box's coordinates: insets top, right, bottom, left, and the corner radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub insets: [f32; 4],
    pub radius: f32,
}

impl Clip {
    fn lerp(a: Clip, b: Clip, t: f32) -> Clip {
        let mut insets = a.insets;
        for (i, inset) in insets.iter_mut().enumerate() {
            *inset += (b.insets[i] - *inset) * t;
        }
        Clip { insets, radius: a.radius + (b.radius - a.radius) * t }
    }

    fn length(&self) -> f32 {
        self.insets.iter().map(|v| v * v).sum::<f32>().sqrt()
    }

    /// The visible window in the box's coordinates.
    pub fn window(&self, frame: Placement) -> Placement {
        let [top, right, bottom, left] = self.insets;
        Placement { x: left, y: top, w: (frame.w - left - right).max(0.0), h: (frame.h - top - bottom).max(0.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Leg {
    open: bool,
    start: Instant,
    /// Seconds.
    duration: f32,
    from: Clip,
    wash_from: f32,
    wash_delay: f32,
    wash_duration: f32,
}

/// One widget's surface, open or folding.
#[derive(Clone, Debug, PartialEq)]
pub struct Morph {
    pub id: String,
    /// The box on the canvas and the tile inside it, both as the open found them.
    pub frame: Placement,
    pub tile: Placement,
    /// The tile's radius and the surface's.
    pub origin_radius: f32,
    pub target_radius: f32,
    /// `calculateDuration` of this surface, in seconds.
    full: f32,
    leg: Leg,
}

/// Where a morph is at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub clip: Clip,
    /// 0 at the tile, 1 at the whole box: how far it is open, by the clip.
    pub progress: f32,
    /// The jelly wash's opacity.
    pub wash: f32,
    /// The leg has run out.
    pub done: bool,
    pub open: bool,
}

impl Morph {
    /// Starts opening `id` from its tile into `frame`.
    pub fn open(id: &str, motion: &Motion, frame: Placement, tile: Placement, origin_radius: f32, target_radius: f32, now: Instant) -> Morph {
        let full = motion.surface_duration((frame.w, frame.h), (tile.w, tile.h));
        let mut morph = Morph {
            id: id.to_string(),
            frame,
            tile,
            origin_radius,
            target_radius,
            full,
            leg: Leg { open: false, start: now, duration: 0.0, from: Clip { insets: [0.0; 4], radius: 0.0 }, wash_from: 0.0, wash_delay: 0.0, wash_duration: 0.0 },
        };
        morph.leg.from = morph.closed();
        morph.reopen(now);
        morph
    }

    /// The clip at the tile.
    fn closed(&self) -> Clip {
        let (f, t) = (self.frame, self.tile);
        Clip {
            insets: [t.y - f.y, (f.x + f.w) - (t.x + t.w), (f.y + f.h) - (t.y + t.h), t.x - f.x].map(|v| v.max(0.0)),
            radius: self.origin_radius,
        }
    }

    fn full_open(&self) -> Clip {
        Clip { insets: [0.0; 4], radius: self.target_radius }
    }

    pub fn is_open(&self) -> bool {
        self.leg.open
    }

    /// An open (again) from where the clip is now: the full `calculateDuration`, as the page
    /// opens.
    pub fn reopen(&mut self, now: Instant) {
        let pose = self.pose(now);
        self.leg = Leg {
            open: true,
            start: now,
            duration: self.full,
            from: pose.clip,
            wash_from: pose.wash,
            wash_delay: 0.0,
            wash_duration: 0.3 * self.full,
        };
    }

    /// A close from where the clip is now.
    pub fn close(&mut self, now: Instant) {
        let pose = self.pose(now);
        let end = self.closed();
        let full_distance = end.length();
        let remaining = if full_distance > 0.0 {
            (Clip { insets: std::array::from_fn(|i| end.insets[i] - pose.clip.insets[i]), radius: 0.0 }.length() / full_distance).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let duration = ((self.full * 0.65 * remaining.sqrt()) * 1000.0).round() / 1000.0;
        self.leg = Leg {
            open: false,
            start: now,
            duration,
            from: pose.clip,
            wash_from: pose.wash,
            wash_delay: 0.5 * duration,
            wash_duration: 0.5 * duration,
        };
    }

    /// The pose at `now`.
    pub fn pose(&self, now: Instant) -> Pose {
        let leg = self.leg;
        let elapsed = now.saturating_duration_since(leg.start).as_secs_f32();
        let t = if leg.duration > 0.0 { (elapsed / leg.duration).clamp(0.0, 1.0) } else { 1.0 };
        let clip = if leg.open {
            Clip::lerp(leg.from, self.full_open(), EXPO_OUT.eval(t))
        } else {
            let end = self.closed();
            let e = SURFACE_EXIT.eval(t);
            let middle = Clip { insets: std::array::from_fn(|i| leg.from.insets[i] + (end.insets[i] - leg.from.insets[i]) * 0.85), radius: self.origin_radius };
            if e <= 0.8 { Clip::lerp(leg.from, middle, e / 0.8) } else { Clip::lerp(middle, end, (e - 0.8) / 0.2) }
        };
        let wash_t = if leg.wash_duration > 0.0 { ((elapsed - leg.wash_delay) / leg.wash_duration).clamp(0.0, 1.0) } else { 1.0 };
        let wash = if leg.open {
            leg.wash_from + (1.0 - leg.wash_from) * EXPO_OUT.eval(wash_t)
        } else {
            leg.wash_from * (1.0 - SURFACE_EXIT.eval(wash_t))
        };
        let closed = self.closed().length();
        let progress = if closed > 0.0 { 1.0 - (clip.length() / closed).clamp(0.0, 1.0) } else if leg.open { 1.0 } else { 0.0 };
        let done = elapsed >= leg.duration.max(leg.wash_delay + leg.wash_duration);
        Pose { clip, progress, wash, done, open: leg.open }
    }

    /// When the running leg ends.
    #[cfg(test)]
    fn ends(&self) -> Instant {
        let leg = self.leg;
        leg.start + std::time::Duration::from_secs_f32(leg.duration.max(leg.wash_delay + leg.wash_duration))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::Profile;

    fn place(x: f32, y: f32, w: f32, h: f32) -> Placement {
        Placement { x, y, w, h }
    }

    fn motion() -> Motion {
        Motion { profile: Profile::Balanced, scale: 1.0, reduced: false }
    }

    #[test]
    fn an_open_runs_from_the_tile_to_the_box_on_expo_out() {
        let now = Instant::now();
        let frame = place(100.0, 50.0, 400.0, 420.0);
        let tile = place(200.0, 100.0, 210.0, 190.0);
        let morph = Morph::open("clock", &motion(), frame, tile, 20.0, 30.0, now);
        let start = morph.pose(now);
        assert_eq!(start.clip.insets, [50.0, 90.0, 180.0, 100.0]);
        assert_eq!(start.clip.radius, 20.0);
        assert_eq!(start.progress, 0.0);
        let end = morph.pose(morph.ends());
        assert_eq!(end.clip.insets, [0.0; 4]);
        assert_eq!(end.clip.radius, 30.0);
        assert!(end.done && end.progress == 1.0 && end.wash == 1.0);
        // calculateDuration of a 190 x 230 growth: 240 + 200 * ln(1 + 2.983) / ln(17) = 338 ms.
        assert_eq!(morph.full, 0.338);
    }

    #[test]
    fn a_close_passes_85_percent_at_0_8_with_the_tiles_radius_and_lasts_by_the_way_left() {
        let now = Instant::now();
        let frame = place(0.0, 0.0, 400.0, 420.0);
        let tile = place(100.0, 100.0, 200.0, 200.0);
        let mut morph = Morph::open("c", &motion(), frame, tile, 20.0, 30.0, now);
        let opened = morph.ends();
        morph.close(opened);
        assert!((morph.leg.duration - (morph.full * 0.65 * 1000.0).round() / 1000.0).abs() < 1e-6);
        // Find the instant the eased progress reaches 0.8: the clip is 85 % of the way.
        let mut at = opened;
        while SURFACE_EXIT.eval((at - opened).as_secs_f32() / morph.leg.duration) < 0.8 {
            at += std::time::Duration::from_micros(200);
        }
        let pose = morph.pose(at);
        let closed = morph.closed();
        assert!((pose.clip.insets[0] - closed.insets[0] * 0.85).abs() < 0.5, "{pose:?}");
        assert!((pose.clip.radius - 20.0).abs() < 0.2);
        let end = morph.pose(morph.ends());
        assert_eq!(end.clip.insets, closed.insets);
        assert_eq!(end.wash, 0.0);
    }

    /// What `close_card` does on a press: starts a morph and commits `instant`, then asks for the
    /// next frame or not. The window's poll is what drives the morph.
    fn wake_after_a_press_that_starts_a_morph(ask: bool) -> Option<f64> {
        struct Desk {
            ask: bool,
            started: bool,
        }
        let mut rt = rune::Runtime::new(
            Desk { ask, started: false },
            |_: &Desk, _: &mut rune::Cx<Desk>| {
                rune::el().size(100.0, 100.0).on_press_tx(|desk: &mut Desk, tx: &mut Tx| {
                    desk.started = true;
                    tx.instant();
                    if desk.ask {
                        keep_frames(tx);
                    }
                })
            },
            (100.0, 100.0),
            rune::Env::default(),
        );
        rt.poll = Some(|desk: &mut Desk, clock: f64, tx: &mut Tx| {
            if desk.started {
                tx.wake_at(clock);
            }
            false
        });
        rt.frame(0.016);
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(0.016);
        rt.wake_deadline()
    }

    #[test]
    fn a_press_that_starts_a_morph_keeps_the_frames_coming() {
        assert_eq!(wake_after_a_press_that_starts_a_morph(false), None, "poll runs before the press, so nothing else asks for a frame");
        assert!(wake_after_a_press_that_starts_a_morph(true).is_some(), "keep_frames arms the wake that runs poll again");
    }

    #[test]
    fn a_close_that_interrupts_an_open_is_shorter_and_starts_where_the_clip_is() {
        let now = Instant::now();
        let mut morph = Morph::open("c", &motion(), place(0.0, 0.0, 400.0, 420.0), place(100.0, 100.0, 200.0, 200.0), 20.0, 30.0, now);
        let early = now + std::time::Duration::from_millis(20);
        let before = morph.pose(early);
        morph.close(early);
        assert_eq!(morph.pose(early).clip, before.clip, "a reversal does not jump");
        assert!(morph.leg.duration < morph.full * 0.65);
        morph.reopen(early);
        assert_eq!(morph.pose(early).clip, before.clip);
    }
}
