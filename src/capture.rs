//! Binding capture: the editor listening for the gesture to bind.
//!
//! Capture cannot consult the tables — any gesture is possible — so every pad
//! waits: a release before the threshold is a tap, a second pad down is a
//! chord, and passing the threshold is a hold. It arms only once the pads held
//! when it opened have released, so the press that started it is not itself
//! captured, and it gives up after an idle spell: a handheld has no Esc, and an
//! accidental capture must not trap input.
//!
//! Time comes in through `now` — the type reads no clock, so a loop pass is one
//! consistent instant and tests drive time directly.

use super::gesture::{KeyGesture, PadGesture};
use super::pad::Pad;
use super::Mods;
use std::time::{Duration, Instant};

/// The gesture capture resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Captured {
    Pad(PadGesture),
    Key(KeyGesture),
}

pub struct Capture {
    on: bool,
    /// Pads held when capture opened have all released, so input counts now.
    armed: bool,
    /// A gesture was already reported; ignore input until capture is closed.
    reported: bool,
    down: Vec<(Pad, Instant)>,
    /// A modifier key down and not yet claimed: alone it is the binding,
    /// resolved on its release; a second key in between takes it as its
    /// modifier instead.
    pending_mod: Option<KeyGesture>,
    armed_at: Option<Instant>,
    /// When capture opened, so the idle give-up has a clock even unarmed.
    opened_at: Option<Instant>,
    hold: Duration,
    timeout: Duration,
}

impl Capture {
    pub fn new(hold: Duration, timeout: Duration) -> Self {
        Self {
            on: false,
            armed: false,
            reported: false,
            // One slot per pad, so presses never reallocate.
            down: Vec::with_capacity(Pad::COUNT),
            pending_mod: None,
            armed_at: None,
            opened_at: None,
            hold,
            timeout,
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Whether capture has started listening, for the "press an input" hint.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Open or close capture. `held` is the pads down right now — the press that
    /// activated the row, which capture must not take as the binding.
    pub fn set(&mut self, on: bool, held: &[Pad], now: Instant) {
        if on == self.on {
            return;
        }
        self.on = on;
        self.reported = false;
        self.pending_mod = None;
        self.armed = on && held.is_empty();
        self.armed_at = self.armed.then_some(now);
        self.opened_at = on.then_some(now);
        self.down.clear();
        if on {
            self.down.extend(held.iter().map(|pad| (*pad, now)));
        }
    }

    pub fn on_press(&mut self, pad: Pad, now: Instant) -> Option<Captured> {
        if !self.on || self.reported {
            return None;
        }
        if !self.armed {
            // Still waiting for the activating press to clear.
            if !self.down.iter().any(|(p, _)| *p == pad) {
                self.down.push((pad, now));
            }
            return None;
        }
        // A second pad down while one is held is a chord.
        if let Some((first, _)) = self.down.first().copied() {
            if first != pad {
                self.reported = true;
                return Some(Captured::Pad(PadGesture::chord(first, pad)));
            }
        }
        self.down.push((pad, now));
        None
    }

    pub fn on_release(&mut self, pad: Pad, now: Instant) -> Option<Captured> {
        if !self.on {
            return None;
        }
        let at = self
            .down
            .iter()
            .position(|(p, _)| *p == pad)
            .map(|i| self.down.remove(i).1);
        if !self.armed {
            // Arm once every pad from the activating press has let go.
            if self.down.is_empty() {
                self.armed = true;
                self.armed_at = Some(now);
            }
            return None;
        }
        if self.reported {
            return None;
        }
        let at = at?;
        // Past the threshold the hold already fired on tick; this is the tap.
        if now.duration_since(at) < self.hold {
            self.reported = true;
            return Some(Captured::Pad(PadGesture::Tap(pad)));
        }
        None
    }

    /// A key press, already resolved to its normalized name by the backend. A
    /// pure modifier is deferred like a pad tap: alone it is the binding, and
    /// only its release can tell — a second key in between takes it as its
    /// modifier instead.
    pub fn on_key(
        &mut self,
        name: &str,
        mods: Mods,
        is_modifier: bool,
        now: Instant,
    ) -> Option<Captured> {
        if !self.on || !self.armed || self.reported {
            return None;
        }
        let gesture = KeyGesture {
            name: name.to_string(),
            mods,
        };
        if is_modifier {
            self.pending_mod = Some(gesture);
            // A combo under way restarts the idle clock, so it is not given
            // up on mid-gesture.
            self.armed_at = Some(now);
            return None;
        }
        self.reported = true;
        Some(Captured::Key(gesture))
    }

    /// A key release: a modifier let go with nothing captured in between is
    /// itself the gesture. Any other release means nothing here.
    pub fn on_key_release(&mut self, name: &str) -> Option<Captured> {
        if !self.on || !self.armed || self.reported {
            return None;
        }
        if self.pending_mod.as_ref().map(|g| g.name.as_str()) != Some(name) {
            return None;
        }
        self.reported = true;
        self.pending_mod.take().map(Captured::Key)
    }

    /// Fire a hold whose threshold passed, or report that capture gave up.
    pub fn tick(&mut self, now: Instant) -> Tick {
        if !self.on || self.reported {
            return Tick::Waiting;
        }
        if self.armed {
            if let Some((pad, at)) = self.down.first().copied() {
                if now.duration_since(at) >= self.hold {
                    self.reported = true;
                    return Tick::Got(Captured::Pad(PadGesture::Hold(pad)));
                }
                // A gesture is still being made; nothing to give up on.
                return Tick::Waiting;
            }
        }
        // Give up whether armed or not. An unarmed capture is waiting on a
        // release that may never arrive, and since capture takes every input
        // while it is open, this is the only way out of that.
        if self
            .idle_since()
            .is_some_and(|t| now.duration_since(t) >= self.timeout)
        {
            return Tick::GaveUp;
        }
        Tick::Waiting
    }

    /// When the loop must wake: the pending hold threshold, else the idle
    /// timeout. `None` only when capture is closed or done — an unarmed capture
    /// still needs its wake-up, or it could never give up.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        if !self.on || self.reported {
            return None;
        }
        if self.armed {
            if let Some((_, at)) = self.down.first() {
                return Some((*at + self.hold).saturating_duration_since(now));
            }
        }
        let since = self.idle_since()?;
        Some((since + self.timeout).saturating_duration_since(now))
    }

    /// When the idle clock started: arming, or opening while still unarmed.
    fn idle_since(&self) -> Option<Instant> {
        self.armed_at.or(self.opened_at)
    }
}

/// What a capture tick produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Tick {
    Waiting,
    Got(Captured),
    /// Idle too long — close capture and change nothing.
    GaveUp,
}

#[cfg(test)]
mod tests {
    use super::super::testkit::at;
    use super::*;

    const HOLD: Duration = Duration::from_millis(400);
    const TIMEOUT: Duration = Duration::from_secs(6);

    fn capture() -> Capture {
        Capture::new(HOLD, TIMEOUT)
    }

    #[test]
    fn the_press_that_opened_capture_is_not_captured() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[Pad::A], t0);
        assert!(!c.is_armed());
        // A releases: that arms capture rather than binding A.
        assert_eq!(c.on_release(Pad::A, at(t0, 100)), None);
        assert!(c.is_armed());
        // Now a press is a candidate: nothing on the press edge, the tap on
        // release.
        assert_eq!(c.on_press(Pad::Y, at(t0, 200)), None);
        assert_eq!(
            c.on_release(Pad::Y, at(t0, 300)),
            Some(Captured::Pad(PadGesture::Tap(Pad::Y)))
        );
    }

    #[test]
    fn a_second_pad_down_captures_a_chord() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        assert_eq!(c.on_press(Pad::L1, t0), None);
        assert_eq!(
            c.on_press(Pad::R1, at(t0, 50)),
            Some(Captured::Pad(PadGesture::chord(Pad::L1, Pad::R1)))
        );
    }

    #[test]
    fn only_one_gesture_is_reported_per_capture() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        c.on_press(Pad::Y, t0);
        assert!(c.on_release(Pad::Y, at(t0, 100)).is_some());
        // Anything after the first gesture is ignored until capture reopens.
        assert_eq!(c.on_press(Pad::A, at(t0, 200)), None);
        assert_eq!(c.on_key("f", Mods::NONE, false, at(t0, 300)), None);
    }

    #[test]
    fn a_key_press_captures_its_gesture() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        assert_eq!(
            c.on_key("pagedown", Mods::CTRL, false, t0),
            Some(Captured::Key(KeyGesture {
                name: "pagedown".to_string(),
                mods: Mods::CTRL,
            }))
        );
    }

    #[test]
    fn a_modifier_alone_captures_on_its_release() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        // Nothing on the press edge: a second key may still make this a combo.
        assert_eq!(c.on_key("leftctrl", Mods::NONE, true, t0), None);
        assert_eq!(
            c.on_key_release("leftctrl"),
            Some(Captured::Key(KeyGesture {
                name: "leftctrl".to_string(),
                mods: Mods::NONE,
            }))
        );
    }

    #[test]
    fn a_key_after_a_modifier_captures_the_combo_not_the_modifier() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        assert_eq!(c.on_key("leftctrl", Mods::NONE, true, t0), None);
        // The backend folds the held modifier into `mods`, so the combo
        // arrives whole on the second key's press edge.
        assert_eq!(
            c.on_key("r", Mods::CTRL, false, at(t0, 100)),
            Some(Captured::Key(KeyGesture {
                name: "r".to_string(),
                mods: Mods::CTRL,
            }))
        );
        // The modifier's own release is no longer a gesture.
        assert_eq!(c.on_key_release("leftctrl"), None);
    }

    #[test]
    fn a_second_modifier_becomes_the_key_and_the_first_its_modifier() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        assert_eq!(c.on_key("leftctrl", Mods::NONE, true, t0), None);
        assert_eq!(c.on_key("leftshift", Mods::CTRL, true, at(t0, 50)), None);
        // Releasing the replaced modifier resolves nothing; the pending one
        // is the gesture, with the first folded in as its modifier.
        assert_eq!(c.on_key_release("leftctrl"), None);
        assert_eq!(
            c.on_key_release("leftshift"),
            Some(Captured::Key(KeyGesture {
                name: "leftshift".to_string(),
                mods: Mods::CTRL,
            }))
        );
    }

    #[test]
    fn a_pending_modifier_restarts_the_idle_clock() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        // Pressed just before the give-up, the combo gets a fresh window
        // rather than being expired mid-gesture.
        assert_eq!(c.on_key("leftctrl", Mods::NONE, true, at(t0, 5_900)), None);
        assert_eq!(c.tick(at(t0, 6_000)), Tick::Waiting);
        assert_eq!(c.tick(at(t0, 11_900)), Tick::GaveUp);
    }

    #[test]
    fn a_closed_capture_reports_nothing_and_wants_no_wakeup() {
        let mut c = capture();
        let t0 = Instant::now();
        assert_eq!(c.on_press(Pad::A, t0), None);
        assert_eq!(c.next_deadline(t0), None);
        assert_eq!(c.tick(t0), Tick::Waiting);
    }

    #[test]
    fn the_activating_press_is_never_a_hold() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[Pad::A], t0);
        // Held far past the threshold it still reports nothing: this is the
        // press that opened capture, not a gesture.
        assert_eq!(c.tick(at(t0, 1000)), Tick::Waiting);
        // The only deadline while unarmed is the give-up, so the loop waits
        // seconds rather than spinning on a threshold that cannot fire.
        assert_eq!(
            c.next_deadline(at(t0, 1000)),
            Some(TIMEOUT - Duration::from_millis(1000))
        );
        // Arming restarts the idle clock.
        c.on_release(Pad::A, at(t0, 1000));
        assert_eq!(c.next_deadline(at(t0, 1000)), Some(TIMEOUT));
    }

    #[test]
    fn a_pad_held_past_the_threshold_captures_a_hold() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        assert_eq!(c.on_press(Pad::Y, t0), None);
        assert_eq!(c.next_deadline(t0), Some(HOLD));
        assert_eq!(c.tick(at(t0, 399)), Tick::Waiting);
        assert_eq!(
            c.tick(at(t0, 400)),
            Tick::Got(Captured::Pad(PadGesture::Hold(Pad::Y)))
        );
        // The release after is not also a tap.
        assert_eq!(c.on_release(Pad::Y, at(t0, 450)), None);
    }

    #[test]
    fn capture_gives_up_after_the_idle_timeout() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        assert_eq!(c.tick(at(t0, 5_999)), Tick::Waiting);
        assert_eq!(c.tick(at(t0, 6_000)), Tick::GaveUp);
    }

    #[test]
    fn an_unarmed_capture_still_gives_up() {
        // The release capture is waiting for may never come — the pad machine
        // can lose track of it — and capture swallows every input while open, so
        // without this the app would be stuck for good.
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[Pad::A], t0);
        assert!(!c.is_armed());
        assert_eq!(c.tick(at(t0, 5_999)), Tick::Waiting);
        // And it must ask to be woken, or the give-up could never run.
        assert!(c.next_deadline(at(t0, 100)).is_some());
        assert_eq!(c.tick(at(t0, 6_000)), Tick::GaveUp);
    }

    #[test]
    fn a_gesture_in_progress_is_not_given_up_on() {
        let mut c = capture();
        let t0 = Instant::now();
        c.set(true, &[], t0);
        c.on_press(Pad::Y, at(t0, 10));
        // Held well past the idle timeout: the hold fires, it does not expire.
        assert_eq!(
            c.tick(at(t0, 7_000)),
            Tick::Got(Captured::Pad(PadGesture::Hold(Pad::Y)))
        );
    }
}
