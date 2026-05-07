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
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch_len = fft.get_inplace_scratch_len();
        let window = (0..FFT_SIZE)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos()
            })
            .collect();

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
        self.samples_since_last_fft = 0;
        for atom in &self.display.mag_m {
            atom.store(0_f32.to_bits(), Ordering::Relaxed);
        }
        for atom in &self.display.coherence {
            atom.store(0_f32.to_bits(), Ordering::Relaxed);
        }
    }

    /// Push one (M, S) pair. Triggers FFT every `HOP` samples.
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
}
