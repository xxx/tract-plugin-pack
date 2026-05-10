//! Schroeder/Gerzon all-pass decorrelator. Used by Stereoize Mode II.
//!
//! Each stage is a 1st-order all-pass with a fractional-feedback structure:
//!   y[n] = -g · x[n] + x[n - D] + g · y[n - D]
//! where D is a stage-specific delay (mutually prime) and g = 0.7 (stable, characteristic
//! decorrelation strength without obvious resonance).
//!
//! The user-exposed `stz_scale` parameter (0.5..2.0×) multiplies all six
//! delays. Buffers are oversized at construction for the max combined
//! (`max_sample_rate × MAX_SCALE`) so `set_scale` can change the
//! effective delay at runtime without reallocating.

const NUM_STAGES: usize = 6;
const FEEDBACK: f32 = 0.7;

/// Hard upper bound on the runtime `stz_scale` multiplier. Used to size
/// each stage's buffer at construction. The user-exposed parameter
/// caps at 2.0× — keeping a small bit of headroom would tempt future
/// range bumps without realloc, but the lock-in is fine here.
const MAX_SCALE: f32 = 2.0;

/// Prime delays at 48 kHz reference. `set_scale(sr, scale)` rescales
/// these by `scale × (sr / 48 kHz)`.
const PRIME_DELAYS_AT_48K: [usize; NUM_STAGES] = [41, 53, 67, 79, 97, 113];

pub struct Decorrelator {
    stages: [AllpassDelayStage; NUM_STAGES],
}

struct AllpassDelayStage {
    /// Backing buffer sized for max delay; effective delay is `delay`.
    buffer: Vec<f32>,
    write_idx: usize,
    /// Effective delay in samples; `<= buffer.len()`.
    delay: usize,
}

impl AllpassDelayStage {
    fn new(max_delay: usize) -> Self {
        let cap = max_delay.max(1);
        Self {
            buffer: vec![0.0; cap],
            write_idx: 0,
            delay: cap,
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_idx = 0;
    }

    fn set_delay(&mut self, delay: usize) {
        self.delay = delay.clamp(1, self.buffer.len());
        if self.write_idx >= self.buffer.len() {
            self.write_idx = 0;
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        // Standard 1st-order all-pass with delay-line feedback:
        //   v = buffer[read]   (this is x[n-D] + g·y[n-D] from prior write)
        //   y = -g·x + v
        //   buffer[write] = x + g·y
        let cap = self.buffer.len();
        let read_idx = (self.write_idx + cap - self.delay) % cap;
        let v = self.buffer[read_idx];
        let y = -FEEDBACK * x + v;
        self.buffer[self.write_idx] = x + FEEDBACK * y;
        self.write_idx = if self.write_idx + 1 == cap {
            0
        } else {
            self.write_idx + 1
        };
        y
    }
}

impl Decorrelator {
    /// Construct a Decorrelator sized for `sample_rate` at the upper
    /// end of the runtime delay-scale range, with effective delays
    /// initialised to 1.0× at that sample rate. Use `set_scale` to
    /// pick a live scale once the actual sample rate is known.
    pub fn new(sample_rate: f32) -> Self {
        let cap_scale = (sample_rate / 48_000.0) * MAX_SCALE;
        let stages = std::array::from_fn(|i| {
            let cap = (PRIME_DELAYS_AT_48K[i] as f32 * cap_scale).round().max(1.0) as usize;
            AllpassDelayStage::new(cap)
        });
        let mut s = Self { stages };
        // Default to the historical 1.0× scale so consumers that
        // never call `set_scale` still get the original behaviour.
        s.set_scale(sample_rate, 1.0);
        s
    }

    pub fn reset(&mut self) {
        for s in &mut self.stages {
            s.reset();
        }
    }

    /// Set the runtime delay scale. Effective delay for stage `i` is
    /// `PRIME_DELAYS_AT_48K[i] × (sample_rate / 48 kHz) × scale`,
    /// clamped to the buffer capacity that was sized at construction.
    pub fn set_scale(&mut self, sample_rate: f32, scale: f32) {
        let sr_factor = sample_rate / 48_000.0;
        let scale = scale.clamp(0.0, MAX_SCALE);
        for (i, s) in self.stages.iter_mut().enumerate() {
            let d = (PRIME_DELAYS_AT_48K[i] as f32 * sr_factor * scale)
                .round()
                .max(1.0) as usize;
            s.set_delay(d);
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut y = x;
        for s in &mut self.stages {
            y = s.process(y);
        }
        y
    }

    /// All-pass cascade has no group delay at DC (frequency-dependent phase only); reported as 0 for PDC purposes.
    pub fn latency_samples(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
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

    /// Pseudo-random white noise (deterministic).
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

    #[test]
    fn cascade_is_magnitude_flat() {
        // All-pass cascade preserves RMS to within numerical error.
        let mut d = Decorrelator::new(48000.0);
        let x = noise(8192);
        let y: Vec<f32> = x.iter().map(|&s| d.process(s)).collect();
        // Skip first 256 samples — startup transient through the longest delay (113).
        let rms_in = rms(&x[256..]);
        let rms_out = rms(&y[256..]);
        assert!(
            (rms_out / rms_in - 1.0).abs() < 0.10,
            "ratio {:.3}",
            rms_out / rms_in
        );
    }

    #[test]
    fn output_decorrelated_from_input() {
        // The whole point: cross-correlation should drop substantially.
        let mut d = Decorrelator::new(48000.0);
        let x = noise(8192);
        let y: Vec<f32> = x.iter().map(|&s| d.process(s)).collect();
        let c = xcorr(&x[256..], &y[256..]);
        assert!(c.abs() < 0.30, "xcorr {c:.3} should be < 0.30");
    }

    #[test]
    fn output_amplitude_bounded() {
        let mut d = Decorrelator::new(48000.0);
        for i in 0..100_000 {
            let x = ((i as f32 * 0.01).sin()).clamp(-1.0, 1.0);
            let y = d.process(x);
            assert!(y.abs() < 5.0, "y={y} at i={i}");
        }
    }

    #[test]
    fn sample_rate_scaling() {
        // Higher SR should produce equivalent decorrelation (xcorr stays low).
        for &sr in &[44100.0, 48000.0, 96000.0, 192000.0_f32] {
            let mut d = Decorrelator::new(sr);
            let n = (sr * 0.2) as usize; // 200 ms of noise
            let x = noise(n);
            let y: Vec<f32> = x.iter().map(|&s| d.process(s)).collect();
            let skip = (sr * 0.01) as usize;
            let c = xcorr(&x[skip..], &y[skip..]);
            assert!(c.abs() < 0.30, "sr={sr}: xcorr {c:.3}");
        }
    }

    #[test]
    fn peak_amplitude_bounded() {
        // Output peak amplitude is bounded — catches gross instability.
        let mut d = Decorrelator::new(48000.0);
        let x = noise(8192);
        let y: Vec<f32> = x.iter().map(|&s| d.process(s)).collect();

        let mean_power = y[256..].iter().map(|s| s * s).sum::<f32>() / (y.len() - 256) as f32;
        let peak_sq = y[256..].iter().map(|s| s * s).fold(0.0_f32, f32::max);
        // 6 dB = 4× power; require no single-sample peak above 16× mean (loose bound,
        // catches gross resonance).
        assert!(
            peak_sq < 16.0 * mean_power,
            "peak² {peak_sq:.4} vs mean² {mean_power:.4}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut d = Decorrelator::new(48000.0);
        for _ in 0..1024 {
            d.process(1.0);
        }
        d.reset();
        // After reset, the first sample's output should not depend on prior state.
        let y0 = d.process(0.0);
        assert!(y0.abs() < 1e-7, "post-reset DC: {y0}");
    }
}
