# Multosis Grid-Editor Interaction Enhancements — Design

**Date:** 2026-05-18
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

Two near-term editor-interaction features for the Multosis plugin, identified after the Phase 1 smoke test:

1. **Drag-paint cell toggling** — dragging across the grid sets the `enabled` state of every cell it crosses, instead of editing one cell per click.
2. **Move the whole loop region** — a hover-revealed grip lets the user translate the loop region as a unit, keeping its size.

Both are additions to the existing CPU-rendered editor (`multosis/src/editor.rs` + `multosis/src/editor/grid_view.rs`). Nothing here touches the audio engine; all changes are GUI-thread interaction and rendering. The deferred visual-polish items (more distinct connection indicators, the cramped grid) are explicitly out of scope.

## Background — current editor behaviour

- `grid_view.rs` owns grid geometry: `cell_rect`, `cell_at`, `cell_zone`/`CellZone` (a cell splits 3×3 — center + 8 send octants), `apply_grid_click`, `draw_grid`, and the loop-region edge handles from Milestone 1b-ii-b-4 (`RegionEdge`, `region_edge_hit`, `column_at`, `row_at`, `apply_region_drag`).
- A left-press on a grid cell currently calls `handle_grid_click(false)` **immediately on `ButtonPressed`**: `cell_zone` resolves the press to a center or octant; center toggles `enabled`, an octant toggles a `send` bit. A right-press toggles `start`.
- `MultosisWindow` tracks left-button gestures: `toolbar_drag` (sliders) and `region_drag: Option<RegionEdge>` (edge resize). `on_event` dispatches `ButtonPressed{Left}` through toolbar → op → region-edge → grid.
- baseview mouse events carry modifier state (the workspace's MSEG editor already reads Alt during a mouse drag), so the editor can read Shift directly from the mouse event.

## §1 Drag-paint cell toggling

**Deferred click.** A left-press on a grid cell no longer edits the grid on `ButtonPressed`. It records a *pending gesture* (the press cell and its `CellZone`). The outcome is decided later:

- **Click** — the button releases with the cursor still in the press cell. On `ButtonReleased`, apply the existing `apply_grid_click` behaviour for that cell + zone (center → toggle `enabled`, octant → toggle the `send` bit). Effect is identical to today; only the timing moves from press to release, which is imperceptible.
- **Paint drag** — the cursor moves into a *different* cell while the button is held. The gesture becomes a paint drag.

**Paint behaviour.** A paint drag sets a single `enabled` value on every cell it touches — *not* a per-cell toggle, so re-crossing a cell never flips it back. The value is fixed for the whole stroke, sampled from the Shift modifier at the moment the drag begins:

- no Shift → paint `enabled = true`
- Shift held → paint `enabled = false`

The stroke includes the cell the press started in, so the entire stroke is consistent. Painting affects any cell regardless of loop-region membership; `send` and `start` are never changed by a paint drag.

**No skipped cells.** On a fast drag a single `CursorMoved` can jump several cells. Each move paints every cell on the line segment between the previous and current cursor positions, so a stroke is continuous. The segment→cells enumeration is a pure function (see §5).

**Scope.** Left button only. Right-click keeps its current press-to-toggle-`start` behaviour with no right-drag painting.

## §2 Move the whole loop region

**Move grip.** A small square **move grip** is drawn at the loop region's geometric center, in the loop-region highlight color, sized like the existing edge nubs. It is drawn **only while the cursor is inside the loop region** — when the user is not interacting with the region, the grid shows no extra mark, so the routing arrows are uncluttered.

**Move behaviour.** Pressing on the grip and dragging translates the entire loop region — all four bounds shift together by the cursor's grid-cell delta from the press point. The region's size (`row1−row0`, `col1−col0`) is unchanged. The translation is clamped so the region always stays fully within the 16×32 grid: dragging further than the edge simply parks the region against that edge.

**Tiny regions.** The grip is a fixed small logical size. For a region too small to contain it without overlapping the edge-resize bands, the grip is clamped into the region interior and may visually overlap the edges — an accepted minor limitation; the user can resize the region larger, move it, and resize back. (1×1 and other tiny regions are still *resizable* via edges as before; only grip-moving is constrained.)

## §3 Interaction priority

`on_event` resolves a `ButtonPressed{Left}` in this fixed order; the first match wins and consumes the press:

1. Toolbar control (`toolbar_hit`) — sliders / buttons, existing.
2. Toolbar op (`op_hit`) — the six grid operations, existing.
3. Loop-region **edge** (`region_edge_hit`) — start an edge resize, existing.
4. Loop-region **move grip** (`region_grip_hit`) — start a region move. Only matches when the cursor is inside the region (the same condition that draws the grip).
5. Grid cell — record the pending drag-paint/click gesture (§1).

Edges win at the region perimeter, the grip wins at the center; for any region ≥3 cells in each dimension they do not overlap. Only one left-button gesture is ever active at a time.

## §4 Rendering

`draw_grid` becomes cursor-aware: it receives the current cursor position (or a precomputed "draw the grip" flag) so it can render the move grip when the cursor is inside the loop region. The grip is a filled square (`widgets::draw_rect`, loop-region color) drawn after the region outline and edge nubs. No other rendering changes; the grid redraws every frame as today, so the grip appears/disappears as the cursor enters/leaves the region.

## §5 Components and file structure

Pure geometry/logic lives in `grid_view.rs` (consistent with `apply_region_drag` from Milestone 1b-ii-b-4), unit-tested:

- `apply_region_move(region: LoopRegion, drow: i32, dcol: i32) -> LoopRegion` — translate all four bounds by `(drow, dcol)`, clamped so the region stays fully on the grid with its size preserved.
- `region_grip_hit(px: f32, py: f32, region: LoopRegion, scale: f32) -> bool` — true when the point is on the move grip. Its grip rect also drives the §4 rendering, so geometry is defined once.
- A segment→cells enumerator — given two cursor points (or two cell coordinates), yield every grid cell on the line between them, so a paint stroke skips nothing.

Editor wiring lives in `editor.rs`:

- `MultosisWindow` gains gesture state for the pending click / active paint drag (press cell + zone; once a drag, the painted value + last painted cell) and for an active region move (the press anchor + the region at press time). These are mutually exclusive with the existing `region_drag` (edge resize) — a single "current left-button gesture" representation is preferred over several parallel `Option` fields.
- `on_event` implements the §3 priority, the deferred click/paint transition on `CursorMoved`, and ends every gesture on `ButtonReleased{Left}`.
- A paint drag and a region move each mutate `params.grid` on the GUI thread and republish via `grid_handoff.publish(*grid)` — the existing `handle_grid_click` path.

## §6 Testing

- **Unit tests (TDD)** for the pure functions: `apply_region_move` (each direction; clamped at every grid edge; size preserved; a 1×1 region moves correctly; a full-grid region cannot move); `region_grip_hit` (hit at center, miss outside, behaviour for a tiny region); the segment enumerator (single cell, straight line, diagonal, a long jump skips nothing, endpoints included).
- **Editor wiring** — verified by `cargo build` (warning-free) and `cargo nextest`; the deferred-click and gesture-priority logic is exercised by the pure functions plus the manual smoke test.
- **Smoke test** — drag-paints a stroke (with and without Shift), confirms no cells skipped on a fast drag, the click-on-release still toggles a single cell, the hover grip appears only inside the region and moves the region clamped to the grid, and edge-resize still works.

## Out of scope

- More visually distinct connection indicators — deferred polish.
- The visually cramped grid — deferred polish.
- Right-drag painting; painting `send` or `start` by drag.
- Conway-style routing and alternative timing methods (future phases).
