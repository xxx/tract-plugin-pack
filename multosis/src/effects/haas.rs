use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Haas / precedence-effect stereo delay. Delays one channel by a
/// short time (<= 25 ms), causing the brain to interpret the signal
/// as coming from the un-delayed side via the precedence effect.
/// Useful for thickening mono sources or pushing a signal off-centre
/// without changing its level balance.
///
/// **Position** is signed: negative delays the *left* channel
/// (so the apparent source pushes right), positive delays the *right*
/// (source pushes left). `Position = 0` is identity. Capped at 25 ms
/// to stay below the ~30 ms threshold where the delayed channel
/// starts being heard as a separate echo rather than blending into
/// the image.
///
/// **Latency:** asymmetric L/R delay is the *point* of the effect; we
/// don't try to compensate it. The delayed channel inherently lags
/// the other by up to 25 ms.
///
/// Per-channel state: one delay ring buffer per channel, sized for
/// the worst case (30 ms at 192 kHz, rounded to the next power of
/// two for bitmask wrap).
pub struct HaasEffect {
    position_ms: f32,
    sample_rate: f32,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,
}

impl HaasEffect {
    /// Worst-case delay in samples: 30 ms at 192 kHz = 5760. Round
    /// to the next power of two for a bitmask-wrapped ring.
    const RING_CAP: usize = 8_192;
    const RING_MASK: usize = Self::RING_CAP - 1;

    const PARAMS: [ParamSpec; 1] = [ParamSpec {
        name: "Position",
        min: -25.0,
        max: 25.0,
        default: 0.0,
        scaling: ParamScaling::Linear,
        format: ParamFormat::Number {
            decimals: 1,
            unit: "ms",
        },
    }];

    pub fn new() -> Self {
        Self {
            position_ms: Self::PARAMS[0].default,
            sample_rate: 48_000.0,
            delay_l: vec![0.0; Self::RING_CAP],
            delay_r: vec![0.0; Self::RING_CAP],
            write_idx: 0,
        }
    }

    /// Integer-sample read from a delay ring. The Haas effect doesn't
    /// need sub-sample precision -- a 1-sample step is well below
    /// the ~6 dB localisation threshold for inter-aural time
    /// difference.
    #[inline]
    fn read(buf: &[f32], write_idx: usize, offset_samples: usize) -> f32 {
        buf[(write_idx + Self::RING_CAP - offset_samples) & Self::RING_MASK]
    }
}

impl Default for HaasEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for HaasEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Always write current samples into both rings so changing
        // Position direction doesn't drop history.
        self.delay_l[self.write_idx] = left;
        self.delay_r[self.write_idx] = right;
        let offset = (self.position_ms.abs() * 0.001 * self.sample_rate.max(1.0)).round() as usize;
        let offset = offset.min(Self::RING_CAP - 1);
        let (l_out, r_out) = if offset == 0 {
            (left, right)
        } else if self.position_ms >= 0.0 {
            // Positive: delay R. Apparent source pushes left.
            (left, Self::read(&self.delay_r, self.write_idx, offset))
        } else {
            // Negative: delay L. Apparent source pushes right.
            (Self::read(&self.delay_l, self.write_idx, offset), right)
        };
        self.write_idx = (self.write_idx + 1) & Self::RING_MASK;
        (l_out, r_out)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn reset(&mut self) {
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.write_idx = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.position_ms = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_declared() {
        let e = HaasEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "Position");
    }

    #[test]
    fn position_zero_is_identity() {
        let mut e = HaasEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 0.0);
        for i in 0..256 {
            let l = (i as f32 * 0.01).sin();
            let r = (i as f32 * 0.013).sin() * 0.7;
            let (yl, yr) = e.process_sample(l, r);
            assert!((yl - l).abs() < 1e-6);
            assert!((yr - r).abs() < 1e-6);
        }
    }

    #[test]
    fn positive_position_delays_right_channel() {
        // With Position = +10 ms the right channel should be delayed
        // by 10 ms = 480 samples at 48 kHz. Send an impulse on the
        // right channel and confirm it appears 480 samples later.
        let mut e = HaasEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 10.0);
        // Impulse on right at t=0.
        e.process_sample(0.0, 1.0);
        for _ in 0..479 {
            let (_, r) = e.process_sample(0.0, 0.0);
            assert!(r.abs() < 1e-6, "right leaked before the delay");
        }
        let (_, r) = e.process_sample(0.0, 0.0);
        assert!(
            (r - 1.0).abs() < 1e-6,
            "expected impulse at sample 480, got {r}"
        );
    }

    #[test]
    fn negative_position_delays_left_channel() {
        let mut e = HaasEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, -10.0);
        e.process_sample(1.0, 0.0);
        for _ in 0..479 {
            let (l, _) = e.process_sample(0.0, 0.0);
            assert!(l.abs() < 1e-6);
        }
        let (l, _) = e.process_sample(0.0, 0.0);
        assert!(
            (l - 1.0).abs() < 1e-6,
            "expected impulse at sample 480, got {l}"
        );
    }

    #[test]
    fn un_delayed_channel_passes_at_unity() {
        // With Position > 0 only R is delayed; L should pass through
        // unchanged sample-by-sample.
        let mut e = HaasEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 15.0);
        for i in 0..1024 {
            let x = (i as f32 * 0.05).sin();
            let (l, _) = e.process_sample(x, 0.0);
            assert!(
                (l - x).abs() < 1e-6,
                "L not transparent at i={i}: {l} vs {x}"
            );
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut e = HaasEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 20.0);
        for _ in 0..2048 {
            e.process_sample(1.0, 1.0);
        }
        e.reset();
        assert_eq!(e.write_idx, 0);
        assert!(e.delay_l.iter().all(|&s| s == 0.0));
        assert!(e.delay_r.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = HaasEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
