//! Cross-instance group state via in-process static global.
//!
//! Uses cumulative canonical deltas for relative mode. Writers add deltas
//! via fetch_add. Readers compute the diff from their last-seen cumulative.
//! Absolute mode uses a separate absolute_gain field with simple load/store.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

const NUM_GROUPS: usize = 16;

struct GroupSlotAtomic {
    /// Running sum of canonical deltas (millibels). Writers use fetch_add.
    cumulative_delta: AtomicI32,
    /// Last writer's effective gain in canonical space (for absolute mode).
    absolute_gain: AtomicI32,
    /// Incremented on rebaseline events (invert toggle, stale slot reset).
    epoch: AtomicU32,
    /// Incremented on every write (for absolute mode change detection).
    generation: AtomicU32,
    /// Active instance count for stale slot detection.
    active_count: AtomicU32,
}

static GROUPS: [GroupSlotAtomic; NUM_GROUPS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const SLOT_INIT: GroupSlotAtomic = GroupSlotAtomic {
        cumulative_delta: AtomicI32::new(0),
        absolute_gain: AtomicI32::new(0),
        epoch: AtomicU32::new(0),
        generation: AtomicU32::new(0),
        active_count: AtomicU32::new(0),
    };
    [SLOT_INIT; NUM_GROUPS]
};

// ── Public types ────────────────────────────────────────────────────────────

/// Snapshot of a group slot's current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotSnapshot {
    pub cumulative_delta: i32,
    pub absolute_gain: i32,
    pub epoch: u32,
    pub generation: u32,
}

// ── Public API ──────────────────────────────────────────────────────────────

fn assert_group(group: u8) {
    assert!(
        group >= 1 && group <= NUM_GROUPS as u8,
        "group must be 1-16, got {group}"
    );
}

fn idx(group: u8) -> usize {
    (group - 1) as usize
}

/// Read a snapshot of the group slot.
pub fn read_slot(group: u8) -> SlotSnapshot {
    assert_group(group);
    let i = idx(group);
    SlotSnapshot {
        cumulative_delta: GROUPS[i].cumulative_delta.load(Ordering::Relaxed),
        absolute_gain: GROUPS[i].absolute_gain.load(Ordering::Relaxed),
        epoch: GROUPS[i].epoch.load(Ordering::Relaxed),
        generation: GROUPS[i].generation.load(Ordering::Relaxed),
    }
}

/// Atomically add a canonical delta to the cumulative sum.
/// Returns the OLD cumulative value (before the add) for self-echo suppression.
/// Also increments generation.
pub fn add_delta(group: u8, canonical_delta_mb: i32) -> i32 {
    assert_group(group);
    let i = idx(group);
    let old = GROUPS[i]
        .cumulative_delta
        .fetch_add(canonical_delta_mb, Ordering::Relaxed);
    GROUPS[i].generation.fetch_add(1, Ordering::Relaxed);
    old
}

/// Store the absolute gain value (canonical space) for absolute mode readers.
pub fn set_absolute(group: u8, canonical_gain_mb: i32) {
    assert_group(group);
    GROUPS[idx(group)]
        .absolute_gain
        .store(canonical_gain_mb, Ordering::Relaxed);
}

/// Bump the epoch counter. Readers will re-baseline on epoch change.
pub fn bump_epoch(group: u8) {
    assert_group(group);
    GROUPS[idx(group)].epoch.fetch_add(1, Ordering::Relaxed);
}

/// Reset cumulative_delta to 0 and bump epoch. Used when the first instance
/// joins a stale slot (active_count was 0).
pub fn reset_cumulative(group: u8) {
    assert_group(group);
    let i = idx(group);
    GROUPS[i].cumulative_delta.store(0, Ordering::Relaxed);
    GROUPS[i].absolute_gain.store(0, Ordering::Relaxed);
    GROUPS[i].epoch.fetch_add(1, Ordering::Relaxed);
    GROUPS[i].generation.store(0, Ordering::Relaxed);
}

pub fn increment_active(group: u8) {
    assert_group(group);
    GROUPS[idx(group)].active_count.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active(group: u8) {
    assert_group(group);
    GROUPS[idx(group)].active_count.fetch_sub(1, Ordering::Relaxed);
}

pub fn active_count(group: u8) -> u32 {
    assert_group(group);
    GROUPS[idx(group)].active_count.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn reset_slot(group: u8) {
    assert_group(group);
    let i = idx(group);
    GROUPS[i].cumulative_delta.store(0, Ordering::Relaxed);
    GROUPS[i].absolute_gain.store(0, Ordering::Relaxed);
    GROUPS[i].epoch.store(0, Ordering::Relaxed);
    GROUPS[i].generation.store(0, Ordering::Relaxed);
    GROUPS[i].active_count.store(0, Ordering::Relaxed);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_slots(groups: &[u8]) {
        for &g in groups {
            reset_slot(g);
        }
    }

    #[test]
    fn test_default_slot_is_zero() {
        reset_slots(&[1]);
        let snap = read_slot(1);
        assert_eq!(snap.cumulative_delta, 0);
        assert_eq!(snap.absolute_gain, 0);
        assert_eq!(snap.epoch, 0);
        assert_eq!(snap.generation, 0);
    }

    #[test]
    fn test_add_delta_accumulates() {
        reset_slots(&[2]);
        add_delta(2, 300); // +3dB canonical
        add_delta(2, 200); // +2dB canonical
        let snap = read_slot(2);
        assert_eq!(snap.cumulative_delta, 500);
        assert_eq!(snap.generation, 2);
    }

    #[test]
    fn test_add_delta_returns_old_cumulative() {
        reset_slots(&[3]);
        let old = add_delta(3, 300);
        assert_eq!(old, 0);
        let old2 = add_delta(3, 200);
        assert_eq!(old2, 300);
    }

    #[test]
    fn test_set_absolute_stores_value() {
        reset_slots(&[4]);
        set_absolute(4, -500);
        let snap = read_slot(4);
        assert_eq!(snap.absolute_gain, -500);
    }

    #[test]
    fn test_bump_epoch_increments() {
        reset_slots(&[5]);
        bump_epoch(5);
        bump_epoch(5);
        let snap = read_slot(5);
        assert_eq!(snap.epoch, 2);
    }

    #[test]
    fn test_reset_cumulative_zeros_delta_and_bumps_epoch() {
        reset_slots(&[6]);
        add_delta(6, 1000);
        reset_cumulative(6);
        let snap = read_slot(6);
        assert_eq!(snap.cumulative_delta, 0);
        assert_eq!(snap.epoch, 1); // epoch bumped
    }

    #[test]
    fn test_active_count() {
        reset_slots(&[7]);
        increment_active(7);
        increment_active(7);
        assert_eq!(active_count(7), 2);
        decrement_active(7);
        assert_eq!(active_count(7), 1);
    }

    #[test]
    fn test_slots_are_independent() {
        reset_slots(&[8, 9]);
        add_delta(8, 100);
        add_delta(9, 200);
        assert_eq!(read_slot(8).cumulative_delta, 100);
        assert_eq!(read_slot(9).cumulative_delta, 200);
    }

    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_group_0_panics() {
        let _ = read_slot(0);
    }

    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_group_17_panics() {
        let _ = read_slot(17);
    }

    #[test]
    fn test_negative_delta() {
        reset_slots(&[10]);
        add_delta(10, -1400);
        assert_eq!(read_slot(10).cumulative_delta, -1400);
    }

    #[test]
    fn test_mixed_deltas_accumulate() {
        reset_slots(&[11]);
        add_delta(11, 500);
        add_delta(11, -300);
        add_delta(11, 100);
        assert_eq!(read_slot(11).cumulative_delta, 300);
        assert_eq!(read_slot(11).generation, 3);
    }
}
