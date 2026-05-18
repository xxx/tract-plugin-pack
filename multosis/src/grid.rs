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
}
