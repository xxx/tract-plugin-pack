//! Deterministic randomization of cell activations and routing.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.3.

use crate::grid::Grid;

/// Deterministic xorshift32 PRNG — no dependency, seeded per call. Matches the
/// MSEG widget's `randomize` PRNG.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // xorshift cannot leave the all-zero state, so map seed 0 to a fixed
        // non-zero constant.
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn bool(&mut self) -> bool {
        self.next_u32() & 1 == 1
    }
}

/// Randomize `enabled` for every cell inside the loop region. Deterministic in
/// `seed`. Leaves `sends`, `is_start`, and cells outside the region untouched.
pub fn randomize_activations(grid: &mut Grid, seed: u32) {
    let mut rng = Rng::new(seed);
    let lr = grid.loop_region;
    for r in lr.row0..=lr.row1 {
        for c in lr.col0..=lr.col1 {
            grid.cell_mut(r, c).enabled = rng.bool();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::LoopRegion;

    #[test]
    fn randomize_activations_is_deterministic() {
        let mut a = Grid::default_routing();
        let mut b = Grid::default_routing();
        randomize_activations(&mut a, 4242);
        randomize_activations(&mut b, 4242);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_activations_differs_by_seed() {
        let mut a = Grid::default_routing();
        let mut b = Grid::default_routing();
        randomize_activations(&mut a, 1);
        randomize_activations(&mut b, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_activations_only_touches_enabled_inside_the_region() {
        let mut g = Grid::default_routing();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 6,
            col0: 10,
            col1: 14,
        };
        randomize_activations(&mut g, 99);
        for r in 0..crate::grid::ROWS {
            for c in 0..crate::grid::COLS {
                let cell = g.cell(r, c);
                // sends and is_start are never altered.
                assert_eq!(cell.sends, 1u8 << crate::grid::Direction::E.bit());
                assert_eq!(cell.is_start, c == 0);
                // Outside the region, enabled is left at its default (true).
                if !g.loop_region.contains(r, c) {
                    assert!(cell.enabled, "cell ({r},{c}) outside region changed");
                }
            }
        }
    }
}
