//! ITU-R BS.1770-4 true-peak detector (polyphase oversampling).
//!
//! Exact 48-tap, 4-phase reference coefficients from ITU-R BS.1770-4 Annex 2.
//! Sample-rate-aware: 4× oversampling below 96 kHz, 2× from 96–192 kHz,
//! bypass at/above 192 kHz. Uses a double-buffered history so the SIMD dot
//! product always reads a contiguous 12-element slice.
//!
//! Extracted verbatim from the copy that previously lived in both
//! `gs-meter/src/meter.rs` and `tinylimit/src/true_peak.rs`.
use std::simd::{f32x16, num::SimdFloat};

// ── True Peak: 4x oversampling per ITU-R BS.1770-4, Annex 2 ─────────────
//
// Uses the exact reference coefficients from the ITU-R BS.1770-4 standard
// (page 17, "order 48, 4-phase, FIR interpolating" filter for 48 kHz).

const TRUE_PEAK_TAPS: usize = 12; // taps per phase
const TRUE_PEAK_PHASES: usize = 4;

/// ITU-R BS.1770-4 Annex 2 reference filter coefficients.
/// 4 phases × 12 taps, exactly as published in the standard.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)] // Coefficients copied verbatim from ITU-R BS.1770-4 Annex 2
const ITU_COEFFS: [[f32; TRUE_PEAK_TAPS]; TRUE_PEAK_PHASES] = [
    // Phase 0
    [ 0.0017089843750, 0.0109863281250,-0.0196533203125, 0.0332031250000,
     -0.0594482421875, 0.1373291015625, 0.9721679687500,-0.1022949218750,
      0.0476074218750,-0.0266113281250, 0.0148925781250,-0.0083007812500],
    // Phase 1
    [-0.0291748046875, 0.0292968750000,-0.0517578125000, 0.0891113281250,
     -0.1665039062500, 0.4650878906250, 0.7797851562500,-0.2003173828125,
      0.1015625000000,-0.0582275390625, 0.0330810546875,-0.0189208984375],
    // Phase 2
    [-0.0189208984375, 0.0330810546875,-0.0582275390625, 0.1015625000000,
     -0.2003173828125, 0.7797851562500, 0.4650878906250,-0.1665039062500,
      0.0891113281250,-0.0517578125000, 0.0292968750000,-0.0291748046875],
    // Phase 3
    [-0.0083007812500, 0.0148925781250,-0.0266113281250, 0.0476074218750,
     -0.1022949218750, 0.9721679687500, 0.1373291015625,-0.0594482421875,
      0.0332031250000,-0.0196533203125, 0.0109863281250, 0.0017089843750],
];

/// ITU coefficients zero-padded to 16 for SIMD f32x16 dot product.
const ITU_COEFFS_PADDED: [[f32; 16]; TRUE_PEAK_PHASES] = {
    let mut padded = [[0.0_f32; 16]; TRUE_PEAK_PHASES];
    let mut p = 0;
    while p < TRUE_PEAK_PHASES {
        let mut t = 0;
        while t < TRUE_PEAK_TAPS {
            padded[p][t] = ITU_COEFFS[p][t];
            t += 1;
        }
        p += 1;
    }
    padded
};

/// SIMD dot product of 12 contiguous samples against 16-element padded coefficients.
#[inline(always)]
fn dot12_simd(history: &[f32], coeffs: &[f32; 16]) -> f32 {
    let mut h = [0.0_f32; 16];
    h[..12].copy_from_slice(&history[..12]);
    let hv = f32x16::from_array(h);
    let cv = f32x16::from_array(*coeffs);
    (hv * cv).reduce_sum()
}

/// Oversampling mode based on input sample rate, per ITU-R BS.1770-4 Annex 2.
#[derive(Clone, Copy, PartialEq)]
enum TruePeakMode {
    /// Input < 96 kHz: use all 4 phases (4x oversampling to ≥192 kHz).
    Oversample4x,
    /// Input 96–191 kHz: use phases 0 and 2 only (2x oversampling).
    Oversample2x,
    /// Input ≥ 192 kHz: no oversampling needed, sample peak suffices.
    Bypass,
}

/// True peak detector using polyphase oversampling (ITU-R BS.1770-4).
/// Uses a double-buffered history for contiguous SIMD reads.
pub struct TruePeakDetector {
    /// Double-buffered history: 2 × 12 elements. Writing to both halves
    /// ensures a contiguous 12-element slice is always available at any pos.
    history: [f32; TRUE_PEAK_TAPS * 2],
    /// Write position (0..TRUE_PEAK_TAPS-1).
    pos: usize,
    /// Highest true peak (linear) since last reset.
    true_peak_max: f32,
    /// Oversampling mode (depends on input sample rate).
    mode: TruePeakMode,
}

impl Default for TruePeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeakDetector {
    pub fn new() -> Self {
        Self {
            history: [0.0; TRUE_PEAK_TAPS * 2],
            pos: 0,
            true_peak_max: 0.0,
            mode: TruePeakMode::Oversample4x,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.mode = if sample_rate >= 192000.0 {
            TruePeakMode::Bypass
        } else if sample_rate >= 96000.0 {
            TruePeakMode::Oversample2x
        } else {
            TruePeakMode::Oversample4x
        };
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.pos = 0;
        self.true_peak_max = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, sample: f32) {
        if self.mode == TruePeakMode::Bypass {
            return;
        }

        // Write to both halves of the double buffer
        self.history[self.pos] = sample;
        self.history[self.pos + TRUE_PEAK_TAPS] = sample;
        self.pos += 1;
        if self.pos == TRUE_PEAK_TAPS {
            self.pos = 0;
        }

        // The contiguous history slice for dot product starts at pos
        // and reads 12 elements (oldest to newest).
        // The original code read newest-first (tap 0 = newest), so we
        // need reversed coefficients. But ITU_COEFFS Phase 0 and Phase 3
        // are reverses of each other, and we use padded coefficients that
        // match the standard's tap ordering. The slice at pos is oldest-first,
        // so we reverse the coefficient order by reading the slice from the
        // END of the double buffer backwards. Instead, we read from
        // (pos + TAPS - 1) downward — but with double buffer, we can just
        // use the slice starting at pos directly if we reverse the coefficients.
        //
        // Simpler approach: the history at [pos..pos+12] is in order
        // [oldest, ..., newest]. The original convolution was
        // h[0]*newest + h[1]*(newest-1) + ... = sum(h[tap] * x[n-tap]).
        // With oldest-first slice: slice[0]=x[n-11], slice[11]=x[n].
        // We need: sum(h[tap] * slice[11-tap]) = sum(h_rev[i] * slice[i])
        // where h_rev is the reversed coefficients.
        // BUT: Phase 0 reversed = Phase 3, and Phase 1 reversed = Phase 2.
        // So we can use ITU_COEFFS[3-p] with the oldest-first slice.
        let hist = &self.history[self.pos..self.pos + TRUE_PEAK_TAPS];

        let phases: &[usize] = match self.mode {
            TruePeakMode::Oversample4x => &[0, 1, 2, 3],
            TruePeakMode::Oversample2x => &[0, 2],
            TruePeakMode::Bypass => unreachable!(),
        };

        for &p in phases {
            // Use reversed phase: oldest-first slice × reversed coefficients
            let rev_p = TRUE_PEAK_PHASES - 1 - p;
            let abs = dot12_simd(hist, &ITU_COEFFS_PADDED[rev_p]).abs();
            if abs > self.true_peak_max {
                self.true_peak_max = abs;
            }
        }
    }

    /// Process one sample and return the instantaneous true peak (linear, absolute).
    ///
    /// Unlike `process_sample`, this returns the peak for just this sample
    /// (the max across all oversampled phases), rather than accumulating a
    /// running max. Also updates the cumulative `true_peak_max`.
    #[inline]
    pub fn process_sample_peak(&mut self, sample: f32) -> f32 {
        if self.mode == TruePeakMode::Bypass {
            let abs = sample.abs();
            if abs > self.true_peak_max {
                self.true_peak_max = abs;
            }
            return abs;
        }

        // Write to both halves of the double buffer
        self.history[self.pos] = sample;
        self.history[self.pos + TRUE_PEAK_TAPS] = sample;
        self.pos += 1;
        if self.pos == TRUE_PEAK_TAPS {
            self.pos = 0;
        }

        let hist = &self.history[self.pos..self.pos + TRUE_PEAK_TAPS];

        let phases: &[usize] = match self.mode {
            TruePeakMode::Oversample4x => &[0, 1, 2, 3],
            TruePeakMode::Oversample2x => &[0, 2],
            TruePeakMode::Bypass => unreachable!(),
        };

        let mut peak = 0.0_f32;
        for &p in phases {
            let rev_p = TRUE_PEAK_PHASES - 1 - p;
            let abs = dot12_simd(hist, &ITU_COEFFS_PADDED[rev_p]).abs();
            if abs > peak {
                peak = abs;
            }
        }
        if peak > self.true_peak_max {
            self.true_peak_max = peak;
        }
        peak
    }

    pub fn true_peak_max(&self) -> f32 {
        self.true_peak_max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot12_simd_matches_scalar() {
        let history: [f32; 12] = [
            0.1, -0.2, 0.3, -0.4, 0.5, -0.6, 0.7, -0.8, 0.9, -1.0, 0.5, -0.3,
        ];
        for phase in 0..4 {
            let simd_result = dot12_simd(&history, &ITU_COEFFS_PADDED[phase]);
            let scalar_result: f32 = history
                .iter()
                .zip(ITU_COEFFS[phase].iter())
                .map(|(h, c)| h * c)
                .sum();
            assert!(
                (simd_result - scalar_result).abs() < 1e-5,
                "phase {phase}: simd={simd_result}, scalar={scalar_result}"
            );
        }
    }

    #[test]
    fn test_true_peak_detects_intersample() {
        let mut det = TruePeakDetector::new();
        det.set_sample_rate(48000.0);
        // 3 samples per cycle of a 1 Hz sine: phases 0°, 120°, 240°.
        // The actual peak at 90° falls between samples; sample peak = sin(120°) ≈ 0.866.
        // Run enough cycles for the FIR history to settle, then check that
        // true peak exceeds the sample peak.
        let sr = 3.0_f64;
        let freq = 1.0_f64;
        for i in 0..30 {
            let t = i as f64 / sr;
            let sample = (t * freq * std::f64::consts::TAU).sin() as f32;
            det.process_sample(sample);
        }
        assert!(
            det.true_peak_max() > 0.9,
            "true peak {} should exceed 0.9 (sample peak ≈ 0.866)",
            det.true_peak_max()
        );
    }

    #[test]
    fn test_true_peak_reset() {
        let mut det = TruePeakDetector::new();
        det.set_sample_rate(48000.0);
        det.process_sample(1.0);
        assert!(det.true_peak_max() > 0.0);
        det.reset();
        assert_eq!(det.true_peak_max(), 0.0);
    }

    #[test]
    fn test_true_peak_quiet_signal() {
        let mut det = TruePeakDetector::new();
        det.set_sample_rate(48000.0);
        for _ in 0..100 {
            det.process_sample(0.0);
        }
        assert_eq!(det.true_peak_max(), 0.0);
    }
}
