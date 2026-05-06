//! 4-band Linkwitz-Riley IIR crossover with Lipshitz/Vanderkooy allpass compensation.
//!
//! Each "split" is an LR4 (24 dB/oct) implemented as a cascade of two LR2 biquads.
//! The split's LP and HP outputs sum to a 2nd-order allpass `AP` at the split frequency.
//!
//! Tree (splits cascaded into upper branch):
//!   input → split1(f1) → low_pre, mh_pre
//!           split2(f2 on mh_pre) → ml_pre, mhh_pre
//!           split3(f3 on mhh_pre) → mh, high
//!
//! Compensation:
//!   low      = AP3(AP2(LP1(input)))
//!   mid_low  = AP3(LP2(HP1(input)))
//!   mid_high = LP3(HP2(HP1(input)))
//!   high     = HP3(HP2(HP1(input)))
//!
//! Σ bands = AP3 ∘ AP2 ∘ AP1 (input)  — magnitude-flat, phase-distorted.
//!
//! Two variants live in this module:
//!  - `CrossoverIir`: Linkwitz-Riley + Lipshitz/Vanderkooy compensation, zero latency,
//!    magnitude-flat band sum.
//!  - `CrossoverFir`: linear-phase windowed-sinc with double-buffered taps + sample-wise
//!    crossfade on redesign. `length / 2` samples of latency. Perfect reconstruction
//!    (Σ bands == delayed input).

use std::f32::consts::PI;

/// Biquad direct-form-II transposed.
#[derive(Default, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Design a 2nd-order Butterworth lowpass biquad (foundation of LR2; cascade two for LR4).
pub fn design_lp_butter2(fc: f32, sr: f32) -> Biquad {
    let w = 2.0 * PI * fc.clamp(20.0, sr * 0.5 - 100.0) / sr;
    let cos_w = w.cos();
    let sin_w = w.sin();
    let q = std::f32::consts::FRAC_1_SQRT_2; // Butterworth Q
    let alpha = sin_w / (2.0 * q);

    let b0 = (1.0 - cos_w) * 0.5;
    let b1 = 1.0 - cos_w;
    let b2 = (1.0 - cos_w) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;

    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
        z1: 0.0,
        z2: 0.0,
    }
}

pub fn design_hp_butter2(fc: f32, sr: f32) -> Biquad {
    let w = 2.0 * PI * fc.clamp(20.0, sr * 0.5 - 100.0) / sr;
    let cos_w = w.cos();
    let sin_w = w.sin();
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let alpha = sin_w / (2.0 * q);

    let b0 = (1.0 + cos_w) * 0.5;
    let b1 = -(1.0 + cos_w);
    let b2 = (1.0 + cos_w) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;

    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
        z1: 0.0,
        z2: 0.0,
    }
}

/// 2nd-order allpass at fc, Q = 1/√2. Magnitude flat; phase rotates from 0 at DC to -2π at Nyquist.
pub fn design_ap_butter2(fc: f32, sr: f32) -> Biquad {
    let w = 2.0 * PI * fc.clamp(20.0, sr * 0.5 - 100.0) / sr;
    let cos_w = w.cos();
    let sin_w = w.sin();
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let alpha = sin_w / (2.0 * q);

    let b0 = 1.0 - alpha;
    let b1 = -2.0 * cos_w;
    let b2 = 1.0 + alpha;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;

    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
        z1: 0.0,
        z2: 0.0,
    }
}

/// LR2 lowpass = single Butter2 lowpass. LR4 lowpass = two cascaded Butter2 lowpasses.
#[derive(Default)]
pub struct Lr4Lowpass {
    b1: Biquad,
    b2: Biquad,
}

impl Lr4Lowpass {
    pub fn redesign(&mut self, fc: f32, sr: f32) {
        let new = design_lp_butter2(fc, sr);
        self.b1.b0 = new.b0;
        self.b1.b1 = new.b1;
        self.b1.b2 = new.b2;
        self.b1.a1 = new.a1;
        self.b1.a2 = new.a2;
        self.b2.b0 = new.b0;
        self.b2.b1 = new.b1;
        self.b2.b2 = new.b2;
        self.b2.a1 = new.a1;
        self.b2.a2 = new.a2;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.b2.process(self.b1.process(x))
    }

    pub fn reset(&mut self) {
        self.b1.reset();
        self.b2.reset();
    }
}

#[derive(Default)]
pub struct Lr4Highpass {
    b1: Biquad,
    b2: Biquad,
}

impl Lr4Highpass {
    pub fn redesign(&mut self, fc: f32, sr: f32) {
        let new = design_hp_butter2(fc, sr);
        self.b1.b0 = new.b0;
        self.b1.b1 = new.b1;
        self.b1.b2 = new.b2;
        self.b1.a1 = new.a1;
        self.b1.a2 = new.a2;
        self.b2.b0 = new.b0;
        self.b2.b1 = new.b1;
        self.b2.b2 = new.b2;
        self.b2.a1 = new.a1;
        self.b2.a2 = new.a2;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.b2.process(self.b1.process(x))
    }

    pub fn reset(&mut self) {
        self.b1.reset();
        self.b2.reset();
    }
}

/// Allpass compensation for an LR4 split: equivalent to LP + HP at the split frequency.
/// Implemented as a 2nd-order allpass at fc.
#[derive(Default)]
pub struct ApComp {
    b: Biquad,
}

impl ApComp {
    pub fn redesign(&mut self, fc: f32, sr: f32) {
        let new = design_ap_butter2(fc, sr);
        self.b.b0 = new.b0;
        self.b.b1 = new.b1;
        self.b.b2 = new.b2;
        self.b.a1 = new.a1;
        self.b.a2 = new.a2;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.b.process(x)
    }

    pub fn reset(&mut self) {
        self.b.reset();
    }
}

/// 4-band IIR Linkwitz-Riley crossover with allpass compensation.
/// Process one channel (M or S); we run two of these in `Plugin::process`.
#[derive(Default)]
pub struct CrossoverIir {
    // Split filters
    lp1: Lr4Lowpass,
    hp1: Lr4Highpass,
    lp2: Lr4Lowpass,
    hp2: Lr4Highpass,
    lp3: Lr4Lowpass,
    hp3: Lr4Highpass,
    // Allpass compensation
    ap2_for_low: ApComp, // applied to low band
    ap3_for_low: ApComp, // applied to low band
    ap3_for_ml: ApComp,  // applied to mid_low band
}

impl CrossoverIir {
    pub fn redesign(&mut self, f1: f32, f2: f32, f3: f32, sr: f32) {
        let f1 = f1.clamp(20.0, sr * 0.5 - 100.0);
        let f2 = f2.clamp(f1 + 50.0, sr * 0.5 - 50.0);
        let f3 = f3.clamp(f2 + 50.0, sr * 0.5 - 50.0);

        self.lp1.redesign(f1, sr);
        self.hp1.redesign(f1, sr);
        self.lp2.redesign(f2, sr);
        self.hp2.redesign(f2, sr);
        self.lp3.redesign(f3, sr);
        self.hp3.redesign(f3, sr);
        self.ap2_for_low.redesign(f2, sr);
        self.ap3_for_low.redesign(f3, sr);
        self.ap3_for_ml.redesign(f3, sr);
    }

    pub fn reset(&mut self) {
        self.lp1.reset();
        self.hp1.reset();
        self.lp2.reset();
        self.hp2.reset();
        self.lp3.reset();
        self.hp3.reset();
        self.ap2_for_low.reset();
        self.ap3_for_low.reset();
        self.ap3_for_ml.reset();
    }

    /// Returns (low, mid_low, mid_high, high) per-sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> [f32; 4] {
        // Top split at f1
        let low_pre = self.lp1.process(x);
        let mh_pre = self.hp1.process(x);

        // Compensate low for the AP2 and AP3 it didn't pass through
        let low = self.ap3_for_low.process(self.ap2_for_low.process(low_pre));

        // Mid split at f2
        let ml_pre = self.lp2.process(mh_pre);
        let mhh_pre = self.hp2.process(mh_pre);

        // Compensate mid_low for the AP3 it didn't pass through
        let mid_low = self.ap3_for_ml.process(ml_pre);

        // High split at f3
        let mid_high = self.lp3.process(mhh_pre);
        let high = self.hp3.process(mhh_pre);

        [low, mid_low, mid_high, high]
    }
}

// ============================================================================
// Linear-phase FIR variant
// ============================================================================

/// Single linear-phase windowed-sinc lowpass FIR.
/// Length is fixed at construction; coefficients are redesigned in place.
pub struct FirLowpass {
    taps_a: Vec<f32>, // current
    taps_b: Vec<f32>, // pending (filled during redesign)
    /// Double-buffered history: `2 * length` elements. Each sample is written at both
    /// `write_idx` and `write_idx + length`, so a contiguous `length`-element slice
    /// starting at any position is always available for the dot product (no per-tap
    /// wraparound branch). Mirrors the pattern in `hilbert.rs`.
    delay_line: Vec<f32>,
    write_idx: usize,
    using_a: bool, // true = process from taps_a
}

impl FirLowpass {
    pub fn new(length: usize) -> Self {
        Self {
            taps_a: vec![0.0; length],
            taps_b: vec![0.0; length],
            delay_line: vec![0.0; 2 * length],
            write_idx: 0,
            using_a: true,
        }
    }

    pub fn length(&self) -> usize {
        self.taps_a.len()
    }

    pub fn latency_samples(&self) -> usize {
        self.taps_a.len() / 2
    }

    /// Design windowed-sinc lowpass into the *pending* tap array.
    pub fn redesign_pending(&mut self, fc: f32, sr: f32) {
        let target = if self.using_a {
            &mut self.taps_b
        } else {
            &mut self.taps_a
        };
        Self::design(target, fc, sr);
    }

    fn design(taps: &mut [f32], fc: f32, sr: f32) {
        let n = taps.len();
        let center = (n - 1) as f32 / 2.0;
        let cutoff_norm = (fc / sr).clamp(0.001, 0.499);
        let mut sum = 0.0_f32;
        for (i, t) in taps.iter_mut().enumerate() {
            let k = i as f32 - center;
            let raw = if k.abs() < 1e-9 {
                2.0 * cutoff_norm
            } else {
                (2.0 * std::f32::consts::PI * cutoff_norm * k).sin() / (std::f32::consts::PI * k)
            };
            // Hann window
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
            *t = raw * w;
            sum += *t;
        }
        // Normalize for unit DC gain.
        if sum.abs() > 1e-9 {
            for t in taps.iter_mut() {
                *t /= sum;
            }
        }
    }

    /// Swap pending → active (call after a crossfade completes).
    pub fn promote_pending(&mut self) {
        self.using_a = !self.using_a;
    }

    /// Process one sample using the current taps. Used during the crossfade,
    /// caller will compute (1-α)·current + α·pending separately.
    #[inline]
    pub fn process_current(&mut self, x: f32) -> f32 {
        let n = self.taps_a.len();
        // Mirror-write into both halves so the dot-product can read a contiguous
        // slice regardless of wraparound.
        self.delay_line[self.write_idx] = x;
        self.delay_line[self.write_idx + n] = x;
        let next = if self.write_idx + 1 == n {
            0
        } else {
            self.write_idx + 1
        };
        let taps = if self.using_a {
            &self.taps_a
        } else {
            &self.taps_b
        };
        // Slice [next .. next + n] holds the last `n` samples in oldest→newest order;
        // zipping taps with the reversed slice gives sum(taps[k] · x[n−1−k]).
        let hist = &self.delay_line[next..next + n];
        let mut acc = 0.0;
        for (&tap, &h) in taps.iter().zip(hist.iter().rev()) {
            acc += tap * h;
        }
        self.write_idx = next;
        acc
    }

    /// Process one sample using the *pending* taps (for crossfading).
    /// Does NOT advance the delay line; call `process_current` first.
    #[inline]
    pub fn process_pending(&self) -> f32 {
        let n = self.taps_a.len();
        let pending = if self.using_a {
            &self.taps_b
        } else {
            &self.taps_a
        };
        // process_current already advanced write_idx so that [write_idx .. write_idx + n]
        // is the oldest→newest contiguous window (with the wraparound absorbed by the
        // mirror copy in the second half of the buffer).
        let hist = &self.delay_line[self.write_idx..self.write_idx + n];
        let mut acc = 0.0;
        for (&tap, &h) in pending.iter().zip(hist.iter().rev()) {
            acc += tap * h;
        }
        acc
    }

    pub fn reset(&mut self) {
        self.delay_line.fill(0.0);
        self.write_idx = 0;
    }
}

/// 4-band linear-phase FIR crossover. Bands are derived from a single LP per split:
///   low  = LP1
///   mid_low  = LP2 - LP1
///   mid_high = LP3 - LP2
///   high     = input - LP3
/// Sum = input — perfect reconstruction (modulo group-delay alignment).
pub struct CrossoverFir {
    lp1: FirLowpass,
    lp2: FirLowpass,
    lp3: FirLowpass,
    /// Input delay line so the "high" branch (input - LP3) aligns with the FIR group delay.
    input_delay: Vec<f32>,
    input_delay_idx: usize,
    /// Crossfade state when a redesign is in flight.
    crossfade_pending_lp: [bool; 3],
    crossfade_counter: usize,
    crossfade_total: usize,
}

impl CrossoverFir {
    pub fn new(length: usize) -> Self {
        let half = length / 2;
        Self {
            lp1: FirLowpass::new(length),
            lp2: FirLowpass::new(length),
            lp3: FirLowpass::new(length),
            input_delay: vec![0.0; half + 1],
            input_delay_idx: 0,
            crossfade_pending_lp: [false; 3],
            crossfade_counter: 0,
            crossfade_total: 0,
        }
    }

    pub fn length(&self) -> usize {
        self.lp1.length()
    }

    pub fn latency_samples(&self) -> usize {
        self.lp1.latency_samples()
    }

    /// Redesign all three lowpasses into the pending arrays. Schedules a crossfade.
    pub fn redesign(&mut self, f1: f32, f2: f32, f3: f32, sr: f32, crossfade_len: usize) {
        // If a previous crossfade is still in flight, snap-promote it now so the new
        // pending taps don't overwrite a target the alpha curve was designed for.
        if self.crossfade_counter > 0 {
            if self.crossfade_pending_lp[0] {
                self.lp1.promote_pending();
            }
            if self.crossfade_pending_lp[1] {
                self.lp2.promote_pending();
            }
            if self.crossfade_pending_lp[2] {
                self.lp3.promote_pending();
            }
            self.crossfade_pending_lp = [false; 3];
            self.crossfade_counter = 0;
        }

        let f1 = f1.clamp(20.0, sr * 0.5 - 100.0);
        let f2 = f2.clamp(f1 + 50.0, sr * 0.5 - 50.0);
        let f3 = f3.clamp(f2 + 50.0, sr * 0.5 - 50.0);

        self.lp1.redesign_pending(f1, sr);
        self.lp2.redesign_pending(f2, sr);
        self.lp3.redesign_pending(f3, sr);

        self.crossfade_pending_lp = [true; 3];
        self.crossfade_counter = crossfade_len;
        self.crossfade_total = crossfade_len.max(1);
    }

    /// Initial design — bypass crossfade, place coefficients in active slots immediately.
    pub fn initialize(&mut self, f1: f32, f2: f32, f3: f32, sr: f32) {
        self.redesign(f1, f2, f3, sr, 1);
        self.lp1.promote_pending();
        self.lp2.promote_pending();
        self.lp3.promote_pending();
        self.crossfade_pending_lp = [false; 3];
        self.crossfade_counter = 0;
    }

    pub fn reset(&mut self) {
        self.lp1.reset();
        self.lp2.reset();
        self.lp3.reset();
        self.input_delay.fill(0.0);
        self.input_delay_idx = 0;
    }

    /// Returns (low, mid_low, mid_high, high).
    #[inline]
    pub fn process(&mut self, x: f32) -> [f32; 4] {
        // Push to input delay
        self.input_delay[self.input_delay_idx] = x;
        let read_idx = if self.input_delay_idx + 1 == self.input_delay.len() {
            0
        } else {
            self.input_delay_idx + 1
        };
        let x_delayed = self.input_delay[read_idx];
        self.input_delay_idx = read_idx;

        // Run each LP. During crossfade, blend current and pending outputs.
        let lp1 = self.run_lp_with_crossfade(0, x);
        let lp2 = self.run_lp_with_crossfade(1, x);
        let lp3 = self.run_lp_with_crossfade(2, x);

        if self.crossfade_counter > 0 {
            self.crossfade_counter -= 1;
            if self.crossfade_counter == 0 {
                if self.crossfade_pending_lp[0] {
                    self.lp1.promote_pending();
                }
                if self.crossfade_pending_lp[1] {
                    self.lp2.promote_pending();
                }
                if self.crossfade_pending_lp[2] {
                    self.lp3.promote_pending();
                }
                self.crossfade_pending_lp = [false; 3];
            }
        }

        [
            lp1,             // low
            lp2 - lp1,       // mid_low
            lp3 - lp2,       // mid_high
            x_delayed - lp3, // high
        ]
    }

    #[inline]
    fn run_lp_with_crossfade(&mut self, idx: usize, x: f32) -> f32 {
        let lp = match idx {
            0 => &mut self.lp1,
            1 => &mut self.lp2,
            2 => &mut self.lp3,
            _ => unreachable!("idx must be 0..=2"),
        };
        let cur = lp.process_current(x);
        if self.crossfade_pending_lp[idx] && self.crossfade_total > 0 {
            let pend = lp.process_pending();
            let alpha = 1.0 - (self.crossfade_counter as f32 / self.crossfade_total as f32);
            (1.0 - alpha) * cur + alpha * pend
        } else {
            cur
        }
    }
}

#[cfg(test)]
mod fir_tests {
    use super::*;

    #[test]
    fn fir_band_sum_equals_delayed_input() {
        let sr = 48_000.0;
        let n_taps = 511; // odd length, integer half-delay
        let mut x = CrossoverFir::new(n_taps);
        x.initialize(120.0, 1000.0, 8000.0, sr);

        let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.013).sin()).collect();
        let mut summed = vec![0.0_f32; input.len()];
        for (i, &s) in input.iter().enumerate() {
            let bands = x.process(s);
            summed[i] = bands.iter().sum();
        }

        let lat = x.latency_samples();
        for i in lat + 256..input.len() {
            let err = (summed[i] - input[i - lat]).abs();
            assert!(err < 1e-3, "i={i}: err {err:e}");
        }
    }

    #[test]
    fn fir_crossfade_no_discontinuity() {
        let sr = 48_000.0;
        let mut x = CrossoverFir::new(511);
        x.initialize(120.0, 1000.0, 8000.0, sr);

        let input: Vec<f32> = (0..2048)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin())
            .collect();
        let mut prev_summed = 0.0_f32;
        for &s in &input[..1024] {
            let bands = x.process(s);
            prev_summed = bands.iter().sum();
        }

        x.redesign(150.0, 1500.0, 9000.0, sr, 256);

        let mut max_jump = 0.0_f32;
        for &s in &input[1024..] {
            let bands = x.process(s);
            let summed: f32 = bands.iter().sum();
            let jump = (summed - prev_summed).abs();
            if jump > max_jump {
                max_jump = jump;
            }
            prev_summed = summed;
        }
        assert!(max_jump < 0.20, "max jump {max_jump:.4} suggests a click");
    }

    /// Sanity check that all process-thread-visible buffers retain their original lengths
    /// across repeated `redesign` + `process` cycles. This is a *proxy* for "no allocations
    /// happened" — the strict guarantee comes from `nih_plug`'s `assert_process_allocs`
    /// feature on the audio thread; here we only verify the obvious in-place property.
    #[test]
    fn fir_redesign_preserves_buffer_lengths() {
        let mut x = CrossoverFir::new(255);
        x.initialize(120.0, 1000.0, 8000.0, 48_000.0);
        let len_before = (
            x.lp1.taps_a.len(),
            x.lp1.taps_b.len(),
            x.lp1.delay_line.len(),
            x.input_delay.len(),
        );
        for f1 in [80.0, 200.0, 500.0, 100.0_f32] {
            x.redesign(f1, 1000.0, 8000.0, 48_000.0, 64);
            for _ in 0..128 {
                let _ = x.process(0.1);
            }
        }
        let len_after = (
            x.lp1.taps_a.len(),
            x.lp1.taps_b.len(),
            x.lp1.delay_line.len(),
            x.input_delay.len(),
        );
        assert_eq!(len_before, len_after);
    }

    #[test]
    fn fir_redesign_during_crossfade_no_click() {
        let sr = 48_000.0;
        let mut x = CrossoverFir::new(511);
        x.initialize(120.0, 1000.0, 8000.0, sr);

        // Settle on a sine
        let input: Vec<f32> = (0..3072)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin())
            .collect();
        let mut prev = 0.0;
        for &s in &input[..1024] {
            let bands = x.process(s);
            prev = bands.iter().sum();
        }

        // First redesign — start a crossfade
        x.redesign(150.0, 1500.0, 9000.0, sr, 512);
        for &s in &input[1024..1280] {
            let bands = x.process(s);
            prev = bands.iter().sum();
        }
        // 256 samples into the crossfade, redesign again — should snap-promote first.
        x.redesign(180.0, 2000.0, 10000.0, sr, 512);

        let mut max_jump = 0.0_f32;
        for &s in &input[1280..] {
            let bands = x.process(s);
            let summed: f32 = bands.iter().sum();
            let jump = (summed - prev).abs();
            if jump > max_jump {
                max_jump = jump;
            }
            prev = summed;
        }
        // The snap-promotion is a one-sample swap from current to (formerly) pending; that
        // sample's jump is bounded by the magnitude of the input itself plus the crossfade
        // residual. 0.30 is a comfortable bound that catches gross clicks.
        assert!(
            max_jump < 0.30,
            "max jump {max_jump:.4} after redesign-during-crossfade"
        );
    }

    #[test]
    fn fir_latency_matches_kernel_half() {
        let x = CrossoverFir::new(513);
        assert_eq!(x.latency_samples(), 256);
    }

    #[test]
    fn fir_each_band_peaks_in_its_passband() {
        let sr = 48_000.0;
        let probes: [(f32, usize); 4] = [(60.0, 0), (400.0, 1), (3000.0, 2), (12000.0, 3)];
        for (f, expected) in probes {
            let mut x = CrossoverFir::new(511);
            x.initialize(120.0, 1000.0, 8000.0, sr);
            let input: Vec<f32> = (0..4096)
                .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin())
                .collect();
            let mut acc = [0.0_f32; 4];
            for &s in &input[1024..] {
                let bands = x.process(s);
                for i in 0..4 {
                    acc[i] += bands[i] * bands[i];
                }
            }
            let max_idx = acc
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap()
                .0;
            assert_eq!(max_idx, expected, "f={f}: acc={acc:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn sine(f: f32, n: usize, sr: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * f * i as f32 / sr).sin())
            .collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut state: u32 = 0xdead_beef;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn band_sum_is_magnitude_flat_within_005db() {
        let sr = 48_000.0;
        let mut x = CrossoverIir::default();
        x.redesign(120.0, 1000.0, 8000.0, sr);
        let input = noise(16384);

        let mut summed = vec![0.0_f32; input.len()];
        for (i, &s) in input.iter().enumerate() {
            let bands = x.process(s);
            summed[i] = bands.iter().sum();
        }

        let rms_in = rms(&input[1024..]);
        let rms_out = rms(&summed[1024..]);
        let ratio_db = 20.0 * (rms_out / rms_in).log10();
        assert!(
            ratio_db.abs() < 0.05,
            "broadband RMS deviation {ratio_db:.4} dB"
        );
    }

    #[test]
    fn band_sum_flat_across_decades() {
        let sr = 48_000.0;
        // Use a longer buffer + later skip so the slowest probe (40 Hz, near the f1 split
        // at 120 Hz) has time to settle past the cascaded-biquad transient. At 40 Hz, one
        // period is 1200 samples; cascaded LR4 + AP3 ringing decays over multiple periods.
        let n: usize = 32_768;
        let skip: usize = 8_192;
        for &f in &[40.0, 100.0, 300.0, 1_000.0, 3_000.0, 10_000.0, 18_000.0_f32] {
            let mut x = CrossoverIir::default();
            x.redesign(120.0, 1000.0, 8000.0, sr);
            let input = sine(f, n, sr);
            let mut summed = vec![0.0_f32; input.len()];
            for (i, &s) in input.iter().enumerate() {
                let bands = x.process(s);
                summed[i] = bands.iter().sum();
            }
            let rms_in = rms(&input[skip..]);
            let rms_out = rms(&summed[skip..]);
            let ratio_db = 20.0 * (rms_out / rms_in).log10();
            assert!(ratio_db.abs() < 0.10, "f={f}: deviation {ratio_db:.4} dB");
        }
    }

    #[test]
    fn each_band_peaks_in_its_passband() {
        let sr = 48_000.0;
        let probes_per_band: [(f32, usize); 4] = [
            (60.0, 0),    // low band
            (400.0, 1),   // mid_low
            (3000.0, 2),  // mid_high
            (12000.0, 3), // high
        ];
        for (f, expected) in probes_per_band {
            let mut x = CrossoverIir::default();
            x.redesign(120.0, 1000.0, 8000.0, sr);
            let input = sine(f, 8192, sr);
            let mut bands_acc = [0.0_f32; 4];
            for &s in &input[2048..] {
                let bands = x.process(s);
                for i in 0..4 {
                    bands_acc[i] += bands[i] * bands[i];
                }
            }
            let max_idx = bands_acc
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap()
                .0;
            assert_eq!(max_idx, expected, "f={f}: bands_acc={bands_acc:?}");
        }
    }

    #[test]
    fn redesign_in_place_no_panic() {
        let sr = 48_000.0;
        let mut x = CrossoverIir::default();
        x.redesign(120.0, 1000.0, 8000.0, sr);
        let _ = x.process(0.5);
        for f1 in [80.0_f32, 200.0, 500.0] {
            for f2 in [800.0_f32, 1500.0, 3000.0] {
                for f3 in [5000.0_f32, 10000.0, 15000.0] {
                    x.redesign(f1, f2, f3, sr);
                    for _ in 0..32 {
                        let bands = x.process(0.1);
                        for b in bands {
                            assert!(b.is_finite());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn boundary_frequencies_no_nan() {
        let sr = 48_000.0;
        let mut x = CrossoverIir::default();
        x.redesign(20.0, 1000.0, sr * 0.5 - 100.0, sr);
        for _ in 0..1024 {
            let b = x.process(0.5);
            for v in b {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn split_ordering_clamps() {
        let sr = 48_000.0;
        let mut x = CrossoverIir::default();
        x.redesign(5000.0, 1000.0, 200.0, sr);
        for _ in 0..256 {
            let b = x.process(0.1);
            for v in b {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn sample_rate_sweep() {
        for &sr in &[44100.0_f32, 48000.0, 96000.0, 192000.0] {
            let mut x = CrossoverIir::default();
            x.redesign(120.0, 1000.0, 8000.0, sr);
            let input = noise(4096);
            for &s in &input {
                let b = x.process(s);
                for v in b {
                    assert!(v.is_finite());
                }
            }
        }
    }

    #[test]
    fn reset_clears_state() {
        let sr = 48_000.0;
        let mut x = CrossoverIir::default();
        x.redesign(120.0, 1000.0, 8000.0, sr);
        for _ in 0..4096 {
            x.process(1.0);
        }
        x.reset();
        let b = x.process(0.0);
        for v in b {
            assert!(v.abs() < 1e-6, "post-reset DC: {v}");
        }
    }
}
