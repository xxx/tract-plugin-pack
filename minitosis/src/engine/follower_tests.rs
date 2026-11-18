use super::*;
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::TriggerSource;

fn setup(sr: f32) -> (Processor, TrackEffect) {
    let track = TrackEffect {
        kind: EffectKind::Bitcrush,
        params: default_params_for_kind(EffectKind::Bitcrush),
        mix: 1.0,
        ..TrackEffect::default()
    };
    let mut cfg = TrackModulation {
        targets: [None; 3],
        ..TrackModulation::default()
    };
    cfg.follower.routes[0].enabled = true;
    cfg.follower.attack_ms = 10.0;
    cfg.follower.release_ms = 0.0;
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
fn empty_route_off_excursions_cannot_overwrite_a_held_follower_parameter() {
    let track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        mix: 1.0,
        ..TrackEffect::default()
    };
    let mut cfg = TrackModulation::default();
    cfg.follower.routes[1].enabled = true;
    cfg.follower.attack_ms = 0.0;
    cfg.follower.release_ms = 0.0;
    cfg.follower.detector.input = tract_effects::modulation::DetectorInput::Sidechain;
    cfg.depths[0] = 0.7;
    let key = PrepareKey {
        kind: track.kind,
        generation: 1,
        sample_rate_bits: 48000.0_f32.to_bits(),
    };
    let mut actual = Processor::new(key, &track, cfg.clone());
    let mut reference = Processor::new(key, &track, cfg.clone());
    let (mut output, mut expected) = ([[0.0; 2]; 8], [[0.0; 2]; 8]);
    assert_no_alloc::assert_no_alloc(|| {
        for p in [&mut actual, &mut reference] {
            p.begin_block(&track, &[0.5; 5], &[0.5; 5], 120.0);
            for _ in 0..5 {
                p.process_sample_with_sidechain([0.5; 2], [1.0; 2], 1.0, false);
            }
        }
        actual.modulation.follower.routes[1].enabled = false;
        actual.begin_block(&track, &[], &[], 120.0);
        actual.modulation.follower.routes[1].enabled = true;
        for p in [&mut actual, &mut reference] {
            p.begin_block(&track, &[0.5; 8], &[0.5; 8], 120.0);
        }
        for (i, value) in output.iter_mut().enumerate() {
            *value = actual.process_sample_with_sidechain([0.5; 2], [1.0; 2], 1.0, false);
            expected[i] = reference.process_sample_with_sidechain([0.5; 2], [1.0; 2], 1.0, false);
        }
    });
    assert_eq!(output, expected);
}

#[test]
fn precise_retriggers_feed_each_follower_sample_once_and_flush_keeps_history_without_allocations() {
    let (mut processor, track) = setup(48000.0);
    processor.modulation.trigger = TriggerSource::FreeHzSample { hz: 12000.0 };
    let mut values = [0.0; 3];
    assert_no_alloc::assert_no_alloc(|| {
        processor.begin_block(&track, &[1.0; 5], &[1.0; 5], 120.0);
        for _ in 0..5 {
            processor.process_sample([1.0; 2], 1.0, false);
        }
        values[0] = processor.modulation_frame().follower.unwrap().envelope;
        processor.flush_dsp();
        processor.retrigger_modulation();
        values[1] = processor.modulation_frame().follower.unwrap().envelope;
        processor.reset_modulation();
        values[2] = processor.modulation_frame().follower.unwrap().envelope;
    });
    assert!((values[0] - (1.0 - (-5.0_f32 / 480.0).exp())).abs() < 1e-7);
    assert_eq!(values[0], values[1]);
    assert_eq!(values[2], 0.0);
}
#[test]
fn same_kind_sample_rate_handoff_copies_follower_and_rescales_hold_in_preallocated_storage() {
    let (mut old, track) = setup(1000.0);
    old.modulation.follower.attack_ms = 0.0;
    old.modulation.follower.hold_ms = 5.0;
    old.begin_block(&track, &[1.0, 0.0, 0.0], &[1.0, 0.0, 0.0], 120.0);
    old.process_sample([1.0; 2], 1.0, false);
    for _ in 0..2 {
        old.process_sample([0.0; 2], 1.0, false);
    }
    let mut key = old.key;
    key.sample_rate_bits = 2000.0_f32.to_bits();
    let mut next = Processor::new(key, &track, old.modulation.clone());
    let mut values = [0.0; 7];
    assert_no_alloc::assert_no_alloc(|| {
        next.inherit_precise_timing(&old);
        next.begin_block(&track, &[0.0; 7], &[0.0; 7], 120.0);
        for value in &mut values {
            next.process_sample([0.0; 2], 1.0, true);
            *value = next.modulation_frame().follower.unwrap().envelope;
        }
    });
    assert_eq!(values, [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0]);
}
