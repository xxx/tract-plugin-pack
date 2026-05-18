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

    /// The current sequence lifecycle state — exposed for the editor's status
    /// readout.
    pub fn sequence_state(&self) -> crate::propagation::SequenceState {
        self.propagator.state
    }

    /// Steps since the wavefront was last armed — exposed for the editor's
    /// status readout.
    pub fn step(&self) -> u64 {
        self.propagator.step
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

    /// Apply the active rows' effects to one dry stereo sample and sum them.
    /// The sum is deliberately un-normalised — with many rows active the wet
    /// signal can exceed the dry level; the `mix` and output-gain controls
    /// manage that (design doc §6).
    fn process_sample(
        &mut self,
        dry_l: f32,
        dry_r: f32,
        active: u16,
        bank: EffectBank,
    ) -> (f32, f32) {
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for r in 0..ROWS {
            if active & (1 << r) == 0 {
                continue;
            }
            match bank {
                EffectBank::Lowpass => {
                    wet_l += self.lowpass.process(r, 0, dry_l);
                    wet_r += self.lowpass.process(r, 1, dry_r);
                }
                EffectBank::Bitcrush => {
                    wet_l += self.bitcrush.process(r, dry_l);
                    wet_r += self.bitcrush.process(r, dry_r);
                }
            }
        }
        (wet_l, wet_r)
    }

    /// Process one stereo block in place. `left`/`right` carry the dry input
    /// on entry and the mixed `dry + (wet - dry) * mix` output on return.
    ///
    /// While `playing`, the clock advances and the wavefront propagates at
    /// each step boundary; while stopped, the wavefront is frozen and the
    /// block is processed with the current lit set. `samples_per_step` comes
    /// from `clock::samples_per_step`.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playing: bool,
        samples_per_step: f64,
        bank: EffectBank,
        mix: f32,
        auto_restart: bool,
        grid: &Grid,
    ) {
        let n = left.len().min(right.len());

        // Gather this block's step-boundary offsets (only while playing).
        let mut boundaries = [0usize; MAX_BOUNDARIES];
        let mut n_boundaries = 0usize;
        if playing {
            self.clock.advance(n, samples_per_step, |offset| {
                if n_boundaries < MAX_BOUNDARIES {
                    boundaries[n_boundaries] = offset;
                    n_boundaries += 1;
                }
            });
        }

        // Walk the block in segments split at each boundary; the wavefront is
        // constant within a segment.
        let mut active = Self::active_rows(grid, &self.propagator.wavefront);
        let mut cursor = 0usize;
        let mut bi = 0usize;
        while cursor < n {
            let seg_end = if bi < n_boundaries {
                boundaries[bi].clamp(cursor, n)
            } else {
                n
            };
            for i in cursor..seg_end {
                let (dry_l, dry_r) = (left[i], right[i]);
                let (wet_l, wet_r) = self.process_sample(dry_l, dry_r, active, bank);
                left[i] = dry_l + (wet_l - dry_l) * mix;
                right[i] = dry_r + (wet_r - dry_r) * mix;
            }
            cursor = seg_end;
            if bi < n_boundaries {
                self.propagator.tick(grid, auto_restart);
                active = Self::active_rows(grid, &self.propagator.wavefront);
                bi += 1;
            }
        }
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

    #[test]
    fn process_at_mix_zero_is_dry_passthrough() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut left = [0.1_f32; 64];
        let mut right = [-0.2_f32; 64];
        engine.process(
            &mut left,
            &mut right,
            true,
            10.0,
            EffectBank::Lowpass,
            0.0,
            true,
            &grid,
        );
        assert!(left.iter().all(|&s| (s - 0.1).abs() < 1e-6));
        assert!(right.iter().all(|&s| (s + 0.2).abs() < 1e-6));
    }

    #[test]
    fn process_empty_wavefront_full_wet_is_silent() {
        // Not playing, fresh engine: the wavefront is empty, so no row is
        // active and the fully-wet output is silence.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut left = [0.5_f32; 64];
        let mut right = [0.5_f32; 64];
        engine.process(
            &mut left,
            &mut right,
            false,
            10.0,
            EffectBank::Lowpass,
            1.0,
            true,
            &grid,
        );
        assert!(left.iter().all(|&s| s == 0.0));
        assert!(right.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn process_arms_and_produces_wet_signal() {
        // Playing, default grid, a short step: once the clock crosses its
        // first boundary the start cells arm, rows become active, and the
        // fully-wet output is no longer silent.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut left = [0.3_f32; 128];
        let mut right = [0.3_f32; 128];
        engine.process(
            &mut left,
            &mut right,
            true,
            10.0,
            EffectBank::Lowpass,
            1.0,
            true,
            &grid,
        );
        assert!(
            left[127] != 0.0,
            "after arming, a fully-wet block should not be silent"
        );
    }

    #[test]
    fn process_reset_returns_to_silence() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut buf = [0.3_f32; 128];
        let mut buf2 = [0.3_f32; 128];
        engine.process(
            &mut buf,
            &mut buf2,
            true,
            10.0,
            EffectBank::Lowpass,
            1.0,
            true,
            &grid,
        );
        engine.reset();
        // After reset, not playing: empty wavefront -> silent at full wet.
        let mut left = [0.4_f32; 64];
        let mut right = [0.4_f32; 64];
        engine.process(
            &mut left,
            &mut right,
            false,
            10.0,
            EffectBank::Lowpass,
            1.0,
            true,
            &grid,
        );
        assert!(left.iter().all(|&s| s == 0.0));
        assert!(right.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn new_engine_reports_initial_state_and_zero_step() {
        let engine = AudioEngine::new();
        assert_eq!(
            engine.sequence_state(),
            crate::propagation::SequenceState::Initial
        );
        assert_eq!(engine.step(), 0);
    }

    #[test]
    fn engine_reports_running_after_arming() {
        let grid = Grid::default_routing(); // left column = start cells
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut left = [0.0_f32; 64];
        let mut right = [0.0_f32; 64];
        // One short step arms the start cells -> Running.
        engine.process(
            &mut left,
            &mut right,
            true,
            10.0,
            EffectBank::Lowpass,
            0.0,
            true,
            &grid,
        );
        assert_eq!(
            engine.sequence_state(),
            crate::propagation::SequenceState::Running
        );
    }
}
