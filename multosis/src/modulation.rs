//! The modulation engine — Phase 2 Milestone 2b. Three MSEGs per track row
//! (one amplitude + two assignable), free-running on their own clocks,
//! driving the 2a effect-parameter and amplitude seams.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

use crate::effects::{Effect, EffectInstance, ParamSpec, TrackEffect};
use tiny_skia_widgets::{advance, value_at_phase, MsegData, PlayMode, SyncMode};

/// The number of track rows. Matches `crate::grid::ROWS`.
const ROWS: usize = 16;

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
}

impl TrackModulation {
    /// The default modulation for track row `row`. The amplitude MSEG is flat
    /// at 1.0 (no level change); `msegs[1]` is a cyclic triangle assigned to
    /// effect parameter 0, its loop length spread by row so each track drifts
    /// at its own rate; `msegs[2]` is an unused cyclic default.
    pub fn default_for_row(row: usize) -> Self {
        // msegs[0] — amplitude: flat at 1.0.
        let mut amplitude = MsegData::default();
        amplitude.nodes[0].value = 1.0;
        amplitude.nodes[1].value = 1.0;
        amplitude.play_mode = PlayMode::Cyclic;

        // msegs[1] — assignable: a cyclic triangle, Beat-synced, length by row.
        let mut sweep = MsegData::default(); // nodes (0,0) and (1,1)
        let _ = sweep.insert_node(0.5, 1.0); // -> (0,0) (0.5,1.0) (1,1.0)
        sweep.move_node(2, 1.0, 0.0); // last node value -> 0: triangle
        sweep.play_mode = PlayMode::Cyclic;
        sweep.sync_mode = SyncMode::Beat;
        sweep.beats = 4.0 + row as f32 * 2.0; // 4..34 beats across the rows

        // msegs[2] — assignable: unused default.
        let spare = MsegData {
            play_mode: PlayMode::Cyclic,
            ..MsegData::default()
        };

        TrackModulation {
            msegs: [amplitude, sweep, spare],
            targets: [Some(0), None],
            depths: [0.4, 0.0],
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
/// tempo. Returns 0.0 for a degenerate (zero/negative) length.
pub fn mseg_phase_delta(mseg: &MsegData, block_len: usize, bpm: f64, sample_rate: f64) -> f32 {
    let length_samples = match mseg.sync_mode {
        SyncMode::Time => mseg.time_seconds as f64 * sample_rate,
        SyncMode::Beat => mseg.beats as f64 * (60.0 / bpm) * sample_rate,
    };
    if length_samples > 0.0 {
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
pub fn assignable_value(mseg_value: f32, base: f32, depth: f32, spec: ParamSpec) -> f32 {
    let bipolar = mseg_value * 2.0 - 1.0;
    let deviation = bipolar * depth * (spec.max - spec.min);
    (base + deviation).clamp(spec.min, spec.max)
}

/// The modulation runtime owned by the audio engine — the per-track config,
/// each MSEG's free-running phase, and the latest per-row amplitude gain.
pub struct Modulation {
    config: [TrackModulation; ROWS],
    /// Free-running phase per `[row][mseg]`.
    phases: [[f32; 3]; ROWS],
    /// Latest per-row amplitude gain, set by `update_block`.
    amplitudes: [f32; ROWS],
}

impl Modulation {
    /// A runtime with the default per-row modulation and zeroed phases.
    pub fn new() -> Self {
        Self {
            config: std::array::from_fn(TrackModulation::default_for_row),
            phases: [[0.0; 3]; ROWS],
            amplitudes: [1.0; ROWS],
        }
    }

    /// Replace the per-track modulation config (bridged from persisted state
    /// at init — off the audio thread).
    pub fn set_config(&mut self, config: &[TrackModulation; ROWS]) {
        self.config = config.clone();
    }

    /// Reset every MSEG phase to 0.
    pub fn reset(&mut self) {
        self.phases = [[0.0; 3]; ROWS];
    }

    /// The latest amplitude gain for `row` (set by the previous `update_block`).
    pub fn amplitude(&self, row: usize) -> f32 {
        self.amplitudes[row]
    }

    /// Advance every MSEG one process block, evaluate it, and apply: the
    /// amplitude MSEG sets `amplitudes[row]`; each assigned assignable MSEG
    /// writes its target effect parameter via `set_param`. Allocation-free.
    pub fn update_block(
        &mut self,
        block_len: usize,
        bpm: f64,
        sample_rate: f64,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        for row in 0..ROWS {
            for k in 0..3 {
                // `MsegData` is `Copy`; copy the needed config out so the
                // immutable `self.config` borrow does not span the
                // `self.phases` / `self.amplitudes` writes below.
                let mseg = self.config[row].msegs[k];
                let dt = mseg_phase_delta(&mseg, block_len, bpm, sample_rate);
                let (next, _finished) = advance(&mseg, self.phases[row][k], dt, false);
                self.phases[row][k] = next;
                let value = value_at_phase(&mseg, next);
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
    fn assignable_value_midline_is_the_base() {
        let spec = ParamSpec {
            name: "p",
            min: 0.0,
            max: 100.0,
            default: 50.0,
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
        };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for &d in &[-1.0_f32, -0.3, 0.0, 0.6, 1.0] {
                let out = assignable_value(v, 8.0, d, spec);
                assert!((5.0..=9.0).contains(&out), "v {v} d {d} -> {out}");
            }
        }
    }

    #[test]
    fn modulation_amplitude_reflects_the_amplitude_mseg() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
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
    fn modulation_applies_an_assignable_mseg_to_its_effect() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        for e in &mut effects {
            e.set_sample_rate(48_000.0);
        }
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block, then drive the effects with a signal and capture output.
        m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_first: Vec<f32> = (0..200)
            .map(|_| effects[0].process_sample(1.0, -1.0).0)
            .collect();
        // Advance many blocks so the cyclic MSEG has moved, re-apply.
        for _ in 0..400 {
            m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
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
            m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        m.reset();
        // After reset, every phase is back at 0.
        assert!(m.phases_all_zero());
    }
}
