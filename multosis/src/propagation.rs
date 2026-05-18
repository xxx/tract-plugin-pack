//! The wavefront propagation engine: the lit-set, the per-step routing rule,
//! and the Initial/Running/Stopped lifecycle.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §5.

use crate::grid::{Direction, Grid, CELL_COUNT, COLS, ROWS};

/// The set of currently-lit cells. `Copy` so it crosses the GUI/audio
/// boundary cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wavefront {
    lit: [bool; CELL_COUNT],
}

impl Wavefront {
    /// An empty wavefront — no cells lit.
    pub fn empty() -> Self {
        Self {
            lit: [false; CELL_COUNT],
        }
    }

    /// Is the cell at `(row, col)` lit?
    pub fn is_lit(&self, row: usize, col: usize) -> bool {
        self.lit[Grid::index(row, col)]
    }

    /// Light or clear the cell at `(row, col)`.
    pub fn set(&mut self, row: usize, col: usize, on: bool) {
        self.lit[Grid::index(row, col)] = on;
    }

    /// Are no cells lit?
    pub fn is_empty(&self) -> bool {
        !self.lit.iter().any(|&l| l)
    }

    /// How many cells are lit.
    pub fn count(&self) -> usize {
        self.lit.iter().filter(|&&l| l).count()
    }
}

impl Default for Wavefront {
    fn default() -> Self {
        Self::empty()
    }
}

/// Compute the next wavefront from `current` by following every lit cell's
/// send directions through `grid.next_cell`. Pure — this is the seam a future
/// Game-of-Life routing mode plugs into (see the design doc §1.3).
pub fn step_manual(grid: &Grid, current: &Wavefront) -> Wavefront {
    let mut next = Wavefront::empty();
    for r in 0..ROWS {
        for c in 0..COLS {
            if !current.is_lit(r, c) {
                continue;
            }
            let cell = *grid.cell(r, c);
            for dir in Direction::ALL {
                if cell.sends_to(dir) {
                    let (nr, nc) = grid.next_cell(r, c, dir);
                    next.set(nr, nc, true);
                }
            }
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavefront_empty_has_nothing_lit() {
        let wf = Wavefront::empty();
        assert!(wf.is_empty());
        assert_eq!(wf.count(), 0);
        assert!(!wf.is_lit(0, 0));
    }

    #[test]
    fn wavefront_set_and_read_back() {
        let mut wf = Wavefront::empty();
        wf.set(3, 7, true);
        wf.set(10, 0, true);
        assert!(wf.is_lit(3, 7));
        assert!(wf.is_lit(10, 0));
        assert!(!wf.is_lit(3, 8));
        assert!(!wf.is_empty());
        assert_eq!(wf.count(), 2);

        wf.set(3, 7, false);
        assert!(!wf.is_lit(3, 7));
        assert_eq!(wf.count(), 1);
    }

    use crate::grid::{Cell, LoopRegion};

    /// A grid where every cell sends nowhere — a blank slate for tests.
    fn blank_grid() -> Grid {
        let mut g = Grid::default_routing();
        for cell in g.cells.iter_mut() {
            *cell = Cell::default(); // enabled, not start, no sends
        }
        g.loop_region = LoopRegion::full();
        g
    }

    #[test]
    fn step_manual_follows_a_single_send() {
        let mut g = blank_grid();
        g.cell_mut(5, 5).set_send(Direction::E, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1);
        assert!(next.is_lit(5, 6));
    }

    #[test]
    fn step_manual_splits_the_wavefront() {
        let mut g = blank_grid();
        g.cell_mut(5, 5).set_send(Direction::E, true);
        g.cell_mut(5, 5).set_send(Direction::S, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 2);
        assert!(next.is_lit(5, 6));
        assert!(next.is_lit(6, 5));
    }

    #[test]
    fn step_manual_merges_two_cells_onto_one() {
        let mut g = blank_grid();
        // (5,5) sends E and (5,7) sends W — both land on (5,6).
        g.cell_mut(5, 5).set_send(Direction::E, true);
        g.cell_mut(5, 7).set_send(Direction::W, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        wf.set(5, 7, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1, "merge collapses onto one cell");
        assert!(next.is_lit(5, 6));
    }

    #[test]
    fn step_manual_with_no_sends_yields_empty() {
        let g = blank_grid(); // no cell sends anywhere
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        assert!(step_manual(&g, &wf).is_empty());
    }

    #[test]
    fn step_manual_on_empty_input_is_empty() {
        let g = Grid::default_routing();
        assert!(step_manual(&g, &Wavefront::empty()).is_empty());
    }

    #[test]
    fn step_manual_loops_within_a_one_cell_region() {
        // A 1×1 loop region: a cell sending E wraps onto itself.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 4,
            col0: 9,
            col1: 9,
        };
        g.cell_mut(4, 9).set_send(Direction::E, true);
        let mut wf = Wavefront::empty();
        wf.set(4, 9, true);
        let next = step_manual(&g, &wf);
        assert!(next.is_lit(4, 9), "1×1 region send loops back onto itself");
        assert_eq!(next.count(), 1);
    }

    #[test]
    fn step_manual_1x1_region_collapses_every_send_onto_self() {
        // In a 1×1 region a cell sending in all 8 directions still lights only
        // itself — every send wraps back onto the one cell.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 8,
            row1: 8,
            col0: 2,
            col1: 2,
        };
        for dir in Direction::ALL {
            g.cell_mut(8, 2).set_send(dir, true);
        }
        let mut wf = Wavefront::empty();
        wf.set(8, 2, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1);
        assert!(next.is_lit(8, 2));
    }
}
