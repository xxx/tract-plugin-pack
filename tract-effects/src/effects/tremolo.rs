use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Classic amplitude-modulation tremolo. A single LFO multiplies both
/// channels' gain in unison, swinging between unity (`1.0`) and
/// `1 - depth` at the LFO trough -- so `Depth = 0%` is bypass and
/// `Depth = 100%` is a full chop. Shape selects sine / triangle /
/// square / saw waveform for the LFO.
///
/// Per-sample work is one transcendental (sin/cos for sine; piecewise
/// math for the rest), one multiply per channel, one phase increment.
pub struct TremoloEffect {
    rate_hz: f32,
    depth_pct: f32,
    shape_idx: f32,
    sample_rate: f32,
    /// LFO phase in turns `[0, 1)`. Sample-rate-independent so changing
    /// SR doesn't desync.
    phase: f32,
}

const TREMOLO_SHAPE_LABELS: &[&str] = &["Sine", "Triangle", "Square", "Saw"];

const SHAPE_SINE: usize = 0;
const SHAPE_TRIANGLE: usize = 1;
const SHAPE_SQUARE: usize = 2;
// Saw uses the `_` arm in `eval_lfo`.

impl TremoloEffect {
    const RATE_MIN_HZ: f32 = 0.05;
    const RATE_MAX_HZ: f32 = 20.0;

    pub(super) const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 5.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Depth",
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
            name: "Shape",
            min: 0.0,
            max: (TREMOLO_SHAPE_LABELS.len() - 1) as f32,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: TREMOLO_SHAPE_LABELS,
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            rate_hz: Self::PARAMS[0].default,
            depth_pct: Self::PARAMS[1].default,
            shape_idx: Self::PARAMS[2].default,
            sample_rate: 48_000.0,
            phase: 0.0,
        }
    }

    /// Evaluate the LFO at the current phase, returning a `[-1, 1]` value.
    fn eval_lfo(&self) -> f32 {
        let p = self.phase;
        match self.shape_idx.round() as usize {
            SHAPE_SINE => (std::f32::consts::TAU * p).sin(),
            SHAPE_TRIANGLE => {
                // Triangle starting at 0, ramping to +1 at 0.25, back to
                // 0 at 0.5, to -1 at 0.75, back to 0 at 1.0.
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            SHAPE_SQUARE => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => 2.0 * p - 1.0, // Saw
        }
    }

    fn advance_phase(&mut self) {
        let inc = self.rate_hz / self.sample_rate.max(1.0);
        self.phase += inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
    }
}

impl Default for TremoloEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for TremoloEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let lfo = self.eval_lfo();
        // Unipolar in [0, 1]: 1 at LFO peak, 0 at trough.
        let unipolar = (lfo + 1.0) * 0.5;
        let depth = (self.depth_pct * 0.01).clamp(0.0, 1.0);
        // gain = 1 at the peak, (1 - depth) at the trough.
        let gain = 1.0 - depth * (1.0 - unipolar);
        self.advance_phase();
        (left * gain, right * gain)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rate_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.depth_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => {
                let max_idx = (TREMOLO_SHAPE_LABELS.len() - 1) as f32;
                self.shape_idx = value.round().clamp(0.0, max_idx);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_effect() -> TremoloEffect {
        let mut effect = TremoloEffect::new();
        effect.set_sample_rate(32_768.0);
        for (index, value) in [16.0, 40.0, 3.0].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    fn assert_stereo_close(actual: (f32, f32), expected: (f32, f32)) {
        assert!(
            (actual.0 - expected.0).abs() < 2e-7,
            "{} vs {}",
            actual.0,
            expected.0
        );
        assert!(
            (actual.1 - expected.1).abs() < 2e-7,
            "{} vs {}",
            actual.1,
            expected.1
        );
    }

    #[test]
    fn all_shapes_follow_the_documented_gain_law_over_two_cycles() {
        // Eight independent waveform landmarks at 40% Depth.
        for (shape, gains) in [
            [
                (0.8, 0.8),
                (0.94142135623731, 0.94142135623731),
                (1.0, 1.0),
                (0.94142135623731, 0.94142135623731),
                (0.8, 0.8),
                (0.65857864376269, 0.65857864376269),
                (0.6, 0.6),
                (0.65857864376269, 0.65857864376269),
            ],
            [
                (0.8, 0.8),
                (0.9, 0.9),
                (1.0, 1.0),
                (0.9, 0.9),
                (0.8, 0.8),
                (0.7, 0.7),
                (0.6, 0.6),
                (0.7, 0.7),
            ],
            [
                (1.0, 1.0),
                (1.0, 1.0),
                (1.0, 1.0),
                (1.0, 1.0),
                (0.6, 0.6),
                (0.6, 0.6),
                (0.6, 0.6),
                (0.6, 0.6),
            ],
            [
                (0.6, 0.6),
                (0.65, 0.65),
                (0.7, 0.7),
                (0.75, 0.75),
                (0.8, 0.8),
                (0.85, 0.85),
                (0.9, 0.9),
                (0.95, 0.95),
            ],
        ]
        .into_iter()
        .enumerate()
        {
            let mut effect = configured_effect();
            effect.set_param(2, shape as f32);
            for sample in 0..=4_096 {
                let (left, right) = effect.process_sample(0.75, -0.5);
                if sample % 256 == 0 {
                    let (gain_l, gain_r) = gains[(sample / 256) % 8];
                    assert!(
                        (left as f64 - 0.75 * gain_l).abs() < 1e-6,
                        "shape {shape}, sample {sample}: {left}"
                    );
                    assert!(
                        (right as f64 + 0.5 * gain_r).abs() < 1e-6,
                        "shape {shape}, sample {sample}: {right}"
                    );
                }
            }
        }
    }

    #[test]
    fn rate_and_sample_rate_changes_preserve_the_running_clock() {
        let mut effect = configured_effect();
        for _ in 0..1_024 {
            effect.process_sample(0.75, -0.5);
        }
        effect.set_param(0, 8.0);
        assert_stereo_close(effect.process_sample(0.75, -0.5), (0.6, -0.4));
        for _ in 1..512 {
            effect.process_sample(0.75, -0.5);
        }
        effect.set_sample_rate(16_384.0);
        assert_stereo_close(effect.process_sample(0.75, -0.5), (0.6375, -0.425));
        for _ in 1..256 {
            effect.process_sample(0.75, -0.5);
        }
        assert_stereo_close(effect.process_sample(0.75, -0.5), (0.675, -0.45));
    }

    #[test]
    fn zero_depth_keeps_the_clock_running_for_later_modulation() {
        let mut effect = configured_effect();
        effect.set_param(1, 0.0);
        for _ in 0..1_536 {
            assert_eq!(effect.process_sample(0.75, -0.5), (0.75, -0.5));
        }
        effect.set_param(1, 40.0);
        assert_stereo_close(effect.process_sample(0.75, -0.5), (0.675, -0.45));
    }

    #[test]
    fn reset_replays_a_fresh_clock_with_the_same_parameters() {
        let mut used = configured_effect();
        let mut fresh = configured_effect();
        for _ in 0..1_703 {
            used.process_sample(0.1, -0.2);
        }
        used.reset();
        for sample in 0..4_097 {
            let input = ((sample as f32 * 0.13).sin(), (sample as f32 * 0.29).cos());
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn parameters_clamp_and_round_shape_without_retriggering() {
        for (index, value, endpoint) in [
            (0, -1.0, 0.05),
            (0, 99.0, 20.0),
            (1, -1.0, 0.0),
            (1, 999.0, 100.0),
            (2, -1.0, 0.0),
            (2, 0.49, 0.0),
            (2, 0.5, 1.0),
            (2, 1.49, 1.0),
            (2, 1.5, 2.0),
            (2, 2.49, 2.0),
            (2, 2.5, 3.0),
            (2, 99.0, 3.0),
        ] {
            let mut clamped = configured_effect();
            let mut reference = configured_effect();
            for _ in 0..257 {
                clamped.process_sample(1.0, -0.5);
                reference.process_sample(1.0, -0.5);
            }
            clamped.set_param(index, value);
            reference.set_param(index, endpoint);
            clamped.set_param(99, -999.0);
            for sample in 0..4_097 {
                let input = ((sample as f32 * 0.17).sin(), (sample as f32 * 0.31).cos());
                assert_eq!(
                    clamped.process_sample(input.0, input.1),
                    reference.process_sample(input.0, input.1)
                );
            }
        }
    }

    #[test]
    fn parameters_are_declared() {
        let e = TremoloEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Rate");
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Shape");
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [(0.05, 20.0, 5.0), (0.0, 100.0, 50.0), (0.0, 3.0, 0.0)]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(matches!(
            specs[1].format,
            ParamFormat::Number {
                decimals: 0,
                unit: "%"
            }
        ));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Enum {
                labels: ["Sine", "Triangle", "Square", "Saw"]
            }
        ));
    }

    #[test]
    fn depth_zero_is_bypass() {
        let mut e = TremoloEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.0);
        for i in 0..512 {
            let x = (i as f32 * 0.01).sin();
            let (l, r) = e.process_sample(x, -x);
            assert!((l - x).abs() < 1e-6);
            assert!((r - -x).abs() < 1e-6);
        }
    }

    #[test]
    fn full_depth_chops_to_zero_at_trough() {
        // A 1 Hz sine LFO at depth=100% must produce a moment where the
        // gain is zero (the LFO trough). Drive a DC input and check we
        // see at least one near-zero output sample.
        let mut e = TremoloEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 1.0); // 1 Hz
        e.set_param(1, 100.0); // full depth
        let mut min_abs = f32::INFINITY;
        for _ in 0..48_000 {
            let (l, _) = e.process_sample(1.0, 1.0);
            min_abs = min_abs.min(l.abs());
        }
        assert!(
            min_abs < 0.01,
            "full-depth tremolo should chop to ~0, got min={min_abs}"
        );
    }

    #[test]
    fn output_never_exceeds_dry_amplitude() {
        // Tremolo only attenuates -- output should never exceed the dry
        // input amplitude. Drive at unity, check no sample is > 1.
        let mut e = TremoloEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 100.0);
        for _ in 0..48_000 {
            let (l, _) = e.process_sample(1.0, 1.0);
            assert!(l <= 1.0 + 1e-6, "tremolo amplified input: l={l}");
            assert!(l >= 0.0 - 1e-6, "tremolo went negative: l={l}");
        }
    }

    #[test]
    fn shape_selector_chooses_distinct_waveforms() {
        // Each shape should produce a distinctly different mean gain
        // pattern over a full LFO cycle. Compare RMS across shapes; they
        // should differ.
        let measure = |shape: f32| -> f32 {
            let mut e = TremoloEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 1.0);
            e.set_param(1, 100.0);
            e.set_param(2, shape);
            let mut sum = 0.0_f32;
            for _ in 0..48_000 {
                let (l, _) = e.process_sample(1.0, 1.0);
                sum += l * l;
            }
            (sum / 48_000.0).sqrt()
        };
        let sine = measure(0.0);
        let square = measure(2.0);
        assert!(
            (sine - square).abs() > 0.01,
            "sine and square should produce distinct RMS (sine={sine}, sq={square})"
        );
    }

    #[test]
    fn reset_zeroes_phase() {
        let mut e = TremoloEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            e.process_sample(0.5, 0.5);
        }
        e.reset();
        assert_eq!(e.phase, 0.0);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = TremoloEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
