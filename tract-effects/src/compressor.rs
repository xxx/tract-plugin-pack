//! Soft-knee peak compressor on the wet bus.
//!
//! Limits the series chain's output before the dry/wet mix — when a row's
//! amplitude MSEG or saturating effect pushes the chain hot, this catches
//! the peak so the user doesn't have to ride a master gain. Sidechain peak
//! detector → one-pole envelope (attack / release) → soft-knee gain
//! computer. Stereo-linked: one detector reads `max(|L|, |R|)` and one
//! gain applies to both channels.

/// Internal fixed timings — exposing them in the UI is YAGNI.
const ATTACK_MS: f32 = 5.0;
const RELEASE_MS: f32 = 50.0;
/// Soft-knee width, in dB around the threshold.
const KNEE_DB: f32 = 6.0;

/// The compressor's audio-thread state. Allocation-free; `set_sample_rate`
/// and `set_params` are called off-thread or at init.
pub struct Compressor {
    /// Linear envelope value (peak detector output, smoothed).
    env: f32,
    attack_coef: f32,
    release_coef: f32,
    /// Threshold as a linear amplitude.
    threshold_lin: f32,
    /// Ratio (≥ 1.0). 1.0 means no compression.
    ratio: f32,
    sample_rate: f32,
}

impl Compressor {
    pub fn new() -> Self {
        let mut c = Self {
            env: 0.0,
            attack_coef: 0.0,
            release_coef: 0.0,
            threshold_lin: db_to_lin(-6.0),
            ratio: 4.0,
            sample_rate: 48_000.0,
        };
        c.recompute_coefs();
        c
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recompute_coefs();
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }

    /// Update the user-facing parameters. `threshold_db` is in dBFS (negative
    /// for typical compression); `ratio` is ≥ 1.0.
    pub fn set_params(&mut self, threshold_db: f32, ratio: f32) {
        self.threshold_lin = db_to_lin(threshold_db);
        self.ratio = ratio.max(1.0);
    }

    /// Process one stereo sample, applying a single common gain to both
    /// channels (stereo-linked detection). Returns the compressed pair.
    pub fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left.abs().max(right.abs());
        // One-pole envelope: attack on rising, release on falling.
        let coef = if peak > self.env {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.env = peak + (self.env - peak) * coef;
        let gain = self.compute_gain(self.env);
        (left * gain, right * gain)
    }

    fn recompute_coefs(&mut self) {
        self.attack_coef = time_to_coef(ATTACK_MS, self.sample_rate);
        self.release_coef = time_to_coef(RELEASE_MS, self.sample_rate);
    }

    /// Soft-knee gain reduction for a given envelope value. Returns the
    /// linear gain to apply (≤ 1.0 above threshold, 1.0 below the knee).
    fn compute_gain(&self, env: f32) -> f32 {
        let env_db = lin_to_db(env);
        let thresh_db = lin_to_db(self.threshold_lin);
        let over_db = env_db - thresh_db;
        let half_knee = KNEE_DB * 0.5;
        // 1 - 1/ratio is the "compression slope" per dB above the knee.
        let slope = 1.0 - 1.0 / self.ratio;
        let gain_db = if over_db <= -half_knee {
            0.0
        } else if over_db >= half_knee {
            -over_db * slope
        } else {
            // Quadratic soft knee: gain transitions smoothly from 0 at the
            // bottom of the knee to -slope * (knee/2) at the top.
            let x = over_db + half_knee;
            -slope * x * x / (2.0 * KNEE_DB)
        };
        db_to_lin(gain_db)
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

fn db_to_lin(db: f32) -> f32 {
    // `db_to_linear_fast` rewrites 10^(dB/20) as exp(dB * LN_10 / 20),
    // which on f32 is ~2x faster than the libm powf path. Matters here
    // when this compressor is used as a per-track effect whose
    // Threshold is MSEG-modulated -- `set_params` then fires every
    // sample, and the powf would dominate. The master-bus call site
    // doesn't care (it runs off-thread on slider changes) but it's
    // happy to inherit the speedup.
    tract_dsp::db::db_to_linear_fast(db)
}

fn lin_to_db(lin: f32) -> f32 {
    tract_dsp::db::linear_to_db(lin.max(1e-12))
}

/// One-pole envelope coefficient for a given time constant (ms) at `sr` Hz.
/// `exp(-1 / (tau_samples))` so `env -> target` over `tau` samples.
fn time_to_coef(time_ms: f32, sr: f32) -> f32 {
    let tau_samples = time_ms * 0.001 * sr;
    if tau_samples > 0.0 {
        (-1.0 / tau_samples).exp()
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_passes_signal_below_threshold() {
        let mut c = Compressor::new();
        c.set_sample_rate(48_000.0);
        c.set_params(-6.0, 4.0);
        // Constant DC at -12 dB (well below -6 dB threshold) — gain should
        // stay at 1.0 after the envelope settles.
        let dc = db_to_lin(-12.0);
        for _ in 0..4800 {
            let _ = c.process_sample(dc, dc);
        }
        let (l, _) = c.process_sample(dc, dc);
        assert!((l - dc).abs() < 1e-3, "below threshold: in {dc}, out {l}");
    }

    #[test]
    fn compressor_reduces_gain_above_threshold() {
        let mut c = Compressor::new();
        c.set_sample_rate(48_000.0);
        c.set_params(-6.0, 4.0);
        // Constant DC at 0 dB (6 dB above threshold). After convergence,
        // gain reduction should be approximately -6 * (1 - 1/4) = -4.5 dB.
        let dc = 1.0_f32;
        let mut out = 0.0;
        for _ in 0..4800 {
            out = c.process_sample(dc, dc).0;
        }
        let reduction_db = lin_to_db(out);
        assert!(
            (reduction_db - (-4.5)).abs() < 0.5,
            "expected ~-4.5 dB GR, got {reduction_db} dB"
        );
    }

    #[test]
    fn compressor_ratio_one_is_a_passthrough() {
        let mut c = Compressor::new();
        c.set_sample_rate(48_000.0);
        c.set_params(-6.0, 1.0);
        // With ratio 1:1, the slope is zero — no gain reduction even above
        // threshold.
        for _ in 0..4800 {
            let _ = c.process_sample(1.0, 1.0);
        }
        let (l, _) = c.process_sample(1.0, 1.0);
        assert!(
            (l - 1.0).abs() < 1e-3,
            "ratio 1:1 should pass-through, got {l}"
        );
    }

    #[test]
    fn compressor_reset_clears_envelope() {
        let mut c = Compressor::new();
        c.set_sample_rate(48_000.0);
        // Pump the envelope up.
        for _ in 0..1000 {
            let _ = c.process_sample(1.0, 1.0);
        }
        c.reset();
        // Next sample should see env=0 → no reduction.
        let (l, _) = c.process_sample(0.1, 0.1);
        assert!((l - 0.1).abs() < 1e-3, "after reset env=0 → unity gain");
    }

    #[test]
    fn time_to_coef_yields_one_over_e_after_tau() {
        // After `tau` samples of decay from 1 toward 0, the value should be
        // approximately 1/e ≈ 0.368.
        let sr = 48_000.0;
        let tau_ms = 10.0;
        let coef = time_to_coef(tau_ms, sr);
        let tau_samples = (tau_ms * 0.001 * sr) as usize;
        let mut v = 1.0_f32;
        for _ in 0..tau_samples {
            v *= coef;
        }
        assert!((v - 1.0 / std::f32::consts::E).abs() < 0.01);
    }
}
