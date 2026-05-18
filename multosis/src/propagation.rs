//! The wavefront propagation engine: the lit-set, the per-step routing rule,
//! and the Initial/Running/Stopped lifecycle.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §5.

use crate::grid::{Grid, CELL_COUNT};

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
}
