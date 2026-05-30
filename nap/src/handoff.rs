//! Lock-free-ish GUI→audio handoff of the baked `VelvetSequence` and of the
//! per-channel `IrSpectra` pair. Both mirror miff's `KernelHandoff` but guard
//! the (larger) copies with a generation counter so the audio thread copies
//! only when the data actually changed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::ir::IrSpectra;
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

// ---------------------------------------------------------------------------
// IrHandoff — GUI→audio handoff of the baked per-channel IrSpectra pair.
// ---------------------------------------------------------------------------

pub struct IrHandoff {
    shared: Mutex<(IrSpectra, IrSpectra)>,
    generation: AtomicU64,
}

impl IrHandoff {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            shared: Mutex::new((IrSpectra::new(sample_rate), IrSpectra::new(sample_rate))),
            generation: AtomicU64::new(0),
        }
    }

    /// Publish freshly-baked L/R spectra (GUI thread). Copies into the shared
    /// slot and bumps the generation.
    pub fn publish(&self, left: &IrSpectra, right: &IrSpectra) {
        if let Ok(mut slot) = self.shared.lock() {
            slot.0.copy_from(left);
            slot.1.copy_from(right);
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Audio thread: if newer spectra exist, copy them into `local_l`/`local_r`
    /// and update `*local_gen`. Returns `true` if the locals changed. Never
    /// blocks hard (uses `try_lock`); never allocates.
    pub fn try_read_into(
        &self,
        local_l: &mut IrSpectra,
        local_r: &mut IrSpectra,
        local_gen: &mut u64,
    ) -> bool {
        if self.generation.load(Ordering::Acquire) == *local_gen {
            return false; // unchanged — skip the lock/copy entirely
        }
        if let Ok(slot) = self.shared.try_lock() {
            local_l.copy_from(&slot.0);
            local_r.copy_from(&slot.1);
            *local_gen = self.generation.load(Ordering::Acquire);
            return true;
        }
        false // contended this block; try again next block
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

    // --- IrHandoff tests ---

    const SR: f32 = 48_000.0;

    fn mk_ir(k: usize, sentinel: f32) -> IrSpectra {
        use tract_dsp::partitioned_conv::BINS;
        let mut s = IrSpectra::new(SR);
        s.k = k;
        if k > 0 {
            s.spectra[0] = rustfft::num_complex::Complex::new(sentinel, 0.0);
        }
        let _ = BINS; // exercised indirectly
        s
    }

    #[test]
    fn ir_first_read_picks_up_published() {
        let h = IrHandoff::new(SR);
        h.publish(&mk_ir(2, 1.5), &mk_ir(3, 2.5));
        let mut ll = IrSpectra::new(SR);
        let mut lr = IrSpectra::new(SR);
        let mut gen = 0u64;
        assert!(h.try_read_into(&mut ll, &mut lr, &mut gen));
        assert_eq!(ll.k, 2);
        assert_eq!(lr.k, 3);
        assert!((ll.spectra[0].re - 1.5).abs() < 1e-6);
        assert!((lr.spectra[0].re - 2.5).abs() < 1e-6);
    }

    #[test]
    fn ir_unchanged_generation_skips_copy() {
        let h = IrHandoff::new(SR);
        h.publish(&mk_ir(1, 0.1), &mk_ir(1, 0.2));
        let mut ll = IrSpectra::new(SR);
        let mut lr = IrSpectra::new(SR);
        let mut gen = 0u64;
        assert!(h.try_read_into(&mut ll, &mut lr, &mut gen)); // first: copies
        assert!(!h.try_read_into(&mut ll, &mut lr, &mut gen)); // second: unchanged
    }

    #[test]
    fn ir_newest_publish_wins() {
        let h = IrHandoff::new(SR);
        h.publish(&mk_ir(1, 9.0), &mk_ir(1, 9.0));
        h.publish(&mk_ir(4, 7.7), &mk_ir(5, 8.8));
        let mut ll = IrSpectra::new(SR);
        let mut lr = IrSpectra::new(SR);
        let mut gen = 0u64;
        h.try_read_into(&mut ll, &mut lr, &mut gen);
        assert_eq!(ll.k, 4);
        assert_eq!(lr.k, 5);
        assert!((ll.spectra[0].re - 7.7).abs() < 1e-6);
        assert!((lr.spectra[0].re - 8.8).abs() < 1e-6);
    }
}
