# Grid Selector Widget — Design

**Status:** draft — sections approved during brainstorm, awaiting user review of the written spec
**Date:** 2026-05-03
**Scope:** A new compact alternative to the existing `draw_stepped_selector` button group. First use site is six-pack's Quality (oversampling) selector.

---

## 1. Problem

The existing `draw_stepped_selector` lays out every option as an equal-width segment, so its width grows with option count. For parameters with many options, segments become too narrow to read. We want a compact alternative that:

- Stays the same height as a button.
- Doesn't widen unbounded as option count grows.
- Cycles through values via single-click of a current-value readout.
- Direct-selects via a small grid of cells, one per value.

## 2. Behavior

The widget has two regions arranged left-to-right:

```
[ VALUE TEXT ] [ GRID ]
```

**Value text region** — shows the current value's display name (e.g. `"4x"`).
- Left-click → cycle to next value (wraps).
- Right-click → cycle to previous value (wraps).

**Grid region** — small swatches, one cell per value. The cell at the current value index is filled with `color_accent()`; all other cells are filled with `color_control_bg()`. All cells have a 1px `color_border()` outline.
- Left-click on a cell → direct-select that cell's value.
- Right-click on a cell → no-op (consistent with stepped selector / button convention).
- Hover → tooltip with the value's display name appears below the cell (flips above when below overflows).
- Cells contain no text or icons — the tooltip is the only way to identify a specific cell's value before selecting it.

**Empty cells** — when the value count doesn't fill the last row (e.g. N=5 in a 3×2 grid), the trailing cells are simply not drawn. No phantom slots, no hover regions, no click targets.

## 3. Layout

Inputs: bounding box `(x, y, w, h)` and `value_count = N`.

**Cell sizing.** Cells are square. `cell_size = h / 3.0`. This is the constraint that gives the widget its "no taller than a button" property: with up to 3 cells stacked vertically, the grid never exceeds the bounding height.

**Rows / columns.**

```
cols = max(1, ceil(N / 3))
rows = max(1, ceil(N / cols))      // never exceeds 3 by construction
```

| N  | cols | rows | layout | empty cells |
|----|------|------|--------|-------------|
| 1  | 1    | 1    | 1×1    | 0           |
| 2  | 1    | 2    | 2×1    | 0           |
| 3  | 1    | 3    | 3×1    | 0           |
| 4  | 2    | 2    | 2×2    | 0           |
| 5  | 2    | 3    | 3×2    | 1 (bottom-right) |
| 6  | 2    | 3    | 3×2    | 0           |
| 7  | 3    | 3    | 3×3    | 2 (right side of bottom row) |
| 9  | 3    | 3    | 3×3    | 0           |
| 10 | 4    | 3    | 3×4    | 2           |

**Fill order (typewriter).** Value index `i` maps to `(row, col) = (i / cols, i % cols)`. Reading left-to-right, top-to-bottom matches how a typewriter advances.

**Cell gap.** `cell_gap = cell_size * 0.15`. Tight enough to read the cells as a unit, separated enough to read individual cells.

**Inner gap.** `inner_gap = cell_size * 0.5` — the horizontal gap between the value-text region and the grid. Visually distinct from the cell-to-cell gap so the two regions read as separate.

**Grid placement within `(x, y, w, h)`.**
- Grid total width  = `cols * cell_size + (cols - 1) * cell_gap`.
- Grid total height = `rows * cell_size + (rows - 1) * cell_gap`.
- Grid is **right-aligned** within the bounding box (with `cell_gap` of right padding).
- Grid is **vertically centered** within `h`.

**Value text region.** `(x, y, value_w, h)` where `value_w = w - grid_w - inner_gap - cell_gap` (the trailing `cell_gap` accounts for the right-edge padding of the grid). Text is left-aligned with 6px pad, vertically centered, font size `(h * 0.5).max(10.0)` — same as `draw_slider` / `draw_button`.

## 4. Visuals

**Value text region.**
- Background: `color_control_bg()`.
- 1px outline: `color_border()`.
- Text: `color_text()`, left-aligned with 6px pad.

**Grid cells.**
- Active cell fill: `color_accent()`.
- Inactive cell fill: `color_control_bg()`.
- All cells: 1px `color_border()` outline.
- No outer outline around the grid as a whole.

**Tooltip.** Visual style mirrors six-pack's `draw_cursor_tooltip` (`six-pack/src/editor/curve_view.rs:393`):
- Background: `#10141c f2`.
- Border: 1px `#c0e0ff ff`.
- Padding: `6.0 * s`.
- Font: `(10.0 * s).max(9.0)`, single line.
- Anchor: directly below the hovered cell with `2.0 * s` gap, horizontally centered on the cell. Flips above when below would overflow `parent_clip`. Horizontal clamp keeps it inside `parent_clip`.

## 5. Public API

New module `tiny-skia-widgets/src/grid_selector.rs`, re-exported from `tiny_skia_widgets::lib`.

```rust
pub struct GridSelectorLayout {
    pub value_rect: (f32, f32, f32, f32),       // (x, y, w, h)
    pub grid_rect:  (f32, f32, f32, f32),
    pub cell_rects: Vec<(f32, f32, f32, f32)>,  // one per value, in value-index order
    pub rows: usize,
    pub cols: usize,
}

/// Compute the layout without drawing.
pub fn grid_selector_layout(
    x: f32, y: f32, w: f32, h: f32,
    value_count: usize,
) -> GridSelectorLayout;

/// Pure drawing function. Draws value-text region and grid cells.
pub fn draw_grid_selector(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    layout: &GridSelectorLayout,
    value_text: &str,
    active_index: usize,
);

/// Draws the tooltip box anchored to a specific cell rect. Caller decides
/// when to call (typically inside `draw_grid_tooltips_pass`).
pub fn draw_grid_tooltip(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cell_rect: (f32, f32, f32, f32),
    label: &str,
    s: f32,
    parent_clip: (f32, f32, f32, f32),
);

/// One-stop drawing + hit-region wiring for a grid selector.
/// Generic over the caller's action enum.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid_selector_with_hit<A: Clone + PartialEq>(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    drag: &mut DragState<A>,
    x: f32, y: f32, w: f32, h: f32,
    value_text: &str,
    value_count: usize,
    active_index: usize,
    cycle_action: A,                      // pushed on the value-text rect; the plugin's
                                          // left-click handler dispatches "next", its
                                          // right-click handler dispatches "prev"
    cell_action: impl Fn(usize) -> A,     // pushed on each cell rect (left-click)
    hover_action: impl Fn(usize) -> A,    // pushed on each cell rect (hover marker)
);

/// End-of-frame pass: walks `drag.regions()`, finds any hovered grid-hover
/// region (gated on `mouse_in_window`, suppressed during an active drag),
/// and paints the tooltip on top of everything else.
///
/// `name_for` returns `Some(name)` for grid-hover action variants, `None`
/// otherwise.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid_tooltips_pass<A: Clone + PartialEq>(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    drag: &DragState<A>,
    s: f32,
    parent_clip: (f32, f32, f32, f32),
    name_for: impl Fn(&A) -> Option<&'static str>,
);
```

The split between `grid_selector_layout`/`draw_grid_selector` (pure) and `draw_grid_selector_with_hit` (DragState-aware, generic over `A`) follows the same pattern as `draw_stepped_selector` plus six-pack's local `draw_stepped_with_hit` — but the `_with_hit` variant lives in the widget crate this time so other plugins reuse it.

## 6. Hit Regions and Per-Button Dispatch

For each grid widget, three hit regions are pushed (one cycle region + one cell + one hover per value):

| Region | Rect | Action passed in | Click semantics |
|--------|------|-------------------|-----------------|
| Value text | `value_rect` | `cycle_action` | left-click → cycle next; right-click → cycle prev |
| Cell `i` | `cell_rects[i]` | `cell_action(i)` | left-click → direct-select i; right-click → no-op |
| Cell `i` (hover) | `cell_rects[i]` | `hover_action(i)` | hover marker for tooltip; not click-dispatched |

`DragState::push_region` in `tiny-skia-widgets/src/drag.rs` is button-agnostic — the same region is consulted on both left and right clicks. Per-button dispatch already happens in each plugin's two separate handler paths: one for left mouse-down, one for `handle_right_press` (see `six-pack/src/editor.rs:494-505,701`). The grid selector pushes a single `cycle_action` region; the plugin's left handler dispatches it as "cycle next" and the right handler dispatches it as "cycle prev." `cell_action(i)` follows the same pattern (left = direct-select, right = no-op via the existing `_ => {}` fallback in `handle_right_press`).

`hover_action(i)` is non-clickable: it's tagged purely so `draw_grid_tooltips_pass` can find the hovered cell at end-of-frame. The plugin's click handlers add no arms for `*HoverCell` variants.

## 7. Six-Pack Integration

The Quality (oversampling) stepped selector at `six-pack/src/editor/bottom_strip.rs:158-171` is replaced.

**New `HitAction` variants** (added to whichever module owns the editor's action enum):

```rust
QualityCycle,
QualityCell(usize),
QualityHoverCell(usize),
```

**Replacement at the call site:**

```rust
const QUALITY_NAMES: &[&str] = &["Off", "4x", "8x", "16x"];
let quality_idx = win.params.quality.value() as usize;
widgets::draw_grid_selector_with_hit(
    &mut win.surface.pixmap, tr, &mut win.drag,
    right_x, qy, right_w, stepper_h,
    QUALITY_NAMES[quality_idx], QUALITY_NAMES.len(), quality_idx,
    HitAction::QualityCycle,
    HitAction::QualityCell,
    HitAction::QualityHoverCell,
);
```

**Click dispatch.** Pseudocode below uses `params` and `setter` as stand-ins; the real handlers receive `self` (in `handle_right_press`) or operate on whichever click-dispatch site calls into the existing `ParamSetter::begin_set_parameter / set_parameter / end_set_parameter` pattern (see `six-pack/src/editor.rs:543-546`).

Left-click handler:

```rust
HitAction::QualityCycle => {
    let n = QUALITY_NAMES.len();
    let i = (params.quality.value() as usize + 1) % n;
    setter.begin_set_parameter(&params.quality);
    setter.set_parameter(&params.quality, i as i32);
    setter.end_set_parameter(&params.quality);
}
HitAction::QualityCell(i) => {
    setter.begin_set_parameter(&params.quality);
    setter.set_parameter(&params.quality, i as i32);
    setter.end_set_parameter(&params.quality);
}
HitAction::QualityHoverCell(_) => { /* no-op on click */ }
```

Right-click handler (added arm in `handle_right_press`):

```rust
HitAction::QualityCycle => {
    let n = QUALITY_NAMES.len();
    let i = (params.quality.value() as usize + n - 1) % n;
    setter.begin_set_parameter(&params.quality);
    setter.set_parameter(&params.quality, i as i32);
    setter.end_set_parameter(&params.quality);
}
// QualityCell and QualityHoverCell fall through the existing `_ => {}` arm.
```

**Tooltip pass** (once at end of `draw_editor`, after all UI is painted):

```rust
widgets::draw_grid_tooltips_pass(
    &mut win.surface.pixmap, tr, &win.drag, s,
    (0.0, 0.0, win.physical_width as f32, win.physical_height as f32),
    |action| match action {
        HitAction::QualityHoverCell(i) => Some(QUALITY_NAMES[i]),
        // future grid widgets add arms here
        _ => None,
    },
);
```

**Per-plugin overhead** for adopting the widget elsewhere: define three action variants, replace one call site, add one arm to the left-click handler, one arm to `handle_right_press`, and one arm to the tooltip-pass `name_for` closure. No copy-pasted helpers.

## 8. Tests

**`tiny-skia-widgets/src/grid_selector.rs` (`#[cfg(test)]`):**

1. `layout_n1_through_n10_matches_table` — rows/cols/cell-count from §3 table.
2. `cell_rects_in_typewriter_order` — index 0 → top-left, index `cols` → leftmost of row 1, etc.
3. `grid_right_aligned_within_bounds` — right-edge invariant.
4. `value_rect_left_of_grid` — `value_rect.x == x`, `value_rect.x + value_rect.w + inner_gap == grid_rect.x`.
5. `cells_vertically_centered` — top/bottom slack is equal within rounding.
6. `draw_grid_selector_lights_active_cell` — pixel sample at the active cell's center matches `color_accent()`; inactive cell sample matches `color_control_bg()`.
7. `draw_grid_selector_n0_no_panic` — `value_count = 0` is a no-op (no panic, no draws into the grid region).
8. `draw_grid_tooltip_paints_into_pixmap` — pixels around the tooltip anchor change vs. baseline.
9. `draw_grid_tooltip_flips_above_when_below_overflows` — anchoring near the bottom of `parent_clip` produces `tooltip_y < cell_y`.

**Test-font helper.** `controls.rs` already includes a ~200-line synthetic-TrueType helper for tests. It's factored into a shared `#[cfg(test)] mod test_font` module so `grid_selector.rs` reuses it without duplication.

**`six-pack/src/editor/bottom_strip.rs` (`#[cfg(test)]`):**

10. `quality_grid_layout_for_n4_uses_2x2` — calling `grid_selector_layout` with the args six-pack passes returns `rows=2, cols=2`.
11. `quality_grid_active_cell_for_each_value` — for each oversampling enum value, the corresponding cell rect contains the active-fill pixel after `draw_grid_selector` runs.

## 9. Out of Scope

- **Hover state on the value-text region.** The existing `draw_button` ignores its `_hovered` argument; the grid selector matches that. If we later add real hover tinting, it goes in across all three widgets at once.
- **Drag-across-cells selection.** Each cell click is independent; no drag-select.
- **Vertical layout `[GRID] [VALUE TEXT]` or value-on-right.** The chosen layout is `[VALUE TEXT] [GRID]` only.
- **Dynamic value labels.** `name_for` returns `&'static str`. Plugins with runtime-generated labels can revisit the API later.
- **Use sites outside six-pack.** The widget is generic and lives in `tiny-skia-widgets`, but only the Quality selector migrates as part of this work. Other plugins can adopt it incrementally.

## 10. Migration Plan

1. Add `tiny-skia-widgets/src/grid_selector.rs` with the layout helper, drawing function, tooltip drawing function, `_with_hit` adapter, and tooltip pass.
2. Factor the test-font helper from `controls.rs` into `#[cfg(test)] mod test_font` and reuse from `grid_selector.rs` tests.
3. Replace the Quality stepped selector in `six-pack/src/editor/bottom_strip.rs` with `draw_grid_selector_with_hit`.
4. Add the three new `HitAction` variants (`QualityCycle`, `QualityCell(usize)`, `QualityHoverCell(usize)`) and arms in the left-click handler and `handle_right_press`.
5. Add the end-of-frame tooltip-pass call in `draw_editor`.
6. Add tests as listed in §8.
7. Manual verification: build standalone six-pack, exercise each oversampling value via grid-cell left-click, value-text left-click cycle, value-text right-click cycle. Confirm tooltip appears on hover and disappears when cursor leaves the window.
