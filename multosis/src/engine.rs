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
    /// Wet-bus compressor (soft-knee peak) — limits the series chain's
    /// output before the dry/wet mix. Useful when a row's amplitude MSEG
    /// or saturating effect pushes the chain hot; the compressor catches
    /// the peak without the user having to ride a master gain.
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

    /// Test-only: the cached per-track effect config (kind/params/mix).
    #[cfg(test)]
    pub fn track_effects_for_test(&self) -> [TrackEffect; ROWS] {
        self.track_effects
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

    /// Swap rows `a` and `b` across every piece of per-row state: the live
    /// `EffectInstance` (so DSP state moves with the track instead of being
    /// rebuilt on the next `set_effects`), the cached `TrackEffect`, and the
    /// modulation runtime (config + MSEG phases + Free-Hz phase + per-row
    /// amplitudes). Called from the audio thread when the editor's
    /// drag-and-drop reorder posts a swap.
    pub fn swap_tracks(&mut self, a: usize, b: usize) {
        if a == b || a >= ROWS || b >= ROWS {
            return;
        }
        self.effects.swap(a, b);
        self.track_effects.swap(a, b);
        self.modulation.swap_rows(a, b);
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

    /// Restart the sequence end-to-end: playhead at the loop start, step
    /// counter zeroed, clock and MSEG phases zeroed, every effect and the
    /// wet-bus compressor flushed. Called on the transport stopped→playing
    /// edge and from the editor's Reset button — the user-visible "play
    /// from the top" reset. NOT the target of `Plugin::reset` from the
    /// host; that calls the narrower `flush_dsp` so a mid-playback PDC
    /// realignment in Bitwig does not retrigger every MSEG.
    pub fn reset(&mut self) {
        self.playhead.reset();
        self.step = 0;
        self.clock.reset();
        self.flush_dsp();
        self.modulation.reset();
        self.last_active = 0;
    }

    /// Flush DSP buffer state — every per-row effect and the wet-bus
    /// compressor. Sequence and modulation timing (playhead, step counter,
    /// clock, MSEG phases, last-active mirror) are deliberately preserved.
    /// This is the audio-thread target of `Plugin::reset`: Bitwig (and
    /// other hosts) call `Plugin::reset` mid-playback on PDC realignment,
    /// and clearing MSEG state there is audible as every MSEG on every
    /// track restarting in lockstep. The transport stopped→playing edge in
    /// `process` still drives the full sequence restart via [`reset`].
    pub fn flush_dsp(&mut self) {
        for e in &mut self.effects {
            e.reset();
        }
        self.compressor.reset();
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

    /// Run the active rows' effects as a series chain on one stereo sample.
    /// Rows are walked top-to-bottom (row 0 → row 15); each active row sees
    /// the previous active row's output, and the first active row sees the
    /// dry input. Per row:
    ///
    /// * `eff = effect.process_sample(input)`
    /// * `lane = input + (eff − input) · mix`   — per-row dry/wet blend
    /// * `next_input = amp · lane`              — amplitude MSEG as a VCA
    ///
    /// Rows whose `EffectKind` is `None` are skipped so an unconfigured slot
    /// is transparent (rather than zeroing the chain — `NoneEffect` outputs
    /// silence). Inactive rows (cell off at the playhead column or outside
    /// the loop zone) are likewise skipped.
    fn process_sample(&mut self, dry_l: f32, dry_r: f32, active: u16) -> (f32, f32) {
        let mut cur_l = dry_l;
        let mut cur_r = dry_r;
        for r in 0..ROWS {
            if active & (1 << r) == 0 {
                continue;
            }
            if self.track_effects[r].kind == crate::effects::EffectKind::None {
                continue;
            }
            let (eff_l, eff_r) = self.effects[r].process_sample(cur_l, cur_r);
            let mix = self.track_effects[r].mix;
            let lane_l = cur_l + (eff_l - cur_l) * mix;
            let lane_r = cur_r + (eff_r - cur_r) * mix;
            let amp = self.modulation.amplitude(r);
            cur_l = amp * lane_l;
            cur_r = amp * lane_r;
        }
        (cur_l, cur_r)
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
    /// `CellLight` edge or a `CellStep` active step resets its row at the
    /// exact boundary sample.
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

        // Block-rate modulation setup: FreeHz + Transient fires, zero the
        // fire mask. The transient detector reads the dry input before the
        // series chain mutates the buffer — feeding the chain output back
        // through the detector would tail-chase its own modulation.
        self.modulation.begin_block(&left[..n], &right[..n], sr);

        // Push the current host tempo into every effect once per block so
        // tempo-syncing effects (Delay's beat-synced subdivisions) compute
        // their per-sample work from a stable BPM. Effects that don't care
        // about tempo use the trait's default no-op.
        let bpm_f32 = bpm as f32;
        for e in &mut self.effects {
            e.set_bpm(bpm_f32);
        }

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

        // Block-rate cache of the mute/solo mask. Each bit set means the row
        // is effectively muted — either its own `muted` flag, or some other
        // row is soloed and this one isn't. Recomputed once per block so the
        // hot per-sample loop stays branch-light. Modulation triggers
        // (CellLight / CellStep) intentionally run from the UN-masked grid
        // activity below: muting a row bypasses its effect but should NOT
        // disrupt its MSEG timing, so when the user un-mutes, the curve is
        // exactly where it would have been.
        let mute_mask = self.effective_mute_mask();

        // Walk the block in segments split at each boundary. Per segment:
        // advance the per-MSEG-clock modulation, then render the audio. At a
        // boundary, fire the CellLight and CellStep rows so the phase reset
        // lands on the very next segment — sample-accurate, no cross-block delay.
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
            let active_audio = active & !mute_mask;
            for i in cursor..seg_end {
                let (dry_l, dry_r) = (left[i], right[i]);
                let (wet_l, wet_r) = self.process_sample(dry_l, dry_r, active_audio);
                // Limit the chain output on the wet bus, then mix dry/wet.
                let (cw_l, cw_r) = self.compressor.process_sample(wet_l, wet_r);
                left[i] = dry_l + (cw_l - dry_l) * mix;
                right[i] = dry_r + (cw_r - dry_r) * mix;
            }
            cursor = seg_end;
            if bi < n_boundaries {
                // Snapshot the active-row mask BEFORE and AFTER the tick.
                // `newly` (became-active) fires CellLight; the post-tick
                // `after` mask fires CellStep.
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
                self.modulation.fire(newly, after);
                active = after;
                bi += 1;
            }
        }
        // last_active is the audible mask — what the user sees in the track
        // listing's "sounding" dot. Muted/non-soloed rows stay dark even when
        // their grid cell is on.
        self.last_active = active & !mute_mask;
    }

    /// Sum of per-effect latency samples across rows that are reachable
    /// in the audio chain — i.e. not effectively muted (own mute or
    /// solo-cancelled) and not `EffectKind::None`. This is what multosis
    /// reports to the host as plugin delay compensation, so PDC keeps the
    /// track aligned with the rest of the project regardless of which
    /// rows have latency-introducing effects (today: only Warp Zone).
    ///
    /// We use the "potentially reachable" set rather than the grid-active
    /// set: a row whose cells are off at the current playhead would
    /// otherwise toggle latency every step, which is impractical for
    /// hosts that re-align on each change. With this conservative sum the
    /// host pulls back enough buffer for any column the user can reach
    /// without re-arranging the track lanes.
    pub fn chain_latency_samples(&self) -> usize {
        let mute_mask = self.effective_mute_mask();
        let mut total = 0;
        for (r, te) in self.track_effects.iter().enumerate() {
            if te.kind == crate::effects::EffectKind::None {
                continue;
            }
            if mute_mask & (1 << r) != 0 {
                continue;
            }
            total += self.effects[r].latency_samples();
        }
        total
    }

    /// Build the per-block mute mask: bit `r` set when row `r` is
    /// effectively bypassed — either its own `muted` flag is on, or some
    /// other row is soloed and this one isn't. A row that is both `muted`
    /// and `soloed` stays muted (the explicit mute wins).
    fn effective_mute_mask(&self) -> u16 {
        let any_soloed = self.track_effects.iter().any(|te| te.soloed);
        let mut mask = 0u16;
        for r in 0..ROWS {
            let te = &self.track_effects[r];
            if te.muted || (any_soloed && !te.soloed) {
                mask |= 1 << r;
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
    fn chain_latency_samples_sums_warpzone_rows_and_skips_muted_or_none() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        // Fresh engine: every row is None → zero chain latency.
        assert_eq!(engine.chain_latency_samples(), 0);
        // Put a Warp Zone on rows 0 and 2; row 1 stays None.
        let mut effects = [TrackEffect::default_for_row(0); ROWS];
        effects[0] = TrackEffect {
            kind: crate::effects::EffectKind::WarpZone,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::WarpZone),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        effects[2] = TrackEffect {
            kind: crate::effects::EffectKind::WarpZone,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::WarpZone),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&effects);
        // Two Warp Zone rows × FFT_SIZE = 2 × 4096 = 8192.
        assert_eq!(engine.chain_latency_samples(), 8192);

        // Muting row 2 drops it from the sum.
        effects[2].muted = true;
        engine.set_effects(&effects);
        assert_eq!(engine.chain_latency_samples(), 4096);

        // Soloing row 0 cancels the un-muted row 2-equivalent — but row 2
        // is already muted, so this is the same 4096.
        effects[0].soloed = true;
        engine.set_effects(&effects);
        assert_eq!(engine.chain_latency_samples(), 4096);

        // Un-mute row 2 and turn solo off; back to 8192.
        effects[2].muted = false;
        effects[0].soloed = false;
        engine.set_effects(&effects);
        assert_eq!(engine.chain_latency_samples(), 8192);

        // Non-WarpZone effects don't contribute (latency_samples = 0).
        effects[1] = TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&effects);
        assert_eq!(engine.chain_latency_samples(), 8192);
    }

    #[test]
    fn chain_latency_samples_tracks_spectral_fft_size_selection() {
        // SpectralRotate's FFT selector controls its reported latency.
        // Selecting a new size via set_effects must (after the SpectralEngine
        // applies the latched switch at the next hop boundary) propagate
        // through to chain_latency_samples so the host's PDC can re-align.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut effects = [TrackEffect::default_for_row(0); ROWS];
        // SpectralRotate with default params: FFT slot is the LAST param
        // (index 1); default value 2.0 -> FFT_SIZES[2] = 2048-pt, hop = 1024.
        effects[0] = TrackEffect {
            kind: crate::effects::EffectKind::SpectralRotate,
            params: crate::effects::default_params_for_kind(
                crate::effects::EffectKind::SpectralRotate,
            ),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&effects);
        assert_eq!(
            engine.chain_latency_samples(),
            2048,
            "default FFT=2048 reports one-window latency"
        );

        // Select FFT=512 (param value 0.0 -> FFT_SIZES[0]) via the same
        // set_effects path the editor uses for a parameter edit.
        effects[0].params[1] = 0.0;
        engine.set_effects(&effects);

        // chain_latency updates IMMEDIATELY -- SpectralEngine::latency_samples
        // reports the pending slot's FFT size so the host can re-align PDC
        // right away, even before any audio flows through the effect. The
        // audio path itself still latches the switch until the next hop
        // boundary.
        assert_eq!(
            engine.chain_latency_samples(),
            512,
            "FFT change must propagate to chain_latency immediately so the host can re-align PDC"
        );

        // Driving samples does not move chain_latency further -- the value
        // was already correct.
        for _ in 0..1100 {
            engine.effects_mut_for_test(0).process_sample(0.0, 0.0);
        }
        assert_eq!(engine.chain_latency_samples(), 512);
    }

    #[test]
    fn muted_row_drops_out_of_the_active_mask_and_audio_chain() {
        // Row 0 with an SVF heavily attenuating a high signal: with the
        // effect active the output is small; with row 0 muted the dry
        // signal passes through.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut effects = [TrackEffect::default_for_row(0); ROWS];
        effects[0] = TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            // Cutoff = 80 Hz; resonance low; LP; 8 poles. With a 0.5 DC-ish
            // input the SVF settles ~unity at DC but the test below uses a
            // simple identity assertion at mute: if muted the audible output
            // matches dry, otherwise it doesn't.
            params: [80.0, 0.1, 0.0, 3.0, 0.0],
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&effects);

        let grid = Grid::default();
        let mut left = [0.5_f32; 64];
        let mut right = [0.5_f32; 64];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);
        // active_mask reports the audible mask — row 0 is set when un-muted.
        assert!(
            engine.active_mask() & 1 != 0,
            "row 0 should be in the active mask when un-muted"
        );

        // Now mute row 0 and run a fresh block from the same input.
        effects[0].muted = true;
        engine.set_effects(&effects);
        engine.reset();
        let mut left2 = [0.5_f32; 64];
        let mut right2 = [0.5_f32; 64];
        engine.process(&mut left2, &mut right2, true, 10.0, 120.0, 1.0, &grid);
        // Muted row drops out of the active mask.
        assert_eq!(
            engine.active_mask() & 1,
            0,
            "row 0 should be cleared from active_mask when muted"
        );
        // And audibly: muted samples equal the dry input.
        for (i, &s) in left2.iter().enumerate() {
            assert!(
                (s - 0.5).abs() < 1e-5,
                "muted row 0 sample {i} should pass dry through, got {s}"
            );
        }
    }

    #[test]
    fn solo_mask_silences_every_non_soloed_row_but_keeps_soloed_rows_audible() {
        // Two rows wired with effects. Solo row 1; row 0 should drop out
        // of active_mask while row 1 stays. No row soloed → both audible.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut effects = [TrackEffect::default_for_row(0); ROWS];
        effects[0] = TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        effects[1] = TrackEffect {
            kind: crate::effects::EffectKind::Bitcrush,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Bitcrush),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&effects);

        let grid = Grid::default();
        let mut left = [0.3_f32; 64];
        let mut right = [0.3_f32; 64];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);
        // Baseline: both rows audible.
        let unsoloed_mask = engine.active_mask();
        assert!(
            unsoloed_mask & 0b11 == 0b11,
            "both rows should be in the active mask before soloing (got {unsoloed_mask:#06b})"
        );

        // Solo row 1 → row 0 drops out, row 1 stays.
        effects[1].soloed = true;
        engine.set_effects(&effects);
        let mut left2 = [0.3_f32; 64];
        let mut right2 = [0.3_f32; 64];
        engine.process(&mut left2, &mut right2, true, 10.0, 120.0, 1.0, &grid);
        let mask = engine.active_mask();
        assert_eq!(
            mask & 1,
            0,
            "row 0 should drop out under solo (got {mask:#06b})"
        );
        assert!(
            mask & 0b10 != 0,
            "row 1 (soloed) stays audible (got {mask:#06b})"
        );
    }

    #[test]
    fn explicit_mute_wins_over_solo_when_both_set() {
        // A row that is both soloed AND muted stays muted — the explicit
        // mute is the user's intent.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut effects = [TrackEffect::default_for_row(0); ROWS];
        effects[2] = TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: true,
            soloed: true,
        };
        engine.set_effects(&effects);

        let grid = Grid::default();
        let mut left = [0.3_f32; 64];
        let mut right = [0.3_f32; 64];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);
        assert_eq!(
            engine.active_mask() & (1 << 2),
            0,
            "muted+soloed row should be silent — mute wins"
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
    fn process_with_default_none_effects_is_dry_passthrough() {
        // Fresh engine, default config: every row's effect is `EffectKind::None`,
        // which the series chain skips. With no chain stages and a sub-threshold
        // input the compressor stays unity, so a fully-wet block equals dry.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default();
        // 0.05 is well below the compressor's −6 dB threshold (≈ 0.5 linear).
        let mut left = [0.05_f32; 64];
        let mut right = [0.05_f32; 64];
        engine.process(&mut left, &mut right, false, 10.0, 120.0, 1.0, &grid);
        assert!(left.iter().all(|&s| (s - 0.05).abs() < 1e-6));
        assert!(right.iter().all(|&s| (s - 0.05).abs() < 1e-6));
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
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
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
    fn series_chain_orders_effects_so_row_swap_changes_output() {
        // The series chain runs row 0 → row 15. Swap an order-sensitive pair
        // (Lowpass and Bitcrush — bit-reducing a band-limited signal vs.
        // band-limiting a bit-reduced one produces different output) and
        // verify the resulting buffer differs. If the engine were still
        // parallel, swapping row 0 ↔ row 1 would leave the sum unchanged.
        let grid = Grid::default();
        let mut lp_first = [TrackEffect::default_for_row(0); ROWS];
        lp_first[0] = TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        lp_first[1] = TrackEffect {
            kind: crate::effects::EffectKind::Bitcrush,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Bitcrush),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        let mut bc_first = lp_first;
        bc_first.swap(0, 1);

        // Drive a small sine through each ordering and compare.
        let sr = 48_000.0_f32;
        let n = 128;
        let mut dry = vec![0.0_f32; n];
        for (i, s) in dry.iter_mut().enumerate() {
            *s = 0.1 * (2.0 * std::f32::consts::PI * 1_000.0 * (i as f32) / sr).sin();
        }

        let render = |effects: &[TrackEffect; ROWS]| -> Vec<f32> {
            let mut e = AudioEngine::new();
            e.set_sample_rate(sr);
            e.set_effects(effects);
            let mut l = dry.clone();
            let mut r = dry.clone();
            e.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, &grid);
            l
        };
        let a = render(&lp_first);
        let b = render(&bc_first);
        let max_diff = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_diff > 1e-4,
            "expected Lowpass→Bitcrush to differ from Bitcrush→Lowpass; max diff = {max_diff}"
        );
    }

    #[test]
    fn series_chain_skips_none_rows_so_they_are_transparent() {
        // A row whose effect is `None` must not break the chain. Place a
        // Lowpass on row 5 with every other row defaulting to None; the
        // output should match running with just row 5 active (None rows
        // are transparent under the series chain).
        let grid = Grid::default();
        let mut sparse = [TrackEffect::default_for_row(0); ROWS];
        sparse[5] = TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
        };

        let sr = 48_000.0_f32;
        let n = 64;
        let dry = vec![0.05_f32; n];

        let mut e = AudioEngine::new();
        e.set_sample_rate(sr);
        e.set_effects(&sparse);
        let mut l = dry.clone();
        let mut r = dry.clone();
        e.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, &grid);
        // The 16 None rows around row 5 must not zero the chain — the
        // tail of the buffer is the Lowpass settled response, not silence.
        assert!(
            l[n - 1].abs() > 1e-3,
            "None rows should be transparent; got near-silence ({})",
            l[n - 1]
        );
    }

    #[test]
    fn process_after_reset_with_default_effects_is_dry_passthrough() {
        // Run a block to populate compressor/modulation state, then reset and
        // run a sub-threshold block. With every row's default `EffectKind::None`
        // the series chain has no stages, the compressor sits below its knee,
        // and a fully-wet block must equal the dry input.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default();
        let mut buf = [0.3_f32; 128];
        let mut buf2 = [0.3_f32; 128];
        engine.process(&mut buf, &mut buf2, true, 10.0, 120.0, 1.0, &grid);
        engine.reset();
        let mut left = [0.05_f32; 64];
        let mut right = [0.05_f32; 64];
        engine.process(&mut left, &mut right, false, 10.0, 120.0, 1.0, &grid);
        assert!(left.iter().all(|&s| (s - 0.05).abs() < 1e-6));
        assert!(right.iter().all(|&s| (s - 0.05).abs() < 1e-6));
    }

    #[test]
    fn flush_dsp_preserves_modulation_phases_and_sequence_position() {
        // `Plugin::reset` (the host's "flush your state" callback) maps to
        // `flush_dsp`, not `reset`. Bitwig calls Plugin::reset mid-playback
        // in response to its own PDC bookkeeping, and clearing MSEG state
        // there is audible as every MSEG on every track restarting in
        // lockstep on any parameter edit that nudges chain latency. Guard
        // against regression: flush_dsp must keep the playhead, step
        // counter, and every MSEG phase exactly where they were.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);

        // Free-running MSEGs on every row so no CellLight/CellStep fires
        // mid-block can mask a regression by zeroing phases for legitimate
        // sequencer reasons.
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        for row in mod_cfg.iter_mut() {
            row.trigger = crate::modulation::TriggerSource::Free;
        }
        engine.set_modulation(&mod_cfg);

        // Run a playing block long enough to advance several step
        // boundaries (sps=10 + 256-sample block crosses ~25 steps) so
        // every piece of state we want to preserve has moved off zero.
        let grid = Grid::default();
        let mut left = [0.3_f32; 256];
        let mut right = [0.3_f32; 256];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, &grid);

        let playhead_before = engine.playhead_column();
        let step_before = engine.step();
        let mut phases_before = Vec::with_capacity(ROWS * 4);
        for r in 0..ROWS {
            for k in 0..4 {
                phases_before.push(engine.modulation_phase(r, k));
            }
        }
        assert!(
            step_before > 0,
            "step counter must advance off zero for the preserve-check to be meaningful"
        );
        assert!(
            phases_before.iter().any(|&p| p > 0.0),
            "at least one MSEG phase must advance off zero for the preserve-check to mean something"
        );

        // The audio-thread target of `Plugin::reset` from the host.
        engine.flush_dsp();

        assert_eq!(
            engine.playhead_column(),
            playhead_before,
            "flush_dsp must preserve the playhead column"
        );
        assert_eq!(
            engine.step(),
            step_before,
            "flush_dsp must preserve the step counter"
        );
        let mut phases_after = Vec::with_capacity(ROWS * 4);
        for r in 0..ROWS {
            for k in 0..4 {
                phases_after.push(engine.modulation_phase(r, k));
            }
        }
        assert_eq!(
            phases_after, phases_before,
            "flush_dsp must preserve every MSEG phase (any zeroing is audible as all-tracks restart)"
        );
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
        // Install a real effect on at least one row — under the series chain a
        // row with `EffectKind::None` is transparent, so an all-None config
        // would pass through dry and not exercise the effect path.
        let mut config: [crate::effects::TrackEffect; 16] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        config[0] = crate::effects::TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
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
    fn swap_tracks_exchanges_track_effects_kind_and_params() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        cfg[3] = crate::effects::TrackEffect {
            kind: crate::effects::EffectKind::Svf,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Svf),
            mix: 0.7,
            muted: false,
            soloed: false,
        };
        cfg[7] = crate::effects::TrackEffect {
            kind: crate::effects::EffectKind::Bitcrush,
            params: crate::effects::default_params_for_kind(crate::effects::EffectKind::Bitcrush),
            mix: 0.4,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&cfg);
        engine.swap_tracks(3, 7);
        let cached = engine.track_effects_for_test();
        assert_eq!(cached[3].kind, crate::effects::EffectKind::Bitcrush);
        assert!((cached[3].mix - 0.4).abs() < 1e-6);
        assert_eq!(cached[7].kind, crate::effects::EffectKind::Svf);
        assert!((cached[7].mix - 0.7).abs() < 1e-6);
        assert_eq!(
            engine.effects_mut_for_test(3).kind(),
            crate::effects::EffectKind::Bitcrush
        );
        assert_eq!(
            engine.effects_mut_for_test(7).kind(),
            crate::effects::EffectKind::Svf
        );
    }

    #[test]
    fn swap_tracks_preserves_effect_dsp_state_so_the_swap_is_seamless() {
        // The whole point of `swap_tracks` exchanging live `EffectInstance`s
        // (rather than rebuilding via `set_effects`) is that the lowpass's
        // delay line stays with its track. Drive row 0 hot, swap with row 5
        // (whose lowpass has no history), and confirm that what was row 0
        // is now at row 5 and still produces a settled (non-transient) output.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(|_| crate::effects::TrackEffect {
                kind: crate::effects::EffectKind::Svf,
                params: [800.0, 0.2, 0.0, 0.0, 0.0],
                mix: 1.0,
                muted: false,
                soloed: false,
            });
        engine.set_effects(&cfg);
        // Charge row 0's lowpass with a DC input.
        for _ in 0..1024 {
            engine.effects_mut_for_test(0).process_sample(1.0, 1.0);
        }
        let row0_settled = engine.effects_mut_for_test(0).process_sample(1.0, 1.0).0;
        engine.swap_tracks(0, 5);
        // The charged state must now be at row 5 (we swapped the instance,
        // not zeroed it). A fresh sample at row 5 should land near the
        // settled value, not near zero.
        let row5_after = engine.effects_mut_for_test(5).process_sample(1.0, 1.0).0;
        assert!(
            (row5_after - row0_settled).abs() < 0.2,
            "swap should carry DSP state across: settled {row0_settled} -> after {row5_after}"
        );
    }

    #[test]
    fn swap_tracks_is_a_noop_when_indices_match_or_are_out_of_range() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        cfg[2].mix = 0.33;
        engine.set_effects(&cfg);
        let before = engine.track_effects_for_test();
        engine.swap_tracks(2, 2);
        assert_eq!(engine.track_effects_for_test()[2].mix, before[2].mix);
        engine.swap_tracks(2, 999);
        assert_eq!(engine.track_effects_for_test()[2].mix, before[2].mix);
        engine.swap_tracks(999, 2);
        assert_eq!(engine.track_effects_for_test()[2].mix, before[2].mix);
    }

    #[test]
    fn set_effects_preserves_dsp_state_when_kind_is_unchanged() {
        // A lowpass with running state; re-applying the same kind with a new
        // cutoff must not reset the filter (no zeroed history => no transient).
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(|_| crate::effects::TrackEffect {
                kind: crate::effects::EffectKind::Svf,
                params: [800.0, 0.2, 0.0, 0.0, 0.0],
                mix: 1.0,
                muted: false,
                soloed: false,
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
            kind: crate::effects::EffectKind::Svf,
            params: [2000.0, 0.1, 0.0, 0.0, 0.0],
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        engine.set_effects(&cfg);
        assert_eq!(
            engine.effects_mut_for_test(0).kind(),
            crate::effects::EffectKind::Svf
        );
        cfg[0].kind = crate::effects::EffectKind::Bitcrush;
        engine.set_effects(&cfg);
        assert_eq!(
            engine.effects_mut_for_test(0).kind(),
            crate::effects::EffectKind::Bitcrush
        );
    }

    #[test]
    fn cell_step_trigger_fires_on_every_step_not_just_the_edge() {
        // The default grid enables every cell, so every row is active at
        // every column. After the opening step a row stays continuously
        // active — no inactive->active edge — so CellLight fires only once
        // (block 1) while CellStep fires on every step (both blocks).
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        mod_cfg[4].trigger = crate::modulation::TriggerSource::CellLight;
        mod_cfg[9].trigger = crate::modulation::TriggerSource::CellStep;
        engine.set_modulation(&mod_cfg);
        let grid = Grid::default();
        // Block 1: the playhead starts; both rows fire on the opening step.
        let mut l1 = [0.0_f32; 64];
        let mut r1 = [0.0_f32; 64];
        engine.process(&mut l1, &mut r1, true, 10.0, 120.0, 1.0, &grid);
        assert!(
            engine.modulation_fires_for_test() & (1 << 4) != 0,
            "CellLight fires on the opening step"
        );
        assert!(
            engine.modulation_fires_for_test() & (1 << 9) != 0,
            "CellStep fires on the opening step"
        );
        // Block 2: both rows stay continuously active (no new edge).
        let mut l2 = [0.0_f32; 64];
        let mut r2 = [0.0_f32; 64];
        engine.process(&mut l2, &mut r2, true, 10.0, 120.0, 1.0, &grid);
        assert_eq!(
            engine.modulation_fires_for_test() & (1 << 4),
            0,
            "CellLight does not fire on a non-edge step"
        );
        assert!(
            engine.modulation_fires_for_test() & (1 << 9) != 0,
            "CellStep fires on every step, including non-edge steps"
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
                kind: crate::effects::EffectKind::Svf,
                params: [2_000.0, 0.15, 0.0, 0.0, 0.0],
                mix: 1.0,
                muted: false,
                soloed: false,
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
