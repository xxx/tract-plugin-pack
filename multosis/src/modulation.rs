//! Sequencer orchestration of the shared per-effect modulation runtime.

pub use tract_effects::modulation::*;

use crate::effects::{EffectInstance, TrackEffect};
use crate::grid::ROWS;

/// Sixteen independent modulation states, with sequencer trigger dispatch.
pub struct Modulation {
    config: [TrackModulation; ROWS],
    states: [ModulationState; ROWS],
}

impl Modulation {
    pub fn new() -> Self {
        Self {
            config: std::array::from_fn(TrackModulation::default_for_row),
            states: std::array::from_fn(|_| ModulationState::default()),
        }
    }

    /// Replace configuration while preserving envelope and detector state.
    pub fn set_config(&mut self, config: &[TrackModulation; ROWS]) {
        self.config = config.clone();
    }

    pub fn swap_rows(&mut self, a: usize, b: usize) {
        if a == b || a >= ROWS || b >= ROWS {
            return;
        }
        self.config.swap(a, b);
        self.states.swap(a, b);
    }

    pub fn reset(&mut self) {
        for state in &mut self.states {
            state.reset();
        }
    }

    pub fn amplitude(&self, row: usize) -> f32 {
        self.states[row].amplitude()
    }

    pub fn phase(&self, row: usize, slot: usize) -> f32 {
        self.states[row].phase(slot)
    }

    pub fn begin_block(&mut self, left: &[f32], right: &[f32], sample_rate: f64) {
        for (state, config) in self.states.iter_mut().zip(&self.config) {
            state.begin_block(config, left, right, sample_rate);
        }
    }

    pub fn advance_segment(
        &mut self,
        seg_len: usize,
        bpm: f64,
        sample_rate: f64,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        for row in 0..ROWS {
            self.states[row].advance_segment(
                &self.config[row],
                &track_effects[row],
                &mut effects[row],
                seg_len,
                bpm,
                sample_rate,
            );
        }
    }

    /// Dispatch cell triggers; other trigger sources run in `begin_block`.
    pub fn fire(&mut self, newly_rows: u16, active_rows: u16) {
        for row in 0..ROWS {
            let reset = match self.config[row].trigger {
                TriggerSource::CellLight => newly_rows & (1 << row) != 0,
                TriggerSource::CellStep => active_rows & (1 << row) != 0,
                _ => false,
            };
            if reset {
                self.states[row].trigger();
            }
        }
    }

    /// Rows triggered since the last block began.
    pub fn fires_last_block(&self) -> u16 {
        self.states
            .iter()
            .enumerate()
            .fold(0, |mask, (row, state)| {
                mask | (u16::from(state.fired()) << row)
            })
    }
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Modulation {
    pub fn phases_all_zero(&self) -> bool {
        self.states
            .iter()
            .all(|s| (0..4).all(|k| s.phase(k) == 0.0))
    }

    pub fn phase_for_test(&self, row: usize, slot: usize) -> f32 {
        self.phase(row, slot)
    }

    pub fn hz_phases_all_zero(&self) -> bool {
        self.states.iter().all(|s| s.hz_phase() == 0.0)
    }

    pub fn hz_phase_for_test(&self, row: usize) -> f32 {
        self.states[row].hz_phase()
    }

    pub fn config_for_test(&self, row: usize) -> &TrackModulation {
        &self.config[row]
    }

    pub fn begin_block_silent(&mut self, block_len: usize, sample_rate: f64) {
        let silence = vec![0.0; block_len];
        self.begin_block(&silence, &silence, sample_rate);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, EffectKind, ParamSpec};
    use tiny_skia_widgets::{MsegData, Polarity, SyncMode, advance};

    #[test]
    fn track_modulation_serde_round_trips() {
        let tm = TrackModulation::default_for_row(3);
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tm);
    }

    #[test]
    fn track_modulation_array_serde_round_trips() {
        // The 16-row TrackModulation array carries (16 * 4 = 64) MsegData
        // instances, each with a 16-slot node table -- the full struct is
        // tens of kilobytes. serde_json's recursive descent through the
        // array elements uses enough additional stack frames that the
        // default 2 MB test-thread stack overflows. Box the round-trip
        // value so the bulk lives on the heap.
        let cfg: Box<[TrackModulation; ROWS]> =
            Box::new(std::array::from_fn(TrackModulation::default_for_row));
        let json = serde_json::to_string(&*cfg).unwrap();
        let back: Box<[TrackModulation; ROWS]> = serde_json::from_str(&json).unwrap();
        assert_eq!(*back, *cfg);
    }

    #[test]
    fn default_for_row_assigns_one_assignable_and_varies_by_row() {
        let a = TrackModulation::default_for_row(0);
        let b = TrackModulation::default_for_row(7);
        assert_eq!(a.targets[0], Some(0));
        assert_eq!(a.targets[1], None);
        assert!(a.depths[0] != 0.0);
        assert_eq!(a.depths[1], 0.0);
        assert!(a.msegs[1].beats != b.msegs[1].beats);
        assert!(
            a.msegs[0].nodes[..a.msegs[0].node_count]
                .iter()
                .all(|n| (n.value - 1.0).abs() < 1e-6)
        );
    }

    #[test]
    fn mseg_phase_delta_time_sync() {
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Time;
        m.time_seconds = 2.0;
        let dt = mseg_phase_delta(&m, 48, 120.0, 48_000.0);
        assert!((dt - 48.0 / 96_000.0).abs() < 1e-9, "got {dt}");
    }

    #[test]
    fn mseg_phase_delta_beat_sync() {
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Beat;
        m.beats = 4.0;
        let dt = mseg_phase_delta(&m, 48, 120.0, 48_000.0);
        assert!((dt - 48.0 / 96_000.0).abs() < 1e-9, "got {dt}");
    }

    #[test]
    fn mseg_phase_delta_zero_bpm_freezes() {
        // A host reporting a 0 BPM must not yield a non-finite delta; the
        // modulation simply freezes (delta 0).
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Beat;
        m.beats = 4.0;
        let dt = mseg_phase_delta(&m, 48, 0.0, 48_000.0);
        assert_eq!(dt, 0.0);
    }

    #[test]
    fn assignable_value_midline_is_the_base() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        assert!((assignable_value(0.5, 40.0, 1.0, spec, Polarity::Bipolar) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn assignable_value_depth_and_sign() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        assert_eq!(
            assignable_value(1.0, 40.0, 1.0, spec, Polarity::Bipolar),
            100.0
        );
        assert_eq!(
            assignable_value(1.0, 40.0, -1.0, spec, Polarity::Bipolar),
            0.0
        );
        assert!((assignable_value(1.0, 20.0, 0.5, spec, Polarity::Bipolar) - 70.0).abs() < 1e-4);
    }

    #[test]
    fn assignable_value_unipolar_references_base_at_zero() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        // Unipolar: value 0 leaves the parameter at base (no modulation),
        // and the curve is additive — value 1 reaches base + depth·range.
        assert!((assignable_value(0.0, 40.0, 1.0, spec, Polarity::Unipolar) - 40.0).abs() < 1e-6);
        assert!((assignable_value(0.5, 40.0, 1.0, spec, Polarity::Unipolar) - 90.0).abs() < 1e-4);
        assert_eq!(
            assignable_value(1.0, 40.0, 1.0, spec, Polarity::Unipolar),
            100.0
        );
        // Negative depth pushes the additive curve downward from base.
        assert!((assignable_value(1.0, 40.0, -0.3, spec, Polarity::Unipolar) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn assignable_value_unipolar_open_param_clamps_silently() {
        // The user's case: a log Cutoff fully open (base = max). A Unipolar
        // curve with positive depth only ever pushes up, so every sample
        // clamps at max — no audible downward sweep.
        let spec = ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: crate::effects::ParamScaling::Log,
            format: crate::effects::ParamFormat::Hertz,
        };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            let out = assignable_value(v, 20_000.0, 0.4, spec, Polarity::Unipolar);
            assert!(
                (out - 20_000.0).abs() < 1e-3,
                "v {v} -> {out}, expected max"
            );
        }
    }

    #[test]
    fn assignable_value_always_within_range() {
        let spec = ParamSpec {
            name: "p",
            min: 5.0,
            max: 9.0,
            default: 7.0,
            scaling: crate::effects::ParamScaling::Linear,
            format: crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            },
        };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for &d in &[-1.0_f32, -0.3, 0.0, 0.6, 1.0] {
                for &pol in &[Polarity::Bipolar, Polarity::Unipolar] {
                    let out = assignable_value(v, 8.0, d, spec, pol);
                    assert!((5.0..=9.0).contains(&out), "v {v} d {d} {pol:?} -> {out}");
                }
            }
        }
    }

    #[test]
    fn assignable_value_log_swings_in_norm_space() {
        // A 20..20000 Hz log param at base 2000 has norm 0.667; depth 0.4
        // means ±0.4 norm. So mseg 0 → norm 0.267 → ~126 Hz; mseg 1 → norm
        // 1.067 clamped to 1.0 → 20000 Hz. The earlier linear formula gave
        // ~20 Hz / ~9992 Hz for the same input (mostly clamped at the dark
        // end, hence the "barely audible" complaint).
        let spec = ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: crate::effects::ParamScaling::Log,
            format: crate::effects::ParamFormat::Hertz,
        };
        let lo = assignable_value(0.0, 2_000.0, 0.4, spec, Polarity::Bipolar);
        let mid = assignable_value(0.5, 2_000.0, 0.4, spec, Polarity::Bipolar);
        let hi = assignable_value(1.0, 2_000.0, 0.4, spec, Polarity::Bipolar);
        // Midline still equals the base, exactly.
        assert!((mid - 2_000.0).abs() < 1e-3, "midline {mid}");
        // Low end is audibly above the parameter floor — well above 20 Hz.
        assert!(lo > 100.0 && lo < 200.0, "log low end {lo}");
        // High end is the full max (clamped by `norm + depth > 1`).
        assert!((hi - 20_000.0).abs() < 1e-3, "log high end {hi}");
    }

    #[test]
    fn modulation_amplitude_reflects_the_amplitude_mseg() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block_silent(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        // The default amplitude MSEG is flat at 1.0.
        for r in 0..ROWS {
            assert!(
                (m.amplitude(r) - 1.0).abs() < 1e-6,
                "row {r}: {}",
                m.amplitude(r)
            );
        }
    }

    #[test]
    fn modulation_amplitude_flat_zero_mseg_silences_the_row() {
        // An amplitude MSEG flat at 0.0 yields a row gain of 0 (spec §8).
        let mut config: [TrackModulation; ROWS] =
            std::array::from_fn(TrackModulation::default_for_row);
        for tm in &mut config {
            for node in &mut tm.msegs[0].nodes {
                node.value = 0.0;
            }
        }
        let mut m = Modulation::new();
        m.set_config(&config);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block_silent(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        for r in 0..ROWS {
            assert_eq!(m.amplitude(r), 0.0, "row {r}");
        }
    }

    #[test]
    fn modulation_applies_an_assignable_mseg_to_its_effect() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        for e in &mut effects {
            e.set_sample_rate(48_000.0);
        }
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block, then drive the effects with a signal and capture output.
        m.begin_block_silent(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_first: Vec<f32> = (0..200)
            .map(|_| effects[0].process_sample(1.0, -1.0).0)
            .collect();
        // Advance many blocks so the cyclic MSEG has moved, re-apply.
        for _ in 0..400 {
            m.begin_block_silent(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        let after_later: Vec<f32> = (0..200)
            .map(|_| effects[0].process_sample(1.0, -1.0).0)
            .collect();
        // The modulated cutoff changed -> the filtered output differs.
        assert!(
            after_first != after_later,
            "an assigned MSEG should modulate the effect over time"
        );
    }

    #[test]
    fn modulation_reset_zeroes_phases() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..100 {
            m.begin_block_silent(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        m.reset();
        // After reset, every phase is back at 0.
        assert!(m.phases_all_zero());
    }

    #[test]
    fn clamp_targets_clears_out_of_range_targets() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.targets = [Some(0), Some(5), Some(3)];
        // An effect with 2 parameters: target 0 survives, targets 5 and 3
        // are out of range.
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [Some(0), None, None]);
        // A target exactly at the count is out of range.
        tm.targets = [Some(2), None, Some(1)];
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [None, None, Some(1)]);
    }

    #[test]
    fn trigger_source_default_is_free() {
        assert_eq!(TriggerSource::default(), TriggerSource::Free);
    }

    #[test]
    fn trigger_source_variants_serde_round_trip() {
        for src in [
            TriggerSource::Free,
            TriggerSource::CellLight,
            TriggerSource::CellStep,
            TriggerSource::FreeHz { hz: 2.5 },
        ] {
            let json = serde_json::to_string(&src).unwrap();
            let back: TriggerSource = serde_json::from_str(&json).unwrap();
            assert_eq!(back, src);
        }
    }

    #[test]
    fn track_modulation_with_trigger_serde_round_trips() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.trigger = TriggerSource::FreeHz { hz: 4.0 };
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.trigger, TriggerSource::FreeHz { hz: 4.0 });
    }

    #[test]
    fn track_modulation_loads_missing_trigger_as_free() {
        // A JSON shaped like a pre-Phase-3 TrackModulation (no "trigger" key)
        // deserialises with trigger = Free, per serde's additive default.
        let tm = TrackModulation::default_for_row(0);
        let json = serde_json::to_string(&tm).unwrap();
        // Strip the trigger field from the JSON to simulate the old shape.
        let stripped = strip_trigger_field(&json);
        let back: TrackModulation = serde_json::from_str(&stripped).unwrap();
        assert_eq!(back.trigger, TriggerSource::Free);
    }

    fn strip_trigger_field(json: &str) -> String {
        // Naively remove the `"trigger":<value>,` substring. Works for the
        // serde_json default representation of small enums.
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut obj = v.as_object().unwrap().clone();
        obj.remove("trigger");
        serde_json::to_string(&serde_json::Value::Object(obj)).unwrap()
    }

    #[test]
    fn fires_last_block_default_is_zero() {
        let m = Modulation::new();
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn cell_light_fires_on_each_cell_light_event() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Each block that signals a cell-light event for row 3 fires the
        // trigger; blocks with no event don't.
        m.begin_block_silent(64, 48_000.0);
        m.fire(1 << 3, 1 << 3);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
        m.begin_block_silent(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
        m.begin_block_silent(64, 48_000.0);
        m.fire(1 << 3, 1 << 3);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
        // A row that didn't get an event doesn't fire even if another did.
        m.begin_block_silent(64, 48_000.0);
        m.fire(1 << 7, 1 << 7);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn free_hz_fires_at_roughly_the_expected_rate() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 10 Hz, 48 kHz, 4800-sample blocks -> exactly 1 fire per block, on average.
        // Run 100 blocks; expect ~100 fires (allow ±1 for the boundary).
        let mut fires = 0usize;
        for _ in 0..100 {
            m.begin_block_silent(4800, 48_000.0);
            m.advance_segment(4800, 120.0, 48_000.0, &mut effects, &track_effects);
            if m.fires_last_block() & (1 << 5) != 0 {
                fires += 1;
            }
        }
        assert!(
            (99..=101).contains(&fires),
            "10 Hz over 100 blocks of 1 cycle each: got {fires} fires"
        );
    }

    #[test]
    fn free_hz_nonpositive_never_fires() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 0.0 };
        cfg[1].trigger = TriggerSource::FreeHz { hz: -2.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..50 {
            m.begin_block_silent(480, 48_000.0);
            m.advance_segment(480, 120.0, 48_000.0, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block() & 0b11, 0);
        }
    }

    #[test]
    fn fire_zeros_the_rows_three_phases() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        // Set row 2's MSEGs to short Beat lengths so the phases advance fast,
        // then verify that a fire on row 2 resets them.
        cfg[2].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Advance many blocks with no fires -> phases drift away from 0.
        for _ in 0..50 {
            m.begin_block_silent(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        // At least one of row 2's three phases should be non-zero now.
        let any_nonzero = (0..3).any(|k| m.phase_for_test(2, k) > 1e-6);
        assert!(any_nonzero, "phases should have drifted with no fires");
        // Now fire row 2 (inactive->active edge).
        m.begin_block_silent(64, 48_000.0);
        m.fire(1 << 2, 1 << 2);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        // After a fire, the row's three phases are reset to 0 (the per-MSEG
        // advance then re-runs from 0; the post-advance phase equals the
        // block's first dt, not 0). So the right test is: less than they
        // would have been without a fire — verified by comparing to the
        // amplitude (flat-1.0 default) seen one block later.
        for k in 0..3 {
            // Read each phase: it should be a *small* value (one block's dt),
            // not the multi-cycle accumulation it was before.
            let phi = m.phase_for_test(2, k);
            assert!(
                phi.abs() < 0.1,
                "after a fire, MSEG[{k}] phase should be near 0, got {phi}"
            );
        }
    }

    #[test]
    fn switch_effect_kind_resets_params_and_clamps_targets() {
        use crate::effects::{EffectKind, TrackEffect};

        // A Bitcrush track (2 params) whose first assignable MSEG targets
        // parameter index 1.
        let mut effect = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: crate::effects::default_params_for_kind(EffectKind::Bitcrush),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        let mut modulation = TrackModulation::default_for_row(0);
        modulation.targets[0] = Some(1);

        // Switch to None (0 parameters).
        switch_effect_kind(&mut effect, &mut modulation, EffectKind::None);

        assert_eq!(effect.kind, EffectKind::None, "kind switched");
        assert_eq!(
            effect.params,
            crate::effects::default_params_for_kind(EffectKind::None),
            "params reset to the new kind's defaults"
        );
        assert_eq!(
            modulation.targets[0], None,
            "out-of-range target cleared — None has 0 params"
        );
    }

    #[test]
    fn switch_effect_kind_keeps_an_in_range_target() {
        use crate::effects::{EffectKind, TrackEffect};

        // Switching between two kinds that both have parameter index 0.
        let mut effect = TrackEffect {
            kind: EffectKind::Svf,
            params: crate::effects::default_params_for_kind(EffectKind::Svf),
            mix: 1.0,
            muted: false,
            soloed: false,
        };
        let mut modulation = TrackModulation::default_for_row(0);
        modulation.targets[0] = Some(0);

        switch_effect_kind(&mut effect, &mut modulation, EffectKind::Bitcrush);

        assert_eq!(effect.kind, EffectKind::Bitcrush);
        assert_eq!(
            modulation.targets[0],
            Some(0),
            "index 0 is in range for Bitcrush — target preserved"
        );
    }

    #[test]
    fn free_source_does_not_fire() {
        let mut m = Modulation::new();
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        // default_for_row's trigger is Free.
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..20 {
            m.begin_block_silent(64, 48_000.0);
            m.fire(0xFFFF, 0xFFFF);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block(), 0);
        }
    }

    #[test]
    fn reset_zeroes_hz_phases() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block so hz_phases advances above 0.
        m.begin_block_silent(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        m.reset();
        assert!(m.hz_phases_all_zero());
    }

    #[test]
    fn begin_block_zeroes_fires_and_decides_free_hz() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        // 10 Hz, 48 kHz, a 4800-sample block = exactly one cycle → one fire.
        m.begin_block_silent(4800, 48_000.0);
        assert_eq!(m.fires_last_block(), 1 << 5);
        // begin_block zeroes `fires` each call: a block with no wrap clears it.
        m.begin_block_silent(64, 48_000.0);
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn fire_resets_cell_light_rows_and_ignores_other_triggers() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[2].trigger = TriggerSource::CellLight;
        cfg[3].trigger = TriggerSource::Free;
        cfg[4].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift the per-MSEG-clock rows' phases away from 0.
        for _ in 0..50 {
            m.begin_block_silent(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(3, 0) > 1e-6, "Free row drifted");
        // Fire rows 2, 3, 4 — only the CellLight row (2) resets and reports.
        m.begin_block_silent(64, 48_000.0);
        m.fire(
            (1 << 2) | (1 << 3) | (1 << 4),
            (1 << 2) | (1 << 3) | (1 << 4),
        );
        assert_eq!(
            m.fires_last_block() & (1 << 2),
            1 << 2,
            "CellLight row fired"
        );
        assert_eq!(m.fires_last_block() & (1 << 3), 0, "Free row did not fire");
        assert_eq!(m.phase_for_test(2, 0), 0.0, "CellLight row phases reset");
        assert!(
            m.phase_for_test(3, 0) > 1e-6,
            "fire must not touch a Free row's phase"
        );
    }

    #[test]
    fn advance_segment_advances_free_hz_rows_at_their_own_clock() {
        // FreeHz is a trigger source — between fires, the row's MSEGs advance
        // at each MSEG's own sync/length (not at the Hz rate). Same as Free /
        // CellLight / CellStep — the trigger only resets phase, not the
        // ongoing clock.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 5.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 64-sample block at 5 Hz / 48 kHz → hz_phase += 64*5/48000 ≈ 0.0067,
        // no wrap → no fire, no phase reset; the MSEG should advance per its
        // own clock during advance_segment.
        m.begin_block_silent(64, 48_000.0);
        let before = m.phase_for_test(0, 0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert!(
            m.phase_for_test(0, 0) > before,
            "FreeHz row's MSEG advances per its own clock between fires"
        );
    }

    #[test]
    fn free_hz_wrap_resets_the_rows_phases_to_zero() {
        // A FreeHz wrap (every 1/hz seconds) is a trigger event — it fires
        // the row exactly like a CellLight or CellStep fire would.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift phase well away from 0 over many no-wrap blocks.
        for _ in 0..30 {
            m.begin_block_silent(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(3, 0) > 1e-6, "row 3 drifted");
        // 10 Hz, 4800-sample block @ 48 kHz = exactly one cycle → one wrap.
        m.begin_block_silent(4800, 48_000.0);
        assert_eq!(m.fires_last_block() & (1 << 3), 1 << 3, "wrap fires");
        assert_eq!(m.phase_for_test(3, 0), 0.0, "wrap resets the row's phases");
    }

    #[test]
    fn advance_segment_zero_length_is_a_noop() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block_silent(64, 48_000.0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let phase = m.phase_for_test(3, 1);
        m.advance_segment(0, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(
            m.phase_for_test(3, 1),
            phase,
            "a zero-length segment must not advance phases"
        );
    }

    #[test]
    fn fire_resets_cell_step_rows_on_every_active_step() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[6].trigger = TriggerSource::CellStep;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift row 6's phases away from 0.
        for _ in 0..50 {
            m.begin_block_silent(64, 48_000.0);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(6, 0) > 1e-6, "row 6 drifted");
        // A step where row 6 is active (in `active_rows`) but NOT newly-lit
        // (absent from `newly_rows`) still fires CellStep — the case CellLight
        // skips.
        m.begin_block_silent(64, 48_000.0);
        m.fire(0, 1 << 6);
        assert_eq!(
            m.phase_for_test(6, 0),
            0.0,
            "CellStep fires on an active, non-newly step"
        );
        assert_eq!(
            m.fires_last_block() & (1 << 6),
            1 << 6,
            "CellStep sets its fires bit"
        );
        // It advances after the reset, then fires again on the next active step.
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert!(
            m.phase_for_test(6, 0) > 1e-6,
            "row 6 advanced after the reset"
        );
        m.fire(0, 1 << 6);
        assert_eq!(
            m.phase_for_test(6, 0),
            0.0,
            "CellStep fires again on the next consecutive active step"
        );
        // A step where row 6 is NOT active does not fire.
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let drifted = m.phase_for_test(6, 0);
        assert!(drifted > 1e-6);
        m.fire(0, 0);
        assert_eq!(
            m.phase_for_test(6, 0),
            drifted,
            "no fire on a step where the row is inactive"
        );
    }

    #[test]
    fn advance_segment_in_two_halves_around_a_fire_resets_at_the_split() {
        // After a `fire`, the next `advance_segment` advances from the reset
        // phase 0 — so splitting a 256-sample block as [100][fire][156] leaves
        // a CellLight row's phase equal to a from-0 advance over only the
        // 156-sample tail. This is what lets the engine (Task 2) place a reset
        // at an exact step boundary.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[1].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Svf));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift row 1's phases well away from 0.
        for _ in 0..30 {
            m.begin_block_silent(256, 48_000.0);
            m.advance_segment(256, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        // The block: a 100-sample segment, fire row 1, then a 156-sample segment.
        m.begin_block_silent(256, 48_000.0);
        m.advance_segment(100, 120.0, 48_000.0, &mut effects, &track_effects);
        m.fire(1 << 1, 1 << 1);
        m.advance_segment(156, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_fire = m.phase_for_test(1, 1);
        // Expected: a from-0 advance over just the 156-sample tail.
        let mseg = cfg[1].msegs[1];
        let dt = mseg_phase_delta(&mseg, 156, 120.0, 48_000.0);
        let (expected, _) = advance(&mseg, 0.0, dt, false);
        assert!(
            (after_fire - expected).abs() < 1e-6,
            "post-fire phase {after_fire} should equal a from-0 tail advance {expected}"
        );
    }

    #[test]
    fn swap_rows_exchanges_config_and_runtime_state() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::FreeHz { hz: 2.0 };
        cfg[7].trigger = TriggerSource::CellStep;
        m.set_config(&cfg);
        m.begin_block_silent(1200, 48_000.0);
        let mut effects = std::array::from_fn(|_| EffectInstance::new(EffectKind::None));
        let tracks = std::array::from_fn(TrackEffect::default_for_row);
        m.advance_segment(1200, 120.0, 48_000.0, &mut effects, &tracks);
        m.fire(0, 1 << 7);
        let before_3 = m.states[3].clone();
        let before_7 = m.states[7].clone();
        assert_ne!(before_3, before_7);
        m.swap_rows(3, 7);
        assert_eq!(m.config_for_test(3), &cfg[7]);
        assert_eq!(m.config_for_test(7), &cfg[3]);
        assert_eq!(m.states[3], before_7);
        assert_eq!(m.states[7], before_3);
    }

    #[test]
    fn transient_detector_fires_on_a_step_input_after_a_steady_warmup() {
        // The warmup tone is long enough (≥ slow envelope settling time + one
        // refractory window) that any rising-edge fire from silence-→-tone
        // has expired before the step. Then the step up to 0.9 produces a
        // fast/slow ratio well above the 1.5 threshold.
        let mut det = TransientDetector::default();
        let sr = 48_000.0_f32;
        for _ in 0..20_000 {
            det.process_sample(0.05, 1.5, 50.0, sr);
        }
        let mut fired = false;
        for _ in 0..1024 {
            if det.process_sample(0.9, 1.5, 50.0, sr) {
                fired = true;
                break;
            }
        }
        assert!(fired, "step input above the noise floor must trigger");
    }

    #[test]
    fn transient_detector_holds_off_during_refractory_window() {
        // After firing once, the detector must not fire again until the
        // refractory window (50 ms) elapses, even while the input stays high.
        let mut det = TransientDetector::default();
        let sr = 48_000.0_f32;
        for _ in 0..20_000 {
            det.process_sample(0.05, 1.5, 50.0, sr);
        }
        let mut first = None;
        for i in 0..2048 {
            if det.process_sample(0.9, 1.5, 50.0, sr) {
                first = Some(i);
                break;
            }
        }
        assert!(first.is_some(), "the first transient must fire");
        // Within the hold window (50 ms ≈ 2400 samples), no further fires.
        let mut re_fired = false;
        for _ in 0..2000 {
            if det.process_sample(0.9, 1.5, 50.0, sr) {
                re_fired = true;
                break;
            }
        }
        assert!(
            !re_fired,
            "refractory window must swallow a sustained high signal"
        );
    }

    #[test]
    fn transient_detector_does_not_fire_on_steady_dc_or_silence() {
        let mut det = TransientDetector::default();
        let sr = 48_000.0_f32;
        // Silence: no envelope motion at all.
        for _ in 0..2400 {
            assert!(!det.process_sample(0.0, 1.5, 50.0, sr));
        }
        // Steady DC: the fast and slow envelopes track each other; ratio ≈ 1.
        let mut det = TransientDetector::default();
        for _ in 0..2400 {
            det.process_sample(0.5, 1.5, 50.0, sr);
        }
        let mut fired = false;
        for _ in 0..2400 {
            if det.process_sample(0.5, 1.5, 50.0, sr) {
                fired = true;
                break;
            }
        }
        assert!(!fired, "steady-state DC must not produce a transient fire");
    }

    #[test]
    fn modulation_transient_trigger_fires_and_resets_phases_on_a_block() {
        // A row configured for `Transient`: a long warm-up settles the
        // envelopes and lets the rising-edge refractory from silence-→-tone
        // expire, then a block containing a clear click fires the row and
        // zeros its three MSEG phases.
        let mut m = Modulation::new();
        let mut cfg: [TrackModulation; ROWS] =
            std::array::from_fn(TrackModulation::default_for_row);
        cfg[2].trigger = TriggerSource::Transient {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        m.set_config(&cfg);
        let sr = 48_000.0_f64;
        // 4 warm-up blocks of 5120 samples each (~425 ms) — past the slow
        // envelope's settling time and the refractory window.
        let warm = vec![0.05_f32; 5120];
        for _ in 0..4 {
            m.begin_block(&warm, &warm, sr);
        }
        // Force non-zero phases so the fire-reset is observable.
        let mut effects = std::array::from_fn(|_| EffectInstance::new(EffectKind::None));
        let tracks = std::array::from_fn(TrackEffect::default_for_row);
        m.advance_segment(2400, 120.0, sr, &mut effects, &tracks);
        assert!(m.phase(2, 0) > 0.0);
        // Hit block: a sharp click in the middle.
        let mut left = vec![0.05_f32; 1024];
        let mut right = vec![0.05_f32; 1024];
        for i in 500..510 {
            left[i] = 0.95;
            right[i] = 0.95;
        }
        m.begin_block(&left, &right, sr);
        assert_ne!(
            m.fires_last_block() & (1 << 2),
            0,
            "Transient row must fire on a clear onset"
        );
        // Phases reset to 0 on fire.
        assert_eq!(m.phase_for_test(2, 0), 0.0);
        assert_eq!(m.phase_for_test(2, 1), 0.0);
        assert_eq!(m.phase_for_test(2, 2), 0.0);
    }

    #[test]
    fn swap_rows_is_a_noop_for_self_or_out_of_range_indices() {
        let mut m = Modulation::new();
        m.fire(0, 1 << 4);
        let before = m.states[4].clone();
        m.swap_rows(4, 4);
        assert_eq!(m.states[4], before);
        m.swap_rows(4, ROWS + 5);
        assert_eq!(m.states[4], before);
        m.swap_rows(ROWS + 5, 4);
        assert_eq!(m.states[4], before);
    }
}
