//! Keyboard input: a key resolves straight to a command, and repeats on the same
//! cadence the pad uses. No gestures here — a key has no hold or chord, so a
//! press emits at once and a release only stops the repeat.

use super::repeat::{Cadence, Repeater};
use super::{Action, Edge};
use std::time::{Duration, Instant};

pub struct KeyState<A: Action> {
    repeat: Repeater<u32, A>,
    /// A held action whose press edge fired here; this key owes its close.
    owed: Option<(u32, A)>,
}

impl<A: Action> KeyState<A> {
    pub fn new(cadence: Cadence) -> Self {
        Self {
            repeat: Repeater::new(cadence),
            owed: None,
        }
    }

    /// Forget the held key, so a reloaded table cannot keep repeating an action
    /// the key no longer carries. Owed releases are emitted, not dropped.
    pub fn reset(&mut self, out: &mut Vec<(A, Edge)>) {
        if let Some((_, action)) = self.owed.take() {
            out.push((action, Edge::Release));
        }
        self.repeat.clear();
    }

    pub fn on_press(&mut self, code: u32, action: A, now: Instant, out: &mut Vec<(A, Edge)>) {
        out.push((action, Edge::Press));
        if action.repeats() {
            self.repeat.start(code, action, now);
        }
        if action.is_held() {
            // A second key taking over closes the first.
            if let Some((_, previous)) = self.owed.replace((code, action)) {
                out.push((previous, Edge::Release));
            }
        }
    }

    pub fn on_release(&mut self, code: u32, out: &mut Vec<(A, Edge)>) {
        self.repeat.stop(code);
        if self.owed.is_some_and(|(held, _)| held == code) {
            let (_, action) = self.owed.take().expect("just checked");
            out.push((action, Edge::Release));
        }
    }

    pub fn tick(&mut self, now: Instant, out: &mut Vec<(A, Edge)>) {
        self.repeat.tick(now, out);
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.repeat.next_deadline(now)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{actions, at, TestAction};
    use super::*;

    fn state() -> KeyState<TestAction> {
        KeyState::new(Cadence {
            initial_delay: Duration::from_millis(300),
            interval: Duration::from_millis(100),
        })
    }

    const CODE: u32 = 42;

    #[test]
    fn a_repeating_action_fires_on_press_then_on_cadence() {
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(CODE, TestAction::NavDown, t0, &mut out);
        assert_eq!(actions(&out), [TestAction::NavDown]);
        out.clear();
        s.tick(at(t0, 299), &mut out);
        assert!(out.is_empty());
        s.tick(at(t0, 300), &mut out);
        assert_eq!(actions(&out), [TestAction::NavDown]);
        out.clear();
        s.tick(at(t0, 400), &mut out);
        assert_eq!(actions(&out), [TestAction::NavDown]);
        out.clear();
        s.on_release(CODE, &mut out);
        s.tick(at(t0, 600), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_one_shot_action_never_repeats_and_wants_no_wakeup() {
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(CODE, TestAction::Confirm, t0, &mut out);
        assert_eq!(actions(&out), [TestAction::Confirm]);
        out.clear();
        assert_eq!(s.next_deadline(t0), None);
        s.tick(at(t0, 1000), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn the_last_key_pressed_takes_over_the_repeat() {
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(CODE, TestAction::NavDown, t0, &mut out);
        s.on_press(CODE + 1, TestAction::PageNext, at(t0, 50), &mut out);
        out.clear();
        s.tick(at(t0, 350), &mut out);
        assert_eq!(actions(&out), [TestAction::PageNext]);
        // Releasing the older key leaves the newer one repeating.
        out.clear();
        s.on_release(CODE, &mut out);
        s.tick(at(t0, 460), &mut out);
        assert_eq!(actions(&out), [TestAction::PageNext]);
    }
}
