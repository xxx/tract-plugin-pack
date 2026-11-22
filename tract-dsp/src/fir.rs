//! Time-domain FIR convolution: a double-buffered history ring + SIMD MAC.

use fearless_simd::{Level, dispatch, f32x16, prelude::*};

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
    simd_level: Level,
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
            simd_level: Level::new(),
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

    /// Multiply-accumulate the most recent samples against oldest-first taps.
    /// Full groups of 16 use SIMD and any remaining taps use scalar arithmetic.
    /// The tap count must fit within the ring capacity.
    #[inline]
    pub fn mac(&self, rev_taps: &[f32]) -> f32 {
        let len = rev_taps.len();
        let cap = self.mask + 1;
        let start = (self.write_pos + cap - len) & self.mask;
        let window = &self.history[start..start + len];
        let (sample_chunks, sample_tail) = window.as_chunks::<16>();
        let (tap_chunks, tap_tail) = rev_taps.as_chunks::<16>();
        dispatch!(self.simd_level, simd => {
            let mut acc = f32x16::splat(simd, 0.0);
            for (samples, taps) in sample_chunks.iter().zip(tap_chunks) {
                let w = f32x16::from_slice(simd, samples);
                let k = f32x16::from_slice(simd, taps);
                acc += w * k;
            }
            let mut sum = acc.reduce_sum();
            for (&sample, &tap) in sample_tail.iter().zip(tap_tail) {
                sum += sample * tap;
            }
            sum
        })
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
    fn silence_threshold_includes_both_signed_endpoints() {
        let threshold = 1e-6_f32;
        for sample in [
            0.0,
            threshold / 2.0,
            -threshold / 2.0,
            threshold,
            -threshold,
        ] {
            let mut ring = FirRing::new(16);
            ring.push(sample);
            assert!(ring.is_silent(), "sample {sample} should remain silent");
        }
        let above = f32::from_bits(threshold.to_bits() + 1);
        for sample in [above, -above] {
            let mut ring = FirRing::new(16);
            ring.push(sample);
            assert!(!ring.is_silent(), "sample {sample} should clear silence");
        }
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

    #[test]
    fn mac_matches_scalar_across_tap_lengths_and_wraparound() {
        let taps: Vec<f32> = (0..65).map(|i| (i as f32 * 0.17).cos() * 0.1).collect();
        let kernels = [1, 15, 16, 17, 48, 65]
            .map(|len| taps[..len].iter().rev().copied().collect::<Vec<_>>());
        for simd_level in [Level::baseline(), Level::new()] {
            let mut ring = FirRing::new(taps.len());
            ring.simd_level = simd_level;
            let mut history = [0.0; 65];
            for i in 0..300 {
                let sample = (i as f32 * 0.13).cos();
                history.rotate_right(1);
                history[0] = sample;
                ring.push(sample);
                for reversed in &kernels {
                    let len = reversed.len();
                    let expected: f32 = taps[..len]
                        .iter()
                        .zip(&history)
                        .map(|(tap, x)| tap * x)
                        .sum();
                    let actual = ring.mac(reversed);
                    assert!((actual - expected).abs() < 1e-6, "sample {i}, {len} taps");
                }
            }
        }
    }

    #[test]
    fn aligned_mac_matches_legacy_accumulation_bits() {
        for (capacity, tap_count) in [(4096, 128), (2048, 2048)] {
            let taps: Vec<f32> = (0..tap_count).map(|i| (i as f32 * 0.17).cos()).collect();
            let input: Vec<f32> = (0..capacity + 16)
                .map(|i| (i as f32 * 0.13).sin())
                .collect();
            for simd_level in [Level::baseline(), Level::new()] {
                let mut ring = FirRing::new(capacity);
                ring.simd_level = simd_level;
                for (i, &sample) in input.iter().enumerate() {
                    ring.push(sample);
                    let end = i + 1;
                    if end < capacity {
                        continue;
                    }

                    // Model the original SIMD lane order with scalar accumulators.
                    let mut lanes = [0.0; 16];
                    for (i, (&sample, &tap)) in
                        input[end - tap_count..end].iter().zip(&taps).enumerate()
                    {
                        lanes[i % 16] += sample * tap;
                    }
                    let expected = dispatch!(simd_level, simd => {
                        f32x16::from_slice(simd, &lanes).reduce_sum()
                    });
                    assert_eq!(
                        ring.mac(&taps).to_bits(),
                        expected.to_bits(),
                        "{tap_count} taps, sample {i}, {simd_level:?}"
                    );
                }
            }
        }
    }
}
