//! The audio engine: drives the Milestone 1a `Propagator` + `StepClock`,
//! applies each lit row's per-track effect to the dry input, and mixes.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.

use crate::clock::StepClock;
use crate::effects::{Effect, EffectInstance, TrackEffect};
use crate::grid::{Grid, COLS, ROWS};
use crate::modulation::{Modulation, TrackModulation};
use crate::propagation::{Propagator, Wavefront};

/// Upper bound on step boundaries handled within one process block. At any
/// realistic tempo/speed/sample-rate, `samples_per_step` is at least a few
/// hundred samples, so a process block crosses at most a handful of
/// boundaries; 64 is a generous ceiling. Extra boundaries in a pathologically
/// large block are simply dropped (graceful, never panics).
const MAX_BOUNDARIES: usize = 64;

/// Ties the propagation engine, step clock, and per-track effects into a
/// stereo block processor.
pub struct AudioEngine {
    propagator: Propagator,
    clock: StepClock,
    effects: [EffectInstance; ROWS],
    /// Per-track effect configuration — kept so the modulation engine can read
    /// each effect's base parameter values when computing assignable targets.
    track_effects: [TrackEffect; ROWS],
    /// The 3-MSEG-per-track modulation engine driving the effects.
    modulation: Modulation,
    /// The most recent process block's active-row bitmask (bit `r` = row `r`
    /// had a lit, enabled cell under the wavefront). Published to the editor.
    last_active: u16,
    sample_rate: f32,
}

impl AudioEngine {
    /// A fresh engine: `Initial` propagator, zeroed clock, default per-track effects.
    pub fn new() -> Self {
        Self {
            propagator: Propagator::new(),
            clock: StepClock::new(),
            effects: std::array::from_fn(|r| {
                let cfg = TrackEffect::default_for_row(r);
                let mut e = EffectInstance::new(cfg.kind);
                for i in 0..e.parameters().len() {
                    e.set_param(i, cfg.params[i]);
                }
                e
            }),
            track_effects: std::array::from_fn(TrackEffect::default_for_row),
            modulation: Modulation::new(),
            last_active: 0,
            sample_rate: 48_000.0,
        }
    }

    /// Test-only: mutable access to a row's live effect instance.
    #[cfg(test)]
    pub fn effects_mut_for_test(&mut self, row: usize) -> &mut EffectInstance {
        &mut self.effects[row]
    }

    /// Bridge `config` into the engine. For each row: if the effect kind is
    /// unchanged, the live instance is kept and only its parameters are
    /// re-applied (DSP state survives — a parameter edit does not click); a
    /// kind change rebuilds that row's instance. `track_effects` is stored
    /// unconditionally so the modulation engine reads fresh base values.
    pub fn set_effects(&mut self, config: &[TrackEffect; ROWS]) {
        for (r, cfg) in config.iter().enumerate() {
            if self.effects[r].kind() != cfg.kind {
                self.effects[r] = EffectInstance::new(cfg.kind);
                self.effects[r].set_sample_rate(self.sample_rate);
            }
            for i in 0..self.effects[r].parameters().len() {
                self.effects[r].set_param(i, cfg.params[i]);
            }
        }
        self.track_effects = *config;
    }

    /// Install the per-track modulation configuration into the modulation
    /// engine.
    pub fn set_modulation(&mut self, config: &[TrackModulation; ROWS]) {
        self.modulation.set_config(config);
    }

    /// Recompute sample-rate-dependent effect coefficients.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        for e in &mut self.effects {
            e.set_sample_rate(sample_rate);
        }
    }

    /// Reset the sequence, clock, and filter state. Called on the transport
    /// stopped→playing edge and on the host's `reset()`.
    pub fn reset(&mut self) {
        self.propagator.reset();
        self.clock.reset();
        for e in &mut self.effects {
            e.reset();
        }
        self.modulation.reset();
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

    /// The last process block's active-row bitmask — bit `r` set when row `r`
    /// had a lit, enabled cell under the wavefront. For the editor's track
    /// listing "currently sounding" indicator.
    pub fn active_mask(&self) -> u16 {
        self.last_active
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
    /// Each active row's wet contribution is scaled by that row's amplitude as
    /// reported by the modulation engine's amplitude MSEG.
    fn process_sample(&mut self, dry_l: f32, dry_r: f32, active: u16) -> (f32, f32) {
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for r in 0..ROWS {
            if active & (1 << r) == 0 {
                continue;
            }
            let (l, r_out) = self.effects[r].process_sample(dry_l, dry_r);
            let amp = self.modulation.amplitude(r);
            wet_l += amp * l;
            wet_r += amp * r_out;
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
        bpm: f64,
        mix: f32,
        auto_restart: bool,
        grid: &Grid,
    ) {
        let n = left.len().min(right.len());

        self.modulation.update_block(
            n,
            bpm,
            self.sample_rate as f64,
            self.last_active,
            &mut self.effects,
            &self.track_effects,
        );

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
                let (wet_l, wet_r) = self.process_sample(dry_l, dry_r, active);
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
        self.last_active = active;
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
    fn active_mask_reports_the_last_blocks_active_rows() {
        // A fresh engine has processed nothing — mask is empty.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        assert_eq!(engine.active_mask(), 0);
        // After a playing block on the default grid, some rows are active.
        let grid = Grid::default_routing();
        let mut left = [0.2_f32; 128];
        let mut right = [0.2_f32; 128];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, true, &grid);
        assert!(
            engine.active_mask() != 0,
            "after arming, at least one row should be active"
        );
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
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 0.0, true, &grid);
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
        engine.process(&mut left, &mut right, false, 10.0, 120.0, 1.0, true, &grid);
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
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, true, &grid);
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
        engine.process(&mut buf, &mut buf2, true, 10.0, 120.0, 1.0, true, &grid);
        engine.reset();
        // After reset, not playing: empty wavefront -> silent at full wet.
        let mut left = [0.4_f32; 64];
        let mut right = [0.4_f32; 64];
        engine.process(&mut left, &mut right, false, 10.0, 120.0, 1.0, true, &grid);
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
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 0.0, true, &grid);
        assert_eq!(
            engine.sequence_state(),
            crate::propagation::SequenceState::Running
        );
    }

    #[test]
    fn engine_runs_per_track_effects() {
        let config: [crate::effects::TrackEffect; 16] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        engine.set_effects(&config);
        let grid = Grid::default_routing();
        let mut left = [0.3_f32; 64];
        let mut right = [0.3_f32; 64];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, true, &grid);
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(
            left.iter().any(|&s| (s - 0.3).abs() > 1e-6),
            "per-track effects should change the signal"
        );
    }

    #[test]
    fn set_effects_preserves_dsp_state_when_kind_is_unchanged() {
        // A lowpass with running state; re-applying the same kind with a new
        // cutoff must not reset the filter (no zeroed history => no transient).
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(|_| crate::effects::TrackEffect {
                kind: crate::effects::EffectKind::Lowpass,
                params: [800.0, 0.2, 0.0, 0.0],
            });
        engine.set_effects(&cfg);
        // Drive row 0's effect so it has non-zero internal state.
        for _ in 0..256 {
            engine.effects_mut_for_test(0).process_sample(1.0, 1.0);
        }
        let before = engine.effects_mut_for_test(0).process_sample(1.0, 1.0).0;
        // Re-apply with only a parameter change, same kind.
        cfg[0].params[0] = 900.0;
        engine.set_effects(&cfg);
        let after = engine.effects_mut_for_test(0).process_sample(1.0, 1.0).0;
        // Continuity: state was preserved, so the two consecutive samples are
        // close — a full rebuild would have snapped `after` toward 0.
        assert!(
            (after - before).abs() < 0.2,
            "kind-unchanged set_effects must keep DSP state: {before} -> {after}"
        );
    }

    #[test]
    fn set_effects_rebuilds_on_a_kind_change() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        cfg[0] = crate::effects::TrackEffect {
            kind: crate::effects::EffectKind::Lowpass,
            params: [2000.0, 0.1, 0.0, 0.0],
        };
        engine.set_effects(&cfg);
        assert_eq!(
            engine.effects_mut_for_test(0).kind(),
            crate::effects::EffectKind::Lowpass
        );
        cfg[0].kind = crate::effects::EffectKind::Bitcrush;
        engine.set_effects(&cfg);
        assert_eq!(
            engine.effects_mut_for_test(0).kind(),
            crate::effects::EffectKind::Bitcrush
        );
    }

    #[test]
    fn engine_applies_modulation() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let effect_cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        engine.set_effects(&effect_cfg);
        let mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        engine.set_modulation(&mod_cfg);
        let grid = Grid::default_routing();
        // Run two well-separated stretches of audio; the default per-track
        // assignable MSEG should make the wet output drift over time.
        let mut a = [0.4_f32; 64];
        let mut b = [0.4_f32; 64];
        let mut a_r = a;
        engine.process(&mut a, &mut a_r, true, 10.0, 120.0, 1.0, true, &grid);
        for _ in 0..300 {
            let mut l = [0.4_f32; 64];
            let mut r = [0.4_f32; 64];
            engine.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, true, &grid);
        }
        let mut b_r = b;
        engine.process(&mut b, &mut b_r, true, 10.0, 120.0, 1.0, true, &grid);
        assert!(a.iter().all(|s| s.is_finite()) && b.iter().all(|s| s.is_finite()));
        assert!(a != b, "modulation should make the output drift over time");
    }
}
