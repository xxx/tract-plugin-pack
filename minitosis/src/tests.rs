use super::*;
use std::cell::Cell;
use tract_effects::effects::EffectKind;

#[derive(Default)]
struct Context {
    request: Cell<Option<PrepareKey>>,
    requests: Cell<usize>,
    latency: Cell<u32>,
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
        None
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

fn render(plugin: &mut Minitosis, context: &Context, playing: bool) -> ([f32; 512], [f32; 512]) {
    let mut left = [0.25; 512];
    let mut right = [-0.5; 512];
    let status = assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, playing, 120.0, context)
    });
    assert!(matches!(status, ProcessStatus::KeepAlive));
    (left, right)
}

#[test]
fn fresh_plugin_is_identity_and_clocks_advance_while_stopped() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    assert_eq!(
        render(&mut plugin, &context, false),
        ([0.25; 512], [-0.5; 512])
    );
    let before = plugin.processor.phase(1);
    render(&mut plugin, &context, false);
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
        render(&mut plugin, &context, false);
        let one_block = plugin.processor.phase(1);
        render(&mut plugin, &context, true);
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
        render(&mut plugin, &context, false),
        ([0.25; 512], [-0.5; 512])
    );
    let key = context.request.take().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, key);
    render(&mut plugin, &context, false);
    assert_eq!(plugin.processor.key, key);
    assert_eq!(
        plugin.shared.active_generation.load(Ordering::Acquire),
        key.generation
    );
}

#[test]
fn rapid_a_b_a_switch_discards_b_and_installs_a_fresh_processor() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    select(&mut plugin, EffectKind::Delay);
    initialize(&mut plugin, &mut context);
    render(&mut plugin, &context, false);
    let original = plugin.processor.key;
    select(&mut plugin, EffectKind::Svf);
    render(&mut plugin, &context, false);
    let obsolete = context.request.take().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, obsolete);
    select(&mut plugin, EffectKind::Delay);
    render(&mut plugin, &context, false);
    assert_eq!(plugin.processor.key, original);
    let wanted = context.request.take().unwrap();
    prepare_processor(&plugin.params, &plugin.shared, wanted);
    render(&mut plugin, &context, false);
    assert_eq!(plugin.processor.key, wanted);
    assert_ne!(wanted.generation, original.generation);
}

#[test]
fn dropped_background_requests_are_retried_but_running_work_is_not_duplicated() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    select(&mut plugin, EffectKind::Svf);
    render(&mut plugin, &context, false);
    let key = context.request.take().unwrap();
    for _ in 0..24 {
        render(&mut plugin, &context, false);
    }
    assert_eq!(context.requests.get(), 2);
    plugin
        .shared
        .preparing
        .store(key.generation, Ordering::Release);
    for _ in 0..24 {
        render(&mut plugin, &context, false);
    }
    assert_eq!(context.requests.get(), 2);
    plugin.shared.preparing.store(0, Ordering::Release);
    render(&mut plugin, &context, false);
    assert_eq!(context.requests.get(), 3);
}

#[test]
fn initialize_restores_selected_effect_and_invalidates_queued_work() {
    let mut plugin = Minitosis::default();
    let mut context = Context::default();
    initialize(&mut plugin, &mut context);
    select(&mut plugin, EffectKind::SpectralShift);
    render(&mut plugin, &context, false);
    let stale = context.request.take().unwrap();
    initialize(&mut plugin, &mut context);
    let installed = plugin.processor.key;
    assert_eq!(installed.kind, EffectKind::SpectralShift);
    assert_ne!(installed.generation, stale.generation);
    prepare_processor(&plugin.params, &plugin.shared, stale);
    render(&mut plugin, &context, false);
    assert_eq!(plugin.processor.key, installed);
    assert!(context.latency.get() > 0);
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
    render(&mut plugin, &context, false);
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
    initialize(&mut plugin, &mut context);
    let latency = context.latency.get() as usize;
    let mut left = vec![0.0; latency + 2];
    let mut right = left.clone();
    left[0] = 1.0;
    right[0] = -0.5;
    assert_no_alloc::assert_no_alloc(|| {
        plugin.render(&mut left, &mut right, false, 120.0, &context)
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
    render(&mut plugin, &context, true);
    let before = plugin.processor.phase(1);
    assert!(plugin.was_playing);
    plugin.reset();
    assert_eq!(plugin.processor.phase(1), before);
    assert!(plugin.was_playing);
    render(&mut plugin, &context, true);
    assert!(plugin.processor.phase(1) > before);
}
