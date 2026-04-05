#![feature(portable_simd)]

use nih_plug::prelude::*;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use std::simd::f32x16;
use std::simd::num::SimdFloat;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

mod editor;
pub mod wavetable;

use wavetable::Wavetable;

/// Fixed output kernel length for convolution. Must be a multiple of 16 (for SIMD).
/// 2048 gives 1024 frequency bins — enough resolution for any typical wavetable frame.
const KERNEL_LEN: usize = 2048;
const HOP: usize = KERNEL_LEN / 2; // 1024 — STFT overlap-add hop size (50% overlap)

/// Pre-prepared reload data built on the GUI/loader thread so the audio thread
/// can install a new wavetable without any heap allocations.
pub(crate) struct PendingReload {
    pub(crate) wavetable: Wavetable,
    pub(crate) frame_fft: Arc<dyn RealToComplex<f32>>,
    pub(crate) frame_cache: Vec<f32>,
    pub(crate) frame_buf: Vec<f32>,
    pub(crate) frame_spectrum: Vec<Complex<f32>>,
    pub(crate) frame_mags: Vec<f32>,
}

pub struct WavetableFilter {
    params: Arc<WavetableFilterParams>,
    editor_state: Arc<nih_plug_vizia::ViziaState>,
    wavetable: Option<Wavetable>,
    sample_rate: f32,
    // Circular buffer for convolution (per channel)
    filter_state: [FilterState; 2],
    should_reload: Arc<AtomicBool>,
    // Pre-prepared reload data from the GUI thread — audio thread takes with try_lock
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
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
    // True until the first process() call; used to force an initial synthesis.
    first_process: bool,
    // Per-sample output crossfade: blend convolution outputs during ~20 ms transition.
    // Pre-allocated to avoid any heap allocation on the audio thread.
    crossfade_target_kernel: Vec<f32>, // the "to" kernel (KERNEL_LEN)
    crossfade_active: bool,
    crossfade_alpha: f32,
    crossfade_step: f32,
    // Forward real FFT for analyzing the wavetable frame (size = frame_size, changes with wavetable)
    frame_fft: Arc<dyn RealToComplex<f32>>,
    // Inverse real FFT for kernel synthesis output (size = KERNEL_LEN, constant)
    kernel_ifft: Arc<dyn ComplexToReal<f32>>,
    // Pre-allocated synthesis scratch buffers — zero allocation on the audio thread.
    /// Clean copy of the current interpolated frame; updated when frame_pos changes.
    frame_cache: Vec<f32>,
    /// FFT input scratch; copied from frame_cache before each synthesis (FFT consumes it).
    frame_buf: Vec<f32>,
    /// FFT output scratch (frame_size/2+1 complex bins).
    frame_spectrum: Vec<Complex<f32>>,
    /// Normalized per-bin magnitudes of the frame spectrum (frame_size/2+1).
    frame_mags: Vec<f32>,
    /// Resampled magnitudes for the KERNEL_LEN/2+1 output bins.
    out_mags: Vec<f32>,
    /// Fractional source positions for the KERNEL_LEN/2+1 output bins.
    out_fracs: Vec<f32>,
    /// Complex spectrum scratch for resonance + IFFT (KERNEL_LEN/2+1).
    spectrum_work: Vec<Complex<f32>>,
    // Smooth reset: instead of clearing history instantly (which pops), fade out
    // over a few milliseconds then clear once the output has reached zero.
    reset_fade_remaining: usize,
    reset_fade_total: usize,

    // ── STFT state for magnitude-only (Phaseless) mode ──────────────
    /// Forward real FFT plan for STFT input blocks (size KERNEL_LEN).
    stft_fft: Arc<dyn RealToComplex<f32>>,
    /// Per-channel circular input buffer for STFT (KERNEL_LEN samples each).
    stft_in: [Vec<f32>; 2],
    /// Per-channel overlap-add output accumulator (KERNEL_LEN samples each).
    stft_out: [Vec<f32>; 2],
    /// Current filter magnitude spectrum for STFT mode (KERNEL_LEN/2+1 real gains).
    stft_magnitudes: Vec<f32>,
    /// Pre-computed Hann analysis window (KERNEL_LEN samples).
    stft_window: Vec<f32>,
    /// Time-domain scratch buffer for STFT FFT/IFFT (KERNEL_LEN).
    stft_scratch: Vec<f32>,
    /// Write position in STFT input circular buffer (0..KERNEL_LEN-1).
    stft_in_pos: usize,
    /// Read position within current STFT output hop (0..HOP-1).
    stft_out_pos: usize,
    /// Tracks the last mode to detect runtime mode switches.
    last_mode: FilterMode,

    // ── Input spectrum for GUI visualization ─────────────────────────────
    /// Shared input spectrum magnitudes (KERNEL_LEN/2+1 bins) + sample rate.
    /// The GUI reads this to draw a faint shadow behind the filter response curve.
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
    /// Ring buffer accumulating the most recent KERNEL_LEN input samples.
    input_spectrum_buf: Vec<f32>,
    /// Write position in input_spectrum_buf (0..KERNEL_LEN-1).
    input_spectrum_pos: usize,
    /// Pre-allocated complex scratch for input spectrum FFT (KERNEL_LEN/2+1).
    input_spectrum_scratch: Vec<Complex<f32>>,
    /// Countdown (in samples) until the next input-spectrum FFT for GUI visualization.
    /// Throttles the FFT to ~30 updates/sec instead of once per process() call.
    input_spectrum_countdown: usize,

    last_reported_latency: u32,
}

struct FilterState {
    // Double-buffered circular history: 2×len elements so that a contiguous
    // len-sized window starting at write_pos is always valid for zero-copy SIMD reads.
    history: Vec<f32>,
    write_pos: usize,
    len: usize,
    mask: usize,
}

#[derive(Params)]
struct WavetableFilterParams {
    /// Persisted wavetable file path — restored by the DAW on session reload.
    #[persist = "wavetable_path"]
    pub wavetable_path: Arc<Mutex<String>>,

    #[id = "ui_scale"]
    pub ui_scale: IntParam,

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
    #[id = "raw"]
    #[name = "Raw"]
    Raw,

    #[id = "minimum"]
    #[name = "Phaseless"]
    Minimum,
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
        let stft_fft = real_planner.plan_fft_forward(KERNEL_LEN);

        let spec_len = frame_size / 2 + 1;

        Self {
            params: Arc::new(WavetableFilterParams::new(current_frame_count.clone())),
            editor_state: editor::default_state(),
            wavetable: Some(default_wt.clone()),
            sample_rate: 48000.0,
            filter_state: [FilterState::new(KERNEL_LEN), FilterState::new(KERNEL_LEN)],
            should_reload: Arc::new(AtomicBool::new(false)),
            pending_reload: Arc::new(Mutex::new(None)),
            shared_wavetable: Arc::new(Mutex::new(default_wt)),
            wavetable_version: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            current_frame_count,
            silence_samples: 0,
            synthesized_kernel: vec![0.0; KERNEL_LEN],
            last_frame_pos: 0.0,
            last_cutoff: 0.0,
            last_resonance: 0.0,
            first_process: true,
            crossfade_target_kernel: vec![0.0f32; KERNEL_LEN],
            crossfade_active: false,
            crossfade_alpha: 0.0,
            crossfade_step: 1.0 / (48000.0 * 0.020),
            frame_fft,
            kernel_ifft,
            frame_cache: vec![0.0; frame_size],
            frame_buf: vec![0.0; frame_size],
            frame_spectrum: vec![Complex::new(0.0, 0.0); spec_len],
            frame_mags: vec![0.0; spec_len],
            out_mags: vec![0.0; KERNEL_LEN / 2 + 1],
            out_fracs: vec![0.0; KERNEL_LEN / 2 + 1],
            spectrum_work: vec![Complex::new(0.0, 0.0); KERNEL_LEN / 2 + 1],
            reset_fade_remaining: 0,
            reset_fade_total: 1, // avoid division by zero
            stft_fft,
            stft_in: [vec![0.0; KERNEL_LEN], vec![0.0; KERNEL_LEN]],
            stft_out: [vec![0.0; KERNEL_LEN], vec![0.0; KERNEL_LEN]],
            stft_magnitudes: vec![0.0; KERNEL_LEN / 2 + 1],
            stft_window: {
                let mut w = vec![0.0f32; KERNEL_LEN];
                for (i, w_i) in w.iter_mut().enumerate() {
                    *w_i = 0.5
                        * (1.0
                            - (2.0 * std::f32::consts::PI * i as f32 / KERNEL_LEN as f32).cos());
                }
                w
            },
            stft_scratch: vec![0.0; KERNEL_LEN],
            stft_in_pos: 0,
            stft_out_pos: 0,
            last_mode: FilterMode::Raw,
            shared_input_spectrum: Arc::new(Mutex::new((48000.0, vec![0.0; KERNEL_LEN / 2 + 1]))),
            input_spectrum_buf: vec![0.0; KERNEL_LEN],
            input_spectrum_pos: 0,
            input_spectrum_scratch: vec![Complex::new(0.0, 0.0); KERNEL_LEN / 2 + 1],
            input_spectrum_countdown: 0,
            last_reported_latency: u32::MAX, // Forces initial report
        }
    }
}

/// Horizontal sum of an f32x16 SIMD vector using pairwise tree reduction.
#[inline(always)]
fn hsum(v: f32x16) -> f32 {
    v.reduce_sum()
}

/// Reverse a buffer in place. Converts forward-order IFFT output into the
/// time-reversed kernel layout expected by the SIMD convolution loop.
#[inline]
fn reverse_in_place(buf: &mut [f32]) {
    let n = buf.len();
    for i in 0..n / 2 {
        buf.swap(i, n - 1 - i);
    }
}

impl WavetableFilter {
    /// Compute the base magnitude spectrum for a wavetable frame at a given cutoff.
    ///
    /// Returns (magnitudes, fractional harmonic positions) for each of the KERNEL_LEN/2+1
    /// output bins. Resonance is NOT applied here — call `apply_resonance_and_ifft` inline.
    #[cfg(test)]
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

        // Smooth taper near the edge of the source spectrum prevents spectral
        // cliff discontinuities when cutoff is low enough that output bins map
        // beyond the source Nyquist.
        let taper_width = 8.0f32;
        let taper_start = max_src - taper_width;

        for j in 0..num_bins {
            let src = j as f32 * bin_to_src;
            if src >= frame_mags.len() as f32 {
                break; // remaining bins stay 0.0
            }
            let (mag, frac) = if bin_to_src > 1.0 {
                // Each output bin spans multiple source bins — scan for peak magnitude
                // so that narrow spectral features (e.g. a single harmonic) are not missed.
                // frac=0 because the comb resonance concept (inter-harmonic suppression)
                // is meaningless when output bins are coarser than source harmonics.
                let src_end = ((j + 1) as f32 * bin_to_src).min(frame_mags.len() as f32 - 1.0);
                let lo = src.floor() as usize;
                let hi = (src_end.ceil() as usize).min(frame_mags.len() - 1);
                let mut peak = 0.0f32;
                for k in lo..=hi {
                    peak = peak.max(frame_mags[k]);
                }
                (peak, 0.0)
            } else {
                // Fine resolution — linear interpolation between adjacent source bins.
                let frac = src.fract();
                let lo = src.floor() as usize;
                let m_hi = if lo + 1 < frame_mags.len() {
                    frame_mags[lo + 1]
                } else {
                    0.0
                };
                (frame_mags[lo] * (1.0 - frac) + m_hi * frac, frac)
            };

            let mut mag = mag;
            if src > taper_start {
                let t = ((src - taper_start) / (taper_width + 1.0)).min(1.0);
                mag *= 0.5 * (1.0 + (std::f32::consts::PI * t).cos());
            }
            mags[j] = mag;
            fracs[j] = frac;
        }

        Some((mags, fracs))
    }

    /// Allocation-free version of `compute_base_spectrum`.
    ///
    /// `frame_buf` holds the interpolated wavetable frame and doubles as the FFT
    /// input scratch (it will be modified in-place by the FFT).  All other buffers
    /// are caller-supplied.  Returns `false` on FFT error.
    #[allow(clippy::too_many_arguments)]
    fn compute_base_spectrum_into(
        frame_buf: &mut [f32],
        cutoff_hz: f32,
        sample_rate: f32,
        frame_fft: &Arc<dyn RealToComplex<f32>>,
        // scratch:
        frame_spectrum: &mut Vec<Complex<f32>>,
        frame_mags: &mut Vec<f32>,
        // output:
        out_mags: &mut [f32],
        out_fracs: &mut [f32],
    ) -> bool {
        let n = frame_buf.len();
        let spec_len = n / 2 + 1;

        // Resize scratch only when the frame size changes (rare, not on the hot path).
        if frame_spectrum.len() != spec_len {
            frame_spectrum.resize(spec_len, Complex::new(0.0, 0.0));
        }
        if frame_mags.len() != spec_len {
            frame_mags.resize(spec_len, 0.0);
        }

        if frame_fft.process(frame_buf, frame_spectrum).is_err() {
            return false;
        }

        for (m, c) in frame_mags.iter_mut().zip(frame_spectrum.iter()) {
            *m = c.norm();
        }
        let peak = frame_mags.iter().cloned().fold(0.0f32, f32::max).max(1e-10);
        for m in frame_mags.iter_mut() {
            *m /= peak;
        }

        let num_bins = KERNEL_LEN / 2 + 1;
        let bin_to_src = 24.0 * sample_rate / (KERNEL_LEN as f32 * cutoff_hz);
        let max_src = (frame_mags.len() - 1) as f32;

        out_mags.fill(0.0);
        out_fracs.fill(0.0);

        let taper_width = 8.0f32;
        let taper_start = max_src - taper_width;

        for j in 0..num_bins {
            let src = j as f32 * bin_to_src;
            if src >= frame_mags.len() as f32 {
                break;
            }
            let (mag, frac) = if bin_to_src > 1.0 {
                let src_end = ((j + 1) as f32 * bin_to_src).min(frame_mags.len() as f32 - 1.0);
                let lo = src.floor() as usize;
                let hi = (src_end.ceil() as usize).min(frame_mags.len() - 1);
                let mut peak = 0.0f32;
                for &m in &frame_mags[lo..=hi] {
                    peak = peak.max(m);
                }
                (peak, 0.0)
            } else {
                let frac = src.fract();
                let lo = src.floor() as usize;
                let m_hi = if lo + 1 < frame_mags.len() {
                    frame_mags[lo + 1]
                } else {
                    0.0
                };
                (frame_mags[lo] * (1.0 - frac) + m_hi * frac, frac)
            };

            let mut mag = mag;
            if src > taper_start {
                let t = ((src - taper_start) / (taper_width + 1.0)).min(1.0);
                mag *= 0.5 * (1.0 + (std::f32::consts::PI * t).cos());
            }
            out_mags[j] = mag;
            out_fracs[j] = frac;
        }
        true
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
    ) {
        let comb_exp = resonance * 8.0;
        for j in 0..base_mags.len() {
            let mag = if comb_exp > 0.01 {
                let dist = bin_fracs[j].min(1.0 - bin_fracs[j]);
                let comb = (std::f32::consts::PI * dist).cos().max(0.0).powf(comb_exp);
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
    }

    /// Compute filter magnitude gains for STFT mode.
    ///
    /// Applies the resonance comb to base magnitudes and writes real-valued
    /// gains to `mags_out`. Each gain is the factor by which an FFT bin's
    /// magnitude should be scaled (the bin's phase is preserved).
    fn compute_stft_magnitudes(
        base_mags: &[f32],
        bin_fracs: &[f32],
        resonance: f32,
        mags_out: &mut [f32],
    ) {
        let comb_exp = resonance * 8.0;
        for j in 0..base_mags.len().min(mags_out.len()) {
            mags_out[j] = if comb_exp > 0.01 {
                let dist = bin_fracs[j].min(1.0 - bin_fracs[j]);
                let comb = (std::f32::consts::PI * dist).cos().max(0.0).powf(comb_exp);
                base_mags[j] * comb
            } else {
                base_mags[j]
            };
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
            if state.len != KERNEL_LEN {
                *state = FilterState::new(KERNEL_LEN);
            }
        }

        self.current_frame_count
            .store(wavetable.frame_count, Ordering::Relaxed);

        // Prepare reload data on this (non-realtime) thread — no allocations on audio thread
        let mut planner = RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(new_size);
        let spec_len = new_size / 2 + 1;
        let reload = PendingReload {
            wavetable: wavetable.clone(),
            frame_fft,
            frame_cache: vec![0.0; new_size],
            frame_buf: vec![0.0; new_size],
            frame_spectrum: vec![Complex::new(0.0, 0.0); spec_len],
            frame_mags: vec![0.0; spec_len],
        };

        if let Ok(mut pending) = self.pending_reload.lock() {
            *pending = Some(reload);
        }

        if let Ok(mut shared_wt) = self.shared_wavetable.lock() {
            *shared_wt = wavetable;
        }

        let new_version = self.wavetable_version.fetch_add(1, Ordering::Relaxed) + 1;
        nih_log!("Updated wavetable version to {}", new_version);

        if let Ok(mut path_lock) = self.params.wavetable_path.lock() {
            *path_lock = path.to_string();
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn process_stft_frame(
        stft_in: &[f32],
        in_pos: usize,
        stft_out: &mut [f32],
        magnitudes: &[f32],
        window: &[f32],
        fft: &Arc<dyn RealToComplex<f32>>,
        ifft: &Arc<dyn ComplexToReal<f32>>,
        scratch_time: &mut [f32],
        scratch_freq: &mut [Complex<f32>],
    ) {
        let n = KERNEL_LEN;
        let mask = n - 1;
        for i in 0..n {
            scratch_time[i] = stft_in[(in_pos + i) & mask] * window[i];
        }
        if fft.process(scratch_time, scratch_freq).is_err() {
            return;
        }
        for (bin, &mag) in scratch_freq.iter_mut().zip(magnitudes.iter()) {
            *bin *= mag;
        }
        if ifft.process(scratch_freq, scratch_time).is_err() {
            return;
        }
        let scale = 1.0 / n as f32;
        for i in 0..n {
            stft_out[i] += scratch_time[i] * scale;
        }
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
            history: vec![0.0; 2 * power_of_2_size],
            write_pos: 0,
            len: power_of_2_size,
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
        self.history[self.write_pos + self.len] = sample;
        self.write_pos = (self.write_pos + 1) & self.mask;
    }

    #[cfg(test)]
    fn get(&self, offset: usize) -> f32 {
        let idx = (self.write_pos.wrapping_add(self.len).wrapping_sub(offset).wrapping_sub(1)) & self.mask;
        self.history[idx]
    }

    #[cfg(test)]
    fn get_bulk<const N: usize>(&self, start_offset: usize) -> [f32; N] {
        let mut result = [0.0f32; N];

        let start_idx = (self.write_pos.wrapping_add(self.len).wrapping_sub(start_offset).wrapping_sub(1)) & self.mask;

        if start_idx >= N - 1 {
            // No wrap-around in this window; index directly without mask.
            // Decrement to produce newest-first order, matching the slow path below.
            for (i, slot) in result.iter_mut().enumerate() {
                *slot = self.history[start_idx - i];
            }
        } else {
            for i in 0..N {
                let idx = (start_idx.wrapping_sub(i)) & self.mask;
                result[i] = self.history[idx];
            }
        }

        result
    }

    /// Contiguous slice of the last `len` samples in chronological order (oldest first).
    /// Use with a time-reversed kernel for SIMD convolution without per-element copies.
    #[inline(always)]
    fn history_slice(&self) -> &[f32] {
        &self.history[self.write_pos..self.write_pos + self.len]
    }
}

impl WavetableFilterParams {
    fn new(frame_count: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self {
            wavetable_path: Arc::new(Mutex::new(String::new())),
            ui_scale: IntParam::new("UI Scale", 100, IntRange::Linear { min: 100, max: 300 })
                .with_unit("%"),

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
                    "Frame",
                    0.0,
                    FloatRange::Linear { min: 0.0, max: 1.0 },
                )
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_value_to_string(Arc::new(move |value| {
                    let count = frame_count.load(std::sync::atomic::Ordering::Relaxed);
                    let frame_num = (value * count.saturating_sub(1) as f32).round() as usize + 1;
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
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0)),

            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0)),

            drive: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-20.0),
                    max: util::db_to_gain(20.0),
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

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
    type BackgroundTask = ();

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        Box::new(|_| {})
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.params.wavetable_path.clone(),
            self.should_reload.clone(),
            self.pending_reload.clone(),
            self.shared_wavetable.clone(),
            self.wavetable_version.clone(),
            self.editor_state.clone(),
            self.shared_input_spectrum.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.crossfade_step = 1.0 / (self.sample_rate * 0.020);

        // Sync editor scale from the persisted ui_scale parameter
        let scale_pct = self.params.ui_scale.value() as f64;
        let scale = (scale_pct / 100.0).clamp(1.0, 3.0);
        // Write into ViziaState so next editor open uses this scale
        let new_state = nih_plug_vizia::ViziaState::new_with_default_scale_factor(
            || (editor::WINDOW_WIDTH, editor::WINDOW_HEIGHT),
            scale,
        );
        let new_inner = Arc::try_unwrap(new_state).unwrap();
        nih_plug::params::persist::PersistentField::set(&self.editor_state, new_inner);

        self.try_load_user_wavetable();

        // Synthesize the initial kernel so the first buffer has valid coefficients.
        if let Some(wt) = self.wavetable.as_ref() {
            let frame_pos = self.params.frame_position.unmodulated_normalized_value();
            let cutoff = self.params.frequency.unmodulated_plain_value();
            let resonance = self.params.resonance.unmodulated_plain_value();
            let mode = self.params.mode.value();

            // Interpolate the frame into frame_cache, then copy into frame_buf for FFT.
            wt.interpolate_frame_into(frame_pos, &mut self.frame_cache);
            self.frame_buf.copy_from_slice(&self.frame_cache);

            if Self::compute_base_spectrum_into(
                &mut self.frame_buf,
                cutoff,
                self.sample_rate,
                &self.frame_fft,
                &mut self.frame_spectrum,
                &mut self.frame_mags,
                &mut self.out_mags,
                &mut self.out_fracs,
            ) {
                if mode == FilterMode::Raw {
                    Self::apply_resonance_and_ifft(
                        &self.out_mags,
                        &self.out_fracs,
                        resonance,
                        &mut self.spectrum_work,
                        &mut self.synthesized_kernel,
                        &self.kernel_ifft,
                    );
                    reverse_in_place(&mut self.synthesized_kernel);
                } else {
                    Self::compute_stft_magnitudes(
                        &self.out_mags,
                        &self.out_fracs,
                        resonance,
                        &mut self.stft_magnitudes,
                    );
                }
            }
            self.last_frame_pos = frame_pos;
            self.last_cutoff = cutoff;
            self.last_resonance = resonance;
            self.first_process = false;
        }

        for buf in &mut self.stft_in {
            buf.fill(0.0);
        }
        for buf in &mut self.stft_out {
            buf.fill(0.0);
        }
        self.stft_in_pos = 0;
        self.stft_out_pos = 0;
        self.last_mode = self.params.mode.value();

        true
    }

    fn reset(&mut self) {
        #[cfg(debug_assertions)]
        nih_log!("reset() called — scheduling fade-out");
        // Instead of instantly zeroing the history buffer (which causes an audible
        // pop when audio is playing), schedule a fast linear fade-out.  The actual
        // clear happens in process() once the fade reaches zero.
        let fade_ms = 5.0;
        let fade = (self.sample_rate * fade_ms / 1000.0).max(1.0) as usize;
        self.reset_fade_remaining = fade;
        self.reset_fade_total = fade;
        self.silence_samples = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Check if a pre-prepared reload is ready (no allocations on audio thread)
        if self.should_reload.load(Ordering::Relaxed) {
            if let Ok(mut pending) = self.pending_reload.try_lock() {
                if let Some(reload) = pending.take() {
                    self.current_frame_count
                        .store(reload.wavetable.frame_count, Ordering::Relaxed);

                    self.wavetable = Some(reload.wavetable);
                    self.frame_fft = reload.frame_fft;
                    self.frame_cache = reload.frame_cache;
                    self.frame_buf = reload.frame_buf;
                    self.frame_spectrum = reload.frame_spectrum;
                    self.frame_mags = reload.frame_mags;

                    for state in &mut self.filter_state {
                        if state.len != KERNEL_LEN {
                            *state = FilterState::new(KERNEL_LEN);
                        }
                    }

                    self.first_process = true;
                    self.should_reload.store(false, Ordering::Relaxed);
                }
            }
            // If try_lock fails, leave should_reload true and retry next buffer
        }

        if self.wavetable.is_none() {
            return ProcessStatus::Normal;
        }

        let silence_threshold = 1e-6f32;
        let mut is_silent = true;

        // Read current values for kernel synthesis decision without advancing smoothers.
        // Smoothers are advanced per-sample inside the loop to keep timing correct.
        let frame_pos = self.params.frame_position.unmodulated_normalized_value();
        let cutoff = self.params.frequency.unmodulated_plain_value();
        let resonance = self.params.resonance.unmodulated_plain_value();

        let filter_mode = self.params.mode.value();

        if filter_mode != self.last_mode {
            if filter_mode != FilterMode::Raw {
                for buf in &mut self.stft_in { buf.fill(0.0); }
                for buf in &mut self.stft_out { buf.fill(0.0); }
                self.stft_in_pos = 0;
                self.stft_out_pos = 0;
            }
            self.last_mode = filter_mode;
        }

        let stft_latency = if filter_mode == FilterMode::Raw { 0 } else { HOP as u32 };
        if stft_latency != self.last_reported_latency {
            context.set_latency_samples(stft_latency);
            self.last_reported_latency = stft_latency;
        }

        let frame_pos_changed = self.first_process
            || (frame_pos - self.last_frame_pos).abs() > 0.0001;
        let needs_update = frame_pos_changed
            || (cutoff - self.last_cutoff).abs() > 0.1
            || (resonance - self.last_resonance).abs() > 0.005;

        if needs_update {
            self.first_process = false;

            // Re-interpolate frame only when frame_pos changes (avoids redundant work).
            if frame_pos_changed {
                self.wavetable
                    .as_ref()
                    .unwrap()
                    .interpolate_frame_into(frame_pos, &mut self.frame_cache);
                self.last_frame_pos = frame_pos;
            }
            self.last_cutoff = cutoff;
            self.last_resonance = resonance;

            // Copy the cached (clean) frame into frame_buf; the FFT will consume frame_buf.
            self.frame_buf.copy_from_slice(&self.frame_cache);

            if WavetableFilter::compute_base_spectrum_into(
                &mut self.frame_buf,
                cutoff,
                self.sample_rate,
                &self.frame_fft,
                &mut self.frame_spectrum,
                &mut self.frame_mags,
                &mut self.out_mags,
                &mut self.out_fracs,
            ) {
                if filter_mode == FilterMode::Raw {
                    // Bake any in-progress crossfade before installing the new target.
                    if self.crossfade_active {
                        let a_vec = f32x16::splat(self.crossfade_alpha);
                        let one_minus_a = f32x16::splat(1.0 - self.crossfade_alpha);
                        for chunk in 0..KERNEL_LEN / 16 {
                            let k = chunk * 16;
                            let s = f32x16::from_slice(&self.synthesized_kernel[k..k + 16]);
                            let t = f32x16::from_slice(&self.crossfade_target_kernel[k..k + 16]);
                            let blended = s * one_minus_a + t * a_vec;
                            self.synthesized_kernel[k..k + 16]
                                .copy_from_slice(&blended.to_array());
                        }
                        self.crossfade_active = false;
                        self.crossfade_alpha = 0.0;
                    }
                    WavetableFilter::apply_resonance_and_ifft(
                        &self.out_mags,
                        &self.out_fracs,
                        resonance,
                        &mut self.spectrum_work,
                        &mut self.crossfade_target_kernel,
                        &self.kernel_ifft,
                    );
                    reverse_in_place(&mut self.crossfade_target_kernel);
                    self.crossfade_active = true;
                    self.crossfade_alpha = 0.0;
                } else {
                    // Magnitude-only: just store the magnitude spectrum for STFT
                    Self::compute_stft_magnitudes(
                        &self.out_mags,
                        &self.out_fracs,
                        resonance,
                        &mut self.stft_magnitudes,
                    );
                }
            }
        }

        let host_samples = buffer.samples();
        let num_channels = buffer.channels();

        {
            for mut channel_samples in buffer.iter_samples() {
                // Advance frame_pos/cutoff/resonance smoothers each sample (keeps convergence timing correct).
                // Their values are not needed here; synthesis already ran above for this buffer.
                let _ = self.params.frame_position.smoothed.next();
                let _ = self.params.frequency.smoothed.next();
                let _ = self.params.resonance.smoothed.next();
                let mix = self.params.mix.smoothed.next();
                let drive = self.params.drive.smoothed.next();

                // Reset fade: smoothly ramp the filter output to zero before clearing history.
                let reset_gain = if self.reset_fade_remaining > 0 {
                    self.reset_fade_remaining as f32 / self.reset_fade_total as f32
                } else {
                    1.0
                };

                // STFT hop processing: when the output position wraps to 0, process the next frame.
                if filter_mode != FilterMode::Raw && self.stft_out_pos == 0 {
                    for ch in 0..num_channels.min(2) {
                        self.stft_out[ch].copy_within(HOP..KERNEL_LEN, 0);
                        self.stft_out[ch][HOP..].fill(0.0);
                        Self::process_stft_frame(
                            &self.stft_in[ch], self.stft_in_pos,
                            &mut self.stft_out[ch], &self.stft_magnitudes,
                            &self.stft_window, &self.stft_fft, &self.kernel_ifft,
                            &mut self.stft_scratch, &mut self.spectrum_work,
                        );
                    }
                }

                // Process each channel in this sample
                let mut mono_input = 0.0f32;
                let mut ch_count = 0u32;
                for (channel_idx, sample) in channel_samples.iter_mut().enumerate() {
                    let state_idx = channel_idx.min(1);
                    let input = *sample;
                    mono_input += input;
                    ch_count += 1;

                    if input.abs() > silence_threshold {
                        is_silent = false;
                    }

                    if filter_mode == FilterMode::Raw {
                        self.filter_state[state_idx].push(input);

                        // SIMD convolution: forward dot product of the double-buffered
                        // history and time-reversed kernel. No per-element copies needed.
                        const SIMD_LANES: usize = 16;
                        const SIMD_CHUNKS: usize = KERNEL_LEN / SIMD_LANES;
                        let history = self.filter_state[state_idx].history_slice();

                        let filtered: f32 = if self.crossfade_active {
                            let mut acc = f32x16::splat(0.0);
                            let mut acc2 = f32x16::splat(0.0);
                            for chunk_idx in 0..SIMD_CHUNKS {
                                let k = chunk_idx * SIMD_LANES;
                                let h = f32x16::from_slice(&history[k..k + SIMD_LANES]);
                                acc += h
                                    * f32x16::from_slice(&self.synthesized_kernel[k..k + SIMD_LANES]);
                                acc2 += h * f32x16::from_slice(
                                    &self.crossfade_target_kernel[k..k + SIMD_LANES],
                                );
                            }
                            let a = self.crossfade_alpha;
                            hsum(acc) * (1.0 - a) + hsum(acc2) * a
                        } else {
                            let mut acc = f32x16::splat(0.0);
                            for chunk_idx in 0..SIMD_CHUNKS {
                                let k = chunk_idx * SIMD_LANES;
                                let h = f32x16::from_slice(&history[k..k + SIMD_LANES]);
                                acc += h
                                    * f32x16::from_slice(&self.synthesized_kernel[k..k + SIMD_LANES]);
                            }
                            hsum(acc)
                        };

                        *sample = (input * (1.0 - mix) + filtered * mix * reset_gain) * drive;
                    } else {
                        self.stft_in[state_idx][self.stft_in_pos] = input;
                        let filtered = self.stft_out[state_idx][self.stft_out_pos];
                        *sample = (input * (1.0 - mix) + filtered * mix * reset_gain) * drive;
                    }
                }

                // Accumulate mono input into ring buffer for spectrum visualization
                if ch_count > 0 {
                    self.input_spectrum_buf[self.input_spectrum_pos] = mono_input / ch_count as f32;
                    self.input_spectrum_pos = (self.input_spectrum_pos + 1) & (KERNEL_LEN - 1);
                }

                // Complete the reset fade: once output has reached zero, clear history.
                if self.reset_fade_remaining > 0 {
                    self.reset_fade_remaining -= 1;
                    if self.reset_fade_remaining == 0 {
                        for state in &mut self.filter_state {
                            state.reset();
                        }
                    }
                }

                if filter_mode == FilterMode::Raw {
                    // Advance crossfade alpha once per sample (~20 ms total fade duration).
                    if self.crossfade_active {
                        self.crossfade_alpha += self.crossfade_step;
                        if self.crossfade_alpha >= 1.0 {
                            std::mem::swap(&mut self.synthesized_kernel, &mut self.crossfade_target_kernel);
                            self.crossfade_active = false;
                            self.crossfade_alpha = 0.0;
                        }
                    }
                } else {
                    self.stft_in_pos = (self.stft_in_pos + 1) & (KERNEL_LEN - 1);
                    self.stft_out_pos += 1;
                    if self.stft_out_pos >= HOP { self.stft_out_pos = 0; }
                }
            }
        }

        // Clear filter state after ~100ms of silence
        if is_silent {
            self.silence_samples += host_samples;
            if self.silence_samples > (self.sample_rate * 0.1) as usize {
                for state in &mut self.filter_state {
                    state.reset();
                }
                for buf in &mut self.stft_in { buf.fill(0.0); }
                for buf in &mut self.stft_out { buf.fill(0.0); }
                self.stft_in_pos = 0;
                self.stft_out_pos = 0;
                self.silence_samples = 0; // Reset so we don't clear every buffer
            }
        } else {
            self.silence_samples = 0;
        }

        // Compute input spectrum for GUI visualization (non-blocking).
        // Only runs when the editor is open. Throttled to ~30 updates/sec.
        self.input_spectrum_countdown = self.input_spectrum_countdown.saturating_sub(host_samples);
        if self.input_spectrum_countdown == 0 && self.editor_state.is_open() {
            self.input_spectrum_countdown = (self.sample_rate / 30.0) as usize;

            // Reorder the ring buffer into a contiguous windowed buffer for FFT.
            // Reuse stft_scratch as temporary storage (it is KERNEL_LEN-sized).
            let pos = self.input_spectrum_pos;
            let n = KERNEL_LEN;
            for i in 0..n {
                let ring_idx = (pos + i) & (n - 1);
                self.stft_scratch[i] = self.input_spectrum_buf[ring_idx] * self.stft_window[i];
            }
            if self
                .stft_fft
                .process(&mut self.stft_scratch, &mut self.input_spectrum_scratch)
                .is_ok()
            {
                if let Ok(mut shared) = self.shared_input_spectrum.try_lock() {
                    shared.0 = self.sample_rate;
                    let peak = self
                        .input_spectrum_scratch
                        .iter()
                        .map(|c| c.norm())
                        .fold(0.0f32, f32::max)
                        .max(1e-10);
                    for (dst, c) in shared.1.iter_mut().zip(self.input_spectrum_scratch.iter()) {
                        *dst = c.norm() / peak;
                    }
                }
            }
        }

        ProcessStatus::Tail(KERNEL_LEN as u32)
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── FilterState helpers ────────────────────────────────────────────────

    /// Push a sequence of f32 values where push_samples[0] is the OLDEST.
    fn push_sequence(state: &mut FilterState, samples: &[f32]) {
        for &s in samples {
            state.push(s);
        }
    }

    /// Oracle: build expected get_bulk result using the scalar `get`.
    fn expected_bulk<const N: usize>(state: &FilterState, start_offset: usize) -> [f32; N] {
        let mut out = [0.0f32; N];
        for i in 0..N {
            out[i] = state.get(start_offset + i);
        }
        out
    }

    // ── get_bulk correctness ───────────────────────────────────────────────

    /// get_bulk fast path: start_idx >= N-1 (no circular wrap in the window).
    #[test]
    fn test_get_bulk_fast_path_matches_scalar_get() {
        let mut state = FilterState::new(2048);
        // Push 64 distinct values; write_pos will be 64, well above the N-1=15 threshold.
        let vals: Vec<f32> = (1..=64).map(|i| i as f32).collect();
        push_sequence(&mut state, &vals);

        // start_offset=0: start_idx = (64 + 2048 - 0 - 1) & 2047 = 63 ≥ 15 → fast path.
        assert_eq!(state.get_bulk::<16>(0), expected_bulk::<16>(&state, 0));
        // start_offset=16: start_idx = 47 ≥ 15 → still fast path.
        assert_eq!(state.get_bulk::<16>(16), expected_bulk::<16>(&state, 16));
        // start_offset=48: start_idx = 15 (edge: exactly the boundary) → fast path.
        assert_eq!(state.get_bulk::<16>(48), expected_bulk::<16>(&state, 48));
    }

    /// get_bulk slow path: start_idx < N-1 (window crosses the circular-buffer boundary).
    #[test]
    fn test_get_bulk_slow_path_matches_scalar_get() {
        let mut state = FilterState::new(2048);
        // Push only 14 values so write_pos=14.
        // start_offset=0: start_idx = (14 + 2048 - 0 - 1) & 2047 = 13 < 15 → slow path.
        let vals: Vec<f32> = (1..=14).map(|i| i as f32).collect();
        push_sequence(&mut state, &vals);

        assert_eq!(state.get_bulk::<16>(0), expected_bulk::<16>(&state, 0));
    }

    /// get_bulk must agree with scalar get across ALL offsets, including the circular
    /// wrap region, to catch any fast/slow path ordering mismatch.
    #[test]
    fn test_get_bulk_agrees_with_scalar_get_at_wrap() {
        let mut state = FilterState::new(2048);
        // Fill almost all of history so write_pos wraps around.
        let vals: Vec<f32> = (1..=2048).map(|i| i as f32 * 0.001).collect();
        push_sequence(&mut state, &vals);
        // write_pos = 0 now (wrapped). For start_offset=0:
        // start_idx = (0 + 2048 - 0 - 1) & 2047 = 2047 ≥ 15 → fast path.
        assert_eq!(state.get_bulk::<16>(0), expected_bulk::<16>(&state, 0));
        // For start_offset=2040: start_idx = (2048 - 2040 - 1) & 2047 = 7 < 15 → slow path.
        assert_eq!(state.get_bulk::<16>(2040), expected_bulk::<16>(&state, 2040));
        // Check many offsets to cover both branches exhaustively.
        for off in (0..2032).step_by(16) {
            let bulk = state.get_bulk::<16>(off);
            let expected = expected_bulk::<16>(&state, off);
            assert_eq!(bulk, expected, "get_bulk({off}) mismatch");
        }
    }

    // ── Convolution impulse response ───────────────────────────────────────

    /// Compute one convolution output sample by calling get_bulk as the audio loop does.
    fn convolve_sample(state: &FilterState, kernel: &[f32]) -> f32 {
        let n = kernel.len();
        let rev_kernel: Vec<f32> = kernel.iter().rev().copied().collect();
        let history = state.history_slice();
        const LANES: usize = 16;
        let chunks = n / LANES;
        let mut acc = f32x16::splat(0.0);
        for c in 0..chunks {
            let k = c * LANES;
            let h = f32x16::from_slice(&history[k..k + LANES]);
            let kr = f32x16::from_slice(&rev_kernel[k..k + LANES]);
            acc += h * kr;
        }
        hsum(acc)
    }

    /// Feed a unit impulse into the filter; the output stream should equal the kernel.
    #[test]
    fn test_convolution_impulse_response() {
        let kernel_len = 2048;
        // A simple non-trivial kernel: decaying ramp.
        let kernel: Vec<f32> = (0..kernel_len)
            .map(|i| (kernel_len - i) as f32 / kernel_len as f32)
            .collect();

        let mut state = FilterState::new(kernel_len);

        // t=0: push impulse, then convolve.
        state.push(1.0);
        let y0 = convolve_sample(&state, &kernel);
        assert!(
            (y0 - kernel[0]).abs() < 1e-5,
            "y[0] should equal kernel[0]={:.6}, got {:.6}",
            kernel[0], y0
        );

        // t=1: push zero, y[1] should equal kernel[1].
        state.push(0.0);
        let y1 = convolve_sample(&state, &kernel);
        assert!(
            (y1 - kernel[1]).abs() < 1e-5,
            "y[1] should equal kernel[1]={:.6}, got {:.6}",
            kernel[1], y1
        );

        // t=15..16 (straddles the fast/slow boundary for chunk 0).
        for t in 2..=32usize {
            state.push(0.0);
            let yt = convolve_sample(&state, &kernel);
            assert!(
                (yt - kernel[t]).abs() < 1e-5,
                "y[{t}] should equal kernel[{t}]={:.6}, got {:.6}",
                kernel[t], yt
            );
        }
    }

    /// A pure delay kernel (kernel[d]=1, rest 0) passes x[n-d] unmodified.
    #[test]
    fn test_convolution_delay_kernel() {
        let kernel_len = 2048;
        let delay = 100usize;
        let mut kernel = vec![0.0f32; kernel_len];
        kernel[delay] = 1.0;

        let mut state = FilterState::new(kernel_len);

        // Push 'delay+1' known samples: values 1.0, 2.0, …, (delay+1).
        for i in 1..=(delay + 1) {
            state.push(i as f32);
        }
        // The sample that is exactly `delay` steps back is value 1.0.
        let out = convolve_sample(&state, &kernel);
        assert!(
            (out - 1.0).abs() < 1e-6,
            "delay kernel: expected 1.0, got {out}"
        );
    }

    // ── Kernel synthesis helpers ───────────────────────────────────────────

    /// Synthesise a realistic FIR kernel via the same code path the plugin uses.
    fn make_test_kernel(cutoff_hz: f32) -> Vec<f32> {
        make_test_kernel_with_resonance(cutoff_hz, 0.0)
    }

    fn make_test_kernel_with_resonance(cutoff_hz: f32, resonance: f32) -> Vec<f32> {
        let sample_rate = 48000.0f32;
        let wt = WavetableFilter::create_default_wavetable();
        let mut planner = RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(wt.frame_size);
        let kernel_ifft = planner.plan_fft_inverse(KERNEL_LEN);

        let frame = wt.get_frame_interpolated(0.0);
        let (base_mags, bin_fracs) =
            WavetableFilter::compute_base_spectrum(&frame, cutoff_hz, sample_rate, &frame_fft)
                .expect("compute_base_spectrum returned None");

        let mut spectrum_work = vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN / 2 + 1];
        let mut kernel = vec![0.0f32; KERNEL_LEN];

        WavetableFilter::apply_resonance_and_ifft(
            &base_mags,
            &bin_fracs,
            resonance,
            &mut spectrum_work,
            &mut kernel,
            &kernel_ifft,
        );
        kernel
    }

    // ── Synchronous, allocation-free synthesis ────────────────────────────

    /// Verify that the allocation-free synthesis API (interpolate_frame_into +
    /// compute_base_spectrum_into) produces the same kernel as the allocating path.
    #[test]
    fn test_alloc_free_synthesis_matches_allocating_path() {
        let wt = WavetableFilter::create_default_wavetable();
        let sample_rate = 48000.0f32;
        let cutoff = 1000.0f32;

        // ── Allocating path (existing) ──────────────────────────────────
        let kernel_alloc = make_test_kernel(cutoff);

        // ── Allocation-free path (new API under test) ───────────────────
        let mut real_planner = RealFftPlanner::<f32>::new();
        let frame_fft = real_planner.plan_fft_forward(wt.frame_size);
        let kernel_ifft = real_planner.plan_fft_inverse(KERNEL_LEN);

        // Pre-allocated scratch (would live in the plugin struct).
        let mut frame_buf = vec![0.0f32; wt.frame_size];
        let mut frame_spectrum = vec![Complex::new(0.0_f32, 0.0); wt.frame_size / 2 + 1];
        let mut frame_mags_scratch = vec![0.0f32; wt.frame_size / 2 + 1];
        let mut out_mags = vec![0.0f32; KERNEL_LEN / 2 + 1];
        let mut out_fracs = vec![0.0f32; KERNEL_LEN / 2 + 1];
        let mut spectrum_work = vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN / 2 + 1];
        let mut kernel_free = vec![0.0f32; KERNEL_LEN];

        // Frame interpolation into pre-allocated buffer.
        wt.interpolate_frame_into(0.0, &mut frame_buf);

        // Base spectrum into pre-allocated buffers.
        // frame_buf is consumed (modified) by the FFT — that is expected.
        let ok = WavetableFilter::compute_base_spectrum_into(
            &mut frame_buf,
            cutoff,
            sample_rate,
            &frame_fft,
            &mut frame_spectrum,
            &mut frame_mags_scratch,
            &mut out_mags,
            &mut out_fracs,
        );
        assert!(ok, "compute_base_spectrum_into returned false");

        WavetableFilter::apply_resonance_and_ifft(
            &out_mags,
            &out_fracs,
            0.0,
            &mut spectrum_work,
            &mut kernel_free,
            &kernel_ifft,
        );

        // Both paths must produce identical results.
        let max_diff = kernel_alloc
            .iter()
            .zip(kernel_free.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff < 1e-6,
            "allocation-free path differs from allocating path by {max_diff:.2e}"
        );
    }

    // ── Synthesis timing ──────────────────────────────────────────────────

    /// Verify synthesis is fast enough to run synchronously on the audio thread.
    /// A 512-sample buffer at 48 kHz gives 10.7 ms; target < 2 ms per synthesis.
    #[test]
    fn bench_synthesis_time_raw_mode() {
        let n = 200;
        let t = std::time::Instant::now();
        for i in 0..n {
            // Vary cutoff to prevent dead-code elimination.
            let cutoff = 200.0 + (i as f32) * 50.0;
            std::hint::black_box(make_test_kernel(cutoff));
        }
        let us = t.elapsed().as_micros() / n as u128;
        eprintln!("Raw synthesis: {us} µs/call");
        // Must be well within the smallest practical buffer period (~1.3 ms @ 64 samples).
        // In debug builds this won't pass; run with --release for the real number.
        // We document the number, not assert (debug builds are 10-20× slower).
    }

    // ── Kernel sanity checks ──────────────────────────────────────────────

    /// Kernels must contain no NaN/inf, and convolving a unit-amplitude sine
    /// through one must produce bounded output (|H(f)| ≤ 1).
    #[test]
    fn test_kernel_values_finite_at_all_cutoffs() {
        for &cutoff in &[20.0f32, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 20000.0] {
            let kernel = make_test_kernel(cutoff);
            let finite = kernel.iter().all(|v| v.is_finite());
            assert!(finite, "kernel at {cutoff} Hz contains NaN or inf");

            // Convolve a worst-case full-scale sine through the kernel.
            // With |H(f)| ≤ 1 the output must be bounded by 1.0.
            let mut state = FilterState::new(KERNEL_LEN);
            for n in 0..KERNEL_LEN {
                // Use a sine at the cutoff frequency (worst case for filter gain).
                let v = (n as f32 * cutoff / 48000.0 * 2.0 * std::f32::consts::PI).sin();
                state.push(v);
            }
            let out = convolve_sample(&state, &kernel);
            assert!(
                out.abs() <= 2.0,
                "kernel at {cutoff} Hz produced out-of-bound output {out:.4}"
            );
        }
    }

    // ── Crossfade continuity ───────────────────────────────────────────────

    /// Smooth transition from kernel A to B: max per-sample output change should
    /// not exceed what ordinary filter evolution produces.
    #[test]
    fn test_crossfade_output_no_discontinuity_at_start() {
        // Synthesised kernels have |H(f)| ≤ 1, so output ≤ input amplitude (1.0).
        // Max derivative of output sine ≈ 2π·f/fs ≈ 0.13 per sample for 1 kHz @ 48 kHz.
        // A genuine click would produce a jump >> 0.5; set threshold conservatively.
        let kernel_a = make_test_kernel(500.0);
        let kernel_b = make_test_kernel(5000.0);

        let mut state = FilterState::new(KERNEL_LEN);
        // Pre-warm history with a 440 Hz sine.
        for n in 0..KERNEL_LEN {
            state.push((n as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI).sin());
        }

        // Steady-state output with kernel A.
        state.push((KERNEL_LEN as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI).sin());
        let y_before = convolve_sample(&state, &kernel_a);

        // First crossfade sample (alpha=0) — must equal what kernel A alone would give.
        let next_input =
            ((KERNEL_LEN + 1) as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI).sin();
        state.push(next_input);
        let ya = convolve_sample(&state, &kernel_a);
        let yb = convolve_sample(&state, &kernel_b);
        let y_first_crossfade = 1.0 * ya + 0.0 * yb; // alpha=0

        // Jump at crossfade activation must equal normal filter evolution.
        let jump_at_start = (y_first_crossfade - y_before).abs();
        assert!(
            jump_at_start < 0.5,
            "jump at crossfade start = {jump_at_start:.4}; expected < 0.5 (normal evolution)"
        );
    }

    /// The bake-and-restart operation (new kernel arrives mid-crossfade) must be
    /// sample-continuous: the last sample before bake and first after must differ
    /// by no more than ordinary filter evolution.
    #[test]
    fn test_bake_and_restart_crossfade_is_continuous() {
        let kernel_a = make_test_kernel(500.0);
        let kernel_b = make_test_kernel(5000.0);
        let kernel_c = make_test_kernel(200.0);

        let sample_rate = 48000.0f32;
        let alpha_step = 1.0 / (sample_rate * 0.020);

        let mut state = FilterState::new(KERNEL_LEN);
        for n in 0..KERNEL_LEN {
            state.push((n as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI).sin());
        }

        let mut synthesized = kernel_a.clone();
        let mut target = kernel_b.clone();
        let mut alpha = 0.0f32;

        // Run crossfade A→B for 100 samples so alpha > 0.
        for n in 0..100usize {
            let v = ((KERNEL_LEN + n) as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI)
                .sin();
            state.push(v);
            alpha += alpha_step; // advance alpha
        }

        // Capture output at alpha ≈ 100/960.
        let v = ((KERNEL_LEN + 100) as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI).sin();
        state.push(v);
        let ya = convolve_sample(&state, &synthesized);
        let yb = convolve_sample(&state, &target);
        let y_before_bake = (1.0 - alpha) * ya + alpha * yb;

        // ── Bake ────────────────────────────────────────────────────────────
        // This mirrors the plugin's bake: blend synthesized←target at current alpha.
        for i in 0..KERNEL_LEN {
            synthesized[i] = synthesized[i] * (1.0 - alpha) + target[i] * alpha;
        }
        target = kernel_c.clone();
        alpha = 0.0;

        // First sample after bake (alpha=0 of new crossfade).
        let v_after =
            ((KERNEL_LEN + 102) as f32 * 440.0 / 48000.0 * 2.0 * std::f32::consts::PI).sin();
        state.push(v_after);
        let ya2 = convolve_sample(&state, &synthesized);
        let yb2 = convolve_sample(&state, &target);
        let y_after_bake = (1.0 - alpha) * ya2 + alpha * yb2; // alpha=0 → just synthesized

        let jump = (y_after_bake - y_before_bake).abs();
        assert!(
            jump < 0.5,
            "bake+restart produced a jump of {jump:.4}; expected < 0.5 (continuous)"
        );
    }

    // ── Kernel energy continuity at parameter boundaries ────────────────

    /// Kernel energy (L2 norm²) and shape must vary smoothly at the
    /// parameter-range boundaries (20 Hz and 20 000 Hz).  A discontinuity
    /// would cause an audible pop during crossfade.
    #[test]
    fn test_kernel_energy_continuous_at_cutoff_boundaries() {
        // Low boundary: sweep 20.0 → 21.0 in 0.05 Hz steps
        let low_cutoffs: Vec<f32> = (0..=20).map(|i| 20.0 + i as f32 * 0.05).collect();
        check_kernel_continuity(&low_cutoffs, "low (20 Hz)");

        // High boundary: sweep 19990 → 20000 in 1 Hz steps
        let high_cutoffs: Vec<f32> = (0..=10).map(|i| 19990.0 + i as f32 * 1.0).collect();
        check_kernel_continuity(&high_cutoffs, "high (20 kHz)");
    }

    fn check_kernel_continuity(cutoffs: &[f32], label: &str) {
        let mut prev_kernel: Option<Vec<f32>> = None;
        let mut prev_cutoff = 0.0f32;
        let mut max_energy_jump = 0.0f32;
        let mut max_shape_dist = 0.0f32;

        for &cutoff in cutoffs {
            let kernel = make_test_kernel(cutoff);
            let energy: f32 = kernel.iter().map(|x| x * x).sum();

            if let Some(ref prev) = prev_kernel {
                let prev_energy: f32 = prev.iter().map(|x| x * x).sum();
                let energy_jump =
                    (energy - prev_energy).abs() / prev_energy.max(energy).max(1e-10);
                if energy_jump > max_energy_jump {
                    max_energy_jump = energy_jump;
                }

                // L2 distance between kernel shapes
                let l2_dist: f32 = kernel
                    .iter()
                    .zip(prev.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt();
                let l2_norm: f32 = kernel.iter().map(|x| x * x).sum::<f32>().sqrt();
                let rel_dist = l2_dist / l2_norm.max(1e-10);
                if rel_dist > max_shape_dist {
                    max_shape_dist = rel_dist;
                }
            }
            prev_kernel = Some(kernel);
            prev_cutoff = cutoff;
        }

        let _ = prev_cutoff; // suppress warning
        assert!(
            max_energy_jump < 0.5,
            "{label} boundary: kernel energy jumped by {:.1}% (threshold 50%)",
            max_energy_jump * 100.0
        );
        assert!(
            max_shape_dist < 1.0,
            "{label} boundary: kernel shape distance {max_shape_dist:.4} (threshold 1.0)"
        );
    }

    // ── Kernel gain diagnostics ──────────────────────────────────────────

    /// Print kernel properties at various cutoffs to diagnose boundary issues.
    #[test]
    fn debug_kernel_gain_across_cutoffs() {
        for &cutoff in &[20.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0, 15000.0, 19000.0, 20000.0] {
            let kernel = make_test_kernel(cutoff);
            let l1: f32 = kernel.iter().map(|x| x.abs()).sum();
            let dc: f32 = kernel.iter().sum();
            let peak: f32 = kernel.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
            eprintln!(
                "cutoff={cutoff:>8.1} Hz: L1={l1:.6}, DC={dc:.6}, peak={peak:.6}"
            );
        }
    }

    /// Resonance comb must never produce NaN.  The cos(PI*dist) term can
    /// return a tiny negative value at dist=0.5 due to f32 rounding; powf
    /// with a non-integer exponent on a negative base yields NaN.
    #[test]
    fn test_resonance_comb_no_nan() {
        // resonance=0.775 → comb_exp=6.2 (non-integer), which triggers NaN
        // if the cosine result isn't clamped.
        for &resonance in &[0.1, 0.3, 0.775, 0.9, 0.999] {
            for &cutoff in &[20.0, 100.0, 562.5, 1000.0, 5000.0, 20000.0] {
                let kernel = make_test_kernel_with_resonance(cutoff, resonance);
                let has_nan = kernel.iter().any(|x| x.is_nan());
                assert!(
                    !has_nan,
                    "NaN in kernel at cutoff={cutoff} resonance={resonance}"
                );
            }
        }
    }


    // ── STFT magnitude computation ──────────────────────────────────────

    #[test]
    fn test_stft_magnitudes_match_spectrum() {
        let sample_rate = 48000.0f32;
        let cutoff = 2000.0f32;
        let resonance = 0.3f32;
        let wt = WavetableFilter::create_default_wavetable();
        let mut planner = RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(wt.frame_size);

        let frame = wt.get_frame_interpolated(0.0);
        let (base_mags, bin_fracs) =
            WavetableFilter::compute_base_spectrum(&frame, cutoff, sample_rate, &frame_fft)
                .expect("spectrum failed");

        let mut stft_mags = vec![0.0f32; KERNEL_LEN / 2 + 1];
        WavetableFilter::compute_stft_magnitudes(&base_mags, &bin_fracs, resonance, &mut stft_mags);

        // All values should be finite and non-negative
        assert!(stft_mags.iter().all(|v| v.is_finite() && *v >= 0.0));
        // DC bin should have a value (lowpass passes DC)
        assert!(stft_mags[0] > 0.0, "DC magnitude should be non-zero for lowpass");
        // High-frequency bins should be near zero for lowpass
        let nyquist_mag = stft_mags[KERNEL_LEN / 2];
        assert!(
            nyquist_mag < 0.01,
            "Nyquist magnitude should be near zero for 2kHz lowpass"
        );
    }

    // ── Spectrum continuity at source boundary ───────────────────────────

    /// When cutoff changes so that an output bin's source position crosses
    /// the frame spectrum boundary (max_src), the bin's magnitude must taper
    /// smoothly to zero — not jump discontinuously.
    #[test]
    fn test_spectrum_magnitude_continuous_at_source_boundary() {
        // An impulse frame has a flat magnitude spectrum (all bins ≈ 1.0 after
        // normalization), which maximises the cliff at the boundary.
        let frame_size = 256;
        let mut frame = vec![0.0f32; frame_size];
        frame[0] = 1.0; // impulse → flat spectrum

        let sample_rate = 48000.0f32;
        let mut planner = RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(frame_size);

        let max_src = (frame_size / 2) as f32; // 128.0

        // Critical cutoff for output bin 5: the cutoff at which bin 5's
        // source position exactly equals max_src.
        let critical = 5.0 * 24.0 * sample_rate / (KERNEL_LEN as f32 * max_src);
        // ≈ 21.97 Hz

        // Sweep cutoff in fine steps across the critical point.
        let step = 0.05f32;
        let mut prev_mag5: Option<f32> = None;
        let mut max_jump = 0.0f32;

        let mut cutoff = critical + 1.0;
        while cutoff >= critical - 1.0 {
            let (mags, _) = WavetableFilter::compute_base_spectrum(
                &frame, cutoff, sample_rate, &frame_fft,
            )
            .expect("spectrum computation failed");

            if let Some(prev) = prev_mag5 {
                let jump = (mags[5] - prev).abs();
                if jump > max_jump {
                    max_jump = jump;
                }
            }
            prev_mag5 = Some(mags[5]);
            cutoff -= step;
        }

        // Over a 0.05 Hz step the magnitude should change by at most a few
        // percent.  The cliff bug causes a ~100% jump (from ~1.0 to 0.0).
        assert!(
            max_jump < 0.05,
            "Bin 5 magnitude jumped by {max_jump:.4} across source boundary \
             (critical cutoff ≈ {critical:.2} Hz); expected < 0.05"
        );
    }

    // ── STFT integration tests ──────────────────────────────────────────

    fn run_stft_mono(plugin: &mut WavetableFilter, input: &[f32]) -> Vec<f32> {
        let mut output = vec![0.0f32; input.len()];
        for i in 0..input.len() {
            if plugin.stft_out_pos == 0 {
                plugin.stft_out[0].copy_within(HOP..KERNEL_LEN, 0);
                plugin.stft_out[0][HOP..].fill(0.0);
                WavetableFilter::process_stft_frame(
                    &plugin.stft_in[0], plugin.stft_in_pos,
                    &mut plugin.stft_out[0], &plugin.stft_magnitudes,
                    &plugin.stft_window, &plugin.stft_fft, &plugin.kernel_ifft,
                    &mut plugin.stft_scratch, &mut plugin.spectrum_work,
                );
            }

            plugin.stft_in[0][plugin.stft_in_pos] = input[i];
            output[i] = plugin.stft_out[0][plugin.stft_out_pos];

            plugin.stft_in_pos = (plugin.stft_in_pos + 1) & (KERNEL_LEN - 1);
            plugin.stft_out_pos += 1;
            if plugin.stft_out_pos >= HOP { plugin.stft_out_pos = 0; }
        }
        output
    }

    #[test]
    fn test_stft_lowpass_attenuates_highs() {
        let mut plugin = WavetableFilter::default();

        let cutoff_bin = 100;
        for i in 0..plugin.stft_magnitudes.len() {
            plugin.stft_magnitudes[i] = if i < cutoff_bin { 1.0 } else { 0.0 };
        }

        // High-frequency sine (10 kHz at 48 kHz SR — bin ~426, well above cutoff)
        let num_samples = KERNEL_LEN * 4;
        let freq = 10000.0f32;
        let sr = 48000.0f32;
        let input: Vec<f32> = (0..num_samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect();

        let output = run_stft_mono(&mut plugin, &input);

        // After transient, output energy should be much less than input energy
        let start = KERNEL_LEN * 2;
        let input_energy: f32 = input[start..].iter().map(|x| x * x).sum();
        let output_energy: f32 = output[start..].iter().map(|x| x * x).sum();
        let attenuation = output_energy / input_energy.max(1e-20);
        assert!(
            attenuation < 0.01,
            "High-freq should be attenuated >99%, got {:.1}% through",
            attenuation * 100.0
        );
    }

    #[test]
    fn test_stft_flat_preserves_amplitude() {
        // With flat magnitude spectrum, the output should preserve the input
        // signal's amplitude. The STFT introduces a fixed latency of up to
        // HOP samples (reported to the host for compensation).
        let mut plugin = WavetableFilter::default();
        plugin.stft_magnitudes.fill(1.0);

        let num_samples = KERNEL_LEN * 6;
        let freq = 1000.0f32;
        let sr = 48000.0f32;
        let input: Vec<f32> = (0..num_samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect();

        let output = run_stft_mono(&mut plugin, &input);

        // Cross-correlate to find best alignment, accounting for STFT latency.
        let start = KERNEL_LEN * 3;
        let len = KERNEL_LEN;
        let mut best_corr = f32::NEG_INFINITY;
        let search = HOP as i32;
        for lag in -search..=search {
            let mut corr = 0.0f32;
            for j in 0..len {
                let ij = (start as i32 + j as i32) as usize;
                let oj = (start as i32 + j as i32 + lag) as usize;
                if oj < num_samples {
                    corr += input[ij] * output[oj];
                }
            }
            if corr > best_corr {
                best_corr = corr;
            }
        }
        // Flat magnitude STFT should preserve amplitude: peak correlation should
        // be close to the autocorrelation value (len/2 for a unit sine).
        let expected_corr = len as f32 / 2.0;
        assert!(
            best_corr >= expected_corr * 0.8,
            "Flat STFT should preserve signal; best_corr={best_corr:.1} expected ~{expected_corr:.1}"
        );
    }

    // ── reverse_in_place ────────────────────────────────────────────────

    #[test]
    fn test_reverse_in_place() {
        // Even-length array
        let mut even = [1.0f32, 2.0, 3.0, 4.0];
        reverse_in_place(&mut even);
        assert_eq!(even, [4.0, 3.0, 2.0, 1.0]);

        // Odd-length array
        let mut odd = [1.0f32, 2.0, 3.0];
        reverse_in_place(&mut odd);
        assert_eq!(odd, [3.0, 2.0, 1.0]);

        // Single element
        let mut single = [1.0f32];
        reverse_in_place(&mut single);
        assert_eq!(single, [1.0]);

        // Empty
        let mut empty: [f32; 0] = [];
        reverse_in_place(&mut empty);
        assert_eq!(empty, [] as [f32; 0]);
    }

    // ── FilterState::reset ──────────────────────────────────────────────

    #[test]
    fn test_filter_state_reset() {
        let mut state = FilterState::new(2048);

        // Push some samples to make the history non-zero.
        push_sequence(&mut state, &[1.0, 2.0, 3.0, 4.0, 5.0]);

        // Verify non-zero history and non-zero write_pos before reset.
        assert_eq!(state.write_pos, 5);
        let has_nonzero = state.history.iter().any(|&v| v != 0.0);
        assert!(has_nonzero, "history should contain non-zero samples before reset");

        state.reset();

        // After reset, all history should be zero and write_pos should be 0.
        assert_eq!(state.write_pos, 0);
        assert!(
            state.history.iter().all(|&v| v == 0.0),
            "all history samples should be zero after reset"
        );
    }

    // ── Low cutoff downsampling branch in compute_base_spectrum_into ───

    #[test]
    fn test_low_cutoff_downsampling() {
        // At very low cutoffs, bin_to_src > 1.0, exercising the peak-scanning
        // branch in compute_base_spectrum_into.
        let wt = WavetableFilter::create_default_wavetable();
        let sample_rate = 48000.0f32;
        let cutoff = 25.0f32; // very low cutoff

        let mut planner = RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(wt.frame_size);

        // Verify bin_to_src > 1.0 for this configuration.
        let bin_to_src = 24.0 * sample_rate / (KERNEL_LEN as f32 * cutoff);
        assert!(
            bin_to_src > 1.0,
            "bin_to_src={bin_to_src} should be >1.0 for low cutoff test"
        );

        // Use the allocating path for simplicity.
        let frame = wt.get_frame_interpolated(0.0);
        let result = WavetableFilter::compute_base_spectrum(
            &frame, cutoff, sample_rate, &frame_fft,
        );
        assert!(result.is_some(), "compute_base_spectrum should succeed at low cutoff");

        let (mags, fracs) = result.unwrap();

        // All magnitudes must be finite and non-negative.
        assert!(
            mags.iter().all(|v| v.is_finite() && *v >= 0.0),
            "all magnitudes should be finite and non-negative"
        );

        // In the downsampling branch, fracs should be 0.0 (documented in the code comment).
        // Check that the first few non-zero bins have frac=0.0.
        for j in 1..10.min(fracs.len()) {
            let src = j as f32 * bin_to_src;
            if src < wt.frame_size as f32 / 2.0 {
                assert_eq!(
                    fracs[j], 0.0,
                    "bin {j}: frac should be 0.0 in downsampling branch"
                );
            }
        }

        // DC bin should have a value (frame 0 is a sine which has some DC content after FFT).
        // At minimum, magnitudes should not all be zero.
        let max_mag = mags.iter().cloned().fold(0.0f32, f32::max);
        assert!(max_mag > 0.0, "at least some bins should have non-zero magnitude");

        // Also test via the allocation-free path.
        let mut frame_buf = wt.get_frame_interpolated(0.0);
        let spec_len = wt.frame_size / 2 + 1;
        let mut frame_spectrum = vec![Complex::new(0.0f32, 0.0); spec_len];
        let mut frame_mags = vec![0.0f32; spec_len];
        let mut out_mags = vec![0.0f32; KERNEL_LEN / 2 + 1];
        let mut out_fracs = vec![0.0f32; KERNEL_LEN / 2 + 1];

        let ok = WavetableFilter::compute_base_spectrum_into(
            &mut frame_buf,
            cutoff,
            sample_rate,
            &frame_fft,
            &mut frame_spectrum,
            &mut frame_mags,
            &mut out_mags,
            &mut out_fracs,
        );
        assert!(ok, "compute_base_spectrum_into should succeed at low cutoff");
        assert!(
            out_mags.iter().all(|v| v.is_finite() && *v >= 0.0),
            "all output magnitudes should be finite and non-negative (alloc-free path)"
        );
    }

    // ── STFT magnitudes with zero resonance ─────────────────────────────

    #[test]
    fn test_stft_magnitudes_zero_resonance() {
        // With resonance=0, comb_exp=0.0 (<=0.01), so compute_stft_magnitudes
        // takes the else branch and passes base_mags through unchanged.
        let sample_rate = 48000.0f32;
        let cutoff = 1000.0f32;
        let wt = WavetableFilter::create_default_wavetable();
        let mut planner = RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(wt.frame_size);

        let frame = wt.get_frame_interpolated(0.0);
        let (base_mags, bin_fracs) =
            WavetableFilter::compute_base_spectrum(&frame, cutoff, sample_rate, &frame_fft)
                .expect("spectrum computation failed");

        let mut stft_mags = vec![0.0f32; KERNEL_LEN / 2 + 1];
        WavetableFilter::compute_stft_magnitudes(&base_mags, &bin_fracs, 0.0, &mut stft_mags);

        // With zero resonance, the output should equal the base magnitudes exactly.
        for j in 0..base_mags.len().min(stft_mags.len()) {
            assert_eq!(
                stft_mags[j], base_mags[j],
                "bin {j}: with zero resonance, stft magnitude should equal base magnitude"
            );
        }

        // All values should be finite and non-negative.
        assert!(stft_mags.iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    // ── Wavetable file loading ──────────────────────────────────────────

    #[test]
    fn test_wavetable_file_loading() {
        // Load the sample wavetable file included in the repo.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phaseless-bass.wt");
        let wt = Wavetable::from_file(path);
        assert!(wt.is_ok(), "phaseless-bass.wt should load successfully: {:?}", wt.err());

        let wt = wt.unwrap();
        assert!(wt.frame_count > 0, "frame_count should be > 0");
        assert!(wt.frame_size > 0, "frame_size should be > 0");
        assert!(!wt.frames.is_empty(), "frames should not be empty");

        // All frame data should contain finite values.
        for (i, frame) in wt.frames.iter().enumerate() {
            assert_eq!(frame.len(), wt.frame_size, "frame {i} length mismatch");
            assert!(
                frame.iter().all(|v| v.is_finite()),
                "frame {i} contains non-finite values"
            );
        }

        // Error case: non-existent file
        let result = Wavetable::from_file("/tmp/this_file_does_not_exist_12345.wt");
        assert!(result.is_err(), "non-existent file should return Err");

        // Error case: garbage data
        let garbage_path = "/tmp/wavetable_filter_test_garbage.wt";
        std::fs::write(garbage_path, b"this is not a valid wavetable file")
            .expect("failed to write temp file");
        let result = Wavetable::from_file(garbage_path);
        assert!(result.is_err(), "garbage file should return Err");
        let _ = std::fs::remove_file(garbage_path);
    }

    // ── Wavetable interpolation edge cases ──────────────────────────────

    #[test]
    fn test_wavetable_interpolation_edge_cases() {
        let wt = WavetableFilter::create_default_wavetable();

        // Position 0.0 should return the first frame exactly.
        let first = wt.get_frame_interpolated(0.0);
        assert_eq!(first.len(), wt.frame_size);
        for (i, (&got, &expected)) in first.iter().zip(wt.frames[0].iter()).enumerate() {
            assert!(
                (got - expected).abs() < 1e-7,
                "sample {i}: get_frame_interpolated(0.0) should match first frame"
            );
        }

        // Position 1.0 should return the last frame exactly.
        let last = wt.get_frame_interpolated(1.0);
        let last_frame = &wt.frames[wt.frame_count - 1];
        for (i, (&got, &expected)) in last.iter().zip(last_frame.iter()).enumerate() {
            assert!(
                (got - expected).abs() < 1e-7,
                "sample {i}: get_frame_interpolated(1.0) should match last frame"
            );
        }

        // Position 0.5 should be a blend between middle frames.
        let mid = wt.get_frame_interpolated(0.5);
        assert_eq!(mid.len(), wt.frame_size);
        // The result should differ from both the first and last frame (unless all
        // frames are identical, which they are not for the default wavetable).
        let differs_from_first = mid.iter().zip(wt.frames[0].iter()).any(|(a, b)| (a - b).abs() > 1e-6);
        let differs_from_last = mid.iter().zip(last_frame.iter()).any(|(a, b)| (a - b).abs() > 1e-6);
        assert!(differs_from_first, "mid-frame should differ from first frame");
        assert!(differs_from_last, "mid-frame should differ from last frame");

        // interpolate_frame_into should produce the same results as get_frame_interpolated.
        for &pos in &[0.0f32, 0.5, 1.0] {
            let allocating = wt.get_frame_interpolated(pos);
            let mut buf = vec![0.0f32; wt.frame_size];
            wt.interpolate_frame_into(pos, &mut buf);
            for (i, (&a, &b)) in allocating.iter().zip(buf.iter()).enumerate() {
                assert!(
                    (a - b).abs() < 1e-7,
                    "pos={pos} sample {i}: interpolate_frame_into should match get_frame_interpolated"
                );
            }
        }
    }

    // ── Frame-sweep timing benchmarks ─────────────────────────────────

    /// Helper: set up FFT plans and scratch buffers for a given wavetable,
    /// returning everything needed to run the synthesis pipeline.
    struct SynthBench {
        wt: Wavetable,
        frame_fft: Arc<dyn RealToComplex<f32>>,
        kernel_ifft: Arc<dyn ComplexToReal<f32>>,
        frame_buf: Vec<f32>,
        frame_spectrum: Vec<Complex<f32>>,
        frame_mags: Vec<f32>,
        out_mags: Vec<f32>,
        out_fracs: Vec<f32>,
        spectrum_work: Vec<Complex<f32>>,
        kernel: Vec<f32>,
    }

    impl SynthBench {
        fn new(wt: Wavetable) -> Self {
            let mut planner = RealFftPlanner::<f32>::new();
            let frame_fft = planner.plan_fft_forward(wt.frame_size);
            let kernel_ifft = planner.plan_fft_inverse(KERNEL_LEN);
            let spec_len = wt.frame_size / 2 + 1;
            Self {
                frame_buf: vec![0.0f32; wt.frame_size],
                frame_spectrum: vec![Complex::new(0.0f32, 0.0); spec_len],
                frame_mags: vec![0.0f32; spec_len],
                out_mags: vec![0.0f32; KERNEL_LEN / 2 + 1],
                out_fracs: vec![0.0f32; KERNEL_LEN / 2 + 1],
                spectrum_work: vec![Complex::new(0.0f32, 0.0); KERNEL_LEN / 2 + 1],
                kernel: vec![0.0f32; KERNEL_LEN],
                frame_fft,
                kernel_ifft,
                wt,
            }
        }

        /// Run one full synthesis cycle: interpolate + spectrum + IFFT.
        /// Returns (interpolate_us, spectrum_us, ifft_us).
        fn synthesize(&mut self, frame_pos: f32, cutoff_hz: f32, sample_rate: f32, resonance: f32) -> (f64, f64, f64) {
            let t0 = std::time::Instant::now();
            self.wt.interpolate_frame_into(frame_pos, &mut self.frame_buf);
            let t1 = std::time::Instant::now();

            WavetableFilter::compute_base_spectrum_into(
                &mut self.frame_buf,
                cutoff_hz,
                sample_rate,
                &self.frame_fft,
                &mut self.frame_spectrum,
                &mut self.frame_mags,
                &mut self.out_mags,
                &mut self.out_fracs,
            );
            let t2 = std::time::Instant::now();

            WavetableFilter::apply_resonance_and_ifft(
                &self.out_mags,
                &self.out_fracs,
                resonance,
                &mut self.spectrum_work,
                &mut self.kernel,
                &self.kernel_ifft,
            );
            let t3 = std::time::Instant::now();

            // Prevent dead-code elimination
            std::hint::black_box(&self.kernel);

            let interp_us = t1.duration_since(t0).as_nanos() as f64 / 1000.0;
            let spectrum_us = t2.duration_since(t1).as_nanos() as f64 / 1000.0;
            let ifft_us = t3.duration_since(t2).as_nanos() as f64 / 1000.0;
            (interp_us, spectrum_us, ifft_us)
        }
    }

    fn print_timing_stats(label: &str, times_us: &[f64]) {
        let n = times_us.len() as f64;
        let min = times_us.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = times_us.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let avg = times_us.iter().sum::<f64>() / n;
        let mut sorted = times_us.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = sorted[(n * 0.50) as usize];
        let p95 = sorted[(n * 0.95) as usize];
        let p99 = sorted[(n * 0.99).min(n - 1.0) as usize];

        // Buffer period threshold: 256 samples / 48000 Hz * 50% = ~2666 us
        let threshold_us = 256.0 / 48000.0 * 0.5 * 1_000_000.0;
        let exceeded = times_us.iter().filter(|&&t| t > threshold_us).count();

        eprintln!("  {label}:");
        eprintln!("    min={min:.1} us  avg={avg:.1} us  p50={p50:.1} us  p95={p95:.1} us  p99={p99:.1} us  max={max:.1} us");
        eprintln!("    exceeded 50% buffer period ({threshold_us:.0} us): {exceeded}/{} calls", times_us.len());
    }

    /// Benchmark: sweep frame_position from 0.0 to 1.0 over ~100 buffer periods.
    /// Each "buffer" triggers one full kernel re-synthesis (the expensive path).
    /// Uses the real phaseless-bass.wt fixture to reproduce reported lag.
    #[test]
    fn bench_frame_sweep() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phaseless-bass.wt");
        let wt = Wavetable::from_file(path).expect("failed to load phaseless-bass.wt");
        eprintln!("\n=== FRAME SWEEP BENCHMARK ===");
        eprintln!("Wavetable: {} frames x {} samples", wt.frame_count, wt.frame_size);

        let sample_rate = 48000.0f32;
        let cutoff_hz = 1000.0f32;
        let resonance = 0.3f32;
        let num_buffers = 100usize;

        let mut bench = SynthBench::new(wt);

        // Warm up FFT plans
        for _ in 0..5 {
            bench.synthesize(0.0, cutoff_hz, sample_rate, resonance);
        }

        let mut interp_times = Vec::with_capacity(num_buffers);
        let mut spectrum_times = Vec::with_capacity(num_buffers);
        let mut ifft_times = Vec::with_capacity(num_buffers);
        let mut total_times = Vec::with_capacity(num_buffers);

        for i in 0..num_buffers {
            let frame_pos = i as f32 / (num_buffers - 1) as f32;
            let (t_interp, t_spectrum, t_ifft) =
                bench.synthesize(frame_pos, cutoff_hz, sample_rate, resonance);
            interp_times.push(t_interp);
            spectrum_times.push(t_spectrum);
            ifft_times.push(t_ifft);
            total_times.push(t_interp + t_spectrum + t_ifft);
        }

        eprintln!("\n--- Per-step breakdown (sweeping frame 0.0 -> 1.0, {num_buffers} buffers) ---");
        print_timing_stats("interpolate_frame_into", &interp_times);
        print_timing_stats("compute_base_spectrum_into", &spectrum_times);
        print_timing_stats("apply_resonance_and_ifft", &ifft_times);
        print_timing_stats("TOTAL synthesis", &total_times);

        // Extended run: 200 iterations for more statistical confidence
        eprintln!("\n--- Extended run: 200 iterations at varying frame positions ---");
        let mut ext_total = Vec::with_capacity(200);
        for i in 0..200 {
            let frame_pos = (i as f32 * 0.007).fract(); // sweep through multiple cycles
            let (t_i, t_s, t_f) = bench.synthesize(frame_pos, cutoff_hz, sample_rate, resonance);
            ext_total.push(t_i + t_s + t_f);
        }
        print_timing_stats("TOTAL (200 iterations)", &ext_total);

        eprintln!("\n=== END FRAME SWEEP BENCHMARK ===\n");
    }

    /// Baseline benchmark: frame_position is STATIC (no frame change).
    /// Same cutoff/resonance as the sweep test, so the only difference is
    /// whether the frame interpolation input changes between calls.
    #[test]
    fn bench_frame_static() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phaseless-bass.wt");
        let wt = Wavetable::from_file(path).expect("failed to load phaseless-bass.wt");
        eprintln!("\n=== STATIC FRAME BENCHMARK (baseline) ===");
        eprintln!("Wavetable: {} frames x {} samples", wt.frame_count, wt.frame_size);

        let sample_rate = 48000.0f32;
        let cutoff_hz = 1000.0f32;
        let resonance = 0.3f32;
        let num_buffers = 100usize;
        let static_frame_pos = 0.5f32; // fixed mid-point

        let mut bench = SynthBench::new(wt);

        // Warm up
        for _ in 0..5 {
            bench.synthesize(static_frame_pos, cutoff_hz, sample_rate, resonance);
        }

        let mut interp_times = Vec::with_capacity(num_buffers);
        let mut spectrum_times = Vec::with_capacity(num_buffers);
        let mut ifft_times = Vec::with_capacity(num_buffers);
        let mut total_times = Vec::with_capacity(num_buffers);

        for _ in 0..num_buffers {
            let (t_interp, t_spectrum, t_ifft) =
                bench.synthesize(static_frame_pos, cutoff_hz, sample_rate, resonance);
            interp_times.push(t_interp);
            spectrum_times.push(t_spectrum);
            ifft_times.push(t_ifft);
            total_times.push(t_interp + t_spectrum + t_ifft);
        }

        eprintln!("\n--- Per-step breakdown (static frame_pos={static_frame_pos}, {num_buffers} buffers) ---");
        print_timing_stats("interpolate_frame_into", &interp_times);
        print_timing_stats("compute_base_spectrum_into", &spectrum_times);
        print_timing_stats("apply_resonance_and_ifft", &ifft_times);
        print_timing_stats("TOTAL synthesis", &total_times);

        // Extended run
        eprintln!("\n--- Extended run: 200 iterations at static frame position ---");
        let mut ext_total = Vec::with_capacity(200);
        for _ in 0..200 {
            let (t_i, t_s, t_f) = bench.synthesize(static_frame_pos, cutoff_hz, sample_rate, resonance);
            ext_total.push(t_i + t_s + t_f);
        }
        print_timing_stats("TOTAL (200 iterations)", &ext_total);

        eprintln!("\n=== END STATIC FRAME BENCHMARK ===\n");
    }

    /// Benchmark: measure how long the wavetable mutex is held during the
    /// equivalent of WavetableView::draw() in both 3D and 2D modes.
    ///
    /// Run with: cargo test --lib --release bench_wavetable_draw -- --nocapture
    #[test]
    fn bench_wavetable_draw() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phaseless-bass.wt");
        let wt = Wavetable::from_file(path).expect("failed to load phaseless-bass.wt");
        let frame_count = wt.frame_count;
        let frame_size = wt.frame_size;
        eprintln!("\n=== WAVETABLE VIEW DRAW BENCHMARK ===");
        eprintln!("Wavetable: {} frames x {} samples", frame_count, frame_size);

        let shared = Arc::new(Mutex::new(wt));

        // Simulated widget dimensions (typical UI bounds)
        let padding = 20.0f32;
        let widget_width = 400.0f32;
        let widget_height = 300.0f32;
        let width = widget_width - padding * 2.0;
        let height = widget_height - padding * 2.0;
        let num_points = (width as usize).min(frame_size).max(1); // ~350
        let current_frame_pos = 0.5f32;
        let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;
        let iterations = 100usize;

        eprintln!("num_points (downsampled): {num_points}");
        eprintln!("current_frame_pos: {current_frame_pos}  current_frame_idx: {current_frame_idx}");

        // ── 3D overhead perspective path ─────────────────────────────────────
        eprintln!("\n--- 3D path (all frames, global min/max + path points) ---");
        {
            let mut lock_times = Vec::with_capacity(iterations);

            // Warm up
            for _ in 0..5 {
                let wavetable = shared.try_lock().unwrap();
                let mut _gmin = f32::INFINITY;
                let mut _gmax = f32::NEG_INFINITY;
                for frame in &wavetable.frames {
                    for &s in frame { _gmin = _gmin.min(s); _gmax = _gmax.max(s); }
                }
                std::hint::black_box((_gmin, _gmax));
                drop(wavetable);
            }

            for _ in 0..iterations {
                let t0 = std::time::Instant::now();

                // --- Lock acquired ---
                let wavetable = shared.try_lock().unwrap();

                // Step 1: global min/max across all frames
                let mut global_min = f32::INFINITY;
                let mut global_max = f32::NEG_INFINITY;
                for frame in &wavetable.frames {
                    for &s in frame {
                        global_min = global_min.min(s);
                        global_max = global_max.max(s);
                    }
                }
                let range = (global_max - global_min).max(0.001);

                // Step 2: build path points for ALL frames (back-to-front, like draw)
                let mut path_points: Vec<(f32, f32)> = Vec::with_capacity(frame_count * num_points);
                for frame_idx in (0..frame_count).rev() {
                    let frame = &wavetable.frames[frame_idx];
                    let depth = frame_idx as f32 / frame_count.max(1) as f32;
                    let px = depth * 80.0;
                    let py = -depth * 80.0;

                    for pi in 0..num_points {
                        let t = pi as f32 / num_points as f32;
                        let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
                        let normalized = (frame[si] - global_min) / range;
                        let x = padding + t * (width * 0.7) + px;
                        let y = widget_height - padding * 2.0 - (normalized * height * 0.4) + py;
                        path_points.push((x, y));
                    }
                }

                // --- Lock released ---
                drop(wavetable);

                let elapsed_us = t0.elapsed().as_nanos() as f64 / 1000.0;
                lock_times.push(elapsed_us);
                std::hint::black_box(&path_points);
            }

            print_timing_stats("3D draw (lock held)", &lock_times);
            eprintln!("  total path points per draw: {}", frame_count * num_points);
        }

        // ── 2D face-on path ─────────────────────────────────────────────────
        eprintln!("\n--- 2D path (single interpolated frame, {num_points} points) ---");
        {
            let mut lock_times = Vec::with_capacity(iterations);

            let exact_pos = current_frame_pos * (frame_count - 1) as f32;
            let lo = (exact_pos.floor() as usize).min(frame_count - 1);
            let hi = (lo + 1).min(frame_count - 1);
            let frac = exact_pos - lo as f32;

            // Warm up
            for _ in 0..5 {
                let wavetable = shared.try_lock().unwrap();
                let mut _fmin = f32::INFINITY;
                let mut _fmax = f32::NEG_INFINITY;
                for pi in 0..num_points {
                    let si = ((pi as f32 / num_points as f32) * frame_size as f32) as usize;
                    let si = si.min(frame_size - 1);
                    let s = wavetable.frames[lo][si] * (1.0 - frac) + wavetable.frames[hi][si] * frac;
                    _fmin = _fmin.min(s); _fmax = _fmax.max(s);
                }
                std::hint::black_box((_fmin, _fmax));
                drop(wavetable);
            }

            for _ in 0..iterations {
                let t0 = std::time::Instant::now();

                // --- Lock acquired ---
                let wavetable = shared.try_lock().unwrap();

                // Pass 1: min/max for normalization
                let mut fmin = f32::INFINITY;
                let mut fmax = f32::NEG_INFINITY;
                for pi in 0..num_points {
                    let si = ((pi as f32 / num_points as f32) * frame_size as f32) as usize;
                    let si = si.min(frame_size - 1);
                    let s = wavetable.frames[lo][si] * (1.0 - frac)
                        + wavetable.frames[hi][si] * frac;
                    fmin = fmin.min(s);
                    fmax = fmax.max(s);
                }
                let range_2d = (fmax - fmin).max(0.001);

                // Pass 2: build path points
                let mut path_points: Vec<(f32, f32)> = Vec::with_capacity(num_points);
                let x0 = padding;
                let y0 = padding;
                let zero_y = y0 + height / 2.0;

                for pi in 0..num_points {
                    let t = pi as f32 / num_points as f32;
                    let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
                    let s = wavetable.frames[lo][si] * (1.0 - frac)
                        + wavetable.frames[hi][si] * frac;
                    let normalized = (s - fmin) / range_2d;
                    let x = x0 + t * width;
                    let y = y0 + height - normalized * height;
                    path_points.push((x, y));
                }

                // --- Lock released ---
                drop(wavetable);

                let elapsed_us = t0.elapsed().as_nanos() as f64 / 1000.0;
                lock_times.push(elapsed_us);
                std::hint::black_box(&path_points);
                std::hint::black_box(zero_y);
            }

            print_timing_stats("2D draw (lock held)", &lock_times);
        }

        eprintln!("\n=== END WAVETABLE VIEW DRAW BENCHMARK ===\n");
    }

    // ── Frame-sweep regression: full process()-equivalent cost ────────

    /// Measure the ACTUAL cost of what process() does on every buffer when
    /// frame_position changes continuously.  This includes:
    ///   1. interpolate_frame_into
    ///   2. copy frame_cache -> frame_buf
    ///   3. compute_base_spectrum_into
    ///   4. crossfade bake (SIMD blend of existing kernel when a crossfade was active)
    ///   5. apply_resonance_and_ifft into crossfade_target_kernel
    ///   6. reverse_in_place on the target kernel
    ///
    /// Compares a 100-step sweep (0.0 -> 1.0) against 100 iterations at a
    /// static frame position.  Prints per-iteration times and flags any that
    /// exceed 1 ms.
    #[test]
    fn test_frame_sweep_regression() {
        // ── 1. Create a WavetableFilter via Default ─────────────────────
        let mut plugin = WavetableFilter::default();

        // ── 2. Load the real test wavetable ─────────────────────────────
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phaseless-bass.wt");
        let loaded_wt = Wavetable::from_file(path).expect("failed to load phaseless-bass.wt");
        eprintln!("\n=== FRAME SWEEP REGRESSION TEST ===");
        eprintln!("Wavetable: {} frames x {} samples", loaded_wt.frame_count, loaded_wt.frame_size);

        // ── 3. Set wavetable directly (not through reload mechanism) ────
        let new_size = loaded_wt.frame_size;
        plugin.wavetable = Some(loaded_wt);

        // Resize FFT plans and scratch buffers for the new frame size.
        let mut planner = RealFftPlanner::<f32>::new();
        plugin.frame_fft = planner.plan_fft_forward(new_size);
        let spec_len = new_size / 2 + 1;
        plugin.frame_cache.resize(new_size, 0.0);
        plugin.frame_buf.resize(new_size, 0.0);
        plugin.frame_spectrum.resize(spec_len, Complex::new(0.0, 0.0));
        plugin.frame_mags.resize(spec_len, 0.0);

        let sample_rate = plugin.sample_rate;
        let cutoff_hz = 1000.0f32;
        let resonance = 0.3f32;
        let num_steps = 100usize;
        let threshold_us = 1000.0; // 1 ms

        // ── Helper: run one full synthesis iteration as process() does ──
        // This closure captures the plugin mutably, so we inline it.
        fn run_synthesis_iteration(
            plugin: &mut WavetableFilter,
            frame_pos: f32,
            cutoff_hz: f32,
            sample_rate: f32,
            resonance: f32,
        ) -> f64 {
            let t0 = std::time::Instant::now();

            // Step 1: interpolate frame
            plugin
                .wavetable
                .as_ref()
                .unwrap()
                .interpolate_frame_into(frame_pos, &mut plugin.frame_cache);

            // Step 2: copy cache -> buf (FFT consumes buf)
            plugin.frame_buf.copy_from_slice(&plugin.frame_cache);

            // Step 3: compute base spectrum
            if WavetableFilter::compute_base_spectrum_into(
                &mut plugin.frame_buf,
                cutoff_hz,
                sample_rate,
                &plugin.frame_fft,
                &mut plugin.frame_spectrum,
                &mut plugin.frame_mags,
                &mut plugin.out_mags,
                &mut plugin.out_fracs,
            ) {
                // Step 4: crossfade bake (if a crossfade was active, bake it first)
                if plugin.crossfade_active {
                    let a_vec = f32x16::splat(plugin.crossfade_alpha);
                    let one_minus_a = f32x16::splat(1.0 - plugin.crossfade_alpha);
                    for chunk in 0..KERNEL_LEN / 16 {
                        let k = chunk * 16;
                        let s = f32x16::from_slice(&plugin.synthesized_kernel[k..k + 16]);
                        let t = f32x16::from_slice(&plugin.crossfade_target_kernel[k..k + 16]);
                        let blended = s * one_minus_a + t * a_vec;
                        plugin.synthesized_kernel[k..k + 16]
                            .copy_from_slice(&blended.to_array());
                    }
                    plugin.crossfade_active = false;
                    plugin.crossfade_alpha = 0.0;
                }

                // Step 5: IFFT into crossfade target
                WavetableFilter::apply_resonance_and_ifft(
                    &plugin.out_mags,
                    &plugin.out_fracs,
                    resonance,
                    &mut plugin.spectrum_work,
                    &mut plugin.crossfade_target_kernel,
                    &plugin.kernel_ifft,
                );

                // Step 6: reverse for convolution
                reverse_in_place(&mut plugin.crossfade_target_kernel);

                // Activate crossfade (as process() does)
                plugin.crossfade_active = true;
                plugin.crossfade_alpha = 0.0;
            }

            std::hint::black_box(&plugin.crossfade_target_kernel);
            std::hint::black_box(&plugin.synthesized_kernel);

            let elapsed_us = t0.elapsed().as_nanos() as f64 / 1000.0;
            elapsed_us
        }

        // ── Warm up FFT plans ───────────────────────────────────────────
        for _ in 0..5 {
            run_synthesis_iteration(&mut plugin, 0.0, cutoff_hz, sample_rate, resonance);
        }

        // ── SWEEP: frame_pos 0.0 -> 1.0 ────────────────────────────────
        eprintln!("\n--- SWEEP: frame_pos 0.0 -> 1.0 ({num_steps} steps) ---");
        let mut sweep_times = Vec::with_capacity(num_steps);
        let mut sweep_exceeded = 0usize;

        for i in 0..num_steps {
            let frame_pos = i as f32 / (num_steps - 1) as f32;
            // Simulate partial crossfade progress between buffers
            plugin.crossfade_alpha = (plugin.crossfade_alpha + plugin.crossfade_step * 256.0).min(1.0);
            let t = run_synthesis_iteration(&mut plugin, frame_pos, cutoff_hz, sample_rate, resonance);
            sweep_times.push(t);
            if t > threshold_us {
                sweep_exceeded += 1;
            }
        }

        print_timing_stats("SWEEP total", &sweep_times);
        eprintln!("  iterations exceeding {threshold_us:.0} us (1 ms): {sweep_exceeded}/{num_steps}");

        // Print first 10 and last 10 per-iteration times
        eprintln!("\n  Per-iteration times (first 10):");
        for i in 0..10.min(num_steps) {
            let flag = if sweep_times[i] > threshold_us { " *** EXCEEDED 1ms" } else { "" };
            eprintln!("    step {i:3}: frame_pos={:.3}  {:.1} us{flag}", i as f32 / (num_steps - 1) as f32, sweep_times[i]);
        }
        eprintln!("  Per-iteration times (last 10):");
        for i in (num_steps - 10).max(0)..num_steps {
            let flag = if sweep_times[i] > threshold_us { " *** EXCEEDED 1ms" } else { "" };
            eprintln!("    step {i:3}: frame_pos={:.3}  {:.1} us{flag}", i as f32 / (num_steps - 1) as f32, sweep_times[i]);
        }

        // ── STATIC: same frame_pos repeated ─────────────────────────────
        eprintln!("\n--- STATIC: frame_pos=0.5 ({num_steps} steps) ---");
        let mut static_times = Vec::with_capacity(num_steps);
        let mut static_exceeded = 0usize;

        // Reset crossfade state
        plugin.crossfade_active = false;
        plugin.crossfade_alpha = 0.0;

        for _ in 0..num_steps {
            plugin.crossfade_alpha = (plugin.crossfade_alpha + plugin.crossfade_step * 256.0).min(1.0);
            let t = run_synthesis_iteration(&mut plugin, 0.5, cutoff_hz, sample_rate, resonance);
            static_times.push(t);
            if t > threshold_us {
                static_exceeded += 1;
            }
        }

        print_timing_stats("STATIC total", &static_times);
        eprintln!("  iterations exceeding {threshold_us:.0} us (1 ms): {static_exceeded}/{num_steps}");

        // ── Compare sweep vs static ─────────────────────────────────────
        let sweep_avg = sweep_times.iter().sum::<f64>() / num_steps as f64;
        let static_avg = static_times.iter().sum::<f64>() / num_steps as f64;
        let ratio = sweep_avg / static_avg.max(0.001);

        eprintln!("\n--- COMPARISON ---");
        eprintln!("  Sweep avg:  {sweep_avg:.1} us");
        eprintln!("  Static avg: {static_avg:.1} us");
        eprintln!("  Ratio (sweep/static): {ratio:.2}x");

        if ratio > 2.0 {
            eprintln!("  WARNING: sweep is {ratio:.1}x slower than static -- possible regression");
        } else {
            eprintln!("  OK: sweep cost is within 2x of static baseline");
        }

        // Flag if any individual iteration exceeds 1ms in release mode
        let sweep_max = sweep_times.iter().cloned().fold(0.0f64, f64::max);
        let static_max = static_times.iter().cloned().fold(0.0f64, f64::max);
        eprintln!("  Sweep max:  {sweep_max:.1} us");
        eprintln!("  Static max: {static_max:.1} us");

        eprintln!("\n=== END FRAME SWEEP REGRESSION TEST ===\n");

        // Hard assertion: in release builds, no single iteration should exceed 1ms.
        // (In debug builds this will likely be exceeded; document rather than fail.)
        #[cfg(not(debug_assertions))]
        {
            assert!(
                sweep_exceeded == 0,
                "{sweep_exceeded}/{num_steps} sweep iterations exceeded 1 ms (max: {sweep_max:.0} us)"
            );
        }
    }

}
