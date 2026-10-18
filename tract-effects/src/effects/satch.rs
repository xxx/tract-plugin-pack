use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A detail-preserving spectral saturator, ported from the `satch` plugin.
/// Wraps two `SpectralClipper`s (one per channel) plus per-channel dry-delay
/// buffers (matching the spectral path's 2048-sample latency), exposing
/// four modulatable params (Gain, Threshold, Detail, Knee).
///
/// **Algorithm** (per sample, per channel):
/// 1. Pull the delayed dry from the per-channel dry-delay buffer.
/// 2. Time-domain saturate the delayed dry → `td` (returns tanh value for
///    the clip mask).
/// 3. Spectral path on undelayed input → `sp` (the FFT-based detail-
///    preserving clip with a built-in 2048-sample delay).
/// 4. Detail rescue: `wet = (td + detail * clip * (sp − td)).clamp(±threshold)`
///    where `clip = tanh²(gain·input/threshold)` only fires where the
///    waveshaper is actively clipping.
///
/// **Latency**: 2048 samples (= FFT size) ≈ 43 ms at 48 kHz. multosis
/// reports this to the host via `Effect::latency_samples`, so PDC keeps
/// the chain aligned automatically. The per-row Mix dial blends in-time
/// dry against this delayed wet — for clean saturation, run Mix at 100 %.
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
    /// Dry delay buffers — must be FFT_SIZE long so the time-domain
    /// waveshaper sees the same sample the spectral path was fed
    /// FFT_SIZE samples ago. Required by the algorithm itself
    /// (`td` and `sp` need to operate on time-aligned signals for
    /// `lost = sp − td` to be the spectral path's detail bonus).
    dry_delay_l: Vec<f32>,
    dry_delay_r: Vec<f32>,
    dry_delay_pos: usize,
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
            dry_delay_l: vec![0.0; Self::FFT_SIZE],
            dry_delay_r: vec![0.0; Self::FFT_SIZE],
            dry_delay_pos: 0,
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
        // Gain and threshold are precomputed in `set_param` — zero
        // `powf`/`exp` calls per sample here.
        let gain = self.gain_linear;
        let threshold = self.threshold_linear;
        let inv_threshold = 1.0 / threshold;
        let detail = self.detail_pct * 0.01;
        let knee = self.knee_pct * 0.01;
        // Skip-FFT optimisation: when Detail is zero, the spectral term
        // is multiplied by zero in the blend, so the FFT pipeline can
        // skip the expensive frame work. Ring state still advances;
        // re-enabling Detail refills the output over roughly four hops.
        let skip_fft = detail <= 0.0;

        // Pull the delayed dry; the spectral path operates on undelayed
        // input but its output emerges 2048 samples late, so we
        // time-align by waveshaping the same-old dry sample.
        let dry_l = self.dry_delay_l[self.dry_delay_pos];
        let dry_r = self.dry_delay_r[self.dry_delay_pos];
        self.dry_delay_l[self.dry_delay_pos] = left;
        self.dry_delay_r[self.dry_delay_pos] = right;
        self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

        // Time-domain waveshaper on the delayed dry (returns tanh so we
        // can build the clip mask without recomputing it).
        let (td_l, tanh_l) = tract_dsp::spectral_clipper::saturate_td_with_tanh_fast(
            dry_l,
            gain,
            threshold,
            inv_threshold,
            knee,
        );
        let (td_r, tanh_r) = tract_dsp::spectral_clipper::saturate_td_with_tanh_fast(
            dry_r,
            gain,
            threshold,
            inv_threshold,
            knee,
        );

        // Spectral path: gives the detail-preserved reconstruction. With
        // skip_fft we still advance ring/counter state (the clipper
        // handles that internally) but bypass the FFT frame work.
        let sp_l = if skip_fft {
            self.spectral_l
                .process_sample_skip_fft_fast(left, gain, threshold, inv_threshold, knee)
        } else {
            self.spectral_l
                .process_sample_fast(left, gain, threshold, inv_threshold, knee)
        };
        let sp_r = if skip_fft {
            self.spectral_r.process_sample_skip_fft_fast(
                right,
                gain,
                threshold,
                inv_threshold,
                knee,
            )
        } else {
            self.spectral_r
                .process_sample_fast(right, gain, threshold, inv_threshold, knee)
        };

        // Clip mask: tanh²(gained/threshold) is ~0 below the knee and ~1
        // above, so detail is only added where the waveshaper is
        // actively clipping. Without this gate the detail term would
        // colour sub-threshold material that the user expects to pass
        // through clean.
        let clip_l = tanh_l * tanh_l;
        let clip_r = tanh_r * tanh_r;
        let lost_l = sp_l - td_l;
        let lost_r = sp_r - td_r;
        let wet_l = (td_l + detail * clip_l * lost_l).clamp(-threshold, threshold);
        let wet_r = (td_r + detail * clip_r * lost_r).clamp(-threshold, threshold);
        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.spectral_l.reset();
        self.spectral_r.reset();
        for s in self.dry_delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.dry_delay_r.iter_mut() {
            *s = 0.0;
        }
        self.dry_delay_pos = 0;
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

    fn reference_mix(
        dry: f32,
        spectral: f32,
        gain: f64,
        threshold: f64,
        detail: f64,
        knee: f64,
    ) -> f64 {
        let gained = dry as f64 * gain;
        let hard = gained.clamp(-threshold, threshold);
        let tanh = (gained / threshold).tanh();
        let td = (1.0 - knee) * hard + knee * threshold * tanh;
        let weight = detail * tanh.powi(2);
        ((1.0 - weight) * td + weight * spectral as f64).clamp(-threshold, threshold)
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
                            let expected = reference_mix(
                                delayed,
                                0.0,
                                gain,
                                threshold,
                                0.0,
                                knee_pct as f64 / 100.0,
                            );
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
    fn detail_uses_a_single_tanh_squared_gate_and_the_aligned_spectral_output() {
        use tract_dsp::spectral_clipper::SpectralClipper;
        let mut detail_energy = 0.0;
        let mut clipped_samples = 0;
        for knee_pct in [0.0, 37.5, 100.0] {
            for detail_pct in [20.0, 65.0, 100.0] {
                let mut effect = configured([12.0, -6.0, detail_pct, knee_pct]);
                let mut left = SpectralClipper::new(2_048, 512);
                let mut right = SpectralClipper::new(2_048, 512);
                let gain = 10.0_f64.powf(12.0 / 20.0);
                let threshold = 10.0_f64.powf(-6.0 / 20.0);
                for sample in 0..8_192 {
                    let input = input_at(sample);
                    let dry = if sample < 2_048 {
                        (0.0, 0.0)
                    } else {
                        input_at(sample - 2_048)
                    };
                    let spectral = (
                        left.process_sample(
                            input.0,
                            gain as f32,
                            threshold as f32,
                            knee_pct / 100.0,
                        ),
                        right.process_sample(
                            input.1,
                            gain as f32,
                            threshold as f32,
                            knee_pct / 100.0,
                        ),
                    );
                    let actual = effect.process_sample(input.0, input.1);
                    for (actual, delayed, sp) in
                        [(actual.0, dry.0, spectral.0), (actual.1, dry.1, spectral.1)]
                    {
                        let expected = reference_mix(
                            delayed,
                            sp,
                            gain,
                            threshold,
                            detail_pct as f64 / 100.0,
                            knee_pct as f64 / 100.0,
                        );
                        assert!(
                            (actual as f64 - expected).abs() < 3e-6,
                            "Detail {detail_pct}, Knee {knee_pct}, sample {sample}: {actual} vs {expected}"
                        );
                        let td = reference_mix(
                            delayed,
                            0.0,
                            gain,
                            threshold,
                            0.0,
                            knee_pct as f64 / 100.0,
                        );
                        detail_energy += (expected - td).powi(2);
                        if (expected.abs() - threshold).abs() < 1e-10 {
                            clipped_samples += 1;
                        }
                    }
                }
            }
        }
        assert!(detail_energy > 0.001);
        assert!(clipped_samples > 0);
    }

    #[test]
    fn live_parameter_and_detail_changes_follow_the_documented_skip_schedule() {
        use tract_dsp::spectral_clipper::SpectralClipper;
        let mut effect = SatchEffect::new();
        let mut left = SpectralClipper::new(2_048, 512);
        let mut right = SpectralClipper::new(2_048, 512);
        let mut params = [6.0, -3.0, 0.0, 100.0];
        for sample in 0..12_288 {
            let next = match sample {
                0 => Some(params),
                3_333 => Some([12.0, -6.0, 75.0, 37.5]),
                6_145 => Some([3.0, -12.0, 0.0, 0.0]),
                8_191 => Some([18.0, -9.0, 35.0, 100.0]),
                10_007 => Some([0.0, 0.0, 100.0, 55.0]),
                _ => None,
            };
            if let Some(next) = next {
                params = next;
                for (slot, value) in params.into_iter().enumerate() {
                    effect.set_param(slot, value);
                }
            }
            let gain = 10.0_f64.powf(params[0] as f64 / 20.0);
            let threshold = 10.0_f64.powf(params[1] as f64 / 20.0);
            let input = input_at(sample);
            let dry = if sample < 2_048 {
                (0.0, 0.0)
            } else {
                input_at(sample - 2_048)
            };
            let mut spectral = [0.0; 2];
            for ((clipper, input), output) in [(&mut left, input.0), (&mut right, input.1)]
                .into_iter()
                .zip(&mut spectral)
            {
                *output = if params[2] == 0.0 {
                    clipper.process_sample_skip_fft(
                        input,
                        gain as f32,
                        threshold as f32,
                        params[3] / 100.0,
                    )
                } else {
                    clipper.process_sample(input, gain as f32, threshold as f32, params[3] / 100.0)
                };
            }
            let actual = effect.process_sample(input.0, input.1);
            for (actual, delayed, sp) in [
                (actual.0, dry.0, spectral[0]),
                (actual.1, dry.1, spectral[1]),
            ] {
                let expected = reference_mix(
                    delayed,
                    sp,
                    gain,
                    threshold,
                    params[2] as f64 / 100.0,
                    params[3] as f64 / 100.0,
                );
                assert!(
                    (actual as f64 - expected).abs() < 3e-6,
                    "sample {sample}: {actual} vs {expected}"
                );
            }
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
    fn reset_replays_a_fresh_stereo_processor_and_reuses_dry_delay_storage() {
        for detail in [0.0, 40.0, 100.0] {
            let params = [12.0, -9.0, detail, 37.5];
            let mut used = configured(params);
            let storage = (used.dry_delay_l.as_ptr(), used.dry_delay_r.as_ptr());
            let capacities = (used.dry_delay_l.capacity(), used.dry_delay_r.capacity());
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
            assert_eq!(
                (used.dry_delay_l.as_ptr(), used.dry_delay_r.as_ptr()),
                storage
            );
            assert_eq!(
                (used.dry_delay_l.len(), used.dry_delay_r.len()),
                (2_048, 2_048)
            );
            assert_eq!(
                (used.dry_delay_l.capacity(), used.dry_delay_r.capacity()),
                capacities
            );
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
    fn satch_reset_zeroes_dry_delay_and_clipper_state() {
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
        assert!(s.dry_delay_l.iter().all(|&v| v == 0.0));
        assert!(s.dry_delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(s.dry_delay_pos, 0);
        // First post-reset sample is finite + L==R symmetric for mono input.
        let (l, r) = s.process_sample(0.5, 0.5);
        assert!(l.is_finite() && r.is_finite());
        assert!((l - r).abs() < 1e-6);
    }
}
