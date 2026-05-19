# Multosis Grid-Editor Layout Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the grid editor breathing room — a margin frame, a toolbar gutter, bigger cells, and a wider gap every 8 columns.

**Architecture:** New logical constants in `grid_view.rs` drive a new cell↔pixel mapping: `cell_rect` gains a margin/gutter offset and a per-group gap; its inverses (`cell_at`, `column_at`, `row_at`, `region_edge_hit`'s bounds guard) are updated to match. The window grows to `1336×758`. The toolbar, whose layout was authored for a 1056-wide window, is re-laid by an affine remap onto the new content span.

**Tech Stack:** Rust (nightly), nih-plug, tiny-skia, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-18-multosis-grid-layout-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (132 multosis tests, 909 workspace tests green):**
- `multosis/src/editor/grid_view.rs`: consts `STATUS_H = 88.0`, `TOOLBAR_ROW_H = 44.0`, `CELL = 33.0`; imports `use crate::grid::{Direction, Grid, LoopRegion, COLS, ROWS};` (`ROWS = 16`, `COLS = 32`).
  - `cell_rect(row, col, scale)` → `x = col*CELL*scale`, `y = (STATUS_H + row*CELL)*scale`, `side = CELL*scale`.
  - `cell_at(px, py, scale) -> Option<(usize,usize)>` inverts it (returns `None` for the status strip / off-grid).
  - `cell_zone` calls `cell_at` then does octant math on `cell_rect` — needs no change (it follows automatically).
  - `column_at(px, scale) -> usize`, `row_at(py, scale) -> usize` — clamped index lookups for dragging.
  - `region_edge_hit` has a grid-bounds guard built from `STATUS_H`/`ROWS`/`COLS`/`CELL`.
  - `region_grip_rect`, `apply_region_drag`, `apply_region_move`, `arrowhead_vertices`, `draw_cell`, `draw_grid`, `draw_wavefront` build on `cell_rect` or on grid indices — they need no formula change.
  - Test module (`#[cfg(test)] mod tests`, `use super::*;`) — three tests assume the old wall-to-wall layout: `window_size_matches_the_grid`, `cell_rect_top_left_and_bottom_right`, `cell_rect_scales`. Other geometry tests build their points via `cell_rect(...)` and are robust to the mapping change.
- `multosis/src/editor.rs`: `pub const WINDOW_WIDTH: u32 = 1056;`, `pub const WINDOW_HEIGHT: u32 = 616;`. `scale` is derived from `WINDOW_WIDTH` (unchanged by this work).
- `multosis/src/lib.rs`: `editor_state: tiny_skia_widgets::EditorState::from_size(1056, 616)`.
- `multosis/src/editor/toolbar.rs`: `ToolbarControl` and `ToolbarOp` each have `logical_x_w()` returning hardcoded `(x, width)` constants for a 1056-wide row inset 6 px each side (controls span x `6..1050`; ops span `6..866`). `control_rect`/`op_rect` multiply by `scale`; `toolbar_hit`/`op_hit` invert; `CTRL_INSET = 4.0`. `draw_toolbar` fills the strip background and draws the six controls, the six ops, and the sequence-status readout (at logical x `878`). Some existing toolbar tests assert coordinates `<= 1056.0`.

---

### Task 1: New layout geometry and window size

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/lib.rs`

This is one atomic change: the cell↔pixel mapping and the window size move together (splitting them leaves the editor mis-rendering or mis-clicking). `cell_zone` and the region/draw functions need no edit — they consume `cell_rect`.

- [ ] **Step 1: Update the breaking tests and add the new ones**

In `multosis/src/editor/grid_view.rs`'s `#[cfg(test)] mod tests` block, **replace** the three tests `window_size_matches_the_grid`, `cell_rect_top_left_and_bottom_right`, and `cell_rect_scales` with these, and **add** the four new tests below them:

```rust
    #[test]
    fn window_size_matches_the_grid() {
        let expect_w = 2.0 * MARGIN
            + COLS as f32 * CELL
            + (COLS / GROUP_SIZE - 1) as f32 * GROUP_GAP;
        let expect_h = STATUS_H + GUTTER + ROWS as f32 * CELL + MARGIN;
        assert_eq!(crate::editor::WINDOW_WIDTH, expect_w as u32);
        assert_eq!(crate::editor::WINDOW_HEIGHT, expect_h as u32);
    }

    #[test]
    fn cell_rect_top_left_and_bottom_right() {
        let (x, y, w, h) = cell_rect(0, 0, 1.0);
        assert_eq!((x, y, w, h), (MARGIN, STATUS_H + GUTTER, CELL, CELL));
        let (x, y, _, _) = cell_rect(ROWS - 1, COLS - 1, 1.0);
        assert_eq!(
            x,
            MARGIN + (COLS - 1) as f32 * CELL + (COLS / GROUP_SIZE - 1) as f32 * GROUP_GAP
        );
        assert_eq!(y, STATUS_H + GUTTER + (ROWS - 1) as f32 * CELL);
    }

    #[test]
    fn cell_rect_scales() {
        // Cell (1, 2): column 2 is in the first group — no group gap.
        let (x, y, w, h) = cell_rect(1, 2, 2.0);
        assert_eq!(
            (x, y, w, h),
            (
                (MARGIN + 2.0 * CELL) * 2.0,
                (STATUS_H + GUTTER + CELL) * 2.0,
                CELL * 2.0,
                CELL * 2.0,
            )
        );
    }

    #[test]
    fn cell_rect_group_gap_offsets_later_groups() {
        // Column 8 starts the second group: one GROUP_GAP further right.
        let (x8, _, _, _) = cell_rect(0, 8, 1.0);
        assert_eq!(x8, MARGIN + 8.0 * CELL + GROUP_GAP);
        // Columns within a group are contiguous, one cell apart.
        let (x4, _, _, _) = cell_rect(0, 4, 1.0);
        let (x5, _, _, _) = cell_rect(0, 5, 1.0);
        assert_eq!(x5 - x4, CELL);
        // Column 16 has crossed two group boundaries.
        let (x16, _, _, _) = cell_rect(0, 16, 1.0);
        assert_eq!(x16, MARGIN + 16.0 * CELL + 2.0 * GROUP_GAP);
    }

    #[test]
    fn cell_at_round_trips_every_cell() {
        for row in 0..ROWS {
            for col in 0..COLS {
                let (x, y, w, h) = cell_rect(row, col, 1.3);
                let mid = (x + w / 2.0, y + h / 2.0);
                assert_eq!(
                    cell_at(mid.0, mid.1, 1.3),
                    Some((row, col)),
                    "cell ({row}, {col}) did not round-trip"
                );
            }
        }
    }

    #[test]
    fn cell_at_none_off_the_cells() {
        // In the left margin.
        assert_eq!(cell_at(MARGIN / 2.0, STATUS_H + GUTTER + 10.0, 1.0), None);
        // In the toolbar / gutter band.
        assert_eq!(cell_at(MARGIN + 10.0, STATUS_H + GUTTER / 2.0, 1.0), None);
        // In a between-group gap, just past column 7's right edge.
        let (x7, y7, w7, _) = cell_rect(0, 7, 1.0);
        assert_eq!(cell_at(x7 + w7 + GROUP_GAP / 2.0, y7 + 5.0, 1.0), None);
        // Far past the grid.
        assert_eq!(cell_at(100_000.0, 100_000.0, 1.0), None);
    }

    #[test]
    fn column_at_and_row_at_clamp_with_the_new_layout() {
        // Left of the grid clamps to column 0; far right to the last column.
        assert_eq!(column_at(0.0, 1.0), 0);
        assert_eq!(column_at(1_000_000.0, 1.0), COLS - 1);
        // The centre of a cell resolves to that column.
        let (x, _, w, _) = cell_rect(0, 20, 1.0);
        assert_eq!(column_at(x + w / 2.0, 1.0), 20);
        // Above the grid clamps to row 0; far below to the last row.
        assert_eq!(row_at(0.0, 1.0), 0);
        assert_eq!(row_at(1_000_000.0, 1.0), ROWS - 1);
        let (_, y, _, h) = cell_rect(9, 0, 1.0);
        assert_eq!(row_at(y + h / 2.0, 1.0), 9);
    }
```

Leave the other existing tests (`cell_at_maps_a_point_back_to_a_cell`, the `cell_zone_*` tests, the `region_*`, `arrowhead_*`, `apply_region_*` tests) unchanged — they build their points via `cell_rect` and stay correct.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(cell_rect) + test(cell_at) + test(window_size) + test(column_at_and_row_at_clamp_with)'`
Expected: build failure — `cannot find value MARGIN` / `GUTTER` / `GROUP_SIZE` / `GROUP_GAP` in scope.

- [ ] **Step 3: Write the implementation**

**3a — `multosis/src/editor/grid_view.rs` constants.** Replace the `CELL` const line and add four constants:

```rust
/// Logical edge length of one square grid cell.
pub const CELL: f32 = 40.0;
/// Logical window-background margin framing the grid (left, right, bottom);
/// also the toolbar controls' side inset.
pub const MARGIN: f32 = 16.0;
/// Logical separation between the toolbar strip and the grid.
pub const GUTTER: f32 = 14.0;
/// Number of columns per visual group.
pub const GROUP_SIZE: usize = 8;
/// Extra logical gap between groups of `GROUP_SIZE` columns.
pub const GROUP_GAP: f32 = 8.0;
```

(`STATUS_H` and `TOOLBAR_ROW_H` keep their current values.)

**3b — `cell_rect`.** Replace its body:

```rust
pub fn cell_rect(row: usize, col: usize, scale: f32) -> (f32, f32, f32, f32) {
    let group = (col / GROUP_SIZE) as f32;
    let x = (MARGIN + col as f32 * CELL + group * GROUP_GAP) * scale;
    let y = (STATUS_H + GUTTER + row as f32 * CELL) * scale;
    let side = CELL * scale;
    (x, y, side, side)
}
```

**3c — `cell_at`.** Replace its body:

```rust
pub fn cell_at(px: f32, py: f32, scale: f32) -> Option<(usize, usize)> {
    if scale <= 0.0 {
        return None;
    }
    let logical_y = py / scale - STATUS_H - GUTTER;
    if logical_y < 0.0 {
        return None;
    }
    let row = (logical_y / CELL) as usize;
    if row >= ROWS {
        return None;
    }
    let lx = px / scale - MARGIN;
    if lx < 0.0 {
        return None;
    }
    let period = GROUP_SIZE as f32 * CELL + GROUP_GAP;
    let group = (lx / period) as usize;
    let within = lx - group as f32 * period;
    if within >= GROUP_SIZE as f32 * CELL {
        return None; // between-group gap — over no cell
    }
    let col = group * GROUP_SIZE + (within / CELL) as usize;
    if col >= COLS {
        return None;
    }
    Some((row, col))
}
```

**3d — `column_at`.** Replace its body:

```rust
pub fn column_at(px: f32, scale: f32) -> usize {
    let lx = px / scale - MARGIN;
    if lx <= 0.0 {
        return 0;
    }
    let period = GROUP_SIZE as f32 * CELL + GROUP_GAP;
    let group = (lx / period) as usize;
    if group >= COLS / GROUP_SIZE {
        return COLS - 1;
    }
    let within = lx - group as f32 * period;
    let cells = GROUP_SIZE as f32 * CELL;
    let in_group = if within >= cells {
        // In the trailing gap — snap to the nearer group-edge column.
        if within - cells < GROUP_GAP / 2.0 {
            GROUP_SIZE - 1
        } else {
            GROUP_SIZE
        }
    } else {
        (within / CELL) as usize
    };
    (group * GROUP_SIZE + in_group).min(COLS - 1)
}
```

**3e — `row_at`.** Replace its body:

```rust
pub fn row_at(py: f32, scale: f32) -> usize {
    let row = ((py / scale - STATUS_H - GUTTER) / CELL).floor();
    row.clamp(0.0, (ROWS - 1) as f32) as usize
}
```

**3f — `region_edge_hit` bounds guard.** Replace the first five lines of the function body (the `grid_top`/`grid_bottom`/`grid_right` block and the `if` guard) with:

```rust
    // Reject the toolbar strip, the gutter, the margins — anything off-grid.
    let grid_top = (STATUS_H + GUTTER) * scale;
    let grid_bottom = (STATUS_H + GUTTER + ROWS as f32 * CELL) * scale;
    let grid_left = MARGIN * scale;
    let grid_right = (MARGIN
        + COLS as f32 * CELL
        + (COLS / GROUP_SIZE - 1) as f32 * GROUP_GAP)
        * scale;
    if py < grid_top || py > grid_bottom || px < grid_left || px > grid_right {
        return None;
    }
```

Leave the rest of `region_edge_hit` (the `cell_rect`-based edge math) unchanged.

**3g — `multosis/src/editor.rs` window constants.** Replace the two constants:

```rust
/// Editor window size. Derived from the grid layout in `grid_view`:
/// width  = 2*MARGIN + COLS*CELL + 3*GROUP_GAP = 16 + 1280 + 24 + 16
/// height = STATUS_H + GUTTER + ROWS*CELL + MARGIN = 88 + 14 + 640 + 16
/// (kept in sync by the `window_size_matches_the_grid` test).
pub const WINDOW_WIDTH: u32 = 1336;
pub const WINDOW_HEIGHT: u32 = 758;
```

**3h — `multosis/src/lib.rs`.** Change `EditorState::from_size(1056, 616)` to `EditorState::from_size(1336, 758)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(cell_rect) + test(cell_at) + test(window_size) + test(column_at_and_row_at_clamp_with)'`
Expected: PASS — 8 tests (the 3 updated, the 4 new, and the existing `cell_at_maps_a_point_back_to_a_cell`, which still round-trips).

Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS, 136 tests (132 pre-existing + 4 new; the three replaced tests are not net-new).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs multosis/src/lib.rs
git commit -m "feat(multosis): frame the grid with margins, gutter, bigger cells, 8-step gaps"
```

---

### Task 2: Toolbar relayout for the wider window

**Files:**
- Modify: `multosis/src/editor/toolbar.rs`

The toolbar's `logical_x_w` coordinates were authored for a 1056-wide window inset 6 px each side. They are remapped — by an affine transform that preserves every control's relative position and width — onto the new content span `[MARGIN, WINDOW_WIDTH − MARGIN]`. The `logical_x_w` constants themselves stay unchanged; `control_rect`/`op_rect`/`draw_toolbar` apply the remap.

- [ ] **Step 1: Write the failing tests**

First read the existing `#[cfg(test)] mod tests` block in `toolbar.rs`. Any existing test that asserts a toolbar coordinate is `<= 1056.0` (e.g. an `op_rects_sit_in_the_lower_toolbar_row`-style test) is now wrong — **update its width bound** from `1056.0` to `crate::editor::WINDOW_WIDTH as f32`. Then **add** these three tests to the block:

```rust
    #[test]
    fn toolbar_rows_lie_within_the_window_margins() {
        let left = crate::editor::grid_view::MARGIN;
        let right = crate::editor::WINDOW_WIDTH as f32 - crate::editor::grid_view::MARGIN;
        for ctrl in ToolbarControl::ALL {
            let (x, _, w, _) = control_rect(ctrl, 1.0);
            assert!(x >= left - 0.5 && x + w <= right + 0.5, "{ctrl:?} outside margins");
        }
        for op in ToolbarOp::ALL {
            let (x, _, w, _) = op_rect(op, 1.0);
            assert!(x >= left - 0.5 && x + w <= right + 0.5, "{op:?} outside margins");
        }
    }

    #[test]
    fn toolbar_controls_do_not_overlap() {
        for row in [
            ToolbarControl::ALL
                .iter()
                .map(|&c| {
                    let (x, _, w, _) = control_rect(c, 1.0);
                    (x, x + w)
                })
                .collect::<Vec<_>>(),
            ToolbarOp::ALL
                .iter()
                .map(|&o| {
                    let (x, _, w, _) = op_rect(o, 1.0);
                    (x, x + w)
                })
                .collect::<Vec<_>>(),
        ] {
            let mut spans = row;
            spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            for pair in spans.windows(2) {
                assert!(pair[0].1 <= pair[1].0 + 0.5, "toolbar items overlap: {pair:?}");
            }
        }
    }

    #[test]
    fn toolbar_hit_round_trips_each_item() {
        for ctrl in ToolbarControl::ALL {
            let (x, y, w, h) = control_rect(ctrl, 1.4);
            assert_eq!(toolbar_hit(x + w / 2.0, y + h / 2.0, 1.4), Some(ctrl));
        }
        for op in ToolbarOp::ALL {
            let (x, y, w, h) = op_rect(op, 1.4);
            assert_eq!(op_hit(x + w / 2.0, y + h / 2.0, 1.4), Some(op));
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(toolbar_rows_lie) + test(toolbar_controls_do_not_overlap) + test(toolbar_hit_round_trips)'`
Expected: FAIL — `toolbar_rows_lie_within_the_window_margins` fails because the controls still span the old `6..1050` range, outside the new `[16, 1320]` margins. (`toolbar_hit_round_trips` may already pass — `control_rect` and `toolbar_hit` are mutually consistent before the remap; it is a regression guard.)

- [ ] **Step 3: Write the implementation**

In `multosis/src/editor/toolbar.rs`:

**3a — imports.** The file already has `use crate::editor::grid_view::TOOLBAR_ROW_H;`. Extend / add imports so `MARGIN` (from `crate::editor::grid_view`) and `WINDOW_WIDTH` (from `crate::editor`) are in scope at module level.

**3b — the remap helper.** Add, near `CTRL_INSET`:

```rust
/// The toolbar layout was authored for a 1056-wide window inset 6 px each
/// side (content span 1044 px). `remap` affinely maps an old logical
/// `(x, width)` onto the current window's content span
/// `[MARGIN, WINDOW_WIDTH - MARGIN]`, preserving every item's relative
/// position and width — so a future window-width change re-fits the toolbar
/// for free.
fn remap(lx: f32, lw: f32) -> (f32, f32) {
    let span = (WINDOW_WIDTH as f32 - 2.0 * MARGIN) / 1044.0;
    (MARGIN + (lx - 6.0) * span, lw * span)
}
```

**3c — `control_rect`.** Replace its body so the logical `(x, w)` is remapped before scaling:

```rust
pub fn control_rect(ctrl: ToolbarControl, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = ctrl.logical_x_w();
    let (rx, rw) = remap(lx, lw);
    let x = rx * scale;
    let y = CTRL_INSET * scale;
    let w = rw * scale;
    let h = (TOOLBAR_ROW_H - 2.0 * CTRL_INSET) * scale;
    (x, y, w, h)
}
```

**3d — `op_rect`.** Replace its body the same way (keeping its lower-row `y`):

```rust
pub fn op_rect(op: ToolbarOp, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = op.logical_x_w();
    let (rx, rw) = remap(lx, lw);
    let x = rx * scale;
    let y = (TOOLBAR_ROW_H + CTRL_INSET) * scale;
    let w = rw * scale;
    let h = (TOOLBAR_ROW_H - 2.0 * CTRL_INSET) * scale;
    (x, y, w, h)
}
```

**3e — `draw_toolbar`.** Two edits:
- The sequence-status readout is drawn at a hardcoded logical x (`878.0`). Remap it: replace that x expression with `remap(878.0, 0.0).0`.
- The two-row strip background fill: confirm it spans the full window width. If it fills a hardcoded `1056.0`-derived width, change it to span `WINDOW_WIDTH as f32 * scale`. If it already fills the pixmap / `physical_width`, leave it.

(`ToolbarControl::logical_x_w` and `ToolbarOp::logical_x_w` keep their current constants — only their doc comments may be updated to note the remap. `toolbar_hit`/`op_hit` are unchanged; they invert via `control_rect`/`op_rect`, which now remap.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(toolbar_rows_lie) + test(toolbar_controls_do_not_overlap) + test(toolbar_hit_round_trips)'`
Expected: PASS — 3 tests.

Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS, 139 tests.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/toolbar.rs
git commit -m "feat(multosis): relay the toolbar across the wider window"
```

---

### Task 3: Verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — 139 tests (132 pre-existing + 4 grid-geometry + 3 toolbar).

Run: `cargo nextest run --workspace`
Expected: PASS — 916 tests.

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
- A window-background margin frames the grid on the left, right and bottom; a gutter separates the toolbar from the grid.
- Cells are visibly larger; every 8 columns there is a wider gap, splitting the 32 steps into four groups.
- The toolbar's controls and operations span the full window width, inset to the margin; the sequence-status readout sits at the right.
- Clicking cells and octants still toggles the right cell/send; right-click toggles start; drag-paint, loop-region edge-resize and the move grip all still hit correctly (the geometry round-trips).
- The loop-region outline, wavefront, arrowheads and toolbar all render correctly; resizing the editor scales cleanly.

Report the smoke-test observations. (This step is a human/visual check.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for the grid-layout redesign"
```

If Step 2 produced no edits, skip this commit.

---

## Definition of done

- The grid is framed by a margin and separated from the toolbar by a gutter; cells are 40 px; a wider gap every 8 columns groups the 32 steps into four; the toolbar spans the `1336×758` window.
- `cargo nextest run -p multosis` is green (139 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.

## Spec coverage check (self-review)

- §1 Layout constants — `CELL`, `MARGIN`, `GUTTER`, `GROUP_SIZE`, `GROUP_GAP` added in Task 1 Step 3a; `WINDOW_WIDTH`/`WINDOW_HEIGHT` = `1336`/`758` in Step 3g, asserted against the formula by `window_size_matches_the_grid`.
- §2 Cell coordinate mapping — `cell_rect` (3b), `cell_at` (3c, `None` in gaps/margins/gutter), `column_at`/`row_at` (3d/3e, clamp + gap-snap), `region_edge_hit` guard (3f). `cell_zone` and `region_grip_rect`/`apply_region_*`/`draw_*`/`arrowhead_vertices` follow from `cell_rect` unchanged — noted in the Task 1 preamble.
- §3 Every-8 group gap — realised by the `group * GROUP_GAP` term in `cell_rect`; nothing drawn in the gap (the editor background shows through). Covered by `cell_rect_group_gap_offsets_later_groups`.
- §4 Toolbar relayout — the affine `remap` (Task 2), applied in `control_rect`/`op_rect`/`draw_toolbar`, inset to `MARGIN`, preserving proportions; tests assert within-margins, no overlap, hit round-trip.
- §5 Window size & persistence — `editor.rs` constants (3g), `lib.rs` `from_size` (3h). Old persisted sizes simply open at ~79 % scale — no migration code needed, as the spec directs.
- §6 Unchanged — routing/propagation/audio, click behaviour, the arrowhead indicators (scale with `CELL` via `cell_rect`), the toolbar control set: untouched.
- §7 Testing — grid geometry round-trip / gap / clamp tests (Task 1), toolbar margin/overlap/hit tests (Task 2), build + smoke test (Task 3).
- Out of scope (UI performance pass, control-set changes, per-group tint/lines) — not implemented.
