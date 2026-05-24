use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A multimode state-variable filter — `n` cascaded TPT-SVF stages, each
/// contributing 2 poles (12 dB/oct). The `Poles` param picks the cascade
/// length from {2, 4, 6, 8} poles (12 / 24 / 36 / 48 dB/oct slopes). The
/// `Type` param selects which SVF output tap each stage emits: lowpass
/// (LP), bandpass (BP), or highpass (HP).
///
/// Resonance is applied to the **last** stage only; earlier stages run
/// Butterworth (Q = 0.707, no peak). If every stage shared the user's Q
/// the resonance peak at cutoff would compound by `Q^stages` — at 8
/// poles even Q = 2 produces a 16× peak. Keeping the cascade
/// Butterworth except for the final stage makes the Resonance knob
/// mean roughly the same peak height across pole counts.
///
/// Stage state is per-cascade-position; only the first `stages_count()`
/// stages are touched on the audio thread. State is preserved across
/// param changes so a cutoff or resonance sweep doesn't click.
pub struct SvfEffect {
    cutoff: f32,
    resonance: f32,
    /// 0..3 selector index into [2, 4, 6, 8] poles. Stored as f32 so the
    /// existing Enum-format dropdown machinery handles it identically to
    /// FM Mode / FM Topology.
    poles_idx: f32,
    /// 0..2 selector index into [LP, BP, HP]. Stored as f32 like the
    /// other Enum-format params.
    type_idx: f32,
    sample_rate: f32,
    /// Butterworth (Q = 0.707, no peak) coefficients — used by every
    /// stage except the last. Tuple order is `(a1, a2, a3, k)` where
    /// `k = 1/Q` (needed for the HP tap `v3 − k · v1`).
    butter: (f32, f32, f32, f32),
    /// User-resonance coefficients — used by the LAST cascade stage only.
    /// At pole count = 2 (one stage), this is also the only set in play.
    res: (f32, f32, f32, f32),
    stages_ic1: [[f32; 2]; Self::MAX_STAGES],
    stages_ic2: [[f32; 2]; Self::MAX_STAGES],
}

/// Poles-dropdown label list. Order matters: `value.round() as usize`
/// indexes it (0 → "2", 1 → "4", 2 → "6", 3 → "8" poles).
const SVF_POLES_LABELS: &[&str] = &["2", "4", "6", "8"];

/// Type-dropdown label list. Order matters: 0 → LP, 1 → BP, 2 → HP.
const SVF_TYPE_LABELS: &[&str] = &["Lowpass", "Bandpass", "Highpass"];

const SVF_TYPE_LP: usize = 0;
const SVF_TYPE_BP: usize = 1;
// Highpass uses the `_` arm in `svf_step` — `set_param` clamps the index
// to `0..=2`, so the only remaining case after LP and BP is HP.

impl SvfEffect {
    const MAX_STAGES: usize = 4;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Resonance",
            min: 0.0,
            max: 1.0,
            default: 0.1,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
        ParamSpec {
            name: "Type",
            min: 0.0,
            max: (SVF_TYPE_LABELS.len() - 1) as f32,
            // Index 0 → Lowpass.
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: SVF_TYPE_LABELS,
            },
        },
        ParamSpec {
            name: "Poles",
            min: 0.0,
            max: (SVF_POLES_LABELS.len() - 1) as f32,
            // Index 0 → 2 poles (12 dB/oct) — the original behaviour.
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: SVF_POLES_LABELS,
            },
        },
    ];

    /// An `SvfEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut svf = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            type_idx: Self::PARAMS[2].default,
            poles_idx: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            butter: (0.0, 0.0, 0.0, 0.0),
            res: (0.0, 0.0, 0.0, 0.0),
            stages_ic1: [[0.0; 2]; Self::MAX_STAGES],
            stages_ic2: [[0.0; 2]; Self::MAX_STAGES],
        };
        svf.recompute();
        svf
    }

    /// Number of cascaded SVF stages: index 0 → 1 stage (2 poles), …,
    /// index 3 → 4 stages (8 poles). Always at least 1.
    fn stages_count(&self) -> usize {
        (self.poles_idx.round() as usize + 1).min(Self::MAX_STAGES)
    }

    /// Build a `(a1, a2, a3, k)` TPT-SVF coefficient tuple for the given
    /// Q. Q < 0.5 critically damps; Q = 0.707 is Butterworth (flat,
    /// 3 dB at cutoff); higher Q peaks the response at cutoff. `k = 1/Q`
    /// is needed for the HP output tap (`v3 − k · v1`).
    fn svf_coefs(g: f32, q: f32) -> (f32, f32, f32, f32) {
        let k = 1.0 / q.max(0.0001);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        (a1, a2, a3, k)
    }

    /// Recompute both coefficient sets from cutoff / resonance / SR.
    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let q_res = 0.5 + self.resonance.clamp(0.0, 1.0) * 9.5;
        let q_butter = std::f32::consts::FRAC_1_SQRT_2;
        self.butter = Self::svf_coefs(g, q_butter);
        self.res = Self::svf_coefs(g, q_res);
    }

    /// One TPT-SVF integrator step for one (stage, channel). Returns the
    /// output of the tap chosen by `type_idx`:
    ///
    /// * `0` (LP): `v2` — lowpass output.
    /// * `1` (BP): `v1` — bandpass output.
    /// * `2` (HP): `v3 − k · v1` — highpass output.
    ///
    /// `coefs` is the precomputed `(a1, a2, a3, k)` tuple — picks
    /// Butterworth or resonance per stage.
    #[inline]
    fn svf_step(
        &mut self,
        x: f32,
        stage: usize,
        ch: usize,
        coefs: (f32, f32, f32, f32),
        type_idx: usize,
    ) -> f32 {
        let (a1, a2, a3, k) = coefs;
        let ic1 = self.stages_ic1[stage][ch];
        let ic2 = self.stages_ic2[stage][ch];
        let v3 = x - ic2;
        let v1 = a1 * ic1 + a2 * v3;
        let v2 = ic2 + a2 * ic1 + a3 * v3;
        self.stages_ic1[stage][ch] = 2.0 * v1 - ic1;
        self.stages_ic2[stage][ch] = 2.0 * v2 - ic2;
        match type_idx {
            SVF_TYPE_LP => v2,
            SVF_TYPE_BP => v1,
            _ => v3 - k * v1, // HP
        }
    }
}

impl Default for SvfEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for SvfEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let stages = self.stages_count();
        let type_idx = self.type_idx.round() as usize;
        let mut l = left;
        let mut r = right;
        for stage in 0..stages {
            // Last stage carries the resonance peak; earlier stages run
            // Butterworth so the peak doesn't compound across the cascade.
            let coefs = if stage + 1 == stages {
                self.res
            } else {
                self.butter
            };
            l = self.svf_step(l, stage, 0, coefs, type_idx);
            r = self.svf_step(r, stage, 1, coefs, type_idx);
        }
        (l, r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.stages_ic1 = [[0.0; 2]; Self::MAX_STAGES];
        self.stages_ic2 = [[0.0; 2]; Self::MAX_STAGES];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            // Type: round to the nearest enum index. Selects which SVF
            // output tap (LP / BP / HP) each stage emits.
            2 => {
                let max_idx = (SVF_TYPE_LABELS.len() - 1) as f32;
                self.type_idx = value.round().clamp(0.0, max_idx);
                return;
            }
            // Poles: round to the nearest enum index. Doesn't change
            // coefficients — only the cascade depth.
            3 => {
                let max_idx = (SVF_POLES_LABELS.len() - 1) as f32;
                self.poles_idx = value.round().clamp(0.0, max_idx);
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
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn svf_effect_parameters_are_declared() {
        let svf = SvfEffect::new();
        let specs = svf.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Type");
        assert_eq!(specs[3].name, "Poles");
        assert!(specs[0].min < specs[0].max);
    }

    #[test]
    fn lowpass_effect_dark_cutoff_attenuates_highs() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 200.0);
        lp.set_param(1, 0.0);
        let mut peak = 0.0_f32;
        for i in 0..2048 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = lp.process_sample(x, x);
            if i > 256 {
                peak = peak.max(l.abs());
            }
        }
        assert!(
            peak < 0.5,
            "a 200 Hz lowpass should kill a fast alternation, got {peak}"
        );
    }

    #[test]
    fn lowpass_effect_open_cutoff_passes_a_constant() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 18_000.0);
        lp.set_param(1, 0.0);
        let mut y = 0.0;
        for _ in 0..2048 {
            y = lp.process_sample(1.0, 1.0).0;
        }
        assert!(y > 0.9, "an open lowpass should pass a constant, got {y}");
    }

    #[test]
    fn lowpass_effect_reset_clears_state() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 300.0);
        for _ in 0..512 {
            lp.process_sample(1.0, 1.0);
        }
        lp.reset();
        let y = lp.process_sample(1.0, 1.0).0;
        assert!(y.abs() < 0.5, "reset should clear filter state, got {y}");
    }

    #[test]
    fn lowpass_effect_set_param_out_of_range_is_ignored() {
        let mut lp = SvfEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(99, 1.0);
        let y = lp.process_sample(0.25, 0.25);
        assert!(y.0.is_finite());
    }

    #[test]
    fn svf_param_formats_match_spec() {
        let specs = SvfEffect::new().parameters();
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(specs[1].format, ParamFormat::Number { .. }));
        // Type at slot 2: Enum-format dropdown over LP / BP / HP.
        assert_eq!(specs[2].name, "Type");
        if let ParamFormat::Enum { labels } = specs[2].format {
            assert_eq!(labels, &["Lowpass", "Bandpass", "Highpass"]);
        } else {
            panic!("Type spec should use ParamFormat::Enum");
        }
        // Poles at slot 3: Enum-format dropdown over the four cascade
        // lengths (2 / 4 / 6 / 8 poles).
        assert_eq!(specs[3].name, "Poles");
        if let ParamFormat::Enum { labels } = specs[3].format {
            assert_eq!(labels, &["2", "4", "6", "8"]);
        } else {
            panic!("Poles spec should use ParamFormat::Enum");
        }
    }

    #[test]
    fn svf_type_changes_which_band_passes() {
        // LP attenuates ABOVE cutoff; HP attenuates BELOW; BP attenuates
        // BOTH sides. Drive each Type at a 1 kHz cutoff with a sine at
        // 250 Hz (well below cutoff) and at 4 kHz (well above), and
        // check the RMS ratios match each filter's identity.
        let measure = |type_idx: f32, freq_hz: f32| -> f32 {
            let mut svf = SvfEffect::new();
            svf.set_sample_rate(48_000.0);
            svf.set_param(0, 1_000.0); // Cutoff = 1 kHz
            svf.set_param(1, 0.0); // No resonance
            svf.set_param(2, type_idx); // Type
            svf.set_param(3, 0.0); // 2 poles
            for i in 0..2048 {
                let s = (std::f32::consts::TAU * freq_hz * i as f32 / 48_000.0).sin();
                svf.process_sample(s, s);
            }
            let mut sum_sq = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let s = (std::f32::consts::TAU * freq_hz * i as f32 / 48_000.0).sin();
                let (l, _r) = svf.process_sample(s, s);
                sum_sq += l * l;
            }
            (sum_sq / 4096.0).sqrt()
        };
        // LP: 250 Hz passes (≈ unity input RMS ≈ 0.707), 4 kHz attenuated.
        let lp_low = measure(0.0, 250.0);
        let lp_high = measure(0.0, 4_000.0);
        assert!(
            lp_low > 0.5 && lp_high < 0.2,
            "LP should pass 250 Hz and attenuate 4 kHz (low={lp_low}, high={lp_high})"
        );
        // HP: opposite — low attenuated, high passes.
        let hp_low = measure(2.0, 250.0);
        let hp_high = measure(2.0, 4_000.0);
        assert!(
            hp_low < 0.2 && hp_high > 0.5,
            "HP should attenuate 250 Hz and pass 4 kHz (low={hp_low}, high={hp_high})"
        );
        // BP: both bands attenuated relative to LP-passing or HP-passing
        // levels; the 1 kHz cutoff itself is the peak (we don't measure
        // it here, just confirm the off-band attenuation).
        let bp_low = measure(1.0, 250.0);
        let bp_high = measure(1.0, 4_000.0);
        assert!(
            bp_low < lp_low && bp_high < hp_high,
            "BP should attenuate both bands relative to their passing types \
             (bp_low={bp_low} vs lp_low={lp_low}, bp_high={bp_high} vs hp_high={hp_high})"
        );
    }

    #[test]
    fn lowpass_higher_pole_count_attenuates_above_cutoff_more() {
        // Decade-above-cutoff response should grow steeper with more
        // stages: each additional 2-pole stage adds 12 dB/oct of rolloff,
        // so the RMS at 10× the cutoff is monotonically smaller as the
        // pole count rises 2 → 8.
        let measure_rms_decade_above = |poles_idx: f32| -> f32 {
            let mut lp = SvfEffect::new();
            lp.set_sample_rate(48_000.0);
            lp.set_param(0, 1_000.0); // Cutoff = 1 kHz
            lp.set_param(1, 0.0); // Resonance = 0 (no peaking)
            lp.set_param(3, poles_idx); // Poles index (slot 3)
            let sr = 48_000.0_f32;
            let f_test = 10_000.0_f32; // one decade above cutoff
                                       // Warm up the cascade, then measure 4096 samples of the
                                       // single-channel RMS.
            for i in 0..2048 {
                let s = (std::f32::consts::TAU * f_test * i as f32 / sr).sin();
                lp.process_sample(s, s);
            }
            let mut sum_sq = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let s = (std::f32::consts::TAU * f_test * i as f32 / sr).sin();
                let (l, _r) = lp.process_sample(s, s);
                sum_sq += l * l;
            }
            (sum_sq / 4096.0).sqrt()
        };
        let rms_2 = measure_rms_decade_above(0.0); // 2 poles
        let rms_4 = measure_rms_decade_above(1.0); // 4 poles
        let rms_6 = measure_rms_decade_above(2.0); // 6 poles
        let rms_8 = measure_rms_decade_above(3.0); // 8 poles
                                                   // Strict ordering: each step adds at least some attenuation. (The
                                                   // exact ratio is 1 / 4^N for N additional 2-pole stages — but
                                                   // even with shared coefficients we expect a clear monotone
                                                   // ordering on a steady sine well above cutoff.)
        assert!(
            rms_2 > rms_4 && rms_4 > rms_6 && rms_6 > rms_8,
            "rolloff at 10× cutoff should strictly steepen with pole count \
             (2p={rms_2}, 4p={rms_4}, 6p={rms_6}, 8p={rms_8})"
        );
        // Sanity: 8 poles should be substantially quieter than 2 poles.
        assert!(
            rms_8 < rms_2 * 0.25,
            "8-pole at 10× cutoff should be much quieter than 2-pole \
             (2p={rms_2}, 8p={rms_8})"
        );
    }
}
