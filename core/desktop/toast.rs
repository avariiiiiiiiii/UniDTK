//! The one toast of the desk (`core/ui/toast.js`, `.toast` in `styles.css`): one at a time, a
//! new one replaces the old, 5 s on screen (an error at least 7 s), an optional action such as
//! "Отменить" (`r[desk.no-blocking-dialogs]`). It rises 8 px into place on the transient
//! token. The host's poll ends it, so a toast costs one wake, not a timer per frame.

use std::time::{Duration, Instant};

use rune::el::{col, row, text, El};
use rune::{Color, Tx};

use crate::controls::Look;
use crate::motion::{Token, EXPO_OUT, SURFACE_EXIT};

/// `UNDO_MS`, and the floor of an error toast.
const LIFETIME: Duration = Duration::from_millis(5000);
const ERROR_LIFETIME: Duration = Duration::from_millis(7000);

pub type Action<S> = Box<dyn Fn(&mut S, &mut Tx)>;

pub struct Toast<S> {
    pub message: String,
    pub error: bool,
    pub action: Option<(String, Action<S>)>,
    pub until: Instant,
    /// Counts toasts, so a replaced one leaves as a new element enters.
    pub serial: u64,
}

pub struct Toasts<S> {
    pub current: Option<Toast<S>>,
    serial: u64,
}

impl<S> Default for Toasts<S> {
    fn default() -> Self {
        Toasts { current: None, serial: 0 }
    }
}

impl<S: 'static> Toasts<S> {
    /// `showToast`: replaces whatever toast is up.
    pub fn show(&mut self, message: impl Into<String>, error: bool, action: Option<(String, Action<S>)>) {
        self.serial += 1;
        let lifetime = if error { ERROR_LIFETIME } else { LIFETIME };
        self.current = Some(Toast { message: message.into(), error, action, until: Instant::now() + lifetime, serial: self.serial });
    }

    /// `showUndo`: the action is "Отменить".
    pub fn undo(&mut self, message: impl Into<String>, undo: Action<S>) {
        self.show(message, false, Some(("Отменить".into(), undo)));
    }

    /// Ends an expired toast; returns when the next one is due, for the poll's wake.
    pub fn expire(&mut self, now: Instant) -> Option<Instant> {
        if self.current.as_ref().is_some_and(|toast| toast.until <= now) {
            self.current = None;
        }
        self.current.as_ref().map(|toast| toast.until)
    }

    /// The action runs once and the toast leaves (`toast.js` disables the button and dismisses).
    pub fn take_action(&mut self) -> Option<Action<S>> {
        self.current.take().and_then(|toast| toast.action.map(|(_, action)| action))
    }
}

/// `.toast-host` at the bottom centre, 84 px up, and the toast in it. The host is always
/// there, so a toast that ends leaves on its transition (8 px down and out, `toast.js`'s
/// dismiss) instead of vanishing with it.
pub fn view<S: 'static>(look: &Look, toasts: &Toasts<S>, canvas: (f32, f32), on_action: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let host = col().key("toast-host").absolute(0.0, 0.0).w(canvas.0).h(canvas.1 - 84.0).items_center().justify_end();
    let Some(toast) = toasts.current.as_ref() else { return host };
    let rise = look.motion.tween(look.motion.duration(Token::Transient), EXPO_OUT);
    let width = (canvas.0 - 32.0).min(520.0);
    let mut body = row()
        .key(format!("hit:toast:{}", toast.serial))
        .max_w(width)
        .gap(12.0)
        .pl(16.0).pr(8.0).py(8.0)
        .radius(14.0)
        .fill(look.surface.alpha(0.97))
        .border(1.0, if toast.error { look.status_error } else { look.elev.alpha(0.14) }).border_layout()
        .shadow(12.0, 36.0, Color::BLACK.alpha(0.35))
        .items_center()
        // In on `expoOut`, out on `surfaceExit`, both over the transient token (`toast.js`).
        .transition(look.motion.presence(rune::Transition::offset(0.0, 8.0).combined(rune::Transition::opacity()), Token::Transient, EXPO_OUT, Some(SURFACE_EXIT)))
        .offset_motion(rise)
        .on_press(|_: &mut S| {})
        .child(col().py(6.0).child(text(toast.message.clone()).font(look.font(13.0, 400.0)).color(look.fg).wrap()));
    if let Some((label, _)) = &toast.action {
        body = body.child(
            col().key(format!("hit:toast-action:{}", toast.serial)).min_h(32.0).px(12.0).py(4.0).radius(9.0)
                .fill(look.elev.alpha(0.14)).items_center().justify_center()
                .child(text(label.clone()).font(look.font(13.0, 600.0)).color(look.fg).single_line())
                .on_press_tx(on_action),
        );
    }
    host.child(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_toast_at_a_time_and_errors_stay_longer() {
        let mut toasts: Toasts<u32> = Toasts::default();
        let now = Instant::now();
        toasts.show("a", false, None);
        toasts.undo("b", Box::new(|n: &mut u32, _| *n += 1));
        assert_eq!(toasts.current.as_ref().unwrap().message, "b");
        let undo_until = toasts.current.as_ref().unwrap().until;
        assert!(undo_until >= now + LIFETIME && undo_until < now + ERROR_LIFETIME);
        toasts.show("broke", true, None);
        assert!(toasts.current.as_ref().unwrap().until >= now + ERROR_LIFETIME);
        assert!(toasts.expire(now).is_some());
        assert!(toasts.expire(now + ERROR_LIFETIME + Duration::from_secs(1)).is_none());
        assert!(toasts.current.is_none());
    }

    #[test]
    fn the_action_runs_once_and_ends_the_toast() {
        let mut toasts: Toasts<u32> = Toasts::default();
        toasts.undo("Раскладка сброшена", Box::new(|n: &mut u32, _| *n += 1));
        assert!(toasts.take_action().is_some());
        assert!(toasts.current.is_none() && toasts.take_action().is_none());
    }
}
