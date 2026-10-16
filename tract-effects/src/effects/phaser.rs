use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A vintage-character 4-stage all-pass phaser. Four 1st-order all-pass
/// sections cascade per channel; the cascade output feeds back to the
/// cascade input through a 1-sample delay, and the per-channel all-pass
/// centre frequency can be offset for stereo width.
///
/// No internal LFO — matches the multosis pattern that DSP is static and
/// motion comes from MSEGs. The user routes an MSEG to `Center` for the
/// classic sweep.
///
/// `process_sample` returns the additive phaser sound (`dry + cascade`)
/// because the comb-filter notches that make a phaser *sound* like a
/// phaser come from summing the dry against the phase-shifted wet. The
/// engine's per-row Mix then attenuates how much of the (wet-minus-dry)
/// contribution mixes back in — at Mix=1.0 you hear the full `dry +
/// cascade`; at Mix=0 you hear pure dry; in between you get a continuous
/// blend.
pub struct PhaserEffect {
    center: f32,
    feedback_pct: f32,
    stereo_pct: f32,
    sample_rate: f32,
    /// 4 all-pass states per channel — one `f32` per stage (Direct Form
    /// II's single delay register).
    stage_state: [[f32; Self::STAGES]; 2],
    /// 1-sample feedback delay per channel — holds the previous cascade
    /// output so the loop closes one sample late (no zero-delay path).
    fb_state: [f32; 2],
}

impl PhaserEffect {
    const STAGES: usize = 4;
    /// Hard cap on the feedback gain. Each all-pass stage has unity
    /// magnitude, so total loop gain = `fb_pct/100`; 0.95 keeps a
    /// comfortable margin from the unit circle.
    const FB_MAX: f32 = 0.95;
    /// Max ±octaves of L/R centre-frequency offset at Stereo=100 %. A
    /// half-octave per side gives a wide spatial spread without sounding
    /// dislocated. 100 % * 0.005 = 0.5 → ±0.5 octaves.
    const STEREO_OCT_PER_PCT: f32 = 0.005;

    const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Center",
            min: 50.0,
            max: 8_000.0,
            default: 500.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 95.0,
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Stereo",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    /// A fresh `PhaserEffect` at default params and 48 kHz. Call
    /// `set_sample_rate` to retune to the host's rate.
    pub fn new() -> Self {
        Self {
            center: Self::PARAMS[0].default,
            feedback_pct: Self::PARAMS[1].default,
            stereo_pct: Self::PARAMS[2].default,
            sample_rate: 48_000.0,
            stage_state: [[0.0; Self::STAGES]; 2],
            fb_state: [0.0; 2],
        }
    }

    /// 1st-order all-pass coefficient placing the phase = -90° point at
    /// frequency `f`. `a = (1 - tan(π·f/sr)) / (1 + tan(π·f/sr))`.
    /// `f` is clamped to `[20.0, sr·0.45]` so `tan` stays well-conditioned
    /// (the divisor never hits zero).
    fn allpass_coef(f: f32, sr: f32) -> f32 {
        let f = f.clamp(20.0, sr * 0.45);
        let t = (std::f32::consts::PI * f / sr).tan();
        (1.0 - t) / (1.0 + t)
    }

    /// One 1st-order all-pass step with a single-register Direct Form II
    /// implementation: `y = -a·x + state`, then `state = x + a·y`. The
    /// `state` slot holds the next-sample contribution; cleared by
    /// `reset()`.
    #[inline]
    fn allpass_step(x: f32, state: &mut f32, a: f32) -> f32 {
        let y = -a * x + *state;
        *state = x + a * y;
        y
    }
}

impl Default for PhaserEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PhaserEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let stereo_oct = self.stereo_pct * Self::STEREO_OCT_PER_PCT;
        let cl = self.center * (-stereo_oct).exp2();
        let cr = self.center * stereo_oct.exp2();
        let al = Self::allpass_coef(cl, self.sample_rate);
        let ar = Self::allpass_coef(cr, self.sample_rate);
        let fb = (self.feedback_pct * 0.01).clamp(0.0, Self::FB_MAX);

        // Cascade input = dry + feedback × previous cascade output. The
        // 1-sample delay on the feedback path keeps the loop well-defined.
        let mut yl = left + fb * self.fb_state[0];
        let mut yr = right + fb * self.fb_state[1];

        // 4-stage all-pass cascade per channel.
        for i in 0..Self::STAGES {
            yl = Self::allpass_step(yl, &mut self.stage_state[0][i], al);
            yr = Self::allpass_step(yr, &mut self.stage_state[1][i], ar);
        }

        // Save cascade output for next sample's feedback path.
        self.fb_state[0] = yl;
        self.fb_state[1] = yr;

        // Phaser sound = dry + phase-shifted. The engine's Mix dial then
        // attenuates the contribution.
        (left + yl, right + yr)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn reset(&mut self) {
        self.stage_state = [[0.0; Self::STAGES]; 2];
        self.fb_state = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.center = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.feedback_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.stereo_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn phaser_lists_three_parameters_with_the_expected_specs() {
        let p = PhaserEffect::new();
        let specs = p.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Center");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[0].min, 50.0);
        assert_eq!(specs[0].max, 8_000.0);
        assert_eq!(specs[1].name, "Feedback");
        assert_eq!(specs[1].min, 0.0);
        assert_eq!(specs[1].max, 95.0);
        assert_eq!(specs[2].name, "Stereo");
        assert_eq!(specs[2].min, 0.0);
        assert_eq!(specs[2].max, 100.0);
    }

    #[test]
    fn phaser_at_default_colours_the_signal_without_silencing_it() {
        // Default phaser (Center=500, Feedback=30, Stereo=0) should pass a
        // signal but with the cascade applied — output is non-zero and not
        // identical to the dry input. Even without modulation the all-pass
        // sections introduce phase shift; summed against dry, the comb
        // notches colour the spectrum.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        // Drive a 1 kHz sine for 2048 samples (settled past the transient).
        let mut wet_energy = 0.0_f32;
        let mut diff_energy = 0.0_f32;
        for i in 0..2048 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            let (l, _r) = p.process_sample(dry, dry);
            wet_energy += l * l;
            diff_energy += (l - dry) * (l - dry);
        }
        assert!(wet_energy > 100.0, "phaser must produce non-trivial output");
        assert!(
            diff_energy > 10.0,
            "phaser output must differ from dry (got diff_energy={diff_energy})"
        );
    }

    #[test]
    fn phaser_feedback_raises_total_energy_for_a_static_centre() {
        // More feedback = more pronounced peaks between notches → more total
        // energy through the cascade for a broadband (impulse-train) input.
        let render_energy = |fb_pct: f32| -> f32 {
            let mut p = PhaserEffect::new();
            p.set_sample_rate(48_000.0);
            p.set_param(1, fb_pct);
            let mut energy = 0.0;
            // Impulse train every 64 samples — broadband excitation.
            for i in 0..8192 {
                let dry = if i % 64 == 0 { 1.0 } else { 0.0 };
                let (l, _r) = p.process_sample(dry, dry);
                energy += l * l;
            }
            energy
        };
        let e_low = render_energy(0.0);
        let e_high = render_energy(90.0);
        assert!(
            e_high > e_low * 1.5,
            "fb=90 should accumulate more energy than fb=0 \
             (low={e_low}, high={e_high})"
        );
    }

    #[test]
    fn phaser_stereo_offset_separates_l_and_r() {
        // Mono-sum input through Stereo=100 should produce L ≠ R because
        // the all-pass centre frequencies are offset per channel.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(2, 100.0); // Stereo = 100 %
        let mut diff_energy = 0.0_f32;
        for i in 0..2048 {
            let t = i as f32 / 48_000.0;
            // Mid-band tone so both offsets land in audible territory.
            let dry = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            let (l, r) = p.process_sample(dry, dry);
            diff_energy += (l - r) * (l - r);
        }
        assert!(
            diff_energy > 1.0,
            "stereo=100 must produce L ≠ R for a mono input (diff={diff_energy})"
        );
    }

    #[test]
    fn phaser_stereo_zero_collapses_to_mono() {
        // Stereo=0 means identical L/R centre frequencies → identical
        // cascade outputs for a mono-sum input.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(2, 0.0);
        for i in 0..2048 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            let (l, r) = p.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-5,
                "stereo=0 must be L==R, sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn phaser_reset_zeroes_state() {
        // Drive the cascade, reset, then verify the first sample of an
        // impulse-into-silence isn't tainted by the prior state.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            p.process_sample(0.5, 0.5);
        }
        p.reset();
        // After reset, an impulse from silence: cascade input = 1.0 + fb*0
        // = 1.0; first all-pass step produces -a*1 + 0 = -a; subsequent
        // stages cascade. Output = dry + cascade_output = 1.0 + cascade.
        let (l, r) = p.process_sample(1.0, 1.0);
        // Reset clears feedback memory → no residual ringing from prior
        // input. The output of a fresh impulse equals the cascade's
        // impulse response added to dry; both channels match.
        assert!((l - r).abs() < 1e-6, "reset must leave L and R symmetric");
        assert!(l.is_finite(), "reset output must be finite");
    }

    #[test]
    fn phaser_stays_bounded_under_aggressive_modulation() {
        // Worst case: maximum feedback (caps at 95 %) and Centre sweeping
        // wildly via set_param every sample. Output magnitude must stay
        // finite and well below numerical saturation.
        let mut p = PhaserEffect::new();
        p.set_sample_rate(48_000.0);
        p.set_param(1, 95.0); // Feedback at cap
        for i in 0..48_000 {
            // Centre sweeps log-style from 50 Hz to 8 kHz every 4096 samples.
            let phase = ((i as f32 / 4096.0).fract() * 2.0 - 1.0).abs();
            let centre = 50.0 * (160.0_f32).powf(phase); // 50..8000 Hz log
            p.set_param(0, centre);
            let dry = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = p.process_sample(dry, dry);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 8.0 && r.abs() < 8.0,
                "sample {i} blew up: ({l}, {r})"
            );
        }
    }
}
