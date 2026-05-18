# Multosis Phase 1 — Milestone 1b-ii-b-1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Multosis editor's grid interactive — single-clicking a cell edits its routing (octant click toggles a send direction, centre click toggles enable, right-click centre toggles start).

**Architecture:** A new `cell_zone` geometry helper splits each cell into a 3×3 of clickable zones (centre + 8 directions). A pure `apply_grid_click` mutates the `Grid` for a given click. The editor's `on_event` tracks the mouse position, hit-tests clicks with `cell_zone`, applies them to the persisted `params.grid`, and republishes to the `grid_handoff` so the audio thread picks up the edit. The toolbar controls and loop-region drag handles are Milestone 1b-ii-b-2.

**Tech Stack:** Rust (nightly), nih-plug, baseview + softbuffer + tiny-skia, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §1.1 (the octant-cell interaction), §7 (grid editor).

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** Every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state:** The `multosis` plugin (Milestone 1b-ii-a, 82 tests green) has a CPU editor that *displays* the grid + wavefront but is not interactive. `editor.rs`: `MultosisWindow` (baseview WindowHandler, `on_event` handles only `Resized`), `MultosisEditor`, `create(params, wavefront_display)`. `editor/grid_view.rs`: `cell_rect`, `cell_at`, `draw_grid`/`draw_wavefront`/`draw_status`. `grid.rs`: `Direction` (with `ALL`, `delta() -> (i32,i32)`, `bit()`), `Cell` (with `enabled`/`is_start`/`sends`, `toggle_send(dir)`), `Grid` (`cell`/`cell_mut`), `ROWS`/`COLS`. The plugin struct `Multosis` has a `grid_handoff: Arc<GridHandoff>` field; `GridHandoff::publish(&self, grid: Grid)` exists. `Grid` is `Copy`.

---

### Task 1: `Direction::from_delta` — the inverse of `delta`

**Files:**
- Modify: `multosis/src/grid.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/grid.rs`:

```rust
    #[test]
    fn direction_from_delta_is_the_inverse_of_delta() {
        for dir in Direction::ALL {
            let (dr, dc) = dir.delta();
            assert_eq!(Direction::from_delta(dr, dc), Some(dir));
        }
    }

    #[test]
    fn direction_from_delta_rejects_non_unit_steps() {
        assert_eq!(Direction::from_delta(0, 0), None); // no movement
        assert_eq!(Direction::from_delta(2, 0), None); // too far
        assert_eq!(Direction::from_delta(-1, 2), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib direction_from_delta`
Expected: build failure — `no function or associated item named from_delta`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/grid.rs`, inside the `impl Direction` block (after `bit`):

```rust
    /// The direction whose `delta()` equals `(drow, dcol)`, or `None` when the
    /// pair is not one of the 8 unit steps. The inverse of `delta`.
    pub fn from_delta(drow: i32, dcol: i32) -> Option<Direction> {
        Direction::ALL
            .into_iter()
            .find(|d| d.delta() == (drow, dcol))
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib direction_from_delta`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/grid.rs
git commit -m "feat(multosis): add Direction::from_delta"
```

---

### Task 2: `CellZone` and `cell_zone` — octant hit-testing

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn cell_zone_centre_third_is_center() {
        // The middle of cell (4, 6) resolves to that cell's Center zone.
        let (x, y, w, h) = cell_rect(4, 6, 1.0);
        let z = cell_zone(x + w / 2.0, y + h / 2.0, 1.0);
        assert_eq!(z, Some((4, 6, CellZone::Center)));
    }

    #[test]
    fn cell_zone_edges_map_to_directions() {
        let (x, y, w, h) = cell_rect(4, 6, 1.0);
        // Top-centre third -> North.
        let top = cell_zone(x + w / 2.0, y + h / 6.0, 1.0);
        assert_eq!(top, Some((4, 6, CellZone::Send(Direction::N))));
        // Right-centre third -> East.
        let right = cell_zone(x + w * 5.0 / 6.0, y + h / 2.0, 1.0);
        assert_eq!(right, Some((4, 6, CellZone::Send(Direction::E))));
        // Bottom-right third -> South-East.
        let se = cell_zone(x + w * 5.0 / 6.0, y + h * 5.0 / 6.0, 1.0);
        assert_eq!(se, Some((4, 6, CellZone::Send(Direction::SE))));
    }

    #[test]
    fn cell_zone_outside_the_grid_is_none() {
        assert_eq!(cell_zone(10.0, 5.0, 1.0), None); // status strip
        assert_eq!(cell_zone(-5.0, 200.0, 1.0), None); // left of grid
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib cell_zone`
Expected: build failure — `cannot find type CellZone` / `cannot find function cell_zone`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `cell_at` function (before the color helpers):

```rust
/// A clickable zone within a cell: the centre, or one of the 8 send
/// directions (the cell is split into a 3×3 — centre third + 8 surrounders).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellZone {
    Center,
    Send(Direction),
}

/// The cell and zone under physical-pixel point `(px, py)` at `scale`, or
/// `None` if the point is outside the grid.
pub fn cell_zone(px: f32, py: f32, scale: f32) -> Option<(usize, usize, CellZone)> {
    let (row, col) = cell_at(px, py, scale)?;
    let (cx, cy, w, h) = cell_rect(row, col, scale);
    // Third index 0..3 within the cell, on each axis.
    let tcol = (((px - cx) / w) * 3.0).floor().clamp(0.0, 2.0) as i32;
    let trow = (((py - cy) / h) * 3.0).floor().clamp(0.0, 2.0) as i32;
    if trow == 1 && tcol == 1 {
        return Some((row, col, CellZone::Center));
    }
    // A non-centre third maps to a unit (drow, dcol) step.
    let dir = Direction::from_delta(trow - 1, tcol - 1)?;
    Some((row, col, CellZone::Send(dir)))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib cell_zone`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add octant cell-zone hit-testing"
```

---

### Task 3: `apply_grid_click` — the cell-edit rule

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn left_click_octant_toggles_a_send() {
        let mut g = Grid::default_routing(); // every cell sends E only
        apply_grid_click(&mut g, 2, 3, CellZone::Send(Direction::S), false);
        assert!(g.cell(2, 3).sends_to(Direction::S));
        // A second left click on the same octant toggles it back off.
        apply_grid_click(&mut g, 2, 3, CellZone::Send(Direction::S), false);
        assert!(!g.cell(2, 3).sends_to(Direction::S));
        // The pre-existing East send is untouched.
        assert!(g.cell(2, 3).sends_to(Direction::E));
    }

    #[test]
    fn left_click_centre_toggles_enabled() {
        let mut g = Grid::default_routing(); // every cell enabled
        apply_grid_click(&mut g, 5, 5, CellZone::Center, false);
        assert!(!g.cell(5, 5).enabled);
        apply_grid_click(&mut g, 5, 5, CellZone::Center, false);
        assert!(g.cell(5, 5).enabled);
    }

    #[test]
    fn right_click_centre_toggles_start() {
        let mut g = Grid::default_routing();
        // Column 7 is not a start cell by default.
        assert!(!g.cell(1, 7).is_start);
        apply_grid_click(&mut g, 1, 7, CellZone::Center, true);
        assert!(g.cell(1, 7).is_start);
        apply_grid_click(&mut g, 1, 7, CellZone::Center, true);
        assert!(!g.cell(1, 7).is_start);
    }

    #[test]
    fn right_click_octant_is_ignored() {
        let mut g = Grid::default_routing();
        let before = *g.cell(3, 3);
        apply_grid_click(&mut g, 3, 3, CellZone::Send(Direction::W), true);
        assert_eq!(*g.cell(3, 3), before, "right-click on an octant does nothing");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib apply_grid_click`
Expected: build failure — `cannot find function apply_grid_click`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, immediately after the `cell_zone` function:

```rust
/// Apply a click on cell `(row, col)`'s `zone` to the grid. A left click
/// (`right == false`) toggles a send direction (octant) or the `enabled`
/// flag (centre); a right click toggles the `is_start` flag (centre only)
/// and does nothing on an octant.
pub fn apply_grid_click(
    grid: &mut Grid,
    row: usize,
    col: usize,
    zone: CellZone,
    right: bool,
) {
    let cell = grid.cell_mut(row, col);
    match (zone, right) {
        (CellZone::Send(dir), false) => cell.toggle_send(dir),
        (CellZone::Center, false) => cell.enabled = !cell.enabled,
        (CellZone::Center, true) => cell.is_start = !cell.is_start,
        (CellZone::Send(_), true) => {} // right-click on an octant: ignored
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib apply_grid_click`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add apply_grid_click cell-edit rule"
```

---

### Task 4: Thread the `grid_handoff` into the editor

**Files:**
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/lib.rs`

Editor wiring — verified by compilation. The editor must republish edited grids to the audio thread; that needs the `GridHandoff`.

- [ ] **Step 1: Add `grid_handoff` to the editor types**

In `multosis/src/editor.rs`:

(a) Add an import — change the `use crate::...` lines so they include the handoff:

```rust
use crate::handoff::GridHandoff;
use crate::wavefront_display::WavefrontDisplay;
use crate::MultosisParams;
```

(b) Add a field to the `MultosisWindow` struct (after `wavefront_display`):

```rust
    grid_handoff: Arc<GridHandoff>,
```

(c) `MultosisWindow::new` — add a `grid_handoff: Arc<GridHandoff>` parameter (after `wavefront_display`) and store it in the returned struct.

(d) Add a field to the `MultosisEditor` struct (after `wavefront_display`):

```rust
    grid_handoff: Arc<GridHandoff>,
```

(e) Change `create` to take the handoff and pass it through:

```rust
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    grid_handoff: Arc<GridHandoff>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        grid_handoff,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}
```

(f) In `Editor::spawn`, clone the handoff and pass it to `MultosisWindow::new`:

```rust
        let grid_handoff = Arc::clone(&self.grid_handoff);
```

and add `grid_handoff` to the `MultosisWindow::new(...)` argument list (in the same position as the `new` parameter — after `wavefront_display`).

- [ ] **Step 2: Update the `editor()` method**

In `multosis/src/lib.rs`, the `editor()` method currently reads:

```rust
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.wavefront_display.clone())
    }
```

Change the `editor::create(...)` call to also pass the grid handoff:

```rust
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.wavefront_display.clone(),
            self.grid_handoff.clone(),
        )
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. A `dead_code` warning for the `MultosisWindow` field `grid_handoff` (it is written here but first read in Task 5) is EXPECTED — do NOT suppress it. No errors.

Run: `cargo nextest run -p multosis`
Expected: PASS — 91 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor.rs multosis/src/lib.rs
git commit -m "feat(multosis): thread the grid handoff into the editor"
```

---

### Task 5: Handle grid clicks in `on_event`

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation; the click logic it calls (`cell_zone`, `apply_grid_click`) is already unit-tested.

- [ ] **Step 1: Add a mouse-position field**

In `multosis/src/editor.rs`, add a field to the `MultosisWindow` struct (after `grid_handoff`):

```rust
    /// Latest cursor position in physical pixels, updated on CursorMoved.
    mouse_pos: (f32, f32),
```

In `MultosisWindow::new`, add `mouse_pos: (0.0, 0.0),` to the returned struct.

- [ ] **Step 2: Add the click handler**

Add a method to the `impl MultosisWindow` block (after `resize_buffers`):

```rust
    /// Apply a grid click at the current cursor position. `right` selects the
    /// right-button behaviour (toggle start). Edits the persisted grid and
    /// republishes it so the audio thread picks up the change.
    fn handle_grid_click(&mut self, right: bool) {
        let (px, py) = self.mouse_pos;
        let Some((row, col, zone)) = grid_view::cell_zone(px, py, self.scale_factor)
        else {
            return;
        };
        if let Ok(mut grid) = self.params.grid.lock() {
            grid_view::apply_grid_click(&mut grid, row, col, zone, right);
            self.grid_handoff.publish(*grid);
        }
    }
```

- [ ] **Step 3: Extend `on_event`**

Replace the entire body of `MultosisWindow`'s `on_event` method with:

```rust
    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                self.scale_factor =
                    (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) => {
                self.mouse_pos = (position.x as f32, position.y as f32);
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                self.handle_grid_click(false);
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                self.handle_grid_click(true);
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles cleanly — NO warnings (the `grid_handoff` field is now read by `handle_grid_click`). No errors.

Run: `cargo nextest run -p multosis`
Expected: PASS — 91 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): edit the grid by clicking cells in the editor"
```

---

### Task 6: Milestone 1b-ii-b-1 verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests green (91: the 82 from Milestone 1b-ii-a, plus `from_delta` ×2, `cell_zone` ×3, `apply_grid_click` ×4).

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
- Clicking an outer (octant) zone of a cell toggles a send pip in that direction; clicking it again removes it.
- Clicking a cell's centre toggles its enabled state (background light↔dark).
- Right-clicking a cell's centre toggles its green start-cell outline.
- With the transport running, edits take audible effect — the wavefront follows the new routing on the next pass.

Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for milestone 1b-ii-b-1"
```

If Step 2 produced no edits, skip this commit.

---

## Milestone 1b-ii-b-1 — definition of done

- The editor's grid is interactive: clicking a cell's octant zones toggles send directions, centre click toggles enable, right-click centre toggles start. Edits update the persisted grid and the audio thread follows them.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.
- The toolbar (Speed/Mix/Bank/Output, Reset, the six grid operations) and the loop-region drag handles are Milestone 1b-ii-b-2.

## Spec coverage check (self-review)

- §1.1 octant-cell interaction — `cell_zone` splits a cell into centre + 8 direction zones (Task 2); `apply_grid_click` (Task 3) makes a single click toggle one send, the enable flag, or (right-click centre) the start flag; `on_event` (Task 5) wires real mouse clicks to it.
- §7 grid editor — clicking the grid edits routing in place; edits flow to the persisted `params.grid` and are republished via the `grid_handoff` so the running sequence follows them.
- Out of scope (Milestone 1b-ii-b-2): the toolbar controls and the six grid operations (reset routing, reinit activations, randomize activations, randomize routing, copy, paste); the draggable loop-region handles.
