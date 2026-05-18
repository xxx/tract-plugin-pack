//! Renders the 16×32 routing grid and the live wavefront into the editor
//! pixmap. Geometry is in logical units; every draw multiplies by `scale`.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7.

use crate::grid::{Direction, Grid, LoopRegion, COLS, ROWS};
use crate::wavefront_display::WavefrontDisplay;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// Logical height of the top toolbar strip (two rows of `TOOLBAR_ROW_H`).
pub const STATUS_H: f32 = 88.0;
/// Logical height of one toolbar row.
pub const TOOLBAR_ROW_H: f32 = 44.0;
/// Logical edge length of one square grid cell.
pub const CELL: f32 = 33.0;

/// The physical-pixel rectangle `(x, y, w, h)` of cell `(row, col)` at `scale`.
pub fn cell_rect(row: usize, col: usize, scale: f32) -> (f32, f32, f32, f32) {
    let x = col as f32 * CELL * scale;
    let y = (STATUS_H + row as f32 * CELL) * scale;
    let side = CELL * scale;
    (x, y, side, side)
}

/// The cell containing physical-pixel point `(px, py)` at `scale`, or `None`
/// if the point is in the status strip or outside the grid.
pub fn cell_at(px: f32, py: f32, scale: f32) -> Option<(usize, usize)> {
    if scale <= 0.0 || px < 0.0 {
        return None;
    }
    let logical_y = py / scale - STATUS_H;
    if logical_y < 0.0 {
        return None;
    }
    let col = (px / scale / CELL) as usize;
    let row = (logical_y / CELL) as usize;
    if row < ROWS && col < COLS {
        Some((row, col))
    } else {
        None
    }
}

/// A clickable zone within a cell: the centre, or one of the 8 send
/// directions (the cell is split into a 3×3 — centre third + 8 surrounders).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellZone {
    Center,
    Send(Direction),
}

/// The cell and zone under physical-pixel point `(px, py)` at `scale`, or
/// `None` if the point is outside the grid.
pub fn cell_zone(px: f32, py: f32, scale: f32) -> Option<(usize, usize, CellZone)> {
    let (row, col) = cell_at(px, py, scale)?;
    let (cx, cy, w, h) = cell_rect(row, col, scale);
    // Third index 0..3 within the cell, on each axis.
    let tcol = (((px - cx) / w) * 3.0).floor().clamp(0.0, 2.0) as i32;
    let trow = (((py - cy) / h) * 3.0).floor().clamp(0.0, 2.0) as i32;
    if trow == 1 && tcol == 1 {
        return Some((row, col, CellZone::Center));
    }
    // A non-centre third maps to a unit (drow, dcol) step.
    let dir = Direction::from_delta(trow - 1, tcol - 1)?;
    Some((row, col, CellZone::Send(dir)))
}

/// One draggable edge of the loop-region rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionEdge {
    /// Left edge — drags `col0`.
    Left,
    /// Right edge — drags `col1`.
    Right,
    /// Top edge — drags `row0`.
    Top,
    /// Bottom edge — drags `row1`.
    Bottom,
}

/// Half-width of the grab band straddling a region edge, in logical pixels.
const EDGE_BAND: f32 = 6.0;

/// The loop-region edge under physical-pixel point `(px, py)` at `scale`, or
/// `None`. The cursor must be within the grid drawing area — points in the
/// toolbar strip never hit an edge.
pub fn region_edge_hit(px: f32, py: f32, region: LoopRegion, scale: f32) -> Option<RegionEdge> {
    // Reject the toolbar strip and anything outside the grid.
    let grid_top = STATUS_H * scale;
    let grid_bottom = (STATUS_H + ROWS as f32 * CELL) * scale;
    let grid_right = COLS as f32 * CELL * scale;
    if py < grid_top || py > grid_bottom || px < 0.0 || px > grid_right {
        return None;
    }
    let (x0, y0, _, _) = cell_rect(region.row0, region.col0, scale);
    let (x1, y1, w1, h1) = cell_rect(region.row1, region.col1, scale);
    let right = x1 + w1;
    let bottom = y1 + h1;
    let band = EDGE_BAND * scale;
    let in_rows = py >= y0 && py <= bottom;
    let in_cols = px >= x0 && px <= right;
    if in_rows && (px - x0).abs() <= band {
        Some(RegionEdge::Left)
    } else if in_rows && (px - right).abs() <= band {
        Some(RegionEdge::Right)
    } else if in_cols && (py - y0).abs() <= band {
        Some(RegionEdge::Top)
    } else if in_cols && (py - bottom).abs() <= band {
        Some(RegionEdge::Bottom)
    } else {
        None
    }
}

/// The grid column under physical-pixel x `px` at `scale`, clamped to
/// `0..=COLS-1`. Used while dragging a region edge, where the cursor may
/// stray off the grid.
pub fn column_at(px: f32, scale: f32) -> usize {
    let col = (px / scale / CELL).floor();
    col.clamp(0.0, (COLS - 1) as f32) as usize
}

/// The grid row under physical-pixel y `py` at `scale`, clamped to
/// `0..=ROWS-1`. Used while dragging a region edge.
pub fn row_at(py: f32, scale: f32) -> usize {
    let row = ((py / scale - STATUS_H) / CELL).floor();
    row.clamp(0.0, (ROWS - 1) as f32) as usize
}

/// Resize the loop region by dragging `edge` to grid `index` (a column index
/// for `Left`/`Right`, a row index for `Top`/`Bottom`, expected already
/// clamped to `0..=COLS-1` / `0..=ROWS-1`). The moved bound is clamped against
/// its opposite so the region can never invert and can shrink to 1×1.
pub fn apply_region_drag(region: LoopRegion, edge: RegionEdge, index: usize) -> LoopRegion {
    let mut r = region;
    match edge {
        RegionEdge::Left => r.col0 = index.min(r.col1),
        RegionEdge::Right => r.col1 = index.max(r.col0).min(COLS - 1),
        RegionEdge::Top => r.row0 = index.min(r.row1),
        RegionEdge::Bottom => r.row1 = index.max(r.row0).min(ROWS - 1),
    }
    r
}

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

/// Apply a click on cell `(row, col)`'s `zone` to the grid. A left click
/// (`right == false`) toggles a send direction (octant) or the `enabled`
/// flag (centre); a right click toggles the `is_start` flag (centre only)
/// and does nothing on an octant.
pub fn apply_grid_click(grid: &mut Grid, row: usize, col: usize, zone: CellZone, right: bool) {
    let cell = grid.cell_mut(row, col);
    match (zone, right) {
        (CellZone::Send(dir), false) => cell.toggle_send(dir),
        (CellZone::Center, false) => cell.enabled = !cell.enabled,
        (CellZone::Center, true) => cell.is_start = !cell.is_start,
        (CellZone::Send(_), true) => {} // right-click on an octant: ignored
    }
}

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
pub fn arrowhead_vertices(cx: f32, cy: f32, w: f32, dir: Direction, scale: f32) -> [(f32, f32); 3] {
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

/// Cell background when the cell is enabled.
fn color_cell_enabled() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x33, 0x37, 0x42, 0xFF)
}
/// Cell background when the cell is disabled.
fn color_cell_disabled() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x20, 0x22, 0x29, 0xFF)
}
/// A lit send-direction arrowhead.
fn color_send() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x86, 0xa6, 0xe8, 0xFF)
}
/// A start-cell marker.
fn color_start() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x5f, 0xd0, 0x9a, 0xFF)
}
/// The loop-region outline.
fn color_loop() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xFF)
}

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

/// Draw one cell's background, send pips, and start marker.
fn draw_cell(pixmap: &mut Pixmap, row: usize, col: usize, cell: &crate::grid::Cell, scale: f32) {
    let (x, y, w, h) = cell_rect(row, col, scale);
    let gap = 1.0 * scale;
    // Background (inset by the gap so cells read as a grid).
    let bg = if cell.enabled {
        color_cell_enabled()
    } else {
        color_cell_disabled()
    };
    widgets::draw_rect(pixmap, x + gap, y + gap, w - 2.0 * gap, h - 2.0 * gap, bg);

    // Send arrowheads: a triangle pointing the way each trigger flows.
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    for dir in Direction::ALL {
        if !cell.sends_to(dir) {
            continue;
        }
        let verts = arrowhead_vertices(cx, cy, w, dir, scale);
        fill_triangle(pixmap, verts, color_send());
    }

    // Start marker: a thin inset outline.
    if cell.is_start {
        widgets::draw_rect_outline(
            pixmap,
            x + gap,
            y + gap,
            w - 2.0 * gap,
            h - 2.0 * gap,
            color_start(),
            1.5 * scale,
        );
    }
}

/// Draw the whole grid — every cell, then the loop-region outline.
pub fn draw_grid(pixmap: &mut Pixmap, grid: &Grid, scale: f32, cursor: Option<(f32, f32)>) {
    for r in 0..ROWS {
        for c in 0..COLS {
            draw_cell(pixmap, r, c, grid.cell(r, c), scale);
        }
    }
    // Loop-region outline: a rectangle spanning the region's cells.
    let lr = grid.loop_region;
    let (x0, y0, _, _) = cell_rect(lr.row0, lr.col0, scale);
    let (x1, y1, w1, h1) = cell_rect(lr.row1, lr.col1, scale);
    widgets::draw_rect_outline(
        pixmap,
        x0,
        y0,
        (x1 + w1) - x0,
        (y1 + h1) - y0,
        color_loop(),
        2.0 * scale,
    );

    // Drag-handle nubs at the midpoint of each region edge.
    let mid_x = (x0 + (x1 + w1)) / 2.0;
    let mid_y = (y0 + (y1 + h1)) / 2.0;
    let long = 16.0 * scale;
    let thick = 4.0 * scale;
    let nub = color_loop();
    // Left and right edges: vertical nubs.
    widgets::draw_rect(
        pixmap,
        x0 - thick / 2.0,
        mid_y - long / 2.0,
        thick,
        long,
        nub,
    );
    widgets::draw_rect(
        pixmap,
        (x1 + w1) - thick / 2.0,
        mid_y - long / 2.0,
        thick,
        long,
        nub,
    );
    // Top and bottom edges: horizontal nubs.
    widgets::draw_rect(
        pixmap,
        mid_x - long / 2.0,
        y0 - thick / 2.0,
        long,
        thick,
        nub,
    );
    widgets::draw_rect(
        pixmap,
        mid_x - long / 2.0,
        (y1 + h1) - thick / 2.0,
        long,
        thick,
        nub,
    );

    // Move grip — drawn only while the cursor is inside the loop region.
    if let Some((cur_x, cur_y)) = cursor {
        if cur_x >= x0 && cur_x <= (x1 + w1) && cur_y >= y0 && cur_y <= (y1 + h1) {
            let (gx, gy, gw, gh) = region_grip_rect(lr, scale);
            widgets::draw_rect(pixmap, gx, gy, gw, gh, color_loop());
        }
    }
}

/// A lit wavefront cell.
fn color_wavefront() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0xd8, 0x89, 0x3a, 0xFF)
}

/// Overlay the live wavefront — every lit cell gets an orange core square.
pub fn draw_wavefront(pixmap: &mut Pixmap, wf: &WavefrontDisplay, scale: f32) {
    for r in 0..ROWS {
        for c in 0..COLS {
            if !wf.is_lit(r, c) {
                continue;
            }
            let (x, y, w, h) = cell_rect(r, c, scale);
            let inset = w * 0.22;
            widgets::draw_rect(
                pixmap,
                x + inset,
                y + inset,
                w - 2.0 * inset,
                h - 2.0 * inset,
                color_wavefront(),
            );
        }
    }
}

/// Draw the top status strip — the plugin title.
pub fn draw_status(pixmap: &mut Pixmap, tr: &mut widgets::TextRenderer, scale: f32) {
    let strip_h = STATUS_H * scale;
    widgets::draw_rect(
        pixmap,
        0.0,
        0.0,
        pixmap.width() as f32,
        strip_h,
        widgets::color_control_bg(),
    );
    let size = 20.0 * scale;
    tr.draw_text(
        pixmap,
        12.0 * scale,
        strip_h / 2.0 + size * 0.36,
        "MULTOSIS",
        size,
        widgets::color_text(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_size_matches_the_grid() {
        // The editor window in editor.rs must exactly fit the grid.
        assert_eq!(crate::editor::WINDOW_WIDTH, (COLS as f32 * CELL) as u32);
        assert_eq!(
            crate::editor::WINDOW_HEIGHT,
            (STATUS_H + ROWS as f32 * CELL) as u32
        );
    }

    #[test]
    fn cell_rect_top_left_and_bottom_right() {
        let (x, y, w, h) = cell_rect(0, 0, 1.0);
        assert_eq!((x, y, w, h), (0.0, STATUS_H, CELL, CELL));
        let (x, y, _, _) = cell_rect(ROWS - 1, COLS - 1, 1.0);
        assert_eq!(x, (COLS - 1) as f32 * CELL);
        assert_eq!(y, STATUS_H + (ROWS - 1) as f32 * CELL);
    }

    #[test]
    fn cell_rect_scales() {
        let (x, y, w, h) = cell_rect(1, 2, 2.0);
        assert_eq!(
            (x, y, w, h),
            (
                2.0 * CELL * 2.0,
                (STATUS_H + CELL) * 2.0,
                CELL * 2.0,
                CELL * 2.0
            )
        );
    }

    #[test]
    fn cell_at_maps_a_point_back_to_a_cell() {
        // A point inside cell (3, 7) resolves to (3, 7).
        let (x, y, w, h) = cell_rect(3, 7, 1.5);
        let mid = (x + w / 2.0, y + h / 2.0);
        assert_eq!(cell_at(mid.0, mid.1, 1.5), Some((3, 7)));
        // A point in the status strip is not a cell.
        assert_eq!(cell_at(10.0, 5.0, 1.0), None);
        // A point past the grid is not a cell.
        assert_eq!(cell_at(100_000.0, 100_000.0, 1.0), None);
    }

    #[test]
    fn cell_zone_centre_third_is_center() {
        // The middle of cell (4, 6) resolves to that cell's Center zone.
        let (x, y, w, h) = cell_rect(4, 6, 1.0);
        let z = cell_zone(x + w / 2.0, y + h / 2.0, 1.0);
        assert_eq!(z, Some((4, 6, CellZone::Center)));
    }

    #[test]
    fn cell_zone_edges_map_to_directions() {
        let (x, y, w, h) = cell_rect(4, 6, 1.0);
        // Top-centre third -> North.
        let top = cell_zone(x + w / 2.0, y + h / 6.0, 1.0);
        assert_eq!(top, Some((4, 6, CellZone::Send(Direction::N))));
        // Right-centre third -> East.
        let right = cell_zone(x + w * 5.0 / 6.0, y + h / 2.0, 1.0);
        assert_eq!(right, Some((4, 6, CellZone::Send(Direction::E))));
        // Bottom-right third -> South-East.
        let se = cell_zone(x + w * 5.0 / 6.0, y + h * 5.0 / 6.0, 1.0);
        assert_eq!(se, Some((4, 6, CellZone::Send(Direction::SE))));
    }

    #[test]
    fn cell_zone_outside_the_grid_is_none() {
        assert_eq!(cell_zone(10.0, 5.0, 1.0), None); // status strip
        assert_eq!(cell_zone(-5.0, 200.0, 1.0), None); // left of grid
    }

    #[test]
    fn left_click_octant_toggles_a_send() {
        let mut g = Grid::default_routing(); // every cell sends E only
        apply_grid_click(&mut g, 2, 3, CellZone::Send(Direction::S), false);
        assert!(g.cell(2, 3).sends_to(Direction::S));
        // A second left click on the same octant toggles it back off.
        apply_grid_click(&mut g, 2, 3, CellZone::Send(Direction::S), false);
        assert!(!g.cell(2, 3).sends_to(Direction::S));
        // The pre-existing East send is untouched.
        assert!(g.cell(2, 3).sends_to(Direction::E));
    }

    #[test]
    fn left_click_centre_toggles_enabled() {
        let mut g = Grid::default_routing(); // every cell enabled
        apply_grid_click(&mut g, 5, 5, CellZone::Center, false);
        assert!(!g.cell(5, 5).enabled);
        apply_grid_click(&mut g, 5, 5, CellZone::Center, false);
        assert!(g.cell(5, 5).enabled);
    }

    #[test]
    fn right_click_centre_toggles_start() {
        let mut g = Grid::default_routing();
        // Column 7 is not a start cell by default.
        assert!(!g.cell(1, 7).is_start);
        apply_grid_click(&mut g, 1, 7, CellZone::Center, true);
        assert!(g.cell(1, 7).is_start);
        apply_grid_click(&mut g, 1, 7, CellZone::Center, true);
        assert!(!g.cell(1, 7).is_start);
    }

    #[test]
    fn right_click_octant_is_ignored() {
        let mut g = Grid::default_routing();
        let before = *g.cell(3, 3);
        apply_grid_click(&mut g, 3, 3, CellZone::Send(Direction::W), true);
        assert_eq!(
            *g.cell(3, 3),
            before,
            "right-click on an octant does nothing"
        );
    }

    #[test]
    fn region_edge_hit_finds_each_edge() {
        let region = LoopRegion {
            row0: 2,
            row1: 8,
            col0: 4,
            col1: 20,
        };
        let (x0, y0, _, _) = cell_rect(2, 4, 1.0);
        let (x1, y1, w, h) = cell_rect(8, 20, 1.0);
        let mid_x = (x0 + x1 + w) / 2.0;
        let mid_y = (y0 + y1 + h) / 2.0;
        assert_eq!(
            region_edge_hit(x0, mid_y, region, 1.0),
            Some(RegionEdge::Left)
        );
        assert_eq!(
            region_edge_hit(x1 + w, mid_y, region, 1.0),
            Some(RegionEdge::Right)
        );
        assert_eq!(
            region_edge_hit(mid_x, y0, region, 1.0),
            Some(RegionEdge::Top)
        );
        assert_eq!(
            region_edge_hit(mid_x, y1 + h, region, 1.0),
            Some(RegionEdge::Bottom)
        );
    }

    #[test]
    fn region_edge_hit_misses_interior_and_toolbar() {
        let region = LoopRegion {
            row0: 2,
            row1: 8,
            col0: 4,
            col1: 20,
        };
        // Centre of an interior cell — far from every edge.
        let (xc, yc, wc, hc) = cell_rect(5, 12, 1.0);
        assert_eq!(
            region_edge_hit(xc + wc / 2.0, yc + hc / 2.0, region, 1.0),
            None
        );
        // A point up in the toolbar strip (y < STATUS_H).
        let (x0, _, _, _) = cell_rect(2, 4, 1.0);
        assert_eq!(region_edge_hit(x0, 10.0, region, 1.0), None);
    }

    #[test]
    fn column_at_and_row_at_clamp_to_grid_bounds() {
        assert_eq!(column_at(0.0, 1.0), 0);
        assert_eq!(column_at(CELL * 5.5, 1.0), 5);
        assert_eq!(column_at(CELL * 10_000.0, 1.0), COLS - 1);
        assert_eq!(row_at(0.0, 1.0), 0); // above the grid -> clamp to 0
        assert_eq!(row_at(STATUS_H, 1.0), 0); // exactly at the grid's top
        assert_eq!(row_at(STATUS_H + CELL * 3.5, 1.0), 3);
        assert_eq!(row_at(STATUS_H + CELL * 10_000.0, 1.0), ROWS - 1);
    }

    #[test]
    fn apply_region_drag_left_edge_moves_col0() {
        let r = LoopRegion {
            row0: 0,
            row1: 15,
            col0: 5,
            col1: 20,
        };
        let out = apply_region_drag(r, RegionEdge::Left, 8);
        assert_eq!((out.col0, out.col1), (8, 20));
    }

    #[test]
    fn apply_region_drag_right_edge_moves_col1() {
        let r = LoopRegion {
            row0: 0,
            row1: 15,
            col0: 5,
            col1: 20,
        };
        let out = apply_region_drag(r, RegionEdge::Right, 12);
        assert_eq!((out.col0, out.col1), (5, 12));
    }

    #[test]
    fn apply_region_drag_top_and_bottom_move_rows() {
        let r = LoopRegion {
            row0: 3,
            row1: 12,
            col0: 0,
            col1: 31,
        };
        assert_eq!(apply_region_drag(r, RegionEdge::Top, 6).row0, 6);
        assert_eq!(apply_region_drag(r, RegionEdge::Bottom, 9).row1, 9);
    }

    #[test]
    fn apply_region_drag_cannot_invert_bounds() {
        let r = LoopRegion {
            row0: 4,
            row1: 10,
            col0: 6,
            col1: 18,
        };
        // Drag the left edge past the right edge -> clamps to col1.
        assert_eq!(apply_region_drag(r, RegionEdge::Left, 25).col0, 18);
        // Drag the right edge past the left edge -> clamps to col0.
        assert_eq!(apply_region_drag(r, RegionEdge::Right, 2).col1, 6);
        // Same for the row edges.
        assert_eq!(apply_region_drag(r, RegionEdge::Top, 15).row0, 10);
        assert_eq!(apply_region_drag(r, RegionEdge::Bottom, 1).row1, 4);
    }

    #[test]
    fn apply_region_drag_can_collapse_to_1x1() {
        let r = LoopRegion {
            row0: 4,
            row1: 10,
            col0: 6,
            col1: 18,
        };
        // Collapse to the single cell (10, 18).
        let a = apply_region_drag(r, RegionEdge::Left, 18);
        let b = apply_region_drag(a, RegionEdge::Top, 10);
        assert_eq!((b.row0, b.row1, b.col0, b.col1), (10, 10, 18, 18));
        assert!(b.contains(10, 18));
        assert!(!b.contains(9, 18));
    }

    #[test]
    fn apply_region_drag_result_is_already_normalized() {
        let r = LoopRegion::full();
        let out = apply_region_drag(r, RegionEdge::Right, 5);
        assert_eq!(out, out.normalized());
    }

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
        assert_eq!(
            (moved.row0, moved.row1, moved.col0, moved.col1),
            (9, 9, 20, 20)
        );
    }

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
        assert_eq!(
            cells_between((4, 1), (4, 4)),
            vec![(4, 1), (4, 2), (4, 3), (4, 4)]
        );
    }

    #[test]
    fn cells_between_vertical_has_no_gap() {
        assert_eq!(
            cells_between((1, 8), (4, 8)),
            vec![(1, 8), (2, 8), (3, 8), (4, 8)]
        );
    }

    #[test]
    fn cells_between_diagonal_steps_both_axes() {
        assert_eq!(
            cells_between((0, 0), (3, 3)),
            vec![(0, 0), (1, 1), (2, 2), (3, 3)]
        );
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
        assert!(
            dist < 3.0,
            "NE tip {ne:?} not near corner {corner:?} (dist {dist})"
        );
    }
}
