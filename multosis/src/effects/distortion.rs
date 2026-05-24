use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Latency-free time-domain distortion / waveshaper. Pick from
/// **Hard** (brick-wall clip — brittle digital character, square-wave
/// bark), **Soft** (tanh — smooth tube-style saturation, predominantly
/// odd harmonics), **Cubic** (`1.5·(x − x³/3)` clamped — analog-style
/// soft saturation, transparent at low drive), **Sine** (sine transfer
/// curve — very rich harmonic content), or **Fold** (Buchla-style
/// wave folder — west-coast synth character; the signal reflects
/// rather than clips, producing new fundamentals out of harmonics at
/// high drive).
///
/// **Bias** adds a DC offset before clipping so symmetric clippers
/// become asymmetric (think single-supply transistor stages),
/// pushing 2nd-harmonic content into the spectrum. The clipped
/// bias value is subtracted back post-clip to mimic AC coupling at
/// the output — silent input always produces silent output regardless
/// of Bias, no matter which shape is selected.
///
/// **Tone** is a tilt EQ pivoted at 700 Hz applied post-distortion:
/// negative values darken (LP boost / HP cut), positive values
/// brighten (HP boost / LP cut). Zero is mathematically transparent
/// (gain_low = gain_high = 1).
///
/// **No anti-aliasing.** All five shapes generate broadband content
/// and we don't oversample — multosis is creative, not transparent.
/// If aliasing matters, throw a Satch or a steep low-pass after.
///
/// **Latency:** zero. **Per-sample work:** ~10 MAC + 1 transcendental
/// for Soft/Sine modes, ~10 MAC for the others.
pub struct DistortionEffect {
    drive_db: f32,
    type_idx: f32,
    bias_pct: f32,
    tone_pct: f32,
    out_db: f32,
    sample_rate: f32,
    /// One-pole LP coefficient for the tilt EQ, computed in
    /// `set_sample_rate` from `TILT_PIVOT_HZ` and the active SR.
    /// `y = (1 − a)·x + a·y_prev` — at `a = 0` the LP is a wire,
    /// at `a → 1` the LP shuts.
    tilt_a: f32,
    tilt_state_l: f32,
    tilt_state_r: f32,
    /// Cached linear gain corresponding to `drive_db` / `out_db` /
    /// `tone_pct`. Recomputed in `set_param` when the source value
    /// changes so the per-sample path is free of `powf`/`exp` calls
    /// — important because MSEG modulation calls `set_param` once
    /// per sample for modulated targets, but only for those targets.
    /// `tone_gain_high` is the post-distortion tilt-EQ HP weight
    /// (`2^tone`); the LP weight is its reciprocal, computed once
    /// per sample with a divide (still cheap).
    drive_gain: f32,
    out_gain: f32,
    tone_gain_high: f32,
}

impl DistortionEffect {
    const TYPE_LABELS: &'static [&'static str] = &["Hard", "Soft", "Cubic", "Sine", "Fold"];
    const TYPE_HARD: usize = 0;
    const TYPE_SOFT: usize = 1;
    const TYPE_CUBIC: usize = 2;
    const TYPE_SINE: usize = 3;
    const TYPE_FOLD: usize = 4;

    /// Tilt EQ pivot frequency (Hz). 700 Hz is the classic "where
    /// vocal presence lives" pivot — moves the perceived darkness
    /// boundary across the most-audible band.
    const TILT_PIVOT_HZ: f32 = 700.0;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Drive",
            min: 0.0,
            max: 48.0,
            default: 12.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
        ParamSpec {
            name: "Type",
            min: 0.0,
            max: (Self::TYPE_LABELS.len() - 1) as f32,
            default: 1.0, // Soft (tanh) is the friendliest default
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::TYPE_LABELS,
            },
        },
        ParamSpec {
            name: "Bias",
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
            name: "Tone",
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
            name: "Out",
            min: -24.0,
            max: 12.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
    ];

    pub fn new() -> Self {
        let drive_db = Self::PARAMS[0].default;
        let tone_pct = Self::PARAMS[3].default;
        let out_db = Self::PARAMS[4].default;
        let mut me = Self {
            drive_db,
            type_idx: Self::PARAMS[1].default,
            bias_pct: Self::PARAMS[2].default,
            tone_pct,
            out_db,
            sample_rate: 48_000.0,
            tilt_a: 0.0,
            tilt_state_l: 0.0,
            tilt_state_r: 0.0,
            // Seed the cached gains from the defaults. Subsequent
            // `set_param` calls keep these in sync.
            drive_gain: tract_dsp::db::db_to_linear_fast(drive_db),
            out_gain: tract_dsp::db::db_to_linear_fast(out_db),
            tone_gain_high: (tone_pct * 0.01).exp2(),
        };
        me.set_sample_rate(me.sample_rate);
        me
    }

    /// Apply the selected waveshape to one sample. `x` may be any
    /// real number; output is bounded in `[-1, +1]` for all five
    /// shapes.
    #[inline]
    fn shape(x: f32, type_idx: usize) -> f32 {
        match type_idx {
            Self::TYPE_HARD => x.clamp(-1.0, 1.0),
            Self::TYPE_SOFT => x.tanh(),
            Self::TYPE_CUBIC => {
                // y = 1.5·(x − x³/3), peak ±1 at x = ±1. Beyond that
                // the curve heads back to zero, so clamp the input
                // first to keep the transfer monotonic.
                if x >= 1.0 {
                    1.0
                } else if x <= -1.0 {
                    -1.0
                } else {
                    1.5 * (x - x * x * x * (1.0 / 3.0))
                }
            }
            Self::TYPE_SINE => {
                // sin(x·π/2) maps [-1, +1] to [-1, +1] with much
                // richer harmonic content than tanh. Past ±1 the sine
                // curve heads back to zero, so clamp.
                let xc = x.clamp(-1.0, 1.0);
                (xc * std::f32::consts::FRAC_PI_2).sin()
            }
            Self::TYPE_FOLD => {
                // Buchla-style triangle-wave folder, peaks at ±1,
                // period 4. Closed-form, no iteration:
                //   phased = x + 1
                //   mod4   = phased mod 4
                //   y      = mod4 − 1   if mod4 < 2
                //   y      = 3 − mod4   otherwise
                let phased = x + 1.0;
                let mod4 = phased - 4.0 * (phased * 0.25).floor();
                if mod4 < 2.0 {
                    mod4 - 1.0
                } else {
                    3.0 - mod4
                }
            }
            // `set_param` clamps `type_idx` into the valid range, so
            // this arm is unreachable in normal operation. Define it
            // as hard-clip rather than panic so a corrupted preset
            // produces a sane sound instead of a crash.
            _ => x.clamp(-1.0, 1.0),
        }
    }
}

impl Default for DistortionEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DistortionEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Gains are precomputed in `set_param` — zero `powf`/`exp` calls
        // per sample when the params aren't being modulated, exactly one
        // per modulated param when they are.
        let drive_gain = self.drive_gain;
        let bias = (self.bias_pct * 0.01).clamp(-1.0, 1.0);
        let out_gain = self.out_gain;
        let type_idx = (self.type_idx.round() as usize).min(Self::TYPE_LABELS.len() - 1);
        // The DC value the clipper outputs at x = 0 with the current
        // bias offset. Subtracting it post-clip AC-couples the output
        // exactly for every shape (including the asymmetric ones at
        // bias near ±1 where simple `−bias` would leak DC).
        let bias_dc = Self::shape(bias, type_idx);

        // Stage 1: drive + bias.
        let sl = left * drive_gain + bias;
        let sr = right * drive_gain + bias;

        // Stage 2: waveshape.
        let cl = Self::shape(sl, type_idx);
        let cr = Self::shape(sr, type_idx);

        // Stage 3: AC-couple (remove the bias DC). Silent input now
        // produces silent output for every Bias and every Type.
        let dl = cl - bias_dc;
        let dr = cr - bias_dc;

        // Stage 4: tilt EQ post-distortion. The clipper's harmonics
        // pass through the LP/HP split unaltered; only their relative
        // weight changes. `tone_gain_high = 2^tone` is precomputed in
        // `set_param`; `gain_low` is its reciprocal (a divide is cheap
        // enough to do per-sample).
        let gain_high = self.tone_gain_high;
        let gain_low = 1.0 / gain_high;

        self.tilt_state_l = (1.0 - self.tilt_a) * dl + self.tilt_a * self.tilt_state_l;
        let lp_l = self.tilt_state_l;
        let hp_l = dl - lp_l;
        let tilt_l = lp_l * gain_low + hp_l * gain_high;

        self.tilt_state_r = (1.0 - self.tilt_a) * dr + self.tilt_a * self.tilt_state_r;
        let lp_r = self.tilt_state_r;
        let hp_r = dr - lp_r;
        let tilt_r = lp_r * gain_low + hp_r * gain_high;

        // Stage 5: output trim.
        (tilt_l * out_gain, tilt_r * out_gain)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        // One-pole LP: a = exp(−2π·fc/sr). At 48 kHz / 700 Hz ≈ 0.913.
        let two_pi = 2.0 * std::f32::consts::PI;
        self.tilt_a = (-two_pi * Self::TILT_PIVOT_HZ / sr).exp();
    }

    fn reset(&mut self) {
        self.tilt_state_l = 0.0;
        self.tilt_state_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.drive_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
                self.drive_gain = tract_dsp::db::db_to_linear_fast(self.drive_db);
            }
            1 => {
                let max_idx = (Self::TYPE_LABELS.len() - 1) as f32;
                self.type_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.bias_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => {
                self.tone_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max);
                self.tone_gain_high = (self.tone_pct * 0.01).exp2();
            }
            4 => {
                self.out_db = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max);
                self.out_gain = tract_dsp::db::db_to_linear_fast(self.out_db);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat};

    #[test]
    fn distortion_lists_five_parameters_with_the_expected_specs() {
        let d = DistortionEffect::new();
        let specs = d.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Drive");
        assert_eq!(specs[0].min, 0.0);
        assert_eq!(specs[0].max, 48.0);
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number { unit: "dB", .. }
        ));
        assert_eq!(specs[1].name, "Type");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[1].default, 1.0); // Soft
        assert_eq!(specs[2].name, "Bias");
        assert_eq!(specs[2].min, -100.0);
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Tone");
        assert_eq!(specs[4].name, "Out");
        assert_eq!(specs[4].min, -24.0);
        assert_eq!(specs[4].max, 12.0);
        assert!(matches!(
            specs[4].format,
            ParamFormat::Number { unit: "dB", .. }
        ));
    }

    #[test]
    fn distortion_set_param_clamps_each_slot() {
        let mut d = DistortionEffect::new();
        d.set_param(0, 999.0);
        assert_eq!(d.drive_db, 48.0);
        d.set_param(0, -10.0);
        assert_eq!(d.drive_db, 0.0);
        d.set_param(1, 99.0);
        assert_eq!(d.type_idx, 4.0);
        d.set_param(1, -5.0);
        assert_eq!(d.type_idx, 0.0);
        d.set_param(2, 999.0);
        assert_eq!(d.bias_pct, 100.0);
        d.set_param(3, -999.0);
        assert_eq!(d.tone_pct, -100.0);
        d.set_param(4, 999.0);
        assert_eq!(d.out_db, 12.0);
        d.set_param(4, -999.0);
        assert_eq!(d.out_db, -24.0);
    }

    #[test]
    fn distortion_hard_clip_at_zero_drive_is_identity() {
        // Drive=0 dB, Type=Hard, Bias=0, Tone=0, Out=0: a signal
        // inside [-1, +1] passes through bit-exactly. (Tone=0 is
        // gain_low = gain_high = 1 → exact identity, no LP/HP weight
        // change.)
        let mut d = DistortionEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 0.0);
        d.set_param(1, 0.0); // Hard
        d.set_param(2, 0.0);
        d.set_param(3, 0.0);
        d.set_param(4, 0.0);
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let x = 0.5 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, r) = d.process_sample(x, x);
            assert!(
                (l - x).abs() < 1e-5 && (r - x).abs() < 1e-5,
                "sample {i}: ({l},{r}) vs {x}"
            );
        }
    }

    #[test]
    fn distortion_every_shape_bounds_output_to_unit_range() {
        // Each clip type must keep its output in [-1, +1] for any
        // bounded input — even at +48 dB drive on a unit-amplitude
        // sine.
        for type_idx in 0..5 {
            let mut d = DistortionEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 48.0); // Max drive
            d.set_param(1, type_idx as f32);
            d.set_param(2, 0.0);
            d.set_param(3, 0.0);
            d.set_param(4, 0.0);
            for i in 0..2_000 {
                let t = i as f32 / 48_000.0;
                let x = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
                let (l, r) = d.process_sample(x, x);
                assert!(
                    l.abs() <= 1.0 + 1e-4 && r.abs() <= 1.0 + 1e-4,
                    "type {type_idx} sample {i} out of range: ({l},{r})"
                );
            }
        }
    }

    #[test]
    fn distortion_soft_clip_is_smoother_than_hard_clip() {
        // Compare the per-sample second-difference (a rough
        // "derivative discontinuity" proxy) between Hard and Soft at
        // equal drive. Hard clip has square corners at ±1 → large
        // 2nd diff; Soft is C∞ → small.
        let test = |type_idx: f32| {
            let mut d = DistortionEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 18.0);
            d.set_param(1, type_idx);
            d.set_param(2, 0.0);
            d.set_param(3, 0.0);
            d.set_param(4, 0.0);
            let mut prev = 0.0_f32;
            let mut prev2 = 0.0_f32;
            let mut roughness = 0.0_f32;
            for i in 0..4_800 {
                let t = i as f32 / 48_000.0;
                let x = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
                let (l, _) = d.process_sample(x, x);
                // 2nd difference ≈ d²/dt²
                roughness += (l - 2.0 * prev + prev2).abs();
                prev2 = prev;
                prev = l;
            }
            roughness
        };
        let hard = test(0.0);
        let soft = test(1.0);
        assert!(
            soft < hard,
            "soft clip should be smoother than hard; soft={soft}, hard={hard}"
        );
    }

    #[test]
    fn distortion_fold_reflects_at_unit_boundary() {
        // Probe the Fold transfer curve directly. fold(0.5) = 0.5
        // (linear region), fold(1.5) = 0.5 (reflected back from
        // peak at 1), fold(2.5) = -0.5, fold(3.5) = -0.5 (reflected
        // from -1).
        let cases = [
            (0.0_f32, 0.0_f32),
            (0.5, 0.5),
            (1.0, 1.0),
            (1.5, 0.5),
            (2.0, 0.0),
            (2.5, -0.5),
            (3.0, -1.0),
            (3.5, -0.5),
            (4.0, 0.0),
            (-0.5, -0.5),
            (-1.0, -1.0),
            (-1.5, -0.5),
        ];
        for (input, expected) in cases {
            let got = DistortionEffect::shape(input, DistortionEffect::TYPE_FOLD);
            assert!(
                (got - expected).abs() < 1e-5,
                "fold({input}) = {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn distortion_bias_zeroes_dc_for_every_shape() {
        // AC coupling check: silent input must produce silent output
        // for any Bias and any Type. (After the tilt LP settles —
        // the LP integrates a constant input toward that constant,
        // and 0 settles to 0 immediately.)
        for type_idx in 0..5 {
            for bias in [-100.0, -50.0, 50.0, 100.0] {
                let mut d = DistortionEffect::new();
                d.set_sample_rate(48_000.0);
                d.set_param(0, 0.0);
                d.set_param(1, type_idx as f32);
                d.set_param(2, bias);
                d.set_param(3, 0.0);
                d.set_param(4, 0.0);
                // Settle the tilt-LP a bit (it's already at 0 → 0,
                // so this is just defensive).
                for _ in 0..100 {
                    let (l, r) = d.process_sample(0.0, 0.0);
                    assert!(
                        l.abs() < 1e-5 && r.abs() < 1e-5,
                        "silent-input DC leak: type={type_idx} bias={bias} got ({l},{r})"
                    );
                }
            }
        }
    }

    #[test]
    fn distortion_positive_bias_clips_positive_peaks_more() {
        // Hard clip at heavy drive + positive bias → positive peaks
        // squashed harder than negative peaks. Measure peak-asymmetry
        // (max - min ≠ 0 in absolute terms — both peaks reach the
        // clipper but the positive side spends more samples saturated).
        let test = |bias: f32| {
            let mut d = DistortionEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 24.0);
            d.set_param(1, 0.0); // Hard
            d.set_param(2, bias);
            d.set_param(3, 0.0);
            d.set_param(4, 0.0);
            let mut max_pos = 0.0_f32;
            let mut min_neg = 0.0_f32;
            for i in 0..4_800 {
                let t = i as f32 / 48_000.0;
                let x = 0.5 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
                let (l, _) = d.process_sample(x, x);
                if l > max_pos {
                    max_pos = l;
                }
                if l < min_neg {
                    min_neg = l;
                }
            }
            (max_pos, min_neg)
        };
        let (pos_sym, neg_sym) = test(0.0);
        let (pos_bias, neg_bias) = test(60.0);
        // Symmetric case: positive and negative peaks roughly equal
        // in magnitude. Biased case: positive peak smaller than
        // negative peak's magnitude (the bias-DC subtraction shifts
        // the whole post-clip waveform down).
        assert!(
            (pos_sym + neg_sym).abs() < 0.05,
            "symmetric case should have ≈ equal peaks: pos={pos_sym} neg={neg_sym}"
        );
        assert!(
            pos_bias < pos_sym - 0.05 && neg_bias.abs() > neg_sym.abs() + 0.05,
            "positive bias should squash + peak and grow - peak: pos {pos_sym}→{pos_bias}, neg {neg_sym}→{neg_bias}"
        );
    }

    #[test]
    fn distortion_tone_positive_brightens_negative_darkens() {
        // Pink-noise-ish input through positive-Tone vs negative-Tone
        // distortion. Positive Tone should yield higher HF content
        // (larger |Δy/Δt|); negative Tone should yield lower.
        let test = |tone: f32| {
            let mut d = DistortionEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 6.0);
            d.set_param(1, 1.0); // Soft
            d.set_param(2, 0.0);
            d.set_param(3, tone);
            d.set_param(4, 0.0);
            let mut prng: u32 = 1;
            let mut prev = 0.0_f32;
            let mut deriv = 0.0_f32;
            for _ in 0..24_000 {
                prng = prng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let n = (prng as i32 as f32) / (i32::MAX as f32);
                let (l, _) = d.process_sample(n * 0.3, n * 0.3);
                deriv += (l - prev).abs();
                prev = l;
            }
            deriv
        };
        let neutral = test(0.0);
        let bright = test(100.0);
        let dark = test(-100.0);
        assert!(
            bright > neutral,
            "positive tone should brighten: neutral={neutral}, bright={bright}"
        );
        assert!(
            dark < neutral,
            "negative tone should darken: neutral={neutral}, dark={dark}"
        );
    }

    #[test]
    fn distortion_out_gain_scales_output_linearly() {
        // Drive=0 Hard, Bias=0, Tone=0: out_db on top of dry pass-
        // through means output = dry × 10^(out_db/20). Compare two
        // settings: 0 dB and +6 dB.
        let test = |out_db: f32| {
            let mut d = DistortionEffect::new();
            d.set_sample_rate(48_000.0);
            d.set_param(0, 0.0);
            d.set_param(1, 0.0);
            d.set_param(2, 0.0);
            d.set_param(3, 0.0);
            d.set_param(4, out_db);
            let mut peak = 0.0_f32;
            for i in 0..1_024 {
                let t = i as f32 / 48_000.0;
                let x = 0.3 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
                let (l, _) = d.process_sample(x, x);
                peak = peak.max(l.abs());
            }
            peak
        };
        let unity = test(0.0);
        let plus_six = test(6.0);
        // +6 dB ≈ × 1.995
        let ratio = plus_six / unity;
        assert!(
            (ratio - 1.995).abs() < 0.05,
            "Out=+6 dB should ~double output; ratio={ratio}"
        );
    }

    #[test]
    fn distortion_stays_bounded_under_aggressive_sweep() {
        let mut d = DistortionEffect::new();
        d.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            d.set_param(0, (i as f32 / 4_096.0).fract() * 48.0);
            d.set_param(1, (i as f32 / 1_000.0).fract() * 4.99);
            d.set_param(2, (i as f32 / 3_000.0).fract() * 200.0 - 100.0);
            d.set_param(3, (i as f32 / 5_000.0).fract() * 200.0 - 100.0);
            d.set_param(4, (i as f32 / 7_000.0).fract() * 36.0 - 24.0);
            let x = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = d.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Worst case: Hard clip at +1 → ±1, × +12 dB Out (4×) ×
            // Tone=±100 % which doubles the HP or LP weight (2×) =
            // 8× the clip output. Allow generous headroom for the
            // tilt-LP transient overshoot at sweep boundaries.
            assert!(
                l.abs() < 16.0 && r.abs() < 16.0,
                "sample {i} blew up: ({l},{r})"
            );
        }
    }

    #[test]
    fn distortion_reset_clears_tilt_state() {
        let mut d = DistortionEffect::new();
        d.set_sample_rate(48_000.0);
        d.set_param(0, 18.0);
        d.set_param(3, 50.0); // Non-zero Tone so the tilt LP charges
        for _ in 0..1_000 {
            let _ = d.process_sample(0.7, 0.7);
        }
        assert!(d.tilt_state_l.abs() > 0.0 || d.tilt_state_r.abs() > 0.0);
        d.reset();
        assert_eq!(d.tilt_state_l, 0.0);
        assert_eq!(d.tilt_state_r, 0.0);
    }
}
