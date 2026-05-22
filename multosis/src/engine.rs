//! The audio engine: drives the Milestone 1a `Playhead` + `StepClock`,
//! applies each active row's per-track effect to the dry input, and mixes.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.

use crate::clock::StepClock;
use crate::compressor::Compressor;
use crate::effects::{Effect, EffectInstance, TrackEffect};
use crate::grid::{Grid, ROWS};
use crate::modulation::{Modulation, TrackModulation};
use crate::propagation::{active_rows, Playhead};

/// Upper bound on step boundaries handled within one process block. At any
/// realistic tempo/speed/sample-rate, `samples_per_step` is at least a few
/// hundred samples, so a process block crosses at most a handful of
/// boundaries; 64 is a generous ceiling. Extra boundaries in a pathologically
/// large block are simply dropped (graceful, never panics).
const MAX_BOUNDARIES: usize = 64;

/// Ties the playhead, step clock, and per-track effects into a
/// stereo block processor.
pub struct AudioEngine {
    playhead: Playhead,
    /// Steps since the last reset — for the editor's status readout.
    step: u64,
    clock: StepClock,
    effects: [EffectInstance; ROWS],
    /// Per-track effect configuration — kept so the modulation engine can read
    /// each effect's base parameter values when computing assignable targets.
    track_effects: [TrackEffect; ROWS],
    /// The 3-MSEG-per-track modulation engine driving the effects.
    modulation: Modulation,
    /// The most recent process block's active-row bitmask (bit `r` = row `r`
    /// had an enabled cell at the playhead column). Published to the editor.
    last_active: u16,
    /// Wet-bus compressor (soft-knee peak) — tames the +N×dry peak that the
    /// per-row effect sum produces when many rows are active simultaneously.
    /// Inserted between the per-sample wet sum and the dry/wet mix.
    compressor: Compressor,
    sample_rate: f32,
}

impl AudioEngine {
    /// A fresh engine: unstarted playhead, zeroed clock, default per-track effects.
    pub fn new() -> Self {
        Self {
            playhead: Playhead::new(),
            step: 0,
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
            compressor: Compressor::new(),
            sample_rate: 48_000.0,
        }
    }

    /// Test-only: mutable access to a row's live effect instance.
    #[cfg(test)]
    pub fn effects_mut_for_test(&mut self, row: usize) -> &mut EffectInstance {
        &mut self.effects[row]
    }

    /// Test-only: the per-row mask of modulation triggers that fired in the
    /// most recent process block.
    #[cfg(test)]
    pub fn modulation_fires_for_test(&self) -> u16 {
        self.modulation.fires_last_block()
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
        self.compressor.set_sample_rate(sample_rate);
    }

    /// Update the wet-bus compressor's user-facing parameters. `threshold_db`
    /// is in dBFS (negative); `ratio` is ≥ 1.0.
    pub fn set_compressor(&mut self, threshold_db: f32, ratio: f32) {
        self.compressor.set_params(threshold_db, ratio);
    }

    /// Reset the sequence, clock, and filter state. Called on the transport
    /// stopped→playing edge and on the host's `reset()`.
    pub fn reset(&mut self) {
        self.playhead.reset();
        self.step = 0;
        self.clock.reset();
        for e in &mut self.effects {
            e.reset();
        }
        self.modulation.reset();
        self.compressor.reset();
        self.last_active = 0;
    }

    /// Steps since the last reset — exposed for the editor's status readout.
    pub fn step(&self) -> u64 {
        self.step
    }

    /// The playhead's current column — exposed for the editor's column
    /// highlight overlay.
    pub fn playhead_column(&self) -> usize {
        self.playhead.column()
    }

    /// The last process block's active-row bitmask — bit `r` set when row `r`
    /// had an enabled cell at the playhead column. For the editor's track
    /// listing "currently sounding" indicator.
    pub fn active_mask(&self) -> u16 {
        self.last_active
    }

    /// The current free-running phase of MSEG `k` on `row` (0..1). For the
    /// editor's MSEG playhead overlay.
    pub fn modulation_phase(&self, row: usize, k: usize) -> f32 {
        self.modulation.phase(row, k)
    }

    /// Apply the active rows' effects to one dry stereo sample and sum them.
    /// Each active row's effect output is first blended with the dry input by
    /// that row's per-track `mix` (`lane = dry + (effect − dry)·mix`), then
    /// scaled by the row's amplitude MSEG. The sum is deliberately
    /// un-normalised — the wet-bus compressor and the global mix manage the
    /// parallel-row peak.
    fn process_sample(&mut self, dry_l: f32, dry_r: f32, active: u16) -> (f32, f32) {
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for r in 0..ROWS {
            if active & (1 << r) == 0 {
                continue;
            }
            let (eff_l, eff_r) = self.effects[r].process_sample(dry_l, dry_r);
            // Per-track dry/wet blend, before the amplitude MSEG.
            let mix = self.track_effects[r].mix;
            let lane_l = dry_l + (eff_l - dry_l) * mix;
            let lane_r = dry_r + (eff_r - dry_r) * mix;
            let amp = self.modulation.amplitude(r);
            wet_l += amp * lane_l;
            wet_r += amp * lane_r;
        }
        (wet_l, wet_r)
    }

    /// Process one stereo block in place. `left`/`right` carry the dry input
    /// on entry and the mixed `dry + (wet - dry) * mix` output on return.
    ///
    /// While `playing`, the clock advances and the playhead advances at each
    /// step boundary; while stopped, the playhead is frozen and the block is
    /// processed at the current column. `samples_per_step` comes from
    /// `clock::samples_per_step`.
    ///
    /// The modulation is driven from the per-segment loop: `begin_block` once,
    /// `advance_segment` per segment, and `fire` at each step boundary — so a
    /// `CellLight` edge resets the row at the exact boundary sample.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playing: bool,
        samples_per_step: f64,
        bpm: f64,
        mix: f32,
        grid: &Grid,
    ) {
        let n = left.len().min(right.len());
        let sr = self.sample_rate as f64;

        // Block-rate modulation setup: FreeHz fires, and zero the fire mask.
        self.modulation
            .begin_block(n, sr, &mut self.effects, &self.track_effects);

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

        // Walk the block in segments split at each boundary. Per segment:
        // advance the per-MSEG-clock modulation, then render the audio. At a
        // boundary, fire any newly-lit CellLight rows so the phase reset lands
        // on the very next segment — sample-accurate, no cross-block delay.
        let mut active = active_rows(grid, &grid.loop_region, self.playhead.column());
        let mut cursor = 0usize;
        let mut bi = 0usize;
        while cursor < n {
            let seg_end = if bi < n_boundaries {
                boundaries[bi].clamp(cursor, n)
            } else {
                n
            };
            self.modulation.advance_segment(
                seg_end - cursor,
                bpm,
                sr,
                &mut self.effects,
                &self.track_effects,
            );
            for i in cursor..seg_end {
                let (dry_l, dry_r) = (left[i], right[i]);
                let (wet_l, wet_r) = self.process_sample(dry_l, dry_r, active);
                // Tame the parallel-row sum on the wet bus, then mix dry/wet.
                let (cw_l, cw_r) = self.compressor.process_sample(wet_l, wet_r);
                left[i] = dry_l + (cw_l - dry_l) * mix;
                right[i] = dry_r + (cw_r - dry_r) * mix;
            }
            cursor = seg_end;
            if bi < n_boundaries {
                // Snapshot the active-row mask BEFORE and AFTER the tick;
                // rows that became active fire their CellLight trigger now.
                // Before the first tick the playhead has not started — nothing
                // was playing, so the pre-tick set is empty. Without this gate
                // an unstarted playhead reports column 0 and `tick()` snaps it
                // to `col0`; for a `col0 == 0` loop zone `before == after` and
                // the opening step's CellLight events would be suppressed.
                let before = if self.playhead.started() {
                    active_rows(grid, &grid.loop_region, self.playhead.column())
                } else {
                    0
                };
                self.playhead.tick(&grid.loop_region);
                self.step += 1;
                let after = active_rows(grid, &grid.loop_region, self.playhead.column());
                let newly = after & !before;
                self.modulation.fire(newly);
                active = after;
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
    fn active_rows_marks_enabled_cells_in_the_loop_zone_at_the_column() {
        use crate::propagation::active_rows;
        let mut grid = Grid::default();
        grid.cell_mut(3, 5).enabled = true;
        grid.cell_mut(7, 5).enabled = false;
        let mask = active_rows(&grid, &grid.loop_region, 5);
        assert!(mask & (1 << 3) != 0, "row 3 enabled at col 5 -> active");
        assert!(mask & (1 << 7) == 0, "row 7 disabled at col 5 -> inactive");
    }

    #[test]
    fn active_rows_excludes_rows_outside_the_loop_zone() {
        use crate::grid::LoopRegion;
        use crate::propagation::active_rows;
        let mut grid = Grid::default();
        grid.loop_region = LoopRegion {
            row0: 4,
            row1: 8,
            col0: 0,
            col1: 31,
        };
        let mask = active_rows(&grid, &grid.loop_region, 0);
        assert!(mask & (1 << 2) == 0, "row 2 is above the loop zone");
        assert!(mask & (1 << 6) != 0, "row 6 is inside the loop zone");
    }

    #[test]
    fn active_mask_reports_the_last_blocks_active_rows() {
        // A fresh engine has processed nothing — mask is empty.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        assert_eq!(engine.active_mask(), 0);
        // After a playing block on the default grid, some rows are active.
        let grid = Grid::default();
        let mut left = [0.2_f32; 128];
        let mut right = [0.2_f32; 128];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);
        assert!(
            engine.active_mask() != 0,
            "after the playhead starts, at least one row should be active"
        );
    }

    #[test]
    fn new_engine_has_an_unstarted_playhead() {
        let engine = AudioEngine::new();
        assert_eq!(engine.playhead_column(), 0);
        assert_eq!(engine.step(), 0);
    }

    #[test]
    fn process_at_mix_zero_is_dry_passthrough() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default();
        let mut left = [0.1_f32; 64];
        let mut right = [-0.2_f32; 64];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 0.0, &grid);
        assert!(left.iter().all(|&s| (s - 0.1).abs() < 1e-6));
        assert!(right.iter().all(|&s| (s + 0.2).abs() < 1e-6));
    }

    #[test]
    fn process_default_effects_full_wet_is_silent() {
        // Not playing, fresh engine: every row's default effect is
        // `EffectKind::None` (an unassigned lane outputs silence), so the
        // fully-wet output is silence.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default();
        let mut left = [0.5_f32; 64];
        let mut right = [0.5_f32; 64];
        engine.process(&mut left, &mut right, false, 10.0, 120.0, 1.0, &grid);
        assert!(left.iter().all(|&s| s == 0.0));
        assert!(right.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn process_starts_and_produces_wet_signal() {
        // Playing, default grid, a short step: once the clock crosses its
        // first boundary the playhead starts scanning, rows become active,
        // and the fully-wet output is no longer silent. The default config
        // has every row set to EffectKind::None (which intentionally outputs
        // silence for an unassigned lane), so install a real effect on row 0
        // first.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut effects = [TrackEffect::default_for_row(0); ROWS];
        effects[0] = TrackEffect {
            kind: crate::effects::EffectKind::Lowpass,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Lowpass),
            mix: 1.0,
        };
        engine.set_effects(&effects);
        let grid = Grid::default();
        let mut left = [0.3_f32; 128];
        let mut right = [0.3_f32; 128];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);
        assert!(
            left[127] != 0.0,
            "after the playhead starts, a fully-wet block should not be silent"
        );
    }

    #[test]
    fn process_reset_returns_to_silence() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default();
        let mut buf = [0.3_f32; 128];
        let mut buf2 = [0.3_f32; 128];
        engine.process(&mut buf, &mut buf2, true, 10.0, 120.0, 1.0, &grid);
        engine.reset();
        // After reset, not playing: default `None` effects -> silent at full wet.
        let mut left = [0.4_f32; 64];
        let mut right = [0.4_f32; 64];
        engine.process(&mut left, &mut right, false, 10.0, 120.0, 1.0, &grid);
        assert!(left.iter().all(|&s| s == 0.0));
        assert!(right.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn engine_advances_the_playhead_after_a_step_boundary() {
        let grid = Grid::default(); // full loop region, col0 = 0
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        // A short block with one boundary (`pending_first` only — the
        // 4-sample accumulation stays below `samples_per_step`): the single
        // tick lands the playhead on the loop zone's left edge and bumps the
        // step counter.
        let mut left = [0.0_f32; 4];
        let mut right = [0.0_f32; 4];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 0.0, &grid);
        assert_eq!(engine.playhead_column(), grid.loop_region.col0);
        assert_eq!(
            engine.step(),
            1,
            "the first boundary tick advances the step"
        );
        // A longer block crosses several boundaries; the playhead scans right.
        let mut l2 = [0.0_f32; 64];
        let mut r2 = [0.0_f32; 64];
        engine.process(&mut l2, &mut r2, true, 10.0, 120.0, 0.0, &grid);
        assert!(
            engine.playhead_column() > grid.loop_region.col0,
            "subsequent ticks move the playhead off the left edge"
        );
    }

    #[test]
    fn cell_light_trigger_fires_on_the_sequences_first_step() {
        // The cell-light edge detector diffs the active-row mask before/after
        // each playhead tick. On the FIRST tick the unstarted playhead reports
        // column 0 and `tick()` snaps it to `col0`; the `Playhead::started()`
        // gate makes the pre-start `before` set empty so the opening step's
        // CellLight event is not suppressed. With per-segment firing the
        // trigger fires in the SAME block the cell lights — no one-block lag.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        mod_cfg[5].trigger = crate::modulation::TriggerSource::CellLight;
        engine.set_modulation(&mod_cfg);
        // The default grid has every cell enabled and a full loop region, so
        // row 5's cell at the opening column (col0 == 0) is lit.
        let grid = Grid::default();
        // One block: the playhead starts, row 5's cell lights on the opening
        // step, and its CellLight trigger fires within this same block.
        let mut l1 = [0.0_f32; 64];
        let mut r1 = [0.0_f32; 64];
        engine.process(&mut l1, &mut r1, true, 1000.0, 120.0, 1.0, &grid);
        assert!(
            engine.modulation_fires_for_test() & (1 << 5) != 0,
            "row 5's CellLight trigger should fire in the same block the cell lights"
        );
    }

    #[test]
    fn cell_light_fires_same_block_for_a_mid_block_step() {
        // A row that only becomes active several columns in still fires in the
        // block whose segment loop crosses that step boundary — not a block
        // later. A small samples_per_step makes one block sweep several
        // columns.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        mod_cfg[8].trigger = crate::modulation::TriggerSource::CellLight;
        engine.set_modulation(&mod_cfg);
        // Row 8 is disabled for columns 0..3 and enabled (the default) from
        // column 3 on — so it goes newly-active partway through the block,
        // not on the opening step.
        let mut grid = Grid::default();
        grid.cell_mut(8, 0).enabled = false;
        grid.cell_mut(8, 1).enabled = false;
        grid.cell_mut(8, 2).enabled = false;
        // samples_per_step 10 over a 128-sample block crosses ~12 boundaries,
        // so the playhead reaches column 3 well inside the block.
        let mut l = [0.0_f32; 128];
        let mut r = [0.0_f32; 128];
        engine.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, &grid);
        assert!(
            engine.modulation_fires_for_test() & (1 << 8) != 0,
            "row 8's CellLight trigger fires in the same block its cell first lights"
        );
    }

    #[test]
    fn engine_runs_per_track_effects() {
        let config: [crate::effects::TrackEffect; 16] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        engine.set_effects(&config);
        let grid = Grid::default();
        let mut left = [0.3_f32; 64];
        let mut right = [0.3_f32; 64];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);
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
                mix: 1.0,
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
    fn per_track_mix_zero_makes_an_active_track_contribute_dry() {
        // Row 0 runs a Bitcrush effect (audibly alters the signal) at mix 0.0;
        // the lane must collapse to the dry input. Compared against the same
        // setup at mix 1.0, which alters it.
        use crate::effects::{EffectKind, TrackEffect};

        let mut wet = TrackEffect::default_for_row(0);
        wet.kind = EffectKind::Bitcrush;
        wet.params = crate::effects::default_params_for_kind(EffectKind::Bitcrush);
        // Use 4-bit depth so the quantization error is large enough (≥ 1/16 ≈
        // 0.0625) to exceed the 1e-4 test threshold.
        wet.params[0] = 4.0;
        wet.mix = 1.0;
        let mut dry_mix = wet;
        dry_mix.mix = 0.0;

        // Two engines, identical but for row 0's mix.
        let build = |te: TrackEffect| {
            let mut e = AudioEngine::new();
            e.set_sample_rate(48_000.0);
            let mut effects = [TrackEffect::default_for_row(0); ROWS];
            effects[0] = te;
            e.set_effects(&effects);
            e
        };
        let mut e_wet = build(wet);
        let mut e_dry = build(dry_mix);

        let mut grid = Grid::default();
        // Ensure row 0 is active at the playhead's start column.
        grid.cell_mut(0, 0).enabled = true;
        // A constant non-zero input; fully wet so the lane reaches the output.
        let input = [0.6_f32; 128];
        let (mut wl, mut wr) = (input, input);
        let (mut dl, mut dr) = (input, input);
        e_wet.process(&mut wl, &mut wr, true, 1000.0, 120.0, 1.0, &grid);
        e_dry.process(&mut dl, &mut dr, true, 1000.0, 120.0, 1.0, &grid);

        // At mix 0.0 row 0's lane is dry, so its output equals the dry input.
        assert!(
            dl.iter().all(|&s| (s - 0.6).abs() < 1e-4),
            "mix 0.0 active track should output dry"
        );
        // At mix 1.0 the Bitcrush alters the signal — outputs differ.
        assert!(
            wl.iter()
                .zip(dl.iter())
                .any(|(&w, &d)| (w - d).abs() > 1e-4),
            "mix 1.0 should differ from mix 0.0"
        );
        let _ = (wr, dr); // right-channel arrays; Bitcrush is symmetric, left-only assertion suffices
    }

    #[test]
    fn per_track_mix_half_is_the_midpoint_of_dry_and_wet() {
        // The per-track blend is `lane = dry + (effect − dry)·mix`, so a mix
        // of 0.5 must land exactly halfway between the dry (mix 0) and fully
        // wet (mix 1) lanes. `process_sample` is called directly so the
        // assertion isolates the lane blend — a fresh engine has amplitude
        // 1.0 for every row (begin_block not yet called) and only row 0 is active, so
        // the result is exactly that one lane: no compressor, no global mix.
        use crate::effects::{EffectKind, TrackEffect};

        // Three engines, identical but for row 0's mix.
        let build = |mix: f32| {
            let mut te = TrackEffect::default_for_row(0);
            te.kind = EffectKind::Bitcrush;
            te.params = crate::effects::default_params_for_kind(EffectKind::Bitcrush);
            // 4-bit depth: a large enough quantization step that the wet lane
            // genuinely differs from the dry one (so the midpoint isn't vacuous).
            te.params[0] = 4.0;
            te.mix = mix;
            let mut e = AudioEngine::new();
            e.set_sample_rate(48_000.0);
            let mut effects = [TrackEffect::default_for_row(0); ROWS];
            effects[0] = te;
            e.set_effects(&effects);
            e
        };
        let mut e_dry = build(0.0);
        let mut e_half = build(0.5);
        let mut e_wet = build(1.0);

        // Active mask = row 0 only.
        let dry = e_dry.process_sample(0.6, 0.6, 1 << 0).0;
        let half = e_half.process_sample(0.6, 0.6, 1 << 0).0;
        let wet = e_wet.process_sample(0.6, 0.6, 1 << 0).0;

        // mix 0 → the lane is the dry input.
        assert!((dry - 0.6).abs() < 1e-6, "mix 0.0 should be dry: dry={dry}");
        // The 4-bit Bitcrush genuinely alters the signal — midpoint isn't vacuous.
        assert!(
            (wet - dry).abs() > 1e-4,
            "wet lane must differ from dry: wet={wet}, dry={dry}"
        );
        // mix 0.5 lane is the exact midpoint of the dry (mix 0) and wet (mix 1) lanes.
        assert!(
            (half - (dry + wet) / 2.0).abs() < 1e-6,
            "mix 0.5 should be the midpoint: half={half}, dry={dry}, wet={wet}"
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
            mix: 1.0,
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
        // Default effects are now `None` (passthrough); set every track to
        // Lowpass so the modulation has an audible parameter to sweep.
        let effect_cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(|_| crate::effects::TrackEffect {
                kind: crate::effects::EffectKind::Lowpass,
                params: [2_000.0, 0.15, 0.0, 0.0],
                mix: 1.0,
            });
        engine.set_effects(&effect_cfg);
        let mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        engine.set_modulation(&mod_cfg);
        let grid = Grid::default();
        // Run two well-separated stretches of audio; the default per-track
        // assignable MSEG should make the wet output drift over time.
        let mut a = [0.4_f32; 64];
        let mut b = [0.4_f32; 64];
        let mut a_r = a;
        engine.process(&mut a, &mut a_r, true, 10.0, 120.0, 1.0, &grid);
        for _ in 0..300 {
            let mut l = [0.4_f32; 64];
            let mut r = [0.4_f32; 64];
            engine.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, &grid);
        }
        let mut b_r = b;
        engine.process(&mut b, &mut b_r, true, 10.0, 120.0, 1.0, &grid);
        assert!(a.iter().all(|s| s.is_finite()) && b.iter().all(|s| s.is_finite()));
        assert!(a != b, "modulation should make the output drift over time");
    }
}
