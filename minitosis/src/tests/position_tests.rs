use super::*;
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
use tract_effects::modulation::{HostPosition, TriggerSource};

#[test]
fn native_position_relocks_after_reset_and_effect_preparation_and_keeps_bypass_tracking() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::Bitcrush);
    let config = tract_effects::modulation::TrackModulation {
        trigger: TriggerSource::DawPosition,
        targets: [None; 3],
        msegs: std::array::from_fn(|_| MsegData {
            sync_mode: SyncMode::Beat,
            beats: 4.0,
            play_mode: PlayMode::Cyclic,
            ..MsegData::default()
        }),
        ..state::neutral_modulation()
    };
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Bitcrush, config);
    initialize(&mut plugin, &mut context);
    plugin.host_position = HostPosition {
        playing: true,
        beats: Some(3.0),
        seconds: Some(1.5),
        bpm: 120.0,
        ..HostPosition::default()
    };
    let mut left = [0.37; 8];
    let mut right = left;
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, true, 120.0, &mut context);
    });
    assert!(left[0] > 0.27 && left[0] < 0.29);
    assert!((plugin.processor.phase(0) - (3.0_f32 + 7.0 / 24000.0) / 4.0).abs() < 1e-7);
    plugin.reset();
    plugin.host_position.beats = Some(1.0);
    plugin.host_position.seconds = Some(0.5);
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, false, 120.0, &mut context);
    });
    assert!((plugin.processor.phase(0) - (1.0_f32 + 7.0 / 24000.0) / 4.0).abs() < 1e-7);
    let params = Arc::get_mut(&mut plugin.params).unwrap();
    params.bypass = BoolParam::new("Bypass", true);
    plugin.host_position.beats = Some(2.0);
    plugin.host_position.seconds = Some(1.0);
    left.fill(0.37);
    right.fill(0.37);
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, false, 120.0, &mut context);
    });
    assert_eq!(left, [0.37; 8]);
    assert!((plugin.processor.phase(0) - (2.0_f32 + 7.0 / 24000.0) / 4.0).abs() < 1e-7);
}
