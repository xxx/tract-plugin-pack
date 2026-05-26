use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Pitch-modulating vibrato. Implemented as a short modulated delay
/// line: the read tap sweeps relative to the write head at the LFO
/// rate, and that varying delay manifests as pitch modulation. Unlike
/// `Chorus` and `Flanger` which output a dry+wet blend, `Vibrato`
/// outputs only the delayed (pitch-shifted) signal -- the dry path
/// is suppressed so you hear pure pitch wobble rather than a chorused
/// thickening.
///
/// `Depth = 0%` parks the read tap at the centre of its swing range,
/// producing a constant short delay (~3 ms). `Depth = 100%` sweeps
/// the tap +/-3 ms around centre. Shape selects sine / triangle /
/// square / saw waveform; square produces stepped pitch (closer to
/// a trill than a vibrato).
pub struct VibratoEffect {
    rate_hz: f32,
    depth_pct: f32,
    shape_idx: f32,
    sample_rate: f32,
    phase: f32,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,
}

const VIBRATO_SHAPE_LABELS: &[&str] = &["Sine", "Triangle", "Square", "Saw"];

const SHAPE_SINE: usize = 0;
const SHAPE_TRIANGLE: usize = 1;
const SHAPE_SQUARE: usize = 2;

impl VibratoEffect {
    const RATE_MIN_HZ: f32 = 0.05;
    const RATE_MAX_HZ: f32 = 20.0;
    /// Centre delay in ms -- the unmodulated read offset from the write
    /// head. Sized so even at Depth = 100% the swing won't pull the read
    /// tap behind the buffer's lower bound.
    const CENTRE_MS: f32 = 3.0;
    /// Max one-sided swing in ms at Depth = 100%.
    const SWING_MS: f32 = 3.0;
    /// Delay buffer length, sized for the worst case: 6 ms (centre +
    /// swing) at 192 kHz = 1152 samples, plus a couple of samples for
    /// the linear-interp pair. Round up to 2048 for cache friendliness.
    const DELAY_BUF_LEN: usize = 2048;

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
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Shape",
            min: 0.0,
            max: (VIBRATO_SHAPE_LABELS.len() - 1) as f32,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: VIBRATO_SHAPE_LABELS,
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
            delay_l: vec![0.0; Self::DELAY_BUF_LEN],
            delay_r: vec![0.0; Self::DELAY_BUF_LEN],
            write_idx: 0,
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

    /// Linear-interpolated read from a delay line. `offset_samples` is
    /// distance from the write head; result is the sample that was
    /// written `offset_samples` ago, with sub-sample precision.
    #[inline]
    fn read_frac(buf: &[f32], write_idx: usize, offset_samples: f32) -> f32 {
        let n = buf.len();
        let offset_clamped = offset_samples.max(1.0).min((n - 2) as f32);
        let int_off = offset_clamped.floor() as usize;
        let frac = offset_clamped - int_off as f32;
        // write_idx points at the NEXT write slot, so read_idx = write_idx - offset
        // wraps backwards through the ring.
        let idx_a = (write_idx + n - int_off) % n;
        let idx_b = (write_idx + n - int_off - 1) % n;
        buf[idx_a] * (1.0 - frac) + buf[idx_b] * frac
    }
}

impl Default for VibratoEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for VibratoEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let sr = self.sample_rate.max(1.0);
        let centre = Self::CENTRE_MS * 0.001 * sr;
        let swing = Self::SWING_MS * 0.001 * sr;
        let depth = (self.depth_pct * 0.01).clamp(0.0, 1.0);
        let lfo = self.eval_lfo();
        // Read offset oscillates around `centre`, swinging +/- `swing * depth`.
        let offset = centre + swing * depth * lfo;

        let wet_l = Self::read_frac(&self.delay_l, self.write_idx, offset);
        let wet_r = Self::read_frac(&self.delay_r, self.write_idx, offset);

        self.delay_l[self.write_idx] = left;
        self.delay_r[self.write_idx] = right;
        self.write_idx = (self.write_idx + 1) % self.delay_l.len();

        self.advance_phase();
        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.write_idx = 0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rate_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.depth_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => {
                let max_idx = (VIBRATO_SHAPE_LABELS.len() - 1) as f32;
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
        let e = VibratoEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Rate");
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Shape");
    }

    #[test]
    fn silent_input_stays_silent() {
        let mut e = VibratoEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn depth_zero_is_fixed_delay() {
        // At Depth = 0 the read tap stays parked at `CENTRE_MS`, so the
        // output is the input delayed by ~3 ms with no modulation.
        // Verify a sine input is reproduced with consistent amplitude
        // (no warble) after the initial delay-line warm-up.
        let mut e = VibratoEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.0);
        // Warm up past the delay so we read written data.
        for i in 0..512 {
            let x = (i as f32 * 0.01).sin();
            e.process_sample(x, x);
        }
        // Measure peak amplitude over a window and verify it stays
        // close to the input peak.
        let mut peak = 0.0_f32;
        for i in 512..(512 + 4096) {
            let x = (i as f32 * 0.01).sin();
            let (l, _) = e.process_sample(x, x);
            peak = peak.max(l.abs());
        }
        assert!(peak > 0.9 && peak < 1.1, "fixed-delay peak should be ~1, got {peak}");
    }

    #[test]
    fn full_depth_changes_amplitude_envelope() {
        // Vibrato modulating delay produces tiny amplitude changes too
        // (the LFO sweep stretches/compresses the read at the boundary).
        // Just confirm output stays bounded and finite under max depth.
        let mut e = VibratoEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 8.0);
        e.set_param(1, 100.0);
        for i in 0..48_000 {
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, r) = e.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "non-finite output: {l}, {r}");
            assert!(l.abs() < 2.0 && r.abs() < 2.0, "out of range: {l}, {r}");
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut e = VibratoEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            e.process_sample(1.0, -1.0);
        }
        e.reset();
        assert_eq!(e.phase, 0.0);
        assert_eq!(e.write_idx, 0);
        assert!(e.delay_l.iter().all(|&s| s == 0.0));
        assert!(e.delay_r.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = VibratoEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
