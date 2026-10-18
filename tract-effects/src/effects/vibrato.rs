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
    fn fractional_reads_interpolate_wrap_and_clamp_to_available_history() {
        let history = [2.0, -4.0, 6.0, 10.0, -8.0, 14.0, 3.0, -5.0];
        for (write, delay, expected) in [
            (0, 2.25, 5.75),
            (1, 2.25, -3.0),
            (7, 2.25, 8.5),
            (2, 2.25, 0.25),
            (0, 5.5, 8.0),
            (7, 6.0, -4.0),
            (0, 2.0, 3.0),
            (2, -10.0, -4.0),
            (2, 100.0, -8.0),
        ] {
            assert_eq!(VibratoEffect::read_frac(&history, write, delay), expected);
        }
    }

    #[test]
    fn zero_depth_impulses_have_the_exact_integer_or_fractional_delay() {
        for (sample_rate, first, second) in [(8_000.0, 24, 24), (12_500.0, 37, 38)] {
            let mut vibrato = VibratoEffect::new();
            vibrato.set_sample_rate(sample_rate);
            vibrato.set_param(1, 0.0);
            for sample in 0..4_200 {
                let expected = if first == second && sample == first {
                    (1.0, -0.25)
                } else if first != second && (sample == first || sample == second) {
                    (0.5, -0.125)
                } else {
                    (0.0, 0.0)
                };
                let input = if sample == 0 { 1.0 } else { 0.0 };
                assert_eq!(
                    vibrato.process_sample(input, -0.25 * input),
                    expected,
                    "rate {sample_rate}, sample {sample}"
                );
            }
        }
    }

    fn filled_ramp() -> VibratoEffect {
        let mut vibrato = VibratoEffect::new();
        vibrato.set_sample_rate(48_000.0);
        vibrato.set_param(1, 50.0);
        for (index, (left, right)) in vibrato
            .delay_l
            .iter_mut()
            .zip(&mut vibrato.delay_r)
            .enumerate()
        {
            *left = index as f32 / 1_024.0;
            *right = -(index as f32) / 2_048.0;
        }
        vibrato.write_idx = 1_024;
        vibrato
    }

    #[test]
    fn every_lfo_shape_places_the_delay_tap_at_its_waveform_landmarks() {
        let q = std::f64::consts::FRAC_1_SQRT_2;
        for (shape, landmarks) in [
            [0.0, q, 1.0, q, 0.0, -q, -1.0, -q],
            [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5],
            [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0],
            [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75],
        ]
        .into_iter()
        .enumerate()
        {
            for (index, wave) in landmarks.into_iter().enumerate() {
                let mut vibrato = filled_ramp();
                vibrato.set_param(2, shape as f32);
                vibrato.phase = index as f32 / 8.0;
                let delay = 144.0 + 72.0 * wave;
                let (left, right) = vibrato.process_sample(0.0, 0.0);
                assert!(
                    (left as f64 - (1_024.0 - delay) / 1_024.0).abs() < 2e-7,
                    "shape {shape}, phase {index}/8: {left}"
                );
                assert!((right as f64 + (1_024.0 - delay) / 2_048.0).abs() < 2e-7);
            }
        }
    }

    #[test]
    fn delay_limits_keep_the_output_causal_and_inside_the_buffer() {
        for (sample_rate, phase, expected_delay) in [(48_000.0, 0.5, 1), (384_000.0, 0.0, 2_046)] {
            let mut vibrato = VibratoEffect::new();
            vibrato.set_sample_rate(sample_rate);
            vibrato.set_param(0, 0.05);
            vibrato.set_param(1, 100.0);
            vibrato.set_param(2, 2.0);
            vibrato.phase = phase;
            for sample in 0..2_052 {
                let input = if sample == 0 { 1.0 } else { 0.0 };
                let expected = if sample == expected_delay {
                    (1.0, -0.25)
                } else {
                    (0.0, 0.0)
                };
                assert_eq!(
                    vibrato.process_sample(input, -0.25 * input),
                    expected,
                    "rate {sample_rate}, sample {sample}"
                );
            }
        }
    }

    #[test]
    fn rate_and_sample_rate_changes_preserve_the_clock_and_input_history() {
        let mut vibrato = filled_ramp();
        vibrato.set_param(2, 3.0);
        let mut phase = 0.9995_f64;
        vibrato.phase = phase as f32;
        let mut write = 1_024.0;
        for (sample_rate, rate) in [(32_768.0, 10.0), (48_000.0, 5.0), (96_000.0, 7.0)] {
            vibrato.set_sample_rate(sample_rate);
            vibrato.set_param(0, rate);
            for _ in 0..16 {
                let delay = (3.0 + 1.5 * (2.0 * phase - 1.0)) * sample_rate as f64 / 1_000.0;
                let (left, right) = vibrato.process_sample(0.0, 0.0);
                assert!((left as f64 - (write - delay) / 1_024.0).abs() < 5e-7);
                assert!((right as f64 + (write - delay) / 2_048.0).abs() < 5e-7);
                assert!((0.0..1.0).contains(&vibrato.phase));
                phase = (phase + rate as f64 / sample_rate as f64).fract();
                write += 1.0;
            }
        }
    }

    #[test]
    fn zero_depth_keeps_the_clock_running_for_later_modulation() {
        let mut vibrato = VibratoEffect::new();
        vibrato.set_sample_rate(32_768.0);
        vibrato.set_param(0, 16.0);
        vibrato.set_param(1, 0.0);
        vibrato.set_param(2, 3.0);
        for sample in 0..1_536 {
            vibrato.process_sample(sample as f32 / 1_024.0, -(sample as f32) / 2_048.0);
        }
        vibrato.set_param(1, 50.0);
        // Phase 3/4 gives a 122.88-sample offset into the written ramp.
        let (left, right) = vibrato.process_sample(1.5, -0.75);
        assert!((left - 1.38).abs() < 2e-7);
        assert!((right + 0.69).abs() < 2e-7);
    }

    fn configured_vibrato() -> VibratoEffect {
        let mut vibrato = VibratoEffect::new();
        vibrato.set_sample_rate(32_000.0);
        for (index, value) in [7.0, 63.0, 1.0].into_iter().enumerate() {
            vibrato.set_param(index, value);
        }
        vibrato
    }

    #[test]
    fn reset_replays_a_fresh_vibrato_with_the_same_parameters() {
        let mut used = configured_vibrato();
        let mut fresh = configured_vibrato();
        for sample in 0..2_100 {
            used.process_sample((sample as f32 * 0.13).sin(), (sample as f32 * 0.31).cos());
        }
        used.reset();
        used.set_param(99, 123.0);
        for sample in 0..4_200 {
            let input = ((sample as f32 * 0.17).cos(), (sample as f32 * 0.29).sin());
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn parameters_clamp_and_round_shapes_without_clearing_history() {
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
            let mut clamped = configured_vibrato();
            let mut reference = configured_vibrato();
            for sample in 0..257 {
                let input = ((sample as f32 * 0.19).sin(), (sample as f32 * 0.37).cos());
                clamped.process_sample(input.0, input.1);
                reference.process_sample(input.0, input.1);
            }
            clamped.set_param(index, value);
            reference.set_param(index, endpoint);
            clamped.set_param(99, -123.0);
            for sample in 0..2_100 {
                let input = ((sample as f32 * 0.23).cos(), (sample as f32 * 0.41).sin());
                assert_eq!(
                    clamped.process_sample(input.0, input.1),
                    reference.process_sample(input.0, input.1)
                );
            }
        }
    }

    #[test]
    fn parameters_are_declared() {
        let e = VibratoEffect::new();
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
            [(0.05, 20.0, 5.0), (0.0, 100.0, 30.0), (0.0, 3.0, 0.0)]
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
        assert!(
            peak > 0.9 && peak < 1.1,
            "fixed-delay peak should be ~1, got {peak}"
        );
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
            assert!(
                l.is_finite() && r.is_finite(),
                "non-finite output: {l}, {r}"
            );
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
