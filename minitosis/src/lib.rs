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
use tract_effects::sidechain::SidechainInput;

pub struct Shared {
    pub handoff: ProcessorHandoff,
    pub sample_rate: AtomicU32,
    pub preparing: AtomicU64,
    pub active_generation: AtomicU64,
    pub reset_modulation: AtomicBool,
    pub modulation_dirty: AtomicBool,
    pub phases: [AtomicU32; 4],
    pub telemetry: Arc<tract_effects::modulation_telemetry::ModulationTelemetry>,
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
            telemetry: Arc::new(
                tract_effects::modulation_telemetry::ModulationTelemetry::default(),
            ),
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
    let mut processor = Box::new(Processor::new(key, &track, modulation));
    processor.connect_waveform(&shared.telemetry);
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
    trace_clock: tract_effects::modulation_telemetry::TraceClock,
    host_position: tract_effects::modulation::HostPosition,
}

impl Default for Minitosis {
    fn default() -> Self {
        let params = Arc::new(MinitosisParams::default());
        let shared = Arc::new(Shared::default());
        let key = prepare_key(&params, &shared);
        let mut processor = Box::new(Processor::new(
            key,
            &params.selected_track(),
            state::neutral_modulation(),
        ));
        processor.connect_waveform(&shared.telemetry);
        Self {
            params,
            shared,
            processor,
            requested: None,
            retry_samples: 0,
            was_playing: false,
            reported_latency: 0,
            trace_clock: tract_effects::modulation_telemetry::TraceClock::default(),
            host_position: tract_effects::modulation::HostPosition::default(),
        }
    }
}

impl Minitosis {
    fn next_midi(
        context: &mut impl ProcessContext<Self>,
        midi: &mut tract_effects::modulation::MidiCursor,
        processor: &Processor,
        samples: usize,
    ) -> Option<tract_effects::modulation::MidiNoteOn> {
        let mut events = std::iter::from_fn(|| {
            loop {
                if let NoteEvent::NoteOn {
                    timing,
                    channel,
                    note,
                    velocity,
                    ..
                } = context.next_event()?
                    && let Some(event) =
                        tract_effects::modulation::MidiNoteOn::new(timing, channel, note, velocity)
                {
                    return Some(event);
                }
            }
        });
        midi.next_matching(&mut events, samples, |event| processor.accepts_midi(event))
    }

    #[cfg(test)]
    fn render(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playing: bool,
        bpm: f64,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        self.render_with_sidechain(
            left,
            right,
            SidechainInput::default(),
            playing,
            bpm,
            context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_with_sidechain(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        sidechain: SidechainInput<'_>,
        playing: bool,
        bpm: f64,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let samples = left.len().min(right.len());
        let key = prepare_key(&self.params, &self.shared);
        if self.processor.key != key {
            if self.shared.handoff.try_install(key, &mut self.processor) {
                self.shared.telemetry.invalidate();
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
        self.processor.set_host_position(self.host_position);
        if self.shared.modulation_dirty.swap(false, Ordering::AcqRel) {
            if let Ok(bank) = self.params.modulation.try_lock() {
                self.processor.modulation = bank.get(kind).clone();
            } else {
                self.shared.modulation_dirty.store(true, Ordering::Release);
            }
        }
        if self.shared.reset_modulation.swap(false, Ordering::AcqRel) {
            self.processor.reset_modulation();
        } else if playing && !self.was_playing && self.params.retrig_on_play.value() {
            self.processor.retrigger_modulation();
        }
        self.was_playing = playing;
        self.processor
            .set_wave_automation(self.params.waves_for(kind).values());
        if self.processor.uses_project_waves()
            && let Ok(palette) = self.params.wave_palette.try_lock()
        {
            self.processor.set_wave_palette(&palette);
        }
        let track = self
            .params
            .controls_for(kind)
            .track(self.params.mix.value(), false);
        let request = self.trace_clock.begin_block(
            &self.shared.telemetry,
            f32::from_bits(self.processor.key.sample_rate_bits) as f64,
        );
        self.processor
            .set_telemetry_enabled(request.matches(0, self.processor.key.kind));
        self.processor
            .prepare_block_with_sidechain(&track, left, right, sidechain, bpm);
        let pending = self.processor.key != key;
        let bypass = self.params.bypass.value();
        let mut midi = tract_effects::modulation::MidiCursor::default();
        let mut note = Self::next_midi(context, &mut midi, &self.processor, samples);
        let mut cursor = 0;
        let mut prepared = false;
        while cursor < samples {
            let mut triggered = false;
            while let Some(event) = note.filter(|event| event.timing == cursor) {
                triggered |= self.processor.midi_note_on(event, prepared);
                note = Self::next_midi(context, &mut midi, &self.processor, samples);
            }
            let end = note.map_or(samples, |event| event.timing);
            if triggered || !prepared {
                self.processor
                    .advance_segment(&track, samples - cursor, bpm);
                prepared = true;
                let latency = self.processor.latency() as u32;
                if latency != self.reported_latency {
                    context.set_latency_samples(latency);
                    self.reported_latency = latency;
                }
            }
            for i in cursor..end {
                let wet = self.processor.process_sample_with_sidechain(
                    [left[i], right[i]],
                    sidechain.sample(i),
                    self.params.mix.value(),
                    bypass || pending,
                );
                let latency = self.processor.latency() as u32;
                if latency != self.reported_latency {
                    context.set_latency_samples(latency);
                    self.reported_latency = latency;
                }
                if self.trace_clock.tick() {
                    self.trace_clock.record(
                        &self.shared.telemetry,
                        0,
                        &self.processor.modulation_frame(),
                    );
                }
                let gain = self.params.output.smoothed.next();
                let gain = if bypass { 1.0 } else { gain };
                left[i] = wet[0] * gain;
                right[i] = wet[1] * gain;
            }
            cursor = end;
        }
        let latency = self.processor.latency() as u32;
        if latency != self.reported_latency {
            context.set_latency_samples(latency);
            self.reported_latency = latency;
        }
        self.processor.finish_waveform_block();
        self.trace_clock.publish(
            &self.shared.telemetry,
            0,
            &self.processor.modulation_frame(),
        );
        for (k, phase) in self.shared.phases.iter().enumerate() {
            phase.store(self.processor.phase(k).to_bits(), Ordering::Relaxed);
        }
        ProcessStatus::KeepAlive
    }
}

impl Plugin for Minitosis {
    fn filter_state(state: &mut PluginState) {
        tiny_skia_widgets::mseg::prepare_wave_palette_state(&mut state.fields);
        tiny_skia_widgets::mseg::WaveControls::prepare_state(state, state::bank_kinds().len());
    }

    const NAME: &'static str = "Minitosis";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: std::num::NonZeroU32::new(2),
            main_output_channels: std::num::NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames {
                aux_inputs: &["Sidechain"],
                ..PortNames::const_default()
            },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: std::num::NonZeroU32::new(2),
            main_output_channels: std::num::NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];
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
        let mut processor = Processor::new(
            key,
            &self
                .params
                .controls_for(key.kind)
                .track(self.params.mix.value(), false),
            modulation,
        );
        processor.inherit_precise_timing(&self.processor);
        *self.processor = processor;
        self.processor.connect_waveform(&self.shared.telemetry);
        self.shared.telemetry.invalidate();
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
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        let playing = transport.playing;
        let bpm = transport
            .tempo
            .filter(|b| b.is_finite() && *b > 0.0)
            .unwrap_or(120.0);
        self.host_position = tract_effects::modulation::HostPosition {
            playing,
            beats: transport.pos_beats(),
            seconds: transport.pos_seconds(),
            bpm: transport
                .tempo
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(0.0),
            loop_beats: transport.loop_range_beats(),
            loop_seconds: transport.loop_range_seconds(),
        };
        let channels = buffer.as_slice();
        if channels.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (left, right) = channels.split_at_mut(1);
        let sidechain = aux
            .inputs
            .first()
            .map(|input| SidechainInput::from_channels(input.as_slice_immutable()))
            .unwrap_or_default();
        self.render_with_sidechain(left[0], right[0], sidechain, playing, bpm, context)
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
