//! The Multosis grid: cells, loop region, and grid-level operations.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.

use serde::{Deserialize, Serialize};

/// Number of tracks (rows) in the grid.
pub const ROWS: usize = 16;
/// Number of steps (columns) in the grid.
pub const COLS: usize = 32;
/// Total cell count, `ROWS * COLS`.
pub const CELL_COUNT: usize = ROWS * COLS;

/// One grid cell — a step in the sequencer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Cell {
    /// When false, the playhead landing on this cell produces silence.
    pub enabled: bool,
}

impl Default for Cell {
    /// An enabled cell.
    fn default() -> Self {
        Self { enabled: true }
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

/// The full 16×32 step-sequencer grid plus its loop region. `Copy` (~1.5 KB) so it
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

impl Default for Grid {
    /// The default grid: every cell enabled, loop region the full grid.
    fn default() -> Self {
        Grid {
            cells: [Cell::default(); CELL_COUNT],
            loop_region: LoopRegion::full(),
        }
    }
}

impl Grid {
    /// Reset every cell to the default — enabled. Backs the Reinit Cells
    /// button. Leaves the loop region untouched.
    pub fn reinit(&mut self) {
        for cell in self.cells.iter_mut() {
            cell.enabled = true;
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
    fn cell_default_is_enabled() {
        assert!(Cell::default().enabled);
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
    fn default_grid_loop_region_is_full() {
        assert_eq!(Grid::default().loop_region, LoopRegion::full());
    }

    #[test]
    fn cell_mut_writes_through() {
        let mut g = Grid::default();
        g.cell_mut(3, 7).enabled = false;
        assert!(!g.cell(3, 7).enabled);
        assert!(g.cell(3, 8).enabled);
    }

    #[test]
    fn grid_json_round_trips() {
        let mut g = Grid::default();
        g.cell_mut(3, 9).enabled = false;
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
        let mut g = Grid::default();
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

    #[test]
    fn reinit_enables_every_cell() {
        let mut g = Grid::default();
        g.cell_mut(4, 4).enabled = false;
        g.cell_mut(9, 1).enabled = false;
        let lr = g.loop_region;
        g.reinit();
        for r in 0..ROWS {
            for c in 0..COLS {
                assert!(g.cell(r, c).enabled);
            }
        }
        assert_eq!(g.loop_region, lr);
    }
}
