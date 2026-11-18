use super::*;
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::{HostPosition, TriggerSource};

fn setup() -> (Processor, TrackEffect) {
    let track = TrackEffect {
        kind: EffectKind::Bitcrush,
        params: default_params_for_kind(EffectKind::Bitcrush),
        mix: 1.0,
        ..TrackEffect::default()
    };
    let config = TrackModulation {
        trigger: TriggerSource::DawPosition,
        targets: [None; 3],
        depths: [0.0; 3],
        msegs: std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats: 4.0,
            ..MsegData::default()
        }),
        ..TrackModulation::default()
    };
    (
        Processor::new(
            PrepareKey {
                kind: track.kind,
                generation: 1,
                sample_rate_bits: 48000.0_f32.to_bits(),
            },
            &track,
            config,
        ),
        track,
    )
}
fn frame(offset: usize) -> HostPosition {
    HostPosition {
        playing: true,
        beats: Some(1.0 + offset as f64 / 24000.0),
        seconds: Some(0.5 + offset as f64 / 48000.0),
        bpm: 120.0,
        ..HostPosition::default()
    }
}

#[test]
fn transport_retrigger_uses_the_current_mode_when_it_changes_at_the_block_boundary() {
    for smooth in [0.0, 10.0] {
        let (mut processor, track) = setup();
        processor.modulation.smooth_ms = [smooth; 4];
        processor.set_host_position(HostPosition {
            playing: false,
            beats: Some(3.0),
            ..frame(0)
        });
        processor.begin_block(&track, &[0.37], &[0.37], 120.0);
        processor.process_sample([0.37; 2], 1.0, false);
        processor.modulation.trigger = TriggerSource::Free;
        assert_no_alloc::assert_no_alloc(|| {
            processor.retrigger_modulation();
            processor.begin_block(&track, &[0.37], &[0.37], 120.0);
            processor.process_sample([0.37; 2], 1.0, false);
        });
        let free_phase = processor.state.precise_phase(0);
        assert!((free_phase - 1.0 / 96000.0).abs() < 1e-7);
        processor.modulation.trigger = TriggerSource::DawPosition;
        assert_no_alloc::assert_no_alloc(|| {
            processor.set_host_position(HostPosition {
                playing: true,
                bpm: 120.0,
                ..HostPosition::default()
            });
            processor.retrigger_modulation();
            processor.begin_block(&track, &[0.37], &[0.37], 120.0);
            processor.process_sample([0.37; 2], 1.0, false);
        });
        assert_eq!(processor.state.precise_phase(0), free_phase);
    }
}

#[test]
fn negative_one_shot_intros_keep_peaks_while_real_same_phase_loop_returns_replace_them() {
    for loop_start in [0.0, 1.0 / 4096.0] {
        for seconds in [-1.0, 0.001] {
            let (mut processor, track) = setup();
            let doc = &mut processor.modulation.msegs[0];
            doc.play_mode = PlayMode::OneShot;
            doc.sync_mode = SyncMode::Time;
            doc.time_seconds = 1.0 / 48.0;
            let start = if loop_start == 0.0 {
                0
            } else {
                doc.insert_node(loop_start, 0.5).unwrap()
            };
            let end = doc.insert_node(loop_start + 1.0 / 4096.0, 0.5).unwrap();
            doc.hold = tiny_skia_widgets::HoldMode::Loop { start, end };
            let telemetry = Arc::new(ModulationTelemetry::default());
            let token = telemetry.waveform.request(0, 0, track.kind);
            processor.connect_waveform(&telemetry);
            assert_no_alloc::assert_no_alloc(|| {
                processor.set_host_position(HostPosition {
                    playing: true,
                    seconds: Some(seconds),
                    bpm: 120.0,
                    ..HostPosition::default()
                });
                processor.begin_block(&track, &[1.0, 0.1], &[1.0, 0.1], 120.0);
                processor.process_sample([1.0; 2], 1.0, false);
                processor.process_sample([0.1; 2], 1.0, false);
                processor.finish_waveform_block();
            });
            let point = telemetry
                .waveform
                .point(telemetry.waveform.head() - 1, token)
                .unwrap();
            assert_eq!(point.bin, 0);
            assert_eq!(
                point.input,
                if seconds < 0.0 { 1.0 } else { 0.1 },
                "start {loop_start}, seconds {seconds}"
            );
        }
    }
}

#[test]
fn transport_retrigger_preserves_daw_phases_when_host_positions_disappear() {
    for smooth in [0.0, 10.0] {
        let (mut processor, track) = setup();
        processor.modulation.smooth_ms = [smooth; 4];
        let (mut before, mut restarted, mut after) = ([0.0; 2], [0.0; 2], [0.0; 2]);
        assert_no_alloc::assert_no_alloc(|| {
            processor.set_host_position(HostPosition {
                playing: false,
                beats: Some(3.0),
                seconds: Some(1.5),
                ..frame(0)
            });
            processor.begin_block(&track, &[0.37], &[0.37], 120.0);
            processor.process_sample([0.37; 2], 1.0, false);
            before = [processor.phase(0) as f64, processor.state.precise_phase(0)];
            processor.retrigger_modulation();
            restarted = [processor.phase(0) as f64, processor.state.precise_phase(0)];
            processor.set_host_position(HostPosition {
                playing: true,
                bpm: 120.0,
                ..HostPosition::default()
            });
            processor.begin_block(&track, &[0.37], &[0.37], 120.0);
            processor.process_sample([0.37; 2], 1.0, false);
            after = [processor.phase(0) as f64, processor.state.precise_phase(0)];
        });
        assert_eq!(before, [0.75; 2]);
        assert_eq!(restarted, before);
        assert_eq!(after, before);
    }
}

#[test]
fn daw_amplitude_uses_host_sample_positions_and_stays_equal_across_subblocks_without_allocations() {
    let (mut actual, track) = setup();
    let (mut reference, _) = setup();
    let input = [0.37; 256];
    let mut output = [[0.0; 2]; 256];
    let mut expected = output;
    assert_no_alloc::assert_no_alloc(|| {
        reference.set_host_position(frame(0));
        reference.begin_block(&track, &input, &input, 120.0);
        for sample in &mut expected {
            *sample = reference.process_sample([0.37; 2], 1.0, false);
        }
        let mut offset = 0;
        for count in [5, 7, 31, 1, 48, 64, 100] {
            actual.set_host_position(frame(offset));
            actual.begin_block(&track, &input[..count], &input[..count], 120.0);
            for sample in &mut output[offset..offset + count] {
                *sample = actual.process_sample([0.37; 2], 1.0, false);
            }
            offset += count;
        }
    });
    for (a, b) in output.iter().zip(expected) {
        assert!((a[0] - b[0]).abs() < 1e-7);
    }
    assert!(actual.modulation_frame().position_sync);
    assert!((actual.phase(0) - ((1.0 + 255.0 / 24000.0) / 4.0) as f32).abs() < 1e-7);
    assert!(output[0][0] > 0.09 && output[0][0] < 0.10);
    assert!(output[1][0] > output[0][0]);
}

#[test]
fn daw_waveforms_pause_on_stop_and_clear_same_bin_peaks_after_an_absolute_seek() {
    let (mut processor, track) = setup();
    let telemetry = Arc::new(ModulationTelemetry::default());
    let token = telemetry.waveform.request(0, 0, track.kind);
    processor.connect_waveform(&telemetry);
    let mut point = frame(0);
    assert_no_alloc::assert_no_alloc(|| {
        processor.set_host_position(point);
        processor.begin_block(&track, &[0.9; 5], &[0.9; 5], 120.0);
        for _ in 0..5 {
            processor.process_sample([0.9; 2], 1.0, false);
        }
        processor.finish_waveform_block();
    });
    let first = telemetry
        .waveform
        .point(telemetry.waveform.head() - 1, token)
        .unwrap();
    assert_eq!(first.bin, 128);
    assert_eq!(first.input, 0.9);
    point.playing = false;
    processor.set_host_position(point);
    processor.begin_block(&track, &[0.1; 5], &[0.1; 5], 120.0);
    let head = telemetry.waveform.head();
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..5 {
            processor.process_sample([0.1; 2], 1.0, false);
        }
        processor.finish_waveform_block();
    });
    assert_eq!(telemetry.waveform.head(), head);
    point.playing = true;
    point.beats = Some(9.0);
    point.seconds = Some(4.5);
    processor.set_host_position(point);
    assert_no_alloc::assert_no_alloc(|| {
        processor.begin_block(&track, &[0.1; 5], &[0.1; 5], 120.0);
        for _ in 0..5 {
            processor.process_sample([0.1; 2], 1.0, false);
        }
        processor.finish_waveform_block();
    });
    let after = telemetry
        .waveform
        .point(telemetry.waveform.head() - 1, token)
        .unwrap();
    assert_eq!(after.bin, 128);
    assert_eq!(after.input, 0.1);
}

#[test]
fn same_effect_rate_adoption_seeds_a_fresh_lag_at_the_current_absolute_position() {
    let (mut old, track) = setup();
    old.modulation.smooth_ms = [10.0; 4];
    old.set_host_position(frame(0));
    old.begin_block(&track, &[0.37; 5], &[0.37; 5], 120.0);
    for _ in 0..5 {
        old.process_sample([0.37; 2], 1.0, false);
    }
    let mut key = old.key;
    key.sample_rate_bits = 96000.0_f32.to_bits();
    let mut ready = Processor::new(key, &track, old.modulation.clone());
    let mut phase = 0.0;
    assert_no_alloc::assert_no_alloc(|| {
        ready.inherit_precise_timing(&old);
        ready.set_host_position(HostPosition {
            beats: Some(3.0),
            seconds: Some(1.5),
            ..frame(0)
        });
        ready.begin_block(&track, &[0.37; 5], &[0.37; 5], 120.0);
        ready.process_sample([0.37; 2], 1.0, false);
        phase = ready.phase(0);
    });
    assert_eq!(phase, 0.75);
    assert_eq!(ready.modulation_frame().outputs[0], 0.75);
}
