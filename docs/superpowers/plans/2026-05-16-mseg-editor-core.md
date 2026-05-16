# MSEG Editor Widget — Core Implementation Plan (Plan 1 of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the headless core of the MSEG envelope widget — the `MsegData` document model, the pure sampler and playback-rule functions, node mutation operations, and the randomizer — all in `tiny-skia-widgets`, fully unit-tested, with no rendering or interaction.

**Architecture:** A new `mseg/` module in `tiny-skia-widgets`. `MsegData` is a fixed-capacity, `Copy`, heap-free document (`[MsegNode; MAX_NODES]` + `node_count`) so it can be handed to an audio thread without allocating. Two pure functions — `value_at_phase` (raw shape) and `advance` (loop/sustain playback rules) — are the single source of truth shared by rendering (Plan 2) and a consuming plugin's DSP. The randomizer regenerates a document deterministically from a seed. Plan 2 (the editor: rendering + interaction) builds on this core.

**Tech Stack:** Rust (nightly), `serde` (new dependency, for persistence). No rendering deps in this plan.

**Reference reading before starting:**
- `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md` — the design spec (authoritative).
- `tiny-skia-widgets/src/dropdown.rs` — sibling widget; the `#[cfg(test)] mod tests` style, module conventions.
- `tiny-skia-widgets/src/lib.rs` — module registration pattern.

**Test command convention:** run one test with
`cargo nextest run -p tiny-skia-widgets <test_name_substring>`.
Run the whole module suite with `cargo nextest run -p tiny-skia-widgets mseg`.

---

## File Structure

- **Create:** `tiny-skia-widgets/src/mseg/mod.rs` — model types, the validity invariant, the sampler (`warp`, `value_at_phase`, `advance`), node mutation operations, and the serde glue.
- **Create:** `tiny-skia-widgets/src/mseg/randomize.rs` — `RandomStyle`, the PRNG, `randomize`.
- **Modify:** `tiny-skia-widgets/src/lib.rs` — register and re-export the module.
- **Modify:** `tiny-skia-widgets/Cargo.toml` — add the `serde` dependency.

One module file holds the model + sampler + node ops because they are one tightly-coupled unit (each operates directly on `MsegData`). The randomizer is separated — it is a distinct concern with its own PRNG. Plan 2 adds `mseg/render.rs` and `mseg/editor.rs`.

---

## Task 1: Module scaffold, types, and registration

**Files:**
- Create: `tiny-skia-widgets/src/mseg/mod.rs`
- Create: `tiny-skia-widgets/src/mseg/randomize.rs`
- Modify: `tiny-skia-widgets/src/lib.rs`
- Modify: `tiny-skia-widgets/Cargo.toml`

- [ ] **Step 1: Add the `serde` dependency**

In `tiny-skia-widgets/Cargo.toml`, under `[dependencies]`, add:

```toml
serde = { version = "1.0", features = ["derive"] }
```

(Read the file first to place it consistently with the existing entries.)

- [ ] **Step 2: Create `mseg/randomize.rs` as an empty-but-valid stub**

So the `pub mod randomize;` in the next step compiles. Create
`tiny-skia-widgets/src/mseg/randomize.rs` with just:

```rust
//! MSEG randomizer — generates randomized envelopes in several styles.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.
```

- [ ] **Step 3: Create `mseg/mod.rs` with the model types**

Create `tiny-skia-widgets/src/mseg/mod.rs`:

```rust
//! MSEG (multi-stage envelope generator) widget — core model, sampler, and
//! playback rules.
//!
//! `MsegData` is a fixed-capacity, `Copy`, heap-free envelope document: the
//! GUI edits it and a consuming plugin's audio thread reads it, and a `Copy`
//! `Vec`-free document crosses that boundary with a lock-free copy that never
//! allocates or frees on the audio thread.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

pub mod randomize;

pub use randomize::*;

/// Maximum number of envelope nodes.
pub const MAX_NODES: usize = 128;

/// How playback behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PlayMode {
    /// Runs once per trigger; honours sustain/loop while held.
    Triggered,
    /// Loops continuously; one MSEG span is one cycle.
    Cyclic,
}

/// How the envelope length is interpreted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SyncMode {
    /// Length is `time_seconds`.
    Time,
    /// Length is `beats` (host tempo).
    Beat,
}

/// The hold behaviour — sustain point or loop region. Mutually exclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum HoldMode {
    None,
    /// Triggered playback holds at this node index until released.
    Sustain(usize),
    /// Loop the `[start, end]` node-index range.
    Loop { start: usize, end: usize },
}

/// One envelope node. `tension`/`stepped` describe the segment FROM this node
/// to the next; the last active node's `tension`/`stepped` are unused.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct MsegNode {
    /// 0..1 normalized phase position.
    pub time: f32,
    /// 0..1 normalized level.
    pub value: f32,
    /// -1..1 segment bow (concave/convex).
    pub tension: f32,
    /// Segment is an instant jump + flat hold.
    pub stepped: bool,
}

impl Default for MsegNode {
    fn default() -> Self {
        Self {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        }
    }
}

/// The editable, serializable envelope document. Fixed-capacity and `Copy`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MsegData {
    /// Storage for up to `MAX_NODES` nodes; only `nodes[..node_count]` are
    /// active, ordered strictly ascending by time.
    pub nodes: [MsegNode; MAX_NODES],
    pub node_count: usize,
    pub play_mode: PlayMode,
    pub hold: HoldMode,
    pub sync_mode: SyncMode,
    /// Active length when `sync_mode == Time`.
    pub time_seconds: f32,
    /// Active length when `sync_mode == Beat`.
    pub beats: f32,
    /// Horizontal grid: N divisions of the 0..1 span.
    pub time_divisions: u32,
    /// Vertical grid: N value levels.
    pub value_steps: u32,
    pub snap: bool,
}

impl Default for MsegData {
    /// A rising 0→1 ramp: two nodes, `Triggered`, `Time` sync, 1 s long.
    fn default() -> Self {
        let mut nodes = [MsegNode::default(); MAX_NODES];
        nodes[0] = MsegNode { time: 0.0, value: 0.0, tension: 0.0, stepped: false };
        nodes[1] = MsegNode { time: 1.0, value: 1.0, tension: 0.0, stepped: false };
        Self {
            nodes,
            node_count: 2,
            play_mode: PlayMode::Triggered,
            hold: HoldMode::None,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            beats: 1.0,
            time_divisions: 16,
            value_steps: 8,
            snap: true,
        }
    }
}

impl MsegData {
    /// The active nodes — `nodes[..node_count]`.
    pub fn active(&self) -> &[MsegNode] {
        &self.nodes[..self.node_count]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_a_two_node_ramp() {
        let d = MsegData::default();
        assert_eq!(d.node_count, 2);
        assert_eq!(d.active().len(), 2);
        assert_eq!(d.nodes[0].time, 0.0);
        assert_eq!(d.nodes[0].value, 0.0);
        assert_eq!(d.nodes[1].time, 1.0);
        assert_eq!(d.nodes[1].value, 1.0);
    }

    #[test]
    fn mseg_data_is_copy() {
        // Compile-time check: MsegData must be Copy (no heap) so it can cross
        // the GUI/audio boundary without allocating.
        fn assert_copy<T: Copy>() {}
        assert_copy::<MsegData>();
    }
}
```

- [ ] **Step 4: Register the module in `lib.rs`**

In `tiny-skia-widgets/src/lib.rs`, add `pub mod mseg;` to the `pub mod` block (alphabetical — after `pub mod grid_selector;`, before `pub mod param_dial;`) and `pub use mseg::*;` to the `pub use` block (matching position). Read `lib.rs` first to match its formatting.

- [ ] **Step 5: Run the tests**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — `default_is_a_two_node_ramp` and `mseg_data_is_copy` pass; the crate compiles. Also run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean. (Every `mseg` item exposed so far — `MsegData::active`, the types — is `pub`, so nothing is dead-code-flagged even before later tasks use it.)

- [ ] **Step 6: Commit**

```bash
git add tiny-skia-widgets/src/mseg/ tiny-skia-widgets/src/lib.rs tiny-skia-widgets/Cargo.toml
git commit -m "feat(mseg): scaffold module with core types"
```

---

## Task 2: Validity invariant

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn default_data_is_valid() {
    assert!(MsegData::default().is_valid());
}

#[test]
fn unsorted_nodes_are_invalid() {
    let mut d = MsegData::default();
    d.node_count = 3;
    d.nodes[0] = MsegNode { time: 0.0, value: 0.0, ..Default::default() };
    d.nodes[1] = MsegNode { time: 0.8, value: 0.5, ..Default::default() };
    d.nodes[2] = MsegNode { time: 0.4, value: 1.0, ..Default::default() }; // out of order
    assert!(!d.is_valid());
}

#[test]
fn endpoints_must_be_pinned() {
    let mut d = MsegData::default();
    d.nodes[0].time = 0.1; // first node must be at time 0.0
    assert!(!d.is_valid());
    let mut d = MsegData::default();
    d.nodes[1].time = 0.9; // last node must be at time 1.0
    assert!(!d.is_valid());
}

#[test]
fn node_count_must_be_in_range() {
    let mut d = MsegData::default();
    d.node_count = 1;
    assert!(!d.is_valid());
    let mut d = MsegData::default();
    d.node_count = MAX_NODES + 1;
    assert!(!d.is_valid());
}

#[test]
fn out_of_range_hold_index_is_invalid() {
    let mut d = MsegData::default(); // node_count == 2
    d.hold = HoldMode::Sustain(5);
    assert!(!d.is_valid());
    let mut d = MsegData::default();
    d.hold = HoldMode::Loop { start: 1, end: 0 }; // start >= end
    assert!(!d.is_valid());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `is_valid` not defined.

- [ ] **Step 3: Implement `is_valid` and `debug_assert_valid`**

Add to the `impl MsegData` block:

```rust
    /// `true` iff the document satisfies every structural invariant. Slots
    /// `>= node_count` are not constrained.
    pub fn is_valid(&self) -> bool {
        if self.node_count < 2 || self.node_count > MAX_NODES {
            return false;
        }
        let a = self.active();
        if a[0].time != 0.0 || a[self.node_count - 1].time != 1.0 {
            return false;
        }
        for i in 0..self.node_count {
            let n = a[i];
            if !(0.0..=1.0).contains(&n.time)
                || !(0.0..=1.0).contains(&n.value)
                || !(-1.0..=1.0).contains(&n.tension)
            {
                return false;
            }
            if i > 0 && n.time <= a[i - 1].time {
                return false; // must be strictly ascending
            }
        }
        match self.hold {
            HoldMode::None => true,
            HoldMode::Sustain(i) => i < self.node_count,
            HoldMode::Loop { start, end } => {
                start < end && end < self.node_count
            }
        }
    }

    /// Debug-only assertion of `is_valid`. No-op in release builds.
    pub fn debug_assert_valid(&self) {
        debug_assert!(self.is_valid(), "MsegData invariant violated: {self:?}");
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 5 new tests plus Task 1's tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(mseg): add MsegData validity invariant"
```

---

## Task 3: Tension curve (`warp`)

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn warp_zero_tension_is_linear() {
    assert!((warp(0.0, 0.0) - 0.0).abs() < 1e-6);
    assert!((warp(0.25, 0.0) - 0.25).abs() < 1e-6);
    assert!((warp(0.5, 0.0) - 0.5).abs() < 1e-6);
    assert!((warp(1.0, 0.0) - 1.0).abs() < 1e-6);
}

#[test]
fn warp_pins_endpoints_for_any_tension() {
    for &k in &[-1.0, -0.5, 0.3, 1.0] {
        assert!((warp(0.0, k) - 0.0).abs() < 1e-5, "k={k}");
        assert!((warp(1.0, k) - 1.0).abs() < 1e-5, "k={k}");
    }
}

#[test]
fn warp_is_monotonic() {
    for &k in &[-1.0, -0.4, 0.4, 1.0] {
        let mut prev = warp(0.0, k);
        for step in 1..=20 {
            let t = step as f32 / 20.0;
            let w = warp(t, k);
            assert!(w >= prev - 1e-5, "non-monotonic at t={t}, k={k}");
            prev = w;
        }
    }
}

#[test]
fn warp_bows_in_opposite_directions() {
    // Positive tension: slow start -> midpoint output below 0.5.
    // Negative tension: fast start -> midpoint output above 0.5.
    assert!(warp(0.5, 1.0) < 0.5);
    assert!(warp(0.5, -1.0) > 0.5);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `warp` not defined.

- [ ] **Step 3: Implement `warp`**

Add at module scope in `mseg/mod.rs` (after the `impl MsegData` block):

```rust
/// Shape factor: tension is scaled by this into the exponential warp exponent.
const TENSION_K: f32 = 5.0;

/// Warp a 0..1 segment-local position by `tension` (-1..1). `tension == 0`
/// is linear; positive bows slow-start (concave), negative fast-start.
/// Always maps 0->0 and 1->1.
pub fn warp(t: f32, tension: f32) -> f32 {
    if tension.abs() < 1e-6 {
        return t;
    }
    let k = tension * TENSION_K;
    ((k * t).exp() - 1.0) / (k.exp() - 1.0)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 4 new tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(mseg): add tension warp curve"
```

---

## Task 4: Shape sampler (`value_at_phase`)

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn value_at_phase_linear_ramp() {
    let d = MsegData::default(); // 0->1 linear ramp
    assert!((value_at_phase(&d, 0.0) - 0.0).abs() < 1e-6);
    assert!((value_at_phase(&d, 0.5) - 0.5).abs() < 1e-6);
    assert!((value_at_phase(&d, 1.0) - 1.0).abs() < 1e-6);
}

#[test]
fn value_at_phase_clamps_out_of_range() {
    let d = MsegData::default();
    assert!((value_at_phase(&d, -0.5) - 0.0).abs() < 1e-6);
    assert!((value_at_phase(&d, 2.0) - 1.0).abs() < 1e-6);
}

#[test]
fn value_at_phase_stepped_segment_holds() {
    let mut d = MsegData::default();
    d.nodes[0].stepped = true; // segment 0 holds nodes[0].value (0.0)
    assert!((value_at_phase(&d, 0.0) - 0.0).abs() < 1e-6);
    assert!((value_at_phase(&d, 0.99) - 0.0).abs() < 1e-6); // still held
    assert!((value_at_phase(&d, 1.0) - 1.0).abs() < 1e-6);  // last node value
}

#[test]
fn value_at_phase_respects_tension() {
    let mut d = MsegData::default();
    d.nodes[0].tension = 1.0; // slow-start bow
    // Midpoint output should sit below the linear 0.5.
    assert!(value_at_phase(&d, 0.5) < 0.5);
}

#[test]
fn value_at_phase_three_nodes() {
    let mut d = MsegData::default();
    d.node_count = 3;
    d.nodes[0] = MsegNode { time: 0.0, value: 0.0, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 0.5, value: 1.0, tension: 0.0, stepped: false };
    d.nodes[2] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
    assert!((value_at_phase(&d, 0.25) - 0.5).abs() < 1e-6); // up-ramp midpoint
    assert!((value_at_phase(&d, 0.5) - 1.0).abs() < 1e-6);  // peak
    assert!((value_at_phase(&d, 0.75) - 0.5).abs() < 1e-6); // down-ramp midpoint
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `value_at_phase` not defined.

- [ ] **Step 3: Implement `value_at_phase`**

Add at module scope in `mseg/mod.rs`:

```rust
/// Sample the envelope's raw shape at `phase` (0..1, clamped). Pure — used by
/// both rendering and a consuming plugin's DSP.
pub fn value_at_phase(data: &MsegData, phase: f32) -> f32 {
    let a = data.active();
    let phase = phase.clamp(0.0, 1.0);

    // At or past the last node -> its value.
    if phase >= a[data.node_count - 1].time {
        return a[data.node_count - 1].value;
    }
    // Find the segment: the last node whose time is <= phase.
    let mut i = 0;
    for k in 0..data.node_count - 1 {
        if a[k].time <= phase {
            i = k;
        } else {
            break;
        }
    }
    let n0 = a[i];
    let n1 = a[i + 1];
    if n0.stepped {
        return n0.value;
    }
    let span = n1.time - n0.time;
    let t = if span > 1e-9 {
        (phase - n0.time) / span
    } else {
        0.0
    };
    n0.value + (n1.value - n0.value) * warp(t, n0.tension)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 5 new tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(mseg): add value_at_phase shape sampler"
```

---

## Task 5: Playback rules (`advance`)

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn advance_triggered_runs_to_end_and_finishes() {
    let d = MsegData::default(); // Triggered, hold None
    let (p, finished) = advance(&d, 0.5, 0.25, false);
    assert!((p - 0.75).abs() < 1e-6);
    assert!(!finished);
    let (p, finished) = advance(&d, 0.9, 0.25, false);
    assert!((p - 1.0).abs() < 1e-6);
    assert!(finished);
}

#[test]
fn advance_cyclic_wraps() {
    let mut d = MsegData::default();
    d.play_mode = PlayMode::Cyclic;
    let (p, finished) = advance(&d, 0.9, 0.25, false);
    assert!(!finished);
    assert!((p - 0.15).abs() < 1e-6, "0.9 + 0.25 wraps to 0.15, got {p}");
}

#[test]
fn advance_sustain_holds_until_released() {
    let mut d = MsegData::default();
    d.node_count = 3;
    d.nodes[0] = MsegNode { time: 0.0, value: 0.0, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 0.5, value: 1.0, tension: 0.0, stepped: false };
    d.nodes[2] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
    d.hold = HoldMode::Sustain(1); // node 1 is at time 0.5
    // Held: phase cannot pass the sustain node's time.
    let (p, finished) = advance(&d, 0.45, 0.25, false);
    assert!((p - 0.5).abs() < 1e-6, "held at sustain time, got {p}");
    assert!(!finished);
    // Released: advances past the sustain point normally.
    let (p, finished) = advance(&d, 0.5, 0.25, true);
    assert!((p - 0.75).abs() < 1e-6);
    assert!(!finished);
}

#[test]
fn advance_loop_wraps_while_held_then_exits_on_release() {
    let mut d = MsegData::default();
    d.node_count = 3;
    d.nodes[0] = MsegNode { time: 0.0, value: 0.0, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 0.25, value: 1.0, tension: 0.0, stepped: false };
    d.nodes[2] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
    d.hold = HoldMode::Loop { start: 0, end: 1 }; // loop [0.0, 0.25]
    // Held: crossing the loop end wraps back toward the loop start.
    let (p, _) = advance(&d, 0.2, 0.1, false);
    assert!(p < 0.25 && p >= 0.0, "looped back into [0,0.25], got {p}");
    // Released: advances past the loop end toward the real end.
    let (p, _) = advance(&d, 0.2, 0.1, true);
    assert!((p - 0.3).abs() < 1e-6, "released advances freely, got {p}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `advance` not defined.

- [ ] **Step 3: Implement `advance`**

Add at module scope in `mseg/mod.rs`:

```rust
/// Wrap `p` into the half-open range `[lo, hi)`.
fn wrap_into(mut p: f32, lo: f32, hi: f32) -> f32 {
    let span = (hi - lo).max(1e-9);
    while p >= hi {
        p -= span;
    }
    while p < lo {
        p += span;
    }
    p
}

/// Advance the playhead one step, applying the document's playback rules.
/// Returns `(next_phase, finished)`. `finished` is only ever `true` in
/// triggered playback once the playhead reaches the end. Pure — the consuming
/// plugin owns the `phase` value and the `released` flag.
pub fn advance(data: &MsegData, phase: f32, dt: f32, released: bool) -> (f32, bool) {
    let a = data.active();
    match data.play_mode {
        PlayMode::Cyclic => {
            let (lo, hi) = match data.hold {
                HoldMode::Loop { start, end } => (a[start].time, a[end].time),
                _ => (0.0, 1.0),
            };
            (wrap_into(phase + dt, lo, hi), false)
        }
        PlayMode::Triggered => {
            let mut p = phase + dt;
            if !released {
                match data.hold {
                    HoldMode::Sustain(i) => {
                        let st = a[i].time;
                        if p > st {
                            p = st;
                        }
                        return (p, false);
                    }
                    HoldMode::Loop { start, end } => {
                        let (lo, hi) = (a[start].time, a[end].time);
                        if p >= hi {
                            p = wrap_into(p, lo, hi);
                        }
                        return (p, false);
                    }
                    HoldMode::None => {}
                }
            }
            if p >= 1.0 {
                (1.0, true)
            } else {
                (p, false)
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 4 new tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(mseg): add advance playback rules"
```

---

## Task 6: Node mutation operations

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

These are the model-level edits the Plan 2 editor will drive: insert, remove, move. Each keeps `MsegData` valid and fixes up `hold` indices.

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn insert_node_keeps_time_order() {
    let mut d = MsegData::default(); // nodes at 0.0 and 1.0
    let idx = d.insert_node(0.5, 0.7).unwrap();
    assert_eq!(idx, 1);
    assert_eq!(d.node_count, 3);
    assert_eq!(d.nodes[1].time, 0.5);
    assert_eq!(d.nodes[1].value, 0.7);
    assert!(d.is_valid());
}

#[test]
fn insert_node_refuses_at_capacity() {
    let mut d = MsegData::default();
    // Fill to MAX_NODES.
    while d.node_count < MAX_NODES {
        let t = d.node_count as f32 / MAX_NODES as f32 * 0.99;
        d.insert_node(t, 0.5);
    }
    assert_eq!(d.node_count, MAX_NODES);
    assert!(d.insert_node(0.999, 0.5).is_none());
}

#[test]
fn insert_node_shifts_hold_indices() {
    let mut d = MsegData::default();
    d.insert_node(0.5, 0.5); // node_count 3; sustain a later node
    d.hold = HoldMode::Sustain(2); // the 1.0 endpoint
    d.insert_node(0.25, 0.5);      // inserts at index 1, pushing others up
    assert_eq!(d.hold, HoldMode::Sustain(3));
    assert!(d.is_valid());
}

#[test]
fn remove_node_refuses_endpoints() {
    let mut d = MsegData::default();
    assert!(!d.remove_node(0));
    assert!(!d.remove_node(1)); // last node
    assert_eq!(d.node_count, 2);
}

#[test]
fn remove_node_deletes_interior_and_fixes_hold() {
    let mut d = MsegData::default();
    d.insert_node(0.3, 0.5); // idx 1
    d.insert_node(0.6, 0.5); // idx 2
    d.hold = HoldMode::Sustain(2);
    assert!(d.remove_node(1)); // remove the 0.3 node
    assert_eq!(d.node_count, 3);
    assert_eq!(d.hold, HoldMode::Sustain(1)); // shifted down
    assert!(d.is_valid());
}

#[test]
fn remove_node_clears_hold_referencing_removed_node() {
    let mut d = MsegData::default();
    d.insert_node(0.5, 0.5);
    d.hold = HoldMode::Sustain(1);
    assert!(d.remove_node(1));
    assert_eq!(d.hold, HoldMode::None);
}

#[test]
fn move_node_clamps_interior_between_neighbors() {
    let mut d = MsegData::default();
    d.insert_node(0.5, 0.5); // idx 1
    d.move_node(1, 5.0, 2.0); // wildly out of range
    assert!(d.nodes[1].time > 0.0 && d.nodes[1].time < 1.0);
    assert!(d.nodes[1].value >= 0.0 && d.nodes[1].value <= 1.0);
    assert!(d.is_valid());
}

#[test]
fn move_node_pins_endpoint_time() {
    let mut d = MsegData::default();
    d.move_node(0, 0.4, 0.8); // try to move the first node's time
    assert_eq!(d.nodes[0].time, 0.0); // time pinned
    assert_eq!(d.nodes[0].value, 0.8); // value moved
    d.move_node(1, 0.4, 0.2);
    assert_eq!(d.nodes[1].time, 1.0); // time pinned
    assert_eq!(d.nodes[1].value, 0.2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `insert_node` / `remove_node` / `move_node` not defined.

- [ ] **Step 3: Implement the mutation operations**

Add to the `impl MsegData` block. First a private `hold` fix-up helper, then the three operations:

```rust
    /// Smallest time gap allowed between adjacent nodes.
    const MIN_NODE_GAP: f32 = 1e-4;

    /// Insert a node at `(time, value)`, keeping time order. `time`/`value`
    /// are clamped to 0..1; `time` is clamped strictly between the existing
    /// nodes it falls between. Returns the new node's index, or `None` if the
    /// document is already at `MAX_NODES`. The new node's segment is linear
    /// (tension 0, not stepped).
    pub fn insert_node(&mut self, time: f32, value: f32) -> Option<usize> {
        if self.node_count >= MAX_NODES {
            return None;
        }
        let value = value.clamp(0.0, 1.0);
        // Insertion index: first active node with a strictly greater time.
        let mut k = self.node_count;
        for i in 0..self.node_count {
            if self.nodes[i].time > time {
                k = i;
                break;
            }
        }
        // Endpoints stay pinned: never insert before index 1 or after the last.
        let k = k.clamp(1, self.node_count - 1);
        let lo = self.nodes[k - 1].time + Self::MIN_NODE_GAP;
        let hi = self.nodes[k].time - Self::MIN_NODE_GAP;
        let time = time.clamp(lo, hi);
        // Shift [k..node_count) up by one.
        let mut i = self.node_count;
        while i > k {
            self.nodes[i] = self.nodes[i - 1];
            i -= 1;
        }
        self.nodes[k] = MsegNode { time, value, tension: 0.0, stepped: false };
        self.node_count += 1;
        self.shift_hold_for_insert(k);
        self.debug_assert_valid();
        Some(k)
    }

    /// Remove the node at `idx`. Endpoints (index 0 and the last) cannot be
    /// removed. Returns `true` if a node was removed.
    pub fn remove_node(&mut self, idx: usize) -> bool {
        if idx == 0 || idx + 1 >= self.node_count {
            return false;
        }
        // Shift (idx, node_count) down by one.
        for i in idx..self.node_count - 1 {
            self.nodes[i] = self.nodes[i + 1];
        }
        self.node_count -= 1;
        self.fix_hold_for_remove(idx);
        self.debug_assert_valid();
        true
    }

    /// Move the node at `idx` to `(time, value)`. `value` is clamped to 0..1.
    /// Endpoint nodes keep their pinned time (0.0 / 1.0); interior nodes have
    /// `time` clamped strictly between their neighbours.
    pub fn move_node(&mut self, idx: usize, time: f32, value: f32) {
        if idx >= self.node_count {
            return;
        }
        self.nodes[idx].value = value.clamp(0.0, 1.0);
        let is_endpoint = idx == 0 || idx + 1 == self.node_count;
        if !is_endpoint {
            let lo = self.nodes[idx - 1].time + Self::MIN_NODE_GAP;
            let hi = self.nodes[idx + 1].time - Self::MIN_NODE_GAP;
            self.nodes[idx].time = time.clamp(lo, hi);
        }
        self.debug_assert_valid();
    }

    /// After inserting a node at index `k`, bump every `hold` index `>= k`.
    fn shift_hold_for_insert(&mut self, k: usize) {
        let bump = |i: usize| if i >= k { i + 1 } else { i };
        self.hold = match self.hold {
            HoldMode::None => HoldMode::None,
            HoldMode::Sustain(i) => HoldMode::Sustain(bump(i)),
            HoldMode::Loop { start, end } => HoldMode::Loop {
                start: bump(start),
                end: bump(end),
            },
        };
    }

    /// After removing the node at `idx`, fix up `hold`: a reference to the
    /// removed node clears `hold`; higher indices shift down.
    fn fix_hold_for_remove(&mut self, idx: usize) {
        let adjust = |i: usize| -> Option<usize> {
            match i.cmp(&idx) {
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(i - 1),
                std::cmp::Ordering::Less => Some(i),
            }
        };
        self.hold = match self.hold {
            HoldMode::None => HoldMode::None,
            HoldMode::Sustain(i) => match adjust(i) {
                Some(i) => HoldMode::Sustain(i),
                None => HoldMode::None,
            },
            HoldMode::Loop { start, end } => match (adjust(start), adjust(end)) {
                (Some(s), Some(e)) if s < e => HoldMode::Loop { start: s, end: e },
                _ => HoldMode::None,
            },
        };
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 8 new tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(mseg): add node insert/remove/move operations"
```

---

## Task 7: Randomizer

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/randomize.rs`

- [ ] **Step 1: Write failing tests**

Add a `#[cfg(test)] mod tests` to `mseg/randomize.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::MsegData;

    #[test]
    fn randomize_is_deterministic() {
        let mut a = MsegData::default();
        let mut b = MsegData::default();
        randomize(&mut a, RandomStyle::Spiky, 12345);
        randomize(&mut b, RandomStyle::Spiky, 12345);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_different_seeds_differ() {
        let mut a = MsegData::default();
        let mut b = MsegData::default();
        randomize(&mut a, RandomStyle::Chaos, 1);
        randomize(&mut b, RandomStyle::Chaos, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_keeps_data_valid_for_every_style() {
        for style in [
            RandomStyle::Smooth,
            RandomStyle::Ramps,
            RandomStyle::Stepped,
            RandomStyle::Spiky,
            RandomStyle::Chaos,
        ] {
            for seed in 0..40u32 {
                let mut d = MsegData::default();
                randomize(&mut d, style, seed);
                assert!(d.is_valid(), "invalid for {style:?} seed {seed}");
                assert!(d.node_count >= 2 && d.node_count <= MAX_NODES);
            }
        }
    }

    #[test]
    fn stepped_style_makes_every_segment_stepped() {
        let mut d = MsegData::default();
        randomize(&mut d, RandomStyle::Stepped, 7);
        // Every segment except the last node's (unused) must be stepped.
        for i in 0..d.node_count - 1 {
            assert!(d.nodes[i].stepped, "segment {i} not stepped");
        }
    }

    #[test]
    fn smooth_style_has_no_stepped_segments() {
        let mut d = MsegData::default();
        randomize(&mut d, RandomStyle::Smooth, 7);
        for i in 0..d.node_count {
            assert!(!d.nodes[i].stepped);
        }
    }

    #[test]
    fn randomize_only_touches_shape() {
        let mut d = MsegData::default();
        d.time_seconds = 3.5;
        d.beats = 2.0;
        d.time_divisions = 12;
        d.value_steps = 5;
        d.play_mode = crate::mseg::PlayMode::Cyclic;
        randomize(&mut d, RandomStyle::Ramps, 9);
        assert_eq!(d.time_seconds, 3.5);
        assert_eq!(d.beats, 2.0);
        assert_eq!(d.time_divisions, 12);
        assert_eq!(d.value_steps, 5);
        assert_eq!(d.play_mode, crate::mseg::PlayMode::Cyclic);
    }

    #[test]
    fn randomize_clears_hold_when_count_changes() {
        let mut d = MsegData::default();
        d.insert_node(0.5, 0.5);
        d.hold = crate::mseg::HoldMode::Sustain(2);
        randomize(&mut d, RandomStyle::Chaos, 3);
        // Chaos changes node_count; the stale hold index must be cleared if
        // it is now out of range. Either way the document stays valid.
        assert!(d.is_valid());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `RandomStyle` / `randomize` not defined.

- [ ] **Step 3: Implement the randomizer**

Replace the contents of `mseg/randomize.rs` (keep the existing doc comment header) with — appending below the header:

```rust
use crate::mseg::{HoldMode, MsegData, MsegNode, MAX_NODES};

/// Randomizer character. Each style biases node count, values, tension, and
/// stepping differently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RandomStyle {
    Smooth,
    Ramps,
    Stepped,
    Spiky,
    Chaos,
}

/// Deterministic xorshift32 PRNG — no dependency, seeded per `randomize` call.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // Avoid the all-zero state, which xorshift cannot leave.
        Rng(seed | 1)
    }
    /// Next raw u32.
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Uniform f32 in 0..1.
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform f32 in `lo..hi`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
    /// Uniform usize in `lo..=hi`.
    fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u32() as usize) % (hi - lo + 1)
    }
    fn bool(&mut self) -> bool {
        self.next_u32() & 1 == 1
    }
}

/// Snap `v` to one of `steps` evenly-spaced levels in 0..1.
fn snap_value(v: f32, steps: u32) -> f32 {
    if steps == 0 {
        return v.clamp(0.0, 1.0);
    }
    let s = steps as f32;
    (v * s).round().clamp(0.0, s) / s
}

/// Regenerate `data.nodes` / `data.node_count` in the given `style`.
/// Deterministic given `seed`. Leaves `play_mode`, `sync_mode`, timing, and
/// grid settings untouched. Any `hold` left referencing an out-of-range node
/// is reset to `HoldMode::None`.
pub fn randomize(data: &mut MsegData, style: RandomStyle, seed: u32) {
    let mut rng = Rng::new(seed);

    // Node count. Stepped/Spiky fill the time grid (one node per cell, +1 for
    // the closing endpoint), capped at MAX_NODES. Smooth/Ramps are sparse.
    // Chaos picks freely.
    let grid_count = (data.time_divisions as usize + 1).clamp(2, MAX_NODES);
    let count = match style {
        RandomStyle::Stepped | RandomStyle::Spiky => grid_count,
        RandomStyle::Smooth | RandomStyle::Ramps => rng.range_usize(3, 6),
        RandomStyle::Chaos => rng.range_usize(3, 16.min(MAX_NODES)),
    };

    for i in 0..count {
        // Time: endpoints pinned; interior spread evenly, then snapped to the
        // time grid when snap is on.
        let time = if i == 0 {
            0.0
        } else if i == count - 1 {
            1.0
        } else {
            let even = i as f32 / (count - 1) as f32;
            if data.snap && data.time_divisions > 0 {
                let d = data.time_divisions as f32;
                ((even * d).round() / d).clamp(0.0, 1.0)
            } else {
                even
            }
        };

        // Value and tension per style.
        let (mut value, tension, stepped) = match style {
            RandomStyle::Smooth => (rng.range(0.25, 0.85), rng.range(-0.6, 0.6), false),
            RandomStyle::Ramps => (rng.next_f32(), 0.0, false),
            RandomStyle::Stepped => (rng.next_f32(), 0.0, true),
            RandomStyle::Spiky => {
                let v = if i % 2 == 0 { rng.range(0.0, 0.15) } else { rng.range(0.85, 1.0) };
                (v, rng.range(-1.0, 1.0), rng.bool())
            }
            RandomStyle::Chaos => (rng.next_f32(), rng.range(-1.0, 1.0), rng.bool()),
        };

        // Value-grid snap for the styles where quantized levels matter.
        if data.snap && matches!(style, RandomStyle::Stepped | RandomStyle::Spiky) {
            value = snap_value(value, data.value_steps);
        }

        data.nodes[i] = MsegNode { time, value, tension, stepped };
    }
    data.node_count = count;

    // The interior times above can collide after snapping. Repair strict
    // ascending order by nudging any node that did not advance.
    for i in 1..count - 1 {
        let prev = data.nodes[i - 1].time;
        if data.nodes[i].time <= prev {
            data.nodes[i].time = (prev + 1e-3).min(1.0 - 1e-3);
        }
    }
    // The closing endpoint must still be the strict maximum.
    if count >= 2 && data.nodes[count - 1].time <= data.nodes[count - 2].time {
        data.nodes[count - 2].time = 1.0 - 1e-3;
    }

    // Invalidate a now-out-of-range hold.
    let hold_ok = match data.hold {
        HoldMode::None => true,
        HoldMode::Sustain(i) => i < count,
        HoldMode::Loop { start, end } => start < end && end < count,
    };
    if !hold_ok {
        data.hold = HoldMode::None;
    }

    data.debug_assert_valid();
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 7 randomizer tests plus every earlier test.

If `randomize_keeps_data_valid_for_every_style` ever fails on a seed, the
ascending-order repair above is the suspect — do not weaken the test; fix the
repair so the document is genuinely valid.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/randomize.rs
git commit -m "feat(mseg): add envelope randomizer"
```

---

## Task 8: Persistence (serde for `MsegData`)

`MsegData` holds a 128-element array. Rather than rely on serde's array
support or persist 128 nodes when 3 are used, `MsegData` (de)serializes via a
compact `Vec`-backed helper struct. The `Vec` lives only during
(de)serialization (load/save time on the GUI thread) — never on the audio
thread.

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

- [ ] **Step 1: Write a failing round-trip test**

Add inside `mod tests`:

```rust
#[test]
fn mseg_data_json_round_trips() {
    let mut d = MsegData::default();
    d.insert_node(0.3, 0.7);
    d.insert_node(0.6, 0.2);
    d.nodes[1].tension = 0.5;
    d.nodes[2].stepped = true;
    d.hold = HoldMode::Sustain(2);
    d.play_mode = PlayMode::Cyclic;
    d.sync_mode = SyncMode::Beat;
    d.beats = 2.0;
    d.time_divisions = 24;
    d.value_steps = 6;
    d.snap = false;

    let json = serde_json::to_string(&d).unwrap();
    let back: MsegData = serde_json::from_str(&json).unwrap();
    assert_eq!(d, back);
    assert!(back.is_valid());
}

#[test]
fn mseg_data_json_is_compact() {
    // The default 2-node document must not serialize all MAX_NODES slots.
    let json = serde_json::to_string(&MsegData::default()).unwrap();
    assert!(
        json.len() < 400,
        "serialized default unexpectedly large ({} bytes)",
        json.len()
    );
}
```

- [ ] **Step 2: Add `serde_json` as a dev-dependency and run the test to verify it fails**

In `tiny-skia-widgets/Cargo.toml`, add under `[dev-dependencies]` (create the
section if absent):

```toml
serde_json = "1.0"
```

Run: `cargo nextest run -p tiny-skia-widgets mseg_data_json`
Expected: FAIL — `MsegData` does not implement `Serialize`/`Deserialize`.

- [ ] **Step 3: Implement the compact serde glue**

Add to `mseg/mod.rs` at module scope (after the `impl MsegData` block):

```rust
/// Compact, `Vec`-backed mirror of `MsegData` used only for (de)serialization.
/// Lives on the GUI thread at load/save time — never on the audio thread.
#[derive(serde::Serialize, serde::Deserialize)]
struct MsegDataSerde {
    nodes: Vec<MsegNode>,
    play_mode: PlayMode,
    hold: HoldMode,
    sync_mode: SyncMode,
    time_seconds: f32,
    beats: f32,
    time_divisions: u32,
    value_steps: u32,
    snap: bool,
}

impl From<&MsegData> for MsegDataSerde {
    fn from(d: &MsegData) -> Self {
        Self {
            nodes: d.active().to_vec(),
            play_mode: d.play_mode,
            hold: d.hold,
            sync_mode: d.sync_mode,
            time_seconds: d.time_seconds,
            beats: d.beats,
            time_divisions: d.time_divisions,
            value_steps: d.value_steps,
            snap: d.snap,
        }
    }
}

impl serde::Serialize for MsegData {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        MsegDataSerde::from(self).serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for MsegData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = MsegDataSerde::deserialize(d)?;
        let mut data = MsegData {
            nodes: [MsegNode::default(); MAX_NODES],
            node_count: raw.nodes.len().clamp(0, MAX_NODES),
            play_mode: raw.play_mode,
            hold: raw.hold,
            sync_mode: raw.sync_mode,
            time_seconds: raw.time_seconds,
            beats: raw.beats,
            time_divisions: raw.time_divisions,
            value_steps: raw.value_steps,
            snap: raw.snap,
        };
        for (i, n) in raw.nodes.iter().take(MAX_NODES).enumerate() {
            data.nodes[i] = *n;
        }
        // A corrupt or hand-edited blob must not yield an invalid document.
        if !data.is_valid() {
            return Err(serde::de::Error::custom("invalid MsegData"));
        }
        Ok(data)
    }
}
```

Note: `MsegNode` and the enums keep their `#[derive(serde::Serialize,
serde::Deserialize)]` from Task 1. Only `MsegData`'s derive is replaced by
these hand-written impls — remove `serde::Serialize, serde::Deserialize` from
`MsegData`'s `#[derive(...)]` if Task 1 had added them (Task 1's `MsegData`
derive was `Clone, Copy, PartialEq, Debug` only, so there is nothing to
remove — confirm this).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — both round-trip tests plus every earlier test.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/mod.rs tiny-skia-widgets/Cargo.toml
git commit -m "feat(mseg): add compact serde persistence for MsegData"
```

---

## Task 9: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Run the whole module suite**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — every `mseg` test.

- [ ] **Step 2: Run the full crate suite + workspace lint + fmt**

Run: `cargo nextest run -p tiny-skia-widgets`
Expected: PASS — the new `mseg` tests plus all pre-existing crate tests, no regressions.

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --check`
Expected: no diff (apply `cargo fmt` if needed).

- [ ] **Step 3: Confirm the public API is re-exported**

Run: `cargo doc -p tiny-skia-widgets --no-deps`
Expected: builds clean. `MsegData`, `MsegNode`, `PlayMode`, `SyncMode`, `HoldMode`, `MAX_NODES`, `warp`, `value_at_phase`, `advance`, `RandomStyle`, and `randomize` are all reachable at the crate root (via `pub use mseg::*;`).

- [ ] **Step 4: No commit** — verification only.

---

## Self-Review Notes

**Spec coverage** — every core-layer spec requirement maps to a task:
- Data model (`MsegData`, `MsegNode`, enums, `MAX_NODES`, fixed-capacity `Copy`) → Task 1.
- Validity invariant → Task 2.
- Tension `warp` → Task 3.
- `value_at_phase` → Task 4.
- `advance` playback rules (cyclic wrap, triggered run-to-end, sustain, loop) → Task 5.
- Node operations (insert/remove/move, `hold` index fix-up) → Task 6.
- Randomizer (5 styles, grid-driven count, determinism, `hold` invalidation) → Task 7.
- Persistence (compact serde) → Task 8.

**Out of scope here (Plan 2 — the editor):** `mseg/render.rs`, `mseg/editor.rs`, `MsegEditState`, `draw_mseg`, the event handlers, the control strip, stepped-draw, hit-testing, render smoke tests. Plan 1 is the headless, fully-tested foundation Plan 2 builds on.

**Type consistency** — `MsegData`, `MsegNode`, `PlayMode`, `SyncMode`,
`HoldMode`, `RandomStyle`, and the function signatures (`warp`,
`value_at_phase`, `advance`, `insert_node`, `remove_node`, `move_node`,
`randomize`) are used identically across all tasks. `advance` returns
`(f32, bool)` everywhere; `insert_node` returns `Option<usize>`;
`remove_node` returns `bool`.

**Note on the design spec's `default()`** — the spec says the default
document is `Triggered` / `SyncMode::Time` / `time_seconds 1.0` /
`time_divisions 16` / `value_steps 8` / `snap true`; Task 1's `Default` impl
matches exactly.
