use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Frequency-modulation effect. The input plays one of two roles, chosen by
/// the `Mode` param:
///
/// * **Carrier** — the input is treated as the audio being FMed. An internal
///   sine at the `Freq` rate modulates a delay line's length around a fixed
///   5 ms centre; the output is the input read out at that varying delay
///   (vibrato at slow `Freq`, sideband-rich FM at audio-rate `Freq`). The
///   delay-line approach is the only practical way to "FM" an arbitrary
///   input without an analytic-signal Hilbert transform, since instantaneous
///   frequency is the derivative of phase.
///
/// * **Modulator** — the input modulates an internal sine carrier. The
///   carrier phase advances at `Freq + depth · input_sample` per sample
///   (through-zero phase modulation). Each channel runs its own carrier so
///   stereo input produces stereo output.
///
/// `Feedback` is DX7-style operator self-modulation in both modes: the
/// previous output sample mixes back into the rotation phase, enriching
/// the timbre (sine → sawtooth-ish at high settings).
///
/// **Modulator-mode input gating**: the internal carrier sine plays only
/// when there's input to modulate. An envelope follower tracks the input
/// level (fast attack, slow release) and scales the carrier's amplitude,
/// so a silent input — including the host having stopped the transport —
/// yields a silent output instead of a continuously-ringing bare carrier.
/// Carrier mode is intrinsically input-driven (the analytic signal of
/// silence is silent), so no gate is needed there.
///
/// **Unified architecture**: both modes go through the same PM/FM rotation
/// math; only the role assignment differs:
///
/// * **Modulator**: the carrier is an internal sine whose phase is rotated
///   by the input (modulator) plus self-feedback.
/// * **Carrier**: the carrier is the **input audio**, converted to an
///   analytic signal via a Hilbert FIR (so its phase is well-defined),
///   then rotated by the internal sine modulator plus self-feedback. The
///   Hilbert filter adds ≈ 32 samples (~0.7 ms at 48 kHz) of latency only
///   while this mode is selected.
pub struct FmEffect {
    // Stored parameters.
    mode: f32, // 0 = Carrier, 1 = Modulator (rounded on set_param).
    freq_hz: f32,
    depth_pct: f32,    // 0..100, divided by 100 inside `process_sample`.
    feedback_pct: f32, // 0..100, divided by 100 inside `process_sample`.
    /// 0 = PM (phase offset at output), 1 = true FM (added to increment).
    topology: f32,
    sample_rate: f32,

    // Internal oscillator phases (0..1).
    carrier_phase_l: f32,
    carrier_phase_r: f32,
    mod_phase: f32,

    /// FM-topology theta accumulator for Carrier mode (in cycles, wraps
    /// modulo 1 every sample). PM mode doesn't use this.
    fm_theta_accum: f32,

    // One-sample feedback memory.
    prev_out_l: f32,
    prev_out_r: f32,

    // Modulator-mode input gate: a one-pole envelope follower over
    // `|left| + |right|`, used as the carrier amplitude. Coefficients
    // are cached from `set_sample_rate`.
    input_env: f32,
    env_attack_coef: f32,
    env_release_coef: f32,

    /// Carrier-mode analytic-signal extractors (Hilbert FIR + delay-matched
    /// real branch). Each channel runs its own — together they convert the
    /// raw input into a `(real, imag)` pair that the rotation math operates
    /// on. Allocated once in `new`; allocation-free thereafter.
    analytic_l: tract_dsp::hilbert::AnalyticSignal,
    analytic_r: tract_dsp::hilbert::AnalyticSignal,
}

/// Mode-dial label list. Order matters: `value.round() as usize` indexes it.
const FM_MODE_LABELS: &[&str] = &["Carrier", "Modulator"];

/// Topology-dial label list (Modulator-mode operator topology). PM uses the
/// previous output as a phase OFFSET at output time (no integration → no
/// drift, sounds like a DX7 operator). True FM adds it to the phase
/// INCREMENT — input still bends the carrier's pitch (which PM only does at
/// audio rates), but self-feedback integrates and can wander at high
/// feedback settings.
const FM_TOPOLOGY_LABELS: &[&str] = &["PM", "FM"];

impl FmEffect {
    /// Hilbert FIR length for the Carrier-mode analytic-signal extractor.
    /// 65 gives ~32 samples (~0.7 ms at 48 kHz) of group delay and a clean
    /// passband above ~1 kHz.
    const HILBERT_LEN: usize = 65;
    /// Modulator-mode input-gate time constants. Fast attack catches
    /// transients without clipping the carrier's onset; slow release lets
    /// the carrier ring out smoothly across short silences.
    const ENV_ATTACK_MS: f32 = 1.0;
    const ENV_RELEASE_MS: f32 = 100.0;

    // Order matters: `targets[0]` (the assignable-MSEG-1 default) is `Some(0)`,
    // so the first param is what fresh tracks modulate. Freq is the natural
    // first audible-modulation target; Mode and Topology are Enum-format
    // selectors the editor renders as dropdowns rather than dials.
    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Freq",
            min: 20.0,
            max: 20_000.0,
            default: 100.0,
            // Log-scaled dial across exactly the audio band — three even
            // decades from 20 Hz to 20 kHz, so each decade takes one-third
            // of the arc. Sub-audio vibrato is reachable by modulating
            // Freq via an MSEG rather than dialing it in directly.
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Depth",
            min: 0.0,
            max: 100.0,
            default: 25.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Mode",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: FM_MODE_LABELS,
            },
        },
        ParamSpec {
            name: "Topology",
            min: 0.0,
            max: 1.0,
            default: 0.0, // PM by default — drift-free; DX7-style.
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: FM_TOPOLOGY_LABELS,
            },
        },
    ];

    /// An `FmEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut fm = Self {
            freq_hz: Self::PARAMS[0].default,
            depth_pct: Self::PARAMS[1].default,
            feedback_pct: Self::PARAMS[2].default,
            mode: Self::PARAMS[3].default,
            topology: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            carrier_phase_l: 0.0,
            carrier_phase_r: 0.0,
            mod_phase: 0.0,
            fm_theta_accum: 0.0,
            prev_out_l: 0.0,
            prev_out_r: 0.0,
            input_env: 0.0,
            env_attack_coef: 0.0,
            env_release_coef: 0.0,
            analytic_l: tract_dsp::hilbert::AnalyticSignal::new(Self::HILBERT_LEN),
            analytic_r: tract_dsp::hilbert::AnalyticSignal::new(Self::HILBERT_LEN),
        };
        fm.recompute_env_coefs();
        fm
    }

    /// Re-derive the input-gate envelope coefficients from the cached
    /// `sample_rate` and `ENV_*_MS` time constants. Cheap (two `exp`); only
    /// called from `new` and `set_sample_rate`.
    fn recompute_env_coefs(&mut self) {
        let sr = self.sample_rate.max(1.0);
        self.env_attack_coef = (-1.0 / (Self::ENV_ATTACK_MS * 0.001 * sr)).exp();
        self.env_release_coef = (-1.0 / (Self::ENV_RELEASE_MS * 0.001 * sr)).exp();
    }
}

impl Default for FmEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for FmEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // ±π rad at feedback = 1 — DX7-style operator self-modulation cap.
        const FB_PHASE_SCALE: f32 = 0.5;
        let two_pi = std::f32::consts::TAU;
        let sr = self.sample_rate.max(1.0);
        let phase_inc = self.freq_hz / sr;
        let depth = self.depth_pct * 0.01;
        let feedback = self.feedback_pct * 0.01;

        if self.mode < 0.5 {
            // Carrier mode: the INPUT plays the role of the carrier. Convert
            // it to its analytic signal `(real, imag)` via a Hilbert FIR so
            // we can phase-rotate it the same way Modulator mode rotates its
            // internal sine. The internal sine LFO acts as the modulator.
            //
            // PM: θ(t) = depth · sin(mod_phase) — instantaneous phase
            //     offset; the input's spectrum is rotated by ±depth cycles.
            // FM: θ(t) = ∫ depth · sin(mod_phase) dτ — accumulated; the
            //     input's instantaneous frequency is shifted by
            //     depth · sin(mod_phase) · sr Hz.
            //
            // Feedback adds the previous output back into θ (DX7-style
            // self-modulation), enriching the timbre.
            //
            // No input gate needed — the analytic signal of silence is
            // silence, so a silent input naturally yields a silent output.
            let mod_sine = (self.mod_phase * two_pi).sin();
            self.mod_phase = (self.mod_phase + phase_inc).rem_euclid(1.0);
            let theta_mod = if self.topology < 0.5 {
                // PM: instantaneous rotation = depth · modulator.
                depth * mod_sine
            } else {
                // FM: accumulate depth · modulator into the rotation phase.
                self.fm_theta_accum = (self.fm_theta_accum + depth * mod_sine).rem_euclid(1.0);
                self.fm_theta_accum
            };
            let theta_l = theta_mod + feedback * FB_PHASE_SCALE * self.prev_out_l;
            let theta_r = theta_mod + feedback * FB_PHASE_SCALE * self.prev_out_r;
            let (real_l, imag_l) = self.analytic_l.process(left);
            let (real_r, imag_r) = self.analytic_r.process(right);
            let (cos_l, sin_l) = {
                let a = theta_l * two_pi;
                (a.cos(), a.sin())
            };
            let (cos_r, sin_r) = {
                let a = theta_r * two_pi;
                (a.cos(), a.sin())
            };
            let out_l = real_l * cos_l - imag_l * sin_l;
            let out_r = real_r * cos_r - imag_r * sin_r;
            self.prev_out_l = out_l;
            self.prev_out_r = out_r;
            (out_l, out_r)
        } else {
            // Modulator mode: the internal sine is the carrier; the input
            // is the modulator. Topology picks PM vs FM; the input-gate
            // envelope follower scales the output so silent input → silent
            // output (avoids the bare carrier ringing when the transport
            // is stopped).
            let target_env = (left.abs() + right.abs()) * 0.5;
            let env_coef = if target_env > self.input_env {
                self.env_attack_coef
            } else {
                self.env_release_coef
            };
            self.input_env = target_env + (self.input_env - target_env) * env_coef;
            let gate = self.input_env.min(1.0);

            let (sin_l, sin_r) = if self.topology < 0.5 {
                // PM: input + feedback applied as a phase OFFSET at output.
                self.carrier_phase_l = (self.carrier_phase_l + phase_inc).rem_euclid(1.0);
                self.carrier_phase_r = (self.carrier_phase_r + phase_inc).rem_euclid(1.0);
                let pm_l = depth * left + feedback * FB_PHASE_SCALE * self.prev_out_l;
                let pm_r = depth * right + feedback * FB_PHASE_SCALE * self.prev_out_r;
                (
                    ((self.carrier_phase_l + pm_l) * two_pi).sin(),
                    ((self.carrier_phase_r + pm_r) * two_pi).sin(),
                )
            } else {
                // FM: input + feedback applied as a phase INCREMENT —
                // the carrier's instantaneous frequency tracks the
                // modulator in cycles/sample.
                let inc_l = phase_inc + depth * left + feedback * FB_PHASE_SCALE * self.prev_out_l;
                let inc_r = phase_inc + depth * right + feedback * FB_PHASE_SCALE * self.prev_out_r;
                self.carrier_phase_l = (self.carrier_phase_l + inc_l).rem_euclid(1.0);
                self.carrier_phase_r = (self.carrier_phase_r + inc_r).rem_euclid(1.0);
                (
                    (self.carrier_phase_l * two_pi).sin(),
                    (self.carrier_phase_r * two_pi).sin(),
                )
            };
            let out_l = gate * sin_l;
            let out_r = gate * sin_r;
            self.prev_out_l = out_l;
            self.prev_out_r = out_r;
            (out_l, out_r)
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute_env_coefs();
    }

    fn reset(&mut self) {
        self.carrier_phase_l = 0.0;
        self.carrier_phase_r = 0.0;
        self.mod_phase = 0.0;
        self.fm_theta_accum = 0.0;
        self.prev_out_l = 0.0;
        self.prev_out_r = 0.0;
        self.input_env = 0.0;
        self.analytic_l.reset();
        self.analytic_r.reset();
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.freq_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.depth_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.feedback_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            // Mode: round to the nearest enum index (0 = Carrier, 1 = Modulator).
            3 => {
                self.mode = if value >= 0.5 { 1.0 } else { 0.0 };
            }
            // Topology: round to the nearest enum index (0 = PM, 1 = FM).
            4 => {
                self.topology = if value >= 0.5 { 1.0 } else { 0.0 };
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    #[test]
    fn fm_effect_lists_five_parameters_with_the_expected_specs() {
        let fm = FmEffect::new();
        let specs = fm.parameters();
        assert_eq!(specs.len(), 5);
        // Freq is param 0 so the default `targets[0] = Some(0)` modulation
        // assignment naturally points at the most useful audible parameter.
        assert_eq!(specs[0].name, "Freq");
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert_eq!(specs[1].name, "Depth");
        assert_eq!(specs[2].name, "Feedback");
        // Mode and Topology are Enum-format — the editor renders dropdowns
        // for both, not dials.
        assert_eq!(specs[3].name, "Mode");
        assert!(matches!(specs[3].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[4].name, "Topology");
        assert!(matches!(specs[4].format, ParamFormat::Enum { .. }));
    }

    #[test]
    fn fm_mode_set_param_rounds_to_zero_or_one() {
        // Mode is at param index 3. Any value < 0.5 collapses to Carrier (0);
        // ≥ 0.5 to Modulator (1). With Mode = Modulator and Depth = 0, the
        // carrier sine is gated by the input envelope — so a constant unity
        // input plays the bare carrier at full amplitude.
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 0.51); // Mode → Modulator
        fm.set_param(0, 200.0); // Freq
        fm.set_param(1, 0.0); // Depth
        fm.set_param(2, 0.0); // Feedback
                              // Warm up the input-gate envelope follower (~5 attack TCs).
        for _ in 0..256 {
            fm.process_sample(1.0, 1.0);
        }
        let mut max_abs = 0.0_f32;
        for _ in 0..1024 {
            let (l, r) = fm.process_sample(1.0, 1.0);
            max_abs = max_abs.max(l.abs().max(r.abs()));
        }
        assert!(
            max_abs > 0.5,
            "Modulator mode with unity input + depth=0 must produce its carrier sine"
        );

        // Below the half-way threshold rounds to Carrier — silent input
        // produces silence (delay line is full of zeros).
        let mut fm2 = FmEffect::new();
        fm2.set_sample_rate(48_000.0);
        fm2.set_param(3, 0.3); // Mode → Carrier
        for _ in 0..1024 {
            let (l, _r) = fm2.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
        }
    }

    #[test]
    fn fm_carrier_mode_with_depth_zero_passes_the_input_through_unchanged() {
        // Carrier mode at depth = 0 means the rotation angle θ stays at 0,
        // so the analytic signal is rotated by 0 cycles — i.e. the output
        // equals the (delay-matched real branch of the) input. After the
        // Hilbert FIR's warm-up, constant 0.5 input gives constant 0.5
        // output on both channels.
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 0.0); // Mode → Carrier
        fm.set_param(0, 5.0); // Freq
        fm.set_param(1, 0.0); // Depth = 0 — no modulation, identity rotation
        fm.set_param(2, 0.0); // Feedback
        let mut last = (0.0_f32, 0.0_f32);
        for _ in 0..1024 {
            last = fm.process_sample(0.5, 0.5);
        }
        assert!(
            (last.0 - 0.5).abs() < 1e-3,
            "after warm-up, output L should match input ({:?})",
            last
        );
        assert!((last.1 - 0.5).abs() < 1e-3);
    }

    #[test]
    fn fm_modulator_mode_feedback_stays_audibly_active_across_the_range() {
        // Modulator mode is now an input-gated PM operator. Driving with a
        // constant unity input warms the gate envelope to ~1.0, so the
        // carrier plays at full amplitude and feedback's timbral change
        // is observable. Three feedback settings must (a) preserve the
        // carrier at fb = 0, (b) stay bounded and audible at fb = 100 %,
        // and (c) measurably change the crest factor at intermediate
        // settings.
        let measure_at_fb = |fb_pct: f32| -> (f32, f32) {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 1.0); // Mode → Modulator
            fm.set_param(0, 200.0); // Freq
            fm.set_param(1, 0.0); // Depth = 0
            fm.set_param(2, fb_pct);
            // Warm up the input-gate envelope follower with constant input.
            for _ in 0..2048 {
                fm.process_sample(1.0, 1.0);
            }
            let mut sum_sq = 0.0_f32;
            let mut peak = 0.0_f32;
            for _ in 0..2048 {
                let (l, _r) = fm.process_sample(1.0, 1.0);
                sum_sq += l * l;
                peak = peak.max(l.abs());
            }
            ((sum_sq / 2048.0).sqrt(), peak)
        };
        let (rms_0, peak_0) = measure_at_fb(0.0);
        let (rms_50, peak_50) = measure_at_fb(50.0);
        let (rms_100, peak_100) = measure_at_fb(100.0);
        // fb=0 with the gate fully open: a 200 Hz sine — RMS ≈ 1/√2 ≈ 0.707,
        // peak ≈ 1.
        assert!(
            (rms_0 - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05,
            "fb=0 RMS should be ~0.707, got {rms_0}"
        );
        assert!(
            (peak_0 - 1.0).abs() < 0.05,
            "fb=0 peak should be ~1.0, got {peak_0}"
        );
        // fb=100%: still audibly present, still bounded.
        assert!(
            rms_100 > 0.1,
            "fb=100% should still produce audible output (RMS > 0.1), got {rms_100}"
        );
        assert!(
            peak_100 < 1.5,
            "fb=100% output should be bounded (peak < 1.5), got {peak_100}"
        );
        // Self-feedback enriches the carrier: the waveform drifts away from
        // a pure sine, so the crest factor (peak / RMS) changes measurably
        // between fb=0 and fb=50%.
        let crest_0 = peak_0 / rms_0;
        let crest_50 = peak_50 / rms_50;
        assert!(
            (crest_50 - crest_0).abs() > 0.05,
            "feedback should change the carrier's timbre \
             (crest@0={crest_0}, crest@50={crest_50})"
        );
    }

    #[test]
    fn fm_modulator_mode_silent_input_yields_silent_output() {
        // The input-gate keeps the carrier asleep until there's input. A
        // pristine silent input must yield exact-zero output forever — this
        // is the fix for "Modulator mode keeps playing while the transport
        // is stopped".
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 1.0); // Mode → Modulator
        fm.set_param(0, 100.0); // Freq
        fm.set_param(1, 0.0); // Depth
        fm.set_param(2, 0.0); // Feedback
        for _ in 0..4096 {
            let (l, r) = fm.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn fm_modulator_mode_with_constant_input_plays_a_pure_sine_at_the_carrier_freq() {
        // Drive with constant unity input so the input-gate envelope settles
        // to ~1.0. With depth=0 and fb=0 the output is then a clean 100 Hz
        // sine at the carrier frequency.
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 1.0); // Mode → Modulator
        fm.set_param(0, 100.0); // Freq
        fm.set_param(1, 0.0); // Depth
        fm.set_param(2, 0.0); // Feedback
                              // Settle the input-gate envelope follower.
        for _ in 0..2048 {
            fm.process_sample(1.0, 1.0);
        }
        // Measure period by finding zero-crossings.
        let mut zero_crossings = 0;
        let mut prev = 0.0_f32;
        for _ in 0..(48_000 / 10) {
            let (l, _r) = fm.process_sample(1.0, 1.0);
            if prev <= 0.0 && l > 0.0 {
                zero_crossings += 1;
            }
            prev = l;
        }
        // 0.1 s of a 100 Hz sine has exactly 10 positive-going zero crossings.
        assert!(
            (8..=12).contains(&zero_crossings),
            "expected ~10 positive zero crossings of a 100 Hz sine in 100 ms, got {zero_crossings}"
        );
    }

    #[test]
    fn fm_reset_clears_state_and_returns_silence() {
        let mut fm = FmEffect::new();
        fm.set_sample_rate(48_000.0);
        fm.set_param(3, 1.0); // Mode → Modulator (visible non-zero output)
        fm.set_param(0, 200.0); // Freq
                                // Drive it for a while to fill delay lines and advance phases.
        for _ in 0..1024 {
            fm.process_sample(0.4, 0.4);
        }
        fm.reset();
        // Switch to Carrier mode. Reset zeroed the delay line, so a silent
        // input produces exactly silence.
        fm.set_param(3, 0.0);
        let (l, r) = fm.process_sample(0.0, 0.0);
        assert_eq!(
            l, 0.0,
            "after reset Carrier mode on silent input is silence"
        );
        assert_eq!(r, 0.0);
    }

    #[test]
    fn fm_carrier_mode_feedback_changes_timbre_audibly() {
        // Carrier mode now routes the input through an analytic-signal
        // rotation, with feedback adding the previous output back into the
        // rotation phase (DX7-style operator self-modulation). Different
        // feedback settings should produce audibly different output
        // sequences on the same input. The output stays bounded at the
        // upper end — no runaway.
        let render = |fb_pct: f32, topology: f32| -> (Vec<f32>, f32) {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 0.0); // Carrier
            fm.set_param(4, topology); // Topology
            fm.set_param(0, 5.0); // Freq (LFO rate)
            fm.set_param(1, 50.0); // Depth = 50%
            fm.set_param(2, fb_pct);
            let two_pi = std::f32::consts::TAU;
            let input = |i: usize| (two_pi * 1000.0 * i as f32 / 48_000.0).sin() * 0.5;
            // Warm up the Hilbert FIR plus the LFO.
            for i in 0..1024 {
                fm.process_sample(input(i), input(i));
            }
            let mut out = Vec::with_capacity(2048);
            let mut peak = 0.0_f32;
            for i in 1024..(1024 + 2048) {
                let (l, _r) = fm.process_sample(input(i), input(i));
                out.push(l);
                peak = peak.max(l.abs());
            }
            (out, peak)
        };
        // PM and FM topologies both — feedback should change the output.
        for &topology in &[0.0_f32, 1.0_f32] {
            let (out_0, peak_0) = render(0.0, topology);
            let (out_50, peak_50) = render(50.0, topology);
            let (_out_90, peak_90) = render(90.0, topology);
            let mean_abs_diff = out_0
                .iter()
                .zip(&out_50)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / out_0.len() as f32;
            assert!(
                mean_abs_diff > 0.01,
                "topology {topology}: feedback must change the output (mean diff {mean_abs_diff})"
            );
            // Bounded: no setting drives the output above a safety ceiling.
            assert!(
                peak_0 < 2.0 && peak_50 < 2.0 && peak_90 < 2.0,
                "topology {topology}: outputs must stay bounded \
                 (peaks {peak_0}, {peak_50}, {peak_90})"
            );
        }
    }

    #[test]
    fn fm_modulator_topology_changes_spectral_content_with_audio_rate_modulator() {
        // For the same depth knob value, FM's effective modulation index at
        // modulator frequency `f_m` is `depth · sr / (2π · f_m)` while PM's
        // is `depth · 2π`. At depth = 0.5 with `f_m` = 200 Hz / sr = 48 kHz,
        // β_FM ≈ 19 and β_PM ≈ 3.14 — FM has ~6× the modulation index and
        // its output is dramatically richer in upper harmonics. The
        // sum-of-absolute-differences between consecutive samples is a
        // crude but effective proxy for that high-frequency content.
        let measure = |topology: f32| -> f32 {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 1.0); // Mode → Modulator
            fm.set_param(4, topology); // Topology
            fm.set_param(0, 1000.0); // Freq: 1 kHz carrier
            fm.set_param(1, 50.0); // Depth = 50 %
            fm.set_param(2, 0.0); // Feedback = 0
            let two_pi = std::f32::consts::TAU;
            let m = |i: usize| (two_pi * 200.0 * i as f32 / 48_000.0).sin() * 0.5;
            // Warm up the input-gate envelope follower.
            for i in 0..2048 {
                fm.process_sample(m(i), m(i));
            }
            let mut prev = 0.0_f32;
            let mut sum_abs_diff = 0.0_f32;
            for i in 2048..(2048 + 4096) {
                let (l, _r) = fm.process_sample(m(i), m(i));
                sum_abs_diff += (l - prev).abs();
                prev = l;
            }
            sum_abs_diff
        };
        let pm_swing = measure(0.0);
        let fm_swing = measure(1.0);
        // FM with ~6× the modulation index should have substantially more
        // high-frequency content than PM at the same depth.
        assert!(
            fm_swing > pm_swing * 1.5,
            "FM topology should produce more spectral content than PM at \
             the same depth knob (PM swing = {pm_swing}, FM swing = {fm_swing})"
        );
    }

    #[test]
    fn fm_modulator_mode_with_topology_fm_lets_input_bend_carrier_pitch() {
        // True FM (Topology = 1) adds `depth · input` to the phase
        // INCREMENT, so a constant positive input bias permanently raises
        // the carrier's instantaneous pitch. The same setup under PM
        // (Topology = 0) keeps the base pitch fixed and just adds a
        // constant phase offset. Detect the difference by counting
        // zero-crossings over a fixed window with a positive DC bias.
        let count_pos_zcs = |topology: f32| -> i32 {
            let mut fm = FmEffect::new();
            fm.set_sample_rate(48_000.0);
            fm.set_param(3, 1.0); // Modulator
            fm.set_param(4, topology); // Topology
            fm.set_param(0, 100.0); // Freq
            fm.set_param(1, 50.0); // Depth = 50 % so the FM contribution is sizeable
            fm.set_param(2, 0.0); // Feedback = 0
            for _ in 0..2048 {
                // Warm up the input-gate envelope follower.
                fm.process_sample(0.5, 0.5);
            }
            let mut zcs = 0;
            let mut prev = 0.0_f32;
            for _ in 0..48_000 / 10 {
                // 0.1 s window.
                let (l, _r) = fm.process_sample(0.5, 0.5);
                if prev <= 0.0 && l > 0.0 {
                    zcs += 1;
                }
                prev = l;
            }
            zcs
        };
        let pm_zcs = count_pos_zcs(0.0);
        let fm_zcs = count_pos_zcs(1.0);
        // PM with DC bias only shifts the phase by a constant — the carrier
        // still runs at exactly 100 Hz, giving ≈ 10 positive zero-crossings.
        assert!(
            (8..=12).contains(&pm_zcs),
            "PM with a constant input bias should run at the carrier rate (~10 ZCs), got {pm_zcs}"
        );
        // Under true FM, the +0.5 DC bias adds `depth · 0.5 = 0.25` cycles/
        // sample to the phase increment, so the instantaneous frequency
        // jumps by sr · 0.25 — orders of magnitude above 100 Hz. The
        // crossings count is dramatically higher.
        assert!(
            fm_zcs > pm_zcs * 5,
            "FM topology should bend the carrier pitch noticeably above PM \
             (PM={pm_zcs}, FM={fm_zcs})"
        );
    }
}
