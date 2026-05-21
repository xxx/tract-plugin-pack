//! Lock-free audio→GUI mirror of the sequencer step counter.
//!
//! The audio thread publishes once per process block; the editor reads it
//! each frame to draw the status readout. One `Relaxed` atomic.

use std::sync::atomic::{AtomicU64, Ordering};

/// The audio→GUI mirror of the running step count.
pub struct SeqStatusDisplay {
    step: AtomicU64,
}

impl SeqStatusDisplay {
    /// A display reading step 0.
    pub fn new() -> Self {
        Self {
            step: AtomicU64::new(0),
        }
    }

    /// Audio thread: publish the current step count.
    pub fn publish(&self, step: u64) {
        self.step.store(step, Ordering::Relaxed);
    }

    /// GUI thread: read the last published step count.
    pub fn read(&self) -> u64 {
        self.step.load(Ordering::Relaxed)
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
    fn new_display_reads_step_zero() {
        assert_eq!(SeqStatusDisplay::new().read(), 0);
    }

    #[test]
    fn publish_round_trips_the_step() {
        let d = SeqStatusDisplay::new();
        d.publish(42);
        assert_eq!(d.read(), 42);
    }
}
