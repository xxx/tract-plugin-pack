//! The Multosis grid: cells, routing geometry, and grid-level operations.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.

use serde::{Deserialize, Serialize};

/// Number of tracks (rows) in the grid.
pub const ROWS: usize = 16;
/// Number of steps (columns) in the grid.
pub const COLS: usize = 32;
/// Total cell count, `ROWS * COLS`.
pub const CELL_COUNT: usize = ROWS * COLS;

/// One of the 8 directions a cell can send a trigger.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum Direction {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

impl Direction {
    /// All 8 directions, in `bit()` order.
    pub const ALL: [Direction; 8] = [
        Direction::N,
        Direction::NE,
        Direction::E,
        Direction::SE,
        Direction::S,
        Direction::SW,
        Direction::W,
        Direction::NW,
    ];

    /// The `(drow, dcol)` step this direction takes. Row increases downward,
    /// column increases rightward.
    pub fn delta(self) -> (i32, i32) {
        match self {
            Direction::N => (-1, 0),
            Direction::NE => (-1, 1),
            Direction::E => (0, 1),
            Direction::SE => (1, 1),
            Direction::S => (1, 0),
            Direction::SW => (1, -1),
            Direction::W => (0, -1),
            Direction::NW => (-1, -1),
        }
    }

    /// This direction's bit index (0..8) within a `Cell::sends` mask.
    pub fn bit(self) -> u8 {
        match self {
            Direction::N => 0,
            Direction::NE => 1,
            Direction::E => 2,
            Direction::SE => 3,
            Direction::S => 4,
            Direction::SW => 5,
            Direction::W => 6,
            Direction::NW => 7,
        }
    }
}

/// One grid cell. `sends` is a bitmask over `Direction::bit()` positions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Cell {
    /// When false, a lit cell produces silence but still routes.
    pub enabled: bool,
    /// When true, this cell is armed by the `Initial` state.
    pub is_start: bool,
    /// Bitmask of the 8 send directions.
    pub sends: u8,
}

impl Default for Cell {
    /// An enabled, non-start cell with no sends.
    fn default() -> Self {
        Self {
            enabled: true,
            is_start: false,
            sends: 0,
        }
    }
}

impl Cell {
    /// Does this cell send in `dir`?
    pub fn sends_to(self, dir: Direction) -> bool {
        self.sends & (1u8 << dir.bit()) != 0
    }

    /// Turn the send in `dir` on or off.
    pub fn set_send(&mut self, dir: Direction, on: bool) {
        let bit = 1u8 << dir.bit();
        if on {
            self.sends |= bit;
        } else {
            self.sends &= !bit;
        }
    }

    /// Flip the send in `dir`.
    pub fn toggle_send(&mut self, dir: Direction) {
        self.sends ^= 1u8 << dir.bit();
    }

    /// True when the cell sends in at least one direction.
    pub fn has_send(self) -> bool {
        self.sends != 0
    }
}

/// A rectangular sub-region of the grid, inclusive on all four bounds.
/// Invariant: `row0 <= row1 < ROWS` and `col0 <= col1 < COLS`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LoopRegion {
    pub row0: usize,
    pub row1: usize,
    pub col0: usize,
    pub col1: usize,
}

impl LoopRegion {
    /// The region covering the entire grid.
    pub fn full() -> Self {
        Self {
            row0: 0,
            row1: ROWS - 1,
            col0: 0,
            col1: COLS - 1,
        }
    }

    /// Is `(row, col)` inside the region?
    pub fn contains(self, row: usize, col: usize) -> bool {
        row >= self.row0 && row <= self.row1 && col >= self.col0 && col <= self.col1
    }

    /// A copy with bounds clamped into the grid and ordered (`row0 <= row1`,
    /// `col0 <= col1`). Repairs a hand-edited or corrupt deserialized region.
    pub fn normalized(self) -> Self {
        let mut r0 = self.row0.min(ROWS - 1);
        let mut r1 = self.row1.min(ROWS - 1);
        let mut c0 = self.col0.min(COLS - 1);
        let mut c1 = self.col1.min(COLS - 1);
        if r0 > r1 {
            std::mem::swap(&mut r0, &mut r1);
        }
        if c0 > c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
        Self {
            row0: r0,
            row1: r1,
            col0: c0,
            col1: c1,
        }
    }
}

impl Default for LoopRegion {
    fn default() -> Self {
        Self::full()
    }
}

/// The full 16×32 routing grid plus its loop region. `Copy` (~1.5 KB) so it
/// crosses the GUI/audio boundary cheaply — the same fixed-capacity approach
/// as the MSEG widget's `MsegData`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Grid {
    /// Row-major cells, indexed `row * COLS + col`.
    pub cells: [Cell; CELL_COUNT],
    pub loop_region: LoopRegion,
}

impl Grid {
    /// The flat index of `(row, col)`.
    #[inline]
    pub fn index(row: usize, col: usize) -> usize {
        row * COLS + col
    }

    /// Shared access to the cell at `(row, col)`.
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[Self::index(row, col)]
    }

    /// Mutable access to the cell at `(row, col)`.
    pub fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.cells[Self::index(row, col)]
    }

    /// The default grid: every cell enabled and sending only East; the left
    /// column (`col == 0`) all start cells; loop region the full grid.
    pub fn default_routing() -> Self {
        let mut g = Grid {
            cells: [Cell::default(); CELL_COUNT],
            loop_region: LoopRegion::full(),
        };
        for r in 0..ROWS {
            for c in 0..COLS {
                let cell = g.cell_mut(r, c);
                cell.enabled = true;
                cell.is_start = c == 0;
                cell.sends = 0;
                cell.set_send(Direction::E, true);
            }
        }
        g
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::default_routing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_all_lists_eight_in_bit_order() {
        assert_eq!(Direction::ALL.len(), 8);
        for (i, dir) in Direction::ALL.iter().enumerate() {
            assert_eq!(dir.bit() as usize, i, "ALL not in bit order");
        }
    }

    #[test]
    fn direction_deltas_point_the_right_way() {
        // Row increases downward, column increases rightward.
        assert_eq!(Direction::N.delta(), (-1, 0));
        assert_eq!(Direction::E.delta(), (0, 1));
        assert_eq!(Direction::S.delta(), (1, 0));
        assert_eq!(Direction::W.delta(), (0, -1));
        assert_eq!(Direction::NE.delta(), (-1, 1));
        assert_eq!(Direction::SW.delta(), (1, -1));
    }

    #[test]
    fn cell_default_is_enabled_with_no_sends() {
        let c = Cell::default();
        assert!(c.enabled);
        assert!(!c.is_start);
        assert_eq!(c.sends, 0);
        assert!(!c.has_send());
    }

    #[test]
    fn cell_send_toggles_one_direction_at_a_time() {
        let mut c = Cell::default();
        c.set_send(Direction::E, true);
        assert!(c.sends_to(Direction::E));
        assert!(!c.sends_to(Direction::W));
        assert!(c.has_send());

        c.set_send(Direction::S, true);
        assert!(c.sends_to(Direction::E));
        assert!(c.sends_to(Direction::S));

        c.set_send(Direction::E, false);
        assert!(!c.sends_to(Direction::E));
        assert!(c.sends_to(Direction::S));

        c.toggle_send(Direction::S);
        assert!(!c.sends_to(Direction::S));
        assert!(!c.has_send());
    }

    #[test]
    fn grid_dimensions_are_16_by_32() {
        assert_eq!(ROWS, 16);
        assert_eq!(COLS, 32);
        assert_eq!(CELL_COUNT, 512);
    }

    #[test]
    fn loop_region_full_covers_the_whole_grid() {
        let r = LoopRegion::full();
        assert_eq!(r.row0, 0);
        assert_eq!(r.row1, ROWS - 1);
        assert_eq!(r.col0, 0);
        assert_eq!(r.col1, COLS - 1);
        assert!(r.contains(0, 0));
        assert!(r.contains(ROWS - 1, COLS - 1));
        assert_eq!(LoopRegion::default(), LoopRegion::full());
    }

    #[test]
    fn loop_region_contains_respects_bounds() {
        let r = LoopRegion {
            row0: 2,
            row1: 5,
            col0: 8,
            col1: 12,
        };
        assert!(r.contains(2, 8));
        assert!(r.contains(5, 12));
        assert!(r.contains(3, 10));
        assert!(!r.contains(1, 10)); // row above
        assert!(!r.contains(6, 10)); // row below
        assert!(!r.contains(3, 7)); // col left
        assert!(!r.contains(3, 13)); // col right
    }

    #[test]
    fn loop_region_normalized_clamps_and_orders() {
        // Out-of-range and inverted bounds get repaired.
        let bad = LoopRegion {
            row0: 9,
            row1: 4,
            col0: 99,
            col1: 1,
        };
        let n = bad.normalized();
        assert!(n.row0 <= n.row1);
        assert!(n.col0 <= n.col1);
        assert!(n.row1 < ROWS);
        assert!(n.col1 < COLS);
    }

    #[test]
    fn grid_index_is_row_major() {
        assert_eq!(Grid::index(0, 0), 0);
        assert_eq!(Grid::index(0, 31), 31);
        assert_eq!(Grid::index(1, 0), 32);
        assert_eq!(Grid::index(15, 31), 511);
    }

    #[test]
    fn default_routing_sends_east_everywhere() {
        let g = Grid::default_routing();
        for r in 0..ROWS {
            for c in 0..COLS {
                let cell = g.cell(r, c);
                assert!(cell.enabled, "cell ({r},{c}) should be enabled");
                assert!(cell.sends_to(Direction::E), "cell ({r},{c}) should send E");
                // East is the only send.
                assert_eq!(cell.sends, 1u8 << Direction::E.bit());
            }
        }
    }

    #[test]
    fn default_routing_starts_the_left_column() {
        let g = Grid::default_routing();
        for r in 0..ROWS {
            assert!(g.cell(r, 0).is_start, "left-column cell ({r},0) is a start");
            for c in 1..COLS {
                assert!(!g.cell(r, c).is_start, "cell ({r},{c}) is not a start");
            }
        }
    }

    #[test]
    fn default_routing_loop_region_is_full() {
        assert_eq!(Grid::default_routing().loop_region, LoopRegion::full());
        assert_eq!(Grid::default(), Grid::default_routing());
    }

    #[test]
    fn cell_mut_writes_through() {
        let mut g = Grid::default_routing();
        g.cell_mut(3, 7).enabled = false;
        assert!(!g.cell(3, 7).enabled);
        assert!(g.cell(3, 8).enabled);
    }
}
