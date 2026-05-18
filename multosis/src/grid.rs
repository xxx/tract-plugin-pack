//! The Multosis grid: cells, routing geometry, and grid-level operations.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.

use serde::{Deserialize, Serialize};

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
}
