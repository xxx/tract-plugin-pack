use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Buchla 259-style west-coast wavefolder with antiderivative
/// anti-aliasing (ADAA1). Distinct from the simple triangle Fold shape
/// in `Distortion`: a five-cell cascade of `G*x - B*sign(x)`-style
/// folding cells (each with its own threshold and mix factor) produces
/// the characteristic bell-like harmonic redistribution rather than
/// piling odd harmonics on the fundamental. Cell coefficients are
/// lifted directly from ChowDSP's `WestCoastWavefolder`; the derivation
/// is in Chowdhury, "Virtual Analog Buchla 259 Wavefolder" (DAFx-19).
///
/// **ADAA1** turns the wavefolder's vicious harmonic content into a
/// soft-knee version that aliases far less. Per sample it stores the
/// previous input, then evaluates the antiderivative of the shape at
/// `x[n]` and `x[n-1]` and divides by `dx`. With `|dx|` tiny it falls
/// back to the direct shape to avoid divide-by-near-zero. The net
/// effect is roughly equivalent to 4x oversampling without the latency
/// or CPU of a polyphase filter pair. No double-tick required.
///
/// A one-pole 10 Hz output DC-blocker catches the offset that asymmetric
/// (Bias != 0) folding introduces.
///
/// Per channel state: one f32 (previous input for ADAA1) plus two f32s
/// (DC blocker). No allocations on the audio thread.
pub struct WavefolderEffect {
    drive_db: f32,
    fold_pct: f32,
    bias_pct: f32,
    sample_rate: f32,

    drive_lin: f32,
    fold_scale: f32,
    bias_amt: f32,
    /// One-pole DC-blocker coefficient (`R` in `y = x - x_n1 + R*y_n1`).
    dc_r: f32,

    /// Previous input to the waveshaping function, per channel. Used by
    /// ADAA1 as `x[n-1]`.
    x_n1: [f32; 2],
    /// DC blocker input-side state (previous unfiltered sample).
    dc_in: [f32; 2],
    /// DC blocker output-side state (previous filtered sample).
    dc_out: [f32; 2],
}

/// One Buchla cell: `func(x) = G*x - B*sign(x)` if `|x| > thresh`, else 0,
/// mixed in at `mix`. Constants lifted from ChowDSP's WestCoastWavefolder.
struct FolderCell {
    g: f32,
    b: f32,
    thresh: f32,
    mix: f32,
}

impl FolderCell {
    /// Continuity offset: `0.5*G*thresh^2 - B*thresh`. The antiderivative
    /// needs this subtracted so it matches the inner-region's zero at
    /// the threshold boundary.
    const fn bp(&self) -> f64 {
        let g = self.g as f64;
        let threshold = self.thresh as f64;
        0.5 * g * threshold * threshold - self.b as f64 * threshold
    }

    /// Shape contribution at `x`.
    #[inline]
    fn func(&self, x: f32) -> f32 {
        if x.abs() > self.thresh {
            self.g * x - self.b * x.signum()
        } else {
            0.0
        }
    }

    /// First antiderivative of `func` at `x` (the `F` in ADAA1). Made
    /// continuous at the threshold by subtracting `Bp`.
    #[inline]
    fn func_ad1(&self, x: f32) -> f64 {
        if x.abs() > self.thresh {
            let x = f64::from(x);
            0.5 * f64::from(self.g) * x * x - f64::from(self.b) * x.abs() - self.bp()
        } else {
            0.0
        }
    }
}

/// Five-cell Buchla 259 cascade. Coefficients from ChowDSP's
/// WestCoastWavefolder; derivation from the desmos linked in that
/// header file's docs.
const CELLS: [FolderCell; 5] = [
    FolderCell {
        g: 0.8333,
        b: 0.5,
        thresh: 0.6,
        mix: -12.0,
    },
    FolderCell {
        g: 0.3768,
        b: 1.1281,
        thresh: 2.994,
        mix: -27.777,
    },
    FolderCell {
        g: 0.2829,
        b: 1.5446,
        thresh: 5.46,
        mix: -21.428,
    },
    FolderCell {
        g: 0.5743,
        b: 1.0338,
        thresh: 1.8,
        mix: 17.647,
    },
    FolderCell {
        g: 0.2673,
        b: 1.0907,
        thresh: 4.08,
        mix: 36.363,
    },
];

/// Linear `x` contribution baked into the canonical Buchla shape.
/// Equivalent to ChowDSP's `xMix`.
const X_MIX: f32 = 5.0;

/// Max one-sided bias added pre-fold at Bias = +/- 100 %. Picked so even
/// at extreme asymmetry the bias stays within the folding range without
/// going pure-DC and silent.
const MAX_BIAS: f32 = 2.0;

/// ADAA1 numerical fallback threshold. When `|x[n] - x[n-1]| < EPS`
/// the integral-divided-by-dx becomes unstable; fall back to direct
/// shape evaluation.
const ADAA_EPS: f32 = 1e-5;

impl WavefolderEffect {
    pub(super) const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Drive",
            min: 0.0,
            max: 24.0,
            default: 6.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
        ParamSpec {
            name: "Fold",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
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
    ];

    pub fn new() -> Self {
        let mut e = Self {
            drive_db: Self::PARAMS[0].default,
            fold_pct: Self::PARAMS[1].default,
            bias_pct: Self::PARAMS[2].default,
            sample_rate: 48_000.0,
            drive_lin: 1.0,
            fold_scale: 0.0,
            bias_amt: 0.0,
            dc_r: 0.0,
            x_n1: [0.0; 2],
            dc_in: [0.0; 2],
            dc_out: [0.0; 2],
        };
        e.recompute();
        e
    }

    fn recompute(&mut self) {
        self.drive_lin = 10.0_f32.powf(self.drive_db.clamp(0.0, 24.0) / 20.0);
        self.fold_scale = (self.fold_pct * 0.01).clamp(0.0, 1.0);
        self.bias_amt = (self.bias_pct * 0.01).clamp(-1.0, 1.0) * MAX_BIAS;
        // One-pole HP at 10 Hz: y = x - x_n1 + R*y_n1, R = 1 - 2*pi*fc/sr.
        let fc = 10.0;
        let sr = self.sample_rate.max(1.0);
        self.dc_r = (1.0 - 2.0 * std::f32::consts::PI * fc / sr).clamp(0.0, 1.0);
    }

    /// Buchla 259 shape: `5*x` plus the per-cell folds, with `Fold`
    /// scaling all cell contributions in unison so `Fold = 0` is pure
    /// linear gain.
    #[inline]
    fn shape(&self, x: f32) -> f32 {
        let mut y = X_MIX * x;
        for cell in CELLS.iter() {
            y += self.fold_scale * cell.mix * cell.func(x);
        }
        y
    }

    /// First antiderivative of [`shape`] -- the `F` in ADAA1's
    /// `(F(x) - F(x_n1)) / (x - x_n1)`.
    /// f64 preserves the small difference between nearby antiderivatives.
    #[inline]
    fn shape_ad1(&self, x: f32) -> f64 {
        let x64 = f64::from(x);
        let mut y = 0.5 * f64::from(X_MIX) * x64 * x64;
        for cell in CELLS.iter() {
            y += f64::from(self.fold_scale) * f64::from(cell.mix) * cell.func_ad1(x);
        }
        y
    }

    /// ADAA1 evaluation. Returns the alias-suppressed waveshaping output
    /// for input `x` given the previous input `x_n1`. Falls back to the
    /// direct shape when `|dx|` is below the numerical threshold.
    #[inline]
    fn adaa1(&self, x: f32, x_n1: f32) -> f32 {
        let dx = f64::from(x) - f64::from(x_n1);
        if dx.abs() < f64::from(ADAA_EPS) {
            self.shape(x)
        } else {
            ((self.shape_ad1(x) - self.shape_ad1(x_n1)) / dx) as f32
        }
    }

    /// One-pole HP DC blocker. Updates the per-channel state and returns
    /// the filtered sample.
    #[inline]
    fn dc_block(&mut self, ch: usize, sample: f32) -> f32 {
        let y = sample - self.dc_in[ch] + self.dc_r * self.dc_out[ch];
        self.dc_in[ch] = sample;
        self.dc_out[ch] = y;
        y
    }

    /// Full per-channel pipeline: drive -> bias -> ADAA1 fold ->
    /// normalise by `X_MIX` -> DC block -> output trim.
    fn process_channel(&mut self, ch: usize, dry: f32) -> f32 {
        let x = self.drive_lin * dry + self.bias_amt;
        let x_n1 = self.x_n1[ch];
        let folded = self.adaa1(x, x_n1);
        self.x_n1[ch] = x;
        // Normalise by X_MIX so small-signal passes at unity (the shape's
        // baseline gain is X_MIX; dividing here makes Drive = 0, Fold = 0
        // a transparent passthrough).
        let normalised = folded * (1.0 / X_MIX);
        self.dc_block(ch, normalised)
    }
}

impl Default for WavefolderEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for WavefolderEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let l = self.process_channel(0, left);
        let r = self.process_channel(1, right);
        (l, r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.x_n1 = [0.0; 2];
        self.dc_in = [0.0; 2];
        self.dc_out = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.drive_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.fold_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.bias_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            _ => return,
        }
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_average(start: f32, end: f32, fold: f64) -> f64 {
        let low = f64::from(start.min(end));
        let high = f64::from(start.max(end));
        let mut boundaries = vec![low, high];
        for cell in &CELLS {
            for boundary in [-f64::from(cell.thresh), f64::from(cell.thresh)] {
                if low < boundary && boundary < high {
                    boundaries.push(boundary);
                }
            }
        }
        boundaries.sort_by(f64::total_cmp);
        // Midpoint quadrature is exact on each linear segment of the shape.
        let integral: f64 = boundaries
            .windows(2)
            .map(|interval| {
                let midpoint = (interval[0] + interval[1]) * 0.5;
                let mut value = 5.0 * midpoint;
                for cell in &CELLS {
                    if midpoint.abs() > f64::from(cell.thresh) {
                        value += fold
                            * f64::from(cell.mix)
                            * (f64::from(cell.g) * midpoint
                                - f64::from(cell.b) * midpoint.signum());
                    }
                }
                value * (interval[1] - interval[0])
            })
            .sum();
        integral / (high - low)
    }

    #[test]
    fn adaa_matches_piecewise_linear_integration_including_nearby_samples() {
        for fold in [0.0, 37.0, 100.0] {
            let mut effect = WavefolderEffect::new();
            effect.set_param(1, fold);
            for (start, end) in [
                (-7.0, 6.0),
                (0.2, 1.0),
                (1.4, 2.3),
                (2.8, 3.4),
                (3.8, 4.4),
                (5.2, 6.0),
                (6.0, 6.000012),
                (-6.000012, -6.0),
                (10.0, 10.000012),
            ] {
                let expected = reference_average(start, end, f64::from(fold) * 0.01);
                for (current, previous) in [(end, start), (start, end)] {
                    let actual = f64::from(effect.adaa1(current, previous));
                    assert!(
                        (actual - expected).abs() < 2e-5,
                        "fold {fold}, {previous} -> {current}: {actual}, expected {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn driven_nearby_samples_do_not_create_cancellation_spikes() {
        let mut effect = WavefolderEffect::new();
        effect.set_param(0, 24.0);
        effect.set_param(1, 100.0);
        let drive = 10.0_f32.powf(24.0 / 20.0);
        let first = 5.999988 / drive;
        let second = 6.0 / drive;
        let x0 = drive * first;
        let x1 = drive * second;
        let initial = reference_average(0.0, x0, 1.0) / 5.0;
        let next = reference_average(x0, x1, 1.0) / 5.0;
        let dc_r = 1.0 - 2.0 * std::f64::consts::PI * 10.0 / 48_000.0;
        effect.process_sample(first, -first);
        let output = effect.process_sample(second, -second);
        let expected = (next + (dc_r - 1.0) * initial) as f32;
        assert!(
            (output.0 - expected).abs() < 1e-5,
            "{output:?}, expected {expected}"
        );
        assert!((output.1 + expected).abs() < 1e-5);
    }

    #[test]
    fn cell_shape_stays_zero_at_both_threshold_boundaries() {
        for cell in &CELLS {
            assert_eq!(cell.func(cell.thresh), 0.0);
            assert_eq!(cell.func(-cell.thresh), 0.0);
        }
    }

    #[test]
    fn cell_shape_matches_known_linear_segments() {
        let cell = FolderCell {
            g: 2.0,
            b: 1.0,
            thresh: 0.5,
            mix: 1.0,
        };
        for (input, expected) in [
            (-2.0, -3.0),
            (-0.75, -0.5),
            (-0.5, 0.0),
            (-0.25, 0.0),
            (0.0, 0.0),
            (0.25, 0.0),
            (0.5, 0.0),
            (0.75, 0.5),
            (2.0, 3.0),
        ] {
            assert_eq!(cell.func(input), expected);
        }
    }

    #[test]
    fn steady_adaa_matches_the_local_linear_shape() {
        for fold in [0.0, 37.0, 100.0] {
            let mut effect = WavefolderEffect::new();
            effect.set_param(1, fold);
            for input in [-6.0, -4.5, -3.5, -2.3, -0.9, 0.9, 2.3, 3.5, 4.5, 6.0] {
                let expected =
                    reference_average(input - 0.01, input + 0.01, f64::from(fold) * 0.01);
                let actual = f64::from(effect.adaa1(input, input));
                assert!(
                    (actual - expected).abs() < 2e-5,
                    "fold {fold}, steady input {input}: {actual}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn folded_shape_matches_reference_points() {
        let mut effect = WavefolderEffect::new();
        effect.set_param(1, 100.0);
        for (input, expected) in [(0.9, 1.500_360_2), (3.5, 0.432_209_7), (6.0, 2.487_611_3)] {
            assert!((effect.adaa1(input, input) - expected).abs() < 2e-5);
            assert!((effect.adaa1(-input, -input) + expected).abs() < 2e-5);
        }
    }

    #[test]
    fn cell_antiderivative_matches_the_translated_integral() {
        for cell in &CELLS {
            for distance in [0.0, 0.01, 0.5, 2.0] {
                for sign in [-1.0, 1.0] {
                    let input = (cell.thresh + distance) * sign;
                    let d = f64::from(input.abs()) - f64::from(cell.thresh);
                    let expected = 0.5 * f64::from(cell.g) * d * d
                        + (f64::from(cell.g) * f64::from(cell.thresh) - f64::from(cell.b)) * d;
                    let actual = cell.func_ad1(input);
                    assert!((actual - expected).abs() < 1e-10 * (1.0 + expected.abs()));
                }
            }
        }
    }

    #[test]
    fn adaa_fallback_stops_strictly_below_epsilon() {
        let mut effect = WavefolderEffect::new();
        effect.set_param(1, 0.0);
        assert_eq!(effect.adaa1(0.0, 0.0), 0.0);
        for sign in [-1.0, 1.0] {
            let half_epsilon = sign * ADAA_EPS * 0.5;
            assert!((effect.adaa1(half_epsilon, 0.0) - 5.0 * half_epsilon).abs() < 1e-10);
            let epsilon = sign * ADAA_EPS;
            assert!((effect.adaa1(epsilon, 0.0) - 2.5 * epsilon).abs() < 1e-10);
            assert!((effect.adaa1(sign * 0.2, sign * 0.2) - sign).abs() < 1e-6);
        }
    }

    #[test]
    fn unfolded_impulse_matches_sample_averaging_and_the_dc_blocker() {
        for sample_rate in [24_000.0, 96_000.0] {
            for drive_db in [0.0, 12.0] {
                let mut effect = WavefolderEffect::new();
                effect.set_sample_rate(sample_rate);
                effect.set_param(0, drive_db);
                effect.set_param(1, 0.0);
                let drive = 10.0_f64.powf(f64::from(drive_db) / 20.0);
                let r = 1.0 - 2.0 * std::f64::consts::PI * 10.0 / f64::from(sample_rate);
                for sample in 0..32 {
                    let input = if sample == 0 { 0.125 } else { 0.0 };
                    let output = effect.process_sample(input, -input * 0.5);
                    let response = match sample {
                        0 => 0.5,
                        1 => 0.5 * r,
                        _ => 0.5 * (r * r - 1.0) * r.powi(sample - 2),
                    };
                    let expected = 0.125 * drive * response;
                    assert!((f64::from(output.0) - expected).abs() < 2e-6);
                    assert!((f64::from(output.1) + expected * 0.5).abs() < 2e-6);
                }
            }
        }
    }

    #[test]
    fn unfolded_bias_transient_matches_the_dc_blocker_response() {
        for bias_pct in [-37.0, 37.0] {
            let mut effect = WavefolderEffect::new();
            effect.set_param(1, 0.0);
            effect.set_param(2, bias_pct);
            let bias = f64::from(bias_pct) * 0.02;
            let r = 1.0 - 2.0 * std::f64::consts::PI * 10.0 / 48_000.0;
            for sample in 0..32 {
                let output = effect.process_sample(0.0, 0.0);
                let expected = if sample == 0 {
                    bias * 0.5
                } else {
                    bias * 0.5 * (1.0 + r) * r.powi(sample - 1)
                };
                assert!((f64::from(output.0) - expected).abs() < 5e-6);
                assert!((f64::from(output.1) - expected).abs() < 5e-6);
            }
        }
    }

    #[test]
    fn parameters_are_declared() {
        let e = WavefolderEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Drive");
        assert_eq!(specs[1].name, "Fold");
        assert_eq!(specs[2].name, "Bias");
    }

    #[test]
    fn silent_input_stays_silent_at_zero_bias() {
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert!(l.abs() < 1e-6 && r.abs() < 1e-6, "{l}, {r}");
        }
    }

    #[test]
    fn fold_zero_drive_zero_preserves_signal_level() {
        // With Fold = 0 the shape is purely linear (5x baked in, divided
        // back out at the output stage), so output RMS should closely
        // track input RMS. Compare RMS rather than sample-by-sample
        // because ADAA1 introduces a half-sample group delay and the DC
        // blocker adds a frequency-dependent phase delay -- so the
        // output is amplitude-preserving but not perfectly phase-aligned
        // with the dry input. RMS comparison cleanly isolates "wave
        // shape change" from "delay".
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 0.0);
        e.set_param(1, 0.0);
        e.set_param(2, 0.0);
        for i in 0..2048 {
            let x = (i as f32 * 0.1).sin();
            e.process_sample(x, x);
        }
        let mut sum_in = 0.0_f32;
        let mut sum_out = 0.0_f32;
        let n = 4096;
        for i in 2048..(2048 + n) {
            let x = (i as f32 * 0.1).sin();
            sum_in += x * x;
            let (l, _) = e.process_sample(x, x);
            sum_out += l * l;
        }
        let rms_in = (sum_in / n as f32).sqrt();
        let rms_out = (sum_out / n as f32).sqrt();
        assert!(
            (rms_out / rms_in - 1.0).abs() < 0.02,
            "RMS should match within 2%: rms_in={rms_in}, rms_out={rms_out}"
        );
    }

    #[test]
    fn high_drive_with_fold_creates_harmonics() {
        // A clean sine pushed hard into the folder should have meaningfully
        // different RMS than its input -- evidence the folder is doing
        // something. Compare RMS across Fold=0 vs Fold=100.
        let measure = |fold: f32| -> f32 {
            let mut e = WavefolderEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 18.0);
            e.set_param(1, fold);
            for i in 0..2048 {
                let x = (i as f32 * 0.05).sin() * 0.5;
                e.process_sample(x, x);
            }
            let mut sum = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let x = (i as f32 * 0.05).sin() * 0.5;
                let (l, _) = e.process_sample(x, x);
                sum += l * l;
            }
            (sum / 4096.0).sqrt()
        };
        let unfolded = measure(0.0);
        let folded = measure(100.0);
        assert!(
            (folded - unfolded).abs() > 0.01,
            "fold should change output character (unfolded={unfolded}, folded={folded})"
        );
    }

    #[test]
    fn bias_introduces_asymmetric_distortion() {
        // With Bias != 0 the folded output should have different positive
        // and negative excursions (the asymmetric fold). The DC blocker
        // strips any net offset, but the *peak asymmetry* survives.
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 12.0);
        e.set_param(1, 100.0);
        e.set_param(2, 80.0);
        // Warm.
        for i in 0..2048 {
            let x = (i as f32 * 0.05).sin() * 0.5;
            e.process_sample(x, x);
        }
        let mut peak_pos = 0.0_f32;
        let mut peak_neg = 0.0_f32;
        for i in 2048..(2048 + 4096) {
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, _) = e.process_sample(x, x);
            peak_pos = peak_pos.max(l);
            peak_neg = peak_neg.min(l);
        }
        assert!(
            (peak_pos.abs() - peak_neg.abs()).abs() > 0.01,
            "Bias should produce asymmetric peaks (pos={peak_pos}, neg={peak_neg})"
        );
    }

    #[test]
    fn output_stays_finite_under_extreme_settings() {
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 24.0);
        e.set_param(1, 100.0);
        e.set_param(2, 100.0);
        for i in 0..16_384 {
            let x = (i as f32 * 0.05).sin();
            let (l, r) = e.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "lost stability: {l}, {r}");
        }
    }

    #[test]
    fn dc_blocker_strips_bias_offset() {
        // Run silent input with Bias != 0. Without the DC blocker the
        // wavefolder would output a constant offset (shape(bias_amt)).
        // After warm-up the steady-state output should be near zero.
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 0.0);
        e.set_param(1, 100.0);
        e.set_param(2, 100.0);
        // Generous warm-up: the DC blocker is one-pole at 10 Hz, ~30 ms
        // time constant at 48 kHz. Drain ~1 second before measuring.
        for _ in 0..48_000 {
            e.process_sample(0.0, 0.0);
        }
        let mut max_abs = 0.0_f32;
        for _ in 0..4096 {
            let (l, _) = e.process_sample(0.0, 0.0);
            max_abs = max_abs.max(l.abs());
        }
        assert!(
            max_abs < 0.01,
            "DC blocker should drain bias offset, got {max_abs}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            e.process_sample(0.5, -0.5);
        }
        e.reset();
        assert_eq!(e.x_n1, [0.0; 2]);
        assert_eq!(e.dc_in, [0.0; 2]);
        assert_eq!(e.dc_out, [0.0; 2]);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = WavefolderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
