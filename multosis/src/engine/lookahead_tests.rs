use super::*;
use crate::effects::EffectKind;
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
use tract_effects::modulation::{TransientAlgorithm, TransientSettings, TriggerSource};

fn setup() -> (Box<AudioEngine>, Grid, [TrackEffect; ROWS]) {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_sample_rate(1000.0);
    engine.set_compressor(0.0, 1.0);
    let tracks = [TrackEffect::default(); ROWS];
    engine.set_effects(&tracks);
    let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
    for (row, cfg) in cfg.iter_mut().enumerate() {
        cfg.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 5.0,
        };
        cfg.detector_response = TransientSettings {
            algorithm: TransientAlgorithm::Peak,
            detail: 100.0,
        };
        cfg.lookahead = [0, 3, 7].contains(&row);
        cfg.targets = [None; 3];
        cfg.msegs = std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..MsegData::default()
        });
    }
    engine.set_modulation(&cfg);
    (engine, Grid::default(), tracks)
}
#[test]
fn inactive_none_rows_delay_global_dry_and_wet_and_compensate_downstream_cues() {
    for mix in [0.0, 0.5, 1.0] {
        let (mut engine, grid, _) = setup();
        assert_eq!(engine.chain_latency_samples(), 0);
        engine.process(&mut [], &mut [], false, 100.0, 120.0, mix, &grid);
        assert_eq!(engine.chain_latency_samples(), 0);
        let mut left: [f32; 24] = std::array::from_fn(|i| if i == 2 { 0.5 } else { 0.0 });
        let mut right = left;
        assert_no_alloc::assert_no_alloc(|| {
            engine.process(&mut left, &mut right, false, 100.0, 120.0, mix, &grid);
        });
        assert_eq!(engine.chain_latency_samples(), 12);
        for (i, value) in left.into_iter().enumerate() {
            assert_eq!(value, if i == 14 { 0.5 } else { 0.0 });
        }
        assert_eq!(right[14], 0.5);
        assert!((engine.modulation.precise_phase(1, 0) - 0.017).abs() < 1e-12);
        assert!((engine.modulation.precise_phase(8, 0) - 0.009).abs() < 1e-12);
        assert_eq!(
            engine.modulation.frame(8, EffectKind::None).outputs[0],
            engine.modulation.phase(8, 0)
        );
    }
}
#[test]
fn mute_and_solo_paths_adopt_only_with_real_samples() {
    let (mut engine, grid, mut tracks) = setup();
    engine.prepare_lookahead();
    assert_eq!(engine.chain_latency_samples(), 12);
    tracks[3].muted = true;
    engine.set_effects(&tracks);
    engine.process(&mut [], &mut [], false, 100.0, 120.0, 1.0, &grid);
    assert_eq!(engine.chain_latency_samples(), 12);
    assert_no_alloc::assert_no_alloc(|| {
        engine.process(
            &mut [0.0; 16],
            &mut [0.0; 16],
            false,
            100.0,
            120.0,
            1.0,
            &grid,
        );
    });
    assert_eq!(engine.chain_latency_samples(), 8);
    tracks[7].soloed = true;
    engine.set_effects(&tracks);
    engine.process(
        &mut [0.0; 16],
        &mut [0.0; 16],
        false,
        100.0,
        120.0,
        1.0,
        &grid,
    );
    assert_eq!(engine.chain_latency_samples(), 4);
}
#[test]
fn captured_series_input_and_follower_original_main_are_distinct_after_lookahead() {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_sample_rate(1000.0);
    engine.set_compressor(0.0, 1.0);
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[0].kind = EffectKind::Bitcrush;
    tracks[0].params = crate::effects::default_params_for_kind(tracks[0].kind);
    tracks[0].params[0] = 2.0;
    engine.set_effects(&tracks);
    let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
    cfg[0].targets = [None; 3];
    cfg[1].trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 5.0,
    };
    cfg[1].detector_response = TransientSettings {
        algorithm: TransientAlgorithm::Peak,
        detail: 100.0,
    };
    cfg[1].lookahead = true;
    cfg[1].follower.routes[0].enabled = true;
    cfg[1].follower.attack_ms = 0.0;
    cfg[1].follower.release_ms = 0.0;
    engine.set_modulation(&cfg);
    let mut grid = Grid::default();
    for col in 0..crate::grid::COLS {
        grid.cell_mut(0, col).enabled = true;
    }
    let token = engine.telemetry.waveform.request(1, 0, EffectKind::None);
    assert_no_alloc::assert_no_alloc(|| {
        engine.process(
            &mut [0.3; 12],
            &mut [0.3; 12],
            false,
            100.0,
            120.0,
            1.0,
            &grid,
        );
    });
    let point = engine
        .telemetry
        .waveform
        .point(engine.telemetry.waveform.head() - 1, token)
        .unwrap();
    assert_eq!(point.input, 0.5);
    assert_eq!(
        engine
            .modulation
            .frame(1, EffectKind::None)
            .follower
            .unwrap()
            .envelope,
        0.3
    );
}
