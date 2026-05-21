//! The sequencer playhead: one column index scanning the loop zone.
//!
//! See `docs/superpowers/specs/2026-05-20-multosis-grid-simplification-design.md`.

use crate::grid::{LoopRegion, ROWS};

/// The sequencer playhead — a single column that scans the loop zone
/// left-to-right and wraps. `Copy` so it crosses the GUI/audio boundary
/// cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Playhead {
    /// The column the playhead currently occupies.
    column: usize,
    /// False until the first tick after a reset; that first tick lands the
    /// playhead on the loop zone's left edge instead of advancing.
    started: bool,
}

impl Playhead {
    /// A fresh playhead that has not yet started scanning.
    pub fn new() -> Self {
        Self {
            column: 0,
            started: false,
        }
    }

    /// The column the playhead currently occupies.
    pub fn column(&self) -> usize {
        self.column
    }

    /// Whether the playhead has started scanning (false until the first tick
    /// after a reset).
    pub fn started(&self) -> bool {
        self.started
    }

    /// Advance one step. The first tick after a reset snaps to the loop
    /// zone's left edge (`col0`); subsequent ticks move one column right,
    /// wrapping at `col1` back to `col0`. If the loop zone was resized so the
    /// current column now sits outside `[col0, col1]`, snap back to `col0`.
    pub fn tick(&mut self, loop_region: &LoopRegion) {
        let (lo, hi) = (loop_region.col0, loop_region.col1);
        if !self.started {
            self.column = lo;
            self.started = true;
        } else if self.column >= hi || self.column < lo {
            self.column = lo;
        } else {
            self.column += 1;
        }
    }

    /// Return the playhead to its unstarted state — the next tick will land
    /// on the loop zone's left edge. Triggered by the Reset button and the
    /// transport stopped→playing edge.
    pub fn reset(&mut self) {
        self.column = 0;
        self.started = false;
    }
}

impl Default for Playhead {
    fn default() -> Self {
        Self::new()
    }
}

/// Bit `r` set when row `r` is inside the loop zone's row span and the cell
/// at `(r, column)` is enabled. The sequencer's active-row rule.
pub fn active_rows(grid: &crate::grid::Grid, loop_region: &LoopRegion, column: usize) -> u16 {
    let mut mask = 0u16;
    for r in loop_region.row0..=loop_region.row1.min(ROWS - 1) {
        if grid.cell(r, column).enabled {
            mask |= 1 << r;
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::LoopRegion;

    fn region(col0: usize, col1: usize) -> LoopRegion {
        LoopRegion {
            row0: 0,
            row1: ROWS - 1,
            col0,
            col1,
        }
    }

    #[test]
    fn new_playhead_has_not_started() {
        let p = Playhead::new();
        assert_eq!(p.column(), 0);
        assert!(!p.started());
    }

    #[test]
    fn started_flips_on_the_first_tick_and_a_reset_clears_it() {
        let mut p = Playhead::new();
        assert!(!p.started());
        p.tick(&region(2, 9));
        assert!(p.started(), "the first tick starts the playhead");
        p.tick(&region(2, 9));
        assert!(p.started(), "it stays started across ticks");
        p.reset();
        assert!(!p.started(), "reset returns it to unstarted");
    }

    #[test]
    fn first_tick_lands_on_the_loop_zone_left_edge() {
        let mut p = Playhead::new();
        p.tick(&region(5, 12));
        assert_eq!(p.column(), 5);
    }

    #[test]
    fn subsequent_ticks_advance_one_column() {
        let mut p = Playhead::new();
        let lr = region(5, 12);
        p.tick(&lr); // -> 5
        p.tick(&lr); // -> 6
        p.tick(&lr); // -> 7
        assert_eq!(p.column(), 7);
    }

    #[test]
    fn tick_wraps_at_the_right_edge_back_to_the_left() {
        let mut p = Playhead::new();
        let lr = region(5, 7);
        p.tick(&lr); // 5
        p.tick(&lr); // 6
        p.tick(&lr); // 7
        p.tick(&lr); // wraps -> 5
        assert_eq!(p.column(), 5);
    }

    #[test]
    fn tick_snaps_back_when_the_loop_zone_shrinks_away() {
        let mut p = Playhead::new();
        let wide = region(0, 20);
        p.tick(&wide);
        p.tick(&wide);
        p.tick(&wide); // column 2
        let narrow = region(10, 15);
        p.tick(&narrow);
        assert_eq!(p.column(), 10, "out-of-range column snaps to col0");
    }

    #[test]
    fn reset_returns_the_playhead_to_unstarted() {
        let mut p = Playhead::new();
        let lr = region(3, 9);
        p.tick(&lr);
        p.tick(&lr); // column 4
        p.reset();
        p.tick(&lr); // first tick again -> col0
        assert_eq!(p.column(), 3);
    }
}
