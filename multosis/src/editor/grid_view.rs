//! Renders the 16×32 routing grid and the live wavefront into the editor
//! pixmap. Geometry is in logical units; every draw multiplies by `scale`.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7.

use crate::grid::{Direction, Grid, COLS, ROWS};
use crate::wavefront_display::WavefrontDisplay;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// Logical height of the top status strip.
pub const STATUS_H: f32 = 48.0;
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

/// Apply a click on cell `(row, col)`'s `zone` to the grid. A left click
/// (`right == false`) toggles a send direction (octant) or the `enabled`
/// flag (centre); a right click toggles the `is_start` flag (centre only)
/// and does nothing on an octant.
pub fn apply_grid_click(
    grid: &mut Grid,
    row: usize,
    col: usize,
    zone: CellZone,
    right: bool,
) {
    let cell = grid.cell_mut(row, col);
    match (zone, right) {
        (CellZone::Send(dir), false) => cell.toggle_send(dir),
        (CellZone::Center, false) => cell.enabled = !cell.enabled,
        (CellZone::Center, true) => cell.is_start = !cell.is_start,
        (CellZone::Send(_), true) => {} // right-click on an octant: ignored
    }
}

/// Cell background when the cell is enabled.
fn color_cell_enabled() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x33, 0x37, 0x42, 0xFF)
}
/// Cell background when the cell is disabled.
fn color_cell_disabled() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x20, 0x22, 0x29, 0xFF)
}
/// A lit send-direction pip.
fn color_send() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x6f, 0x8a, 0xb8, 0xFF)
}
/// A start-cell marker.
fn color_start() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x5f, 0xd0, 0x9a, 0xFF)
}
/// The loop-region outline.
fn color_loop() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xFF)
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

    // Send pips: a small square pulled toward each sent direction.
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
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
pub fn draw_grid(pixmap: &mut Pixmap, grid: &Grid, scale: f32) {
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
        assert_eq!(*g.cell(3, 3), before, "right-click on an octant does nothing");
    }
}
