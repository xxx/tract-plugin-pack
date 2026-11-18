use super::*;
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::{
    DetectorInput, TransientAlgorithm, TransientSettings, TriggerSource,
};

#[test]
fn each_algorithm_preserves_remaining_hold_across_sample_rate_handoff_without_allocations() {
    for algorithm in [
        TransientAlgorithm::Envelope,
        TransientAlgorithm::Peak,
        TransientAlgorithm::Rms,
    ] {
        let track = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: default_params_for_kind(EffectKind::Bitcrush),
            ..TrackEffect::default()
        };
        let cfg = TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 10.0,
            },
            detector_response: TransientSettings {
                algorithm,
                detail: 50.0,
            },
            targets: [None; 3],
            ..TrackModulation::default()
        };
        let key = PrepareKey {
            kind: track.kind,
            generation: 1,
            sample_rate_bits: 1000.0_f32.to_bits(),
        };
        let mut old = Processor::new(key, &track, cfg.clone());
        old.begin_block(&track, &[0.1, 0.0, 0.0, 0.0], &[0.1, 0.0, 0.0, 0.0], 120.0);
        for value in [0.1, 0.0, 0.0, 0.0] {
            old.process_sample([value; 2], 1.0, false);
        }
        let mut next = Processor::new(
            PrepareKey {
                sample_rate_bits: 2000.0_f32.to_bits(),
                ..key
            },
            &track,
            cfg.clone(),
        );
        let samples: [f32; 15] = std::array::from_fn(|i| 0.0001 * 2.0_f32.powi(i as i32));
        let mut epochs = [0; 15];
        assert_no_alloc::assert_no_alloc(|| {
            next.inherit_precise_timing(&old);
            next.begin_block(&track, &samples, &samples, 120.0);
            for (i, value) in samples.into_iter().enumerate() {
                next.process_sample([value; 2], 1.0, false);
                epochs[i] = next.state.trigger_epoch();
            }
        });
        assert_eq!(&epochs[..14], &[0; 14], "{algorithm:?}");
        assert_eq!(epochs[14], 1, "{algorithm:?}");
        assert_eq!(
            next.modulation_frame().detector.unwrap().response,
            cfg.detector_response
        );
    }
}
#[test]
fn all_detector_algorithms_keep_native_offsets_sidechain_bypass_and_reference_readings_without_allocations()
 {
    for algorithm in [
        TransientAlgorithm::Envelope,
        TransientAlgorithm::Peak,
        TransientAlgorithm::Rms,
    ] {
        for detail in [0.0, 100.0] {
            let track = TrackEffect {
                kind: EffectKind::Bitcrush,
                params: default_params_for_kind(EffectKind::Bitcrush),
                ..TrackEffect::default()
            };
            let mut cfg = TrackModulation {
                trigger: TriggerSource::TransientSample {
                    threshold: 1.5,
                    hold_ms: 5.0,
                },
                detector_response: TransientSettings { algorithm, detail },
                targets: [None; 3],
                ..TrackModulation::default()
            };
            cfg.audio_trigger.input = DetectorInput::Sidechain;
            let mut p = Processor::new(
                PrepareKey {
                    kind: track.kind,
                    generation: 1,
                    sample_rate_bits: 1000.0_f32.to_bits(),
                },
                &track,
                cfg.clone(),
            );
            let input = [0.2; 512];
            let side: [f32; 512] =
                std::array::from_fn(|i| if i >= 17 && i % 12 == 5 { 0.8 } else { 0.0 });
            let mut reference = ModulationState::default();
            let mut actual = [usize::MAX; 128];
            let mut expected = actual;
            let mut counts = [0; 2];
            assert_no_alloc::assert_no_alloc(|| {
                p.prepare_block_with_sidechain(
                    &track,
                    &input,
                    &input,
                    SidechainInput::new(&side, &side),
                    120.0,
                );
                p.advance_segment(&track, 512, 120.0);
                reference.begin_block(&cfg, &input, &input, 1000.0);
                let mut epoch = p.state.trigger_epoch();
                for (i, side) in side.into_iter().enumerate() {
                    if reference.next_precise_trigger_with_sidechain([0.2; 2], [side; 2]) {
                        expected[counts[1]] = i;
                        counts[1] += 1;
                    }
                    p.process_sample_with_sidechain([0.2; 2], [side; 2], 1.0, true);
                    let after = p.state.trigger_epoch();
                    if after != epoch {
                        actual[counts[0]] = i;
                        counts[0] += 1;
                    }
                    epoch = after;
                }
            });
            assert!(counts[0] > 0);
            assert_eq!(counts[0], counts[1]);
            assert_eq!(actual, expected);
            assert_eq!(p.modulation_frame().detector, reference.detector_reading());
        }
    }
}
