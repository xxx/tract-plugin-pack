# Multosis Phase 1 — Milestone 1a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the headless routing model of the Multosis sequencer — the grid, the wavefront propagation engine, and the step clock — as a pure-logic, fully unit-tested Rust crate.

**Architecture:** A new workspace crate `multosis`. Milestone 1a is library-only (plain rlib, no nih-plug, no GUI): five focused modules — `grid` (cells, routing geometry, grid operations), `region` (copy/paste), `randomize` (deterministic randomization), `propagation` (wavefront lifecycle), `clock` (tempo-synced stepping). The nih-plug plugin shell, audio engine, and editor are Milestone 1b — out of scope here.

**Tech Stack:** Rust (nightly, pinned by the workspace `rust-toolchain.toml`); `serde` for grid serialization; `cargo nextest` as the test runner.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` — §3.1, §4, §5 cover this milestone.

**Working branch:** `multosis` (already created). All work commits to it.

**Commit convention:** Every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
(repo convention). The `git commit` commands below omit it for brevity — add it to each.

---

### Task 1: Crate skeleton

**Files:**
- Create: `multosis/Cargo.toml`
- Create: `multosis/src/lib.rs`
- Modify: `Cargo.toml` (workspace root, line 2 — the `members` array)

- [ ] **Step 1: Create the crate manifest**

Create `multosis/Cargo.toml`:

```toml
[package]
name = "multosis"
version = "0.1.0"
edition = "2021"
description = "A multi-FX routing sequencer"
license = "GPL-3.0-or-later"

# Milestone 1a is library-only: plain rlib, no cdylib/bin, no nih-plug.
# Milestone 1b adds the [lib] crate-type, the [[bin]], and the plugin deps.

[dependencies]
serde = { version = "1.0", features = ["derive"] }

[dev-dependencies]
serde_json = "1.0"
```

- [ ] **Step 2: Create the crate root**

Create `multosis/src/lib.rs`:

```rust
//! `multosis` — a multi-FX routing sequencer.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md`.
//! Milestone 1a is the headless routing model: the grid, the wavefront
//! propagation engine, and the step clock. No GUI, no audio, no nih-plug.
```

- [ ] **Step 3: Register the crate in the workspace**

In the root `Cargo.toml`, add `"multosis"` to the `members` array on line 2. The array becomes:

```toml
members = ["wavetable-filter", "gs-meter", "gain-brain", "tinylimit", "satch", "pope-scope", "warp-zone", "six-pack", "imagine", "miff", "multosis", "tiny-skia-widgets", "tract-dsp", "xtask", "bench-suite"]
```

- [ ] **Step 4: Verify the crate builds**

Run: `cargo build -p multosis`
Expected: builds cleanly (an empty library crate compiles with no warnings).

- [ ] **Step 5: Commit**

```bash
git add multosis/Cargo.toml multosis/src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(multosis): add crate skeleton"
```

---

### Task 2: `Direction` type

**Files:**
- Create: `multosis/src/grid.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/grid.rs`:

```rust
//! The Multosis grid: cells, routing geometry, and grid-level operations.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.

use serde::{Deserialize, Serialize};

/// One of the 8 directions a cell can send a trigger.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum Direction {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_all_lists_eight_in_bit_order() {
        assert_eq!(Direction::ALL.len(), 8);
        for (i, dir) in Direction::ALL.iter().enumerate() {
            assert_eq!(dir.bit() as usize, i, "ALL not in bit order");
        }
    }

    #[test]
    fn direction_deltas_point_the_right_way() {
        // Row increases downward, column increases rightward.
        assert_eq!(Direction::N.delta(), (-1, 0));
        assert_eq!(Direction::E.delta(), (0, 1));
        assert_eq!(Direction::S.delta(), (1, 0));
        assert_eq!(Direction::W.delta(), (0, -1));
        assert_eq!(Direction::NE.delta(), (-1, 1));
        assert_eq!(Direction::SW.delta(), (1, -1));
    }
}
```

Add `pub mod grid;` to `multosis/src/lib.rs` (after the doc comment):

```rust
pub mod grid;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis direction`
Expected: build failure — `no associated item named ALL` / `no method named bit` / `no method named delta`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, immediately after the `Direction` enum:

```rust
impl Direction {
    /// All 8 directions, in `bit()` order.
    pub const ALL: [Direction; 8] = [
        Direction::N,
        Direction::NE,
        Direction::E,
        Direction::SE,
        Direction::S,
        Direction::SW,
        Direction::W,
        Direction::NW,
    ];

    /// The `(drow, dcol)` step this direction takes. Row increases downward,
    /// column increases rightward.
    pub fn delta(self) -> (i32, i32) {
        match self {
            Direction::N => (-1, 0),
            Direction::NE => (-1, 1),
            Direction::E => (0, 1),
            Direction::SE => (1, 1),
            Direction::S => (1, 0),
            Direction::SW => (1, -1),
            Direction::W => (0, -1),
            Direction::NW => (-1, -1),
        }
    }

    /// This direction's bit index (0..8) within a `Cell::sends` mask.
    pub fn bit(self) -> u8 {
        match self {
            Direction::N => 0,
            Direction::NE => 1,
            Direction::E => 2,
            Direction::SE => 3,
            Direction::S => 4,
            Direction::SW => 5,
            Direction::W => 6,
            Direction::NW => 7,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis direction`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs multosis/src/lib.rs
git commit -m "feat(multosis): add Direction type"
```

---

### Task 3: `Cell` type

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn cell_default_is_enabled_with_no_sends() {
        let c = Cell::default();
        assert!(c.enabled);
        assert!(!c.is_start);
        assert_eq!(c.sends, 0);
        assert!(!c.has_send());
    }

    #[test]
    fn cell_send_toggles_one_direction_at_a_time() {
        let mut c = Cell::default();
        c.set_send(Direction::E, true);
        assert!(c.sends_to(Direction::E));
        assert!(!c.sends_to(Direction::W));
        assert!(c.has_send());

        c.set_send(Direction::S, true);
        assert!(c.sends_to(Direction::E));
        assert!(c.sends_to(Direction::S));

        c.set_send(Direction::E, false);
        assert!(!c.sends_to(Direction::E));
        assert!(c.sends_to(Direction::S));

        c.toggle_send(Direction::S);
        assert!(!c.sends_to(Direction::S));
        assert!(!c.has_send());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis cell_`
Expected: build failure — `cannot find type Cell`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, after the `Direction` `impl` block:

```rust
/// One grid cell. `sends` is a bitmask over `Direction::bit()` positions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Cell {
    /// When false, a lit cell produces silence but still routes.
    pub enabled: bool,
    /// When true, this cell is armed by the `Initial` state.
    pub is_start: bool,
    /// Bitmask of the 8 send directions.
    pub sends: u8,
}

impl Default for Cell {
    /// An enabled, non-start cell with no sends.
    fn default() -> Self {
        Self {
            enabled: true,
            is_start: false,
            sends: 0,
        }
    }
}

impl Cell {
    /// Does this cell send in `dir`?
    pub fn sends_to(self, dir: Direction) -> bool {
        self.sends & (1u8 << dir.bit()) != 0
    }

    /// Turn the send in `dir` on or off.
    pub fn set_send(&mut self, dir: Direction, on: bool) {
        let bit = 1u8 << dir.bit();
        if on {
            self.sends |= bit;
        } else {
            self.sends &= !bit;
        }
    }

    /// Flip the send in `dir`.
    pub fn toggle_send(&mut self, dir: Direction) {
        self.sends ^= 1u8 << dir.bit();
    }

    /// True when the cell sends in at least one direction.
    pub fn has_send(self) -> bool {
        self.sends != 0
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis cell_`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add Cell type"
```

---

### Task 4: Grid dimensions and `LoopRegion`

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn grid_dimensions_are_16_by_32() {
        assert_eq!(ROWS, 16);
        assert_eq!(COLS, 32);
        assert_eq!(CELL_COUNT, 512);
    }

    #[test]
    fn loop_region_full_covers_the_whole_grid() {
        let r = LoopRegion::full();
        assert_eq!(r.row0, 0);
        assert_eq!(r.row1, ROWS - 1);
        assert_eq!(r.col0, 0);
        assert_eq!(r.col1, COLS - 1);
        assert!(r.contains(0, 0));
        assert!(r.contains(ROWS - 1, COLS - 1));
        assert_eq!(LoopRegion::default(), LoopRegion::full());
    }

    #[test]
    fn loop_region_contains_respects_bounds() {
        let r = LoopRegion {
            row0: 2,
            row1: 5,
            col0: 8,
            col1: 12,
        };
        assert!(r.contains(2, 8));
        assert!(r.contains(5, 12));
        assert!(r.contains(3, 10));
        assert!(!r.contains(1, 10)); // row above
        assert!(!r.contains(6, 10)); // row below
        assert!(!r.contains(3, 7)); // col left
        assert!(!r.contains(3, 13)); // col right
    }

    #[test]
    fn loop_region_normalized_clamps_and_orders() {
        // Out-of-range and inverted bounds get repaired.
        let bad = LoopRegion {
            row0: 9,
            row1: 4,
            col0: 99,
            col1: 1,
        };
        let n = bad.normalized();
        assert!(n.row0 <= n.row1);
        assert!(n.col0 <= n.col1);
        assert!(n.row1 < ROWS);
        assert!(n.col1 < COLS);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis loop_region`
Expected: build failure — `cannot find value ROWS` / `cannot find type LoopRegion`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, immediately after the `use serde::...` line (before the `Direction` enum):

```rust
/// Number of tracks (rows) in the grid.
pub const ROWS: usize = 16;
/// Number of steps (columns) in the grid.
pub const COLS: usize = 32;
/// Total cell count, `ROWS * COLS`.
pub const CELL_COUNT: usize = ROWS * COLS;
```

Add to `multosis/src/grid.rs`, after the `Cell` `impl` block:

```rust
/// A rectangular sub-region of the grid, inclusive on all four bounds.
/// Invariant: `row0 <= row1 < ROWS` and `col0 <= col1 < COLS`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LoopRegion {
    pub row0: usize,
    pub row1: usize,
    pub col0: usize,
    pub col1: usize,
}

impl LoopRegion {
    /// The region covering the entire grid.
    pub fn full() -> Self {
        Self {
            row0: 0,
            row1: ROWS - 1,
            col0: 0,
            col1: COLS - 1,
        }
    }

    /// Is `(row, col)` inside the region?
    pub fn contains(self, row: usize, col: usize) -> bool {
        row >= self.row0 && row <= self.row1 && col >= self.col0 && col <= self.col1
    }

    /// A copy with bounds clamped into the grid and ordered (`row0 <= row1`,
    /// `col0 <= col1`). Repairs a hand-edited or corrupt deserialized region.
    pub fn normalized(self) -> Self {
        let mut r0 = self.row0.min(ROWS - 1);
        let mut r1 = self.row1.min(ROWS - 1);
        let mut c0 = self.col0.min(COLS - 1);
        let mut c1 = self.col1.min(COLS - 1);
        if r0 > r1 {
            std::mem::swap(&mut r0, &mut r1);
        }
        if c0 > c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
        Self {
            row0: r0,
            row1: r1,
            col0: c0,
            col1: c1,
        }
    }
}

impl Default for LoopRegion {
    fn default() -> Self {
        Self::full()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(loop_region) + test(grid_dimensions)'`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add grid dimensions and LoopRegion"
```

---

### Task 5: `Grid` type and `default_routing`

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn grid_index_is_row_major() {
        assert_eq!(Grid::index(0, 0), 0);
        assert_eq!(Grid::index(0, 31), 31);
        assert_eq!(Grid::index(1, 0), 32);
        assert_eq!(Grid::index(15, 31), 511);
    }

    #[test]
    fn default_routing_sends_east_everywhere() {
        let g = Grid::default_routing();
        for r in 0..ROWS {
            for c in 0..COLS {
                let cell = g.cell(r, c);
                assert!(cell.enabled, "cell ({r},{c}) should be enabled");
                assert!(cell.sends_to(Direction::E), "cell ({r},{c}) should send E");
                // East is the only send.
                assert_eq!(cell.sends, 1u8 << Direction::E.bit());
            }
        }
    }

    #[test]
    fn default_routing_starts_the_left_column() {
        let g = Grid::default_routing();
        for r in 0..ROWS {
            assert!(g.cell(r, 0).is_start, "left-column cell ({r},0) is a start");
            for c in 1..COLS {
                assert!(!g.cell(r, c).is_start, "cell ({r},{c}) is not a start");
            }
        }
    }

    #[test]
    fn default_routing_loop_region_is_full() {
        assert_eq!(Grid::default_routing().loop_region, LoopRegion::full());
        assert_eq!(Grid::default(), Grid::default_routing());
    }

    #[test]
    fn cell_mut_writes_through() {
        let mut g = Grid::default_routing();
        g.cell_mut(3, 7).enabled = false;
        assert!(!g.cell(3, 7).enabled);
        assert!(g.cell(3, 8).enabled);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis -E 'test(grid_index) + test(default_routing) + test(cell_mut)'`
Expected: build failure — `cannot find type Grid`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, after the `LoopRegion` code:

```rust
/// The full 16×32 routing grid plus its loop region. `Copy` (~1.5 KB) so it
/// crosses the GUI/audio boundary cheaply — the same fixed-capacity approach
/// as the MSEG widget's `MsegData`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Grid {
    /// Row-major cells, indexed `row * COLS + col`.
    pub cells: [Cell; CELL_COUNT],
    pub loop_region: LoopRegion,
}

impl Grid {
    /// The flat index of `(row, col)`.
    #[inline]
    pub fn index(row: usize, col: usize) -> usize {
        row * COLS + col
    }

    /// Shared access to the cell at `(row, col)`.
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[Self::index(row, col)]
    }

    /// Mutable access to the cell at `(row, col)`.
    pub fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.cells[Self::index(row, col)]
    }

    /// The default grid: every cell enabled and sending only East; the left
    /// column (`col == 0`) all start cells; loop region the full grid.
    pub fn default_routing() -> Self {
        let mut g = Grid {
            cells: [Cell::default(); CELL_COUNT],
            loop_region: LoopRegion::full(),
        };
        for r in 0..ROWS {
            for c in 0..COLS {
                let cell = g.cell_mut(r, c);
                cell.enabled = true;
                cell.is_start = c == 0;
                cell.sends = 0;
                cell.set_send(Direction::E, true);
            }
        }
        g
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::default_routing()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(grid_index) + test(default_routing) + test(cell_mut)'`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add Grid type and default_routing"
```

---

### Task 6: Routing geometry — `next_cell`

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn next_cell_moves_inside_the_grid() {
        let g = Grid::default_routing(); // loop region is full
        assert_eq!(g.next_cell(5, 5, Direction::E), (5, 6));
        assert_eq!(g.next_cell(5, 5, Direction::W), (5, 4));
        assert_eq!(g.next_cell(5, 5, Direction::N), (4, 5));
        assert_eq!(g.next_cell(5, 5, Direction::S), (6, 5));
        assert_eq!(g.next_cell(5, 5, Direction::SE), (6, 6));
    }

    #[test]
    fn next_cell_wraps_every_grid_edge() {
        let g = Grid::default_routing();
        assert_eq!(g.next_cell(5, COLS - 1, Direction::E), (5, 0));
        assert_eq!(g.next_cell(5, 0, Direction::W), (5, COLS - 1));
        assert_eq!(g.next_cell(0, 5, Direction::N), (ROWS - 1, 5));
        assert_eq!(g.next_cell(ROWS - 1, 5, Direction::S), (0, 5));
        // Corner diagonal wraps both axes at once.
        assert_eq!(g.next_cell(0, 0, Direction::NW), (ROWS - 1, COLS - 1));
        assert_eq!(
            g.next_cell(ROWS - 1, COLS - 1, Direction::SE),
            (0, 0)
        );
    }

    #[test]
    fn next_cell_wraps_within_the_loop_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 11,
            col0: 8,
            col1: 23,
        };
        // A cell on the region's right edge sending E wraps to the region's
        // left edge — NOT to the grid edge.
        assert_eq!(g.next_cell(6, 23, Direction::E), (6, 8));
        // Region's bottom edge sending S wraps to the region's top edge.
        assert_eq!(g.next_cell(11, 15, Direction::S), (4, 15));
        // Region's top-left corner sending NW wraps to its bottom-right corner.
        assert_eq!(g.next_cell(4, 8, Direction::NW), (11, 23));
        // Interior of the region moves normally.
        assert_eq!(g.next_cell(6, 15, Direction::E), (6, 16));
    }

    #[test]
    fn next_cell_outside_the_region_wraps_the_full_grid() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 11,
            col0: 8,
            col1: 23,
        };
        // (2, 31) is outside the region: E wraps to the grid's left edge.
        assert_eq!(g.next_cell(2, COLS - 1, Direction::E), (2, 0));
    }

    #[test]
    fn no_send_from_a_region_cell_ever_escapes_the_region() {
        let mut g = Grid::default_routing();
        let lr = LoopRegion {
            row0: 3,
            row1: 9,
            col0: 5,
            col1: 20,
        };
        g.loop_region = lr;
        for r in lr.row0..=lr.row1 {
            for c in lr.col0..=lr.col1 {
                for dir in Direction::ALL {
                    let (nr, nc) = g.next_cell(r, c, dir);
                    assert!(
                        lr.contains(nr, nc),
                        "send {dir:?} from ({r},{c}) escaped to ({nr},{nc})"
                    );
                }
            }
        }
    }

    #[test]
    fn next_cell_in_a_1x1_region_loops_every_direction_onto_itself() {
        // A 1×1 loop region: the single cell has nowhere to wrap to, so every
        // one of the 8 directions must resolve back onto the cell itself.
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 7,
            row1: 7,
            col0: 13,
            col1: 13,
        };
        for dir in Direction::ALL {
            assert_eq!(
                g.next_cell(7, 13, dir),
                (7, 13),
                "direction {dir:?} did not loop onto itself"
            );
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis next_cell`
Expected: build failure — `no method named next_cell`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, after the `impl Default for Grid` block:

```rust
/// Wrap `v` into the inclusive range `[lo, hi]`.
fn wrap(v: i32, lo: usize, hi: usize) -> usize {
    let lo = lo as i32;
    let span = hi as i32 - lo + 1;
    (lo + (v - lo).rem_euclid(span)) as usize
}

impl Grid {
    /// Where a send in `dir` from `(row, col)` lands. If `(row, col)` is inside
    /// the loop region the result wraps within the region; otherwise it wraps
    /// within the full grid. This is the only place loop-region containment is
    /// enforced.
    pub fn next_cell(&self, row: usize, col: usize, dir: Direction) -> (usize, usize) {
        let (dr, dc) = dir.delta();
        let nr = row as i32 + dr;
        let nc = col as i32 + dc;
        if self.loop_region.contains(row, col) {
            let lr = self.loop_region;
            (wrap(nr, lr.row0, lr.row1), wrap(nc, lr.col0, lr.col1))
        } else {
            (wrap(nr, 0, ROWS - 1), wrap(nc, 0, COLS - 1))
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(next_cell) + test(no_send)'`
Expected: PASS — 6 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add next_cell routing geometry"
```

---

### Task 7: `reset_routing` and `reinit_activations`

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn reset_routing_restores_east_and_keeps_activations() {
        let mut g = Grid::default_routing();
        // Scramble routing and activations.
        g.cell_mut(2, 2).sends = 0b1010_1010;
        g.cell_mut(2, 2).enabled = false;
        g.cell_mut(2, 2).is_start = true;
        g.cell_mut(7, 9).sends = 0;

        g.reset_routing();

        // Every cell sends East only again.
        for r in 0..ROWS {
            for c in 0..COLS {
                assert_eq!(g.cell(r, c).sends, 1u8 << Direction::E.bit());
            }
        }
        // Activations are untouched.
        assert!(!g.cell(2, 2).enabled);
        assert!(g.cell(2, 2).is_start);
    }

    #[test]
    fn reinit_activations_restores_defaults_and_keeps_routing() {
        let mut g = Grid::default_routing();
        g.cell_mut(4, 4).sends = 0b0001_1000;
        g.cell_mut(4, 4).enabled = false;
        g.cell_mut(0, 5).is_start = true; // a stray start away from col 0
        g.cell_mut(3, 0).is_start = false; // clear a default start

        g.reinit_activations();

        // Activations back to default: all enabled, left column the starts.
        for r in 0..ROWS {
            for c in 0..COLS {
                assert!(g.cell(r, c).enabled);
                assert_eq!(g.cell(r, c).is_start, c == 0);
            }
        }
        // Routing is untouched.
        assert_eq!(g.cell(4, 4).sends, 0b0001_1000);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis -E 'test(reset_routing) + test(reinit_activations)'`
Expected: build failure — `no method named reset_routing`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, inside a new `impl Grid` block after the `next_cell` block:

```rust
impl Grid {
    /// Restore default sends (East only) on every cell; leave `enabled` and
    /// `is_start` untouched. Recovers from user-created dead ends.
    pub fn reset_routing(&mut self) {
        for cell in self.cells.iter_mut() {
            cell.sends = 0;
            cell.set_send(Direction::E, true);
        }
    }

    /// Restore default activations: every cell `enabled`, the left column the
    /// start cells. Leaves `sends` untouched.
    pub fn reinit_activations(&mut self) {
        for r in 0..ROWS {
            for c in 0..COLS {
                let cell = self.cell_mut(r, c);
                cell.enabled = true;
                cell.is_start = c == 0;
            }
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(reset_routing) + test(reinit_activations)'`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add reset_routing and reinit_activations"
```

---

### Task 8: Grid serialization and `sanitize`

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn grid_json_round_trips() {
        let mut g = Grid::default_routing();
        g.cell_mut(3, 9).sends = 0b1100_0011;
        g.cell_mut(3, 9).enabled = false;
        g.cell_mut(10, 0).is_start = false;
        g.loop_region = LoopRegion {
            row0: 2,
            row1: 12,
            col0: 4,
            col1: 28,
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: Grid = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn sanitize_repairs_a_bad_loop_region() {
        let mut g = Grid::default_routing();
        // Inverted, out-of-range bounds — as a corrupt blob might carry.
        g.loop_region = LoopRegion {
            row0: 14,
            row1: 3,
            col0: 99,
            col1: 0,
        };
        g.sanitize();
        let lr = g.loop_region;
        assert!(lr.row0 <= lr.row1 && lr.row1 < ROWS);
        assert!(lr.col0 <= lr.col1 && lr.col1 < COLS);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis -E 'test(grid_json) + test(sanitize)'`
Expected: build failure — `no method named sanitize`. (`grid_json_round_trips` would compile — the serde derives are already present — but the run fails because `sanitize` does not.)

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, inside the `impl Grid` block from Task 7 (after `reinit_activations`):

```rust
    /// Repair structural invariants after loading a possibly hand-edited or
    /// corrupt grid: clamp and order the loop region. Called by the plugin
    /// after deserializing persisted state (Milestone 1b).
    pub fn sanitize(&mut self) {
        self.loop_region = self.loop_region.normalized();
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(grid_json) + test(sanitize)'`
Expected: PASS — 2 tests. (`grid_json_round_trips` confirms the 512-element `[Cell; 512]` array serializes correctly via serde's const-generic array support.)

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add Grid serde round-trip guard and sanitize"
```

---

### Task 9: Region copy/paste

**Files:**
- Create: `multosis/src/region.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/region.rs`:

```rust
//! Copy and paste of the block of cells under the loop region.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.3.

use crate::grid::{Cell, Grid, COLS, ROWS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Direction, LoopRegion};

    #[test]
    fn copy_region_snapshots_the_loop_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 1,
            row1: 3,
            col0: 5,
            col1: 9,
        };
        g.cell_mut(1, 5).enabled = false;
        let snap = g.copy_region();
        assert_eq!(snap.rows, 3);
        assert_eq!(snap.cols, 5);
        assert_eq!(snap.cells.len(), 15);
        // First snapshot cell is the region's top-left, (1,5).
        assert!(!snap.cells[0].enabled);
    }

    #[test]
    fn paste_region_writes_at_the_loop_region_top_left() {
        let mut g = Grid::default_routing();
        // Copy a 2×2 block from rows 0..2, cols 0..2 with a marker.
        g.loop_region = LoopRegion {
            row0: 0,
            row1: 1,
            col0: 0,
            col1: 1,
        };
        g.cell_mut(0, 0).sends = 0b1111_1111;
        let snap = g.copy_region();
        // Move the loop region and paste.
        g.loop_region = LoopRegion {
            row0: 10,
            row1: 11,
            col0: 20,
            col1: 21,
        };
        g.paste_region(&snap);
        assert_eq!(g.cell(10, 20).sends, 0b1111_1111);
        // A cell outside the pasted block is unchanged (still East-only).
        assert_eq!(g.cell(12, 22).sends, 1u8 << Direction::E.bit());
    }

    #[test]
    fn paste_region_truncates_on_overflow() {
        let mut g = Grid::default_routing();
        // Copy a 3×3 block.
        g.loop_region = LoopRegion {
            row0: 0,
            row1: 2,
            col0: 0,
            col1: 2,
        };
        g.cell_mut(0, 0).enabled = false;
        let snap = g.copy_region();
        // Anchor the paste so only a 1×1 corner fits inside the grid.
        g.loop_region = LoopRegion {
            row0: ROWS - 1,
            row1: ROWS - 1,
            col0: COLS - 1,
            col1: COLS - 1,
        };
        g.paste_region(&snap); // must not panic
        // The one in-bounds cell received the snapshot's top-left.
        assert!(!g.cell(ROWS - 1, COLS - 1).enabled);
    }
}
```

Add `pub mod region;` to `multosis/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis region`
Expected: build failure — `no method named copy_region` / `cannot find type ... RegionSnapshot`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/region.rs`, after the `use` line (before the test module):

```rust
/// A rectangular block of cells lifted from a grid's loop region. A
/// GUI-thread clipboard value — it owns a `Vec` and never crosses to the
/// audio thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionSnapshot {
    pub rows: usize,
    pub cols: usize,
    /// `rows * cols` cells, row-major.
    pub cells: Vec<Cell>,
}

impl Grid {
    /// Snapshot the cells currently covered by the loop region.
    pub fn copy_region(&self) -> RegionSnapshot {
        let lr = self.loop_region;
        let rows = lr.row1 - lr.row0 + 1;
        let cols = lr.col1 - lr.col0 + 1;
        let mut cells = Vec::with_capacity(rows * cols);
        for r in lr.row0..=lr.row1 {
            for c in lr.col0..=lr.col1 {
                cells.push(*self.cell(r, c));
            }
        }
        RegionSnapshot { rows, cols, cells }
    }

    /// Paste `snap` anchored at the loop region's top-left corner. Cells that
    /// would fall outside the grid are truncated (skipped).
    pub fn paste_region(&mut self, snap: &RegionSnapshot) {
        let lr = self.loop_region;
        for sr in 0..snap.rows {
            for sc in 0..snap.cols {
                let dr = lr.row0 + sr;
                let dc = lr.col0 + sc;
                if dr < ROWS && dc < COLS {
                    *self.cell_mut(dr, dc) = snap.cells[sr * snap.cols + sc];
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis region`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/region.rs multosis/src/lib.rs
git commit -m "feat(multosis): add region copy/paste with truncation"
```

---

### Task 10: PRNG and `randomize_activations`

**Files:**
- Create: `multosis/src/randomize.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/randomize.rs`:

```rust
//! Deterministic randomization of cell activations and routing.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.3.

use crate::grid::Grid;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::LoopRegion;

    #[test]
    fn randomize_activations_is_deterministic() {
        let mut a = Grid::default_routing();
        let mut b = Grid::default_routing();
        randomize_activations(&mut a, 4242);
        randomize_activations(&mut b, 4242);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_activations_differs_by_seed() {
        let mut a = Grid::default_routing();
        let mut b = Grid::default_routing();
        randomize_activations(&mut a, 1);
        randomize_activations(&mut b, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_activations_only_touches_enabled_inside_the_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 6,
            col0: 10,
            col1: 14,
        };
        randomize_activations(&mut g, 99);
        for r in 0..crate::grid::ROWS {
            for c in 0..crate::grid::COLS {
                let cell = g.cell(r, c);
                // sends and is_start are never altered.
                assert_eq!(cell.sends, 1u8 << crate::grid::Direction::E.bit());
                assert_eq!(cell.is_start, c == 0);
                // Outside the region, enabled is left at its default (true).
                if !g.loop_region.contains(r, c) {
                    assert!(cell.enabled, "cell ({r},{c}) outside region changed");
                }
            }
        }
    }
}
```

Add `pub mod randomize;` to `multosis/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis randomize_activations`
Expected: build failure — `cannot find function randomize_activations`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/randomize.rs`, after the `use crate::grid::Grid;` line:

```rust
/// Deterministic xorshift32 PRNG — no dependency, seeded per call. Matches the
/// MSEG widget's `randomize` PRNG.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // xorshift cannot leave the all-zero state, so map seed 0 to a fixed
        // non-zero constant.
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn bool(&mut self) -> bool {
        self.next_u32() & 1 == 1
    }
}

/// Randomize `enabled` for every cell inside the loop region. Deterministic in
/// `seed`. Leaves `sends`, `is_start`, and cells outside the region untouched.
pub fn randomize_activations(grid: &mut Grid, seed: u32) {
    let mut rng = Rng::new(seed);
    let lr = grid.loop_region;
    for r in lr.row0..=lr.row1 {
        for c in lr.col0..=lr.col1 {
            grid.cell_mut(r, c).enabled = rng.bool();
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis randomize_activations`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/randomize.rs multosis/src/lib.rs
git commit -m "feat(multosis): add PRNG and randomize_activations"
```

---

### Task 11: `randomize_routing` (no dead ends)

**Files:**
- Modify: `multosis/src/randomize.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/randomize.rs`:

```rust
    #[test]
    fn randomize_routing_is_deterministic() {
        let mut a = Grid::default_routing();
        let mut b = Grid::default_routing();
        randomize_routing(&mut a, 777);
        randomize_routing(&mut b, 777);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_routing_never_creates_a_dead_end() {
        // Every randomized cell must keep at least one send, for every seed.
        for seed in 0..200u32 {
            let mut g = Grid::default_routing();
            randomize_routing(&mut g, seed);
            for r in 0..crate::grid::ROWS {
                for c in 0..crate::grid::COLS {
                    assert!(
                        g.cell(r, c).has_send(),
                        "seed {seed}: cell ({r},{c}) is a dead end"
                    );
                }
            }
        }
    }

    #[test]
    fn randomize_routing_only_touches_sends_inside_the_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 2,
            row1: 4,
            col0: 6,
            col1: 9,
        };
        randomize_routing(&mut g, 55);
        for r in 0..crate::grid::ROWS {
            for c in 0..crate::grid::COLS {
                let cell = g.cell(r, c);
                assert!(cell.enabled);
                assert_eq!(cell.is_start, c == 0);
                if !g.loop_region.contains(r, c) {
                    // Outside the region routing is the default East-only.
                    assert_eq!(cell.sends, 1u8 << crate::grid::Direction::E.bit());
                }
            }
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis randomize_routing`
Expected: build failure — `cannot find function randomize_routing`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/randomize.rs`, after the `randomize_activations` function:

```rust
/// Randomize `sends` for every cell inside the loop region, guaranteeing no
/// dead ends — every randomized cell keeps at least one send. Deterministic in
/// `seed`. Leaves `enabled`, `is_start`, and cells outside the region
/// untouched.
pub fn randomize_routing(grid: &mut Grid, seed: u32) {
    let mut rng = Rng::new(seed);
    let lr = grid.loop_region;
    for r in lr.row0..=lr.row1 {
        for c in lr.col0..=lr.col1 {
            let mut sends = (rng.next_u32() & 0xFF) as u8;
            if sends == 0 {
                // No dead ends: force at least one direction.
                let bit = (rng.next_u32() % 8) as u8;
                sends = 1u8 << bit;
            }
            grid.cell_mut(r, c).sends = sends;
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis randomize_routing`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/randomize.rs
git commit -m "feat(multosis): add randomize_routing with no dead ends"
```

---

### Task 12: `Wavefront` type

**Files:**
- Create: `multosis/src/propagation.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/propagation.rs`:

```rust
//! The wavefront propagation engine: the lit-set, the per-step routing rule,
//! and the Initial/Running/Stopped lifecycle.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §5.

use crate::grid::{Direction, Grid, CELL_COUNT, COLS, ROWS};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavefront_empty_has_nothing_lit() {
        let wf = Wavefront::empty();
        assert!(wf.is_empty());
        assert_eq!(wf.count(), 0);
        assert!(!wf.is_lit(0, 0));
    }

    #[test]
    fn wavefront_set_and_read_back() {
        let mut wf = Wavefront::empty();
        wf.set(3, 7, true);
        wf.set(10, 0, true);
        assert!(wf.is_lit(3, 7));
        assert!(wf.is_lit(10, 0));
        assert!(!wf.is_lit(3, 8));
        assert!(!wf.is_empty());
        assert_eq!(wf.count(), 2);

        wf.set(3, 7, false);
        assert!(!wf.is_lit(3, 7));
        assert_eq!(wf.count(), 1);
    }
}
```

Add `pub mod propagation;` to `multosis/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis wavefront`
Expected: build failure — `cannot find type Wavefront`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/propagation.rs`, after the `use` line (before the test module):

```rust
/// The set of currently-lit cells. `Copy` so it crosses the GUI/audio
/// boundary cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wavefront {
    lit: [bool; CELL_COUNT],
}

impl Wavefront {
    /// An empty wavefront — no cells lit.
    pub fn empty() -> Self {
        Self {
            lit: [false; CELL_COUNT],
        }
    }

    /// Is the cell at `(row, col)` lit?
    pub fn is_lit(&self, row: usize, col: usize) -> bool {
        self.lit[Grid::index(row, col)]
    }

    /// Light or clear the cell at `(row, col)`.
    pub fn set(&mut self, row: usize, col: usize, on: bool) {
        self.lit[Grid::index(row, col)] = on;
    }

    /// Are no cells lit?
    pub fn is_empty(&self) -> bool {
        !self.lit.iter().any(|&l| l)
    }

    /// How many cells are lit.
    pub fn count(&self) -> usize {
        self.lit.iter().filter(|&&l| l).count()
    }
}

impl Default for Wavefront {
    fn default() -> Self {
        Self::empty()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis wavefront`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/propagation.rs multosis/src/lib.rs
git commit -m "feat(multosis): add Wavefront type"
```

---

### Task 13: `step_manual` — the per-step routing rule

**Files:**
- Modify: `multosis/src/propagation.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/propagation.rs`:

```rust
    use crate::grid::{Cell, LoopRegion};

    /// A grid where every cell sends nowhere — a blank slate for tests.
    fn blank_grid() -> Grid {
        let mut g = Grid::default_routing();
        for cell in g.cells.iter_mut() {
            *cell = Cell::default(); // enabled, not start, no sends
        }
        g.loop_region = LoopRegion::full();
        g
    }

    #[test]
    fn step_manual_follows_a_single_send() {
        let mut g = blank_grid();
        g.cell_mut(5, 5).set_send(Direction::E, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1);
        assert!(next.is_lit(5, 6));
    }

    #[test]
    fn step_manual_splits_the_wavefront() {
        let mut g = blank_grid();
        g.cell_mut(5, 5).set_send(Direction::E, true);
        g.cell_mut(5, 5).set_send(Direction::S, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 2);
        assert!(next.is_lit(5, 6));
        assert!(next.is_lit(6, 5));
    }

    #[test]
    fn step_manual_merges_two_cells_onto_one() {
        let mut g = blank_grid();
        // (5,5) sends E and (5,7) sends W — both land on (5,6).
        g.cell_mut(5, 5).set_send(Direction::E, true);
        g.cell_mut(5, 7).set_send(Direction::W, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        wf.set(5, 7, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1, "merge collapses onto one cell");
        assert!(next.is_lit(5, 6));
    }

    #[test]
    fn step_manual_with_no_sends_yields_empty() {
        let g = blank_grid(); // no cell sends anywhere
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        assert!(step_manual(&g, &wf).is_empty());
    }

    #[test]
    fn step_manual_on_empty_input_is_empty() {
        let g = Grid::default_routing();
        assert!(step_manual(&g, &Wavefront::empty()).is_empty());
    }

    #[test]
    fn step_manual_loops_within_a_one_cell_region() {
        // A 1×1 loop region: a cell sending E wraps onto itself.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 4,
            col0: 9,
            col1: 9,
        };
        g.cell_mut(4, 9).set_send(Direction::E, true);
        let mut wf = Wavefront::empty();
        wf.set(4, 9, true);
        let next = step_manual(&g, &wf);
        assert!(next.is_lit(4, 9), "1×1 region send loops back onto itself");
        assert_eq!(next.count(), 1);
    }

    #[test]
    fn step_manual_1x1_region_collapses_every_send_onto_self() {
        // In a 1×1 region a cell sending in all 8 directions still lights only
        // itself — every send wraps back onto the one cell.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 8,
            row1: 8,
            col0: 2,
            col1: 2,
        };
        for dir in Direction::ALL {
            g.cell_mut(8, 2).set_send(dir, true);
        }
        let mut wf = Wavefront::empty();
        wf.set(8, 2, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1);
        assert!(next.is_lit(8, 2));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis step_manual`
Expected: build failure — `cannot find function step_manual`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/propagation.rs`, after the `impl Default for Wavefront` block:

```rust
/// Compute the next wavefront from `current` by following every lit cell's
/// send directions through `grid.next_cell`. Pure — this is the seam a future
/// Game-of-Life routing mode plugs into (see the design doc §1.3).
pub fn step_manual(grid: &Grid, current: &Wavefront) -> Wavefront {
    let mut next = Wavefront::empty();
    for r in 0..ROWS {
        for c in 0..COLS {
            if !current.is_lit(r, c) {
                continue;
            }
            let cell = *grid.cell(r, c);
            for dir in Direction::ALL {
                if cell.sends_to(dir) {
                    let (nr, nc) = grid.next_cell(r, c, dir);
                    next.set(nr, nc, true);
                }
            }
        }
    }
    next
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis step_manual`
Expected: PASS — 7 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/propagation.rs
git commit -m "feat(multosis): add step_manual routing rule"
```

---

### Task 14: `Propagator` — the lifecycle state machine

**Files:**
- Modify: `multosis/src/propagation.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/propagation.rs`:

```rust
    /// A grid with exactly one start cell at `(row, col)` and no sends — it
    /// dead-ends one tick after arming.
    fn one_start_no_sends(row: usize, col: usize) -> Grid {
        let mut g = blank_grid();
        g.cell_mut(row, col).is_start = true;
        g
    }

    #[test]
    fn propagator_new_is_initial_and_empty() {
        let p = Propagator::new();
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
        assert_eq!(p.step, 0);
    }

    #[test]
    fn tick_from_initial_arms_every_start_cell() {
        let g = Grid::default_routing(); // left column = 16 start cells
        let mut p = Propagator::new();
        p.tick(&g, true);
        assert_eq!(p.state, SequenceState::Running);
        assert_eq!(p.wavefront.count(), ROWS);
        for r in 0..ROWS {
            assert!(p.wavefront.is_lit(r, 0));
        }
    }

    #[test]
    fn tick_from_initial_with_no_start_cells_stays_initial() {
        let g = blank_grid(); // nothing is a start cell
        let mut p = Propagator::new();
        p.tick(&g, true);
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
    }

    #[test]
    fn tick_running_propagates_the_wavefront() {
        let g = Grid::default_routing(); // every cell sends East
        let mut p = Propagator::new();
        p.tick(&g, true); // arm: column 0
        p.tick(&g, true); // propagate East: column 1
        assert_eq!(p.state, SequenceState::Running);
        assert_eq!(p.step, 1);
        for r in 0..ROWS {
            assert!(p.wavefront.is_lit(r, 1));
            assert!(!p.wavefront.is_lit(r, 0));
        }
    }

    #[test]
    fn dead_end_with_auto_restart_returns_to_initial() {
        let g = one_start_no_sends(0, 0);
        let mut p = Propagator::new();
        p.tick(&g, true); // arm (0,0) -> Running
        assert_eq!(p.state, SequenceState::Running);
        p.tick(&g, true); // (0,0) sends nowhere -> dead end
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
        // The next tick re-arms the start cells.
        p.tick(&g, true);
        assert_eq!(p.state, SequenceState::Running);
        assert!(p.wavefront.is_lit(0, 0));
    }

    #[test]
    fn dead_end_without_auto_restart_stops() {
        let g = one_start_no_sends(2, 4);
        let mut p = Propagator::new();
        p.tick(&g, false); // arm -> Running
        p.tick(&g, false); // dead end
        assert_eq!(p.state, SequenceState::Stopped);
        assert!(p.wavefront.is_empty());
        // Further ticks are no-ops.
        p.tick(&g, false);
        assert_eq!(p.state, SequenceState::Stopped);
    }

    #[test]
    fn reset_returns_a_stopped_propagator_to_initial() {
        let g = one_start_no_sends(2, 4);
        let mut p = Propagator::new();
        p.tick(&g, false);
        p.tick(&g, false); // -> Stopped
        p.reset();
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
        p.tick(&g, false); // arms again
        assert_eq!(p.state, SequenceState::Running);
    }

    #[test]
    fn propagator_loops_forever_in_a_1x1_self_routing_region() {
        // A 1×1 loop region whose single cell is a start and sends East: East
        // wraps onto itself, so the wavefront never dead-ends.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 5,
            row1: 5,
            col0: 6,
            col1: 6,
        };
        g.cell_mut(5, 6).is_start = true;
        g.cell_mut(5, 6).set_send(Direction::E, true);
        let mut p = Propagator::new();
        p.tick(&g, false); // arm
        for _ in 0..50 {
            p.tick(&g, false);
            assert_eq!(p.state, SequenceState::Running);
            assert_eq!(p.wavefront.count(), 1);
            assert!(p.wavefront.is_lit(5, 6));
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis -E 'test(propagator) + test(tick) + test(dead_end) + test(reset_returns)'`
Expected: build failure — `cannot find type Propagator` / `SequenceState`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/propagation.rs`, after the `step_manual` function:

```rust
/// Lifecycle state of the sequence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SequenceState {
    /// Fresh or reset — the next tick arms the start cells.
    Initial,
    /// A non-empty wavefront is propagating.
    Running,
    /// A dead end reached with `auto_restart` off — silent until `reset()`.
    Stopped,
}

/// Drives the wavefront through its Initial/Running/Stopped lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Propagator {
    pub state: SequenceState,
    pub wavefront: Wavefront,
    /// Steps since the wavefront was last armed — for display.
    pub step: u64,
}

impl Propagator {
    /// A fresh propagator in the `Initial` state.
    pub fn new() -> Self {
        Self {
            state: SequenceState::Initial,
            wavefront: Wavefront::empty(),
            step: 0,
        }
    }

    /// Clear the wavefront and return to `Initial`. Triggered by the manual
    /// Reset button and the host transport's stopped→playing edge.
    pub fn reset(&mut self) {
        self.state = SequenceState::Initial;
        self.wavefront = Wavefront::empty();
        self.step = 0;
    }

    /// Advance the sequence one step. `auto_restart` governs the dead-end
    /// response (design doc §5.1).
    pub fn tick(&mut self, grid: &Grid, auto_restart: bool) {
        match self.state {
            SequenceState::Initial => {
                // Arm every start cell.
                let mut wf = Wavefront::empty();
                for r in 0..ROWS {
                    for c in 0..COLS {
                        if grid.cell(r, c).is_start {
                            wf.set(r, c, true);
                        }
                    }
                }
                self.wavefront = wf;
                self.step = 0;
                if !wf.is_empty() {
                    self.state = SequenceState::Running;
                }
                // No start cells -> stay Initial with an empty wavefront.
            }
            SequenceState::Running => {
                let next = step_manual(grid, &self.wavefront);
                if next.is_empty() {
                    // Dead end: every lit cell routed nowhere.
                    self.wavefront = Wavefront::empty();
                    if auto_restart {
                        self.state = SequenceState::Initial;
                        self.step = 0;
                    } else {
                        self.state = SequenceState::Stopped;
                    }
                } else {
                    self.wavefront = next;
                    self.step += 1;
                }
            }
            SequenceState::Stopped => {
                // No-op until reset().
            }
        }
    }
}

impl Default for Propagator {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(propagator) + test(tick) + test(dead_end) + test(reset_returns)'`
Expected: PASS — 8 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/propagation.rs
git commit -m "feat(multosis): add Propagator lifecycle state machine"
```

---

### Task 15: `Speed` and `samples_per_step`

**Files:**
- Create: `multosis/src/clock.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/clock.rs`:

```rust
//! The tempo-synced step clock that drives wavefront propagation.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §5.2.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_all_lists_six_divisions() {
        assert_eq!(Speed::ALL.len(), 6);
    }

    #[test]
    fn speed_quarter_notes_are_correct() {
        assert_eq!(Speed::Div32.quarter_notes(), 0.125);
        assert_eq!(Speed::Div16.quarter_notes(), 0.25);
        assert_eq!(Speed::Div8.quarter_notes(), 0.5);
        assert_eq!(Speed::Div4.quarter_notes(), 1.0);
        assert_eq!(Speed::Div2.quarter_notes(), 2.0);
        assert_eq!(Speed::Div1.quarter_notes(), 4.0);
    }

    #[test]
    fn samples_per_step_at_120_bpm() {
        // 120 BPM -> 0.5 s per quarter note. A 1/16 step is 0.25 quarter
        // notes -> 0.125 s -> 6000 samples at 48 kHz.
        let n = samples_per_step(Speed::Div16, 120.0, 48_000.0);
        assert!((n - 6000.0).abs() < 1e-6, "got {n}");
        // A 1/4 step at 120 BPM is 0.5 s -> 24000 samples.
        let q = samples_per_step(Speed::Div4, 120.0, 48_000.0);
        assert!((q - 24_000.0).abs() < 1e-6, "got {q}");
    }
}
```

Add `pub mod clock;` to `multosis/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis -E 'test(speed_) + test(samples_per_step)'`
Expected: build failure — `cannot find type Speed`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/clock.rs`, at the top of the file (after the doc comment, before the test module):

```rust
/// How fast the wavefront advances — a musical note division.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speed {
    /// 1/32 note.
    Div32,
    /// 1/16 note.
    Div16,
    /// 1/8 note.
    Div8,
    /// 1/4 note.
    Div4,
    /// 1/2 note.
    Div2,
    /// Whole note.
    Div1,
}

impl Speed {
    /// All six speeds, fastest to slowest.
    pub const ALL: [Speed; 6] = [
        Speed::Div32,
        Speed::Div16,
        Speed::Div8,
        Speed::Div4,
        Speed::Div2,
        Speed::Div1,
    ];

    /// The length of one step in quarter notes (a 1/16 note is 0.25 quarter
    /// notes; a whole note is 4.0).
    pub fn quarter_notes(self) -> f64 {
        match self {
            Speed::Div32 => 0.125,
            Speed::Div16 => 0.25,
            Speed::Div8 => 0.5,
            Speed::Div4 => 1.0,
            Speed::Div2 => 2.0,
            Speed::Div1 => 4.0,
        }
    }
}

/// Samples spanning one step at the given speed, tempo, and sample rate.
/// `bpm` is quarter notes per minute.
pub fn samples_per_step(speed: Speed, bpm: f64, sample_rate: f64) -> f64 {
    let seconds_per_quarter = 60.0 / bpm;
    speed.quarter_notes() * seconds_per_quarter * sample_rate
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(speed_) + test(samples_per_step)'`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/clock.rs multosis/src/lib.rs
git commit -m "feat(multosis): add Speed and samples_per_step"
```

---

### Task 16: `StepClock` — block-accurate step boundaries

**Files:**
- Modify: `multosis/src/clock.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/clock.rs`:

```rust
    #[test]
    fn step_clock_fires_at_each_boundary_with_offsets() {
        let mut clk = StepClock::new();
        let mut offsets = Vec::new();
        // 100 samples per step; a 250-sample block crosses two boundaries.
        clk.advance(250, 100.0, |off| offsets.push(off));
        assert_eq!(offsets, vec![100, 200]);
    }

    #[test]
    fn step_clock_carries_the_remainder_across_blocks() {
        let mut clk = StepClock::new();
        let mut offsets = Vec::new();
        clk.advance(250, 100.0, |off| offsets.push(off)); // accum left at 50
        offsets.clear();
        // Next boundary is 50 samples in (100 - 50 carried).
        clk.advance(100, 100.0, |off| offsets.push(off));
        assert_eq!(offsets, vec![50]);
    }

    #[test]
    fn step_clock_block_shorter_than_a_step_fires_nothing() {
        let mut clk = StepClock::new();
        let mut count = 0;
        clk.advance(40, 100.0, |_| count += 1);
        clk.advance(40, 100.0, |_| count += 1);
        assert_eq!(count, 0); // 80 samples total, no boundary yet
        clk.advance(40, 100.0, |_| count += 1);
        assert_eq!(count, 1); // 120 samples total crosses the 100 boundary
    }

    #[test]
    fn step_clock_zero_samples_per_step_fires_nothing() {
        let mut clk = StepClock::new();
        let mut count = 0;
        clk.advance(1000, 0.0, |_| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn step_clock_reset_clears_the_accumulator() {
        let mut clk = StepClock::new();
        clk.advance(70, 100.0, |_| {});
        clk.reset();
        let mut offsets = Vec::new();
        // After reset the next boundary is a full step away.
        clk.advance(150, 100.0, |off| offsets.push(off));
        assert_eq!(offsets, vec![100]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis step_clock`
Expected: build failure — `cannot find type StepClock`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/clock.rs`, after the `samples_per_step` function (before the test module):

```rust
/// Accumulates elapsed samples and reports when step boundaries are crossed.
/// The accumulator is the number of samples elapsed since the last boundary;
/// it is always kept in `[0, samples_per_step)`.
#[derive(Clone, Copy, Debug)]
pub struct StepClock {
    accum: f64,
}

impl StepClock {
    /// A clock with its accumulator at zero — the first boundary is a full
    /// step away.
    pub fn new() -> Self {
        Self { accum: 0.0 }
    }

    /// Clear the accumulator. Used when the sequence is reset so the next
    /// step boundary is a full step away.
    pub fn reset(&mut self) {
        self.accum = 0.0;
    }

    /// Advance the clock across a process block of `block_len` samples at the
    /// given `samples_per_step`. `on_step` is called once per step boundary
    /// that falls within the block, with the sample offset of the boundary
    /// inside the block. A non-positive `samples_per_step` fires nothing.
    pub fn advance(
        &mut self,
        block_len: usize,
        samples_per_step: f64,
        mut on_step: impl FnMut(usize),
    ) {
        if samples_per_step <= 0.0 {
            return;
        }
        let block = block_len as f64;
        // The first boundary lands `samples_per_step - accum` samples in.
        let mut boundary = samples_per_step - self.accum;
        while boundary < block {
            let offset = if boundary < 0.0 { 0 } else { boundary as usize };
            on_step(offset);
            boundary += samples_per_step;
        }
        self.accum = (self.accum + block).rem_euclid(samples_per_step);
    }
}

impl Default for StepClock {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis step_clock`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/clock.rs
git commit -m "feat(multosis): add StepClock block-accurate stepping"
```

---

### Task 17: Milestone 1a verification

**Files:** none — this task only runs checks.

- [ ] **Step 1: Run the full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests green (≈ 57 tests across the five modules).

- [ ] **Step 2: Run clippy with CI's strictness**

Run: `cargo clippy -p multosis -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 3: Check formatting**

Run: `cargo fmt -p multosis -- --check`
Expected: no diff (exit 0).

If any check fails, fix the issue, re-run the relevant `cargo nextest` filter to confirm, and amend the most recent task's commit (or add a `fix:` commit).

- [ ] **Step 4: Commit (only if Step 2 or 3 required edits)**

```bash
git add multosis/
git commit -m "style(multosis): satisfy clippy and rustfmt for milestone 1a"
```

If Steps 2 and 3 produced no edits, skip this commit.

---

## Milestone 1a — definition of done

- The `multosis` crate is a registered workspace member and builds.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean.
- The headless routing model is complete: `Grid` (cells, `next_cell` geometry, `default_routing`/`reset_routing`/`reinit_activations`/`sanitize`, serde), region copy/paste with truncation, deterministic `randomize_activations`/`randomize_routing`, the `Wavefront` + `step_manual` + `Propagator` lifecycle, and the `Speed`/`samples_per_step`/`StepClock` clock.
- No GUI, no audio, no nih-plug — those are Milestone 1b.

## Spec coverage check (self-review)

- §3.1 params/state: `Speed` enum (Task 15) and `Grid` serde (Task 8) back the `speed` param and persisted state; the `auto_restart` behavior is exercised via `Propagator::tick`'s argument (Task 14). The nih-plug `MultosisParams` struct itself is Milestone 1b (plugin shell).
- §4.1 types: `Direction`/`Cell`/`LoopRegion`/`Grid` — Tasks 2–5.
- §4.2 `next_cell` geometry incl. loop-region containment — Task 6.
- §4.3 operations: `default_routing` (Task 5), `reset_routing`/`reinit_activations` (Task 7), `randomize_activations`/`randomize_routing` (Tasks 10–11), `copy_region`/`paste_region` (Task 9), serde (Task 8).
- §5.1 `Wavefront`/`step_manual`/`Propagator` incl. Initial/Running/Stopped and the `auto_restart` dead-end branch — Tasks 12–14.
- §5.2 clock — Tasks 15–16.
- §8 milestone 1a test list — every named case has a test: edge wrap, loop-region containment, propagation split/merge/loop, dead-end death, randomize-no-dead-ends, copy/paste truncation, serde round-trip, clock math, and 1×1 loop-region self-wrap (covered in `next_cell`, `step_manual`, and `Propagator`).
