use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// 4-pole diode ladder lowpass with TB-303-style HP in the feedback path.
/// Sibling of the Moog `Ladder` effect: same family, same parameter
/// surface, different topology and character. Where the Moog is smooth
/// and self-oscillates cleanly, the diode is squelchy and aggressive --
/// the canonical "acid" sound comes from the high-pass filter sitting
/// inside the resonance feedback loop, not from a different slope.
///
/// Port of Matt Tytel's Vital `DiodeFilter` (GPLv3), simplified to the
/// 24 dB/oct topology (Vital's LP/HP/BP morph is a synth feature, not
/// an effect feature). Four lossy TPT integrator stages cross-couple to
/// their neighbours' previous-sample saturated state -- the diode
/// "current sharing" approximation that breaks the algebraic loop in
/// one pass without a Newton-Raphson solver. Stage 1 uses `tanh()`,
/// stage 4 uses a `[-1, 1]` clamp, stages 2 and 3 are linear.
///
/// The 20 Hz HP-in-feedback filter (Vital's `high_pass_feedback_`) is
/// the source of the squelch: it removes DC from the resonance return,
/// so heavy resonance can't accumulate offset and instead "honks" at
/// the cutoff frequency.
///
/// 2x oversampling via double-tick on the same input sample, matching
/// the Moog `Ladder`. Zero added latency.
///
/// Per channel state: 4 unsaturated integrator states + 4 saturated
/// states + 1 HP-feedback state. No allocations on the audio thread.
pub struct DiodeEffect {
    cutoff: f32,
    /// User-facing 0..1; mapped cubically internally to Vital's
    /// `kMinResonance..kMaxResonance` (0.7..17.0) for the actual loop
    /// gain. Self-oscillation kicks in around user-resonance >= 0.97.
    user_resonance: f32,
    drive_db: f32,
    mix: f32,
    sample_rate: f32,
    /// Pre-multiplied drive (linear, derived from `drive_db`).
    drive_lin: f32,
    /// Internal resonance after the cubic mapping.
    resonance: f32,
    /// `1 / sqrt(drive_lin)` -- gain compensation so Drive isn't doubling
    /// as a volume knob.
    post_multiply: f32,
    /// TPT one-pole LP coefficient `G = g / (1 + g)` for the cutoff.
    coef: f32,
    /// TPT one-pole LP coefficient for the 20 Hz HP-in-feedback path.
    /// Sample-rate dependent; cached on `set_sample_rate`.
    hp_fb_coef: f32,
    /// Per-channel unsaturated TPT integrator state per ladder stage.
    stage_s: [[f32; 4]; 2],
    /// Per-channel saturated integrator state per stage -- the value the
    /// NEXT tick reads as feedback. For linear stages (2, 3) this equals
    /// `stage_s`; for saturated stages (1, 4) it differs.
    stage_sat: [[f32; 4]; 2],
    /// Per-channel HP-in-feedback one-pole state (Vital's
    /// `high_pass_feedback_`).
    hp_fb_s: [f32; 2],
}

impl DiodeEffect {
    /// Vital's `kHighPassFrequency`: the corner of the in-loop HP that
    /// gives the diode topology its squelch by stripping DC from the
    /// resonance feedback.
    const HP_FEEDBACK_HZ: f32 = 20.0;
    /// Vital's `kMinResonance` -- the minimum internal feedback gain
    /// even at user resonance = 0.
    const MIN_RES: f32 = 0.7;
    /// Vital's `kMaxResonance` -- the feedback gain at user resonance
    /// = 1. Anything beyond is unstable; the cubic mapping keeps the
    /// useful self-oscillation region near the top of the knob.
    const MAX_RES: f32 = 17.0;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 800.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Resonance",
            min: 0.0,
            max: 1.0,
            default: 0.5,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
        ParamSpec {
            name: "Drive",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
        ParamSpec {
            name: "Mix",
            min: 0.0,
            max: 1.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
    ];

    pub fn new() -> Self {
        let mut effect = Self {
            cutoff: Self::PARAMS[0].default,
            user_resonance: Self::PARAMS[1].default,
            drive_db: Self::PARAMS[2].default,
            mix: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            drive_lin: 1.0,
            resonance: 0.0,
            post_multiply: 1.0,
            coef: 0.0,
            hp_fb_coef: 0.0,
            stage_s: [[0.0; 4]; 2],
            stage_sat: [[0.0; 4]; 2],
            hp_fb_s: [0.0; 2],
        };
        effect.recompute();
        effect
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        // Standard TPT prewarp: g = tan(pi * fc / sr); G = g / (1 + g).
        let g = (std::f32::consts::PI * fc / sr).tan();
        self.coef = g / (1.0 + g);

        let hp_g = (std::f32::consts::PI * Self::HP_FEEDBACK_HZ / sr).tan();
        self.hp_fb_coef = hp_g / (1.0 + hp_g);

        // Cubic resonance curve gives a soft toe near zero and a sharp
        // self-oscillation knee near the top -- the user-facing dial
        // feels roughly linear in perceived peak height.
        let r = self.user_resonance.clamp(0.0, 1.0);
        let r3 = r * r * r;
        self.resonance = Self::MIN_RES + r3 * (Self::MAX_RES - Self::MIN_RES);

        let drive_lin = 10.0_f32.powf(self.drive_db.clamp(0.0, 24.0) / 20.0);
        self.drive_lin = drive_lin;
        // Output trim so cranking Drive doesn't double as a volume knob.
        // sqrt() approximates an equal-loudness compensation for the
        // pre-saturation gain.
        self.post_multiply = 1.0 / drive_lin.sqrt();
    }

    /// TPT one-pole LP without saturation (Vital's `tickBasic`). The
    /// state-update is a textbook trapezoidal integrator: half-step,
    /// emit, half-step. Returns the LP output; state is updated in
    /// place.
    #[inline]
    fn tick_basic(state: &mut f32, input: f32, coef: f32) -> f32 {
        let delta = coef * (input - *state);
        *state += delta;
        let out = *state;
        *state += delta;
        out
    }

    /// TPT one-pole LP with `tanh` saturation on the integrator state.
    /// The unsaturated integrator `s` advances normally; the returned
    /// output is `tanh(s)` at the half-step, and the cached saturated
    /// state is `tanh(s)` after the full step -- it's what the next
    /// tick reads as the feedback term.
    #[inline]
    fn tick_tanh(s: &mut f32, sat: &mut f32, input: f32, coef: f32) -> f32 {
        let delta = coef * (input - *sat);
        *s += delta;
        let out = s.tanh();
        *s += delta;
        *sat = s.tanh();
        out
    }

    /// Same as `tick_tanh` but with a hard `[-1, 1]` clamp instead of
    /// `tanh` -- cheaper and produces sharper character at extreme
    /// drive. Used by the final ladder stage in Vital's design.
    #[inline]
    fn tick_clamp(s: &mut f32, sat: &mut f32, input: f32, coef: f32) -> f32 {
        let delta = coef * (input - *sat);
        *s += delta;
        let out = s.clamp(-1.0, 1.0);
        *s += delta;
        *sat = s.clamp(-1.0, 1.0);
        out
    }

    /// One ladder tick for one channel. The ZDF-style ordering matters:
    /// stage_k+1's previous-sample saturated state is read BEFORE that
    /// stage ticks, which is what breaks the otherwise-algebraic loop
    /// in single-pass.
    #[inline]
    fn diode_tick(&mut self, ch: usize, dry: f32) -> f32 {
        // Previous-sample stage 4 saturated state closes the resonance loop.
        let prev_stage4_sat = self.stage_sat[ch][3];
        let filter_input = (self.drive_lin * dry - self.resonance * prev_stage4_sat) * 0.5;
        let sat_input = filter_input.tanh();

        // HP-in-feedback: feedback_input = sat_input + stage2's previous
        // saturated state. Subtract the LP of that to get the HP of it --
        // that's the 20 Hz cutoff stripping DC from the resonance loop.
        let feedback_input = sat_input + self.stage_sat[ch][1];
        let hp_lp = Self::tick_basic(&mut self.hp_fb_s[ch], feedback_input, self.hp_fb_coef);
        let stage1_in = feedback_input - hp_lp;

        // Stage 1: tanh-saturated TPT LP. After this returns, the stage's
        // sat is updated; stages 2..3 read PREV sat values for their
        // cross-coupling because they haven't ticked yet.
        let stage_s = &mut self.stage_s[ch];
        let stage_sat = &mut self.stage_sat[ch];
        let stage1_out = {
            let (s_head, _) = stage_s.split_at_mut(1);
            let (sat_head, _) = stage_sat.split_at_mut(1);
            Self::tick_tanh(&mut s_head[0], &mut sat_head[0], stage1_in, self.coef)
        };

        // Stage 2: linear TPT LP, input = (stage1.current + stage3.PREV_sat) / 2.
        let stage3_prev_sat = stage_sat[2];
        let stage2_in = (stage1_out + stage3_prev_sat) * 0.5;
        let stage2_out = Self::tick_basic(&mut stage_s[1], stage2_in, self.coef);
        stage_sat[1] = stage_s[1];

        // Stage 3: linear TPT LP, input = (stage2.current + stage4.PREV_sat) / 2.
        let stage4_prev_sat = stage_sat[3];
        let stage3_in = (stage2_out + stage4_prev_sat) * 0.5;
        let stage3_out = Self::tick_basic(&mut stage_s[2], stage3_in, self.coef);
        stage_sat[2] = stage_s[2];

        // Stage 4: clamp-saturated TPT LP, fed straight from stage3.
        let stage4_out = Self::tick_clamp(&mut stage_s[3], &mut stage_sat[3], stage3_out, self.coef);
        stage4_out * self.post_multiply
    }

    fn process_channel(&mut self, ch: usize, dry: f32) -> f32 {
        // 2x oversampling via double-tick on the same input sample. Same
        // shortcut as the Moog `Ladder` -- the tanh stages alias most at
        // high cutoffs, and a second tick collapses most of the out-of-
        // band energy without introducing latency.
        self.diode_tick(ch, dry);
        self.diode_tick(ch, dry)
    }
}

impl Default for DiodeEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for DiodeEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let wet_l = self.process_channel(0, left);
        let wet_r = self.process_channel(1, right);
        let mix = self.mix.clamp(0.0, 1.0);
        let l = left + (wet_l - left) * mix;
        let r = right + (wet_r - right) * mix;
        (l, r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.stage_s = [[0.0; 4]; 2];
        self.stage_sat = [[0.0; 4]; 2];
        self.hp_fb_s = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.user_resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.drive_db = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => {
                self.mix = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max);
                return;
            }
            _ => return,
        }
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_declared() {
        let e = DiodeEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Drive");
        assert_eq!(specs[3].name, "Mix");
    }

    #[test]
    fn silent_input_stays_silent() {
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.5);
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn mix_zero_is_dry() {
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(3, 0.0);
        for i in 0..256 {
            let x = (i as f32 * 0.01).sin();
            let (l, r) = e.process_sample(x, -x);
            assert!((l - x).abs() < 1e-6);
            assert!((r - -x).abs() < 1e-6);
        }
    }

    #[test]
    fn dark_cutoff_attenuates_highs() {
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 200.0);
        e.set_param(1, 0.0);
        let mut peak = 0.0_f32;
        for i in 0..4096 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = e.process_sample(x, x);
            if i > 512 {
                peak = peak.max(l.abs());
            }
        }
        assert!(
            peak < 0.2,
            "a 200 Hz diode lowpass should crush Nyquist alternation, got {peak}"
        );
    }

    #[test]
    fn stable_under_modulation_and_max_resonance() {
        // Diode topology is more sensitive than Moog because resonance
        // moves pole locations, not just gain. Sweep cutoff at max
        // resonance and check the loop stays finite.
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 1.0);
        for i in 0..16_384 {
            let t = i as f32 / 16_384.0;
            let cutoff = 20.0 * 1000.0_f32.powf(t);
            e.set_param(0, cutoff);
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, r) = e.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite(), "lost stability: {l}, {r}");
        }
    }

    #[test]
    fn high_resonance_self_oscillates_after_a_kick() {
        // Vital's mapping puts self-oscillation right at the top of the
        // resonance knob (cubic curve into [0.7, 17.0]). Kick the filter
        // and check the tail still rings thousands of samples later.
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 800.0);
        e.set_param(1, 0.99);
        for _ in 0..16 {
            e.process_sample(1.0, 1.0);
        }
        for _ in 0..2048 {
            e.process_sample(0.0, 0.0);
        }
        let mut sum_sq = 0.0_f32;
        for _ in 0..4096 {
            let (l, _) = e.process_sample(0.0, 0.0);
            sum_sq += l * l;
        }
        let rms = (sum_sq / 4096.0).sqrt();
        assert!(
            rms > 0.005,
            "near-unity resonance should self-oscillate, got rms={rms}"
        );
    }

    #[test]
    fn low_resonance_decays_after_a_kick() {
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 800.0);
        e.set_param(1, 0.1);
        for _ in 0..16 {
            e.process_sample(1.0, 1.0);
        }
        for _ in 0..4096 {
            e.process_sample(0.0, 0.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..4096 {
            let (l, _) = e.process_sample(0.0, 0.0);
            peak = peak.max(l.abs());
        }
        assert!(
            peak < 1e-3,
            "low resonance should decay to silence, got peak={peak}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.9);
        for _ in 0..1024 {
            e.process_sample(1.0, -1.0);
        }
        e.reset();
        assert_eq!(e.stage_s, [[0.0; 4]; 2]);
        assert_eq!(e.stage_sat, [[0.0; 4]; 2]);
        assert_eq!(e.hp_fb_s, [0.0; 2]);
    }

    #[test]
    fn drive_changes_response() {
        let measure = |drive_db: f32| -> f32 {
            let mut e = DiodeEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 1_500.0);
            e.set_param(1, 0.4);
            e.set_param(2, drive_db);
            for i in 0..1024 {
                let x = (i as f32 * 0.05).sin() * 0.5;
                e.process_sample(x, x);
            }
            let mut sum = 0.0_f32;
            for i in 1024..(1024 + 4096) {
                let x = (i as f32 * 0.05).sin() * 0.5;
                let (l, _) = e.process_sample(x, x);
                sum += l * l;
            }
            (sum / 4096.0).sqrt()
        };
        let a = measure(0.0);
        let b = measure(24.0);
        assert!(
            (a - b).abs() > 1e-4,
            "drive should change response (a={a}, b={b})"
        );
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = DiodeEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
