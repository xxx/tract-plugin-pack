use super::*;
use tiny_skia_widgets::{MsegData, PlayMode};
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::{MidiNoteOn, TriggerSource};

fn setup(sr: f32, smooth: f32) -> (Processor, TrackEffect) {
    let track = TrackEffect {
        kind: EffectKind::Bitcrush,
        params: default_params_for_kind(EffectKind::Bitcrush),
        ..TrackEffect::default()
    };
    let cfg = TrackModulation {
        trigger: TriggerSource::MidiPitch {
            transpose: 0,
            channel: Some(1),
        },
        targets: [None; 3],
        smooth_ms: [smooth; 4],
        msegs: std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            ..MsegData::default()
        }),
        ..TrackModulation::default()
    };
    (
        Processor::new(
            PrepareKey {
                kind: track.kind,
                generation: 1,
                sample_rate_bits: sr.to_bits(),
            },
            &track,
            cfg,
        ),
        track,
    )
}
#[test]
fn pitch_native_boundaries_same_offset_priority_and_bypass_preserve_exact_sample_rates() {
    for smooth in [0.0, 7.0] {
        let (mut actual, track) = setup(48000.0, smooth);
        let (mut reference, _) = setup(48000.0, smooth);
        let mut phases = [0.0; 8];
        let mut expected = [0.0; 8];
        assert_no_alloc::assert_no_alloc(|| {
            for p in [&mut actual, &mut reference] {
                p.begin_block(&track, &[0.37; 5], &[0.37; 5], 120.0);
                p.midi_note_on(
                    MidiNoteOn {
                        timing: 0,
                        channel: 1,
                        note: 60,
                    },
                    false,
                );
                p.midi_note_on(
                    MidiNoteOn {
                        timing: 0,
                        channel: 1,
                        note: 69,
                    },
                    false,
                );
                p.midi_note_on(
                    MidiNoteOn {
                        timing: 0,
                        channel: 0,
                        note: 127,
                    },
                    false,
                );
                p.advance_segment(&track, 5, 120.0);
                for _ in 0..5 {
                    p.process_sample([0.37; 2], 1.0, false);
                }
            }
            actual.begin_block(&track, &[0.37; 8], &[0.37; 8], 37.0);
            actual.midi_note_on(
                MidiNoteOn {
                    timing: 0,
                    channel: 1,
                    note: 81,
                },
                false,
            );
            actual.advance_segment(&track, 8, 37.0);
            reference.midi_note_on(
                MidiNoteOn {
                    timing: 5,
                    channel: 1,
                    note: 81,
                },
                true,
            );
            reference.advance_segment(&track, 8, 211.0);
            for i in 0..8 {
                actual.process_sample([0.37; 2], 1.0, true);
                reference.process_sample([0.37; 2], 1.0, false);
                phases[i] = actual.state.precise_phase(0);
                expected[i] = reference.state.precise_phase(0);
            }
        });
        assert_eq!(phases, expected);
        assert_eq!(phases[0], 0.0);
        assert!((phases[7] - 7.0 * 880.0 / 48000.0).abs() < 1e-15);
        assert_eq!(actual.modulation_frame().pitch.unwrap().note, Some(81));
        assert_eq!(actual.state.output(0), reference.state.output(0));
    }
}
#[test]
fn pitch_flush_and_sample_rate_handoff_keep_f64_phase_and_source_lag_without_allocations() {
    let (mut old, track) = setup(48000.0, 10.0);
    old.begin_block(&track, &[0.37; 17], &[0.37; 17], 120.0);
    old.midi_note_on(
        MidiNoteOn {
            timing: 0,
            channel: 1,
            note: 69,
        },
        false,
    );
    old.advance_segment(&track, 17, 120.0);
    for _ in 0..17 {
        old.process_sample([0.37; 2], 1.0, false);
    }
    let phase = old.state.precise_phase(0);
    let level = old.state.output(0);
    let (mut next, _) = setup(96000.0, 10.0);
    let mut adopted = [0.0; 2];
    let mut flushed = 0.0;
    assert_no_alloc::assert_no_alloc(|| {
        next.inherit_precise_timing(&old);
        next.begin_block(&track, &[0.37], &[0.37], 120.0);
        next.process_sample([0.37; 2], 1.0, false);
        adopted = [next.state.precise_phase(0), next.state.output(0) as f64];
        old.flush_dsp();
        old.begin_block(&track, &[0.37], &[0.37], 120.0);
        old.process_sample([0.37; 2], 1.0, false);
        flushed = old.state.precise_phase(0);
    });
    assert_eq!(flushed, phase + 440.0 / 48000.0);
    assert_eq!(adopted[0], phase + 440.0 / 96000.0);
    let expected = level as f64 + (1.0 - (-1.0_f64 / 960.0).exp()) * (adopted[0] - level as f64);
    assert!((adopted[1] - expected).abs() < 1e-7);
    assert_eq!(next.modulation_frame().pitch.unwrap().hz, 440.0);
}

#[test]
fn pitch_transport_restart_replaces_same_phase_waveform_peaks_and_loop_intros_keep_peaks() {
    let (mut p, track) = setup(440.0, 0.0);
    let telemetry = Arc::new(ModulationTelemetry::default());
    let token = telemetry.waveform.request(0, 0, track.kind);
    p.connect_waveform(&telemetry);
    assert_no_alloc::assert_no_alloc(|| {
        p.prepare_block(&track, &[1.0], &[1.0], 120.0);
        p.midi_note_on(
            MidiNoteOn {
                timing: 0,
                channel: 1,
                note: 69,
            },
            false,
        );
        p.advance_segment(&track, 1, 120.0);
        p.process_sample([1.0; 2], 1.0, false);
        p.finish_waveform_block();
        p.retrigger_modulation();
        p.begin_block(&track, &[0.1], &[0.1], 120.0);
        p.process_sample([0.1; 2], 1.0, false);
        p.finish_waveform_block();
    });
    assert_eq!(
        telemetry
            .waveform
            .point(telemetry.waveform.head() - 1, token)
            .unwrap()
            .input,
        0.1
    );
    for saved_length in [0.0, 0.017066667] {
        let (mut p, track) = setup(480000.0, 0.0);
        let doc = &mut p.modulation.msegs[0];
        doc.sync_mode = tiny_skia_widgets::SyncMode::Time;
        doc.time_seconds = saved_length;
        doc.play_mode = PlayMode::OneShot;
        let start = doc.insert_node(0.5, 0.5).unwrap();
        let end = doc.insert_node(0.5 + 1.0 / 8192.0, 0.5).unwrap();
        doc.hold = tiny_skia_widgets::HoldMode::Loop { start, end };
        let telemetry = Arc::new(ModulationTelemetry::default());
        let token = telemetry.waveform.request(0, 0, track.kind);
        p.connect_waveform(&telemetry);
        assert_no_alloc::assert_no_alloc(|| {
            p.prepare_block(&track, &[1.0, 0.1, 0.1], &[1.0, 0.1, 0.1], 0.0);
            p.midi_note_on(
                MidiNoteOn {
                    timing: 0,
                    channel: 1,
                    note: 69,
                },
                false,
            );
            p.advance_segment(&track, 3, 0.0);
            p.process_sample([1.0; 2], 1.0, false);
            p.process_sample([0.1; 2], 1.0, false);
            p.process_sample([0.1; 2], 1.0, false);
            p.finish_waveform_block();
        });
        let point = telemetry
            .waveform
            .point(telemetry.waveform.head() - 1, token)
            .unwrap();
        assert_eq!(point.bin, 0);
        assert_eq!(point.input, 1.0);
    }
}
