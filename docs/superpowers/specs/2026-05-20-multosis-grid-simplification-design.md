# Multosis Grid Simplification — Design

**Date:** 2026-05-20
**Status:** Approved

## Summary

Reduce the multosis grid from a generalized cell-routing system to a plain
left-to-right step sequencer. Custom per-cell routing, configurable start
cells, and the game-of-life propagation seam are removed; the previous
*default* routing behavior becomes the single supported behavior. The 2D
loop zone is kept. The result is a 16×32 step grid scanned by one playhead
column that wraps within the loop zone.

## Motivation

The current grid (`grid.rs`, `propagation.rs`) implements a generalized
cellular router: each `Cell` carries an 8-direction `sends` bitmask plus an
`is_start` flag, and a 512-cell `Wavefront` propagates by following each lit
cell's sends. This generality was built to leave room for a game-of-life
sequencing mode that will not be pursued. The only behavior actually used is
the default — every cell sends East, the left column starts — i.e. a plain
horizontal scan. Carrying the routing machinery costs UI surface (octant
click zones, routing buttons), audio-thread work (512-bool wavefront copies),
and conceptual weight for no benefit.

## Scope

### Removed

- **Custom routing** — per-cell `sends` bitmask and all editing of it.
- **Start cells** — per-cell `is_start` flag and all editing of it.
- **Game-of-life sequencing** — `step_manual` propagation seam and the
  `Wavefront` / `Propagator` / `SequenceState` machinery.
- **Randomize Routing** toolbar button and `randomize_routing`.
- **Reset Routing** toolbar button (nothing left to reset).
- **Auto-Restart** toolbar toggle and the `auto_restart` parameter — a
  wrapping playhead has no dead ends, so there is nothing to restart.
- **`Direction`** enum — only routing used it.
- **`CellZone`** enum and `cell_zone()` 3×3 hit-test split.

### Kept

- **Loop zone** — stays a 2D rectangle (`LoopRegion`: row + column bounds)
  with edge, corner, and grip drag. Only rows inside the zone play; the
  playhead scans the zone's columns.
- **Copy / Paste** of the loop region (`region.rs`).
- **Reinit Cells** toolbar button — resets every cell to the default (all on).
- **Randomize Activations** toolbar button and `randomize_activations` — still
  meaningful: randomizes the on/off step pattern inside the loop zone.
- **Drag-paint** of cells, including shift-drag to erase.
- **`StepClock`** (`clock.rs`) unchanged, including the `pending_first`
  sample-0 fire on transport start.
- **`CellLight`** modulation trigger source — still fires when a row's
  playhead enters an enabled cell.

## Design

### 1. Data model (`grid.rs`)

- `Cell` collapses to a plain `bool` (the cell's enabled state). The `Cell`
  struct, `sends`, `is_start`, `set_send`, `sends_to`, `toggle_send`,
  `has_send` are all removed.
- `Direction` enum is removed.
- `Grid` becomes `{ cells: [bool; 512], loop_region: LoopRegion }`.
  - `Grid::default()` — all cells `true` (on), `loop_region` full.
  - `default_routing`, `reset_routing`, `reinit_activations`, `next_cell`
    are removed. A new `Grid::reinit()` sets all cells to `true` (backs the
    Reinit Cells button).
- `LoopRegion` is unchanged — 2D rect, `contains()`, `normalized()`,
  `full()`, sanitize/repair.
- `index(row, col)` / `cell(row, col)` / `cell_mut(row, col)` accessors stay,
  now returning `bool`.

### 2. Sequencing (`propagation.rs`)

`Wavefront`, `Propagator`, `SequenceState`, and `step_manual` are removed.
The file is reduced to a `Playhead`:

```rust
pub struct Playhead {
    /// The column the playhead currently occupies.
    column: usize,
    /// False until the first tick after a reset; the first tick lands the
    /// playhead on the loop zone's left edge rather than advancing.
    started: bool,
}

impl Playhead {
    pub fn new() -> Self { Self { column: 0, started: false } }

    /// Advance one step. The first tick after a reset snaps to `col0`;
    /// subsequent ticks move one column right, wrapping at `col1` back to
    /// `col0`. If the loop zone was resized so the column now sits outside
    /// it, snap back to `col0`.
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

    pub fn column(&self) -> usize { self.column }

    pub fn reset(&mut self) {
        self.column = 0;
        self.started = false;
    }
}
```

A `u64` step counter is kept alongside the playhead (in the engine) for the
sequence-status readout; it increments once per tick and resets with the
playhead.

### 3. Engine integration (`engine.rs`)

- **Active rows** — a row `r` is active when `r` is within
  `[loop_region.row0, loop_region.row1]` *and*
  `grid.cell(r, playhead.column())` is on. `active_rows()` computes a `u16`
  bitmask from this; `lit_enabled_per_row` collapses into it.
- **Cell-light triggers** — at each step boundary the engine snapshots the
  active-row mask before and after `playhead.tick()`; rows newly set fire a
  `CellLight` modulation event, exactly as today.
- **`auto_restart` removed** — dropped from `process()`'s signature, from the
  `Multosis` params, and from the toolbar. The dead-end branch is gone with
  `Propagator`.
- **`process()`** still walks the block in segments split at step boundaries;
  at each boundary it ticks the playhead instead of the propagator.

### 4. Audio→GUI mirror

`WavefrontDisplay` (16 × `AtomicU32`) is replaced by a single atomic playhead
column (`AtomicU32`, or `AtomicUsize`). The GUI already holds the grid and
loop zone, so the column index is sufficient to draw which cells are
sounding. The `active_rows: AtomicU16` mirror (track-list sounding dots) is
unchanged.

### 5. Editor / UI

- **Hit-testing** — `CellZone` and `cell_zone()` are removed. `cell_at()`
  (whole-cell hit) stays. A left-click anywhere on a cell toggles its
  `enabled` state; left-drag paints (shift-drag erases), unchanged from
  today's paint gesture. Right-click on a cell becomes a no-op (it formerly
  toggled `is_start`).
- **Cell drawing** (`grid_view.rs`) — send arrowheads and the green start-cell
  outline are removed. A cell draws as on/off only.
- **Playhead overlay** — the per-cell orange "wavefront core" overlay is
  replaced by a single column highlight at `playhead.column()`, drawn across
  the loop zone's rows.
- **Toolbar** (`toolbar.rs`) — lower row drops **Reset Routing** and
  **Randomize Routing**, leaving **Reinit Cells**, **Randomize Activations**,
  **Copy**, **Paste**. Top row drops the **Auto-Restart** toggle. Remaining
  controls re-flow to fill the freed space.

### 6. Sequence status (`seq_status.rs`)

`SequenceState` (Initial / Running / Stopped) is removed. The toolbar's
sequence-status readout today shows `"Initial"` / `"Running · {step}"` /
`"Stopped"` at the right end of the lower row. In the new model there is no
lifecycle state — the playhead simply scans — so the readout shows the
running step counter only: `"Step {n}"`.

`SeqStatusDisplay` keeps its `step: AtomicU64` and drops the `state:
AtomicU8`; `publish` takes just the step count, `read` returns just the step.

### 7. Randomization (`randomize.rs`)

`randomize_routing` is removed. `randomize_activations` and the xorshift32
`Rng` are kept; the Randomize Activations button continues to randomize
`enabled` flags inside the loop zone.

### 8. Region copy/paste (`region.rs`)

`RegionSnapshot` now snapshots `bool` cells (`Vec<bool>`). `copy_region` /
`paste_region` are otherwise unchanged.

## Data flow

```
StepClock ──boundaries──▶ engine.process()
                              │  at each boundary:
                              ▼
                         Playhead.tick(loop_region)   ── column advances, wraps
                              │
                              ▼
       active_rows = rows in loop-zone row span with cell(row, column) on
                              │
              ┌───────────────┼────────────────┐
              ▼               ▼                ▼
       wet effect sum   CellLight events   playhead-column atomic
                                                  │
                                                  ▼
                                       editor draws column highlight
```

## Persistence & migration

`Cell` changes shape (struct → `bool`), so `GridSerde` changes. Presets saved
by the current build carry the old `Cell` representation and will fail to
deserialize; nih-plug falls back to the default grid in that case. This is
acceptable — multosis is in active development and not yet released. No
migration shim is written.

## Testing

Removed tests: everything covering `Direction`, `sends`, `next_cell`,
`is_start`, `Wavefront`, `Propagator`, `SequenceState`, `step_manual`,
`randomize_routing`, and `CellZone` / `cell_zone`.

New / updated tests:

- `Playhead::tick` — first tick lands on `col0`; subsequent ticks advance one
  column; wraps at `col1` back to `col0`.
- `Playhead::tick` — when the loop zone is resized so the current column falls
  outside `[col0, col1]`, the next tick snaps back to `col0`.
- `Playhead::reset` — clears `started` and column.
- Active-row computation — a row inside the loop zone with an on cell at the
  playhead column is active; a row outside the row span is not; an off cell
  is not.
- Cell-light edge detection — a row whose playhead enters an enabled cell
  fires; one that does not, does not.
- Whole-cell hit-testing — `cell_at` returns the cell for any point inside it;
  a left-click toggles `enabled`.
- Grid serde round-trip with the new `bool` cell representation.
- `region.rs` copy/paste round-trip with `bool` cells.
- `randomize_activations` still confined to the loop zone and deterministic
  by seed.

All work lands behind `cargo build`, `cargo clippy --workspace -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run -p multosis` clean.

## Out of scope

- No change to effects, modulation/MSEG, the wet-bus compressor, or the
  track listing.
- No change to `LoopRegion`'s shape or its drag interactions.
- No change to `StepClock` timing.
- No preset migration shim.
