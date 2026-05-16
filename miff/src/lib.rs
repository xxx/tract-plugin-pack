#![feature(portable_simd)]
//! miff — a convolution filter whose FIR kernel is hand-drawn with an MSEG
//! editor. See `docs/superpowers/specs/2026-05-16-miff-design.md`.

pub mod convolution;
pub mod kernel;

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use tiny_skia_widgets::mseg::MsegData;

/// Filter mode: direct convolution or STFT magnitude-only.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
enum MiffMode {
    #[id = "raw"]
    #[name = "Raw"]
    Raw,
    #[id = "phaseless"]
    #[name = "Phaseless"]
    Phaseless,
}

#[derive(Params)]
struct MiffParams {
    /// The hand-drawn curve — miff's filter kernel source. Persisted via
    /// MsegData's compact serde; `Arc<Mutex<..>>` is the nih-plug
    /// `PersistentField` shape. The GUI edits it; the GUI thread bakes from it.
    #[persist = "curve"]
    pub curve: Arc<Mutex<MsegData>>,

    #[id = "mode"]
    pub mode: EnumParam<MiffMode>,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "length"]
    pub length: IntParam,
}

impl Default for MiffParams {
    fn default() -> Self {
        Self {
            curve: Arc::new(Mutex::new(crate::kernel::default_flat_curve())),
            mode: EnumParam::new("Mode", MiffMode::Raw),
            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-20.0),
                    max: util::db_to_gain(20.0),
                    factor: FloatRange::gain_skew_factor(-20.0, 20.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            length: IntParam::new("Length", 256, IntRange::Linear { min: 64, max: 4096 })
                .non_automatable(),
        }
    }
}

// fields used by process() in Task 8
#[allow(dead_code)]
pub struct Miff {
    params: Arc<MiffParams>,
    /// Sample rate from `initialize`.
    sample_rate: f32,
    /// Per-channel Raw convolution state (stereo).
    raw: [convolution::RawChannel; 2],
    /// Per-channel Phaseless STFT state (stereo).
    phaseless: [convolution::PhaselessChannel; 2],
    /// GUI→audio kernel handoff.
    kernel_handoff: Arc<kernel::KernelHandoff>,
    /// The audio thread's current kernel (updated from `kernel_handoff`).
    kernel: kernel::Kernel,
    /// Last mode, to detect a Raw<->Phaseless switch for click-safe reset.
    last_mode: MiffMode,
    /// Last latency reported to the host (re-report only on change).
    last_reported_latency: u32,
}

impl Default for Miff {
    fn default() -> Self {
        Self {
            params: Arc::new(MiffParams::default()),
            sample_rate: 44100.0,
            raw: [convolution::RawChannel::new(), convolution::RawChannel::new()],
            phaseless: [
                convolution::PhaselessChannel::new(),
                convolution::PhaselessChannel::new(),
            ],
            kernel_handoff: Arc::new(kernel::KernelHandoff::new()),
            kernel: kernel::Kernel::default(),
            last_mode: MiffMode::Raw,
            last_reported_latency: u32::MAX, // forces a report on first process
        }
    }
}

impl Plugin for Miff {
    const NAME: &'static str = "miff";
    const VENDOR: &'static str = "Michael Dungan";
    const URL: &'static str = "https://github.com/xxx/miff";
    const EMAIL: &'static str = "no-reply@example.com";
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

    const HARD_REALTIME_ONLY: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        Box::new(|_| {})
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        // Bake the current curve so the kernel is ready before the first edit.
        if let Ok(curve) = self.params.curve.lock() {
            let len = self.params.length.value() as usize;
            self.kernel_handoff.publish(kernel::bake(&curve, len));
        }
        true
    }

    fn reset(&mut self) {
        for ch in &mut self.raw {
            ch.reset();
        }
        for ch in &mut self.phaseless {
            ch.reset();
        }
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Miff {
    const CLAP_ID: &'static str = "com.mpd.miff";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A convolution filter whose kernel is hand-drawn with an MSEG editor");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Filter, ClapFeature::Stereo];
}

impl Vst3Plugin for Miff {
    const VST3_CLASS_ID: [u8; 16] = *b"MiffMpdConvFiltr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Filter];
}

nih_export_clap!(Miff);
nih_export_vst3!(Miff);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_curve_is_the_flat_passthrough() {
        let p = MiffParams::default();
        let curve = p.curve.lock().unwrap();
        let k = kernel::bake(&curve, 256);
        assert!(k.is_zero, "fresh miff must be a clean passthrough");
    }

    #[test]
    fn length_param_defaults_to_256() {
        let p = MiffParams::default();
        assert_eq!(p.length.value(), 256);
    }
}
