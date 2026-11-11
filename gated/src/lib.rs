//! Gated: a zero-latency rhythmic amplitude gate.

use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub mod dsp;
mod editor;
pub mod pattern;

use dsp::{BeatClock, Envelope, EnvelopeClock, GainSlew, RESOLUTIONS, RenderTime, Sequence};
use pattern::PatternBank;

pub struct Telemetry {
    pub step: AtomicU32,
    pub song_position: AtomicU32,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            step: AtomicU32::new(u32::MAX),
            song_position: AtomicU32::new(1),
        }
    }
}

#[derive(Params)]
pub struct GatedParams {
    pub state_generation: Arc<std::sync::atomic::AtomicU64>,
    #[persist = "editor-state"]
    pub editor_state: Arc<tiny_skia_widgets::EditorState>,
    #[persist = "patterns-v1"]
    pub bank: Arc<Mutex<PatternBank>>,
    #[id = "pattern"]
    pub pattern: IntParam,
    #[id = "resolution"]
    pub resolution: IntParam,
    #[id = "attack"]
    pub attack: FloatParam,
    #[id = "decay"]
    pub decay: FloatParam,
    #[id = "sustain"]
    pub sustain: FloatParam,
    #[id = "release"]
    pub release: FloatParam,
    #[id = "gate"]
    pub gate: FloatParam,
    #[id = "swing"]
    pub swing: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "density"]
    pub density: FloatParam,
    #[id = "tie_probability"]
    pub tie_probability: FloatParam,
    #[id = "bypass"]
    pub bypass: BoolParam,
    // Host edit notifications for document changes also mark the project dirty.
    #[id = "pattern_revision"]
    pub revision: IntParam,
}

fn percent(name: &str, default: f32, min: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max: 1.0 })
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

fn time(name: &str, default: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min: 0.0,
            max,
            factor: FloatRange::skew_factor(-2.0),
        },
    )
    .with_unit(" ms")
    .with_value_to_string(formatters::v2s_f32_rounded(1))
}

impl Default for GatedParams {
    fn default() -> Self {
        Self {
            state_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            editor_state: editor::default_editor_state(),
            bank: Arc::new(Mutex::new(PatternBank::default())),
            pattern: IntParam::new("Pattern", 1, IntRange::Linear { min: 1, max: 8 }),
            resolution: IntParam::new("Resolution", 12, IntRange::Linear { min: 0, max: 20 })
                .with_value_to_string(Arc::new(|index| {
                    RESOLUTIONS[index.clamp(0, 20) as usize].to_owned()
                }))
                .with_string_to_value(Arc::new(|text| {
                    RESOLUTIONS
                        .iter()
                        .position(|item| item.eq_ignore_ascii_case(text.trim()))
                        .map(|i| i as i32)
                })),
            attack: time("Attack", 2.0, 2000.0),
            decay: time("Decay", 50.0, 2000.0),
            sustain: percent("Sustain", 1.0, 0.0),
            release: time("Release", 20.0, 4000.0),
            gate: percent("Gate", 0.5, 0.01),
            swing: percent("Swing", 0.0, 0.0),
            mix: percent("Mix", 1.0, 0.0).with_smoother(SmoothingStyle::Linear(10.0)),
            density: percent("Density", 0.5, 0.0),
            tie_probability: percent("Tie Probability", 0.25, 0.0),
            bypass: BoolParam::new("Bypass", false),
            revision: IntParam::new(
                "Pattern revision",
                0,
                IntRange::Linear {
                    min: 0,
                    max: i32::MAX,
                },
            )
            .non_automatable()
            .hide(),
        }
    }
}

pub struct Gated {
    params: Arc<GatedParams>,
    telemetry: Arc<Telemetry>,
    bank: PatternBank,
    sequence: Sequence,
    envelope_clock: EnvelopeClock,
    last_resolution: usize,
    slew: GainSlew,
    sample_rate: f64,
    fallback_beat: f64,
    was_playing: bool,
}

impl Default for Gated {
    fn default() -> Self {
        Self {
            params: Arc::new(GatedParams::default()),
            telemetry: Arc::new(Telemetry::default()),
            bank: PatternBank::default(),
            sequence: Sequence::default(),
            envelope_clock: EnvelopeClock::default(),
            last_resolution: 12,
            slew: GainSlew::default(),
            sample_rate: 48_000.0,
            fallback_beat: 0.0,
            was_playing: false,
        }
    }
}

impl Plugin for Gated {
    const NAME: &'static str = "Gated";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.telemetry.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = f64::from(config.sample_rate);
        self.slew.set_sample_rate(config.sample_rate);
        true
    }

    fn reset(&mut self) {
        // The gate has no audio history; hosts may reset while transport runs.
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        if let Ok(bank) = self.params.bank.try_lock() {
            self.bank = *bank;
        }
        let pattern = self.bank.patterns[(self.params.pattern.value() - 1) as usize];
        let resolution = self.params.resolution.value() as usize;
        let sequence_changed = self.sequence.prepare(
            pattern,
            self.params.swing.value().into(),
            self.params.gate.value().into(),
        );
        let envelope = Envelope {
            attack: f64::from(self.params.attack.value()) * 0.001,
            decay: f64::from(self.params.decay.value()) * 0.001,
            sustain: self.params.sustain.value().into(),
            release: f64::from(self.params.release.value()) * 0.001,
        };
        let transport = context.transport();
        let playing = transport.playing;
        let bpm = transport
            .tempo
            .filter(|tempo| tempo.is_finite() && *tempo > 0.0)
            .unwrap_or(120.0);
        let position = transport
            .pos_beats()
            .filter(|position| position.is_finite());
        if playing && !self.was_playing && position.is_none() {
            self.fallback_beat = 0.0;
        }
        let start = position.unwrap_or(self.fallback_beat);
        let increment = bpm / (60.0 * self.sample_rate);
        let beats = BeatClock {
            start,
            increment,
            loop_range: transport.loop_range_beats(),
        };
        let reseed = sequence_changed
            || resolution != self.last_resolution
            || !self.was_playing
            || (start - self.fallback_beat).abs() > 1e-7;
        let bypass = self.params.bypass.value();
        let count = buffer.samples();
        for (index, channels) in buffer.iter_samples().enumerate() {
            let beat = beats.at(index);
            let envelope_gain = if playing {
                self.envelope_clock.value(
                    &self.sequence,
                    RenderTime {
                        beat,
                        resolution,
                        bpm,
                        sample_rate: self.sample_rate,
                    },
                    envelope,
                    reseed && index == 0,
                )
            } else {
                1.0
            };
            let wet = if bypass { 1.0 } else { envelope_gain };
            let mix = self.params.mix.smoothed.next();
            let gain = self.slew.next(1.0 + mix * (wet - 1.0));
            for sample in channels {
                *sample *= gain;
            }
        }
        if playing {
            self.fallback_beat = beats.at(count);
        }
        self.was_playing = playing;
        self.last_resolution = resolution;
        let step = if playing {
            self.sequence
                .position(beats.at(count.saturating_sub(1)), resolution)
                .0 as u32
        } else {
            u32::MAX
        };
        self.telemetry.step.store(step, Ordering::Relaxed);
        self.telemetry
            .song_position
            .store(u32::from(position.is_some()), Ordering::Relaxed);
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Gated {
    const CLAP_ID: &'static str = "com.mpd.gated";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Rhythmic gate with eight patterns, ADSR, swing, and random population");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Tremolo,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ];
}

impl Vst3Plugin for Gated {
    const VST3_CLASS_ID: [u8; 16] = *b"GatedTranceMpd01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

nih_export_clap!(Gated);
nih_export_vst3!(Gated);

/// Render the actual editor panel without an audio device or window server.
pub fn render_preview(path: &std::path::Path, width: u32, height: u32) -> Result<(), String> {
    editor::render_preview(path, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_state_roundtrips_all_slots_and_future_bits() {
        let params = GatedParams::default();
        {
            let mut bank = params.bank.lock().unwrap();
            bank.patterns[6].set_length(64);
            bank.patterns[6].set_on(63, true);
            bank.patterns[6].active[1] = 1;
        }
        let state = params.serialize_fields();
        let restored = GatedParams::default();
        restored.deserialize_fields(&state);
        assert_eq!(*params.bank.lock().unwrap(), *restored.bank.lock().unwrap());
    }
}
