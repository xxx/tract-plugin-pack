//! Cross-instance group state via in-process static global.
//!
//! All gain-brain instances in the same host process share 16 atomic group
//! slots. No files, no mmap, no unsafe. Lock-free reads/writes suitable for
//! the audio thread.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

const NUM_GROUPS: usize = 16;

/// Per-group shared state. All fields are atomic for lock-free access
/// from multiple plugin instances on the audio thread.
struct GroupSlotAtomic {
    gain_millibels: AtomicI32,
    generation: AtomicU32,
    baseline_generation: AtomicU32,
}

/// Global shared group state. Lives for the lifetime of the host process.
/// All gain-brain instances in the same process share this directly.
static GROUPS: [GroupSlotAtomic; NUM_GROUPS] = {
    // This const is only used once, as the array repeat initializer below.
    // Clippy warns about interior mutability in consts, but this is the
    // standard pattern for initializing arrays of atomics at compile time.
    #[allow(clippy::declare_interior_mutable_const)]
    const SLOT_INIT: GroupSlotAtomic = GroupSlotAtomic {
        gain_millibels: AtomicI32::new(0),
        generation: AtomicU32::new(0),
        baseline_generation: AtomicU32::new(0),
    };
    [SLOT_INIT; NUM_GROUPS]
};

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupSlot {
    pub gain_millibels: i32,
    pub generation: u32,
    /// Incremented when an invert toggle or similar event requires readers
    /// to re-baseline without applying a delta.
    pub baseline_generation: u32,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Read a group slot. `group` must be 1-16 (panics otherwise).
pub fn read_slot(group: u8) -> GroupSlot {
    assert!(
        group >= 1 && group <= NUM_GROUPS as u8,
        "group must be 1-16, got {group}"
    );
    let idx = (group - 1) as usize;
    GroupSlot {
        gain_millibels: GROUPS[idx].gain_millibels.load(Ordering::Relaxed),
        generation: GROUPS[idx].generation.load(Ordering::Relaxed),
        baseline_generation: GROUPS[idx].baseline_generation.load(Ordering::Relaxed),
    }
}

/// Write gain to a group slot and increment generation. `group` must be 1-16.
pub fn write_slot(group: u8, gain_millibels: i32) {
    assert!(
        group >= 1 && group <= NUM_GROUPS as u8,
        "group must be 1-16, got {group}"
    );
    let idx = (group - 1) as usize;
    GROUPS[idx]
        .gain_millibels
        .store(gain_millibels, Ordering::Relaxed);
    GROUPS[idx].generation.fetch_add(1, Ordering::Relaxed);
}

/// Write gain and increment BOTH generation and baseline_generation.
/// Used for invert toggles: readers should re-baseline without applying a delta.
pub fn write_slot_rebaseline(group: u8, gain_millibels: i32) {
    assert!(
        group >= 1 && group <= NUM_GROUPS as u8,
        "group must be 1-16, got {group}"
    );
    let idx = (group - 1) as usize;
    GROUPS[idx]
        .gain_millibels
        .store(gain_millibels, Ordering::Relaxed);
    GROUPS[idx].generation.fetch_add(1, Ordering::Relaxed);
    GROUPS[idx].baseline_generation.fetch_add(1, Ordering::Relaxed);
}

/// Reset a group slot to its initial state (gain=0, generation=0, baseline=0).
/// Used in tests to isolate slot state between test runs.
#[cfg(test)]
pub(crate) fn reset_slot(group: u8) {
    assert!(
        group >= 1 && group <= NUM_GROUPS as u8,
        "group must be 1-16, got {group}"
    );
    let idx = (group - 1) as usize;
    GROUPS[idx].gain_millibels.store(0, Ordering::Relaxed);
    GROUPS[idx].generation.store(0, Ordering::Relaxed);
    GROUPS[idx].baseline_generation.store(0, Ordering::Relaxed);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Reset slots used by a test to avoid cross-test pollution from the
    /// global static. Tests use distinct slot ranges where possible, and
    /// reset before each test as a safety net.
    fn reset_slots(groups: &[u8]) {
        for &g in groups {
            reset_slot(g);
        }
    }

    // 1. Default slot has gain=0 and generation=0.
    #[test]
    fn test_read_default_slot_is_zero() {
        reset_slots(&[1]);
        let slot = read_slot(1);
        assert_eq!(slot.gain_millibels, 0);
        assert_eq!(slot.generation, 0);
        assert_eq!(slot.baseline_generation, 0);
    }

    // 2. Write gain to slot 3, read it back.
    #[test]
    fn test_write_and_read_slot() {
        reset_slots(&[3]);
        write_slot(3, 350);
        let slot = read_slot(3);
        assert_eq!(slot.gain_millibels, 350);
        assert_eq!(slot.generation, 1);
    }

    // 3. Write to same slot 3 times -> generation == 3.
    #[test]
    fn test_generation_increments() {
        reset_slots(&[4]);
        write_slot(4, 100);
        write_slot(4, 200);
        write_slot(4, 300);
        let slot = read_slot(4);
        assert_eq!(slot.generation, 3);
        assert_eq!(slot.gain_millibels, 300);
    }

    // 4. Write different values to slots 5 and 6; slot 7 remains zero.
    #[test]
    fn test_slots_are_independent() {
        reset_slots(&[5, 6, 7]);
        write_slot(5, 100);
        write_slot(6, 200);

        let s5 = read_slot(5);
        let s6 = read_slot(6);
        let s7 = read_slot(7);

        assert_eq!(s5.gain_millibels, 100);
        assert_eq!(s6.gain_millibels, 200);
        assert_eq!(s7.gain_millibels, 0);
        assert_eq!(s7.generation, 0);
    }

    // 5. Reading slot 0 panics.
    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_group_0_panics() {
        let _ = read_slot(0);
    }

    // 6. Reading slot 17 panics.
    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_group_17_panics() {
        let _ = read_slot(17);
    }

    // 7. Writing slot 0 panics.
    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_write_group_0_panics() {
        write_slot(0, 100);
    }

    // 8. Writing slot 17 panics.
    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_write_group_17_panics() {
        write_slot(17, 100);
    }

    // 9. Negative gain -1400 (-14.00 dB) round-trips correctly.
    #[test]
    fn test_negative_gain_millibels() {
        reset_slots(&[8]);
        write_slot(8, -1400);
        let slot = read_slot(8);
        assert_eq!(slot.gain_millibels, -1400);
    }

    // 10. Extreme values +/-6000 (+/-60.00 dB) round-trip correctly.
    #[test]
    fn test_extreme_values() {
        reset_slots(&[9, 10]);

        write_slot(9, 6000);
        assert_eq!(read_slot(9).gain_millibels, 6000);

        write_slot(10, -6000);
        assert_eq!(read_slot(10).gain_millibels, -6000);
    }

    // 11. write_slot_rebaseline increments both generation and baseline_generation.
    #[test]
    fn test_write_slot_rebaseline() {
        reset_slots(&[11]);
        write_slot_rebaseline(11, 500);
        let slot = read_slot(11);
        assert_eq!(slot.gain_millibels, 500);
        assert_eq!(slot.generation, 1);
        assert_eq!(slot.baseline_generation, 1);

        // A normal write after rebaseline increments generation only.
        write_slot(11, 600);
        let slot2 = read_slot(11);
        assert_eq!(slot2.gain_millibels, 600);
        assert_eq!(slot2.generation, 2);
        assert_eq!(slot2.baseline_generation, 1);
    }
}
