# Multosis Loop-Region Corner Resize — Design

**Date:** 2026-05-18
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

The Multosis grid editor's loop region can be resized by dragging its four edges (each moves one bound). Dragging a *corner* currently resolves to a single edge — so a corner adjusts only one axis. This feature makes a corner grab resize **both** bounds at once, the standard rectangle-resize interaction.

The change is confined to the editor geometry/interaction: `multosis/src/editor/grid_view.rs` and `multosis/src/editor.rs`. No rendering change — corners are invisible grab zones.

## Background — current behaviour

`grid_view.rs` has `pub enum RegionEdge { Left, Right, Top, Bottom }`, a `const EDGE_BAND: f32 = 6.0` (the grab band half-width), and:

- `region_edge_hit(px, py, region, scale) -> Option<RegionEdge>` — returns the region edge under the cursor. At a corner the Left/Right and Top/Bottom bands overlap; the function checks Left, Right, Top, Bottom in order and returns the first match, so a corner press resolves to one edge.
- `apply_region_drag(region: LoopRegion, edge: RegionEdge, index: usize) -> LoopRegion` — moves the one bound `edge` names to grid `index`, clamped against the opposite bound (never inverts, can collapse to 1×1).
- `column_at(px, scale) -> usize`, `row_at(py, scale) -> usize` — cursor → clamped grid index.

`editor.rs` has `enum LeftGesture { ResizeRegion(grid_view::RegionEdge), … }`; `region_edge_under_cursor() -> Option<RegionEdge>` (calls `region_edge_hit`); `update_region_drag(edge)` (maps the cursor to a column index for Left/Right or a row index for Top/Bottom via `column_at`/`row_at`, then `apply_region_drag`). A region-edge press sets `LeftGesture::ResizeRegion(edge)`; `CursorMoved` drives `update_region_drag`.

The Milestone 1b-ii-b-4 spec explicitly deferred corner handles ("a corner press resolves to one edge; the user does two drags"). This feature delivers them.

## §1 The corner model

Two new types in `grid_view.rs`:

```
pub enum RegionCorner { NW, NE, SW, SE }
pub enum RegionHandle { Edge(RegionEdge), Corner(RegionCorner) }
```

Each corner is the meeting of one vertical edge and one horizontal edge. `RegionCorner::edges(self) -> (RegionEdge, RegionEdge)` returns `(vertical, horizontal)`:

- `NW → (Left, Top)`
- `NE → (Right, Top)`
- `SW → (Left, Bottom)`
- `SE → (Right, Bottom)`

`RegionEdge` is unchanged. `RegionHandle` is what the hit-test returns — a draggable region resize handle is either an edge or a corner.

## §2 Hit-testing

`region_edge_hit` is replaced by:

```
pub fn region_handle_hit(px: f32, py: f32, region: LoopRegion, scale: f32) -> Option<RegionHandle>
```

It keeps the existing off-grid guard (toolbar/gutter/margins → `None`). It then determines, independently:

- the **vertical** edge the cursor is within `EDGE_BAND*scale` of — `Left` (near the region's left x), `Right` (near its right x), or none — and only when the cursor's y is within the region's row span;
- the **horizontal** edge — `Top`, `Bottom`, or none — and only when the cursor's x is within the region's column span.

Then:

- both present → `Corner` (the `RegionCorner` for that vertical+horizontal pair),
- exactly one present → `Edge`,
- neither → `None`.

Because a corner needs *both* bands, corners take priority over edges precisely in the band-overlap square at each corner — there is no ambiguity to resolve by ordering.

## §3 Resize logic

A new pure function in `grid_view.rs`:

```
pub fn apply_region_corner_drag(region: LoopRegion, corner: RegionCorner, row: usize, col: usize) -> LoopRegion
```

It resizes both bounds by composing the existing `apply_region_drag` once per edge of the corner: apply the vertical edge with `col`, then the horizontal edge with `row` (`corner.edges()` gives the pair). Because `apply_region_drag` already clamps each moved bound against its opposite, the corner drag inherits the same guarantees — it never inverts the region and can collapse it toward 1×1.

## §4 Editor wiring

In `editor.rs`:

- `LeftGesture::ResizeRegion` carries a `RegionHandle` instead of a `RegionEdge`.
- `region_edge_under_cursor()` becomes `region_handle_under_cursor() -> Option<RegionHandle>`, calling `region_handle_hit`.
- `update_region_drag` takes the `RegionHandle`: an `Edge` drives the existing single-axis resize unchanged; a `Corner` maps the cursor to both a row (`row_at`) and a column (`column_at`) and calls `apply_region_corner_drag`.
- The `ButtonPressed`/`CursorMoved` dispatch is otherwise unchanged — a region-handle press sets `LeftGesture::ResizeRegion(handle)`; `CursorMoved` drives `update_region_drag`; `ButtonReleased` clears it.

## §5 What is unchanged

- Rendering — no corner nubs; `draw_grid` and the four edge-midpoint nubs are untouched.
- Edge resize, the move grip, drag-paint, cell editing, routing, propagation, audio.
- `apply_region_drag`, `column_at`, `row_at`, `EDGE_BAND`, `RegionEdge`.

## §6 Testing

- **Unit tests (TDD)** in `grid_view.rs`:
  - `region_handle_hit` returns the correct `Corner` at each of the four corner band-overlap squares, and the correct `Edge` along each edge away from the corners, and `None` for the region interior and off-grid points.
  - `apply_region_corner_drag` for each corner moves both the row and the column bound; dragging a corner past its opposite bounds clamps (no inversion); a corner can collapse the region toward 1×1.
- **Editor wiring** verified by `cargo build` (warning-free) + `cargo clippy`; the gesture behaviour is exercised by the pure functions and the smoke test.
- **Smoke test**: dragging each corner resizes the region on both axes at once; edge resize, the move grip and drag-paint still work.

## Out of scope

- Visible corner handle nubs (the user chose invisible grab zones).
- The UI performance pass — the remaining backlog item.
- Any change to edge resize or the move grip.
