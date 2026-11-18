use super::*;
use crate::effects::{EffectKind, default_params_for_kind};
use crate::grid::COLS;
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
use tract_effects::modulation::{
    TransientAlgorithm, TransientSettings, TriggerSource, TriggerTiming,
};

fn setup() -> (Box<AudioEngine>, Grid) {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_sample_rate(48000.0);
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
            algorithm: TransientAlgorithm::Peak,
            detail: 100.0,
        };
        cfg.trigger_timing = TriggerTiming {
            audio_delay_ms: 3.0,
            blend_ms: 1.0,
        };
        cfg.targets = [None; 3];
        cfg.msegs = std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..MsegData::default()
        });
    }
    cfg[15].trigger = TriggerSource::MidiNote {
        note: None,
        channel: None,
    };
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
fn delayed_resets_and_blends_keep_audio_across_grid_host_and_peer_midi_boundaries() {
    let (mut actual, grid) = setup();
    let (mut reference, _) = setup();
    let mut left: [f32; 1024] =
        std::array::from_fn(|i| if [0, 256, 700].contains(&i) { 0.8 } else { 0.0 });
    let mut right = left;
    let (mut expected_l, mut expected_r) = (left, right);
    let notes = [17, 193, 444];
    assert_no_alloc::assert_no_alloc(|| {
        reference.process_with_midi(
            &mut expected_l,
            &mut expected_r,
            true,
            200.0,
            120.0,
            1.0,
            &grid,
            notes.into_iter().map(|timing| MidiNoteOn {
                timing,
                channel: 0,
                note: 60,
            }),
        );
        let mut start = 0;
        for n in [1, 7, 136, 1, 111, 144, 1, 299, 144, 1, 179] {
            actual.process_with_midi(
                &mut left[start..start + n],
                &mut right[start..start + n],
                true,
                144.0,
                120.0,
                1.0,
                &grid,
                notes
                    .into_iter()
                    .filter(|t| *t >= start && *t < start + n)
                    .map(|t| MidiNoteOn {
                        timing: t - start,
                        channel: 0,
                        note: 60,
                    }),
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
    }
}
