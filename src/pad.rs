//! The pad vocabulary and its gesture machine: tap, hold and chord resolution,
//! plus auto-repeat for the actions that want it.
//!
//! A pad whose tap is its only binding fires on the press edge. One that also
//! carries a hold or joins a chord is ambiguous there, so it is *deferred*: the
//! tap waits for release, the hold fires once the threshold passes, and a chord
//! fires the moment its second pad goes down. Each press resolves its own
//! actions up front and remembers them, so release and tick need no tables —
//! and a surface change mid-gesture cannot rewrite what a press meant.
//!
//! Time comes in through `now` — the machine reads no clock, so a loop pass is
//! one consistent instant and tests drive time directly.

use super::repeat::{Cadence, Repeater};
use super::{Action, Bindings, SurfaceId};
use std::time::{Duration, Instant};

/// The inputs a pad offers, whatever the device prints on them. Dense and
/// small on purpose: every runtime table indexes by `as usize`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Pad {
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    L3,
    R3,
    Start,
    Select,
    Up,
    Down,
    Left,
    Right,
}

/// Chord masks and the deferred set pack pads into a `u16`; a 17th pad means
/// widening them first.
const _: () = assert!(Pad::COUNT <= u16::BITS as usize);

impl Pad {
    pub const COUNT: usize = 16;

    pub const ALL: [Pad; Self::COUNT] = [
        Pad::A,
        Pad::B,
        Pad::X,
        Pad::Y,
        Pad::L1,
        Pad::R1,
        Pad::L2,
        Pad::R2,
        Pad::L3,
        Pad::R3,
        Pad::Start,
        Pad::Select,
        Pad::Up,
        Pad::Down,
        Pad::Left,
        Pad::Right,
    ];

    /// Spelling in the bindings file; [`Pad::parse`] is its inverse.
    pub fn name(self) -> &'static str {
        match self {
            Pad::A => "a",
            Pad::B => "b",
            Pad::X => "x",
            Pad::Y => "y",
            Pad::L1 => "l1",
            Pad::R1 => "r1",
            Pad::L2 => "l2",
            Pad::R2 => "r2",
            Pad::L3 => "l3",
            Pad::R3 => "r3",
            Pad::Start => "start",
            Pad::Select => "select",
            Pad::Up => "up",
            Pad::Down => "down",
            Pad::Left => "left",
            Pad::Right => "right",
        }
    }

    pub fn parse(name: &str) -> Option<Pad> {
        Pad::ALL.into_iter().find(|pad| pad.name() == name)
    }

    /// A direction, if this is one — the four that a stick can also stand in
    /// for.
    pub fn direction(self) -> bool {
        matches!(self, Pad::Up | Pad::Down | Pad::Left | Pad::Right)
    }

    pub(super) fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// One pad currently down, with the actions its press already resolved.
struct Held<A> {
    pad: Pad,
    at: Instant,
    /// The gesture is decided: the tap fired on press, a chord consumed the
    /// press, or the hold fired. Release is then a no-op.
    resolved: bool,
    tap: Option<A>,
    hold: Option<A>,
}

/// Gesture resolution and auto-repeat over the pads currently down.
pub struct PadState<A: Action> {
    held: Vec<Held<A>>,
    repeat: Repeater<Pad, A>,
    hold: Duration,
}

impl<A: Action> PadState<A> {
    pub fn new(hold: Duration, cadence: Cadence) -> Self {
        Self {
            // One slot per pad, so presses never reallocate.
            held: Vec::with_capacity(Pad::COUNT),
            repeat: Repeater::new(cadence),
            hold,
        }
    }

    /// Forget every pad down and any repeat — for a bindings reload, whose new
    /// tables would otherwise resolve a press that started under the old ones.
    pub fn reset(&mut self) {
        self.held.clear();
        self.repeat.clear();
    }

    pub fn on_press(
        &mut self,
        pad: Pad,
        now: Instant,
        bindings: &Bindings<A>,
        surface: Option<SurfaceId>,
        out: &mut Vec<A>,
    ) {
        // A chord resolves the moment its second pad goes down, consuming both
        // presses so neither fires its own tap on release.
        let chord = self
            .held
            .iter()
            .enumerate()
            .filter(|(_, h)| !h.resolved)
            .find_map(|(i, h)| bindings.chord(h.pad, pad).map(|action| (i, h.pad, action)));
        if let Some((i, first, action)) = chord {
            self.held[i].resolved = true;
            self.repeat.stop(first);
            self.held.push(Held {
                pad,
                at: now,
                resolved: true,
                tap: None,
                hold: None,
            });
            out.push(action);
            return;
        }

        // A press for a pad already down (a release we never saw) replaces it
        // rather than stacking, so `held_pads` cannot grow phantoms.
        self.held.retain(|h| h.pad != pad);
        let tap = bindings.tap(pad, surface);
        let hold = bindings.hold(pad);
        let immediate = !bindings.is_deferred(pad);
        self.held.push(Held {
            pad,
            at: now,
            resolved: immediate,
            tap,
            hold,
        });
        if !immediate {
            return;
        }
        if let Some(action) = tap {
            out.push(action);
            if action.repeats() {
                self.repeat.start(pad, action, now);
            }
        }
    }

    pub fn on_release(&mut self, pad: Pad, now: Instant, out: &mut Vec<A>) {
        self.repeat.stop(pad);
        let Some(i) = self.held.iter().position(|h| h.pad == pad) else {
            return; // held before this state existed, or already consumed
        };
        let held = self.held.remove(i);
        if held.resolved {
            return;
        }
        // Released before the threshold it is a tap; past it the hold fires
        // here as well, in case no tick ran in between.
        let action = if now.duration_since(held.at) >= self.hold {
            held.hold.or(held.tap)
        } else {
            held.tap
        };
        if let Some(action) = action {
            out.push(action);
        }
    }

    /// Fire any hold whose threshold just passed and any due repeat. Called
    /// once per loop pass; [`Self::next_deadline`] says when that must be.
    pub fn tick(&mut self, now: Instant, out: &mut Vec<A>) {
        for held in &mut self.held {
            if held.resolved || now.duration_since(held.at) < self.hold {
                continue;
            }
            if let Some(action) = held.hold {
                out.push(action);
                held.resolved = true;
            }
        }
        self.repeat.tick(now, out);
    }

    /// How long the event loop may block: the earliest pending hold threshold
    /// or repeat. `None` when neither is pending, so a still screen blocks
    /// indefinitely and costs nothing.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        let hold = self
            .held
            .iter()
            .filter(|h| !h.resolved && h.hold.is_some())
            .map(|h| (h.at + self.hold).saturating_duration_since(now))
            .min();
        let repeat = self.repeat.next_deadline(now);
        match (hold, repeat) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The pads down right now. Allocates, but only capture asks, and only when
    /// it opens.
    pub fn held_pads(&self) -> Vec<Pad> {
        self.held.iter().map(|h| h.pad).collect()
    }
}

/// Hysteresis: an engaged input releases only below this fraction of its
/// engage threshold, so a value wobbling around the threshold cannot chatter.
const RELEASE_RATIO: f32 = 0.6;

/// An analog stick folded into pad directions, with hysteresis so a wobbling
/// stick does not chatter.
pub struct Stick {
    x: f32,
    y: f32,
    engaged: Option<Pad>,
    deadzone: f32,
}

impl Stick {
    pub fn new(deadzone: f32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            engaged: None,
            deadzone,
        }
    }

    /// Feed one axis (`-1.0..=1.0`, y positive downward as SDL reports it).
    /// Returns the direction change as (released, pressed), either of which the
    /// caller feeds to [`PadState`] as an ordinary press or release.
    pub fn axis(&mut self, horizontal: bool, value: f32) -> (Option<Pad>, Option<Pad>) {
        if horizontal {
            self.x = value;
        } else {
            self.y = value;
        }
        let engage = self.deadzone;
        let release = engage * RELEASE_RATIO;
        let magnitude = self.x.abs().max(self.y.abs());
        let next = if magnitude >= engage {
            Some(if self.x.abs() > self.y.abs() {
                if self.x > 0.0 {
                    Pad::Right
                } else {
                    Pad::Left
                }
            } else if self.y > 0.0 {
                Pad::Down
            } else {
                Pad::Up
            })
        } else if magnitude <= release {
            None
        } else {
            self.engaged // inside the hysteresis band: hold the current state
        };
        if next == self.engaged {
            return (None, None);
        }
        let was = self.engaged;
        self.engaged = next;
        (was, next)
    }
}

/// An analog trigger folded into one pad's presses, with the same hysteresis
/// as [`Stick`].
pub struct Trigger {
    pad: Pad,
    threshold: f32,
    engaged: bool,
}

impl Trigger {
    pub fn new(pad: Pad, threshold: f32) -> Self {
        Self {
            pad,
            threshold,
            engaged: false,
        }
    }

    /// Feed the trigger's axis (`0.0..=1.0`). Returns the edge as
    /// (released, pressed), shaped like [`Stick::axis`] so the caller treats
    /// both the same.
    pub fn axis(&mut self, value: f32) -> (Option<Pad>, Option<Pad>) {
        let engaged = if value >= self.threshold {
            true
        } else if value <= self.threshold * RELEASE_RATIO {
            false
        } else {
            self.engaged // inside the hysteresis band: hold the current state
        };
        if engaged == self.engaged {
            return (None, None);
        }
        self.engaged = engaged;
        if engaged {
            (None, Some(self.pad))
        } else {
            (Some(self.pad), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{at, build, TestAction};
    use super::*;

    const HOLD: Duration = Duration::from_millis(400);
    const DELAY: Duration = Duration::from_millis(300);
    const INTERVAL: Duration = Duration::from_millis(100);

    fn state() -> PadState<TestAction> {
        PadState::new(
            HOLD,
            Cadence {
                initial_delay: DELAY,
                interval: INTERVAL,
            },
        )
    }

    #[test]
    fn a_hold_fires_at_its_threshold_and_release_is_then_a_no_op() {
        let b = build("[gamepad]\ny = \"confirm\"\n\"hold:y\" = \"theme_next\"\n");
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(Pad::Y, t0, &b, None, &mut out);
        assert!(out.is_empty());
        s.tick(at(t0, 399), &mut out);
        assert!(out.is_empty());
        s.tick(at(t0, 400), &mut out);
        assert_eq!(out, [TestAction::ThemeNext]);
        out.clear();
        s.on_release(Pad::Y, at(t0, 500), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_deferred_tap_fires_on_release_and_a_missed_tick_still_fires_the_hold() {
        let b = build("[gamepad]\ny = \"confirm\"\n\"hold:y\" = \"theme_next\"\n");
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(Pad::Y, t0, &b, None, &mut out);
        s.on_release(Pad::Y, at(t0, 100), &mut out);
        assert_eq!(out, [TestAction::Confirm]);

        // Past the threshold with no tick in between, release fires the hold.
        out.clear();
        s.on_press(Pad::Y, at(t0, 1000), &b, None, &mut out);
        s.on_release(Pad::Y, at(t0, 1400), &mut out);
        assert_eq!(out, [TestAction::ThemeNext]);
    }

    #[test]
    fn a_repeating_tap_fires_on_press_then_on_cadence_until_release() {
        let b = build("[gamepad]\ndown = \"nav_down\"\n");
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(Pad::Down, t0, &b, None, &mut out);
        assert_eq!(out, [TestAction::NavDown]);
        out.clear();
        s.tick(at(t0, 299), &mut out);
        assert!(out.is_empty());
        s.tick(at(t0, 300), &mut out);
        assert_eq!(out, [TestAction::NavDown]);
        out.clear();
        s.tick(at(t0, 350), &mut out);
        assert!(out.is_empty());
        s.tick(at(t0, 400), &mut out);
        assert_eq!(out, [TestAction::NavDown]);
        out.clear();
        s.on_release(Pad::Down, at(t0, 450), &mut out);
        s.tick(at(t0, 600), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_chord_fires_on_the_second_press_and_consumes_both() {
        let b = build("[gamepad]\nstart = \"confirm\"\n\"start+select\" = \"theme_next\"\n");
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        s.on_press(Pad::Start, t0, &b, None, &mut out);
        assert!(out.is_empty());
        s.on_press(Pad::Select, at(t0, 50), &b, None, &mut out);
        assert_eq!(out, [TestAction::ThemeNext]);
        // Neither release fires the tap the chord consumed.
        out.clear();
        s.on_release(Pad::Select, at(t0, 200), &mut out);
        s.on_release(Pad::Start, at(t0, 220), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn next_deadline_is_the_earliest_pending_hold_or_repeat() {
        let b = build("[gamepad]\n\"hold:y\" = \"theme_next\"\ndown = \"nav_down\"\n");
        let mut s = state();
        let mut out = Vec::new();
        let t0 = Instant::now();
        assert_eq!(s.next_deadline(t0), None);
        // Y's hold is due at 400.
        s.on_press(Pad::Y, t0, &b, None, &mut out);
        assert_eq!(
            s.next_deadline(at(t0, 100)),
            Some(Duration::from_millis(300))
        );
        // Down's first repeat is due at 550; the hold still comes first.
        s.on_press(Pad::Down, at(t0, 250), &b, None, &mut out);
        assert_eq!(
            s.next_deadline(at(t0, 300)),
            Some(Duration::from_millis(100))
        );
        // Once the hold fires, the repeat is what remains.
        s.tick(at(t0, 400), &mut out);
        assert_eq!(
            s.next_deadline(at(t0, 400)),
            Some(Duration::from_millis(150))
        );
    }

    #[test]
    fn every_pad_round_trips_through_its_name() {
        for pad in Pad::ALL {
            assert_eq!(Pad::parse(pad.name()), Some(pad));
        }
        assert_eq!(Pad::parse("elbow"), None);
    }

    #[test]
    fn pad_indices_stay_inside_the_table_width() {
        for (i, pad) in Pad::ALL.into_iter().enumerate() {
            assert_eq!(pad as usize, i);
        }
        assert!(Pad::ALL.len() == Pad::COUNT);
    }

    #[test]
    fn a_stick_engages_past_the_deadzone_and_holds_through_the_band() {
        let mut stick = Stick::new(0.5);
        assert_eq!(stick.axis(false, 0.9), (None, Some(Pad::Down)));
        // Inside the band it neither re-fires nor releases.
        assert_eq!(stick.axis(false, 0.4), (None, None));
        assert_eq!(stick.axis(false, 0.1), (Some(Pad::Down), None));
    }

    #[test]
    fn a_trigger_engages_at_its_threshold_and_holds_through_the_band() {
        let mut trigger = Trigger::new(Pad::L2, 0.5);
        assert_eq!(trigger.axis(0.6), (None, Some(Pad::L2)));
        // Inside the band it neither re-fires nor releases.
        assert_eq!(trigger.axis(0.4), (None, None));
        assert_eq!(trigger.axis(0.2), (Some(Pad::L2), None));
    }
}
