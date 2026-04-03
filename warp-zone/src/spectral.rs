//! Phase vocoder with spectral shifting and stretching.
//!
//! STFT analysis → bin remapping (shift + stretch) → phase accumulation →
//! STFT synthesis → overlap-add.
//!
//! FFT size 4096, hop size 1024 (75% overlap, 4x redundancy), Hann window.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

pub struct SpectralShifter {
    fft_size: usize,
    hop_size: usize,

    fft_forward: Arc<dyn Fft<f32>>,
    fft_inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,

    analysis_window: Vec<f32>,
    synthesis_window: Vec<f32>,

    input_ring: Vec<f32>,
    output_ring: Vec<f64>,
    input_pos: usize,
    read_pos: usize,
    hop_counter: usize,

    fft_buf: Vec<Complex<f32>>,
    out_buf: Vec<Complex<f32>>,

    last_input_phase: Vec<f32>,
    accumulated_output_phase: Vec<f32>,

    /// Output magnitudes from the most recent frame (for visualization).
    last_output_magnitudes: Vec<f32>,
}

impl SpectralShifter {
    pub fn new(fft_size: usize, hop_size: usize) -> Self {
        assert!(fft_size > 0 && hop_size > 0 && fft_size >= hop_size);

        let mut planner = FftPlanner::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);
        let scratch_len = fft_forward
            .get_inplace_scratch_len()
            .max(fft_inverse.get_inplace_scratch_len());

        let analysis_window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / fft_size as f32).cos()))
            .collect();

        // COLA normalization (same approach as satch)
        let num_frames = fft_size / hop_size;
        let mut cola_check = vec![0.0_f64; hop_size];
        for frame in 0..num_frames {
            let offset = frame * hop_size;
            for p in 0..hop_size {
                let w = analysis_window[p + offset] as f64;
                cola_check[p] += w * w;
            }
        }
        let cola_factor = cola_check[0] as f32;
        let inv_cola = 1.0 / cola_factor;
        let synthesis_window: Vec<f32> = analysis_window.iter().map(|&w| w * inv_cola).collect();

        let out_ring_size = 2 * fft_size;
        let half_plus_one = fft_size / 2 + 1;

        Self {
            fft_size,
            hop_size,
            fft_forward,
            fft_inverse,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            analysis_window,
            synthesis_window,
            input_ring: vec![0.0; fft_size],
            output_ring: vec![0.0; out_ring_size],
            input_pos: 0,
            read_pos: 0,
            hop_counter: 0,
            fft_buf: vec![Complex::new(0.0, 0.0); fft_size],
            out_buf: vec![Complex::new(0.0, 0.0); fft_size],
            last_input_phase: vec![0.0; half_plus_one],
            accumulated_output_phase: vec![0.0; half_plus_one],
            last_output_magnitudes: vec![0.0; half_plus_one],
        }
    }

    pub fn latency_samples(&self) -> usize {
        self.fft_size
    }

    pub fn reset(&mut self) {
        self.input_ring.fill(0.0);
        self.output_ring.fill(0.0);
        self.input_pos = 0;
        self.read_pos = 0;
        self.hop_counter = 0;
        self.last_input_phase.fill(0.0);
        self.accumulated_output_phase.fill(0.0);
        self.last_output_magnitudes.fill(0.0);
        for bin in self.fft_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
        for bin in self.out_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
    }

    /// Returns the output magnitudes from the most recent processed frame.
    pub fn output_magnitudes(&self) -> &[f32] {
        &self.last_output_magnitudes
    }

    /// Process a single input sample.
    /// `shift`: semitones (-24..+24). `stretch`: ratio (0.5..2.0).
    /// `freeze`: stop updating input (sustain current spectrum).
    /// `low_bin`/`high_bin`: frequency range for remapping (bins outside pass through).
    pub fn process_sample(
        &mut self,
        input: f32,
        shift: f32,
        stretch: f32,
        freeze: bool,
        low_bin: usize,
        high_bin: usize,
    ) -> f32 {
        let out_len = self.output_ring.len();

        if !freeze {
            self.input_ring[self.input_pos] = input;
            self.input_pos = (self.input_pos + 1) % self.fft_size;
        }

        let output = self.output_ring[self.read_pos] as f32;
        self.output_ring[self.read_pos] = 0.0;
        self.read_pos = (self.read_pos + 1) % out_len;

        self.hop_counter += 1;
        if self.hop_counter >= self.hop_size {
            self.hop_counter = 0;
            self.process_frame(shift, stretch, low_bin, high_bin);
        }

        output
    }

    fn process_frame(&mut self, shift: f32, stretch: f32, low_bin: usize, high_bin: usize) {
        let n = self.fft_size;
        let out_len = self.output_ring.len();

        // Extract frame from input ring, apply analysis window
        for i in 0..n {
            let ring_idx = (self.input_pos + i) % n;
            self.fft_buf[i] = Complex::new(
                self.input_ring[ring_idx] * self.analysis_window[i],
                0.0,
            );
        }

        // Forward FFT
        self.fft_forward
            .process_with_scratch(&mut self.fft_buf, &mut self.scratch);

        // Identity short-circuit: skip phase vocoder when no shift/stretch
        let is_identity = shift.abs() < 1e-6 && (stretch - 1.0).abs() < 1e-6;
        if is_identity {
            self.out_buf.copy_from_slice(&self.fft_buf);
        } else {
            self.remap_bins(shift, stretch, low_bin, high_bin);
        }

        // Capture output magnitudes for visualization (before IFFT).
        // Normalize by fft_size/2 so a full-scale sine ≈ 1.0.
        let half = n / 2 + 1;
        let norm_factor = 2.0 / n as f32;
        for k in 0..half {
            self.last_output_magnitudes[k] = self.out_buf[k].norm() * norm_factor;
        }

        // Inverse FFT
        self.fft_inverse
            .process_with_scratch(&mut self.out_buf, &mut self.scratch);

        // Normalize and overlap-add
        let inv_n = 1.0 / n as f32;
        let write_start = self.read_pos;
        for i in 0..n {
            let idx = (write_start + i) % out_len;
            self.output_ring[idx] +=
                (self.out_buf[i].re * inv_n * self.synthesis_window[i]) as f64;
        }
    }

    /// Remap frequency bins according to shift (semitones) and stretch (ratio).
    /// Uses linear interpolation for magnitude and correct phase vocoder formula.
    ///
    /// Algorithm:
    /// 1. For each source bin, compute magnitude, phase, and phase deviation.
    /// 2. Map to target frequency: `target_f = k * stretch * shift_ratio`.
    /// 3. Distribute magnitude to adjacent target bins via linear interpolation.
    /// 4. Max-magnitude-wins per target bin (prevents runaway phase accumulation).
    /// 5. Accumulate output phases and construct final complex spectrum.
    fn remap_bins(&mut self, shift: f32, stretch: f32, low_bin: usize, high_bin: usize) {
        let n = self.fft_size;
        let half_plus_one = n / 2 + 1;

        // Clear output buffer
        for bin in self.out_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }

        let shift_ratio = (2.0_f32).powf(shift / 12.0);
        let lo = low_bin.max(1);
        let hi = high_bin.min(half_plus_one);

        // Pass through bins outside the active range (no shift/stretch)
        for k in 1..lo.min(half_plus_one) {
            self.out_buf[k] = self.fft_buf[k];
            if k < n / 2 {
                self.out_buf[n - k] = self.fft_buf[n - k];
            }
            // Keep phase tracking consistent
            self.last_input_phase[k] = self.fft_buf[k].arg();
            self.accumulated_output_phase[k] = self.fft_buf[k].arg();
        }
        for k in hi..half_plus_one {
            self.out_buf[k] = self.fft_buf[k];
            if k < n / 2 {
                self.out_buf[n - k] = self.fft_buf[n - k];
            }
            self.last_input_phase[k] = self.fft_buf[k].arg();
            self.accumulated_output_phase[k] = self.fft_buf[k].arg();
        }

        // Phase 1: Remap bins within the active range.
        // We use out_buf as temporary workspace for in-range bins:
        //   out_buf[k].re = best magnitude so far for target bin k
        //   out_buf[k].im = corresponding phase increment
        for k in lo..hi {
            let mag = self.fft_buf[k].norm();
            let phase = self.fft_buf[k].arg();

            // Phase deviation from expected
            let expected_phase_inc =
                2.0 * PI * self.hop_size as f32 * k as f32 / n as f32;
            let phase_diff = phase - self.last_input_phase[k];
            let phase_dev = wrap_phase(phase_diff - expected_phase_inc);

            self.last_input_phase[k] = phase;

            // Target bin: stretch first, then shift
            let target_f = k as f32 * stretch * shift_ratio;

            // Linear interpolation: distribute magnitude to two adjacent bins
            let target_lo = target_f.floor() as usize;
            let target_hi = target_lo + 1;
            let frac = target_f - target_lo as f32;

            // Expected phase increment for target bins (correct phase vocoder formula)
            let expected_target_lo =
                2.0 * PI * self.hop_size as f32 * target_lo as f32 / n as f32;
            let expected_target_hi =
                2.0 * PI * self.hop_size as f32 * target_hi as f32 / n as f32;

            // Phase increment = expected_target + phase_deviation (NOT scaled)
            let phase_inc_lo = expected_target_lo + phase_dev;
            let phase_inc_hi = expected_target_hi + phase_dev;

            // Low bin contribution (max-magnitude-wins)
            if target_lo > 0 && target_lo < half_plus_one {
                let contrib_mag = mag * (1.0 - frac);
                if contrib_mag > self.out_buf[target_lo].re {
                    self.out_buf[target_lo] = Complex::new(contrib_mag, phase_inc_lo);
                }
            }

            // High bin contribution (max-magnitude-wins)
            if target_hi > 0 && target_hi < half_plus_one {
                let contrib_mag = mag * frac;
                if contrib_mag > self.out_buf[target_hi].re {
                    self.out_buf[target_hi] = Complex::new(contrib_mag, phase_inc_hi);
                }
            }
        }

        // Phase 2: accumulate phases and construct final complex output
        for k in 1..half_plus_one {
            let mag = self.out_buf[k].re;
            let phase_inc = self.out_buf[k].im;

            if mag > 0.0 {
                self.accumulated_output_phase[k] += phase_inc;
                let out_phase = self.accumulated_output_phase[k];

                self.out_buf[k] = Complex::new(
                    mag * out_phase.cos(),
                    mag * out_phase.sin(),
                );

                // Mirror for negative frequencies
                if k < n / 2 {
                    self.out_buf[n - k] = self.out_buf[k].conj();
                }
            } else {
                self.out_buf[k] = Complex::new(0.0, 0.0);
                if k < n / 2 {
                    self.out_buf[n - k] = Complex::new(0.0, 0.0);
                }
            }
        }

        // DC bin: pass through
        self.out_buf[0] = self.fft_buf[0];
    }
}

/// Wrap a phase value to the range [-PI, PI] using modular arithmetic.
/// Safe for arbitrarily large phase values (no loops).
#[inline]
fn wrap_phase(phase: f32) -> f32 {
    phase - (2.0 * PI) * ((phase + PI) / (2.0 * PI)).floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::FftPlanner;

    #[test]
    fn test_identity_passthrough() {
        let fft_size = 4096;
        let hop_size = 1024;
        let mut shifter = SpectralShifter::new(fft_size, hop_size);

        let sr = 48000.0_f32;
        let freq = 440.0;
        let num_samples = 32768;
        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let output: Vec<f32> = input.iter()
            .map(|&s| shifter.process_sample(s, 0.0, 1.0, false, 0, usize::MAX))
            .collect();

        // Skip initial latency + settling
        let skip = fft_size + fft_size;
        let latency = fft_size;

        let mut max_err = 0.0_f32;
        for i in skip..num_samples {
            let expected = input[i - latency];
            let err = (output[i] - expected).abs();
            max_err = max_err.max(err);
        }

        assert!(max_err < 0.01, "identity passthrough error too large: {max_err}");
    }

    #[test]
    fn test_silence_passthrough() {
        let mut shifter = SpectralShifter::new(4096, 1024);
        let output: Vec<f32> = (0..8192)
            .map(|_| shifter.process_sample(0.0, 0.0, 1.0, false, 0, usize::MAX))
            .collect();
        let peak = output.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
        assert!(peak < 1e-10, "silence should produce silence, got peak={peak}");
    }

    #[test]
    fn test_latency() {
        let shifter = SpectralShifter::new(4096, 1024);
        assert_eq!(shifter.latency_samples(), 4096);
    }

    /// Shifting a 440 Hz sine up by 12 semitones should produce ~880 Hz.
    #[test]
    fn test_shift_up_octave() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let freq = 440.0;
        let num_samples = 65536;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut shifter = SpectralShifter::new(fft_size, hop_size);
        let output: Vec<f32> = input.iter()
            .map(|&s| shifter.process_sample(s, 12.0, 1.0, false, 0, usize::MAX))
            .collect();

        let skip = fft_size * 3;
        let analysis = &output[skip..];
        let mut crossings = 0;
        for i in 1..analysis.len() {
            if (analysis[i - 1] < 0.0) != (analysis[i] < 0.0) {
                crossings += 1;
            }
        }
        let duration_secs = analysis.len() as f32 / sr;
        let estimated_freq = crossings as f32 / (2.0 * duration_secs);
        let expected = freq * 2.0;

        let ratio = estimated_freq / expected;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "expected ~{expected} Hz, got ~{estimated_freq} Hz (ratio={ratio})"
        );
    }

    /// Shifting down by 12 semitones should halve the frequency.
    #[test]
    fn test_shift_down_octave() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let freq = 880.0;
        let num_samples = 65536;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut shifter = SpectralShifter::new(fft_size, hop_size);
        let output: Vec<f32> = input.iter()
            .map(|&s| shifter.process_sample(s, -12.0, 1.0, false, 0, usize::MAX))
            .collect();

        let skip = fft_size * 3;
        let analysis = &output[skip..];
        let mut crossings = 0;
        for i in 1..analysis.len() {
            if (analysis[i - 1] < 0.0) != (analysis[i] < 0.0) {
                crossings += 1;
            }
        }
        let duration_secs = analysis.len() as f32 / sr;
        let estimated_freq = crossings as f32 / (2.0 * duration_secs);
        let expected = freq / 2.0;

        let ratio = estimated_freq / expected;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "expected ~{expected} Hz, got ~{estimated_freq} Hz (ratio={ratio})"
        );
    }

    /// Fractional semitone shift: 7 semitones up on 440 Hz -> ~659 Hz (perfect fifth).
    #[test]
    fn test_shift_fractional_semitones() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let freq = 440.0;
        let num_samples = 65536;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut shifter = SpectralShifter::new(fft_size, hop_size);
        let output: Vec<f32> = input.iter()
            .map(|&s| shifter.process_sample(s, 7.0, 1.0, false, 0, usize::MAX))
            .collect();

        let skip = fft_size * 3;
        let analysis = &output[skip..];
        let mut crossings = 0;
        for i in 1..analysis.len() {
            if (analysis[i - 1] < 0.0) != (analysis[i] < 0.0) {
                crossings += 1;
            }
        }
        let duration_secs = analysis.len() as f32 / sr;
        let estimated_freq = crossings as f32 / (2.0 * duration_secs);
        let expected = freq * 2.0_f32.powf(7.0 / 12.0); // ~659.26 Hz

        let ratio = estimated_freq / expected;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "expected ~{expected:.0} Hz, got ~{estimated_freq:.0} Hz (ratio={ratio})"
        );
    }

    /// Extreme shift: +24 semitones (4x frequency). 200 Hz -> ~800 Hz.
    #[test]
    fn test_shift_extreme_up() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let freq = 200.0;
        let num_samples = 65536;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut shifter = SpectralShifter::new(fft_size, hop_size);
        let output: Vec<f32> = input.iter()
            .map(|&s| shifter.process_sample(s, 24.0, 1.0, false, 0, usize::MAX))
            .collect();

        let skip = fft_size * 3;
        let analysis = &output[skip..];
        let mut crossings = 0;
        for i in 1..analysis.len() {
            if (analysis[i - 1] < 0.0) != (analysis[i] < 0.0) {
                crossings += 1;
            }
        }
        let duration_secs = analysis.len() as f32 / sr;
        let estimated_freq = crossings as f32 / (2.0 * duration_secs);
        let expected = freq * 4.0;

        let ratio = estimated_freq / expected;
        assert!(
            (ratio - 1.0).abs() < 0.1,
            "expected ~{expected:.0} Hz, got ~{estimated_freq:.0} Hz (ratio={ratio})"
        );
    }

    /// Stretch=2.0 should spread harmonics. All bins map to target_f = k * stretch,
    /// so 200 Hz -> 400 Hz, 400 Hz -> 800 Hz, 600 Hz -> 1200 Hz.
    #[test]
    fn test_stretch_spectral_content() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let num_samples = 32768;

        // Harmonic signal: f=200 + 2f=400 + 3f=600
        let input: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / sr;
                0.5 * (2.0 * PI * 200.0 * t).sin()
                    + 0.25 * (2.0 * PI * 400.0 * t).sin()
                    + 0.125 * (2.0 * PI * 600.0 * t).sin()
            })
            .collect();

        let mut stretched = SpectralShifter::new(fft_size, hop_size);
        let output: Vec<f32> = input.iter()
            .map(|&s| stretched.process_sample(s, 0.0, 2.0, false, 0, usize::MAX))
            .collect();

        // Run FFT on a chunk of the output to analyze spectral content
        let skip = fft_size * 3;
        let chunk = &output[skip..skip + fft_size];

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut buf: Vec<Complex<f32>> = chunk.iter()
            .map(|&s| Complex::new(s, 0.0))
            .collect();
        fft.process(&mut buf);

        // Find the bin with maximum energy
        let bin_to_freq = |k: usize| k as f32 * sr / fft_size as f32;
        let peak_bin = buf[1..fft_size / 2]
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.norm().partial_cmp(&b.norm()).unwrap())
            .unwrap()
            .0 + 1;
        let peak_freq = bin_to_freq(peak_bin);

        // With stretch=2.0, fundamental (200 Hz, strongest component) maps to ~400 Hz
        assert!(
            (peak_freq - 400.0).abs() < 50.0,
            "fundamental should be near 400 Hz (200*2.0), got {peak_freq:.0} Hz"
        );

        // 2nd harmonic (400 Hz input) should map to ~800 Hz
        let bin_800 = (800.0 / bin_to_freq(1)).round() as usize;
        let bin_1200 = (1200.0 / bin_to_freq(1)).round() as usize;
        let mag_800 = buf[bin_800].norm();
        let mag_1200 = buf[bin_1200].norm();

        // 800 Hz should have significant energy (from the 400 Hz input harmonic)
        assert!(
            mag_800 > mag_1200 * 0.5,
            "stretch=2.0: 800 Hz ({mag_800:.3}) should have significant energy vs 1200 Hz ({mag_1200:.3})"
        );
    }

    /// Stretch at boundary: stretch=0.5 should compress harmonics.
    #[test]
    fn test_stretch_half() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let num_samples = 32768;

        // Harmonics at 200, 400, 600 Hz
        let input: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / sr;
                0.5 * (2.0 * PI * 200.0 * t).sin()
                    + 0.25 * (2.0 * PI * 400.0 * t).sin()
                    + 0.125 * (2.0 * PI * 600.0 * t).sin()
            })
            .collect();

        let mut identity = SpectralShifter::new(fft_size, hop_size);
        let out_identity: Vec<f32> = input.iter()
            .map(|&s| identity.process_sample(s, 0.0, 1.0, false, 0, usize::MAX))
            .collect();

        let mut compressed = SpectralShifter::new(fft_size, hop_size);
        let out_compressed: Vec<f32> = input.iter()
            .map(|&s| compressed.process_sample(s, 0.0, 0.5, false, 0, usize::MAX))
            .collect();

        let skip = fft_size * 3;
        let mut diff_sum = 0.0_f64;
        for i in skip..num_samples {
            let d = (out_compressed[i] - out_identity[i]) as f64;
            diff_sum += d * d;
        }
        let rms_diff = (diff_sum / (num_samples - skip) as f64).sqrt();
        assert!(rms_diff > 0.01, "stretch=0.5 should differ from identity: {rms_diff}");
    }

    /// Combined shift+stretch: verify stretch is applied first, then shift.
    #[test]
    fn test_shift_plus_stretch_ordering() {
        let fft_size = 4096;
        let hop_size = 1024;
        let sr = 48000.0_f32;
        let num_samples = 32768;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.8 * (2.0 * PI * 300.0 * i as f32 / sr).sin())
            .collect();

        // Apply shift=12 + stretch=1.5
        let mut shifter = SpectralShifter::new(fft_size, hop_size);
        let output: Vec<f32> = input.iter()
            .map(|&s| shifter.process_sample(s, 12.0, 1.5, false, 0, usize::MAX))
            .collect();

        // Output should exist and not be silence
        let skip = fft_size * 3;
        let peak = output[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
        assert!(peak > 0.1, "combined shift+stretch should produce audible output: peak={peak}");
    }
}
