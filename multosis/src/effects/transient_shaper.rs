use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Transient shaper in the SPL Transient Designer lineage. Two
/// peak-tracking envelope followers run in parallel:
/// * `env_fast` -- ~1 ms attack, ~30 ms release. Reaches the
///   instantaneous peak of every transient quickly.
/// * `env_slow` -- ~30 ms attack, ~30 ms release. Lags transient
///   onsets, so during a transient `env_fast > env_slow`; during
///   sustained portions they converge.
///
/// The ratio `env_fast / env_slow` (in dB) is the "attack-ness"
/// of the current signal. We map it onto two gain offsets:
/// * **Attack** (-100..+100%) -- scales the positive part of the
///   ratio. Positive Attack boosts transients, negative Attack
///   softens them.
/// * **Sustain** (-100..+100%) -- scales the inverse, applied to
///   the portion where the envelopes converge. Positive Sustain
///   makes the tail louder; negative pulls it down.
///
/// Both ranges map to +/- ~12 dB at the extremes -- aggressive
/// enough to do real shaping without runaway distortion.
///
/// Stereo-linked: one envelope pair driven by the peak across
/// both channels, single common gain applied to both.
///
/// Per-channel state: 2 envelope-follower states (shared across
/// channels). No allocations on the audio thread.
pub struct TransientShaperEffect {
    attack_pct: f32,
    sustain_pct: f32,
    sample_rate: f32,

    /// One-pole coefficient for the fast envelope's attack stage.
    alpha_fast_a: f32,
    /// One-pole coefficient for the fast envelope's release stage.
    alpha_fast_r: f32,
    alpha_slow_a: f32,
    alpha_slow_r: f32,

    env_fast: f32,
    env_slow: f32,
}

impl TransientShaperEffect {
    const FAST_ATTACK_MS: f32 = 1.0;
    const FAST_RELEASE_MS: f32 = 30.0;
    const SLOW_ATTACK_MS: f32 = 30.0;
    const SLOW_RELEASE_MS: f32 = 30.0;

    /// Maximum gain offset (dB) Attack / Sustain at +/- 100 %.
    const MAX_DB: f32 = 12.0;

    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Attack",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Sustain",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        let mut e = Self {
            attack_pct: Self::PARAMS[0].default,
            sustain_pct: Self::PARAMS[1].default,
            sample_rate: 48_000.0,
            alpha_fast_a: 0.0,
            alpha_fast_r: 0.0,
            alpha_slow_a: 0.0,
            alpha_slow_r: 0.0,
            env_fast: 0.0,
            env_slow: 0.0,
        };
        e.recompute();
        e
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.alpha_fast_a = Self::alpha(sr, Self::FAST_ATTACK_MS);
        self.alpha_fast_r = Self::alpha(sr, Self::FAST_RELEASE_MS);
        self.alpha_slow_a = Self::alpha(sr, Self::SLOW_ATTACK_MS);
        self.alpha_slow_r = Self::alpha(sr, Self::SLOW_RELEASE_MS);
    }

    #[inline]
    fn alpha(sr: f32, time_ms: f32) -> f32 {
        (-1.0 / (sr * time_ms * 0.001)).exp()
    }

    /// One-pole peak follower with asymmetric attack / release.
    #[inline]
    fn follower_step(env: &mut f32, peak: f32, alpha_a: f32, alpha_r: f32) -> f32 {
        let alpha = if peak > *env { alpha_a } else { alpha_r };
        *env = alpha * *env + (1.0 - alpha) * peak;
        *env
    }
}

impl Default for TransientShaperEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for TransientShaperEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left.abs().max(right.abs());
        let fast = Self::follower_step(
            &mut self.env_fast,
            peak,
            self.alpha_fast_a,
            self.alpha_fast_r,
        );
        let slow = Self::follower_step(
            &mut self.env_slow,
            peak,
            self.alpha_slow_a,
            self.alpha_slow_r,
        );

        // dB ratio of fast to slow. Positive means a transient is
        // arriving (fast has jumped ahead of slow); near zero means
        // we're in the sustain portion.
        let ratio_db = if slow > 1e-9 {
            20.0 * (fast / slow).log10()
        } else {
            0.0
        };

        // Normalised "attack-ness" in [0, 1] -- maps the typical
        // transient ratio range (~0..6 dB at musical levels) onto
        // a smooth 0..1 curve.
        let attack_amount = (ratio_db / 6.0).clamp(0.0, 1.0);
        // The complement is the "sustain-ness".
        let sustain_amount = 1.0 - attack_amount;

        let attack_gain_db = (self.attack_pct * 0.01) * Self::MAX_DB * attack_amount;
        let sustain_gain_db = (self.sustain_pct * 0.01) * Self::MAX_DB * sustain_amount;
        let gain = 10.0_f32.powf((attack_gain_db + sustain_gain_db) / 20.0);
        (left * gain, right * gain)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.env_fast = 0.0;
        self.env_slow = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.attack_pct = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.sustain_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_declared() {
        let e = TransientShaperEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Attack");
        assert_eq!(specs[1].name, "Sustain");
    }

    #[test]
    fn at_default_settings_is_transparent() {
        // Attack = 0, Sustain = 0 should be unity gain regardless of
        // envelope state.
        let mut e = TransientShaperEffect::new();
        e.set_sample_rate(48_000.0);
        for i in 0..2048 {
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, r) = e.process_sample(x, x);
            assert!((l - x).abs() < 1e-6, "non-unity at i={i}");
            assert!((r - x).abs() < 1e-6, "non-unity at i={i}");
        }
    }

    #[test]
    fn positive_attack_boosts_a_transient() {
        // Drive a steady tone, then a sudden loud burst (the
        // transient). With Attack = +100 the burst should come out
        // louder than its input level. Without the transient shape
        // active (Attack = 0) the burst would pass at unity.
        let measure = |attack_pct: f32| -> f32 {
            let mut e = TransientShaperEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, attack_pct);
            // Steady soft tone to settle the slow envelope.
            for _ in 0..(48_000 / 10) {
                e.process_sample(0.05, 0.05);
            }
            // Sudden burst.
            let mut peak = 0.0_f32;
            for _ in 0..64 {
                let (l, _) = e.process_sample(0.5, 0.5);
                peak = peak.max(l.abs());
            }
            peak
        };
        let unity = measure(0.0);
        let boosted = measure(100.0);
        assert!(
            boosted > unity * 1.5,
            "Attack = +100 should clearly boost transient (unity={unity}, boosted={boosted})"
        );
    }

    #[test]
    fn negative_sustain_pulls_down_the_tail() {
        // Drive a sustained signal long enough for the envelopes to
        // settle in the sustain region. With Sustain = -100 the
        // signal level should drop relative to Sustain = 0.
        let measure = |sustain_pct: f32| -> f32 {
            let mut e = TransientShaperEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(1, sustain_pct);
            for _ in 0..48_000 {
                e.process_sample(0.5, 0.5);
            }
            let mut sum = 0.0_f32;
            for _ in 0..2048 {
                let (l, _) = e.process_sample(0.5, 0.5);
                sum += l * l;
            }
            (sum / 2048.0).sqrt()
        };
        let unity = measure(0.0);
        let cut = measure(-100.0);
        assert!(
            cut < unity * 0.9,
            "Sustain = -100 should pull the tail down (unity={unity}, cut={cut})"
        );
    }

    #[test]
    fn reset_clears_envelopes() {
        let mut e = TransientShaperEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..2048 {
            e.process_sample(0.5, 0.5);
        }
        e.reset();
        assert_eq!(e.env_fast, 0.0);
        assert_eq!(e.env_slow, 0.0);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = TransientShaperEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
