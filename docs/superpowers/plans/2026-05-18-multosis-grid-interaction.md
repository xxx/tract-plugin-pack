# Multosis Grid-Editor Interaction Enhancements — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add drag-paint cell toggling and a hover-revealed move grip for translating the loop region to the Multosis grid editor.

**Architecture:** Pure geometry/logic functions land in `multosis/src/editor/grid_view.rs` (alongside the Milestone 1b-ii-b-4 region functions) and are unit-tested. The editor's left-button gesture state is unified into one `LeftGesture` enum on `MultosisWindow`; grid clicks become deferred (decided on release) so a drag's first cell paints consistently. A paint drag fills `enabled` along the cursor's line so fast drags skip no cell; a region move translates all four bounds, clamped on-grid.

**Tech Stack:** Rust (nightly), nih-plug, baseview + softbuffer + tiny-skia + `tiny-skia-widgets`, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-18-multosis-grid-interaction-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (114 multosis tests, 891 workspace tests green):**
- `multosis/src/editor/grid_view.rs` — consts `STATUS_H=88.0`, `TOOLBAR_ROW_H=44.0`, `CELL=33.0`; imports from `crate::grid` include `Grid`, `LoopRegion`, `ROWS` (16), `COLS` (32), `Direction`. `cell_rect(row, col, scale) -> (f32,f32,f32,f32)` (`x = col*CELL*scale`, `y = (STATUS_H + row*CELL)*scale`, `side = CELL*scale`). `cell_at(px, py, scale) -> Option<(usize,usize)>`. `cell_zone(px, py, scale) -> Option<(usize, usize, CellZone)>` where `pub enum CellZone { Center, Send(Direction) }` (derives `Clone, Copy, PartialEq, Eq, Debug`). `apply_grid_click(grid: &mut Grid, row: usize, col: usize, zone: CellZone, right: bool)`. `draw_grid(pixmap: &mut Pixmap, grid: &Grid, scale: f32)` draws cells, the loop-region outline (locals `lr = grid.loop_region`, `x0,y0` from `cell_rect(lr.row0,lr.col0,scale)`, `x1,y1,w1,h1` from `cell_rect(lr.row1,lr.col1,scale)`), then four edge nubs. Milestone 1b-ii-b-4 added `pub enum RegionEdge { Left, Right, Top, Bottom }`, `region_edge_hit(px,py,region,scale) -> Option<RegionEdge>`, `column_at(px,scale) -> usize` (clamped `0..=COLS-1`), `row_at(py,scale) -> usize` (clamped `0..=ROWS-1`), `apply_region_drag(region,edge,index) -> LoopRegion`. A local `color_loop()` returns the region color; `widgets::draw_rect(pixmap, x, y, w, h, color: tiny_skia::Color)` draws a filled rect. `#[cfg(test)] mod tests` has `use super::*;`.
- `crate::grid` — `LoopRegion { pub row0,row1,col0,col1: usize }` inclusive bounds, `Copy`, `PartialEq`; `LoopRegion::full()`, `contains`, `normalized()`. `Grid` is `Copy`, has `cell(row,col) -> &Cell`, `cell_mut(row,col) -> &mut Cell`, public field `loop_region: LoopRegion`. `Cell` has public `enabled: bool`.
- `multosis/src/editor.rs` — `MultosisWindow` fields include `params: Arc<MultosisParams>` (`params.grid: Arc<Mutex<Grid>>`), `grid_handoff: Arc<GridHandoff>`, `mouse_pos: (f32,f32)`, `scale_factor: f32`, `toolbar_drag`, `clipboard`, `rng_seed`, `region_drag: Option<grid_view::RegionEdge>`. Methods: `handle_grid_click(&mut self, right: bool)` (reads `mouse_pos`, `grid_view::cell_zone`, locks `params.grid`, `grid_view::apply_grid_click`, `grid_handoff.publish(*grid)`), `region_edge_under_cursor() -> Option<RegionEdge>`, `update_region_drag(&mut self, edge: RegionEdge)`. `MultosisWindow::draw` calls `grid_view::draw_grid(&mut self.surface.pixmap, &grid, self.scale_factor)`. `on_event`:
  - `CursorMoved { position, .. }` — sets `mouse_pos`, `toolbar_drag.set_mouse`, drives an active slider drag, then `if let Some(edge) = self.region_drag { self.update_region_drag(edge); }`.
  - `ButtonPressed { button: Left, .. }` — `match toolbar::toolbar_hit(...) { Some(Mix|Output) => slider begin, Some(ctrl) => handle_toolbar_button, None => match toolbar::op_hit(...) { Some(op) => handle_toolbar_op, None => { if let Some(edge) = region_edge_under_cursor() { region_drag = Some(edge) } else { handle_grid_click(false) } } } }`.
  - `ButtonPressed { button: Right, .. }` — `handle_grid_click(true)`.
  - `ButtonReleased { button: Left, .. }` — `if let Some(ctrl) = toolbar_drag.end_drag() { end_slider(ctrl) }`; `region_drag = None`.
- baseview mouse events carry modifier state (the workspace's MSEG editor reads a held modifier during a mouse drag).

---

### Task 1: `apply_region_move` — clamped region translation

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn apply_region_move_translates_in_each_direction() {
        let r = LoopRegion {
            row0: 4,
            row1: 7,
            col0: 5,
            col1: 9,
        };
        let right = apply_region_move(r, 0, 3);
        assert_eq!((right.col0, right.col1), (8, 12));
        assert_eq!((right.row0, right.row1), (4, 7));
        let down = apply_region_move(r, 2, 0);
        assert_eq!((down.row0, down.row1), (6, 9));
    }

    #[test]
    fn apply_region_move_preserves_size() {
        let r = LoopRegion {
            row0: 4,
            row1: 7,
            col0: 5,
            col1: 9,
        };
        let moved = apply_region_move(r, -3, 4);
        assert_eq!(moved.row1 - moved.row0, r.row1 - r.row0);
        assert_eq!(moved.col1 - moved.col0, r.col1 - r.col0);
    }

    #[test]
    fn apply_region_move_clamps_at_grid_edges() {
        let r = LoopRegion {
            row0: 4,
            row1: 7,
            col0: 5,
            col1: 9,
        };
        // Far negative -> parks at the top-left.
        let tl = apply_region_move(r, -100, -100);
        assert_eq!((tl.row0, tl.col0), (0, 0));
        assert_eq!((tl.row1, tl.col1), (3, 4));
        // Far positive -> parks at the bottom-right (ROWS-1=15, COLS-1=31).
        let br = apply_region_move(r, 100, 100);
        assert_eq!((br.row1, br.col1), (15, 31));
        assert_eq!((br.row0, br.col0), (12, 27));
    }

    #[test]
    fn apply_region_move_full_region_cannot_move() {
        let r = LoopRegion::full();
        assert_eq!(apply_region_move(r, 5, -8), r);
    }

    #[test]
    fn apply_region_move_1x1_region_moves() {
        let r = LoopRegion {
            row0: 0,
            row1: 0,
            col0: 0,
            col1: 0,
        };
        let moved = apply_region_move(r, 9, 20);
        assert_eq!((moved.row0, moved.row1, moved.col0, moved.col1), (9, 9, 20, 20));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(apply_region_move)'`
Expected: build failure — `cannot find function apply_region_move`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `apply_region_drag` function:

```rust
/// Translate the loop region by `(drow, dcol)` grid cells, preserving its
/// size. The translation is clamped so the region always stays fully within
/// the 16×32 grid — dragging past an edge parks the region against it.
pub fn apply_region_move(region: LoopRegion, drow: i32, dcol: i32) -> LoopRegion {
    let height = region.row1 - region.row0;
    let width = region.col1 - region.col0;
    let max_row0 = (ROWS - 1 - height) as i32;
    let max_col0 = (COLS - 1 - width) as i32;
    let row0 = (region.row0 as i32 + drow).clamp(0, max_row0) as usize;
    let col0 = (region.col0 as i32 + dcol).clamp(0, max_col0) as usize;
    LoopRegion {
        row0,
        row1: row0 + height,
        col0,
        col1: col0 + width,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(apply_region_move)'`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add clamped loop-region translation"
```

---

### Task 2: `region_grip_rect` + `region_grip_hit` — the move grip

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn region_grip_hit_at_region_centre() {
        let region = LoopRegion {
            row0: 2,
            row1: 10,
            col0: 4,
            col1: 24,
        };
        let (x0, y0, _, _) = cell_rect(2, 4, 1.0);
        let (x1, y1, w, h) = cell_rect(10, 24, 1.0);
        let cx = (x0 + x1 + w) / 2.0;
        let cy = (y0 + y1 + h) / 2.0;
        assert!(region_grip_hit(cx, cy, region, 1.0));
    }

    #[test]
    fn region_grip_hit_misses_region_corner_and_outside() {
        let region = LoopRegion {
            row0: 2,
            row1: 10,
            col0: 4,
            col1: 24,
        };
        // Centre of the top-left cell — far from the centre grip.
        let (xc, yc, wc, hc) = cell_rect(2, 4, 1.0);
        assert!(!region_grip_hit(xc + wc / 2.0, yc + hc / 2.0, region, 1.0));
        // A point well outside the region.
        assert!(!region_grip_hit(5.0, 5.0, region, 1.0));
    }

    #[test]
    fn region_grip_rect_fits_inside_a_1x1_region() {
        let region = LoopRegion {
            row0: 6,
            row1: 6,
            col0: 6,
            col1: 6,
        };
        let (gx, gy, gw, gh) = region_grip_rect(region, 1.0);
        let (cx, cy, cw, ch) = cell_rect(6, 6, 1.0);
        // The grip is fully within the single cell.
        assert!(gx >= cx && gx + gw <= cx + cw);
        assert!(gy >= cy && gy + gh <= cy + ch);
        assert!(gw > 0.0 && gh > 0.0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(region_grip)'`
Expected: build failure — `cannot find function region_grip_hit`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `apply_region_move` function:

```rust
/// Logical side length of the loop-region move grip.
const GRIP_SIZE: f32 = 16.0;

/// The physical-pixel rectangle `(x, y, w, h)` of the loop region's move
/// grip — a fixed-size square centred in the region, shrunk if necessary so
/// it never exceeds the region's own bounds.
pub fn region_grip_rect(region: LoopRegion, scale: f32) -> (f32, f32, f32, f32) {
    let (x0, y0, _, _) = cell_rect(region.row0, region.col0, scale);
    let (x1, y1, w1, h1) = cell_rect(region.row1, region.col1, scale);
    let right = x1 + w1;
    let bottom = y1 + h1;
    let size = (GRIP_SIZE * scale).min(right - x0).min(bottom - y0);
    let cx = (x0 + right) / 2.0;
    let cy = (y0 + bottom) / 2.0;
    (cx - size / 2.0, cy - size / 2.0, size, size)
}

/// True when physical-pixel point `(px, py)` is on the loop-region move grip.
pub fn region_grip_hit(px: f32, py: f32, region: LoopRegion, scale: f32) -> bool {
    let (gx, gy, gw, gh) = region_grip_rect(region, scale);
    px >= gx && px < gx + gw && py >= gy && py < gy + gh
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(region_grip)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles (a `dead_code`/unused warning is acceptable here; the functions are `pub` so they likely will not warn; Task 4 and Task 7 consume them. Do NOT add `#[allow]`.)

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add loop-region move-grip geometry"
```

---

### Task 3: `cells_between` — paint-stroke line enumeration

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn cells_between_single_cell() {
        assert_eq!(cells_between((5, 7), (5, 7)), vec![(5, 7)]);
    }

    #[test]
    fn cells_between_includes_both_endpoints() {
        let line = cells_between((2, 3), (2, 6));
        assert_eq!(line.first(), Some(&(2, 3)));
        assert_eq!(line.last(), Some(&(2, 6)));
    }

    #[test]
    fn cells_between_horizontal_has_no_gap() {
        assert_eq!(cells_between((4, 1), (4, 4)), vec![(4, 1), (4, 2), (4, 3), (4, 4)]);
    }

    #[test]
    fn cells_between_vertical_has_no_gap() {
        assert_eq!(cells_between((1, 8), (4, 8)), vec![(1, 8), (2, 8), (3, 8), (4, 8)]);
    }

    #[test]
    fn cells_between_diagonal_steps_both_axes() {
        assert_eq!(cells_between((0, 0), (3, 3)), vec![(0, 0), (1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn cells_between_long_jump_is_contiguous() {
        // A fast drag from one corner of the grid to the other.
        let line = cells_between((0, 0), (15, 31));
        assert_eq!(line.first(), Some(&(0, 0)));
        assert_eq!(line.last(), Some(&(15, 31)));
        // Every consecutive pair is a king-move step (adjacent incl. diagonal).
        for pair in line.windows(2) {
            let dr = (pair[0].0 as i32 - pair[1].0 as i32).abs();
            let dc = (pair[0].1 as i32 - pair[1].1 as i32).abs();
            assert!(dr <= 1 && dc <= 1 && (dr + dc) > 0, "gap between {pair:?}");
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(cells_between)'`
Expected: build failure — `cannot find function cells_between`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `region_grip_hit` function:

```rust
/// Every grid cell on the straight line from cell `a` to cell `b`, inclusive
/// of both endpoints (Bresenham's line) — so a fast paint drag, whose
/// `CursorMoved` events can jump several cells, skips no cell in the stroke.
pub fn cells_between(a: (usize, usize), b: (usize, usize)) -> Vec<(usize, usize)> {
    let (mut r, mut c) = (a.0 as i32, a.1 as i32);
    let (r1, c1) = (b.0 as i32, b.1 as i32);
    let dr = (r1 - r).abs();
    let dc = (c1 - c).abs();
    let sr = if r < r1 { 1 } else { -1 };
    let sc = if c < c1 { 1 } else { -1 };
    let mut err = dc - dr;
    let mut out = Vec::new();
    loop {
        out.push((r as usize, c as usize));
        if r == r1 && c == c1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dr {
            err -= dr;
            c += sc;
        }
        if e2 < dc {
            err += dc;
            r += sr;
        }
    }
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(cells_between)'`
Expected: PASS — 6 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add paint-stroke line enumeration"
```

---

### Task 4: Draw the move grip on hover

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`

Rendering — verified by compilation; visual check in Task 8.

- [ ] **Step 1: Add the cursor parameter and grip drawing to `draw_grid`**

In `multosis/src/editor/grid_view.rs`, change the `draw_grid` signature to add a trailing `cursor` parameter:

```rust
pub fn draw_grid(pixmap: &mut Pixmap, grid: &Grid, scale: f32, cursor: Option<(f32, f32)>) {
```

(Keep the existing parameter names/types for `pixmap`/`grid`/`scale` — only ADD `cursor`.)

At the END of `draw_grid`'s body — after the four edge-nub `draw_rect` calls, before the closing brace — append:

```rust
    // Move grip — drawn only while the cursor is inside the loop region.
    if let Some((cur_x, cur_y)) = cursor {
        if cur_x >= x0 && cur_x <= (x1 + w1) && cur_y >= y0 && cur_y <= (y1 + h1) {
            let (gx, gy, gw, gh) = region_grip_rect(lr, scale);
            widgets::draw_rect(pixmap, gx, gy, gw, gh, color_loop());
        }
    }
```

NOTE: `lr`, `x0`, `y0`, `x1`, `y1`, `w1`, `h1` are the locals the existing region-outline / edge-nub code already binds in `draw_grid`. Confirm their names by reading the function; if a name differs, use the actual one. Do NOT change the existing cell / outline / nub drawing.

- [ ] **Step 2: Update the `draw_grid` call site**

In `multosis/src/editor.rs`, `MultosisWindow::draw` calls `grid_view::draw_grid(&mut self.surface.pixmap, &grid, self.scale_factor)`. Add the cursor argument:

```rust
        grid_view::draw_grid(
            &mut self.surface.pixmap,
            &grid,
            self.scale_factor,
            Some(self.mouse_pos),
        );
```

Adapt the receiver expressions (`&grid` / pixmap) to whatever the existing call uses — only ADD the `Some(self.mouse_pos)` argument.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. Run `cargo nextest run -p multosis` — PASS, 128 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs
git commit -m "feat(multosis): draw the loop-region move grip on hover"
```

---

### Task 5: `LeftGesture` enum + deferred grid clicks

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation. No new unit tests (the editor's event handling is not unit-tested in this codebase; the pure logic is covered by Tasks 1–3).

This task replaces `region_drag: Option<RegionEdge>` with a unified `left_gesture: Option<LeftGesture>`, migrates the existing region-edge resize into it, and makes left grid clicks *deferred* — recorded on press, applied on release — which is the precondition for drag-paint (Task 6).

- [ ] **Step 1: Add the `LeftGesture` enum**

In `multosis/src/editor.rs`, add near the top (after the `use` imports, before `struct MultosisWindow`):

```rust
/// The in-progress left-button gesture on the grid or loop region. The press
/// dispatch in `on_event` selects exactly one; `None` means no left drag.
#[derive(Clone, Copy, Debug)]
enum LeftGesture {
    /// Dragging a loop-region edge to resize it.
    ResizeRegion(grid_view::RegionEdge),
    /// A left press on a grid cell that has not moved yet — a click in
    /// waiting. Becomes a click on release, or a paint drag if the cursor
    /// leaves the cell (Task 6).
    GridPending {
        row: usize,
        col: usize,
        zone: grid_view::CellZone,
    },
}
```

- [ ] **Step 2: Replace the `region_drag` field**

In the `MultosisWindow` struct, replace the field

```rust
    region_drag: Option<grid_view::RegionEdge>,
```

with

```rust
    /// The active left-button gesture, if any.
    left_gesture: Option<LeftGesture>,
```

In `MultosisWindow::new`, replace `region_drag: None,` in the returned struct literal with `left_gesture: None,`.

- [ ] **Step 3: Extract `commit_click` and rewire `handle_grid_click`**

In the `impl MultosisWindow` block, add a `commit_click` method and rewrite `handle_grid_click` to use it (place `commit_click` right before `handle_grid_click`):

```rust
    /// Apply a resolved cell edit and republish the grid.
    fn commit_click(&mut self, row: usize, col: usize, zone: grid_view::CellZone, right: bool) {
        if let Ok(mut grid) = self.params.grid.lock() {
            grid_view::apply_grid_click(&mut grid, row, col, zone, right);
            self.grid_handoff.publish(*grid);
        }
    }

    /// Apply a click at the current cursor position (used for right-click,
    /// which still edits on press).
    fn handle_grid_click(&mut self, right: bool) {
        let (px, py) = self.mouse_pos;
        if let Some((row, col, zone)) = grid_view::cell_zone(px, py, self.scale_factor) {
            self.commit_click(row, col, zone, right);
        }
    }
```

(If the existing `handle_grid_click` body differs, the net result must be: `handle_grid_click` resolves the cell+zone from `mouse_pos` and delegates to `commit_click`; `commit_click` does the lock + `apply_grid_click` + republish.)

- [ ] **Step 4: Dispatch a left grid press to `GridPending`**

In `on_event`, the `ButtonPressed { Left }` arm's inner `op_hit` `None` arm currently reads:

```rust
                        None => {
                            if let Some(edge) = self.region_edge_under_cursor() {
                                self.region_drag = Some(edge);
                            } else {
                                self.handle_grid_click(false);
                            }
                        }
```

Replace it with:

```rust
                        None => {
                            if let Some(edge) = self.region_edge_under_cursor() {
                                self.left_gesture = Some(LeftGesture::ResizeRegion(edge));
                            } else if let Some((row, col, zone)) =
                                grid_view::cell_zone(px, py, self.scale_factor)
                            {
                                self.left_gesture =
                                    Some(LeftGesture::GridPending { row, col, zone });
                            }
                        }
```

Leave the `Some(...)` arms and the right-click handler unchanged. Match the real `px`/`py` names.

- [ ] **Step 5: Update `CursorMoved` and `ButtonReleased`**

In `on_event`'s `CursorMoved` arm, replace the existing region-drag line

```rust
                if let Some(edge) = self.region_drag {
                    self.update_region_drag(edge);
                }
```

with a match on the gesture:

```rust
                match self.left_gesture {
                    Some(LeftGesture::ResizeRegion(edge)) => self.update_region_drag(edge),
                    Some(LeftGesture::GridPending { .. }) => {}
                    None => {}
                }
```

In `on_event`'s `ButtonReleased { Left }` arm, replace `self.region_drag = None;` with:

```rust
                if let Some(LeftGesture::GridPending { row, col, zone }) = self.left_gesture {
                    self.commit_click(row, col, zone, false);
                }
                self.left_gesture = None;
```

(Keep the existing slider-end logic in that arm unchanged.)

- [ ] **Step 6: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings. Run `cargo nextest run -p multosis` — PASS, 128 tests. Run `cargo clippy -p multosis -- -D warnings` — clean.

(Behaviour is unchanged for the user except that a left grid click now toggles on mouse-release instead of mouse-press, and region-edge resize is now routed through `LeftGesture::ResizeRegion`.)

- [ ] **Step 7: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): unify left-button gestures and defer grid clicks"
```

---

### Task 6: Drag-paint cell toggling

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation.

- [ ] **Step 1: Add the `GridPaint` variant**

In `multosis/src/editor.rs`, add a variant to the `LeftGesture` enum:

```rust
    /// An active paint drag — `value` is the `enabled` state being painted
    /// across the stroke, `last` is the most recently painted cell.
    GridPaint { value: bool, last: (usize, usize) },
```

- [ ] **Step 2: Add the `paint_cells` helper**

Add a method to the `impl MultosisWindow` block (after `commit_click`):

```rust
    /// Set `enabled = value` on every given cell and republish the grid.
    fn paint_cells(&mut self, value: bool, cells: &[(usize, usize)]) {
        if cells.is_empty() {
            return;
        }
        if let Ok(mut grid) = self.params.grid.lock() {
            for &(row, col) in cells {
                grid.cell_mut(row, col).enabled = value;
            }
            self.grid_handoff.publish(*grid);
        }
    }
```

NOTE: confirm `Grid::cell_mut(row, col) -> &mut Cell` and the public `Cell::enabled: bool` field by reading `multosis/src/grid.rs`. Adapt if the names differ.

- [ ] **Step 3: Determine the Shift modifier source**

The paint value is `enabled = true` normally, `enabled = false` while Shift is held. Determine how baseview exposes modifier state on `MouseEvent::CursorMoved`:

- Read `multosis/src/editor.rs`'s `CursorMoved` arm and search the workspace for how a held modifier is read during a mouse drag (the MSEG editor's modifier-held draw — try `rg -n "modifiers" --type rust` across the crates and `nih-plug-widgets`/`tiny-skia-widgets`, and inspect baseview's `MouseEvent` definition).
- If `MouseEvent::CursorMoved` carries a `modifiers` field, destructure it in the `CursorMoved` arm (`CursorMoved { position, modifiers, .. }`) and compute `let shift = <modifiers shift test>;` using baseview's real API (e.g. a `.shift()` method or a `SHIFT` flag — use whatever the codebase/baseview actually provides).
- If `CursorMoved` does NOT carry modifiers, instead add a `shift_held: bool` field to `MultosisWindow` (initialised `false` in `new`), and set it from `Event::Keyboard` key-down/up of the Shift key — add a `Keyboard` arm to `on_event` if one does not exist. Use `self.shift_held` as `shift`.

Use whichever mechanism the real API supports. The rest of this task refers to a boolean `shift` that is true when Shift is held.

- [ ] **Step 4: Transition `GridPending` → `GridPaint` and paint on `CursorMoved`**

In `on_event`'s `CursorMoved` arm, replace the gesture match from Task 5 with:

```rust
                match self.left_gesture {
                    Some(LeftGesture::ResizeRegion(edge)) => self.update_region_drag(edge),
                    Some(LeftGesture::GridPending { row, col, zone: _ }) => {
                        let cur = (
                            grid_view::row_at(py, self.scale_factor),
                            grid_view::column_at(px, self.scale_factor),
                        );
                        if cur != (row, col) {
                            // The press has become a paint drag.
                            let value = !shift;
                            let cells = grid_view::cells_between((row, col), cur);
                            self.paint_cells(value, &cells);
                            self.left_gesture =
                                Some(LeftGesture::GridPaint { value, last: cur });
                        }
                    }
                    Some(LeftGesture::GridPaint { value, last }) => {
                        let cur = (
                            grid_view::row_at(py, self.scale_factor),
                            grid_view::column_at(px, self.scale_factor),
                        );
                        if cur != last {
                            let cells = grid_view::cells_between(last, cur);
                            self.paint_cells(value, &cells);
                            self.left_gesture =
                                Some(LeftGesture::GridPaint { value, last: cur });
                        }
                    }
                    None => {}
                }
```

`px`/`py` are the cursor coordinates the `CursorMoved` arm already binds; `shift` is from Step 3.

- [ ] **Step 5: Update `ButtonReleased`**

The `ButtonReleased { Left }` arm from Task 5 already commits a click only for `LeftGesture::GridPending` and clears `left_gesture` for everything else. A `GridPaint` gesture therefore correctly does nothing extra on release (the cells were painted live) and is cleared. No change needed — but confirm the arm still reads:

```rust
                if let Some(LeftGesture::GridPending { row, col, zone }) = self.left_gesture {
                    self.commit_click(row, col, zone, false);
                }
                self.left_gesture = None;
```

- [ ] **Step 6: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings. Run `cargo nextest run -p multosis` — PASS, 128 tests. Run `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 7: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): drag-paint cell enabled state"
```

---

### Task 7: Move the loop region with the grip

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation.

- [ ] **Step 1: Add the `MoveRegion` variant**

In `multosis/src/editor.rs`, add a variant to the `LeftGesture` enum:

```rust
    /// An active loop-region move — `press` is the cursor position when the
    /// grip was grabbed, `region_at_press` is the region geometry then.
    MoveRegion {
        press: (f32, f32),
        region_at_press: LoopRegion,
    },
```

NOTE: this needs `LoopRegion` in scope. Add `use crate::grid::LoopRegion;` to the imports if it is not already there (the editor already imports `Grid` from `crate::grid`).

- [ ] **Step 2: Add the move helpers**

Add two methods to the `impl MultosisWindow` block (after `region_edge_under_cursor`):

```rust
    /// If the cursor is over the loop region's move grip, begin a move
    /// gesture and return `true`.
    fn try_begin_region_move(&mut self) -> bool {
        let region = match self.params.grid.lock() {
            Ok(grid) => grid.loop_region,
            Err(_) => return false,
        };
        let (px, py) = self.mouse_pos;
        if grid_view::region_grip_hit(px, py, region, self.scale_factor) {
            self.left_gesture = Some(LeftGesture::MoveRegion {
                press: self.mouse_pos,
                region_at_press: region,
            });
            true
        } else {
            false
        }
    }

    /// Translate the loop region for the in-progress move and republish.
    fn update_region_move(&mut self, press: (f32, f32), region_at_press: LoopRegion) {
        let scale = self.scale_factor;
        let (px, py) = self.mouse_pos;
        let drow = grid_view::row_at(py, scale) as i32 - grid_view::row_at(press.1, scale) as i32;
        let dcol =
            grid_view::column_at(px, scale) as i32 - grid_view::column_at(press.0, scale) as i32;
        if let Ok(mut grid) = self.params.grid.lock() {
            grid.loop_region = grid_view::apply_region_move(region_at_press, drow, dcol);
            self.grid_handoff.publish(*grid);
        }
    }
```

NOTE: `try_begin_region_move` reads `loop_region`, releasing the lock before any further work (the `match` block ends, dropping the guard). `update_region_move` recomputes the translation absolutely from `region_at_press` each move, so it cannot drift.

- [ ] **Step 3: Dispatch the grip press**

In `on_event`'s `ButtonPressed { Left }` arm, the inner `op_hit` `None` arm (rewritten in Task 5) currently chooses between region-edge resize and `GridPending`. Insert the grip check between them:

```rust
                        None => {
                            if let Some(edge) = self.region_edge_under_cursor() {
                                self.left_gesture = Some(LeftGesture::ResizeRegion(edge));
                            } else if self.try_begin_region_move() {
                                // left_gesture set inside try_begin_region_move
                            } else if let Some((row, col, zone)) =
                                grid_view::cell_zone(px, py, self.scale_factor)
                            {
                                self.left_gesture =
                                    Some(LeftGesture::GridPending { row, col, zone });
                            }
                        }
```

This realises the §3 priority: toolbar → op → region edge → move grip → grid cell.

- [ ] **Step 4: Drive the move on `CursorMoved`**

In `on_event`'s `CursorMoved` gesture match (from Task 6), add a `MoveRegion` arm:

```rust
                    Some(LeftGesture::MoveRegion {
                        press,
                        region_at_press,
                    }) => self.update_region_move(press, region_at_press),
```

Place it alongside the `ResizeRegion` / `GridPending` / `GridPaint` arms. The `ButtonReleased { Left }` arm needs no change — it already clears `left_gesture` and only commits a click for `GridPending`.

- [ ] **Step 5: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings (all four `LeftGesture` variants are now constructed). Run `cargo nextest run -p multosis` — PASS, 128 tests. Run `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 6: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): move the loop region with the hover grip"
```

---

### Task 8: Verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — 128 tests (114 pre-existing + `apply_region_move` ×5 + `region_grip` ×3 + `cells_between` ×6).

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
- Clicking a single cell still toggles it (center → enabled, octant → send, right-click → start). The toggle now happens on mouse-release.
- Left-dragging across the grid enables every cell the stroke crosses; a fast drag skips no cell.
- Holding **Shift** while dragging disables the crossed cells instead.
- A small move grip appears at the loop region's center only while the cursor is inside the region, and is gone when the cursor is elsewhere.
- Dragging the grip moves the whole region; the region keeps its size and stops at the grid edges.
- Loop-region edge-resize still works; the six toolbar operations and the status readout still work.

Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for grid-interaction enhancements"
```

If Step 2 produced no edits, skip this commit.

---

## Definition of done

- Drag-paint sets `enabled` across a stroke (Shift inverts to disable), skipping no cell; single clicks still toggle one cell on release.
- A hover-revealed move grip translates the loop region as a unit, clamped on-grid; edge-resize is unaffected.
- `cargo nextest run -p multosis` is green (128 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.

## Spec coverage check (self-review)

- §1 Drag-paint — deferred click (`GridPending`, Task 5), the `GridPaint` transition + line fill (`cells_between` Task 3, wired Task 6), Shift inverts the painted value (Task 6 Step 3–4), left-button only / right-click unchanged (Tasks 5–6 leave the right-click path alone).
- §2 Move loop region — `apply_region_move` (Task 1), the grip geometry `region_grip_rect`/`region_grip_hit` (Task 2), hover rendering (Task 4), the move gesture (Task 7). Tiny-region handling: `region_grip_rect` shrinks the grip to fit (Task 2 test `region_grip_rect_fits_inside_a_1x1_region`).
- §3 Interaction priority — the `ButtonPressed{Left}` dispatch order toolbar → op → region edge → move grip → grid cell (Task 7 Step 3).
- §4 Rendering — `draw_grid` gains a `cursor` parameter and draws the grip when the cursor is inside the region (Task 4).
- §5 Components — pure functions in `grid_view.rs` (`apply_region_move`, `region_grip_rect`/`region_grip_hit`, `cells_between`); the unified `LeftGesture` enum replacing parallel `Option` fields (Task 5, extended Tasks 6–7); paint/move both mutate `params.grid` and republish via `grid_handoff` (Tasks 6–7).
- §6 Testing — TDD unit tests for every pure function incl. 1×1 / tiny-region and long-jump cases (Tasks 1–3); editor wiring verified by warning-free build + clippy + the smoke test (Tasks 5–8).
- Out of scope (connection-indicator polish, cramped grid, right-drag, send/start painting) — not implemented, as the spec directs.
