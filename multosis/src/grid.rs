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

/// `Vec`-backed serialization mirror of `Grid`. serde implements its array
/// traits only for `[T; 0..=32]`, so the 512-cell `[Cell; CELL_COUNT]` field
/// cannot derive serde directly; this mirror crosses that gap — the same
/// approach the MSEG widget's `MsegData` uses for its 128-node array.
#[derive(Serialize, Deserialize)]
struct GridSerde {
    cells: Vec<Cell>,
    loop_region: LoopRegion,
}

impl Serialize for Grid {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        GridSerde {
            cells: self.cells.to_vec(),
            loop_region: self.loop_region,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Grid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = GridSerde::deserialize(d)?;
        if raw.cells.len() != CELL_COUNT {
            return Err(serde::de::Error::custom(format!(
                "Grid needs {CELL_COUNT} cells, got {}",
                raw.cells.len()
            )));
        }
        let mut cells = [Cell::default(); CELL_COUNT];
        cells.copy_from_slice(&raw.cells);
        Ok(Grid {
            cells,
            loop_region: raw.loop_region,
        })
    }
}

/// Wrap `v` into the inclusive range `[lo, hi]`.
fn wrap(v: i32, lo: usize, hi: usize) -> usize {
    let lo = lo as i32;
    let span = hi as i32 - lo + 1;
    (lo + (v - lo).rem_euclid(span)) as usize
}

impl Grid {
    /// Where a send in `dir` from `(row, col)` lands. If `(row, col)` is inside
    /// the loop region the result wraps within the region; otherwise it wraps
    /// within the full grid. This is the only place loop-region containment is
    /// enforced.
    pub fn next_cell(&self, row: usize, col: usize, dir: Direction) -> (usize, usize) {
        let (dr, dc) = dir.delta();
        let nr = row as i32 + dr;
        let nc = col as i32 + dc;
        if self.loop_region.contains(row, col) {
            let lr = self.loop_region;
            (wrap(nr, lr.row0, lr.row1), wrap(nc, lr.col0, lr.col1))
        } else {
            (wrap(nr, 0, ROWS - 1), wrap(nc, 0, COLS - 1))
        }
    }
}

impl Grid {
    /// Restore default sends (East only) on every cell; leave `enabled` and
    /// `is_start` untouched. Recovers from user-created dead ends.
    pub fn reset_routing(&mut self) {
        for cell in self.cells.iter_mut() {
            cell.sends = 0;
            cell.set_send(Direction::E, true);
        }
    }

    /// Restore default activations: every cell `enabled`, the left column the
    /// start cells. Leaves `sends` untouched.
    pub fn reinit_activations(&mut self) {
        for r in 0..ROWS {
            for c in 0..COLS {
                let cell = self.cell_mut(r, c);
                cell.enabled = true;
                cell.is_start = c == 0;
            }
        }
    }

    /// Repair structural invariants after loading a possibly hand-edited or
    /// corrupt grid: clamp and order the loop region. Called by the plugin
    /// after deserializing persisted state (Milestone 1b).
    pub fn sanitize(&mut self) {
        self.loop_region = self.loop_region.normalized();
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

    #[test]
    fn next_cell_moves_inside_the_grid() {
        let g = Grid::default_routing(); // loop region is full
        assert_eq!(g.next_cell(5, 5, Direction::E), (5, 6));
        assert_eq!(g.next_cell(5, 5, Direction::W), (5, 4));
        assert_eq!(g.next_cell(5, 5, Direction::N), (4, 5));
        assert_eq!(g.next_cell(5, 5, Direction::S), (6, 5));
        assert_eq!(g.next_cell(5, 5, Direction::SE), (6, 6));
    }

    #[test]
    fn next_cell_wraps_every_grid_edge() {
        let g = Grid::default_routing();
        assert_eq!(g.next_cell(5, COLS - 1, Direction::E), (5, 0));
        assert_eq!(g.next_cell(5, 0, Direction::W), (5, COLS - 1));
        assert_eq!(g.next_cell(0, 5, Direction::N), (ROWS - 1, 5));
        assert_eq!(g.next_cell(ROWS - 1, 5, Direction::S), (0, 5));
        // Corner diagonal wraps both axes at once.
        assert_eq!(g.next_cell(0, 0, Direction::NW), (ROWS - 1, COLS - 1));
        assert_eq!(g.next_cell(ROWS - 1, COLS - 1, Direction::SE), (0, 0));
    }

    #[test]
    fn next_cell_wraps_within_the_loop_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 11,
            col0: 8,
            col1: 23,
        };
        // A cell on the region's right edge sending E wraps to the region's
        // left edge — NOT to the grid edge.
        assert_eq!(g.next_cell(6, 23, Direction::E), (6, 8));
        // Region's bottom edge sending S wraps to the region's top edge.
        assert_eq!(g.next_cell(11, 15, Direction::S), (4, 15));
        // Region's top-left corner sending NW wraps to its bottom-right corner.
        assert_eq!(g.next_cell(4, 8, Direction::NW), (11, 23));
        // Interior of the region moves normally.
        assert_eq!(g.next_cell(6, 15, Direction::E), (6, 16));
    }

    #[test]
    fn next_cell_outside_the_region_wraps_the_full_grid() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 11,
            col0: 8,
            col1: 23,
        };
        // (2, 31) is outside the region: E wraps to the grid's left edge.
        assert_eq!(g.next_cell(2, COLS - 1, Direction::E), (2, 0));
    }

    #[test]
    fn no_send_from_a_region_cell_ever_escapes_the_region() {
        let mut g = Grid::default_routing();
        let lr = LoopRegion {
            row0: 3,
            row1: 9,
            col0: 5,
            col1: 20,
        };
        g.loop_region = lr;
        for r in lr.row0..=lr.row1 {
            for c in lr.col0..=lr.col1 {
                for dir in Direction::ALL {
                    let (nr, nc) = g.next_cell(r, c, dir);
                    assert!(
                        lr.contains(nr, nc),
                        "send {dir:?} from ({r},{c}) escaped to ({nr},{nc})"
                    );
                }
            }
        }
    }

    #[test]
    fn next_cell_in_a_1x1_region_loops_every_direction_onto_itself() {
        // A 1×1 loop region: the single cell has nowhere to wrap to, so every
        // one of the 8 directions must resolve back onto the cell itself.
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 7,
            row1: 7,
            col0: 13,
            col1: 13,
        };
        for dir in Direction::ALL {
            assert_eq!(
                g.next_cell(7, 13, dir),
                (7, 13),
                "direction {dir:?} did not loop onto itself"
            );
        }
    }

    #[test]
    fn reset_routing_restores_east_and_keeps_activations() {
        let mut g = Grid::default_routing();
        // Scramble routing and activations.
        g.cell_mut(2, 2).sends = 0b1010_1010;
        g.cell_mut(2, 2).enabled = false;
        g.cell_mut(2, 2).is_start = true;
        g.cell_mut(7, 9).sends = 0;

        g.reset_routing();

        // Every cell sends East only again.
        for r in 0..ROWS {
            for c in 0..COLS {
                assert_eq!(g.cell(r, c).sends, 1u8 << Direction::E.bit());
            }
        }
        // Activations are untouched.
        assert!(!g.cell(2, 2).enabled);
        assert!(g.cell(2, 2).is_start);
    }

    #[test]
    fn reinit_activations_restores_defaults_and_keeps_routing() {
        let mut g = Grid::default_routing();
        g.cell_mut(4, 4).sends = 0b0001_1000;
        g.cell_mut(4, 4).enabled = false;
        g.cell_mut(0, 5).is_start = true; // a stray start away from col 0
        g.cell_mut(3, 0).is_start = false; // clear a default start

        g.reinit_activations();

        // Activations back to default: all enabled, left column the starts.
        for r in 0..ROWS {
            for c in 0..COLS {
                assert!(g.cell(r, c).enabled);
                assert_eq!(g.cell(r, c).is_start, c == 0);
            }
        }
        // Routing is untouched.
        assert_eq!(g.cell(4, 4).sends, 0b0001_1000);
    }

    #[test]
    fn grid_json_round_trips() {
        let mut g = Grid::default_routing();
        g.cell_mut(3, 9).sends = 0b1100_0011;
        g.cell_mut(3, 9).enabled = false;
        g.cell_mut(10, 0).is_start = false;
        g.loop_region = LoopRegion {
            row0: 2,
            row1: 12,
            col0: 4,
            col1: 28,
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: Grid = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn grid_json_rejects_wrong_cell_count() {
        // A blob with the wrong number of cells must be rejected, not
        // silently accepted with a truncated or padded grid.
        let json = r#"{"cells":[],"loop_region":{"row0":0,"row1":15,"col0":0,"col1":31}}"#;
        let result: Result<Grid, _> = serde_json::from_str(json);
        assert!(result.is_err(), "empty cell list must be rejected");
    }

    #[test]
    fn sanitize_repairs_a_bad_loop_region() {
        let mut g = Grid::default_routing();
        // Inverted, out-of-range bounds — as a corrupt blob might carry.
        g.loop_region = LoopRegion {
            row0: 14,
            row1: 3,
            col0: 99,
            col1: 0,
        };
        g.sanitize();
        let lr = g.loop_region;
        assert!(lr.row0 <= lr.row1 && lr.row1 < ROWS);
        assert!(lr.col0 <= lr.col1 && lr.col1 < COLS);
    }
}
