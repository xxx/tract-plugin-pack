use super::*;
use tiny_skia_widgets::mseg::{FactoryShape, MsegData, PlayMode, SyncMode, factory_shape};
use tract_effects::modulation::{MidiWaveMapping, TriggerSource};

fn setup(waves: bool) -> (Minitosis, Context) {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::Bitcrush);
    let mut config = state::neutral_modulation();
    config.targets = [None; 3];
    config.depths = [0.0; 3];
    config.trigger = TriggerSource::Free;
    config.msegs[0] = MsegData {
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        play_mode: PlayMode::Cyclic,
        ..MsegData::default()
    };
    config.msegs[0].nodes[0].value = if waves { 0.0 } else { 1.0 };
    config.msegs[0].nodes[1].value = if waves { 0.0 } else { 1.0 };
    config.wave_switch = waves.then_some(MidiWaveMapping {
        envelope: 0,
        first_note: 36,
        channel: Some(2),
    });
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Bitcrush, config);
    plugin
        .params
        .wave_palette
        .lock()
        .unwrap()
        .store(1, &factory_shape(FactoryShape::FlatHigh))
        .unwrap();
    initialize(&mut plugin, &mut context);
    (plugin, context)
}
fn block(plugin: &mut Minitosis, context: &mut Context) -> [f32; 64] {
    let (mut left, mut right) = ([0.37; 64], [0.37; 64]);
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, false, 120.0, context)
    });
    left
}

#[test]
fn native_wave_notes_keep_offsets_and_ignore_zero_velocity_wrong_channels_and_empty_slots() {
    for offset in [0, 1, 31, 63] {
        let (mut plugin, mut context) = setup(true);
        let (mut reference, mut reference_context) = setup(false);
        let saved = plugin
            .params
            .modulation
            .lock()
            .unwrap()
            .get(EffectKind::Bitcrush)
            .clone();
        context.events = vec![
            midi_note(0, 3, 37, 1.0),
            midi_note(0, 2, 36, 1.0),
            midi_note(0, 2, 37, 0.0),
            midi_note(offset, 2, 37, 1.0),
            midi_note(64, 2, 37, 1.0),
        ];
        let actual = block(&mut plugin, &mut context);
        let expected = block(&mut reference, &mut reference_context);
        assert!(actual[..offset as usize].iter().all(|value| *value == 0.0));
        assert_eq!(&actual[offset as usize..], &expected[offset as usize..]);
        assert_eq!(plugin.processor.phase(0), reference.processor.phase(0));
        assert_eq!(
            plugin
                .params
                .modulation
                .lock()
                .unwrap()
                .get(EffectKind::Bitcrush),
            &saved
        );
        assert_eq!(plugin.processor.modulation_frame().wave.unwrap().1, Some(1));
    }
}

#[test]
fn native_bank_contention_keeps_ready_data_then_refreshes_without_an_editor_or_allocations() {
    let (mut plugin, mut context) = setup(true);
    block(&mut plugin, &mut context);
    let binding = plugin.params.wave_palette.clone();
    let mut locked = binding.lock().unwrap();
    let mut zero = MsegData::default();
    zero.nodes[1].value = 0.0;
    locked
        .store(1, &tiny_skia_widgets::mseg::CurveShape::capture(&zero))
        .unwrap();
    context.events = vec![midi_note(0, 2, 37, 1.0)];
    context.event_index = 0;
    let ready = block(&mut plugin, &mut context);
    assert!(ready.iter().all(|value| *value > 0.2));
    drop(locked);
    context.events.clear();
    context.event_index = 0;
    let refreshed = block(&mut plugin, &mut context);
    assert_eq!(refreshed, [0.0; 64]);
    assert_eq!(plugin.processor.modulation_frame().wave.unwrap().1, Some(1));
}

#[test]
fn native_mapping_disable_and_reset_restore_author_data_without_allocation() {
    let (mut plugin, mut context) = setup(true);
    context.events = vec![midi_note(0, 2, 37, 1.0)];
    block(&mut plugin, &mut context);
    let mut config = plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .get(EffectKind::Bitcrush)
        .clone();
    config.wave_switch = None;
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Bitcrush, config.clone());
    plugin
        .shared
        .modulation_dirty
        .store(true, Ordering::Release);
    context.events.clear();
    context.event_index = 0;
    assert_eq!(block(&mut plugin, &mut context), [0.0; 64]);
    assert_eq!(plugin.processor.modulation_frame().wave, None);
    config.wave_switch = Some(MidiWaveMapping {
        envelope: 0,
        first_note: 36,
        channel: Some(2),
    });
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Bitcrush, config);
    plugin
        .shared
        .modulation_dirty
        .store(true, Ordering::Release);
    context.events = vec![midi_note(0, 2, 37, 1.0)];
    block(&mut plugin, &mut context);
    plugin
        .shared
        .reset_modulation
        .store(true, Ordering::Release);
    context.events.clear();
    context.event_index = 0;
    assert_eq!(block(&mut plugin, &mut context), [0.0; 64]);
    assert_eq!(plugin.processor.modulation_frame().wave.unwrap().1, None);
}

#[test]
fn smoothed_wave_note_at_a_later_block_start_matches_an_interior_note() {
    let (mut actual, mut context) = setup(true);
    let (mut reference, mut reference_context) = setup(true);
    for plugin in [&mut actual, &mut reference] {
        let mut config = plugin.processor.modulation.clone();
        config.smooth_ms[0] = 10.0;
        plugin
            .params
            .modulation
            .lock()
            .unwrap()
            .set(EffectKind::Bitcrush, config);
        plugin
            .shared
            .modulation_dirty
            .store(true, Ordering::Release);
    }
    context.events = vec![midi_note(0, 2, 37, 1.0)];
    reference_context.events = vec![midi_note(5, 2, 37, 1.0)];
    let (mut warm_l, mut warm_r) = ([0.37; 5], [0.37; 5]);
    let (mut left, mut right) = ([0.37; 8], [0.37; 8]);
    let (mut expected_l, mut expected_r) = ([0.37; 13], [0.37; 13]);
    let mut silent_context = Context::default();
    assert_no_alloc::assert_no_alloc(|| {
        actual.render(&mut warm_l, &mut warm_r, false, 120.0, &mut silent_context);
        actual.render(&mut left, &mut right, false, 120.0, &mut context);
        reference.render(
            &mut expected_l,
            &mut expected_r,
            false,
            120.0,
            &mut reference_context,
        );
    });
    assert_eq!(warm_l, expected_l[..5]);
    assert_eq!(left, expected_l[5..]);
    assert_eq!(right, expected_r[5..]);
    assert!(left[0] > 0.0);
    assert_eq!(actual.processor.phase(0), reference.processor.phase(0));
}
