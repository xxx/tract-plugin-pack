use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Static notch-comb filter — a cascade of 12 one-pole allpass stages whose
/// break frequency is Cutoff, summed against the dry signal so the response
/// is a periodic notch-comb. Sits in the Filter family because the user
/// drives sweep motion through Cutoff modulation (LFO/MSEG), exactly like
/// any other filter; the "Phaser" effect in the Modulation family is the
/// LFO-driven sweep variant.
///
/// Mirrors the structure of Xfer Serum's filter-section "Phaser" and the
/// open implementation in Matt Tytel's Vital (`PhaserFilter`, GPLv3): three
/// blocks of four one-pole allpass stages, with output taps after stages 4,
/// 8, 12. Blend crossfades across the three taps, producing the Phs12 / Phs24
/// / Phs36 timbres from a single continuous knob. Resonance is feedback
/// around the cascade, band-conditioned by two slow lowpass states so DC
/// and Nyquist can't blow it up; the feedback is `tanh`-clamped.
///
/// Per channel state: 12 one-pole LP integrator states for the allpass
/// cascade, 2 conditioning LP states, the previous allpass output (the
/// feedback term). No allocations on the audio thread.
pub struct PhaserFilterEffect {
    cutoff: f32,
    resonance: f32,
    /// 0..1; mapped to Vital's `pass_blend` 0..2 internally.
    blend: f32,
    mix: f32,
    sample_rate: f32,
    /// Smoothed one-pole TPT coefficient `tan(pi*f/sr) / (1 + tan(pi*f/sr))`.
    coef: f32,
    /// Per-channel allpass cascade state (one f32 per stage).
    stages: [[f32; Self::N_STAGES]; 2],
    /// Per-channel "remove lows" conditioning LP state (slow tracker of the
    /// allpass output).
    cond_low: [f32; 2],
    /// Per-channel "remove highs" conditioning LP state (even slower).
    cond_high: [f32; 2],
    /// Per-channel previous-sample allpass output — the feedback source.
    last_ap: [f32; 2],
}

impl PhaserFilterEffect {
    /// Four allpass stages per peak tap (after stage 4, 8, and 12 the
    /// cascade re-taps for peak1/peak3/peak5 respectively).
    const PEAK_STAGE: usize = 4;
    /// Total allpass stages: three blocks of four.
    const N_STAGES: usize = 3 * Self::PEAK_STAGE;
    /// Ratio between the cascade cutoff and the two conditioning filters
    /// that bandlimit the feedback (so DC and Nyquist can't accumulate).
    /// Vital's `kClearRatio`.
    const CLEAR_RATIO: f32 = 20.0;

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
            default: 0.3,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
        ParamSpec {
            name: "Blend",
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

    /// A `PhaserFilterEffect` at its default parameters; call
    /// `set_sample_rate` before processing.
    pub fn new() -> Self {
        let mut effect = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            blend: Self::PARAMS[2].default,
            mix: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            coef: 0.0,
            stages: [[0.0; Self::N_STAGES]; 2],
            cond_low: [0.0; 2],
            cond_high: [0.0; 2],
            last_ap: [0.0; 2],
        };
        effect.recompute();
        effect
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        let tan_w = (std::f32::consts::PI * fc / sr).tan();
        self.coef = tan_w / (1.0 + tan_w);
    }

    /// Decompose the 0..1 Blend knob into the three tap weights so they sum
    /// to 1 and crossfade peak1 -> peak3 -> peak5 across the range. Mirrors
    /// Vital's `pass_blend` formula with the input pre-doubled.
    fn tap_weights(&self) -> (f32, f32, f32) {
        let b = self.blend.clamp(0.0, 1.0) * 2.0;
        let p1 = (1.0 - b).clamp(0.0, 1.0);
        let p5 = (b - 1.0).clamp(0.0, 1.0);
        let p3 = (1.0 - p1 - p5).max(0.0);
        (p1, p3, p5)
    }

    #[inline]
    fn one_pole_lp(state: &mut f32, input: f32, coef: f32) -> f32 {
        *state += coef * (input - *state);
        *state
    }

    /// One sample through one channel's full phaser-filter chain. Per-stage
    /// state and `last_ap` are read/updated in place; the dry input is mixed
    /// at the caller.
    fn process_channel(&mut self, ch: usize, dry: f32) -> f32 {
        // Two slow LP states condition the previous allpass output into a
        // bandlimited feedback term — Vital's `remove_lows_stage_` /
        // `remove_highs_stage_`. Centre wide so this band-limits without
        // notching the feedback near cutoff.
        let prev = self.last_ap[ch];
        let lo_coef = (self.coef * Self::CLEAR_RATIO).min(0.9);
        let hi_coef = self.coef * (1.0 / Self::CLEAR_RATIO);
        let lows = Self::one_pole_lp(&mut self.cond_low[ch], prev, lo_coef);
        let highs = Self::one_pole_lp(&mut self.cond_high[ch], lows, hi_coef);
        // tanh-saturated resonance keeps the cascade stable at res = 1.
        let fb = (self.resonance * (lows - highs)).tanh();

        // Slight resonance-tied drive boost matches Vital's `(res*0.5 + 1)`.
        let drive = self.resonance * 0.5 + 1.0;
        let mut ap = drive * dry + fb;

        let coef = self.coef;
        let stages = &mut self.stages[ch];
        // One-pole allpass: out = in - 2 * LP(in). Cascade 12 stages,
        // capturing the running signal after stages 4, 8, 12.
        for stage in stages.iter_mut().take(Self::PEAK_STAGE) {
            let lp = Self::one_pole_lp(stage, ap, coef);
            ap -= 2.0 * lp;
        }
        let peak1 = ap;
        for stage in stages
            .iter_mut()
            .skip(Self::PEAK_STAGE)
            .take(Self::PEAK_STAGE)
        {
            let lp = Self::one_pole_lp(stage, ap, coef);
            ap -= 2.0 * lp;
        }
        let peak3 = ap;
        for stage in stages.iter_mut().skip(2 * Self::PEAK_STAGE) {
            let lp = Self::one_pole_lp(stage, ap, coef);
            ap -= 2.0 * lp;
        }
        let peak5 = ap;

        let (w1, w3, w5) = self.tap_weights();
        let allpass_out = w1 * peak1 + w3 * peak3 + w5 * peak5;
        self.last_ap[ch] = allpass_out;
        // Sum-of-dry-and-allpass produces the notch comb.
        0.5 * (dry + allpass_out)
    }
}

impl Default for PhaserFilterEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PhaserFilterEffect {
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
        self.stages = [[0.0; Self::N_STAGES]; 2];
        self.cond_low = [0.0; 2];
        self.cond_high = [0.0; 2];
        self.last_ap = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max);
                self.recompute();
            }
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.blend = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.mix = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_declared() {
        let e = PhaserFilterEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Blend");
        assert_eq!(specs[3].name, "Mix");
    }

    #[test]
    fn silent_input_stays_silent() {
        let mut e = PhaserFilterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 1.0); // max resonance
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn mix_zero_is_dry() {
        let mut e = PhaserFilterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(3, 0.0); // mix = 0
        for i in 0..256 {
            let x = (i as f32 * 0.01).sin();
            let (l, r) = e.process_sample(x, -x);
            assert!((l - x).abs() < 1e-6);
            assert!((r - -x).abs() < 1e-6);
        }
    }

    #[test]
    fn stable_at_max_resonance_with_dc() {
        // Worst case for an allpass cascade with feedback: DC input + max
        // resonance. The conditioning + tanh feedback path must keep the
        // output bounded.
        let mut e = PhaserFilterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 800.0);
        e.set_param(1, 1.0);
        for _ in 0..16_384 {
            let (l, r) = e.process_sample(1.0, 1.0);
            assert!(l.is_finite() && r.is_finite(), "lost stability: {l}, {r}");
            assert!(l.abs() < 5.0 && r.abs() < 5.0, "runaway feedback: {l}, {r}");
        }
    }

    #[test]
    fn stable_across_cutoff_sweep() {
        // Sweep cutoff across the audible range at high resonance, with a
        // moving sine input. Output stays finite and bounded throughout.
        let mut e = PhaserFilterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.9);
        for i in 0..8192 {
            let t = i as f32 / 8192.0;
            let cutoff = 20.0 * (1000.0_f32.powf(t)); // 20 Hz -> 20 kHz
            e.set_param(0, cutoff);
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, r) = e.process_sample(x, x);
            assert!(l.is_finite() && r.is_finite());
            assert!(l.abs() < 3.0 && r.abs() < 3.0);
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut e = PhaserFilterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.8);
        for _ in 0..1024 {
            e.process_sample(1.0, -1.0);
        }
        e.reset();
        for &s in e.stages[0].iter().chain(e.stages[1].iter()) {
            assert_eq!(s, 0.0);
        }
        assert_eq!(e.cond_low, [0.0; 2]);
        assert_eq!(e.cond_high, [0.0; 2]);
        assert_eq!(e.last_ap, [0.0; 2]);
    }

    #[test]
    fn blend_changes_response() {
        // Different Blend settings emphasise different tap counts and so
        // produce different outputs from the same input. Compare the RMS
        // of two settings to confirm they're not identical.
        let measure = |blend: f32| -> f32 {
            let mut e = PhaserFilterEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 1_000.0);
            e.set_param(1, 0.5);
            e.set_param(2, blend);
            let mut sum = 0.0_f32;
            for i in 0..4096 {
                let x = (i as f32 * 0.05).sin() * 0.5;
                let (l, _) = e.process_sample(x, x);
                sum += l * l;
            }
            (sum / 4096.0).sqrt()
        };
        let a = measure(0.0);
        let b = measure(1.0);
        assert!(
            (a - b).abs() > 1e-3,
            "Blend should change the response (a={a}, b={b})"
        );
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = PhaserFilterEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
