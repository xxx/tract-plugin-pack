//! The modulation engine — Phase 2 Milestone 2b. Three MSEGs per track row
//! (one amplitude + two assignable), free-running on their own clocks,
//! driving the 2a effect-parameter and amplitude seams.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
use crate::effects::ParamSpec;

/// The number of track rows. Matches `crate::grid::ROWS`.
#[allow(dead_code)]
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
        let spec = ParamSpec { name: "p", min: 0.0, max: 100.0, default: 50.0 };
        assert!((assignable_value(0.5, 40.0, 1.0, spec) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn assignable_value_depth_and_sign() {
        let spec = ParamSpec { name: "p", min: 0.0, max: 100.0, default: 50.0 };
        assert_eq!(assignable_value(1.0, 40.0, 1.0, spec), 100.0);
        assert_eq!(assignable_value(1.0, 40.0, -1.0, spec), 0.0);
        assert!((assignable_value(1.0, 20.0, 0.5, spec) - 70.0).abs() < 1e-4);
    }

    #[test]
    fn assignable_value_always_within_range() {
        let spec = ParamSpec { name: "p", min: 5.0, max: 9.0, default: 7.0 };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for &d in &[-1.0_f32, -0.3, 0.0, 0.6, 1.0] {
                let out = assignable_value(v, 8.0, d, spec);
                assert!((5.0..=9.0).contains(&out), "v {v} d {d} -> {out}");
            }
        }
    }
}
