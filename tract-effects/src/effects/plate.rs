use super::{Effect, ParamFormat, ParamScaling, ParamSpec};

/// Dattorro 1997 plate reverb. Distinct from the existing
/// Schroeder-Moorer `Reverb`: pre-delay -> bandwidth LP -> 4 series
/// allpass diffusers -> cross-coupled figure-8 tank with modulated
/// allpass + damping LP per branch -> 7-tap weighted output mix.
/// Bright, dense, no early reflections -- the classic EMT 140 / Lexicon
/// 224 character.
///
/// Port of the Valley Audio Plateau / WeirdConstructor synfx-dsp Rust
/// implementation, both of which faithfully implement Dattorro's
/// "Effect Design Part 1: Reverberator and Other Filters" (JAES 1997).
/// Tap times and allpass coefficients are taken from Dattorro's paper
/// at his canonical 29761 Hz sample rate and scaled to the current SR
/// at `set_sample_rate` time -- the magic numbers are what define the
/// sound, so we preserve them exactly.
///
/// Per-channel state in this implementation: 4 input-side diffuser
/// allpass rings (mono, shared across channels), pre-delay ring, two
/// tank branches with [modulated APF + tank delay + damping LP + fixed
/// APF + tank delay], plus the cross-coupling sum. Output DC blocker
/// strips the offset that asymmetric tap weighting can accumulate.
///
/// Buffers are sized for 192 kHz so `set_sample_rate` only re-caches
/// integer offsets without allocating. The struct is boxed inside
/// `EffectInstance` because the total delay-line state weighs in around
/// 1 MB.
pub struct PlateEffect {
    pre_delay_ms: f32,
    decay_pct: f32,
    damping_pct: f32,
    bandwidth_hz: f32,
    width_pct: f32,
    sample_rate: f32,

    // Cached derived coefficients (per recompute).
    /// Number of samples to read back from `pre_delay` for the pre-delay tap.
    pre_delay_samples: f32,
    /// Internal tank decay (0.1..0.95). Bounded strictly < 1 so the
    /// cross-coupled tank stays stable.
    decay: f32,
    /// Second tank APF coefficient -- Dattorro's derived rule
    /// `clamp(decay + 0.15, 0.25, 0.50)`.
    decay_diff_2: f32,
    /// One-pole LP coefficient for the input bandwidth filter.
    bw_coef: f32,
    /// One-pole LP coefficient for the in-tank damping filters.
    damp_coef: f32,
    /// LFO peak excursion in samples at the current sample rate.
    lfo_excursion: f32,
    /// Sample-rate-scaled delay tap counts (constant per SR; rescaled
    /// on `set_sample_rate`).
    input_apf_len: [f32; 4],
    tank_apf1_len: [f32; 2],
    tank_apf2_len: [f32; 2],
    tank_delay1_len: [f32; 2],
    tank_delay2_len: [f32; 2],
    left_tap_offsets: [f32; 7],
    right_tap_offsets: [f32; 7],
    /// LFO phase advance per sample (Hz / sr).
    lfo_inc: f32,

    // Per-channel filter state.
    input_lp_s: f32,
    damp_lp_s: [f32; 2],

    // LFO phase in `[0, 1)`.
    lfo_phase: f32,

    // Cross-coupling sum.
    left_sum: f32,
    right_sum: f32,

    // Pre-delay ring.
    pre_delay: Vec<f32>,
    pre_delay_pos: usize,

    // Input diffuser allpass rings.
    input_apf_bufs: [Vec<f32>; 4],
    input_apf_pos: [usize; 4],

    // Tank allpass rings (modulated and fixed).
    tank_apf1_bufs: [Vec<f32>; 2],
    tank_apf1_pos: [usize; 2],
    tank_apf2_bufs: [Vec<f32>; 2],
    tank_apf2_pos: [usize; 2],

    // Tank fixed delays.
    tank_delay1_bufs: [Vec<f32>; 2],
    tank_delay1_pos: [usize; 2],
    tank_delay2_bufs: [Vec<f32>; 2],
    tank_delay2_pos: [usize; 2],

    // Output DC blocker state.
    dc_in: [f32; 2],
    dc_out: [f32; 2],
    /// One-pole HP pole for the output DC blocker (`y = x - x_n1 + R*y_n1`).
    dc_r: f32,
}

/// Dattorro's reference sample rate. All tap counts below are at this rate;
/// scale by `sr / DAT_SR` before use.
const DAT_SR: f32 = 29_761.0;

/// Input diffuser tap counts (`DAT_SR` reference): 4 series allpasses
/// before the tank.
const INPUT_APF_TAPS: [f32; 4] = [142.0, 107.0, 379.0, 277.0];
/// Input diffuser coefficients (first two `0.75`, last two `0.625` per
/// Dattorro Table 1).
const INPUT_APF_GAINS: [f32; 4] = [0.75, 0.75, 0.625, 0.625];

/// Tank modulated-APF tap counts at `DAT_SR` -- the per-branch first
/// allpass that gets LFO-modulated. L and R are different lengths --
/// the asymmetry is what gives the plate its stereo image.
const TANK_APF1_TAPS: [f32; 2] = [672.0, 908.0];
/// Tank fixed-APF tap counts at `DAT_SR`.
const TANK_APF2_TAPS: [f32; 2] = [1800.0, 2656.0];
/// Tank first fixed-delay counts at `DAT_SR`.
const TANK_DELAY1_TAPS: [f32; 2] = [4453.0, 4217.0];
/// Tank second fixed-delay counts at `DAT_SR`.
const TANK_DELAY2_TAPS: [f32; 2] = [3720.0, 3163.0];

/// Output tap offsets for the left channel sum (from Dattorro Table 2,
/// `DAT_SR` reference). Indices into [delay1L, delay1L, apf2L, delay2L,
/// delay1R, apf2R, delay2R]; the cross-channel taps are what create
/// the stereo image.
const LEFT_TAP_OFFSETS: [f32; 7] = [266.0, 2974.0, 1913.0, 1996.0, 1990.0, 187.0, 1066.0];
/// Output tap offsets for the right channel sum.
const RIGHT_TAP_OFFSETS: [f32; 7] = [353.0, 3627.0, 1228.0, 2673.0, 2111.0, 335.0, 121.0];

/// Modulated-APF coefficient for tank (sign included -- negative per
/// Dattorro Fig. 1's "note sign" annotation, load-bearing for the swirl
/// character).
const DECAY_DIFF_1: f32 = -0.7;
/// LFO peak excursion in samples at `DAT_SR` (Dattorro Table 1: `EXCURSION = 16`).
const LFO_EXCURSION_SAMPLES_AT_DAT_SR: f32 = 16.0;
/// LFO rate. The paper says "on the order of 1 Hz"; quadrature L/R via
/// sin/cos derived from a single phase accumulator.
const LFO_RATE_HZ: f32 = 1.0;

/// Buffer capacity used for every delay ring. Sized for 192 kHz with
/// headroom so `set_sample_rate` never has to reallocate. Pre-delay
/// needs the most space (200 ms * 192 kHz ≈ 38 400, round up).
const RING_CAP: usize = 65_536;
const RING_MASK: usize = RING_CAP - 1;

impl PlateEffect {
    /// Strict upper bound on Decay -- below 1 by enough headroom that
    /// sample-rate-scaled rounding can't push the loop unstable.
    const DECAY_CAP: f32 = 0.95;
    /// Lower bound on Decay -- below this the tail dies in under 100 ms
    /// and the effect feels broken.
    const DECAY_FLOOR: f32 = 0.10;

    const PARAMS: [ParamSpec; 5] = [
        ParamSpec {
            name: "Pre-Delay",
            min: 0.0,
            max: 200.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "ms",
            },
        },
        ParamSpec {
            name: "Decay",
            min: 10.0,
            max: 95.0,
            default: 60.0,
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
            default: 40.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: "%",
            },
        },
        ParamSpec {
            name: "Bandwidth",
            min: 200.0,
            max: 20_000.0,
            default: 10_000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
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
        let mut e = Self {
            pre_delay_ms: Self::PARAMS[0].default,
            decay_pct: Self::PARAMS[1].default,
            damping_pct: Self::PARAMS[2].default,
            bandwidth_hz: Self::PARAMS[3].default,
            width_pct: Self::PARAMS[4].default,
            sample_rate: 48_000.0,

            pre_delay_samples: 0.0,
            decay: 0.0,
            decay_diff_2: 0.0,
            bw_coef: 0.0,
            damp_coef: 0.0,
            lfo_excursion: 0.0,
            input_apf_len: [0.0; 4],
            tank_apf1_len: [0.0; 2],
            tank_apf2_len: [0.0; 2],
            tank_delay1_len: [0.0; 2],
            tank_delay2_len: [0.0; 2],
            left_tap_offsets: [0.0; 7],
            right_tap_offsets: [0.0; 7],
            lfo_inc: 0.0,

            input_lp_s: 0.0,
            damp_lp_s: [0.0; 2],

            lfo_phase: 0.0,

            left_sum: 0.0,
            right_sum: 0.0,

            pre_delay: vec![0.0; RING_CAP],
            pre_delay_pos: 0,

            input_apf_bufs: [
                vec![0.0; RING_CAP],
                vec![0.0; RING_CAP],
                vec![0.0; RING_CAP],
                vec![0.0; RING_CAP],
            ],
            input_apf_pos: [0; 4],

            tank_apf1_bufs: [vec![0.0; RING_CAP], vec![0.0; RING_CAP]],
            tank_apf1_pos: [0; 2],
            tank_apf2_bufs: [vec![0.0; RING_CAP], vec![0.0; RING_CAP]],
            tank_apf2_pos: [0; 2],
            tank_delay1_bufs: [vec![0.0; RING_CAP], vec![0.0; RING_CAP]],
            tank_delay1_pos: [0; 2],
            tank_delay2_bufs: [vec![0.0; RING_CAP], vec![0.0; RING_CAP]],
            tank_delay2_pos: [0; 2],

            dc_in: [0.0; 2],
            dc_out: [0.0; 2],
            dc_r: 0.0,
        };
        e.recompute();
        e
    }

    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let scale = sr / DAT_SR;

        self.pre_delay_samples = (self.pre_delay_ms * 0.001 * sr).clamp(0.0, (RING_CAP - 4) as f32);

        let decay_norm = (self.decay_pct * 0.01).clamp(0.0, 1.0);
        self.decay = Self::DECAY_FLOOR + decay_norm * (Self::DECAY_CAP - Self::DECAY_FLOOR);
        // Dattorro's derived rule keeps the second tank APF coefficient
        // tied to the user's Decay setting, ramping its diffusion as
        // the tail lengthens.
        self.decay_diff_2 = (self.decay + 0.15).clamp(0.25, 0.50);

        // One-pole LP coefficient `c = 1 - exp(-2*pi*fc/sr)`. At low
        // damping the cutoff stays near Nyquist (no HF loss); at high
        // damping it drops into the 1-2 kHz range for a dark tail.
        let damp_norm = (self.damping_pct * 0.01).clamp(0.0, 1.0);
        let damp_fc = 20_000.0 * (1.0 - damp_norm).powi(2) + 200.0 * damp_norm;
        self.damp_coef = 1.0 - (-std::f32::consts::TAU * damp_fc / sr).exp();

        let bw_fc = self.bandwidth_hz.clamp(200.0, 20_000.0).min(sr * 0.49);
        self.bw_coef = 1.0 - (-std::f32::consts::TAU * bw_fc / sr).exp();

        self.lfo_excursion = LFO_EXCURSION_SAMPLES_AT_DAT_SR * scale;
        self.lfo_inc = LFO_RATE_HZ / sr;

        for (i, &tap) in INPUT_APF_TAPS.iter().enumerate() {
            self.input_apf_len[i] = tap * scale;
        }
        for i in 0..2 {
            self.tank_apf1_len[i] = TANK_APF1_TAPS[i] * scale;
            self.tank_apf2_len[i] = TANK_APF2_TAPS[i] * scale;
            self.tank_delay1_len[i] = TANK_DELAY1_TAPS[i] * scale;
            self.tank_delay2_len[i] = TANK_DELAY2_TAPS[i] * scale;
        }
        for i in 0..7 {
            self.left_tap_offsets[i] = LEFT_TAP_OFFSETS[i] * scale;
            self.right_tap_offsets[i] = RIGHT_TAP_OFFSETS[i] * scale;
        }

        // Output DC blocker pole at 10 Hz.
        let dc_fc = 10.0;
        self.dc_r = (1.0 - std::f32::consts::TAU * dc_fc / sr).clamp(0.0, 1.0);
    }

    /// Fractional read from a ring buffer at `offset` samples behind
    /// the write head. Linear interpolation between the two nearest
    /// integer offsets. `RING_MASK` bitmask wrap relies on the power-
    /// of-two capacity.
    #[inline]
    fn read_frac(buf: &[f32], write_pos: usize, offset: f32) -> f32 {
        let offset = offset.max(1.0);
        let int_off = offset.floor() as usize;
        let frac = offset - int_off as f32;
        let i0 = (write_pos + RING_CAP - int_off) & RING_MASK;
        let i1 = (write_pos + RING_CAP - int_off - 1) & RING_MASK;
        buf[i0] * (1.0 - frac) + buf[i1] * frac
    }

    /// Integer read for output taps where sub-sample precision doesn't
    /// matter.
    #[inline]
    fn read_int(buf: &[f32], write_pos: usize, offset: usize) -> f32 {
        buf[(write_pos + RING_CAP - offset) & RING_MASK]
    }

    /// Write one sample to a ring buffer and advance the write head.
    #[inline]
    fn write_ring(buf: &mut [f32], pos: &mut usize, sample: f32) {
        buf[*pos] = sample;
        *pos = (*pos + 1) & RING_MASK;
    }

    /// One allpass tick. `out = -g*x + buf[delay]`, then write
    /// `x + g*out` into the ring. Standard Dattorro form.
    #[inline]
    fn allpass_step(buf: &mut [f32], pos: &mut usize, x: f32, delay_samples: f32, g: f32) -> f32 {
        let delayed = Self::read_frac(buf, *pos, delay_samples);
        let out = -g * x + delayed;
        Self::write_ring(buf, pos, x + g * out);
        out
    }

    /// One-pole LP step: `s += coef * (in - s); return s`.
    #[inline]
    fn one_pole_lp(state: &mut f32, input: f32, coef: f32) -> f32 {
        *state += coef * (input - *state);
        *state
    }
}

impl Default for PlateEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for PlateEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        // 1. Sum input channels into the diffuser input and apply the
        //    pre-delay ring.
        let in_sum = (left + right) * 0.5;
        Self::write_ring(&mut self.pre_delay, &mut self.pre_delay_pos, in_sum);
        let pre_delayed = if self.pre_delay_samples < 1.0 {
            in_sum
        } else {
            Self::read_frac(&self.pre_delay, self.pre_delay_pos, self.pre_delay_samples)
        };

        // 2. Bandwidth filter (1-pole LP).
        let bandwidth_out = Self::one_pole_lp(&mut self.input_lp_s, pre_delayed, self.bw_coef);

        // 3. Four series allpass diffusers.
        let mut x = bandwidth_out;
        for (i, &g) in INPUT_APF_GAINS.iter().enumerate() {
            let (buf, pos) = (&mut self.input_apf_bufs[i], &mut self.input_apf_pos[i]);
            x = Self::allpass_step(buf, pos, x, self.input_apf_len[i], g);
        }
        let tank_input = x;

        // 4. LFO quadrature: a single phase accumulator drives both L
        //    and R tank-APF1 modulations 90 degrees apart, matching
        //    Dattorro's two-LFO-in-quadrature recommendation.
        let phase = self.lfo_phase * std::f32::consts::TAU;
        let lfo_l = phase.sin() * self.lfo_excursion;
        let lfo_r = phase.cos() * self.lfo_excursion;
        self.lfo_phase += self.lfo_inc;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= self.lfo_phase.floor();
        }

        // 5. Left tank branch. Input is the diffuser output PLUS the
        //    right branch's previous output (cross-coupling). The
        //    branch ordering matters -- read the previous-sample
        //    cross-coupling sums BEFORE updating any state.
        let left_in = tank_input + self.right_sum;
        let l = Self::allpass_step(
            &mut self.tank_apf1_bufs[0],
            &mut self.tank_apf1_pos[0],
            left_in,
            self.tank_apf1_len[0] + lfo_l,
            DECAY_DIFF_1,
        );
        Self::write_ring(
            &mut self.tank_delay1_bufs[0],
            &mut self.tank_delay1_pos[0],
            l,
        );
        let l = Self::read_frac(
            &self.tank_delay1_bufs[0],
            self.tank_delay1_pos[0],
            self.tank_delay1_len[0],
        );
        let l = Self::one_pole_lp(&mut self.damp_lp_s[0], l, self.damp_coef);
        let l = l * self.decay;
        let l = Self::allpass_step(
            &mut self.tank_apf2_bufs[0],
            &mut self.tank_apf2_pos[0],
            l,
            self.tank_apf2_len[0],
            self.decay_diff_2,
        );
        Self::write_ring(
            &mut self.tank_delay2_bufs[0],
            &mut self.tank_delay2_pos[0],
            l,
        );
        let l_branch = Self::read_frac(
            &self.tank_delay2_bufs[0],
            self.tank_delay2_pos[0],
            self.tank_delay2_len[0],
        );

        // 6. Right tank branch, symmetric. Reads the LEFT branch's
        //    previous cross-coupling sum (already captured in
        //    `self.left_sum` from the last process_sample call).
        let right_in = tank_input + self.left_sum;
        let r = Self::allpass_step(
            &mut self.tank_apf1_bufs[1],
            &mut self.tank_apf1_pos[1],
            right_in,
            self.tank_apf1_len[1] + lfo_r,
            DECAY_DIFF_1,
        );
        Self::write_ring(
            &mut self.tank_delay1_bufs[1],
            &mut self.tank_delay1_pos[1],
            r,
        );
        let r = Self::read_frac(
            &self.tank_delay1_bufs[1],
            self.tank_delay1_pos[1],
            self.tank_delay1_len[1],
        );
        let r = Self::one_pole_lp(&mut self.damp_lp_s[1], r, self.damp_coef);
        let r = r * self.decay;
        let r = Self::allpass_step(
            &mut self.tank_apf2_bufs[1],
            &mut self.tank_apf2_pos[1],
            r,
            self.tank_apf2_len[1],
            self.decay_diff_2,
        );
        Self::write_ring(
            &mut self.tank_delay2_bufs[1],
            &mut self.tank_delay2_pos[1],
            r,
        );
        let r_branch = Self::read_frac(
            &self.tank_delay2_bufs[1],
            self.tank_delay2_pos[1],
            self.tank_delay2_len[1],
        );

        // 7. Cross-couple for next sample.
        self.left_sum = r_branch * self.decay;
        self.right_sum = l_branch * self.decay;

        // 8. Output taps -- 7 weighted reads per channel, alternating
        //    in-branch and cross-branch to synthesize the stereo image.
        //    Indices match Dattorro Table 2:
        //      L = +d1L[0] + d1L[1] - apf2L[2] + d2L[3]
        //          - d1R[4] - apf2R[5] - d2R[6]
        //      R = +d1R[0] + d1R[1] - apf2R[2] + d2R[3]
        //          - d1L[4] - apf2L[5] - d2L[6]
        let yl = Self::read_int(
            &self.tank_delay1_bufs[0],
            self.tank_delay1_pos[0],
            self.left_tap_offsets[0] as usize,
        ) + Self::read_int(
            &self.tank_delay1_bufs[0],
            self.tank_delay1_pos[0],
            self.left_tap_offsets[1] as usize,
        ) - Self::read_int(
            &self.tank_apf2_bufs[0],
            self.tank_apf2_pos[0],
            self.left_tap_offsets[2] as usize,
        ) + Self::read_int(
            &self.tank_delay2_bufs[0],
            self.tank_delay2_pos[0],
            self.left_tap_offsets[3] as usize,
        ) - Self::read_int(
            &self.tank_delay1_bufs[1],
            self.tank_delay1_pos[1],
            self.left_tap_offsets[4] as usize,
        ) - Self::read_int(
            &self.tank_apf2_bufs[1],
            self.tank_apf2_pos[1],
            self.left_tap_offsets[5] as usize,
        ) - Self::read_int(
            &self.tank_delay2_bufs[1],
            self.tank_delay2_pos[1],
            self.left_tap_offsets[6] as usize,
        );
        let yr = Self::read_int(
            &self.tank_delay1_bufs[1],
            self.tank_delay1_pos[1],
            self.right_tap_offsets[0] as usize,
        ) + Self::read_int(
            &self.tank_delay1_bufs[1],
            self.tank_delay1_pos[1],
            self.right_tap_offsets[1] as usize,
        ) - Self::read_int(
            &self.tank_apf2_bufs[1],
            self.tank_apf2_pos[1],
            self.right_tap_offsets[2] as usize,
        ) + Self::read_int(
            &self.tank_delay2_bufs[1],
            self.tank_delay2_pos[1],
            self.right_tap_offsets[3] as usize,
        ) - Self::read_int(
            &self.tank_delay1_bufs[0],
            self.tank_delay1_pos[0],
            self.right_tap_offsets[4] as usize,
        ) - Self::read_int(
            &self.tank_apf2_bufs[0],
            self.tank_apf2_pos[0],
            self.right_tap_offsets[5] as usize,
        ) - Self::read_int(
            &self.tank_delay2_bufs[0],
            self.tank_delay2_pos[0],
            self.right_tap_offsets[6] as usize,
        );

        // 9. Width: at 100% the L/R taps stay as-is (full plate stereo);
        //    at 0% they collapse to mono. Implemented as Mid/Side
        //    blending so center is preserved across all Width values.
        let mid = (yl + yr) * 0.5;
        let side = (yl - yr) * 0.5;
        let width = (self.width_pct * 0.01).clamp(0.0, 1.0);
        let yl = mid + width * side;
        let yr = mid - width * side;

        // 10. Tap weight: the 7-tap sum can grow large, particularly at
        //     high decay. Dattorro and Valley both scale by 0.6 per tap;
        //     baked in here as a single 0.5 trim that gives unity-ish
        //     output level on most program material.
        let yl = yl * 0.5;
        let yr = yr * 0.5;

        // 11. Output DC blocker. The asymmetric tap weighting can
        //     accumulate a small DC offset over time; a one-pole HP at
        //     10 Hz strips it without affecting the audible spectrum.
        let dc_l = yl - self.dc_in[0] + self.dc_r * self.dc_out[0];
        self.dc_in[0] = yl;
        self.dc_out[0] = dc_l;
        let dc_r = yr - self.dc_in[1] + self.dc_r * self.dc_out[1];
        self.dc_in[1] = yr;
        self.dc_out[1] = dc_r;

        (dc_l, dc_r)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.pre_delay.fill(0.0);
        self.pre_delay_pos = 0;
        for buf in self.input_apf_bufs.iter_mut() {
            buf.fill(0.0);
        }
        self.input_apf_pos = [0; 4];
        for buf in self.tank_apf1_bufs.iter_mut() {
            buf.fill(0.0);
        }
        for buf in self.tank_apf2_bufs.iter_mut() {
            buf.fill(0.0);
        }
        for buf in self.tank_delay1_bufs.iter_mut() {
            buf.fill(0.0);
        }
        for buf in self.tank_delay2_bufs.iter_mut() {
            buf.fill(0.0);
        }
        self.tank_apf1_pos = [0; 2];
        self.tank_apf2_pos = [0; 2];
        self.tank_delay1_pos = [0; 2];
        self.tank_delay2_pos = [0; 2];
        self.input_lp_s = 0.0;
        self.damp_lp_s = [0.0; 2];
        self.lfo_phase = 0.0;
        self.left_sum = 0.0;
        self.right_sum = 0.0;
        self.dc_in = [0.0; 2];
        self.dc_out = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.pre_delay_ms = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.decay_pct = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            2 => self.damping_pct = value.clamp(Self::PARAMS[2].min, Self::PARAMS[2].max),
            3 => self.bandwidth_hz = value.clamp(Self::PARAMS[3].min, Self::PARAMS[3].max),
            4 => self.width_pct = value.clamp(Self::PARAMS[4].min, Self::PARAMS[4].max),
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
        let e = PlateEffect::new();
        let specs = e.parameters();
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "Pre-Delay");
        assert_eq!(specs[1].name, "Decay");
        assert_eq!(specs[2].name, "Damping");
        assert_eq!(specs[3].name, "Bandwidth");
        assert_eq!(specs[4].name, "Width");
    }

    #[test]
    fn silent_input_stays_silent() {
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..16_384 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert!(l.abs() < 1e-9 && r.abs() < 1e-9, "non-silent: {l}, {r}");
        }
    }

    #[test]
    fn impulse_response_decays_to_silence() {
        // Drive a single impulse, then check the reverb tail decays
        // toward silence over a few seconds. Tail measured well past
        // the longest delay so we sample the actual decay envelope.
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 50.0);
        e.set_param(2, 40.0);
        // Impulse.
        e.process_sample(1.0, 1.0);
        // Drain past the longest tank delay (4453 samples at DAT_SR *
        // 1.613 ≈ 7180 at 48 kHz).
        for _ in 0..8192 {
            e.process_sample(0.0, 0.0);
        }
        let mut sum_early = 0.0_f32;
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            sum_early += l * l + r * r;
        }
        // Drain ~3 seconds.
        for _ in 0..(48_000 * 3) {
            e.process_sample(0.0, 0.0);
        }
        let mut sum_late = 0.0_f32;
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.0, 0.0);
            sum_late += l * l + r * r;
        }
        assert!(
            sum_late < sum_early * 0.1,
            "tail not decaying: early={sum_early}, late={sum_late}"
        );
    }

    #[test]
    fn impulse_response_produces_diffuse_tail() {
        // After the input has stopped, the reverb tail should NOT be
        // silent immediately -- the cascading allpasses + tank should
        // sustain energy for tens of milliseconds at minimum.
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 80.0);
        e.process_sample(1.0, 1.0);
        // Drain enough to be past the pre-delay + diffuser stack but
        // well inside the decay region.
        let mut had_signal = false;
        for _ in 0..48_000 {
            let (l, r) = e.process_sample(0.0, 0.0);
            if l.abs() > 1e-4 || r.abs() > 1e-4 {
                had_signal = true;
            }
        }
        assert!(had_signal, "no detectable reverb tail");
    }

    #[test]
    fn longer_decay_increases_tail_energy() {
        // At higher Decay the tail should retain more energy a fixed
        // time after the impulse.
        let measure_tail = |decay: f32| -> f32 {
            let mut e = PlateEffect::new();
            e.set_sample_rate(48_000.0);
            e.set_param(1, decay);
            e.set_param(2, 20.0);
            e.process_sample(1.0, 1.0);
            for _ in 0..24_000 {
                e.process_sample(0.0, 0.0);
            }
            let mut sum = 0.0_f32;
            for _ in 0..4096 {
                let (l, r) = e.process_sample(0.0, 0.0);
                sum += l * l + r * r;
            }
            (sum / 4096.0).sqrt()
        };
        let short = measure_tail(30.0);
        let long = measure_tail(90.0);
        assert!(
            long > short * 1.5,
            "Decay should grow tail: short={short}, long={long}"
        );
    }

    #[test]
    fn stable_under_max_decay() {
        // With Decay at the upper bound, sustained loud input must not
        // cause runaway accumulation. The DECAY_CAP < 1 contract is
        // what guarantees this.
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(1, 95.0);
        for i in 0..48_000 {
            let x = (i as f32 * 0.05).sin() * 0.5;
            let (l, r) = e.process_sample(x, x);
            assert!(
                l.is_finite() && r.is_finite(),
                "non-finite at i={i}: {l}, {r}"
            );
            assert!(
                l.abs() < 32.0 && r.abs() < 32.0,
                "runaway at i={i}: {l}, {r}"
            );
        }
    }

    #[test]
    fn width_zero_gives_mono_output() {
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(4, 0.0);
        // Drive with very different L/R to highlight stereo, then check
        // outputs match (mono collapse).
        for _ in 0..4096 {
            e.process_sample(1.0, -1.0);
        }
        let mut max_diff = 0.0_f32;
        for _ in 0..4096 {
            let (l, r) = e.process_sample(0.5, -0.5);
            max_diff = max_diff.max((l - r).abs());
        }
        assert!(
            max_diff < 1e-6,
            "Width=0 should collapse to mono, got max diff={max_diff}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        for _ in 0..1024 {
            e.process_sample(0.5, -0.5);
        }
        e.reset();
        // After reset, silent input must produce silent output for at
        // least a few samples (no stored state to bleed through).
        for _ in 0..64 {
            let (l, r) = e.process_sample(0.0, 0.0);
            assert_eq!(l, 0.0);
            assert_eq!(r, 0.0);
        }
    }

    #[test]
    fn set_param_out_of_range_is_ignored() {
        let mut e = PlateEffect::new();
        e.set_sample_rate(48_000.0);
        e.set_param(99, 1.0);
        let (l, _) = e.process_sample(0.25, 0.25);
        assert!(l.is_finite());
    }
}
