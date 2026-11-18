use super::*;
fn settings(input: DetectorInput) -> AuditionSettings {
    AuditionSettings {
        detector: AudioTriggerSettings {
            input,
            ..AudioTriggerSettings::default()
        },
        gain_db: 0.0,
    }
}
#[test]
fn leases_reject_old_release_and_prepare_exact_normalized_settings() {
    let control = AuditionControl::default();
    let cfg = TrackModulation::default();
    let old = control.begin(
        0,
        EffectKind::Svf,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        3,
    );
    let next = control.begin(
        1,
        EffectKind::Svf,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        4,
    );
    control.release(old);
    let request = control.snapshot().unwrap();
    assert_eq!(request.token, next);
    assert!(request.matches(1, EffectKind::Svf, &cfg, 4));
    assert!(!request.matches(1, EffectKind::Svf, &cfg, 3));
    assert!(!request.matches(0, EffectKind::Svf, &cfg, 4));
    assert!(!control.update(old, settings(DetectorInput::Sidechain)));
    assert!(control.update(next, settings(DetectorInput::Sidechain)));
    assert!(
        !control
            .snapshot()
            .unwrap()
            .matches(1, EffectKind::Svf, &cfg, 4)
    );
    control.invalidate();
    assert!(control.snapshot().is_none());
    assert!(!control.is_current(next));
}
#[test]
fn mono_trigger_and_stereo_follower_monitor_the_input_before_envelope_gating() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(16);
    control.begin(
        0,
        EffectKind::Svf,
        AuditionSource::Trigger,
        settings(DetectorInput::Main),
        0,
    );
    runtime.prepare(control.snapshot(), 1000.0);
    for _ in 0..5 {
        runtime.next([0.2; 2], [0.3, -0.7], [0.0; 2], &control, 0, true);
    }
    let mono = runtime.next([0.2; 2], [0.3, -0.7], [0.0; 2], &control, 0, true);
    assert!((mono[0] + 0.2).abs() < 1e-7 && mono[0] == mono[1]);
    let mut follower = settings(DetectorInput::Sidechain);
    follower.detector.level_db = Some(0.0);
    follower.gain_db = 6.0;
    control.begin(0, EffectKind::Svf, AuditionSource::Follower, follower, 0);
    runtime.prepare(control.snapshot(), 1000.0);
    let mut actual = [0.0; 2];
    for _ in 0..5 {
        actual = runtime.next([0.2; 2], [1.0; 2], [0.1, -0.2], &control, 0, true);
    }
    let gain = 10.0_f32.powf(6.0 / 20.0);
    assert!((actual[0] - 0.1 * gain).abs() < 1e-7);
    assert!((actual[1] + 0.2 * gain).abs() < 1e-7);
}
#[test]
fn latency_warmup_fades_and_release_follow_samples_and_bypass_keeps_ordinary_audio() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(16);
    let token = control.begin(
        0,
        EffectKind::Svf,
        AuditionSource::Follower,
        settings(DetectorInput::Sidechain),
        0,
    );
    runtime.prepare(control.snapshot(), 1000.0);
    let actual: Vec<_> = (0..8)
        .map(|_| runtime.next([0.0; 2], [0.0; 2], [0.5; 2], &control, 3, true)[0])
        .collect();
    assert_eq!(&actual[..3], &[0.0; 3]);
    for (i, value) in actual[3..].iter().enumerate() {
        assert!((*value - 0.5 * (i + 1) as f32 / 5.0).abs() < 1e-7);
    }
    control.release(token);
    for i in 0..5 {
        let value = runtime.next([0.0; 2], [0.0; 2], [0.5; 2], &control, 3, true)[0];
        assert!((value - 0.5 * (4 - i) as f32 / 5.0).abs() < 1e-7);
    }
    assert_eq!(
        runtime.next([-0.0, 0.7], [0.0; 2], [0.0; 2], &control, 3, true)[0].to_bits(),
        (-0.0_f32).to_bits()
    );
    control.begin(
        0,
        EffectKind::Svf,
        AuditionSource::Follower,
        settings(DetectorInput::Sidechain),
        0,
    );
    runtime.prepare(control.snapshot(), 1000.0);
    assert_eq!(
        runtime.next([0.3, -0.7], [0.0; 2], [1.0; 2], &control, 3, false),
        [0.3, -0.7]
    );
}

#[test]
fn nonaudible_controls_never_change_the_monitor_signature_and_pending_handoffs_keep_the_lease() {
    let control = AuditionControl::default();
    let mut cfg = TrackModulation::default();
    let initial = AuditionSettings::for_source(&cfg, AuditionSource::Follower).unwrap();
    let token = control.begin(0, EffectKind::None, AuditionSource::Follower, initial, 7);
    let request = control.snapshot().unwrap();
    cfg.follower.attack_ms = 1000.0;
    cfg.follower.hold_ms = 2000.0;
    cfg.follower.release_ms = 5000.0;
    cfg.follower.detector.level_db = Some(0.0);
    cfg.follower.routes[0].enabled = true;
    cfg.follower.routes[0].mode = super::super::FollowerMode::Add;
    assert_eq!(
        control.resolve(request, EffectKind::None, &cfg, 7),
        Some(request)
    );
    let mut edited = initial;
    edited.detector.highpass_hz = Some(500.0);
    control.update(token, edited);
    let pending = control.snapshot().unwrap();
    assert!(
        control
            .resolve(pending, EffectKind::None, &cfg, 7)
            .is_none()
    );
    assert!(control.is_current(token));
    cfg.follower.detector.highpass_hz = Some(500.0);
    assert_eq!(
        control.resolve(pending, EffectKind::None, &cfg, 7),
        Some(pending)
    );
    assert!(
        control
            .resolve(pending, EffectKind::None, &cfg, 8)
            .is_none()
    );
    assert!(!control.is_current(token));
    assert!(!control.update(token, initial));
}
#[test]
fn changing_input_or_latency_discards_the_old_queue_and_fully_on_ignores_invalid_normal_audio() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(16);
    let token = control.begin(
        0,
        EffectKind::None,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        0,
    );
    runtime.prepare(control.snapshot(), 1000.0);
    for _ in 0..16 {
        runtime.next([0.0; 2], [0.7; 2], [-0.3; 2], &control, 4, true);
    }
    control.update(token, settings(DetectorInput::Sidechain));
    runtime.prepare(control.snapshot(), 1000.0);
    for _ in 0..4 {
        assert_eq!(
            runtime.next([0.0; 2], [0.7; 2], [-0.3; 2], &control, 4, true),
            [0.0; 2]
        );
    }
    for _ in 0..5 {
        runtime.next([0.0; 2], [0.7; 2], [-0.3; 2], &control, 4, true);
    }
    assert_eq!(
        runtime.next([f32::NAN; 2], [0.7; 2], [-0.3; 2], &control, 4, true),
        [-0.3; 2]
    );
    for _ in 0..8 {
        assert_eq!(
            runtime.next([0.0; 2], [0.7; 2], [-0.3; 2], &control, 8, true),
            [0.0; 2]
        );
    }
    for value in [f32::NAN, f32::INFINITY, 1e38, -1e38, 0.0]
        .into_iter()
        .cycle()
        .take(64)
    {
        let output = runtime.next([0.0; 2], [0.0; 2], [value; 2], &control, 8, true);
        assert!(output.into_iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    }
}
#[test]
fn monitor_filters_use_the_detector_response_without_threshold_or_rectification() {
    for (highpass, hz, maximum) in [(true, 100.0, 0.11), (false, 8000.0, 0.15)] {
        let control = AuditionControl::default();
        let mut runtime = AuditionRuntime::new(0);
        let mut cfg = settings(DetectorInput::Main);
        cfg.detector.highpass_hz = highpass.then_some(1000.0);
        cfg.detector.lowpass_hz = (!highpass).then_some(1000.0);
        cfg.detector.level_db = Some(0.0);
        control.begin(0, EffectKind::None, AuditionSource::Follower, cfg, 0);
        runtime.prepare(control.snapshot(), 48000.0);
        let mut energy = 0.0_f64;
        let mut input_energy = 0.0;
        let mut minimum = 0.0_f32;
        for i in 0..48000 {
            let value = 0.1 * (std::f64::consts::TAU * hz * i as f64 / 48000.0).sin() as f32;
            let out = runtime.next([0.0; 2], [value; 2], [0.0; 2], &control, 0, true);
            if i > 24000 {
                energy += (out[0] as f64).powi(2);
                input_energy += (value as f64).powi(2);
                minimum = minimum.min(out[0]);
            }
        }
        assert!((energy / input_energy).sqrt() < maximum);
        assert!(minimum < -1e-4);
    }
}

#[test]
fn rapid_same_input_listen_reversal_preserves_the_current_fade_and_queue() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(16);
    let cfg = settings(DetectorInput::Main);
    let token = control.begin(0, EffectKind::None, AuditionSource::Follower, cfg, 1);
    runtime.prepare(control.snapshot(), 1000.0);
    for _ in 0..10 {
        runtime.next([0.0; 2], [1.0; 2], [0.0; 2], &control, 3, true);
    }
    control.release(token);
    let down = runtime.next([0.0; 2], [1.0; 2], [0.0; 2], &control, 3, true);
    assert_eq!(down, [0.8; 2]);
    control.begin(0, EffectKind::None, AuditionSource::Follower, cfg, 1);
    runtime.prepare(control.snapshot(), 1000.0);
    let up = runtime.next([0.0; 2], [1.0; 2], [0.0; 2], &control, 3, true);
    assert_eq!(up, [1.0; 2]);
}
#[test]
fn bypass_does_not_freeze_filter_history_and_revive_a_previous_signal() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(0);
    let mut cfg = settings(DetectorInput::Main);
    cfg.detector.lowpass_hz = Some(20.0);
    control.begin(0, EffectKind::None, AuditionSource::Follower, cfg, 1);
    runtime.prepare(control.snapshot(), 1000.0);
    for _ in 0..1000 {
        runtime.next([0.0; 2], [1.0; 2], [0.0; 2], &control, 0, true);
    }
    for _ in 0..1000 {
        runtime.next([0.0; 2], [0.0; 2], [0.0; 2], &control, 0, false);
    }
    for _ in 0..16 {
        let out = runtime.next([0.0; 2], [0.0; 2], [0.0; 2], &control, 0, true);
        assert!(out[0].abs() < 1e-6);
    }
}

#[test]
fn trigger_audition_requires_sample_trigger_and_ignores_threshold_and_gain() {
    let mut config = TrackModulation::default();
    config.audio_trigger = AudioTriggerSettings {
        input: DetectorInput::Sidechain,
        level_db: Some(-18.0),
        highpass_hz: Some(200.0),
        lowpass_hz: Some(2000.0),
    };
    assert_eq!(
        AuditionSettings::for_source(&config, AuditionSource::Trigger),
        None
    );
    config.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 15.0,
    };
    let mut requested = AuditionSettings::for_source(&config, AuditionSource::Trigger).unwrap();
    assert_eq!(requested.detector.level_db, None);
    assert_eq!(requested.detector.input, DetectorInput::Sidechain);
    assert_eq!(requested.detector.highpass_hz, Some(200.0));
    assert_eq!(requested.detector.lowpass_hz, Some(2000.0));
    assert_eq!(requested.gain_db, 0.0);

    let control = AuditionControl::default();
    requested.detector.level_db = Some(-18.0);
    requested.gain_db = 12.0;
    let first = control.begin(15, EffectKind::Svf, AuditionSource::Trigger, requested, 27);
    let original = control.snapshot().unwrap();
    assert_eq!(original.token, first);
    assert_eq!(original.row, 15);
    assert_eq!(original.kind, EffectKind::Svf);
    assert_eq!(original.generation, 27);
    assert_eq!(original.settings.detector.level_db, None);
    assert_eq!(original.settings.gain_db, 0.0);
    requested.detector.lowpass_hz = Some(3000.0);
    assert!(control.update(first, requested));
    let updated = control.snapshot().unwrap();
    assert_eq!(updated.settings.detector.lowpass_hz, Some(3000.0));
    assert_eq!(updated.settings.gain_db, 0.0);
    assert_eq!(
        control.begin(16, EffectKind::Svf, AuditionSource::Trigger, requested, 27),
        0
    );
    assert_eq!(control.snapshot(), Some(updated));

    let next = control.begin(15, EffectKind::Svf, AuditionSource::Trigger, requested, 27);
    assert_ne!(next, first);
    control.release(first);
    assert!(control.is_current(next));
}

#[test]
fn inactive_or_invalid_rate_returns_ordinary_audio_and_drops_the_stream() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(0);
    let ordinary = [0.125, -0.25];
    let hot = [1.0; 2];
    assert!(!runtime.needs_processing());
    control.begin(
        0,
        EffectKind::None,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        0,
    );
    for sr in [0.0, -48000.0, f64::NAN, f64::INFINITY] {
        runtime.prepare(control.snapshot(), sr);
        assert!(!runtime.needs_processing(), "rate {sr}");
        assert_eq!(
            runtime.next(ordinary, hot, hot, &control, 0, true),
            ordinary
        );
    }
    runtime.prepare(control.snapshot(), 200.0);
    assert!(runtime.needs_processing());
    assert_eq!(runtime.next(ordinary, hot, hot, &control, 0, true), hot);
    runtime.prepare(None, 200.0);
    assert_eq!(
        runtime.next(ordinary, hot, hot, &control, 0, true),
        ordinary
    );
    assert_eq!(
        runtime.next(ordinary, hot, hot, &control, 0, true),
        ordinary
    );
    assert!(!runtime.needs_processing());
    runtime.prepare(control.snapshot(), 200.0);
    runtime.reset();
    assert!(!runtime.needs_processing());
}

#[test]
fn reserve_grows_for_maximum_latency_and_keeps_a_live_smaller_queue() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(1);
    let ordinary = [0.125, -0.25];
    control.begin(
        0,
        EffectKind::None,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        0,
    );
    runtime.prepare(control.snapshot(), 200.0);
    runtime.reserve(4);
    assert!(!runtime.needs_processing());
    runtime.prepare(control.snapshot(), 200.0);
    let first = [0.5, -0.5];
    assert_eq!(
        runtime.next(ordinary, first, [0.0; 2], &control, 4, true),
        ordinary
    );
    runtime.reserve(4);
    runtime.reserve(1);
    for _ in 1..4 {
        assert_eq!(
            runtime.next(ordinary, [0.0; 2], [0.0; 2], &control, 4, true),
            ordinary
        );
    }
    assert_eq!(
        runtime.next(ordinary, [0.0; 2], [0.0; 2], &control, 4, true),
        first
    );
}

#[test]
fn kind_generation_and_source_changes_invalidate_queued_monitor_audio() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(3);
    let cfg = settings(DetectorInput::Main);
    let ordinary = [0.125, -0.25];
    let new_input = [-0.25, -0.25];
    for (kind, source, generation) in [
        (EffectKind::None, AuditionSource::Follower, 0),
        (EffectKind::Svf, AuditionSource::Follower, 0),
        (EffectKind::Svf, AuditionSource::Follower, 1),
        (EffectKind::Svf, AuditionSource::Trigger, 1),
    ] {
        control.begin(0, kind, source, cfg, generation);
        runtime.prepare(control.snapshot(), 200.0);
        for _ in 0..3 {
            assert_eq!(
                runtime.next(ordinary, new_input, [0.0; 2], &control, 3, true),
                ordinary
            );
        }
        assert_eq!(
            runtime.next(ordinary, [0.75; 2], [0.0; 2], &control, 3, true),
            new_input
        );
    }
}

#[test]
fn fade_interpolates_both_channels_and_snaps_to_the_exact_endpoints() {
    let control = AuditionControl::default();
    let mut runtime = AuditionRuntime::new(0);
    let token = control.begin(
        0,
        EffectKind::None,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        0,
    );
    let ordinary = [0.25, 0.5];
    let main = [0.75, -0.5];
    runtime.prepare(control.snapshot(), 48000.0);
    for i in 1..240 {
        let mix = i as f32 / 240.0;
        let sample = runtime.next(ordinary, main, [0.0; 2], &control, 0, true);
        assert!((sample[0] - (0.25 + 0.5 * mix)).abs() <= 1e-7);
        assert!((sample[1] - (0.5 - mix)).abs() <= 1e-7);
    }
    assert_eq!(
        runtime.next([f32::NAN; 2], main, [0.0; 2], &control, 0, true),
        main
    );
    control.release(token);
    for _ in 1..240 {
        runtime.next(ordinary, main, [0.0; 2], &control, 0, true);
    }
    let last = runtime.next([-0.0, 0.5], main, [0.0; 2], &control, 0, true);
    assert_eq!(last[0].to_bits(), (-0.0_f32).to_bits());
}

#[test]
fn endpoint_cleanup_keeps_the_exact_fade_thresholds_until_the_next_step() {
    let control = AuditionControl::default();
    let token = control.begin(
        0,
        EffectKind::None,
        AuditionSource::Follower,
        settings(DetectorInput::Main),
        0,
    );
    let mut runtime = AuditionRuntime::new(0);
    runtime.prepare(control.snapshot(), 48000.0);
    runtime.fade_step = 1e-12;
    runtime.next([0.0; 2], [0.5; 2], [0.0; 2], &control, 0, true);
    assert_eq!(runtime.blend, 1e-12);
    runtime.blend = 1.0;
    control.release(token);
    runtime.next([0.0; 2], [0.5; 2], [0.0; 2], &control, 0, true);
    assert_eq!(runtime.blend, 1.0 - 1e-12);
}

#[test]
fn incomplete_settings_are_hidden_and_each_edit_publishes_a_fresh_revision() {
    let control = AuditionControl::default();
    let settings = settings(DetectorInput::Sidechain);
    let token = control.begin(15, EffectKind::Svf, AuditionSource::Follower, settings, 0);
    let stable = control.stamp.load(Ordering::Acquire);
    control.stamp.store(stable | 1, Ordering::Release);
    assert!(control.snapshot().is_none());
    control.stamp.store(stable, Ordering::Release);
    assert_eq!(control.snapshot().unwrap().token, token);
    for expected in [stable + 2, stable + 4, stable + 6] {
        assert!(control.update(token, settings));
        assert_eq!(control.stamp.load(Ordering::Acquire), expected);
        assert_eq!(control.snapshot().unwrap().settings, settings);
    }
}

#[test]
fn concurrent_settings_snapshots_keep_input_gain_and_both_filters_together() {
    let control = AuditionControl::default();
    let quiet = AuditionSettings {
        detector: AudioTriggerSettings {
            highpass_hz: Some(200.0),
            lowpass_hz: Some(2000.0),
            ..AudioTriggerSettings::default()
        },
        gain_db: -9.0,
    };
    let loud = AuditionSettings {
        detector: AudioTriggerSettings {
            input: DetectorInput::Sidechain,
            highpass_hz: Some(400.0),
            lowpass_hz: Some(4000.0),
            ..AudioTriggerSettings::default()
        },
        gain_db: 9.0,
    };
    let token = control.begin(0, EffectKind::None, AuditionSource::Follower, quiet, 0);
    let boundary = std::sync::Barrier::new(2);
    let mut mixed = None;
    let mut stable = true;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..8 {
                boundary.wait();
                for settings in [quiet, loud].into_iter().cycle().take(2000) {
                    control.update(token, settings);
                }
                boundary.wait();
                boundary.wait();
            }
        });
        for _ in 0..8 {
            boundary.wait();
            for _ in 0..2000 {
                if let Some(request) = control.snapshot()
                    && request.settings != quiet
                    && request.settings != loud
                {
                    mixed = Some(request);
                }
            }
            boundary.wait();
            stable &= control.snapshot().map(|request| request.settings) == Some(loud);
            boundary.wait();
        }
    });
    assert!(mixed.is_none(), "inconsistent monitor settings: {mixed:?}");
    assert!(
        stable,
        "the completed settings must be visible after a write batch"
    );
}
