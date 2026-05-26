use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// In-chain peak limiter -- a thin, zero-latency variant of the
/// standalone `tinylimit` plugin sized for a multosis effect slot.
/// Drops tinylimit's lookahead and dual-stage envelope; keeps the
/// soft-knee piecewise-quadratic gain computer (Giannoulis et al.,
/// 2012) and the single one-pole release envelope.
///
/// Stereo-linked: peak is computed across both channels and a
/// single gain is applied to both, preserving the stereo image
/// through limiting.
///
/// Per-channel state: just the envelope follower (one f32 shared
/// across channels). No allocations on the audio thread.
pub struct LimiterEffect {
    threshold_db: f32,
    release_ms: f32,
    ceiling_db: f32,
    knee_db: f32,
    sample_rate: f32,

    threshold_lin: f32,
    ceiling_lin: f32,
    alpha_release: f32,
    /// Smoothed gain reduction in dB (always <= 0). Tracked across
    /// samples so release fades cleanly instead of clicking.
    env_gr_db: f32,
}

impl LimiterEffect {
    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Threshold",
            min: -24.0,
            max: 0.0,
            default: -3.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 1, unit: "dB" },
        },
        ParamSpec {
            name: "Release",
            min: 1.0,
            max: 1_000.0,
            default: 50.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number { decimals: 0, unit: "ms" },
        },
        ParamSpec {
            name: "Ceiling",
            min: -6.0,
            max: 0.0,
            default: -0.3,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 1, unit: "dB" },
        },
        ParamSpec {
            name: "Knee",
            min: 0.0,
            max: 12.0,
            default: 3.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 1, unit: "dB" },
        },
    ];

    pub fn new() -> Self {
        let mut e = Self {
            threshold_db: Self::PARAMS[0].default,
            release_ms: Self::PARAMS[1].default,
            ceiling_db: Self::PARAMS[2].default,
            knee_db: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            threshold_lin: 1.0,
            ceiling_lin: 1.0,
            alpha_release: 0.0,
            env_gr_db: 0.0,
        };
        e.recompute();
        e
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.threshold_lin = 10.0_f32.powf(self.threshold_db / 20.0);
        self.ceiling_lin = 10.0_f32.powf(self.ceiling_db / 20.0);
        let time_s = (self.release_ms.max(0.1)) * 0.001;
        self.alpha_release = (-1.0 / (sr * time_s)).exp();
    }

    /// Soft-knee gain computer (Giannoulis et al. 2012). `input_db`
    /// is the input level relative to the threshold (positive ->
    /// over the threshold). Returns the gain reduction in dB,
    /// always <= 0.
    #[inline]
    fn gain_computer_db(input_db: f32, knee_db: f32) -> f32 {
        if knee_db < 0.01 {
            // Hard knee.
            if input_db <= 0.0 {
                0.0
            } else {
                -input_db
            }
        } else {
            let half = knee_db * 0.5;
            if input_db < -half {
                0.0
            } else if input_db <= half {
                // Quadratic transition through the knee.
                let t = input_db + half;
                -(t * t) / (2.0 * knee_db)
            } else {
                -input_db
            }
        }
    }
}

impl Default for LimiterEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for LimiterEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Stereo-linked peak detection.
        let peak = left.abs().max(right.abs());
        // Input level in dB relative to threshold. log10 only for
        // peaks that could be in the knee or above (skip otherwise
        // to avoid the cost when there's nothing to limit).
        let target_gr_db = if peak <= 1e-9 {
            0.0
        } else {
            let in_over_thresh = peak / self.threshold_lin;
            let in_db = 20.0 * in_over_thresh.log10();
            Self::gain_computer_db(in_db, self.knee_db)
        };
        // Envelope: attack is instantaneous (peak limiter), release
        // is a one-pole filter on the gain-reduction value. The
        // envelope tracks the MOST NEGATIVE GR (the deepest limit)
        // and releases back toward 0 dB.
        if target_gr_db <= self.env_gr_db {
            self.env_gr_db = target_gr_db;
        } else {
            self.env_gr_db = self.alpha_release * self.env_gr_db
                + (1.0 - self.alpha_release) * target_gr_db;
        }
        let gain = 10.0_f32.powf(self.env_gr_db / 20.0);
        let mut l = left * gain;
        let mut r = right * gain;
        // Safety clip at the ceiling: with zero lookahead, the
        // envelope's instantaneous attack still can't catch the
        // sample-of-attack overshoot. Clamping at the ceiling is
        // free insurance.
        l = l.clamp(-self.ceiling_lin, self.ceiling_lin);
        r = r.clamp(-self.ceiling_lin, self.ceiling_lin);
        (l, r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.env_gr_db = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.threshold_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.release_ms = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.ceiling_db = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.knee_db = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => return,
        }
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_declared() {
        let e = LimiterEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Threshold");
        assert_eq!(specs[1].name, "Release");
        assert_eq!(specs[2].name, "Ceiling");
        assert_eq!(specs[3].name, "Knee");
    }

    #[test]
    fn below_threshold_is_transparent() {
        let mut e = LimiterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -3.0);
        e.set_param(3, 0.0); // hard knee
        // Drive at -12 dBFS -- well below the -3 dB threshold.
        let x = 0.25;
        for _ in 0..1024 {
            let (l, r) = e.process_sample(x, -x);
            assert!((l - x).abs() < 1e-6);
            assert!((r - -x).abs() < 1e-6);
        }
    }

    #[test]
    fn loud_input_is_clamped_under_ceiling() {
        let mut e = LimiterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -12.0);
        e.set_param(2, -0.3);
        e.set_param(3, 0.0);
        // Hot input that would otherwise clip at 0 dBFS.
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.9, -0.9);
            assert!(l.abs() <= 1.0_f32.min(10.0_f32.powf(-0.3 / 20.0)) + 1e-5);
            assert!(r.abs() <= 1.0_f32.min(10.0_f32.powf(-0.3 / 20.0)) + 1e-5);
        }
    }

    #[test]
    fn stereo_link_preserves_image() {
        // A loud sample on L alone should still reduce R's gain by
        // the same amount (stereo-linked limiting). Verify by
        // checking the ratio L:R stays the same as input.
        let mut e = LimiterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -12.0);
        e.set_param(3, 0.0);
        // Warm the envelope.
        for _ in 0..512 {
            e.process_sample(0.8, 0.2);
        }
        let (l, r) = e.process_sample(0.8, 0.2);
        let ratio = l / r;
        assert!((ratio - 4.0).abs() < 0.05, "stereo balance broken: {l}/{r} = {ratio}");
    }

    #[test]
    fn release_fades_back_to_unity_gain() {
        let mut e = LimiterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -12.0);
        e.set_param(1, 50.0);
        e.set_param(3, 0.0);
        // Hammer it loud to push the envelope deep.
        for _ in 0..2048 {
            e.process_sample(0.9, 0.9);
        }
        // Then a long quiet tail.
        for _ in 0..48_000 {
            e.process_sample(0.0, 0.0);
        }
        // Final state should be near unity.
        assert!(
            e.env_gr_db.abs() < 0.1,
            "release didn't recover: env_gr_db={}",
            e.env_gr_db
        );
    }

    #[test]
    fn reset_clears_envelope() {
        let mut e = LimiterEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..512 {
            e.process_sample(0.9, 0.9);
        }
        e.reset();
        assert_eq!(e.env_gr_db, 0.0);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = LimiterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
