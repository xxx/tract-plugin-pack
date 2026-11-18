use super::*;
use crate::state::neutral_modulation;
use tiny_skia_widgets::mseg::{
    CurveShape, FactoryShape, MsegData, PlayMode, SyncMode, WavePalette, factory_shape,
};
use tract_effects::modulation::{MidiNoteOn, MidiWaveMapping, TriggerSource};

fn setup() -> (Processor, TrackEffect, WavePalette) {
    let track = TrackEffect {
        kind: EffectKind::Bitcrush,
        params: tract_effects::effects::default_params_for_kind(EffectKind::Bitcrush),
        mix: 1.0,
        ..TrackEffect::default()
    };
    let mut config = neutral_modulation();
    config.targets = [None; 3];
    config.msegs[0] = MsegData {
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        play_mode: PlayMode::Cyclic,
        ..MsegData::default()
    };
    config.msegs[0].nodes[1].value = 0.0;
    config.wave_switch = Some(MidiWaveMapping {
        envelope: 0,
        first_note: 36,
        channel: Some(2),
    });
    let key = PrepareKey {
        kind: track.kind,
        generation: 1,
        sample_rate_bits: 48000.0_f32.to_bits(),
    };
    let mut processor = Processor::new(key, &track, config);
    let mut palette = WavePalette::default();
    palette
        .store(0, &CurveShape::capture(&processor.modulation.msegs[0]))
        .unwrap();
    palette
        .store(1, &factory_shape(FactoryShape::FlatHigh))
        .unwrap();
    processor.set_wave_palette(&palette);
    (processor, track, palette)
}

#[test]
fn zero_frame_automation_changes_do_not_refresh_lag_before_audio_resumes() {
    let (mut actual, track, mut palette) = setup();
    let (mut reference, _, _) = setup();
    actual.modulation.smooth_ms[0] = 10.0;
    actual.modulation.msegs[0].time_seconds = 0.001;
    reference.modulation = actual.modulation.clone();
    palette
        .store(0, &factory_shape(FactoryShape::RampUp))
        .unwrap();
    palette
        .store(1, &CurveShape::capture(&actual.modulation.msegs[0]))
        .unwrap();
    for processor in [&mut actual, &mut reference] {
        processor.set_wave_palette(&palette);
        processor.set_wave_automation([1, 0, 0, 0]);
        processor.begin_block(&track, &[0.37; 5], &[0.37; 5], 120.0);
        processor.midi_note_on(
            MidiNoteOn {
                timing: 0,
                channel: 2,
                note: 37,
            },
            false,
        );
        for _ in 0..5 {
            processor.process_sample([0.37; 2], 1.0, false);
        }
    }
    let (mut actual_audio, mut expected_audio) = ([[0.0; 2]; 8], [[0.0; 2]; 8]);
    assert_no_alloc::assert_no_alloc(|| {
        actual.set_wave_automation([0; 4]);
        actual.prepare_block(&track, &[], &[], 120.0);
        actual.set_wave_automation([1, 0, 0, 0]);
        actual.begin_block(&track, &[0.37; 8], &[0.37; 8], 120.0);
        reference.begin_block(&track, &[0.37; 8], &[0.37; 8], 120.0);
        for (actual_sample, expected_sample) in actual_audio.iter_mut().zip(&mut expected_audio) {
            *actual_sample = actual.process_sample([0.37; 2], 1.0, false);
            *expected_sample = reference.process_sample([0.37; 2], 1.0, false);
        }
    });
    assert_eq!(actual_audio, expected_audio);
    assert_eq!(
        actual.modulation_frame().outputs,
        reference.modulation_frame().outputs
    );
    assert_eq!(actual.phase(0), reference.phase(0));
}

#[test]
fn automation_handoff_keeps_ready_bank_during_the_first_refresh_and_uses_no_heap_work() {
    let (mut previous, track, _) = setup();
    previous.modulation.wave_switch = None;
    previous.set_wave_automation([2, 8, 0, 2]);
    previous.begin_block(&track, &[0.37; 16], &[0.37; 16], 120.0);
    for _ in 0..16 {
        previous.process_sample([0.37; 2], 1.0, false);
    }
    let mut key = previous.key;
    key.sample_rate_bits = 96000.0_f32.to_bits();
    let mut next = Processor::new(key, &track, previous.modulation.clone());
    let mut output = [0.0; 2];
    assert_no_alloc::assert_no_alloc(|| {
        next.inherit_precise_timing(&previous);
        next.begin_block(&track, &[0.37; 16], &[0.37; 16], 120.0);
        output = next.process_sample([0.37; 2], 1.0, false);
    });
    assert!(output[0] > 0.3);
    assert!(next.phase(0) > 0.0);
    assert_eq!(next.modulation_frame().wave_automation, [2, 8, 0, 2]);
    assert_eq!(
        next.state.effective_mseg(&next.modulation, 3).nodes[0].value,
        1.0
    );
}

#[test]
fn wave_note_offsets_preserve_phase_prefix_and_authored_data_without_allocations() {
    for offset in [0, 1, 63, 127] {
        let (mut actual, track, _) = setup();
        let saved = actual.modulation.clone();
        let mut reference_config = saved.clone();
        reference_config.wave_switch = None;
        reference_config.msegs[0].nodes[0].value = 1.0;
        reference_config.msegs[0].nodes[1].value = 1.0;
        let mut reference = Processor::new(actual.key, &track, reference_config);
        let input = [0.37; 128];
        let mut output = [[0.0; 2]; 128];
        let mut expected = output;
        assert_no_alloc::assert_no_alloc(|| {
            actual.begin_block(&track, &input, &input, 120.0);
            reference.begin_block(&track, &input, &input, 120.0);
            for index in 0..128 {
                if index == offset {
                    assert!(!actual.midi_note_on(
                        MidiNoteOn {
                            timing: index,
                            channel: 2,
                            note: 37
                        },
                        true
                    ));
                }
                output[index] = actual.process_sample([input[index]; 2], 1.0, false);
                expected[index] = reference.process_sample([input[index]; 2], 1.0, false);
            }
        });
        assert!(output[..offset].iter().all(|sample| *sample == [0.0; 2]));
        assert_eq!(&output[offset..], &expected[offset..]);
        assert_eq!(actual.phase(0), reference.phase(0));
        assert_eq!(actual.modulation, saved);
        assert_eq!(
            actual.modulation_frame().wave,
            Some((saved.wave_switch.unwrap(), Some(1)))
        );
    }
}

#[test]
fn wave_selection_precedes_matching_retrigger_and_handles_bank_refresh_reset_and_bypass() {
    let (mut processor, track, mut palette) = setup();
    processor.modulation.trigger = TriggerSource::MidiNote {
        note: Some(37),
        channel: Some(2),
    };
    let input = [0.37; 64];
    processor.begin_block(&track, &input, &input, 120.0);
    for _ in 0..20 {
        processor.process_sample([0.37; 2], 1.0, false);
    }
    assert!(processor.midi_note_on(
        MidiNoteOn {
            timing: 20,
            channel: 2,
            note: 37
        },
        true
    ));
    processor.advance_segment(&track, 44, 120.0);
    assert!((processor.phase(0) - 44.0 / 48000.0).abs() < 1e-7);
    let before = processor.modulation.msegs[0];
    palette.clear(1).unwrap();
    assert_no_alloc::assert_no_alloc(|| {
        processor.set_wave_palette(&palette);
        processor.begin_block(&track, &input, &input, 120.0);
        assert_eq!(processor.process_sample([0.37; 2], 1.0, true), [0.37; 2]);
        processor.reset_modulation();
        processor.modulation.wave_switch = None;
        processor.begin_block(&track, &input, &input, 120.0);
        processor.process_sample([0.37; 2], 1.0, false);
        processor.modulation.wave_switch = Some(MidiWaveMapping {
            envelope: 0,
            first_note: 36,
            channel: Some(2),
        });
        processor.begin_block(&track, &input, &input, 120.0);
    });
    assert_eq!(processor.modulation.msegs[0], before);
    assert_eq!(processor.modulation_frame().wave.unwrap().1, None);
}

#[test]
fn simultaneous_notes_apply_only_the_final_wave_and_invalid_slots_do_not_match() {
    let (mut processor, track, _) = setup();
    let input = [0.37; 64];
    processor.begin_block(&track, &input, &input, 120.0);
    assert!(!processor.accepts_midi(MidiNoteOn {
        timing: 0,
        channel: 3,
        note: 37
    }));
    assert!(!processor.accepts_midi(MidiNoteOn {
        timing: 0,
        channel: 2,
        note: 43
    }));
    for note in [37, 36, 37] {
        processor.midi_note_on(
            MidiNoteOn {
                timing: 0,
                channel: 2,
                note,
            },
            true,
        );
    }
    let output = processor.process_sample([0.37; 2], 1.0, false);
    assert!(output[0] > 0.2);
    assert_eq!(processor.modulation_frame().wave.unwrap().1, Some(1));
}

#[test]
fn delayed_sidechain_reset_commits_the_midi_wave_at_the_same_sample_across_host_splits() {
    use tract_effects::modulation::{
        DetectorInput, ModulationTarget, TransientAlgorithm, TransientSettings, TriggerTiming,
    };

    const CUE: usize = 144;
    const FRAMES: usize = CUE + 16;
    let mut input = [0.37; FRAMES];
    input[CUE] = 0.81;
    let mut aux = [0.0; FRAMES];
    aux[0] = 0.8;
    let sidechain = SidechainInput::new(&aux, &aux);
    let render = |split: bool| {
        let (mut processor, mut track, mut palette) = setup();
        track.mix = 0.35;
        let config = &mut processor.modulation;
        config.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 5.0,
        };
        config.audio_trigger.input = DetectorInput::Sidechain;
        config.detector_response = TransientSettings {
            algorithm: TransientAlgorithm::Peak,
            detail: 100.0,
        };
        config.trigger_timing = TriggerTiming {
            audio_delay_ms: 3.0,
            blend_ms: 1.0,
        };
        config.msegs[0].time_seconds = 0.01;
        config.msegs[1] = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 0.01,
            ..MsegData::default()
        };
        config.targets[0] = Some(ModulationTarget::Mix);
        config.depths[0] = 0.5;
        palette
            .store(0, &factory_shape(FactoryShape::FlatCenter))
            .unwrap();
        palette
            .store(1, &factory_shape(FactoryShape::RampDown))
            .unwrap();
        processor.set_wave_palette(&palette);
        let telemetry = Arc::new(ModulationTelemetry::default());
        let token = telemetry.waveform.request(0, 0, track.kind);
        processor.connect_waveform(&telemetry);
        processor.set_wave_automation([u8::from(split), 0, 0, 0]);
        let note = |timing, note| MidiNoteOn {
            timing,
            note,
            channel: 2,
        };
        let mut output = [[0.0; 2]; FRAMES];
        let ends: &[usize] = if split { &[CUE, FRAMES] } else { &[FRAMES] };
        assert_no_alloc::assert_no_alloc(|| {
            let mut start = 0;
            for &end in ends {
                if start == CUE {
                    processor.set_wave_automation([0; 4]);
                    processor.prepare_block(&track, &[], &[], 120.0);
                }
                let block = &input[start..end];
                processor.prepare_block_with_sidechain(
                    &track,
                    block,
                    block,
                    sidechain.offset(start),
                    120.0,
                );
                assert!(!processor.midi_note_on(note(0, if start == 0 { 36 } else { 37 }), false));
                processor.advance_segment(&track, end - start, 120.0);
                for index in start..end {
                    if index == CUE && start == 0 {
                        assert!(!processor.midi_note_on(note(CUE, 37), true));
                    }
                    output[index] = processor.process_sample_with_sidechain(
                        [input[index]; 2],
                        sidechain.sample(index),
                        track.mix,
                        false,
                    );
                    assert_eq!(processor.state.trigger_epoch(), u64::from(index >= CUE));
                    if index == CUE {
                        let frame = processor.modulation_frame();
                        assert_eq!(frame.phases[0], 0.0);
                        assert_eq!(frame.wave.unwrap().1, Some(1));
                        assert!((0.49..0.5).contains(&frame.mix.unwrap().0));
                    }
                }
                processor.finish_waveform_block();
                start = end;
            }
        });
        let frame = processor.modulation_frame();
        assert!(frame.mix.is_some());
        let point = (0..telemetry.waveform.head())
            .filter_map(|index| telemetry.waveform.point(index, token))
            .find(|point| point.input == input[CUE])
            .unwrap();
        assert_eq!(point.bin, 0);
        assert_eq!(point.output, output[CUE][0].abs());
        (output, frame)
    };
    assert_eq!(render(false), render(true));
}

#[test]
fn same_sample_audio_trigger_uses_the_final_wave_without_erasing_fft_history() {
    use tract_effects::modulation::ModulationTarget;
    let kind = EffectKind::SpectralShift;
    let mut track = TrackEffect {
        kind,
        params: tract_effects::effects::default_params_for_kind(kind),
        mix: 1.0,
        ..TrackEffect::default()
    };
    let key = PrepareKey {
        kind,
        generation: 1,
        sample_rate_bits: 48000.0_f32.to_bits(),
    };
    let mut processor = Processor::new(key, &track, neutral_modulation());
    let fft = processor
        .effect
        .parameters()
        .iter()
        .position(|spec| spec.name == "FFT")
        .unwrap();
    track.params[fft] = 0.0;
    processor.modulation.targets = [Some(ModulationTarget::EffectParameter(fft)), None, None];
    processor.modulation.depths = [1.0, 0.0, 0.0];
    processor.modulation.msegs[1] = MsegData {
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        ..MsegData::default()
    };
    processor.modulation.msegs[1].nodes[1].value = 0.0;
    processor.modulation.wave_switch = Some(MidiWaveMapping {
        envelope: 1,
        first_note: 36,
        channel: None,
    });
    let mut wave = MsegData {
        sync_mode: SyncMode::Time,
        ..MsegData::default()
    };
    wave.insert_node(0.5, 1.0).unwrap();
    wave.nodes[0].stepped = true;
    let mut bank = WavePalette::default();
    bank.store(1, &CurveShape::capture(&wave)).unwrap();
    processor.set_wave_palette(&bank);
    let silent = vec![0.0; 36000];
    processor.begin_block(&track, &silent, &silent, 120.0);
    processor.effect.set_param(fft, 0.0);
    for _ in 0..8192 {
        processor.effect.process_sample(0.1, 0.1);
    }
    assert!(processor.effect.process_sample(0.1, 0.1).0 > 0.05);
    processor.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    let mut output = [0.0; 2];
    assert_no_alloc::assert_no_alloc(|| {
        processor.begin_block(&track, &[0.0; 64], &[0.0; 64], 120.0);
        processor.process_sample([0.0; 2], 1.0, false);
        assert!(!processor.midi_note_on(
            MidiNoteOn {
                timing: 1,
                channel: 0,
                note: 37
            },
            true
        ));
        assert_eq!(processor.effect.latency_samples(), 512);
        output = processor.process_sample([2.0; 2], 1.0, false);
    });
    assert_eq!(processor.latency(), 512);
    assert!((processor.phase(1) - 63.0 / 48000.0).abs() < 1e-7);
    assert!(
        output[0] > 0.05,
        "an intermediate old-endpoint setter erased the warmed FFT history"
    );
    assert_eq!(processor.modulation_frame().wave.unwrap().1, Some(1));
}

#[test]
fn sample_rate_adoption_copies_the_selected_wave_without_allocating() {
    let (mut previous, track, bank) = setup();
    previous.begin_block(&track, &[0.37; 64], &[0.37; 64], 120.0);
    previous.midi_note_on(
        MidiNoteOn {
            timing: 0,
            channel: 2,
            note: 37,
        },
        true,
    );
    previous.process_sample([0.37; 2], 1.0, false);
    let mut key = previous.key;
    key.sample_rate_bits = 96000.0_f32.to_bits();
    let mut next = Processor::new(key, &track, previous.modulation.clone());
    assert_no_alloc::assert_no_alloc(|| {
        next.inherit_precise_timing(&previous);
        next.begin_block(&track, &[0.37; 64], &[0.37; 64], 120.0);
        next.process_sample([0.37; 2], 1.0, false);
        assert_eq!(next.modulation_frame().wave.unwrap().1, Some(1));
        next.set_wave_palette(&bank);
        next.begin_block(&track, &[0.37; 64], &[0.37; 64], 120.0);
    });
    assert_eq!(next.modulation_frame().wave.unwrap().1, Some(1));
}

#[test]
fn wave_parameter_latency_changes_are_applied_before_dry_mixing_in_the_same_sample() {
    use tract_effects::modulation::ModulationTarget;
    let kind = EffectKind::SpectralShift;
    let mut track = TrackEffect {
        kind,
        params: tract_effects::effects::default_params_for_kind(kind),
        mix: 0.5,
        ..TrackEffect::default()
    };
    let mut processor = Processor::new(
        PrepareKey {
            kind,
            generation: 1,
            sample_rate_bits: 48000.0_f32.to_bits(),
        },
        &track,
        neutral_modulation(),
    );
    let fft = processor
        .effect
        .parameters()
        .iter()
        .position(|spec| spec.name == "FFT")
        .unwrap();
    track.params[fft] = 0.0;
    processor.modulation.targets = [Some(ModulationTarget::EffectParameter(fft)), None, None];
    processor.modulation.depths = [1.0, 0.0, 0.0];
    processor.modulation.msegs[1].nodes[0].value = 0.0;
    processor.modulation.msegs[1].nodes[1].value = 0.0;
    processor.modulation.wave_switch = Some(MidiWaveMapping {
        envelope: 1,
        first_note: 36,
        channel: None,
    });
    let mut palette = WavePalette::default();
    palette
        .store(1, &factory_shape(FactoryShape::FlatHigh))
        .unwrap();
    processor.set_wave_palette(&palette);
    let mut measured = (0, 0, 0);
    assert_no_alloc::assert_no_alloc(|| {
        processor.begin_block(&track, &[0.37; 64], &[0.37; 64], 120.0);
        measured.0 = processor.latency();
        processor.process_sample([0.37; 2], 0.5, false);
        processor.midi_note_on(
            MidiNoteOn {
                timing: 1,
                channel: 0,
                note: 37,
            },
            true,
        );
        processor.process_sample([0.37; 2], 0.5, false);
        measured.1 = processor.effect.latency_samples();
        measured.2 = processor.latency();
    });
    assert_eq!(measured, (512, 4096, 4096));
}
