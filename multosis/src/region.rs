//! Copy and paste of the block of cells under the loop region.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.3.

use crate::grid::{Cell, Grid, COLS, ROWS};

/// A rectangular block of cells lifted from a grid's loop region. A
/// GUI-thread clipboard value — it owns a `Vec` and never crosses to the
/// audio thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionSnapshot {
    pub rows: usize,
    pub cols: usize,
    /// `rows * cols` cells, row-major.
    pub cells: Vec<Cell>,
}

impl Grid {
    /// Snapshot the cells currently covered by the loop region.
    pub fn copy_region(&self) -> RegionSnapshot {
        let lr = self.loop_region;
        let rows = lr.row1 - lr.row0 + 1;
        let cols = lr.col1 - lr.col0 + 1;
        let mut cells = Vec::with_capacity(rows * cols);
        for r in lr.row0..=lr.row1 {
            for c in lr.col0..=lr.col1 {
                cells.push(*self.cell(r, c));
            }
        }
        RegionSnapshot { rows, cols, cells }
    }

    /// Paste `snap` anchored at the loop region's top-left corner. Cells that
    /// would fall outside the grid are truncated (skipped).
    pub fn paste_region(&mut self, snap: &RegionSnapshot) {
        let lr = self.loop_region;
        for sr in 0..snap.rows {
            for sc in 0..snap.cols {
                let dr = lr.row0 + sr;
                let dc = lr.col0 + sc;
                if dr < ROWS && dc < COLS {
                    *self.cell_mut(dr, dc) = snap.cells[sr * snap.cols + sc];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Direction, LoopRegion};

    #[test]
    fn copy_region_snapshots_the_loop_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 1,
            row1: 3,
            col0: 5,
            col1: 9,
        };
        g.cell_mut(1, 5).enabled = false;
        let snap = g.copy_region();
        assert_eq!(snap.rows, 3);
        assert_eq!(snap.cols, 5);
        assert_eq!(snap.cells.len(), 15);
        // First snapshot cell is the region's top-left, (1,5).
        assert!(!snap.cells[0].enabled);
    }

    #[test]
    fn paste_region_writes_at_the_loop_region_top_left() {
        let mut g = Grid::default_routing();
        // Copy a 2×2 block from rows 0..2, cols 0..2 with a marker.
        g.loop_region = LoopRegion {
            row0: 0,
            row1: 1,
            col0: 0,
            col1: 1,
        };
        g.cell_mut(0, 0).sends = 0b1111_1111;
        let snap = g.copy_region();
        // Move the loop region and paste.
        g.loop_region = LoopRegion {
            row0: 10,
            row1: 11,
            col0: 20,
            col1: 21,
        };
        g.paste_region(&snap);
        assert_eq!(g.cell(10, 20).sends, 0b1111_1111);
        // A cell outside the pasted block is unchanged (still East-only).
        assert_eq!(g.cell(12, 22).sends, 1u8 << Direction::E.bit());
    }

    #[test]
    fn paste_region_truncates_on_overflow() {
        let mut g = Grid::default_routing();
        // Copy a 3×3 block.
        g.loop_region = LoopRegion {
            row0: 0,
            row1: 2,
            col0: 0,
            col1: 2,
        };
        g.cell_mut(0, 0).enabled = false;
        let snap = g.copy_region();
        // Anchor the paste so only a 1×1 corner fits inside the grid.
        g.loop_region = LoopRegion {
            row0: ROWS - 1,
            row1: ROWS - 1,
            col0: COLS - 1,
            col1: COLS - 1,
        };
        g.paste_region(&snap); // must not panic
        // The one in-bounds cell received the snapshot's top-left.
        assert!(!g.cell(ROWS - 1, COLS - 1).enabled);
    }
}
