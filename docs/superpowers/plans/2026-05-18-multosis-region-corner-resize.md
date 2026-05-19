# Multosis Loop-Region Corner Resize — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the loop region be resized from its corners — a corner drag moves both bounds at once.

**Architecture:** `grid_view.rs` gains a `RegionCorner` enum and a `RegionHandle` (edge or corner); `region_edge_hit` is replaced by `region_handle_hit`, which detects a corner where both edge bands meet. A pure `apply_region_corner_drag` composes the existing per-edge `apply_region_drag` twice. The editor's `ResizeRegion` gesture carries a `RegionHandle`.

**Tech Stack:** Rust (nightly), nih-plug, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-18-multosis-region-corner-resize-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Why one implementation task:** the change replaces `region_edge_hit` (used by `editor.rs`) and re-types the `LeftGesture::ResizeRegion` variant. Splitting `grid_view.rs` and `editor.rs` into separate tasks would leave the crate non-compiling between them — so its tests could not run. The feature is small and cohesive; it lands as one task, then a verification task.

**Pre-existing state (138 multosis tests, 915 workspace tests green):**
- `multosis/src/editor/grid_view.rs`: `pub enum RegionEdge { Left, Right, Top, Bottom }` (derives `Clone, Copy, PartialEq, Eq, Debug`); `const EDGE_BAND: f32 = 6.0`; layout consts `STATUS_H`, `GUTTER`, `MARGIN`, `CELL`, `GROUP_SIZE`, `GROUP_GAP`, and `ROWS`/`COLS` from `crate::grid`.
  - `region_edge_hit(px, py, region: LoopRegion, scale) -> Option<RegionEdge>` — an off-grid guard, then `cell_rect`-based detection within `EDGE_BAND*scale` of an edge line; at a corner it returns the first edge matched.
  - `apply_region_drag(region: LoopRegion, edge: RegionEdge, index: usize) -> LoopRegion` — moves the one bound `edge` names to grid `index`, clamped against the opposite bound (never inverts; can collapse to 1×1).
  - `column_at(px, scale) -> usize`, `row_at(py, scale) -> usize` — cursor → clamped grid index.
  - `#[cfg(test)] mod tests` has two tests exercising `region_edge_hit` (named like `region_edge_hit_finds_each_edge` and `region_edge_hit_misses_interior_and_toolbar`): they call `region_edge_hit` and expect `Some(RegionEdge::…)` / `None`, with the tested points at the edge *midpoints* (far from corners).
- `multosis/src/editor.rs`: `enum LeftGesture { ResizeRegion(grid_view::RegionEdge), … }` (derives `Clone, Copy, Debug`); `fn region_edge_under_cursor(&self) -> Option<grid_view::RegionEdge>` (locks `params.grid`, reads `loop_region`, calls `region_edge_hit`); `fn update_region_drag(&mut self, edge: grid_view::RegionEdge)` (maps the cursor to a column index for `Left`/`Right` or a row index for `Top`/`Bottom` via `column_at`/`row_at`, then `apply_region_drag` on `grid.loop_region`, then `grid_handoff.publish(*grid)`). `on_event`: a `ButtonPressed{Left}` arm sets `LeftGesture::ResizeRegion(edge)` from `region_edge_under_cursor()`; the `CursorMoved` gesture match has `Some(LeftGesture::ResizeRegion(edge)) => self.update_region_drag(edge)`.
- The only caller of `region_edge_hit` is `region_edge_under_cursor`.

---

### Task 1: Corner-resize — hit-testing, resize logic, and editor wiring

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`

- [ ] **Step 1: Write the failing tests**

In `grid_view.rs`'s `#[cfg(test)] mod tests` block:

(a) Find the two existing tests that call `region_edge_hit`. **Rename** them to `region_handle_hit_finds_each_edge` and `region_handle_hit_misses_interior_and_toolbar` (keeping whatever each does). In each, change every `region_edge_hit(` call to `region_handle_hit(`, and wrap every expected `Some(RegionEdge::X)` in `Some(RegionHandle::Edge(RegionEdge::X))`. `None` expectations stay `None`. The tested points sit at edge *midpoints*, far from any corner, so they remain `Edge` results.

(b) **Add** these four tests:

```rust
    #[test]
    fn region_handle_hit_finds_each_corner() {
        let region = LoopRegion {
            row0: 2,
            row1: 9,
            col0: 5,
            col1: 22,
        };
        let (x0, y0, _, _) = cell_rect(2, 5, 1.0);
        let (x1, y1, w, h) = cell_rect(9, 22, 1.0);
        let right = x1 + w;
        let bottom = y1 + h;
        assert_eq!(
            region_handle_hit(x0, y0, region, 1.0),
            Some(RegionHandle::Corner(RegionCorner::NW))
        );
        assert_eq!(
            region_handle_hit(right, y0, region, 1.0),
            Some(RegionHandle::Corner(RegionCorner::NE))
        );
        assert_eq!(
            region_handle_hit(x0, bottom, region, 1.0),
            Some(RegionHandle::Corner(RegionCorner::SW))
        );
        assert_eq!(
            region_handle_hit(right, bottom, region, 1.0),
            Some(RegionHandle::Corner(RegionCorner::SE))
        );
    }

    #[test]
    fn apply_region_corner_drag_moves_both_bounds() {
        let r = LoopRegion {
            row0: 4,
            row1: 12,
            col0: 6,
            col1: 20,
        };
        // NW corner to (row 7, col 9): row0 and col0 move; row1/col1 hold.
        let nw = apply_region_corner_drag(r, RegionCorner::NW, 7, 9);
        assert_eq!((nw.row0, nw.col0), (7, 9));
        assert_eq!((nw.row1, nw.col1), (12, 20));
        // SE corner to (row 15, col 28): row1 and col1 move; row0/col0 hold.
        let se = apply_region_corner_drag(r, RegionCorner::SE, 15, 28);
        assert_eq!((se.row1, se.col1), (15, 28));
        assert_eq!((se.row0, se.col0), (4, 6));
    }

    #[test]
    fn apply_region_corner_drag_cannot_invert() {
        let r = LoopRegion {
            row0: 4,
            row1: 12,
            col0: 6,
            col1: 20,
        };
        // Drag NW far past the opposite bounds — clamps, never inverts.
        let out = apply_region_corner_drag(r, RegionCorner::NW, 99, 99);
        assert_eq!((out.row0, out.col0), (12, 20));
        assert!(out.row0 <= out.row1 && out.col0 <= out.col1);
    }

    #[test]
    fn apply_region_corner_drag_can_collapse_to_1x1() {
        let r = LoopRegion {
            row0: 4,
            row1: 12,
            col0: 6,
            col1: 20,
        };
        let out = apply_region_corner_drag(r, RegionCorner::NW, 12, 20);
        assert_eq!((out.row0, out.row1, out.col0, out.col1), (12, 12, 20, 20));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(region_handle_hit) + test(apply_region_corner_drag)'`
Expected: build failure — `cannot find type RegionHandle` / `RegionCorner`, functions `region_handle_hit` / `apply_region_corner_drag` not found.

- [ ] **Step 3: Implement — `grid_view.rs`**

**3a — add the types.** After the `RegionEdge` enum, add:

```rust
/// One corner of the loop-region rectangle — the meeting of a vertical edge
/// and a horizontal edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionCorner {
    /// North-west — Left + Top.
    NW,
    /// North-east — Right + Top.
    NE,
    /// South-west — Left + Bottom.
    SW,
    /// South-east — Right + Bottom.
    SE,
}

impl RegionCorner {
    /// The `(vertical, horizontal)` edges this corner is made of.
    pub fn edges(self) -> (RegionEdge, RegionEdge) {
        match self {
            RegionCorner::NW => (RegionEdge::Left, RegionEdge::Top),
            RegionCorner::NE => (RegionEdge::Right, RegionEdge::Top),
            RegionCorner::SW => (RegionEdge::Left, RegionEdge::Bottom),
            RegionCorner::SE => (RegionEdge::Right, RegionEdge::Bottom),
        }
    }
}

/// A draggable loop-region resize handle — an edge (one axis) or a corner
/// (both axes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionHandle {
    Edge(RegionEdge),
    Corner(RegionCorner),
}
```

**3b — replace `region_edge_hit` with `region_handle_hit`.** Replace the entire `region_edge_hit` function with:

```rust
/// The loop-region resize handle under physical-pixel point `(px, py)` at
/// `scale`, or `None`. A point within `EDGE_BAND` of a corner in *both* axes
/// is a `Corner`; within `EDGE_BAND` of one edge line (along the region's
/// span) is an `Edge`. Points in the toolbar strip / gutter / margins never
/// hit a handle.
pub fn region_handle_hit(
    px: f32,
    py: f32,
    region: LoopRegion,
    scale: f32,
) -> Option<RegionHandle> {
    // Reject the toolbar strip, the gutter, the margins — anything off-grid.
    let grid_top = (STATUS_H + GUTTER) * scale;
    let grid_bottom = (STATUS_H + GUTTER + ROWS as f32 * CELL) * scale;
    let grid_left = MARGIN * scale;
    let grid_right =
        (MARGIN + COLS as f32 * CELL + (COLS / GROUP_SIZE - 1) as f32 * GROUP_GAP) * scale;
    if py < grid_top || py > grid_bottom || px < grid_left || px > grid_right {
        return None;
    }
    let (x0, y0, _, _) = cell_rect(region.row0, region.col0, scale);
    let (x1, y1, w1, h1) = cell_rect(region.row1, region.col1, scale);
    let right = x1 + w1;
    let bottom = y1 + h1;
    let band = EDGE_BAND * scale;
    let near_left = (px - x0).abs() <= band;
    let near_right = (px - right).abs() <= band;
    let near_top = (py - y0).abs() <= band;
    let near_bottom = (py - bottom).abs() <= band;
    // Corners first — within `band` of a corner point in both axes.
    let corner = match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Some(RegionCorner::NW),
        (_, true, true, _) => Some(RegionCorner::NE),
        (true, _, _, true) => Some(RegionCorner::SW),
        (_, true, _, true) => Some(RegionCorner::SE),
        _ => None,
    };
    if let Some(c) = corner {
        return Some(RegionHandle::Corner(c));
    }
    // Then edges — within `band` of an edge line, along the region's span.
    let in_rows = py >= y0 && py <= bottom;
    let in_cols = px >= x0 && px <= right;
    if in_rows && near_left {
        Some(RegionHandle::Edge(RegionEdge::Left))
    } else if in_rows && near_right {
        Some(RegionHandle::Edge(RegionEdge::Right))
    } else if in_cols && near_top {
        Some(RegionHandle::Edge(RegionEdge::Top))
    } else if in_cols && near_bottom {
        Some(RegionHandle::Edge(RegionEdge::Bottom))
    } else {
        None
    }
}
```

**3c — add `apply_region_corner_drag`.** After `apply_region_drag`, add:

```rust
/// Resize the loop region by dragging `corner` to grid cell `(row, col)`,
/// moving both bounds the corner is made of. Composes `apply_region_drag`
/// once per edge, so the corner drag inherits its clamping — it never
/// inverts the region and can collapse it toward 1×1.
pub fn apply_region_corner_drag(
    region: LoopRegion,
    corner: RegionCorner,
    row: usize,
    col: usize,
) -> LoopRegion {
    let (vertical, horizontal) = corner.edges();
    let r = apply_region_drag(region, vertical, col);
    apply_region_drag(r, horizontal, row)
}
```

- [ ] **Step 4: Implement — `editor.rs`**

**4a — the `LeftGesture` variant.** Change:
```rust
    /// Dragging a loop-region edge or corner to resize it.
    ResizeRegion(grid_view::RegionHandle),
```
(replacing `ResizeRegion(grid_view::RegionEdge)`; update the doc comment as shown).

**4b — rename `region_edge_under_cursor`.** Replace it with:
```rust
    /// The loop-region resize handle under the cursor, if the cursor is over
    /// one.
    fn region_handle_under_cursor(&self) -> Option<grid_view::RegionHandle> {
        let (px, py) = self.mouse_pos;
        let region = self.params.grid.lock().ok()?.loop_region;
        grid_view::region_handle_hit(px, py, region, self.scale_factor)
    }
```
(Keep the real lock-and-read shape of the existing `region_edge_under_cursor`; only the name, return type, and hit-test call change.)

**4c — `update_region_drag`.** Replace it with:
```rust
    /// Resize the loop region for the in-progress drag of `handle`, then
    /// republish the grid.
    fn update_region_drag(&mut self, handle: grid_view::RegionHandle) {
        let (px, py) = self.mouse_pos;
        let scale = self.scale_factor;
        if let Ok(mut grid) = self.params.grid.lock() {
            grid.loop_region = match handle {
                grid_view::RegionHandle::Edge(edge) => {
                    let index = match edge {
                        grid_view::RegionEdge::Left | grid_view::RegionEdge::Right => {
                            grid_view::column_at(px, scale)
                        }
                        grid_view::RegionEdge::Top | grid_view::RegionEdge::Bottom => {
                            grid_view::row_at(py, scale)
                        }
                    };
                    grid_view::apply_region_drag(grid.loop_region, edge, index)
                }
                grid_view::RegionHandle::Corner(corner) => grid_view::apply_region_corner_drag(
                    grid.loop_region,
                    corner,
                    grid_view::row_at(py, scale),
                    grid_view::column_at(px, scale),
                ),
            };
            self.grid_handoff.publish(*grid);
        }
    }
```
(If the existing `update_region_drag` locks / publishes slightly differently, keep its real lock-and-publish shape — only the argument type and the `Edge`/`Corner` match are new.)

**4d — the `on_event` call sites.**
- The `ButtonPressed{Left}` arm sets the resize gesture from `region_edge_under_cursor()` — change that call to `region_handle_under_cursor()`; the bound value is now a `RegionHandle`: `self.left_gesture = Some(LeftGesture::ResizeRegion(handle));`.
- The `CursorMoved` gesture match arm `Some(LeftGesture::ResizeRegion(edge)) => self.update_region_drag(edge)` — rename the bound variable to `handle` (it is now a `RegionHandle`); `update_region_drag(handle)` accepts it.

Leave the rest of the dispatch (toolbar, ops, move grip, grid cells, `ButtonReleased`) unchanged.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(region_handle_hit) + test(apply_region_corner_drag)'`
Expected: PASS — 6 tests (the 2 renamed edge tests + the new corner test + 3 `apply_region_corner_drag` tests).

Run: `cargo build -p multosis` — compiles with NO warnings.
Run: `cargo nextest run -p multosis` — PASS, 142 tests.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 6: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs
git commit -m "feat(multosis): resize the loop region from its corners"
```

---

### Task 2: Verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — 142 tests (138 pre-existing + 1 corner hit-test + 3 `apply_region_corner_drag`).

Run: `cargo nextest run --workspace`
Expected: PASS — 919 tests.

- [ ] **Step 2: Lint and format**

Run: `cargo clippy -p multosis -- -D warnings`
Expected: no warnings.

Run: `cargo fmt -p multosis -- --check`
Expected: clean. If it reports a diff, run `cargo fmt -p multosis` and commit it in Step 5.

- [ ] **Step 3: Release build and bundle**

Run: `cargo build --bin multosis --release`
Expected: the standalone binary builds.

Run: `cargo nih-plug bundle multosis --release`
Expected: a VST3 + CLAP bundle is produced with no errors.

- [ ] **Step 4: Manual smoke test**

Run the standalone binary (`cargo run --bin multosis`). Confirm:
- Dragging each of the four loop-region corners resizes the region on both axes at once.
- Dragging an edge still resizes one axis; the move grip still moves the region; the region never inverts and can collapse toward 1×1.
- Cell editing, drag-paint, and the toolbar still work.

Report the smoke-test observations. (This step is a human/visual check.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for region corner resize"
```

If Step 2 produced no edits, skip this commit.

---

## Definition of done

- The loop region resizes from its four corners (both bounds at once) as well as its edges; corner drags clamp and can collapse toward 1×1.
- `cargo nextest run -p multosis` is green (142 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.

## Spec coverage check (self-review)

- §1 The corner model — `RegionCorner` (+`edges()`) and `RegionHandle` (Task 1 Step 3a).
- §2 Hit-testing — `region_handle_hit` replaces `region_edge_hit` (Step 3b); a corner needs both bands, checked before edges; off-grid guard kept.
- §3 Resize logic — `apply_region_corner_drag` composes `apply_region_drag` twice (Step 3c); tests cover both-bounds, no-invert, 1×1 collapse.
- §4 Editor wiring — `LeftGesture::ResizeRegion(RegionHandle)`, `region_handle_under_cursor`, `update_region_drag` with the `Edge`/`Corner` match, the `on_event` call sites (Step 4).
- §5 Unchanged — no rendering change (corners invisible — `draw_grid` untouched); `apply_region_drag`, `column_at`, `row_at`, `EDGE_BAND`, `RegionEdge`, edge resize, the move grip, drag-paint all untouched.
- §6 Testing — `region_handle_hit` corner + edge + miss tests, `apply_region_corner_drag` tests (Task 1), build + smoke test (Task 2).
- Out of scope (corner nubs, the UI performance pass) — not implemented.
