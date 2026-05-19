# Multosis Connection-Indicator Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Multosis grid's square send pips with directional arrowheads — cardinals at edge midpoints, diagonals at corners, in a brighter blue.

**Architecture:** All changes live in `multosis/src/editor/grid_view.rs`. A pure, unit-tested geometry function computes an arrowhead's three triangle vertices for a given cell and send direction; a small local helper fills a triangle via tiny-skia; `draw_cell`'s send loop swaps the square-pip `draw_rect` for an arrowhead fill, and `color_send()` is brightened.

**Tech Stack:** Rust (nightly), nih-plug, tiny-skia, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-18-multosis-connection-indicators-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (128 multosis tests, 905 workspace tests green):**
- `multosis/src/editor/grid_view.rs` imports `use crate::grid::{Direction, Grid, LoopRegion, COLS, ROWS};`. `Direction` is the 8-compass enum (`N, NE, E, SE, S, SW, W, NW`), derives `Clone, Copy, PartialEq, Eq, Debug`, has `Direction::ALL` (`[Direction; 8]`) and `delta() -> (i32, i32)` returning `(dr, dc)` with components in `{-1,0,1}` (cardinals have one zero component, diagonals none).
- `color_send()` in `grid_view.rs` returns `tiny_skia::Color::from_rgba8(0x6f, 0x8a, 0xb8, 0xFF)`.
- `draw_cell(pixmap: &mut Pixmap, row: usize, col: usize, cell: &crate::grid::Cell, scale: f32)` draws the cell background, then a loop over `Direction::ALL` that for each `cell.sends_to(dir)` draws a square pip with `widgets::draw_rect`, then the start marker. Inside `draw_cell`, `(x, y, w, h)` is the cell rect, `cx = x + w/2.0`, `cy = y + h/2.0` the centre, `w == h`. The current send loop:
  ```rust
      let pip = w * 0.16;
      for dir in Direction::ALL {
          if !cell.sends_to(dir) {
              continue;
          }
          let (dr, dc) = dir.delta();
          let px = cx + dc as f32 * w * 0.34 - pip / 2.0;
          let py = cy + dr as f32 * h * 0.34 - pip / 2.0;
          widgets::draw_rect(pixmap, px, py, pip, pip, color_send());
      }
  ```
- `Pixmap` is `tiny_skia::Pixmap`. `tiny-skia-widgets/src/primitives.rs` and the other plugin editors (e.g. `six-pack/src/editor/curve_view.rs`, `pope-scope/src/renderer.rs`) already build `tiny_skia` paths and `Paint`s and fill them — a reference for the exact tiny-skia API in the pinned version.
- `grid_view.rs` has a `#[cfg(test)] mod tests` block with `use super::*;`.

---

### Task 1: `arrowhead_vertices` — arrowhead geometry

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/editor/grid_view.rs`:

```rust
    #[test]
    fn arrowhead_vertices_stay_within_the_cell() {
        let (cx, cy, w) = (100.0_f32, 100.0_f32, 33.0_f32);
        let (left, right) = (cx - w / 2.0, cx + w / 2.0);
        let (top, bottom) = (cy - w / 2.0, cy + w / 2.0);
        for dir in Direction::ALL {
            for (vx, vy) in arrowhead_vertices(cx, cy, w, dir, 1.0) {
                assert!(
                    vx >= left && vx <= right && vy >= top && vy <= bottom,
                    "{dir:?} vertex ({vx}, {vy}) outside cell [{left}..{right}, {top}..{bottom}]"
                );
            }
        }
    }

    #[test]
    fn arrowhead_tip_points_outward() {
        let (cx, cy, w) = (100.0_f32, 100.0_f32, 33.0_f32);
        for dir in Direction::ALL {
            let verts = arrowhead_vertices(cx, cy, w, dir, 1.0);
            let (dr, dc) = dir.delta();
            let (fx, fy) = (dc as f32, dr as f32);
            let len = (fx * fx + fy * fy).sqrt();
            let (ux, uy) = (fx / len, fy / len);
            // Project each vertex onto the send direction; vertex 0 (the tip)
            // must be the furthest out.
            let proj = |(vx, vy): (f32, f32)| (vx - cx) * ux + (vy - cy) * uy;
            let tip = proj(verts[0]);
            assert!(tip > proj(verts[1]), "{dir:?}: tip not outermost vs v1");
            assert!(tip > proj(verts[2]), "{dir:?}: tip not outermost vs v2");
        }
    }

    #[test]
    fn arrowhead_cardinal_tip_on_edge_midline() {
        let (cx, cy, w) = (100.0_f32, 100.0_f32, 33.0_f32);
        // East tip: on the horizontal centreline, to the right of centre.
        let e = arrowhead_vertices(cx, cy, w, Direction::E, 1.0)[0];
        assert!((e.1 - cy).abs() < 0.01, "E tip off the midline: {e:?}");
        assert!(e.0 > cx, "E tip not to the right: {e:?}");
        // North tip: on the vertical centreline, above centre.
        let n = arrowhead_vertices(cx, cy, w, Direction::N, 1.0)[0];
        assert!((n.0 - cx).abs() < 0.01, "N tip off the midline: {n:?}");
        assert!(n.1 < cy, "N tip not above centre: {n:?}");
    }

    #[test]
    fn arrowhead_diagonal_tip_near_corner() {
        let (cx, cy, w) = (100.0_f32, 100.0_f32, 33.0_f32);
        let ne = arrowhead_vertices(cx, cy, w, Direction::NE, 1.0)[0];
        let corner = (cx + w / 2.0, cy - w / 2.0);
        let dist = ((ne.0 - corner.0).powi(2) + (ne.1 - corner.1).powi(2)).sqrt();
        assert!(dist < 3.0, "NE tip {ne:?} not near corner {corner:?} (dist {dist})");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(arrowhead)'`
Expected: build failure — `cannot find function arrowhead_vertices`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, immediately before the `draw_cell` function:

```rust
/// Logical pixels the arrowhead tip is held in from the cell edge / corner.
const ARROW_INSET: f32 = 1.5;
/// Arrowhead length along the send direction, as a fraction of the cell side.
const ARROW_LEN_FRAC: f32 = 0.22;
/// Arrowhead half-width perpendicular to the direction, fraction of the side.
const ARROW_HALFWIDTH_FRAC: f32 = 0.13;

/// The three triangle vertices of the send-direction arrowhead for a cell
/// centred at `(cx, cy)` with side `w`. Vertex 0 is the outward-pointing tip:
/// at the edge midpoint for a cardinal direction, at the cell corner for a
/// diagonal — held `ARROW_INSET` logical px inside the cell so nothing
/// overhangs into the inter-cell gap.
pub fn arrowhead_vertices(
    cx: f32,
    cy: f32,
    w: f32,
    dir: Direction,
    scale: f32,
) -> [(f32, f32); 3] {
    let (dr, dc) = dir.delta();
    // Unit vector along the send direction (x = dc, y = dr).
    let (fx, fy) = (dc as f32, dr as f32);
    let len = (fx * fx + fy * fy).sqrt();
    let (ux, uy) = (fx / len, fy / len);
    // Unit perpendicular.
    let (perp_x, perp_y) = (-uy, ux);
    // Distance from the centre to the boundary along `dir`: half a side for a
    // cardinal, half the diagonal for a diagonal.
    let diagonal = dr != 0 && dc != 0;
    let boundary = if diagonal {
        0.5 * w * std::f32::consts::SQRT_2
    } else {
        0.5 * w
    };
    let tip_dist = boundary - ARROW_INSET * scale;
    let tip = (cx + ux * tip_dist, cy + uy * tip_dist);
    let head_len = ARROW_LEN_FRAC * w;
    let half_w = ARROW_HALFWIDTH_FRAC * w;
    let base = (tip.0 - ux * head_len, tip.1 - uy * head_len);
    [
        tip,
        (base.0 + perp_x * half_w, base.1 + perp_y * half_w),
        (base.0 - perp_x * half_w, base.1 - perp_y * half_w),
    ]
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(arrowhead)'`
Expected: PASS — 4 tests. Then `cargo build -p multosis` — compiles (no warnings; `arrowhead_vertices` is `pub`, consistent with the other geometry functions in this file, so it does not trigger `dead_code` before Task 2 wires it in).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add send-arrowhead geometry"
```

---

### Task 2: Draw the arrowheads

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

Rendering — verified by compilation; visual check in Task 3. No new unit tests.

- [ ] **Step 1: Brighten `color_send()`**

In `multosis/src/editor/grid_view.rs`, change the body of `color_send()` to return the brighter blue:

```rust
/// A lit send-direction arrowhead.
fn color_send() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x86, 0xa6, 0xe8, 0xFF)
}
```

(Update the doc comment from "pip" to "arrowhead" as shown.)

- [ ] **Step 2: Add the `fill_triangle` helper**

Add to `multosis/src/editor/grid_view.rs`, immediately before `draw_cell` (after `arrowhead_vertices`):

```rust
/// Fill a triangle with `color`, anti-aliased, via tiny-skia.
fn fill_triangle(pixmap: &mut Pixmap, verts: [(f32, f32); 3], color: tiny_skia::Color) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(verts[0].0, verts[0].1);
    pb.line_to(verts[1].0, verts[1].1);
    pb.line_to(verts[2].0, verts[2].1);
    pb.close();
    let Some(path) = pb.finish() else {
        return;
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );
}
```

IMPORTANT — match the real tiny-skia API. The version is pinned in the workspace lock file. Confirm the exact calls by reading how `tiny-skia-widgets/src/primitives.rs` and an existing path-filling editor (e.g. `six-pack/src/editor/curve_view.rs` or `pope-scope/src/renderer.rs`) build a `PathBuilder`, a `Paint`, and call `fill_path`:
- `Paint` colour setter may be `set_color(Color)` or `set_color_rgba8(r,g,b,a)` — use whatever those files use.
- `fill_path` may be a method on `Pixmap` directly or on `pixmap.as_mut()` (`PixmapMut`) — use the form the existing code uses.
- `PathBuilder::finish()` returns `Option<Path>`.
Keep the intent: an anti-aliased solid-colour triangle fill. If you cannot fill a path, report BLOCKED.

- [ ] **Step 3: Rewire `draw_cell`'s send loop**

In `draw_cell`, replace the entire square-pip send loop (the `let pip = w * 0.16;` line through the `for dir in Direction::ALL { … }` block shown in the pre-existing-state section above) with:

```rust
    // Send arrowheads: a triangle pointing the way each trigger flows.
    for dir in Direction::ALL {
        if !cell.sends_to(dir) {
            continue;
        }
        let verts = arrowhead_vertices(cx, cy, w, dir, scale);
        fill_triangle(pixmap, verts, color_send());
    }
```

`cx`, `cy`, `w`, `scale`, `pixmap`, `cell` are all already in scope in `draw_cell`. Leave the cell-background drawing above the loop and the start-marker drawing below it unchanged. Remove the now-unused `let pip = ...` line (it is part of the replaced block).

- [ ] **Step 4: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings (the old `pip` local is gone; `arrowhead_vertices` and `fill_triangle` are both used).

Run: `cargo nextest run -p multosis` — PASS, 132 tests.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): draw send directions as arrowheads"
```

---

### Task 3: Verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — 132 tests (128 pre-existing + `arrowhead` ×4).

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
- Each send direction draws as an arrowhead pointing outward in that direction, in the brighter blue.
- Cardinal arrowheads sit at the cell edge midpoints; diagonal arrowheads sit at the cell corners.
- No arrowhead overhangs into a neighbouring cell.
- A cell with all eight sends, and a dense patch of routed cells, both stay readable.
- Clicking an octant still toggles that send (hit-testing unaffected); the start marker, wavefront, and loop region are unchanged.

If the arrowhead proportions need adjusting, tune `ARROW_INSET`, `ARROW_LEN_FRAC`, `ARROW_HALFWIDTH_FRAC` in `grid_view.rs` and re-check. Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 or Step 4 required edits)**

```bash
git add multosis/
git commit -m "style(multosis): tune connection-indicator rendering"
```

If Steps 2 and 4 produced no edits, skip this commit.

---

## Definition of done

- Grid send directions render as outward-pointing arrowheads (cardinals at edge midpoints, diagonals at corners) in the brighter `#86a6e8` blue; nothing overhangs.
- `cargo nextest run -p multosis` is green (132 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.

## Spec coverage check (self-review)

- §1 Visual design — arrowhead shape, cardinal-at-edge / diagonal-at-corner placement, inset (no overhang), `#86a6e8` colour, proportional size: `arrowhead_vertices` (Task 1) + `color_send()` change + `draw_cell` rewire (Task 2).
- §2 Arrowhead geometry — the unit/perp/boundary/tip/base model is `arrowhead_vertices` (Task 1), implemented exactly as the spec's formula.
- §3 Implementation — `color_send()` (Task 2 Step 1), the pure geometry fn (Task 1), the local `fill_triangle` helper (Task 2 Step 2), the `draw_cell` loop swap (Task 2 Step 3). Triangle fills via tiny-skia anti-aliased path fill; the performance note is acknowledged and explicitly left for the later UI-performance backlog item.
- §4 Unchanged — `cell_zone` hit-testing, routing/`Direction`/`Cell`, the start marker, wavefront, loop region: untouched (Task 2 changes only the send loop in `draw_cell`).
- §5 Testing — unit tests assert vertices stay within the cell, the tip points outward, cardinals on the edge midline, diagonals near the corner (Task 1); build/clippy + the smoke test cover rendering (Tasks 2–3); smoke-test tuning of the size constants (Task 3 Step 4).
- Out of scope (cramped grid, UI performance pass, route colour-coding) — not implemented, as the spec directs.
