//! The page's motion tokens (`core/ui/motion.js`) for the native host: the canonical easings
//! and durations, the motion profile and the user's scale from the theme, reduced motion,
//! and `calculateDuration` for surfaces. Every animation in the host takes its curve and its
//! time from here (`r[motion.canonical-tokens]`, `r[motion.no-hardcoded-ms]`,
//! `r[motion.reduced-motion]`).

use rune::{Anim, Curve, Easing};

/// `EASINGS` of `motion.js`.
pub const STANDARD: Easing = Easing::cubic(0.4, 0.0, 0.2, 1.0);
pub const EXPO_OUT: Easing = Easing::cubic(0.30, 0.95, 0.35, 1.00);
pub const SURFACE_EXIT: Easing = Easing::cubic(0.38, 0.05, 0.45, 1.00);
pub const SMOOTH: Easing = Easing::cubic(0.25, 0.1, 0.25, 1.0);
pub const CONTENT: Easing = Easing::cubic(0.25, 1.0, 0.5, 1.0);

/// `DURATIONS` of `motion.js`, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Hover,
    Transient,
    Motion,
    Layout,
}

impl Token {
    fn base_ms(self) -> f32 {
        match self {
            Token::Hover => 120.0,
            Token::Transient => 220.0,
            Token::Motion => 300.0,
            Token::Layout => 340.0,
        }
    }
}

/// `MOTION_PROFILES`: per-token factors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Responsive,
    Balanced,
    Fluid,
}

impl Profile {
    /// `setMotionProfile`: `bouncy` and `snappy` were renamed `responsive`; anything else is
    /// balanced.
    pub fn parse(name: &str) -> Profile {
        match name {
            "responsive" | "bouncy" | "snappy" => Profile::Responsive,
            "fluid" => Profile::Fluid,
            _ => Profile::Balanced,
        }
    }

    fn factor(self, token: Token) -> f32 {
        match (self, token) {
            (Profile::Balanced, _) => 1.0,
            (Profile::Responsive, Token::Hover) => 0.85,
            (Profile::Responsive, Token::Transient) => 0.9,
            (Profile::Responsive, Token::Motion) => 0.8,
            (Profile::Responsive, Token::Layout) => 0.75,
            (Profile::Fluid, Token::Hover) => 1.0,
            (Profile::Fluid, Token::Transient) => 1.1,
            (Profile::Fluid, Token::Motion) => 1.2,
            (Profile::Fluid, Token::Layout) => 1.3,
        }
    }
}

/// The motion state the theme and the system set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub profile: Profile,
    /// `motionScale`, 0..=3; 0 is "без анимаций".
    pub scale: f32,
    /// Windows' "Show animations" is off: every duration is 1 ms, as `prefers-reduced-motion`
    /// makes it on the page.
    pub reduced: bool,
}

impl Motion {
    pub fn new(profile: Profile, scale: f32) -> Motion {
        let scale = if scale.is_finite() { scale.clamp(0.0, 3.0) } else { 1.0 };
        Motion { profile, scale, reduced: system_reduces_motion() }
    }

    /// `currentScale`: 0.01 under reduced motion, the user's scale otherwise.
    fn current_scale(&self) -> f32 {
        if self.reduced { 0.01 } else { self.scale }
    }

    /// `getDuration(token)`, in seconds.
    pub fn duration(&self, token: Token) -> f32 {
        if self.reduced {
            return 0.001;
        }
        (token.base_ms() * self.profile.factor(token) * self.current_scale()).round().max(1.0) / 1000.0
    }

    /// `calculateDuration`: 240..440 ms on a log scale of how far the box changes size, times
    /// the layout factor and the scale, in seconds.
    pub fn surface_duration(&self, target: (f32, f32), origin: (f32, f32)) -> f32 {
        if self.reduced {
            return 0.001;
        }
        if self.current_scale() <= 0.0 {
            return 0.0;
        }
        let delta = (target.0 - origin.0).abs().hypot((target.1 - origin.1).abs());
        let ratio = ((1.0 + delta / 100.0).ln() / (1.0 + 1600.0 / 100.0f32).ln()).clamp(0.0, 1.0);
        let t = 240.0 + 200.0 * ratio;
        (t * self.profile.factor(Token::Layout) * self.current_scale()).round().max(1.0) / 1000.0
    }

    pub fn tween(&self, seconds: f32, easing: Easing) -> Anim {
        if seconds <= 0.0 { Anim::INSTANT } else { Anim::new(Curve::tween(seconds, easing)) }
    }
}

/// Windows' client-area animation setting, which WebView2 reports as `prefers-reduced-motion`.
fn system_reduces_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut enabled = windows::Win32::Foundation::BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut enabled as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
    };
    ok.is_ok() && !enabled.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(profile: Profile, scale: f32) -> Motion {
        Motion { profile, scale, reduced: false }
    }

    #[test]
    fn durations_are_the_pages() {
        let balanced = motion(Profile::Balanced, 1.0);
        assert_eq!(balanced.duration(Token::Motion), 0.3);
        assert_eq!(motion(Profile::Responsive, 1.0).duration(Token::Layout), 0.255);
        assert_eq!(motion(Profile::Fluid, 2.0).duration(Token::Transient), 0.484);
        assert_eq!(Motion { reduced: true, ..balanced }.duration(Token::Layout), 0.001);
        assert_eq!(Profile::parse("snappy"), Profile::Responsive);
        assert_eq!(Profile::parse("whatever"), Profile::Balanced);
    }

    #[test]
    fn a_surface_takes_240_to_440_ms_by_how_much_it_grows() {
        let balanced = motion(Profile::Balanced, 1.0);
        // The worked example of motion.js: a 400 px change takes 354 ms.
        assert_eq!(balanced.surface_duration((400.0, 0.0), (0.0, 0.0)), 0.354);
        assert_eq!(balanced.surface_duration((10.0, 10.0), (10.0, 10.0)), 0.24);
        assert_eq!(balanced.surface_duration((4000.0, 4000.0), (0.0, 0.0)), 0.44);
        assert_eq!(motion(Profile::Balanced, 0.0).surface_duration((400.0, 0.0), (0.0, 0.0)), 0.0);
    }
}
