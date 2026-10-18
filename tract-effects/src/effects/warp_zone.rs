use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A phase-vocoder spectral shifter/stretcher, ported from the `warp-zone`
/// plugin. Wraps two `SpectralShifter`s (one per channel) plus a clamped
/// feedback loop, exposing five modulatable params (Shift, Stretch,
/// Feedback, Low, High).
///
/// **Latency**: 4096 samples (= FFT size) ≈ 85 ms at 48 kHz. The multosis
/// engine does not latency-compensate the per-row Mix dial, so at
/// intermediate Mix the in-time dry comb-filters against the delayed wet
/// — musically usable as sound design, but for a "clean" pitch shift run
/// the row at Mix = 100 %.
pub struct WarpZoneEffect {
    shift_st: f32,
    stretch: f32,
    feedback_pct: f32,
    low_hz: f32,
    high_hz: f32,
    sample_rate: f32,
    /// One shifter per channel — they share params but maintain independent
    /// FFT state so stereo information is preserved through the cascade.
    shifter_l: tract_dsp::spectral_shifter::SpectralShifter,
    shifter_r: tract_dsp::spectral_shifter::SpectralShifter,
    /// Feedback memory per channel — the previous sample's wet output,
    /// clamped to ±4 to keep the loop from running away even at the
    /// 95 % cap.
    fb_l: f32,
    fb_r: f32,
}

impl WarpZoneEffect {
    /// Phase-vocoder FFT size. Matches the warp-zone plugin so the per-
    /// sample behaviour is identical.
    const FFT_SIZE: usize = 4096;
    /// Hop size — 75 % overlap = 4× redundancy with Hann window.
    const HOP_SIZE: usize = 1024;
    /// Feedback gain cap. 95 % stays well clear of runaway after the
    /// per-sample ±4 clamp on `fb_l`/`fb_r`.
    const FB_MAX: f32 = 0.95;
    /// Per-sample feedback safety clamp (mirrors warp-zone). Keeps the
    /// loop bounded even when the spectral path produces a transient
    /// peak above unity.
    const FB_CLAMP: f32 = 4.0;

    pub(super) const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Shift",
            min: -24.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: " st",
            },
        },
        ParamSpec {
            name: "Stretch",
            min: 0.5,
            max: 2.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "x",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 95.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Low",
            min: 20.0,
            max: 20_000.0,
            default: 20.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "High",
            min: 20.0,
            max: 20_000.0,
            default: 20_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
    ];

    pub fn new() -> Self {
        Self {
            shift_st: Self::PARAMS[0].default,
            stretch: Self::PARAMS[1].default,
            feedback_pct: Self::PARAMS[2].default,
            low_hz: Self::PARAMS[3].default,
            high_hz: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            shifter_l: tract_dsp::spectral_shifter::SpectralShifter::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            shifter_r: tract_dsp::spectral_shifter::SpectralShifter::new(
                Self::FFT_SIZE,
                Self::HOP_SIZE,
            ),
            fb_l: 0.0,
            fb_r: 0.0,
        }
    }

    /// Convert the Low/High Hz pair into bin indices for the current SR.
    /// Mirrors warp-zone's clamping: low ≥ 1 (skip DC), high ≥ low, both
    /// capped at `fft_size/2 + 1`.
    fn frequency_bins(&self) -> (usize, usize) {
        let half_plus_one = Self::FFT_SIZE / 2 + 1;
        let bin_hz = self.sample_rate / Self::FFT_SIZE as f32;
        let low_bin = (self.low_hz / bin_hz).round() as usize;
        let high_bin = (self.high_hz / bin_hz).round() as usize;
        let low_bin = low_bin.max(1).min(half_plus_one);
        let high_bin = high_bin.max(low_bin).min(half_plus_one);
        (low_bin, high_bin)
    }
}

impl Default for WarpZoneEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for WarpZoneEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (low_bin, high_bin) = self.frequency_bins();
        let fb = (self.feedback_pct * 0.01).clamp(0.0, Self::FB_MAX);

        // Inject the previous wet (clamped) into the cascade input, then
        // run the shifter. The shifter returns the wet sample; we save it
        // for the next-iteration feedback after a safety clamp.
        let in_l = left + self.fb_l * fb;
        let in_r = right + self.fb_r * fb;
        let wet_l = self.shifter_l.process_sample(
            in_l,
            self.shift_st,
            self.stretch,
            false,
            low_bin,
            high_bin,
        );
        let wet_r = self.shifter_r.process_sample(
            in_r,
            self.shift_st,
            self.stretch,
            false,
            low_bin,
            high_bin,
        );
        self.fb_l = wet_l.clamp(-Self::FB_CLAMP, Self::FB_CLAMP);
        self.fb_r = wet_r.clamp(-Self::FB_CLAMP, Self::FB_CLAMP);
        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.shifter_l.reset();
        self.shifter_r.reset();
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.shift_st = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.stretch = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.feedback_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.low_hz = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.high_hz = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }

    /// 4096-sample FFT delay through the phase vocoder. The engine sums
    /// this across non-muted/non-solo-cancelled WarpZone rows to report a
    /// dynamic latency to the host.
    fn latency_samples(&self) -> usize {
        Self::FFT_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    fn configured(params: [f32; 5], sample_rate: f32) -> WarpZoneEffect {
        let mut effect = WarpZoneEffect::default();
        for (slot, value) in params.into_iter().enumerate() {
            effect.set_param(slot, value);
        }
        effect.set_sample_rate(sample_rate);
        effect
    }

    fn input_at(sample: usize) -> (f32, f32) {
        let t = sample as f32;
        let nyquist = if sample.is_multiple_of(2) { 1.0 } else { -1.0 };
        (
            0.25 * (t * 0.071).sin() + 0.08 * (t * 0.73).cos() + 0.12 * nyquist,
            -0.18 * (t * 0.13).cos() + 0.06 * (t * 1.17).sin() - 0.09 * nyquist,
        )
    }

    #[test]
    fn frequency_bounds_round_at_half_bins_and_follow_the_sample_rate() {
        for (sr, low, high, expected) in [
            (48_000.0, 20.0, 20_000.0, (2, 1_707)),
            (44_100.0, 20.0, 20_000.0, (2, 1_858)),
            (96_000.0, 20.0, 20_000.0, (1, 853)),
            (192_000.0, 20.0, 20_000.0, (1, 427)),
            (192_000.0, 20.0, 20.0, (1, 1)),
            (32_000.0, 20.0, 15_000.0, (3, 1_920)),
            (8_000.0, 20.0, 2_000.0, (10, 1_024)),
            (8_000.0, 20.0, 4_001.0, (10, 2_049)),
            (40_960.0, 24.999, 34.999, (2, 3)),
            (40_960.0, 25.0, 35.0, (3, 4)),
            (40_960.0, 25.001, 35.001, (3, 4)),
            (40_960.0, 1_000.0, 500.0, (100, 100)),
        ] {
            let effect = configured([12.0, 1.3, 0.0, low, high], sr);
            assert_eq!(
                effect.frequency_bins(),
                expected,
                "SR {sr}, Low {low}, High {high}"
            );
        }
    }

    #[test]
    fn identity_feedback_matches_an_independent_delayed_echo_model() {
        fn impulse(sample: usize) -> (f64, f64) {
            match sample {
                8_192 => (12.0, -10.0),
                8_209 => (-11.0, 14.0),
                _ => (0.0, 0.0),
            }
        }
        let trim = 10.0_f64.powf(-3.0 / 20.0);
        for (percent, feedback) in [(0.0, 0.0), (50.0, 0.5), (95.0, 0.95)] {
            let mut effect = configured([0.0, 1.0, percent, 20.0, 20_000.0], 48_000.0);
            let mut history: Vec<(f64, f64)> = Vec::new();
            let mut clamp_signs = [false; 4];
            for sample in 0..32_768 {
                let expected = if sample < 4_096 {
                    (0.0, 0.0)
                } else {
                    let dry = impulse(sample - 4_096);
                    let previous = if sample > 4_096 {
                        history[sample - 4_097]
                    } else {
                        (0.0, 0.0)
                    };
                    (
                        trim * (dry.0 + feedback * previous.0.clamp(-4.0, 4.0)),
                        trim * (dry.1 + feedback * previous.1.clamp(-4.0, 4.0)),
                    )
                };
                history.push(expected);
                let input = impulse(sample);
                let actual = effect.process_sample(input.0 as f32, input.1 as f32);
                assert!(
                    (actual.0 as f64 - expected.0).abs() < 2e-5
                        && (actual.1 as f64 - expected.1).abs() < 2e-5,
                    "feedback {percent}, sample {sample}: {actual:?} vs {expected:?}"
                );
                clamp_signs[0] |= actual.0 > 4.0;
                clamp_signs[1] |= actual.0 < -4.0;
                clamp_signs[2] |= actual.1 > 4.0;
                clamp_signs[3] |= actual.1 < -4.0;
            }
            assert!(clamp_signs.into_iter().all(|seen| seen));
        }
    }

    #[test]
    fn empty_and_above_nyquist_ranges_preserve_delayed_audio_at_unity() {
        for (sr, low, high) in [(40_960.0, 1_000.0, 500.0), (8_000.0, 10_000.0, 20_000.0)] {
            let mut effect = configured([12.0, 1.3, 0.0, low, high], sr);
            let mut energy = [0.0; 2];
            for sample in 0..16_384 {
                let input = input_at(sample);
                let actual = effect.process_sample(input.0, input.1);
                if sample >= 8_192 {
                    let expected = input_at(sample - 4_096);
                    assert!(
                        (actual.0 - expected.0).abs() < 3e-6
                            && (actual.1 - expected.1).abs() < 3e-6,
                        "SR {sr}, sample {sample}: {actual:?} vs {expected:?}"
                    );
                    energy[0] += actual.0 * actual.0;
                    energy[1] += actual.1 * actual.1;
                }
            }
            assert!(energy.into_iter().all(|energy| energy > 0.01));
        }
    }

    #[test]
    fn shifted_stereo_audio_matches_separate_shifters_with_known_bin_ranges() {
        use tract_dsp::spectral_shifter::SpectralShifter;
        for (params, sr, low_bin, high_bin) in [
            ([12.0, 1.0, 0.0, 200.0, 4_000.0], 40_960.0, 20, 400),
            ([-7.0, 0.75, 0.0, 1_000.0, 12_000.0], 48_000.0, 85, 1_024),
            ([3.0, 1.5, 0.0, 500.0, 8_000.0], 32_000.0, 64, 1_024),
            ([-12.0, 1.0, 0.0, 20.0, 20_000.0], 8_000.0, 10, usize::MAX),
        ] {
            let mut effect = configured(params, sr);
            let mut left = SpectralShifter::new(4_096, 1_024);
            let mut right = SpectralShifter::new(4_096, 1_024);
            let mut energy = [0.0; 2];
            for sample in 0..16_384 {
                let input = input_at(sample);
                let expected = (
                    left.process_sample(input.0, params[0], params[1], false, low_bin, high_bin),
                    right.process_sample(input.1, params[0], params[1], false, low_bin, high_bin),
                );
                let actual = effect.process_sample(input.0, input.1);
                assert_eq!(actual, expected, "SR {sr}, sample {sample}");
                energy[0] += actual.0 * actual.0;
                energy[1] += actual.1 * actual.1;
            }
            assert!(energy.into_iter().all(|energy| energy > 0.01));
        }
    }

    #[test]
    fn live_rate_changes_update_bin_ranges_without_resetting_feedback_or_shifter_history() {
        use tract_dsp::spectral_shifter::SpectralShifter;
        let mut effect = configured([-5.0, 1.2, 50.0, 1_000.0, 8_000.0], 40_960.0);
        let mut left = SpectralShifter::new(4_096, 1_024);
        let mut right = SpectralShifter::new(4_096, 1_024);
        let mut previous = (0.0_f32, 0.0_f32);
        let mut bins = (100, 800);
        let mut energy = [0.0; 2];
        for sample in 0..16_384 {
            match sample {
                3_033 => {
                    effect.set_sample_rate(32_000.0);
                    bins = (128, 1_024);
                }
                6_107 => {
                    effect.set_sample_rate(96_000.0);
                    bins = (43, 341);
                }
                8_191 => {
                    effect.set_sample_rate(8_000.0);
                    bins = (512, usize::MAX);
                }
                _ => {}
            }
            let input = input_at(sample);
            let expected = (
                left.process_sample(
                    input.0 + previous.0.clamp(-4.0, 4.0) * 0.5,
                    -5.0,
                    1.2,
                    false,
                    bins.0,
                    bins.1,
                ),
                right.process_sample(
                    input.1 + previous.1.clamp(-4.0, 4.0) * 0.5,
                    -5.0,
                    1.2,
                    false,
                    bins.0,
                    bins.1,
                ),
            );
            previous = expected;
            let actual = effect.process_sample(input.0, input.1);
            assert_eq!(actual, expected, "sample {sample}");
            assert_eq!(effect.latency_samples(), 4_096);
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_replays_a_fresh_stereo_processor_with_the_same_parameters_and_rate() {
        for feedback in [0.0, 75.0, 95.0] {
            let params = [7.0, 0.75, feedback, 400.0, 11_000.0];
            let mut used = configured(params, 32_000.0);
            let mut warm_energy = [0.0; 2];
            for sample in 0..16_384 {
                let input = input_at(sample);
                let output = used.process_sample(input.0, input.1);
                warm_energy[0] += output.0 * output.0;
                warm_energy[1] += output.1 * output.1;
            }
            assert!(warm_energy.into_iter().all(|energy| energy > 0.01));
            used.reset();
            let mut fresh = configured(params, 32_000.0);
            let mut energy = [0.0; 2];
            for sample in 0..16_384 {
                let input = input_at(sample + 137);
                let actual = used.process_sample(input.0, input.1);
                assert_eq!(actual, fresh.process_sample(input.0, input.1));
                energy[0] += actual.0 * actual.0;
                energy[1] += actual.1 * actual.1;
            }
            assert!(energy.into_iter().all(|energy| energy > 0.01));
        }
    }

    #[test]
    fn warpzone_lists_five_parameters_with_the_expected_specs() {
        let w = WarpZoneEffect::new();
        let specs = w.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Shift", -24.0, 24.0, 0.0),
                ("Stretch", 0.5, 2.0, 1.0),
                ("Feedback", 0.0, 95.0, 0.0),
                ("Low", 20.0, 20_000.0, 20.0),
                ("High", 20.0, 20_000.0, 20_000.0)
            ]
        );
        assert_eq!(
            (w.shift_st, w.stretch, w.feedback_pct, w.low_hz, w.high_hz),
            (0.0, 1.0, 0.0, 20.0, 20_000.0)
        );
        assert!(
            specs[..3]
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
        assert!(
            specs[3..]
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Log))
        );
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 1,
                unit: " st"
            }
        ));
        assert!(matches!(
            specs[1].format,
            ParamFormat::Number {
                decimals: 2,
                unit: "x"
            }
        ));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number {
                decimals: 0,
                unit: "%"
            }
        ));
        assert!(
            specs[3..]
                .iter()
                .all(|spec| matches!(spec.format, ParamFormat::Hertz))
        );
    }

    #[test]
    fn warpzone_set_param_clamps_to_each_spec_range() {
        let mut w = WarpZoneEffect::new();
        for (slot, cases) in [
            (0, [(-999.0, -24.0), (7.25, 7.25), (99_999.0, 24.0)]),
            (1, [(-999.0, 0.5), (1.25, 1.25), (99_999.0, 2.0)]),
            (2, [(-999.0, 0.0), (37.5, 37.5), (99_999.0, 95.0)]),
            (3, [(-999.0, 20.0), (750.0, 750.0), (99_999.0, 20_000.0)]),
            (
                4,
                [(-999.0, 20.0), (3_750.0, 3_750.0), (99_999.0, 20_000.0)],
            ),
        ] {
            for (value, expected) in cases {
                w.set_param(slot, value);
                assert_eq!(
                    [w.shift_st, w.stretch, w.feedback_pct, w.low_hz, w.high_hz][slot],
                    expected
                );
            }
        }
        w.set_param(99, f32::INFINITY);
        assert_eq!(
            (w.shift_st, w.stretch, w.feedback_pct, w.low_hz, w.high_hz),
            (24.0, 2.0, 95.0, 20_000.0, 20_000.0)
        );
    }

    #[test]
    fn warpzone_output_is_finite_under_aggressive_modulation_and_max_feedback() {
        // Drive the cascade hard: feedback at the cap and a sustained
        // wide-band input. Output must stay bounded indefinitely.
        let mut w = WarpZoneEffect::new();
        w.set_sample_rate(48_000.0);
        w.set_param(2, 95.0); // Feedback at cap
        w.set_param(0, 12.0); // +12 st shift (× 2 frequency)
        for i in 0..96_000 {
            // 2 s
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.3 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = w.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 16.0 && r.abs() < 16.0,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }

    #[test]
    fn warpzone_reset_clears_feedback_and_shifter_state() {
        // Drive some signal in, reset, then verify the first sample with
        // pure-dry input doesn't carry residue from before.
        let mut w = WarpZoneEffect::new();
        w.set_sample_rate(48_000.0);
        w.set_param(2, 80.0); // High feedback to load state heavily
        for _ in 0..6_000 {
            w.process_sample(0.5, 0.5);
        }
        w.reset();
        // After reset both feedback slots are zero.
        assert_eq!(w.fb_l, 0.0);
        assert_eq!(w.fb_r, 0.0);
        // And the next impulse pair produces finite, well-bounded output
        // (the first few samples are silence — STFT hasn't synthesised yet).
        let (l, r) = w.process_sample(1.0, 1.0);
        assert!(l.is_finite() && r.is_finite());
        assert!(l.abs() < 4.0 && r.abs() < 4.0);
    }

    #[test]
    fn warpzone_default_params_match_pass_through_intent() {
        // Default settings = identity (shift=0, stretch=1, fb=0, full band).
        // After the FFT's settling latency, output should track input
        // closely enough to be recognisable as the same signal. We don't
        // assert sample-level equality (the STFT pipeline imparts the
        // documented identity-path trim and a 4096-sample delay), but the
        // RMS-of-output should be a meaningful fraction of the RMS-of-input
        // once samples have propagated through.
        let mut w = WarpZoneEffect::new();
        w.set_sample_rate(48_000.0);
        // Feed 2× FFT-size samples of a 1 kHz sine; measure RMS of the
        // SECOND half (past the latency).
        let n = 8192;
        let mut out_rms = 0.0_f32;
        let mut count = 0usize;
        for i in 0..n {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let (l, _r) = w.process_sample(dry, dry);
            if i >= n / 2 {
                out_rms += l * l;
                count += 1;
            }
        }
        let rms = (out_rms / count as f32).sqrt();
        // 1 kHz sine RMS = 1/√2 ≈ 0.707. Identity path trims by ~3 dB
        // (0.707 × 10^(-3/20) ≈ 0.5), so anywhere ≥ 0.2 confirms the
        // signal made it through.
        assert!(
            rms > 0.2,
            "default warpzone should pass signal, got rms={rms}"
        );
    }
}
