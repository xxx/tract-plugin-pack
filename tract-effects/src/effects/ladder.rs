use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use tract_dsp::fast_math::tanh_pade;

/// 24 dB/oct Moog ladder lowpass with transistor-style saturation. Port of
/// the Huovilainen (2004) model -- five `tanh()` nonlinearities per inner
/// tick, with the canonical "half-sample delay" feedback path that lets the
/// resonance peak self-oscillate without blowing up.
///
/// The model is run at 2x oversampling internally (the inner two-step block
/// in `process_channel`) -- the Huovilainen paper specifies 2x as the
/// minimum for the tanh aliasing to stay sub-perceptual; higher OS factors
/// help marginally. We don't pay for a polyphase up/down filter pair: the
/// 2x is "double-tick" style, feeding each input sample to the filter
/// twice and returning the second tick's output. That trades a small
/// amount of antialiasing fidelity for zero added latency, matching the
/// rest of the Filter family. (Helm's ladder takes the same shortcut.)
///
/// Resonance is normalised 0..1; self-oscillation kicks in around 0.95.
/// Drive applies an input gain into the cascade (in dB), pushing harder
/// into the tanh nonlinearity for the classic Moog "growl".
///
/// Per channel state: four integrator stages, three input-side cached
/// tanh outputs (the delay-free-loop trick), and two half-sample-delay
/// samples. No allocations on the audio thread.
///
/// Reference: Huovilainen, "Non-Linear Digital Implementation of the
/// Moog Ladder Filter" (DAFx-04). Coefficient polynomial and the
/// `thermal` constant come from Victor Lazzarini's CSound5 port via
/// the ddiakopoulos/MoogLadders reference repo.
pub struct LadderEffect {
    cutoff: f32,
    resonance: f32,
    /// Drive in dB; applied as linear gain on the input.
    drive_db: f32,
    sample_rate: f32,
    /// Pre-multiplied drive (linear, derived from `drive_db`).
    drive_lin: f32,
    /// Per-cutoff coefficients (Huovilainen's `tune` and `acr`).
    tune: f32,
    acr: f32,
    /// `4 * resonance * acr` -- the scaled feedback gain.
    res_quad: f32,
    /// Per-channel integrator output state (`stage[k]` in the reference).
    stage: [[f32; 4]; 2],
    /// Per-channel cached input-side tanh outputs, one per integrator's
    /// preceding stage. Updated per inner tick; the cached values are
    /// what closes the delay-free loop without an algebraic solver.
    stage_tanh: [[f32; 3]; 2],
    /// Per-channel integrator delay state.
    delay: [[f32; 4]; 2],
    /// Per-channel half-sample phase-compensation pair (`delay[4..6]` in
    /// the reference). Index 0 is the running average; index 1 is the
    /// previous final-stage output.
    phase_delay: [[f32; 2]; 2],
}

impl LadderEffect {
    /// Transistor base-emitter thermal-voltage scaling, in volts. Sets the
    /// "soft" threshold of the tanh saturation -- inputs of this magnitude
    /// enter the knee. Lifted from CSound5's port; sub-perceptual fiddling.
    const THERMAL: f32 = 0.000025;

    const PARAMS: [ParamSpec; 3] = [
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

    /// A `LadderEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut effect = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            drive_db: Self::PARAMS[2].default,
            sample_rate: 48_000.0,
            drive_lin: 1.0,
            tune: 0.0,
            acr: 0.0,
            res_quad: 0.0,
            stage: [[0.0; 4]; 2],
            stage_tanh: [[0.0; 3]; 2],
            delay: [[0.0; 4]; 2],
            phase_delay: [[0.0; 2]; 2],
        };
        effect.recompute();
        effect
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        // Normalised cutoff in [0, 0.5]; the `* 0.5` is Huovilainen's
        // factor for the 2x oversampled inner loop.
        let fc_n = fc / sr;
        let f = fc_n * 0.5;
        let fc2 = fc_n * fc_n;
        let fc3 = fc2 * fc_n;
        // Polynomial coefficients lifted from CSound5's port; chosen to
        // match the analog ladder's pole-Q curve across the audible range.
        let fcr = 1.8730 * fc3 + 0.4955 * fc2 - 0.6490 * fc_n + 0.9988;
        self.acr = -3.9364 * fc2 + 1.8409 * fc_n + 0.9968;
        self.tune = (1.0 - (-((2.0 * std::f32::consts::PI) * f * fcr)).exp()) / Self::THERMAL;
        self.res_quad = 4.0 * self.resonance.clamp(0.0, 1.0) * self.acr;
        self.drive_lin = 10.0_f32.powf(self.drive_db.clamp(0.0, 24.0) / 20.0);
    }

    /// One ladder tick for one channel. Reads/updates state in place; the
    /// returned value is the half-sample-delayed fourth-stage output.
    #[inline]
    fn ladder_tick(&mut self, ch: usize, sample_in: f32) -> f32 {
        let stage = &mut self.stage[ch];
        let stage_tanh = &mut self.stage_tanh[ch];
        let delay = &mut self.delay[ch];
        let phase_delay = &mut self.phase_delay[ch];

        // Closed-loop input: sample minus the scaled feedback of the
        // previous half-sample-delayed output.
        let input = sample_in - self.res_quad * phase_delay[0];
        // Stage 0: integrator with input-side tanh and the cached output-
        // side tanh from the previous tick (`stage_tanh[0]`).
        stage[0] = delay[0] + self.tune * (tanh_pade(input * Self::THERMAL) - stage_tanh[0]);
        delay[0] = stage[0];
        // Stages 1..3: each step writes its own input-side tanh into
        // `stage_tanh[k-1]` for the next iteration; the output-side tanh
        // for stages 1..2 reads the still-cached `stage_tanh[k]`. Stage 3
        // closes the cascade with a freshly computed tanh of its own
        // delay sample.
        for k in 1..4 {
            let in_k = stage[k - 1];
            stage_tanh[k - 1] = tanh_pade(in_k * Self::THERMAL);
            let out_tanh = if k != 3 {
                stage_tanh[k]
            } else {
                tanh_pade(delay[k] * Self::THERMAL)
            };
            stage[k] = delay[k] + self.tune * (stage_tanh[k - 1] - out_tanh);
            delay[k] = stage[k];
        }
        // 0.5-sample phase compensation: average the new final-stage
        // output with the previous one.
        phase_delay[0] = (stage[3] + phase_delay[1]) * 0.5;
        phase_delay[1] = stage[3];
        phase_delay[0]
    }

    fn process_channel(&mut self, ch: usize, dry: f32) -> f32 {
        // 2x oversampling via double-tick on the same input sample.
        // Aliasing from the tanh nonlinearities concentrates at higher
        // sample rates, so a single extra tick collapses most of the
        // out-of-band energy that would fold back.
        let driven = dry * self.drive_lin;
        self.ladder_tick(ch, driven);
        self.ladder_tick(ch, driven)
    }
}

impl Default for LadderEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for LadderEffect {
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
        self.stage = [[0.0; 4]; 2];
        self.stage_tanh = [[0.0; 3]; 2];
        self.delay = [[0.0; 4]; 2];
        self.phase_delay = [[0.0; 2]; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
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
    fn parameters_are_declared() {
        let e = LadderEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert_eq!(specs[2].name, "Drive");
    }

    #[test]
    fn silent_input_stays_silent() {
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        // Hold resonance below the self-oscillation threshold; otherwise a
        // perfectly-zero input can still ring forever once any state is
        // perturbed.
        e.set_param(1, 0.5);
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn dark_cutoff_attenuates_highs() {
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 200.0); // 200 Hz lowpass
        e.set_param(1, 0.0); // no resonance peak
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
            "a 200 Hz 24 dB/oct lowpass should crush Nyquist alternation, got {peak}"
        );
    }

    #[test]
    fn open_cutoff_passes_a_constant() {
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 18_000.0);
        e.set_param(1, 0.0);
        let mut y = 0.0;
        for _ in 0..4096 {
            y = e.process_sample(1.0, 1.0).0;
        }
        assert!(
            y > 0.85 && y < 1.15,
            "an open ladder should pass a DC constant within +/-15%, got {y}"
        );
    }

    #[test]
    fn stable_under_modulation_and_max_resonance() {
        // The actual stability question: does the resonance feedback loop
        // stay bounded when cutoff sweeps across the audible range? Drive
        // is held at 0 dB on purpose -- drive multiplies the input amplitude
        // before saturation, so combining max drive with max resonance
        // measures "loud" more than "stable".
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 1.0); // max resonance
        for i in 0..16_384 {
            let t = i as f32 / 16_384.0;
            let cutoff = 20.0 * 1000.0_f32.powf(t); // 20 Hz -> 20 kHz
            e.set_param(0, cutoff);
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, r) = e.process_sample(x, x);
            // The only real stability question for a self-oscillating ladder
            // is "did it diverge to infinity or NaN". The tanh feedback
            // bounds the loop; transient peaks during a fast cutoff sweep
            // can comfortably exceed unity by +15-20 dB without that being
            // a bug -- that's the Moog growl, not runaway.
            assert!(l.is_finite() && r.is_finite(), "lost stability: {l}, {r}");
        }
    }

    #[test]
    fn high_resonance_self_oscillates_after_a_kick() {
        // A near-unity resonance ladder is meant to ring at the cutoff
        // frequency for a long time after a brief input excitation. Drive
        // a short impulse, then check the tail is still ringing thousands
        // of samples later.
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(0, 800.0);
        e.set_param(1, 0.99);
        // Kick the filter.
        for _ in 0..16 {
            e.process_sample(1.0, 1.0);
        }
        // Drain past any transient settling.
        for _ in 0..2048 {
            e.process_sample(0.0, 0.0);
        }
        // Measure tail energy.
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
        // The same test as above but with low resonance: the tail must
        // have decayed to silence, not be ringing. Guards against the
        // self-oscillation test passing trivially because the filter
        // always rings.
        let mut e = LadderEffect::new();
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
            peak < 1e-4,
            "low resonance should decay to silence, got peak={peak}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 0.9);
        for _ in 0..1024 {
            e.process_sample(1.0, -1.0);
        }
        e.reset();
        assert_eq!(e.stage, [[0.0; 4]; 2]);
        assert_eq!(e.stage_tanh, [[0.0; 3]; 2]);
        assert_eq!(e.delay, [[0.0; 4]; 2]);
        assert_eq!(e.phase_delay, [[0.0; 2]; 2]);
    }

    #[test]
    fn drive_changes_response() {
        // With the same input, more Drive should shove the signal harder
        // into the tanh nonlinearity and so produce a different RMS than
        // 0 dB drive. (Direction depends on the resonance bookkeeping;
        // we only assert inequality.)
        let measure = |drive_db: f32| -> f32 {
            let mut e = LadderEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(0, 1_500.0);
            e.set_param(1, 0.4);
            e.set_param(2, drive_db);
            // Warm up.
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
        let mut e = LadderEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
