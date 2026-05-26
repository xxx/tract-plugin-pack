use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Sallen-Key 2-pole filter in the Korg-35 / MS-20 lineage. Distinct
/// topology from the existing SVF (state-variable), Ladder (Moog), and
/// Diode (TB-303) filters: two cascaded TPT one-poles cross-coupled
/// with positive feedback, with a `tanh` saturator on the feedback-
/// summing node for the famous "bite" character.
///
/// Direct port of Surge XT's `sst-filters` `K35Filter` (GPL-3, same
/// license as multosis). Both Lowpass and Highpass variants share the
/// coefficient calculation but differ in how the two TPT one-poles
/// are connected -- mirrored here as `process_channel_lp` and
/// `process_channel_hp`. Type selects between them per-sample without
/// any state reset, so modulating Type doesn't click.
///
/// 2x internal oversampling via double-tick on the same input sample
/// matches the Ladder / Diode pattern; the tanh in the feedback path
/// is the chief aliasing source and a second tick collapses most of
/// its out-of-band energy at no latency cost.
///
/// Per-channel state: 3 z-1 values (LPF1, HPF1, second-stage). No
/// allocations on the audio thread.
pub struct SallenKeyEffect {
    cutoff: f32,
    user_resonance: f32,
    drive_db: f32,
    type_idx: f32,
    sample_rate: f32,

    // Cached coefficients (recomputed when cutoff / resonance / drive
    // change or sample rate updates).
    coef_g: f32,
    /// Type-dependent feedback mixing coefficient for the second-stage
    /// z-1 (`k35_lb` in Surge's nomenclature).
    coef_lb: f32,
    /// Type-dependent feedback mixing coefficient for the first-stage
    /// HPF z-1 (`k35_hb` in Surge's nomenclature).
    coef_hb: f32,
    /// Resonance feedback gain `k = user_resonance * 1.96`. The 1.96
    /// upper bound is Surge's calibration -- self-oscillation kicks
    /// in right at the top of the dial.
    coef_k: f32,
    /// Loop-closing scalar derived from `g` and `k`.
    coef_alpha: f32,
    /// `tanh` saturation amount derived from Drive; doubles as the
    /// clean / driven blend factor below 1.0.
    coef_saturation: f32,
    coef_sat_blend: f32,
    coef_sat_blend_inv: f32,

    // Per-channel state -- LPF1 z-1, HPF1 z-1, second-stage z-1.
    lz: [f32; 2],
    hz: [f32; 2],
    z2: [f32; 2],
}

const SK_TYPE_LABELS: &[&str] = &["Lowpass", "Highpass"];

const SK_TYPE_LP: usize = 0;
// Highpass uses the `_` arm in `process_channel`; only LP needs an
// explicit constant.

impl SallenKeyEffect {
    /// Maximum value of `k` -- Surge's K35 self-oscillation bound.
    const K_MAX: f32 = 1.96;
    /// Minimum value of `k` -- below this the resonance loop is so
    /// damped the filter is effectively passive, which is fine for
    /// `Resonance = 0` UX.
    const K_MIN: f32 = 0.01;

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
            format: ParamFormat::Number { decimals: 2, unit: "" },
        },
        ParamSpec {
            name: "Drive",
            min: 0.0,
            max: 24.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 1, unit: "dB" },
        },
        ParamSpec {
            name: "Type",
            min: 0.0,
            max: (SK_TYPE_LABELS.len() - 1) as f32,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum { labels: SK_TYPE_LABELS },
        },
    ];

    pub fn new() -> Self {
        let mut e = Self {
            cutoff: Self::PARAMS[0].default,
            user_resonance: Self::PARAMS[1].default,
            drive_db: Self::PARAMS[2].default,
            type_idx: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            coef_g: 0.0,
            coef_lb: 0.0,
            coef_hb: 0.0,
            coef_k: 0.0,
            coef_alpha: 0.0,
            coef_saturation: 0.0,
            coef_sat_blend: 0.0,
            coef_sat_blend_inv: 1.0,
            lz: [0.0; 2],
            hz: [0.0; 2],
            z2: [0.0; 2],
        };
        e.recompute();
        e
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        // TPT prewarp: g = tan(pi * fc / sr), G = g / (1 + g).
        let g = (std::f32::consts::PI * fc / sr).tan();
        let gp1 = 1.0 + g;
        self.coef_g = g / gp1;

        let mk = (self.user_resonance.clamp(0.0, 1.0) * Self::K_MAX).clamp(Self::K_MIN, Self::K_MAX);
        self.coef_k = mk;
        self.coef_alpha = 1.0 / (1.0 - mk * self.coef_g + mk * self.coef_g * self.coef_g);

        match self.type_idx.round() as usize {
            SK_TYPE_LP => {
                // LP mixing: lb = k * (1 - G) / (1 + g) = (k - k*G) / (1+g)
                self.coef_lb = (mk - mk * self.coef_g) / gp1;
                self.coef_hb = -1.0 / gp1;
            }
            _ => {
                // HP mixing: lb = 1 / (1 + g); hb = -G / (1 + g)
                self.coef_lb = 1.0 / gp1;
                self.coef_hb = -self.coef_g / gp1;
            }
        }

        // Drive (dB) -> saturation amount. Drive = 0 dB gives saturation
        // = 0 (fully clean via the sat_blend = 0 path); higher Drive
        // pushes harder into the tanh AND blends in more of the driven
        // signal until sat_blend = 1 at Drive ~ 6 dB, past which the
        // tanh argument keeps growing (more harmonic content) while
        // the blend stays fully driven.
        let drive_clamped = self.drive_db.clamp(0.0, 24.0);
        let saturation = 10.0_f32.powf(drive_clamped / 20.0) - 1.0;
        self.coef_saturation = saturation;
        self.coef_sat_blend = saturation.min(1.0);
        self.coef_sat_blend_inv = 1.0 - self.coef_sat_blend;
    }

    /// TPT one-pole LP step. Updates the `z` state in place and returns
    /// the LP output. Mirrors Surge's `doLpf`.
    #[inline]
    fn do_lpf(g: f32, input: f32, z: &mut f32) -> f32 {
        let v = (input - *z) * g;
        let out = v + *z;
        *z = out + v;
        out
    }

    fn process_channel_lp(&mut self, ch: usize, input: f32) -> f32 {
        let g = self.coef_g;
        let y1 = Self::do_lpf(g, input, &mut self.lz[ch]);
        // Feedback summing node: mixes the two prior-stage z-1 values.
        let s35 = self.coef_lb * self.z2[ch] + self.coef_hb * self.hz[ch];
        // Loop close: alpha resolves the algebraic feedback path.
        let u_clean = self.coef_alpha * (y1 + s35);
        let u_driven = (u_clean * self.coef_saturation).tanh();
        let u = u_clean * self.coef_sat_blend_inv + u_driven * self.coef_sat_blend;
        // Second-stage LPF on the saturated feedback signal.
        let lp2 = Self::do_lpf(g, u, &mut self.z2[ch]);
        let y = self.coef_k * lp2;
        // Update hz state via a doLpf side-effect; the LP output of
        // this stage isn't used in the LP path, only the z update.
        let _ = Self::do_lpf(g, y, &mut self.hz[ch]);
        y / self.coef_k
    }

    fn process_channel_hp(&mut self, ch: usize, input: f32) -> f32 {
        let g = self.coef_g;
        // y1 = doHpf(G, input, hz) = input - doLpf(G, input, hz)
        let lp1 = Self::do_lpf(g, input, &mut self.hz[ch]);
        let y1 = input - lp1;
        // Feedback summing -- coefficients swap roles vs the LP path.
        let s35 = self.coef_lb * self.lz[ch] + self.coef_hb * self.z2[ch];
        let u = self.coef_alpha * (y1 + s35);
        let y_clean = self.coef_k * u;
        let y_driven = (y_clean * self.coef_saturation).tanh();
        let y = y_clean * self.coef_sat_blend_inv + y_driven * self.coef_sat_blend;
        // doLpf(G, doHpf(G, y, z2), lz): the inner doHpf updates z2
        // and emits y - doLpf(G, y, z2); the outer doLpf updates lz.
        let lp_inner = Self::do_lpf(g, y, &mut self.z2[ch]);
        let hp_inner = y - lp_inner;
        let _ = Self::do_lpf(g, hp_inner, &mut self.lz[ch]);
        y / self.coef_k
    }

    fn process_channel(&mut self, ch: usize, input: f32) -> f32 {
        // Double-tick at 2x: aliasing from the in-feedback tanh
        // concentrates at the sample-rate boundary; a second tick
        // pushes most of it out of band at zero added latency.
        match self.type_idx.round() as usize {
            SK_TYPE_LP => {
                self.process_channel_lp(ch, input);
                self.process_channel_lp(ch, input)
            }
            _ => {
                self.process_channel_hp(ch, input);
                self.process_channel_hp(ch, input)
            }
        }
    }
}

impl Default for SallenKeyEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for SallenKeyEffect {
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
        self.lz = [0.0; 2];
        self.hz = [0.0; 2];
        self.z2 = [0.0; 2];
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
                let max_idx = (SK_TYPE_LABELS.len() - 1) as f32;
                self.type_idx = value.round().clamp(0.0, max_idx);
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
        let e = SallenKeyEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Drive");
        assert_eq!(specs[3].name, "Type");
    }

    #[test]
    fn silent_input_stays_silent() {
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.5);
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn lowpass_attenuates_highs() {
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 300.0);
        e.set_param(1, 0.0);
        e.set_param(3, 0.0); // LP
        let mut peak = 0.0_f32;
        for i in 0..4096 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = e.process_sample(x, x);
            if i > 512 {
                peak = peak.max(l.abs());
            }
        }
        assert!(peak < 0.3, "300 Hz LP should crush Nyquist, got {peak}");
    }

    #[test]
    fn highpass_attenuates_lows() {
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 5_000.0);
        e.set_param(1, 0.0);
        e.set_param(3, 1.0); // HP
        let mut y = 0.0_f32;
        for _ in 0..4096 {
            y = e.process_sample(1.0, 1.0).0;
        }
        assert!(y.abs() < 0.3, "5 kHz HP should kill DC, got {y}");
    }

    #[test]
    fn stable_under_modulation_and_max_resonance() {
        // Sweep cutoff with max resonance + max drive. The tanh in
        // the feedback path is what bounds the loop -- output must
        // stay finite throughout.
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 1.0);
        e.set_param(2, 24.0);
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
        // The K35 self-oscillation threshold sits at mk ~= 1.96 (Surge's
        // `K_MAX`). Pure linear feedback at that boundary leaves the
        // loop gain at exactly 1 -- floating-point rounding decides
        // whether the oscillation grows or decays. A small amount of
        // Drive engages the tanh saturator, which clamps the loop at a
        // stable self-oscillating amplitude. This matches the practical
        // K35 use case (some drive is the normal state).
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 800.0);
        e.set_param(1, 1.0);
        e.set_param(2, 12.0);
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
        assert!(rms > 0.005, "high resonance should self-oscillate, got rms={rms}");
    }

    #[test]
    fn type_switch_changes_response() {
        // Same cutoff + input, different Type -> different output.
        let measure = |type_idx: f32| -> f32 {
            let mut e = SallenKeyEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 1_000.0);
            e.set_param(1, 0.3);
            e.set_param(3, type_idx);
            for i in 0..2048 {
                let x = (i as f32 * 0.02).sin();
                e.process_sample(x, x);
            }
            let mut sum = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let x = (i as f32 * 0.02).sin();
                let (l, _) = e.process_sample(x, x);
                sum += l * l;
            }
            (sum / 4096.0).sqrt()
        };
        let lp = measure(0.0);
        let hp = measure(1.0);
        assert!(
            (lp - hp).abs() > 0.05,
            "LP and HP should produce distinct responses (lp={lp}, hp={hp})"
        );
    }

    #[test]
    fn drive_introduces_nonlinearity() {
        let measure = |drive: f32| -> f32 {
            let mut e = SallenKeyEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 1_500.0);
            e.set_param(1, 0.4);
            e.set_param(2, drive);
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
        let clean = measure(0.0);
        let hot = measure(24.0);
        assert!(
            (clean - hot).abs() > 1e-3,
            "drive should change response (clean={clean}, hot={hot})"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.9);
        for _ in 0..1024 {
            e.process_sample(1.0, -1.0);
        }
        e.reset();
        assert_eq!(e.lz, [0.0; 2]);
        assert_eq!(e.hz, [0.0; 2]);
        assert_eq!(e.z2, [0.0; 2]);
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = SallenKeyEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
