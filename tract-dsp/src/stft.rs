//! Magnitude-only STFT convolution: fixed-frame Hann-windowed overlap-add.

use crate::window::hann_periodic;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use std::sync::Arc;

/// Per-channel magnitude-only STFT convolution. A fixed `frame`-point real
/// transform; each frame's spectrum has its per-bin magnitude scaled (phase
/// preserved), then inverse-transformed and overlap-added at 50% overlap with
/// `1/frame` normalisation. Output is delayed by `hop = frame / 2` samples.
pub struct StftConvolver {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// Circular input buffer, `frame` samples; oldest sample at `in_pos`.
    in_buf: Vec<f32>,
    in_pos: usize,
    /// Overlap-add output accumulator, `frame` samples.
    out_buf: Vec<f32>,
    /// Read/write position within the current hop (`0..hop`).
    out_pos: usize,
    scratch_time: Vec<f32>,
    scratch_freq: Vec<Complex<f32>>,
    /// Pre-allocated realfft scratch (forward and inverse can differ).
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    frame: usize,
    hop: usize,
}

impl StftConvolver {
    /// A convolver with a fixed `frame`-point transform and `hop = frame / 2`
    /// (50% overlap). `frame` must be even and a power of two. The analysis
    /// window is a periodic Hann window.
    pub fn new(frame: usize) -> Self {
        let hop = frame / 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(frame);
        let ifft = planner.plan_fft_inverse(frame);
        let scratch_fwd = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        Self {
            fft,
            ifft,
            window: hann_periodic(frame),
            in_buf: vec![0.0; frame],
            in_pos: 0,
            out_buf: vec![0.0; frame],
            out_pos: 0,
            scratch_time: vec![0.0; frame],
            scratch_freq: vec![Complex::new(0.0, 0.0); frame / 2 + 1],
            scratch_fwd,
            scratch_inv,
            frame,
            hop,
        }
    }

    /// Zero all state.
    pub fn reset(&mut self) {
        self.in_buf.iter_mut().for_each(|s| *s = 0.0);
        self.out_buf.iter_mut().for_each(|s| *s = 0.0);
        self.in_pos = 0;
        self.out_pos = 0;
    }

    /// Inherent latency in samples (`= hop`).
    pub fn latency(&self) -> usize {
        self.hop
    }

    /// Process one sample. `mags` is the per-bin magnitude gain
    /// (`frame / 2 + 1` bins). When `apply` is false the per-bin multiply is
    /// skipped — a delayed dry passthrough (identity). Output is delayed by
    /// `hop` samples.
    pub fn process(&mut self, sample: f32, mags: &[f32], apply: bool) -> f32 {
        if self.out_pos == 0 {
            self.out_buf.copy_within(self.hop..self.frame, 0);
            self.out_buf[self.hop..].fill(0.0);
            Self::process_frame(
                &self.in_buf,
                self.in_pos,
                &mut self.out_buf,
                mags,
                apply,
                &self.window,
                self.fft.as_ref(),
                self.ifft.as_ref(),
                &mut self.scratch_time,
                &mut self.scratch_freq,
                &mut self.scratch_fwd,
                &mut self.scratch_inv,
            );
        }
        self.in_buf[self.in_pos] = sample;
        let out = self.out_buf[self.out_pos];
        self.in_pos = (self.in_pos + 1) & (self.frame - 1);
        self.out_pos += 1;
        if self.out_pos >= self.hop {
            self.out_pos = 0;
        }
        out
    }

    /// STFT frame: window → FFT → per-bin magnitude multiply → IFFT →
    /// overlap-add with `1/n` normalisation (the correct gain for a
    /// Hann-windowed 50%-overlap reconstruction). `apply == false` skips the
    /// multiply (identity).
    #[allow(clippy::too_many_arguments)]
    fn process_frame(
        in_buf: &[f32],
        in_pos: usize,
        out_buf: &mut [f32],
        mags: &[f32],
        apply: bool,
        window: &[f32],
        fft: &dyn RealToComplex<f32>,
        ifft: &dyn ComplexToReal<f32>,
        scratch_time: &mut [f32],
        scratch_freq: &mut [Complex<f32>],
        scratch_fwd: &mut [Complex<f32>],
        scratch_inv: &mut [Complex<f32>],
    ) {
        let n = in_buf.len();
        let mask = n - 1;
        for i in 0..n {
            scratch_time[i] = in_buf[(in_pos + i) & mask] * window[i];
        }
        if fft
            .process_with_scratch(scratch_time, scratch_freq, scratch_fwd)
            .is_err()
        {
            return;
        }
        if apply {
            for (bin, &mag) in scratch_freq.iter_mut().zip(mags.iter()) {
                *bin *= mag;
            }
        }
        if ifft
            .process_with_scratch(scratch_freq, scratch_time, scratch_inv)
            .is_err()
        {
            return;
        }
        let scale = 1.0 / n as f32;
        for i in 0..n {
            out_buf[i] += scratch_time[i] * scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_is_half_the_frame() {
        assert_eq!(StftConvolver::new(4096).latency(), 2048);
        assert_eq!(StftConvolver::new(2048).latency(), 1024);
    }

    #[test]
    fn identity_passthrough_when_not_applied() {
        // apply = false → delayed dry passthrough.
        let frame = 2048;
        let mags = vec![0.0_f32; frame / 2 + 1]; // ignored when apply = false
        let mut c = StftConvolver::new(frame);
        let mut last = 0.0;
        for _ in 0..8 * frame {
            last = c.process(0.5, &mags, false);
        }
        assert!(
            (last - 0.5).abs() < 1e-3,
            "identity passthrough, got {last}"
        );
    }

    #[test]
    fn flat_magnitude_preserves_a_steady_signal() {
        // All-ones magnitude spectrum ≈ unity gain after the pipeline fills.
        let frame = 2048;
        let mags = vec![1.0_f32; frame / 2 + 1];
        let mut c = StftConvolver::new(frame);
        let mut last = 0.0;
        for _ in 0..16 * frame {
            last = c.process(0.5, &mags, true);
        }
        assert!(
            (last - 0.5).abs() < 5e-3,
            "flat magnitude ~unity, got {last}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let frame = 2048;
        let mags = vec![1.0_f32; frame / 2 + 1];
        let mut c = StftConvolver::new(frame);
        for _ in 0..4 * frame {
            c.process(0.9, &mags, true);
        }
        c.reset();
        // First sample after reset: output is the freshly-zeroed delay line.
        assert_eq!(c.process(0.0, &mags, true), 0.0);
    }
}
