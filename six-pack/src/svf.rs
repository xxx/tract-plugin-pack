//! TPT state-variable filter: low-shelf, peak, high-shelf.
//!
//! Mix-form: peak output is `dry + (peak_gain − 1) · bandpass`. At gain=1.0
//! (= 0 dB), the bandpass term is multiplied by zero so the filter is
//! analytically unity. This is required for the boost-only diff-trick to work.

use std::f32::consts::PI;

#[derive(Default, Clone, Copy)]
pub struct Svf {
    // TPT integrator state
    ic1eq: f32,
    ic2eq: f32,
    // Cached coefficients
    g: f32,
    k: f32,
    a1: f32,
    a2: f32,
    // Filter-specific gain mixing: `(peak_gain − 1)` for peak; analogous for shelves.
    mix_bp: f32,   // bandpass mix coefficient
    mix_low: f32,  // lowpass mix coefficient (used by shelves)
    mix_high: f32, // highpass mix coefficient (used by shelves)
}

impl Svf {
    /// Configure as a peak filter at center freq with Q and gain (in dB).
    pub fn set_peak(&mut self, freq_hz: f32, q: f32, gain_db: f32, sample_rate: f32) {
        let g = (PI * freq_hz / sample_rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        self.g = g;
        self.k = k;
        self.a1 = a1;
        self.a2 = a2;
        let peak_gain = 10.0_f32.powf(gain_db / 20.0);
        self.mix_bp = peak_gain - 1.0;
        self.mix_low = 0.0;
        self.mix_high = 0.0;
    }

    /// Process one sample through the peak filter.
    /// Output = dry + (peak_gain − 1) · bandpass.
    pub fn process_peak(&mut self, x: f32) -> f32 {
        // Trapezoidal SVF integration
        let v3 = x - self.ic2eq;
        let v1 = self.a1 * self.ic1eq + self.a2 * v3;
        let v2 = self.ic2eq + self.g * v1;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        // bandpass = v1
        x + self.mix_bp * v1
    }

    /// Reset internal state.
    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At gain = 0 dB (linear gain ratio = 1.0), a peak filter must pass the
    /// input unchanged. This is load-bearing for the diff-trick.
    #[test]
    fn peak_unity_at_0db() {
        let sr = 48_000.0;
        let mut svf = Svf::default();
        svf.set_peak(1_000.0, 0.71, 0.0_f32, sr);
        let probe = [0.0f32, 0.5, -0.5, 1.0, -1.0, 0.123, 0.456, -0.789];
        for &x in &probe {
            let y = svf.process_peak(x);
            assert!(
                (y - x).abs() < 1e-7,
                "peak at 0 dB must be unity: input={} output={}",
                x,
                y
            );
        }
    }

    #[test]
    fn peak_stable_under_noise() {
        let sr = 48_000.0;
        for &freq in &[60.0, 1_000.0, 8_000.0] {
            for &q in &[0.1, 0.71, 5.0, 10.0] {
                for &gain in &[0.0, 9.0, 18.0] {
                    let mut svf = Svf::default();
                    svf.set_peak(freq, q, gain, sr);
                    let mut max_abs = 0.0f32;
                    let mut x = 0.5_f32;
                    for n in 0..(sr as usize) {
                        // pseudo-random walk in [-1, 1]
                        x = ((x * 9301.0 + 49297.0) % 233280.0) / 233280.0 * 2.0 - 1.0;
                        let y = svf.process_peak(x);
                        if !y.is_finite() {
                            panic!("non-finite at n={n} freq={freq} q={q} gain={gain}");
                        }
                        max_abs = max_abs.max(y.abs());
                    }
                    // Output bounded — pessimistic upper bound for safety:
                    assert!(
                        max_abs < 200.0,
                        "max_abs={max_abs} at freq={freq} q={q} gain={gain}"
                    );
                }
            }
        }
    }
}
