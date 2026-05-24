use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use crate::compressor::Compressor;

/// Per-track soft-knee peak compressor. Thin wrapper around the
/// same `crate::compressor::Compressor` the master bus uses, so
/// behavior is identical: stereo-linked peak detection, fixed
/// 5 ms attack / 50 ms release / 6 dB soft knee, single common
/// gain to both channels.
///
/// **Threshold** (-60..0 dB) is where compression begins.
/// **Ratio** (1..20:1, log) is the per-dB slope above the knee --
/// log scaling makes the musically-spaced values (2:1, 4:1, 10:1)
/// evenly distributed on the dial.
///
/// The fixed attack/release/knee match the master bus's tuning --
/// "transparent peak catcher" character. For pumping or transient-
/// shaping use a different effect (the inner Compressor doesn't
/// expose those timings as parameters).
///
/// **Latency:** zero. **Per-sample work:** one envelope step + the
/// inner `compute_gain` call (a couple of log10s + a fast exp for
/// the dB->linear gain step). Set_param caches `threshold_lin` via
/// `db_to_linear_fast` so MSEG-modulating Threshold doesn't pay
/// the powf cost per sample.
pub struct CompressorEffect {
    inner: Compressor,
    threshold_db: f32,
    ratio: f32,
}

impl CompressorEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Threshold",
            min: -60.0,
            max: 0.0,
            default: -6.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
        ParamSpec {
            name: "Ratio",
            min: 1.0,
            max: 20.0,
            default: 4.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "x",
            },
        },
    ];

    pub fn new() -> Self {
        let threshold_db = Self::PARAMS[0].default;
        let ratio = Self::PARAMS[1].default;
        let mut inner = Compressor::new();
        inner.set_params(threshold_db, ratio);
        Self {
            inner,
            threshold_db,
            ratio,
        }
    }
}

impl Default for CompressorEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for CompressorEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.inner.process_sample(left, right)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.inner.set_sample_rate(sample_rate);
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.threshold_db = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.ratio = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => return,
        }
        // `Compressor::set_params` re-stores both values + the cached
        // `threshold_lin`. Cheap (one fast exp for the dB conversion);
        // no allocations. Re-applying both even on a single-param
        // change keeps the code path uniform.
        self.inner.set_params(self.threshold_db, self.ratio);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn compressor_lists_two_parameters_with_the_expected_specs() {
        let c = CompressorEffect::new();
        let specs = c.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Threshold");
        assert_eq!(specs[0].min, -60.0);
        assert_eq!(specs[0].max, 0.0);
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number { unit: "dB", .. }
        ));
        assert_eq!(specs[1].name, "Ratio");
        assert_eq!(specs[1].min, 1.0);
        assert_eq!(specs[1].max, 20.0);
        assert!(matches!(specs[1].scaling, ParamScaling::Log));
    }

    #[test]
    fn compressor_set_param_clamps_each_slot() {
        let mut c = CompressorEffect::new();
        c.set_param(0, 999.0);
        assert_eq!(c.threshold_db, 0.0);
        c.set_param(0, -999.0);
        assert_eq!(c.threshold_db, -60.0);
        c.set_param(1, 999.0);
        assert_eq!(c.ratio, 20.0);
        c.set_param(1, 0.0);
        assert_eq!(c.ratio, 1.0);
    }

    #[test]
    fn compressor_passes_signal_well_below_threshold() {
        // -12 dB DC under a -6 dB threshold: no GR, output ~= input.
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, -6.0);
        c.set_param(1, 4.0);
        let dc = 10.0_f32.powf(-12.0 / 20.0); // -12 dB
        for _ in 0..4_800 {
            let _ = c.process_sample(dc, dc);
        }
        let (l, r) = c.process_sample(dc, dc);
        assert!(
            (l - dc).abs() < 1e-3 && (r - dc).abs() < 1e-3,
            "below threshold should pass through; in={dc}, out=({l},{r})"
        );
    }

    #[test]
    fn compressor_reduces_gain_above_threshold() {
        // DC at 0 dB, threshold -6 dB, ratio 4:1: expected GR ~=
        // -6 * (1 - 1/4) = -4.5 dB (matches the inner Compressor's
        // own algorithm test).
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, -6.0);
        c.set_param(1, 4.0);
        let mut out = 0.0_f32;
        for _ in 0..4_800 {
            out = c.process_sample(1.0, 1.0).0;
        }
        let gr_db = 20.0 * out.max(1e-12).log10();
        assert!(
            (gr_db - (-4.5)).abs() < 0.5,
            "expected ~-4.5 dB GR, got {gr_db} dB"
        );
    }

    #[test]
    fn compressor_ratio_one_is_a_passthrough() {
        // Ratio = 1:1 -> compression slope is zero -> no GR even
        // above threshold.
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, -12.0);
        c.set_param(1, 1.0);
        for _ in 0..4_800 {
            let _ = c.process_sample(1.0, 1.0);
        }
        let (l, _) = c.process_sample(1.0, 1.0);
        assert!(
            (l - 1.0).abs() < 1e-3,
            "ratio 1:1 must pass through, got {l}"
        );
    }

    #[test]
    fn compressor_stereo_linked_uses_max_of_l_and_r() {
        // Asymmetric input: L = 1.0 (above threshold), R = 0.0
        // (silence). With stereo-linked detection both channels see
        // the same gain reduction, so R should be 0 (silence stays
        // silent) but the L peak is compressed.
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, -12.0);
        c.set_param(1, 8.0);
        let mut l_out = 0.0_f32;
        let mut r_out = 0.0_f32;
        for _ in 0..4_800 {
            let pair = c.process_sample(1.0, 0.0);
            l_out = pair.0;
            r_out = pair.1;
        }
        assert!(l_out < 1.0, "L should be compressed; got {l_out}");
        assert!(
            r_out.abs() < 1e-5,
            "R should remain silent (any gain * 0 = 0); got {r_out}"
        );
    }

    #[test]
    fn compressor_reset_clears_envelope() {
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, -12.0);
        c.set_param(1, 8.0);
        for _ in 0..1_000 {
            let _ = c.process_sample(1.0, 1.0);
        }
        c.reset();
        // First sample after reset: envelope = 0 -> no GR.
        let (l, _) = c.process_sample(0.1, 0.1);
        assert!(
            (l - 0.1).abs() < 1e-3,
            "after reset, unity gain at low level; got {l}"
        );
    }

    #[test]
    fn compressor_silent_input_produces_silent_output() {
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, -12.0);
        c.set_param(1, 8.0);
        for _ in 0..4_800 {
            let (l, r) = c.process_sample(0.0, 0.0);
            assert!(l == 0.0 && r == 0.0, "silent in -> silent out");
        }
    }

    #[test]
    fn compressor_stays_bounded_under_aggressive_sweep() {
        let mut c = CompressorEffect::new();
        c.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            // Sweep both params per-sample (worst case for set_param
            // path).
            c.set_param(0, (i as f32 / 4_000.0).fract() * 60.0 - 60.0);
            c.set_param(1, 1.0 + (i as f32 / 5_000.0).fract() * 19.0);
            let x = 0.8 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = c.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() <= 1.0 + 1e-3 && r.abs() <= 1.0 + 1e-3,
                "sample {i} exceeded input magnitude: ({l},{r})"
            );
        }
    }
}
