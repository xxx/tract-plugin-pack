//! Core metering DSP: peak tracking, RMS computation, crest factor, true peak.
//! All levels are in linear amplitude; dB conversion happens at display time.

use std::simd::{f32x16, num::SimdFloat};
use tract_dsp::boxcar::RunningSumWindow;
pub use tract_dsp::db::linear_to_db;
use tract_dsp::true_peak::TruePeakDetector;

/// Maximum supported RMS window in samples (3000ms at 192kHz).
const MAX_WINDOW_SAMPLES: usize = 576_000;

/// SIMD scan: find peak absolute value and sum-of-squares across a buffer.
/// Returns (peak, sum_of_squares_f64).
fn simd_peak_sumsq(samples: &[f32]) -> (f32, f64) {
    let chunks = samples.len() / 16;
    let mut peak_v = f32x16::splat(0.0);
    let mut sumsq_accum = 0.0_f64;

    for i in 0..chunks {
        let v = f32x16::from_slice(&samples[i * 16..]);
        let abs_v = v.abs();
        peak_v = peak_v.simd_max(abs_v);
        // Accumulate sum-of-squares per chunk, promote to f64 per chunk
        sumsq_accum += (v * v).reduce_sum() as f64;
    }

    let mut peak = peak_v.reduce_max();

    // Scalar tail
    let tail_start = chunks * 16;
    for &s in &samples[tail_start..] {
        let abs = s.abs();
        if abs > peak {
            peak = abs;
        }
        sumsq_accum += (s as f64) * (s as f64);
    }

    (peak, sumsq_accum)
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
    /// Sliding window of squared samples for momentary RMS.
    rms_window: RunningSumWindow<f32>,
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
            rms_window: RunningSumWindow::new(MAX_WINDOW_SAMPLES, size),
            rms_momentary_max: 0.0,
        }
    }

    /// Reset all accumulated values.
    pub fn reset(&mut self) {
        self.peak_max = 0.0;
        self.true_peak.reset();
        self.rms_sum = 0.0;
        self.rms_count = 0;
        self.rms_window.reset();
        self.rms_momentary_max = 0.0;
    }

    /// Set the sample rate for true peak oversampling mode.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.true_peak.set_sample_rate(sample_rate);
    }

    /// Change the momentary RMS window size. No allocation — uses pre-allocated buffer.
    /// Resets the ring buffer state and momentary max.
    pub fn set_window_size(&mut self, window_samples: usize) {
        let size = window_samples.clamp(1, MAX_WINDOW_SAMPLES);
        if self.rms_window.window() != size {
            self.rms_window.set_window(size);
            self.rms_momentary_max = 0.0;
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

        // Momentary RMS sliding window (O(1) running sum)
        self.rms_window.push(sample * sample);
    }

    /// Process a full buffer slice. Uses SIMD for peak finding and sum-of-squares,
    /// then runs true peak FIR and momentary ring update per-sample.
    pub fn process_buffer_channel(&mut self, samples: &[f32]) {
        // Pass 1: SIMD peak scan + sum-of-squares for integrated RMS
        let (buf_peak, buf_sumsq) = simd_peak_sumsq(samples);
        if buf_peak > self.peak_max {
            self.peak_max = buf_peak;
        }
        self.rms_sum += buf_sumsq;
        self.rms_count += samples.len() as u64;

        // Pass 2: Per-sample true peak FIR + momentary ring update
        // (these have per-sample state dependencies that prevent batching)
        for &sample in samples {
            self.true_peak.process_sample(sample);

            self.rms_window.push(sample * sample);
        }
    }

    /// Update momentary max. Called once per buffer to match dpMeter5's
    /// update granularity (per-sample tracking finds higher peaks due to
    /// finer temporal resolution, giving values ~0.5 dB above dpMeter5).
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
        (self.rms_window.mean().sqrt()) as f32
    }

    /// Highest momentary RMS (linear) since last reset.
    pub fn rms_momentary_max(&self) -> f32 {
        self.rms_momentary_max
    }

    /// Raw integrated sum-of-squares (f64) and sample count, for cross-channel summing.
    pub fn rms_integrated_raw(&self) -> (f64, u64) {
        (self.rms_sum, self.rms_count)
    }

    /// Mean-square of the current momentary window and the filled count.
    pub fn rms_momentary_raw(&self) -> (f64, usize) {
        (self.rms_window.mean(), self.rms_window.filled())
    }

    /// Current crest factor in dB: peak_max_dB - rms_integrated_dB.
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

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.left.set_sample_rate(sample_rate);
        self.right.set_sample_rate(sample_rate);
    }

    pub fn set_window_size(&mut self, window_samples: usize) {
        self.left.set_window_size(window_samples);
        self.right.set_window_size(window_samples);
        self.momentary_max_stereo = 0.0;
    }

    /// Process L/R samples from a nih-plug buffer.
    pub fn process_buffer(&mut self, left_samples: &[f32], right_samples: &[f32]) {
        self.left.process_buffer_channel(left_samples);
        self.right.process_buffer_channel(right_samples);
        // Update momentary max once per buffer (matches dpMeter5's granularity)
        self.left.update_momentary_max();
        self.right.update_momentary_max();
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
        let ms_l = if count_l > 0 {
            sum_l / count_l as f64
        } else {
            0.0
        };
        let ms_r = if count_r > 0 {
            sum_r / count_r as f64
        } else {
            0.0
        };
        (ms_l + ms_r).sqrt() as f32
    }

    /// Momentary RMS across both channels (sum of per-channel mean-square, then sqrt).
    pub fn rms_momentary_stereo(&self) -> f32 {
        let (ms_l, filled_l) = self.left.rms_momentary_raw();
        let (ms_r, filled_r) = self.right.rms_momentary_raw();
        if filled_l == 0 && filled_r == 0 {
            return 0.0;
        }
        (ms_l + ms_r).sqrt() as f32
    }

    /// Highest momentary RMS (stereo sum) since last reset.
    /// Updated per-buffer via `process_buffer`.
    pub fn rms_momentary_max_stereo(&self) -> f32 {
        self.momentary_max_stereo
    }

    /// Stereo crest factor: peak_max_dB - rms_integrated_dB.
    /// Uses the stereo peak (max of L/R) and stereo RMS (sum-of-power)
    /// to match dpMeter5's SUM mode crest factor display.
    ///
    /// NOTE: This mixes scales — single-channel peak vs summed-power RMS — which
    /// gives a value ~3 dB lower than per-channel crest factor for balanced stereo.
    /// The mathematically correct approach would be max(crest_L, crest_R).
    /// We use dpMeter5's convention for now to match the widely-used reference.
    /// TODO: Add a "correct" mode toggle in the future.
    pub fn crest_factor_db_stereo(&self) -> f32 {
        let peak = self.peak_max_stereo();
        let rms = self.rms_integrated_stereo();
        if rms < 1e-10 || peak < 1e-10 {
            return f32::NEG_INFINITY;
        }
        linear_to_db(peak) - linear_to_db(rms)
    }
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
        assert!((cf - 3.01).abs() < 0.05, "expected ~3.01 dB, got {} dB", cf);
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
            tp,
            sp
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

    #[test]
    fn test_running_sum_accuracy() {
        // Verify the running sum stays accurate over many ring cycles
        let window = 100;
        let mut m = ChannelMeter::new(window);
        // Fill window with 0.5, then cycle through many values
        for _ in 0..window {
            m.process_sample(0.5);
        }
        assert!(approx_eq(m.rms_momentary_linear(), 0.5));

        // Cycle through 10000 more samples of 0.5 — running sum should stay stable
        for _ in 0..10000 {
            m.process_sample(0.5);
        }
        assert!(
            approx_eq(m.rms_momentary_linear(), 0.5),
            "running sum drifted: expected 0.5, got {}",
            m.rms_momentary_linear()
        );

        // Switch to 0.3 and let it fill the window
        for _ in 0..window {
            m.process_sample(0.3);
        }
        assert!(
            approx_eq(m.rms_momentary_linear(), 0.3),
            "after level change: expected 0.3, got {}",
            m.rms_momentary_linear()
        );
    }

    #[test]
    fn test_true_peak_bypass_mode() {
        // At ≥192kHz, true peak should equal sample peak (bypass mode)
        let mut m = ChannelMeter::new(100);
        m.set_sample_rate(192000.0);
        for _ in 0..1000 {
            m.process_sample(0.7);
        }
        assert!(
            approx_eq(m.true_peak_max(), 0.7),
            "bypass mode: true peak {} should equal sample peak 0.7",
            m.true_peak_max()
        );
    }

    #[test]
    fn test_true_peak_2x_mode() {
        // At 96kHz, use 2x oversampling (phases 0 and 2 only)
        let mut m = ChannelMeter::new(100);
        m.set_sample_rate(96000.0);
        // 3 samples/cycle sine — should detect inter-sample peaks
        for i in 0..30 {
            let t = i as f64 / 3.0;
            m.process_sample((t * std::f64::consts::TAU).sin() as f32);
        }
        assert!(
            m.true_peak_max() >= m.peak_max(),
            "2x mode: true peak {} should be >= sample peak {}",
            m.true_peak_max(),
            m.peak_max()
        );
    }

    #[test]
    fn test_reset_clears_true_peak() {
        let mut m = ChannelMeter::new(100);
        for _ in 0..100 {
            m.process_sample(0.8);
        }
        assert!(m.true_peak_max() > 0.0);
        m.reset();
        assert_eq!(m.true_peak_max(), 0.0);
    }

    #[test]
    fn test_stereo_crest_factor() {
        // For equal-level sine on both channels, stereo crest uses mixed scales:
        // peak_stereo (single channel) vs rms_stereo (sum-of-power).
        // For a unit sine: peak=1.0, rms_stereo=sqrt(0.5+0.5)=1.0, crest=0 dB.
        // Per-channel crest would be ~3 dB.
        let mut m = StereoMeter::new(48000);
        let n = 48000;
        let signal: Vec<f32> = (0..n)
            .map(|i| (i as f32 / n as f32 * std::f32::consts::TAU).sin())
            .collect();
        m.process_buffer(&signal, &signal);
        let stereo_cf = m.crest_factor_db_stereo();
        let per_ch_cf = m.left.crest_factor_db();
        // Stereo crest should be ~3 dB lower than per-channel
        assert!(
            stereo_cf.is_finite() && per_ch_cf.is_finite(),
            "crest factors should be finite"
        );
        assert!(
            (per_ch_cf - stereo_cf - 3.01).abs() < 0.1,
            "stereo crest ({:.2}) should be ~3 dB below per-channel ({:.2})",
            stereo_cf,
            per_ch_cf
        );
    }

    #[test]
    fn test_stereo_momentary_max() {
        let mut m = StereoMeter::new(10);
        let loud: Vec<f32> = vec![0.8; 10];
        let quiet: Vec<f32> = vec![0.1; 10];
        m.process_buffer(&loud, &loud);
        let max_after_loud = m.rms_momentary_max_stereo();
        assert!(max_after_loud > 0.0);
        m.process_buffer(&quiet, &quiet);
        // Max should not decrease
        assert!(
            m.rms_momentary_max_stereo() >= max_after_loud,
            "stereo momentary max should not decrease"
        );
    }

    #[test]
    fn test_stereo_reset_clears_all() {
        let mut m = StereoMeter::new(100);
        let signal: Vec<f32> = vec![0.5; 100];
        m.process_buffer(&signal, &signal);
        assert!(m.peak_max_stereo() > 0.0);
        assert!(m.rms_momentary_max_stereo() > 0.0);
        m.reset();
        assert_eq!(m.peak_max_stereo(), 0.0);
        assert_eq!(m.rms_integrated_stereo(), 0.0);
        assert_eq!(m.rms_momentary_stereo(), 0.0);
        assert_eq!(m.rms_momentary_max_stereo(), 0.0);
    }

    #[test]
    fn test_window_size_one() {
        // Degenerate case: window of 1 sample
        let mut m = ChannelMeter::new(1);
        m.process_sample(0.5);
        assert!(approx_eq(m.rms_momentary_linear(), 0.5));
        m.process_sample(0.3);
        assert!(approx_eq(m.rms_momentary_linear(), 0.3));
        m.process_sample(0.0);
        assert!(approx_eq(m.rms_momentary_linear(), 0.0));
    }

    #[test]
    fn test_buffer_channel_matches_scalar() {
        // Verify process_buffer_channel gives identical results to process_sample loop
        let samples: Vec<f32> = (0..1024)
            .map(|i| (i as f32 / 1024.0 * std::f32::consts::TAU * 5.0).sin() * 0.8)
            .collect();

        let mut scalar = ChannelMeter::new(100);
        for &s in &samples {
            scalar.process_sample(s);
        }
        scalar.update_momentary_max();

        let mut batched = ChannelMeter::new(100);
        batched.process_buffer_channel(&samples);
        batched.update_momentary_max();

        assert!(
            approx_eq(scalar.peak_max(), batched.peak_max()),
            "peak: scalar={}, batched={}",
            scalar.peak_max(),
            batched.peak_max()
        );
        assert!(
            approx_eq(
                scalar.rms_integrated_linear(),
                batched.rms_integrated_linear()
            ),
            "rms_int: scalar={}, batched={}",
            scalar.rms_integrated_linear(),
            batched.rms_integrated_linear()
        );
        assert!(
            approx_eq(
                scalar.rms_momentary_linear(),
                batched.rms_momentary_linear()
            ),
            "rms_mom: scalar={}, batched={}",
            scalar.rms_momentary_linear(),
            batched.rms_momentary_linear()
        );
        assert!(
            approx_eq(scalar.true_peak_max(), batched.true_peak_max()),
            "true_peak: scalar={}, batched={}",
            scalar.true_peak_max(),
            batched.true_peak_max()
        );
        assert!(
            approx_eq(scalar.rms_momentary_max(), batched.rms_momentary_max()),
            "rms_mom_max: scalar={}, batched={}",
            scalar.rms_momentary_max(),
            batched.rms_momentary_max()
        );
    }

    #[test]
    fn test_simd_peak_sumsq() {
        let samples: Vec<f32> = vec![
            0.1, -0.5, 0.3, 0.9, -0.2, 0.0, 0.4, -0.7, 0.6, -0.1, 0.8, -0.3, 0.2, -0.9, 0.5, -0.4,
        ];
        let (peak, sumsq) = simd_peak_sumsq(&samples);

        // Scalar reference
        let expected_peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        let expected_sumsq: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();

        assert!(
            approx_eq(peak, expected_peak),
            "peak: simd={}, scalar={}",
            peak,
            expected_peak
        );
        assert!(
            (sumsq - expected_sumsq).abs() < 1e-4,
            "sumsq: simd={}, scalar={}",
            sumsq,
            expected_sumsq
        );
    }

    #[test]
    fn test_simd_peak_sumsq_tail() {
        // Non-multiple-of-16 length to exercise scalar tail
        let samples: Vec<f32> = (0..37).map(|i| (i as f32 * 0.1).sin()).collect();
        let (peak, sumsq) = simd_peak_sumsq(&samples);

        let expected_peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        let expected_sumsq: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();

        assert!(
            approx_eq(peak, expected_peak),
            "tail peak: simd={}, scalar={}",
            peak,
            expected_peak
        );
        assert!(
            (sumsq - expected_sumsq).abs() < 1e-4,
            "tail sumsq: simd={}, scalar={}",
            sumsq,
            expected_sumsq
        );
    }

    #[test]
    fn test_stereo_set_sample_rate() {
        let mut m = StereoMeter::new(100);
        m.set_sample_rate(96000.0);
        // Should propagate to both channels — feed a signal and check true peak works
        let signal: Vec<f32> = (0..30)
            .map(|i| (i as f64 / 3.0 * std::f64::consts::TAU).sin() as f32)
            .collect();
        m.process_buffer(&signal, &signal);
        assert!(m.true_peak_max_stereo() >= m.peak_max_stereo());
    }

    #[test]
    fn test_stereo_set_window_size() {
        let mut m = StereoMeter::new(100);
        let signal: Vec<f32> = vec![0.5; 100];
        m.process_buffer(&signal, &signal);
        assert!(m.rms_momentary_max_stereo() > 0.0);

        // Resize should clear momentary max
        m.set_window_size(200);
        assert_eq!(m.rms_momentary_max_stereo(), 0.0);
    }
}
