use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// The shared Satch spectral saturator, with independent stereo processors.
///
/// Each processor owns its fixed 2048-sample delay, spectral separation,
/// detail headroom, and final waveshaping; host parameters remain Gain,
/// Threshold, Detail, and Knee.
pub struct SatchEffect {
    gain_db: f32,
    threshold_db: f32,
    detail_pct: f32,
    knee_pct: f32,
    sample_rate: f32,
    /// Cached linear gain/threshold corresponding to the dB-valued
    /// params, recomputed in `set_param` so the per-sample path is
    /// free of `powf`/`exp` calls. MSEG modulation calls `set_param`
    /// once per sample for modulated targets only — same total cost
    /// as computing inline, but zero cost when not modulating.
    gain_linear: f32,
    threshold_linear: f32,
    spectral_l: tract_dsp::spectral_clipper::SpectralClipper,
    spectral_r: tract_dsp::spectral_clipper::SpectralClipper,
}

impl SatchEffect {
    /// FFT frame length — matches the satch plugin.
    const FFT_SIZE: usize = 2048;
    /// Hop size — 75 % overlap = 4× redundancy with Hann window.
    const HOP_SIZE: usize = 512;

    pub(super) const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Gain",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " dB",
            },
        },
        ParamSpec {
            name: "Threshold",
            min: -24.0,
            max: 0.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " dB",
            },
        },
        ParamSpec {
            name: "Detail",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Knee",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        let gain_db = Self::PARAMS[0].default;
        let threshold_db = Self::PARAMS[1].default;
        Self {
            gain_db,
            threshold_db,
            detail_pct: Self::PARAMS[2].default,
            knee_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            // Seed the cache from defaults; `set_param` keeps it in sync.
            gain_linear: tract_dsp::db::db_to_linear_fast(gain_db),
            threshold_linear: tract_dsp::db::db_to_linear_fast(threshold_db),
            spectral_l: tract_dsp::spectral_clipper::SpectralClipper::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            spectral_r: tract_dsp::spectral_clipper::SpectralClipper::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
        }
    }
}

impl Default for SatchEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for SatchEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let gain = self.gain_linear;
        let threshold = self.threshold_linear;
        let inv_threshold = 1.0 / threshold;
        let detail = self.detail_pct * 0.01;
        let knee = self.knee_pct * 0.01;
        (
            self.spectral_l.process_sample_with_detail_fast(
                left,
                gain,
                threshold,
                inv_threshold,
                detail,
                knee,
            ),
            self.spectral_r.process_sample_with_detail_fast(
                right,
                gain,
                threshold,
                inv_threshold,
                detail,
                knee,
            ),
        )
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.spectral_l.reset();
        self.spectral_r.reset();
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.gain_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
                self.gain_linear = tract_dsp::db::db_to_linear_fast(self.gain_db);
            }
            1 => {
                self.threshold_db = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max);
                self.threshold_linear = tract_dsp::db::db_to_linear_fast(self.threshold_db);
            }
            2 => self.detail_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.knee_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }

    /// 2048-sample FFT delay through the spectral clipper. Reported to
    /// the host via the engine's chain-latency sum so PDC keeps the
    /// multosis output aligned with the rest of the project.
    fn latency_samples(&self) -> usize {
        Self::FFT_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Effect;

    fn configured(params: [f32; 4]) -> SatchEffect {
        let mut effect = SatchEffect::default();
        for (slot, value) in params.into_iter().enumerate() {
            effect.set_param(slot, value);
        }
        effect
    }

    fn input_at(sample: usize) -> (f32, f32) {
        let t = sample as f32;
        (
            0.3 * (t * 0.11).sin() + 0.02 * (t * 0.71).cos(),
            0.17 * (t * 0.23).cos() - 0.03 * (t * 1.11).sin(),
        )
    }

    fn detail_tone(sample: usize) -> f32 {
        (std::f64::consts::TAU * 215.0 * sample as f64 / 2048.0).sin() as f32
    }

    #[test]
    fn detail_preserves_both_halves_of_a_quiet_tone_beneath_the_ceiling() {
        let gain = 10.0_f32.powf(12.0 / 20.0);
        for detail in [25.0, 50.0, 100.0] {
            let mut effect = configured([12.0, 0.0, detail, 0.0]);
            let amplitude = gain * 0.02 * detail / 100.0;
            for sample in 0..16_384 {
                let quiet = 0.02 * detail_tone(sample);
                let output = effect.process_sample(0.8 + quiet, -0.8 + quiet);
                if sample >= 8192 {
                    let preserved = amplitude * detail_tone(sample - 2048);
                    for (actual, sign) in [(output.0, 1.0), (output.1, -1.0)] {
                        let expected = sign * (1.0 - amplitude) + preserved;
                        assert!(actual.abs() <= 1.0, "ceiling exceeded: {actual}");
                        assert!(
                            (actual - expected).abs() < 1e-5,
                            "Detail {detail}, sample {sample}: {actual}, expected {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn multitone_detail_is_balanced_around_a_steady_internal_clip_line() {
        let mut effect = configured([12.041_2, 0.0, 100.0, 0.0]);
        let gain = 10.0_f32.powf(12.041_2 / 20.0);
        let period: Vec<f32> = (0..2048)
            .map(|sample| {
                let phase = std::f64::consts::TAU * 65.0 * sample as f64 / 2048.0;
                (0.02 * phase.sin() + 0.014 * (3.0 * phase).sin() + 0.009 * (5.0 * phase).sin())
                    as f32
            })
            .collect();
        let peak = period.iter().map(|q| q.abs()).fold(0.0_f32, f32::max);
        let center = 1.0 - gain * peak;
        for sample in 0..24_576 {
            let quiet = period[sample % period.len()];
            let (left, right) = effect.process_sample(0.8 + quiet, -0.8 + quiet);
            if sample >= 16_384 {
                let preserved = gain * quiet;
                assert!((left - (center + preserved)).abs() < 1e-5);
                assert!((right - (-center + preserved)).abs() < 1e-5);
                assert!(left.abs() <= 1.0 && right.abs() <= 1.0);
            }
        }
    }

    #[test]
    fn detail_does_not_change_material_below_the_threshold_for_any_knee() {
        for knee in [0.0, 37.5, 100.0] {
            let mut standard = configured([0.0, 0.0, 0.0, knee]);
            let mut preserved = configured([0.0, 0.0, 100.0, knee]);
            for sample in 0..16_384 {
                let loud = (std::f64::consts::TAU * 8.0 * sample as f64 / 2048.0).sin() as f32;
                let input = 0.8 * loud + 0.02 * detail_tone(sample);
                assert_eq!(
                    preserved.process_sample(input, -input),
                    standard.process_sample(input, -input),
                    "Knee {knee}, sample {sample}"
                );
            }
        }
    }

    #[test]
    fn detail_recovery_depends_on_the_gained_signal_not_the_input_gain_staging() {
        for detail in [0.0, 40.0, 100.0] {
            let mut direct = configured([0.0, -20.0, detail, 0.0]);
            let mut boosted = configured([20.0, -20.0, detail, 0.0]);
            for sample in 0..16_384 {
                let input = 0.8 + 0.02 * detail_tone(sample);
                let a = direct.process_sample(input, -input);
                let b = boosted.process_sample(input * 0.1, -input * 0.1);
                assert!(
                    (a.0 - b.0).abs() < 2e-6 && (a.1 - b.1).abs() < 2e-6,
                    "Detail {detail}, sample {sample}: {a:?} vs {b:?}"
                );
            }
        }
    }

    fn reference_td(dry: f32, gain: f64, threshold: f64, knee: f64) -> f64 {
        let gained = dry as f64 * gain;
        let hard = gained.clamp(-threshold, threshold);
        let soft = threshold * (gained / threshold).tanh();
        (1.0 - knee) * hard + knee * soft
    }

    #[test]
    fn hard_clip_has_exact_stereo_latency_across_multiple_dry_delay_wraps() {
        let mut effect = configured([0.0, 0.0, 0.0, 0.0]);
        for sample in 0..10_240 {
            let input = input_at(sample);
            let expected = if sample < 2_048 {
                (0.0, 0.0)
            } else {
                input_at(sample - 2_048)
            };
            assert_eq!(
                effect.process_sample(input.0, input.1),
                expected,
                "sample {sample}"
            );
        }
    }

    #[test]
    fn detail_zero_matches_independent_gain_threshold_and_knee_math() {
        for gain_db in [0.0, 6.0, 20.0, 24.0] {
            for threshold_db in [0.0, -6.0, -24.0] {
                for knee_pct in [0.0, 37.5, 100.0] {
                    let mut effect = configured([gain_db, threshold_db, 0.0, knee_pct]);
                    let gain = 10.0_f64.powf(gain_db as f64 / 20.0);
                    let threshold = 10.0_f64.powf(threshold_db as f64 / 20.0);
                    for sample in 0..6_144 {
                        let input = input_at(sample);
                        let dry = if sample < 2_048 {
                            (0.0, 0.0)
                        } else {
                            input_at(sample - 2_048)
                        };
                        let actual = effect.process_sample(input.0, input.1);
                        for (actual, delayed) in [(actual.0, dry.0), (actual.1, dry.1)] {
                            let expected =
                                reference_td(delayed, gain, threshold, knee_pct as f64 / 100.0);
                            assert!(
                                (actual as f64 - expected).abs() < 2e-6,
                                "Gain {gain_db}, Threshold {threshold_db}, Knee {knee_pct}, sample {sample}: {actual} vs {expected}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shared_processor_receives_live_parameters_and_fft_skip_transitions() {
        use tract_dsp::spectral_clipper::SpectralClipper;
        let mut effect = SatchEffect::new();
        let mut left = SpectralClipper::new(2048, 512);
        let mut right = SpectralClipper::new(2048, 512);
        let mut params = [6.0, -3.0, 0.0, 100.0];
        for sample in 0..12_288 {
            let next = match sample {
                0 => Some(params),
                3333 => Some([12.0, -6.0, 75.0, 37.5]),
                6145 => Some([3.0, -12.0, 0.0, 0.0]),
                8191 => Some([18.0, -9.0, 35.0, 100.0]),
                10_007 => Some([0.0, 0.0, 100.0, 55.0]),
                _ => None,
            };
            if let Some(next) = next {
                params = next;
                for (slot, value) in params.into_iter().enumerate() {
                    effect.set_param(slot, value);
                }
            }
            let gain = 10.0_f32.powf(params[0] / 20.0);
            let threshold = 10.0_f32.powf(params[1] / 20.0);
            let input = input_at(sample);
            let actual = effect.process_sample(input.0, input.1);
            let expected = (
                left.process_sample_with_detail(
                    input.0,
                    gain,
                    threshold,
                    params[2] / 100.0,
                    params[3] / 100.0,
                ),
                right.process_sample_with_detail(
                    input.1,
                    gain,
                    threshold,
                    params[2] / 100.0,
                    params[3] / 100.0,
                ),
            );
            assert!((actual.0 - expected.0).abs() < 3e-6, "sample {sample}");
            assert!((actual.1 - expected.1).abs() < 3e-6, "sample {sample}");
            assert!(actual.0.is_finite() && actual.1.is_finite());
            assert!(actual.0.abs() <= threshold + 1e-6 && actual.1.abs() <= threshold + 1e-6);
        }
    }

    #[test]
    fn reenabled_detail_settles_to_the_continuously_active_spectral_path() {
        let mut switched = configured([12.0, -6.0, 0.0, 37.5]);
        let mut active = configured([12.0, -6.0, 75.0, 37.5]);
        let mut early_difference = 0.0;
        for sample in 0..12_288 {
            if sample == 5_000 {
                switched.set_param(2, 75.0);
            }
            let input = input_at(sample);
            let actual = switched.process_sample(input.0, input.1);
            let expected = active.process_sample(input.0, input.1);
            if (5_000..5_512).contains(&sample) {
                early_difference +=
                    (actual.0 - expected.0).powi(2) + (actual.1 - expected.1).powi(2);
            }
            if sample >= 9_096 {
                assert_eq!(actual, expected, "sample {sample}");
            }
        }
        assert!(early_difference > 1e-5);
    }

    #[test]
    fn sample_rate_changes_preserve_the_fixed_sample_latency_and_processing_state() {
        let mut fixed = configured([9.0, -9.0, 65.0, 37.5]);
        let mut changing = configured([9.0, -9.0, 65.0, 37.5]);
        for sample in 0..10_240 {
            match sample {
                0 => changing.set_sample_rate(8_000.0),
                700 => changing.set_sample_rate(32_000.0),
                2_900 => changing.set_sample_rate(96_000.0),
                7_000 => changing.set_sample_rate(192_000.0),
                _ => {}
            }
            let input = input_at(sample);
            assert_eq!(
                changing.process_sample(input.0, input.1),
                fixed.process_sample(input.0, input.1)
            );
            assert_eq!(changing.latency_samples(), 2_048);
        }
    }

    #[test]
    fn reset_replays_a_fresh_stereo_processor() {
        for detail in [0.0, 40.0, 100.0] {
            let params = [12.0, -9.0, detail, 37.5];
            let mut used = configured(params);
            let mut warm_energy = [0.0; 2];
            for sample in 0..10_000 {
                let input = input_at(sample);
                let output = used.process_sample(input.0, input.1);
                warm_energy[0] += output.0 * output.0;
                warm_energy[1] += output.1 * output.1;
            }
            assert!(warm_energy.into_iter().all(|energy| energy > 0.01));
            used.reset();
            let mut fresh = configured(params);
            for sample in 0..8_192 {
                let input = input_at(sample + 333);
                assert_eq!(
                    used.process_sample(input.0, input.1),
                    fresh.process_sample(input.0, input.1)
                );
            }
        }
    }

    #[test]
    fn satch_lists_four_parameters_with_the_expected_specs() {
        let s = SatchEffect::new();
        let specs = s.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Gain", 0.0, 24.0, 0.0),
                ("Threshold", -24.0, 0.0, 0.0),
                ("Detail", 0.0, 100.0, 0.0),
                ("Knee", 0.0, 100.0, 100.0)
            ]
        );
        assert_eq!(
            (s.gain_db, s.threshold_db, s.detail_pct, s.knee_pct),
            (0.0, 0.0, 0.0, 100.0)
        );
        assert!(
            specs
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
        for spec in &specs[..2] {
            assert!(matches!(
                spec.format,
                ParamFormat::Number {
                    decimals: 1,
                    unit: " dB"
                }
            ));
        }
        for spec in &specs[2..] {
            assert!(matches!(
                spec.format,
                ParamFormat::Number {
                    decimals: 0,
                    unit: "%"
                }
            ));
        }
    }

    #[test]
    fn satch_reports_fft_size_latency() {
        let s = SatchEffect::new();
        assert_eq!(s.latency_samples(), 2048);
    }

    #[test]
    fn satch_set_param_clamps_to_each_spec_range() {
        let mut s = SatchEffect::new();
        for (slot, cases) in [
            (0, [(-999.0, 0.0), (9.5, 9.5), (999.0, 24.0)]),
            (1, [(-999.0, -24.0), (-9.5, -9.5), (999.0, 0.0)]),
            (2, [(-999.0, 0.0), (37.5, 37.5), (999.0, 100.0)]),
            (3, [(-999.0, 0.0), (37.5, 37.5), (999.0, 100.0)]),
        ] {
            for (value, expected) in cases {
                s.set_param(slot, value);
                assert_eq!(
                    [s.gain_db, s.threshold_db, s.detail_pct, s.knee_pct][slot],
                    expected
                );
            }
        }
        s.set_param(99, f32::INFINITY);
        assert_eq!(
            (s.gain_db, s.threshold_db, s.detail_pct, s.knee_pct),
            (24.0, 0.0, 100.0, 100.0)
        );
    }

    #[test]
    fn satch_output_stays_bounded_under_aggressive_gain_and_detail() {
        // Max gain + min threshold + full detail + soft knee — every per-
        // sample dial is at its limit. The wet output must stay within the
        // ±threshold clamp the algorithm enforces (≈ -24 dB ≈ 0.063 linear)
        // for every sample.
        let mut s = SatchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 24.0); // +24 dB gain
        s.set_param(1, -24.0); // -24 dB threshold
        s.set_param(2, 100.0); // full Detail
        s.set_param(3, 100.0); // full Knee (tanh soft clip)
        let threshold_linear = 10.0_f32.powf(-24.0 / 20.0);
        for i in 0..96_000 {
            // 2 s of program
            let t = i as f32 / 48_000.0;
            let dry = 0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = s.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Algorithm clamps the wet to ±threshold; ride a tiny epsilon
            // above to absorb f32 round-trip rounding.
            let cap = threshold_linear + 1e-4;
            assert!(
                l.abs() <= cap && r.abs() <= cap,
                "sample {i} ({l}, {r}) exceeds threshold {threshold_linear}"
            );
        }
    }

    #[test]
    fn satch_reset_clears_delayed_audio_and_spectral_state() {
        let mut s = SatchEffect::new();
        s.set_sample_rate(48_000.0);
        s.set_param(0, 12.0);
        s.set_param(2, 50.0);
        // Pump signal through to fill the delay + STFT rings.
        for i in 0..4_096 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let _ = s.process_sample(dry, dry);
        }
        s.reset();
        // First post-reset sample is finite + L==R symmetric for mono input.
        let (l, r) = s.process_sample(0.5, 0.5);
        assert!(l.is_finite() && r.is_finite());
        assert!((l - r).abs() < 1e-6);
    }
}
