#![feature(portable_simd)]

use nih_plug::prelude::*;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner as ComplexFftPlanner};
use std::simd::f32x16;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

mod editor;
pub mod wavetable;

use wavetable::Wavetable;

/// Fixed output kernel length for convolution. Must be a multiple of 16 (for SIMD).
/// 2048 gives 1024 frequency bins — enough resolution for any typical wavetable frame.
const KERNEL_LEN: usize = 2048;

pub struct WavetableFilter {
    params: Arc<WavetableFilterParams>,
    wavetable: Option<Wavetable>,
    sample_rate: f32,
    // Circular buffer for convolution (per channel)
    filter_state: [FilterState; 2],
    should_reload: Arc<AtomicBool>,
    // Shared wavetable for UI display
    shared_wavetable: Arc<Mutex<Wavetable>>,
    // Version counter to trigger UI updates
    wavetable_version: Arc<std::sync::atomic::AtomicU32>,
    // Current frame count for parameter display
    current_frame_count: Arc<std::sync::atomic::AtomicUsize>,
    // Silence detection counter
    silence_samples: usize,
    // Final kernel used for convolution (always KERNEL_LEN samples)
    synthesized_kernel: Vec<f32>,
    last_frame_pos: f32,
    last_cutoff: f32,
    last_resonance: f32,
    // True until the first process() call; used to force an initial synthesis dispatch.
    first_process: bool,
    // Final kernel produced by the background synthesis thread; audio thread crossfades to it.
    pending_kernel: Arc<Mutex<Option<Vec<f32>>>>,
    // Params waiting to be dispatched once the current in-flight task finishes.
    pending_dispatch: Option<(f32, f32, f32)>, // (frame_pos, cutoff_hz, resonance)
    synthesis_in_flight: Arc<AtomicBool>,
    // Per-sample output crossfade: blend convolution outputs during ~20 ms transition.
    // Pre-allocated to avoid any heap allocation on the audio thread.
    crossfade_target_kernel: Vec<f32>, // the "to" kernel (KERNEL_LEN)
    crossfade_active: bool,
    crossfade_alpha: f32,
    // Forward real FFT for analyzing the wavetable frame (size = frame_size, changes with wavetable)
    frame_fft: Arc<dyn RealToComplex<f32>>,
    // Inverse real FFT for kernel synthesis output (size = KERNEL_LEN, constant)
    kernel_ifft: Arc<dyn ComplexToReal<f32>>,
    // Complex FFTs for the minimum-phase cepstral algorithm (size = KERNEL_LEN, constant)
    cplx_fft: Arc<dyn Fft<f32>>,
    cplx_ifft: Arc<dyn Fft<f32>>,
}

struct FilterState {
    // Circular buffer for input history (power-of-2 size for fast bit-masking)
    history: Vec<f32>,
    write_pos: usize,
    mask: usize,
}

#[derive(Params)]
struct WavetableFilterParams {
    /// Persisted wavetable file path — restored by the DAW on session reload.
    #[persist = "wavetable_path"]
    pub wavetable_path: Arc<Mutex<String>>,

    #[id = "frequency"]
    pub frequency: FloatParam,

    #[id = "frame_position"]
    pub frame_position: FloatParam,

    #[id = "resonance"]
    pub resonance: FloatParam,

    #[id = "mix"]
    pub mix: FloatParam,

    #[id = "drive"]
    pub drive: FloatParam,

    #[id = "mode"]
    pub mode: EnumParam<FilterMode>,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
enum FilterMode {
    #[id = "minimum"]
    #[name = "Minimum"]
    Minimum,

    #[id = "raw"]
    #[name = "Raw"]
    Raw,
}

/// Sent to the background thread to synthesize the complete FIR kernel.
/// All FFT work (frame analysis, resonance comb, IFFT, min-phase) runs off the audio thread.
pub struct SynthesisTask {
    frame: Vec<f32>,
    cutoff_hz: f32,
    resonance: f32,
    sample_rate: f32,
    filter_mode: FilterMode,
    pending_kernel: Arc<Mutex<Option<Vec<f32>>>>,
    synthesis_in_flight: Arc<AtomicBool>,
    // Pre-built FFT plans shared from the plugin struct — no planner construction per task.
    frame_fft: Arc<dyn RealToComplex<f32>>,
    kernel_ifft: Arc<dyn ComplexToReal<f32>>,
    cplx_fft: Arc<dyn Fft<f32>>,
    cplx_ifft: Arc<dyn Fft<f32>>,
}

impl Default for WavetableFilter {
    fn default() -> Self {
        let default_wt = Self::create_default_wavetable();
        let frame_count = default_wt.frame_count;
        let frame_size = default_wt.frame_size;

        let current_frame_count = Arc::new(std::sync::atomic::AtomicUsize::new(frame_count));

        let mut real_planner = RealFftPlanner::<f32>::new();
        let frame_fft = real_planner.plan_fft_forward(frame_size);
        let kernel_ifft = real_planner.plan_fft_inverse(KERNEL_LEN);

        let mut cplx_planner = ComplexFftPlanner::<f32>::new();
        let cplx_fft = cplx_planner.plan_fft_forward(KERNEL_LEN);
        let cplx_ifft = cplx_planner.plan_fft_inverse(KERNEL_LEN);

        Self {
            params: Arc::new(WavetableFilterParams::new(current_frame_count.clone())),
            wavetable: Some(default_wt.clone()),
            sample_rate: 48000.0,
            filter_state: [FilterState::new(KERNEL_LEN), FilterState::new(KERNEL_LEN)],
            should_reload: Arc::new(AtomicBool::new(false)),
            shared_wavetable: Arc::new(Mutex::new(default_wt)),
            wavetable_version: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            current_frame_count,
            silence_samples: 0,
            synthesized_kernel: vec![0.0; KERNEL_LEN],
            last_frame_pos: 0.0,
            last_cutoff: 0.0,
            last_resonance: 0.0,
            first_process: true,
            pending_kernel: Arc::new(Mutex::new(None)),
            pending_dispatch: None,
            synthesis_in_flight: Arc::new(AtomicBool::new(false)),
            crossfade_target_kernel: vec![0.0f32; KERNEL_LEN],
            crossfade_active: false,
            crossfade_alpha: 0.0,
            frame_fft,
            kernel_ifft,
            cplx_fft,
            cplx_ifft,
        }
    }
}

/// Horizontal sum of an f32x16 SIMD vector using pairwise tree reduction.
#[inline(always)]
fn hsum(v: f32x16) -> f32 {
    let a = v.to_array();
    let s0 = a[0] + a[1];
    let s1 = a[2] + a[3];
    let s2 = a[4] + a[5];
    let s3 = a[6] + a[7];
    let s4 = a[8] + a[9];
    let s5 = a[10] + a[11];
    let s6 = a[12] + a[13];
    let s7 = a[14] + a[15];
    (s0 + s1) + (s2 + s3) + (s4 + s5) + (s6 + s7)
}

impl WavetableFilter {
    /// Compute the base magnitude spectrum for a wavetable frame at a given cutoff.
    ///
    /// Returns (magnitudes, fractional harmonic positions) for each of the KERNEL_LEN/2+1
    /// output bins. Resonance is NOT applied here — call `apply_resonance_and_ifft` inline.
    fn compute_base_spectrum(
        frame: &[f32],
        cutoff_hz: f32,
        sample_rate: f32,
        frame_fft: &Arc<dyn RealToComplex<f32>>,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        let mut frame_buf = frame.to_vec();
        let mut frame_spectrum = vec![Complex::new(0.0_f32, 0.0); frame.len() / 2 + 1];
        if frame_fft.process(&mut frame_buf, &mut frame_spectrum).is_err() {
            return None;
        }

        let mut frame_mags: Vec<f32> = frame_spectrum.iter().map(|c| c.norm()).collect();
        let peak = frame_mags.iter().cloned().fold(0.0f32, f32::max).max(1e-10);
        for m in &mut frame_mags {
            *m /= peak;
        }

        let num_bins = KERNEL_LEN / 2 + 1;
        let bin_to_src = 24.0 * sample_rate / (KERNEL_LEN as f32 * cutoff_hz);
        let max_src = (frame_mags.len() - 1) as f32;

        let mut mags = vec![0.0f32; num_bins];
        let mut fracs = vec![0.0f32; num_bins];

        for j in 0..num_bins {
            let src = j as f32 * bin_to_src;
            if src < max_src {
                let lo = src.floor() as usize;
                let frac = src - lo as f32;
                mags[j] = frame_mags[lo] * (1.0 - frac) + frame_mags[lo + 1] * frac;
                fracs[j] = frac;
            }
            // src >= max_src: mags[j] stays 0.0
        }

        Some((mags, fracs))
    }

    /// Apply the resonance comb window to the base spectrum and IFFT into `kernel_out`.
    ///
    /// All buffers are caller-provided (pre-allocated) — zero heap allocation on the audio thread.
    fn apply_resonance_and_ifft(
        base_mags: &[f32],
        bin_fracs: &[f32],
        resonance: f32,
        spectrum_work: &mut [Complex<f32>],
        kernel_out: &mut [f32],
        kernel_ifft: &Arc<dyn ComplexToReal<f32>>,
        mode: FilterMode,
        cplx_work: &mut [Complex<f32>],
        cplx_fft: &Arc<dyn Fft<f32>>,
        cplx_ifft: &Arc<dyn Fft<f32>>,
    ) {
        let comb_exp = resonance * 8.0;
        for j in 0..base_mags.len() {
            let mag = if comb_exp > 0.01 {
                let dist = bin_fracs[j].min(1.0 - bin_fracs[j]);
                let comb = (std::f32::consts::PI * dist).cos().powf(comb_exp);
                base_mags[j] * comb
            } else {
                base_mags[j]
            };
            spectrum_work[j] = Complex::new(mag, 0.0);
        }

        kernel_out.fill(0.0);
        if kernel_ifft.process(spectrum_work, kernel_out).is_err() {
            kernel_out.fill(0.0);
            kernel_out[0] = 1.0;
            return;
        }

        let scale = 1.0 / KERNEL_LEN as f32;
        for s in kernel_out.iter_mut() {
            *s *= scale;
        }

        if mode == FilterMode::Minimum {
            Self::compute_minimum_phase_kernel_inplace(kernel_out, cplx_work, cplx_fft, cplx_ifft);
        }
    }

    /// Convert a kernel to minimum phase in-place using the cepstral method.
    ///
    /// `cplx_work` is a pre-allocated scratch buffer of length KERNEL_LEN — no allocation.
    fn compute_minimum_phase_kernel_inplace(
        kernel: &mut [f32],
        cplx_work: &mut [Complex<f32>],
        cplx_fft: &Arc<dyn Fft<f32>>,
        cplx_ifft: &Arc<dyn Fft<f32>>,
    ) {
        let n = kernel.len();
        let scale = 1.0 / n as f32;
        let epsilon = 1e-10_f32;

        for (c, &k) in cplx_work.iter_mut().zip(kernel.iter()) {
            *c = Complex::new(k, 0.0);
        }
        cplx_fft.process(cplx_work);

        for b in cplx_work.iter_mut() {
            let mag = b.norm().max(epsilon);
            *b = Complex::new(mag.ln(), 0.0);
        }

        cplx_ifft.process(cplx_work);
        for b in cplx_work.iter_mut() {
            *b *= scale;
        }

        for i in 1..n / 2 {
            cplx_work[i] *= 2.0;
        }
        for i in (n / 2 + 1)..n {
            cplx_work[i] = Complex::new(0.0, 0.0);
        }

        cplx_fft.process(cplx_work);

        for b in cplx_work.iter_mut() {
            let mag = b.re.exp();
            let phase = b.im;
            *b = Complex::new(mag * phase.cos(), mag * phase.sin());
        }

        cplx_ifft.process(cplx_work);

        for (k, c) in kernel.iter_mut().zip(cplx_work.iter()) {
            *k = c.re * scale;
        }
    }

    /// Create a default lowpass filter wavetable
    pub fn create_default_wavetable() -> Wavetable {
        const FRAME_SIZE: usize = 256;
        use std::f32::consts::PI;

        let mut samples = Vec::with_capacity(FRAME_SIZE * 4);

        // Frame 0: Sine
        for i in 0..FRAME_SIZE {
            let phase = i as f32 / FRAME_SIZE as f32;
            samples.push((phase * 2.0 * PI).sin());
        }

        // Frame 1: Triangle
        for i in 0..FRAME_SIZE {
            let phase = i as f32 / FRAME_SIZE as f32;
            samples.push(if phase < 0.25 {
                phase * 4.0
            } else if phase < 0.75 {
                2.0 - phase * 4.0
            } else {
                phase * 4.0 - 4.0
            });
        }

        // Frame 2: Square (band-limited via additive synthesis to avoid aliasing)
        for i in 0..FRAME_SIZE {
            let phase = i as f32 / FRAME_SIZE as f32;
            let mut s = 0.0f32;
            let mut k = 1;
            while k < FRAME_SIZE / 2 {
                s += (phase * 2.0 * PI * k as f32).sin() / k as f32;
                k += 2; // odd harmonics only
            }
            samples.push(s * (4.0 / PI)); // normalize to ±1
        }

        // Frame 3: Sawtooth (band-limited via additive synthesis)
        for i in 0..FRAME_SIZE {
            let phase = i as f32 / FRAME_SIZE as f32;
            let mut s = 0.0f32;
            for k in 1..FRAME_SIZE / 2 {
                s += (phase * 2.0 * PI * k as f32).sin() / k as f32;
            }
            samples.push(s * (2.0 / PI)); // normalize to ±1
        }

        Wavetable::new(samples, FRAME_SIZE).expect("Failed to create default wavetable")
    }

    pub fn load_wavetable_from_file(&mut self, path: &str) -> Result<(), String> {
        let wavetable = Wavetable::from_file(path)?;

        nih_log!(
            "Loaded wavetable: {} frames, {} samples per frame",
            wavetable.frame_count,
            wavetable.frame_size
        );

        let new_size = wavetable.frame_size;

        // Resize history buffer only if necessary (kernel is always KERNEL_LEN)
        for state in &mut self.filter_state {
            if state.history.len() != KERNEL_LEN {
                *state = FilterState::new(KERNEL_LEN);
            }
        }

        self.current_frame_count
            .store(wavetable.frame_count, Ordering::Relaxed);

        self.wavetable = Some(wavetable.clone());

        if let Ok(mut shared_wt) = self.shared_wavetable.lock() {
            *shared_wt = wavetable;
        }

        let new_version = self.wavetable_version.fetch_add(1, Ordering::Relaxed) + 1;
        nih_log!("Updated wavetable version to {}", new_version);

        // Update frame FFT plan for the new frame size
        let mut planner = RealFftPlanner::<f32>::new();
        self.frame_fft = planner.plan_fft_forward(new_size);
        self.first_process = true;

        if let Ok(mut path_lock) = self.params.wavetable_path.lock() {
            *path_lock = path.to_string();
        }
        Ok(())
    }

    pub fn set_wavetable_path(&self, path: String) {
        if let Ok(mut path_lock) = self.params.wavetable_path.lock() {
            *path_lock = path;
        }
        self.should_reload.store(true, Ordering::Relaxed);
    }

    pub fn try_load_user_wavetable(&mut self) {
        // 1. Persisted path from the DAW session (highest priority)
        let persisted = self.params.wavetable_path.lock().ok()
            .map(|p| p.clone())
            .filter(|p| !p.is_empty());
        if let Some(path) = persisted {
            if std::path::Path::new(&path).exists()
                && self.load_wavetable_from_file(&path).is_ok()
            {
                return;
            }
        }

        // 2. Environment variable override
        if let Ok(path) = std::env::var("WAVETABLE_FILTER_PATH") {
            if std::path::Path::new(&path).exists() && self.load_wavetable_from_file(&path).is_ok()
            {
                return;
            }
        }

        // 3. Default file location
        if let Some(home) = std::env::var_os("HOME") {
            let base_path = std::path::Path::new(&home).join("wavetable-filter");

            for ext in &["wav", "wt"] {
                let path = base_path.join(format!("wavetable.{}", ext));
                if path.exists()
                    && self
                        .load_wavetable_from_file(path.to_str().unwrap())
                        .is_ok()
                {
                    return;
                }
            }
        }
    }
}

impl FilterState {
    fn new(size: usize) -> Self {
        let power_of_2_size = size.next_power_of_two();
        Self {
            history: vec![0.0; power_of_2_size],
            write_pos: 0,
            mask: power_of_2_size - 1,
        }
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.write_pos = 0;
    }

    #[inline(always)]
    fn push(&mut self, sample: f32) {
        self.history[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) & self.mask;
    }

    #[inline(always)]
    fn get(&self, offset: usize) -> f32 {
        let idx = (self.write_pos.wrapping_add(self.history.len()).wrapping_sub(offset).wrapping_sub(1)) & self.mask;
        self.history[idx]
    }

    /// Bulk read for SIMD — reads N consecutive samples into an array
    #[inline(always)]
    fn get_bulk<const N: usize>(&self, start_offset: usize) -> [f32; N] {
        let mut result = [0.0f32; N];

        let start_idx = (self.write_pos.wrapping_add(self.history.len()).wrapping_sub(start_offset).wrapping_sub(1)) & self.mask;

        if start_idx >= N - 1 {
            let src_start = start_idx - (N - 1);
            result.copy_from_slice(&self.history[src_start..src_start + N]);
        } else {
            for i in 0..N {
                let idx = (start_idx.wrapping_sub(i)) & self.mask;
                result[i] = self.history[idx];
            }
        }

        result
    }
}

impl WavetableFilterParams {
    fn new(frame_count: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self {
            wavetable_path: Arc::new(Mutex::new(String::new())),

            frequency: FloatParam::new(
                "Frequency",
                1000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" Hz"),

            frame_position: {
                let frame_count_clone = frame_count.clone();
                FloatParam::new(
                    "Frame Position",
                    0.0,
                    FloatRange::Linear { min: 0.0, max: 1.0 },
                )
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_value_to_string(Arc::new(move |value| {
                    let count = frame_count.load(std::sync::atomic::Ordering::Relaxed);
                    let frame_num = (value * (count - 1) as f32).round() as usize + 1;
                    format!("{}", frame_num)
                }))
                .with_string_to_value(Arc::new(move |string| {
                    let count = frame_count_clone.load(std::sync::atomic::Ordering::Relaxed);
                    string
                        .parse::<f32>()
                        .ok()
                        .map(|frame| (frame - 1.0) / (count - 1).max(1) as f32)
                }))
            },

            resonance: FloatParam::new(
                "Resonance",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_value_to_string(formatters::v2s_f32_percentage(0)),

            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0)),

            drive: FloatParam::new(
                "Drive",
                1.0,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0)),

            mode: EnumParam::new("Mode", FilterMode::Raw),
        }
    }
}

impl Plugin for WavetableFilter {
    const NAME: &'static str = "Wavetable Filter";
    const VENDOR: &'static str = "Michael Dungan";
    const URL: &'static str = "https://github.com/xxx/wavetable-filter";
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
    type BackgroundTask = SynthesisTask;

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        Box::new(|task| {
            let SynthesisTask {
                frame,
                cutoff_hz,
                resonance,
                sample_rate,
                filter_mode,
                pending_kernel,
                synthesis_in_flight,
                frame_fft,
                kernel_ifft,
                cplx_fft,
                cplx_ifft,
            } = task;

            if let Some((base_mags, bin_fracs)) =
                Self::compute_base_spectrum(&frame, cutoff_hz, sample_rate, &frame_fft)
            {
                let mut spectrum_work =
                    vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN / 2 + 1];
                let mut cplx_work = vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN];
                let mut kernel = vec![0.0f32; KERNEL_LEN];

                Self::apply_resonance_and_ifft(
                    &base_mags,
                    &bin_fracs,
                    resonance,
                    &mut spectrum_work,
                    &mut kernel,
                    &kernel_ifft,
                    filter_mode,
                    &mut cplx_work,
                    &cplx_fft,
                    &cplx_ifft,
                );

                if let Ok(mut pending) = pending_kernel.lock() {
                    *pending = Some(kernel);
                }
            }

            synthesis_in_flight.store(false, Ordering::Relaxed);
        })
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.params.wavetable_path.clone(),
            self.should_reload.clone(),
            self.shared_wavetable.clone(),
            self.wavetable_version.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.try_load_user_wavetable();

        // Synthesize the initial kernel so the first buffer has valid coefficients.
        // (Not on the audio thread — allocations are fine here.)
        if let Some(ref wt) = self.wavetable {
            let frame_pos = self.params.frame_position.unmodulated_normalized_value();
            let cutoff = self.params.frequency.unmodulated_plain_value();
            let resonance = self.params.resonance.unmodulated_plain_value();
            let mode = self.params.mode.value();
            let frame = wt.get_frame_interpolated(frame_pos);

            if let Some((base_mags, bin_fracs)) =
                Self::compute_base_spectrum(&frame, cutoff, self.sample_rate, &self.frame_fft)
            {
                let mut spectrum_work =
                    vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN / 2 + 1];
                let mut cplx_work = vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN];
                Self::apply_resonance_and_ifft(
                    &base_mags,
                    &bin_fracs,
                    resonance,
                    &mut spectrum_work,
                    &mut self.synthesized_kernel,
                    &self.kernel_ifft,
                    mode,
                    &mut cplx_work,
                    &self.cplx_fft,
                    &self.cplx_ifft,
                );
            }
            self.last_frame_pos = frame_pos;
            self.last_cutoff = cutoff;
            self.last_resonance = resonance;
            self.first_process = false;
        }

        true
    }

    fn reset(&mut self) {
        for state in &mut self.filter_state {
            state.reset();
        }
        self.silence_samples = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Check if we should reload the wavetable
        if self.should_reload.load(Ordering::Relaxed) {
            if let Ok(shared_wt) = self.shared_wavetable.lock() {
                let new_size = shared_wt.frame_size;

                self.current_frame_count
                    .store(shared_wt.frame_count, Ordering::Relaxed);

                self.wavetable = Some(shared_wt.clone());

                for state in &mut self.filter_state {
                    if state.history.len() != KERNEL_LEN {
                        *state = FilterState::new(KERNEL_LEN);
                    }
                }

                // Update frame FFT for new frame size; kernel_ifft and cplx_* are KERNEL_LEN constant
                let mut planner = RealFftPlanner::<f32>::new();
                self.frame_fft = planner.plan_fft_forward(new_size);

                self.first_process = true;
            }
            self.should_reload.store(false, Ordering::Relaxed);
        }

        // Pick up a completed kernel from the background thread.
        let mut do_pending_dispatch = false;
        if let Ok(mut pending) = self.pending_kernel.try_lock() {
            if let Some(new_kernel) = pending.take() {
                // Bake any in-progress crossfade before installing the new target.
                if self.crossfade_active {
                    let a = self.crossfade_alpha;
                    for i in 0..KERNEL_LEN {
                        self.synthesized_kernel[i] = self.synthesized_kernel[i] * (1.0 - a)
                            + self.crossfade_target_kernel[i] * a;
                    }
                    self.crossfade_active = false;
                    self.crossfade_alpha = 0.0;
                }
                self.crossfade_target_kernel.copy_from_slice(&new_kernel);
                self.crossfade_active = true;
                self.crossfade_alpha = 0.0;
                // synthesis_in_flight was already cleared by the task itself.
                do_pending_dispatch = self.pending_dispatch.is_some();
            }
        }

        let Some(ref wavetable) = self.wavetable else {
            return ProcessStatus::Normal;
        };

        let silence_threshold = 1e-6f32;
        let mut is_silent = true;

        let filter_mode = self.params.mode.value();

        // Advance smoothers once to read current values; the per-sample loop advances them again.
        let frame_pos = self.params.frame_position.smoothed.next();
        let cutoff = self.params.frequency.smoothed.next();
        let resonance = self.params.resonance.smoothed.next();

        // If a task just completed and there is a queued dispatch, fire it now.
        if do_pending_dispatch {
            if let Some((pd_fp, pd_cut, pd_res)) = self.pending_dispatch.take() {
                self.synthesis_in_flight.store(true, Ordering::Relaxed);
                context.execute_background(SynthesisTask {
                    frame: wavetable.get_frame_interpolated(pd_fp),
                    cutoff_hz: pd_cut,
                    resonance: pd_res,
                    sample_rate: self.sample_rate,
                    filter_mode,
                    pending_kernel: self.pending_kernel.clone(),
                    synthesis_in_flight: self.synthesis_in_flight.clone(),
                    frame_fft: self.frame_fft.clone(),
                    kernel_ifft: self.kernel_ifft.clone(),
                    cplx_fft: self.cplx_fft.clone(),
                    cplx_ifft: self.cplx_ifft.clone(),
                });
            }
        }

        // Dispatch a new task whenever frame/cutoff/resonance changes enough.
        // If a task is already in flight, store the params in pending_dispatch (latest-wins).
        let needs_update = self.first_process
            || (frame_pos - self.last_frame_pos).abs() > 0.0001
            || (cutoff - self.last_cutoff).abs() > 0.1
            || (resonance - self.last_resonance).abs() > 0.005;

        if needs_update {
            self.first_process = false;
            self.last_frame_pos = frame_pos;
            self.last_cutoff = cutoff;
            self.last_resonance = resonance;

            if !self.synthesis_in_flight.load(Ordering::Relaxed) {
                self.synthesis_in_flight.store(true, Ordering::Relaxed);
                context.execute_background(SynthesisTask {
                    frame: wavetable.get_frame_interpolated(frame_pos),
                    cutoff_hz: cutoff,
                    resonance,
                    sample_rate: self.sample_rate,
                    filter_mode,
                    pending_kernel: self.pending_kernel.clone(),
                    synthesis_in_flight: self.synthesis_in_flight.clone(),
                    frame_fft: self.frame_fft.clone(),
                    kernel_ifft: self.kernel_ifft.clone(),
                    cplx_fft: self.cplx_fft.clone(),
                    cplx_ifft: self.cplx_ifft.clone(),
                });
            } else {
                // Another task is running; queue these params for dispatch when it finishes.
                self.pending_dispatch = Some((frame_pos, cutoff, resonance));
            }
        }

        for mut channel_samples in buffer.iter_samples() {
            // Advance frame_pos/cutoff/resonance smoothers each sample (keeps convergence timing correct).
            // Their values are not needed here; synthesis already ran above for this buffer.
            let _ = self.params.frame_position.smoothed.next();
            let _ = self.params.frequency.smoothed.next();
            let _ = self.params.resonance.smoothed.next();
            let mix = self.params.mix.smoothed.next();
            let drive = self.params.drive.smoothed.next();

            // Process each channel in this sample
            for (channel_idx, sample) in channel_samples.iter_mut().enumerate() {
                let state_idx = channel_idx.min(1);
                let input = *sample;

                if input.abs() > silence_threshold {
                    is_silent = false;
                }

                let driven_input = (input * drive).tanh();
                self.filter_state[state_idx].push(driven_input);

                // SIMD convolution: 16 samples at a time.
                // During crossfade the history is read once and multiplied against both kernels
                // in the same loop to avoid fetching history twice.
                const SIMD_LANES: usize = 16;
                const SIMD_CHUNKS: usize = KERNEL_LEN / SIMD_LANES;

                let filtered: f32 = if self.crossfade_active {
                    let mut acc = f32x16::splat(0.0);
                    let mut acc2 = f32x16::splat(0.0);
                    for chunk_idx in 0..SIMD_CHUNKS {
                        let k = chunk_idx * SIMD_LANES;
                        let history_vec =
                            f32x16::from_array(self.filter_state[state_idx].get_bulk::<16>(k));
                        acc += history_vec
                            * f32x16::from_slice(&self.synthesized_kernel[k..k + SIMD_LANES]);
                        acc2 += history_vec * f32x16::from_slice(
                            &self.crossfade_target_kernel[k..k + SIMD_LANES],
                        );
                    }
                    let a = self.crossfade_alpha;
                    hsum(acc) * (1.0 - a) + hsum(acc2) * a
                } else {
                    let mut acc = f32x16::splat(0.0);
                    for chunk_idx in 0..SIMD_CHUNKS {
                        let k = chunk_idx * SIMD_LANES;
                        let history_vec =
                            f32x16::from_array(self.filter_state[state_idx].get_bulk::<16>(k));
                        acc += history_vec
                            * f32x16::from_slice(&self.synthesized_kernel[k..k + SIMD_LANES]);
                    }
                    hsum(acc)
                };

                *sample = input * (1.0 - mix) + filtered * mix;
            }

            // Advance crossfade alpha once per sample (~5 ms total fade duration).
            if self.crossfade_active {
                self.crossfade_alpha += 1.0 / (self.sample_rate * 0.005);
                if self.crossfade_alpha >= 1.0 {
                    std::mem::swap(&mut self.synthesized_kernel, &mut self.crossfade_target_kernel);
                    self.crossfade_active = false;
                    self.crossfade_alpha = 0.0;
                }
            }
        }

        // Clear filter state after ~100ms of silence
        if is_silent {
            self.silence_samples += buffer.samples();
            if self.silence_samples > (self.sample_rate * 0.1) as usize {
                for state in &mut self.filter_state {
                    state.reset();
                }
            }
        } else {
            self.silence_samples = 0;
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for WavetableFilter {
    const CLAP_ID: &'static str = "com.mpd.wavetable-filter";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A wavetable-based filter plugin");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Filter,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for WavetableFilter {
    const VST3_CLASS_ID: [u8; 16] = *b"WavetableFilter1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Filter];
}

nih_export_clap!(WavetableFilter);
nih_export_vst3!(WavetableFilter);
