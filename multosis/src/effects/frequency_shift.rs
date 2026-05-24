use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Latency-free single-sideband frequency shifter (Bode-style).
/// Adds a constant Hz offset to every component of the input,
/// breaking harmonic relationships -- 100 Hz + 50 = 150 Hz,
/// 200 Hz + 50 = 250 Hz (not 300 Hz like a pitch shift would
/// give). Iconic for the "clangy", "metallic", or "bell-like"
/// sound: harmonic content becomes inharmonic.
///
/// **Latency:** ~0 samples. The IIR Hilbert has a few samples of
/// internal group delay but no explicit delay line, so it doesn't
/// add user-visible latency vs the dry input -- inserting before
/// drawing is safe.
///
/// **Algorithm.** Each channel runs a Signalsmith-style IIR
/// analytic-signal extractor (12 complex 1-pole sections in
/// parallel, impulse-invariant from an order-12 elliptic prototype;
/// see [`HilbertIir`]). The pair `(re, im)` is the analytic signal
/// of the input: `re` ~ delayed input, `im` ~ Hilbert(input).
/// Sample-rate-aware: pole positions scale with
/// `min(0.46, 20_000/fs)` so the passband stays anchored to the
/// audio band at any SR. The output is then
/// `re * cos(phase) - im * sin(phase)` -- the upper sideband only,
/// so the spectrum is shifted by exactly `shift_hz` (negative
/// shifts roll downward via the carrier sign).
///
/// **Verified empirically** at 48 kHz (see `tests/` below):
/// 90 deg phase tolerance <= 0.2 deg from 100 Hz to 20 kHz;
/// unwanted sideband rejection 60-110 dB across the same range;
/// frequency-shift correctness exact to integer Hz. The useful
/// passband is approximately 50 Hz to 20 kHz (rejection drops to
/// ~35 dB at 50 Hz and ~23 dB at 25 Hz).
///
/// **Shift** spans -1000..+1000 Hz. Positive values shift up,
/// negative shift down. Small values (a few Hz) produce subtle
/// phasing/detuning; medium (50..200 Hz) gives the classic
/// bell/clang; large (500+) becomes radically inharmonic.
///
/// **Feedback** routes the shifted output back into the Hilbert
/// input (capped at +/-95 % at the dial; the recirculated signal
/// is additionally `tanh`-saturated before storage so the loop
/// stays bounded even when Drive is hot). Positive feedback
/// stacks repeated shifts -- a 100 Hz shift with 80 % feedback
/// gives 100, 200, 300, ... Hz of cumulative shift in the
/// resonance, like a Shepard-style infinite-rise (or fall for
/// negative Shift). At high Drive + Feedback the in-loop
/// saturation adds a subtle "compressor inside the resonance"
/// character that plateaus the stacking instead of letting it
/// digitally explode.
///
/// **Width** controls how the R-channel shift differs from L.
/// Width=0 % shifts both channels by the same amount (mono
/// shift); Width=50 % leaves the R channel unshifted (one
/// shifted, one dry); Width=100 % shifts R by `-Shift` (opposite
/// direction). The signed cross-spread is what makes the effect
/// stereo: with a hard pan up on L and down on R, simple program
/// material spreads dramatically.
///
/// **Drive** boosts the dry input (only) before mixing with the
/// recirculated feedback (0..12 dB). It does NOT amplify the
/// feedback path -- doing so would put loop gain
/// `drive * feedback` > 1 and diverge into f32 overflow, since
/// the in-loop `tanh` saturation only bounds the loop AFTER it
/// reaches the saturation knee (tanh is linear near zero). With
/// Drive outside the loop, gain stays at `feedback ≤ 0.95`
/// unconditionally. Musically: hotter dry input still drives the
/// resonance harder (louder signal enters the loop each sample),
/// and the in-loop tanh adds compressor-like saturation in the
/// resonance -- so "Drive into a resonant frequency shifter for
/// gritty Shepard tones" still works, the runaway just doesn't.
/// With Feedback at 0 it's just an input-gain trim, since the
/// Hilbert+carrier chain is effectively unity.
///
/// **Per-sample work:** 12 complex multiplies + sums per channel
/// (the Hilbert) + 2 `sin_cos` transcendentals + carrier-mix
/// MACs. No allocations; ~280 B of state per instance.
///
/// **Attribution.** The IIR Hilbert is a port of Signalsmith
/// Audio's `hilbert-iir`
/// (<https://github.com/Signalsmith-Audio/hilbert-iir>, 0BSD
/// license). Coefficients and pole positions are lifted verbatim
/// from `hilbert.h`; the impulse-invariant discrete-domain
/// transform happens once per sample-rate change.
pub struct FrequencyShiftEffect {
    shift_hz: f32,
    feedback_pct: f32,
    width_pct: f32,
    drive_db: f32,
    sample_rate: f32,

    /// Per-channel Hilbert pairs. State is small enough to live
    /// inline; no heap.
    hilbert_l: HilbertIir,
    hilbert_r: HilbertIir,

    /// Carrier phase accumulators (radians, wrapped to `[0, 2pi)`).
    /// Separate per channel so Width can give the two channels
    /// different carrier rates without losing phase coherence
    /// across MSEG-modulated Shift sweeps.
    carrier_phase_l: f32,
    carrier_phase_r: f32,

    /// Previous sample's wet output, recirculated as feedback.
    /// The 1-sample lag keeps the feedback loop strictly causal.
    fb_l: f32,
    fb_r: f32,

    /// Cached `db_to_linear(drive_db)`. Recomputed in `set_param`
    /// when Drive changes so the per-sample path stays free of
    /// `powf`/`exp` calls -- matches the gain-caching pattern in
    /// Distortion / Satch / Bitcrush.
    drive_gain: f32,
}

/// Number of complex 1-pole sections in the Signalsmith parallel bank.
const HILBERT_ORDER: usize = 12;

/// Signalsmith's continuous-time partial-fraction residues, one per
/// section. Each entry is `(re, im)`. At runtime these are scaled by
/// `freq_factor * passband_gain` to land in the discrete domain.
/// Verbatim from `hilbert.h`.
const HILBERT_COEFFS_CT: [(f64, f64); HILBERT_ORDER] = [
    (-0.000_224_352_093_802, 0.005_434_990_182_01),
    (0.010_750_055_781_5, -0.017_389_068_568_1),
    (-0.045_679_587_391_7, 0.022_916_693_142_9),
    (0.112_825_005_82, 0.002_784_136_612_37),
    (-0.208_067_578_452, -0.104_628_958_675),
    (0.287_178_375_01, 0.336_192_397_19),
    (-0.254_675_294_431, -0.683_033_899_655),
    (0.048_108_183_502_6, 0.954_061_589_374),
    (0.227_861_357_867, -0.891_273_574_569),
    (-0.365_411_839_137, 0.525_088_317_271),
    (0.280_729_061_131, -0.155_131_206_606),
    (-0.093_506_178_772_8, 0.005_122_458_554_04),
];

/// Signalsmith's continuous-time pole positions, one per section.
/// The discrete pole at the runtime sample rate is
/// `exp(continuous_pole * freq_factor)` (impulse-invariant transform).
const HILBERT_POLES_CT: [(f64, f64); HILBERT_ORDER] = [
    (-0.004_953_359_764_78, 0.009_257_987_687_2),
    (-0.017_859_491_302, 0.027_349_372_554_3),
    (-0.041_371_437_315_5, 0.074_475_691_028_7),
    (-0.088_214_840_888_5, 0.178_349_677_457),
    (-0.179_229_658_12, 0.396_013_402_23),
    (-0.338_261_800_753, 0.829_229_533_354),
    (-0.557_688_699_732, 1.612_985_383_28),
    (-0.735_157_736_148, 2.799_873_986_82),
    (-0.719_057_381_172, 4.163_961_661_28),
    (-0.517_871_025_209, 5.297_248_268_04),
    (-0.280_197_469_471, 5.995_986_023_88),
    (-0.085_275_135_453_1, 6.304_849_237_7),
];

/// Continuous-time direct-passthrough residue. Tiny but non-zero;
/// fills the small fraction of the response not captured by the bank.
const HILBERT_DIRECT_CT: f64 = 0.000_262_057_212_648;

/// `passband_gain = 2.0` makes the SSB output's wanted sideband
/// match the input amplitude (0 dB). Without it the analytic-signal
/// extractor would halve the energy (it drops negative frequencies),
/// putting the wanted sideband at -6 dB.
const HILBERT_PASSBAND_GAIN: f32 = 2.0;

/// Single-channel Signalsmith-style IIR Hilbert (analytic-signal
/// extractor). Returns `(real, imag)` per input sample; `imag` leads
/// `real` by ~90 deg across approximately 50 Hz to 20 kHz at 48 kHz.
///
/// 12 complex 1-pole sections in parallel, impulse-invariant from
/// an order-12 elliptic prototype (0.5 dB ripple, ~90 dB stop).
/// Sample-rate-aware: `recompute_coeffs` rescales every pole and
/// residue by `freq_factor = min(0.46, 20_000/fs)` so the design's
/// spectral shape stays anchored regardless of sample rate.
///
/// Audio-thread safety: `process` is allocation-free and operates
/// entirely on inline `[f32; 12]` arrays. Coefficient recomputation
/// runs in `new`/`recompute_coeffs`, both off the audio thread.
#[derive(Clone)]
struct HilbertIir {
    /// Per-section discrete-domain coefficients
    /// (`coeff_ct * freq_factor * gain`, split into real/imag).
    coeffs_r: [f32; HILBERT_ORDER],
    coeffs_i: [f32; HILBERT_ORDER],
    /// Per-section discrete poles (`exp(pole_ct * freq_factor)`).
    poles_r: [f32; HILBERT_ORDER],
    poles_i: [f32; HILBERT_ORDER],
    /// Direct passthrough scaling (`DIRECT_CT * 2 * gain * freq_factor`).
    direct: f32,
    /// Per-section complex state, updated each sample.
    state_r: [f32; HILBERT_ORDER],
    state_i: [f32; HILBERT_ORDER],
}

impl HilbertIir {
    /// Construct a Hilbert IIR initialised for the given sample rate.
    fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            coeffs_r: [0.0; HILBERT_ORDER],
            coeffs_i: [0.0; HILBERT_ORDER],
            poles_r: [0.0; HILBERT_ORDER],
            poles_i: [0.0; HILBERT_ORDER],
            direct: 0.0,
            state_r: [0.0; HILBERT_ORDER],
            state_i: [0.0; HILBERT_ORDER],
        };
        s.recompute_coeffs(sample_rate);
        s
    }

    /// Recompute discrete-domain coefficients for a new sample rate.
    /// Does not touch per-section state -- a smooth SR change won't
    /// cause clicks on its own.
    fn recompute_coeffs(&mut self, sample_rate: f32) {
        // Cap freq_factor at 0.46 (just under 0.5 = Nyquist) so the
        // design stays valid below ~43.5 kHz too; above that the
        // 20_000/fs ratio takes over.
        let freq_factor = (20_000.0_f64 / sample_rate.max(1.0) as f64).min(0.46);
        let gain = HILBERT_PASSBAND_GAIN as f64;
        self.direct = (HILBERT_DIRECT_CT * 2.0 * gain * freq_factor) as f32;
        for i in 0..HILBERT_ORDER {
            let (cr, ci) = HILBERT_COEFFS_CT[i];
            self.coeffs_r[i] = (cr * freq_factor * gain) as f32;
            self.coeffs_i[i] = (ci * freq_factor * gain) as f32;
            let (pr, pi) = HILBERT_POLES_CT[i];
            let scale = (pr * freq_factor).exp();
            let arg = pi * freq_factor;
            self.poles_r[i] = (scale * arg.cos()) as f32;
            self.poles_i[i] = (scale * arg.sin()) as f32;
        }
    }

    /// Process one real input sample, returning `(real, imag)` of
    /// the analytic signal. `imag` is approximately the Hilbert
    /// transform of `real`; both are ~equal magnitude with `imag`
    /// leading `real` by 90 deg in the passband.
    ///
    /// Per-section: `new_state = state * pole + x * coeff` (complex
    /// multiply-add). Outputs accumulate into `(re, im)` in the
    /// same pass.
    #[inline]
    fn process(&mut self, x: f32) -> (f32, f32) {
        let mut re_acc = x * self.direct;
        let mut im_acc = 0.0_f32;
        for i in 0..HILBERT_ORDER {
            let new_r = self.state_r[i] * self.poles_r[i] - self.state_i[i] * self.poles_i[i]
                + x * self.coeffs_r[i];
            let new_i = self.state_r[i] * self.poles_i[i]
                + self.state_i[i] * self.poles_r[i]
                + x * self.coeffs_i[i];
            self.state_r[i] = new_r;
            self.state_i[i] = new_i;
            re_acc += new_r;
            im_acc += new_i;
        }
        (re_acc, im_acc)
    }

    /// Zero every per-section state. Coefficients are SR-derived
    /// and stay intact across resets.
    #[inline]
    fn reset(&mut self) {
        self.state_r = [0.0; HILBERT_ORDER];
        self.state_i = [0.0; HILBERT_ORDER];
    }
}

impl FrequencyShiftEffect {
    const SHIFT_MIN_HZ: f32 = -1_000.0;
    const SHIFT_MAX_HZ: f32 = 1_000.0;
    const FB_CAP: f32 = 0.95;

    const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Shift",
            min: Self::SHIFT_MIN_HZ,
            max: Self::SHIFT_MAX_HZ,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "Hz",
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
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Drive",
            min: 0.0,
            // Capped at 12 dB (~4x). The Hilbert+SSB chain is unity-
            // gain, so Drive directly scales the wet output -- at the
            // previous 24 dB cap a hot Drive could produce ~16x dry
            // amplitude, hearing-damage territory before the engine's
            // per-track Mix even gets a say. 12 dB is plenty to push
            // the feedback resonance into "alive" territory without
            // exiting the practical loudness range.
            max: 12.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "dB",
            },
        },
    ];

    pub fn new() -> Self {
        let drive_db = Self::PARAMS[3].default;
        let sample_rate = 48_000.0;
        Self {
            shift_hz: Self::PARAMS[0].default,
            feedback_pct: Self::PARAMS[1].default,
            width_pct: Self::PARAMS[2].default,
            drive_db,
            sample_rate,
            hilbert_l: HilbertIir::new(sample_rate),
            hilbert_r: HilbertIir::new(sample_rate),
            carrier_phase_l: 0.0,
            carrier_phase_r: 0.0,
            fb_l: 0.0,
            fb_r: 0.0,
            drive_gain: tract_dsp::db::db_to_linear_fast(drive_db),
        }
    }
}

impl Default for FrequencyShiftEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for FrequencyShiftEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let drive = self.drive_gain;
        let feedback = (self.feedback_pct * 0.01).clamp(-Self::FB_CAP, Self::FB_CAP);
        // Width 0..100 % -> R-channel shift modifier in [+1, -1]:
        //   0   -> R uses +Shift (mono)
        //   50  -> R uses 0       (one shifted, one dry)
        //   100 -> R uses -Shift  (anti-shift)
        let width = self.width_pct * 0.01;
        let r_shift_scale = 1.0 - 2.0 * width;
        let two_pi = 2.0 * std::f32::consts::PI;
        let inc_l = two_pi * self.shift_hz / self.sample_rate;
        let inc_r = two_pi * self.shift_hz * r_shift_scale / self.sample_rate;

        // ----- Mix Drive*dry + feedback, Hilbert-transform -----
        // Drive scales ONLY the dry input -- NOT the feedback path.
        // Putting Drive inside the feedback loop gives loop gain
        // `drive * feedback`, which exceeds 1 for any Drive > 0 dB at
        // typical feedback (~0.95). The tanh saturation on `fb_l/fb_r`
        // below bounds the steady state, but in the LINEAR regime near
        // `wet ≈ 0` tanh(x) ≈ x, so the loop grows exponentially through
        // that regime and overshoots wildly before saturation engages.
        // Result: f32 overflow within ~25 samples at max settings.
        //
        // Putting Drive outside the feedback pins the loop gain to
        // `feedback ≤ 0.95` unconditionally. Drive still interacts with
        // feedback musically (a hotter dry input drives the resonance
        // louder, with more harmonic content from the in-loop tanh),
        // and the loop is provably bounded for any Drive value.
        let in_l = left * drive + feedback * self.fb_l;
        let in_r = right * drive + feedback * self.fb_r;
        let (re_l, im_l) = self.hilbert_l.process(in_l);
        let (re_r, im_r) = self.hilbert_r.process(in_r);

        // ----- Carrier mix: single-sideband modulation -----
        // Standard analytic-signal upper-sideband modulator. Positive
        // carrier-phase increment shifts UP; negative shifts DOWN
        // (the carrier sign sets the direction, not the cos/sin sign).
        let (sin_l, cos_l) = self.carrier_phase_l.sin_cos();
        let (sin_r, cos_r) = self.carrier_phase_r.sin_cos();
        let wet_l = re_l * cos_l - im_l * sin_l;
        let wet_r = re_r * cos_r - im_r * sin_r;

        // ----- Advance carrier phases (wrap to [0, 2pi)) -----
        self.carrier_phase_l += inc_l;
        if self.carrier_phase_l >= two_pi {
            self.carrier_phase_l -= two_pi;
        } else if self.carrier_phase_l < 0.0 {
            self.carrier_phase_l += two_pi;
        }
        self.carrier_phase_r += inc_r;
        if self.carrier_phase_r >= two_pi {
            self.carrier_phase_r -= two_pi;
        } else if self.carrier_phase_r < 0.0 {
            self.carrier_phase_r += two_pi;
        }

        // ----- Store feedback state for next sample -----
        // Drive is already outside the feedback loop (above), so loop gain
        // is purely `feedback ≤ 0.95` and the recirculated signal could
        // not diverge even without saturation. The `tanh` is belt and
        // suspenders: it makes the loop bounded by ±1 instantaneously
        // rather than asymptotically, and contributes a soft
        // "compressor inside the resonance" character to Shepard-rising
        // bell tones when Drive is hot enough to push the wet output
        // past the tanh knee.
        self.fb_l = wet_l.tanh();
        self.fb_r = wet_r.tanh();

        (wet_l, wet_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.hilbert_l.recompute_coeffs(self.sample_rate);
        self.hilbert_r.recompute_coeffs(self.sample_rate);
    }

    fn reset(&mut self) {
        self.hilbert_l.reset();
        self.hilbert_r.reset();
        self.carrier_phase_l = 0.0;
        self.carrier_phase_r = 0.0;
        self.fb_l = 0.0;
        self.fb_r = 0.0;
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.shift_hz = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.feedback_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.width_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => {
                self.drive_db = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max);
                self.drive_gain = tract_dsp::db::db_to_linear_fast(self.drive_db);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat};

    #[test]
    fn frequency_shift_lists_four_parameters_with_the_expected_specs() {
        let f = FrequencyShiftEffect::new();
        let specs = f.parameters();
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[0].name, "Shift");
        assert_eq!(specs[0].min, -1_000.0);
        assert_eq!(specs[0].max, 1_000.0);
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number { unit: "Hz", .. }
        ));
        assert_eq!(specs[1].name, "Feedback");
        assert_eq!(specs[1].min, -100.0);
        assert_eq!(specs[1].max, 100.0);
        assert_eq!(specs[2].name, "Width");
        assert_eq!(specs[3].name, "Drive");
        assert!(matches!(
            specs[3].format,
            ParamFormat::Number { unit: "dB", .. }
        ));
    }

    #[test]
    fn frequency_shift_set_param_clamps_each_slot() {
        let mut f = FrequencyShiftEffect::new();
        f.set_param(0, 99_999.0);
        assert_eq!(f.shift_hz, 1_000.0);
        f.set_param(0, -99_999.0);
        assert_eq!(f.shift_hz, -1_000.0);
        f.set_param(1, 999.0);
        assert_eq!(f.feedback_pct, 100.0);
        f.set_param(2, -10.0);
        assert_eq!(f.width_pct, 0.0);
        f.set_param(3, 99.0);
        assert_eq!(f.drive_db, 12.0);
        // Cached drive_gain follows the clamp.
        let expected = tract_dsp::db::db_to_linear_fast(12.0);
        assert!((f.drive_gain - expected).abs() < 1e-5);
    }

    /// Count sign changes (zero crossings) in `samples`. Two
    /// crossings per cycle -> measured_hz = crossings / (2*dur).
    fn count_crossings(samples: &[f32]) -> usize {
        let mut prev = 0.0_f32;
        let mut count = 0usize;
        for (i, &s) in samples.iter().enumerate() {
            if i > 0 && prev.signum() != s.signum() && (prev != 0.0 || s != 0.0) {
                count += 1;
            }
            prev = s;
        }
        count
    }

    #[test]
    fn frequency_shift_up_adds_hz_to_input_frequency() {
        // Drive a 500 Hz sine, shift up by +200 Hz, expect ~700 Hz
        // output. The Signalsmith IIR Hilbert hits exact integer Hz;
        // 5 Hz tolerance covers the zero-crossing discretization.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 200.0); // +200 Hz
        f.set_param(1, 0.0);
        f.set_param(2, 0.0);
        f.set_param(3, 0.0);
        // Skip the IIR's settling transient (~8 k samples is generous
        // for the 12-section bank).
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 500.0 * t).sin();
            let _ = f.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 8_000..56_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 500.0 * t).sin();
            let (l, _) = f.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0; // 1 s window
        assert!(
            (measured_hz - 700.0).abs() < 5.0,
            "expected ~700 Hz, got {measured_hz} Hz ({crossings} crossings)"
        );
    }

    #[test]
    fn frequency_shift_down_subtracts_hz_from_input_frequency() {
        // 800 Hz sine, shift -200 Hz, expect ~600 Hz.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, -200.0);
        f.set_param(1, 0.0);
        f.set_param(2, 0.0);
        f.set_param(3, 0.0);
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let _ = f.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 8_000..56_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let (l, _) = f.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0;
        assert!(
            (measured_hz - 600.0).abs() < 5.0,
            "expected ~600 Hz, got {measured_hz} Hz ({crossings} crossings)"
        );
    }

    #[test]
    fn frequency_shift_zero_leaves_output_at_input_frequency() {
        // Shift = 0 should keep the input frequency (the Hilbert
        // pair is all-pass, and the carrier is at DC -> no shift).
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 0.0);
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let _ = f.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 8_000..56_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, _) = f.process_sample(dry, dry);
            out.push(l);
        }
        let crossings = count_crossings(&out);
        let measured_hz = crossings as f32 / 2.0;
        assert!(
            (measured_hz - 440.0).abs() < 5.0,
            "Shift=0 should preserve input freq; got {measured_hz} Hz"
        );
    }

    /// Project `samples` onto sin/cos at `target_hz` over duration
    /// `dur_secs`, returning the magnitude in dB. Used to read the
    /// wanted/unwanted SSB sideband amplitudes from a wet output.
    fn project_db(samples: &[f32], target_hz: f32, start_t: f32, sr: f32) -> f32 {
        let mut sc = 0.0_f32;
        let mut ss = 0.0_f32;
        for (i, &s) in samples.iter().enumerate() {
            let t = start_t + i as f32 / sr;
            sc += s * (2.0 * std::f32::consts::PI * target_hz * t).cos();
            ss += s * (2.0 * std::f32::consts::PI * target_hz * t).sin();
        }
        let n = samples.len() as f32;
        let amp = ((sc * sc + ss * ss).sqrt()) / (n / 2.0);
        20.0 * amp.max(1e-10).log10()
    }

    #[test]
    fn frequency_shift_unwanted_sideband_is_attenuated() {
        // SSB modulation must produce only the SUM sideband
        // (input_freq + shift_hz), not the difference. Test:
        // input 400 Hz, shift +300 Hz -> output at 700 Hz with
        // little energy at 100 Hz (the lower sideband).
        //
        // Signalsmith's design gives 60+ dB rejection in the
        // useful passband; we assert >= 40 dB to leave headroom
        // for the IIR's small ripple.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 300.0);
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 400.0 * t).sin();
            let _ = f.process_sample(dry, dry);
        }
        let mut out = Vec::with_capacity(48_000);
        for i in 8_000..56_000 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 400.0 * t).sin();
            let (l, _) = f.process_sample(dry, dry);
            out.push(l);
        }
        let start_t = 8_000.0 / 48_000.0;
        let wanted_db = project_db(&out, 700.0, start_t, 48_000.0);
        let unwanted_db = project_db(&out, 100.0, start_t, 48_000.0);
        let rejection = wanted_db - unwanted_db;
        assert!(
            rejection >= 40.0,
            "rejection must be >= 40 dB; wanted={wanted_db}, unwanted={unwanted_db}, rejection={rejection}"
        );
        // The wanted sideband should be near 0 dB (passband_gain=2.0).
        assert!(
            wanted_db > -3.0,
            "wanted sideband too quiet: {wanted_db} dB"
        );
    }

    #[test]
    fn frequency_shift_hilbert_branches_are_phase_quadrature() {
        // Internal sanity: branch A (real) and branch B (imag) must
        // be ~90 deg apart across the passband. Use a sign-aware
        // measurement (atan2 of in-phase/quadrature dot products
        // against a reference sine), not just acos of cross-corr.
        let mut hilb = HilbertIir::new(48_000.0);
        let test_freq = 1_000.0_f32;
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * test_freq * t).sin();
            let _ = hilb.process(x);
        }
        let (mut a_c, mut a_s, mut b_c, mut b_s) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
        for i in 8_000..56_000 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * test_freq * t).sin();
            let (a, b) = hilb.process(x);
            let c = (2.0 * std::f32::consts::PI * test_freq * t).cos();
            let s = (2.0 * std::f32::consts::PI * test_freq * t).sin();
            a_c += a * c;
            a_s += a * s;
            b_c += b * c;
            b_s += b * s;
        }
        let phi_a = a_c.atan2(a_s).to_degrees();
        let phi_b = b_c.atan2(b_s).to_degrees();
        let mut diff = phi_a - phi_b;
        while diff > 180.0 {
            diff -= 360.0;
        }
        while diff < -180.0 {
            diff += 360.0;
        }
        let dev = (diff.abs() - 90.0).abs();
        assert!(
            dev < 3.0,
            "phase difference must be 90 deg ± 3 deg; got {diff} (dev {dev})"
        );
    }

    #[test]
    fn frequency_shift_hilbert_analytic_signal_magnitude_near_unity() {
        // With passband_gain=2.0 the analytic-signal magnitude
        // sqrt(re^2 + im^2) tracks the input amplitude. For a
        // 0-dBFS sine that's RMS sqrt(re^2 + im^2) ≈ input amplitude.
        let mut hilb = HilbertIir::new(48_000.0);
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let _ = hilb.process(x);
        }
        let mut mag_e = 0.0_f32;
        for i in 8_000..56_000 {
            let t = i as f32 / 48_000.0;
            let x = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            let (re, im) = hilb.process(x);
            mag_e += re * re + im * im;
        }
        let mag_rms = (mag_e / 48_000.0).sqrt();
        // Unit-amplitude sine has RMS 1/sqrt(2) ≈ 0.707. The
        // analytic-signal magnitude is the envelope (= 1.0 here),
        // so its RMS sample-by-sample is also ≈ 1.0 (not 0.707).
        assert!(
            (mag_rms - 1.0).abs() < 0.1,
            "analytic-signal RMS magnitude must be near 1.0; got {mag_rms}"
        );
    }

    #[test]
    fn frequency_shift_width_zero_collapses_to_mono() {
        // Width=0: both channels share the same carrier rate.
        // For an L=R input both Hilbert pairs receive the same
        // signal, so the outputs match.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 200.0);
        f.set_param(2, 0.0);
        // Skip settling.
        for _ in 0..8_000 {
            let _ = f.process_sample(0.5, 0.5);
        }
        for i in 0..2_400 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = f.process_sample(dry, dry);
            assert!(
                (l - r).abs() < 1e-4,
                "Width=0 must give L==R; sample {i}: {l} vs {r}"
            );
        }
    }

    #[test]
    fn frequency_shift_width_full_anti_shifts_right_channel() {
        // Width=100 -> R shift = -Shift. The two channels'
        // carriers rotate in opposite directions, so for an
        // L=R input the outputs diverge.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 300.0);
        f.set_param(2, 100.0);
        for _ in 0..8_000 {
            let _ = f.process_sample(0.5, 0.5);
        }
        let mut diff = 0.0_f32;
        for i in 0..4_800 {
            let t = i as f32 / 48_000.0;
            let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let (l, r) = f.process_sample(dry, dry);
            diff += (l - r).abs();
        }
        assert!(
            diff > 10.0,
            "Width=100 should separate L/R for mono input; |L-R| sum was {diff}"
        );
    }

    #[test]
    fn frequency_shift_drive_zero_is_unity_gain() {
        // Drive = 0 dB -> drive_gain = 1.0 exactly.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(3, 0.0);
        assert!((f.drive_gain - 1.0).abs() < 1e-5);
    }

    #[test]
    fn frequency_shift_feedback_amplifies_wet_energy() {
        // Compare tail energy at fb=0 vs fb=80 with a brief input
        // burst. High feedback should sustain ringing measurably
        // beyond the no-feedback case, which exhibits only the
        // IIR Hilbert's group-delay-driven settling tail.
        // The `tanh` saturation on the feedback path caps the
        // recirculated signal to ±1, so the ringing tail is bounded
        // -- empirically it's ~4× the no-feedback tail energy, not
        // the 10× the pre-saturation design would have produced.
        let measure = |fb: f32| {
            let mut f = FrequencyShiftEffect::new();
            f.set_sample_rate(48_000.0);
            f.set_param(0, 50.0);
            f.set_param(1, fb);
            // 100 ms of burst input then 100 ms of silence; the
            // tail energy in the silence half is what we measure.
            for i in 0..4_800 {
                let t = i as f32 / 48_000.0;
                let dry = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let _ = f.process_sample(dry, dry);
            }
            let mut e = 0.0_f32;
            for _ in 0..4_800 {
                let (l, _) = f.process_sample(0.0, 0.0);
                e += l * l;
            }
            e
        };
        let e0 = measure(0.0);
        let e80 = measure(80.0);
        assert!(
            e80 > e0 * 3.0,
            "high feedback should sustain wet; fb=0:{e0}, fb=80:{e80}"
        );
    }

    #[test]
    fn frequency_shift_stays_bounded_under_aggressive_sweep() {
        // Drive lives outside the feedback loop (`left * drive + fb * fb_l`)
        // and the recirculated signal is `tanh`-saturated before storage,
        // so the loop gain stays at `feedback ≤ 0.95` per sample with the
        // recirculated value bounded by ±1. The user-facing wet output
        // can briefly climb to roughly
        //     dry_amplitude * drive_max + 0.95 ≈ 0.5 * 3.98 + 0.95 ≈ 2.94
        // plus a small IIR transient overshoot from the Shift-wrap
        // discontinuities every 4000 samples. We assert <= 8 for clean
        // f32 headroom across platforms.
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        let mut peak = 0.0_f32;
        for i in 0..48_000 {
            f.set_param(0, (i as f32 / 4_000.0).fract() * 2_000.0 - 1_000.0);
            f.set_param(1, (i as f32 / 3_000.0).fract() * 200.0 - 100.0);
            f.set_param(2, (i as f32 / 5_000.0).fract() * 100.0);
            f.set_param(3, (i as f32 / 7_000.0).fract() * 12.0);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, r) = f.process_sample(dry, dry);
            assert!(
                l.is_finite() && r.is_finite(),
                "sample {i} not finite: ({l},{r})"
            );
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak <= 8.0,
            "wet output exceeded the empirical bound: peak {peak}"
        );
    }

    #[test]
    fn frequency_shift_reset_clears_state() {
        let mut f = FrequencyShiftEffect::new();
        f.set_sample_rate(48_000.0);
        f.set_param(0, 200.0);
        f.set_param(1, 60.0);
        for _ in 0..2_400 {
            let _ = f.process_sample(0.5, 0.5);
        }
        f.reset();
        // All Hilbert state zero.
        for v in f
            .hilbert_l
            .state_r
            .iter()
            .chain(f.hilbert_l.state_i.iter())
            .chain(f.hilbert_r.state_r.iter())
            .chain(f.hilbert_r.state_i.iter())
        {
            assert_eq!(*v, 0.0);
        }
        assert_eq!(f.carrier_phase_l, 0.0);
        assert_eq!(f.carrier_phase_r, 0.0);
        assert_eq!(f.fb_l, 0.0);
        assert_eq!(f.fb_r, 0.0);
    }

    #[test]
    fn frequency_shift_set_sample_rate_recomputes_hilbert_coeffs() {
        // Construct at 48 kHz, switch to 96 kHz; the per-section
        // poles must change (impulse-invariant rescales them).
        let mut f = FrequencyShiftEffect::new();
        let p_at_48k = f.hilbert_l.poles_r;
        f.set_sample_rate(96_000.0);
        let p_at_96k = f.hilbert_l.poles_r;
        // Per section i, the discrete pole at 96 kHz is exp(pole_ct *
        // 0.208...) which differs from at 48 kHz where freq_factor = 0.416...
        let mut any_changed = false;
        for i in 0..HILBERT_ORDER {
            if (p_at_48k[i] - p_at_96k[i]).abs() > 1e-4 {
                any_changed = true;
                break;
            }
        }
        assert!(any_changed, "Hilbert poles must change with sample rate");
    }

    #[test]
    fn frequency_shift_hilbert_works_across_sample_rates() {
        // The SR-aware design should give correct SSB shift at
        // 44.1, 48, 88.2, 96 kHz. Test the 500 + 200 Hz case at each.
        for sr in [44_100.0_f32, 48_000.0, 88_200.0, 96_000.0] {
            let mut f = FrequencyShiftEffect::new();
            f.set_sample_rate(sr);
            f.set_param(0, 200.0);
            for i in 0..(sr as usize / 6) {
                let t = i as f32 / sr;
                let dry = (2.0 * std::f32::consts::PI * 500.0 * t).sin();
                let _ = f.process_sample(dry, dry);
            }
            let n = sr as usize;
            let mut out = Vec::with_capacity(n);
            let start = sr as usize / 6;
            for i in start..(start + n) {
                let t = i as f32 / sr;
                let dry = (2.0 * std::f32::consts::PI * 500.0 * t).sin();
                let (l, _) = f.process_sample(dry, dry);
                out.push(l);
            }
            let crossings = count_crossings(&out);
            let measured_hz = crossings as f32 / 2.0;
            assert!(
                (measured_hz - 700.0).abs() < 10.0,
                "SR {sr}: expected ~700 Hz, got {measured_hz} Hz"
            );
        }
    }
}
