# Multosis Phase 1 — Milestone 1b-ii-b-3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill the editor toolbar's lower row with the six grid operations (reset routing, reinit cells, randomize activations, randomize routing, copy, paste) and add the sequence-status readout (Initial/Running/Stopped + step count).

**Architecture:** A new `SeqStatusDisplay` (`AtomicU8` + `AtomicU64`) mirrors the propagation lifecycle from the audio thread to the editor, mirroring `WavefrontDisplay`. `toolbar.rs` grows a `ToolbarOp` enum + lower-row geometry and a pure `apply_grid_op` for the four grid-mutating operations; copy/paste run in the editor against an `Option<RegionSnapshot>` clipboard. `draw_toolbar` renders the six op buttons and the status text. The loop-region drag handles are the final milestone, 1b-ii-b-4.

**Tech Stack:** Rust (nightly), nih-plug, baseview + softbuffer + tiny-skia + `tiny-skia-widgets`, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §3.1, §7 (the six routing/activation operations; the sequence status readout).

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (Milestone 1b-ii-b-2, 94 tests green):** The editor toolbar's upper row has Speed/Bank/Auto-Restart/Mix/Output/Reset, all interactive. `editor.rs`: `MultosisWindow` (fields incl. `params`, `grid_handoff`, `mouse_pos`, `scale_factor`, `gui_context`, `reset_request`, `toolbar_drag`, `text_renderer`, `wavefront_display`), `MultosisEditor`, `create(params, wavefront_display, grid_handoff, reset_request)`, the `editor()` method in `lib.rs`. `editor/toolbar.rs`: `ToolbarControl`, `control_rect`, `toolbar_hit`, `draw_toolbar(pixmap, tr, params, scale)`, `TOOLBAR_ROW_H` (44.0, in grid_view.rs), `STATUS_H` (88.0). `grid.rs`: `Grid::reset_routing`/`reinit_activations`. `randomize.rs`: `randomize_activations(&mut Grid, u32)`, `randomize_routing(&mut Grid, u32)`. `region.rs`: `RegionSnapshot`, `Grid::copy_region(&self) -> RegionSnapshot`, `Grid::paste_region(&mut self, &RegionSnapshot)`. `propagation.rs`: `SequenceState` (`Initial`/`Running`/`Stopped`, derives `Clone, Copy, PartialEq, Eq, Debug`), `Propagator` with `pub state: SequenceState` and `pub step: u64`. `engine.rs`: `AudioEngine` has a private `propagator: Propagator` and `wavefront(&self) -> &Wavefront`. The plugin `Multosis` has `engine`, `grid_handoff`, `wavefront_display`, `reset_request`; `process()` already calls `engine.process(...)` then `wavefront_display.publish(...)`.

---

### Task 1: `SeqStatusDisplay` — audio→GUI sequence-status mirror

**Files:**
- Create: `multosis/src/seq_status.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/seq_status.rs`:

```rust
//! Lock-free audio→GUI mirror of the sequence lifecycle state and step count.
//!
//! The audio thread publishes once per process block; the editor reads it each
//! frame to draw the status readout. Two `Relaxed` atomics — a torn pair is
//! sub-frame and visually irrelevant.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §3.1.

use crate::propagation::SequenceState;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_display_reads_initial_state() {
        let d = SeqStatusDisplay::new();
        assert_eq!(d.read(), (SequenceState::Initial, 0));
    }

    #[test]
    fn publish_round_trips_every_state() {
        let d = SeqStatusDisplay::new();
        for (state, step) in [
            (SequenceState::Initial, 0),
            (SequenceState::Running, 14),
            (SequenceState::Stopped, 7),
        ] {
            d.publish(state, step);
            assert_eq!(d.read(), (state, step));
        }
    }
}
```

Add `pub mod seq_status;` to `multosis/src/lib.rs` (with the other `pub mod` lines).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib seq_status`
Expected: build failure — `cannot find type SeqStatusDisplay`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/seq_status.rs`, after the `use` lines (before the `#[cfg(test)]` module):

```rust
/// The audio→GUI mirror: the lifecycle state (as a `u8` code) and the step
/// count.
pub struct SeqStatusDisplay {
    /// 0 = Initial, 1 = Running, 2 = Stopped.
    state: AtomicU8,
    step: AtomicU64,
}

impl SeqStatusDisplay {
    /// A display reading `Initial`, step 0.
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            step: AtomicU64::new(0),
        }
    }

    /// Audio thread: publish the current lifecycle state and step count.
    pub fn publish(&self, state: SequenceState, step: u64) {
        let code = match state {
            SequenceState::Initial => 0,
            SequenceState::Running => 1,
            SequenceState::Stopped => 2,
        };
        self.state.store(code, Ordering::Relaxed);
        self.step.store(step, Ordering::Relaxed);
    }

    /// GUI thread: read the last published `(state, step)`.
    pub fn read(&self) -> (SequenceState, u64) {
        let state = match self.state.load(Ordering::Relaxed) {
            0 => SequenceState::Initial,
            1 => SequenceState::Running,
            _ => SequenceState::Stopped,
        };
        (state, self.step.load(Ordering::Relaxed))
    }
}

impl Default for SeqStatusDisplay {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib seq_status`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/seq_status.rs multosis/src/lib.rs
git commit -m "feat(multosis): add SeqStatusDisplay audio-to-GUI mirror"
```

---

### Task 2: `AudioEngine` status accessors

**Files:**
- Modify: `multosis/src/engine.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/engine.rs`:

```rust
    #[test]
    fn new_engine_reports_initial_state_and_zero_step() {
        let engine = AudioEngine::new();
        assert_eq!(engine.sequence_state(), crate::propagation::SequenceState::Initial);
        assert_eq!(engine.step(), 0);
    }

    #[test]
    fn engine_reports_running_after_arming() {
        let grid = Grid::default_routing(); // left column = start cells
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut left = [0.0_f32; 64];
        let mut right = [0.0_f32; 64];
        // One short step arms the start cells -> Running.
        engine.process(
            &mut left, &mut right, true, 10.0, EffectBank::Lowpass, 0.0, true, &grid,
        );
        assert_eq!(
            engine.sequence_state(),
            crate::propagation::SequenceState::Running
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(new_engine_reports) + test(engine_reports_running)'`
Expected: build failure — `no method named sequence_state`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/engine.rs`, inside the `impl AudioEngine` block (after the `wavefront` method):

```rust
    /// The current sequence lifecycle state — exposed for the editor's status
    /// readout.
    pub fn sequence_state(&self) -> crate::propagation::SequenceState {
        self.propagator.state
    }

    /// Steps since the wavefront was last armed — exposed for the editor's
    /// status readout.
    pub fn step(&self) -> u64 {
        self.propagator.step
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(new_engine_reports) + test(engine_reports_running)'`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/engine.rs
git commit -m "feat(multosis): expose AudioEngine sequence state and step"
```

---

### Task 3: Wire `SeqStatusDisplay` into the plugin and editor

**Files:**
- Modify: `multosis/src/lib.rs`
- Modify: `multosis/src/editor.rs`

Plugin/editor wiring — verified by compilation.

- [ ] **Step 1: Add the field to the plugin and publish it**

In `multosis/src/lib.rs`:

(a) Add a field to the `Multosis` struct (after `wavefront_display`):

```rust
    /// Audio→GUI sequence-status mirror, shared with the editor.
    seq_status: Arc<crate::seq_status::SeqStatusDisplay>,
```

(b) In `impl Default for Multosis`, add to the returned struct:

```rust
            seq_status: Arc::new(crate::seq_status::SeqStatusDisplay::new()),
```

(c) In `process()`, immediately AFTER the existing `self.wavefront_display.publish(self.engine.wavefront());` line, add:

```rust
        self.seq_status
            .publish(self.engine.sequence_state(), self.engine.step());
```

- [ ] **Step 2: Thread it into the editor**

In `multosis/src/editor.rs`:

(a) Add an import (with the other `use crate::...` lines):

```rust
use crate::seq_status::SeqStatusDisplay;
```

(b) Add a field to `MultosisWindow` (after `wavefront_display`):

```rust
    seq_status: Arc<SeqStatusDisplay>,
```

(c) `MultosisWindow::new` — add a parameter `seq_status: Arc<SeqStatusDisplay>` immediately after the `wavefront_display` parameter; store it in the returned struct.

(d) Add a field to `MultosisEditor` (after `wavefront_display`):

```rust
    seq_status: Arc<SeqStatusDisplay>,
```

(e) `create` — add a `seq_status: Arc<SeqStatusDisplay>` parameter immediately after `wavefront_display`, and forward it into the `MultosisEditor`:

```rust
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        seq_status,
        grid_handoff,
        reset_request,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}
```

(f) In `Editor::spawn`, add `let seq_status = Arc::clone(&self.seq_status);` alongside the other clones, and pass `seq_status` to `MultosisWindow::new` (immediately after the `wavefront_display` argument).

- [ ] **Step 3: Update the `editor()` call in `lib.rs`**

In `multosis/src/lib.rs`, the `editor()` method's `editor::create(...)` call passes four arguments. Insert `self.seq_status.clone()` as the SECOND argument (after `self.wavefront_display.clone()`):

```rust
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.wavefront_display.clone(),
            self.seq_status.clone(),
            self.grid_handoff.clone(),
            self.reset_request.clone(),
        )
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. A `dead_code` warning for the `MultosisWindow` field `seq_status` (read by the renderer in Task 6) is EXPECTED — do NOT suppress it. No errors. Run `cargo nextest run -p multosis` — PASS, 98 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/lib.rs multosis/src/editor.rs
git commit -m "feat(multosis): wire SeqStatusDisplay into the plugin and editor"
```

---

### Task 4: `ToolbarOp` — lower-row operation geometry

**Files:**
- Modify: `multosis/src/editor/toolbar.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/toolbar.rs`:

```rust
    #[test]
    fn op_rects_sit_in_the_lower_toolbar_row() {
        for op in ToolbarOp::ALL {
            let (x, y, w, h) = op_rect(op, 1.0);
            assert!(x >= 0.0 && x + w <= 1056.0, "{op:?} out of width");
            // Entirely within the lower row [TOOLBAR_ROW_H, 2*TOOLBAR_ROW_H].
            assert!(
                y >= TOOLBAR_ROW_H && y + h <= 2.0 * TOOLBAR_ROW_H,
                "{op:?} out of the lower row"
            );
        }
    }

    #[test]
    fn op_rects_do_not_overlap() {
        let mut rects: Vec<(f32, f32)> = ToolbarOp::ALL
            .iter()
            .map(|o| {
                let (x, _, w, _) = op_rect(*o, 1.0);
                (x, x + w)
            })
            .collect();
        rects.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for pair in rects.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "ops overlap: {pair:?}");
        }
    }

    #[test]
    fn op_hit_finds_an_op_and_misses_elsewhere() {
        let (x, y, w, h) = op_rect(ToolbarOp::Copy, 1.5);
        assert_eq!(op_hit(x + w / 2.0, y + h / 2.0, 1.5), Some(ToolbarOp::Copy));
        // A point in the upper toolbar row is not an op hit.
        assert_eq!(op_hit(20.0, 10.0, 1.0), None);
        // A point in the grid (below the strip) is not an op hit.
        assert_eq!(op_hit(500.0, 400.0, 1.0), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(op_rects) + test(op_hit)'`
Expected: build failure — `cannot find type ToolbarOp`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/toolbar.rs`, after the `ToolbarControl` `impl` block (before `CTRL_INSET`):

```rust
/// One grid-operation button in the lower toolbar row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarOp {
    /// Restore default East-only routing.
    ResetRouting,
    /// Restore default activations (all enabled, left column start).
    ReinitCells,
    /// Randomize the enabled flags in the loop region.
    RandomizeActivations,
    /// Randomize the routing in the loop region (no dead ends).
    RandomizeRouting,
    /// Copy the loop region to the clipboard.
    Copy,
    /// Paste the clipboard at the loop region.
    Paste,
}

impl ToolbarOp {
    /// The six operations, left to right.
    pub const ALL: [ToolbarOp; 6] = [
        ToolbarOp::ResetRouting,
        ToolbarOp::ReinitCells,
        ToolbarOp::RandomizeActivations,
        ToolbarOp::RandomizeRouting,
        ToolbarOp::Copy,
        ToolbarOp::Paste,
    ];

    /// Logical `(x, width)` of this op button within the 1056-wide row.
    fn logical_x_w(self) -> (f32, f32) {
        match self {
            ToolbarOp::ResetRouting => (6.0, 140.0),
            ToolbarOp::ReinitCells => (150.0, 140.0),
            ToolbarOp::RandomizeActivations => (294.0, 140.0),
            ToolbarOp::RandomizeRouting => (438.0, 140.0),
            ToolbarOp::Copy => (582.0, 140.0),
            ToolbarOp::Paste => (726.0, 140.0),
        }
    }

    /// The button's centred label.
    pub fn label(self) -> &'static str {
        match self {
            ToolbarOp::ResetRouting => "Reset Route",
            ToolbarOp::ReinitCells => "Reinit Cells",
            ToolbarOp::RandomizeActivations => "Rnd Cells",
            ToolbarOp::RandomizeRouting => "Rnd Route",
            ToolbarOp::Copy => "Copy",
            ToolbarOp::Paste => "Paste",
        }
    }
}
```

Then add, after the `toolbar_hit` function:

```rust
/// The physical-pixel rectangle `(x, y, w, h)` of op button `op` at `scale`.
/// The op buttons live in the toolbar's lower row.
pub fn op_rect(op: ToolbarOp, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = op.logical_x_w();
    let x = lx * scale;
    let y = (TOOLBAR_ROW_H + CTRL_INSET) * scale;
    let w = lw * scale;
    let h = (TOOLBAR_ROW_H - 2.0 * CTRL_INSET) * scale;
    (x, y, w, h)
}

/// The op button under physical-pixel point `(px, py)` at `scale`, or `None`.
pub fn op_hit(px: f32, py: f32, scale: f32) -> Option<ToolbarOp> {
    ToolbarOp::ALL.into_iter().find(|&op| {
        let (x, y, w, h) = op_rect(op, scale);
        px >= x && px < x + w && py >= y && py < y + h
    })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(op_rects) + test(op_hit)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles (a `dead_code` warning for `ToolbarOp::label` until Task 6 uses it is acceptable; do not remove it).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/toolbar.rs
git commit -m "feat(multosis): add lower-row toolbar operation geometry"
```

---

### Task 5: `apply_grid_op` — the grid-mutating operations

**Files:**
- Modify: `multosis/src/editor/toolbar.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/toolbar.rs`:

```rust
    #[test]
    fn apply_grid_op_reset_routing_restores_east() {
        use crate::grid::{Direction, Grid};
        let mut g = Grid::default_routing();
        g.cell_mut(2, 2).sends = 0b1010_1010;
        apply_grid_op(&mut g, ToolbarOp::ResetRouting, 0);
        assert_eq!(g.cell(2, 2).sends, 1u8 << Direction::E.bit());
    }

    #[test]
    fn apply_grid_op_reinit_cells_restores_activations() {
        use crate::grid::Grid;
        let mut g = Grid::default_routing();
        g.cell_mut(4, 4).enabled = false;
        apply_grid_op(&mut g, ToolbarOp::ReinitCells, 0);
        assert!(g.cell(4, 4).enabled);
    }

    #[test]
    fn apply_grid_op_randomize_is_deterministic_in_seed() {
        use crate::grid::Grid;
        let mut a = Grid::default_routing();
        let mut b = Grid::default_routing();
        apply_grid_op(&mut a, ToolbarOp::RandomizeRouting, 1234);
        apply_grid_op(&mut b, ToolbarOp::RandomizeRouting, 1234);
        assert_eq!(a, b);
    }

    #[test]
    fn apply_grid_op_copy_and_paste_do_not_mutate_the_grid() {
        use crate::grid::Grid;
        let mut g = Grid::default_routing();
        let before = g;
        apply_grid_op(&mut g, ToolbarOp::Copy, 0);
        apply_grid_op(&mut g, ToolbarOp::Paste, 0);
        assert_eq!(g, before, "Copy/Paste are handled by the editor, not apply_grid_op");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib apply_grid_op`
Expected: build failure — `cannot find function apply_grid_op`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/toolbar.rs`, after the `op_hit` function:

```rust
/// Apply a grid-mutating operation in place. `ResetRouting`/`ReinitCells`
/// ignore `seed`; the randomize ops are deterministic in it. `Copy`/`Paste`
/// are NOT handled here — they need the editor's clipboard, so this is a
/// no-op for them.
pub fn apply_grid_op(grid: &mut crate::grid::Grid, op: ToolbarOp, seed: u32) {
    match op {
        ToolbarOp::ResetRouting => grid.reset_routing(),
        ToolbarOp::ReinitCells => grid.reinit_activations(),
        ToolbarOp::RandomizeActivations => {
            crate::randomize::randomize_activations(grid, seed)
        }
        ToolbarOp::RandomizeRouting => crate::randomize::randomize_routing(grid, seed),
        ToolbarOp::Copy | ToolbarOp::Paste => {}
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib apply_grid_op`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/toolbar.rs
git commit -m "feat(multosis): add apply_grid_op for the toolbar operations"
```

---

### Task 6: Render the op buttons and the status readout

**Files:**
- Modify: `multosis/src/editor/toolbar.rs`
- Modify: `multosis/src/editor.rs`

Rendering — verified by compilation; visual check in Task 8.

- [ ] **Step 1: Extend `draw_toolbar`**

In `multosis/src/editor/toolbar.rs`, replace the `draw_toolbar` function's signature and add the op-button + status rendering. The new signature takes the `SeqStatusDisplay`; the body keeps the strip-background fill and the existing upper-row `for ctrl in ToolbarControl::ALL` loop unchanged, then appends the lower-row loop and the status text.

Replace the `draw_toolbar` signature line:

```rust
pub fn draw_toolbar(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    params: &MultosisParams,
    seq_status: &crate::seq_status::SeqStatusDisplay,
    scale: f32,
) {
```

Then, at the END of `draw_toolbar`'s body (after the existing `for ctrl in ToolbarControl::ALL { ... }` loop closes, before the function's closing brace), add:

```rust
    // Lower row: the six grid-operation buttons.
    for op in ToolbarOp::ALL {
        let (x, y, w, h) = op_rect(op, scale);
        widgets::draw_button(pixmap, tr, x, y, w, h, op.label(), false, false);
    }

    // Sequence-status readout, at the right end of the lower row.
    let (state, step) = seq_status.read();
    let status = match state {
        crate::propagation::SequenceState::Initial => "Initial".to_string(),
        crate::propagation::SequenceState::Running => format!("Running · {step}"),
        crate::propagation::SequenceState::Stopped => "Stopped".to_string(),
    };
    let size = 16.0 * scale;
    let sx = 878.0 * scale;
    let sy = (TOOLBAR_ROW_H + TOOLBAR_ROW_H / 2.0) * scale + size * 0.36;
    tr.draw_text(pixmap, sx, sy, &status, size, widgets::color_text());
```

- [ ] **Step 2: Update the `draw_toolbar` call**

In `multosis/src/editor.rs`, `MultosisWindow::draw` calls `toolbar::draw_toolbar(&mut self.surface.pixmap, &mut self.text_renderer, &self.params, self.scale_factor)`. Add the `seq_status` argument (as the 4th, before `scale_factor`):

```rust
        toolbar::draw_toolbar(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.params,
            &self.seq_status,
            self.scale_factor,
        );
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles cleanly with NO warnings (`seq_status` is now read; `ToolbarOp::label` is now used). No errors. Run `cargo nextest run -p multosis` — PASS, 105 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/toolbar.rs multosis/src/editor.rs
git commit -m "feat(multosis): render the toolbar operations and status readout"
```

---

### Task 7: Handle the operation-button clicks

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation; `apply_grid_op` is already unit-tested.

- [ ] **Step 1: Add the clipboard and seed fields**

In `multosis/src/editor.rs`:

(a) Add an import (with the other `use crate::...` lines):

```rust
use crate::region::RegionSnapshot;
```

(b) Add two fields to `MultosisWindow` (after `toolbar_drag`):

```rust
    /// The loop-region clipboard for Copy/Paste.
    clipboard: Option<RegionSnapshot>,
    /// Seed advanced on each randomize op so successive clicks differ.
    rng_seed: u32,
```

In `MultosisWindow::new`, initialise them in the returned struct: `clipboard: None,` and `rng_seed: 1,`.

- [ ] **Step 2: Add the op-click handler**

Add a method to the `impl MultosisWindow` block (after `handle_toolbar_button`):

```rust
    /// Handle a left click on a lower-row operation button.
    fn handle_toolbar_op(&mut self, op: ToolbarOp) {
        let Ok(mut grid) = self.params.grid.lock() else {
            return;
        };
        match op {
            ToolbarOp::Copy => {
                // Copy snapshots the loop region; it does not change the grid.
                self.clipboard = Some(grid.copy_region());
                return;
            }
            ToolbarOp::Paste => {
                if let Some(snap) = &self.clipboard {
                    grid.paste_region(snap);
                }
            }
            other => {
                toolbar::apply_grid_op(&mut grid, other, self.rng_seed);
                self.rng_seed = self.rng_seed.wrapping_add(1);
            }
        }
        // Paste / Reset / Reinit / Randomize all changed the grid — republish.
        self.grid_handoff.publish(*grid);
    }
```

- [ ] **Step 3: Dispatch op clicks in `on_event`**

In `multosis/src/editor.rs`, in `on_event`'s `ButtonPressed { Left, .. }` arm, the `match toolbar::toolbar_hit(...)` currently handles `Some(Mix|Output)`, `Some(ctrl)`, and `None`. The `None` arm currently calls `self.handle_grid_click(false)`. Change ONLY the `None` arm so it first checks the lower-row op buttons:

```rust
                    None => match toolbar::op_hit(px, py, self.scale_factor) {
                        Some(op) => self.handle_toolbar_op(op),
                        None => self.handle_grid_click(false),
                    },
```

(Leave the `Some(ctrl @ (ToolbarControl::Mix | ToolbarControl::Output))` and `Some(ctrl)` arms unchanged.)

- [ ] **Step 4: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings — `clipboard` and `rng_seed` are now consumed. No errors. Run `cargo nextest run -p multosis` — PASS, 105 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): wire the toolbar grid-operation buttons"
```

---

### Task 8: Milestone 1b-ii-b-3 verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests green (105: the 94 from Milestone 1b-ii-b-2, plus `seq_status` ×2, engine status ×2, `op_rects`/`op_hit` ×3, `apply_grid_op` ×4).

- [ ] **Step 2: Lint and format**

Run: `cargo clippy -p multosis -- -D warnings`
Expected: no warnings.

Run: `cargo fmt -p multosis -- --check`
Expected: clean (exit 0). If it reports a diff, run `cargo fmt -p multosis` and include the change in the final commit.

- [ ] **Step 3: Release build and bundle**

Run: `cargo build --bin multosis --release`
Expected: the standalone binary builds.

Run: `cargo nih-plug bundle multosis --release`
Expected: a VST3 + CLAP bundle is produced with no errors.

- [ ] **Step 4: Manual smoke test**

Run the standalone binary (`cargo run --bin multosis`). Confirm:
- The lower toolbar row shows six buttons: Reset Route, Reinit Cells, Rnd Cells, Rnd Route, Copy, Paste.
- The status readout at the right of the lower row shows "Initial" before play, "Running · N" while the transport runs, and "Stopped" after a dead end with Auto-Restart off.
- Reset Route restores East-only routing; Reinit Cells restores the default activations; Rnd Cells / Rnd Route scramble the loop region (each click differs); Copy then moving the grid is not possible yet (the loop region is fixed until 1b-ii-b-4), but Copy followed by Paste with the default full-grid region round-trips the grid.
- All operations take audible effect on the next sequencer pass.

Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for milestone 1b-ii-b-3"
```

If Step 2 produced no edits, skip this commit.

---

## Milestone 1b-ii-b-3 — definition of done

- The toolbar's lower row carries the six grid operations (reset routing, reinit cells, randomize activations, randomize routing, copy, paste), all functional; the sequence-status readout shows Initial/Running/Stopped + step count.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.
- Only the loop-region drag handles (1b-ii-b-4) remain to finish Phase 1.

## Spec coverage check (self-review)

- §7 "the six routing/activation operations" — `ToolbarOp` + `apply_grid_op` (Tasks 4–5), the op buttons (Task 6), click handling incl. the Copy/Paste clipboard and the per-click randomize seed (Task 7).
- §7 "sequence status (Initial / Running + step count)" — `SeqStatusDisplay` (Task 1), `AudioEngine` accessors (Task 2), published in `process()` and threaded to the editor (Task 3), rendered in the toolbar (Task 6). This closes the §7 gap noted in the Milestone 1b-ii-b-2 review.
- §3.1 audio→GUI mirrors — `SeqStatusDisplay` follows the `WavefrontDisplay` `Relaxed`-atomic pattern.
- Out of scope (Milestone 1b-ii-b-4): the draggable loop-region handles. The copy/paste operations work on the loop region, which until 1b-ii-b-4 is fixed at the full grid — so copy/paste round-trips the whole grid; 1b-ii-b-4 makes the region (and thus the copy/paste target) movable.
