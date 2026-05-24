use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Classic stereo flanger: each channel reads a modulated tap from
/// its own short delay line, summed with the dry signal via the
/// engine's per-track Mix. Triangle LFO so the perceived pitch
/// sweep is linear between peaks -- that's the iconic flanger
/// "whoosh" / "jet" character, distinct from the chorus's sine
/// sweep which feels more sinusoidal-warbly.
///
/// Topologically similar to the Chorus effect, but pre-tuned for
/// flanging: shorter delay range (0.5..15 ms vs 1..50 ms), higher
/// default Feedback (50 % vs 0 %), and a higher safety cap on
/// Feedback (+/-97 % vs +/-95 %) so the comb resonance can scream
/// without going unstable.
///
/// **Rate** is the LFO frequency (0.05..10 Hz, log -- subsonic
/// for slow sweeps, fast for buzzing fluttery effects).
///
/// **Depth** scales the LFO swing up to +/-5 ms one-sided. At
/// Manual = 0.5 ms with Depth = 100 % the sweep tries to read
/// before t = 0; the floor clamp at 2 samples means the tap
/// "sticks" at minimum delay through the bottom of the sweep,
/// producing a one-sided flange with characteristic notch
/// movement near zero.
///
/// **Manual** is the unmodulated delay-line read position
/// (0.5..15 ms log). At small Manual the comb spacing is wide
/// (audible peaks/notches across the spectrum -- "metallic"
/// resonance); at larger Manual the comb is dense (smoother
/// chorus-ish texture).
///
/// **Feedback** routes the wet output back into the delay-line
/// input. Capped internally at +/-97 % for loop stability;
/// negative values invert the recirculated phase for sharper
/// flange notches, positive values build resonant peaks.
///
/// **Width** = 0 % puts both LFOs in phase (mono flange);
/// = 100 % puts them 180 deg out of phase (max stereo swirl).
/// Mid values (40..70 %) read most natural on stereo program.
///
/// **No PDC.** The wet path is delayed by `Manual` but the dry
/// passes through immediately; the engine's per-track Mix sums
/// them and *that* difference is the flange. Reporting latency
/// would time-align dry to wet and kill the effect.
/// **Per-sample work:** 1 triangle LFO + 2 fractional taps +
/// 2 ring writes -- a handful of MACs, no transcendentals.
pub struct FlangerEffect {
    rate_hz: f32,
    depth_pct: f32,
    manual_ms: f32,
    feedback_pct: f32,
    width_pct: f32,
    sample_rate: f32,

    /// Per-channel delay lines. Sized for the worst case (15 ms
    /// manual + 5 ms depth swing at 192 kHz = ~3840 samples;
    /// rounded up).
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,

    /// Shared LFO phase in cycles, [0, 1). The right channel's
    /// LFO phase derives from this plus a Width-controlled
    /// offset, so modulating Rate doesn't desync L/R.
    lfo_phase: f32,

    /// Previous sample's wet output, recirculated as feedback.
    /// The 1-sample delay keeps the feedback loop strictly causal.
    fb_l: f32,
    fb_r: f32,
}

impl FlangerEffect {
    const RATE_MIN_HZ: f32 = 0.05;
    const RATE_MAX_HZ: f32 = 10.0;
    const MANUAL_MIN_MS: f32 = 0.5;
    const MANUAL_MAX_MS: f32 = 15.0;
    /// Maximum LFO swing at Depth = 100 % (one-sided, ms). 5 ms
    /// is enough to sweep across the full Manual range from any
    /// starting point.
    const DEPTH_MAX_MS: f32 = 5.0;
    /// Hard cap on feedback gain. +/-97 % is hotter than Chorus's
    /// +/-95 % because flangers are defined by their resonance;
    /// dialing toward the cap produces the iconic "screaming"
    /// peak character without losing stability.
    const FB_CAP: f32 = 0.97;
    /// Minimum delay-line read offset (samples). Two samples
    /// below the write head leaves room for the linear-interp
    /// pair and prevents a read-during-write race in `read_frac`.
    const MIN_DELAY_SAMPLES: f32 = 2.0;

    /// Delay-buffer length. 20 ms x 192 kHz = 3840 samples (15 ms
    /// max Manual + 5 ms max Depth swing). Round up.
    const DELAY_BUF_LEN: usize = 4_096;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 0.3,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Depth",
            min: 0.0,
            max: 100.0,
            default: 70.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Manual",
            min: Self::MANUAL_MIN_MS,
            max: Self::MANUAL_MAX_MS,
            default: 3.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: -100.0,
            max: 100.0,
            default: 50.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Width",
            min: 0.0,
            max: 100.0,
            default: 70.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        Self {
            rate_hz: Self::PARAMS[0].default,
            depth_pct: Self::PARAMS[1].default,
            manual_ms: Self::PARAMS[2].default,
            feedback_pct: Self::PARAMS[3].default,
            width_pct: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            delay_l: vec![0.0; Self::DELAY_BUF_LEN],
            delay_r: vec![0.0; Self::DELAY_BUF_LEN],
            write_idx: 0,
            lfo_phase: 0.0,
            fb_l: 0.0,
            fb_r: 0.0,
        }
    }

    /// Triangle wave with peaks at +/-1 and zero crossings at
    /// phase = 0, 0.5. Linear sweep gives the classic flanger
    /// "linear pitch slide" character between peaks -- distinct
    /// from a sine LFO which would slow down at the extremes.
    ///
    /// Mapping: phase in [0, 1) -> output in [-1, +1].
    ///   phase 0.00 -> 0
    ///   phase 0.25 -> +1
    ///   phase 0.50 -> 0
    ///   phase 0.75 -> -1
    ///   phase 1.00 -> 0 (wraps)
    #[inline]
    fn triangle_lfo(phase: f32) -> f32 {
        // Wrap into [0, 1) defensively -- callers maintain this,
        // but `phase + width_offset` can land slightly outside if
        // width_offset is itself near 1.
        let p = phase - phase.floor();
        if p < 0.25 {
            p * 4.0
        } else if p < 0.75 {
            2.0 - p * 4.0
        } else {
            p * 4.0 - 4.0
        }
    }

    /// Fractional ring-buffer read with linear interpolation.
    /// `delay_samples` is the distance back from `write_idx`.
    /// Caller guarantees `delay_samples >= MIN_DELAY_SAMPLES`.
    #[inline]
    fn read_frac(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let pos = write_idx as f32 + n as f32 - delay_samples;
        let i_floor = pos.floor();
        let frac = pos - i_floor;
        let i0 = (i_floor as usize) % n;
        let i1 = (i0 + 1) % n;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }
}

impl Default for FlangerEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for FlangerEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let manual_samples = self.manual_ms * 0.001 * self.sample_rate;
        let depth_samples = (self.depth_pct * 0.01) * Self::DEPTH_MAX_MS * 0.001 * self.sample_rate;
        let feedback = (self.feedback_pct * 0.01).clamp(-Self::FB_CAP, Self::FB_CAP);
        // Width 0..100 % -> 0..0.5 cycles of LFO phase offset
        // between L and R (0.5 cycles = 180 deg, antiphase).
        let width_offset = self.width_pct * 0.005;

        let lfo_l = Self::triangle_lfo(self.lfo_phase);
        let lfo_r = Self::triangle_lfo(self.lfo_phase + width_offset);

        // Modulated tap delays. Clamped so a deep LFO swing on a
        // short Manual can't read past the write head; the user
        // hears that as a "stuck at minimum" flange floor near
        // the trough of the sweep, which is musically interesting
        // and inherent to fixed-delay-line flangers.
        let max_delay = (Self::DELAY_BUF_LEN - 2) as f32;
        let tap_l =
            (manual_samples + depth_samples * lfo_l).clamp(Self::MIN_DELAY_SAMPLES, max_delay);
        let tap_r =
            (manual_samples + depth_samples * lfo_r).clamp(Self::MIN_DELAY_SAMPLES, max_delay);

        let wet_l = Self::read_frac(&self.delay_l, self.write_idx, tap_l);
        let wet_r = Self::read_frac(&self.delay_r, self.write_idx, tap_r);

        // Write input + feedback from the PREVIOUS sample's wet
        // output -- the 1-sample lag makes the feedback loop
        // strictly causal regardless of Manual.
        self.delay_l[self.write_idx] = left + feedback * self.fb_l;
        self.delay_r[self.write_idx] = right + feedback * self.fb_r;
        self.write_idx = (self.write_idx + 1) % Self::DELAY_BUF_LEN;

        self.fb_l = wet_l;
        self.fb_r = wet_r;

        // Advance LFO phase in cycles. set_param clamps Rate to
        // [0.05, 10] Hz so phase_inc stays small even at 11 kHz SR.
        let phase_inc = self.rate_hz / self.sample_rate;
        self.lfo_phase += phase_inc;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= self.lfo_phase.floor();
        }

        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        for s in self.delay_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.delay_r.iter_mut() {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.lfo_phase = 0.0;
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rate_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.depth_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.manual_ms = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.feedback_pct = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.width_pct = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn flanger_lists_five_parameters_with_the_expected_specs() {
        let f = FlangerEffect::new();
        let specs = f.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Rate");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 0.05);
        assert_eq!(specs[0].max, 10.0);
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Manual");
        assert!(matches!(specs[2].scaling, ParamScaling::Log));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number { unit: "ms", .. }
        ));
        // Manual range is *shorter* than Chorus's Center (1..50 ms);
        // this is the defining param-range difference between the
        // two effects.
        assert_eq!(specs[2].min, 0.5);
        assert_eq!(specs[2].max, 15.0);
        assert_eq!(specs[3].name, "Feedback");
        // Flanger defaults to hot feedback (50 % vs Chorus's 0 %).
        assert_eq!(specs[3].default, 50.0);
        assert_eq!(specs[4].name, "Width");
    }

    #[test]
    fn flanger_set_param_clamps_each_slot() {
        let mut f = FlangerEffect::new();
        f.set_param(0, 999.0);
        assert_eq!(f.rate_hz, 10.0);
        f.set_param(0, 0.0);
        assert_eq!(f.rate_hz, 0.05);
        f.set_param(1, 999.0);
        assert_eq!(f.depth_pct, 100.0);
        f.set_param(2, 0.0);
        assert_eq!(f.manual_ms, 0.5);
        f.set_param(2, 999.0);
        assert_eq!(f.manual_ms, 15.0);
        f.set_param(3, -999.0);
        assert_eq!(f.feedback_pct, -100.0);
        f.set_param(4, 999.0);
        assert_eq!(f.width_pct, 100.0);
    }

    #[test]
    fn flanger_triangle_lfo_hits_known_points() {
        // Spot-check the LFO shape at the four cardinal phases.
        let cases = [
            (0.0_f32, 0.0_f32),
            (0.125, 0.5),
            (0.25, 1.0),
            (0.375, 0.5),
            (0.5, 0.0),
            (0.625, -0.5),
            (0.75, -1.0),
            (0.875, -0.5),
        ];
        for (phase, expected) in cases {
            let got = FlangerEffect::triangle_lfo(phase);
            assert!(
                (got - expected).abs() < 1e-5,
                "triangle_lfo({phase}) = {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn flanger_depth_zero_is_pure_delay() {
        // With Depth=0, the LFO swing is zero -> the flanger
        // collapses to a stereo delay of `Manual` ms. Feed an
        // impulse and verify the wet output peaks at the right
        // delay.
        let mut f = FlangerEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(1, 0.0); // Depth = 0
        f.set_param(2, 5.0); // Manual = 5 ms = 240 samples @ 48 kHz
        f.set_param(3, 0.0); // No feedback
        f.set_param(4, 0.0);
        let _ = f.process_sample(1.0, 1.0);
        let mut peak_idx = 0usize;
        let mut peak_val = 0.0_f32;
        for i in 1..1_024 {
            let (l, _) = f.process_sample(0.0, 0.0);
            if l.abs() > peak_val {
                peak_val = l.abs();
                peak_idx = i;
            }
        }
        // 240 samples for 5 ms at 48 kHz, +/- 1 for interp boundary.
        assert!(
            peak_val > 0.95,
            "impulse should pass to the tap; peak={peak_val}"
        );
        assert!(
            (239..=241).contains(&peak_idx),
            "tap should land at 240 samples (5 ms x 48 kHz), got {peak_idx}"
        );
    }

    #[test]
    fn flanger_feedback_extends_the_echo_train() {
        // High feedback -> multiple recirculations -> sustained
        // ringing. Compare wet-energy tail for fb=0 vs fb=80.
        let measure = |fb: f32| {
            let mut f = FlangerEffect::new();
            f.set_sample_rate(48_000.0);
            f.set_param(0, 0.1); // Slow rate so LFO barely moves over the test
            f.set_param(1, 0.0); // Depth=0 so the effect is a fixed comb
            f.set_param(2, 3.0); // Manual = 3 ms
            f.set_param(3, fb);
            f.set_param(4, 0.0);
            let _ = f.process_sample(1.0, 1.0);
            let mut e = 0.0_f32;
            for _ in 0..4_800 {
                let (l, _) = f.process_sample(0.0, 0.0);
                e += l * l;
            }
            e
        };
        let e0 = measure(0.0);
        let e80 = measure(80.0);
        // 2x rather than 3x: the test window is only ~33 grain
        // cycles long; with the 0.95-effective-per-cycle decay
        // (0.8 fb plus interp loss) the energy is bounded by the
        // sum of a geometric series with ratio < 1.
        assert!(
            e80 > e0 * 2.0,
            "high feedback should sustain the comb; fb=0:{e0}, fb=80:{e80}"
        );
    }

    #[test]
    fn flanger_width_zero_collapses_to_mono() {
        let mut f = FlangerEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 2.0);
        f.set_param(1, 70.0);
        f.set_param(4, 0.0); // Width = 0
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = f.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "Width=0 must give L==R; sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn flanger_width_full_separates_l_and_r() {
        // Width=100 -> L and R LFOs antiphase -> stereo separation
        // even for a mono input.
        let mut f = FlangerEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 1.0);
        f.set_param(1, 80.0);
        f.set_param(4, 100.0);
        // Prime the delay lines
        for _ in 0..1_500 {
            let _ = f.process_sample(0.5, 0.5);
        }
        let mut diff = 0.0_f32;
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = f.process_sample(dry, dry);
            diff += (l - r).abs();
        }
        assert!(
            diff > 1.0,
            "Width=100 should split L/R for a mono input; total |L-R|={diff}"
        );
    }

    #[test]
    fn flanger_silent_input_at_feedback_cap_decays_to_silence() {
        // The +/-97 % cap is hotter than Chorus's +/-95 %, but the
        // loop must still be strictly stable -- silent input has
        // to decay all the way to silence.
        let mut f = FlangerEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(1, 0.0);
        f.set_param(2, 3.0);
        f.set_param(3, 97.0); // At the cap (clamps to 0.97)
        f.set_param(4, 0.0);
        let _ = f.process_sample(1.0, 1.0);
        // RT60 at 0.97 / 3 ms grain ~ -3 / log(0.97) ~ 100 cycles
        // ~ 0.3 s. 5 s of silence is plenty of decay time.
        for _ in 0..240_000 {
            let _ = f.process_sample(0.0, 0.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..1_024 {
            let (l, r) = f.process_sample(0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak < 1e-3, "feedback loop must decay; peak={peak}");
    }

    #[test]
    fn flanger_stays_bounded_under_aggressive_sweep() {
        let mut f = FlangerEffect::new();
        f.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            let p = (i as f32 / 4_096.0).fract();
            f.set_param(0, 0.05 * 200.0_f32.powf(p)); // 0.05..10 Hz log
            f.set_param(1, (i as f32 / 5_000.0).fract() * 100.0);
            let pm = (i as f32 / 6_000.0).fract();
            f.set_param(2, 0.5 * 30.0_f32.powf(pm)); // 0.5..15 ms log
            f.set_param(3, (i as f32 / 3_000.0).fract() * 200.0 - 100.0);
            f.set_param(4, (i as f32 / 7_000.0).fract() * 100.0);
            let x = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = f.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Wet at the cap can ring -- bound at 8x dry magnitude.
            assert!(
                l.abs() < 8.0 && r.abs() < 8.0,
                "sample {i} blew up: ({l},{r})"
            );
        }
    }

    #[test]
    fn flanger_reset_clears_state() {
        let mut f = FlangerEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(3, 80.0);
        for _ in 0..4_800 {
            let _ = f.process_sample(0.5, 0.5);
        }
        f.reset();
        assert!(f.delay_l.iter().all(|&v| v == 0.0));
        assert!(f.delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(f.write_idx, 0);
        assert_eq!(f.lfo_phase, 0.0);
        assert_eq!(f.fb_l, 0.0);
        assert_eq!(f.fb_r, 0.0);
    }
}
