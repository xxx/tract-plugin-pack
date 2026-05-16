//! Raw + Phaseless convolution engines, adapted from wavetable-filter's
//! proven DSP. A self-contained module — no plugin or GUI types.

use crate::kernel::{Kernel, MAX_KERNEL};
use std::simd::{f32x16, num::SimdFloat};

/// Per-channel time-domain convolution state: a double-buffered history ring
/// so the MAC loop gets a contiguous window. Adapted from wavetable-filter's
/// `FilterState`.
pub struct RawChannel {
    history: Vec<f32>,
    write_pos: usize,
    mask: usize,
    is_silent: bool,
}

impl RawChannel {
    /// A channel sized for kernels up to `MAX_KERNEL` taps.
    pub fn new() -> Self {
        let cap = MAX_KERNEL.next_power_of_two();
        Self {
            history: vec![0.0; cap * 2],
            write_pos: 0,
            mask: cap - 1,
            is_silent: true,
        }
    }

    /// Zero the history.
    pub fn reset(&mut self) {
        self.history.iter_mut().for_each(|s| *s = 0.0);
        self.write_pos = 0;
        self.is_silent = true;
    }

    /// Push one input sample (double-buffered write).
    fn push(&mut self, sample: f32) {
        if sample.abs() > 1e-6 {
            self.is_silent = false;
        }
        let cap = self.mask + 1;
        self.history[self.write_pos] = sample;
        self.history[self.write_pos + cap] = sample;
        self.write_pos = (self.write_pos + 1) & self.mask;
    }

    /// Process one sample through `kernel`; returns the filtered output.
    /// All-zero kernel -> the input is returned unchanged (dry passthrough).
    pub fn process(&mut self, sample: f32, kernel: &Kernel) -> f32 {
        self.push(sample);
        if kernel.is_zero {
            return sample; // dry passthrough — see the miff spec
        }
        if self.is_silent {
            return 0.0; // silence fast-path: history is all zero
        }
        let len = kernel.len;
        let cap = self.mask + 1;
        let start = (self.write_pos + cap - len) & self.mask;
        // `window` is oldest-first: window[0] = x[n-len+1], window[len-1] = x[n].
        // Convolution: y[n] = sum_k taps[k]*x[n-k]
        //            = taps[0]*window[len-1] + taps[1]*window[len-2] + ...
        // So window[c*16+j] pairs with taps[len-1-(c*16+j)].
        let window = &self.history[start..start + len];
        let mut acc = f32x16::splat(0.0);
        let chunks = len / 16;
        for c in 0..chunks {
            let w = f32x16::from_slice(&window[c * 16..c * 16 + 16]);
            let mut kchunk = [0.0_f32; 16];
            for (j, kv) in kchunk.iter_mut().enumerate() {
                *kv = kernel.taps[len - 1 - (c * 16 + j)];
            }
            let k = f32x16::from_slice(&kchunk);
            acc += w * k;
        }
        acc.reduce_sum()
    }
}

impl Default for RawChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::bake;
    use tiny_skia_widgets::mseg::MsegData;

    /// Build a `Kernel` directly from explicit taps, bypassing the curve bake.
    fn kernel_from_taps(taps: &[f32]) -> Kernel {
        let mut k = Kernel::default();
        k.is_zero = false;
        k.len = taps.len();
        k.taps[..taps.len()].copy_from_slice(taps);
        k
    }

    #[test]
    fn unit_impulse_kernel_passes_audio_through() {
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        let k = kernel_from_taps(&taps);
        let mut ch = RawChannel::new();
        let input = [0.5, -0.3, 0.9, 0.1];
        for &s in &input {
            let out = ch.process(s, &k);
            assert!((out - s).abs() < 1e-6, "impulse kernel must pass {s} through, got {out}");
        }
    }

    #[test]
    fn zero_kernel_is_dry_passthrough() {
        let k = Kernel::default(); // is_zero
        let mut ch = RawChannel::new();
        for &s in &[0.7, -0.4, 0.2] {
            assert!((ch.process(s, &k) - s).abs() < 1e-6, "zero kernel must pass through");
        }
    }

    #[test]
    fn silence_fast_path_outputs_exact_zero() {
        let k = bake(&MsegData::default(), 256); // non-zero kernel
        let mut ch = RawChannel::new();
        for _ in 0..10 {
            assert_eq!(ch.process(0.0, &k), 0.0);
        }
    }

    #[test]
    fn known_kernel_yields_known_output() {
        // ASYMMETRIC 2-tap kernel h = [1.0, 0.5] padded to 16. The FIR
        // convolution is y[n] = taps[0]*x[n] + taps[1]*x[n-1]
        //                     = 1.0*x[n] + 0.5*x[n-1].
        // Asymmetric on purpose: a kernel/window reversal bug (computing
        // 0.5*x[n] + 1.0*x[n-1] instead) fails this test.
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        taps[1] = 0.5;
        let k = kernel_from_taps(&taps);
        let mut ch = RawChannel::new();
        assert!((ch.process(1.0, &k) - 1.0).abs() < 1e-6); // 1*1 + 0.5*0
        assert!((ch.process(1.0, &k) - 1.5).abs() < 1e-6); // 1*1 + 0.5*1
        assert!((ch.process(0.0, &k) - 0.5).abs() < 1e-6); // 1*0 + 0.5*1
    }
}
