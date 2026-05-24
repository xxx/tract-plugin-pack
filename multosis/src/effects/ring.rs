use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Ring modulator: multiplies the input by an internal carrier oscillator.
///
/// The classic Bode-style RM has no dry path — only the sum/difference
/// sidebands survive — which makes it clangy and metallic. Multosis's
/// **Bias** control morphs that hard ring-mod into amplitude modulation
/// (carrier offset toward +1 lets some dry through) or even into phase-
/// inverted dry (carrier toward −1) without leaving the same single
/// multiply. Formally `carrier = bias + (1 − |bias|)·wave`, so
/// `bias = 0` is pure RM, `bias = ±1` is straight ±dry, and intermediate
/// values produce continuous AM-style behaviour.
///
/// **Shape** picks the carrier waveform. Only bandlimited shapes are
/// offered (sine, triangle): saw/square multiplied with audio sprays
/// aliased sidebands that aren't musically useful in this context. Sine
/// is the canonical RM sound; triangle adds a bit of odd-harmonic crunch.
///
/// **Stereo** offsets the right channel's carrier phase by 0..180° (linear
/// in the parameter, so 100 % = π = antiphase). At 0 % the modulator is
/// mono; at 100 % L and R are antiphase. A single phase accumulator is
/// shared so the L/R phase relationship is stable across MSEG-modulated
/// frequency sweeps.
pub struct RingEffect {
    freq_hz: f32,
    shape_idx: f32,
    bias_pct: f32,
    stereo_pct: f32,
    sample_rate: f32,
    /// Carrier phase accumulator in [0, 1). R-channel phase is derived
    /// from this plus a Stereo-controlled offset, so changing Stereo on
    /// the fly doesn't desynchronise the channels.
    phase: f32,
}

impl RingEffect {
    /// 0.1 Hz lower bound → 10 s carrier period (very slow tremolo);
    /// 5 kHz upper bound covers the audible RM range without crowding
    /// Nyquist on lower sample rates.
    const FREQ_MIN_HZ: f32 = 0.1;
    const FREQ_MAX_HZ: f32 = 5_000.0;

    const SHAPE_LABELS: &'static [&'static str] = &["Sine", "Triangle"];
    const SHAPE_TRIANGLE: usize = 1;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Freq",
            min: Self::FREQ_MIN_HZ,
            max: Self::FREQ_MAX_HZ,
            default: 100.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Shape",
            min: 0.0,
            max: (Self::SHAPE_LABELS.len() - 1) as f32,
            default: 0.0, // Sine
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::SHAPE_LABELS,
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
            name: "Stereo",
            min: 0.0,
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
        Self {
            freq_hz: Self::PARAMS[0].default,
            shape_idx: Self::PARAMS[1].default,
            bias_pct: Self::PARAMS[2].default,
            stereo_pct: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            phase: 0.0,
        }
    }

    /// Evaluate the carrier wave at `phase` (in [0, 1) cycles). Both
    /// shapes are returned in the [−1, +1] range.
    #[inline]
    fn carrier_wave(phase: f32, shape_idx: usize) -> f32 {
        // Wrap phase into [0, 1). The accumulator already wraps each
        // sample, but adding the per-channel stereo offset can push it
        // past 1, so the floor-subtract is needed here too.
        let p = phase - phase.floor();
        if shape_idx == Self::SHAPE_TRIANGLE {
            // |p - 0.5| ∈ [0, 0.5]; scale to [-1, +1]: 4·|p-0.5| - 1.
            // At p=0 → +1 (positive peak), at p=0.5 → -1 (negative peak),
            // at p=1 → +1 again. Same phase reference as a cosine, which
            // matches the sine arm's `(2π·p).sin()` only when p=0
            // crosses zero — but for ring-mod purposes the absolute
            // phase reference is irrelevant.
            4.0 * (p - 0.5).abs() - 1.0
        } else {
            // Sine (the default / SHAPE_SINE arm).
            (2.0 * std::f32::consts::PI * p).sin()
        }
    }
}

impl Default for RingEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for RingEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let shape = (self.shape_idx.round() as usize).min(Self::SHAPE_LABELS.len() - 1);
        // bias ∈ [-1, +1]; mix ∈ [0, 1] is how much carrier wave survives.
        // bias=0 → mix=1 (pure RM); bias=±1 → mix=0 (straight ±dry).
        let bias = (self.bias_pct * 0.01).clamp(-1.0, 1.0);
        let mix = 1.0 - bias.abs();
        // Stereo: 0..100 % maps to 0..0.5 cycles = 0..180° phase offset
        // for the right carrier.
        let stereo_offset = (self.stereo_pct * 0.005).clamp(0.0, 0.5);
        let carrier_l = bias + mix * Self::carrier_wave(self.phase, shape);
        let carrier_r = bias + mix * Self::carrier_wave(self.phase + stereo_offset, shape);

        let out = (left * carrier_l, right * carrier_r);

        // Advance phase after evaluating both channels so they share the
        // same time index. `set_param` clamps Freq into the valid range,
        // so `phase_inc` is guaranteed small (< 0.5 even at 5 kHz / 11 kHz
        // SR worst case — comfortably under one full cycle per sample).
        let phase_inc = self.freq_hz / self.sample_rate;
        self.phase += phase_inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }

        out
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.freq_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => {
                let max_idx = (Self::SHAPE_LABELS.len() - 1) as f32;
                self.shape_idx = value.round().clamp(0.0, max_idx);
            }
            2 => self.bias_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.stereo_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn ring_lists_four_parameters_with_the_expected_specs() {
        let r = RingEffect::new();
        let specs = r.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Freq");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 0.1);
        assert_eq!(specs[0].max, 5_000.0);
        assert_eq!(specs[1].name, "Shape");
        assert!(matches!(specs[1].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[1].default, 0.0); // Sine
        assert_eq!(specs[2].name, "Bias");
        assert_eq!(specs[2].min, -100.0);
        assert_eq!(specs[2].max, 100.0);
        assert_eq!(specs[3].name, "Stereo");
        assert_eq!(specs[3].min, 0.0);
        assert_eq!(specs[3].max, 100.0);
    }

    #[test]
    fn ring_set_param_clamps_each_slot() {
        let mut r = RingEffect::new();
        // Freq clamps to [0.1, 5000].
        r.set_param(0, 50_000.0);
        assert_eq!(r.freq_hz, 5_000.0);
        r.set_param(0, 0.0);
        assert_eq!(r.freq_hz, 0.1);
        // Shape clamps to [0, SHAPE_LABELS.len() - 1] = [0, 1].
        r.set_param(1, 99.0);
        assert_eq!(r.shape_idx, 1.0);
        r.set_param(1, -5.0);
        assert_eq!(r.shape_idx, 0.0);
        // Bias clamps to [-100, +100].
        r.set_param(2, 999.0);
        assert_eq!(r.bias_pct, 100.0);
        r.set_param(2, -999.0);
        assert_eq!(r.bias_pct, -100.0);
        // Stereo clamps to [0, 100].
        r.set_param(3, 999.0);
        assert_eq!(r.stereo_pct, 100.0);
        r.set_param(3, -10.0);
        assert_eq!(r.stereo_pct, 0.0);
    }

    #[test]
    fn ring_bias_full_positive_is_dry_passthrough() {
        // Bias = +100 % → carrier = +1 regardless of shape/phase →
        // output should equal input exactly, sample for sample.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 1_000.0); // Any non-zero freq
        r.set_param(2, 100.0);
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(
                (l - dry).abs() < 1e-6 && (ri - dry).abs() < 1e-6,
                "bias=+100 must be dry passthrough; sample {i}: ({l},{ri}) vs {dry}"
            );
        }
    }

    #[test]
    fn ring_bias_full_negative_is_dry_inverted() {
        // Bias = -100 % → carrier = -1 → output = -input exactly.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 1_000.0);
        r.set_param(2, -100.0);
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
            let (l, _) = r.process_sample(dry, dry);
            assert!(
                (l - (-dry)).abs() < 1e-6,
                "bias=-100 must invert; sample {i}: {l} vs {}",
                -dry
            );
        }
    }

    #[test]
    fn ring_pure_rm_on_dc_input_traces_the_carrier() {
        // Bias = 0 (pure RM) on dry=1 means output equals the carrier
        // wave itself. For a sine carrier at 100 Hz, output should
        // average to zero and peak near ±1 over a full cycle.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 100.0); // 100 Hz carrier
        r.set_param(2, 0.0); // Pure RM
        let mut sum = 0.0_f32;
        let mut peak = 0.0_f32;
        // 480 samples = exactly one cycle at 100 Hz / 48 kHz.
        for _ in 0..480 {
            let (l, _) = r.process_sample(1.0, 1.0);
            sum += l;
            peak = peak.max(l.abs());
        }
        // Tolerance ≈ 0.01: 480 f32 accumulations plus a tiny phase-
        // increment quantization error (1/480 isn't finitely
        // representable in binary). The carrier itself is bit-exact;
        // this is just summation drift.
        assert!(
            sum.abs() < 1e-2,
            "DC × pure-RM sine carrier should integrate to 0 over a cycle, got {sum}"
        );
        assert!(
            (peak - 1.0).abs() < 1e-2,
            "carrier peak should be ±1 (got {peak})"
        );
    }

    #[test]
    fn ring_freq_determines_carrier_rate_via_zero_crossings() {
        // Pure RM on dry=1 puts a clean sine of frequency `f` at the
        // output. Count zero crossings over 1 s and verify it equals
        // 2·f within rounding.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        let f_hz = 250.0;
        r.set_param(0, f_hz);
        r.set_param(2, 0.0); // Pure RM
        let mut prev = 0.0_f32;
        let mut crossings = 0usize;
        for i in 0..48_000 {
            let (l, _) = r.process_sample(1.0, 1.0);
            // Skip the very first sample (prev is the initial 0 by accident).
            if i > 0 && prev.signum() != l.signum() && (prev != 0.0 || l != 0.0) {
                crossings += 1;
            }
            prev = l;
        }
        let expected = (2.0 * f_hz) as isize;
        let diff = (crossings as isize - expected).abs();
        assert!(
            diff <= 2,
            "expected ~{expected} zero crossings at {f_hz} Hz, got {crossings}"
        );
    }

    #[test]
    fn ring_triangle_shape_has_triangular_peak_distribution() {
        // A sine carrier's instantaneous value is concentrated near ±1
        // (arcsine PDF); a triangle's is uniform over [-1, +1]. Use
        // that to verify the Shape selector actually switches waveforms:
        // count samples whose magnitude exceeds 0.7. For sine over one
        // cycle ~50 % are above 0.7; for triangle only ~30 %.
        let mut r_sine = RingEffect::new();
        r_sine.set_sample_rate(48_000.0);
        r_sine.set_param(0, 100.0);
        r_sine.set_param(1, 0.0); // Sine
        r_sine.set_param(2, 0.0); // Pure RM
        let mut r_tri = RingEffect::new();
        r_tri.set_sample_rate(48_000.0);
        r_tri.set_param(0, 100.0);
        r_tri.set_param(1, 1.0); // Triangle
        r_tri.set_param(2, 0.0);

        // 4800 samples = 10 full cycles at 100 Hz / 48 kHz.
        let mut sine_above = 0usize;
        let mut tri_above = 0usize;
        for _ in 0..4_800 {
            let (s, _) = r_sine.process_sample(1.0, 1.0);
            let (t, _) = r_tri.process_sample(1.0, 1.0);
            if s.abs() > 0.7 {
                sine_above += 1;
            }
            if t.abs() > 0.7 {
                tri_above += 1;
            }
        }
        // Sine: ~50 % above 0.7. Triangle: ~30 %. Demand a clear gap.
        assert!(
            sine_above > tri_above + 500,
            "sine should spend much more time near ±1 than triangle (sine={sine_above}, tri={tri_above})"
        );
    }

    #[test]
    fn ring_stereo_zero_collapses_to_mono() {
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 333.0);
        r.set_param(2, 0.0); // Pure RM
        r.set_param(3, 0.0); // Stereo = 0
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 500.0 * t).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(
                (l - ri).abs() < 1e-6,
                "Stereo = 0 must give L == R, sample {i}: {l} vs {ri}"
            );
        }
    }

    #[test]
    fn ring_stereo_100_inverts_right_carrier() {
        // Stereo = 100 % → R carrier is offset by 0.5 cycle (180°) →
        // for a sine carrier, R = -L · (dry). With identical L/R dry,
        // output_r should equal -output_l on every sample.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 500.0);
        r.set_param(2, 0.0); // Pure RM
        r.set_param(3, 100.0); // 180° offset
        for i in 0..1_024 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(
                (l + ri).abs() < 1e-4,
                "Stereo = 100 should antiphase the channels, sample {i}: ({l},{ri})"
            );
        }
    }

    #[test]
    fn ring_stays_bounded_under_aggressive_freq_sweep() {
        // RM has no feedback path so output is at most |dry|·|carrier| ≤
        // |dry|·1, but we still want to make sure phase wrapping doesn't
        // produce NaNs at extreme rates.
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(2, 0.0);
        r.set_param(3, 100.0);
        for i in 0..48_000 {
            // Sweep Freq from 0.1 Hz to 5 kHz log over 1 second.
            let p = (i as f32 / 4096.0).fract();
            let freq = 0.1 * 50_000.0_f32.powf(p);
            r.set_param(0, freq);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(l.is_finite() && ri.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() <= 0.5 + 1e-6 && ri.abs() <= 0.5 + 1e-6,
                "sample {i} exceeded |dry|: ({l},{ri})"
            );
        }
    }

    #[test]
    fn ring_reset_clears_phase() {
        let mut r = RingEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 1_000.0);
        for _ in 0..500 {
            let _ = r.process_sample(0.5, 0.5);
        }
        assert!(r.phase != 0.0, "phase should have advanced");
        r.reset();
        assert_eq!(r.phase, 0.0);
        // First sample after reset: phase = 0, sine carrier at 0 = 0,
        // pure-RM (bias=0) → output = dry · 0 = 0.
        let (l, ri) = r.process_sample(0.7, 0.7);
        assert!(
            l.abs() < 1e-6 && ri.abs() < 1e-6,
            "post-reset first sample should be 0 for sine RM at phase 0, got ({l},{ri})"
        );
    }
}
