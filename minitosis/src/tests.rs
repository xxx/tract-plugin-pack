use super::*;
use std::cell::Cell;
use tract_effects::effects::EffectKind;
use tract_effects::modulation::ModulationTarget;

#[derive(Default)]
struct Context {
    request: Cell<Option<PrepareKey>>,
    requests: Cell<usize>,
    latency: Cell<u32>,
    events: Vec<PluginNoteEvent<Minitosis>>,
    event_index: usize,
}
impl ProcessContext<Minitosis> for Context {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute_background(&self, task: PrepareKey) {
        self.request.set(Some(task));
        self.requests.set(self.requests.get() + 1);
    }
    fn execute_gui(&self, _: PrepareKey) {}
    fn transport(&self) -> &Transport {
        panic!("transport is passed explicitly to the rendering seam")
    }
    fn next_event(&mut self) -> Option<PluginNoteEvent<Minitosis>> {
        let event = self.events.get(self.event_index).copied();
        if event.is_some() {
            self.event_index += 1;
        }
        event
    }
    fn send_event(&mut self, _: PluginNoteEvent<Minitosis>) {}
    fn set_latency_samples(&self, samples: u32) {
        self.latency.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}

impl InitContext<Minitosis> for Context {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn execute(&self, _: PrepareKey) {}
    fn set_latency_samples(&self, samples: u32) {
        self.latency.set(samples);
    }
    fn set_current_voice_capacity(&self, _: u32) {}
}

fn select(plugin: &mut Minitosis, kind: EffectKind) {
    let params = Arc::get_mut(&mut plugin.params).unwrap();
    params.effect = IntParam::new(
        "Effect",
        state::bank_index(kind) as i32,
        IntRange::Linear { min: 0, max: 49 },
    )
    .non_automatable();
    params.selection_generation.fetch_add(1, Ordering::AcqRel);
}

fn initialize(plugin: &mut Minitosis, context: &mut Context) {
    plugin
        .params
        .output
        .smoothed
        .reset(plugin.params.output.value());
    let config = BufferConfig {
        sample_rate: 48000.0,
        min_buffer_size: Some(1),
        max_buffer_size: 1024,
        process_mode: ProcessMode::Realtime,
    };
    assert!(plugin.initialize(&Minitosis::AUDIO_IO_LAYOUTS[0], &config, context));
    plugin.reset();
}

fn render(
    plugin: &mut Minitosis,
    context: &mut Context,
    playing: bool,
) -> ([f32; 512], [f32; 512]) {
    let mut left = [0.25; 512];
    let mut right = [-0.5; 512];
    let status = assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, playing, 120.0, context)
    });
    assert!(matches!(status, ProcessStatus::KeepAlive));
    (left, right)
}

fn midi_plugin(kind: EffectKind, smooth: f32, bypass: bool) -> (Minitosis, Context) {
    use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, kind);
    Arc::get_mut(&mut plugin.params).unwrap().bypass = BoolParam::new("Bypass", bypass);
    let mut curve = MsegData {
        play_mode: PlayMode::OneShot,
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        ..MsegData::default()
    };
    curve.insert_node(0.5, 1.0).unwrap();
    curve.nodes[0].stepped = true;
    let mut modulation = state::neutral_modulation();
    modulation.msegs = [curve; 4];
    modulation.smooth_ms = [smooth; 4];
    modulation.trigger = tract_effects::modulation::TriggerSource::MidiNote {
        note: Some(60),
        channel: Some(2),
    };
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(kind, modulation);
    initialize(&mut plugin, &mut context);
    let mut left = vec![0.25; 36000];
    let mut right = vec![-0.5; 36000];
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, false, 120.0, &mut context)
    });
    (plugin, context)
}

fn midi_note(timing: u32, channel: u8, note: u8, velocity: f32) -> PluginNoteEvent<Minitosis> {
    NoteEvent::NoteOn {
        timing,
        voice_id: None,
        channel,
        note,
        velocity,
    }
}

#[test]
fn midi_offsets_filters_and_last_sample_retrigger_preserve_bypass_and_none() {
    assert_eq!(Minitosis::MIDI_INPUT, MidiConfig::Basic);
    assert_eq!(Minitosis::MIDI_OUTPUT, MidiConfig::None);
    for (kind, bypass) in [
        (EffectKind::Bitcrush, false),
        (EffectKind::Bitcrush, true),
        (EffectKind::None, false),
    ] {
        let (mut plugin, mut context) = midi_plugin(kind, 0.0, bypass);
        context.events = vec![
            midi_note(5, 2, 61, 1.0),
            midi_note(7, 3, 60, 1.0),
            midi_note(9, 2, 60, 0.0),
            NoteEvent::NoteOff {
                timing: 11,
                voice_id: None,
                channel: 2,
                note: 60,
                velocity: 1.0,
            },
            midi_note(17, 2, 60, 1.0),
            midi_note(17, 2, 60, 1.0),
            midi_note(127, 2, 60, 1.0),
            midi_note(511, 2, 60, 1.0),
            midi_note(512, 2, 60, 1.0),
        ];
        let (left, right) = render(&mut plugin, &mut context, false);
        if kind == EffectKind::Bitcrush && !bypass {
            assert!(left[..17].iter().all(|sample| *sample > 0.24));
            assert!(left[17..].iter().all(|sample| *sample == 0.0));
        } else {
            assert_eq!(left, [0.25; 512]);
            assert_eq!(right, [-0.5; 512]);
        }
        for slot in 0..4 {
            assert!((plugin.processor.phase(slot) - 1.0 / 48000.0).abs() < 1e-7);
        }
        assert_eq!(context.event_index, context.events.len());
    }
}

#[test]
fn midi_retrigger_retains_smoothing_and_streams_dense_or_empty_buffers_without_allocating() {
    let (mut plugin, mut context) = midi_plugin(EffectKind::Bitcrush, 20.0, false);
    let before = plugin.processor.modulation_frame().outputs[0];
    context.events = (0..10000).map(|_| midi_note(0, 2, 60, 1.0)).collect();
    let (left, _) = render(&mut plugin, &mut context, false);
    assert!(left[0] > 0.24, "MIDI retrigger cleared the lag");
    let after = plugin.processor.modulation_frame().outputs[0];
    assert!(after > 0.5 && after < before);
    assert_eq!(context.event_index, 10000);
    for slot in 0..4 {
        assert!((plugin.processor.phase(slot) - 512.0 / 48000.0).abs() < 1e-6);
    }
    context.events = vec![midi_note(0, 2, 60, 1.0), midi_note(u32::MAX, 2, 60, 1.0)];
    context.event_index = 0;
    let phases = std::array::from_fn::<_, 4, _>(|slot| plugin.processor.phase(slot));
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut [], &mut [], false, 120.0, &mut context)
    });
    assert_eq!(context.event_index, 2);
    assert_eq!(
        std::array::from_fn::<_, 4, _>(|slot| plugin.processor.phase(slot)),
        phases
    );
}

#[test]
fn ignored_midi_does_not_change_legacy_audio_or_modulation_phase() {
    let (mut reference, mut reference_context) = midi_plugin(EffectKind::Bitcrush, 0.0, false);
    let (mut actual, mut context) = midi_plugin(EffectKind::Bitcrush, 0.0, false);
    for p in [&mut reference, &mut actual] {
        p.processor.modulation.trigger = tract_effects::modulation::TriggerSource::Free;
    }
    context.events = (0..512)
        .map(|timing| midi_note(timing, 2, 60, 1.0))
        .collect();
    assert_eq!(
        render(&mut actual, &mut context, false),
        render(&mut reference, &mut reference_context, false)
    );
    for slot in 0..4 {
        assert_eq!(
            actual.processor.phase(slot),
            reference.processor.phase(slot)
        );
    }
    assert_eq!(context.event_index, 512);
}

#[test]
fn pending_and_adopted_effects_use_their_own_recalled_midi_filters() {
    use tract_effects::modulation::TriggerSource;
    let (mut plugin, mut context) = midi_plugin(EffectKind::Bitcrush, 0.0, false);
    let mut target = plugin.processor.modulation.clone();
    target.trigger = TriggerSource::MidiNote {
        note: Some(61),
        channel: Some(5),
    };
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Svf, target);
    select(&mut plugin, EffectKind::Svf);
    context.events = vec![midi_note(17, 5, 61, 1.0), midi_note(63, 2, 60, 1.0)];
    context.event_index = 0;
    let (left, right) = render(&mut plugin, &mut context, false);
    assert_eq!(plugin.processor.key.kind, EffectKind::Bitcrush);
    assert_eq!(left, [0.25; 512]);
    assert_eq!(right, [-0.5; 512]);
    assert!((plugin.processor.phase(0) - 449.0 / 48000.0).abs() < 1e-7);
    let key = context.request.get().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, key);
    context.events = vec![midi_note(3, 2, 60, 1.0), midi_note(511, 5, 61, 1.0)];
    context.event_index = 0;
    render(&mut plugin, &mut context, false);
    assert_eq!(plugin.processor.key.kind, EffectKind::Svf);
    assert!((plugin.processor.phase(0) - 1.0 / 48000.0).abs() < 1e-7);
}

#[test]
fn midi_does_not_jump_an_interrupted_constant_mix_ramp() {
    let (mut actual, mut context) = midi_plugin(EffectKind::Bitcrush, 0.0, false);
    let (mut reference, mut reference_context) = midi_plugin(EffectKind::Bitcrush, 0.0, false);
    for plugin in [&mut actual, &mut reference] {
        let params = Arc::get_mut(&mut plugin.params).unwrap();
        params.effects[state::bank_index(EffectKind::Bitcrush)].p0 =
            FloatParam::new("Bits", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 });
        params.mix = FloatParam::new("Mix", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 });
        for curve in &mut plugin.processor.modulation.msegs {
            for node in &mut curve.nodes {
                node.value = 1.0;
            }
        }
        plugin.processor.modulation.targets[0] = Some(ModulationTarget::Mix);
        plugin.processor.modulation.depths[0] = 1.0;
    }
    context.events = vec![midi_note(17, 2, 60, 1.0)];
    let (mut left, mut right, mut expected_left, mut expected_right) =
        ([0.6; 512], [-0.6; 512], [0.6; 512], [-0.6; 512]);
    assert_no_alloc::assert_no_alloc(|| {
        actual.render(&mut left, &mut right, false, 120.0, &mut context);
        reference.render(
            &mut expected_left,
            &mut expected_right,
            false,
            120.0,
            &mut reference_context,
        );
    });
    assert!(
        expected_left[511] > 0.9,
        "wet and dry must differ to expose Mix jumps"
    );
    assert_eq!(left, expected_left);
    assert_eq!(right, expected_right);
}

#[test]
fn fresh_plugin_is_identity_and_clocks_advance_while_stopped() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    assert_eq!(
        render(&mut plugin, &mut context, false),
        ([0.25; 512], [-0.5; 512])
    );
    let before = plugin.processor.phase(1);
    render(&mut plugin, &mut context, false);
    assert!(plugin.processor.phase(1) > before);
    assert_eq!(context.requests.get(), 0);
}

#[test]
fn transport_start_retrigger_is_optional_and_does_not_stop_clocks() {
    for enabled in [false, true] {
        let mut plugin = Minitosis::default();
        let mut context = Context::default();
        Arc::get_mut(&mut plugin.params).unwrap().retrig_on_play =
            BoolParam::new("Retrig", enabled);
        initialize(&mut plugin, &mut context);
        render(&mut plugin, &mut context, false);
        let one_block = plugin.processor.phase(1);
        render(&mut plugin, &mut context, true);
        let expected = one_block * if enabled { 1.0 } else { 2.0 };
        assert!((plugin.processor.phase(1) - expected).abs() < 1e-7);
    }
}

#[test]
fn closed_editor_switch_prepares_and_adopts_without_audio_thread_allocation() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    select(&mut plugin, EffectKind::Svf);
    assert_eq!(
        render(&mut plugin, &mut context, false),
        ([0.25; 512], [-0.5; 512])
    );
    let key = context.request.take().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, key);
    render(&mut plugin, &mut context, false);
    assert_eq!(plugin.processor.key, key);
    assert_eq!(
        plugin.shared.active_generation.load(Ordering::Acquire),
        key.generation
    );
}

#[test]
fn pending_switch_stays_dry_when_the_previous_effect_has_a_mix_assignment() {
    let mut plugin = Minitosis::default();
    select(&mut plugin, EffectKind::Bitcrush);
    let params = Arc::get_mut(&mut plugin.params).unwrap();
    params.effects[state::bank_index(EffectKind::Bitcrush)].p0 =
        FloatParam::new("Bits", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 });
    let mut modulation = state::neutral_modulation();
    modulation.targets[0] = Some(ModulationTarget::Mix);
    modulation.depths[0] = 1.0;
    for node in &mut modulation.msegs[1].nodes {
        node.value = 1.0;
    }
    params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Bitcrush, modulation);
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    let dry = ([0.25; 512], [-0.5; 512]);
    assert_ne!(render(&mut plugin, &mut context, false), dry);
    select(&mut plugin, EffectKind::Svf);
    assert_eq!(render(&mut plugin, &mut context, false), dry);
}

#[test]
fn rapid_a_b_a_switch_discards_b_and_installs_a_fresh_processor() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::Delay);
    initialize(&mut plugin, &mut context);
    render(&mut plugin, &mut context, false);
    let original = plugin.processor.key;
    select(&mut plugin, EffectKind::Svf);
    render(&mut plugin, &mut context, false);
    let obsolete = context.request.take().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, obsolete);
    select(&mut plugin, EffectKind::Delay);
    render(&mut plugin, &mut context, false);
    assert_eq!(plugin.processor.key, original);
    let wanted = context.request.take().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, wanted);
    render(&mut plugin, &mut context, false);
    assert_eq!(plugin.processor.key, wanted);
    assert_ne!(wanted.generation, original.generation);
}

#[test]
fn dropped_background_requests_are_retried_but_running_work_is_not_duplicated() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    select(&mut plugin, EffectKind::Svf);
    render(&mut plugin, &mut context, false);
    let key = context.request.take().unwrap();
    for _ in 0..24 {
        render(&mut plugin, &mut context, false);
    }
    assert_eq!(context.requests.get(), 2);
    plugin
        .shared
        .preparing
        .store(key.generation, Ordering::Release);
    for _ in 0..24 {
        render(&mut plugin, &mut context, false);
    }
    assert_eq!(context.requests.get(), 2);
    plugin.shared.preparing.store(0, Ordering::Release);
    render(&mut plugin, &mut context, false);
    assert_eq!(context.requests.get(), 3);
}

#[test]
fn initialize_restores_selected_effect_and_invalidates_queued_work() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    select(&mut plugin, EffectKind::SpectralShift);
    render(&mut plugin, &mut context, false);
    let stale = context.request.take().unwrap();
    initialize(&mut plugin, &mut context);
    let installed = plugin.processor.key;
    assert_eq!(installed.kind, EffectKind::SpectralShift);
    assert_ne!(installed.generation, stale.generation);
    prepare_processor(&plugin.params, &plugin.shared, stale);
    render(&mut plugin, &mut context, false);
    assert_eq!(plugin.processor.key, installed);
    assert!(context.latency.get() > 0);
}

#[test]
fn phaser_filter_cutoff_and_mix_msegs_change_audio_after_live_edits() {
    for target in [ModulationTarget::EffectParameter(0), ModulationTarget::Mix] {
        let mut reference = Minitosis::default();
        let mut modulated = Minitosis::default();
        for plugin in [&mut reference, &mut modulated] {
            select(plugin, EffectKind::PhaserFilter);
            Arc::get_mut(&mut plugin.params).unwrap().mix =
                FloatParam::new("Mix", 0.25, FloatRange::Linear { min: 0.0, max: 1.0 });
            initialize(plugin, &mut Context::default());
        }

        let mut modulation = state::neutral_modulation();
        modulation.targets[0] = Some(target);
        modulation.depths[0] = 0.6;
        modulation.msegs[1].sync_mode = tiny_skia_widgets::SyncMode::Time;
        modulation.msegs[1].time_seconds = 0.5;
        modulation.msegs[1].polarity = tiny_skia_widgets::Polarity::Bipolar;
        modulated
            .params
            .modulation
            .lock()
            .unwrap()
            .set(EffectKind::PhaserFilter, modulation);
        modulated
            .shared
            .modulation_dirty
            .store(true, Ordering::Release);

        let base = modulated
            .params
            .controls_for(EffectKind::PhaserFilter)
            .p0
            .value();
        let mut context = Context::default();
        let mut difference = 0.0_f64;
        let mut signal_energy = 0.0_f64;
        for block in 0..128 {
            let input: [f32; 512] = std::array::from_fn(|i| {
                let time = (block * 512 + i) as f32 / 48000.0;
                [200.0, 1000.0, 5000.0]
                    .into_iter()
                    .map(|hz| (std::f32::consts::TAU * hz * time).sin() * 0.1)
                    .sum()
            });
            let mut reference_left = input;
            let mut reference_right = input;
            let mut modulated_left = input;
            let mut modulated_right = input;
            assert_no_alloc::assert_no_alloc(|| {
                reference.render(
                    &mut reference_left,
                    &mut reference_right,
                    false,
                    120.0,
                    &mut context,
                );
                modulated.render(
                    &mut modulated_left,
                    &mut modulated_right,
                    false,
                    120.0,
                    &mut context,
                );
            });
            for (reference, modulated) in reference_left.into_iter().zip(modulated_left) {
                difference += f64::from(modulated - reference).powi(2);
                signal_energy += f64::from(reference).powi(2);
            }
        }
        assert!(
            difference > signal_energy * 0.01,
            "{target:?} modulation should change the rendered audio: difference={difference}, signal={signal_energy}"
        );
        assert_eq!(
            modulated
                .params
                .controls_for(EffectKind::PhaserFilter)
                .p0
                .value(),
            base
        );
        assert_eq!(modulated.params.mix.value(), 0.25);
    }
}

#[test]
fn modulation_does_not_write_to_automated_base_parameters() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::Svf);
    let mut modulation = state::neutral_modulation();
    modulation.depths[0] = 0.8;
    plugin
        .params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Svf, modulation);
    initialize(&mut plugin, &mut context);
    let before = plugin.params.controls_for(EffectKind::Svf).p0.value();
    render(&mut plugin, &mut context, false);
    assert_eq!(
        plugin.params.controls_for(EffectKind::Svf).p0.value(),
        before
    );
}

#[test]
fn bypass_ignores_output_gain_and_still_matches_installed_latency() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::SpectralShift);
    let params = Arc::get_mut(&mut plugin.params).unwrap();
    params.bypass = BoolParam::new("Bypass", true);
    params.output = FloatParam::new("Output", 2.0, FloatRange::Linear { min: 0.0, max: 4.0 });
    params.mix = FloatParam::new("Mix", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 });
    let mut modulation = state::neutral_modulation();
    modulation.targets[0] = Some(ModulationTarget::Mix);
    modulation.depths[0] = 1.0;
    for node in &mut modulation.msegs[1].nodes {
        node.value = 1.0;
    }
    params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::SpectralShift, modulation);
    initialize(&mut plugin, &mut context);
    let latency = context.latency.get() as usize;
    let mut left = vec![0.0; latency + 2];
    let mut right = left.clone();
    left[0] = 1.0;
    right[0] = -0.5;
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, false, 120.0, &mut context)
    });
    assert_eq!(left[latency], 1.0);
    assert_eq!(right[latency], -0.5);
    assert!(left[..latency].iter().all(|v| *v == 0.0));
}

#[test]
fn modulation_bank_content_is_persisted_with_editor_closed() {
    let params = MinitosisParams::default();
    let mut modulation = state::neutral_modulation();
    modulation.depths = [0.25, -0.5, 0.75];
    params
        .modulation
        .lock()
        .unwrap()
        .set(EffectKind::Svf, modulation.clone());
    let saved = params.serialize_fields();
    let restored = MinitosisParams::default();
    restored.deserialize_fields(&saved);
    assert_eq!(
        restored.modulation.lock().unwrap().get(EffectKind::Svf),
        &modulation
    );
}

#[test]
fn host_reset_preserves_mseg_phase_and_play_state_during_pdc_realignment() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::SpectralShift);
    Arc::get_mut(&mut plugin.params).unwrap().retrig_on_play = BoolParam::new("Retrig", true);
    initialize(&mut plugin, &mut context);
    render(&mut plugin, &mut context, true);
    let before = plugin.processor.phase(1);
    assert!(plugin.was_playing);
    plugin.reset();
    assert_eq!(plugin.processor.phase(1), before);
    assert!(plugin.was_playing);
    render(&mut plugin, &mut context, true);
    assert!(plugin.processor.phase(1) > before);
}
