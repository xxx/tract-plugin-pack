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
