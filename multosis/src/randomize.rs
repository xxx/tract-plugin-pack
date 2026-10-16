//! Sequencer cell randomization and shared effect randomization.

use crate::grid::Grid;
pub use tract_effects::randomize::{Rng, randomize_track_effect};

/// Randomize `enabled` for every cell inside the loop region. Deterministic in
/// `seed`. Leaves cells outside the region untouched.
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
        let mut a = Grid::default();
        let mut b = Grid::default();
        randomize_activations(&mut a, 4242);
        randomize_activations(&mut b, 4242);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_activations_differs_by_seed() {
        let mut a = Grid::default();
        let mut b = Grid::default();
        randomize_activations(&mut a, 1);
        randomize_activations(&mut b, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_activations_only_touches_cells_inside_the_region() {
        let mut g = Grid::default();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 6,
            col0: 10,
            col1: 14,
        };
        randomize_activations(&mut g, 99);
        for r in 0..crate::grid::ROWS {
            for c in 0..crate::grid::COLS {
                if !g.loop_region.contains(r, c) {
                    assert!(
                        g.cell(r, c).enabled,
                        "cell ({r},{c}) outside region changed"
                    );
                }
            }
        }
    }
}
