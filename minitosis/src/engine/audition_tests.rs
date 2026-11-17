use super::*;
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::{AuditionSettings, AuditionSource};

fn setup(kind: EffectKind) -> (Processor, TrackEffect, Arc<ModulationTelemetry>) {
    let track = TrackEffect {
        kind,
        params: default_params_for_kind(kind),
        ..TrackEffect::default()
    };
    let mut cfg = TrackModulation::default();
    cfg.follower.detector.input = tract_effects::modulation::DetectorInput::Sidechain;
    let mut p = Processor::new(
        PrepareKey {
            kind,
            generation: 1,
            sample_rate_bits: 1000.0_f32.to_bits(),
        },
        &track,
        cfg,
    );
    let telemetry = Arc::new(ModulationTelemetry::default());
    p.connect_waveform(&telemetry);
    (p, track, telemetry)
}
fn listen(p: &Processor, telemetry: &ModulationTelemetry) -> u64 {
    telemetry.audition.begin(
        0,
        p.key.kind,
        AuditionSource::Follower,
        AuditionSettings::for_source(&p.modulation, AuditionSource::Follower).unwrap(),
        telemetry.waveform.generation(),
    )
}
#[test]
fn listen_preserves_native_dsp_and_modulation_and_bypass_then_host_reset_has_priority() {
    let (mut monitored, track, telemetry) = setup(EffectKind::Bitcrush);
    let (mut reference, _, _) = setup(EffectKind::Bitcrush);
    let token = listen(&monitored, &telemetry);
    let mut actual = [[0.0; 2]; 12];
    let mut expected = [[0.0; 2]; 12];
    assert_no_alloc::assert_no_alloc(|| {
        for p in [&mut monitored, &mut reference] {
            p.begin_block(&track, &[0.1; 12], &[0.1; 12], 120.0);
        }
        for i in 0..12 {
            actual[i] = monitored.process_sample_with_sidechain([0.1; 2], [0.6, -0.3], 1.0, false);
            expected[i] =
                reference.process_sample_with_sidechain([0.1; 2], [0.6, -0.3], 1.0, false);
        }
    });
    assert_eq!(actual[11], [0.6, -0.3]);
    assert_eq!(monitored.modulation_frame(), reference.modulation_frame());
    assert_no_alloc::assert_no_alloc(|| {
        let bypass = monitored.process_sample_with_sidechain([0.1; 2], [0.6, -0.3], 1.0, true);
        actual[0] = bypass;
    });
    assert_eq!(actual[0], [0.1; 2]);
    assert!(telemetry.audition.is_current(token));
    telemetry.audition.release(token);
    assert_no_alloc::assert_no_alloc(|| {
        for i in 0..12 {
            actual[i] = monitored.process_sample_with_sidechain([0.1; 2], [0.6, -0.3], 1.0, false);
            expected[i] =
                reference.process_sample_with_sidechain([0.1; 2], [0.6, -0.3], 1.0, false);
        }
    });
    assert_eq!(&actual[5..], &expected[5..]);
    listen(&monitored, &telemetry);
    assert_no_alloc::assert_no_alloc(|| {
        monitored.flush_dsp();
    });
    assert!(telemetry.audition.snapshot().is_none());
}
#[test]
fn monitor_impulse_follows_native_reported_latency_and_routes_can_remain_off() {
    let (mut p, track, telemetry) = setup(EffectKind::WarpZone);
    assert_eq!(p.latency(), 4096);
    assert_eq!(p.modulation.follower.mask(), 0);
    listen(&p, &telemetry);
    let mut first = [0.0; 3];
    assert_no_alloc::assert_no_alloc(|| {
        p.begin_block(&track, &[0.0; 4098], &[0.0; 4098], 120.0);
        for i in 0..4098 {
            let out = p.process_sample_with_sidechain(
                [0.0; 2],
                [if i == 0 { 1.0 } else { 0.0 }; 2],
                1.0,
                false,
            );
            if i >= 4095 {
                first[i - 4095] = out[0];
            }
        }
    });
    assert_eq!(first, [0.0, 0.2, 0.0]);
    assert_eq!(p.latency(), 4096);
}
#[test]
fn pending_gui_filter_handoff_suspends_without_revoking_and_preset_reset_prevents_revival() {
    let (mut p, track, telemetry) = setup(EffectKind::None);
    let token = listen(&p, &telemetry);
    let mut settings =
        AuditionSettings::for_source(&p.modulation, AuditionSource::Follower).unwrap();
    settings.detector.highpass_hz = Some(100.0);
    telemetry.audition.update(token, settings);
    assert_no_alloc::assert_no_alloc(|| {
        p.begin_block(&track, &[0.2; 8], &[0.2; 8], 120.0);
    });
    assert!(telemetry.audition.is_current(token));
    p.modulation.follower.detector.highpass_hz = Some(100.0);
    let mut out = [0.0; 2];
    assert_no_alloc::assert_no_alloc(|| {
        p.begin_block(&track, &[0.2; 16], &[0.2; 16], 120.0);
        for _ in 0..16 {
            out = p.process_sample_with_sidechain([0.2; 2], [0.0; 2], 1.0, false);
        }
    });
    assert_eq!(out, [0.0; 2]);
    assert!(telemetry.audition.is_current(token));
    p.flush_dsp();
    p.modulation.follower.detector.highpass_hz = None;
    p.modulation.follower.detector.highpass_hz = Some(100.0);
    p.begin_block(&track, &[0.2; 8], &[0.2; 8], 120.0);
    for _ in 0..8 {
        out = p.process_sample_with_sidechain([0.2; 2], [0.0; 2], 1.0, false);
    }
    assert_eq!(out, [0.2; 2]);
    assert!(telemetry.audition.snapshot().is_none());
}
