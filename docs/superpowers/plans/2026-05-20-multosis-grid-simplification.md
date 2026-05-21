# Multosis Grid Simplification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce the multosis grid from a generalized cell-routing system to a plain left-to-right step sequencer scanned by one playhead column.

**Architecture:** Custom per-cell routing, configurable start cells, and the game-of-life propagation seam are removed; the previous *default* routing (every cell sends East, left column starts) becomes the single hardwired behavior. The 512-bool `Wavefront` and `Propagator` state machine collapse to a `Playhead` holding one column index that advances and wraps within the loop zone. Cells become a plain enabled flag toggled by a whole-cell click.

**Tech Stack:** Rust (nightly), nih-plug plugin, `cargo nextest` test runner. Workspace at `/home/mpd/git-sources/tract-plugin-pack`, crate `multosis`.

**Spec:** `docs/superpowers/specs/2026-05-20-multosis-grid-simplification-design.md`

**Conventions:**
- Build: `cargo build -p multosis`. Tests: `cargo nextest run -p multosis`. Lint: `cargo clippy -p multosis -- -D warnings`. Format: `cargo fmt -p multosis`.
- Never use `#[allow(...)]` to silence a warning without strong justification.
- Commit message trailer MUST be exactly:
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- One refinement from the spec: `Cell` becomes a one-field struct `Cell { enabled: bool }` rather than a bare `bool`. This honors the same intent (no routing/start data) with far less churn — `[Cell; 512]`, `Vec<Cell>` in `region.rs`, `GridSerde`, and every `cell.enabled` call site stay unchanged.

---

## Task 1: Remove the Auto-Restart feature

A wrapping playhead has no dead ends, so the dead-end / auto-restart machinery
has nothing to govern. Remove the parameter, the toolbar toggle, and the
`process()` argument; make `Propagator::tick` always loop on a dead end (it is
deleted entirely in Task 4, so its interim behavior just needs to compile and
keep tests green).

**Files:**
- Modify: `multosis/src/lib.rs` — `auto_restart` param field + its `params()` registration
- Modify: `multosis/src/editor/toolbar.rs` — `ToolbarControl::AutoRestart` variant + its layout/draw/hit arms
- Modify: `multosis/src/editor.rs` — the `AutoRestart` press/handler arm
- Modify: `multosis/src/engine.rs` — `process()` signature drops `auto_restart`
- Modify: `multosis/src/propagation.rs` — `Propagator::tick` drops the `auto_restart` parameter

- [ ] **Step 1: Drop `auto_restart` from `Propagator::tick`**

In `multosis/src/propagation.rs`, change `tick`'s signature and the dead-end
branch so it always restarts:

```rust
    /// Advance the sequence one step. A dead end (every lit cell routed
    /// nowhere) restarts the sequence.
    pub fn tick(&mut self, grid: &Grid) {
        match self.state {
            SequenceState::Initial => {
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
            }
            SequenceState::Running => {
                let next = step_manual(grid, &self.wavefront);
                if next.is_empty() {
                    self.wavefront = Wavefront::empty();
                    self.state = SequenceState::Initial;
                    self.step = 0;
                } else {
                    self.wavefront = next;
                    self.step += 1;
                }
            }
            SequenceState::Stopped => {}
        }
    }
```

In the same file's `#[cfg(test)]` module, update every `p.tick(&g, true)` /
`p.tick(&g, false)` call to `p.tick(&g)`. Delete the test
`dead_end_without_auto_restart_stops` (the `Stopped` path is now unreachable)
and the test `reset_returns_a_stopped_propagator_to_initial` (it depends on
`Stopped`); these tests and `SequenceState::Stopped` itself are removed in
Task 4 anyway.

- [ ] **Step 2: Verify propagation.rs compiles and tests pass**

Run: `cargo nextest run -p multosis propagation`
Expected: PASS (the remaining propagation tests; the two deleted ones gone).

- [ ] **Step 3: Drop `auto_restart` from `AudioEngine::process`**

In `multosis/src/engine.rs`, `process()` currently takes
`auto_restart: bool`. Remove that parameter. Inside `process()`, the
`self.propagator.tick(grid, auto_restart)` call becomes
`self.propagator.tick(grid)`.

In the `engine.rs` `#[cfg(test)]` module, every `engine.process(...)` call
passes `auto_restart` as the second-to-last argument (`true` in the existing
tests, e.g. `engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, true, &grid)`).
Remove that argument from every call so they read
`engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid)`.

- [ ] **Step 4: Remove the `auto_restart` parameter from the plugin**

In `multosis/src/lib.rs`, remove the `auto_restart` parameter field from the
params struct and its registration. Grep first to find every reference:

Run: `rg -n auto_restart multosis/src/lib.rs`

Remove the field declaration (a `BoolParam`), its initializer in the params
`Default`/constructor, and the line in `process()` that reads
`self.params.auto_restart.value()` and passes it to `engine.process(...)`.

- [ ] **Step 5: Remove the Auto-Restart toolbar control**

In `multosis/src/editor/toolbar.rs`, remove the `ToolbarControl::AutoRestart`
enum variant. Grep for every arm that matches it:

Run: `rg -n AutoRestart multosis/src/editor/toolbar.rs multosis/src/editor.rs`

Remove: the variant from `ToolbarControl::ALL`, its `logical_x_w()` arm, its
draw arm in the toolbar draw function, and any hit-test arm. The remaining
top-row controls (`Speed`, `Mix`, `Output`, `CompThreshold`, `CompRatio`,
`Reset`) re-flow: renumber their `logical_x_w()` x-offsets so they sit flush
with no gap where `AutoRestart` used to be (each control is 144 px wide with a
6 px gap — the existing pattern). In `multosis/src/editor.rs`, remove the
`ToolbarControl::AutoRestart` arm from the press/handler match.

- [ ] **Step 6: Build, lint, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
git add multosis/src
git commit -m "$(cat <<'EOF'
refactor(multosis): remove the Auto-Restart feature

A left-to-right wrapping playhead has no dead ends, so auto-restart has
nothing to govern. Remove the parameter, the toolbar toggle, and the
process() argument; Propagator::tick now always restarts on a dead end
(the propagator is removed entirely in a later step).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Whole-cell click — drop the routing/start zone editing

Today a grid cell is split into a 3×3 zone grid (`CellZone`): the center third
toggles `enabled`, the 8 octants toggle send directions, and a right-click on
the center toggles `is_start`. Remove the zone system so a click anywhere on a
cell toggles `enabled`. The `Cell` still carries `sends`/`is_start` fields —
they are simply no longer edited or drawn; they are removed in Task 5.

**Files:**
- Modify: `multosis/src/editor/grid_view.rs` — remove `CellZone`, `cell_zone()`; rewrite `apply_grid_click`; trim `draw_cell`
- Modify: `multosis/src/editor.rs` — the grid press handler uses `cell_at` directly

- [ ] **Step 1: Read the current cell hit-testing and click code**

Read `multosis/src/editor/grid_view.rs` around the `CellZone` enum,
`cell_zone()`, `cell_at()`, `apply_grid_click()`, and `draw_cell()` (the
Explore map places these at roughly lines 31–92, 335–342, 452–462). Read the
grid-press handler in `multosis/src/editor.rs` (it calls `cell_zone` then
`apply_grid_click`).

- [ ] **Step 2: Write the failing test for whole-cell toggle**

The grid press path is UI-glued, so test the pure helper instead. In
`multosis/src/editor/grid_view.rs`'s `#[cfg(test)]` module, add:

```rust
    #[test]
    fn apply_grid_click_toggles_the_whole_cell() {
        let mut g = Grid::default_routing();
        assert!(g.cell(4, 9).enabled);
        apply_grid_click(&mut g, 4, 9); // any point on the cell
        assert!(!g.cell(4, 9).enabled, "first click turns the cell off");
        apply_grid_click(&mut g, 4, 9);
        assert!(g.cell(4, 9).enabled, "second click turns it back on");
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo nextest run -p multosis apply_grid_click_toggles_the_whole_cell`
Expected: FAIL — `apply_grid_click` currently takes a `CellZone` and a `right`
flag, so this call does not compile / does not match.

- [ ] **Step 4: Remove `CellZone` and rewrite `apply_grid_click`**

In `multosis/src/editor/grid_view.rs`:
- Delete the `CellZone` enum and the `cell_zone()` function entirely.
- Replace `apply_grid_click` with this signature and body:

```rust
/// Toggle the `enabled` flag of the cell at `(row, col)`.
pub fn apply_grid_click(grid: &mut Grid, row: usize, col: usize) {
    let cell = grid.cell_mut(row, col);
    cell.enabled = !cell.enabled;
}
```

Keep `cell_at()` and `cell_rect()` unchanged — `cell_at` already hit-tests the
whole cell.

- [ ] **Step 5: Update the editor's grid press handler**

In `multosis/src/editor.rs`, the left-press path for the grid view currently
calls `grid_view::cell_zone(px, py, scale)` and then
`apply_grid_click(grid, row, col, zone, right)`. Change it to call
`grid_view::cell_at(px, py, scale)` and `apply_grid_click(grid, row, col)`.
The right-click path that previously toggled `is_start` becomes a no-op for
grid cells — remove that arm so a right-click on a grid cell does nothing.

The drag-paint gesture (`GridPaint`, `cells_between`, `paint_cells`) is kept;
it already toggles `enabled` and is unaffected by the zone removal. Verify
`paint_cells` does not reference `CellZone` — if it does, it already operates
on `enabled` and just needs the `CellZone` reference dropped.

- [ ] **Step 6: Trim `draw_cell`**

In `multosis/src/editor/grid_view.rs`, `draw_cell()` currently draws send
arrowheads (from `cell.sends`) and a green inset outline for `cell.is_start`.
Remove both: delete the block that iterates send directions to draw arrowheads
and the block that draws the `is_start` outline (and the `color_start()`
helper if it becomes unused). A cell now draws as a filled rectangle whose
shade reflects `enabled` only. Leave the enabled/disabled background shading
as-is.

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo nextest run -p multosis apply_grid_click_toggles_the_whole_cell`
Expected: PASS.

- [ ] **Step 8: Build, lint, full test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings. Delete any now-stale `cell_zone` tests the
clippy/test run surfaces.

- [ ] **Step 9: Commit**

```bash
git add multosis/src
git commit -m "$(cat <<'EOF'
refactor(multosis): whole-cell click replaces the 3x3 zone grid

A grid cell was split into a 3x3 zone grid - center toggled enabled,
octants toggled send directions, right-click toggled is_start. Drop
CellZone and cell_zone(): a click anywhere on a cell now toggles
enabled. draw_cell stops drawing send arrowheads and the start
outline. The Cell still carries sends/is_start fields - inert now,
removed in a later step.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Remove the Routing toolbar buttons

The lower toolbar row has six op buttons. **Reset Routing** and **Randomize
Routing** edit per-cell routing, which no longer exists as an editable
concept. Remove both, plus the `randomize_routing` function they call.

**Files:**
- Modify: `multosis/src/editor/toolbar.rs` — `ToolbarOp` variants + layout/draw/hit/dispatch
- Modify: `multosis/src/editor.rs` — the toolbar-op handler arms
- Modify: `multosis/src/randomize.rs` — delete `randomize_routing` + its tests

- [ ] **Step 1: Remove `randomize_routing`**

In `multosis/src/randomize.rs`, delete the `randomize_routing` function and,
from the `#[cfg(test)]` module, the tests `randomize_routing_is_deterministic`,
`randomize_routing_never_creates_a_dead_end`, and
`randomize_routing_only_touches_sends_inside_the_region`. Keep `Rng`,
`randomize_activations`, and the `randomize_activations_*` tests.

- [ ] **Step 2: Remove the two `ToolbarOp` variants**

In `multosis/src/editor/toolbar.rs`, find every reference:

Run: `rg -n 'ResetRouting|RandomizeRouting' multosis/src`

Remove the `ToolbarOp::ResetRouting` and `ToolbarOp::RandomizeRouting`
variants, their entries in the op list / `ALL`, their `op_rect()` layout
arms, their `label()` arms, and their dispatch arms in `apply_grid_op`. The
remaining ops are `ReinitCells`, `RandomizeActivations`, `Copy`, `Paste` —
renumber their `op_rect()` x-offsets so the four buttons sit flush (same
144 px width / 6 px gap pattern the existing ops use).

- [ ] **Step 3: Update the editor's toolbar-op handler**

In `multosis/src/editor.rs`, remove the `ResetRouting` and `RandomizeRouting`
arms from the toolbar-op handler match. `ReinitCells` currently calls
`Grid::reinit_activations()` — leave that call as-is for now; it is renamed to
`Grid::reinit()` in Task 5.

- [ ] **Step 4: Build, lint, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo nextest run -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src
git commit -m "$(cat <<'EOF'
refactor(multosis): remove the Reset/Randomize Routing toolbar buttons

Per-cell routing is no longer editable, so the two routing op buttons
have nothing to act on. Remove them and the randomize_routing
function. The lower toolbar row keeps Reinit Cells, Randomize
Activations, Copy, and Paste.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Collapse the sequencer core to a `Playhead`

Replace the 512-bool `Wavefront`, the `Propagator` state machine, the
`SequenceState` enum, and the `step_manual` propagation rule with a single
`Playhead` holding one column index. Rewire the engine, the audio→GUI mirror,
the status readout, and the editor's overlay drawing.

After this task, `grid.rs` still defines `Direction`, `Cell::sends`,
`Cell::is_start`, `Grid::next_cell`, `reset_routing`, and `reinit_activations`
— all now unused. **`cargo clippy` will report them as dead code; that is
expected and Task 5 removes them.** This task's gate is `cargo build` +
`cargo nextest`, not clippy.

**Files:**
- Rewrite: `multosis/src/propagation.rs` — `Playhead` replaces `Wavefront`/`Propagator`/`SequenceState`/`step_manual`
- Modify: `multosis/src/engine.rs` — drive the `Playhead`, recompute active rows
- Rewrite: `multosis/src/wavefront_display.rs` — a single atomic playhead column
- Modify: `multosis/src/seq_status.rs` — step counter only
- Modify: `multosis/src/lib.rs` — wiring of the renamed mirror
- Modify: `multosis/src/editor/grid_view.rs` — playhead column overlay
- Modify: `multosis/src/editor/toolbar.rs` — status readout shows `"Step {n}"`
- Modify: `multosis/src/editor.rs` — overlay draw call

- [ ] **Step 1: Write the failing `Playhead` tests**

Replace the entire `#[cfg(test)]` module of `multosis/src/propagation.rs`
with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::LoopRegion;

    fn region(col0: usize, col1: usize) -> LoopRegion {
        LoopRegion { row0: 0, row1: ROWS - 1, col0, col1 }
    }

    #[test]
    fn new_playhead_has_not_started() {
        let p = Playhead::new();
        assert_eq!(p.column(), 0);
    }

    #[test]
    fn first_tick_lands_on_the_loop_zone_left_edge() {
        let mut p = Playhead::new();
        p.tick(&region(5, 12));
        assert_eq!(p.column(), 5);
    }

    #[test]
    fn subsequent_ticks_advance_one_column() {
        let mut p = Playhead::new();
        let lr = region(5, 12);
        p.tick(&lr); // -> 5
        p.tick(&lr); // -> 6
        p.tick(&lr); // -> 7
        assert_eq!(p.column(), 7);
    }

    #[test]
    fn tick_wraps_at_the_right_edge_back_to_the_left() {
        let mut p = Playhead::new();
        let lr = region(5, 7);
        p.tick(&lr); // 5
        p.tick(&lr); // 6
        p.tick(&lr); // 7
        p.tick(&lr); // wraps -> 5
        assert_eq!(p.column(), 5);
    }

    #[test]
    fn tick_snaps_back_when_the_loop_zone_shrinks_away() {
        let mut p = Playhead::new();
        let wide = region(0, 20);
        p.tick(&wide);
        p.tick(&wide);
        p.tick(&wide); // column 2
        // The loop zone is resized so column 2 is now outside it.
        let narrow = region(10, 15);
        p.tick(&narrow);
        assert_eq!(p.column(), 10, "out-of-range column snaps to col0");
    }

    #[test]
    fn reset_returns_the_playhead_to_unstarted() {
        let mut p = Playhead::new();
        let lr = region(3, 9);
        p.tick(&lr);
        p.tick(&lr); // column 4
        p.reset();
        p.tick(&lr); // first tick again -> col0
        assert_eq!(p.column(), 3);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis propagation`
Expected: FAIL — `Playhead` does not exist yet.

- [ ] **Step 3: Rewrite `propagation.rs` with `Playhead`**

Replace everything in `multosis/src/propagation.rs` *above* the
`#[cfg(test)]` module with:

```rust
//! The sequencer playhead: one column index scanning the loop zone.
//!
//! See `docs/superpowers/specs/2026-05-20-multosis-grid-simplification-design.md`.

use crate::grid::{LoopRegion, ROWS};

/// The sequencer playhead — a single column that scans the loop zone
/// left-to-right and wraps. `Copy` so it crosses the GUI/audio boundary
/// cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Playhead {
    /// The column the playhead currently occupies.
    column: usize,
    /// False until the first tick after a reset; that first tick lands the
    /// playhead on the loop zone's left edge instead of advancing.
    started: bool,
}

impl Playhead {
    /// A fresh playhead that has not yet started scanning.
    pub fn new() -> Self {
        Self { column: 0, started: false }
    }

    /// The column the playhead currently occupies.
    pub fn column(&self) -> usize {
        self.column
    }

    /// Advance one step. The first tick after a reset snaps to the loop
    /// zone's left edge (`col0`); subsequent ticks move one column right,
    /// wrapping at `col1` back to `col0`. If the loop zone was resized so the
    /// current column now sits outside `[col0, col1]`, snap back to `col0`.
    pub fn tick(&mut self, loop_region: &LoopRegion) {
        let (lo, hi) = (loop_region.col0, loop_region.col1);
        if !self.started {
            self.column = lo;
            self.started = true;
        } else if self.column >= hi || self.column < lo {
            self.column = lo;
        } else {
            self.column += 1;
        }
    }

    /// Return the playhead to its unstarted state — the next tick will land
    /// on the loop zone's left edge. Triggered by the Reset button and the
    /// transport stopped→playing edge.
    pub fn reset(&mut self) {
        self.column = 0;
        self.started = false;
    }
}

impl Default for Playhead {
    fn default() -> Self {
        Self::new()
    }
}

/// Bit `r` set when row `r` is inside the loop zone's row span and the cell
/// at `(r, column)` is enabled. The sequencer's active-row rule.
pub fn active_rows(grid: &crate::grid::Grid, loop_region: &LoopRegion, column: usize) -> u16 {
    let mut mask = 0u16;
    for r in loop_region.row0..=loop_region.row1.min(ROWS - 1) {
        if grid.cell(r, column).enabled {
            mask |= 1 << r;
        }
    }
    mask
}
```

Note: `active_rows` is a free function here so both the engine and tests can
reach it. The `column` argument is always within `[0, COLS)` because the
engine only ever passes `playhead.column()`.

- [ ] **Step 4: Run the `Playhead` tests to verify they pass**

Run: `cargo nextest run -p multosis propagation`
Expected: PASS — the six `Playhead` tests.

- [ ] **Step 5: Rewrite `wavefront_display.rs`**

Replace the entire contents of `multosis/src/wavefront_display.rs` with:

```rust
//! Lock-free audio→GUI mirror of the sequencer playhead column.
//!
//! The audio thread publishes the playhead's column once per process block;
//! the editor reads it each frame to draw the column highlight. One
//! `AtomicU32`, `Relaxed` ordering — a torn read is sub-frame and visually
//! irrelevant.

use std::sync::atomic::{AtomicU32, Ordering};

/// The audio→GUI playhead-column mirror.
pub struct PlayheadDisplay {
    column: AtomicU32,
}

impl PlayheadDisplay {
    /// A display with the playhead at column 0.
    pub fn new() -> Self {
        Self { column: AtomicU32::new(0) }
    }

    /// Audio thread: publish the current playhead column.
    pub fn publish(&self, column: usize) {
        self.column.store(column as u32, Ordering::Relaxed);
    }

    /// GUI thread: the last published playhead column.
    pub fn column(&self) -> usize {
        self.column.load(Ordering::Relaxed) as usize
    }
}

impl Default for PlayheadDisplay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_display_reads_column_zero() {
        assert_eq!(PlayheadDisplay::new().column(), 0);
    }

    #[test]
    fn publish_round_trips_the_column() {
        let d = PlayheadDisplay::new();
        d.publish(17);
        assert_eq!(d.column(), 17);
        d.publish(3);
        assert_eq!(d.column(), 3);
    }
}
```

Rename the file's module if the crate's `mod` declaration uses a path — keep
the filename `wavefront_display.rs` (renaming the file is optional churn) but
the type is `PlayheadDisplay`. Update `mod` doc comments as needed.

- [ ] **Step 6: Simplify `seq_status.rs`**

Edit `multosis/src/seq_status.rs`: drop the `state: AtomicU8` field and the
`SequenceState` import. `SeqStatusDisplay` keeps only `step: AtomicU64`.
`publish` takes just `step: u64`; `read` returns just `u64`:

```rust
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
        Self { step: AtomicU64::new(0) }
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
```

Update the `#[cfg(test)]` module of `seq_status.rs` to match the new API
(publish/read a bare `u64`); rewrite any test that referenced `SequenceState`.

- [ ] **Step 7: Drive the `Playhead` from the engine**

In `multosis/src/engine.rs`:
- Change the imports: drop `use crate::propagation::{Propagator, Wavefront};`,
  add `use crate::propagation::{active_rows, Playhead};`. `COLS` is no longer
  needed by `engine.rs` — drop it from the `use crate::grid::...` line if
  unused.
- Replace the `propagator: Propagator` field with `playhead: Playhead` and add
  a `step: u64` field.
- `AudioEngine::new()` — `propagator: Propagator::new()` becomes
  `playhead: Playhead::new()`, add `step: 0`.
- `reset()` — `self.propagator.reset()` becomes `self.playhead.reset(); self.step = 0;`.
- Delete the `wavefront()` accessor and the `sequence_state()` accessor.
- `step()` — return `self.step`.
- Replace `lit_enabled_per_row` and the old `active_rows` associated function:
  the per-row "lit and enabled" mask at a given column is exactly
  `active_rows(grid, &grid.loop_region, column)` from `propagation.rs`. In
  `process()`, the cell-light edge detection currently snapshots
  `lit_enabled_per_row` before and after each `propagator.tick`. Rewrite it as:
  - before a tick: `let before = active_rows(grid, &grid.loop_region, self.playhead.column());`
  - tick: `self.playhead.tick(&grid.loop_region); self.step += 1;`
  - after: `let after = active_rows(grid, &grid.loop_region, self.playhead.column());`
  - newly-active rows: `let newly = after & !before;` accumulate into
    `self.pending_cell_lights |= newly;` (these are already per-row `u16`
    masks — simpler than the old per-row `[u32; ROWS]`).
- The per-segment active mask: `active` was
  `Self::active_rows(grid, &self.propagator.wavefront)`. It becomes
  `active_rows(grid, &grid.loop_region, self.playhead.column())`. Recompute it
  after each tick inside the segment loop, exactly where the wavefront-derived
  mask was recomputed.
- `self.last_active = active;` at the end of `process()` is unchanged.

- [ ] **Step 8: Update `engine.rs` tests**

In the `engine.rs` `#[cfg(test)]` module:
- Delete `active_rows_marks_lit_enabled_cells`, `active_rows_ignores_disabled_cells`,
  `active_rows_dedupes_a_row_with_two_lit_cells`, and
  `new_engine_has_an_empty_wavefront` — they test the removed `Wavefront` /
  associated `active_rows`.
- Add a replacement covering the new active-row rule:

```rust
    #[test]
    fn active_rows_marks_enabled_cells_in_the_loop_zone_at_the_column() {
        use crate::propagation::active_rows;
        let mut grid = Grid::default_routing();
        grid.cell_mut(3, 5).enabled = true;
        grid.cell_mut(7, 5).enabled = false;
        let mask = active_rows(&grid, &grid.loop_region, 5);
        assert!(mask & (1 << 3) != 0, "row 3 enabled at col 5 -> active");
        assert!(mask & (1 << 7) == 0, "row 7 disabled at col 5 -> inactive");
    }

    #[test]
    fn active_rows_excludes_rows_outside_the_loop_zone() {
        use crate::propagation::active_rows;
        use crate::grid::LoopRegion;
        let mut grid = Grid::default_routing();
        grid.loop_region = LoopRegion { row0: 4, row1: 8, col0: 0, col1: 31 };
        let mask = active_rows(&grid, &grid.loop_region, 0);
        assert!(mask & (1 << 2) == 0, "row 2 is above the loop zone");
        assert!(mask & (1 << 6) != 0, "row 6 is inside the loop zone");
    }
```

- Any remaining `engine.process(...)` test calls already had `auto_restart`
  removed in Task 1 — no further signature change here.

- [ ] **Step 9: Update `lib.rs` wiring**

In `multosis/src/lib.rs`:
- Wherever `WavefrontDisplay` was constructed / stored / shared with the
  editor, use `PlayheadDisplay` instead (same `Arc` wiring).
- In `process()`, the block that did `self.wavefront_display.publish(self.engine.wavefront())`
  becomes `self.playhead_display.publish(self.engine.playhead_column())` —
  add a `playhead_column(&self) -> usize` accessor to `AudioEngine` that
  returns `self.playhead.column()`.
- The `seq_status.publish(...)` call dropped its state argument: it is now
  `self.seq_status.publish(self.engine.step())`.
- Rename the field `wavefront_display` to `playhead_display` for clarity
  (grep `rg -n wavefront_display multosis/src` and update every reference,
  including the editor constructor parameter).

- [ ] **Step 10: Editor — playhead column overlay**

In `multosis/src/editor/grid_view.rs`, the `draw_wavefront` function draws an
orange core inside every lit cell using `WavefrontDisplay::is_lit`. Replace it
with a column highlight. Rename it `draw_playhead` and give it this shape:

```rust
/// Overlay the playhead — a translucent highlight over the current column,
/// spanning the loop zone's rows.
pub fn draw_playhead(pixmap: &mut Pixmap, column: usize, loop_region: LoopRegion, scale: f32) {
    for row in loop_region.row0..=loop_region.row1.min(ROWS - 1) {
        let (x, y, w, h) = cell_rect(row, column, scale);
        widgets::draw_rect(pixmap, x, y, w, h, color_wavefront());
    }
}
```

Use a translucent `color_wavefront()` (or blend) so the cell's on/off state
still reads through. Keep `color_wavefront()`; drop the per-cell `inset`
square logic. In `multosis/src/editor.rs`, the call site that did
`grid_view::draw_wavefront(pixmap, &self.wavefront_display, scale)` becomes
`grid_view::draw_playhead(pixmap, self.playhead_display.column(), grid.loop_region, scale)`
— pass the loop region from whatever `Grid` the editor already holds for
drawing.

- [ ] **Step 11: Toolbar status readout**

In `multosis/src/editor/toolbar.rs`, the sequence-status readout currently
matches on `SequenceState`. Replace that block with:

```rust
    // Sequence-status readout, at the right end of the lower row.
    let step = seq_status.read();
    let status = format!("Step {step}");
```

Leave the `draw_text` call that renders `status` unchanged.

- [ ] **Step 12: Build and test**

Run: `cargo build -p multosis && cargo nextest run -p multosis`
Expected: PASS. `cargo clippy` is **expected to warn** about unused
`Direction`, `Cell::sends`, `Cell::is_start`, `Grid::next_cell`,
`reset_routing`, `reinit_activations` — do not fix those here; Task 5 removes
them. Do not add `#[allow(...)]`.

- [ ] **Step 13: Commit**

```bash
git add multosis/src
git commit -m "$(cat <<'EOF'
refactor(multosis): collapse the sequencer to a single Playhead

Replace the 512-bool Wavefront, the Propagator state machine,
SequenceState, and the step_manual propagation rule with a Playhead
holding one column index that scans the loop zone left-to-right and
wraps. The engine drives the Playhead, computes active rows from the
loop zone + the column, and detects cell-light events from the
before/after active masks. The audio->GUI mirror collapses to a single
playhead-column atomic; the status readout shows the step counter; the
editor draws a column highlight.

The now-unused routing types in grid.rs (Direction, Cell.sends,
Cell.is_start, next_cell) are removed in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Remove the dead routing/start data model

Strip `grid.rs` down to the step-sequencer model: `Cell { enabled: bool }`,
no `Direction`, no routing. This clears the dead-code warnings Task 4 left.

**Files:**
- Modify: `multosis/src/grid.rs` — remove `Direction`, `Cell` send fields/methods, `next_cell`, routing methods; add `reinit`
- Modify: `multosis/src/randomize.rs` — update tests that reference removed items
- Modify: `multosis/src/region.rs` — update tests that reference removed items
- Modify: `multosis/src/editor.rs` — `reinit_activations` → `reinit`

- [ ] **Step 1: Collapse `Cell`**

In `multosis/src/grid.rs`, replace the `Cell` struct, its `Default`, and its
`impl` block with:

```rust
/// One grid cell — a step in the sequencer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Cell {
    /// When false, the playhead landing on this cell produces silence.
    pub enabled: bool,
}

impl Default for Cell {
    /// An enabled cell.
    fn default() -> Self {
        Self { enabled: true }
    }
}
```

Delete `Cell::sends_to`, `set_send`, `toggle_send`, `has_send`, and the
`is_start` / `sends` fields entirely.

- [ ] **Step 2: Delete the `Direction` enum**

In `multosis/src/grid.rs`, delete the entire `Direction` enum and its `impl`
block (`ALL`, `delta`, `bit`, `from_delta`).

- [ ] **Step 3: Delete routing methods and `next_cell`**

In `multosis/src/grid.rs`:
- Delete the `wrap` free function.
- Delete the `impl Grid` block containing `next_cell`.
- Delete `reset_routing`.
- Replace `reinit_activations` with `reinit`:

```rust
    /// Reset every cell to the default — enabled. Backs the Reinit Cells
    /// button. Leaves the loop region untouched.
    pub fn reinit(&mut self) {
        for cell in self.cells.iter_mut() {
            cell.enabled = true;
        }
    }
```

- Keep `sanitize` (it only normalizes the loop region).

- [ ] **Step 4: Simplify `default_routing`**

In `multosis/src/grid.rs`, replace `default_routing` with a private builder
used by the `Default` impl, and rename it `Grid::default()` directly:

```rust
impl Default for Grid {
    /// The default grid: every cell enabled, loop region the full grid.
    fn default() -> Self {
        Grid {
            cells: [Cell::default(); CELL_COUNT],
            loop_region: LoopRegion::full(),
        }
    }
}
```

Delete the old `default_routing` function. Then update every call site:

Run: `rg -n 'default_routing' multosis/src`

Replace each `Grid::default_routing()` with `Grid::default()` — this hits
test modules in `grid.rs`, `engine.rs`, `randomize.rs`, `region.rs`, and any
editor test helpers.

- [ ] **Step 5: Fix `grid.rs` tests**

In the `grid.rs` `#[cfg(test)]` module, delete every test that referenced the
removed items: `direction_all_lists_eight_in_bit_order`,
`direction_deltas_point_the_right_way`,
`cell_send_toggles_one_direction_at_a_time`,
`default_routing_sends_east_everywhere`,
`default_routing_starts_the_left_column`, `next_cell_*` (all of them),
`no_send_from_a_region_cell_ever_escapes_the_region`,
`reset_routing_restores_east_and_keeps_activations`,
`reinit_activations_restores_defaults_and_keeps_routing`,
`direction_from_delta_is_the_inverse_of_delta`,
`direction_from_delta_rejects_non_unit_steps`.

Rewrite `cell_default_is_enabled_with_no_sends` →

```rust
    #[test]
    fn cell_default_is_enabled() {
        assert!(Cell::default().enabled);
    }
```

Rewrite `default_routing_loop_region_is_full` →

```rust
    #[test]
    fn default_grid_loop_region_is_full() {
        assert_eq!(Grid::default().loop_region, LoopRegion::full());
    }
```

Rewrite `grid_json_round_trips` to drop the `sends`/`is_start` mutations:

```rust
    #[test]
    fn grid_json_round_trips() {
        let mut g = Grid::default();
        g.cell_mut(3, 9).enabled = false;
        g.loop_region = LoopRegion { row0: 2, row1: 12, col0: 4, col1: 28 };
        let json = serde_json::to_string(&g).unwrap();
        let back: Grid = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }
```

Add a test for `reinit`:

```rust
    #[test]
    fn reinit_enables_every_cell() {
        let mut g = Grid::default();
        g.cell_mut(4, 4).enabled = false;
        g.cell_mut(9, 1).enabled = false;
        g.reinit();
        for r in 0..ROWS {
            for c in 0..COLS {
                assert!(g.cell(r, c).enabled);
            }
        }
    }
```

Keep `grid_dimensions_are_16_by_32`, `loop_region_*`, `grid_index_is_row_major`,
`cell_mut_writes_through`, `grid_json_rejects_wrong_cell_count`,
`sanitize_repairs_a_bad_loop_region`.

- [ ] **Step 6: Fix `randomize.rs` tests**

In `multosis/src/randomize.rs`, the surviving `randomize_activations` tests
assert `cell.sends` / `cell.is_start` are untouched and reference
`Direction::E`. Rewrite them to assert only `enabled`:

```rust
    #[test]
    fn randomize_activations_only_touches_cells_inside_the_region() {
        let mut g = Grid::default();
        g.loop_region = LoopRegion { row0: 4, row1: 6, col0: 10, col1: 14 };
        randomize_activations(&mut g, 99);
        for r in 0..crate::grid::ROWS {
            for c in 0..crate::grid::COLS {
                if !g.loop_region.contains(r, c) {
                    assert!(g.cell(r, c).enabled, "cell ({r},{c}) outside region changed");
                }
            }
        }
    }
```

Keep `randomize_activations_is_deterministic` and
`randomize_activations_differs_by_seed` (update `Grid::default_routing()` →
`Grid::default()`). Update the `randomize_activations` doc comment to drop the
"Leaves `sends`, `is_start` … untouched" clause.

- [ ] **Step 7: Fix `region.rs` tests**

In `multosis/src/region.rs`, the test module imports `Direction` and the
tests mutate `cell.sends`. Drop the `Direction` import and rewrite the two
affected tests to use `enabled`:

```rust
    #[test]
    fn paste_region_writes_at_the_loop_region_top_left() {
        let mut g = Grid::default();
        g.loop_region = LoopRegion { row0: 0, row1: 1, col0: 0, col1: 1 };
        g.cell_mut(0, 0).enabled = false;
        let snap = g.copy_region();
        g.loop_region = LoopRegion { row0: 10, row1: 11, col0: 20, col1: 21 };
        g.paste_region(&snap);
        assert!(!g.cell(10, 20).enabled);
        assert!(g.cell(12, 22).enabled, "a cell outside the paste is unchanged");
    }
```

`copy_region_snapshots_the_loop_region` and `paste_region_truncates_on_overflow`
already use `enabled` — just update `Grid::default_routing()` →
`Grid::default()`.

- [ ] **Step 8: Rename `reinit_activations` at its call site**

In `multosis/src/editor.rs`, the `ReinitCells` toolbar-op handler calls
`Grid::reinit_activations()`. Change it to `Grid::reinit()`.

Run: `rg -n 'reinit_activations|reset_routing|next_cell|is_start|\.sends|Direction' multosis/src`
Expected: no matches outside comments — every reference removed.

- [ ] **Step 9: Build, lint, test — fully clean**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo fmt -p multosis --check && cargo nextest run -p multosis`
Expected: all PASS, **no warnings** — the dead-code warnings from Task 4 are
now resolved.

- [ ] **Step 10: Commit**

```bash
git add multosis/src
git commit -m "$(cat <<'EOF'
refactor(multosis): strip the routing data model from the grid

Cell collapses to a single enabled flag. Delete the Direction enum,
the Cell send bitmask + methods, Grid::next_cell, reset_routing, and
reinit_activations (replaced by reinit). The default grid is simply
every cell enabled. Clears the dead-code warnings the sequencer
collapse left behind.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Final verification and bundle

- [ ] **Step 1: Full workspace check**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: all PASS, no warnings. (Other crates are unaffected but the
workspace build confirms nothing else referenced multosis's removed API.)

- [ ] **Step 2: Bundle**

Run: `cargo xtask native nih-plug bundle multosis --release`
Expected: standalone + CLAP + VST3 bundles created under `target/bundled/`.

- [ ] **Step 3: Manual smoke check**

Run the standalone (`./target/release/multosis` or the bundled standalone) and
confirm: the grid is a plain on/off step grid; a click anywhere on a cell
toggles it; the playhead is a single column highlight scanning the loop zone
left-to-right and wrapping; the loop zone still resizes via edges/corners/grip;
the toolbar has Reinit Cells / Randomize Activations / Copy / Paste and no
routing buttons or Auto-Restart; the status readout shows `Step {n}`.

---

## Self-Review

**Spec coverage:**
- Remove custom routing → Tasks 2, 4, 5. ✓
- Remove start cells → Tasks 2, 4, 5 (`is_start` editing, arming, field). ✓
- Remove game-of-life → Task 4 (`step_manual`, `Wavefront`, `Propagator`). ✓
- Remove Randomize Routing → Task 3. ✓
- Remove Reset Routing → Task 3. ✓
- Remove Auto-Restart → Task 1. ✓
- Keep loop zone (2D) → untouched throughout; `LoopRegion` unchanged. ✓
- Playhead left-to-right wrap → Task 4 (`Playhead::tick`). ✓
- Whole-cell click → Task 2. ✓
- `WavefrontDisplay` → playhead atomic → Task 4 (`PlayheadDisplay`). ✓
- `SequenceState` removed, status shows step → Tasks 4. ✓
- `randomize_activations` / region copy-paste kept → Tasks 3, 5. ✓
- Old presets reset to default → covered by the spec; the `Grid` deserialize
  already rejects a wrong-shaped blob, so an old preset fails to load and
  nih-plug falls back to default. No migration shim. ✓

**Type consistency:** `Playhead` (`column()`, `tick(&LoopRegion)`, `reset()`),
`active_rows(grid, &LoopRegion, column) -> u16`, `PlayheadDisplay`
(`publish(usize)`, `column() -> usize`), `SeqStatusDisplay`
(`publish(u64)`, `read() -> u64`), `Cell { enabled: bool }`, `Grid::reinit()`,
`Grid::default()` — used consistently across Tasks 4–5.

**Note on the Task 4 → Task 5 clippy gap:** Task 4 deliberately leaves
`grid.rs` with unused routing types; `cargo build` + `cargo nextest` are green
but `cargo clippy` is not. Task 5 resolves this. A reviewer between Tasks 4 and
5 should expect the dead-code warnings and not treat them as a defect.
