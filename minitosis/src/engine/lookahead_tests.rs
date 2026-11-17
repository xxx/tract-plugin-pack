use super::*;
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::{
    TransientAlgorithm, TransientSettings, TriggerSource, TriggerTiming,
};

fn setup(kind: EffectKind, shift: f32) -> (Processor, TrackEffect) {
    let track = TrackEffect {
        kind,
        params: default_params_for_kind(kind),
        ..TrackEffect::default()
    };
    let cfg = TrackModulation {
        trigger: TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 5.0,
        },
        detector_response: TransientSettings {
            algorithm: TransientAlgorithm::Peak,
            detail: 100.0,
        },
        lookahead: true,
        trigger_timing: TriggerTiming {
            audio_delay_ms: shift,
            blend_ms: 0.0,
        },
        targets: [None; 3],
        ..TrackModulation::default()
    };
    (
        Processor::new(
            PrepareKey {
                kind,
                generation: 1,
                sample_rate_bits: 1000.0_f32.to_bits(),
            },
            &track,
            cfg,
        ),
        track,
    )
}
#[test]
fn signed_cues_and_dry_wet_bypass_impulses_match_reported_added_latency_without_allocations() {
    for shift in [-4.0, 0.0, 4.0] {
        for (kind, mix, bypass) in [
            (EffectKind::Bitcrush, 1.0, false),
            (EffectKind::Bitcrush, 0.0, false),
            (EffectKind::Bitcrush, 1.0, true),
            (EffectKind::WarpZone, 0.0, false),
            (EffectKind::None, 1.0, false),
        ] {
            let (mut p, track) = setup(kind, shift);
            let latency = p.latency();
            assert_eq!(
                latency,
                if kind == EffectKind::WarpZone {
                    4100
                } else {
                    4
                }
            );
            let mut cue = usize::MAX;
            let mut peak = 0.0;
            let mut wrong = 0.0_f32;
            let mut epoch = 0;
            let input: [f32; 4110] = std::array::from_fn(|i| if i == 2 { 1.0 } else { 0.0 });
            assert_no_alloc::assert_no_alloc(|| {
                p.begin_block(&track, &input, &input, 120.0);
                for (i, value) in input.into_iter().enumerate() {
                    let out = p.process_sample([value; 2], mix, bypass);
                    let after = p.state.trigger_epoch();
                    if after != epoch {
                        cue = i;
                    }
                    epoch = after;
                    if i == latency + 2 {
                        peak = out[0];
                    } else {
                        wrong = wrong.max(out[0].abs());
                    }
                }
            });
            assert_eq!(cue, (6.0 + shift) as usize, "{kind:?}");
            assert_eq!(peak, 1.0, "{kind:?}");
            assert!(wrong < 1e-6, "{kind:?}: {wrong}");
        }
    }
}
#[test]
fn empty_callbacks_defer_path_changes_and_follower_input_tracks_delayed_original_audio() {
    let (mut p, track) = setup(EffectKind::None, 0.0);
    p.modulation.lookahead = false;
    p.begin_block(&track, &[], &[], 120.0);
    assert_eq!(p.latency(), 4);
    p.begin_block(&track, &[0.0], &[0.0], 120.0);
    assert_eq!(p.latency(), 0);
    p.modulation.lookahead = true;
    p.modulation.follower.routes[0].enabled = true;
    p.modulation.follower.attack_ms = 0.0;
    p.modulation.follower.release_ms = 0.0;
    let mut levels = [0.0; 7];
    assert_no_alloc::assert_no_alloc(|| {
        p.begin_block(&track, &[0.7; 7], &[0.7; 7], 120.0);
        for level in &mut levels {
            p.process_sample([0.7; 2], 1.0, false);
            *level = p.modulation_frame().follower.unwrap().envelope;
        }
    });
    assert_eq!(&levels[..4], &[0.0; 4]);
    assert_eq!(&levels[4..], &[0.7; 3]);
}
#[test]
fn changing_lookahead_resets_live_capture_aggregation_without_invalidating_frozen_generations() {
    let (mut p, track) = setup(EffectKind::None, 0.0);
    p.modulation.msegs[0].sync_mode = tiny_skia_widgets::SyncMode::Time;
    p.modulation.msegs[0].time_seconds = 1.0;
    let shared = Arc::new(ModulationTelemetry::default());
    let token = shared.waveform.request(0, 0, track.kind);
    let generation = shared.waveform.generation();
    p.connect_waveform(&shared);
    assert_no_alloc::assert_no_alloc(|| {
        p.begin_block(
            &track,
            &[1.0, 0.0, 0.0, 0.0, 0.0],
            &[1.0, 0.0, 0.0, 0.0, 0.0],
            120.0,
        );
        for value in [1.0, 0.0, 0.0, 0.0, 0.0] {
            p.process_sample([value; 2], 1.0, false);
        }
        p.finish_waveform_block();
    });
    assert_eq!(
        shared
            .waveform
            .point(shared.waveform.head() - 1, token)
            .unwrap()
            .input,
        1.0
    );
    let epoch = p.state.trigger_epoch();
    p.modulation.lookahead = false;
    assert_no_alloc::assert_no_alloc(|| {
        p.begin_block(&track, &[0.1], &[0.1], 120.0);
        p.process_sample([0.1; 2], 1.0, false);
        p.finish_waveform_block();
    });
    assert_eq!(
        shared
            .waveform
            .point(shared.waveform.head() - 1, token)
            .unwrap()
            .input,
        0.1
    );
    assert_eq!(shared.waveform.generation(), generation);
    assert_eq!(p.state.trigger_epoch(), epoch);
}
