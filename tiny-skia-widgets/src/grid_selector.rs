//! Grid selector widget — a compact alternative to `draw_stepped_selector`.
//!
//! See `docs/superpowers/specs/2026-05-03-grid-selector-widget-design.md`.

use tiny_skia::Pixmap;

use crate::drag::DragState;
use crate::primitives::{
    color_accent, color_border, color_control_bg, color_text, draw_rect, draw_rect_outline,
};
use crate::text::TextRenderer;

/// Layout result: rects for the value-text region and each grid cell.
pub struct GridSelectorLayout {
    pub value_rect: (f32, f32, f32, f32),
    pub grid_rect: (f32, f32, f32, f32),
    pub cell_rects: Vec<(f32, f32, f32, f32)>,
    pub rows: usize,
    pub cols: usize,
}

/// Compute the layout for a grid selector without drawing.
///
/// Inputs: bounding box `(x, y, w, h)` and the number of values to display.
/// Cell size is `h / 3.0` (square cells). `cols = max(1, ceil(N / 3))`,
/// `rows = max(1, ceil(N / cols))` (capped at 3 by construction). Empty
/// trailing cells are not represented in `cell_rects`.
///
/// `value_count = 0` returns a layout with no cell rects; the value-text
/// region still spans the full width minus the inner gap (the grid_w is
/// zero in that case).
pub fn grid_selector_layout(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    value_count: usize,
) -> GridSelectorLayout {
    let n = value_count;
    let cell_size = h / 3.0;
    let cell_gap = cell_size * 0.15;
    let inner_gap = cell_size * 0.5;

    let cols = if n == 0 { 0 } else { n.div_ceil(3).max(1) };
    let rows = if n == 0 { 0 } else { n.div_ceil(cols).max(1) };

    let grid_w = if cols == 0 {
        0.0
    } else {
        cols as f32 * cell_size + (cols.saturating_sub(1)) as f32 * cell_gap
    };
    let grid_h = if rows == 0 {
        0.0
    } else {
        rows as f32 * cell_size + (rows.saturating_sub(1)) as f32 * cell_gap
    };

    // Grid is right-aligned with `cell_gap` of right padding, vertically centered.
    let grid_right_pad = cell_gap;
    let grid_x = if cols == 0 {
        x + w
    } else {
        x + w - grid_w - grid_right_pad
    };
    let grid_y = y + (h - grid_h) * 0.5;

    // Value text region runs from x to the start of the grid minus inner_gap.
    let value_w = if cols == 0 {
        // No grid → value text spans the full width minus the right padding only.
        (w - grid_right_pad).max(0.0)
    } else {
        (grid_x - inner_gap - x).max(0.0)
    };

    let mut cell_rects = Vec::with_capacity(n);
    for i in 0..n {
        let row = i / cols;
        let col = i % cols;
        let cx = grid_x + col as f32 * (cell_size + cell_gap);
        let cy = grid_y + row as f32 * (cell_size + cell_gap);
        cell_rects.push((cx, cy, cell_size, cell_size));
    }

    GridSelectorLayout {
        value_rect: (x, y, value_w, h),
        grid_rect: (grid_x, grid_y, grid_w, grid_h),
        cell_rects,
        rows,
        cols,
    }
}

/// Draw the grid selector: value-text region (left) + cell grid (right).
///
/// Out-of-range `active_index` is silently treated as "no cell highlighted"
/// — every cell is drawn with the inactive fill.
pub fn draw_grid_selector(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    layout: &GridSelectorLayout,
    value_text: &str,
    active_index: usize,
) {
    // Value-text region.
    let (vx, vy, vw, vh) = layout.value_rect;
    if vw > 0.0 && vh > 0.0 {
        draw_rect(pixmap, vx, vy, vw, vh, color_control_bg());
        draw_rect_outline(pixmap, vx, vy, vw, vh, color_border(), 1.0);

        let text_size = (vh * 0.5).max(10.0);
        let text_y = vy + (vh + text_size) * 0.5 - 2.0;
        let pad = 6.0;
        text_renderer.draw_text(
            pixmap,
            vx + pad,
            text_y,
            value_text,
            text_size,
            color_text(),
        );
    }

    // Grid cells.
    for (i, &(cx, cy, cw, ch)) in layout.cell_rects.iter().enumerate() {
        let fill = if i == active_index {
            color_accent()
        } else {
            color_control_bg()
        };
        draw_rect(pixmap, cx, cy, cw, ch, fill);
        draw_rect_outline(pixmap, cx, cy, cw, ch, color_border(), 1.0);
    }
}

/// Draw a tooltip box anchored to `cell_rect`.
///
/// Visual style: dark bg, light cyan-blue border, single-line text. The
/// tooltip sits **above** the cell with a `6.0 * s` gap by default,
/// horizontally centered on the cell. Above is preferred because the
/// arrow cursor extends down-right from its hot spot — placing the
/// tooltip below would put it under the cursor and obscure the label.
/// If placing it above would overflow `parent_clip`, the tooltip flips
/// below the cell instead. Horizontal position is clamped to stay inside
/// `parent_clip`.
pub fn draw_grid_tooltip(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cell_rect: (f32, f32, f32, f32),
    label: &str,
    s: f32,
    parent_clip: (f32, f32, f32, f32),
) {
    let (cx, cy, cw, ch) = cell_rect;
    let (px, py, pw, ph) = parent_clip;

    let pad = 6.0 * s;
    let font_size = (10.0 * s).max(9.0);
    let gap = 6.0 * s;

    let text_w = text_renderer.text_width(label, font_size);
    let tip_w = text_w + pad * 2.0;
    let tip_h = font_size + pad * 2.0;

    // Horizontally centered on the cell, clamped inside parent_clip.
    let mut tip_x = cx + (cw - tip_w) * 0.5;
    if tip_x < px {
        tip_x = px;
    }
    if tip_x + tip_w > px + pw {
        tip_x = (px + pw - tip_w).max(px);
    }

    // Above the cell by default (out of the cursor's down-right swing); flip
    // below when above overflows.
    let above_y = cy - gap - tip_h;
    let below_y = cy + ch + gap;
    let tip_y = if above_y >= py {
        above_y
    } else if below_y + tip_h <= py + ph {
        below_y
    } else {
        // Neither fits cleanly — clamp into the parent clip.
        py.max(0.0)
    };

    let bg = tiny_skia::Color::from_rgba8(0x10, 0x14, 0x1c, 0xf2);
    let border = tiny_skia::Color::from_rgba8(0xc0, 0xe0, 0xff, 0xff);
    draw_rect(pixmap, tip_x, tip_y, tip_w, tip_h, bg);
    draw_rect_outline(pixmap, tip_x, tip_y, tip_w, tip_h, border, 1.0);

    let text_y = tip_y + pad + font_size;
    text_renderer.draw_text(pixmap, tip_x + pad, text_y, label, font_size, color_text());
}

/// One-stop drawing + hit-region wiring for a grid selector.
///
/// Pushes one hit region for the value-text rect (tagged with `cycle_action`),
/// one for each cell rect (tagged with `cell_action(i)`), and one hover-marker
/// region per cell rect (tagged with `hover_action(i)`). The plugin's mouse
/// handlers dispatch the cycle action as "next" on left-click and "prev" on
/// right-click; the cell action is dispatched as "direct-select i" on
/// left-click. Hover regions are non-clickable — they're tagged for the
/// end-of-frame `draw_grid_tooltips_pass`.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid_selector_with_hit<A: Clone + PartialEq>(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    drag: &mut DragState<A>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    value_text: &str,
    value_count: usize,
    active_index: usize,
    cycle_action: A,
    cell_action: impl Fn(usize) -> A,
    hover_action: impl Fn(usize) -> A,
) {
    let layout = grid_selector_layout(x, y, w, h, value_count);
    draw_grid_selector(pixmap, text_renderer, &layout, value_text, active_index);

    let (vx, vy, vw, vh) = layout.value_rect;
    if vw > 0.0 && vh > 0.0 {
        drag.push_region(vx, vy, vw, vh, cycle_action);
    }

    for (i, &(cx, cy, cw, ch)) in layout.cell_rects.iter().enumerate() {
        drag.push_region(cx, cy, cw, ch, cell_action(i));
        drag.push_region(cx, cy, cw, ch, hover_action(i));
    }
}

/// End-of-frame pass: walks `drag.regions()`, finds any hovered grid-hover
/// region (gated on `mouse_in_window`, suppressed during an active drag),
/// and paints a tooltip on top of everything else.
///
/// `name_for` returns `Some(name)` for grid-hover action variants and `None`
/// otherwise. The first matching region under the cursor wins.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid_tooltips_pass<A: Clone + PartialEq>(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    drag: &DragState<A>,
    s: f32,
    parent_clip: (f32, f32, f32, f32),
    name_for: impl Fn(&A) -> Option<&'static str>,
) {
    if !drag.mouse_in_window() || drag.active_action().is_some() {
        return;
    }
    let (mx, my) = drag.mouse_pos();
    for region in drag.regions() {
        let Some(label) = name_for(&region.action) else {
            continue;
        };
        if mx >= region.x && mx < region.x + region.w && my >= region.y && my < region.y + region.h
        {
            draw_grid_tooltip(
                pixmap,
                text_renderer,
                (region.x, region.y, region.w, region.h),
                label,
                s,
                parent_clip,
            );
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::color_to_rgba_u8;
    use crate::test_font::test_font_data;
    use tiny_skia::Pixmap;

    /// Test action enum used by the integration helper tests.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TestAction {
        Cycle,
        Cell(usize),
        Hover(usize),
    }

    /// Sample the pixel at the center of `cell_rect`.
    fn pixel_at_cell_center(pm: &Pixmap, cell_rect: (f32, f32, f32, f32)) -> (u8, u8, u8, u8) {
        let (cx, cy, cw, ch) = cell_rect;
        let px = (cx + cw * 0.5) as u32;
        let py = (cy + ch * 0.5) as u32;
        let p = pm.pixels()[(py * pm.width() + px) as usize];
        (p.red(), p.green(), p.blue(), p.alpha())
    }

    #[test]
    fn with_hit_pushes_one_cycle_region_plus_two_per_cell() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 60).unwrap();
        let mut drag: DragState<TestAction> = DragState::new();

        draw_grid_selector_with_hit(
            &mut pm,
            &mut tr,
            &mut drag,
            10.0,
            10.0,
            200.0,
            30.0,
            "8x",
            4,
            2,
            TestAction::Cycle,
            TestAction::Cell,
            TestAction::Hover,
        );

        // 1 cycle region + 4 cell regions + 4 hover regions = 9.
        assert_eq!(drag.regions().len(), 9);

        let mut cycle_count = 0;
        let mut cell_count = 0;
        let mut hover_count = 0;
        for r in drag.regions() {
            match r.action {
                TestAction::Cycle => cycle_count += 1,
                TestAction::Cell(_) => cell_count += 1,
                TestAction::Hover(_) => hover_count += 1,
            }
        }
        assert_eq!(cycle_count, 1, "exactly one cycle region");
        assert_eq!(cell_count, 4, "one cell region per value");
        assert_eq!(hover_count, 4, "one hover region per value");
    }

    #[test]
    fn tooltips_pass_draws_when_mouse_over_hover_region() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 100).unwrap();
        let mut drag: DragState<TestAction> = DragState::new();

        // Hover region with plenty of room above and below.
        drag.push_region(50.0, 60.0, 10.0, 10.0, TestAction::Hover(0));
        drag.set_mouse(55.0, 65.0);

        draw_grid_tooltips_pass(
            &mut pm,
            &mut tr,
            &drag,
            1.0,
            (0.0, 0.0, 300.0, 100.0),
            |action| match action {
                TestAction::Hover(_) => Some("8x"),
                _ => None,
            },
        );

        // Default placement is above the hover region.
        let sample_x = 55_u32;
        let sample_y = 50_u32;
        let p = pm.pixels()[(sample_y * pm.width() + sample_x) as usize];
        assert!(
            p.alpha() > 0,
            "tooltip should paint above hover region; got alpha=0"
        );
    }

    #[test]
    fn tooltips_pass_no_op_when_mouse_outside_window() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 100).unwrap();
        let mut drag: DragState<TestAction> = DragState::new();

        drag.push_region(50.0, 60.0, 10.0, 10.0, TestAction::Hover(0));
        // Note: drag.mouse_in_window() defaults to false; do not call set_mouse.

        draw_grid_tooltips_pass(
            &mut pm,
            &mut tr,
            &drag,
            1.0,
            (0.0, 0.0, 300.0, 100.0),
            |_| Some("8x"),
        );

        // Sample where the ABOVE-default tooltip would have landed if drawn.
        let p = pm.pixels()[(50 * pm.width() + 55) as usize];
        assert_eq!(
            p.alpha(),
            0,
            "no tooltip should be drawn when mouse is outside window"
        );
    }

    /// Default placement: tooltip lands ABOVE the cell so it sits clear of
    /// the arrow cursor (which extends down-right from its hot spot).
    /// Mirrors the slider editing-highlight test pattern.
    #[test]
    fn draw_grid_tooltip_paints_above_cell_by_default() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 100).unwrap();

        // Cell mid-pixmap with plenty of room both above and below.
        let cell_rect = (50.0, 60.0, 10.0, 10.0);
        let parent_clip = (0.0, 0.0, 300.0, 100.0);

        draw_grid_tooltip(&mut pm, &mut tr, cell_rect, "8x", 1.0, parent_clip);

        let sample_x = (cell_rect.0 + cell_rect.2 * 0.5) as u32;
        let above_y = (cell_rect.1 - 12.0) as u32;
        let below_y = (cell_rect.1 + cell_rect.3 + 12.0) as u32;
        let above_px = pm.pixels()[(above_y * pm.width() + sample_x) as usize];
        let below_px = pm.pixels()[(below_y * pm.width() + sample_x) as usize];

        assert!(
            above_px.alpha() > 0,
            "tooltip should paint above the cell by default"
        );
        assert_eq!(
            below_px.alpha(),
            0,
            "below-the-cell region should remain untouched when above fits"
        );
    }

    /// When placing the tooltip above the cell would overflow `parent_clip`,
    /// the tooltip flips below the cell. Anchored near the top of the clip.
    #[test]
    fn draw_grid_tooltip_flips_below_when_above_overflows() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 100).unwrap();

        // Cell sitting near the top of the parent clip — almost no room above.
        let cell_rect = (50.0, 4.0, 10.0, 7.0);
        let parent_clip = (0.0, 0.0, 300.0, 100.0);

        draw_grid_tooltip(&mut pm, &mut tr, cell_rect, "8x", 1.0, parent_clip);

        let sample_x = (cell_rect.0 + cell_rect.2 * 0.5) as u32;
        let below_y = (cell_rect.1 + cell_rect.3 + 12.0) as u32;
        let below_px = pm.pixels()[(below_y * pm.width() + sample_x) as usize];
        assert!(
            below_px.alpha() > 0,
            "after flip-below, pixels below the cell should be touched; got alpha=0"
        );
    }

    #[test]
    fn draw_grid_selector_n0_no_panic() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 60).unwrap();

        let layout = grid_selector_layout(0.0, 0.0, 200.0, 30.0, 0);
        // Should not panic with empty cell_rects.
        draw_grid_selector(&mut pm, &mut tr, &layout, "", usize::MAX);
    }

    #[test]
    fn draw_grid_selector_lights_active_cell() {
        let font = test_font_data();
        let mut tr = TextRenderer::new(&font);
        let mut pm = Pixmap::new(300, 60).unwrap();

        // 4 values, active = index 2 (bottom-left in a 2x2 grid).
        let layout = grid_selector_layout(10.0, 10.0, 200.0, 30.0, 4);
        draw_grid_selector(&mut pm, &mut tr, &layout, "8x", 2);

        let active = pixel_at_cell_center(&pm, layout.cell_rects[2]);
        let inactive = pixel_at_cell_center(&pm, layout.cell_rects[0]);

        let expected_active = color_to_rgba_u8(color_accent());
        let expected_inactive = color_to_rgba_u8(color_control_bg());

        assert_eq!(
            active, expected_active,
            "active cell center should be color_accent()"
        );
        assert_eq!(
            inactive, expected_inactive,
            "inactive cell center should be color_control_bg()"
        );
    }

    /// Per the layout table in the spec (§3): for each N, expected (rows, cols, cell_count).
    /// Grid is right-aligned within the bounding box, with `cell_gap` of
    /// right padding (so `grid_x + grid_w + cell_gap == x + w`).
    #[test]
    fn grid_right_aligned_within_bounds() {
        let x = 10.0;
        let w = 200.0;
        let h = 30.0;
        let layout = grid_selector_layout(x, 0.0, w, h, 4);
        let (gx, _, gw, _) = layout.grid_rect;
        let cell_gap = (h / 3.0) * 0.15;
        let expected_right = x + w - cell_gap;
        assert!(
            (gx + gw - expected_right).abs() < 0.001,
            "grid right edge should be x+w-cell_gap; got grid_x={}, grid_w={}, expected_right={}",
            gx,
            gw,
            expected_right
        );
    }

    /// Value-text region starts at `x`, and its right edge plus `inner_gap`
    /// equals the grid's left edge.
    #[test]
    fn value_rect_left_of_grid() {
        let x = 5.0;
        let layout = grid_selector_layout(x, 0.0, 200.0, 30.0, 4);
        let (vx, _, vw, _) = layout.value_rect;
        let (gx, _, _, _) = layout.grid_rect;
        let inner_gap = (30.0_f32 / 3.0) * 0.5;

        assert!(
            (vx - x).abs() < 0.001,
            "value_rect.x should equal x; got {}",
            vx
        );
        assert!(
            (vx + vw + inner_gap - gx).abs() < 0.001,
            "value_rect right + inner_gap should equal grid_x; got vx+vw={}, inner_gap={}, gx={}",
            vx + vw,
            inner_gap,
            gx
        );
    }

    /// Grid is vertically centered within `(y, h)`: top slack equals bottom slack.
    #[test]
    fn cells_vertically_centered() {
        let y = 4.0;
        let h = 30.0;
        // N=4 → 2 rows, so grid_h = 2/3 * h, leaving h/6 slack top and bottom.
        let layout = grid_selector_layout(0.0, y, 200.0, h, 4);
        let (_, gy, _, gh) = layout.grid_rect;
        let top_slack = gy - y;
        let bot_slack = (y + h) - (gy + gh);
        assert!(
            (top_slack - bot_slack).abs() < 0.001,
            "top slack ({}) should equal bottom slack ({})",
            top_slack,
            bot_slack
        );
    }

    /// `value_count = 0` returns an empty cell list, no panic. The value-text
    /// region still exists.
    #[test]
    fn layout_n0_returns_empty_cells() {
        let layout = grid_selector_layout(0.0, 0.0, 200.0, 30.0, 0);
        assert_eq!(layout.cell_rects.len(), 0);
        assert_eq!(layout.rows, 0);
        assert_eq!(layout.cols, 0);
        // Value rect still spans the bounding height.
        let (_, _, _, vh) = layout.value_rect;
        assert!((vh - 30.0).abs() < 0.001);
    }

    /// Typewriter fill: value index 0 → top-left, indices fill left-to-right
    /// across each row before dropping to the next.
    #[test]
    fn cell_rects_in_typewriter_order() {
        // 4 values in a 2-row × 2-col grid.
        let layout = grid_selector_layout(0.0, 0.0, 200.0, 30.0, 4);
        assert_eq!(layout.rows, 2);
        assert_eq!(layout.cols, 2);

        let (top_left_x, top_left_y, _, _) = layout.cell_rects[0];
        let (top_right_x, top_right_y, _, _) = layout.cell_rects[1];
        let (bot_left_x, bot_left_y, _, _) = layout.cell_rects[2];
        let (bot_right_x, bot_right_y, _, _) = layout.cell_rects[3];

        // Index 0 and 1 share the top row; 2 and 3 share the bottom row.
        assert!(
            (top_left_y - top_right_y).abs() < 0.001,
            "indices 0 and 1 should be in the same row"
        );
        assert!(
            (bot_left_y - bot_right_y).abs() < 0.001,
            "indices 2 and 3 should be in the same row"
        );
        assert!(
            bot_left_y > top_left_y,
            "row 1 should be below row 0 (greater y)"
        );

        // Within a row, index N+1 is to the right of index N.
        assert!(
            top_right_x > top_left_x,
            "index 1 should be right of index 0"
        );
        assert!(
            bot_right_x > bot_left_x,
            "index 3 should be right of index 2"
        );

        // Index `cols` (=2) starts at the leftmost x of its row.
        assert!(
            (bot_left_x - top_left_x).abs() < 0.001,
            "indices 0 and 2 should share the leftmost x"
        );
    }

    #[test]
    fn layout_n1_through_n10_matches_table() {
        let cases: &[(usize, usize, usize)] = &[
            // (N, rows, cols)
            (1, 1, 1),
            (2, 2, 1),
            (3, 3, 1),
            (4, 2, 2),
            (5, 3, 2),
            (6, 3, 2),
            (7, 3, 3),
            (8, 3, 3),
            (9, 3, 3),
            (10, 3, 4),
        ];
        for &(n, expected_rows, expected_cols) in cases {
            let layout = grid_selector_layout(0.0, 0.0, 200.0, 30.0, n);
            assert_eq!(
                layout.rows, expected_rows,
                "N={}: expected rows={}, got {}",
                n, expected_rows, layout.rows
            );
            assert_eq!(
                layout.cols, expected_cols,
                "N={}: expected cols={}, got {}",
                n, expected_cols, layout.cols
            );
            assert_eq!(
                layout.cell_rects.len(),
                n,
                "N={}: expected {} cell rects, got {}",
                n,
                n,
                layout.cell_rects.len()
            );
        }
    }
}
