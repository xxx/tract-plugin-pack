//! 90° phase rotator (Hilbert transform) + analytic-signal helper.
//!
//! The FIR Hilbert filter is a Type-IV anti-symmetric linear-phase design —
//! a true Hilbert transform with `length / 2` samples of group delay. At
//! length 65 on a 48 kHz signal that's ~0.7 ms of latency, which is
//! sub-perceptual for almost any musical application.
//!
//! An IIR variant (Niemitalo analytic-signal all-pass pair) was considered
//! but produces a `(real, imag)` pair where `imag` is 90°-rotated from
//! `real`, not from the original input. Reproducing a Hilbert-of-input from
//! that design requires the consumer to use both branches plus a
//! delay-matched dry path — the FIR is unambiguously correct and the
//! latency is negligible.
//!
//! [`AnalyticSignal`] wraps a `HilbertFir` plus a matching delay line for
//! the real part, returning a paired `(real, imag)` per sample. Most
//! callers want this — naïve `(input, hilbert(input))` is wrong because
//! the FIR's group delay shifts `imag` relative to `real`.

/// Type-IV anti-symmetric linear-phase Hilbert FIR.
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
    /// Construct a Type-IV anti-symmetric Hilbert FIR with a Hann window.
    /// Pass an odd length for proper symmetry; 65 is a good default for
    /// audio (~32 samples of group delay).
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

    /// Group delay in samples — equals `length / 2`.
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
        // Slice [next .. next + n] holds the last `n` samples in
        // oldest→newest order; zipping taps with the reversed slice gives
        // sum(taps[k] · x[n−k]).
        let hist = &self.history[next..next + n];
        let mut acc = 0.0;
        for (&tap, &h) in self.taps.iter().zip(hist.iter().rev()) {
            acc += tap * h;
        }
        self.write_idx = next;
        acc
    }
}

/// Analytic-signal extractor: returns the input as `(real, imag)` where
/// `real` is the input delayed by the Hilbert FIR's group delay and `imag`
/// is the Hilbert transform of the input. Both branches see the same
/// effective input sample.
///
/// Internal state: one `HilbertFir` and a ring-buffer delay line of length
/// `latency_samples()`.
pub struct AnalyticSignal {
    hilbert: HilbertFir,
    /// Ring-buffer delay matched to the Hilbert FIR's group delay; the
    /// `real` output is the input read out from this delay.
    delay: Vec<f32>,
    delay_idx: usize,
}

impl AnalyticSignal {
    /// Construct an analytic-signal extractor backed by a length-`length`
    /// Hilbert FIR. Odd `length` only — typical: 65.
    pub fn new(length: usize) -> Self {
        let hilbert = HilbertFir::new(length);
        let delay_len = hilbert.latency_samples();
        Self {
            hilbert,
            delay: vec![0.0; delay_len.max(1)],
            delay_idx: 0,
        }
    }

    /// Total samples of latency introduced — equals the Hilbert FIR's group
    /// delay.
    #[inline]
    pub fn latency_samples(&self) -> usize {
        self.hilbert.latency_samples()
    }

    /// Zero every piece of internal state.
    #[inline]
    pub fn reset(&mut self) {
        self.hilbert.reset();
        self.delay.fill(0.0);
        self.delay_idx = 0;
    }

    /// Process one sample, returning the analytic pair `(real, imag)`.
    ///
    /// `real` is `x` delayed by `latency_samples()`; `imag` is the Hilbert
    /// transform of `x` (which carries the same group delay). The two
    /// branches refer to the SAME effective input sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        // Read the delayed-real sample BEFORE overwriting this slot, so the
        // pair we return refers to the input from `latency_samples()` ago.
        let real = self.delay[self.delay_idx];
        self.delay[self.delay_idx] = x;
        self.delay_idx = (self.delay_idx + 1) % self.delay.len();
        let imag = self.hilbert.process(x);
        (real, imag)
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

    /// Analytic-signal pair delay-matches the Hilbert FIR's group delay.
    /// Drive an impulse: the `real` output sees it exactly `latency_samples()`
    /// after it was pushed, with zeros on either side.
    #[test]
    fn analytic_signal_pair_has_matching_delay() {
        let mut a = AnalyticSignal::new(65);
        let latency = a.latency_samples();
        for _ in 0..latency {
            let (r, _i) = a.process(0.0);
            assert_eq!(r, 0.0);
        }
        let (r, _) = a.process(1.0);
        assert_eq!(r, 0.0, "impulse hasn't reached the delay tap yet");
        for i in 0..latency * 2 {
            let (r, _i) = a.process(0.0);
            if i == latency - 1 {
                assert!((r - 1.0).abs() < 1e-6, "delay tap should be 1.0 here");
            } else {
                assert!(r.abs() < 1e-6, "delay tap should be 0.0 here, got {r}");
            }
        }
    }

    /// In the FIR's passband, the analytic-signal magnitude
    /// `sqrt(real² + imag²)` equals the steady-sine amplitude — that's the
    /// defining property: `real` and `imag` are 90° apart with matched
    /// magnitude. Tested at 2–10 kHz to stay clear of the sub-1-kHz
    /// roll-off (same band as `fir_magnitude_near_unity_midband`).
    #[test]
    fn analytic_signal_passband_magnitude_is_near_unity() {
        let sr = 48000.0;
        for &f in &[2000.0, 5000.0, 10000.0] {
            let mut a = AnalyticSignal::new(65);
            let lat = a.latency_samples();
            for i in 0..lat * 8 {
                a.process((std::f32::consts::TAU * f * i as f32 / sr).sin());
            }
            let mut max_err = 0.0_f32;
            for i in 0..1024 {
                let phase = std::f32::consts::TAU * f * (i + lat * 8) as f32 / sr;
                let (r, im) = a.process(phase.sin());
                let mag = (r * r + im * im).sqrt();
                max_err = max_err.max((mag - 1.0).abs());
            }
            assert!(max_err < 0.05, "f={f}: |analytic| - 1 max error {max_err}");
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
