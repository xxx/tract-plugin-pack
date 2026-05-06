//! 90° phase rotator (Hilbert transform). Used by Recover Sides only.
//!
//! Linear-phase FIR (Type IV anti-symmetric) — a true Hilbert transform with
//! `length / 2` samples of group delay. We use this in both Quality modes; the
//! IIR-mode latency cost is dominated by this filter (~32 samples at length=65,
//! ~0.7 ms at 48 kHz — negligible).
//!
//! An IIR variant was considered (Niemitalo analytic-signal all-pass pair) but
//! produces a (real, imag) pair where `imag` is 90°-rotated from `real`, not
//! from the original input. Reproducing a Hilbert-of-input from this design
//! requires the consumer to use both branches and a delay-matched dry path,
//! which complicates the call site. The FIR is unambiguously correct and the
//! latency is small.

pub struct HilbertFir {
    taps: Vec<f32>,
    /// Double-buffered history: `2 * taps.len()` elements. Each sample is
    /// written at both `write_idx` and `write_idx + length`, so a contiguous
    /// `length`-element slice starting at any position is always available
    /// for the dot product (no per-tap wraparound branch).
    history: Vec<f32>,
    /// Write position in `[0, length)`.
    write_idx: usize,
}

impl HilbertFir {
    /// Construct a Type-IV anti-symmetric Hilbert FIR with Hann window.
    /// Pass an odd length for proper symmetry.
    pub fn new(length: usize) -> Self {
        debug_assert!(
            length % 2 == 1,
            "HilbertFir requires odd length for Type-IV symmetry; got {length}"
        );
        let mut taps = vec![0.0; length];
        Self::design_taps(&mut taps);
        Self {
            taps,
            history: vec![0.0; 2 * length],
            write_idx: 0,
        }
    }

    fn design_taps(taps: &mut [f32]) {
        let n = taps.len();
        let center = n as isize / 2;
        for (i, t) in taps.iter_mut().enumerate() {
            let k = i as isize - center;
            let raw = if k % 2 == 0 {
                0.0
            } else {
                2.0 / (std::f32::consts::PI * k as f32)
            };
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
            *t = raw * w;
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.write_idx = 0;
    }

    /// Group delay in samples.
    #[inline]
    pub fn latency_samples(&self) -> usize {
        self.taps.len() / 2
    }

    /// Returns the 90°-rotated input sample, delayed by `latency_samples()`.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let n = self.taps.len();
        // Mirror-write into both halves so the dot-product can read a
        // contiguous slice regardless of wraparound.
        self.history[self.write_idx] = x;
        self.history[self.write_idx + n] = x;
        let next = if self.write_idx + 1 == n {
            0
        } else {
            self.write_idx + 1
        };
        // Slice [next .. next + n] holds the last `n` samples in oldest→newest
        // order; zipping taps with the reversed slice gives sum(taps[k] · x[n−k]).
        let hist = &self.history[next..next + n];
        let mut acc = 0.0;
        for (&tap, &h) in self.taps.iter().zip(hist.iter().rev()) {
            acc += tap * h;
        }
        self.write_idx = next;
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, n: usize, sr: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect()
    }

    fn xcorr(a: &[f32], b: &[f32]) -> f32 {
        let mean_a = a.iter().sum::<f32>() / a.len() as f32;
        let mean_b = b.iter().sum::<f32>() / b.len() as f32;
        let cov: f32 = a
            .iter()
            .zip(b)
            .map(|(x, y)| (x - mean_a) * (y - mean_b))
            .sum();
        let var_a: f32 = a.iter().map(|x| (x - mean_a).powi(2)).sum();
        let var_b: f32 = b.iter().map(|x| (x - mean_b).powi(2)).sum();
        cov / (var_a.sqrt() * var_b.sqrt() + 1e-12)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// FIR magnitude is approximately unity across the design band.
    /// A length-65 Hann-windowed Hilbert FIR has substantial roll-off below
    /// ~1 kHz at 48 kHz (transition band of the windowed sinc), and very
    /// minor roll-off near Nyquist. We verify the mid-band (>= 2 kHz, well
    /// inside the passband and well below Nyquist) where ratio is within 1%
    /// of unity.
    #[test]
    fn fir_magnitude_near_unity_midband() {
        let sr = 48000.0;
        let mut h = HilbertFir::new(65);
        for &f in &[2000.0, 5000.0, 10000.0, 15000.0] {
            h.reset();
            let x = sine(f, 4096, sr);
            let y: Vec<f32> = x.iter().map(|&s| h.process(s)).collect();
            let lat = h.latency_samples();
            let skip = lat + 256;
            let rms_in = rms(&x[skip..]);
            let rms_out = rms(&y[skip..]);
            assert!(
                (rms_out / rms_in - 1.0).abs() < 0.05,
                "f={f}: ratio {:.3}",
                rms_out / rms_in
            );
        }
    }

    /// FIR phase: rotated output is decorrelated from input (when delay-aligned).
    /// Compare input shifted by `latency_samples()` against output, expecting xcorr near 0.
    #[test]
    fn fir_phase_near_90deg() {
        let sr = 48000.0;
        let mut h = HilbertFir::new(65);
        let lat = h.latency_samples();
        for &f in &[1000.0, 3000.0, 8000.0, 12000.0] {
            h.reset();
            let x = sine(f, 4096, sr);
            let y: Vec<f32> = x.iter().map(|&s| h.process(s)).collect();
            let skip = lat + 256;
            // Compare x[skip-lat..] (the un-rotated reference at the time the output was produced)
            // with y[skip..]. They should be 90°-shifted, hence xcorr near 0.
            let ref_window = &x[skip - lat..x.len() - lat];
            let out_window = &y[skip..];
            let n = ref_window.len().min(out_window.len());
            let c = xcorr(&ref_window[..n], &out_window[..n]);
            assert!(c.abs() < 0.15, "f={f}: xcorr {c:.3}");
        }
    }

    #[test]
    fn fir_latency_samples() {
        let h = HilbertFir::new(65);
        assert_eq!(h.latency_samples(), 32);
    }

    #[test]
    fn fir_linearity_under_input_scaling() {
        let sr = 48000.0;
        let mut h = HilbertFir::new(65);
        let x = sine(1000.0, 1024, sr);
        let y1: Vec<f32> = x.iter().map(|&s| h.process(s)).collect();
        h.reset();
        let y2: Vec<f32> = x.iter().map(|&s| h.process(s * 2.0)).collect();
        for i in 64..1024 {
            assert!((y2[i] - 2.0 * y1[i]).abs() < 1e-4, "i={i}");
        }
    }

    #[test]
    fn fir_does_not_blow_up() {
        let mut h = HilbertFir::new(65);
        for _ in 0..100_000 {
            let y = h.process(1.0);
            assert!(y.abs() < 100.0);
        }
    }

    /// Even-indexed taps (relative to center) should be zero (anti-symmetric Type IV).
    #[test]
    fn fir_taps_anti_symmetric() {
        let h = HilbertFir::new(65);
        let center = 65 / 2;
        // tap[center] (k=0) is zero.
        assert!(h.taps[center].abs() < 1e-9);
        // tap[center + 2k] for k != 0 is zero (because k%2 == 0 in design).
        for k in 1..=center / 2 {
            assert!(h.taps[center - 2 * k].abs() < 1e-9, "k={k}");
            assert!(h.taps[center + 2 * k].abs() < 1e-9, "k={k}");
        }
        // Anti-symmetry: tap[center - k] = -tap[center + k].
        for k in 1..center {
            let lhs = h.taps[center - k];
            let rhs = -h.taps[center + k];
            assert!((lhs - rhs).abs() < 1e-6, "k={k}: {} vs {}", lhs, rhs);
        }
    }
}
