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
fn automation_selects_four_sources_independently_and_keeps_fixed_row_lanes_on_swap() {
    let (mut engine, grid, mut config, _) = setup();
    config[0].wave_switch = None;
    config[1].wave_switch = None;
    engine.set_modulation(&config);
    let mut values = [[0; 4]; ROWS];
    values[0] = [2, 0, 2, 8];
    values[1] = [8, 2, 0, 2];
    let (mut left, mut right) = ([0.37; 32], [0.37; 32]);
    assert_no_alloc::assert_no_alloc(|| {
        engine.set_wave_automation(values);
        engine.process(&mut left, &mut right, false, 1000.0, 120.0, 1.0, &grid);
    });
    assert!(left.iter().all(|value| *value > 0.3));
    let a = engine.modulation.frame(0, EffectKind::Bitcrush);
    let b = engine.modulation.frame(1, EffectKind::None);
    assert_eq!(a.wave_automation, values[0]);
    assert_eq!(b.wave_automation, values[1]);
    assert_eq!(a.outputs[0], 1.0);
    assert_eq!(a.outputs[2], 1.0);
    assert_eq!(b.outputs[1], 1.0);
    assert_eq!(b.outputs[3], 1.0);
    let untouched = engine.modulation.frame(15, EffectKind::None);
    assert_eq!(untouched.wave_automation, [0; 4]);
    assert_no_alloc::assert_no_alloc(|| engine.swap_tracks(0, 1));
    assert_eq!(
        engine.modulation.frame(0, EffectKind::None).wave_automation,
        values[0]
    );
    assert_eq!(
        engine
            .modulation
            .frame(1, EffectKind::Bitcrush)
            .wave_automation,
        values[1]
    );
}

#[test]
fn automation_release_batches_with_boundary_midi_before_any_smoothing_refresh() {
    for empty_block in [false, true] {
        let (mut actual, grid, mut config, mut palette) = setup();
        let (mut reference, _, _, _) = setup();
        config[0].smooth_ms[0] = 10.0;
        config[0].msegs[0].time_seconds = 0.001;
        palette
            .store(0, &factory_shape(FactoryShape::RampUp))
            .unwrap();
        palette
            .store(
                1,
                &tiny_skia_widgets::mseg::CurveShape::capture(&config[0].msegs[0]),
            )
            .unwrap();
        let mut values = [[0; 4]; ROWS];
        values[0][0] = 1;
        for engine in [&mut actual, &mut reference] {
            engine.set_modulation(&config);
            engine.set_wave_palette(&palette);
            engine.set_wave_automation(values);
            engine.process_with_midi(
                &mut [0.37; 5],
                &mut [0.37; 5],
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
        }
        let (mut left, mut right) = ([0.37; 8], [0.37; 8]);
        let (mut expected_l, mut expected_r) = (left, right);
        assert_no_alloc::assert_no_alloc(|| {
            actual.set_wave_automation([[0; 4]; ROWS]);
            if empty_block {
                actual.process(&mut [], &mut [], false, 1000.0, 120.0, 1.0, &grid);
                actual.set_wave_automation(values);
            }
            actual.process_with_midi(
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
                    note: 36,
                }),
            );
            reference.process_with_midi(
                &mut expected_l,
                &mut expected_r,
                false,
                1000.0,
                120.0,
                1.0,
                &grid,
                std::iter::once(MidiNoteOn {
                    timing: 0,
                    channel: 2,
                    note: 36,
                }),
            );
        });
        assert_eq!(left, expected_l);
        assert_eq!(right, expected_r);
        assert_eq!(
            actual.modulation.phase(0, 0),
            reference.modulation.phase(0, 0)
        );
        // No-note release also needs dispatch through the pending-row mask.
        actual.set_wave_automation(values);
        actual.process(
            &mut [0.37; 5],
            &mut [0.37; 5],
            false,
            1000.0,
            120.0,
            1.0,
            &grid,
        );
        actual.modulation.fire_midi(
            MidiNoteOn {
                timing: 0,
                channel: 2,
                note: 37,
            },
            false,
        );
        let previous = actual.modulation.frame(0, EffectKind::Bitcrush).outputs[0];
        actual.set_wave_automation([[0; 4]; ROWS]);
        actual.process(
            &mut [0.37; 1],
            &mut [0.37; 1],
            false,
            1000.0,
            120.0,
            1.0,
            &grid,
        );
        assert!(
            (actual.modulation.frame(0, EffectKind::Bitcrush).outputs[0]
                - previous * (-1.0_f32 / 480.0).exp())
            .abs()
                < 1e-7
        );
    }
}

#[test]
fn delayed_sidechain_reset_commits_the_midi_wave_across_host_and_grid_boundaries() {
    use crate::modulation::{
        DetectorInput, ModulationTarget, TransientAlgorithm, TransientSettings, TriggerTiming,
    };

    const CUE: usize = 144;
    const FRAMES: usize = CUE + 16;
    let mut aux = [0.0; FRAMES];
    aux[0] = 0.8;
    let sidechain = SidechainInput::new(&aux, &aux);
    let render = |playing: bool, split: bool| {
        let (mut engine, grid, mut config, mut palette) = setup();
        let mut tracks = engine.track_effects;
        tracks[0].mix = 0.35;
        engine.set_effects(&tracks);
        let row = &mut config[0];
        row.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 5.0,
        };
        row.audio_trigger.input = DetectorInput::Sidechain;
        row.detector_response = TransientSettings {
            algorithm: TransientAlgorithm::Peak,
            detail: 100.0,
        };
        row.trigger_timing = TriggerTiming {
            audio_delay_ms: 3.0,
            blend_ms: 1.0,
        };
        row.msegs[0].time_seconds = 0.01;
        row.msegs[1].time_seconds = 0.01;
        row.targets[0] = Some(ModulationTarget::Mix);
        row.depths[0] = 0.5;
        config[15].smooth_ms[0] = 1.0;
        engine.set_modulation(&config);
        palette
            .store(0, &factory_shape(FactoryShape::FlatCenter))
            .unwrap();
        palette
            .store(1, &factory_shape(FactoryShape::RampDown))
            .unwrap();
        engine.set_wave_palette(&palette);
        let token = engine.telemetry.waveform.request(0, 0, tracks[0].kind);
        let mut automation = [[0; 4]; ROWS];
        automation[0][0] = u8::from(split);
        engine.set_wave_automation(automation);
        let mut left = [0.37; FRAMES];
        left[CUE] = 0.81;
        let mut right = left;
        let ends: &[usize] = if split { &[CUE, FRAMES] } else { &[FRAMES] };
        assert_no_alloc::assert_no_alloc(|| {
            let mut start = 0;
            for &end in ends {
                if start == CUE {
                    engine.set_wave_automation([[0; 4]; ROWS]);
                    engine.process(&mut [], &mut [], false, CUE as f64, 120.0, 1.0, &grid);
                }
                let events = [(0, 36), (CUE, 37)]
                    .into_iter()
                    .filter(|(offset, _)| (start..end).contains(offset))
                    .map(|(offset, note)| MidiNoteOn {
                        timing: offset - start,
                        note,
                        channel: 2,
                    });
                engine.process_with_inputs(
                    &mut left[start..end],
                    &mut right[start..end],
                    sidechain.offset(start),
                    playing,
                    CUE as f64,
                    120.0,
                    1.0,
                    &grid,
                    events,
                );
                start = end;
            }
        });
        let frame = engine.modulation.frame(0, tracks[0].kind);
        assert_eq!(engine.modulation.capture_epoch(0, 0), 1);
        assert_eq!(frame.wave.unwrap().1, Some(1));
        assert!(frame.mix.is_some());
        let capture = &engine.telemetry.waveform;
        let point = (0..capture.head())
            .filter_map(|index| capture.point(index, token))
            .find(|point| point.input == 0.81)
            .unwrap();
        assert_eq!(point.bin, 0);
        assert_eq!(point.output, left[CUE].abs());
        let peer_phase = engine.modulation.precise_phase(15, 0);
        (left, right, frame, peer_phase)
    };
    let interior = render(false, false);
    assert_eq!(render(false, true), interior);
    assert_eq!(render(true, false), interior);
}

#[test]
fn smoothed_automation_on_a_subblock_boundary_matches_a_sample_timed_midi_switch() {
    let (mut actual, grid, mut config, _) = setup();
    let (mut reference, _, _, _) = setup();
    config[0].smooth_ms[0] = 10.0;
    actual.set_modulation(&config);
    reference.set_modulation(&config);
    let (mut left, mut right) = ([0.37; 13], [0.37; 13]);
    let (mut expected_l, mut expected_r) = (left, right);
    let mut values = [[0; 4]; ROWS];
    values[0][0] = 2;
    assert_no_alloc::assert_no_alloc(|| {
        actual.process(
            &mut left[..5],
            &mut right[..5],
            false,
            1000.0,
            120.0,
            1.0,
            &grid,
        );
        actual.set_wave_automation(values);
        actual.process(
            &mut left[5..],
            &mut right[5..],
            false,
            1000.0,
            120.0,
            1.0,
            &grid,
        );
        reference.process_with_midi(
            &mut expected_l,
            &mut expected_r,
            false,
            1000.0,
            120.0,
            1.0,
            &grid,
            std::iter::once(MidiNoteOn {
                timing: 5,
                channel: 2,
                note: 37,
            }),
        );
    });
    assert_eq!(left, expected_l);
    assert_eq!(right, expected_r);
    assert_eq!(
        actual.modulation.phase(0, 0),
        reference.modulation.phase(0, 0)
    );
    assert!(left[5] > 0.0);
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
