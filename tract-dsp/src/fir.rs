//! Time-domain FIR convolution: a double-buffered history ring + SIMD MAC.

use std::simd::{f32x16, num::SimdFloat};

/// A per-channel FIR convolution history: a double-buffered ring so the SIMD
/// MAC always reads a contiguous window with no per-chunk wraparound.
///
/// `push` and `mac` are deliberately separate so a caller can MAC one window
/// against multiple kernels (e.g. a crossfade between two kernels) after a
/// single `push`.
///
/// The silence flag is only re-armed by `reset`; a host loop should `reset`
/// after sustained input silence.
pub struct FirRing {
    /// Double-buffered history: `2 * cap` samples. Each sample is written at
    /// both `write_pos` and `write_pos + cap`, so a contiguous window ending
    /// at the newest sample is always a single readable slice.
    history: Vec<f32>,
    write_pos: usize,
    mask: usize,
    is_silent: bool,
}

impl FirRing {
    /// A ring sized for kernels up to `max_len` taps. Capacity is rounded up
    /// to a power of two.
    pub fn new(max_len: usize) -> Self {
        let cap = max_len.next_power_of_two();
        Self {
            history: vec![0.0; cap * 2],
            write_pos: 0,
            mask: cap - 1,
            is_silent: true,
        }
    }

    /// Zero the history and re-arm the silence flag.
    pub fn reset(&mut self) {
        self.history.iter_mut().for_each(|s| *s = 0.0);
        self.write_pos = 0;
        self.is_silent = true;
    }

    /// Push one input sample (double-buffered write). Clears the silence flag
    /// when `sample.abs() > 1e-6`.
    #[inline]
    pub fn push(&mut self, sample: f32) {
        if sample.abs() > 1e-6 {
            self.is_silent = false;
        }
        let cap = self.mask + 1;
        self.history[self.write_pos] = sample;
        self.history[self.write_pos + cap] = sample;
        self.write_pos = (self.write_pos + 1) & self.mask;
    }

    /// `true` iff only (near-)zero samples have been pushed since the last
    /// `reset` — the MAC output is then guaranteed zero and may be skipped.
    #[inline]
    pub fn is_silent(&self) -> bool {
        self.is_silent
    }

    /// `f32x16` multiply-accumulate of the most-recent `rev_taps.len()` samples
    /// against `rev_taps` — the kernel pre-reversed so the MAC reads it
    /// contiguously. `rev_taps.len()` must be a non-zero multiple of 16 and
    /// must not exceed the ring capacity.
    ///
    /// The window is oldest-first: `window[0]` is the oldest of the
    /// `rev_taps.len()` most-recent samples, `window[len-1]` the newest.
    #[inline]
    pub fn mac(&self, rev_taps: &[f32]) -> f32 {
        let len = rev_taps.len();
        let cap = self.mask + 1;
        let start = (self.write_pos + cap - len) & self.mask;
        let window = &self.history[start..start + len];
        let mut acc = f32x16::splat(0.0);
        for c in 0..len / 16 {
            let w = f32x16::from_slice(&window[c * 16..c * 16 + 16]);
            let k = f32x16::from_slice(&rev_taps[c * 16..c * 16 + 16]);
            acc += w * k;
        }
        acc.reduce_sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a length-16 reversed-tap kernel from explicit taps (oldest-first
    /// convolution taps `taps[0..]`; reversed so `mac` reads contiguously).
    fn rev16(taps: [f32; 16]) -> Vec<f32> {
        let mut r = [0.0_f32; 16];
        for j in 0..16 {
            r[j] = taps[16 - 1 - j];
        }
        r.to_vec()
    }

    #[test]
    fn unit_impulse_passes_input_through() {
        // taps[0] = 1.0 → y[n] = x[n].
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        let rev = rev16(taps);
        let mut ring = FirRing::new(16);
        for &s in &[0.5, -0.3, 0.9, 0.1] {
            ring.push(s);
            assert!((ring.mac(&rev) - s).abs() < 1e-6);
        }
    }

    #[test]
    fn asymmetric_two_tap_kernel() {
        // taps = [1.0, 0.5] → y[n] = 1.0*x[n] + 0.5*x[n-1]. Asymmetric on
        // purpose: a window/kernel reversal bug fails this.
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        taps[1] = 0.5;
        let rev = rev16(taps);
        let mut ring = FirRing::new(16);
        ring.push(1.0);
        assert!((ring.mac(&rev) - 1.0).abs() < 1e-6); // 1*1 + 0.5*0
        ring.push(1.0);
        assert!((ring.mac(&rev) - 1.5).abs() < 1e-6); // 1*1 + 0.5*1
        ring.push(0.0);
        assert!((ring.mac(&rev) - 0.5).abs() < 1e-6); // 1*0 + 0.5*1
    }

    #[test]
    fn silence_flag_arms_and_rearms() {
        let mut ring = FirRing::new(64);
        assert!(ring.is_silent());
        ring.push(0.0);
        assert!(ring.is_silent(), "a zero sample keeps silence");
        ring.push(0.5);
        assert!(!ring.is_silent(), "a non-zero sample clears silence");
        ring.reset();
        assert!(ring.is_silent(), "reset re-arms silence");
    }

    #[test]
    fn wraparound_keeps_window_contiguous() {
        // Push far more than capacity; the most-recent-16 MAC must still be
        // correct (double-buffer keeps the window a single slice).
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0; // newest sample only
        let rev = rev16(taps);
        let mut ring = FirRing::new(16); // cap 16
        for i in 0..1000 {
            ring.push(i as f32);
            assert!((ring.mac(&rev) - i as f32).abs() < 1e-6, "i={i}");
        }
    }
}
