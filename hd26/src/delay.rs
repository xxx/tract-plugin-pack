//! Single-channel modulated fractional delay line with 4-point cubic
//! (Catmull-Rom) interpolation. Pre-allocated ring buffer — no allocation
//! after construction. The core new DSP primitive for HD26's Hyper voices and
//! Dimension taps.

pub struct DelayLine {
    buf: Vec<f32>,
    write: usize,
    /// `len - 1`. The buffer length is always a power of two, so ring indexing
    /// is a bitwise `& mask` instead of a (runtime, non-pow2) integer `% len` —
    /// the modulo was ~20% of the per-sample cost in profiling.
    mask: usize,
}

impl DelayLine {
    /// Allocate a delay line holding up to `max_len` samples of history. The
    /// buffer is rounded up to the next power of two (minimum 4) so the read
    /// index is a bitmask rather than an integer modulo; cubic interpolation
    /// needs four taps, hence the floor of 4.
    pub fn new(max_len: usize) -> Self {
        let len = max_len.max(4).next_power_of_two();
        Self {
            buf: vec![0.0; len],
            write: 0,
            mask: len - 1,
        }
    }

    /// Clear all history.
    pub fn reset(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }

    /// Buffer capacity in samples.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Push one sample, advancing the write head. After this call the pushed
    /// sample is the most-recent (age 1) sample.
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) & self.mask;
    }

    /// Read the sample `delay` samples in the past (fractional), using 4-point
    /// cubic (Catmull-Rom) interpolation. `delay` is clamped to
    /// `[2.0, capacity - 2]` so all four taps remain in range. Reproduces DC
    /// and linear ramps exactly.
    #[inline]
    pub fn read_cubic(&self, delay: f32) -> f32 {
        let len = self.buf.len();
        let d = delay.clamp(2.0, (len - 2) as f32);
        let i0 = d.floor() as usize;
        let frac = d - i0 as f32;

        // tap(age): sample written `age` samples ago (age 1 = most recent).
        // `len` is a power of two, so `& self.mask` == `% len`.
        let tap = |age: usize| self.buf[(self.write + len - age) & self.mask];

        let ym1 = tap(i0 - 1);
        let y0 = tap(i0);
        let y1 = tap(i0 + 1);
        let y2 = tap(i0 + 2);

        // Catmull-Rom cubic between y0 (frac=0) and y1 (frac=1).
        let c0 = y0;
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        ((c3 * frac + c2) * frac + c1) * frac + c0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(dl: &mut DelayLine, seq: &[f32]) {
        for &x in seq {
            dl.write(x);
        }
    }

    #[test]
    fn integer_delay_returns_exact_past_sample() {
        let mut dl = DelayLine::new(64);
        // Write 0,1,2,...,19. Most recent (age 1) is 19.0.
        let seq: Vec<f32> = (0..20).map(|i| i as f32).collect();
        fill(&mut dl, &seq);
        // age 5 -> value written 5 samples ago = 19 - 4 = 15.0.
        let v = dl.read_cubic(5.0);
        assert!((v - 15.0).abs() < 1e-3, "got {v}");
    }

    #[test]
    fn dc_is_preserved() {
        let mut dl = DelayLine::new(64);
        fill(&mut dl, &[1.0; 64]);
        for d in [2.0, 2.5, 7.3, 30.9, 61.0] {
            let v = dl.read_cubic(d);
            assert!((v - 1.0).abs() < 1e-5, "delay {d} gave {v}");
        }
    }

    #[test]
    fn linear_ramp_is_reproduced() {
        // Catmull-Rom reproduces linear functions exactly.
        let mut dl = DelayLine::new(128);
        let seq: Vec<f32> = (0..100).map(|i| i as f32).collect();
        fill(&mut dl, &seq);
        // Most recent (age 1) is 99.0. age d -> 99 - (d-1) = 100 - d.
        let d = 10.5;
        let v = dl.read_cubic(d);
        assert!((v - (100.0 - d)).abs() < 1e-3, "got {v}");
    }

    #[test]
    fn clamps_without_panic() {
        let mut dl = DelayLine::new(16);
        fill(&mut dl, &[0.5; 16]);
        let _ = dl.read_cubic(0.0);
        let _ = dl.read_cubic(1.0);
        let _ = dl.read_cubic(1_000.0);
        let _ = dl.read_cubic(-5.0);
    }

    #[test]
    fn no_nan_on_varied_input() {
        let mut dl = DelayLine::new(256);
        for n in 0..2000 {
            let x = (0.31 * n as f32).sin() * 0.5;
            dl.write(x);
            let v = dl.read_cubic(40.0 + 20.0 * (0.01 * n as f32).sin());
            assert!(v.is_finite(), "non-finite at {n}");
        }
    }

    #[test]
    fn reset_clears_history() {
        let mut dl = DelayLine::new(32);
        fill(&mut dl, &[1.0; 32]);
        dl.reset();
        assert!(dl.read_cubic(5.0).abs() < 1e-6);
    }
}
