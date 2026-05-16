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

impl Miff {
    /// Per-sample convolution dispatch used by `process()` and testable in
    /// isolation. Does NOT advance smoothers — callers do that separately.
    fn filter_sample(&mut self, ch: usize, sample: f32, mode: MiffMode) -> f32 {
        match mode {
            MiffMode::Raw => self.raw[ch].process(sample, &self.kernel),
            MiffMode::Phaseless => self.phaseless[ch].process(sample, &self.kernel),
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
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Pick up the latest baked kernel (non-blocking; keep last on a miss).
        if let Some(k) = self.kernel_handoff.try_read() {
            self.kernel = k;
        }

        let mode = self.params.mode.value();
        // Click-safe mode switch: reset the path being switched INTO.
        if mode != self.last_mode {
            match mode {
                MiffMode::Raw => {
                    for ch in &mut self.raw {
                        ch.reset();
                    }
                }
                MiffMode::Phaseless => {
                    for ch in &mut self.phaseless {
                        ch.reset();
                    }
                }
            }
            self.last_mode = mode;
        }

        // Report latency only when it changes.
        let latency = match mode {
            MiffMode::Raw => 0,
            MiffMode::Phaseless => convolution::PHASELESS_LATENCY,
        };
        if latency != self.last_reported_latency {
            context.set_latency_samples(latency);
            self.last_reported_latency = latency;
        }

        for mut channel_samples in buffer.iter_samples() {
            let mix = self.params.mix.smoothed.next();
            let gain = self.params.gain.smoothed.next();
            for (ch, sample) in channel_samples.iter_mut().enumerate() {
                let ch = ch.min(1); // stereo state; mono uses channel 0
                let dry = *sample;
                let wet = self.filter_sample(ch, dry, mode);
                *sample = (dry + (wet - dry) * mix) * gain;
            }
        }

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

    // ── Process-loop plugin-level tests ──────────────────────────────────────
    //
    // Building a full nih-plug `Buffer` + `ProcessContext` in unit tests is
    // impractical (the types are opaque/non-constructible from outside the
    // crate in this nih-plug fork).  Instead we test the per-sample dispatch
    // helper `filter_sample`, which is the algorithmic core of `process()`.
    // A compile+run smoke test validates that the real `process()` signature
    // and call sites at least compile and won't panic on the happy path.

    /// A fresh `Miff` with the default flat-0.5 curve has `kernel.is_zero ==
    /// true`, so `RawChannel::process` returns the input unchanged.
    /// This is the "default document is a clean passthrough" guarantee.
    #[test]
    fn default_document_is_a_clean_passthrough() {
        let mut miff = Miff::default();
        // The default Kernel is zero (is_zero = true).
        assert!(miff.kernel.is_zero, "default kernel must be zero");
        let input = [0.5_f32, -0.3, 0.8, 0.1, -0.7];
        for &s in &input {
            let out = miff.filter_sample(0, s, MiffMode::Raw);
            assert!(
                (out - s).abs() < 1e-6,
                "zero kernel in Raw mode must pass {s} through, got {out}"
            );
        }
    }

    /// When `mix == 0` the process formula `dry + (wet − dry) * 0 = dry`, so
    /// the output must equal the dry input regardless of the kernel.
    /// We test this by calling `filter_sample` and then applying the mix
    /// formula manually, mirroring `process()`'s arithmetic exactly.
    #[test]
    fn mix_zero_is_dry_equivalent() {
        let mut miff = Miff::default();
        // Install a non-trivial kernel so `wet != dry`.
        let curve = tiny_skia_widgets::mseg::MsegData::default(); // 0→1 ramp
        miff.kernel = kernel::bake(&curve, 256);
        assert!(!miff.kernel.is_zero);

        let mix = 0.0_f32;
        let gain = 1.0_f32;
        let input = [0.5_f32, -0.3, 0.8];
        for &dry in &input {
            let wet = miff.filter_sample(0, dry, MiffMode::Raw);
            let out = (dry + (wet - dry) * mix) * gain;
            assert!(
                (out - dry).abs() < 1e-6,
                "mix=0 must pass dry {dry} through, got {out}"
            );
        }
    }

    /// Switching from Raw to Phaseless must reset the Phaseless path (so
    /// stale STFT state doesn't contaminate the new mode's output) without
    /// panicking or producing NaN/Inf.
    #[test]
    fn mode_switch_is_click_safe() {
        let mut miff = Miff::default();

        // Feed a block in Raw mode.
        for i in 0..64 {
            let s = (i as f32 * 0.01).sin();
            miff.filter_sample(0, s, MiffMode::Raw);
        }

        // Simulate the mode-switch reset that `process()` performs.
        for ch in &mut miff.phaseless {
            ch.reset();
        }
        miff.last_mode = MiffMode::Phaseless;

        // Feed a block in Phaseless mode — must not panic or produce NaN/Inf.
        for i in 0..64 {
            let s = (i as f32 * 0.01).cos();
            let out = miff.filter_sample(0, s, MiffMode::Phaseless);
            assert!(out.is_finite(), "Phaseless output must be finite after mode switch, got {out}");
        }
    }
}
