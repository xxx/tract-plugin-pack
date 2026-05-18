# Multosis Connection-Indicator Redesign — Design

**Date:** 2026-05-18
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

Redesign the Multosis grid editor's connection (send) indicators. Today each of a cell's up-to-8 send directions is drawn as a small muted-blue square pip. The pips do not convey *direction* — a square reads as a dot — and a dense grid of identical pips makes individual routes hard to follow. This redesign replaces each pip with a directional **arrowhead** pointing the way the trigger flows.

This is a visual-polish change confined to one function (`draw_cell` in `multosis/src/editor/grid_view.rs`). It changes nothing about routing behaviour, hit-testing, or any other cell element.

## Background — current rendering

`draw_cell` (in `multosis/src/editor/grid_view.rs`) draws, per cell: the background, then for each lit send direction a square pip, then the start marker. The pip code:

```rust
let pip = w * 0.16;
for dir in Direction::ALL {
    if !cell.sends_to(dir) { continue; }
    let (dr, dc) = dir.delta();
    let px = cx + dc as f32 * w * 0.34 - pip / 2.0;
    let py = cy + dr as f32 * h * 0.34 - pip / 2.0;
    widgets::draw_rect(pixmap, px, py, pip, pip, color_send());
}
```

`color_send()` returns `#6f8ab8`. `Direction::ALL` is the 8 compass directions; `Direction::delta()` returns `(dr, dc)` with each component in `{-1, 0, 1}` — cardinals have exactly one zero component, diagonals have none. The cell rectangle, `cx`/`cy` (cell centre), and `w`/`h` (cell side, `w == h`) are already in scope in `draw_cell`.

## §1 Visual design — the arrowhead

Each lit send direction is drawn as a small **filled triangular arrowhead** pointing outward in that direction (replacing the square pip).

- **Cardinal sends (N, E, S, W):** the arrowhead sits at the midpoint of the corresponding cell edge, tip pointing straight out through that edge.
- **Diagonal sends (NE, SE, SW, NW):** the arrowhead sits at the corresponding cell corner, tip pointing diagonally out through that corner.
- **Inset — no overhang:** every arrowhead is anchored so the whole triangle stays *within* the cell's drawn rectangle. The tip is held a small inset (`EDGE_INSET`, ~1.5 logical px) short of the cell boundary. Nothing pokes into the 1 px inter-cell gap, so a cell's arrowheads never touch a neighbour.
- **Colour:** `#86a6e8` — a brighter, higher-contrast blue than the current `#6f8ab8`, so indicators stand out against the `#333742` enabled-cell background. `color_send()` is updated to return this value.
- **Size:** proportional to the cell. Starting values: head length `≈ 0.22·w` along the direction, head half-width `≈ 0.13·w` perpendicular. These are tuned in the smoke test (Task: §4) against the real 33 px cell — the goal is an arrowhead comparable in footprint to the old pip, clearly readable but not crowding the cell.

The mockup the user approved: a single cell with one to eight arrowheads, and a dense 6×4 patch confirming routes stay traceable when many cells have sends.

## §2 Arrowhead geometry

The three triangle vertices for one send direction are a pure function of the cell rectangle, the direction, and `scale`. Model:

- Let `unit = (ux, uy)` be the send direction as a unit vector: a cardinal is axis-aligned (`(±1,0)` or `(0,±1)`); a diagonal is `(±0.70710677, ±0.70710677)`. Derive it by normalising `Direction::delta()`'s `(dc, dr)` (note: `dc` is the x component, `dr` the y component).
- Let `perp = (-uy, ux)` be the unit perpendicular.
- The cell boundary distance from the centre along `unit` is `0.5·w` for a cardinal and `0.5·w·√2` for a diagonal (the corner). The arrowhead **tip** is `tip = centre + unit · (boundary − EDGE_INSET·scale)`.
- The base centre is `base = tip − unit · (head_len)`; the two base vertices are `base ± perp · (head_halfwidth)`.

So the three vertices are `tip`, `base + perp·hw`, `base − perp·hw`. `head_len` and `head_halfwidth` are `0.22·w` and `0.13·w` respectively (subject to smoke-test tuning).

This geometry is implemented as a unit-tested pure function (see §5).

## §3 Implementation

All changes are in `multosis/src/editor/grid_view.rs`:

1. **`color_send()`** — change the returned colour to `#86a6e8`.
2. **A pure geometry function** — e.g. `arrowhead_vertices(cell_x, cell_y, w, dir, scale) -> [(f32, f32); 3]` (exact name/signature decided in the plan) computing the three triangle vertices per §2. Pure and unit-tested.
3. **A triangle-fill helper** — a small local helper in `grid_view.rs` that fills a triangle given three points and a colour, using `tiny_skia::PathBuilder` (`move_to` → `line_to` → `line_to` → `close`) and an anti-aliased `Paint` fill. Local to Multosis (no other plugin needs it; promote to `tiny-skia-widgets` later only if a second consumer appears — YAGNI).
4. **`draw_cell`** — replace the square-pip loop: for each lit send direction, compute the vertices and fill the arrowhead. The loop structure (`for dir in Direction::ALL { if !cell.sends_to(dir) { continue; } … }`) is unchanged.

**Performance note:** a filled triangle goes through tiny-skia's anti-aliased rasteriser, which is heavier than the opaque `draw_rect` fast-path the square pip used. Worst case is 8 arrowheads × 512 cells per frame; the common case (default routing) is one per cell. This is acceptable for now and is explicitly in scope for the later *UI performance pass* backlog item — do not pre-optimise here.

## §4 What is unchanged

- **Hit-testing** — `cell_zone` maps clicks to the 3×3 octant grid independently of how sends are drawn; clicking an octant to toggle a send is unaffected.
- **Routing, propagation, the data model** — `Cell::sends`, `sends_to`, `Direction` — untouched.
- **Other cell elements** — the cell background (enabled/disabled colours), the start marker (green inset outline), the wavefront highlight, the loop-region outline and its handles/grip — all unchanged.

## §5 Testing

- **Unit tests (TDD)** for the geometry function: for a representative cell rectangle and `scale`, assert for every `Direction` that (a) all three vertices lie within the cell rectangle (no overhang), (b) the tip is the vertex furthest along the send direction (it points outward), (c) a cardinal's tip is on the relevant edge midline and a diagonal's tip is near the relevant corner.
- **Rendering** — the triangle-fill helper and the `draw_cell` change are verified by `cargo build` (warning-free) and `cargo clippy`; the visual result is verified by the smoke test.
- **Smoke test** — run the standalone, confirm: send directions read as outward-pointing arrowheads; cardinals sit at edge midpoints, diagonals at corners; nothing overhangs into neighbouring cells; a busy grid stays traceable. Tune `head_len` / `head_halfwidth` / `EDGE_INSET` here if the proportions need adjusting.

## Out of scope

- The cramped-grid spacing and the UI performance pass — separate backlog items.
- Any change to routing behaviour, the start marker, the wavefront, or the loop region.
- Animating or colour-coding indicators by route — not requested.
