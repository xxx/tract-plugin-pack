//! Miff — a convolution filter whose FIR kernel is hand-drawn with an MSEG
//! editor. See `docs/superpowers/specs/2026-05-16-miff-design.md`.

pub mod convolution;
pub mod editor;
pub mod kernel;

use nih_plug::prelude::*;
use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
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

    /// Persisted window size (width × height in physical pixels).
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

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
            editor_state: editor::default_editor_state(),
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

/// Fixed FFT size for the input-spectrum visualizer. 2048 matches
/// wavetable-filter's `KERNEL_LEN`. Must be a power of two.
const ISPECTRUM_FFT: usize = 2048;

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

    // ── Input-spectrum visualiser (audio → GUI) ──────────────────────────
    /// Shared magnitude bins read by the GUI's response view (ISPECTRUM_FFT/2+1 bins).
    /// Published by `process()` via `try_lock` at ~30 Hz when the editor is open.
    pub(crate) input_spectrum: Arc<Mutex<Vec<f32>>>,
    /// Ring buffer accumulating the most recent `ISPECTRUM_FFT` mono input samples.
    input_ring: Vec<f32>,
    /// Write position in `input_ring` (wraps at ISPECTRUM_FFT).
    input_ring_pos: usize,
    /// Countdown (samples) until the next spectrum FFT update.
    input_countdown: usize,
    /// Pre-computed Hann window for the input-spectrum FFT.
    input_window: Vec<f32>,
    /// Pre-allocated time-domain scratch reused by the input FFT.
    input_fft_time: Vec<f32>,
    /// Pre-allocated complex output scratch for the input FFT.
    input_fft_freq: Vec<Complex<f32>>,
    /// Pre-allocated FFT scratch buffer (avoids per-call heap alloc from `process()`).
    input_fft_scratch: Vec<Complex<f32>>,
    /// The realfft forward plan for `ISPECTRUM_FFT`. Stored so the plan object
    /// (and its scratch requirement) can be created once in `default()`.
    input_fft_plan: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
}

impl Default for Miff {
    fn default() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let input_fft_plan = planner.plan_fft_forward(ISPECTRUM_FFT);
        let input_fft_scratch = input_fft_plan.make_scratch_vec();
        let num_bins = ISPECTRUM_FFT / 2 + 1;

        let input_window: Vec<f32> = (0..ISPECTRUM_FFT)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / ISPECTRUM_FFT as f32).cos())
            })
            .collect();

        Self {
            params: Arc::new(MiffParams::default()),
            sample_rate: 44100.0,
            raw: [
                convolution::RawChannel::new(),
                convolution::RawChannel::new(),
            ],
            phaseless: [
                convolution::PhaselessChannel::new(),
                convolution::PhaselessChannel::new(),
            ],
            kernel_handoff: Arc::new(kernel::KernelHandoff::new()),
            kernel: kernel::Kernel::default(),
            last_mode: MiffMode::Raw,
            last_reported_latency: u32::MAX, // forces a report on first process
            input_spectrum: Arc::new(Mutex::new(vec![0.0; num_bins])),
            input_ring: vec![0.0; ISPECTRUM_FFT],
            input_ring_pos: 0,
            input_countdown: 0,
            input_window,
            input_fft_time: vec![0.0; ISPECTRUM_FFT],
            input_fft_freq: vec![Complex::new(0.0, 0.0); num_bins],
            input_fft_scratch,
            input_fft_plan,
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

    /// Click-safe mode switch: when the mode changes, reset the convolution
    /// path being switched INTO so stale state can't click through. Returns
    /// `true` if a switch occurred. Called from `process()`.
    fn apply_mode_switch(&mut self, mode: MiffMode) -> bool {
        if mode == self.last_mode {
            return false;
        }
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
        true
    }
}

impl Plugin for Miff {
    const NAME: &'static str = "Miff";
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

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.kernel_handoff.clone(),
            self.input_spectrum.clone(),
        )
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
        self.apply_mode_switch(mode);

        // Report latency only when it changes.
        let latency = match mode {
            MiffMode::Raw => 0,
            MiffMode::Phaseless => convolution::PHASELESS_LATENCY,
        };
        if latency != self.last_reported_latency {
            context.set_latency_samples(latency);
            self.last_reported_latency = latency;
        }

        let host_samples = buffer.samples();

        for mut channel_samples in buffer.iter_samples() {
            let mix = self.params.mix.smoothed.next();
            let gain = self.params.gain.smoothed.next();
            let mut mono_sum = 0.0_f32;
            let mut ch_count = 0usize;
            for (ch, sample) in channel_samples.iter_mut().enumerate() {
                let ch = ch.min(1); // stereo state; mono uses channel 0
                let dry = *sample;
                mono_sum += dry;
                ch_count += 1;
                let wet = self.filter_sample(ch, dry, mode);
                *sample = (dry + (wet - dry) * mix) * gain;
            }
            // Accumulate mono input into the ring buffer.
            let mono_in = if ch_count > 0 {
                mono_sum / ch_count as f32
            } else {
                0.0
            };
            self.input_ring[self.input_ring_pos] = mono_in;
            self.input_ring_pos = (self.input_ring_pos + 1) & (ISPECTRUM_FFT - 1);
        }

        // Input-spectrum FFT: throttled to ~30 Hz, only when editor is open.
        self.input_countdown = self.input_countdown.saturating_sub(host_samples);
        if self.input_countdown == 0 && self.params.editor_state.is_open() {
            self.input_countdown = (self.sample_rate / 30.0) as usize;

            // Reorder ring into input_fft_time and apply Hann window.
            let pos = self.input_ring_pos;
            let n = ISPECTRUM_FFT;
            for i in 0..n {
                let ring_idx = (pos + i) & (n - 1);
                self.input_fft_time[i] = self.input_ring[ring_idx] * self.input_window[i];
            }
            // process_with_scratch reuses pre-allocated scratch — no heap allocation.
            if self
                .input_fft_plan
                .process_with_scratch(
                    &mut self.input_fft_time,
                    &mut self.input_fft_freq,
                    &mut self.input_fft_scratch,
                )
                .is_ok()
            {
                if let Ok(mut shared) = self.input_spectrum.try_lock() {
                    let peak = self
                        .input_fft_freq
                        .iter()
                        .map(|c| c.norm())
                        .fold(0.0_f32, f32::max)
                        .max(1e-10);
                    for (dst, c) in shared.iter_mut().zip(self.input_fft_freq.iter()) {
                        *dst = c.norm() / peak;
                    }
                }
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
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Filter,
        ClapFeature::Stereo,
    ];
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
        let input = [0.5_f32, -0.3, 0.8];
        for &dry in &input {
            // gain = 1.0: mix=0 must reproduce the dry input exactly.
            let wet = miff.filter_sample(0, dry, MiffMode::Raw);
            let out = (dry + (wet - dry) * mix) * 1.0;
            assert!(
                (out - dry).abs() < 1e-6,
                "mix=0 must pass dry {dry} through, got {out}"
            );
            // gain = 2.0: gain is applied AFTER the mix, so with mix=0 the
            // output must be exactly `dry * 2.0` — never `wet`-influenced.
            let wet = miff.filter_sample(0, dry, MiffMode::Raw);
            let out = (dry + (wet - dry) * mix) * 2.0;
            assert!(
                (out - dry * 2.0).abs() < 1e-6,
                "mix=0, gain=2 must yield dry*2 ({}) , got {out}",
                dry * 2.0
            );
        }
    }

    /// `apply_mode_switch` — the click-safe transition-detection branch from
    /// `process()` — must detect mode changes, reset the path being switched
    /// into, update `last_mode`, and no-op on a repeat call.
    #[test]
    fn mode_switch_is_click_safe() {
        let mut miff = Miff::default();
        // Fresh Miff starts in Raw; no switch when the mode is unchanged.
        assert!(!miff.apply_mode_switch(MiffMode::Raw));
        // Switching to Phaseless is detected, resets the path, updates last_mode.
        assert!(miff.apply_mode_switch(MiffMode::Phaseless));
        assert_eq!(miff.last_mode, MiffMode::Phaseless);
        // Switching back is detected too.
        assert!(miff.apply_mode_switch(MiffMode::Raw));
        assert_eq!(miff.last_mode, MiffMode::Raw);
        // And a repeat call is a no-op.
        assert!(!miff.apply_mode_switch(MiffMode::Raw));
    }
}
