use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Stereo auto-panner. A single LFO drives a balance position between
/// the left and right channels. At `Depth = 0%` the pan position stays
/// centred and both channels pass through unchanged; at `Depth = 100%`
/// the LFO sweeps the full range, attenuating one channel to silence at
/// each extreme. Shape selects sine / triangle / square / saw.
///
/// Uses the linear "balance" law (one channel fades to zero while the
/// other holds at unity) rather than constant-power. Constant-power
/// would give a -3 dB dip at centre which conflicts with the `Depth = 0`
/// = bypass contract; balance keeps the centre transparent. Per-sample
/// work: one LFO evaluation, two multiplies.
pub struct AutoPanEffect {
    rate_hz: f32,
    depth_pct: f32,
    shape_idx: f32,
    sample_rate: f32,
    phase: f32,
}

const AUTOPAN_SHAPE_LABELS: &[&str] = &["Sine", "Triangle", "Square", "Saw"];

const SHAPE_SINE: usize = 0;
const SHAPE_TRIANGLE: usize = 1;
const SHAPE_SQUARE: usize = 2;

impl AutoPanEffect {
    const RATE_MIN_HZ: f32 = 0.05;
    const RATE_MAX_HZ: f32 = 20.0;

    const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 1.0,
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
            max: (AUTOPAN_SHAPE_LABELS.len() - 1) as f32,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: AUTOPAN_SHAPE_LABELS,
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

    fn eval_lfo(&self) -> f32 {
        let p = self.phase;
        match self.shape_idx.round() as usize {
            SHAPE_SINE => (std::f32::consts::TAU * p).sin(),
            SHAPE_TRIANGLE => {
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
            _ => 2.0 * p - 1.0,
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

impl Default for AutoPanEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for AutoPanEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let lfo = self.eval_lfo();
        let depth = (self.depth_pct * 0.01).clamp(0.0, 1.0);
        // Pan position in [-1, +1]. Negative -> left, positive -> right.
        let pan = lfo * depth;
        // Balance law: each channel's gain stays at 1 while pan is on the
        // OTHER side, then fades linearly to 0 as pan reaches THIS side.
        let gain_l = 1.0 - pan.max(0.0);
        let gain_r = 1.0 + pan.min(0.0);
        self.advance_phase();
        (left * gain_l, right * gain_r)
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
                let max_idx = (AUTOPAN_SHAPE_LABELS.len() - 1) as f32;
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
        let e = AutoPanEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Rate");
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Shape");
    }

    #[test]
    fn depth_zero_is_bypass() {
        let mut e = AutoPanEffect::new();
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
    fn full_depth_silences_each_channel_at_extremes() {
        // A 1 Hz sine LFO at depth=100% must produce moments where one
        // channel is silent and the other isn't. Drive matched DC and
        // check both extremes occur.
        let mut e = AutoPanEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 1.0);
        e.set_param(1, 100.0);
        let mut min_l = f32::INFINITY;
        let mut min_r = f32::INFINITY;
        for _ in 0..48_000 {
            let (l, r) = e.process_sample(1.0, 1.0);
            min_l = min_l.min(l.abs());
            min_r = min_r.min(r.abs());
        }
        assert!(min_l < 0.01, "L should hit ~0 at the right extreme, got {min_l}");
        assert!(min_r < 0.01, "R should hit ~0 at the left extreme, got {min_r}");
    }

    #[test]
    fn output_never_exceeds_dry_amplitude() {
        // Balance law: gains are <= 1, so output magnitude never exceeds
        // input magnitude. Validate with DC.
        let mut e = AutoPanEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 100.0);
        for _ in 0..48_000 {
            let (l, r) = e.process_sample(1.0, 1.0);
            assert!(l <= 1.0 + 1e-6 && l >= -1e-6, "L out of range: {l}");
            assert!(r <= 1.0 + 1e-6 && r >= -1e-6, "R out of range: {r}");
        }
    }

    #[test]
    fn reset_zeroes_phase() {
        let mut e = AutoPanEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            e.process_sample(0.5, 0.5);
        }
        e.reset();
        assert_eq!(e.phase, 0.0);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = AutoPanEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
