//! Audio-thread FFT analyzer.
//!
//! Single complex FFT of `M + jS` yields the M and S magnitude spectra in one transform:
//!   X[k] = FFT(M + jS)[k]
//!   |M|²[k] = (Re(X[k]) + Re(X[N-k]))² / 4 + (Im(X[k]) - Im(X[N-k]))² / 4
//!   |S|²[k] = (Im(X[k]) + Im(X[N-k]))² / 4 + (Re(X[k]) - Re(X[N-k]))² / 4
//! (Standard "two real FFTs in one complex FFT" trick.)
//!
//! Coherence per bin: γ²(k) = |Sxy|² / (Sxx · Syy)  ∈ [0, 1].
//! Computed audio-side via exponentially-smoothed cross-spectrum and auto-spectra.
//! Published as `1 - γ²` per log-spaced bin (high = decorrelated/wide, low = coherent).

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const FFT_SIZE: usize = 1024;
pub const HOP: usize = 1024;
pub const NUM_LOG_BINS: usize = 128;

/// Polar Level emit period in seconds. The audio thread peak-picks the
/// loudest sample over this window and emits one ray every interval.
/// Tuned together with `polar_rays::RING_CAPACITY` so the visible decay
/// window is `RING_CAPACITY × POLAR_EMIT_INTERVAL_S` (32 × 30 ms ≈ 960 ms).
pub const POLAR_EMIT_INTERVAL_S: f32 = 0.030;

/// Reference amplitude that maps to full disc radius for the Polar Level
/// rays. Each emit's amplitude is `magnitude(peak M, peak S) /
/// POLAR_LEVEL_REF`, clamped to `[0, 1]`. `0.3` ≈ -10 dBFS audio peak
/// reaches the rim — most 30 ms emit windows in typical music peak in
/// that range, so rays consistently reach the upper half of the disc.
/// Loud transients clamp; quiet content shows quiet (manual: "length =
/// amplitude").
pub const POLAR_LEVEL_REF: f32 = 0.3;

/// Lock-free spectrum publication shared between audio and GUI threads.
pub struct SpectrumDisplay {
    pub mag_m: [AtomicU32; NUM_LOG_BINS],
    pub coherence: [AtomicU32; NUM_LOG_BINS],
}

impl SpectrumDisplay {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            mag_m: std::array::from_fn(|_| AtomicU32::new(0)),
            coherence: std::array::from_fn(|_| AtomicU32::new(0)),
        })
    }

    pub fn read_mag_m(&self, idx: usize) -> f32 {
        f32::from_bits(self.mag_m[idx].load(Ordering::Relaxed))
    }

    pub fn read_coherence(&self, idx: usize) -> f32 {
        f32::from_bits(self.coherence[idx].load(Ordering::Relaxed))
    }
}

impl Default for SpectrumDisplay {
    fn default() -> Self {
        Self {
            mag_m: std::array::from_fn(|_| AtomicU32::new(0)),
            coherence: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

/// Audio-thread FFT analyzer state. Pre-allocate everything in `new`.
pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    /// Hann window, pre-computed.
    window: Vec<f32>,
    /// Input ring (interleaved M, S).
    ring_m: Vec<f32>,
    ring_s: Vec<f32>,
    ring_idx: usize,

    /// Scratch buffer for FFT input/output.
    scratch: Vec<Complex32>,
    /// Pre-allocated scratch buffer for `Fft::process_with_scratch` — avoids audio-thread alloc.
    fft_scratch: Vec<Complex32>,

    /// Smoothed auto/cross spectra for coherence (one bin per FFT bin we use).
    sxx: Vec<f32>,
    syy: Vec<f32>,
    sxy_re: Vec<f32>,
    sxy_im: Vec<f32>,

    /// Polar Level emit window: peak-pick the loudest (M, S) sample
    /// over the current emit interval, emit it once `polar_emit_period`
    /// samples have passed. Signed-mean averaging would collapse to
    /// zero for periodic audio (positive/negative half-cycles cancel),
    /// producing tiny rays. Peak-picking keeps the ray's length tied
    /// to the loudest moment in the window.
    polar_peak_amp_sq: f32,
    polar_peak_m: f32,
    polar_peak_s: f32,
    samples_since_polar_emit: usize,
    polar_emit_period: usize,
    polar_producer: Option<Arc<crate::polar_rays::PolarRayProducer>>,

    /// Throttle: hop counter.
    samples_since_last_fft: usize,

    /// Display sink.
    display: Arc<SpectrumDisplay>,

    /// Log-spaced bin centers (linear FFT bin → log bin mapping).
    log_bin_starts: [usize; NUM_LOG_BINS],
    log_bin_ends: [usize; NUM_LOG_BINS],
}

impl Analyzer {
    pub fn new(sample_rate: f32, display: Arc<SpectrumDisplay>) -> Self {
        Self::with_polar_producer(sample_rate, display, None)
    }

    pub fn with_polar_producer(
        sample_rate: f32,
        display: Arc<SpectrumDisplay>,
        polar_producer: Option<Arc<crate::polar_rays::PolarRayProducer>>,
    ) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch_len = fft.get_inplace_scratch_len();
        let window = tract_dsp::window::hann_symmetric(FFT_SIZE);

        let mut log_bin_starts = [0_usize; NUM_LOG_BINS];
        let mut log_bin_ends = [0_usize; NUM_LOG_BINS];
        let f_min = 20.0_f32;
        let f_max = (sample_rate * 0.5).min(20_000.0);
        let log_min = f_min.ln();
        let log_max = f_max.ln();
        for i in 0..NUM_LOG_BINS {
            let l_lo = log_min + (log_max - log_min) * i as f32 / NUM_LOG_BINS as f32;
            let l_hi = log_min + (log_max - log_min) * (i + 1) as f32 / NUM_LOG_BINS as f32;
            let f_lo = l_lo.exp();
            let f_hi = l_hi.exp();
            let k_lo = ((f_lo / sample_rate) * FFT_SIZE as f32) as usize;
            let k_hi = ((f_hi / sample_rate) * FFT_SIZE as f32).ceil() as usize;
            log_bin_starts[i] = k_lo.min(FFT_SIZE / 2);
            log_bin_ends[i] = k_hi.min(FFT_SIZE / 2).max(k_lo + 1);
        }

        let n_useful = FFT_SIZE / 2 + 1;
        let polar_emit_period = ((sample_rate * POLAR_EMIT_INTERVAL_S).round() as usize).max(1);
        Self {
            fft,
            window,
            ring_m: vec![0.0; FFT_SIZE],
            ring_s: vec![0.0; FFT_SIZE],
            ring_idx: 0,
            scratch: vec![Complex32::default(); FFT_SIZE],
            fft_scratch: vec![Complex32::default(); scratch_len],
            sxx: vec![0.0; n_useful],
            syy: vec![0.0; n_useful],
            sxy_re: vec![0.0; n_useful],
            sxy_im: vec![0.0; n_useful],
            polar_peak_amp_sq: 0.0,
            polar_peak_m: 0.0,
            polar_peak_s: 0.0,
            samples_since_polar_emit: 0,
            polar_emit_period,
            polar_producer,
            samples_since_last_fft: 0,
            display,
            log_bin_starts,
            log_bin_ends,
        }
    }

    pub fn reset(&mut self) {
        self.ring_m.fill(0.0);
        self.ring_s.fill(0.0);
        self.ring_idx = 0;
        self.sxx.fill(0.0);
        self.syy.fill(0.0);
        self.sxy_re.fill(0.0);
        self.sxy_im.fill(0.0);
        self.polar_peak_amp_sq = 0.0;
        self.polar_peak_m = 0.0;
        self.polar_peak_s = 0.0;
        self.samples_since_polar_emit = 0;
        self.samples_since_last_fft = 0;
        for atom in &self.display.mag_m {
            atom.store(0_f32.to_bits(), Ordering::Relaxed);
        }
        for atom in &self.display.coherence {
            atom.store(0_f32.to_bits(), Ordering::Relaxed);
        }
    }

    /// Push one (M, S) pair. Triggers FFT every `HOP` samples and
    /// emits a polar-level ray every `POLAR_EMIT_INTERVAL_S` seconds.
    #[inline]
    pub fn push(&mut self, m: f32, s: f32) {
        self.ring_m[self.ring_idx] = m;
        self.ring_s[self.ring_idx] = s;
        self.ring_idx = if self.ring_idx + 1 == FFT_SIZE {
            0
        } else {
            self.ring_idx + 1
        };
        self.samples_since_last_fft += 1;
        if self.samples_since_last_fft >= HOP {
            self.samples_since_last_fft = 0;
            self.compute_and_publish();
        }
        self.polar_emit_step(m, s);
    }

    /// Periodic polar ray emit. Tracks the loudest sample (largest
    /// `M² + S²`) over the current emit interval and publishes that
    /// sample's polar coordinates as a single ray. Signed-mean averaging
    /// would collapse to zero for periodic audio (positive/negative
    /// half-cycles cancel), so peak picking is the only way to keep
    /// each ray's length tied to the real audio amplitude.
    ///
    /// Polarity-invariant pan formula (matches Ozone's convention):
    ///   - `angle = π/2 + atan2(S · sign(M), |M|)`
    ///   - `amp   = magnitude(M, S) / POLAR_LEVEL_REF`, clamped to `[0, 1]`
    ///
    /// Mappings:
    ///   - Mono in-phase:                       π/2 (top of disc)
    ///   - Hard-L in-phase:                     3π/4 (upper-left spoke)
    ///   - Hard-R in-phase:                     π/4 (upper-right spoke)
    ///   - Anti-phase L-dominant (M≈0, S>0):    π   (left baseline corner)
    ///   - Anti-phase R-dominant (M≈0, S<0):    0   (right baseline corner)
    #[inline]
    fn polar_emit_step(&mut self, m: f32, s: f32) {
        let amp_sq = m * m + s * s;
        if amp_sq > self.polar_peak_amp_sq {
            self.polar_peak_amp_sq = amp_sq;
            self.polar_peak_m = m;
            self.polar_peak_s = s;
        }
        self.samples_since_polar_emit += 1;
        if self.samples_since_polar_emit < self.polar_emit_period {
            return;
        }
        let peak_m = self.polar_peak_m;
        let peak_s = self.polar_peak_s;
        let mag = self.polar_peak_amp_sq.sqrt();
        // Reset for the next emit interval.
        self.polar_peak_amp_sq = 0.0;
        self.polar_peak_m = 0.0;
        self.polar_peak_s = 0.0;
        self.samples_since_polar_emit = 0;

        if mag < 1e-9 {
            // Silent emit window — push zero amplitude so the ring slot
            // updates and the GUI sees the lack of recent content.
            if let Some(p) = self.polar_producer.as_ref() {
                p.emit(std::f32::consts::FRAC_PI_2, 0.0);
            }
            return;
        }
        let abs_m = peak_m.abs();
        let m_sign: f32 = if peak_m >= 0.0 { 1.0 } else { -1.0 };
        let s_norm = peak_s * m_sign;
        let angle = std::f32::consts::FRAC_PI_2 + s_norm.atan2(abs_m);
        let amp = (mag / POLAR_LEVEL_REF).min(1.0);
        if let Some(p) = self.polar_producer.as_ref() {
            p.emit(angle, amp);
        }
    }

    // Allocation-free per audio-thread invariant: process_with_scratch uses pre-allocated buffers.
    // Verified by `assert_process_allocs` once the analyzer is wired into Imagine::process.
    fn compute_and_publish(&mut self) {
        // Fill scratch with windowed M + jS, in chronological order.
        for i in 0..FFT_SIZE {
            let r = (self.ring_idx + i) % FFT_SIZE;
            let w = self.window[i];
            self.scratch[i] = Complex32::new(self.ring_m[r] * w, self.ring_s[r] * w);
        }

        self.fft
            .process_with_scratch(&mut self.scratch, &mut self.fft_scratch);

        // Decode |M|, |S|, and cross-spectrum from the two-real-in-one-complex FFT.
        const ALPHA: f32 = 0.3;
        let n = FFT_SIZE;
        let n_useful = n / 2 + 1;

        for k in 0..n_useful {
            let xk = self.scratch[k];
            let xnk = self.scratch[(n - k) % n];
            let m_re = (xk.re + xnk.re) * 0.5;
            let m_im = (xk.im - xnk.im) * 0.5;
            let s_re = (xk.im + xnk.im) * 0.5;
            let s_im = (xnk.re - xk.re) * 0.5;
            let mag_m_sq = m_re * m_re + m_im * m_im;
            let mag_s_sq = s_re * s_re + s_im * s_im;

            // Cross-spectrum X·conj(Y) where X=M, Y=S (using complex M and S)
            let cross_re = m_re * s_re + m_im * s_im;
            let cross_im = m_im * s_re - m_re * s_im;

            // Exponential smoothing
            self.sxx[k] = (1.0 - ALPHA) * self.sxx[k] + ALPHA * mag_m_sq;
            self.syy[k] = (1.0 - ALPHA) * self.syy[k] + ALPHA * mag_s_sq;
            self.sxy_re[k] = (1.0 - ALPHA) * self.sxy_re[k] + ALPHA * cross_re;
            self.sxy_im[k] = (1.0 - ALPHA) * self.sxy_im[k] + ALPHA * cross_im;
        }

        // Average linear bins → log-spaced bins for display.
        for li in 0..NUM_LOG_BINS {
            let lo = self.log_bin_starts[li].min(n_useful);
            let hi = self.log_bin_ends[li].min(n_useful);
            if hi <= lo {
                continue;
            }
            let mut mag_m_acc = 0.0;
            let mut sxx_acc = 0.0;
            let mut syy_acc = 0.0;
            let mut sxy_re_acc = 0.0;
            let mut sxy_im_acc = 0.0;
            for k in lo..hi {
                mag_m_acc += self.sxx[k].sqrt();
                sxx_acc += self.sxx[k];
                syy_acc += self.syy[k];
                sxy_re_acc += self.sxy_re[k];
                sxy_im_acc += self.sxy_im[k];
            }
            let count = (hi - lo) as f32;
            let mag_m_avg = mag_m_acc / count;
            // Coherence γ² = |Sxy|² / (Sxx · Syy) ∈ [0, 1].
            let cross_mag_sq = sxy_re_acc * sxy_re_acc + sxy_im_acc * sxy_im_acc;
            let denom = sxx_acc * syy_acc;
            // Display 1 - γ² so high values mean "decorrelated / wide".
            // No-signal case (denom near zero) publishes 0 instead of 1 so empty
            // buffers render as empty bars rather than full-pink.
            let width_metric = if denom > 1e-12 {
                let gamma_sq = (cross_mag_sq / denom).clamp(0.0, 1.0);
                1.0 - gamma_sq
            } else {
                0.0
            };

            self.display.mag_m[li].store(mag_m_avg.to_bits(), Ordering::Relaxed);
            self.display.coherence[li].store(width_metric.to_bits(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f32, n: usize, sr: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin())
            .collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut state: u32 = 0xdead_beef;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    fn noise_seeded(seed: u32, n: usize) -> Vec<f32> {
        let mut state = seed;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn sine_at_1khz_peaks_at_log_bin() {
        let sr = 48_000.0_f32;
        let display = SpectrumDisplay::new();
        let mut a = Analyzer::new(sr, display.clone());

        let m = sine(1000.0, 16_384, sr);
        for &mv in &m {
            a.push(mv, 0.0); // pure mid signal
        }
        let mut max_idx = 0;
        let mut max_val = 0.0;
        for i in 0..NUM_LOG_BINS {
            let v = display.read_mag_m(i);
            if v > max_val {
                max_val = v;
                max_idx = i;
            }
        }

        let f_min = 20.0_f32.ln();
        let f_max = 20_000.0_f32.ln();
        let target = ((1000.0_f32.ln() - f_min) / (f_max - f_min) * NUM_LOG_BINS as f32) as usize;
        let dist = (max_idx as i32 - target as i32).abs();
        assert!(dist <= 2, "max at {max_idx}, expected ~{target}");
    }

    #[test]
    fn coherent_input_low_width_metric() {
        let sr = 48_000.0_f32;
        let display = SpectrumDisplay::new();
        let mut a = Analyzer::new(sr, display.clone());
        // L = 0.9·R — highly correlated stereo.
        let r = noise(16_384);
        let l: Vec<f32> = r.iter().map(|&v| v * 0.9).collect();
        for i in 0..r.len() {
            let m = (l[i] + r[i]) * 0.5;
            let s = (l[i] - r[i]) * 0.5;
            a.push(m, s);
        }
        let mut sum = 0.0;
        for i in 0..NUM_LOG_BINS {
            sum += display.read_coherence(i);
        }
        let mean = sum / NUM_LOG_BINS as f32;
        assert!(mean < 0.30, "coherent input mean width metric {mean:.3}");
    }

    #[test]
    fn decorrelated_input_high_width_metric() {
        let sr = 48_000.0_f32;
        let display = SpectrumDisplay::new();
        let mut a = Analyzer::new(sr, display.clone());
        // Independent noise on L and R.
        let l = noise_seeded(0xdead_beef, 16_384);
        let r = noise_seeded(0xfeed_face, 16_384);
        for i in 0..l.len() {
            let m = (l[i] + r[i]) * 0.5;
            let s = (l[i] - r[i]) * 0.5;
            a.push(m, s);
        }
        let mut sum = 0.0;
        for i in 0..NUM_LOG_BINS {
            sum += display.read_coherence(i);
        }
        let mean = sum / NUM_LOG_BINS as f32;
        assert!(
            mean > 0.50,
            "decorrelated input mean width metric {mean:.3}"
        );
    }

    #[test]
    fn no_panic_at_extreme_sample_rates() {
        for &sr in &[44_100.0_f32, 96_000.0, 192_000.0] {
            let display = SpectrumDisplay::new();
            let mut a = Analyzer::new(sr, display);
            for i in 0..2048 {
                let s = (i as f32 * 0.01).sin();
                a.push(s, s * 0.5);
            }
        }
    }

    #[test]
    fn bin_count_stable() {
        for &sr in &[44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
            let display = SpectrumDisplay::new();
            let _ = Analyzer::new(sr, display);
        }
        assert_eq!(NUM_LOG_BINS, 128);
    }

    /// Sanity-check the polar emit pan-angle formula against the panel's
    /// half-disc convention: `0` = right baseline, `π/2` = top (mono),
    /// `π` = left baseline. Hard-L in-phase content must land in the
    /// LEFT half (angle > π/2); hard-R in-phase in the RIGHT half
    /// (angle < π/2). A regression here would explain a "mirrored bias"
    /// between Imagine and Ozone.
    #[test]
    fn polar_emit_hard_l_in_phase_is_left_side() {
        let sr = 48_000.0_f32;
        let display = SpectrumDisplay::new();
        let (prod, cons) = crate::polar_rays::ring_pair();
        let mut a = Analyzer::with_polar_producer(sr, display, Some(Arc::new(prod)));
        let dur = (sr * 0.040) as usize; // 40 ms — at least one emit
        for i in 0..dur {
            let l = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin();
            let r = 0.0_f32;
            let (m, s) = crate::midside::encode(l, r);
            a.push(m, s);
        }
        let mut rays = [crate::polar_rays::Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; crate::polar_rays::RING_CAPACITY];
        let n = cons.snapshot(&mut rays);
        assert!(n > 0, "no emits produced");
        let r = rays[0];
        assert!(
            r.angle > std::f32::consts::FRAC_PI_2,
            "hard-L in-phase peak emitted at angle {:.3} rad — expected > π/2 (left half)",
            r.angle
        );
    }

    #[test]
    fn polar_emit_hard_r_in_phase_is_right_side() {
        let sr = 48_000.0_f32;
        let display = SpectrumDisplay::new();
        let (prod, cons) = crate::polar_rays::ring_pair();
        let mut a = Analyzer::with_polar_producer(sr, display, Some(Arc::new(prod)));
        let dur = (sr * 0.040) as usize;
        for i in 0..dur {
            let l = 0.0_f32;
            let r = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin();
            let (m, s) = crate::midside::encode(l, r);
            a.push(m, s);
        }
        let mut rays = [crate::polar_rays::Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; crate::polar_rays::RING_CAPACITY];
        let n = cons.snapshot(&mut rays);
        assert!(n > 0, "no emits produced");
        let r = rays[0];
        assert!(
            r.angle < std::f32::consts::FRAC_PI_2,
            "hard-R in-phase peak emitted at angle {:.3} rad — expected < π/2 (right half)",
            r.angle
        );
    }

    #[test]
    fn polar_emit_mono_lands_at_top() {
        let sr = 48_000.0_f32;
        let display = SpectrumDisplay::new();
        let (prod, cons) = crate::polar_rays::ring_pair();
        let mut a = Analyzer::with_polar_producer(sr, display, Some(Arc::new(prod)));
        let dur = (sr * 0.040) as usize;
        for i in 0..dur {
            let v = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin();
            let (m, s) = crate::midside::encode(v, v);
            a.push(m, s);
        }
        let mut rays = [crate::polar_rays::Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; crate::polar_rays::RING_CAPACITY];
        let n = cons.snapshot(&mut rays);
        assert!(n > 0, "no emits produced");
        let r = rays[0];
        assert!(
            (r.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "mono emit at angle {:.3} rad — expected π/2",
            r.angle
        );
    }
}
