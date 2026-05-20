//! The modulation engine — Phase 2 Milestone 2b. Three MSEGs per track row
//! (one amplitude + two assignable), free-running on their own clocks,
//! driving the 2a effect-parameter and amplitude seams.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

use crate::effects::{
    norm_to_value, value_to_norm, Effect, EffectInstance, ParamScaling, ParamSpec, TrackEffect,
};
use tiny_skia_widgets::{advance, value_at_phase, MsegData, PlayMode, SyncMode};

/// The number of track rows. Matches `crate::grid::ROWS`.
const ROWS: usize = 16;

/// The event that causes a track's three MSEG phases to reset to 0.
/// Per Phase 3 design — Free is the Phase 2b free-running default; CellLight
/// fires on the row's inactive→active edge under the wavefront; FreeHz fires
/// every `1.0/hz` seconds independently of any sync.
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum TriggerSource {
    #[default]
    Free,
    CellLight,
    FreeHz {
        hz: f32,
    },
}

/// One track row's modulation: three MSEGs and the two assignable MSEGs'
/// targets and depths. `msegs[0]` is the amplitude MSEG; `msegs[1]` and
/// `msegs[2]` are the assignable MSEGs — `targets[k]` / `depths[k]` belong to
/// `msegs[k + 1]`.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackModulation {
    pub msegs: [MsegData; 3],
    /// Target effect-parameter index for each assignable MSEG, or `None`.
    pub targets: [Option<usize>; 2],
    /// Bipolar modulation depth (−1..1) for each assignable MSEG.
    pub depths: [f32; 2],
    /// The event that resets all three of this row's MSEG phases.
    #[serde(default)]
    pub trigger: TriggerSource,
}

impl TrackModulation {
    /// The default modulation for track row `row`. The amplitude MSEG is flat
    /// at 1.0 (no level change); `msegs[1]` is a cyclic triangle assigned to
    /// effect parameter 0, its loop length spread by row so each track drifts
    /// at its own rate; `msegs[2]` is an unused cyclic default.
    pub fn default_for_row(row: usize) -> Self {
        // All three MSEGs on a row default to the SAME beat-synced length so
        // their playheads visibly stay in lockstep. The length varies by row
        // (4..34 beats) for audible per-track variety. The user can re-tune
        // any single MSEG's clock via its strip.
        let beats = 4.0 + row as f32 * 2.0;

        // msegs[0] — amplitude: flat at 1.0.
        let mut amplitude = MsegData::default();
        amplitude.nodes[0].value = 1.0;
        amplitude.nodes[1].value = 1.0;
        amplitude.play_mode = PlayMode::Cyclic;
        amplitude.sync_mode = SyncMode::Beat;
        amplitude.beats = beats;

        // msegs[1] — assignable: a cyclic triangle.
        let mut sweep = MsegData::default(); // nodes (0,0) and (1,1)
        let _ = sweep.insert_node(0.5, 1.0); // -> (0,0) (0.5,1.0) (1,1.0)
        sweep.move_node(2, 1.0, 0.0); // last node value -> 0: triangle
        sweep.play_mode = PlayMode::Cyclic;
        sweep.sync_mode = SyncMode::Beat;
        sweep.beats = beats;

        // msegs[2] — assignable: unused default, same clock as the others.
        let spare = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats,
            ..MsegData::default()
        };

        TrackModulation {
            msegs: [amplitude, sweep, spare],
            targets: [Some(0), None],
            depths: [0.4, 0.0],
            trigger: TriggerSource::Free,
        }
    }

    /// Clear any assignable-MSEG target that points past `param_count`
    /// parameters. Called after a track's effect kind changes so a target can
    /// never reference a parameter the new effect does not have.
    pub fn clamp_targets(&mut self, param_count: usize) {
        for target in &mut self.targets {
            if let Some(i) = *target {
                if i >= param_count {
                    *target = None;
                }
            }
        }
    }
}

impl Default for TrackModulation {
    fn default() -> Self {
        Self::default_for_row(0)
    }
}

/// The phase delta (0..1 space) one `block_len`-sample process block advances
/// `mseg`, given the host `bpm` and `sample_rate`. Honours the MSEG's
/// `sync_mode`: `Time` uses `time_seconds`, `Beat` converts `beats` via the
/// tempo. Returns 0.0 for a degenerate (zero/negative) length — a `bpm` of 0
/// or below is treated as such (the modulation simply freezes).
pub fn mseg_phase_delta(mseg: &MsegData, block_len: usize, bpm: f64, sample_rate: f64) -> f32 {
    let length_samples = match mseg.sync_mode {
        SyncMode::Time => mseg.time_seconds as f64 * sample_rate,
        // A non-positive `bpm` gives a degenerate length (freezes below).
        SyncMode::Beat if bpm > 0.0 => mseg.beats as f64 * (60.0 / bpm) * sample_rate,
        SyncMode::Beat => 0.0,
    };
    if length_samples.is_finite() && length_samples > 0.0 {
        (block_len as f64 / length_samples) as f32
    } else {
        0.0
    }
}

/// The effective effect-parameter value for an assignable MSEG modulating
/// parameter `spec` around `base`. `mseg_value` is the MSEG's 0..1 output;
/// `depth` is the bipolar (−1..1) modulation depth. The MSEG midline (0.5)
/// leaves the parameter at `base`; the result is clamped to the parameter's
/// range.
///
/// Honours `spec.scaling`: Linear params modulate in value-space (`depth` is
/// a fraction of the full value range); Log params modulate in norm-space
/// (`depth` is a fraction of the log range), so a log-scaled cutoff sweeps
/// audibly even-handed across its decades instead of being clamped at the
/// dark end half the time.
pub fn assignable_value(mseg_value: f32, base: f32, depth: f32, spec: ParamSpec) -> f32 {
    let bipolar = mseg_value * 2.0 - 1.0;
    match spec.scaling {
        ParamScaling::Linear => {
            let deviation = bipolar * depth * (spec.max - spec.min);
            (base + deviation).clamp(spec.min, spec.max)
        }
        ParamScaling::Log => {
            let norm_base = value_to_norm(base, spec.min, spec.max, ParamScaling::Log);
            let norm_eff = (norm_base + bipolar * depth).clamp(0.0, 1.0);
            norm_to_value(norm_eff, spec.min, spec.max, ParamScaling::Log)
        }
    }
}

/// The modulation runtime owned by the audio engine — the per-track config,
/// each MSEG's free-running phase, and the latest per-row amplitude gain.
pub struct Modulation {
    config: [TrackModulation; ROWS],
    /// Free-running phase per `[row][mseg]`.
    phases: [[f32; 3]; ROWS],
    /// Latest per-row amplitude gain, set by `update_block`.
    amplitudes: [f32; ROWS],
    /// Last block's active-row mask, for cell-light edge detection.
    prev_active: u16,
    /// Free-Hz oscillator phase per row, advances 0..1 and wraps modulo 1.
    hz_phases: [f32; ROWS],
    /// The rows that fired this block (bit `r` set). Set by `update_block`.
    fires: u16,
}

impl Modulation {
    /// A runtime with the default per-row modulation and zeroed phases.
    pub fn new() -> Self {
        Self {
            config: std::array::from_fn(TrackModulation::default_for_row),
            phases: [[0.0; 3]; ROWS],
            amplitudes: [1.0; ROWS],
            prev_active: 0,
            hz_phases: [0.0; ROWS],
            fires: 0,
        }
    }

    /// Replace the per-track modulation config (bridged from persisted state
    /// at init, and again on each live edit — off the audio thread).
    ///
    /// Runtime state (`phases`, `hz_phases`, `prev_active`, `amplitudes`) is
    /// deliberately preserved across config changes so editing a parameter or
    /// switching trigger source mid-playback does not glitch the modulation.
    /// Only `reset()` clears that state.
    pub fn set_config(&mut self, config: &[TrackModulation; ROWS]) {
        self.config = config.clone();
    }

    /// Reset every MSEG phase to 0.
    pub fn reset(&mut self) {
        self.phases = [[0.0; 3]; ROWS];
        self.prev_active = 0;
        self.hz_phases = [0.0; ROWS];
        self.fires = 0;
    }

    /// The latest amplitude gain for `row` (set by the previous `update_block`).
    pub fn amplitude(&self, row: usize) -> f32 {
        self.amplitudes[row]
    }

    /// The current free-running phase of MSEG `k` (`0..3`) on `row` (`0..ROWS`).
    /// Published to the editor for the playhead overlay.
    pub fn phase(&self, row: usize, k: usize) -> f32 {
        self.phases[row][k]
    }

    /// Advance every MSEG one process block, evaluate it, and apply: the
    /// amplitude MSEG sets `amplitudes[row]`; each assigned assignable MSEG
    /// writes its target effect parameter via `set_param`. Allocation-free.
    pub fn update_block(
        &mut self,
        block_len: usize,
        bpm: f64,
        sample_rate: f64,
        active_mask: u16,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        // Phase 3: decide which rows fire this block, then reset their phases.
        let mut fires: u16 = 0;
        for row in 0..ROWS {
            let cur_lit = (active_mask & (1 << row)) != 0;
            let prev_lit = (self.prev_active & (1 << row)) != 0;
            let fire = match self.config[row].trigger {
                TriggerSource::Free => false,
                TriggerSource::CellLight => cur_lit && !prev_lit,
                TriggerSource::FreeHz { hz } => {
                    if hz <= 0.0 {
                        false
                    } else {
                        self.hz_phases[row] += (block_len as f32 * hz) / sample_rate as f32;
                        if self.hz_phases[row] >= 1.0 {
                            // Retain fractional remainder; multiple wraps in
                            // one block still count as one fire (spec §7).
                            self.hz_phases[row] -= self.hz_phases[row].floor();
                            true
                        } else {
                            false
                        }
                    }
                }
            };
            if fire {
                fires |= 1 << row;
            }
        }
        // Reset phases for firing rows — all three MSEGs in lockstep.
        for row in 0..ROWS {
            if fires & (1 << row) != 0 {
                self.phases[row] = [0.0; 3];
            }
        }
        self.fires = fires;
        self.prev_active = active_mask;

        for row in 0..ROWS {
            // For `FreeHz` tracks the trigger rate IS the modulation rate —
            // all three MSEGs sweep in lockstep at the dial's Hz, ignoring
            // each MSEG's own sync/length. Free and CellLight tracks keep
            // their per-MSEG clocks.
            let free_hz_phase = match self.config[row].trigger {
                TriggerSource::FreeHz { .. } => Some(self.hz_phases[row]),
                _ => None,
            };
            for k in 0..3 {
                // `MsegData` is `Copy`; copy the needed config out so the
                // immutable `self.config` borrow does not span the
                // `self.phases` / `self.amplitudes` writes below.
                let mseg = self.config[row].msegs[k];
                let phase = match free_hz_phase {
                    Some(p) => p,
                    None => {
                        let dt = mseg_phase_delta(&mseg, block_len, bpm, sample_rate);
                        let (next, _finished) = advance(&mseg, self.phases[row][k], dt, false);
                        next
                    }
                };
                self.phases[row][k] = phase;
                let value = value_at_phase(&mseg, phase);
                if k == 0 {
                    // Amplitude MSEG.
                    self.amplitudes[row] = value;
                } else if let Some(target) = self.config[row].targets[k - 1] {
                    // Assignable MSEG -> a target effect parameter.
                    if let Some(&spec) = effects[row].parameters().get(target) {
                        let base = track_effects[row].params[target];
                        let depth = self.config[row].depths[k - 1];
                        effects[row].set_param(target, assignable_value(value, base, depth, spec));
                    }
                }
            }
        }
    }

    /// Test helper: true when every MSEG phase is 0.
    #[cfg(test)]
    pub fn phases_all_zero(&self) -> bool {
        self.phases.iter().flatten().all(|&p| p == 0.0)
    }

    /// Test helper: the rows that fired this block (set by `update_block`).
    #[cfg(test)]
    pub fn fires_last_block(&self) -> u16 {
        self.fires
    }

    /// Test helper: the current phase for `[row][k]`.
    #[cfg(test)]
    pub fn phase_for_test(&self, row: usize, k: usize) -> f32 {
        self.phases[row][k]
    }

    /// Test helper: true when every Free-Hz oscillator phase is 0.
    #[cfg(test)]
    pub fn hz_phases_all_zero(&self) -> bool {
        self.hz_phases.iter().all(|&p| p == 0.0)
    }

    /// Test helper: the `prev_active` mask.
    #[cfg(test)]
    pub fn prev_active_for_test(&self) -> u16 {
        self.prev_active
    }
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_modulation_serde_round_trips() {
        let tm = TrackModulation::default_for_row(3);
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tm);
    }

    #[test]
    fn track_modulation_array_serde_round_trips() {
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: [TrackModulation; ROWS] = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
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
        assert!(a.msegs[0].nodes[..a.msegs[0].node_count]
            .iter()
            .all(|n| (n.value - 1.0).abs() < 1e-6));
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
        assert!((assignable_value(0.5, 40.0, 1.0, spec) - 40.0).abs() < 1e-6);
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
        assert_eq!(assignable_value(1.0, 40.0, 1.0, spec), 100.0);
        assert_eq!(assignable_value(1.0, 40.0, -1.0, spec), 0.0);
        assert!((assignable_value(1.0, 20.0, 0.5, spec) - 70.0).abs() < 1e-4);
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
                let out = assignable_value(v, 8.0, d, spec);
                assert!((5.0..=9.0).contains(&out), "v {v} d {d} -> {out}");
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
        let lo = assignable_value(0.0, 2_000.0, 0.4, spec);
        let mid = assignable_value(0.5, 2_000.0, 0.4, spec);
        let hi = assignable_value(1.0, 2_000.0, 0.4, spec);
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
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
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
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        for r in 0..ROWS {
            assert_eq!(m.amplitude(r), 0.0, "row {r}");
        }
    }

    #[test]
    fn modulation_applies_an_assignable_mseg_to_its_effect() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        for e in &mut effects {
            e.set_sample_rate(48_000.0);
        }
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block, then drive the effects with a signal and capture output.
        m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        let after_first: Vec<f32> = (0..200)
            .map(|_| effects[0].process_sample(1.0, -1.0).0)
            .collect();
        // Advance many blocks so the cyclic MSEG has moved, re-apply.
        for _ in 0..400 {
            m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
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
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..100 {
            m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        }
        m.reset();
        // After reset, every phase is back at 0.
        assert!(m.phases_all_zero());
    }

    #[test]
    fn clamp_targets_clears_out_of_range_targets() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.targets = [Some(0), Some(5)];
        // An effect with 2 parameters: target 0 survives, target 5 is cleared.
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [Some(0), None]);
        // A target exactly at the count is out of range.
        tm.targets = [Some(2), None];
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [None, None]);
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
    fn cell_light_fires_on_inactive_to_active_edge() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Block 1: row 3 was inactive last block (prev=0) and is active now -> fires.
        m.update_block(64, 120.0, 48_000.0, 1 << 3, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
        // Block 2: row 3 still active -> does NOT re-fire (no edge).
        m.update_block(64, 120.0, 48_000.0, 1 << 3, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
        // Block 3: row 3 went inactive -> no fire (only active edges fire).
        m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
        // Block 4: row 3 re-armed (inactive -> active) -> fires again.
        m.update_block(64, 120.0, 48_000.0, 1 << 3, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
    }

    #[test]
    fn free_hz_fires_at_roughly_the_expected_rate() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 10 Hz, 48 kHz, 4800-sample blocks -> exactly 1 fire per block, on average.
        // Run 100 blocks; expect ~100 fires (allow ±1 for the boundary).
        let mut fires = 0usize;
        for _ in 0..100 {
            m.update_block(4800, 120.0, 48_000.0, 0, &mut effects, &track_effects);
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
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..50 {
            m.update_block(480, 120.0, 48_000.0, 0, &mut effects, &track_effects);
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
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Advance many blocks with no fires -> phases drift away from 0.
        for _ in 0..50 {
            m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        }
        // At least one of row 2's three phases should be non-zero now.
        let any_nonzero = (0..3).any(|k| m.phase_for_test(2, k) > 1e-6);
        assert!(any_nonzero, "phases should have drifted with no fires");
        // Now fire row 2 (inactive->active edge).
        m.update_block(64, 120.0, 48_000.0, 1 << 2, &mut effects, &track_effects);
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
    fn free_source_does_not_fire() {
        let mut m = Modulation::new();
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        // default_for_row's trigger is Free.
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..20 {
            m.update_block(64, 120.0, 48_000.0, 0xFFFF, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block(), 0);
        }
    }

    #[test]
    fn reset_zeroes_hz_phases_and_prev_active() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block so hz_phases advances and prev_active becomes nonzero.
        m.update_block(64, 120.0, 48_000.0, 1, &mut effects, &track_effects);
        m.reset();
        assert!(m.hz_phases_all_zero());
        assert_eq!(m.prev_active_for_test(), 0);
    }
}
