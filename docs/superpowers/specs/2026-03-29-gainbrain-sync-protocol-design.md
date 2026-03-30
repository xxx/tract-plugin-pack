# Gain Brain Sync Protocol Redesign

## Problem

The current protocol stores absolute gain values in a shared slot. Each instance computes deltas by diffing against a baseline. This breaks when instances have different invert states because inverted and non-inverted writers store values in different coordinate spaces (positive vs negative), making delta calculations across writers nonsensical.

## Design: Cumulative Canonical Deltas

### Core Concept

The slot stores a **running sum of deltas in canonical (non-inverted) space**. Writers atomically add their canonical delta. Readers compute the difference between the current cumulative and their last-seen value. No deltas are ever lost.

Invert is applied at the boundaries — writers and readers transform to/from canonical space. The slot itself is coordinate-space-neutral.

### Slot Structure

```rust
struct GroupSlotAtomic {
    /// Running sum of all canonical deltas (millibels). Writers use fetch_add.
    cumulative_delta: AtomicI32,
    /// Incremented on rebaseline events (invert toggle, group join).
    /// Readers detect epoch change and re-baseline without applying a delta.
    epoch: AtomicU32,
    /// Number of active instances in this group. Used to detect stale slots
    /// on join (active=0 means no live writers, slot data is from a dead session).
    active_count: AtomicU32,
}
```

### Writer (in sync_group, when user moves knob)

```
local_delta_mb = current_param_mb - last_param_mb
canonical_delta = if invert { -local_delta_mb } else { local_delta_mb }
slot.cumulative_delta.fetch_add(canonical_delta, Relaxed)
```

The writer converts its local movement to canonical space before adding. An inverted instance moving +3dB adds -300mb (canonical = opposite direction in non-inverted space).

### Reader (in sync_group, when slot changes)

```
current_cumulative = slot.cumulative_delta.load(Relaxed)
canonical_delta = current_cumulative - my_last_seen_cumulative
local_delta = if invert { -canonical_delta } else { canonical_delta }
effective_gain_db += millibels_to_db(local_delta)
my_last_seen_cumulative = current_cumulative
```

The reader computes how much the cumulative moved since it last looked, then transforms from canonical to local space.

### Self-echo suppression

A writer must not read back its own delta as an external change. After a fetch_add, the writer updates `last_seen_cumulative` to the new cumulative value:

```
old_cumulative = slot.cumulative_delta.fetch_add(canonical_delta, Relaxed)
my_last_seen_cumulative = old_cumulative + canonical_delta
```

On the next read, `current_cumulative - last_seen_cumulative` will be 0 if no other instance wrote. If another instance also wrote, the difference is exactly the other instance's delta.

### Invert toggle (rebaseline)

When an instance toggles invert, it bumps the epoch:

```
slot.epoch.fetch_add(1, Relaxed)
```

All readers detect the epoch change and reset `last_seen_cumulative` to the current `cumulative_delta` without applying a delta. This prevents the coordinate-space flip from being interpreted as a movement.

The toggling instance also resets its own `last_seen_cumulative` after bumping the epoch.

### Group join

**Relative mode:**
- If `active_count == 1` (first instance, stale slot): reset `cumulative_delta` to 0, set `last_seen_cumulative = 0`. Bump epoch.
- If `active_count > 1` (joining a live group): set `last_seen_cumulative = current cumulative_delta`. Future deltas from other instances will be applied. The joiner keeps its own effective gain.

**Absolute mode:**
- Unchanged from current behavior. The slot stores an absolute value separately for absolute mode (or absolute mode can be handled by reading the cumulative and converting).

Actually — absolute mode is simpler. In absolute mode, all instances should have the same gain (or negated if inverted). This can be handled by storing the writer's absolute effective gain (in canonical space) alongside the cumulative delta. Readers in absolute mode adopt the absolute value; readers in relative mode use the cumulative delta.

### Revised slot structure (supporting both modes)

```rust
struct GroupSlotAtomic {
    /// Running sum of canonical deltas for relative mode.
    cumulative_delta: AtomicI32,
    /// Last writer's effective gain in canonical space for absolute mode.
    absolute_gain: AtomicI32,
    /// Incremented on rebaseline events.
    epoch: AtomicU32,
    /// Writer generation (incremented on every write) for change detection.
    generation: AtomicU32,
    /// Active instance count for stale detection.
    active_count: AtomicU32,
}
```

### Writer (unified for both modes)

```
local_delta_mb = current_param_mb - last_param_mb
canonical_delta = if invert { -local_delta_mb } else { local_delta_mb }
canonical_absolute = if invert { -current_effective_mb } else { current_effective_mb }

slot.cumulative_delta.fetch_add(canonical_delta, Relaxed)
slot.absolute_gain.store(canonical_absolute, Relaxed)
slot.generation.fetch_add(1, Relaxed)

// Self-echo: update last_seen so we don't read our own write
last_seen_cumulative += canonical_delta
last_seen_generation = slot.generation.load(Relaxed)
```

### Reader — Relative mode

```
if slot.epoch != my_last_epoch:
    // Rebaseline: someone toggled invert. Don't apply delta.
    my_last_seen_cumulative = slot.cumulative_delta.load(Relaxed)
    my_last_epoch = slot.epoch
    return

current_cumulative = slot.cumulative_delta.load(Relaxed)
if current_cumulative != my_last_seen_cumulative:
    canonical_delta = current_cumulative - my_last_seen_cumulative
    local_delta = if invert { -canonical_delta } else { canonical_delta }
    effective_gain_db += millibels_to_db(local_delta)
    my_last_seen_cumulative = current_cumulative
```

### Reader — Absolute mode

```
if slot.generation != my_last_seen_generation:
    canonical = slot.absolute_gain.load(Relaxed)
    local = if invert { -canonical } else { canonical }
    effective_gain_db = millibels_to_db(local)
    my_last_seen_generation = slot.generation
    // Also sync cumulative tracking to prevent spurious relative reads
    my_last_seen_cumulative = slot.cumulative_delta.load(Relaxed)
```

### Per-instance state

```rust
struct SyncState {
    last_seen_cumulative: i32,   // tracks cumulative_delta for relative reads
    last_seen_generation: u32,   // tracks generation for absolute reads
    last_seen_epoch: u32,        // tracks epoch for rebaseline detection
    last_param_mb: i32,          // tracks param for write detection
    effective_gain_db: f32,      // the actual gain being applied
}
```

### Correctness properties

1. **No missed deltas**: `fetch_add` accumulates all writes. Reader computes net delta from its last-seen position. Even with many writes between reads, the net is exact.

2. **Invert-correct**: canonical space transformation at boundaries ensures inverted↔inverted = same direction, inverted↔non-inverted = opposite direction.

3. **No feedback loop**: self-echo suppression via `last_seen_cumulative` update after write. The SyncGainParam task updates `params.gain` but not the slot, so no spurious writes from param sync.

4. **Stale slot handling**: `active_count` refcount detects dead sessions. First joiner resets the slot.

5. **Rebaseline safety**: epoch change tells readers to re-sync without applying a delta. The coordinate-space flip from invert toggle doesn't produce a false movement.

### What changes from current code

**groups.rs**: Complete rewrite of slot structure and public API. New fields, new functions (fetch_add-based write, epoch bump, etc.).

**lib.rs sync_group()**: Simplified. No more baseline tracking, no more complex read/write path interactions. Writer does fetch_add + store. Reader does load + diff. Self-echo is just tracking the cumulative.

**lib.rs handle_transition()**: Simplified. Join = set last_seen_cumulative. Leave = decrement active. No writes to slot on join (except for stale reset).

**Absolute mode**: Uses `absolute_gain` field with simple load/store. No delta computation.

### Testing strategy

1. **Two instances, relative, no invert**: A moves +3dB → B follows +3dB
2. **Two instances, relative, B inverted**: A moves +3dB → B moves -3dB
3. **Invert toggle mid-session**: no jump on either instance
4. **Multiple rapid writes, delayed read**: reader gets exact cumulative delta
5. **Self-echo**: writer doesn't read its own delta
6. **Stale slot on join**: first joiner resets, second baselines correctly
7. **Active instance + late joiner**: joiner doesn't clobber active writer
8. **Restart cycle**: deactivate → initialize → correct refcount
9. **Absolute mode**: still works with the new slot structure
10. **Mixed modes**: one instance absolute, one relative — no interference
