# Multosis Grid-Editor Layout Redesign — Design

**Date:** 2026-05-18
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

The Multosis grid editor feels visually cramped: the 32×16 grid of 33 px cells runs flush to every window edge and butts directly against the toolbar, with no whitespace anywhere. This redesign gives the grid breathing room (a margin frames it, a gutter separates it from the toolbar), enlarges the cells, and adds a wider gap every 8 columns so the 32 steps read as four groups.

The change is layout-only — no routing, propagation, or audio behaviour changes. It touches the editor's geometry: `multosis/src/editor/grid_view.rs` (cell sizing and the cell↔pixel mapping), `multosis/src/editor.rs` (the window-size constants), `multosis/src/editor/toolbar.rs` (the toolbar relayout the wider window forces), and `multosis/src/lib.rs` (the persisted default size).

## Background — current layout

- `editor.rs`: `WINDOW_WIDTH = 1056`, `WINDOW_HEIGHT = 616`. The editor is freely resizable; `scale = physical_width / WINDOW_WIDTH`.
- `grid_view.rs`: `STATUS_H = 88.0` (toolbar strip, two `TOOLBAR_ROW_H = 44.0` rows), `CELL = 33.0`. `cell_rect(row, col, scale)` returns `x = col*CELL*scale`, `y = (STATUS_H + row*CELL)*scale`, `side = CELL*scale`. So the grid spans x `0..1056`, y `88..616` — wall to wall. `cell_at` and `cell_zone` invert this mapping; `column_at`/`row_at` map a cursor to a clamped grid index (used by region drag and drag-paint); `region_edge_hit` has a grid-bounds guard built from `STATUS_H`/`ROWS`/`COLS`/`CELL`; `region_grip_rect` and `draw_grid`/`draw_cell`/`draw_wavefront` build on `cell_rect`.
- `toolbar.rs`: `ToolbarControl` (6 controls) and `ToolbarOp` (6 ops) each have a `logical_x_w()` returning hardcoded `(x, width)` constants authored for a 1056-wide row; `control_rect`/`op_rect` multiply by `scale`; `toolbar_hit`/`op_hit` invert; `draw_toolbar` draws the strip and the sequence-status readout (at a hardcoded logical x).
- `lib.rs`: `editor_state: EditorState::from_size(1056, 616)`.
- 16 rows, 32 columns (`ROWS`, `COLS` in `crate::grid`).

## §1 Layout constants

New / changed logical constants in `grid_view.rs`:

| Constant | Value | Meaning |
|----------|-------|---------|
| `CELL` | `40.0` (was `33.0`) | Cell side. |
| `MARGIN` | `16.0` | Window-background frame on the grid's left, right and bottom; also the toolbar controls' side inset. |
| `GUTTER` | `14.0` | Separation between the toolbar strip and the grid. |
| `GROUP_SIZE` | `8` | Columns per group. |
| `GROUP_GAP` | `8.0` | Extra horizontal gap between groups of 8 columns. |
| `STATUS_H` | `88.0` (unchanged) | Toolbar strip height. |

These yield the window size (in `editor.rs`):

- `WINDOW_WIDTH  = MARGIN + COLS*CELL + (COLS/GROUP_SIZE − 1)*GROUP_GAP + MARGIN = 16 + 1280 + 24 + 16 = 1336`
- `WINDOW_HEIGHT = STATUS_H + GUTTER + ROWS*CELL + MARGIN = 88 + 14 + 640 + 16 = 758`

The editor remains freely resizable; `1336 × 758` is only the default.

## §2 Cell coordinate mapping

`cell_rect(row, col, scale)` becomes:

```
group   = col / GROUP_SIZE              // integer division: 0,0..0,1,1..1,2,..,3
x       = (MARGIN + col*CELL + group*GROUP_GAP) * scale
y       = (STATUS_H + GUTTER + row*CELL) * scale
side    =  CELL * scale
```

`group*GROUP_GAP` is the cumulative extra spacing from every group boundary left of `col` (columns 0–7 add 0, 8–15 add one gap, 16–23 two, 24–31 three).

The inverse functions must faithfully invert this:

- **`cell_at(px, py, scale) -> Option<(row, col)>`** — returns `None` for any point in the toolbar strip, the gutter, the margins, or a between-group gap (those points are over no cell). For a point over a cell, returns its `(row, col)`.
- **`cell_zone`** — unchanged in spirit (resolves a cell hit to the centre or one of 8 octants); it resolves the cell via `cell_at` and the octant via `cell_rect`, so it follows automatically once those are correct.
- **`column_at(px, scale) -> usize`** and **`row_at(py, scale) -> usize`** — used while dragging (region move, drag-paint), where the cursor may stray off the grid or into a gap. They clamp to `0..=COLS-1` / `0..=ROWS-1`; a cursor in a between-group gap clamps to the nearer of the two adjacent columns.
- **`region_edge_hit`** — its grid-bounds guard is rebuilt from the new constants: grid top `(STATUS_H+GUTTER)*scale`, bottom `(STATUS_H+GUTTER+ROWS*CELL)*scale`, left `MARGIN*scale`, right `(WINDOW_WIDTH−MARGIN)*scale`.

`region_grip_rect`/`region_grip_hit`, `apply_region_drag`/`apply_region_move` (pure index math), `draw_grid`/`draw_cell`/`draw_wavefront`, and the loop-region outline all build on `cell_rect` or on grid indices and need no formula changes beyond what `cell_rect` already gives them. A loop region that spans a group boundary simply has its outline rectangle include the gap — acceptable.

## §3 The every-8 group gap

The `GROUP_GAP` is realised purely by the `cell_rect` x-offset above — columns after a group boundary are shifted right by `GROUP_GAP`. Nothing is *drawn* in the gap: like the existing 1 px inter-cell gap, the gap shows the editor background. The four groups of 8 therefore read as separate blocks. No tint, no line.

## §4 Toolbar relayout

Widening the window to 1336 px strands the toolbar, whose control/op coordinates are hardcoded for 1056. The toolbar's two rows are re-laid to span the new width:

- The control row and the operation row lay out within `WINDOW_WIDTH − 2*MARGIN`, inset by `MARGIN` (16) on each side — matching the grid's side margin.
- The six controls and six operations keep their current *relative* proportions; their `logical_x_w()` coordinates are recomputed so the rows fill the new content width. `control_rect`, `op_rect`, `toolbar_hit`, `op_hit`, and the sequence-status readout's x in `draw_toolbar` all follow from the recomputed coordinates.
- The toolbar strip itself stays a full-width header at the top (height `STATUS_H`, unchanged); only the controls within it re-spread.

Expressing the toolbar coordinates relative to `WINDOW_WIDTH` (rather than re-hardcoding new magic numbers) is preferred, so a future width change does not strand the toolbar again.

## §5 Window size and persistence

- `editor.rs` `WINDOW_WIDTH`/`WINDOW_HEIGHT` become `1336`/`758`. The `scale` derivation from `WINDOW_WIDTH` is unchanged and follows the constants.
- `lib.rs` `EditorState::from_size(1056, 616)` becomes `from_size(1336, 758)`.
- A project saved before this change has `1056 × 616` persisted; on load `scale = 1056/1336 ≈ 0.79` (within the `0.5..4.0` clamp) — the editor simply opens at ~79% scale and is resizable. Acceptable; no migration needed.

## §6 What is unchanged

- Routing, propagation, the audio engine, the data model.
- Click behaviour — `cell_zone` still resolves clicks to the centre or 8 octants; drag-paint, region resize/move still work (they consume the updated geometry functions).
- The arrowhead connection indicators (recently shipped) — they scale with `CELL` automatically via `cell_rect`/`arrowhead_vertices`.
- The toolbar's control set, behaviour, and the two-row strip height.

## §7 Testing

- **Unit tests (TDD)** in `grid_view.rs`:
  - `cell_rect` round-trips: for every `(row, col)`, the centre of `cell_rect(row, col, scale)` maps back through `cell_at` to `Some((row, col))`.
  - `cell_at` returns `None` for a point in the margin, in the gutter, in the toolbar strip, and in a between-group gap.
  - the `GROUP_GAP` offset: `cell_rect(_, 8)` starts `GROUP_GAP*scale` further right than a no-gap layout would place it; columns within a group are contiguous.
  - `column_at`/`row_at` clamp at and beyond both ends and resolve a gap point to an adjacent column.
  - the window constants compute to `1336 × 758`.
- **Toolbar tests** in `toolbar.rs`: every control/op rectangle lies within `[MARGIN, WINDOW_WIDTH−MARGIN]`; rows do not overlap; `toolbar_hit`/`op_hit` round-trip the centre of each rectangle.
- **Build + smoke test**: the editor renders with the margin frame, the toolbar gutter, bigger cells and the four 8-column groups; clicking cells and octants, drag-paint, region resize/move, and every toolbar control still work; the loop region, wavefront and arrowheads render correctly.

## Out of scope

- The UI performance pass — the next and final backlog item.
- Any change to the toolbar's control set or the number of grid rows/columns.
- Per-group tinting or drawn divider lines (the extra gap was the chosen treatment).
