//! The modulation engine — Phase 2 Milestone 2b. Three MSEGs per track row
//! (one amplitude + two assignable), free-running on their own clocks,
//! driving the 2a effect-parameter and amplitude seams.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};

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
}
