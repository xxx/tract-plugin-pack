use super::*;
use crate::effects::EffectKind;
use tract_effects::modulation::{AuditionSettings, AuditionSource, DetectorInput};

#[test]
fn monitor_none_inactive_row_overrides_global_mix_without_changing_clock_or_audio_history() {
    let mut actual = Box::new(AudioEngine::new());
    let mut reference = Box::new(AudioEngine::new());
    let grid = Grid::default();
    let tracks = [TrackEffect::default(); ROWS];
    let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
    cfg[3].follower.detector.input = DetectorInput::Sidechain;
    for engine in [&mut actual, &mut reference] {
        engine.set_sample_rate(1000.0);
        engine.set_effects(&tracks);
        engine.set_modulation(&cfg);
    }
    let token = actual.telemetry.audition.begin(
        3,
        EffectKind::None,
        AuditionSource::Follower,
        AuditionSettings::for_source(&cfg[3], AuditionSource::Follower).unwrap(),
        actual.telemetry.waveform.generation(),
    );
    let (mut left, mut right) = ([0.1; 16], [-0.2; 16]);
    let (mut expected_l, mut expected_r) = (left, right);
    let side_l = [0.6; 16];
    let side_r = [-0.3; 16];
    let side = SidechainInput::new(&side_l, &side_r);
    assert_no_alloc::assert_no_alloc(|| {
        actual.process_with_inputs(
            &mut left,
            &mut right,
            side,
            true,
            3.0,
            120.0,
            0.0,
            &grid,
            std::iter::empty(),
        );
        reference.process_with_inputs(
            &mut expected_l,
            &mut expected_r,
            side,
            true,
            3.0,
            120.0,
            0.0,
            &grid,
            std::iter::empty(),
        );
    });
    assert_eq!([left[15], right[15]], [0.6, -0.3]);
    for row in 0..ROWS {
        for slot in 0..4 {
            assert_eq!(
                actual.modulation.precise_phase(row, slot),
                reference.modulation.precise_phase(row, slot)
            );
        }
    }
    actual.telemetry.audition.release(token);
    let (mut left, mut right) = ([0.1; 16], [-0.2; 16]);
    assert_no_alloc::assert_no_alloc(|| {
        actual.process_with_inputs(
            &mut left,
            &mut right,
            side,
            true,
            3.0,
            120.0,
            0.0,
            &grid,
            std::iter::empty(),
        );
    });
    assert_eq!(&left[5..], &[0.1; 11]);
    assert_eq!(&right[5..], &[-0.2; 11]);
}
