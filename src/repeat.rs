//! Auto-repeat, shared by the pad and the keyboard: one input repeats at a time,
//! the last one pressed winning. Time comes in through `now`, as everywhere else
//! here, so a loop pass is one instant and tests drive it directly.

use std::time::{Duration, Instant};

/// How a held input repeats.
#[derive(Copy, Clone)]
pub struct Cadence {
    pub initial_delay: Duration,
    pub interval: Duration,
}

struct Held<K, A> {
    input: K,
    action: A,
    pressed_at: Instant,
    last: Option<Instant>,
}

pub(super) struct Repeater<K, A> {
    held: Option<Held<K, A>>,
    cadence: Cadence,
}

impl<K: Copy + PartialEq, A: Copy> Repeater<K, A> {
    pub(super) fn new(cadence: Cadence) -> Self {
        Self {
            held: None,
            cadence,
        }
    }

    /// Take over as the repeating input. The action has already fired once on
    /// the press edge, so the first repeat waits the initial delay.
    pub(super) fn start(&mut self, input: K, action: A, now: Instant) {
        self.held = Some(Held {
            input,
            action,
            pressed_at: now,
            last: None,
        });
    }

    /// Stop if this is the input repeating; another input's release is not ours
    /// to act on.
    pub(super) fn stop(&mut self, input: K) {
        if self.held.as_ref().is_some_and(|h| h.input == input) {
            self.held = None;
        }
    }

    pub(super) fn clear(&mut self) {
        self.held = None;
    }

    pub(super) fn tick(&mut self, now: Instant, out: &mut Vec<A>) {
        let Some(held) = &mut self.held else { return };
        let due = match held.last {
            None => now.duration_since(held.pressed_at) >= self.cadence.initial_delay,
            Some(last) => now.duration_since(last) >= self.cadence.interval,
        };
        if due {
            out.push(held.action);
            held.last = Some(now);
        }
    }

    pub(super) fn next_deadline(&self, now: Instant) -> Option<Duration> {
        let held = self.held.as_ref()?;
        let due = match held.last {
            None => held.pressed_at + self.cadence.initial_delay,
            Some(last) => last + self.cadence.interval,
        };
        Some(due.saturating_duration_since(now))
    }
}
