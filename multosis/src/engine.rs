//! The audio engine: drives the Milestone 1a `Propagator` + `StepClock`,
//! applies the lit rows' throwaway effects to the dry input, and mixes.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.

use crate::clock::StepClock;
use crate::effects::{BitcrushBank, EffectBank, LowpassBank};
use crate::grid::{Grid, COLS, ROWS};
use crate::propagation::{Propagator, Wavefront};

/// Upper bound on step boundaries handled within one process block. At any
/// realistic tempo/speed/sample-rate, `samples_per_step` is at least a few
/// hundred samples, so a process block crosses at most a handful of
/// boundaries; 64 is a generous ceiling. Extra boundaries in a pathologically
/// large block are simply dropped (graceful, never panics).
const MAX_BOUNDARIES: usize = 64;

/// Ties the propagation engine, step clock, and throwaway effects into a
/// stereo block processor.
pub struct AudioEngine {
    propagator: Propagator,
    clock: StepClock,
    lowpass: LowpassBank,
    bitcrush: BitcrushBank,
}

impl AudioEngine {
    /// A fresh engine: `Initial` propagator, zeroed clock, default effects.
    pub fn new() -> Self {
        Self {
            propagator: Propagator::new(),
            clock: StepClock::new(),
            lowpass: LowpassBank::new(),
            bitcrush: BitcrushBank::new(),
        }
    }

    /// Recompute sample-rate-dependent effect coefficients.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.lowpass.set_sample_rate(sample_rate);
    }

    /// Reset the sequence, clock, and filter state. Called on the transport
    /// stopped→playing edge and on the host's `reset()`.
    pub fn reset(&mut self) {
        self.propagator.reset();
        self.clock.reset();
        self.lowpass.reset();
    }

    /// The current wavefront — exposed so the Milestone 1b-ii editor can draw
    /// it.
    pub fn wavefront(&self) -> &Wavefront {
        &self.propagator.wavefront
    }

    /// Bitmask (bit `R`) of rows holding at least one cell that is both lit in
    /// `wf` and `enabled` in `grid`. A disabled lit cell contributes nothing;
    /// two lit cells in one row collapse to a single bit.
    fn active_rows(grid: &Grid, wf: &Wavefront) -> u16 {
        let mut mask = 0u16;
        for r in 0..ROWS {
            for c in 0..COLS {
                if wf.is_lit(r, c) && grid.cell(r, c).enabled {
                    mask |= 1 << r;
                    break;
                }
            }
        }
        mask
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_rows_marks_lit_enabled_cells() {
        let grid = Grid::default_routing();
        let mut wf = Wavefront::empty();
        wf.set(3, 0, true);
        wf.set(9, 5, true);
        let mask = AudioEngine::active_rows(&grid, &wf);
        assert_eq!(mask, (1 << 3) | (1 << 9));
    }

    #[test]
    fn active_rows_ignores_disabled_cells() {
        let mut grid = Grid::default_routing();
        grid.cell_mut(7, 2).enabled = false;
        let mut wf = Wavefront::empty();
        wf.set(7, 2, true); // lit but disabled -> not active
        assert_eq!(AudioEngine::active_rows(&grid, &wf), 0);
    }

    #[test]
    fn active_rows_dedupes_a_row_with_two_lit_cells() {
        let grid = Grid::default_routing();
        let mut wf = Wavefront::empty();
        wf.set(4, 1, true);
        wf.set(4, 30, true); // same row, twice -> one bit
        assert_eq!(AudioEngine::active_rows(&grid, &wf), 1 << 4);
    }

    #[test]
    fn new_engine_has_an_empty_wavefront() {
        let engine = AudioEngine::new();
        assert!(engine.wavefront().is_empty());
    }
}
