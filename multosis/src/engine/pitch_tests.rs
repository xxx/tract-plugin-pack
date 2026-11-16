use super::*;
use crate::effects::{EffectKind, default_params_for_kind};
use crate::grid::COLS;
use crate::modulation::TriggerSource;
use tiny_skia_widgets::{MsegData, PlayMode};

fn setup(smooth: f32) -> (Box<AudioEngine>, Grid) {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_compressor(0.0, 1.0);
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[0].kind = EffectKind::Bitcrush;
    tracks[0].params = default_params_for_kind(tracks[0].kind);
    engine.set_effects(&tracks);
    let mut config = std::array::from_fn(TrackModulation::default_for_row);
    for cfg in &mut config {
        cfg.trigger = TriggerSource::MidiPitch {
            transpose: 0,
            channel: Some(1),
        };
        cfg.targets = [None; 3];
        cfg.smooth_ms = [smooth; 4];
        cfg.msegs = std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            ..MsegData::default()
        });
    }
    engine.set_modulation(&config);
    let mut grid = Grid::default();
    for row in 0..ROWS {
        for col in 0..COLS {
            grid.cell_mut(row, col).enabled = row == 0;
        }
    }
    (engine, grid)
}
#[test]
fn midi_pitch_audio_and_inactive_row_phases_are_invariant_across_host_and_grid_splits() {
    for smooth in [0.0, 10.0] {
        let (mut actual, grid) = setup(smooth);
        let (mut reference, _) = setup(smooth);
        let notes = [
            (0, 69, 1),
            (17, 60, 1),
            (17, 81, 1),
            (17, 127, 0),
            (64, 57, 1),
            (255, 72, 1),
        ];
        let mut left = [0.37; 256];
        let mut right = left;
        let mut expected_l = left;
        let mut expected_r = left;
        assert_no_alloc::assert_no_alloc(|| {
            reference.process_with_midi(
                &mut expected_l,
                &mut expected_r,
                true,
                1e9,
                120.0,
                1.0,
                &grid,
                notes.into_iter().map(|(timing, note, channel)| MidiNoteOn {
                    timing,
                    note,
                    channel,
                }),
            );
            let mut start = 0;
            for count in [5, 12, 1, 31, 15, 1, 64, 32, 94, 1] {
                actual.process_with_midi(
                    &mut left[start..start + count],
                    &mut right[start..start + count],
                    true,
                    17.0,
                    37.0,
                    1.0,
                    &grid,
                    notes
                        .into_iter()
                        .filter(|(time, _, _)| *time >= start && *time < start + count)
                        .map(|(time, note, channel)| MidiNoteOn {
                            timing: time - start,
                            note,
                            channel,
                        }),
                );
                start += count;
            }
        });
        assert_eq!(left, expected_l);
        assert_eq!(right, expected_r);
        for row in 0..ROWS {
            let frame = actual.modulation.frame(
                row,
                if row == 0 {
                    EffectKind::Bitcrush
                } else {
                    EffectKind::None
                },
            );
            assert_eq!(frame.pitch.unwrap().note, Some(72));
            assert_eq!(actual.modulation.precise_phase(row, 0), 0.0);
            assert_eq!(
                frame.outputs,
                reference.modulation.frame(row, frame.kind).outputs
            );
        }
        let mut polls = 0;
        let events = std::iter::from_fn(|| {
            polls += 1;
            Some(MidiNoteOn {
                timing: 0,
                channel: 1,
                note: 69,
            })
        });
        actual.process_with_midi(&mut [], &mut [], true, 17.0, 120.0, 1.0, &grid, events);
        assert_eq!(polls, 0);
    }
}
