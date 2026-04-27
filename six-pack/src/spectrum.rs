//! Input spectrum analyzer for the GUI background overlay.
//!
//! Audio thread maintains a 2048-sample mono ring buffer. Every 1024 samples
//! pushed, runs an FFT once and writes magnitude bins to atomic storage.
//! GUI thread reads bins lock-free.

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::atomic::{AtomicU32, Ordering};

const FFT_SIZE: usize = 2048;
const HOP: usize = 1024;
pub const N_BINS: usize = 128;

pub struct SpectrumAnalyzer {
    ring: Vec<f32>,
    write_pos: usize,
    samples_since_fft: usize,
    /// Per-instance random offset to stagger FFTs across instances.
    initial_offset: usize,
    fft: std::sync::Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,
    fft_input: Vec<Complex32>,
    window: Vec<f32>,
    pub bins: std::sync::Arc<[AtomicU32; N_BINS]>,
}

fn make_atomic_bin_array() -> std::sync::Arc<[AtomicU32; N_BINS]> {
    let arr = std::array::from_fn(|_| AtomicU32::new(0));
    std::sync::Arc::new(arr)
}

impl SpectrumAnalyzer {
    pub fn new(seed: u32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch_len = fft.get_inplace_scratch_len();

        // Hann window
        let window = (0..FFT_SIZE)
            .map(|n| {
                0.5 - 0.5
                    * (2.0 * std::f32::consts::PI * (n as f32) / ((FFT_SIZE - 1) as f32)).cos()
            })
            .collect();

        // Random offset for FFT throttle phase staggering across instances.
        // Seed deterministically from the input value so tests are stable.
        let initial_offset = (seed as usize) % HOP;

        Self {
            ring: vec![0.0; FFT_SIZE],
            write_pos: 0,
            samples_since_fft: initial_offset,
            initial_offset,
            fft,
            fft_scratch: vec![Complex32::default(); scratch_len],
            fft_input: vec![Complex32::default(); FFT_SIZE],
            window,
            bins: make_atomic_bin_array(),
        }
    }

    /// Push one mono sample (e.g., (L+R)/2). Triggers FFT every HOP samples.
    pub fn push_sample(&mut self, x: f32) {
        self.ring[self.write_pos] = x;
        self.write_pos = (self.write_pos + 1) % FFT_SIZE;
        self.samples_since_fft += 1;
        if self.samples_since_fft >= HOP {
            self.samples_since_fft = 0;
            self.run_fft();
        }
    }

    fn run_fft(&mut self) {
        // Copy windowed ring into fft_input, oldest sample first.
        for i in 0..FFT_SIZE {
            let r = (self.write_pos + i) % FFT_SIZE;
            self.fft_input[i] = Complex32::new(self.ring[r] * self.window[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.fft_input, &mut self.fft_scratch);

        // Reduce FFT_SIZE/2 magnitude bins to N_BINS log-spaced bins.
        let n_freq_bins = FFT_SIZE / 2;
        let log_min = (1.0_f32).ln();
        let log_max = (n_freq_bins as f32).ln();
        for k in 0..N_BINS {
            // Map output bin k (linear in log space) to FFT bin index range.
            let frac_lo = k as f32 / N_BINS as f32;
            let frac_hi = (k + 1) as f32 / N_BINS as f32;
            let lo = ((log_min + frac_lo * (log_max - log_min)).exp() as usize).max(1);
            let hi = ((log_min + frac_hi * (log_max - log_min)).exp() as usize).min(n_freq_bins);
            let mut max_mag = 0.0_f32;
            for j in lo..hi {
                let m = self.fft_input[j].norm();
                if m > max_mag {
                    max_mag = m;
                }
            }
            // Normalize: 2/FFT_SIZE for one-sided magnitude.
            let mag = (max_mag * 2.0 / (FFT_SIZE as f32)).min(2.0);
            self.bins[k].store(mag.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write_pos = 0;
        self.samples_since_fft = self.initial_offset;
        for bin in self.bins.iter() {
            bin.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_khz_sine_peaks_at_one_khz_bin() {
        let sr = 48_000.0_f32;
        let mut sa = SpectrumAnalyzer::new(0);
        let target = 1_000.0_f32;
        for i in 0..(FFT_SIZE * 4) {
            let phase = (i as f32) / sr * std::f32::consts::TAU * target;
            sa.push_sample(phase.sin());
        }
        let (max_idx, max_val) = sa
            .bins
            .iter()
            .enumerate()
            .map(|(i, a)| (i, f32::from_bits(a.load(Ordering::Relaxed))))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();

        let n_freq_bins = (FFT_SIZE / 2) as f32;
        let log_max = n_freq_bins.ln();
        let frac = (max_idx as f32 + 0.5) / N_BINS as f32;
        let bin_freq_idx = (frac * log_max).exp();
        let bin_freq_hz = bin_freq_idx * sr / (FFT_SIZE as f32);
        assert!(
            (bin_freq_hz - target).abs() / target < 0.15,
            "peak bin freq = {bin_freq_hz} Hz (expected ~{target}); max_val={max_val}"
        );
    }
}
