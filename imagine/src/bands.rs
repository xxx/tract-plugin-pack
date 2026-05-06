//! Per-band processor: constant-power M/S Width gain + Stereoize injection.
//!
//! Width law (equal-power M/S pan):
//!   θ      = (width + 100) / 200 · π        // θ ∈ [0, π]
//!   M_gain = √2 · cos(θ / 2)                 // [√2, 1, 0]   at width = [-100, 0, +100]
//!   S_gain = √2 · sin(θ / 2)                 // [0,  1, √2]
//! Total power M_gain² + S_gain² = 2 (constant).
//!
//! Recover-Sides accumulator:
//!   S_removed = if width < 0 { S · (1 − S_gain) } else { 0 }
//! Width=0 already gives S_gain=1 → S_removed=0; positive widths leave Recover unaffected.

use crate::decorrelator::Decorrelator;
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StereoizeMode {
    ModeI,
    ModeII,
}

/// Per-band state. One per band (4 total in the plugin).
pub struct Band {
    /// Haas delay line for Mode I. Sized at construction for max τ at max sample rate.
    haas_buffer: Vec<f32>,
    haas_write_idx: usize,
    haas_delay_samples: usize,

    /// Decorrelator for Mode II.
    decorr: Decorrelator,
}

/// Width gains computed once per sample (or once per block when smoothed).
#[inline]
pub fn width_gains(width_param: f32) -> (f32, f32) {
    let theta = (width_param + 100.0) / 200.0 * PI;
    let m_gain = std::f32::consts::SQRT_2 * (theta * 0.5).cos();
    let s_gain = std::f32::consts::SQRT_2 * (theta * 0.5).sin();
    (m_gain, s_gain)
}

impl Band {
    /// `max_haas_ms` = upper bound for Haas τ in milliseconds (e.g. 25.0).
    /// `max_sample_rate` is used to size the Haas buffer.
    pub fn new(max_haas_ms: f32, max_sample_rate: f32) -> Self {
        let max_samples = (max_haas_ms * 0.001 * max_sample_rate).ceil() as usize + 16;
        Self {
            haas_buffer: vec![0.0; max_samples],
            haas_write_idx: 0,
            haas_delay_samples: 0,
            decorr: Decorrelator::new(max_sample_rate),
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32, haas_ms: f32) {
        self.haas_delay_samples =
            ((haas_ms * 0.001 * sr).round() as usize).min(self.haas_buffer.len() - 1);
        // Decorrelator is sample-rate-aware; rebuild it.
        self.decorr = Decorrelator::new(sr);
    }

    pub fn reset(&mut self) {
        self.haas_buffer.fill(0.0);
        self.haas_write_idx = 0;
        self.decorr.reset();
    }

    /// Process one (M, S) pair for this band. Returns (M_out, S_out, S_removed).
    ///
    /// `width_param` is the raw -100..+100 Width parameter for this band.
    /// `stereoize_amount` is 0..100 (will be normalized internally to 0..1).
    /// `mode` selects Mode I (Haas) or Mode II (decorrelator).
    #[inline]
    pub fn process(
        &mut self,
        m: f32,
        s: f32,
        width_param: f32,
        stereoize_amount: f32,
        mode: StereoizeMode,
    ) -> (f32, f32, f32) {
        let (m_gain, s_gain) = width_gains(width_param);
        let m_scaled = m * m_gain;
        let s_scaled = s * s_gain;

        let s_removed = if width_param < 0.0 {
            s * (1.0 - s_gain)
        } else {
            0.0
        };

        let amount_norm = (stereoize_amount * 0.01).clamp(0.0, 1.0);
        let inject = if amount_norm > 0.0 {
            match mode {
                StereoizeMode::ModeI => self.haas_process(m) * amount_norm,
                StereoizeMode::ModeII => self.decorr.process(m) * amount_norm,
            }
        } else {
            // Still advance Haas delay buffer to keep state coherent.
            self.haas_advance(m);
            0.0
        };

        (m_scaled, s_scaled + inject, s_removed)
    }

    #[inline]
    fn haas_process(&mut self, m_in: f32) -> f32 {
        let n = self.haas_buffer.len();
        let read_idx = (self.haas_write_idx + n - self.haas_delay_samples) % n;
        let delayed = self.haas_buffer[read_idx];
        self.haas_buffer[self.haas_write_idx] = m_in;
        self.haas_write_idx = if self.haas_write_idx + 1 == n {
            0
        } else {
            self.haas_write_idx + 1
        };
        delayed
    }

    #[inline]
    fn haas_advance(&mut self, m_in: f32) {
        let n = self.haas_buffer.len();
        self.haas_buffer[self.haas_write_idx] = m_in;
        self.haas_write_idx = if self.haas_write_idx + 1 == n {
            0
        } else {
            self.haas_write_idx + 1
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn width_unity_at_zero() {
        let (m, s) = width_gains(0.0);
        assert!((m - 1.0).abs() < 1e-6, "M_gain(0) = {m}");
        assert!((s - 1.0).abs() < 1e-6, "S_gain(0) = {s}");
    }

    #[test]
    fn width_minus100_zero_side() {
        let (m, s) = width_gains(-100.0);
        assert!(
            (m - std::f32::consts::SQRT_2).abs() < 1e-6,
            "M_gain(-100) = {m}"
        );
        assert!(s.abs() < 1e-6, "S_gain(-100) = {s}");
    }

    #[test]
    fn width_plus100_zero_mid() {
        let (m, s) = width_gains(100.0);
        assert!(m.abs() < 1e-6, "M_gain(+100) = {m}");
        assert!(
            (s - std::f32::consts::SQRT_2).abs() < 1e-6,
            "S_gain(+100) = {s}"
        );
    }

    #[test]
    fn width_constant_power() {
        for w in -100..=100 {
            let (mg, sg) = width_gains(w as f32);
            let total = mg * mg + sg * sg;
            assert!((total - 2.0).abs() < 1e-5, "w={w}: M²+S² = {total}");
        }
    }

    #[test]
    fn no_op_at_zero_width_no_stereoize() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 12.0);
        let (m, s, r) = b.process(0.5, 0.3, 0.0, 0.0, StereoizeMode::ModeI);
        assert!((m - 0.5).abs() < 1e-6);
        assert!((s - 0.3).abs() < 1e-6);
        assert!(r.abs() < 1e-6);
    }

    #[test]
    fn s_removed_zero_for_positive_width() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 12.0);
        for w in [10.0, 50.0, 100.0_f32] {
            let (_, _, r) = b.process(0.5, 0.3, w, 0.0, StereoizeMode::ModeI);
            assert!(r.abs() < 1e-6, "w={w}: S_removed = {r}");
        }
    }

    #[test]
    fn s_removed_for_negative_width() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 12.0);
        let s_in = 0.4;
        let w = -50.0;
        let (m_g, s_g) = width_gains(w);
        let (_, _, r) = b.process(0.0, s_in, w, 0.0, StereoizeMode::ModeI);
        let expected = s_in * (1.0 - s_g);
        assert!(
            (r - expected).abs() < 1e-6,
            "got {r}, expected {expected}, m_g={m_g}, s_g={s_g}"
        );
    }

    #[test]
    fn mode_i_delays_mid_into_side() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 1.0); // 1 ms delay = 48 samples
        let delay = 48;

        let m_pulse: Vec<f32> = std::iter::once(1.0)
            .chain(std::iter::repeat(0.0).take(100))
            .collect();

        // Width=0 (S unchanged), Stereoize=100%, Mode I.
        let mut s_outs = Vec::new();
        for &m in &m_pulse {
            let (_, s_o, _) = b.process(m, 0.0, 0.0, 100.0, StereoizeMode::ModeI);
            s_outs.push(s_o);
        }
        let max_idx = s_outs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .unwrap()
            .0;
        assert!(
            (max_idx as i32 - delay as i32).abs() <= 1,
            "max at {max_idx}, expected {delay}"
        );
    }

    #[test]
    fn mode_ii_decorrelates() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 12.0);

        let m_in = noise(8192);
        let mut s_inject = Vec::with_capacity(m_in.len());
        for &m in &m_in {
            // Width=0 so S_scaled=0 (we input S=0). Stereoize=100% Mode II.
            let (_, s_o, _) = b.process(m, 0.0, 0.0, 100.0, StereoizeMode::ModeII);
            s_inject.push(s_o);
        }

        let c = xcorr(&m_in[256..], &s_inject[256..]);
        assert!(c.abs() < 0.30, "Mode II xcorr {c:.3} should be < 0.30");
    }

    #[test]
    fn stereoize_amount_zero_no_injection() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 12.0);
        for &mode in &[StereoizeMode::ModeI, StereoizeMode::ModeII] {
            for i in 0..512 {
                let m = ((i as f32 * 0.1).sin()) * 0.5;
                let (_, s_o, _) = b.process(m, 0.0, 0.0, 0.0, mode);
                assert!(s_o.abs() < 1e-6, "i={i} mode={mode:?} s_o={s_o}");
            }
        }
    }

    #[test]
    fn haas_delay_sample_rate_correct() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0_f32] {
            let mut b = Band::new(25.0, 192_000.0);
            b.set_sample_rate(sr, 5.0); // 5 ms
            let expected = (5.0_f32 * 0.001 * sr).round() as usize;
            assert_eq!(b.haas_delay_samples, expected, "sr={sr}");
        }
    }

    #[test]
    fn process_returns_finite_for_extreme_inputs() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0, 12.0);
        for w in [-100.0, -50.0, 0.0, 50.0, 100.0_f32] {
            for amount in [0.0, 50.0, 100.0_f32] {
                for mode in [StereoizeMode::ModeI, StereoizeMode::ModeII] {
                    let (m_o, s_o, r) = b.process(1.0, -1.0, w, amount, mode);
                    assert!(
                        m_o.is_finite() && s_o.is_finite() && r.is_finite(),
                        "w={w} amount={amount} mode={mode:?}"
                    );
                }
            }
        }
    }
}
