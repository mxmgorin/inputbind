//! SDL controller events → pad inputs. Gesture resolution and repeat belong to
//! [`PadState`]; this only translates and folds the analog axes into presses.

use super::axis_value;
use crate::{Action, Bindings, Cadence, Edge, Pad, PadState, Stick, SurfaceId, Trigger};
use sdl2::controller::Axis;
use std::time::{Duration, Instant};

pub struct Gamepad<A: Action> {
    state: PadState<A>,
    stick: Stick,
    left_trigger: Trigger,
    right_trigger: Trigger,
}

impl<A: Action> Gamepad<A> {
    pub fn new(hold: Duration, cadence: Cadence, deadzone: f32, trigger_threshold: f32) -> Self {
        Self {
            state: PadState::new(hold, cadence),
            stick: Stick::new(deadzone),
            left_trigger: Trigger::new(Pad::L2, trigger_threshold),
            right_trigger: Trigger::new(Pad::R2, trigger_threshold),
        }
    }

    /// Drop pads down and any repeat, so a reloaded table never resolves a press
    /// that began under the old one. Owed release edges are emitted, not lost.
    pub fn reset(&mut self, out: &mut Vec<(A, Edge)>) {
        self.state.reset(out);
    }

    pub fn press(
        &mut self,
        pad: Pad,
        pressed: bool,
        now: Instant,
        bindings: &Bindings<A>,
        surface: Option<SurfaceId>,
        out: &mut Vec<(A, Edge)>,
    ) {
        if pressed {
            self.state.on_press(pad, now, bindings, surface, out);
        } else {
            self.state.on_release(pad, now, out);
        }
    }

    pub fn on_axis(
        &mut self,
        axis: Axis,
        value: i16,
        now: Instant,
        bindings: &Bindings<A>,
        surface: Option<SurfaceId>,
        out: &mut Vec<(A, Edge)>,
    ) {
        let edges = match axis {
            Axis::LeftX => self.stick.axis(true, axis_value(value)),
            Axis::LeftY => self.stick.axis(false, axis_value(value)),
            Axis::TriggerLeft | Axis::TriggerRight => self.trigger_edges(axis, value),
            _ => return,
        };
        // Release before press, so a direction change ends the old hold first.
        let (released, pressed) = edges;
        if let Some(pad) = released {
            self.press(pad, false, now, bindings, surface, out);
        }
        if let Some(pad) = pressed {
            self.press(pad, true, now, bindings, surface, out);
        }
    }

    pub fn tick(&mut self, now: Instant, out: &mut Vec<(A, Edge)>) {
        self.state.tick(now, out);
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.state.next_deadline(now)
    }

    /// The pads down right now, for capture to know which press activated it.
    pub fn held(&self) -> Vec<Pad> {
        self.state.held_pads()
    }

    /// Fold a trigger axis through its hysteresis and return the edge, shaped
    /// like [`Stick::axis`]; `(None, None)` for any other axis. Capture also
    /// feeds triggers here — the only bindable axis gesture — so their engaged
    /// state lives in one place across that boundary.
    pub fn trigger_edges(&mut self, axis: Axis, value: i16) -> (Option<Pad>, Option<Pad>) {
        let value = axis_value(value);
        match axis {
            Axis::TriggerLeft => self.left_trigger.axis(value),
            Axis::TriggerRight => self.right_trigger.axis(value),
            _ => (None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{actions, at, build, TestAction};

    const HOLD: Duration = Duration::from_millis(400);
    const CADENCE: Cadence = Cadence {
        initial_delay: Duration::from_millis(300),
        interval: Duration::from_millis(100),
    };

    fn gamepad() -> Gamepad<TestAction> {
        Gamepad::new(HOLD, CADENCE, 0.5, 0.5)
    }

    #[test]
    fn a_stick_push_acts_as_its_direction() {
        let b = build("[gamepad]\ndown = \"nav_down\"\n");
        let mut g = gamepad();
        let mut out = Vec::new();
        let t0 = Instant::now();
        g.on_axis(Axis::LeftY, i16::MAX, t0, &b, None, &mut out);
        assert_eq!(actions(&out), [TestAction::NavDown]);
        // Back to center: the release stops the repeat, emitting nothing.
        out.clear();
        g.on_axis(Axis::LeftY, 0, at(t0, 50), &b, None, &mut out);
        g.tick(at(t0, 400), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn trigger_edges_fold_through_the_hysteresis() {
        let mut g = gamepad();
        let full = i16::MAX;
        // Engaged at the threshold, held through the band, released below it.
        assert_eq!(
            g.trigger_edges(Axis::TriggerLeft, full),
            (None, Some(Pad::L2))
        );
        assert_eq!(
            g.trigger_edges(Axis::TriggerLeft, full / 2 - 1),
            (None, None)
        );
        assert_eq!(
            g.trigger_edges(Axis::TriggerLeft, full / 5),
            (Some(Pad::L2), None)
        );
        // A stick axis is not a bindable gesture here.
        assert_eq!(g.trigger_edges(Axis::LeftX, full), (None, None));
    }
}
