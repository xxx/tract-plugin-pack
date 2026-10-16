//! A single continuously active Multosis effect with drawable modulation.

pub mod editor;
pub mod engine;
pub mod handoff;
pub mod params;
pub mod state;

use engine::{PrepareKey, Processor};
use handoff::ProcessorHandoff;
use nih_plug::prelude::*;
use params::MinitosisParams;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub struct Shared {
    pub handoff: ProcessorHandoff,
    pub sample_rate: AtomicU32,
    pub preparing: AtomicU64,
    pub active_generation: AtomicU64,
    pub reset_modulation: AtomicBool,
    pub modulation_dirty: AtomicBool,
    pub phases: [AtomicU32; 4],
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            handoff: ProcessorHandoff::default(),
            sample_rate: AtomicU32::new(48000.0_f32.to_bits()),
            preparing: AtomicU64::new(0),
            active_generation: AtomicU64::new(0),
            reset_modulation: AtomicBool::new(false),
            modulation_dirty: AtomicBool::new(true),
            phases: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

pub fn prepare_key(params: &MinitosisParams, shared: &Shared) -> PrepareKey {
    PrepareKey {
        kind: params.selected_kind(),
        generation: params.selection_generation.load(Ordering::Acquire),
        sample_rate_bits: shared.sample_rate.load(Ordering::Acquire),
    }
}

/// Runs only on nih-plug's background executor.
pub fn prepare_processor(params: &MinitosisParams, shared: &Shared, key: PrepareKey) {
    if prepare_key(params, shared) != key
        || shared.active_generation.load(Ordering::Acquire) == key.generation
        || shared.handoff.is_ready(key)
    {
        return;
    }
    shared.preparing.store(key.generation, Ordering::Release);
    let track = params
        .controls_for(key.kind)
        .track(params.mix.value(), false);
    let modulation = params
        .modulation
        .lock()
        .map(|bank| bank.get(key.kind).clone())
        .unwrap_or_else(|_| state::neutral_modulation());
    let processor = Box::new(Processor::new(key, &track, modulation));
    if prepare_key(params, shared) == key
        && shared.active_generation.load(Ordering::Acquire) != key.generation
    {
        shared.handoff.publish(processor);
    }
    let _ =
        shared
            .preparing
            .compare_exchange(key.generation, 0, Ordering::AcqRel, Ordering::Relaxed);
}

pub struct Minitosis {
    pub params: Arc<MinitosisParams>,
    pub shared: Arc<Shared>,
    processor: Box<Processor>,
    requested: Option<PrepareKey>,
    retry_samples: usize,
    was_playing: bool,
    reported_latency: u32,
}

impl Default for Minitosis {
    fn default() -> Self {
        let params = Arc::new(MinitosisParams::default());
        let shared = Arc::new(Shared::default());
        let key = prepare_key(&params, &shared);
        let processor = Box::new(Processor::new(
            key,
            &params.selected_track(),
            state::neutral_modulation(),
        ));
        Self {
            params,
            shared,
            processor,
            requested: None,
            retry_samples: 0,
            was_playing: false,
            reported_latency: 0,
        }
    }
}

impl Minitosis {
    fn render(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playing: bool,
        bpm: f64,
        context: &impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let samples = left.len().min(right.len());
        let key = prepare_key(&self.params, &self.shared);
        if self.processor.key != key {
            if self.shared.handoff.try_install(key, &mut self.processor) {
                self.shared
                    .active_generation
                    .store(key.generation, Ordering::Release);
                self.shared.modulation_dirty.store(true, Ordering::Release);
                self.requested = None;
            } else {
                self.retry_samples = self.retry_samples.saturating_add(samples);
                let retry_after = f32::from_bits(key.sample_rate_bits) as usize / 4;
                if self.requested != Some(key)
                    || (self.retry_samples >= retry_after
                        && self.shared.preparing.load(Ordering::Acquire) != key.generation)
                {
                    context.execute_background(key);
                    self.requested = Some(key);
                    self.retry_samples = 0;
                }
            }
        }
        let kind = self.processor.key.kind;
        if self.shared.modulation_dirty.swap(false, Ordering::AcqRel) {
            if let Ok(bank) = self.params.modulation.try_lock() {
                self.processor.modulation = bank.get(kind).clone();
            } else {
                self.shared.modulation_dirty.store(true, Ordering::Release);
            }
        }
        if self.shared.reset_modulation.swap(false, Ordering::AcqRel)
            || (playing && !self.was_playing && self.params.retrig_on_play.value())
        {
            self.processor.reset_modulation();
        }
        self.was_playing = playing;
        let track = self
            .params
            .controls_for(kind)
            .track(self.params.mix.value(), false);
        self.processor.begin_block(&track, left, right, bpm);
        let latency = self.processor.latency() as u32;
        if latency != self.reported_latency {
            context.set_latency_samples(latency);
            self.reported_latency = latency;
        }
        let pending = self.processor.key != key;
        let bypass = self.params.bypass.value();
        for i in 0..samples {
            let wet = self.processor.process_sample(
                [left[i], right[i]],
                self.params.mix.value(),
                bypass || pending,
            );
            let gain = self.params.output.smoothed.next();
            let gain = if bypass { 1.0 } else { gain };
            left[i] = wet[0] * gain;
            right[i] = wet[1] * gain;
        }
        for (k, phase) in self.shared.phases.iter().enumerate() {
            phase.store(self.processor.phase(k).to_bits(), Ordering::Relaxed);
        }
        ProcessStatus::KeepAlive
    }
}

impl Plugin for Minitosis {
    const NAME: &'static str = "Minitosis";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: std::num::NonZeroU32::new(2),
        main_output_channels: std::num::NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type SysExMessage = ();
    type BackgroundTask = PrepareKey;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let params = self.params.clone();
        let shared = self.shared.clone();
        Box::new(move |key| prepare_processor(&params, &shared, key))
    }

    fn editor(&mut self, executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone(), executor)
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.shared
            .sample_rate
            .store(config.sample_rate.to_bits(), Ordering::Release);
        self.params
            .selection_generation
            .fetch_add(1, Ordering::AcqRel);
        let key = prepare_key(&self.params, &self.shared);
        let modulation = self
            .params
            .modulation
            .lock()
            .map(|bank| bank.get(key.kind).clone())
            .unwrap_or_else(|_| state::neutral_modulation());
        *self.processor = Processor::new(
            key,
            &self
                .params
                .controls_for(key.kind)
                .track(self.params.mix.value(), false),
            modulation,
        );
        self.shared
            .active_generation
            .store(key.generation, Ordering::Release);
        self.shared.modulation_dirty.store(false, Ordering::Release);
        self.requested = None;
        self.reported_latency = self.processor.latency() as u32;
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.processor.flush_dsp();
        self.shared.modulation_dirty.store(true, Ordering::Release);
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        let playing = transport.playing;
        let bpm = transport
            .tempo
            .filter(|b| b.is_finite() && *b > 0.0)
            .unwrap_or(120.0);
        let channels = buffer.as_slice();
        if channels.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (left, right) = channels.split_at_mut(1);
        self.render(left[0], right[0], playing, bpm, context)
    }
}

impl ClapPlugin for Minitosis {
    const CLAP_ID: &'static str = "com.mpd.minitosis";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("One effect with drawable modulation");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::MultiEffects,
    ];
}

impl Vst3Plugin for Minitosis {
    const VST3_CLASS_ID: [u8; 16] = *b"MinitosisMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

nih_export_clap!(Minitosis);
nih_export_vst3!(Minitosis);

#[cfg(test)]
mod tests;
