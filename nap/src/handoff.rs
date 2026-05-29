//! Lock-free-ish GUI→audio handoff of the baked `VelvetSequence`. Mirrors
//! miff's `KernelHandoff`, but guards the (larger) copy with a generation
//! counter so the audio thread copies only when the sequence actually changed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::sequence::VelvetSequence;

pub struct SequenceHandoff {
    shared: Mutex<VelvetSequence>,
    generation: AtomicU64,
}

impl SequenceHandoff {
    pub fn new() -> Self {
        Self {
            shared: Mutex::new(VelvetSequence::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Publish a freshly-generated sequence (GUI thread). Copies into the
    /// shared slot and bumps the generation.
    pub fn publish(&self, seq: &VelvetSequence) {
        if let Ok(mut slot) = self.shared.lock() {
            slot.copy_from(seq);
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Audio thread: if a newer sequence exists, copy it into `local` and
    /// update `*local_gen`. Returns `true` if `local` changed. Never blocks
    /// hard (uses `try_lock`); never allocates.
    pub fn try_read_into(&self, local: &mut VelvetSequence, local_gen: &mut u64) -> bool {
        if self.generation.load(Ordering::Acquire) == *local_gen {
            return false; // unchanged — skip the lock/copy entirely
        }
        if let Ok(slot) = self.shared.try_lock() {
            local.copy_from(&slot);
            *local_gen = self.generation.load(Ordering::Acquire);
            return true;
        }
        false // contended this block; try again next block
    }
}

impl Default for SequenceHandoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(count: usize, first_loc: u32) -> VelvetSequence {
        let mut s = VelvetSequence::new();
        s.count = count;
        s.tail_len = first_loc as usize + 1;
        if count > 0 {
            s.location[0] = first_loc;
        }
        s
    }

    #[test]
    fn first_read_picks_up_published_sequence() {
        let h = SequenceHandoff::new();
        h.publish(&mk(3, 42));
        let mut local = VelvetSequence::new();
        let mut local_gen = 0;
        assert!(h.try_read_into(&mut local, &mut local_gen));
        assert_eq!(local.count, 3);
        assert_eq!(local.location[0], 42);
    }

    #[test]
    fn unchanged_generation_skips_copy() {
        let h = SequenceHandoff::new();
        h.publish(&mk(1, 7));
        let mut local = VelvetSequence::new();
        let mut local_gen = 0;
        assert!(h.try_read_into(&mut local, &mut local_gen)); // first: copies
        assert!(!h.try_read_into(&mut local, &mut local_gen)); // second: no change
    }

    #[test]
    fn newest_publish_wins() {
        let h = SequenceHandoff::new();
        h.publish(&mk(1, 1));
        h.publish(&mk(2, 5));
        let mut local = VelvetSequence::new();
        let mut local_gen = 0;
        h.try_read_into(&mut local, &mut local_gen);
        assert_eq!(local.count, 2);
        assert_eq!(local.location[0], 5);
    }
}
