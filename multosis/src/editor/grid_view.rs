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
        assert_eq!((x, y, w, h), (2.0 * CELL * 2.0, (STATUS_H + CELL) * 2.0, CELL * 2.0, CELL * 2.0));
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
}
