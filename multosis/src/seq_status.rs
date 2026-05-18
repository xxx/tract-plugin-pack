//! Lock-free audio→GUI mirror of the sequence lifecycle state and step count.
//!
//! The audio thread publishes once per process block; the editor reads it each
//! frame to draw the status readout. Two `Relaxed` atomics — a torn pair is
//! sub-frame and visually irrelevant.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §3.1.

use crate::propagation::SequenceState;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

/// The audio→GUI mirror: the lifecycle state (as a `u8` code) and the step
/// count.
pub struct SeqStatusDisplay {
    /// 0 = Initial, 1 = Running, 2 = Stopped.
    state: AtomicU8,
    step: AtomicU64,
}

impl SeqStatusDisplay {
    /// A display reading `Initial`, step 0.
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            step: AtomicU64::new(0),
        }
    }

    /// Audio thread: publish the current lifecycle state and step count.
    pub fn publish(&self, state: SequenceState, step: u64) {
        let code = match state {
            SequenceState::Initial => 0,
            SequenceState::Running => 1,
            SequenceState::Stopped => 2,
        };
        self.state.store(code, Ordering::Relaxed);
        self.step.store(step, Ordering::Relaxed);
    }

    /// GUI thread: read the last published `(state, step)`.
    pub fn read(&self) -> (SequenceState, u64) {
        let state = match self.state.load(Ordering::Relaxed) {
            0 => SequenceState::Initial,
            1 => SequenceState::Running,
            _ => SequenceState::Stopped,
        };
        (state, self.step.load(Ordering::Relaxed))
    }
}

impl Default for SeqStatusDisplay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_display_reads_initial_state() {
        let d = SeqStatusDisplay::new();
        assert_eq!(d.read(), (SequenceState::Initial, 0));
    }

    #[test]
    fn publish_round_trips_every_state() {
        let d = SeqStatusDisplay::new();
        for (state, step) in [
            (SequenceState::Initial, 0),
            (SequenceState::Running, 14),
            (SequenceState::Stopped, 7),
        ] {
            d.publish(state, step);
            assert_eq!(d.read(), (state, step));
        }
    }
}
