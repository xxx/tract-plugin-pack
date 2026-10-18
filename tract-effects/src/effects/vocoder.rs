use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// 16-band channel vocoder. The input is the modulator (typically
/// vocal-like material -- speech, drum loops, melodic phrases);
/// an internally-generated carrier (Saw, Square, or Noise at a
/// user-set Pitch) is shaped by the modulator's per-band
/// amplitude envelope to produce the classic "robot voice"
/// effect.
///
/// **Algorithm.** Split both the modulator and the carrier into
/// 16 frequency bands via 2-pole TPT-SVF bandpass filters
/// (log-spaced from 100 Hz to 8 kHz, ~third-octave). For each
/// band, follow the modulator's amplitude envelope with a fast-
/// attack / slow-release peak detector. The output is the sum
/// over all bands of `carrier_bandpass[b] * env[b]` -- the
/// carrier "speaks" the modulator's spectral envelope.
///
/// **Carrier** picks the internal carrier waveform. **Saw** gives
/// the buzzy classic vocoder tone (rich harmonics across all
/// bands). **Square** is similar but with only odd harmonics
/// (hollow / clarinet-ish). **Noise** ignores Pitch entirely and
/// gives a "whisper voice" / sibilant character (the modulator's
/// envelope shapes broadband noise).
///
/// **Pitch** is the carrier oscillator's fundamental frequency
/// (50..2000 Hz log). For Saw / Square this directly sets the
/// voice's pitch; Noise ignores it.
///
/// **Q** is the bandpass selectivity (1..16). Q=1 gives broadly
/// overlapping bands -- smoother, less intelligible. Q=16 gives
/// sharply-tuned bands -- more "robotic" / formant-heavy, and
/// individual carrier harmonics become audibly separated.
///
/// **Smooth** sets the envelope-follower release time
/// (1..200 ms log). Short release tracks transients tightly
/// (intelligible consonants); long release smears speech into
/// pads. Attack is fixed at 5 ms -- fast enough to catch
/// consonants without exposing per-cycle ripple on the
/// rectified bandpass output.
///
/// **Latency:** zero (no FFT, no lookahead). **Per-sample work:**
/// ~80 SVF updates (16 mod x 2 channels + 16 shared carrier) +
/// 32 envelope steps + sum + carrier osc. Roughly Reverb-scale.
///
/// **Note on use.** Vocoders sound best when the modulator is
/// content with strong amplitude dynamics across the spectrum
/// (voice, percussive material, loops). On sustained pads or
/// drones the effect is subtle.
pub struct VocoderEffect {
    carrier_idx: f32,
    pitch_hz: f32,
    q: f32,
    smooth_ms: f32,
    sample_rate: f32,

    /// Modulator bandpasses: one per band per channel.
    mod_bp_l: [SvfBp; Self::N_BANDS],
    mod_bp_r: [SvfBp; Self::N_BANDS],
    /// Carrier bandpasses: one per band, shared across L/R
    /// because the carrier is mono and the bandpass step is
    /// stateless after construction. Saves 16 SVF state updates
    /// per sample without changing the audio result.
    carrier_bp: [SvfBp; Self::N_BANDS],

    /// Per-band envelope follower state, per channel.
    env_l: [f32; Self::N_BANDS],
    env_r: [f32; Self::N_BANDS],

    /// Carrier oscillator phase in `[0, 1)` cycles.
    carrier_phase: f32,
    /// xorshift32 RNG state for the Noise carrier.
    rng_state: u32,

    /// Cached envelope-follower release coefficient. Recomputed in
    /// `set_param` when Smooth changes; attack coefficient is fixed
    /// (5 ms always).
    env_release_coef: f32,
    env_attack_coef: f32,
}

/// 2-pole topology-preserving-transform state-variable filter,
/// bandpass tap. Per Vadim Zavalishin's "The Art of VA Filter
/// Design" -- the standard zero-delay-feedback SVF. We carry the
/// integrator states (`ic1eq`, `ic2eq`) plus the precomputed
/// `g` (frequency coefficient) and `k` (= 1/Q damping). `a1`,
/// `a2` are recomputed per sample so a single Q change touches
/// every band cheaply.
#[derive(Clone, Copy)]
struct SvfBp {
    ic1eq: f32,
    ic2eq: f32,
    /// Frequency coefficient `tan(pi * fc / sr)`. Cached at SR or
    /// frequency change so the per-sample step doesn't call `tan`.
    g: f32,
    /// Damping `1 / Q`. Cached on Q change.
    k: f32,
}

impl SvfBp {
    fn new() -> Self {
        Self {
            ic1eq: 0.0,
            ic2eq: 0.0,
            g: 0.0,
            k: 1.0,
        }
    }

    /// Recompute `g` for the given frequency at sample rate `sr`.
    /// Called when SR or band frequency changes.
    fn set_freq(&mut self, freq_hz: f32, sr: f32) {
        // Clamp to a safe range below Nyquist; `tan` blows up
        // near pi/2, so don't let `fc/sr` approach 0.5.
        let fc = freq_hz.max(1.0).min(sr * 0.45);
        self.g = (std::f32::consts::PI * fc / sr).tan();
    }

    /// Recompute `k` for the given Q.
    fn set_q(&mut self, q: f32) {
        self.k = 1.0 / q.max(0.5);
    }

    /// Zero integrator states.
    fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    /// One sample step; returns the bandpass output. The hi/lo
    /// taps are also available from the same intermediates but
    /// we only need bandpass here.
    #[inline]
    fn step_bp(&mut self, x: f32) -> f32 {
        let a1 = 1.0 / (1.0 + self.g * (self.g + self.k));
        let a2 = self.g * a1;
        let v3 = x - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + self.g * v1;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        v1
    }
}

impl VocoderEffect {
    const N_BANDS: usize = 16;

    const CARRIER_LABELS: &'static [&'static str] = &["Saw", "Square", "Noise"];
    const CARRIER_SAW: usize = 0;
    const CARRIER_SQUARE: usize = 1;
    const CARRIER_NOISE: usize = 2;

    const PITCH_MIN_HZ: f32 = 50.0;
    const PITCH_MAX_HZ: f32 = 2_000.0;
    const Q_MIN: f32 = 1.0;
    const Q_MAX: f32 = 16.0;
    const SMOOTH_MIN_MS: f32 = 1.0;
    const SMOOTH_MAX_MS: f32 = 200.0;
    /// Fixed envelope-follower attack time. Fast enough to catch
    /// consonants and the rising edge of percussive transients
    /// without exposing per-cycle ripple on the rectified
    /// bandpass output.
    const ATTACK_MS: f32 = 5.0;

    /// Band center frequencies (Hz), log-spaced from 100 Hz to
    /// 8 kHz. Generated by
    /// `100 * (8000/100)^(i/15)` for i in 0..16. Pre-rounded for
    /// readability; the small rounding error vs the exact log-
    /// spacing is sub-perceptual.
    const BAND_FREQS: [f32; Self::N_BANDS] = [
        100.0, 133.9, 179.4, 240.3, 321.9, 431.3, 577.7, 773.8, 1036.4, 1388.1, 1859.1, 2490.0,
        3335.2, 4467.4, 5983.6, 8014.1,
    ];

    pub(super) const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Carrier",
            min: 0.0,
            max: (Self::CARRIER_LABELS.len() - 1) as f32,
            default: 0.0, // Saw
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: Self::CARRIER_LABELS,
            },
        },
        ParamSpec {
            name: "Pitch",
            min: Self::PITCH_MIN_HZ,
            max: Self::PITCH_MAX_HZ,
            default: 110.0, // A2 -- a natural male-vocal-ish fundamental
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Q",
            min: Self::Q_MIN,
            max: Self::Q_MAX,
            default: 4.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "",
            },
        },
        ParamSpec {
            name: "Smooth",
            min: Self::SMOOTH_MIN_MS,
            max: Self::SMOOTH_MAX_MS,
            default: 20.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "ms",
            },
        },
    ];

    pub fn new() -> Self {
        let mut me = Self {
            carrier_idx: Self::PARAMS[0].default,
            pitch_hz: Self::PARAMS[1].default,
            q: Self::PARAMS[2].default,
            smooth_ms: Self::PARAMS[3].default,
            sample_rate: 48_000.0,
            mod_bp_l: [SvfBp::new(); Self::N_BANDS],
            mod_bp_r: [SvfBp::new(); Self::N_BANDS],
            carrier_bp: [SvfBp::new(); Self::N_BANDS],
            env_l: [0.0; Self::N_BANDS],
            env_r: [0.0; Self::N_BANDS],
            carrier_phase: 0.0,
            rng_state: 0x9E37_79B9,
            env_release_coef: 0.0,
            env_attack_coef: 0.0,
        };
        me.recompute_coefs();
        me
    }

    /// Recompute every cached coefficient (filter g/k + envelope
    /// attack/release). Called from `new`, `set_sample_rate`, and
    /// the Q / Smooth arms of `set_param`.
    fn recompute_coefs(&mut self) {
        let sr = self.sample_rate;
        // SVF filter coefficients per band.
        for (i, &fc) in Self::BAND_FREQS.iter().enumerate() {
            self.mod_bp_l[i].set_freq(fc, sr);
            self.mod_bp_l[i].set_q(self.q);
            self.mod_bp_r[i].set_freq(fc, sr);
            self.mod_bp_r[i].set_q(self.q);
            self.carrier_bp[i].set_freq(fc, sr);
            self.carrier_bp[i].set_q(self.q);
        }
        // Envelope follower coefficients. One-pole `coef = 1 -
        // exp(-1 / (tau_ms * sr / 1000))` -- step toward the new
        // input value over `tau` samples worth of time.
        let attack_samples = (Self::ATTACK_MS * 0.001 * sr).max(1.0);
        let release_samples = (self.smooth_ms * 0.001 * sr).max(1.0);
        self.env_attack_coef = 1.0 - (-1.0 / attack_samples).exp();
        self.env_release_coef = 1.0 - (-1.0 / release_samples).exp();
    }

    /// Recompute just the per-band Q coefficient (cheaper than
    /// `recompute_coefs` which also touches frequency and
    /// envelope timings). Called from the Q `set_param` arm so
    /// MSEG modulation of Q doesn't pay for tan / exp every call.
    fn recompute_q(&mut self) {
        for i in 0..Self::N_BANDS {
            self.mod_bp_l[i].set_q(self.q);
            self.mod_bp_r[i].set_q(self.q);
            self.carrier_bp[i].set_q(self.q);
        }
    }

    /// Recompute just the envelope-follower release coefficient
    /// (Smooth's responsibility -- attack is constant).
    fn recompute_env_release(&mut self) {
        let release_samples = (self.smooth_ms * 0.001 * self.sample_rate).max(1.0);
        self.env_release_coef = 1.0 - (-1.0 / release_samples).exp();
    }

    /// xorshift32 step for the Noise carrier.
    #[inline]
    fn rng_next(&mut self) -> u32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        x
    }

    /// Generate one sample of the carrier waveform.
    #[inline]
    fn next_carrier(&mut self) -> f32 {
        let mode = (self.carrier_idx.round() as usize).min(Self::CARRIER_LABELS.len() - 1);
        match mode {
            Self::CARRIER_SAW => {
                let phase_inc = self.pitch_hz / self.sample_rate;
                self.carrier_phase += phase_inc;
                if self.carrier_phase >= 1.0 {
                    self.carrier_phase -= self.carrier_phase.floor();
                }
                2.0 * self.carrier_phase - 1.0
            }
            Self::CARRIER_SQUARE => {
                let phase_inc = self.pitch_hz / self.sample_rate;
                self.carrier_phase += phase_inc;
                if self.carrier_phase >= 1.0 {
                    self.carrier_phase -= self.carrier_phase.floor();
                }
                if self.carrier_phase < 0.5 { 1.0 } else { -1.0 }
            }
            Self::CARRIER_NOISE => {
                // White noise in `[-1, +1)` via top-24-bit
                // u32 -> f32 trick.
                let bits = self.rng_next() >> 8;
                (bits as f32) * (1.0 / (1u32 << 23) as f32) - 1.0
            }
            // `set_param` already clamps the index into the valid
            // range, so this arm is unreachable in normal operation.
            // Define it as silence rather than panic so a degenerate
            // preset (corrupt index past Noise) produces inaudible
            // output instead of crashing the audio thread.
            _ => 0.0,
        }
    }

    /// Step one envelope follower with the rectified band signal.
    /// Fast attack / slow release peak-detector: rising signal
    /// gets caught quickly, falling signal smooths slowly into
    /// the next syllable.
    #[inline]
    fn step_env(&self, env: f32, abs_signal: f32) -> f32 {
        if abs_signal > env {
            env + (abs_signal - env) * self.env_attack_coef
        } else {
            env + (abs_signal - env) * self.env_release_coef
        }
    }
}

impl Default for VocoderEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for VocoderEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // ----- Generate one carrier sample -----
        let carrier = self.next_carrier();

        // ----- Per-band processing -----
        let mut out_l = 0.0_f32;
        let mut out_r = 0.0_f32;
        for i in 0..Self::N_BANDS {
            // Bandpass the carrier (shared between channels).
            let carr_b = self.carrier_bp[i].step_bp(carrier);
            // Bandpass each channel's modulator + envelope follow.
            let mod_l = self.mod_bp_l[i].step_bp(left);
            let mod_r = self.mod_bp_r[i].step_bp(right);
            self.env_l[i] = self.step_env(self.env_l[i], mod_l.abs());
            self.env_r[i] = self.step_env(self.env_r[i], mod_r.abs());
            // The carrier band, modulated by the modulator's
            // envelope in that band, is this band's contribution.
            out_l += carr_b * self.env_l[i];
            out_r += carr_b * self.env_r[i];
        }

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recompute_coefs();
    }

    fn reset(&mut self) {
        for f in self
            .mod_bp_l
            .iter_mut()
            .chain(self.mod_bp_r.iter_mut())
            .chain(self.carrier_bp.iter_mut())
        {
            f.reset();
        }
        self.env_l = [0.0; Self::N_BANDS];
        self.env_r = [0.0; Self::N_BANDS];
        self.carrier_phase = 0.0;
        // Don't reset rng_state -- preserving it across resets
        // avoids replaying identical Noise samples after each one.
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                let max_idx = (Self::CARRIER_LABELS.len() - 1) as f32;
                self.carrier_idx = value.round().clamp(0.0, max_idx);
            }
            1 => self.pitch_hz = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => {
                self.q = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max);
                self.recompute_q();
            }
            3 => {
                self.smooth_ms = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max);
                self.recompute_env_release();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat, ParamScaling};

    fn configured_vocoder(sample_rate: f32, params: [f32; 4]) -> VocoderEffect {
        let mut vocoder = VocoderEffect::new();
        vocoder.set_sample_rate(sample_rate);
        for (index, value) in params.into_iter().enumerate() {
            vocoder.set_param(index, value);
        }
        vocoder
    }

    #[test]
    fn sample_rates_below_one_use_the_minimum_rate_without_panicking() {
        for requested in [-48_000.0, 0.0, 0.5, 1.0, 2.0, 8_000.0] {
            let mut vocoder = VocoderEffect::new();
            vocoder.set_sample_rate(requested);
            let rate = (requested as f64).max(1.0);
            assert_eq!(vocoder.sample_rate as f64, rate);
            for filter in vocoder
                .mod_bp_l
                .iter()
                .chain(&vocoder.mod_bp_r)
                .chain(&vocoder.carrier_bp)
            {
                assert!(filter.g.is_finite());
                assert!(filter.g > 0.0 && filter.g < 6.314);
            }
            for _ in 0..64 {
                let (left, right) = vocoder.process_sample(0.125, -0.25);
                assert!(left.is_finite() && right.is_finite());
            }
        }
    }

    #[test]
    fn bandpass_coefficients_bound_frequency_and_q_without_clearing_history() {
        for (rate, frequency, bounded_frequency) in [
            (48_000.0, -100.0, 1.0),
            (48_000.0, 731.0, 731.0),
            (8_000.0, 8_014.1, 3_600.0),
            (1.0, 100.0, 0.45),
        ] {
            let mut filter = SvfBp::new();
            filter.ic1eq = 0.25;
            filter.ic2eq = -0.125;
            filter.set_freq(frequency, rate);
            let expected = (std::f64::consts::PI * bounded_frequency / rate as f64).tan();
            assert!((filter.g as f64 - expected).abs() < 4e-6);
            for (q, expected) in [(-1.0, 2.0), (0.5, 2.0), (4.0, 0.25), (16.0, 0.0625)] {
                filter.set_q(q);
                assert_eq!(filter.k, expected);
                assert_eq!(filter.ic1eq, 0.25);
                assert_eq!(filter.ic2eq, -0.125);
            }
        }
    }

    #[test]
    fn bandpass_impulse_matches_an_independent_biquad_transfer_function() {
        for (rate, frequency, q) in [
            (48_000.0, 100.0, 1.0),
            (48_000.0, 1_388.1, 4.0),
            (8_000.0, 3_600.0, 16.0),
        ] {
            let mut filter = SvfBp::new();
            filter.set_freq(frequency, rate);
            filter.set_q(q);
            // H(z) = b0 * (1 - z^-2) / (1 + a1*z^-1 + a2*z^-2).
            let g = (std::f64::consts::PI * frequency as f64 / rate as f64).tan();
            let damping = g / q as f64;
            let denominator = 1.0 + damping + g * g;
            let b0 = g / denominator;
            let a1 = 2.0 * (g * g - 1.0) / denominator;
            let a2 = (1.0 - damping + g * g) / denominator;
            let mut previous = [0.0; 2];
            for sample in 0..256 {
                let numerator = match sample {
                    0 => b0,
                    2 => -b0,
                    _ => 0.0,
                };
                let expected = numerator - a1 * previous[0] - a2 * previous[1];
                previous = [expected, previous[0]];
                let actual = filter.step_bp(if sample == 0 { 1.0 } else { 0.0 });
                assert!(
                    (actual as f64 - expected).abs() < 5e-6,
                    "rate {rate}, frequency {frequency}, Q {q}, sample {sample}: {actual} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn coefficient_caches_follow_band_centers_q_and_envelope_time_constants() {
        let frequencies: [f64; 16] = [
            100.0, 133.9, 179.4, 240.3, 321.9, 431.3, 577.7, 773.8, 1_036.4, 1_388.1, 1_859.1,
            2_490.0, 3_335.2, 4_467.4, 5_983.6, 8_014.1,
        ];
        for rate in [8_000.0, 44_100.0, 96_000.0] {
            let mut vocoder = VocoderEffect::new();
            for (q, smooth) in [(4.0, 20.0), (1.0, 1.0), (16.0, 200.0)] {
                vocoder.set_param(2, q);
                vocoder.set_param(3, smooth);
                vocoder.set_sample_rate(rate);
                for (band, frequency) in frequencies.into_iter().enumerate() {
                    let bounded = frequency.min(rate as f64 * 0.45);
                    let expected = (std::f64::consts::PI * bounded / rate as f64).tan();
                    for filter in [
                        &vocoder.mod_bp_l[band],
                        &vocoder.mod_bp_r[band],
                        &vocoder.carrier_bp[band],
                    ] {
                        assert!((filter.g as f64 - expected).abs() < 4e-6);
                        assert_eq!(filter.k, 1.0 / q);
                    }
                }
                let attack = 1.0 - (-1_000.0 / (5.0 * rate as f64)).exp();
                let release = 1.0 - (-1_000.0 / (smooth as f64 * rate as f64)).exp();
                assert!((vocoder.env_attack_coef as f64 - attack).abs() < 4e-8);
                assert!((vocoder.env_release_coef as f64 - release).abs() < 4e-8);
            }
        }
    }

    #[test]
    fn pitched_carriers_advance_before_output_and_wrap_at_exact_and_multiple_cycles() {
        for (mode, expected) in [(0.0, [-0.5, 0.0, 0.5, -1.0]), (1.0, [1.0, -1.0, -1.0, 1.0])] {
            let mut vocoder = configured_vocoder(400.0, [mode, 100.0, 4.0, 20.0]);
            for _ in 0..3 {
                for (index, output) in expected.into_iter().enumerate() {
                    assert_eq!(vocoder.next_carrier(), output);
                    assert_eq!(vocoder.carrier_phase, ((index + 1) % 4) as f32 * 0.25);
                }
            }
            vocoder.set_sample_rate(800.0);
            vocoder.set_param(1, 1_800.0);
            assert_eq!(vocoder.next_carrier(), expected[0]);
            assert_eq!(vocoder.carrier_phase, 0.25);
        }
    }

    #[test]
    fn noise_carrier_matches_fixed_rng_words_and_the_bipolar_sample_mapping() {
        let words = [
            0x510c_4619,
            0xe02e_553e,
            0x7bb9_8f3a,
            0x0183_a8b5,
            0xe633_6d1f,
            0xf989_d237,
            0xba25_29d0,
            0xfcfb_edbf,
        ];
        let mut vocoder = configured_vocoder(48_000.0, [2.0, 110.0, 4.0, 20.0]);
        for word in words {
            let expected = ((word / 256) as f64 / 8_388_608.0 - 1.0) as f32;
            assert_eq!(vocoder.next_carrier(), expected);
            assert_eq!(vocoder.rng_state, word);
        }
    }

    #[test]
    fn noise_ignores_pitch_and_holds_the_pitched_carrier_phase() {
        let mut low = configured_vocoder(400.0, [0.0, 100.0, 4.0, 20.0]);
        assert_eq!(low.next_carrier(), -0.5);
        low.set_param(0, 2.0);
        low.set_param(1, 50.0);
        let mut high = configured_vocoder(400.0, [2.0, 2_000.0, 4.0, 20.0]);
        for _ in 0..32 {
            assert_eq!(low.next_carrier(), high.next_carrier());
            assert_eq!(low.carrier_phase, 0.25);
            assert_eq!(high.carrier_phase, 0.0);
        }
        low.set_param(1, 100.0);
        low.set_param(0, 1.0);
        assert_eq!(low.next_carrier(), -1.0);
        assert_eq!(low.carrier_phase, 0.5);
    }

    #[test]
    fn envelope_attack_and_release_match_closed_form_step_responses() {
        for smooth in [1.0, 37.5, 200.0] {
            let vocoder = configured_vocoder(8_000.0, [0.0, 110.0, 4.0, smooth]);
            let mut envelope = 0.125;
            for sample in 1..=80 {
                envelope = vocoder.step_env(envelope, 0.75);
                let expected = 0.75 - 0.625 * (-(sample as f64) / 40.0).exp();
                assert!((envelope as f64 - expected).abs() < 3e-6);
            }
            let attack_end = 0.75 - 0.625 * (-2.0_f64).exp();
            for sample in 1..=80 {
                envelope = vocoder.step_env(envelope, 0.0625);
                let expected = 0.0625
                    + (attack_end - 0.0625) * (-(sample as f64) / (smooth as f64 * 8.0)).exp();
                assert!((envelope as f64 - expected).abs() < 3e-6);
            }
            assert_eq!(vocoder.step_env(0.25, 0.25), 0.25);
        }
    }

    #[test]
    fn first_sample_combines_all_carrier_bands_with_rectified_stereo_envelopes() {
        for rate in [8_000.0, 48_000.0] {
            for (mode, carrier) in [
                (0.0, 2.0 * 250.0 / rate as f64 - 1.0),
                (1.0, 1.0),
                (2.0, -0.36681294441223145),
            ] {
                let mut vocoder = configured_vocoder(rate, [mode, 250.0, 7.0, 37.5]);
                let weight: f64 = VocoderEffect::BAND_FREQS
                    .iter()
                    .map(|&frequency| {
                        let g = (std::f64::consts::PI * (frequency as f64).min(rate as f64 * 0.45)
                            / rate as f64)
                            .tan();
                        (g / (1.0 + g / 7.0 + g * g)).powi(2)
                    })
                    .sum();
                let attack = 1.0 - (-1_000.0 / (5.0 * rate as f64)).exp();
                let actual = vocoder.process_sample(0.25, -0.625);
                let expected = (
                    carrier * 0.25 * attack * weight,
                    carrier * 0.625 * attack * weight,
                );
                assert!((actual.0 as f64 - expected.0).abs() < 2e-7);
                assert!((actual.1 as f64 - expected.1).abs() < 2e-7);
            }
        }
    }

    #[test]
    fn stereo_channels_are_independent_and_rectification_preserves_polarity_symmetry() {
        for mode in [0.0, 1.0, 2.0] {
            let params = [mode, 250.0, 4.0, 37.5];
            let mut left_only = configured_vocoder(8_000.0, params);
            let mut right_only = configured_vocoder(8_000.0, params);
            let mut inverted = configured_vocoder(8_000.0, params);
            let mut energy = 0.0;
            for sample in 0..1_024 {
                let input = (sample as f32 * 0.37).sin() * 0.5;
                let left = left_only.process_sample(input, 0.0);
                let right = right_only.process_sample(0.0, input);
                let opposite = inverted.process_sample(-input, 0.0);
                assert_eq!(left.1, 0.0);
                assert_eq!(right.0, 0.0);
                assert_eq!(left.0, right.1);
                assert_eq!(left, opposite);
                energy += left.0 * left.0;
            }
            assert!(energy > 1e-4);
        }
    }

    #[test]
    fn reset_replays_fresh_audio_while_preserving_noise_progression_and_configuration() {
        for mode in [0.0, 1.0, 2.0] {
            let params = [mode, 173.0, 7.0, 37.5];
            let mut used = configured_vocoder(8_000.0, params);
            let mut fresh = configured_vocoder(8_000.0, params);
            for sample in 0..1_024 {
                used.process_sample((sample as f32 * 0.37).sin() * 0.5, 0.125);
            }
            let rng = used.rng_state;
            used.reset();
            assert_eq!(used.rng_state, rng);
            fresh.rng_state = rng;
            for sample in 0..1_024 {
                let input = ((sample as f32 * 0.23).cos() * 0.25, -0.125);
                assert_eq!(
                    used.process_sample(input.0, input.1),
                    fresh.process_sample(input.0, input.1)
                );
            }
        }
    }

    #[test]
    fn parameter_and_sample_rate_changes_preserve_active_filter_and_envelope_history() {
        let params = [2.0, 173.0, 7.0, 37.5];
        let mut changed = configured_vocoder(8_000.0, params);
        let mut reference = configured_vocoder(8_000.0, params);
        for sample in 0..1_024 {
            let input = ((sample as f32 * 0.37).sin() * 0.5, -0.125);
            changed.process_sample(input.0, input.1);
            reference.process_sample(input.0, input.1);
        }
        changed.set_sample_rate(96_000.0);
        for (index, value) in [0.0, 2_000.0, 1.0, 200.0].into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_sample_rate(8_000.0);
        for (index, value) in params.into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_param(99, f32::INFINITY);
        for _ in 0..1_024 {
            assert_eq!(
                changed.process_sample(0.0, 0.0),
                reference.process_sample(0.0, 0.0)
            );
        }
    }

    #[test]
    fn vocoder_lists_four_parameters_with_the_expected_specs() {
        let v = VocoderEffect::new();
        let specs = v.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Carrier");
        assert!(matches!(specs[0].format, ParamFormat::Enum { .. }));
        assert_eq!(specs[1].name, "Pitch");
        assert!(matches!(specs[1].scaling, ParamScaling::Log));
        assert!(matches!(specs[1].format, ParamFormat::Hertz));
        assert_eq!(specs[1].min, 50.0);
        assert_eq!(specs[1].max, 2_000.0);
        assert_eq!(specs[2].name, "Q");
        assert_eq!(specs[2].min, 1.0);
        assert_eq!(specs[2].max, 16.0);
        assert_eq!(specs[3].name, "Smooth");
        assert!(matches!(specs[3].scaling, ParamScaling::Log));
        assert!(matches!(
            specs[3].format,
            ParamFormat::Number { unit: "ms", .. }
        ));
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                (0.0, 2.0, 0.0),
                (50.0, 2_000.0, 110.0),
                (1.0, 16.0, 4.0),
                (1.0, 200.0, 20.0)
            ]
        );
        assert!(
            matches!(specs[0].format, ParamFormat::Enum { labels } if labels == ["Saw", "Square", "Noise"])
        );
        assert_eq!(
            [v.carrier_idx, v.pitch_hz, v.q, v.smooth_ms],
            [0.0, 110.0, 4.0, 20.0]
        );
    }

    #[test]
    fn vocoder_set_param_clamps_each_slot() {
        let mut v = VocoderEffect::new();
        v.set_param(0, 99.0);
        assert_eq!(v.carrier_idx, 2.0); // Noise (last index)
        v.set_param(0, -5.0);
        assert_eq!(v.carrier_idx, 0.0);
        v.set_param(1, 99_999.0);
        assert_eq!(v.pitch_hz, 2_000.0);
        v.set_param(1, 0.0);
        assert_eq!(v.pitch_hz, 50.0);
        v.set_param(2, 99.0);
        assert_eq!(v.q, 16.0);
        v.set_param(2, 0.0);
        assert_eq!(v.q, 1.0);
        v.set_param(3, 0.0);
        assert_eq!(v.smooth_ms, 1.0);
        v.set_param(3, 99_999.0);
        assert_eq!(v.smooth_ms, 200.0);
    }

    #[test]
    fn vocoder_silent_modulator_produces_silent_output() {
        // With zero modulator (no input signal), every envelope
        // follower decays to 0, so each band's output is 0,
        // regardless of carrier.
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 0.0); // Saw
        v.set_param(1, 220.0);
        // Settle (envelopes start at 0, but ensure carrier isn't
        // poking through any transient state).
        for _ in 0..4_800 {
            let _ = v.process_sample(0.0, 0.0);
        }
        // Measure 1 second of silent-input output.
        let mut peak = 0.0_f32;
        for _ in 0..48_000 {
            let (l, r) = v.process_sample(0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak < 1e-4,
            "silent modulator must produce silent output; peak {peak}"
        );
    }

    #[test]
    fn vocoder_silent_modulator_with_noise_carrier_also_silent() {
        // Noise carrier ignores Pitch; same invariant must hold.
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 2.0); // Noise
        for _ in 0..4_800 {
            let _ = v.process_sample(0.0, 0.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..48_000 {
            let (l, r) = v.process_sample(0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak < 1e-4,
            "silent modulator + Noise carrier must produce silent output; peak {peak}"
        );
    }

    #[test]
    fn vocoder_modulator_envelope_opens_carrier_output() {
        // A non-silent modulator should produce non-silent output.
        // Drive a 440 Hz sine through the modulator path with Saw
        // carrier at 110 Hz -> output should have measurable
        // energy after the envelopes settle.
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(0, 0.0); // Saw
        v.set_param(1, 110.0);
        v.set_param(2, 4.0);
        v.set_param(3, 20.0);
        // Settle.
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let mod_in = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let _ = v.process_sample(mod_in, mod_in);
        }
        let mut energy = 0.0_f32;
        for i in 4_800..52_800 {
            let t = i as f32 / 48_000.0;
            let mod_in = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, _) = v.process_sample(mod_in, mod_in);
            energy += l * l;
        }
        assert!(
            energy > 0.01,
            "modulator energy should open the carrier output; got energy={energy}"
        );
    }

    #[test]
    fn vocoder_carrier_choice_changes_output_character() {
        // Same modulator and Pitch, different Carrier -> output
        // should differ measurably. Saw and Square share the
        // oscillator phase logic but produce different waveforms,
        // so the band energies differ.
        let run = |carrier_idx: f32| {
            let mut v = VocoderEffect::new();
            v.set_sample_rate(48_000.0);
            v.set_param(0, carrier_idx);
            v.set_param(1, 220.0);
            v.set_param(2, 4.0);
            v.set_param(3, 20.0);
            for i in 0..4_800 {
                let t = i as f32 / 48_000.0;
                let m = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let _ = v.process_sample(m, m);
            }
            let mut out = Vec::with_capacity(2_000);
            for i in 4_800..6_800 {
                let t = i as f32 / 48_000.0;
                let m = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let (l, _) = v.process_sample(m, m);
                out.push(l);
            }
            out
        };
        let saw_out = run(0.0);
        let square_out = run(1.0);
        let mut rms_diff = 0.0_f32;
        for i in 0..saw_out.len() {
            rms_diff += (saw_out[i] - square_out[i]) * (saw_out[i] - square_out[i]);
        }
        let rms = (rms_diff / saw_out.len() as f32).sqrt();
        assert!(
            rms > 0.001,
            "Saw vs Square carrier should produce measurably different outputs; rms={rms}"
        );
    }

    #[test]
    fn vocoder_envelope_follower_attack_is_faster_than_release() {
        // Internal sanity on the cached coefficients: ATTACK_MS
        // (5 ms) is faster than the default Smooth (20 ms), so
        // attack_coef > release_coef.
        let v = VocoderEffect::new();
        assert!(
            v.env_attack_coef > v.env_release_coef,
            "attack coef ({}) should be > release coef ({}) at default Smooth",
            v.env_attack_coef,
            v.env_release_coef
        );
    }

    #[test]
    fn vocoder_smooth_param_lengthens_release() {
        // Larger Smooth -> longer release -> smaller release_coef
        // (step a smaller fraction toward the new value per
        // sample).
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(3, 10.0); // Short release
        let short = v.env_release_coef;
        v.set_param(3, 200.0); // Long release
        let long = v.env_release_coef;
        assert!(
            short > long,
            "shorter Smooth must give a larger release coef ({} vs {})",
            short,
            long
        );
    }

    #[test]
    fn vocoder_higher_q_sharpens_bandpass_response() {
        // Higher Q -> smaller k (= 1/Q) in each filter. Verify
        // by changing Q and inspecting the cached k.
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        v.set_param(2, 2.0);
        let k_low_q = v.mod_bp_l[0].k;
        v.set_param(2, 16.0);
        let k_high_q = v.mod_bp_l[0].k;
        assert!(
            k_high_q < k_low_q,
            "Q=16 should give smaller k than Q=2 (k = 1/Q): {} vs {}",
            k_high_q,
            k_low_q
        );
        // And the carrier filters track the same Q.
        assert_eq!(v.carrier_bp[0].k, k_high_q);
    }

    #[test]
    fn vocoder_set_sample_rate_recomputes_filter_freq() {
        // Same band frequency at different sample rates produces
        // different g (= tan(pi*fc/sr)). Verify the cached g
        // tracks sample-rate changes.
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        let g_48k = v.mod_bp_l[5].g;
        v.set_sample_rate(96_000.0);
        let g_96k = v.mod_bp_l[5].g;
        // At higher SR the same band freq is a smaller fraction
        // of SR, so g (= tan(pi * fc / sr)) is smaller.
        assert!(
            g_96k < g_48k,
            "g should decrease as SR rises: 48k={}, 96k={}",
            g_48k,
            g_96k
        );
    }

    #[test]
    fn vocoder_stays_bounded_under_aggressive_sweep() {
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            v.set_param(0, (i as f32 / 1_000.0).fract() * 3.0);
            let pp = (i as f32 / 4_000.0).fract();
            v.set_param(1, 50.0 * 40.0_f32.powf(pp)); // 50..2000 Hz log
            v.set_param(2, 1.0 + (i as f32 / 3_000.0).fract() * 15.0);
            let ps = (i as f32 / 5_000.0).fract();
            v.set_param(3, 1.0 * 200.0_f32.powf(ps)); // 1..200 ms log
            let m = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = v.process_sample(m, m);
            assert!(l.is_finite() && r.is_finite(), "sample {i} not finite");
            // The sum of 16 bands of (carrier_band * env) can
            // briefly reach a few x the input magnitude at hot Q
            // settings where bands resonate. 32 is comfortably
            // above the worst observed peak under this sweep.
            assert!(
                l.abs() < 32.0 && r.abs() < 32.0,
                "sample {i} blew up: ({l},{r})"
            );
        }
    }

    #[test]
    fn vocoder_reset_clears_filter_state_and_envelopes() {
        let mut v = VocoderEffect::new();
        v.set_sample_rate(48_000.0);
        // Drive some signal in to charge the state.
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let m = 0.7 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let _ = v.process_sample(m, m);
        }
        v.reset();
        // Filter integrator states cleared.
        for f in v
            .mod_bp_l
            .iter()
            .chain(v.mod_bp_r.iter())
            .chain(v.carrier_bp.iter())
        {
            assert_eq!(f.ic1eq, 0.0);
            assert_eq!(f.ic2eq, 0.0);
        }
        // Envelopes cleared.
        assert!(v.env_l.iter().all(|&e| e == 0.0));
        assert!(v.env_r.iter().all(|&e| e == 0.0));
        assert_eq!(v.carrier_phase, 0.0);
    }
}
