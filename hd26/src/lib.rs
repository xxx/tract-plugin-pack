use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub mod delay;
pub mod dimension;
mod editor;
pub mod hyper;
pub mod lfo;
pub mod transient;

use dimension::{DimMode, DimParams, Dimension};
use hyper::{Hyper, HyperParams};
use transient::TransientDetector;

/// Dimension modulation mode, exposed as a stepped parameter.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DimensionMode {
    #[id = "am"]
    #[name = "AM"]
    Am,
    #[id = "pitch"]
    #[name = "Pitch"]
    Pitch,
}

impl DimensionMode {
    fn to_dsp(self) -> DimMode {
        match self {
            DimensionMode::Am => DimMode::Am,
            DimensionMode::Pitch => DimMode::Pitch,
        }
    }
}

/// Lock-free audio→GUI telemetry for the Retrig activity indicator.
pub struct Telemetry {
    /// Fast-envelope level (f32 bit pattern) for the level bar.
    pub level: AtomicU32,
    /// Incremented once per fired retrig; the editor flashes the LED on change.
    pub trigger_count: AtomicU32,
}

impl Telemetry {
    fn new() -> Self {
        Self {
            level: AtomicU32::new(0),
            trigger_count: AtomicU32::new(0),
        }
    }
}

pub struct Hd26 {
    params: Arc<Hd26Params>,
    pub telemetry: Arc<Telemetry>,
    hyper: Hyper,
    dimension: Dimension,
    transient: TransientDetector,
    sample_rate: f32,
}

#[derive(Params)]
pub struct Hd26Params {
    #[persist = "editor-state"]
    pub editor_state: Arc<tiny_skia_widgets::EditorState>,

    // ── Hyper ──
    #[id = "h_unison"]
    pub hyper_unison: IntParam,
    #[id = "h_detune"]
    pub hyper_detune: FloatParam,
    #[id = "h_rate"]
    pub hyper_rate: FloatParam,
    #[id = "h_width"]
    pub hyper_width: FloatParam,
    #[id = "h_retrig"]
    pub hyper_retrig: BoolParam,
    #[id = "h_sens"]
    pub hyper_sensitivity: FloatParam,
    #[id = "h_mix"]
    pub hyper_mix: FloatParam,

    // ── Dimension ──
    #[id = "d_size"]
    pub dim_size: FloatParam,
    #[id = "d_mode"]
    pub dim_mode: EnumParam<DimensionMode>,
    #[id = "d_hpf"]
    pub dim_hpf: FloatParam,
    #[id = "d_mix"]
    pub dim_mix: FloatParam,

    // ── Global ──
    #[id = "output"]
    pub output: FloatParam,
    #[id = "bypass"]
    pub bypass: BoolParam,
}

impl Default for Hd26 {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            params: Arc::new(Hd26Params::new()),
            telemetry: Arc::new(Telemetry::new()),
            hyper: Hyper::new(sr),
            dimension: Dimension::new(sr),
            transient: TransientDetector::new(sr),
            sample_rate: sr,
        }
    }
}

impl Hd26Params {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),

            hyper_unison: IntParam::new("H Unison", 3, IntRange::Linear { min: 0, max: 7 }),
            hyper_detune: FloatParam::new(
                "H Detune",
                0.30,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage())
            .with_smoother(SmoothingStyle::Linear(20.0)),
            hyper_rate: FloatParam::new(
                "H Rate",
                1.0,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(2))
            .with_smoother(SmoothingStyle::Linear(20.0)),
            hyper_width: FloatParam::new("H Width", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage())
                .with_smoother(SmoothingStyle::Linear(20.0)),
            hyper_retrig: BoolParam::new("H Retrig", false),
            hyper_sensitivity: FloatParam::new(
                "H Sensitivity",
                0.5,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            hyper_mix: FloatParam::new("H Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage())
                .with_smoother(SmoothingStyle::Linear(20.0)),

            dim_size: FloatParam::new("D Size", 0.30, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage())
                .with_smoother(SmoothingStyle::Linear(20.0)),
            dim_mode: EnumParam::new("D Mode", DimensionMode::Am),
            dim_hpf: FloatParam::new(
                "D Wet HPF",
                120.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 500.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(0))
            .with_smoother(SmoothingStyle::Linear(20.0)),
            dim_mix: FloatParam::new("D Mix", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage())
                .with_smoother(SmoothingStyle::Linear(20.0)),

            output: FloatParam::new(
                "Output",
                0.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 12.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1))
            .with_smoother(SmoothingStyle::Linear(50.0)),
            bypass: BoolParam::new("Bypass", false),
        }
    }
}

impl Plugin for Hd26 {
    const NAME: &'static str = "HD26";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.telemetry.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.hyper.set_sample_rate(self.sample_rate);
        self.dimension.set_sample_rate(self.sample_rate);
        self.transient.set_sample_rate(self.sample_rate);
        true
    }

    fn reset(&mut self) {
        self.hyper.reset();
        self.dimension.reset();
        self.transient.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        if buffer.channels() < 2 {
            return ProcessStatus::Normal;
        }
        if self.params.bypass.value() {
            return ProcessStatus::Normal;
        }

        // Per-block (non-smoothed) reads.
        let voices = self.params.hyper_unison.value() as usize;
        let retrig_on = self.params.hyper_retrig.value();
        let dim_mode = self.params.dim_mode.value().to_dsp();
        self.transient
            .set_sensitivity(self.params.hyper_sensitivity.value());

        let num_samples = buffer.samples();
        let chans = buffer.as_slice();

        #[allow(clippy::needless_range_loop)]
        for i in 0..num_samples {
            // Smoothed per-sample reads.
            let hp = HyperParams {
                voices,
                detune: self.params.hyper_detune.smoothed.next(),
                rate_hz: self.params.hyper_rate.smoothed.next(),
                width: self.params.hyper_width.smoothed.next(),
                mix: self.params.hyper_mix.smoothed.next(),
            };
            let dp = DimParams {
                size: self.params.dim_size.smoothed.next(),
                mode: dim_mode,
                hpf_hz: self.params.dim_hpf.smoothed.next(),
                mix: self.params.dim_mix.smoothed.next(),
            };
            let out_gain = util::db_to_gain(self.params.output.smoothed.next());

            let l = chans[0][i];
            let r = chans[1][i];

            // The detector runs every sample and the LED reflects every detected
            // transient — so Sensitivity can be tuned (and the LED watched) even
            // with Retrig off. The actual voice retrigger only fires when Retrig
            // is enabled.
            if self.transient.process_sample((l + r) * 0.5) {
                self.telemetry.trigger_count.fetch_add(1, Ordering::Relaxed);
                if retrig_on {
                    self.hyper.retrig();
                }
            }

            let (hl, hr) = self.hyper.process_sample(l, r, &hp);
            let (dl, dr) = self.dimension.process_sample(hl, hr, &dp);

            chans[0][i] = dl * out_gain;
            chans[1][i] = dr * out_gain;
        }

        // Publish the current level for the GUI level bar (once per block).
        self.telemetry
            .level
            .store(self.transient.fast_env().to_bits(), Ordering::Relaxed);

        ProcessStatus::Normal
    }
}

impl ClapPlugin for Hd26 {
    const CLAP_ID: &'static str = "com.mpd.hd26";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Serum-style Hyper/Dimension chorus + widener");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Chorus,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Hd26 {
    const VST3_CLASS_ID: [u8; 16] = *b"HD26HyperDimMpd\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

nih_export_clap!(Hd26);
nih_export_vst3!(Hd26);

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> Hd26 {
        let mut p = Hd26::default();
        p.hyper.set_sample_rate(48_000.0);
        p.dimension.set_sample_rate(48_000.0);
        p.transient.set_sample_rate(48_000.0);
        p
    }

    #[test]
    fn default_params_match_spec() {
        let p = Hd26Params::new();
        assert_eq!(p.hyper_unison.value(), 3);
        assert!((p.hyper_mix.value() - 1.0).abs() < 1e-6);
        assert!(p.dim_mix.value().abs() < 1e-6);
        assert_eq!(p.dim_mode.value(), DimensionMode::Am);
    }

    #[test]
    fn dimension_mode_maps_to_dsp() {
        assert_eq!(DimensionMode::Am.to_dsp(), DimMode::Am);
        assert_eq!(DimensionMode::Pitch.to_dsp(), DimMode::Pitch);
    }

    #[test]
    fn stages_chain_without_nan() {
        // Drive a few blocks through the raw DSP chain (mirrors process()).
        let mut p = make();
        let hp = HyperParams {
            voices: 7,
            detune: 0.8,
            rate_hz: 1.0,
            width: 0.5,
            mix: 1.0,
        };
        let dp = DimParams {
            size: 0.5,
            mode: DimMode::Am,
            hpf_hz: 120.0,
            mix: 0.7,
        };
        for n in 0..4000 {
            let x = (0.09 * n as f32).sin() * 0.8;
            let (hl, hr) = p.hyper.process_sample(x, x, &hp);
            let (dl, dr) = p.dimension.process_sample(hl, hr, &dp);
            assert!(dl.is_finite() && dr.is_finite(), "non-finite at {n}");
        }
    }
}
