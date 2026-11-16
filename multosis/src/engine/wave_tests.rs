use super::*;
use crate::effects::{EffectKind, default_params_for_kind};
use crate::modulation::{MidiWaveMapping, TrackModulation, TriggerSource};
use tiny_skia_widgets::mseg::{FactoryShape, PlayMode, SyncMode, WavePalette, factory_shape};

fn setup() -> (Box<AudioEngine>, Grid, [TrackModulation; ROWS], WavePalette) {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_compressor(0.0, 1.0);
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[0].kind = EffectKind::Bitcrush;
    tracks[0].params = default_params_for_kind(tracks[0].kind);
    tracks[0].mix = 1.0;
    engine.set_effects(&tracks);
    let mut config = std::array::from_fn(TrackModulation::default_for_row);
    for row in &mut config {
        row.targets = [None; 3];
        row.depths = [0.0; 3];
        row.trigger = TriggerSource::Free;
        for curve in &mut row.msegs {
            curve.sync_mode = SyncMode::Time;
            curve.time_seconds = 1.0;
            curve.play_mode = PlayMode::Cyclic;
        }
    }
    config[0].msegs[0].nodes[0].value = 0.0;
    config[0].msegs[0].nodes[1].value = 0.0;
    config[0].wave_switch = Some(MidiWaveMapping {
        envelope: 0,
        first_note: 36,
        channel: Some(2),
    });
    engine.set_modulation(&config);
    let mut palette = WavePalette::default();
    palette
        .store(1, &factory_shape(FactoryShape::FlatHigh))
        .unwrap();
    engine.set_wave_palette(&palette);
    let mut grid = Grid::default();
    for row in 0..ROWS {
        grid.cell_mut(row, 0).enabled = false;
    }
    grid.cell_mut(0, 0).enabled = true;
    (engine, grid, config, palette)
}

#[test]
fn midi_wave_boundaries_preserve_prefix_phase_and_unaffected_rows() {
    for offset in [0, 1, 31, 63] {
        let (mut actual, grid, mut config, _) = setup();
        let (mut reference, _, _, _) = setup();
        config[0].wave_switch = None;
        config[0].msegs[0].nodes[0].value = 1.0;
        config[0].msegs[0].nodes[1].value = 1.0;
        reference.set_modulation(&config);
        let (mut left, mut right) = ([0.37; 64], [0.37; 64]);
        let (mut expected_l, mut expected_r) = (left, right);
        assert_no_alloc::assert_no_alloc(|| {
            actual.process_with_midi(
                &mut left,
                &mut right,
                false,
                1000.0,
                120.0,
                1.0,
                &grid,
                std::iter::once(MidiNoteOn {
                    timing: offset,
                    channel: 2,
                    note: 37,
                }),
            );
            reference.process(
                &mut expected_l,
                &mut expected_r,
                false,
                1000.0,
                120.0,
                1.0,
                &grid,
            );
        });
        assert!(left[..offset].iter().all(|value| *value == 0.0));
        assert_eq!(&left[offset..], &expected_l[offset..]);
        assert_eq!(&right[offset..], &expected_r[offset..]);
        for row in [0, 1, 15] {
            for slot in 0..4 {
                assert_eq!(
                    actual.modulation.phase(row, slot),
                    reference.modulation.phase(row, slot)
                );
            }
        }
        assert_eq!(
            actual
                .modulation
                .frame(0, EffectKind::Bitcrush)
                .wave
                .unwrap()
                .1,
            Some(1)
        );
    }
}

#[test]
fn bank_updates_configuration_changes_and_reset_stay_allocation_free() {
    let (mut engine, grid, mut config, mut palette) = setup();
    let (mut left, mut right) = ([0.37; 64], [0.37; 64]);
    engine.process_with_midi(
        &mut left,
        &mut right,
        false,
        1000.0,
        120.0,
        1.0,
        &grid,
        std::iter::once(MidiNoteOn {
            timing: 0,
            channel: 2,
            note: 37,
        }),
    );
    assert_eq!(
        engine
            .modulation
            .frame(0, EffectKind::Bitcrush)
            .wave
            .unwrap()
            .1,
        Some(1)
    );
    palette.clear(1).unwrap();
    assert_no_alloc::assert_no_alloc(|| {
        engine.set_wave_palette(&palette);
        engine.process(&mut left, &mut right, false, 1000.0, 120.0, 1.0, &grid);
        engine.reset();
        config[0].wave_switch = None;
        engine.set_modulation(&config);
        engine.process(&mut left, &mut right, false, 1000.0, 120.0, 1.0, &grid);
        config[0].wave_switch = Some(MidiWaveMapping {
            envelope: 0,
            first_note: 36,
            channel: Some(2),
        });
        engine.set_modulation(&config);
        engine.restart_transport();
        engine.process(&mut left, &mut right, false, 1000.0, 120.0, 1.0, &grid);
    });
    assert_eq!(
        engine
            .modulation
            .frame(0, EffectKind::Bitcrush)
            .wave
            .unwrap()
            .1,
        None
    );
}

#[test]
fn smoothed_wave_notes_match_interior_timing_at_host_and_grid_boundaries() {
    for grid_boundary in [false, true] {
        let (mut actual, mut grid, mut config, _) = setup();
        let (mut reference, _, _, _) = setup();
        config[0].smooth_ms[0] = 10.0;
        actual.set_modulation(&config);
        reference.set_modulation(&config);
        grid.loop_region.col0 = 0;
        grid.loop_region.col1 = 0;
        let (mut left, mut right) = ([0.37; 13], [0.37; 13]);
        let (mut expected_l, mut expected_r) = (left, right);
        assert_no_alloc::assert_no_alloc(|| {
            reference.process_with_midi(
                &mut expected_l,
                &mut expected_r,
                false,
                5.0,
                120.0,
                1.0,
                &grid,
                std::iter::once(MidiNoteOn {
                    timing: 5,
                    channel: 2,
                    note: 37,
                }),
            );
            if grid_boundary {
                actual.process_with_midi(
                    &mut left,
                    &mut right,
                    true,
                    5.0,
                    120.0,
                    1.0,
                    &grid,
                    std::iter::once(MidiNoteOn {
                        timing: 5,
                        channel: 2,
                        note: 37,
                    }),
                );
            } else {
                actual.process(
                    &mut left[..5],
                    &mut right[..5],
                    false,
                    5.0,
                    120.0,
                    1.0,
                    &grid,
                );
                actual.process_with_midi(
                    &mut left[5..],
                    &mut right[5..],
                    false,
                    5.0,
                    120.0,
                    1.0,
                    &grid,
                    std::iter::once(MidiNoteOn {
                        timing: 0,
                        channel: 2,
                        note: 37,
                    }),
                );
            }
        });
        assert_eq!(left, expected_l, "grid boundary {grid_boundary}");
        assert_eq!(right, expected_r, "grid boundary {grid_boundary}");
        assert!(left[5] > 0.0);
        assert_eq!(
            actual.modulation.phase(0, 0),
            reference.modulation.phase(0, 0)
        );
    }
}
