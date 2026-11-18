use super::*;
use crate::effects::{EffectKind, default_params_for_kind};

#[test]
fn empty_route_off_config_bridges_preserve_the_held_effect_value() {
    let mut actual = Box::new(AudioEngine::new());
    let mut reference = Box::new(AudioEngine::new());
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[0].kind = EffectKind::Svf;
    tracks[0].params = default_params_for_kind(tracks[0].kind);
    let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
    cfg[0].follower.routes[1].enabled = true;
    cfg[0].follower.attack_ms = 0.0;
    cfg[0].follower.release_ms = 0.0;
    cfg[0].follower.detector.input = crate::modulation::DetectorInput::Sidechain;
    cfg[0].depths[0] = 0.7;
    let mut off = cfg.clone();
    off[0].follower.routes[1].enabled = false;
    let mut grid = Grid::default();
    for row in 0..ROWS {
        grid.cell_mut(row, 0).enabled = row == 0;
    }
    let side = [1.0; 8];
    let mut output = [0.5; 8];
    let mut right = output;
    let mut expected = output;
    let mut expected_r = output;
    for engine in [&mut actual, &mut reference] {
        engine.set_compressor(0.0, 1.0);
        engine.set_modulation(&cfg);
        engine.set_effects(&tracks);
    }
    assert_no_alloc::assert_no_alloc(|| {
        for engine in [&mut actual, &mut reference] {
            engine.process_with_inputs(
                &mut [0.5; 5],
                &mut [0.5; 5],
                SidechainInput::new(&side, &side),
                false,
                1e9,
                120.0,
                1.0,
                &grid,
                std::iter::empty(),
            );
        }
        actual.set_modulation(&off);
        actual.set_effects(&tracks);
        actual.process(&mut [], &mut [], false, 1e9, 120.0, 1.0, &grid);
        actual.set_modulation(&cfg);
        actual.set_effects(&tracks);
        actual.process_with_inputs(
            &mut output,
            &mut right,
            SidechainInput::new(&side, &side),
            false,
            1e9,
            120.0,
            1.0,
            &grid,
            std::iter::empty(),
        );
        reference.process_with_inputs(
            &mut expected,
            &mut expected_r,
            SidechainInput::new(&side, &side),
            false,
            1e9,
            120.0,
            1.0,
            &grid,
            std::iter::empty(),
        );
    });
    assert_eq!(output, expected);
    assert_eq!(right, expected_r);
}

#[test]
fn followers_receive_original_main_audio_through_active_muted_inactive_and_none_rows() {
    for mode in 0..4 {
        let mut engine = Box::new(AudioEngine::new());
        engine.set_compressor(0.0, 1.0);
        let mut tracks = [TrackEffect::default(); ROWS];
        tracks[0].kind = EffectKind::Svf;
        tracks[0].params = default_params_for_kind(tracks[0].kind);
        tracks[1].kind = if mode == 2 {
            EffectKind::None
        } else {
            EffectKind::Bitcrush
        };
        tracks[1].params = default_params_for_kind(tracks[1].kind);
        tracks[1].muted = mode == 1;
        engine.set_effects(&tracks);
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[1].follower.routes[0].enabled = true;
        cfg[1].follower.attack_ms = 10.0;
        cfg[1].targets = [None; 3];
        engine.set_modulation(&cfg);
        let mut grid = Grid::default();
        for row in 0..ROWS {
            grid.cell_mut(row, 0).enabled = row == 0 || (row == 1 && mode != 3);
        }
        assert_no_alloc::assert_no_alloc(|| {
            engine.process(
                &mut [0.5; 48],
                &mut [-0.5; 48],
                false,
                1e9,
                120.0,
                1.0,
                &grid,
            );
        });
        let frame = engine.modulation.frame(1, tracks[1].kind);
        let reading = frame.follower.unwrap();
        assert_eq!(reading.level, 0.5);
        assert!((reading.envelope - 0.5 * (1.0 - (-0.1_f32).exp())).abs() < 1e-6);
    }
}
