use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// A Schroeder–Moorer "Freeverb"-style algorithmic reverb. 8 LP-comb
/// filters in parallel feed 4 series allpass diffusers; the comb
/// feedback paths include a one-pole lowpass for HF damping (Moorer's
/// extension to Schroeder's original 1962 design). Jezar Wakefield's
/// 1999 reference C++ implementation provided the canonical delay
/// lengths used here (scaled from 44.1 kHz to the active sample rate).
///
/// Multosis additions on top of plain Freeverb:
///
/// - **Per-comb LFO modulation.** Each of the 8 comb tap reads has
///   its own sub-Hz LFO at a mutually-prime rate; the LFOs add up
///   to ±MOD_DEPTH samples of fractional offset on the read tap.
///   At Mod = 0 this collapses to plain Freeverb (LFOs idle); at
///   Mod = 100 the tail picks up the gentle pitch shimmer that
///   gives plates and the better hall algorithms their liveliness.
/// - **Pre-delay** before the reverb input, useful for separating
///   the dry hit from the tail onset.
/// - **Width** continuously blends between mono (0 %) and Jezar's
///   standard 23-sample L/R offsets (100 %).
///
/// **Latency:** zero (no FFT). **Per-sample work:** ~150 MAC/stereo.
///
/// Buffers are sized at construction for the worst case (192 kHz +
/// max modulation depth); `set_sample_rate` only recomputes which
/// portion of each buffer is read/written. No allocations on the
/// audio thread.
pub struct ReverbEffect {
    decay_pct: f32,
    damping_pct: f32,
    mod_pct: f32,
    pre_delay_ms: f32,
    width_pct: f32,
    sample_rate: f32,

    /// 8 comb filters per channel. Each holds its own ring buffer +
    /// LP damping state. Right-channel delays are offset by
    /// `STEREO_SPREAD_44K` samples (scaled to SR) for Jezar's
    /// Freeverb stereo spread.
    comb_l: [CombLine; Self::N_COMBS],
    comb_r: [CombLine; Self::N_COMBS],

    /// 4 allpass diffusers per channel, run in series after the
    /// comb sum.
    ap_l: [AllpassLine; Self::N_APS],
    ap_r: [AllpassLine; Self::N_APS],

    /// Pre-delay ring buffers — sized for 100 ms at 192 kHz.
    pre_buf_l: Vec<f32>,
    pre_buf_r: Vec<f32>,
    pre_write: usize,

    /// Per-comb LFO phase accumulators in radians. The same phase
    /// drives both channels' comb at index `i`, so the modulation
    /// is correlated between L and R — uncorrelated LFOs would
    /// flatten the stereo image we just bought with `width`.
    lfo_phase: [f32; Self::N_COMBS],
    /// Per-comb LFO phase increment per sample (radians).
    /// Recomputed in `set_sample_rate`.
    lfo_inc: [f32; Self::N_COMBS],
}

/// One Schroeder–Moorer comb filter: a delay line with an LP-filtered
/// feedback path. The LP coefficient is the `Damping` parameter and
/// is shared across all 8 combs.
struct CombLine {
    buf: Vec<f32>,
    write_idx: usize,
    /// Center tap delay in samples at the current SR (with no
    /// modulation offset added). The LFO add a fractional offset
    /// to this on read.
    delay_samples: f32,
    /// One-pole LP state in the feedback path.
    lp_state: f32,
}

/// One Schroeder allpass: `y = -g·x + buf[tap]`, `buf_write = x + g·y`,
/// where `g = AP_FEEDBACK`. Magnitude-flat at every frequency; only
/// the phase varies, which diffuses the comb output's echo pattern.
struct AllpassLine {
    buf: Vec<f32>,
    write_idx: usize,
    delay_samples: f32,
}

impl ReverbEffect {
    const N_COMBS: usize = 8;
    const N_APS: usize = 4;

    /// Reference delay values from Jezar Wakefield's Freeverb (left
    /// channel, in samples at 44.1 kHz).
    const COMB_DELAYS_L_44K: [usize; Self::N_COMBS] =
        [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    /// Right-channel stereo spread (samples added to each L delay at
    /// 44.1 kHz). Jezar's canonical 23 samples.
    const STEREO_SPREAD_44K: usize = 23;
    /// Allpass delay constants (left; right adds STEREO_SPREAD_44K).
    const AP_DELAYS_L_44K: [usize; Self::N_APS] = [556, 441, 341, 225];
    /// Fixed allpass feedback coefficient (Jezar's value).
    const AP_FEEDBACK: f32 = 0.5;

    /// Sample rate that the delay constants above are tuned for.
    const REF_SR: f32 = 44_100.0;
    /// Largest comb buffer the engine must size for. At 192 kHz the
    /// longest right-channel delay is `(1617 + 23) × 192 / 44.1 ≈
    /// 7141` samples; round up to the next power of two so the ring
    /// wrap in `read_frac` and the per-sample write-index advance
    /// become bitmasks instead of `%`.
    const MAX_COMB_LEN: usize = 8_192;
    /// Largest allpass buffer. `(556 + 23) × 192 / 44.1 ≈ 2522`;
    /// round up to the next power of two.
    const MAX_AP_LEN: usize = 4_096;
    /// Maximum pre-delay buffer length: 100 ms × 192 kHz = 19 200
    /// samples; round up to the next power of two.
    const MAX_PRE_DELAY: usize = 32_768;

    /// LFO modulation depth at Mod = 100 % (samples, one-sided).
    /// Small enough to not push the read tap close to the write
    /// head; large enough to be clearly audible as gentle pitch
    /// shimmer on long tails.
    const MOD_DEPTH_SAMPLES: f32 = 10.0;

    /// Per-comb LFO frequencies in Hz. Chosen as mutually-prime
    /// ratios so the 8 LFOs don't lock into a single rate — the
    /// resulting beating pattern is what makes the tail sound
    /// "alive" instead of like a single chorus.
    const LFO_FREQS_HZ: [f32; Self::N_COMBS] =
        [0.317, 0.421, 0.547, 0.671, 0.797, 0.911, 1.063, 1.237];

    /// Decay parameter (0..100 %) maps to the comb feedback gain
    /// in this range. The lower bound keeps even a "small room"
    /// audible (RT60 ≈ 0.4 s); the upper bound is below 1.0 to
    /// stay stable on top of the LP damping.
    const FB_MIN: f32 = 0.65;
    const FB_MAX: f32 = 0.98;

    /// Damping parameter (0..100 %) maps to this LP coefficient
    /// range. 0.5 is Jezar's canonical maximum; further values
    /// over-damp the tail into uselessness.
    const DAMP_MAX: f32 = 0.5;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Decay",
            min: 0.0,
            max: 100.0,
            default: 50.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Damping",
            min: 0.0,
            max: 100.0,
            default: 30.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Mod",
            min: 0.0,
            max: 100.0,
            default: 20.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Pre-delay",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 1,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Width",
            min: 0.0,
            max: 100.0,
            default: 100.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
    ];

    pub fn new() -> Self {
        let new_comb = || CombLine {
            buf: vec![0.0; Self::MAX_COMB_LEN],
            write_idx: 0,
            delay_samples: 0.0,
            lp_state: 0.0,
        };
        let new_ap = || AllpassLine {
            buf: vec![0.0; Self::MAX_AP_LEN],
            write_idx: 0,
            delay_samples: 0.0,
        };

        let mut me = Self {
            decay_pct: Self::PARAMS[0].default,
            damping_pct: Self::PARAMS[1].default,
            mod_pct: Self::PARAMS[2].default,
            pre_delay_ms: Self::PARAMS[3].default,
            width_pct: Self::PARAMS[4].default,
            sample_rate: 48_000.0,
            comb_l: std::array::from_fn(|_| new_comb()),
            comb_r: std::array::from_fn(|_| new_comb()),
            ap_l: std::array::from_fn(|_| new_ap()),
            ap_r: std::array::from_fn(|_| new_ap()),
            pre_buf_l: vec![0.0; Self::MAX_PRE_DELAY],
            pre_buf_r: vec![0.0; Self::MAX_PRE_DELAY],
            pre_write: 0,
            lfo_phase: [0.0; Self::N_COMBS],
            lfo_inc: [0.0; Self::N_COMBS],
        };
        me.set_sample_rate(me.sample_rate);
        me
    }

    /// Read a ring buffer at fractional `delay_samples` back from
    /// `write_idx`, linearly interpolating between adjacent slots.
    /// `delay_samples` must satisfy `2 ≤ delay_samples < buf.len()`
    /// — the per-channel `delay_samples + mod_offset` math
    /// guarantees this in practice (min delay ≈ 1116 samples at
    /// 44.1 kHz, modulation ≤ 10 samples).
    ///
    /// `buf.len()` must be a power of two; the wrap is a bitmask
    /// (`MAX_COMB_LEN`, `MAX_AP_LEN`, `MAX_PRE_DELAY` are all pow2).
    #[inline]
    fn read_frac(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let mask = n - 1;
        let pos = write_idx as f32 + n as f32 - delay_samples;
        let i_floor = pos.floor();
        let frac = pos - i_floor;
        let i0 = (i_floor as usize) & mask;
        let i1 = (i0 + 1) & mask;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }
}

impl Default for ReverbEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for ReverbEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Map parameters to coefficients per sample. set_param has
        // already clamped each pct into [0, 100], so the arithmetic
        // below stays well-defined.
        let feedback = Self::FB_MIN + (self.decay_pct * 0.01) * (Self::FB_MAX - Self::FB_MIN);
        let damping = (self.damping_pct * 0.01) * Self::DAMP_MAX;
        let mod_depth = (self.mod_pct * 0.01) * Self::MOD_DEPTH_SAMPLES;
        let pre_samples = (self.pre_delay_ms * 0.001 * self.sample_rate)
            .clamp(0.0, (Self::MAX_PRE_DELAY - 2) as f32);
        let width = self.width_pct * 0.01;

        // ----- Pre-delay ----------------------------------------
        // Write current input, read delayed input.
        self.pre_buf_l[self.pre_write] = left;
        self.pre_buf_r[self.pre_write] = right;
        // `pre_samples + 1` so that pre_delay=0 still reads the
        // PREVIOUS sample (the one we just wrote was for the next
        // pass). This keeps the engine's per-sample latency consistent
        // regardless of pre-delay setting.
        let in_l = Self::read_frac(&self.pre_buf_l, self.pre_write, pre_samples + 1.0);
        let in_r = Self::read_frac(&self.pre_buf_r, self.pre_write, pre_samples + 1.0);
        self.pre_write = (self.pre_write + 1) & (Self::MAX_PRE_DELAY - 1);

        // ----- Bus: mono input into the reverb engine --------------
        // Standard Freeverb feeds (L + R) / 2 into every comb. The
        // stereo separation comes from the right-channel delay offsets,
        // not from carrying separate L/R signal paths into the combs.
        let input = (in_l + in_r) * 0.5;

        // ----- Advance per-comb LFOs once per sample ---------------
        let two_pi = 2.0 * std::f32::consts::PI;
        for i in 0..Self::N_COMBS {
            self.lfo_phase[i] += self.lfo_inc[i];
            if self.lfo_phase[i] > two_pi {
                self.lfo_phase[i] -= two_pi;
            }
        }

        // ----- Parallel combs (per channel) ------------------------
        let mut sum_l = 0.0_f32;
        let mut sum_r = 0.0_f32;
        for i in 0..Self::N_COMBS {
            // One sine call per comb per sample = 8 × SR per second.
            // At 192 kHz that's ~1.5M sine/sec — still cheap.
            let mod_offset = mod_depth * self.lfo_phase[i].sin();

            // Left comb -------------------------------------------
            let delay_l = (self.comb_l[i].delay_samples + mod_offset).max(2.0);
            let tap_l = Self::read_frac(&self.comb_l[i].buf, self.comb_l[i].write_idx, delay_l);
            // LP damping in feedback path: y = (1-d)·x + d·y_prev.
            self.comb_l[i].lp_state = (1.0 - damping) * tap_l + damping * self.comb_l[i].lp_state;
            self.comb_l[i].buf[self.comb_l[i].write_idx] =
                input + self.comb_l[i].lp_state * feedback;
            self.comb_l[i].write_idx =
                (self.comb_l[i].write_idx + 1) & (self.comb_l[i].buf.len() - 1);
            sum_l += tap_l;

            // Right comb (stereo-spread delay) ---------------------
            let delay_r = (self.comb_r[i].delay_samples + mod_offset).max(2.0);
            let tap_r = Self::read_frac(&self.comb_r[i].buf, self.comb_r[i].write_idx, delay_r);
            self.comb_r[i].lp_state = (1.0 - damping) * tap_r + damping * self.comb_r[i].lp_state;
            self.comb_r[i].buf[self.comb_r[i].write_idx] =
                input + self.comb_r[i].lp_state * feedback;
            self.comb_r[i].write_idx =
                (self.comb_r[i].write_idx + 1) & (self.comb_r[i].buf.len() - 1);
            sum_r += tap_r;
        }

        // ----- Series allpass diffusion (per channel) --------------
        let mut y_l = sum_l;
        let mut y_r = sum_r;
        for i in 0..Self::N_APS {
            // y_out = -g·y_in + tap; buf_write = y_in + g·y_out.
            let tap_l = Self::read_frac(
                &self.ap_l[i].buf,
                self.ap_l[i].write_idx,
                self.ap_l[i].delay_samples,
            );
            let new_l = -Self::AP_FEEDBACK * y_l + tap_l;
            self.ap_l[i].buf[self.ap_l[i].write_idx] = y_l + Self::AP_FEEDBACK * new_l;
            self.ap_l[i].write_idx =
                (self.ap_l[i].write_idx + 1) & (self.ap_l[i].buf.len() - 1);
            y_l = new_l;

            let tap_r = Self::read_frac(
                &self.ap_r[i].buf,
                self.ap_r[i].write_idx,
                self.ap_r[i].delay_samples,
            );
            let new_r = -Self::AP_FEEDBACK * y_r + tap_r;
            self.ap_r[i].buf[self.ap_r[i].write_idx] = y_r + Self::AP_FEEDBACK * new_r;
            self.ap_r[i].write_idx =
                (self.ap_r[i].write_idx + 1) & (self.ap_r[i].buf.len() - 1);
            y_r = new_r;
        }

        // ----- Output mixing ---------------------------------------
        // Scale the 8-comb sum so wet level lands near unity for a
        // ~unity-amplitude impulse train at default Decay.
        let scale = 1.0 / Self::N_COMBS as f32;
        let wet_l = y_l * scale;
        let wet_r = y_r * scale;

        // Width blend: at 0 % both channels read the mid (mono);
        // at 100 % they pass through with full Jezar stereo spread.
        let mid = (wet_l + wet_r) * 0.5;
        let out_l = mid + width * (wet_l - mid);
        let out_r = mid + width * (wet_r - mid);

        (out_l, out_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        let scale = sr / Self::REF_SR;
        let two_pi = 2.0 * std::f32::consts::PI;

        for i in 0..Self::N_COMBS {
            self.comb_l[i].delay_samples = Self::COMB_DELAYS_L_44K[i] as f32 * scale;
            self.comb_r[i].delay_samples =
                (Self::COMB_DELAYS_L_44K[i] + Self::STEREO_SPREAD_44K) as f32 * scale;
            self.lfo_inc[i] = two_pi * Self::LFO_FREQS_HZ[i] / sr;
        }
        for i in 0..Self::N_APS {
            self.ap_l[i].delay_samples = Self::AP_DELAYS_L_44K[i] as f32 * scale;
            self.ap_r[i].delay_samples =
                (Self::AP_DELAYS_L_44K[i] + Self::STEREO_SPREAD_44K) as f32 * scale;
        }
    }

    fn reset(&mut self) {
        for c in self.comb_l.iter_mut().chain(self.comb_r.iter_mut()) {
            for s in c.buf.iter_mut() {
                *s = 0.0;
            }
            c.write_idx = 0;
            c.lp_state = 0.0;
        }
        for a in self.ap_l.iter_mut().chain(self.ap_r.iter_mut()) {
            for s in a.buf.iter_mut() {
                *s = 0.0;
            }
            a.write_idx = 0;
        }
        for s in self.pre_buf_l.iter_mut() {
            *s = 0.0;
        }
        for s in self.pre_buf_r.iter_mut() {
            *s = 0.0;
        }
        self.pre_write = 0;
        for p in self.lfo_phase.iter_mut() {
            *p = 0.0;
        }
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.decay_pct = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.damping_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.mod_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.pre_delay_ms = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.width_pct = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, ParamFormat};

    #[test]
    fn reverb_lists_five_parameters_with_the_expected_specs() {
        let r = ReverbEffect::new();
        let specs = r.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Decay");
        assert_eq!(specs[0].min, 0.0);
        assert_eq!(specs[0].max, 100.0);
        assert_eq!(specs[1].name, "Damping");
        assert_eq!(specs[2].name, "Mod");
        assert_eq!(specs[3].name, "Pre-delay");
        assert!(matches!(
            specs[3].format,
            ParamFormat::Number { unit: "ms", .. }
        ));
        assert_eq!(specs[4].name, "Width");
        assert_eq!(specs[4].default, 100.0);
    }

    #[test]
    fn reverb_set_param_clamps_each_slot() {
        let mut r = ReverbEffect::new();
        r.set_param(0, 999.0);
        assert_eq!(r.decay_pct, 100.0);
        r.set_param(0, -10.0);
        assert_eq!(r.decay_pct, 0.0);
        r.set_param(1, 999.0);
        assert_eq!(r.damping_pct, 100.0);
        r.set_param(2, -1.0);
        assert_eq!(r.mod_pct, 0.0);
        r.set_param(3, 999.0);
        assert_eq!(r.pre_delay_ms, 100.0);
        r.set_param(4, 200.0);
        assert_eq!(r.width_pct, 100.0);
    }

    #[test]
    fn reverb_silent_input_eventually_decays_to_silence() {
        // Reverb with silent input must decay (no perpetual ringdown).
        // After ~5 s the output should be essentially zero.
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 50.0);
        for _ in 0..240_000 {
            let _ = r.process_sample(0.0, 0.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..1_024 {
            let (l, ri) = r.process_sample(0.0, 0.0);
            peak = peak.max(l.abs()).max(ri.abs());
        }
        assert!(
            peak < 1e-3,
            "silent input → silent output after RT60; peak={peak}"
        );
    }

    #[test]
    fn reverb_impulse_produces_decaying_tail() {
        // A unit impulse should produce a sustained nonzero output —
        // not a single echo, a tail.
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 60.0);
        let _ = r.process_sample(1.0, 1.0);
        let mut energy = 0.0_f32;
        for _ in 0..48_000 {
            let (l, _) = r.process_sample(0.0, 0.0);
            energy += l * l;
        }
        assert!(
            energy > 0.001,
            "impulse should produce nonzero tail energy, got {energy}"
        );
    }

    #[test]
    fn reverb_higher_decay_produces_longer_tail() {
        // Compare tail energy 100..200 ms after an impulse, at low vs
        // high Decay. The high-Decay instance should sustain
        // substantially more energy in the late window.
        let test = |decay: f32| {
            let mut r = ReverbEffect::new();
            r.set_sample_rate(48_000.0);
            r.set_param(0, decay);
            let _ = r.process_sample(1.0, 1.0);
            for _ in 0..4_800 {
                let _ = r.process_sample(0.0, 0.0);
            }
            let mut e = 0.0_f32;
            for _ in 0..4_800 {
                let (l, _) = r.process_sample(0.0, 0.0);
                e += l * l;
            }
            e
        };
        let e_low = test(10.0);
        let e_high = test(90.0);
        // 2× rather than 5× because the 100..200 ms measurement
        // window still catches early comb echoes at low Decay; the
        // ratio only opens up after the echo pattern settles.
        assert!(
            e_high > e_low * 2.0,
            "high decay should have more tail energy; low={e_low}, high={e_high}"
        );
    }

    #[test]
    fn reverb_damping_reduces_high_frequency_content() {
        // High damping should lower the HF content of the reverb
        // tail. Use sample-to-sample absolute difference as a
        // rough HF proxy — a brighter signal has larger ‖d/dt‖.
        let test = |damping: f32| {
            let mut r = ReverbEffect::new();
            r.set_sample_rate(48_000.0);
            r.set_param(0, 70.0);
            r.set_param(1, damping);
            r.set_param(2, 0.0); // No LFO modulation — keep test deterministic
                                 // Warm up with pseudo-noise input
            let mut prng: u32 = 1;
            for _ in 0..24_000 {
                prng = prng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let n = (prng as i32 as f32) / (i32::MAX as f32);
                let _ = r.process_sample(n * 0.3, n * 0.3);
            }
            let mut prev = 0.0_f32;
            let mut deriv = 0.0_f32;
            for _ in 0..24_000 {
                prng = prng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let n = (prng as i32 as f32) / (i32::MAX as f32);
                let (l, _) = r.process_sample(n * 0.3, n * 0.3);
                deriv += (l - prev).abs();
                prev = l;
            }
            deriv
        };
        let bright = test(0.0);
        let dark = test(100.0);
        assert!(
            dark < bright,
            "high damping should reduce HF content; bright={bright}, dark={dark}"
        );
    }

    #[test]
    fn reverb_width_zero_collapses_to_mono() {
        // Width=0 forces both outputs to the mid signal — exactly L==R.
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(4, 0.0);
        for i in 0..8_000 {
            let t = i as f32 / 48_000.0;
            // Asymmetric input so L≠R going in.
            let in_l = (2.0 * std::f32::consts::PI * 400.0 * t).sin();
            let in_r = (2.0 * std::f32::consts::PI * 800.0 * t).sin();
            let (out_l, out_r) = r.process_sample(in_l, in_r);
            assert!(
                (out_l - out_r).abs() < 1e-5,
                "Width=0 must give L==R; sample {i}: {out_l} vs {out_r}"
            );
        }
    }

    #[test]
    fn reverb_width_full_separates_l_and_r() {
        // Width=100% with the same input on both channels: the
        // L/R delay-spread should split the output.
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 60.0);
        r.set_param(2, 0.0); // Disable LFOs so we're not measuring chorus
        r.set_param(4, 100.0);
        for _ in 0..4_800 {
            let _ = r.process_sample(0.5, 0.5);
        }
        let mut diff = 0.0_f32;
        for _ in 0..4_800 {
            let (l, ri) = r.process_sample(0.5, 0.5);
            diff += (l - ri).abs();
        }
        assert!(
            diff > 0.1,
            "Width=100% should separate L/R; total |L-R| was {diff}"
        );
    }

    #[test]
    fn reverb_mod_zero_is_deterministic() {
        // With Mod=0 the LFOs are still ticking but their offset
        // contribution is multiplied by zero, so the output must
        // match across two runs from the same start state.
        let run = || {
            let mut r = ReverbEffect::new();
            r.set_sample_rate(48_000.0);
            r.set_param(0, 50.0);
            r.set_param(2, 0.0);
            let mut out = Vec::with_capacity(2_000);
            for i in 0..2_000 {
                let t = i as f32 / 48_000.0;
                let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let (l, _) = r.process_sample(s, s);
                out.push(l);
            }
            out
        };
        let a = run();
        let b = run();
        for i in 0..a.len() {
            assert!(
                (a[i] - b[i]).abs() < 1e-6,
                "Mod=0 must be deterministic; sample {i}: {} vs {}",
                a[i],
                b[i]
            );
        }
    }

    #[test]
    fn reverb_pre_delay_holds_off_the_tail() {
        // With 50 ms pre-delay and a single impulse, the first few
        // ms of the output should still be silence (we haven't yet
        // released the impulse into the reverb engine).
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 70.0);
        r.set_param(2, 0.0);
        r.set_param(3, 50.0);
        let _ = r.process_sample(1.0, 1.0);
        // Check 0.5 ms (24 samples) — well before the 50 ms pre-delay.
        for i in 0..24 {
            let (l, _) = r.process_sample(0.0, 0.0);
            assert!(
                l.abs() < 1e-4,
                "sample {i} should be inside pre-delay silence; got {l}"
            );
        }
    }

    #[test]
    fn reverb_stays_bounded_under_aggressive_sweep() {
        // Sweep every parameter wildly while a sustained tone plays.
        // Output must stay finite and within a reasonable bound.
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        for i in 0..48_000 {
            r.set_param(0, (i as f32 / 4_096.0).fract() * 100.0);
            r.set_param(1, (i as f32 / 5_000.0).fract() * 100.0);
            r.set_param(2, (i as f32 / 3_000.0).fract() * 100.0);
            r.set_param(4, (i as f32 / 7_000.0).fract() * 100.0);
            let dry = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let (l, ri) = r.process_sample(dry, dry);
            assert!(l.is_finite() && ri.is_finite(), "sample {i} not finite");
            assert!(
                l.abs() < 16.0 && ri.abs() < 16.0,
                "sample {i} blew up: ({l},{ri})"
            );
        }
    }

    #[test]
    fn reverb_reset_clears_state() {
        let mut r = ReverbEffect::new();
        r.set_sample_rate(48_000.0);
        r.set_param(0, 70.0);
        for _ in 0..4_800 {
            let _ = r.process_sample(0.5, 0.5);
        }
        r.reset();
        for c in r.comb_l.iter().chain(r.comb_r.iter()) {
            assert!(c.buf.iter().all(|&v| v == 0.0));
            assert_eq!(c.lp_state, 0.0);
            assert_eq!(c.write_idx, 0);
        }
        for a in r.ap_l.iter().chain(r.ap_r.iter()) {
            assert!(a.buf.iter().all(|&v| v == 0.0));
            assert_eq!(a.write_idx, 0);
        }
        assert!(r.pre_buf_l.iter().all(|&v| v == 0.0));
        assert!(r.pre_buf_r.iter().all(|&v| v == 0.0));
        assert_eq!(r.pre_write, 0);
        assert!(r.lfo_phase.iter().all(|&p| p == 0.0));
    }
}
