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
