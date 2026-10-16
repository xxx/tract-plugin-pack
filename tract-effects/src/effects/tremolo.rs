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

    const PARAMS: [ParamSpec; 3] = [
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

    #[test]
    fn parameters_are_declared() {
        let e = TremoloEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Rate");
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Shape");
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
