use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use tract_dsp::fast_math::tanh_pade;

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

    pub(super) const PARAMS: [ParamSpec; 3] = [
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
    ];

    pub fn new() -> Self {
        let mut effect = Self {
            cutoff: Self::PARAMS[0].default,
            user_resonance: Self::PARAMS[1].default,
            drive_db: Self::PARAMS[2].default,
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
        let inner_sample_rate = 2.0 * sr;
        // TPT prewarp at the double-ticked rate: G = g / (1 + g).
        let g = (std::f32::consts::PI * fc / inner_sample_rate).tan();
        self.coef = g / (1.0 + g);

        let hp_g = (std::f32::consts::PI * Self::HP_FEEDBACK_HZ / inner_sample_rate).tan();
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
        let out = tanh_pade(*s);
        *s += delta;
        *sat = tanh_pade(*s);
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
        let sat_input = tanh_pade(filter_input);

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
        let stage4_out =
            Self::tick_clamp(&mut stage_s[3], &mut stage_sat[3], stage3_out, self.coef);
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
        (wet_l, wet_r)
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
            _ => return,
        }
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_highpass_corner_is_twenty_hertz_at_the_inner_tick_rate() {
        for sample_rate in [48_000.0, 96_000.0] {
            let mut diode = DiodeEffect::new();
            diode.set_sample_rate(sample_rate);
            let period = (2.0 * sample_rate / 20.0) as usize;
            let mut state = 0.0;
            let mut measured = [0.0_f64; 2];
            for sample in 0..8 * period {
                let angle = std::f64::consts::TAU * sample as f64 / period as f64;
                let (sin, cos) = angle.sin_cos();
                let low = DiodeEffect::tick_basic(&mut state, sin as f32, diode.hp_fb_coef);
                let high = sin - low as f64;
                if sample >= 4 * period {
                    measured[0] += high * sin;
                    measured[1] += high * cos;
                }
            }
            // A first-order highpass at its corner has H = (1 + j) / 2.
            for correlation in measured {
                let actual = correlation * 2.0 / (4 * period) as f64;
                assert!(
                    (actual - 0.5).abs() < 2e-5,
                    "rate {sample_rate}: {actual} vs 0.5"
                );
            }
        }
    }

    #[test]
    fn coefficient_calibration_uses_the_inner_rate_and_cubic_resonance() {
        for (sample_rate, cutoff, coef, hp_coef) in [
            (48_000.0, 800.0, 0.02551771664256012, 0.0006540704747675379),
            (
                44_100.0,
                20_000.0,
                0.46342553640548645,
                0.0007118722838799357,
            ),
            (
                96_000.0,
                12_000.0,
                0.16591068104035053,
                0.00032714218939591344,
            ),
            (8_000.0, 20_000.0, 0.4921453723381675, 0.003911649911246544),
        ] {
            let mut diode = DiodeEffect::new();
            diode.set_param(0, cutoff);
            diode.set_param(1, 0.375);
            diode.set_param(2, 13.7);
            diode.set_sample_rate(sample_rate);
            // f64 TPT prewarp anchors at twice the host rate.
            assert!((diode.coef as f64 - coef).abs() < 1e-7);
            assert!((diode.hp_fb_coef as f64 - hp_coef).abs() < 1e-9);
            assert!((diode.resonance as f64 - 1.5595703125).abs() < 2e-7);
            assert!((diode.drive_lin as f64 - 4.841723675840993).abs() < 2e-6);
            assert!((diode.post_multiply as f64 - 0.45446453720950875).abs() < 2e-7);
        }
    }

    #[test]
    fn linear_tick_emits_the_half_step_before_finishing_the_state_update() {
        for (initial, input, coef, expected_output, expected_state) in
            [(-0.4, 0.8, 0.25, -0.1, 0.2), (0.7, -0.5, 0.6, -0.02, -0.74)]
        {
            let mut state = initial;
            let output = DiodeEffect::tick_basic(&mut state, input, coef);
            assert!((output - expected_output).abs() < 1e-7);
            assert!((state - expected_state).abs() < 1e-7);
        }
    }

    #[test]
    fn saturated_tick_keeps_distinct_half_and_full_step_tanh_values() {
        for (initial, input, coef) in [(0.2_f64, 0.8, 0.25), (-0.9, -1.3, 0.4)] {
            let mut state = initial as f32;
            let mut saturated = initial.tanh() as f32;
            let delta = coef * (input - initial.tanh());
            let output =
                DiodeEffect::tick_tanh(&mut state, &mut saturated, input as f32, coef as f32);
            assert!((output as f64 - (initial + delta).tanh()).abs() < 2e-6);
            assert!((state as f64 - (initial + 2.0 * delta)).abs() < 2e-6);
            assert!((saturated as f64 - (initial + 2.0 * delta).tanh()).abs() < 2e-6);
        }
    }

    #[test]
    fn clamp_tick_preserves_the_raw_state_and_clips_both_polarities() {
        for (initial, input, coef, expected_output, expected_state, expected_saturated) in [
            (0.8, 4.0, 0.5, 1.0, 4.0, 1.0),
            (-0.8, -4.0, 0.5, -1.0, -4.0, -1.0),
            (0.2, -0.6, 0.25, 0.0, -0.2, -0.2),
            (0.8, 1.6, 0.2, 0.96, 1.12, 1.0),
        ] {
            let mut state = initial;
            let mut saturated = initial;
            let output = DiodeEffect::tick_clamp(&mut state, &mut saturated, input, coef);
            assert!((output - expected_output).abs() < 3e-7);
            assert!((state - expected_state).abs() < 3e-7);
            assert!((saturated - expected_saturated).abs() < 3e-7);
        }
    }

    #[test]
    fn nonlinear_tick_uses_previous_cross_coupled_states() {
        let mut diode = DiodeEffect::new();
        diode.coef = 0.2;
        diode.hp_fb_coef = 0.05;
        diode.drive_lin = 2.0;
        diode.resonance = 3.0;
        diode.post_multiply = 0.75;
        diode.stage_s[0] = [0.2, -0.3, 0.4, -0.5];
        diode.stage_sat[0] = [0.2_f32.tanh(), -0.3, 0.4, -0.5];
        diode.hp_fb_s[0] = 0.1;
        // f64 one-tick reference with libm tanh and nonzero prior states.
        assert!((diode.diode_tick(0, 0.6) as f64 + 0.26213178308).abs() < 2e-6);
        for (actual, expected) in diode.stage_s[0].iter().zip([
            0.301190121307,
            -0.050904410665,
            0.104909558934,
            -0.199018088213,
        ]) {
            assert!((*actual as f64 - expected).abs() < 2e-6);
        }
        for (actual, expected) in diode.stage_sat[0].iter().zip([
            0.292401358469,
            -0.050904410665,
            0.104909558934,
            -0.199018088213,
        ]) {
            assert!((*actual as f64 - expected).abs() < 2e-6);
        }
        assert!((diode.hp_fb_s[0] as f64 - 0.147405328789).abs() < 2e-6);
        assert_eq!(diode.stage_s[1], [0.0; 4]);
        assert_eq!(diode.stage_sat[1], [0.0; 4]);
        assert_eq!(diode.hp_fb_s[1], 0.0);
    }

    fn configured_diode() -> DiodeEffect {
        let mut diode = DiodeEffect::new();
        diode.set_sample_rate(96_000.0);
        for (index, value) in [4_200.0, 0.63, 13.7].into_iter().enumerate() {
            diode.set_param(index, value);
        }
        diode
    }

    #[test]
    fn stereo_channels_match_independent_mono_histories() {
        let mut stereo = configured_diode();
        let mut left_only = configured_diode();
        let mut right_only = configured_diode();
        for sample in 0..1_024 {
            let left = (sample as f32 * 0.17).sin() * 0.3;
            let right = (sample as f32 * 0.41).cos() * 0.7;
            let actual = stereo.process_sample(left, right);
            assert_eq!(actual.0, left_only.process_sample(left, 0.0).0);
            assert_eq!(actual.1, right_only.process_sample(0.0, right).1);
        }
    }

    #[test]
    fn reset_replays_a_fresh_diode_with_the_same_settings() {
        let mut used = configured_diode();
        let mut fresh = configured_diode();
        for sample in 0..1_024 {
            used.process_sample((sample as f32 * 0.19).sin(), (sample as f32 * 0.37).cos());
        }
        used.reset();
        used.set_param(99, 999.0);
        for sample in 0..512 {
            let input = ((sample as f32 * 0.27).cos(), (sample as f32 * 0.31).sin());
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn parameter_and_rate_changes_retain_diode_history() {
        let mut changed = configured_diode();
        let mut reference = configured_diode();
        for sample in 0..512 {
            let input = ((sample as f32 * 0.23).sin(), (sample as f32 * 0.47).cos());
            changed.process_sample(input.0, input.1);
            reference.process_sample(input.0, input.1);
        }
        changed.set_sample_rate(44_100.0);
        for (index, value) in [300.0, 0.1, 24.0].into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_sample_rate(96_000.0);
        for (index, value) in [4_200.0, 0.63, 13.7].into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_param(99, -1.0);
        for sample in 0..256 {
            let input = ((sample as f32 * 0.29).cos(), (sample as f32 * 0.31).sin());
            assert_eq!(
                changed.process_sample(input.0, input.1),
                reference.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn parameter_limits_match_their_endpoint_responses() {
        for (index, value, endpoint) in [
            (0, -1.0, 20.0),
            (0, 90_000.0, 20_000.0),
            (1, -1.0, 0.0),
            (1, 2.0, 1.0),
            (2, -6.0, 0.0),
            (2, 48.0, 24.0),
        ] {
            let mut clamped = configured_diode();
            let mut reference = configured_diode();
            clamped.set_param(index, value);
            reference.set_param(index, endpoint);
            for sample in 0..256 {
                let input = ((sample as f32 * 0.31).sin(), (sample as f32 * 0.47).cos());
                assert_eq!(
                    clamped.process_sample(input.0, input.1),
                    reference.process_sample(input.0, input.1)
                );
            }
        }
    }

    #[test]
    fn parameters_are_declared() {
        let e = DiodeEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Drive");
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [(20.0, 20_000.0, 800.0), (0.0, 1.0, 0.5), (0.0, 24.0, 0.0)]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(matches!(
            specs[1].format,
            ParamFormat::Number {
                decimals: 2,
                unit: ""
            }
        ));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number {
                decimals: 1,
                unit: "dB"
            }
        ));
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
