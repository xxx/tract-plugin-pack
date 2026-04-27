//! TPT state-variable filter: low-shelf, peak, high-shelf.
//!
//! Mix-form: peak output is `dry + (peak_gain − 1) · k · bandpass`, where
//! `k = 1/Q`. The `k` factor cancels the bandpass's `1/k` magnitude at center,
//! giving Q-independent peak magnitude (peak height = `peak_gain` exactly).
//! At gain = 0 dB the `(peak_gain − 1)` term is zero, so the filter is
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
        // At center frequency, bandpass magnitude = 1/k = Q. Scaling the mix by k
        // makes the peak height exactly `peak_gain` rather than `1 + (peak_gain-1)*Q`.
        // Preserves unity at 0 dB since (peak_gain − 1) = 0.
        self.mix_bp = (peak_gain - 1.0) * k;
        self.mix_low = 0.0;
        self.mix_high = 0.0;
    }

    /// Process one sample through the peak filter.
    /// Output = dry + (peak_gain − 1) · k · bandpass, where k = 1/Q.
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

    /// Configure as a low-shelf at corner freq with Q and gain (in dB).
    /// Mix-form: output = high + sqrt(A) · band + A · low, where A = 10^(gain/40).
    /// At gain = 0 dB → A = 1 → output = high + band + low = dry. (Unity.)
    pub fn set_low_shelf(&mut self, freq_hz: f32, q: f32, gain_db: f32, sample_rate: f32) {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let g = (PI * freq_hz / sample_rate).tan() / a.sqrt();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        self.g = g;
        self.k = k;
        self.a1 = a1;
        self.a2 = a2;
        // dry = low + band + high. We want gain on lowpass, sqrt-gain on bandpass.
        // Reformulate to the dry + (...) form so unity at A=1 is structural:
        //   output = high + sqrt(A) · band + A · low
        //          = (low + band + high) + (sqrt(A) − 1) · band + (A − 1) · low
        //          = dry + (sqrt(A) − 1) · band + (A − 1) · low
        self.mix_bp = a.sqrt() - 1.0;
        self.mix_low = a - 1.0;
        self.mix_high = 0.0;
    }

    /// Configure as a high-shelf at corner freq with Q and gain (in dB).
    /// Mix-form: output = A · high + sqrt(A) · band + low, where A = 10^(gain/40).
    /// At gain = 0 dB → A = 1 → output = high + band + low = dry. (Unity.)
    pub fn set_high_shelf(&mut self, freq_hz: f32, q: f32, gain_db: f32, sample_rate: f32) {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let g = (PI * freq_hz / sample_rate).tan() * a.sqrt();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        self.g = g;
        self.k = k;
        self.a1 = a1;
        self.a2 = a2;
        self.mix_bp = a.sqrt() - 1.0;
        self.mix_low = 0.0;
        self.mix_high = a - 1.0;
    }

    /// Process one sample through whichever shelf is currently configured.
    /// Output = dry + mix_bp · band + mix_low · low + mix_high · high.
    pub fn process_shelf(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2eq;
        let v1 = self.a1 * self.ic1eq + self.a2 * v3;
        let v2 = self.ic2eq + self.g * v1;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        let low = v2;
        let band = v1;
        let high = x - self.k * v1 - v2;
        x + self.mix_bp * band + self.mix_low * low + self.mix_high * high
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
    fn low_shelf_unity_at_0db() {
        let sr = 48_000.0;
        let mut svf = Svf::default();
        svf.set_low_shelf(80.0, 0.71, 0.0_f32, sr);
        let probe = [0.0f32, 0.5, -0.5, 1.0, -1.0, 0.123, 0.456];
        for &x in &probe {
            let y = svf.process_shelf(x);
            assert!(
                (y - x).abs() < 1e-7,
                "low-shelf at 0 dB must be unity: x={x} y={y}"
            );
        }
    }

    #[test]
    fn high_shelf_unity_at_0db() {
        let sr = 48_000.0;
        let mut svf = Svf::default();
        svf.set_high_shelf(8_000.0, 0.71, 0.0_f32, sr);
        let probe = [0.0f32, 0.5, -0.5, 1.0, -1.0, 0.123, 0.456];
        for &x in &probe {
            let y = svf.process_shelf(x);
            assert!(
                (y - x).abs() < 1e-7,
                "high-shelf at 0 dB must be unity: x={x} y={y}"
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

    #[test]
    fn peak_magnitude_at_center() {
        let sr = 48_000.0;
        let mut svf = Svf::default();
        svf.set_peak(1_000.0, 0.71, 9.0, sr);

        let n = (sr * 0.1) as usize;
        let two_pi = 2.0 * PI;
        let mut max_y = 0.0f32;
        for i in 0..n {
            let x = (two_pi * 1_000.0 * (i as f32) / sr).sin();
            let y = svf.process_peak(x);
            if i > n / 2 {
                max_y = max_y.max(y.abs());
            }
        }
        let expected = 10.0_f32.powf(9.0 / 20.0);
        let ratio_db = 20.0 * (max_y / expected).log10();
        assert!(
            ratio_db.abs() < 0.5,
            "peak gain at 1 kHz: measured={max_y} expected={expected} ratio_db={ratio_db}"
        );
    }
}
