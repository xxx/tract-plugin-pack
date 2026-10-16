use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Stereo chorus: each channel reads a modulated tap from its own
/// delay line; the right channel's LFO phase is offset from the
/// left's by 0..180° per Width. Single-voice CE-1-style — the
/// signature "warble" comes from one well-tuned modulated tap, not
/// from a wall of voices.
///
/// **Center** is the unmodulated delay time and effectively chooses
/// the *category* of effect:
/// - 1–5 ms → flanger character (esp. with Feedback ≠ 0)
/// - 10–25 ms → classic chorus
/// - 30–50 ms → slow doubler / micro-delay
///
/// **Depth** scales the LFO swing up to ±5 ms one-sided.
///
/// **Feedback** routes the wet output back into the delay-line input,
/// capped at ±95 % for loop stability. Negative values invert the
/// sign and produce phase-inverted comb notches (sharper "flange"
/// character); positive values build resonant comb peaks.
///
/// **Width** = 0 % puts both LFOs in phase (mono chorus); = 100 %
/// puts them antiphase (max stereo swirl, but L and R briefly land
/// on the same value as they cross). Mid values (50–70 %) feel
/// most natural on music.
///
/// **No PDC.** The wet path is delayed by `Center` but the dry path
/// is immediate — the engine's per-track Mix combines them and *that
/// difference* is the chorus effect. Reporting latency would
/// time-align the wet against the dry and kill the modulation we
/// just bought. **Per-sample work:** 2 sin + 2 fractional taps +
/// 2 ring writes ≈ 8 MAC + 2 transcendentals.
pub struct ChorusEffect {
    rate_hz: f32,
    depth_pct: f32,
    center_ms: f32,
    feedback_pct: f32,
    width_pct: f32,
    sample_rate: f32,

    /// Per-channel delay lines. Sized for the worst case (50 ms
    /// center + 5 ms depth swing at 192 kHz ≈ 10 560 samples;
    /// rounded up).
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_idx: usize,

    /// Shared LFO phase accumulator (radians). The R-channel LFO
    /// derives from this plus a Width-controlled offset; sharing
    /// the accumulator means modulating Rate doesn't desync L/R.
    lfo_phase: f32,

    /// Per-channel feedback state — the previous sample's wet
    /// output, mixed back into this sample's delay-line input.
    fb_l: f32,
    fb_r: f32,
}

impl ChorusEffect {
    const RATE_MIN_HZ: f32 = 0.05;
    const RATE_MAX_HZ: f32 = 10.0;
    const CENTER_MIN_MS: f32 = 1.0;
    const CENTER_MAX_MS: f32 = 50.0;
    /// Maximum LFO swing at Depth = 100 % (one-sided, ms). Picked
    /// so that even at minimum Center (1 ms), the modulated tap
    /// never reaches the buffer's clamp floor in the middle of the
    /// audible range; with Depth = 100 % at Center = 1 ms it clips
    /// only at the very deepest LFO troughs.
    const DEPTH_MAX_MS: f32 = 5.0;
    /// Hard cap on feedback gain (positive or negative). The +95 %
    /// cap is below the unit circle by a safe margin; combined with
    /// the small bit of loss from fractional-read interpolation the
    /// loop stays stable indefinitely.
    const FB_CAP: f32 = 0.95;
    /// Minimum modulated tap delay (samples). Two samples back from
    /// the write head leaves room for the linear-interp pair and
    /// guarantees no read-during-write race in `read_frac`.
    const MIN_DELAY_SAMPLES: f32 = 2.0;

    /// Delay-buffer length. 55 ms × 192 kHz = 10 560 samples; round
    /// up to a clean power-of-2-ish number for cache friendliness.
    const DELAY_BUF_LEN: usize = 11_264;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Rate",
            min: Self::RATE_MIN_HZ,
            max: Self::RATE_MAX_HZ,
            default: 0.5,
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
            name: "Center",
            min: Self::CENTER_MIN_MS,
            max: Self::CENTER_MAX_MS,
            default: 15.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: -100.0,
            max: 100.0,
            default: 0.0,
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
            default: 60.0,
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
            center_ms: Self::PARAMS[2].default,
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

    /// Fractional ring-buffer read with linear interpolation.
    /// `delay_samples` is the distance back from `write_idx`. The
    /// caller guarantees `delay_samples ∈ [MIN_DELAY_SAMPLES,
    /// buf.len() − 2]`.
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

impl Default for ChorusEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for ChorusEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let two_pi = 2.0 * std::f32::consts::PI;
        let center_samples = self.center_ms * 0.001 * self.sample_rate;
        let depth_samples = (self.depth_pct * 0.01) * Self::DEPTH_MAX_MS * 0.001 * self.sample_rate;
        let feedback = (self.feedback_pct * 0.01).clamp(-Self::FB_CAP, Self::FB_CAP);
        // Width 0..100 % → 0..π radians (= 0..180°) of L/R LFO phase
        // offset. 50 % = 90° (orthogonal); 100 % = antiphase.
        let width_offset = self.width_pct * 0.01 * std::f32::consts::PI;

        let lfo_l = self.lfo_phase.sin();
        let lfo_r = (self.lfo_phase + width_offset).sin();

        // Modulated tap delays. Clamped so a deep LFO swing on a
        // short Center can't read past the write head or out the
        // far end of the buffer.
        let max_delay = (Self::DELAY_BUF_LEN - 2) as f32;
        let tap_l =
            (center_samples + depth_samples * lfo_l).clamp(Self::MIN_DELAY_SAMPLES, max_delay);
        let tap_r =
            (center_samples + depth_samples * lfo_r).clamp(Self::MIN_DELAY_SAMPLES, max_delay);

        let wet_l = Self::read_frac(&self.delay_l, self.write_idx, tap_l);
        let wet_r = Self::read_frac(&self.delay_r, self.write_idx, tap_r);

        // Write input + feedback into the delay line. Feedback comes
        // from the PREVIOUS sample's wet output (stored in fb_l/fb_r)
        // so the feedback loop has a 1-sample minimum delay — keeps
        // the loop strictly causal regardless of `Center`.
        self.delay_l[self.write_idx] = left + feedback * self.fb_l;
        self.delay_r[self.write_idx] = right + feedback * self.fb_r;
        self.write_idx = (self.write_idx + 1) % Self::DELAY_BUF_LEN;

        self.fb_l = wet_l;
        self.fb_r = wet_r;

        // Advance LFO phase. set_param clamps Rate to [0.05, 10] Hz
        // so phase_inc is small even at 11 kHz SR.
        let phase_inc = two_pi * self.rate_hz / self.sample_rate;
        self.lfo_phase += phase_inc;
        if self.lfo_phase > two_pi {
            self.lfo_phase -= two_pi;
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
            2 => self.center_ms = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
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
    fn chorus_lists_five_parameters_with_the_expected_specs() {
        let c = ChorusEffect::new();
        let specs = c.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Rate");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 0.05);
        assert_eq!(specs[0].max, 10.0);
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Center");
        assert!(matches!(specs[2].scaling, ParamScaling::Log));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number { unit: "ms", .. }
        ));
        assert_eq!(specs[3].name, "Feedback");
        assert_eq!(specs[3].min, -100.0);
        assert_eq!(specs[3].max, 100.0);
        assert_eq!(specs[4].name, "Width");
    }

    #[test]
    fn chorus_set_param_clamps_each_slot() {
        let mut c = ChorusEffect::new();
        c.set_param(0, 999.0);
        assert_eq!(c.rate_hz, 10.0);
        c.set_param(0, 0.0);
        assert_eq!(c.rate_hz, 0.05);
        c.set_param(1, 999.0);
        assert_eq!(c.depth_pct, 100.0);
        c.set_param(2, 0.0);
        assert_eq!(c.center_ms, 1.0);
        c.set_param(2, 999.0);
        assert_eq!(c.center_ms, 50.0);
        c.set_param(3, -999.0);
        assert_eq!(c.feedback_pct, -100.0);
        c.set_param(4, 999.0);
        assert_eq!(c.width_pct, 100.0);
    }

    #[test]
    fn chorus_depth_zero_is_a_pure_delay() {
        // With Depth=0 the LFO swing collapses to zero — the chorus
        // becomes a plain stereo delay of `Center` ms. Feed an
        // impulse and verify the wet output is exactly delayed.
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(1, 0.0); // Depth = 0
        c.set_param(2, 10.0); // Center = 10 ms = 480 samples @ 48 kHz
        c.set_param(3, 0.0); // No feedback
        c.set_param(4, 0.0); // No stereo offset → L = R for symmetric input
        let _ = c.process_sample(1.0, 1.0);
        let mut peak_idx = 0usize;
        let mut peak_val = 0.0_f32;
        for i in 1..1_024 {
            let (l, _) = c.process_sample(0.0, 0.0);
            if l.abs() > peak_val {
                peak_val = l.abs();
                peak_idx = i;
            }
        }
        // 480 samples ± 1 for the linear-interp pair boundary.
        assert!(
            peak_val > 0.95,
            "impulse should pass through to the wet tap; peak={peak_val}"
        );
        assert!(
            (479..=481).contains(&peak_idx),
            "tap should land at 480 samples (10 ms × 48 kHz), got {peak_idx}"
        );
    }

    #[test]
    fn chorus_modulation_changes_the_wet_envelope_over_time() {
        // With Depth > 0 the tap walks around Center → for a steady
        // sine input the wet output's amplitude envelope or phase
        // shifts over time. Test: feed a 1 kHz sine for several LFO
        // cycles and verify the wet has a sample-to-sample variance
        // larger than the input's (chorus picks up the pitch shift
        // → cross-correlation against dry drops).
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 5.0); // Fast rate so multiple cycles per test
        c.set_param(1, 80.0); // Heavy depth
        c.set_param(2, 15.0); // Mid Center
        c.set_param(3, 0.0);
        c.set_param(4, 0.0);
        let mut dry_energy = 0.0_f32;
        let mut diff_energy = 0.0_f32;
        // Skip the first 24 ms (>= Center) so the wet path is primed.
        for i in 0..1_500 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let _ = c.process_sample(dry, dry);
        }
        for i in 1_500..48_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let (l, _) = c.process_sample(dry, dry);
            dry_energy += dry * dry;
            diff_energy += (l - dry) * (l - dry);
        }
        // Wet should be substantially different from dry (the chorus
        // pitch shift breaks the phase relationship).
        assert!(
            diff_energy > dry_energy * 0.1,
            "modulation should de-correlate wet from dry; dry_e={dry_energy}, diff_e={diff_energy}"
        );
    }

    #[test]
    fn chorus_width_zero_collapses_to_mono() {
        // Width = 0 → L and R LFOs in phase. For an L = R input the
        // wet output must satisfy wet_l == wet_r at every sample.
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 2.0);
        c.set_param(1, 70.0);
        c.set_param(4, 0.0); // Width = 0
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = c.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "Width=0 must give L==R; sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn chorus_width_full_separates_l_and_r() {
        // At Width=100 % the LFOs are antiphase → L and R taps land
        // at different positions → measurable L/R difference for an
        // L=R input.
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(0, 1.5);
        c.set_param(1, 70.0);
        c.set_param(4, 100.0);
        for _ in 0..1_500 {
            let _ = c.process_sample(0.5, 0.5); // Prime
        }
        let mut diff = 0.0_f32;
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let (l, r) = c.process_sample(dry, dry);
            diff += (l - r).abs();
        }
        assert!(
            diff > 1.0,
            "Width=100% should split L/R for a mono input; total |L-R| was {diff}"
        );
    }

    #[test]
    fn chorus_feedback_amplifies_the_wet_signal() {
        // Compare wet output energy with feedback=0 vs feedback=80 %.
        // The positive-feedback loop should sustain more energy.
        let measure = |fb: f32| {
            let mut c = ChorusEffect::new();
            c.set_sample_rate(48_000.0);
            c.set_param(0, 0.3);
            c.set_param(1, 30.0);
            c.set_param(2, 5.0); // Short delay → comb-style feedback
            c.set_param(3, fb);
            c.set_param(4, 0.0);
            let _ = c.process_sample(1.0, 1.0);
            let mut e = 0.0_f32;
            for _ in 0..2_400 {
                let (l, _) = c.process_sample(0.0, 0.0);
                e += l * l;
            }
            e
        };
        let e0 = measure(0.0);
        let e80 = measure(80.0);
        assert!(
            e80 > e0 * 2.0,
            "high feedback should sustain wet; fb=0:{e0}, fb=80:{e80}"
        );
    }

    #[test]
    fn chorus_silent_input_with_feedback_settles_to_silence() {
        // Even at the FB_CAP feedback cap, silent input must decay
        // to silence eventually — the loop must be strictly stable.
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(1, 0.0);
        c.set_param(2, 5.0);
        c.set_param(3, 95.0); // Right at the cap (clamps to 0.95)
        c.set_param(4, 0.0);
        let _ = c.process_sample(1.0, 1.0);
        // 5 s should be plenty given a 0.95 loop gain at 5 ms (RT60 ≈
        // -3 × 0.005 / ln(0.95) ≈ 0.29 s — well under our window).
        for _ in 0..240_000 {
            let _ = c.process_sample(0.0, 0.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..1_024 {
            let (l, r) = c.process_sample(0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak < 1e-3,
            "feedback loop must decay; residual peak={peak}"
        );
    }

    #[test]
    fn chorus_stays_bounded_under_aggressive_sweep() {
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            let p = (i as f32 / 4_096.0).fract();
            c.set_param(0, 0.05 * 200.0_f32.powf(p)); // 0.05..10 Hz log
            c.set_param(1, (i as f32 / 5_000.0).fract() * 100.0);
            c.set_param(2, 1.0 * 50.0_f32.powf((i as f32 / 6_000.0).fract())); // 1..50 ms log
            c.set_param(3, (i as f32 / 3_000.0).fract() * 200.0 - 100.0);
            c.set_param(4, (i as f32 / 7_000.0).fract() * 100.0);
            let x = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = c.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // Wet alone is bounded near unity even at high feedback —
            // 4× headroom catches genuine blow-ups.
            assert!(
                l.abs() < 4.0 && r.abs() < 4.0,
                "sample {i} blew up: ({l},{r})"
            );
        }
    }

    #[test]
    fn chorus_reset_clears_state() {
        let mut c = ChorusEffect::new();
        c.set_sample_rate(48_000.0);
        c.set_param(3, 70.0);
        for _ in 0..4_800 {
            let _ = c.process_sample(0.5, 0.5);
        }
        c.reset();
        assert!(c.delay_l.iter().all(|&v| v == 0.0));
        assert!(c.delay_r.iter().all(|&v| v == 0.0));
        assert_eq!(c.write_idx, 0);
        assert_eq!(c.lfo_phase, 0.0);
        assert_eq!(c.fb_l, 0.0);
        assert_eq!(c.fb_r, 0.0);
    }
}
