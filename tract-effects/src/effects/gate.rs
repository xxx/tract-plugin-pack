use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Noise gate: passes signal above `Threshold`, attenuates below it.
/// Stereo-linked envelope follower (peak across both channels) drives
/// a binary open/close state with hysteresis to prevent chatter on
/// signals near the threshold, then smooths the gate gain with
/// asymmetric attack / release ramps.
///
/// **Hysteresis** is how far below the open threshold the envelope
/// must fall before the gate closes again. `Threshold - Hysteresis`
/// becomes the close threshold. Without this a signal floating
/// around the threshold would gate on and off rapidly.
///
/// Per-channel state: one envelope follower (shared across channels),
/// one gate-state flag, one smoothed gain. No allocations on the
/// audio thread.
pub struct GateEffect {
    threshold_db: f32,
    attack_ms: f32,
    release_ms: f32,
    hysteresis_db: f32,
    sample_rate: f32,

    open_thresh_lin: f32,
    close_thresh_lin: f32,
    /// One-pole coefficient for the peak-tracking envelope follower
    /// (controls how fast the envelope tracks the input signal).
    alpha_env: f32,
    /// One-pole coefficient for the gate gain ramping up (open).
    alpha_attack: f32,
    /// One-pole coefficient for the gate gain ramping down (close).
    alpha_release: f32,

    /// Peak-tracking envelope of `|signal|`.
    env: f32,
    /// Smoothed gate gain in `[0, 1]`. Targets 1.0 when open, 0.0
    /// when closed; one-pole attack / release brings it there.
    gain: f32,
    /// `true` once the envelope crosses the open threshold; flips
    /// back when the envelope falls below `close_thresh`. The
    /// hysteresis gap prevents flutter.
    is_open: bool,
}

impl GateEffect {
    /// Fixed time constant for the input envelope follower. Faster
    /// than the gate's attack / release so the envelope reflects
    /// the signal level accurately when the gate decides whether
    /// to open.
    const ENV_FOLLOW_MS: f32 = 5.0;

    pub(super) const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Threshold",
            min: -80.0,
            max: 0.0,
            default: -40.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
        ParamSpec {
            name: "Attack",
            min: 0.1,
            max: 100.0,
            default: 2.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Release",
            min: 5.0,
            max: 2_000.0,
            default: 100.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Hysteresis",
            min: 0.0,
            max: 24.0,
            default: 6.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
    ];

    pub fn new() -> Self {
        let mut e = Self {
            threshold_db: Self::PARAMS[0].default,
            attack_ms: Self::PARAMS[1].default,
            release_ms: Self::PARAMS[2].default,
            hysteresis_db: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            open_thresh_lin: 0.0,
            close_thresh_lin: 0.0,
            alpha_env: 0.0,
            alpha_attack: 0.0,
            alpha_release: 0.0,
            env: 0.0,
            gain: 0.0,
            is_open: false,
        };
        e.recompute();
        e
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.open_thresh_lin = 10.0_f32.powf(self.threshold_db / 20.0);
        let close_db = self.threshold_db - self.hysteresis_db;
        self.close_thresh_lin = 10.0_f32.powf(close_db / 20.0);
        self.alpha_env = Self::alpha(sr, Self::ENV_FOLLOW_MS);
        self.alpha_attack = Self::alpha(sr, self.attack_ms);
        self.alpha_release = Self::alpha(sr, self.release_ms);
    }

    #[inline]
    fn alpha(sr: f32, time_ms: f32) -> f32 {
        if time_ms <= 0.0 {
            return 0.0;
        }
        (-1.0 / (sr * time_ms * 0.001)).exp()
    }
}

impl Default for GateEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for GateEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Stereo-linked peak follower (one-pole). Tracks the
        // current signal level for the threshold comparison.
        let peak = left.abs().max(right.abs());
        if peak > self.env {
            // Fast attack on the envelope follower: jump straight
            // to the new peak so a transient above the threshold
            // opens the gate immediately.
            self.env = peak;
        } else {
            self.env += (1.0 - self.alpha_env) * (peak - self.env);
        }

        // Hysteresis-driven open/close state machine.
        if self.is_open {
            if self.env < self.close_thresh_lin {
                self.is_open = false;
            }
        } else if self.env > self.open_thresh_lin {
            self.is_open = true;
        }

        let target = if self.is_open { 1.0 } else { 0.0 };
        let alpha = if target > self.gain {
            self.alpha_attack
        } else {
            self.alpha_release
        };
        self.gain = alpha * self.gain + (1.0 - alpha) * target;
        (left * self.gain, right * self.gain)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.gain = 0.0;
        self.is_open = false;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.threshold_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.attack_ms = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.release_ms = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.hysteresis_db = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => return,
        }
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_gain_matches_one_time_constant_at_different_rates() {
        for sample_rate in [24_000.0, 96_000.0] {
            let mut effect = GateEffect::new();
            effect.set_sample_rate(sample_rate);
            effect.set_param(1, 10.0);
            let mut output = (0.0, 0.0);
            for _ in 0..(sample_rate * 0.01) as usize {
                output = effect.process_sample(-0.25, 1.0);
            }
            let gain = 1.0 - (-1.0_f32).exp();
            assert!((output.0 + 0.25 * gain).abs() < 1e-5);
            assert!((output.1 - gain).abs() < 4e-5, "{sample_rate}: {output:?}");
        }
    }

    #[test]
    fn detector_release_tracks_a_lower_nonzero_peak() {
        for sample_rate in [24_000.0, 96_000.0] {
            let mut effect = GateEffect::new();
            effect.set_sample_rate(sample_rate);
            effect.process_sample(0.25, -1.0);
            assert_eq!(effect.env, 1.0);
            for _ in 0..(sample_rate * 0.005) as usize {
                effect.process_sample(-0.02, 0.01);
            }
            let expected = 0.02 + 0.98 * (-1.0_f32).exp();
            assert!(
                (effect.env - expected).abs() < 4e-5,
                "{sample_rate}: {}",
                effect.env
            );
        }
    }

    #[test]
    fn closed_gate_gain_releases_over_the_selected_duration() {
        for sample_rate in [24_000.0, 96_000.0] {
            for release_ms in [20.0, 200.0] {
                let mut effect = GateEffect::new();
                effect.set_sample_rate(sample_rate);
                effect.set_param(0, -20.0);
                effect.set_param(2, release_ms);
                for _ in 0..(sample_rate * 0.02) as usize {
                    effect.process_sample(1.0, -0.25);
                }
                for _ in 0..sample_rate as usize {
                    effect.process_sample(0.0, 0.0);
                    if !effect.is_open {
                        break;
                    }
                }
                assert!(!effect.is_open);
                let initial_gain = effect.gain;
                assert!(initial_gain > 0.5);
                let samples = (sample_rate * release_ms * 0.001) as usize;
                let mut output = (0.0, 0.0);
                for _ in 0..samples {
                    output = effect.process_sample(0.001, -0.002);
                }
                let elapsed = samples as f64 / f64::from(sample_rate);
                let gain =
                    f64::from(initial_gain) * (-elapsed / (f64::from(release_ms) * 0.001)).exp();
                assert!(
                    (output.0 / 0.001 - gain as f32).abs() < 2e-4,
                    "{sample_rate} Hz, {release_ms} ms: gain {}, expected {gain}",
                    output.0 / 0.001
                );
                assert!((output.1 / -0.002 - gain as f32).abs() < 2e-4);
            }
        }
    }

    #[test]
    fn hysteresis_deadband_retains_both_open_and_closed_states() {
        let mut effect = GateEffect::new();
        effect.set_sample_rate(1_000.0);
        effect.set_param(0, -20.0);
        effect.set_param(3, 12.0);
        for _ in 0..100 {
            effect.process_sample(0.05, -0.025);
            assert!(!effect.is_open);
        }
        effect.process_sample(0.2, -0.1);
        assert!(effect.is_open);
        for _ in 0..100 {
            effect.process_sample(0.05, -0.025);
            assert!(effect.is_open);
        }
        for _ in 0..100 {
            effect.process_sample(0.01, -0.005);
        }
        assert!(!effect.is_open);
        for _ in 0..100 {
            effect.process_sample(0.05, -0.025);
            assert!(!effect.is_open);
        }
    }

    #[test]
    fn opening_and_closing_require_crossing_the_threshold() {
        let mut effect = GateEffect::new();
        // At 1 Hz the detector settles in one sample, making the boundary exact.
        effect.set_sample_rate(1.0);
        effect.set_param(0, 0.0);
        effect.set_param(3, 0.0);
        effect.process_sample(1.0, 0.0);
        assert!(!effect.is_open);
        effect.process_sample(1.0_f32.next_up(), 0.0);
        assert!(effect.is_open);
        effect.process_sample(1.0, 0.0);
        assert!(effect.is_open);
        effect.process_sample(1.0_f32.next_down(), 0.0);
        assert!(!effect.is_open);
    }

    #[test]
    fn steady_input_at_the_threshold_does_not_open_the_gate() {
        let mut effect = GateEffect::new();
        effect.set_sample_rate(48_000.0);
        effect.set_param(0, -5.5);
        let threshold = 10.0_f32.powf(-5.5 / 20.0);
        for sample in 0..16 {
            assert_eq!(
                effect.process_sample(threshold, -threshold * 0.5),
                (0.0, 0.0),
                "gate opened on threshold sample {sample}"
            );
        }
    }

    #[test]
    fn parameters_are_declared() {
        let e = GateEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Threshold");
        assert_eq!(specs[1].name, "Attack");
        assert_eq!(specs[2].name, "Release");
        assert_eq!(specs[3].name, "Hysteresis");
    }

    #[test]
    fn loud_signal_passes() {
        let mut e = GateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -40.0);
        // Warm the gate open.
        for _ in 0..1024 {
            e.process_sample(0.5, 0.5);
        }
        // Now check that subsequent samples pass at unity gain.
        let mut max_diff = 0.0_f32;
        for i in 0..256 {
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, _) = e.process_sample(x, x);
            max_diff = max_diff.max((l - x).abs());
        }
        assert!(
            max_diff < 0.01,
            "loud signal didn't pass cleanly: {max_diff}"
        );
    }

    #[test]
    fn silent_signal_is_gated() {
        let mut e = GateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -40.0);
        e.set_param(2, 50.0); // fast release
        // Initial state: gate is closed; verify silent input stays
        // silent for many samples after the gain rampdown completes.
        for _ in 0..48_000 {
            e.process_sample(0.0, 0.0);
        }
        let (l, r) = e.process_sample(0.0, 0.0);
        assert_eq!(l, 0.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn quiet_below_threshold_is_attenuated() {
        let mut e = GateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -20.0); // -20 dBFS = ~0.1 linear
        e.set_param(2, 100.0);
        // Very quiet input (-60 dBFS = 0.001 linear) -- well below
        // threshold. After the release the gate should be closed.
        for _ in 0..96_000 {
            e.process_sample(0.001, 0.001);
        }
        let mut peak = 0.0_f32;
        for _ in 0..2048 {
            let (l, _) = e.process_sample(0.001, 0.001);
            peak = peak.max(l.abs());
        }
        assert!(peak < 1e-4, "quiet signal leaked through gate: peak={peak}");
    }

    #[test]
    fn hysteresis_prevents_chatter() {
        // A signal floating across the threshold should not toggle
        // the gate every sample. Verify by counting state flips
        // over a slow ramp through the threshold.
        let mut e = GateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -20.0);
        e.set_param(3, 6.0); // 6 dB hysteresis
        let mut flips = 0;
        let mut prev_open = e.is_open;
        // Ramp from below close threshold to above open threshold,
        // back down, repeated. Without hysteresis a single sweep
        // would toggle once per direction; with hysteresis no extra
        // chatter is produced.
        for cycle in 0..10 {
            for i in 0..1024 {
                let t = (i as f32) / 1024.0;
                let x = if cycle % 2 == 0 {
                    t * 0.2
                } else {
                    (1.0 - t) * 0.2
                };
                e.process_sample(x, x);
                if e.is_open != prev_open {
                    flips += 1;
                    prev_open = e.is_open;
                }
            }
        }
        assert!(
            flips <= 20,
            "too many state flips ({flips}) -- hysteresis ineffective"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut e = GateEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..2048 {
            e.process_sample(0.5, 0.5);
        }
        e.reset();
        assert_eq!(e.env, 0.0);
        assert_eq!(e.gain, 0.0);
        assert!(!e.is_open);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = GateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
