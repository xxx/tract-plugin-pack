use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Single-knob stereo widener. Decodes the input to mid/side, scales
/// the side, re-encodes. The `Width` law matches the `imagine` plugin
/// (Ozone-style scale-the-side): mid is preserved, side is multiplied
/// by `Width / 100`. At `Width = 0` the output collapses to mono; at
/// `Width = 100` it's identity; at `Width = 200` the side is doubled
/// (wider-than-stereo image).
///
/// No state (memoryless transform); no allocations.
pub struct StereoWidenerEffect {
    width_pct: f32,
}

impl StereoWidenerEffect {
    const PARAMS: [ParamSpec; 1] = [ParamSpec {
        name: "Width",
        min: 0.0,
        max: 200.0,
        default: 100.0,
        scaling: ParamScaling::Linear,
        format: ParamFormat::Number { decimals: 0, unit: "%" },
    }];

    pub fn new() -> Self {
        Self {
            width_pct: Self::PARAMS[0].default,
        }
    }
}

impl Default for StereoWidenerEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for StereoWidenerEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let w = (self.width_pct * 0.01).max(0.0);
        let m = (left + right) * 0.5;
        let s = (left - right) * 0.5;
        let s_scaled = s * w;
        (m + s_scaled, m - s_scaled)
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn reset(&mut self) {}

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.width_pct = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_declared() {
        let e = StereoWidenerEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "Width");
    }

    #[test]
    fn width_100_is_identity() {
        let mut e = StereoWidenerEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 100.0);
        for i in 0..256 {
            let l = (i as f32 * 0.01).sin();
            let r = (i as f32 * 0.013).sin() * 0.7;
            let (yl, yr) = e.process_sample(l, r);
            assert!((yl - l).abs() < 1e-6);
            assert!((yr - r).abs() < 1e-6);
        }
    }

    #[test]
    fn width_zero_collapses_to_mono() {
        let mut e = StereoWidenerEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 0.0);
        for i in 0..256 {
            let l = (i as f32 * 0.01).sin();
            let r = (i as f32 * 0.013).sin() * 0.7;
            let (yl, yr) = e.process_sample(l, r);
            assert!((yl - yr).abs() < 1e-6, "channels diverge: {yl} vs {yr}");
            let m = (l + r) * 0.5;
            assert!((yl - m).abs() < 1e-6, "not the mid sum");
        }
    }

    #[test]
    fn width_200_doubles_the_side() {
        let mut e = StereoWidenerEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 200.0);
        // L = 1, R = -1: pure side. Doubling the side should give
        // (mid + 2*side, mid - 2*side) = (2, -2).
        let (yl, yr) = e.process_sample(1.0, -1.0);
        assert!((yl - 2.0).abs() < 1e-6);
        assert!((yr - -2.0).abs() < 1e-6);
    }

    #[test]
    fn mono_input_is_untouched() {
        // L == R means pure mid, zero side. Side scaling can't change
        // the output -- it should be transparent at any Width.
        let mut e = StereoWidenerEffect::new();
        e.set_sample_rate(48_000.0);
        for &w in &[0.0, 50.0, 100.0, 150.0, 200.0] {
            e.set_param(0, w);
            let (yl, yr) = e.process_sample(0.5, 0.5);
            assert!((yl - 0.5).abs() < 1e-6, "Width={w}: {yl}");
            assert!((yr - 0.5).abs() < 1e-6, "Width={w}: {yr}");
        }
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = StereoWidenerEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, -0.25);
        assert!(l.is_finite());
    }
}
