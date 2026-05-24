use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Bit-depth reduction plus sample-rate reduction (sample-and-hold).
pub struct BitcrushEffect {
    bit_depth: f32,
    rate_reduction: f32,
    held: [f32; 2],
    phase: [f32; 2],
}

impl BitcrushEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Bit Depth",
            min: 1.0,
            max: 16.0,
            default: 16.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "bits",
            },
        },
        ParamSpec {
            name: "Rate Reduction",
            min: 1.0,
            max: 50.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "x",
            },
        },
    ];

    /// A `BitcrushEffect` at its default (near-clean) parameters.
    pub fn new() -> Self {
        Self {
            bit_depth: Self::PARAMS[0].default,
            rate_reduction: Self::PARAMS[1].default,
            held: [0.0; 2],
            phase: [Self::PARAMS[1].default; 2],
        }
    }

    /// Quantize `x` to the current bit depth.
    fn quantize(&self, x: f32) -> f32 {
        let levels = 2.0_f32.powf(self.bit_depth);
        let step = 2.0 / levels;
        (x / step).round() * step
    }

    /// One channel's sample-and-hold + quantization step.
    fn crush_step(&mut self, x: f32, ch: usize) -> f32 {
        self.phase[ch] += 1.0;
        if self.phase[ch] >= self.rate_reduction {
            self.phase[ch] -= self.rate_reduction;
            self.held[ch] = self.quantize(x);
        }
        self.held[ch]
    }
}

impl Default for BitcrushEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for BitcrushEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.crush_step(left, 0), self.crush_step(right, 1))
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn reset(&mut self) {
        self.held = [0.0; 2];
        self.phase = [self.rate_reduction; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.bit_depth = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.rate_reduction = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat};

    #[test]
    fn bitcrush_effect_parameters_are_declared() {
        let bc = BitcrushEffect::new();
        let specs = bc.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Bit Depth");
        assert_eq!(specs[1].name, "Rate Reduction");
    }

    #[test]
    fn bitcrush_effect_low_bit_depth_quantizes_coarsely() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 2.0);
        bc.set_param(1, 1.0);
        let crushed = bc.process_sample(0.1, 0.1).0;
        bc.set_param(0, 16.0);
        let clean = bc.process_sample(0.1, 0.1).0;
        assert!(
            (crushed - 0.1).abs() > (clean - 0.1).abs(),
            "2-bit ({crushed}) should distort more than 16-bit ({clean})"
        );
    }

    #[test]
    fn bitcrush_effect_rate_reduction_holds_samples() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 16.0);
        bc.set_param(1, 4.0);
        let first = bc.process_sample(1.0, 1.0).0;
        let held = bc.process_sample(-1.0, -1.0).0;
        assert!(
            (first - held).abs() < 1e-6,
            "rate reduction should hold the sample"
        );
    }

    #[test]
    fn bitcrush_effect_output_is_bounded() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 3.0);
        for &x in &[-1.0_f32, -0.3, 0.0, 0.42, 1.0] {
            let (l, r) = bc.process_sample(x, x);
            assert!(
                l.abs() <= 1.5 && r.abs() <= 1.5,
                "x {x} -> ({l},{r}) out of range"
            );
        }
    }

    #[test]
    fn bitcrush_effect_reset_clears_hold_state() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(1, 8.0);
        bc.process_sample(0.7, 0.7);
        bc.reset();
        let y = bc.process_sample(0.25, 0.25).0;
        assert!((y - 0.25).abs() < 0.1, "reset should re-sample, got {y}");
    }

    #[test]
    fn bitcrush_param_formats_carry_their_units() {
        let specs = BitcrushEffect::new().parameters();
        if let ParamFormat::Number { unit, .. } = specs[0].format {
            assert_eq!(unit, "bits");
        } else {
            panic!("bit-depth format should be Number");
        }
        if let ParamFormat::Number { unit, .. } = specs[1].format {
            assert_eq!(unit, "x");
        } else {
            panic!("rate-reduction format should be Number");
        }
    }
}
