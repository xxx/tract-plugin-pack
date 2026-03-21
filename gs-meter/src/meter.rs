//! Core metering DSP: peak tracking, RMS computation, crest factor, true peak.
//! All levels are in linear amplitude; dB conversion happens at display time.

/// Maximum supported RMS window in samples (3000ms at 192kHz).
const MAX_WINDOW_SAMPLES: usize = 576_000;

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

/// True peak detector using 4x polyphase oversampling (ITU-R BS.1770-4).
pub struct TruePeakDetector {
    /// Circular history buffer.
    history: [f32; TRUE_PEAK_TAPS],
    /// Write position in history.
    pos: usize,
    /// Highest true peak (linear) since last reset.
    true_peak_max: f32,
}

impl Default for TruePeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeakDetector {
    pub fn new() -> Self {
        Self {
            history: [0.0; TRUE_PEAK_TAPS],
            pos: 0,
            true_peak_max: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.pos = 0;
        self.true_peak_max = 0.0;
    }

    #[inline]
    pub fn process_sample(&mut self, sample: f32) {
        self.history[self.pos] = sample;
        self.pos = (self.pos + 1) % TRUE_PEAK_TAPS;

        for phase in &ITU_COEFFS {
            let mut sum = 0.0_f32;
            for (tap, &coeff) in phase.iter().enumerate() {
                let idx = (self.pos + TRUE_PEAK_TAPS - 1 - tap) % TRUE_PEAK_TAPS;
                sum += self.history[idx] * coeff;
            }
            let abs = sum.abs();
            if abs > self.true_peak_max {
                self.true_peak_max = abs;
            }
        }
    }

    pub fn true_peak_max(&self) -> f32 {
        self.true_peak_max
    }
}

/// Per-channel metering state.
pub struct ChannelMeter {
    /// Highest absolute sample value since last reset.
    peak_max: f32,
    /// True peak detector (4x oversampled).
    true_peak: TruePeakDetector,
    /// Running sum of squared samples since last reset (for integrated RMS).
    rms_sum: f64,
    /// Number of samples accumulated in rms_sum.
    rms_count: u64,
    /// Ring buffer of squared samples, pre-allocated to MAX_WINDOW_SAMPLES.
    rms_ring: Vec<f32>,
    /// Logical window size (may be smaller than rms_ring.len()).
    rms_window_size: usize,
    /// Write position in the ring buffer (0..rms_window_size-1).
    rms_ring_pos: usize,
    /// Number of valid samples in the ring buffer (up to rms_window_size).
    rms_ring_filled: usize,
    /// Highest momentary RMS (linear) since last reset.
    rms_momentary_max: f32,
}

impl ChannelMeter {
    pub fn new(window_samples: usize) -> Self {
        let size = window_samples.clamp(1, MAX_WINDOW_SAMPLES);
        Self {
            peak_max: 0.0,
            true_peak: TruePeakDetector::new(),
            rms_sum: 0.0,
            rms_count: 0,
            rms_ring: vec![0.0; MAX_WINDOW_SAMPLES],
            rms_window_size: size,
            rms_ring_pos: 0,
            rms_ring_filled: 0,
            rms_momentary_max: 0.0,
        }
    }

    /// Reset all accumulated values.
    pub fn reset(&mut self) {
        self.peak_max = 0.0;
        self.true_peak.reset();
        self.rms_sum = 0.0;
        self.rms_count = 0;
        self.rms_ring[..self.rms_window_size].fill(0.0);
        self.rms_ring_pos = 0;
        self.rms_ring_filled = 0;
        self.rms_momentary_max = 0.0;
    }

    /// Change the momentary RMS window size. No allocation — uses pre-allocated buffer.
    /// Resets the ring buffer state.
    pub fn set_window_size(&mut self, window_samples: usize) {
        let size = window_samples.clamp(1, MAX_WINDOW_SAMPLES);
        if self.rms_window_size != size {
            self.rms_window_size = size;
            self.rms_ring[..size].fill(0.0);
            self.rms_ring_pos = 0;
            self.rms_ring_filled = 0;
        }
    }

    /// Process a single sample. Updates all running statistics.
    #[inline]
    pub fn process_sample(&mut self, sample: f32) {
        let abs = sample.abs();

        // Peak tracking (sample peak)
        if abs > self.peak_max {
            self.peak_max = abs;
        }

        // True peak (4x oversampled)
        self.true_peak.process_sample(sample);

        // Integrated RMS accumulation
        let sq = (sample as f64) * (sample as f64);
        self.rms_sum += sq;
        self.rms_count += 1;

        // Momentary RMS ring buffer
        let sq_f32 = sample * sample;
        self.rms_ring[self.rms_ring_pos] = sq_f32;
        self.rms_ring_pos += 1;
        if self.rms_ring_pos >= self.rms_window_size {
            self.rms_ring_pos = 0;
        }
        if self.rms_ring_filled < self.rms_window_size {
            self.rms_ring_filled += 1;
        }
    }

    /// Update momentary max after processing a buffer.
    /// Call this once per buffer, not per sample, for efficiency.
    pub fn update_momentary_max(&mut self) {
        let mom = self.rms_momentary_linear();
        if mom > self.rms_momentary_max {
            self.rms_momentary_max = mom;
        }
    }

    /// Current sample peak max in linear amplitude.
    pub fn peak_max(&self) -> f32 {
        self.peak_max
    }

    /// Current true peak max in linear amplitude (4x oversampled).
    /// Always >= sample peak by definition.
    pub fn true_peak_max(&self) -> f32 {
        self.true_peak.true_peak_max().max(self.peak_max)
    }

    /// Integrated RMS in linear amplitude (since last reset).
    pub fn rms_integrated_linear(&self) -> f32 {
        if self.rms_count == 0 {
            return 0.0;
        }
        (self.rms_sum / self.rms_count as f64).sqrt() as f32
    }

    /// Momentary RMS in linear amplitude (over the current window).
    pub fn rms_momentary_linear(&self) -> f32 {
        if self.rms_ring_filled == 0 {
            return 0.0;
        }
        let sum: f32 = if self.rms_ring_filled == self.rms_window_size {
            self.rms_ring[..self.rms_window_size].iter().sum()
        } else {
            self.rms_ring[..self.rms_ring_filled].iter().sum()
        };
        (sum / self.rms_ring_filled as f32).sqrt()
    }

    /// Highest momentary RMS (linear) since last reset.
    pub fn rms_momentary_max(&self) -> f32 {
        self.rms_momentary_max
    }

    /// Raw integrated sum-of-squares (f64) and sample count, for cross-channel summing.
    pub fn rms_integrated_raw(&self) -> (f64, u64) {
        (self.rms_sum, self.rms_count)
    }

    /// Sum of squared samples in the current momentary window and the filled count.
    pub fn rms_momentary_raw(&self) -> (f32, usize) {
        let sum: f32 = if self.rms_ring_filled == self.rms_window_size {
            self.rms_ring[..self.rms_window_size].iter().sum()
        } else {
            self.rms_ring[..self.rms_ring_filled].iter().sum()
        };
        (sum, self.rms_ring_filled)
    }

    /// Crest factor in dB: peak_max_dB - rms_integrated_dB.
    /// Returns `f32::NEG_INFINITY` if insufficient data.
    pub fn crest_factor_db(&self) -> f32 {
        let peak = self.peak_max;
        let rms = self.rms_integrated_linear();
        if rms < 1e-10 || peak < 1e-10 {
            return f32::NEG_INFINITY;
        }
        linear_to_db(peak) - linear_to_db(rms)
    }
}

/// Stereo meter that combines two channels.
pub struct StereoMeter {
    pub left: ChannelMeter,
    pub right: ChannelMeter,
    /// Highest stereo momentary RMS (summed power) since last reset.
    momentary_max_stereo: f32,
}

impl StereoMeter {
    pub fn new(window_samples: usize) -> Self {
        Self {
            left: ChannelMeter::new(window_samples),
            right: ChannelMeter::new(window_samples),
            momentary_max_stereo: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.momentary_max_stereo = 0.0;
    }

    pub fn set_window_size(&mut self, window_samples: usize) {
        self.left.set_window_size(window_samples);
        self.right.set_window_size(window_samples);
    }

    /// Process L/R samples from a nih-plug buffer.
    pub fn process_buffer(&mut self, left_samples: &[f32], right_samples: &[f32]) {
        for &s in left_samples {
            self.left.process_sample(s);
        }
        for &s in right_samples {
            self.right.process_sample(s);
        }
        self.left.update_momentary_max();
        self.right.update_momentary_max();
        // Track stereo momentary max (summed power)
        let mom = self.rms_momentary_stereo();
        if mom > self.momentary_max_stereo {
            self.momentary_max_stereo = mom;
        }
    }

    /// Max of L/R sample peak.
    pub fn peak_max_stereo(&self) -> f32 {
        self.left.peak_max().max(self.right.peak_max())
    }

    /// Max of L/R true peak.
    pub fn true_peak_max_stereo(&self) -> f32 {
        self.left.true_peak_max().max(self.right.true_peak_max())
    }

    /// Integrated RMS across both channels (sum of per-channel mean-square, then sqrt).
    /// Matches dpMeter5 SUM mode: sqrt(ms_L + ms_R), not averaged.
    pub fn rms_integrated_stereo(&self) -> f32 {
        let (sum_l, count_l) = self.left.rms_integrated_raw();
        let (sum_r, count_r) = self.right.rms_integrated_raw();
        if count_l == 0 && count_r == 0 {
            return 0.0;
        }
        let ms_l = if count_l > 0 { sum_l / count_l as f64 } else { 0.0 };
        let ms_r = if count_r > 0 { sum_r / count_r as f64 } else { 0.0 };
        (ms_l + ms_r).sqrt() as f32
    }

    /// Momentary RMS across both channels (sum of per-channel mean-square, then sqrt).
    pub fn rms_momentary_stereo(&self) -> f32 {
        let (sum_l, filled_l) = self.left.rms_momentary_raw();
        let (sum_r, filled_r) = self.right.rms_momentary_raw();
        if filled_l == 0 && filled_r == 0 {
            return 0.0;
        }
        let ms_l = if filled_l > 0 { sum_l / filled_l as f32 } else { 0.0 };
        let ms_r = if filled_r > 0 { sum_r / filled_r as f32 } else { 0.0 };
        (ms_l + ms_r).sqrt()
    }

    /// Highest momentary RMS (stereo sum) since last reset.
    /// Updated per-buffer via `process_buffer`.
    pub fn rms_momentary_max_stereo(&self) -> f32 {
        self.momentary_max_stereo
    }

    /// Stereo crest factor: peak_max_dB - rms_integrated_dB (both summed).
    pub fn crest_factor_db_stereo(&self) -> f32 {
        let peak = self.peak_max_stereo();
        let rms = self.rms_integrated_stereo();
        if rms < 1e-10 || peak < 1e-10 {
            return f32::NEG_INFINITY;
        }
        linear_to_db(peak) - linear_to_db(rms)
    }
}

/// Convert linear amplitude to dB. Returns -f32::INFINITY for zero.
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * linear.log10()
    }
}

/// Convert dB to linear amplitude.
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    #[test]
    fn test_new_meter_is_zeroed() {
        let m = ChannelMeter::new(100);
        assert_eq!(m.peak_max(), 0.0);
        assert_eq!(m.rms_integrated_linear(), 0.0);
        assert_eq!(m.rms_momentary_linear(), 0.0);
        assert_eq!(m.rms_momentary_max(), 0.0);
        assert!(m.crest_factor_db().is_infinite() && m.crest_factor_db() < 0.0);
    }

    #[test]
    fn test_peak_tracking() {
        let mut m = ChannelMeter::new(100);
        m.process_sample(0.5);
        m.process_sample(-0.8);
        m.process_sample(0.3);
        assert!(approx_eq(m.peak_max(), 0.8));
    }

    #[test]
    fn test_peak_tracks_negative() {
        let mut m = ChannelMeter::new(100);
        m.process_sample(-1.0);
        assert!(approx_eq(m.peak_max(), 1.0));
    }

    #[test]
    fn test_rms_dc_signal() {
        let mut m = ChannelMeter::new(1000);
        // 1000 samples of 0.5 amplitude
        for _ in 0..1000 {
            m.process_sample(0.5);
        }
        // RMS of a DC signal = the signal itself
        assert!(approx_eq(m.rms_integrated_linear(), 0.5));
        assert!(approx_eq(m.rms_momentary_linear(), 0.5));
    }

    #[test]
    fn test_rms_sine_wave() {
        let mut m = ChannelMeter::new(48000);
        // One full cycle of a sine at 1.0 amplitude
        let n = 48000;
        for i in 0..n {
            let phase = i as f32 / n as f32;
            m.process_sample((phase * std::f32::consts::TAU).sin());
        }
        // RMS of a sine = 1/sqrt(2) ≈ 0.7071
        let expected = 1.0 / 2.0_f32.sqrt();
        assert!(
            approx_eq(m.rms_integrated_linear(), expected),
            "expected ~{}, got {}",
            expected,
            m.rms_integrated_linear()
        );
    }

    #[test]
    fn test_rms_momentary_window() {
        // Window of 10 samples
        let mut m = ChannelMeter::new(10);

        // Fill with 10 samples of 1.0
        for _ in 0..10 {
            m.process_sample(1.0);
        }
        assert!(approx_eq(m.rms_momentary_linear(), 1.0));

        // Now push 10 samples of 0.0 — window should be all zeros
        for _ in 0..10 {
            m.process_sample(0.0);
        }
        assert!(approx_eq(m.rms_momentary_linear(), 0.0));
    }

    #[test]
    fn test_rms_momentary_max_tracks() {
        let mut m = ChannelMeter::new(10);
        // Feed loud signal
        for _ in 0..10 {
            m.process_sample(1.0);
        }
        m.update_momentary_max();
        assert!(approx_eq(m.rms_momentary_max(), 1.0));

        // Feed quiet signal
        for _ in 0..10 {
            m.process_sample(0.1);
        }
        m.update_momentary_max();
        // Max should still be 1.0
        assert!(approx_eq(m.rms_momentary_max(), 1.0));
    }

    #[test]
    fn test_crest_factor_sine() {
        let mut m = ChannelMeter::new(48000);
        let n = 48000;
        for i in 0..n {
            let phase = i as f32 / n as f32;
            m.process_sample((phase * std::f32::consts::TAU).sin());
        }
        // Crest factor of sine = 20*log10(1.0) - 20*log10(1/sqrt(2)) = 3.01 dB
        let cf = m.crest_factor_db();
        assert!(
            (cf - 3.01).abs() < 0.05,
            "expected ~3.01 dB, got {} dB",
            cf
        );
    }

    #[test]
    fn test_reset_clears_everything() {
        let mut m = ChannelMeter::new(100);
        for _ in 0..100 {
            m.process_sample(0.8);
        }
        m.update_momentary_max();
        assert!(m.peak_max() > 0.0);
        assert!(m.rms_integrated_linear() > 0.0);
        assert!(m.rms_momentary_max() > 0.0);

        m.reset();
        assert_eq!(m.peak_max(), 0.0);
        assert_eq!(m.rms_integrated_linear(), 0.0);
        assert_eq!(m.rms_momentary_linear(), 0.0);
        assert_eq!(m.rms_momentary_max(), 0.0);
    }

    #[test]
    fn test_set_window_size() {
        let mut m = ChannelMeter::new(100);
        for _ in 0..100 {
            m.process_sample(0.5);
        }
        assert!(approx_eq(m.rms_momentary_linear(), 0.5));

        // Resize clears the ring
        m.set_window_size(200);
        assert_eq!(m.rms_momentary_linear(), 0.0);
    }

    #[test]
    fn test_stereo_meter_peak_is_max() {
        let mut m = StereoMeter::new(100);
        let left: Vec<f32> = vec![0.5; 100];
        let right: Vec<f32> = vec![0.8; 100];
        m.process_buffer(&left, &right);

        // Peak is max of L/R
        assert!(approx_eq(m.peak_max_stereo(), 0.8));
    }

    #[test]
    fn test_stereo_rms_sums_power() {
        let mut m = StereoMeter::new(100);
        // Both channels at 0.5: ms_L = 0.25, ms_R = 0.25, sum = 0.5, sqrt = 0.7071
        let left: Vec<f32> = vec![0.5; 100];
        let right: Vec<f32> = vec![0.5; 100];
        m.process_buffer(&left, &right);

        let expected = (0.5_f32).sqrt(); // sqrt(0.25 + 0.25)
        assert!(
            approx_eq(m.rms_integrated_stereo(), expected),
            "expected ~{}, got {}",
            expected,
            m.rms_integrated_stereo()
        );
    }

    #[test]
    fn test_stereo_rms_asymmetric() {
        let mut m = StereoMeter::new(100);
        // Left loud, right silent: ms_L = 1.0, ms_R = 0.0, sum = 1.0, sqrt = 1.0
        let left: Vec<f32> = vec![1.0; 100];
        let right: Vec<f32> = vec![0.0; 100];
        m.process_buffer(&left, &right);

        assert!(
            approx_eq(m.rms_integrated_stereo(), 1.0),
            "expected 1.0, got {}",
            m.rms_integrated_stereo()
        );
    }

    #[test]
    fn test_stereo_rms_equals_single_channel_for_mono() {
        // For a mono signal (L == R), stereo sum should be 3 dB above single channel
        let mut m = StereoMeter::new(100);
        let signal: Vec<f32> = vec![0.5; 100];
        m.process_buffer(&signal, &signal);

        let single_rms = 0.5_f32; // RMS of DC 0.5
        let stereo_rms = m.rms_integrated_stereo();
        let diff_db = linear_to_db(stereo_rms) - linear_to_db(single_rms);
        assert!(
            (diff_db - 3.01).abs() < 0.05,
            "stereo should be ~3 dB above single channel, got {} dB diff",
            diff_db
        );
    }

    #[test]
    fn test_linear_to_db() {
        assert!(approx_eq(linear_to_db(1.0), 0.0));
        assert!(approx_eq(linear_to_db(0.5), -6.0206));
        assert_eq!(linear_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn test_db_to_linear() {
        assert!(approx_eq(db_to_linear(0.0), 1.0));
        assert!(approx_eq(db_to_linear(-6.0206), 0.5));
    }

    #[test]
    fn test_db_roundtrip() {
        for db in [-40.0, -20.0, -6.0, 0.0, 6.0, 20.0] {
            let rt = linear_to_db(db_to_linear(db));
            assert!(
                (rt - db).abs() < 0.001,
                "roundtrip failed for {} dB: got {}",
                db,
                rt
            );
        }
    }

    #[test]
    fn test_integrated_rms_uses_f64_accumulator() {
        // Process a large number of samples to verify f64 accumulator doesn't lose precision
        let mut m = ChannelMeter::new(100);
        let n = 1_000_000;
        for _ in 0..n {
            m.process_sample(0.1);
        }
        // RMS of DC 0.1 should be 0.1
        assert!(
            approx_eq(m.rms_integrated_linear(), 0.1),
            "expected 0.1, got {}",
            m.rms_integrated_linear()
        );
    }

    #[test]
    fn test_true_peak_ge_sample_peak() {
        // True peak should always be >= sample peak
        let mut m = ChannelMeter::new(100);
        let n = 4800;
        for i in 0..n {
            let phase = i as f32 / n as f32;
            m.process_sample((phase * std::f32::consts::TAU).sin());
        }
        assert!(
            m.true_peak_max() >= m.peak_max(),
            "true peak {} should be >= sample peak {}",
            m.true_peak_max(),
            m.peak_max()
        );
    }

    #[test]
    fn test_true_peak_detects_intersample() {
        // 3 samples per cycle: phases 0, 120°, 240°.
        // Peak at 90° falls between samples. Sample peak = sin(120°) = 0.866.
        // True peak should detect the actual peak closer to 1.0.
        let mut m = ChannelMeter::new(100);
        let sr = 3.0_f64;
        let freq = 1.0_f64;
        // Run several cycles to let the filter settle
        for i in 0..30 {
            let t = i as f64 / sr;
            let sample = (t * freq * std::f64::consts::TAU).sin() as f32;
            m.process_sample(sample);
        }
        let sample_peak = m.peak_max();
        let true_peak = m.true_peak_max();
        assert!(
            (sample_peak - 0.866).abs() < 0.01,
            "sample peak {} should be ~0.866 (sin 120°)",
            sample_peak
        );
        assert!(
            true_peak > sample_peak,
            "true peak {} should be > sample peak {} (inter-sample detection)",
            true_peak,
            sample_peak
        );
    }

    #[test]
    fn test_true_peak_at_realistic_frequency() {
        // 15kHz sine at 48kHz — 3.2 samples per cycle.
        // Sample peak will be noticeably below 1.0, true peak should be closer to 1.0.
        let mut m = ChannelMeter::new(100);
        let sr = 48000.0_f64;
        let freq = 15000.0_f64;
        for i in 0..48000 {
            let t = i as f64 / sr;
            m.process_sample((t * freq * std::f64::consts::TAU).sin() as f32);
        }
        let sp = m.peak_max();
        let tp = m.true_peak_max();
        let diff_db = linear_to_db(tp) - linear_to_db(sp);
        assert!(
            tp > sp,
            "at 15kHz/48kHz: true peak {} should exceed sample peak {}",
            tp, sp
        );
        assert!(
            diff_db > 0.01,
            "true peak should be measurably above sample peak (diff = {} dB)",
            diff_db
        );
    }

    #[test]
    fn test_true_peak_ge_sample_peak_dc() {
        // For DC, true peak >= sample peak. Startup Gibbs ringing causes a
        // transient overshoot at the 0→DC step edge — this is correct behavior
        // (the filter detects the real inter-sample overshoot of the step function).
        let mut m = ChannelMeter::new(100);
        for _ in 0..1000 {
            m.process_sample(0.5);
        }
        assert!(
            m.true_peak_max() >= m.peak_max(),
            "true peak {} should be >= sample peak {}",
            m.true_peak_max(),
            m.peak_max()
        );
    }

    #[test]
    fn test_true_peak_reset() {
        let mut det = TruePeakDetector::new();
        det.process_sample(1.0);
        assert!(det.true_peak_max() > 0.0);
        det.reset();
        assert_eq!(det.true_peak_max(), 0.0);
    }
}
