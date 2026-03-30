# Gain Brain Cumulative Delta Sync Protocol Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace gain-brain's absolute-value slot protocol with cumulative canonical deltas so relative mode works correctly with invert.

**Architecture:** The shared slot stores a running sum of deltas in canonical (non-inverted) space via `fetch_add`. Writers transform local deltas to canonical before adding. Readers compute the diff from their last-seen cumulative and transform from canonical to local. Self-echo is suppressed by tracking the cumulative after each write. Invert toggles bump an epoch counter; readers re-baseline on epoch change.

**Tech Stack:** Rust, `std::sync::atomic`, nih-plug plugin framework.

---

### Task 1: Rewrite groups.rs slot structure and API

**Files:**
- Rewrite: `gain-brain/src/groups.rs`

- [ ] **Step 1: Write failing tests for the new API**

Replace all existing tests in `groups.rs` with tests for the new API. The new slot has `cumulative_delta`, `absolute_gain`, `epoch`, `generation`, `active_count`.

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gain-brain -- groups::tests -v`
Expected: FAIL — functions `add_delta`, `set_absolute`, `bump_epoch`, `reset_cumulative`, `read_slot` return wrong types.

- [ ] **Step 3: Implement the new groups.rs**

Replace the entire `groups.rs` with:

```rust
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
    let old = GROUPS[i].cumulative_delta.fetch_add(canonical_delta_mb, Ordering::Relaxed);
    GROUPS[i].generation.fetch_add(1, Ordering::Relaxed);
    old
}

/// Store the absolute gain value (canonical space) for absolute mode readers.
pub fn set_absolute(group: u8, canonical_gain_mb: i32) {
    assert_group(group);
    GROUPS[idx(group)].absolute_gain.store(canonical_gain_mb, Ordering::Relaxed);
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gain-brain -- groups::tests -v`
Expected: All 13 pass.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy --package gain-brain -- -D warnings`
Expected: Clean.

---

### Task 2: Rewrite sync_group and handle_transition in lib.rs

**Files:**
- Modify: `gain-brain/src/lib.rs`

This is the core protocol change. Replace the entire sync logic.

- [ ] **Step 1: Update GainBrain struct fields**

Remove: `last_seen_generation`, `last_baseline_generation`, `last_sent_gain_millibels`, `relative_baseline_mb`.

Add/rename:
```rust
/// Last cumulative_delta value we observed. Used for self-echo suppression
/// and relative delta computation.
last_seen_cumulative: i32,
/// Last epoch we observed. Used for rebaseline detection.
last_seen_epoch: u32,
/// Last generation we observed (for absolute mode change detection).
last_seen_generation: u32,
```

Update `Default::default()` and `SyncState` accordingly.

- [ ] **Step 2: Rewrite sync_group**

Replace the entire `sync_group` method with the new protocol:

```rust
fn sync_group(&mut self) {
    let group = self.params.group.value();
    let link_mode = self.params.link_mode.value();
    let invert = self.params.invert.value();

    // ── Transitions (group/mode changes) ──
    let group_changed = group != self.last_group;
    let mode_changed = link_mode != self.last_link_mode;
    if group_changed || mode_changed {
        self.handle_transition(group, link_mode);
        self.last_group = group;
        self.last_link_mode = link_mode;
    }

    if !(1..=16).contains(&group) {
        return;
    }

    // ── Invert toggle → bump epoch ──
    if invert != self.last_invert {
        groups::bump_epoch(group as u8);
        let snap = groups::read_slot(group as u8);
        self.last_seen_cumulative = snap.cumulative_delta;
        self.last_seen_epoch = snap.epoch;
        self.last_invert = invert;
    }

    let snap = groups::read_slot(group as u8);

    // ── READ PATH ──
    let mut read_fired = false;

    // Epoch change → rebaseline (don't apply delta)
    if snap.epoch != self.last_seen_epoch {
        self.last_seen_cumulative = snap.cumulative_delta;
        self.last_seen_epoch = snap.epoch;
        self.last_seen_generation = snap.generation;
        // For absolute mode, also adopt the absolute gain
        if link_mode == LinkMode::Absolute {
            let canonical = snap.absolute_gain;
            let local = if invert { -canonical } else { canonical };
            let local = local.clamp(-6000, 6000);
            self.group_gain_override.store(local, Ordering::Relaxed);
            self.effective_gain_db = millibels_to_db(local);
            self.last_param_value_mb = local;
        }
        read_fired = true;
    } else {
        match link_mode {
            LinkMode::Absolute => {
                if snap.generation != self.last_seen_generation {
                    let canonical = snap.absolute_gain;
                    let local = if invert { -canonical } else { canonical };
                    let local = local.clamp(-6000, 6000);
                    self.group_gain_override.store(local, Ordering::Relaxed);
                    self.effective_gain_db = millibels_to_db(local);
                    self.last_seen_generation = snap.generation;
                    self.last_seen_cumulative = snap.cumulative_delta;
                    self.last_param_value_mb = local;
                    read_fired = true;
                }
            }
            LinkMode::Relative => {
                if snap.cumulative_delta != self.last_seen_cumulative {
                    let canonical_delta = snap.cumulative_delta - self.last_seen_cumulative;
                    let local_delta = if invert { -canonical_delta } else { canonical_delta };
                    let current_mb = db_to_millibels(self.effective_gain_db);
                    let new_mb = current_mb + local_delta;
                    let clamped_db = clamp_db(millibels_to_db(new_mb));
                    let clamped_mb = db_to_millibels(clamped_db);
                    self.group_gain_override.store(clamped_mb, Ordering::Relaxed);
                    self.effective_gain_db = clamped_db;
                    self.last_seen_cumulative = snap.cumulative_delta;
                    self.last_seen_generation = snap.generation;
                    self.last_param_value_mb = clamped_mb;
                    read_fired = true;
                }
            }
        }
    }

    // ── WRITE PATH ──
    let current_gain_db = util::gain_to_db(self.params.gain.value());
    let current_mb = db_to_millibels(current_gain_db);

    if (current_mb - self.last_param_value_mb).abs() > 1
        && !read_fired
        && !self.param_sync_pending
    {
        let local_delta = current_mb - self.last_param_value_mb;
        let canonical_delta = if invert { -local_delta } else { local_delta };
        let canonical_absolute = if invert { -current_mb } else { current_mb };

        let old_cumulative = groups::add_delta(group as u8, canonical_delta);
        groups::set_absolute(group as u8, canonical_absolute);

        // Self-echo suppression
        self.last_seen_cumulative = old_cumulative + canonical_delta;
        self.last_seen_generation = groups::read_slot(group as u8).generation;

        self.effective_gain_db = current_gain_db;
    }

    if !read_fired {
        self.last_param_value_mb = current_mb;
    }
}
```

- [ ] **Step 3: Rewrite handle_transition**

```rust
fn handle_transition(&mut self, new_group: i32, new_link_mode: LinkMode) {
    let old_group = self.last_group;

    // Leaving a group
    if (1..=16).contains(&old_group) {
        groups::decrement_active(old_group as u8);
    }

    // Not joining any group
    if !(1..=16).contains(&new_group) {
        self.last_seen_cumulative = 0;
        self.last_seen_epoch = 0;
        self.last_seen_generation = 0;
        return;
    }

    // Joining a group
    groups::increment_active(new_group as u8);
    let count = groups::active_count(new_group as u8);

    if count <= 1 {
        // First instance (stale slot) — reset
        groups::reset_cumulative(new_group as u8);
        self.last_seen_cumulative = 0;
        self.last_seen_epoch = groups::read_slot(new_group as u8).epoch;
        self.last_seen_generation = 0;
    } else {
        // Joining a live group — baseline to current state
        let snap = groups::read_slot(new_group as u8);
        self.last_seen_cumulative = snap.cumulative_delta;
        self.last_seen_epoch = snap.epoch;
        self.last_seen_generation = snap.generation;

        if new_link_mode == LinkMode::Absolute {
            let canonical = snap.absolute_gain;
            let invert = self.params.invert.value();
            let local = if invert { -canonical } else { canonical };
            let local = local.clamp(-6000, 6000);
            self.group_gain_override.store(local, Ordering::Relaxed);
            self.effective_gain_db = millibels_to_db(local);
        }
        // Relative: keep own effective gain, just baseline cumulative
    }

    // Sync param tracking
    let param_db = util::gain_to_db(self.params.gain.value());
    self.last_param_value_mb = db_to_millibels(param_db);
    self.last_invert = self.params.invert.value();
}
```

- [ ] **Step 4: Update initialize and deactivate**

`initialize()`: use the persisted effective gain, sync `last_group`/`last_param_value_mb`/`last_invert` to current param values. Increment active count for the current group.

`deactivate()`: decrement active count, reset `last_group = 0`.

These should be mostly unchanged from the current code, just removing references to deleted fields.

- [ ] **Step 5: Remove SyncState struct**

The `SyncState` borrow-splitting struct is no longer needed since `sync_group` now operates directly on `self` fields (the protocol is simpler and doesn't need to pass a mutable reference to a separate function). Remove `SyncState` and inline all field access.

- [ ] **Step 6: Compile check**

Run: `cargo check --package gain-brain`
Expected: Compiles (tests may fail — that's Task 3).

---

### Task 3: Write integration tests for the new protocol

**Files:**
- Modify: `gain-brain/src/lib.rs` (test module)

- [ ] **Step 1: Write test helpers**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(gain_db: f32, group: i32, link_mode: LinkMode, invert: bool) -> Arc<GainBrainParams> {
        // Create params with specific initial values
        // (same pattern as existing make_params helper)
    }

    fn make_instance(params: Arc<GainBrainParams>) -> GainBrain {
        // Create instance with custom params
    }

    fn tick(inst: &mut GainBrain) {
        // Run sync_group + drain override (simulates one process() buffer)
    }

    fn init(inst: &mut GainBrain) {
        // Simulate initialize() lifecycle
    }

    fn deinit(inst: &mut GainBrain) {
        // Simulate deactivate() lifecycle
    }
}
```

- [ ] **Step 2: Test — two instances, relative, no invert**

```rust
#[test]
fn test_relative_no_invert() {
    groups::reset_slot(1);
    let mut a = make_instance(make_params(0.0, 1, LinkMode::Relative, false));
    let mut b = make_instance(make_params(0.0, 1, LinkMode::Relative, false));
    init(&mut a); init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // A drags to +3dB
    a.params = make_params(3.0, 1, LinkMode::Relative, false);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    assert!((b.effective_gain_db - 3.0).abs() < 0.5, "B should follow A: got {}", b.effective_gain_db);
}
```

- [ ] **Step 3: Test — two instances, relative, B inverted**

```rust
#[test]
fn test_relative_b_inverted() {
    groups::reset_slot(2);
    let mut a = make_instance(make_params(0.0, 2, LinkMode::Relative, false));
    let mut b = make_instance(make_params(0.0, 2, LinkMode::Relative, true));
    init(&mut a); init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // A drags to +3dB
    a.params = make_params(3.0, 2, LinkMode::Relative, false);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    assert!((b.effective_gain_db - (-3.0)).abs() < 0.5, "B inverted should go -3dB: got {}", b.effective_gain_db);
}
```

- [ ] **Step 4: Test — invert toggle mid-session**

```rust
#[test]
fn test_invert_toggle_no_jump() {
    groups::reset_slot(3);
    let mut a = make_instance(make_params(5.0, 3, LinkMode::Relative, false));
    let mut b = make_instance(make_params(5.0, 3, LinkMode::Relative, false));
    init(&mut a); init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    let b_before = b.effective_gain_db;
    // B toggles invert
    b.params = make_params(5.0, 3, LinkMode::Relative, true);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // B's effective should not jump
    assert!((b.effective_gain_db - b_before).abs() < 0.5, "B should not jump on invert toggle");
    // A's effective should not jump
    assert!((a.effective_gain_db - 5.0).abs() < 0.5, "A should not jump on B's invert toggle");
}
```

- [ ] **Step 5: Test — multiple rapid writes, delayed read**

```rust
#[test]
fn test_rapid_writes_delayed_read() {
    groups::reset_slot(4);
    let mut a = make_instance(make_params(0.0, 4, LinkMode::Relative, false));
    let mut b = make_instance(make_params(0.0, 4, LinkMode::Relative, false));
    init(&mut a); init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // A writes 5 times without B reading
    for gain in [1.0, 2.0, 3.0, 4.0, 5.0] {
        a.params = make_params(gain, 4, LinkMode::Relative, false);
        tick(&mut a);
    }

    // B reads once — should get the full +5dB delta
    for _ in 0..3 { tick(&mut b); }
    assert!((b.effective_gain_db - 5.0).abs() < 0.5, "B should get cumulative delta: got {}", b.effective_gain_db);
}
```

- [ ] **Step 6: Test — self-echo suppression**

```rust
#[test]
fn test_self_echo_suppression() {
    groups::reset_slot(5);
    let mut a = make_instance(make_params(0.0, 5, LinkMode::Relative, false));
    init(&mut a);
    for _ in 0..3 { tick(&mut a); }

    a.params = make_params(3.0, 5, LinkMode::Relative, false);
    tick(&mut a);

    // A should be at 3dB (from its own write), not 6dB (from reading its own delta)
    assert!((a.effective_gain_db - 3.0).abs() < 0.5, "A should not read its own delta: got {}", a.effective_gain_db);
}
```

- [ ] **Step 7: Test — stale slot on join**

```rust
#[test]
fn test_stale_slot_cleared_on_join() {
    groups::reset_slot(6);
    // Simulate stale data
    groups::add_delta(6, 1000);
    // No active instances

    let mut a = make_instance(make_params(0.0, 6, LinkMode::Relative, false));
    init(&mut a);
    for _ in 0..3 { tick(&mut a); }

    // A should be at 0dB, not affected by stale 1000mb
    assert!(a.effective_gain_db.abs() < 0.5, "A should not be affected by stale data: got {}", a.effective_gain_db);
}
```

- [ ] **Step 8: Test — active instance + late joiner**

```rust
#[test]
fn test_late_joiner_doesnt_clobber() {
    groups::reset_slot(7);
    let mut a = make_instance(make_params(6.0, 7, LinkMode::Relative, false));
    init(&mut a);
    for _ in 0..3 { tick(&mut a); }

    let mut b = make_instance(make_params(0.0, 7, LinkMode::Relative, false));
    init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // A should still be at ~6dB
    assert!((a.effective_gain_db - 6.0).abs() < 0.5, "A should not be clobbered: got {}", a.effective_gain_db);
}
```

- [ ] **Step 9: Test — absolute mode**

```rust
#[test]
fn test_absolute_mode() {
    groups::reset_slot(8);
    let mut a = make_instance(make_params(5.0, 8, LinkMode::Absolute, false));
    let mut b = make_instance(make_params(0.0, 8, LinkMode::Absolute, false));
    init(&mut a); init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // B should adopt A's value
    assert!((b.effective_gain_db - 5.0).abs() < 0.5, "B should adopt A's gain: got {}", b.effective_gain_db);
}
```

- [ ] **Step 10: Test — absolute mode with invert**

```rust
#[test]
fn test_absolute_mode_inverted() {
    groups::reset_slot(9);
    let mut a = make_instance(make_params(5.0, 9, LinkMode::Absolute, false));
    let mut b = make_instance(make_params(0.0, 9, LinkMode::Absolute, true));
    init(&mut a); init(&mut b);
    for _ in 0..3 { tick(&mut a); tick(&mut b); }

    // B inverted should adopt -5dB
    assert!((b.effective_gain_db - (-5.0)).abs() < 0.5, "B inverted should adopt -5dB: got {}", b.effective_gain_db);
}
```

- [ ] **Step 11: Run all tests**

Run: `cargo test --package gain-brain`
Expected: All pass.

---

### Task 4: Clean up and build

**Files:**
- Modify: `gain-brain/src/lib.rs` (remove debug logging throttle, clean up)

- [ ] **Step 1: Keep essential debug logging, remove throttle counter**

Keep `nih_log!` calls for TRANSITION, INVERT TOGGLE, READ, WRITE, APPLY OVERRIDE. Remove the `sync_call_count` field and the first-20-calls throttle (it clutters the code). Debug logs are cheap when nothing is happening.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --package gain-brain -- -D warnings`
Expected: Clean.

- [ ] **Step 3: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 4: Build release**

Run: `touch gain-brain/src/lib.rs && cargo nih-plug bundle gain-brain --release`
Expected: CLAP + VST3 bundles built.

- [ ] **Step 5: Update manual**

Update `docs/gain-brain/gain-brain-manual.md` Technical Notes section to describe cumulative delta protocol instead of the old absolute-value protocol.
