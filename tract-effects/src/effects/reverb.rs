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
///   At Mod = 0 the offset is zero while the LFOs keep running; at
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

    pub(super) const PARAMS: [ParamSpec; 5] = [
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
    /// `buf.len()` must be a power of two; the wrap is a bitmask
    /// (`MAX_COMB_LEN`, `MAX_AP_LEN`, `MAX_PRE_DELAY` are all pow2).
    #[inline]
    fn read_frac(buf: &[f32], write_idx: usize, delay_samples: f32) -> f32 {
        let n = buf.len();
        let mask = n - 1;
        let whole = delay_samples.floor();
        let frac = delay_samples - whole;
        // Keep wrapping in integer space so the fractional offset retains its precision.
        let newer = (write_idx + n - ((whole as usize) & mask)) & mask;
        let older = (newer + n - 1) & mask;
        buf[newer] * (1.0 - frac) + buf[older] * frac
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
            if self.lfo_phase[i] >= two_pi {
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
            self.ap_l[i].write_idx = (self.ap_l[i].write_idx + 1) & (self.ap_l[i].buf.len() - 1);
            y_l = new_l;

            let tap_r = Self::read_frac(
                &self.ap_r[i].buf,
                self.ap_r[i].write_idx,
                self.ap_r[i].delay_samples,
            );
            let new_r = -Self::AP_FEEDBACK * y_r + tap_r;
            self.ap_r[i].buf[self.ap_r[i].write_idx] = y_r + Self::AP_FEEDBACK * new_r;
            self.ap_r[i].write_idx = (self.ap_r[i].write_idx + 1) & (self.ap_r[i].buf.len() - 1);
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

    fn configured_reverb(sample_rate: f32, params: [f32; 5]) -> ReverbEffect {
        let mut reverb = ReverbEffect::new();
        reverb.set_sample_rate(sample_rate);
        for (index, value) in params.into_iter().enumerate() {
            reverb.set_param(index, value);
        }
        reverb
    }

    #[test]
    fn fractional_reads_preserve_subsample_precision_at_every_ring_position() {
        for length in [8, 4_096, 8_192, 32_768] {
            for write in [0, length / 2, length - 1] {
                for delay in [1.25, 2.0 + 1.0 / 65_536.0, length as f32 - 1.5] {
                    let mut buffer = vec![0.0; length];
                    let whole = delay.floor() as usize;
                    buffer[(write + length - whole) % length] = 0.75;
                    buffer[(write + length - whole - 1) % length] = -0.25;
                    let expected = 0.75 - (delay - whole as f32);
                    assert_eq!(
                        ReverbEffect::read_frac(&buffer, write, delay),
                        expected,
                        "length {length}, write {write}, delay {delay}"
                    );
                }
            }
        }
    }

    #[test]
    fn lfos_wrap_at_the_exact_cycle_boundary_even_when_modulation_is_muted() {
        let mut reverb = configured_reverb(96_000.0, [50.0, 30.0, 0.0, 0.0, 100.0]);
        for i in 0..8 {
            reverb.lfo_phase[i] = std::f32::consts::TAU - reverb.lfo_inc[i];
            assert_eq!(
                reverb.lfo_phase[i] + reverb.lfo_inc[i],
                std::f32::consts::TAU
            );
        }
        reverb.process_sample(0.0, 0.0);
        assert_eq!(reverb.lfo_phase, [0.0; 8]);
        reverb.process_sample(0.0, 0.0);
        assert_eq!(reverb.lfo_phase, reverb.lfo_inc);
        let expected: [f64; 8] = std::array::from_fn(|i| {
            reverb.lfo_phase[i] = std::f32::consts::TAU - reverb.lfo_inc[i] / 2.0;
            reverb.lfo_phase[i] as f64 + reverb.lfo_inc[i] as f64 - std::f32::consts::TAU as f64
        });
        reverb.process_sample(0.0, 0.0);
        for (phase, expected) in reverb.lfo_phase.into_iter().zip(expected) {
            assert!(phase > 0.0 && phase < std::f32::consts::TAU);
            assert!((phase as f64 - expected).abs() < 5e-7);
        }
    }

    #[test]
    fn long_pre_delays_respect_the_existing_storage_limit() {
        for (sample_rate, delay) in [(192_000.0, 19_200), (384_000.0, 32_766)] {
            let mut reverb = configured_reverb(sample_rate, [0.0, 0.0, 0.0, 100.0, 100.0]);
            let source = ReverbEffect::MAX_PRE_DELAY - delay - 1;
            reverb.pre_buf_l[source] = 0.75;
            reverb.pre_buf_r[source] = -0.25;
            reverb.process_sample(0.0, 0.0);
            for comb in reverb.comb_l.iter().chain(reverb.comb_r.iter()) {
                assert_eq!(comb.buf[0], 0.25, "sample rate {sample_rate}");
            }
        }
    }

    fn analytic_channel_impulse(length: usize, spread: usize, feedback: f64) -> Vec<f64> {
        let mut response = vec![0.0; length];
        for delay in [1_116, 1_188, 1_277, 1_356, 1_422, 1_491, 1_557, 1_617] {
            let delay = delay + spread;
            for echo in 1..length.div_ceil(delay) {
                response[echo * delay] += feedback.powi(echo as i32 - 1) / 8.0;
            }
        }
        for delay in [556, 441, 341, 225] {
            let delay = delay + spread;
            let mut diffused = vec![0.0; length];
            for (sample, &amplitude) in response.iter().enumerate() {
                // (z^-D - g)/(1 - g*z^-D): h[0]=-g, h[kD]=(1-g²)*g^(k-1).
                diffused[sample] -= 0.5 * amplitude;
                if amplitude == 0.0 {
                    continue;
                }
                for echo in 1..(length - sample).div_ceil(delay) {
                    diffused[sample + echo * delay] +=
                        amplitude * 0.75 * 0.5_f64.powi(echo as i32 - 1);
                }
            }
            response = diffused;
        }
        response
    }

    #[test]
    fn impulse_responses_match_independent_comb_and_allpass_transfer_functions() {
        for (decay, pre_delay, pre_samples, width) in
            [(0.0, 0.0, 0, 100.0), (100.0, 20.0, 882, 37.5)]
        {
            let mut reverb = configured_reverb(44_100.0, [decay, 0.0, 0.0, pre_delay, width]);
            let feedback = 0.65 + decay as f64 * 0.0033;
            let left = analytic_channel_impulse(9_000, 0, feedback);
            let right = analytic_channel_impulse(9_000, 23, feedback);
            for sample in 0..9_000 {
                let input = if sample == 0 {
                    (1.0, -0.25)
                } else {
                    (0.0, 0.0)
                };
                let (wet_l, wet_r) = if sample > pre_samples {
                    (
                        0.375 * left[sample - pre_samples - 1],
                        0.375 * right[sample - pre_samples - 1],
                    )
                } else {
                    (0.0, 0.0)
                };
                let side_gain = width as f64 / 100.0;
                let expected = (
                    ((1.0 + side_gain) * wet_l + (1.0 - side_gain) * wet_r) / 2.0,
                    ((1.0 - side_gain) * wet_l + (1.0 + side_gain) * wet_r) / 2.0,
                );
                let actual = reverb.process_sample(input.0, input.1);
                assert!(
                    (actual.0 as f64 - expected.0).abs() < 2e-6,
                    "decay {decay}, sample {sample}: {actual:?} vs {expected:?}"
                );
                assert!((actual.1 as f64 - expected.1).abs() < 2e-6);
            }
        }
    }

    #[test]
    fn seeded_comb_and_diffuser_states_follow_the_analytic_one_step_update() {
        for decay in [0.0, 50.0, 100.0] {
            for damping in [0.0, 50.0, 100.0] {
                for width in [0.0, 37.5, 100.0] {
                    let mut reverb = configured_reverb(44_100.0, [decay, damping, 0.0, 0.0, width]);
                    reverb.pre_write = ReverbEffect::MAX_PRE_DELAY - 1;
                    reverb.pre_buf_l[ReverbEffect::MAX_PRE_DELAY - 2] = 0.5;
                    reverb.pre_buf_r[ReverbEffect::MAX_PRE_DELAY - 2] = -0.25;
                    for i in 0..8 {
                        for (comb, delay, tap, state) in [
                            (&mut reverb.comb_l[i], i + 2, (i + 1) as f32 / 8.0, 0.25),
                            (
                                &mut reverb.comb_r[i],
                                i + 3,
                                -(i as f32 + 1.0) / 16.0,
                                -0.375,
                            ),
                        ] {
                            comb.delay_samples = delay as f32;
                            comb.write_idx = if i % 2 == 0 { 0 } else { comb.buf.len() - 1 };
                            let tap_index =
                                (comb.write_idx + comb.buf.len() - delay) % comb.buf.len();
                            comb.buf[tap_index] = tap;
                            comb.lp_state = state;
                        }
                    }
                    for allpass in reverb.ap_l.iter_mut().chain(reverb.ap_r.iter_mut()) {
                        allpass.write_idx = allpass.buf.len() - 1;
                    }
                    let actual = reverb.process_sample(0.75, -0.125);
                    let wet_l = 4.5 / 128.0;
                    let wet_r = -2.25 / 128.0;
                    let w = width as f64 / 100.0;
                    let expected = (
                        ((1.0 + w) * wet_l + (1.0 - w) * wet_r) / 2.0,
                        ((1.0 - w) * wet_l + (1.0 + w) * wet_r) / 2.0,
                    );
                    assert!((actual.0 as f64 - expected.0).abs() < 2e-7);
                    assert!((actual.1 as f64 - expected.1).abs() < 2e-7);
                    for i in 0..8 {
                        for (comb, tap, state) in [
                            (&reverb.comb_l[i], (i + 1) as f64 / 8.0, 0.25),
                            (&reverb.comb_r[i], -(i as f64 + 1.0) / 16.0, -0.375),
                        ] {
                            let d = damping as f64 / 200.0;
                            let lp = (1.0 - d) * tap + d * state;
                            let written = 0.125 + lp * (0.65 + decay as f64 * 0.0033);
                            let previous = if i % 2 == 0 { 0 } else { comb.buf.len() - 1 };
                            assert!((comb.lp_state as f64 - lp).abs() < 2e-7);
                            assert!((comb.buf[previous] as f64 - written).abs() < 2e-7);
                            assert_eq!(comb.write_idx, if i % 2 == 0 { 1 } else { 0 });
                        }
                    }
                    for i in 0..4 {
                        let factor = 0.75 * (-0.5_f64).powi(i as i32);
                        assert_eq!(
                            reverb.ap_l[i].buf[ReverbEffect::MAX_AP_LEN - 1] as f64,
                            4.5 * factor
                        );
                        assert_eq!(
                            reverb.ap_r[i].buf[ReverbEffect::MAX_AP_LEN - 1] as f64,
                            -2.25 * factor
                        );
                        assert_eq!(reverb.ap_l[i].write_idx, 0);
                        assert_eq!(reverb.ap_r[i].write_idx, 0);
                    }
                    assert_eq!(reverb.pre_write, 0);
                    assert_eq!(reverb.pre_buf_l[ReverbEffect::MAX_PRE_DELAY - 1], 0.75);
                    assert_eq!(reverb.pre_buf_r[ReverbEffect::MAX_PRE_DELAY - 1], -0.125);
                }
            }
        }
    }

    #[test]
    fn modulation_moves_each_stereo_comb_to_the_expected_fractional_taps() {
        for (phase, offset, sum_l, sum_r) in [
            (std::f32::consts::FRAC_PI_2, 2.5, 1.125, 0.6875),
            (3.0 * std::f32::consts::FRAC_PI_2, -2.5, 1.125, 0.6875),
            (std::f32::consts::PI / 6.0, 1.25, 2.8125, -0.34375),
        ] {
            let mut reverb = configured_reverb(44_100.0, [50.0, 50.0, 25.0, 0.0, 100.0]);
            for i in 0..8 {
                reverb.lfo_phase[i] = phase - reverb.lfo_inc[i];
                for (comb, newer, older) in [
                    (
                        &mut reverb.comb_l[i],
                        (i + 1) as f32 / 8.0,
                        -(i as f32 + 1.0) / 16.0,
                    ),
                    (
                        &mut reverb.comb_r[i],
                        -(i as f32 + 2.0) / 32.0,
                        (i + 2) as f32 / 16.0,
                    ),
                ] {
                    let whole = (comb.delay_samples + offset).floor() as usize;
                    let newer_index = comb.buf.len() - whole;
                    comb.buf[newer_index] = newer;
                    comb.buf[newer_index - 1] = older;
                }
            }
            let actual = reverb.process_sample(0.0, 0.0);
            assert!((actual.0 - sum_l / 128.0).abs() < 2e-7);
            assert!((actual.1 - sum_r / 128.0).abs() < 2e-7);
            for &actual_phase in &reverb.lfo_phase {
                assert_eq!(actual_phase, phase);
            }
        }
    }

    #[test]
    fn fractional_pre_delay_matches_fractional_input_delay_through_the_whole_engine() {
        let mut delayed = configured_reverb(48_000.0, [75.0, 60.0, 0.0, 1.5 / 48.0, 62.5]);
        let mut reference = configured_reverb(48_000.0, [75.0, 60.0, 0.0, 0.0, 62.5]);
        let mut previous = [(0.0, 0.0); 2];
        for sample in 0..12_000 {
            let input = if sample < 3_000 {
                (
                    (sample as f32 * 0.17).sin() * 0.25,
                    (sample as f32 * 0.31).cos() * 0.375,
                )
            } else {
                (0.0, 0.0)
            };
            let shifted = (
                0.5 * (previous[0].0 + previous[1].0),
                0.5 * (previous[0].1 + previous[1].1),
            );
            let actual = delayed.process_sample(input.0, input.1);
            let expected = reference.process_sample(shifted.0, shifted.1);
            assert!(
                (actual.0 - expected.0).abs() < 5e-7,
                "sample {sample}: {actual:?} vs {expected:?}"
            );
            assert!((actual.1 - expected.1).abs() < 5e-7);
            previous = [input, previous[0]];
        }
    }

    #[test]
    fn sample_rates_scale_all_delay_lengths_and_lfo_rates_without_resetting_phases() {
        let mut reverb = ReverbEffect::new();
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for (index, phase) in reverb.lfo_phase.iter_mut().enumerate() {
                *phase = index as f32 * 0.25;
            }
            reverb.set_sample_rate(sample_rate);
            for (i, delay) in [1_116, 1_188, 1_277, 1_356, 1_422, 1_491, 1_557, 1_617]
                .into_iter()
                .enumerate()
            {
                let left = delay as f64 * sample_rate as f64 / 44_100.0;
                let right = (delay + 23) as f64 * sample_rate as f64 / 44_100.0;
                assert!((reverb.comb_l[i].delay_samples as f64 - left).abs() < 0.002);
                assert!((reverb.comb_r[i].delay_samples as f64 - right).abs() < 0.002);
                let frequency = [0.317, 0.421, 0.547, 0.671, 0.797, 0.911, 1.063, 1.237][i];
                let increment = std::f64::consts::TAU * frequency / sample_rate as f64;
                assert!((reverb.lfo_inc[i] as f64 - increment).abs() < 2e-10);
                assert_eq!(reverb.lfo_phase[i], i as f32 * 0.25);
            }
            for (i, delay) in [556, 441, 341, 225].into_iter().enumerate() {
                assert!(
                    (reverb.ap_l[i].delay_samples as f64
                        - delay as f64 * sample_rate as f64 / 44_100.0)
                        .abs()
                        < 0.002
                );
                assert!(
                    (reverb.ap_r[i].delay_samples as f64
                        - (delay + 23) as f64 * sample_rate as f64 / 44_100.0)
                        .abs()
                        < 0.002
                );
            }
        }
        reverb.set_sample_rate(0.0);
        assert_eq!(reverb.sample_rate, 1.0);
    }

    #[test]
    fn parameter_and_sample_rate_changes_preserve_the_existing_reverb_tail() {
        let params = [73.0, 64.0, 45.0, 7.0, 62.5];
        let mut changed = configured_reverb(48_000.0, params);
        let mut reference = configured_reverb(48_000.0, params);
        for sample in 0..10_000 {
            let input = (
                (sample as f32 * 0.17).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.375,
            );
            changed.process_sample(input.0, input.1);
            reference.process_sample(input.0, input.1);
        }
        changed.set_sample_rate(96_000.0);
        for (index, value) in [0.0, 0.0, 0.0, 100.0, 0.0].into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_sample_rate(48_000.0);
        for (index, value) in params.into_iter().enumerate() {
            changed.set_param(index, value);
        }
        changed.set_param(99, 123.0);
        for _ in 0..10_000 {
            assert_eq!(
                changed.process_sample(0.0, 0.0),
                reference.process_sample(0.0, 0.0)
            );
        }
    }

    #[test]
    fn reset_replays_a_fresh_processor_with_the_current_parameters_and_rate() {
        let params = [73.0, 64.0, 45.0, 7.0, 62.5];
        let mut used = configured_reverb(96_000.0, params);
        let mut fresh = configured_reverb(96_000.0, params);
        for sample in 0..10_000 {
            used.process_sample((sample as f32 * 0.17).sin() * 0.25, -0.375);
        }
        used.reset();
        for sample in 0..10_000 {
            let input = if sample < 1_000 {
                (0.25, (sample as f32 * 0.31).cos() * 0.375)
            } else {
                (0.0, 0.0)
            };
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn every_parameter_clamps_to_its_endpoints_without_retriggering_the_tail() {
        for slot in 0..5 {
            for (value, endpoint) in [(-1.0, 0.0), (999.0, 100.0)] {
                let mut clamped = configured_reverb(48_000.0, [73.0, 64.0, 45.0, 7.0, 62.5]);
                let mut reference = configured_reverb(48_000.0, [73.0, 64.0, 45.0, 7.0, 62.5]);
                for sample in 0..2_000 {
                    let input = ((sample as f32 * 0.17).sin() * 0.25, -0.375);
                    clamped.process_sample(input.0, input.1);
                    reference.process_sample(input.0, input.1);
                }
                clamped.set_param(slot, value);
                reference.set_param(slot, endpoint);
                let stored = [
                    clamped.decay_pct,
                    clamped.damping_pct,
                    clamped.mod_pct,
                    clamped.pre_delay_ms,
                    clamped.width_pct,
                ];
                assert_eq!(stored[slot], endpoint);
                for _ in 0..6_000 {
                    assert_eq!(
                        clamped.process_sample(0.0, 0.0),
                        reference.process_sample(0.0, 0.0)
                    );
                }
            }
        }
    }

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
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                (0.0, 100.0, 50.0),
                (0.0, 100.0, 30.0),
                (0.0, 100.0, 20.0),
                (0.0, 100.0, 0.0),
                (0.0, 100.0, 100.0)
            ]
        );
        for (index, spec) in specs.iter().enumerate() {
            assert!(matches!(spec.scaling, ParamScaling::Linear));
            if index == 3 {
                assert!(matches!(
                    spec.format,
                    ParamFormat::Number {
                        decimals: 1,
                        unit: "ms"
                    }
                ));
            } else {
                assert!(matches!(
                    spec.format,
                    ParamFormat::Number {
                        decimals: 0,
                        unit: "%"
                    }
                ));
            }
        }
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
