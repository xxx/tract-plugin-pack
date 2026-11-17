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
