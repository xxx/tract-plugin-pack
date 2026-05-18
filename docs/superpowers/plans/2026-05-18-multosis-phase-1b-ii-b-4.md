# Multosis Phase 1 — Milestone 1b-ii-b-4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the editor's loop region resizable by dragging the edges of its highlighted rectangle — the final piece of Phase 1.

**Architecture:** The loop region (`Grid::loop_region`, a `LoopRegion` with inclusive `row0/row1/col0/col1` bounds) is already drawn as an outline in `grid_view.rs`. This milestone adds: a `RegionEdge` hit-test (`region_edge_hit`) plus cursor→grid-index helpers (`column_at`/`row_at`); a pure `apply_region_drag` that resizes one edge while preserving the `row0≤row1`, `col0≤col1` invariant (so the region can never invert and can collapse to 1×1); drag-handle nubs drawn at each edge midpoint; and the drag wired into `MultosisWindow` via a `region_drag: Option<RegionEdge>` field handled across `ButtonPressed`/`CursorMoved`/`ButtonReleased`. Each drag step mutates `params.grid.loop_region` and republishes through `grid_handoff` — the same path `handle_grid_click` already uses.

**Tech Stack:** Rust (nightly), nih-plug, baseview + softbuffer + tiny-skia + `tiny-skia-widgets`, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7 ("Loop region: drawn as a highlighted rectangle; drag handles … resize it").

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (Milestone 1b-ii-b-3, 105 tests green):**
- `multosis/src/grid.rs` — `pub struct LoopRegion { pub row0, row1, col0, col1: usize }`, inclusive bounds, invariant `row0≤row1<ROWS`, `col0≤col1<COLS`; `LoopRegion::full()`, `contains(self, row, col) -> bool`, `normalized(self) -> LoopRegion`, `Default` = `full()`. `Grid` has a public `loop_region: LoopRegion` field; `Grid` is `Copy`. `ROWS=16`, `COLS=32`.
- `multosis/src/editor/grid_view.rs` — consts `STATUS_H=88.0`, `TOOLBAR_ROW_H=44.0`, `CELL=33.0`; `cell_rect(row, col, scale) -> (f32,f32,f32,f32)` (`x = col*CELL*scale`, `y = (STATUS_H + row*CELL)*scale`, `side = CELL*scale`); `cell_at`, `cell_zone`/`CellZone`, `apply_grid_click`; `draw_grid(pixmap, grid, scale)` draws every cell then the loop-region outline via `widgets::draw_rect_outline`, with locals `x0,y0` from `cell_rect(lr.row0, lr.col0, scale)` and `x1,y1,w1,h1` from `cell_rect(lr.row1, lr.col1, scale)`. A local `color_loop()` returns the region highlight color.
- `multosis/src/editor.rs` — `MultosisWindow` with fields incl. `params: Arc<MultosisParams>` (`params.grid: Arc<Mutex<Grid>>`), `grid_handoff: Arc<GridHandoff>`, `mouse_pos: (f32,f32)`, `scale_factor: f32`, `toolbar_drag`, `clipboard`, `rng_seed`. `handle_grid_click(&mut self, right: bool)` locks `params.grid`, applies the click, and republishes via `grid_handoff.publish(*grid)`. `on_event` handles `ButtonPressed{Left}` (a `match toolbar::toolbar_hit(...)` whose `None` arm does `match toolbar::op_hit(...) { Some(op) => handle_toolbar_op(op), None => handle_grid_click(false) }`), `CursorMoved` (updates `mouse_pos`, drives `toolbar_drag`), and `ButtonReleased{Left}` (ends slider drags).
- Region geometry is read by `Grid::next_cell` (propagation wrapping), `randomize_activations`/`randomize_routing`, and `copy_region`/`paste_region` — all use inclusive `row0..=row1` / `col0..=col1`. Resizing the region therefore needs no changes there.

---

### Task 1: `RegionEdge` + edge hit-test + cursor→index helpers

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

First, ensure `LoopRegion` is imported. `grid_view.rs` imports items from `crate::grid` (e.g. `Grid`, `ROWS`, `COLS`). Add `LoopRegion` to that existing `use crate::grid::{...}` list.

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn region_edge_hit_finds_each_edge() {
        let region = LoopRegion {
            row0: 2,
            row1: 8,
            col0: 4,
            col1: 20,
        };
        let (x0, y0, _, _) = cell_rect(2, 4, 1.0);
        let (x1, y1, w, h) = cell_rect(8, 20, 1.0);
        let mid_x = (x0 + x1 + w) / 2.0;
        let mid_y = (y0 + y1 + h) / 2.0;
        assert_eq!(
            region_edge_hit(x0, mid_y, region, 1.0),
            Some(RegionEdge::Left)
        );
        assert_eq!(
            region_edge_hit(x1 + w, mid_y, region, 1.0),
            Some(RegionEdge::Right)
        );
        assert_eq!(
            region_edge_hit(mid_x, y0, region, 1.0),
            Some(RegionEdge::Top)
        );
        assert_eq!(
            region_edge_hit(mid_x, y1 + h, region, 1.0),
            Some(RegionEdge::Bottom)
        );
    }

    #[test]
    fn region_edge_hit_misses_interior_and_toolbar() {
        let region = LoopRegion {
            row0: 2,
            row1: 8,
            col0: 4,
            col1: 20,
        };
        // Centre of an interior cell — far from every edge.
        let (xc, yc, wc, hc) = cell_rect(5, 12, 1.0);
        assert_eq!(
            region_edge_hit(xc + wc / 2.0, yc + hc / 2.0, region, 1.0),
            None
        );
        // A point up in the toolbar strip (y < STATUS_H).
        let (x0, _, _, _) = cell_rect(2, 4, 1.0);
        assert_eq!(region_edge_hit(x0, 10.0, region, 1.0), None);
    }

    #[test]
    fn column_at_and_row_at_clamp_to_grid_bounds() {
        assert_eq!(column_at(0.0, 1.0), 0);
        assert_eq!(column_at(CELL * 5.5, 1.0), 5);
        assert_eq!(column_at(CELL * 10_000.0, 1.0), COLS - 1);
        assert_eq!(row_at(0.0, 1.0), 0); // above the grid -> clamp to 0
        assert_eq!(row_at(STATUS_H, 1.0), 0); // exactly at the grid's top
        assert_eq!(row_at(STATUS_H + CELL * 3.5, 1.0), 3);
        assert_eq!(row_at(STATUS_H + CELL * 10_000.0, 1.0), ROWS - 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(region_edge_hit) + test(column_at_and_row_at)'`
Expected: build failure — `cannot find type RegionEdge` / `cannot find function region_edge_hit`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `cell_zone` function:

```rust
/// One draggable edge of the loop-region rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionEdge {
    /// Left edge — drags `col0`.
    Left,
    /// Right edge — drags `col1`.
    Right,
    /// Top edge — drags `row0`.
    Top,
    /// Bottom edge — drags `row1`.
    Bottom,
}

/// Half-width of the grab band straddling a region edge, in logical pixels.
const EDGE_BAND: f32 = 6.0;

/// The loop-region edge under physical-pixel point `(px, py)` at `scale`, or
/// `None`. The cursor must be within the grid drawing area — points in the
/// toolbar strip never hit an edge.
pub fn region_edge_hit(
    px: f32,
    py: f32,
    region: LoopRegion,
    scale: f32,
) -> Option<RegionEdge> {
    // Reject the toolbar strip and anything outside the grid.
    let grid_top = STATUS_H * scale;
    let grid_bottom = (STATUS_H + ROWS as f32 * CELL) * scale;
    let grid_right = COLS as f32 * CELL * scale;
    if py < grid_top || py > grid_bottom || px < 0.0 || px > grid_right {
        return None;
    }
    let (x0, y0, _, _) = cell_rect(region.row0, region.col0, scale);
    let (x1, y1, w1, h1) = cell_rect(region.row1, region.col1, scale);
    let right = x1 + w1;
    let bottom = y1 + h1;
    let band = EDGE_BAND * scale;
    let in_rows = py >= y0 && py <= bottom;
    let in_cols = px >= x0 && px <= right;
    if in_rows && (px - x0).abs() <= band {
        Some(RegionEdge::Left)
    } else if in_rows && (px - right).abs() <= band {
        Some(RegionEdge::Right)
    } else if in_cols && (py - y0).abs() <= band {
        Some(RegionEdge::Top)
    } else if in_cols && (py - bottom).abs() <= band {
        Some(RegionEdge::Bottom)
    } else {
        None
    }
}

/// The grid column under physical-pixel x `px` at `scale`, clamped to
/// `0..=COLS-1`. Used while dragging a region edge, where the cursor may
/// stray off the grid.
pub fn column_at(px: f32, scale: f32) -> usize {
    let col = (px / scale / CELL).floor();
    col.clamp(0.0, (COLS - 1) as f32) as usize
}

/// The grid row under physical-pixel y `py` at `scale`, clamped to
/// `0..=ROWS-1`. Used while dragging a region edge.
pub fn row_at(py: f32, scale: f32) -> usize {
    let row = ((py / scale - STATUS_H) / CELL).floor();
    row.clamp(0.0, (ROWS - 1) as f32) as usize
}
```

NOTE: confirm `STATUS_H`, `CELL`, `ROWS`, `COLS` are all in scope at this point in `grid_view.rs` (they are used by `cell_rect`/`draw_grid` already). Use the same reference paths the existing code uses.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(region_edge_hit) + test(column_at_and_row_at)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles (an `unused`/`dead_code` warning for `region_edge_hit`/`column_at`/`row_at`/`RegionEdge` is acceptable here — Task 4 consumes them; they are `pub` so they likely will not warn anyway. Do NOT add `#[allow]`.)

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add loop-region edge hit-testing"
```

---

### Task 2: `apply_region_drag` — invariant-preserving resize

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn apply_region_drag_left_edge_moves_col0() {
        let r = LoopRegion {
            row0: 0,
            row1: 15,
            col0: 5,
            col1: 20,
        };
        let out = apply_region_drag(r, RegionEdge::Left, 8);
        assert_eq!((out.col0, out.col1), (8, 20));
    }

    #[test]
    fn apply_region_drag_right_edge_moves_col1() {
        let r = LoopRegion {
            row0: 0,
            row1: 15,
            col0: 5,
            col1: 20,
        };
        let out = apply_region_drag(r, RegionEdge::Right, 12);
        assert_eq!((out.col0, out.col1), (5, 12));
    }

    #[test]
    fn apply_region_drag_top_and_bottom_move_rows() {
        let r = LoopRegion {
            row0: 3,
            row1: 12,
            col0: 0,
            col1: 31,
        };
        assert_eq!(apply_region_drag(r, RegionEdge::Top, 6).row0, 6);
        assert_eq!(apply_region_drag(r, RegionEdge::Bottom, 9).row1, 9);
    }

    #[test]
    fn apply_region_drag_cannot_invert_bounds() {
        let r = LoopRegion {
            row0: 4,
            row1: 10,
            col0: 6,
            col1: 18,
        };
        // Drag the left edge past the right edge -> clamps to col1.
        assert_eq!(apply_region_drag(r, RegionEdge::Left, 25).col0, 18);
        // Drag the right edge past the left edge -> clamps to col0.
        assert_eq!(apply_region_drag(r, RegionEdge::Right, 2).col1, 6);
        // Same for the row edges.
        assert_eq!(apply_region_drag(r, RegionEdge::Top, 15).row0, 10);
        assert_eq!(apply_region_drag(r, RegionEdge::Bottom, 1).row1, 4);
    }

    #[test]
    fn apply_region_drag_can_collapse_to_1x1() {
        let r = LoopRegion {
            row0: 4,
            row1: 10,
            col0: 6,
            col1: 18,
        };
        // Collapse to the single cell (10, 18).
        let a = apply_region_drag(r, RegionEdge::Left, 18);
        let b = apply_region_drag(a, RegionEdge::Top, 10);
        assert_eq!((b.row0, b.row1, b.col0, b.col1), (10, 10, 18, 18));
        assert!(b.contains(10, 18));
        assert!(!b.contains(9, 18));
    }

    #[test]
    fn apply_region_drag_result_is_already_normalized() {
        let r = LoopRegion::full();
        let out = apply_region_drag(r, RegionEdge::Right, 5);
        assert_eq!(out, out.normalized());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(apply_region_drag)'`
Expected: build failure — `cannot find function apply_region_drag`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `row_at` function:

```rust
/// Resize the loop region by dragging `edge` to grid `index` (a column index
/// for `Left`/`Right`, a row index for `Top`/`Bottom`, expected already
/// clamped to `0..=COLS-1` / `0..=ROWS-1`). The moved bound is clamped against
/// its opposite so the region can never invert and can shrink to 1×1.
pub fn apply_region_drag(region: LoopRegion, edge: RegionEdge, index: usize) -> LoopRegion {
    let mut r = region;
    match edge {
        RegionEdge::Left => r.col0 = index.min(r.col1),
        RegionEdge::Right => r.col1 = index.max(r.col0).min(COLS - 1),
        RegionEdge::Top => r.row0 = index.min(r.row1),
        RegionEdge::Bottom => r.row1 = index.max(r.row0).min(ROWS - 1),
    }
    r
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(apply_region_drag)'`
Expected: PASS — 6 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add invariant-preserving region resize"
```

---

### Task 3: Draw the region drag-handle nubs

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

Rendering — verified by compilation; visual check in Task 5.

- [ ] **Step 1: Append the nub drawing to `draw_grid`**

In `multosis/src/editor/grid_view.rs`, `draw_grid` currently ends by drawing the loop-region outline with `widgets::draw_rect_outline`, using locals `x0, y0` (from `cell_rect(lr.row0, lr.col0, scale)`) and `x1, y1, w1, h1` (from `cell_rect(lr.row1, lr.col1, scale)`). Immediately AFTER the `draw_rect_outline` call, before `draw_grid`'s closing brace, append:

```rust
    // Drag-handle nubs at the midpoint of each region edge.
    let mid_x = (x0 + (x1 + w1)) / 2.0;
    let mid_y = (y0 + (y1 + h1)) / 2.0;
    let long = 16.0 * scale;
    let thick = 4.0 * scale;
    let nub = color_loop();
    // Left and right edges: vertical nubs.
    widgets::draw_rect(pixmap, x0 - thick / 2.0, mid_y - long / 2.0, thick, long, nub);
    widgets::draw_rect(
        pixmap,
        (x1 + w1) - thick / 2.0,
        mid_y - long / 2.0,
        thick,
        long,
        nub,
    );
    // Top and bottom edges: horizontal nubs.
    widgets::draw_rect(pixmap, mid_x - long / 2.0, y0 - thick / 2.0, long, thick, nub);
    widgets::draw_rect(
        pixmap,
        mid_x - long / 2.0,
        (y1 + h1) - thick / 2.0,
        long,
        thick,
        nub,
    );
```

IMPORTANT — match the real API: confirm `widgets::draw_rect`'s signature (it is the opaque-fast-path filled-rectangle helper in `tiny-skia-widgets` `primitives.rs`). The call above assumes `draw_rect(pixmap, x, y, w, h, color)`. If the real signature differs (different argument order, or a `Color`/`u32` color type), adapt every call — keep the intent: draw four small filled rectangles in the `color_loop()` color, centred on each region edge's midpoint. If `draw_rect` cannot draw a plain filled rect, report BLOCKED.

`color_loop()` is the local helper already used for the outline color — reuse it. Do NOT change the existing outline drawing.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. Run `cargo nextest run -p multosis` — PASS, 114 tests.

- [ ] **Step 3: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): draw loop-region drag-handle nubs"
```

---

### Task 4: Wire the region-edge drag into the editor

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation; the resize/hit-test logic is already unit-tested in Tasks 1–2. No new tests.

- [ ] **Step 1: Add the `region_drag` field**

In `multosis/src/editor.rs`:

(a) Add a field to the `MultosisWindow` struct (after the `rng_seed` field):
```rust
    /// The loop-region edge currently being dragged, if any.
    region_drag: Option<grid_view::RegionEdge>,
```

(b) In `MultosisWindow::new`, initialise it in the returned struct literal: `region_drag: None,`.

NOTE: confirm `grid_view` is in scope in `editor.rs` (the file already calls `grid_view::cell_zone` / `grid_view::apply_grid_click`). Use the same path. `RegionEdge` derives `Copy`, so `Option<RegionEdge>` is `Copy`.

- [ ] **Step 2: Add the drag helper methods**

Add two methods to the `impl MultosisWindow` block (place them right after `handle_grid_click`):

```rust
    /// The loop-region edge under the cursor, if the cursor is over one.
    fn region_edge_under_cursor(&self) -> Option<grid_view::RegionEdge> {
        let (px, py) = self.mouse_pos;
        let region = self.params.grid.lock().ok()?.loop_region;
        grid_view::region_edge_hit(px, py, region, self.scale_factor)
    }

    /// Resize the loop region for the in-progress drag of `edge`, then
    /// republish the grid so the audio thread picks up the new region.
    fn update_region_drag(&mut self, edge: grid_view::RegionEdge) {
        let (px, py) = self.mouse_pos;
        let index = match edge {
            grid_view::RegionEdge::Left | grid_view::RegionEdge::Right => {
                grid_view::column_at(px, self.scale_factor)
            }
            grid_view::RegionEdge::Top | grid_view::RegionEdge::Bottom => {
                grid_view::row_at(py, self.scale_factor)
            }
        };
        if let Ok(mut grid) = self.params.grid.lock() {
            grid.loop_region = grid_view::apply_region_drag(grid.loop_region, edge, index);
            self.grid_handoff.publish(*grid);
        }
    }
```

NOTE: confirm `params.grid` is `Arc<Mutex<Grid>>` accessed via `.lock()` (mirror `handle_grid_click`), `Grid::loop_region` is a public field, and `grid_handoff.publish(*grid)` matches how `handle_grid_click` republishes. Adapt if the real code differs.

- [ ] **Step 3: Start the drag on `ButtonPressed`**

In `on_event`, the `ButtonPressed { Left }` arm's `match toolbar::toolbar_hit(...)` has a `None` arm currently reading:
```rust
                    None => match toolbar::op_hit(px, py, self.scale_factor) {
                        Some(op) => self.handle_toolbar_op(op),
                        None => self.handle_grid_click(false),
                    },
```
Replace ONLY the inner `None => self.handle_grid_click(false),` line so a region-edge press starts a drag instead of a cell click:
```rust
                    None => match toolbar::op_hit(px, py, self.scale_factor) {
                        Some(op) => self.handle_toolbar_op(op),
                        None => {
                            if let Some(edge) = self.region_edge_under_cursor() {
                                self.region_drag = Some(edge);
                            } else {
                                self.handle_grid_click(false);
                            }
                        }
                    },
```
Leave the `Some(...)` arms of the `toolbar_hit` match and the right-click handler unchanged. Match the real `px`/`py`/`self.scale_factor` names used by the surrounding code.

- [ ] **Step 4: Drive the drag on `CursorMoved`**

In `on_event`, the `CursorMoved` arm updates `mouse_pos`, then `toolbar_drag`, then processes an active slider drag. AFTER that existing slider-drag block (still inside the `CursorMoved` arm), append:
```rust
                if let Some(edge) = self.region_drag {
                    self.update_region_drag(edge);
                }
```
`mouse_pos` must already be updated to the new cursor position at this point (it is — that happens at the top of the `CursorMoved` arm). Do not reorder the existing code.

- [ ] **Step 5: End the drag on `ButtonReleased`**

In `on_event`, the `ButtonReleased { Left }` arm currently ends an active slider drag. Add — in the same arm, alongside the existing slider-end logic:
```rust
                self.region_drag = None;
```

- [ ] **Step 6: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings (`region_drag` is now written by Steps 3/5 and read by Step 4; `region_edge_hit`/`column_at`/`row_at`/`apply_region_drag`/`RegionEdge` are all consumed). If a warning remains, investigate and fix it — do NOT add `#[allow]`.

Run: `cargo nextest run -p multosis` — PASS, 114 tests.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 7: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): drag loop-region edges to resize"
```

---

### Task 5: Milestone 1b-ii-b-4 verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — 114 tests (the 105 from Milestone 1b-ii-b-3, plus `region_edge_hit`/`column_at_and_row_at` ×3 and `apply_region_drag` ×6).

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
- The loop-region rectangle shows a small handle nub at the midpoint of each of its four edges.
- Pressing on an edge (or its nub) and dragging resizes the region: the left/right edges follow the cursor's column, the top/bottom edges follow the cursor's row.
- The region can be dragged down to a single cell (1×1) and never inverts (an edge dragged past its opposite stops at it).
- While dragging an edge, no cell toggles; releasing the mouse ends the drag. A press in the region interior still toggles cells / sends as before.
- After a resize, the sequencer's wavefront wraps within the new region bounds on the next pass, and Rnd Cells / Rnd Route affect only the resized region.

Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for milestone 1b-ii-b-4"
```

If Step 2 produced no edits, skip this commit.

---

## Milestone 1b-ii-b-4 — definition of done

- The editor's loop region is resizable by dragging the edges of its highlighted rectangle; handle nubs mark each edge; the region cannot invert and can collapse to 1×1.
- `cargo nextest run -p multosis` is green (114 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.
- **Phase 1 is complete** — this is the last milestone.

## Spec coverage check (self-review)

- §7 "Loop region: drawn as a highlighted rectangle" — unchanged (already drawn); this milestone adds the handle nubs (Task 3).
- §7 "drag handles … resize it" — `RegionEdge` + `region_edge_hit` (Task 1), `apply_region_drag` (Task 2), the nubs (Task 3), and the drag wiring (Task 4). The implementation makes all four edges of the highlighted rectangle draggable: left/right resize the column range, top/bottom resize the row range. The spec phrases this as "handles on the grid's top edge (column range) and left edge (row range)"; with no margin/gutter around the 1056×616 grid there is no separate strip for standalone handles, so resizing is realized as direct region-edge dragging — the standard selection-rectangle interaction, which fully delivers the resize capability the spec requires. The handle nubs make the edges discoverable.
- 1×1 loop region — explicitly exercised by `apply_region_drag_can_collapse_to_1x1` (Task 2) and the smoke test (Task 5), per the standing request to test 1×1 region edge cases.
- Region geometry consumers (`next_cell` propagation wrap, `randomize_*`, `copy_region`/`paste_region`) all read `loop_region` live with inclusive bounds — resizing needs no change there; the smoke test confirms propagation and randomize follow the resized region.
- Out of scope: corner (two-axis) handles — a corner press resolves to one edge; the user does two drags. The spec does not require corner handles.
