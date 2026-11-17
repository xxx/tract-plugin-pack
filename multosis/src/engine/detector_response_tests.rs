use super::*;
use crate::effects::{EffectKind, default_params_for_kind};
use crate::grid::COLS;
use tract_effects::modulation::{TransientAlgorithm, TransientSettings, TriggerSource};

fn setup(algorithm: TransientAlgorithm) -> (Box<AudioEngine>, Grid) {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_sample_rate(1000.0);
    engine.set_compressor(0.0, 1.0);
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[0].kind = EffectKind::Bitcrush;
    tracks[0].params = default_params_for_kind(tracks[0].kind);
    engine.set_effects(&tracks);
    let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
    for cfg in &mut cfg {
        cfg.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 5.0,
        };
        cfg.detector_response = TransientSettings {
            algorithm,
            detail: 100.0,
        };
        cfg.smooth_ms = [3.0; 4];
        cfg.targets = [None; 3];
    }
    engine.set_modulation(&cfg);
    let mut grid = Grid::default();
    for row in 0..ROWS {
        for col in 0..COLS {
            grid.cell_mut(row, col).enabled = row == 0;
        }
    }
    (engine, grid)
}
#[test]
fn detector_algorithms_keep_audio_and_inactive_row_history_across_host_grid_and_midi_splits() {
    for algorithm in [
        TransientAlgorithm::Envelope,
        TransientAlgorithm::Peak,
        TransientAlgorithm::Rms,
    ] {
        let (mut actual, grid) = setup(algorithm);
        let (mut reference, _) = setup(algorithm);
        let mut left: [f32; 512] = std::array::from_fn(|i| if i % 17 == 0 { 0.8 } else { 0.0 });
        let mut right = left;
        let mut expected_l = left;
        let mut expected_r = left;
        assert_no_alloc::assert_no_alloc(|| {
            reference.process(
                &mut expected_l,
                &mut expected_r,
                true,
                1e9,
                120.0,
                1.0,
                &grid,
            );
            let mut start = 0;
            for n in [1, 16, 2, 61, 128, 1, 33, 200, 70] {
                actual.process_with_midi(
                    &mut left[start..start + n],
                    &mut right[start..start + n],
                    true,
                    17.0,
                    120.0,
                    1.0,
                    &grid,
                    [MidiNoteOn {
                        timing: 0,
                        channel: 0,
                        note: 60,
                    }]
                    .into_iter(),
                );
                start += n;
            }
        });
        assert_eq!(left, expected_l);
        assert_eq!(right, expected_r);
        for row in 0..ROWS {
            assert_eq!(
                actual.modulation.precise_phase(row, 0),
                reference.modulation.precise_phase(row, 0)
            );
            assert_eq!(
                actual.modulation.frame(row, EffectKind::None).detector,
                reference.modulation.frame(row, EffectKind::None).detector
            );
        }
    }
}
