//! Per-band processor: M/S Width gain + Stereoize injection.
//!
//! Width law (Ozone-style — scale Side, leave Mid alone):
//!   W      = (width + 100) / 100              // W ∈ [0, 2]
//!   M_gain = 1                                 // mid untouched
//!   S_gain = W                                 // [0, 1, 2] at width = [-100, 0, +100]
//!
//! At Width = -100 the side is fully muted (mono); at +100 the side is
//! doubled (full stereo widening). The mid stays at unity always, so a
//! mono signal stays mono regardless of Width — matches user intuition
//! and standard imager behavior. Note: at Width=+100 with strongly stereo
//! content the output may exceed 0 dBFS — the user is expected to manage
//! headroom with downstream gain.
//!
//! Recover-Sides accumulator:
//!   S_removed = if width < 0 { S · (1 − S_gain) } else { 0 }
//! Width=0 already gives S_gain=1 → S_removed=0; positive widths leave Recover unaffected
//! (no side energy was lost; in fact more was added).

use crate::decorrelator::Decorrelator;

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
    /// Cached sample rate used to convert `stz_ms` → samples on each
    /// process call. Updated by `set_sample_rate`.
    sample_rate: f32,

    /// Decorrelator for Mode II.
    decorr: Decorrelator,
}

/// Width gains computed once per sample (or once per block when smoothed).
/// Returns `(M_gain, S_gain)`. M is always 1.0; S scales linearly from 0 (at
/// width = -100) through 1 (at width = 0, unity) to 2 (at width = +100).
#[inline]
pub fn width_gains(width_param: f32) -> (f32, f32) {
    let w = (width_param + 100.0) / 100.0;
    (1.0, w.clamp(0.0, 2.0))
}

impl Band {
    /// `max_haas_ms` = upper bound for the Haas delay buffer in ms
    /// (sized at construction; needs headroom above the user-exposed
    /// max so the dynamic `stz_ms` parameter can sweep without
    /// reallocating).
    /// `max_sample_rate` is used to size the Haas buffer.
    pub fn new(max_haas_ms: f32, max_sample_rate: f32) -> Self {
        let max_samples = (max_haas_ms * 0.001 * max_sample_rate).ceil() as usize + 16;
        Self {
            haas_buffer: vec![0.0; max_samples],
            haas_write_idx: 0,
            haas_delay_samples: 0,
            sample_rate: max_sample_rate,
            decorr: Decorrelator::new(max_sample_rate),
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        // Decorrelator is sample-rate-aware; rebuild it.
        self.decorr = Decorrelator::new(sr);
    }

    pub fn reset(&mut self) {
        self.haas_buffer.fill(0.0);
        self.haas_write_idx = 0;
        self.haas_delay_samples = 0;
        self.decorr.reset();
    }

    /// Process one (M, S) pair for this band. Returns (M_out, S_out, S_removed).
    ///
    /// `width_param` is the raw -100..+100 Width parameter for this band.
    /// `stz_ms` is the Mode I Haas delay (1..20 ms); only used when
    /// `stz_on` is true and `mode` is Mode I.
    /// `stz_scale` is the Mode II decorrelator delay scale (0.5..2.0×);
    /// only used when `stz_on` is true and `mode` is Mode II.
    /// `stz_on` gates the stereoize stage entirely — when false, no
    /// haas/decorrelator contribution is added to S.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        m: f32,
        s: f32,
        width_param: f32,
        stz_ms: f32,
        stz_scale: f32,
        stz_on: bool,
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

        let inject = if stz_on {
            match mode {
                StereoizeMode::ModeI => {
                    // Update the Haas delay tap to the requested ms.
                    // `stz_ms` is smoothed at the param level so this
                    // changes once per sample by at most a few samples.
                    let target = ((stz_ms * 0.001 * self.sample_rate).round() as usize)
                        .clamp(1, self.haas_buffer.len() - 1);
                    self.haas_delay_samples = target;
                    self.haas_process(m)
                }
                StereoizeMode::ModeII => {
                    // Update the decorrelator delays for the live scale
                    // before processing. Cheap — 6 stages, integer ops.
                    self.decorr.set_scale(self.sample_rate, stz_scale);
                    self.decorr.process(m)
                }
            }
        } else {
            // Still advance Haas delay buffer to keep state coherent so
            // toggling on doesn't expose a stale tail.
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
        assert!((m - 1.0).abs() < 1e-6, "M_gain(-100) = {m}");
        assert!(s.abs() < 1e-6, "S_gain(-100) = {s}");
    }

    #[test]
    fn width_plus100_doubles_side() {
        let (m, s) = width_gains(100.0);
        assert!((m - 1.0).abs() < 1e-6, "M_gain(+100) = {m}");
        assert!((s - 2.0).abs() < 1e-6, "S_gain(+100) = {s}");
    }

    #[test]
    fn width_mid_is_constant_unity() {
        for w in -100..=100 {
            let (mg, _) = width_gains(w as f32);
            assert!((mg - 1.0).abs() < 1e-6, "w={w}: M_gain = {mg}");
        }
    }

    #[test]
    fn width_side_scales_linearly() {
        // S_gain at width w should equal (w + 100) / 100, clamped to [0, 2].
        for w in [-100, -50, 0, 50, 100] {
            let (_, sg) = width_gains(w as f32);
            let expected = (w as f32 + 100.0) / 100.0;
            assert!(
                (sg - expected).abs() < 1e-6,
                "w={w}: S_gain={sg} expected={expected}"
            );
        }
    }

    #[test]
    fn no_op_at_zero_width_no_stereoize() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0);
        let (m, s, r) = b.process(0.5, 0.3, 0.0, 6.0, 1.0, false, StereoizeMode::ModeI);
        assert!((m - 0.5).abs() < 1e-6);
        assert!((s - 0.3).abs() < 1e-6);
        assert!(r.abs() < 1e-6);
    }

    #[test]
    fn s_removed_zero_for_positive_width() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0);
        for w in [10.0, 50.0, 100.0_f32] {
            let (_, _, r) = b.process(0.5, 0.3, w, 6.0, 1.0, false, StereoizeMode::ModeI);
            assert!(r.abs() < 1e-6, "w={w}: S_removed = {r}");
        }
    }

    #[test]
    fn s_removed_for_negative_width() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0);
        let s_in = 0.4;
        let w = -50.0;
        let (m_g, s_g) = width_gains(w);
        let (_, _, r) = b.process(0.0, s_in, w, 6.0, 1.0, false, StereoizeMode::ModeI);
        let expected = s_in * (1.0 - s_g);
        assert!(
            (r - expected).abs() < 1e-6,
            "got {r}, expected {expected}, m_g={m_g}, s_g={s_g}"
        );
    }

    #[test]
    fn mode_i_delays_mid_into_side() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0);
        let delay = 48; // 1 ms at 48 kHz

        let m_pulse: Vec<f32> = std::iter::once(1.0)
            .chain(std::iter::repeat(0.0).take(100))
            .collect();

        // Width=0 (S unchanged), Stereoize on at 1 ms, Mode I.
        let mut s_outs = Vec::new();
        for &m in &m_pulse {
            let (_, s_o, _) = b.process(m, 0.0, 0.0, 1.0, 1.0, true, StereoizeMode::ModeI);
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
        b.set_sample_rate(48_000.0);

        let m_in = noise(8192);
        let mut s_inject = Vec::with_capacity(m_in.len());
        for &m in &m_in {
            // Width=0 so S_scaled=0 (we input S=0). Stereoize on, Mode II
            // (ms is irrelevant for the decorrelator).
            let (_, s_o, _) = b.process(m, 0.0, 0.0, 6.0, 1.0, true, StereoizeMode::ModeII);
            s_inject.push(s_o);
        }

        let c = xcorr(&m_in[256..], &s_inject[256..]);
        assert!(c.abs() < 0.30, "Mode II xcorr {c:.3} should be < 0.30");
    }

    #[test]
    fn stereoize_off_no_injection() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0);
        for &mode in &[StereoizeMode::ModeI, StereoizeMode::ModeII] {
            for i in 0..512 {
                let m = ((i as f32 * 0.1).sin()) * 0.5;
                // stz_on = false: no injection regardless of mode or ms.
                let (_, s_o, _) = b.process(m, 0.0, 0.0, 6.0, 1.0, false, mode);
                assert!(s_o.abs() < 1e-6, "i={i} mode={mode:?} s_o={s_o}");
            }
        }
    }

    #[test]
    fn haas_delay_sample_rate_correct() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0_f32] {
            let mut b = Band::new(25.0, 192_000.0);
            b.set_sample_rate(sr);
            // The Haas delay is set inside `process` from the live ms
            // value; advance one sample with stz_on to apply it.
            let _ = b.process(0.0, 0.0, 0.0, 5.0, 1.0, true, StereoizeMode::ModeI);
            let expected = (5.0_f32 * 0.001 * sr).round() as usize;
            assert_eq!(b.haas_delay_samples, expected, "sr={sr}");
        }
    }

    #[test]
    fn process_returns_finite_for_extreme_inputs() {
        let mut b = Band::new(25.0, 192_000.0);
        b.set_sample_rate(48_000.0);
        for w in [-100.0, -50.0, 0.0, 50.0, 100.0_f32] {
            for ms in [1.0, 6.0, 20.0_f32] {
                for stz_on in [false, true] {
                    for mode in [StereoizeMode::ModeI, StereoizeMode::ModeII] {
                        for scale in [0.5, 1.0, 2.0_f32] {
                            let (m_o, s_o, r) =
                                b.process(1.0, -1.0, w, ms, scale, stz_on, mode);
                            assert!(
                                m_o.is_finite() && s_o.is_finite() && r.is_finite(),
                                "w={w} ms={ms} scale={scale} on={stz_on} mode={mode:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
