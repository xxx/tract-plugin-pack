//! Spectral detail preservation with headroom beneath a fixed output ceiling.
//!
//! Peak-relative STFT masking extracts quiet components; an inverse FFT
//! reconstructs their waveform. Recent waveform peaks reserve steady headroom
//! with a smooth release. The protected component bypasses the waveshaper,
//! whose internal ceiling is lowered by that reservation. Gain applies to both
//! paths. Excessive detail is scaled to at most half the ceiling so it cannot
//! invert a saturated lobe.
//!
//! Detail engages smoothly between one and two times the output threshold;
//! samples below the threshold retain the ordinary hard/soft waveshaper output.
//! FFT size 2048 and hop 512 give the plugin its fixed 2048-sample latency.

use crate::stft_analysis::StftAnalyzer;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// Ratio of spectral peak above which bins are classified as 100% loud.
/// -6 dB relative to the frame's peak magnitude.
const LOUD_RATIO: f32 = 0.5;

/// Ratio of spectral peak below which bins are classified as 100% quiet.
/// -20 dB relative to the frame's peak magnitude.
const QUIET_RATIO: f32 = 0.1;

/// Time-domain saturation with gain and threshold.
///
/// `gain` boosts the input signal. `threshold` sets the clip ceiling (linear).
/// `knee` controls the clipping shape:
/// - `knee=0`: hard clip at ±threshold
/// - `knee=1`: soft clip (tanh) at ±threshold
/// - In between: linear crossfade between hard and soft
///
/// Small signals (gain*x << threshold) pass through unchanged.
/// Loud signals (gain*x >> threshold) are clipped at ±threshold.
#[inline]
pub fn saturate_td(x: f32, gain: f32, threshold: f32, knee: f32) -> f32 {
    let gained = gain * x;
    let hard = gained.clamp(-threshold, threshold);
    let soft = threshold * (gained / threshold).tanh();
    hard + knee * (soft - hard)
}

/// Like [`saturate_td`], also returning the normalized tanh transfer value.
#[inline]
pub fn saturate_td_with_tanh(x: f32, gain: f32, threshold: f32, knee: f32) -> (f32, f32) {
    saturate_td_with_tanh_fast(x, gain, threshold, 1.0 / threshold, knee)
}

/// Fast variant of [`saturate_td_with_tanh`] that takes a precomputed
/// `inv_threshold = 1.0 / threshold` to avoid a per-call division.
///
/// Used on the audio hot path where `inv_threshold` is computed once per
/// sample and reused across both channels and the spectral path.
#[inline]
pub fn saturate_td_with_tanh_fast(
    x: f32,
    gain: f32,
    threshold: f32,
    inv_threshold: f32,
    knee: f32,
) -> (f32, f32) {
    let gained = gain * x;
    let norm = gained * inv_threshold;
    let hard = gained.clamp(-threshold, threshold);
    let tanh_val = norm.tanh();
    let soft = threshold * tanh_val;
    (hard + knee * (soft - hard), tanh_val)
}

struct DetailPeak {
    block_peaks: Vec<f32>,
    block_index: usize,
    samples_in_block: usize,
    hop_size: usize,
    window_peak: f32,
    held_peak: f32,
    release: f32,
    active: bool,
}

impl DetailPeak {
    fn new(fft_size: usize, hop_size: usize) -> Self {
        Self {
            block_peaks: vec![0.0; fft_size / hop_size + 1],
            block_index: 0,
            samples_in_block: 0,
            hop_size,
            window_peak: 0.0,
            held_peak: 0.0,
            // Recover about 98% of an expired peak over one FFT window.
            release: (-4.0 / fft_size as f32).exp(),
            active: false,
        }
    }

    fn reset(&mut self) {
        self.clear_peaks();
        self.block_index = 0;
        self.samples_in_block = 0;
    }

    fn clear_peaks(&mut self) {
        self.block_peaks.fill(0.0);
        self.window_peak = 0.0;
        self.held_peak = 0.0;
        self.active = false;
    }

    fn advance(&mut self) {
        self.samples_in_block += 1;
        if self.samples_in_block == self.hop_size {
            self.samples_in_block = 0;
            self.block_index = (self.block_index + 1) % self.block_peaks.len();
            self.block_peaks[self.block_index] = 0.0;
            self.window_peak = self.block_peaks.iter().copied().fold(0.0, f32::max);
        }
    }

    fn process(&mut self, quiet: f32, enabled: bool) -> f32 {
        if !enabled {
            if self.active {
                self.clear_peaks();
            }
            // Bypass keeps block boundaries aligned with continuously active processing.
            self.advance();
            return 0.0;
        }
        self.active = true;
        let magnitude = quiet.abs();
        self.block_peaks[self.block_index] = self.block_peaks[self.block_index].max(magnitude);
        self.window_peak = self.window_peak.max(magnitude);
        self.held_peak = self.window_peak.max(self.held_peak * self.release);
        let output = self.held_peak;

        self.advance();
        output
    }
}

/// Allocation-free streaming spectral clipper with a latency of `fft_size` samples.
///
/// The analyzer's input ring also supplies the exact delayed dry signal.
/// An overlap-add ring supplies the quiet waveform; recent block peaks hold
/// its headroom steady without introducing another audio delay.
pub struct SpectralClipper {
    fft_size: usize,
    hop_size: usize,

    /// Shared STFT analysis front-end (input ring, analysis window, forward
    /// FFT, COLA synthesis window).
    stft: StftAnalyzer,

    // Inverse FFT plan + its in-place scratch.
    fft_inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,

    // Pre-allocated workspace for per-bin magnitudes (avoids recomputing norm)
    mag_buf: Vec<f32>,

    // Ring buffers
    /// Reconstructed quiet component.
    quiet_output_ring: Vec<f64>,
    detail_peak: DetailPeak,
    /// Current read position in output rings.
    read_pos: usize,
    /// Sample counter within current hop.
    hop_counter: usize,

    // Pre-allocated inverse-FFT (ISTFT) input buffers
    /// Conjugate-symmetric quiet spectrum for inverse FFT.
    quiet_buf: Vec<Complex<f32>>,
}

impl SpectralClipper {
    /// Create a new `SpectralClipper`.
    ///
    /// - `fft_size`: FFT frame length (e.g. 2048).
    /// - `hop_size`: hop between successive frames (e.g. 512 for 75% overlap).
    pub fn new(fft_size: usize, hop_size: usize) -> Self {
        assert!(fft_size > 0 && hop_size > 0 && fft_size >= hop_size);

        let mut planner = FftPlanner::new();
        let fft_inverse = planner.plan_fft_inverse(fft_size);
        let scratch_len = fft_inverse.get_inplace_scratch_len();

        let out_ring_size = 2 * fft_size;

        Self {
            fft_size,
            hop_size,
            stft: StftAnalyzer::new(fft_size, hop_size),
            fft_inverse,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            mag_buf: vec![0.0; fft_size],
            quiet_output_ring: vec![0.0; out_ring_size],
            detail_peak: DetailPeak::new(fft_size, hop_size),
            read_pos: 0,
            hop_counter: 0,
            quiet_buf: vec![Complex::new(0.0, 0.0); fft_size],
        }
    }

    /// Latency in samples introduced by the STFT process.
    pub fn latency_samples(&self) -> usize {
        self.fft_size
    }

    /// Reset all internal state (ring buffers, counters) to zero.
    pub fn reset(&mut self) {
        self.stft.reset();
        self.quiet_output_ring.fill(0.0);
        self.detail_peak.reset();
        self.read_pos = 0;
        self.hop_counter = 0;
        for bin in self.quiet_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
    }

    /// Process one sample with full detail preservation.
    pub fn process_sample(&mut self, input: f32, gain: f32, threshold: f32, knee: f32) -> f32 {
        self.process_sample_with_detail(input, gain, threshold, 1.0, knee)
    }

    /// Process ordinary saturation without FFT work, while maintaining ring timing.
    pub fn process_sample_skip_fft(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        knee: f32,
    ) -> f32 {
        self.process_sample_with_detail(input, gain, threshold, 0.0, knee)
    }

    /// Full-detail processing with a precomputed reciprocal of `threshold`.
    #[inline]
    pub fn process_sample_fast(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        inv_threshold: f32,
        knee: f32,
    ) -> f32 {
        self.process_sample_with_detail_fast(input, gain, threshold, inv_threshold, 1.0, knee)
    }

    /// FFT-skipped processing with a precomputed reciprocal of `threshold`.
    #[inline]
    pub fn process_sample_skip_fft_fast(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        inv_threshold: f32,
        knee: f32,
    ) -> f32 {
        self.process_sample_with_detail_fast(input, gain, threshold, inv_threshold, 0.0, knee)
    }

    /// Process one sample with `detail` in `0..=1`, automatically skipping FFTs at zero.
    ///
    /// Re-enabling detail fills the spectral output over approximately four hops;
    /// the delayed dry path remains complete throughout that transition.
    pub fn process_sample_with_detail(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        detail: f32,
        knee: f32,
    ) -> f32 {
        self.process_sample_with_detail_fast(input, gain, threshold, 1.0 / threshold, detail, knee)
    }

    /// Like [`Self::process_sample_with_detail`] with a precomputed threshold reciprocal.
    #[inline]
    pub fn process_sample_with_detail_fast(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        inv_threshold: f32,
        detail: f32,
        knee: f32,
    ) -> f32 {
        let dry = self.stft.delayed_sample();
        self.stft.write(input);

        let quiet = self.quiet_output_ring[self.read_pos] as f32;
        self.quiet_output_ring[self.read_pos] = 0.0;
        self.read_pos = (self.read_pos + 1) % self.quiet_output_ring.len();
        self.hop_counter += 1;
        if self.hop_counter >= self.hop_size {
            self.hop_counter = 0;
            if detail > 0.0 {
                self.process_frame();
            }
        }

        let quiet_peak = self.detail_peak.process(quiet, detail > 0.0);
        let (standard, _) = saturate_td_with_tanh_fast(dry, gain, threshold, inv_threshold, knee);
        let gained = gain * dry;
        if detail <= 0.0 || gained.abs() <= threshold {
            return standard;
        }

        // Hold waveform peaks so headroom does not modulate the quiet oscillations.
        let amount = gain.abs() * detail.clamp(0.0, 1.0);
        let mut headroom = amount * quiet_peak;
        let mut preserved = gain.signum() * amount * quiet;
        // Soft-knee rounding also reduces the body available to support the detail.
        let maximum_headroom = 0.5 * standard.abs();
        if headroom > maximum_headroom {
            preserved *= maximum_headroom / headroom;
            headroom = maximum_headroom;
        }
        if headroom == 0.0 {
            return standard;
        }

        // Remove the protected component before shaping so it is added only once.
        let inner_threshold = threshold - headroom;
        let shaped = saturate_td(gained - preserved, 1.0, inner_threshold, knee) + preserved;
        let activation = (gained.abs() * inv_threshold - 1.0).clamp(0.0, 1.0);
        let blend = activation * activation * (3.0 - 2.0 * activation);
        (standard + blend * (shaped - standard)).clamp(-threshold, threshold)
    }

    /// Reconstruct the quiet signal with a peak-relative spectral mask.
    fn process_frame(&mut self) {
        let n = self.fft_size;
        let out_len = self.quiet_output_ring.len();

        // 1+2. Analysis front-end (windowed extract + forward FFT) is owned by
        //      the StftAnalyzer. Normalize the spectrum by 1/N and cache
        //      magnitudes. Using norm_sqr (no sqrt) for threshold comparisons
        //      eliminates ~4096 sqrt calls per frame; sqrt is only computed for
        //      the small number of bins in the transition band.
        let frame = self.stft.analyze();

        let inv_n = 1.0 / n as f32;
        let mut max_mag_sq = 0.0_f32;
        for k in 0..n {
            frame.spectrum[k] *= inv_n;
            let mag_sq = frame.spectrum[k].norm_sqr();
            self.mag_buf[k] = mag_sq;
            if mag_sq > max_mag_sq {
                max_mag_sq = mag_sq;
            }
        }

        // 3. Split bins using peak-relative threshold (squared for comparison).
        //    The threshold adapts to the actual spectral content, ensuring
        //    detail bins go to the quiet path regardless of drive level.
        //    - Above LOUD_RATIO * peak (-6 dB): 100% loud (dominant components)
        //    - Below QUIET_RATIO * peak (-20 dB): 100% quiet (detail)
        //    - Between: smooth crossfade (14 dB transition band)
        let hi_sq = max_mag_sq * (LOUD_RATIO * LOUD_RATIO);
        let lo_sq = max_mag_sq * (QUIET_RATIO * QUIET_RATIO);
        // For the transition band crossfade, we need linear magnitudes.
        // Compute hi/lo from sqrt only once (not per bin).
        let max_mag = max_mag_sq.sqrt();
        let hi = max_mag * LOUD_RATIO;
        let lo = max_mag * QUIET_RATIO;
        let inv_band = if hi > lo { 1.0 / (hi - lo) } else { 1.0 };

        for k in 0..n {
            let mag_sq = self.mag_buf[k];
            if mag_sq >= hi_sq {
                // Clearly loud — 100% to loud path
                self.quiet_buf[k] = Complex::new(0.0, 0.0);
            } else if mag_sq <= lo_sq {
                // Clearly quiet — 100% to quiet path (detail preserved)
                self.quiet_buf[k] = frame.spectrum[k];
            } else {
                // Transition band — smooth crossfade (sqrt only here).
                let mag = mag_sq.sqrt();
                let t = (mag - lo) * inv_band; // 0 at lo, 1 at hi
                self.quiet_buf[k] = frame.spectrum[k] * (1.0 - t);
            }
        }

        self.fft_inverse
            .process_with_scratch(&mut self.quiet_buf, &mut self.scratch);

        for i in 0..n {
            let out_idx = (self.read_pos + i) % out_len;
            let w = frame.synthesis_window[i];
            self.quiet_output_ring[out_idx] += (self.quiet_buf[i].re * w) as f64;
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48000.0;

    #[test]
    fn hard_knee_preserves_bipolar_sign_below_the_threshold() {
        for input in [-0.25, 0.25] {
            let mut clipper = SpectralClipper::new(32, 8);
            for i in 0..128 {
                let output = clipper.process_sample(input, 1.0, 0.5, 0.0);
                if i >= 64 {
                    assert!((output - input).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn oversized_detail_is_scaled_without_clipping_or_inverting_the_lobe() {
        for sign in [-1.0, 1.0] {
            let mut clipper = SpectralClipper::new(64, 16);
            for sample in 0..1024 {
                let angle = std::f64::consts::TAU * 8.0 * sample as f64 / 64.0;
                let input = 0.8 * sign + 0.1 * angle.sin() as f32;
                let output = clipper.process_sample(input, 16.0, 0.4, 0.0);
                if sample >= 256 {
                    let expected = 0.2 * sign + 0.2 * angle.sin() as f32;
                    assert!((output - expected).abs() < 2e-6, "{output} vs {expected}");
                    assert!(output.abs() <= 0.4);
                    assert!(output * sign >= -1e-6);
                }
            }
        }
    }

    #[test]
    fn transition_band_uses_peak_relative_linear_interpolation() {
        let mut clipper = SpectralClipper::new(64, 16);
        for i in 0..64 {
            let angle = std::f32::consts::TAU * i as f32 / 64.0;
            clipper
                .stft
                .write((4.0 * angle).cos() + 0.3 * (12.0 * angle).cos());
        }
        clipper.process_frame();
        for (i, &actual) in clipper.quiet_output_ring[..64].iter().enumerate() {
            let angle = std::f32::consts::TAU * i as f32 / 64.0;
            let quiet = 0.075 * (12.0 * angle).cos()
                - 0.065625 * ((11.0 * angle).cos() + (13.0 * angle).cos());
            let synthesis_window = (1.0 - angle.cos()) / 3.0;
            assert!(
                (actual as f32 - quiet * synthesis_window).abs() < 1e-6,
                "sample {i}: {actual}"
            );
        }
    }

    #[test]
    fn fft_runs_only_after_a_complete_hop() {
        let mut clipper = SpectralClipper::new(32, 8);
        for i in 1..=17 {
            clipper.process_sample(0.25, 1.0, 1.0, 0.0);
            assert_eq!(clipper.hop_counter, i % 8);
            if i < 8 {
                assert!(
                    clipper
                        .quiet_output_ring
                        .iter()
                        .all(|&sample| sample == 0.0)
                );
            }
        }
    }

    #[test]
    fn saturation_variants_match_at_nonunit_thresholds() {
        for threshold in [0.25, 0.8, 2.0] {
            for gain in [0.5, 3.0] {
                for knee in [0.0, 0.4, 1.0] {
                    for input in [-1.2, -0.1, 0.0, 0.3, 1.2] {
                        let expected = saturate_td(input, gain, threshold, knee);
                        let expected_tanh = (f64::from(input) * f64::from(gain)
                            / f64::from(threshold))
                        .tanh() as f32;
                        for (output, tanh) in [
                            saturate_td_with_tanh(input, gain, threshold, knee),
                            saturate_td_with_tanh_fast(
                                input,
                                gain,
                                threshold,
                                1.0 / threshold,
                                knee,
                            ),
                        ] {
                            assert!((output - expected).abs() < 1e-6);
                            assert!((tanh - expected_tanh).abs() < 1e-6);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fast_processing_matches_reference_across_fft_skip_transitions() {
        let mut reference = SpectralClipper::new(64, 16);
        let mut fast = SpectralClipper::new(64, 16);
        let threshold = 0.37;
        for i in 0..1024 {
            let input = (i as f32 * 0.17).sin();
            let (expected, output) = if (256..512).contains(&i) {
                (
                    reference.process_sample_skip_fft(input, 1.5, threshold, 0.4),
                    fast.process_sample_skip_fft_fast(input, 1.5, threshold, 1.0 / threshold, 0.4),
                )
            } else {
                (
                    reference.process_sample(input, 1.5, threshold, 0.4),
                    fast.process_sample_fast(input, 1.5, threshold, 1.0 / threshold, 0.4),
                )
            };
            assert_eq!(output, expected, "sample {i}");
        }
    }

    #[test]
    fn reset_restarts_clipper_without_old_tail() {
        let mut used = SpectralClipper::new(64, 16);
        let mut fresh = SpectralClipper::new(64, 16);
        for i in 0..263 {
            used.process_sample((i as f32 * 0.17).sin(), 1.5, 0.37, 0.4);
        }
        used.reset();
        for i in 0..512 {
            let input = (i as f32 * 0.23).cos();
            assert_eq!(
                used.process_sample(input, 1.5, 0.37, 0.4),
                fresh.process_sample(input, 1.5, 0.37, 0.4),
                "sample {i}"
            );
        }
    }

    #[test]
    fn soft_knee_normalizes_by_nonunit_threshold_after_headroom_release() {
        let mut clipper = SpectralClipper::new(64, 16);
        let expected = 0.37 * (1.5_f32 * 0.8 / 0.37).tanh();
        for i in 0..1024 {
            let output = clipper.process_sample(0.8, 1.5, 0.37, 1.0);
            if i >= 512 {
                assert!(
                    (output - expected).abs() < 1e-5,
                    "sample {i}: {output}, expected {expected}"
                );
            }
        }
    }

    fn make_sine(freq: f32, amplitude: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / SR).sin())
            .collect()
    }

    /// Helper: run a signal through the SpectralClipper and return output.
    fn run_spectral(input: &[f32], gain: f32, threshold: f32) -> Vec<f32> {
        let mut sc = SpectralClipper::new(2048, 512);
        input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, 1.0))
            .collect()
    }

    /// Helper: compute peak of a signal slice, skipping initial settling.
    fn peak_after(signal: &[f32], skip: usize) -> f32 {
        signal[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()))
    }

    // ── New gain/threshold architecture tests ─────────────────────────────

    #[test]
    fn test_gain_zero_db_passthrough() {
        // gain=1 (0dB), threshold=1 (0dB) -> output = input
        let x = 0.5_f32;
        let out = saturate_td(x, 1.0, 1.0, 1.0);
        // tanh(0.5/1.0) * 1.0 = tanh(0.5) ≈ 0.4621 (soft clip)
        // With knee=1 (full soft): hard=0.5, soft=0.4621, out = 0.5 + 1*(0.4621-0.5) = 0.4621
        // For small signals: tanh(x) ≈ x, so passthrough is approximate.
        // At knee=0 (hard clip): out = gained.clamp(-1,1) = 0.5 (exact pass).
        let out_hard = saturate_td(x, 1.0, 1.0, 0.0);
        assert!(
            (out_hard - x).abs() < 1e-6,
            "gain=1, threshold=1, knee=0 should pass through: got {out_hard}"
        );
        // With knee=1, small signals are nearly unchanged (tanh(0.5) ≈ 0.462)
        assert!(
            (out - x).abs() < 0.05,
            "gain=1, threshold=1, knee=1 should be near passthrough for 0.5: got {out}"
        );
    }

    #[test]
    fn test_gain_boosts_signal() {
        // gain=10 (+20dB), threshold=1 -> output peak ≈ 1.0 (clipped at ceiling)
        let out = saturate_td(0.5, 10.0, 1.0, 1.0);
        // gained = 5.0, tanh(5.0) ≈ 1.0, so soft ≈ 1.0, hard = 1.0
        assert!(
            out > 0.95,
            "high gain should clip near threshold=1.0: got {out}"
        );
        assert!(
            out <= 1.0 + 1e-6,
            "output should not exceed threshold: got {out}"
        );
    }

    #[test]
    fn test_threshold_clips_without_gain() {
        // gain=1, threshold=0.5 -> 0.8 input -> output ≈ 0.5
        let out = saturate_td(0.8, 1.0, 0.5, 0.0); // hard knee
        assert!(
            (out - 0.5).abs() < 1e-6,
            "hard clip at threshold=0.5 should clamp 0.8 to 0.5: got {out}"
        );
    }

    #[test]
    fn test_below_threshold_unchanged() {
        // gain=1, threshold=0.5, input=0.3 -> output ≈ 0.3 (below ceiling)
        let out = saturate_td(0.3, 1.0, 0.5, 0.0); // hard knee
        assert!(
            (out - 0.3).abs() < 1e-6,
            "below-threshold signal should pass through at knee=0: got {out}"
        );
        // With soft knee, still nearly unchanged since 0.3/0.5 = 0.6 → tanh(0.6) ≈ 0.537
        // soft = 0.5 * 0.537 ≈ 0.269, hard = 0.3, knee=1: 0.3 + 1*(0.269-0.3) = 0.269
        // Some coloring expected with soft knee, but no hard clipping.
        let out_soft = saturate_td(0.3, 1.0, 0.5, 1.0);
        assert!(
            out_soft.abs() < 0.5,
            "below-threshold should stay below threshold: got {out_soft}"
        );
    }

    #[test]
    fn test_gain_plus_threshold() {
        // gain=4, threshold=0.25 -> output peak ≈ 0.25
        let out = saturate_td(0.5, 4.0, 0.25, 0.0); // hard knee
        assert!(
            (out - 0.25).abs() < 1e-6,
            "gain=4, threshold=0.25 should clip 0.5 at 0.25: got {out}"
        );
    }

    #[test]
    fn test_negative_symmetry() {
        let pos = saturate_td(0.75, 10.0, 1.0, 1.0);
        let neg = saturate_td(-0.75, 10.0, 1.0, 1.0);
        assert!(
            (pos + neg).abs() < 1e-6,
            "saturation should be symmetric: pos={pos}, neg={neg}"
        );
    }

    #[test]
    fn test_more_gain_pushes_toward_threshold() {
        let input = 0.5_f32;
        let threshold = 1.0_f32;
        let mut prev_out = saturate_td(input, 1.0, threshold, 1.0);
        for gain_db in [6.0, 12.0, 18.0, 24.0] {
            let gain = 10.0_f32.powf(gain_db / 20.0);
            let out = saturate_td(input, gain, threshold, 1.0);
            assert!(
                out >= prev_out - 0.001,
                "more gain should push closer to threshold: {gain_db} dB gave {out}, prev was {prev_out}"
            );
            prev_out = out;
        }
        assert!(
            prev_out > 0.95,
            "at 24 dB gain, 0.5 should be near threshold 1.0, got {prev_out}"
        );
    }

    // ── Knee parameter tests ────────────────────────────────────────────

    #[test]
    fn test_hard_knee_produces_hard_clip() {
        // At knee=0, output should be hard-clipped at ±threshold.
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let input = 0.75_f32;
        let out = saturate_td(input, gain, threshold, 0.0);
        let expected = (gain * input).clamp(-threshold, threshold);
        assert!(
            (out - expected).abs() < 1e-6,
            "knee=0 should produce hard clip: expected {expected}, got {out}"
        );
    }

    #[test]
    fn test_soft_knee_matches_tanh() {
        // At knee=1, output should match the tanh formula exactly.
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let input = 0.75_f32;
        let out = saturate_td(input, gain, threshold, 1.0);
        let expected = threshold * (gain * input / threshold).tanh();
        assert!(
            (out - expected).abs() < 1e-6,
            "knee=1 should match tanh: expected {expected}, got {out}"
        );
    }

    #[test]
    fn test_knee_interpolates() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let input = 0.75_f32;
        let out_hard = saturate_td(input, gain, threshold, 0.0);
        let out_soft = saturate_td(input, gain, threshold, 1.0);
        let out_mid = saturate_td(input, gain, threshold, 0.5);

        let lo = out_soft.min(out_hard);
        let hi = out_soft.max(out_hard);
        assert!(
            out_mid >= lo - 1e-6 && out_mid <= hi + 1e-6,
            "knee=0.5 should interpolate between hard ({out_hard}) and soft ({out_soft}): got {out_mid}"
        );
    }

    // ── Basic STFT tests ──────────────────────────────────��──────────────

    #[test]
    fn test_latency_is_fft_size() {
        let sc = SpectralClipper::new(2048, 512);
        assert_eq!(sc.latency_samples(), 2048);
    }

    #[test]
    fn subthreshold_sine_follows_the_exact_delayed_waveshaper() {
        // At gain=1, threshold=1: small signal should reconstruct well.
        let input = make_sine(1000.0, 0.1, 16384);
        let output = run_spectral(&input, 1.0, 1.0);

        let latency = 2048;
        let skip = latency + 2048;
        let mut rms_error = 0.0_f64;
        let mut rms_signal = 0.0_f64;
        let compare_len = input.len() - skip - latency;
        for i in 0..compare_len {
            let inp = (input[i + skip - latency] as f64).tanh();
            let out = output[i + skip] as f64;
            rms_error += (out - inp).powi(2);
            rms_signal += inp.powi(2);
        }
        rms_error = (rms_error / compare_len as f64).sqrt();
        rms_signal = (rms_signal / compare_len as f64).sqrt();
        let snr = if rms_error > 0.0 {
            rms_signal / rms_error
        } else {
            f64::INFINITY
        };
        assert!(snr > 1e6, "SNR {snr:.1} too low for the delayed dry path");
    }

    #[test]
    fn subthreshold_dc_follows_the_exact_dry_delay() {
        let input = vec![0.05_f32; 16384];
        let output = run_spectral(&input, 1.0, 1.0);

        let skip = 4096;
        for (i, &s) in output.iter().enumerate().skip(skip) {
            assert!(
                (s - 0.05_f32.tanh()).abs() < 1e-6,
                "Delayed soft-shaped DC mismatch at {i}: got {s}",
            );
        }
    }

    #[test]
    fn test_silence_produces_silence() {
        let input = vec![0.0_f32; 16384];
        let output = run_spectral(&input, 1.0, 1.0);
        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.abs() < 1e-6,
                "silence should produce silence, got {s} at {i}"
            );
        }
    }

    #[test]
    fn test_saturation_changes_signal() {
        // High gain should modify signal
        let input = make_sine(440.0, 0.8, 16384);
        let output = run_spectral(&input, 10.0, 1.0);

        let skip = 4096;
        let mut total_diff = 0.0_f64;
        for i in skip..input.len() {
            total_diff += (output[i] as f64 - input[i] as f64).abs();
        }
        let avg_diff = total_diff / (input.len() - skip) as f64;
        assert!(
            avg_diff > 0.01,
            "heavy gain should modify signal, avg diff = {avg_diff}"
        );
    }

    // ── Spectral clipper tests ────────────────────────────────────────────

    #[test]
    fn test_spectral_produces_clipping_character() {
        let input = make_sine(440.0, 0.8, 32768);
        let mut sc = SpectralClipper::new(2048, 512);
        let mut output = Vec::new();
        for &s in &input {
            output.push(sc.process_sample(s, 10.0, 1.0, 1.0));
        }

        let skip = 8192;
        let out_peak: f32 = output[skip..].iter().map(|x| x.abs()).fold(0.0, f32::max);

        let near_peak = output[skip..]
            .iter()
            .filter(|&&s| s.abs() > out_peak * 0.95)
            .count();

        let total = output.len() - skip;
        let pct = near_peak as f32 / total as f32 * 100.0;
        assert!(
            pct > 5.0,
            "spectral should show flat-top clipping: only {pct:.1}% samples near peak {out_peak:.3}"
        );
    }

    #[test]
    fn test_spectral_output_bounded() {
        // Full-detail wet output respects the final ceiling.
        let input = make_sine(440.0, 0.8, 32768);
        let threshold = 1.0_f32;
        let mut sc = SpectralClipper::new(2048, 512);
        for &s in &input {
            let out = sc.process_sample(s, 10.0, threshold, 1.0);
            assert!(
                out.abs() <= threshold,
                "output {out} exceeds the final ceiling"
            );
        }
    }

    // ── Detail preservation: spectral vs time-domain comparison ────────

    #[test]
    fn test_spectral_preserves_more_detail_than_td() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;
        let sr = 48000.0;
        let skip = 8192;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, gain, threshold, 1.0))
            .collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let sp_output: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, 1.0))
            .collect();

        let fft_size = 2048;
        let bin_5k = (5000.0 / (sr / fft_size as f32)).round() as usize;

        fn measure_bin_energy(signal: &[f32], skip: usize, fft_size: usize, bin: usize) -> f64 {
            let mut total_mag = 0.0_f64;
            let mut count = 0;
            let mut pos = skip;
            while pos + fft_size <= signal.len() {
                let mut re_sum = 0.0_f64;
                let mut im_sum = 0.0_f64;
                for i in 0..fft_size {
                    let w = 0.5
                        * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / fft_size as f64).cos());
                    let s = signal[pos + i] as f64 * w;
                    let angle =
                        2.0 * std::f64::consts::PI * bin as f64 * i as f64 / fft_size as f64;
                    re_sum += s * angle.cos();
                    im_sum -= s * angle.sin();
                }
                let mag = (re_sum * re_sum + im_sum * im_sum).sqrt() / fft_size as f64;
                total_mag += mag;
                count += 1;
                pos += fft_size;
            }
            total_mag / count as f64
        }

        let td_5k_energy = measure_bin_energy(&td_output, skip, fft_size, bin_5k);
        let sp_5k_energy = measure_bin_energy(&sp_output, skip, fft_size, bin_5k);

        assert!(
            sp_5k_energy > td_5k_energy,
            "spectral should preserve more 5kHz detail than TD: spectral={sp_5k_energy:.6}, td={td_5k_energy:.6}"
        );
    }

    #[test]
    fn test_spectral_tonal_balance_preserved() {
        let gain = 8.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;
        let sr = 48000.0;
        let skip = 8192;
        let fft_size = 2048;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (2.0 * PI * 200.0 * t).sin()
                    + 0.1 * (2.0 * PI * 2000.0 * t).sin()
                    + 0.02 * (2.0 * PI * 8000.0 * t).sin()
            })
            .collect();

        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, gain, threshold, 1.0))
            .collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let sp_output: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, 1.0))
            .collect();

        let bin_2k = (2000.0 / (sr / fft_size as f32)).round() as usize;
        let bin_8k = (8000.0 / (sr / fft_size as f32)).round() as usize;

        fn bin_mag(signal: &[f32], skip: usize, fft_size: usize, bin: usize) -> f64 {
            let mut total = 0.0_f64;
            let mut count = 0;
            let mut pos = skip;
            while pos + fft_size <= signal.len() {
                let mut re = 0.0_f64;
                let mut im = 0.0_f64;
                for i in 0..fft_size {
                    let w = 0.5
                        * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / fft_size as f64).cos());
                    let s = signal[pos + i] as f64 * w;
                    let angle =
                        2.0 * std::f64::consts::PI * bin as f64 * i as f64 / fft_size as f64;
                    re += s * angle.cos();
                    im -= s * angle.sin();
                }
                total += (re * re + im * im).sqrt() / fft_size as f64;
                count += 1;
                pos += fft_size;
            }
            total / count as f64
        }

        let td_2k = bin_mag(&td_output, skip, fft_size, bin_2k);
        let td_8k = bin_mag(&td_output, skip, fft_size, bin_8k);
        let sp_2k = bin_mag(&sp_output, skip, fft_size, bin_2k);
        let sp_8k = bin_mag(&sp_output, skip, fft_size, bin_8k);

        let td_ratio = if td_2k > 1e-10 { td_8k / td_2k } else { 0.0 };
        let sp_ratio = if sp_2k > 1e-10 { sp_8k / sp_2k } else { 0.0 };
        let input_ratio = 0.02 / 0.1;

        let td_error = (td_ratio - input_ratio).abs();
        let sp_error = (sp_ratio - input_ratio).abs();

        assert!(
            sp_error <= td_error + 0.05,
            "spectral should preserve tonal balance better: sp_ratio={sp_ratio:.4} (err={sp_error:.4}), td_ratio={td_ratio:.4} (err={td_error:.4}), input_ratio={input_ratio:.4}"
        );
    }

    // ── Time-domain path: flat-top clipping toward threshold ─────────────

    #[test]
    fn test_td_flat_top_clipping() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let input = make_sine(440.0, 0.75, 8192);
        let output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, gain, threshold, 1.0))
            .collect();

        let out_peak = peak_after(&output, 0);

        assert!(
            out_peak > 0.9,
            "TD should push peaks toward threshold, got {out_peak}"
        );
        assert!(
            out_peak <= threshold + 1e-6,
            "TD output should not exceed threshold, got {out_peak}"
        );

        let near_peak_count = output
            .iter()
            .filter(|&&s| s.abs() > out_peak * 0.95)
            .count();
        assert!(
            near_peak_count > 500,
            "should have many flat-top samples, got {near_peak_count}"
        );
    }

    // ── Loud/quiet split algorithm tests ─────────────────────────────

    #[test]
    fn test_spectral_has_flat_top_clipping() {
        let input = make_sine(100.0, 0.8, 65536);
        let output = run_spectral(&input, 10.0, 1.0);

        let skip = 8192;
        let out_peak = peak_after(&output, skip);

        let near_peak = output[skip..]
            .iter()
            .filter(|&&s| s.abs() > out_peak * 0.95)
            .count();
        let total = output.len() - skip;
        let pct = near_peak as f64 / total as f64 * 100.0;
        assert!(
            pct > 30.0,
            "spectral should have flat-top clipping: only {pct:.1}% samples near peak {out_peak:.3} (need >30%)"
        );
    }

    fn quiet_tone(sample: usize, fft_size: usize, bin: usize) -> f32 {
        (std::f64::consts::TAU * bin as f64 * sample as f64 / fft_size as f64).sin() as f32
    }

    #[test]
    fn recent_peaks_are_held_for_a_window_then_release_smoothly() {
        for offset in 0..8 {
            let mut peak = DetailPeak::new(32, 8);
            for _ in 0..offset {
                peak.process(0.0, true);
            }
            assert_eq!(peak.process(-1.0, true), 1.0);
            for _ in 0..32 {
                assert_eq!(peak.process(0.25, true), 1.0);
            }
            let mut previous = 1.0_f32;
            for _ in 0..64 {
                let current = peak.process(0.25, true);
                assert!((0.25..=previous).contains(&current));
                assert!(current >= previous * (-4.0_f32 / 32.0).exp() - 1e-6);
                previous = current;
            }
            assert_eq!(previous, 0.25);
        }
    }

    #[test]
    fn an_expired_peak_loses_about_98_percent_over_one_window() {
        let mut peak = DetailPeak::new(32, 8);
        peak.process(1.0, true);
        let mut value = 1.0;
        for _ in 1..72 {
            value = peak.process(0.0, true);
        }
        assert!((value - 0.018_315_64).abs() < 1e-6, "{value}");
    }

    #[test]
    fn disabling_detail_forgets_the_held_peak_without_reallocating() {
        let mut peak = DetailPeak::new(32, 8);
        let storage = peak.block_peaks.as_ptr();
        for _ in 0..100 {
            peak.process(1.0, true);
        }
        assert_eq!(peak.process(2.0, false), 0.0);
        assert!(!peak.active);
        assert_eq!(peak.held_peak, 0.0);
        assert!(peak.block_peaks.iter().all(|&x| x == 0.0));
        assert_eq!(peak.process(0.0, true), 0.0);
        assert_eq!(peak.block_peaks.as_ptr(), storage);
    }

    #[test]
    fn the_peak_window_keeps_the_same_clock_across_detail_enable_changes() {
        let mut active = DetailPeak::new(32, 8);
        let mut switched = DetailPeak::new(32, 8);
        for sample in 0..1000 {
            let quiet = if sample % 101 == 0 { 1.0 } else { 0.1 };
            let expected = active.process(quiet, true);
            let enabled = sample >= 19 && !(241..319).contains(&sample);
            let actual = switched.process(quiet, enabled);
            if (100..241).contains(&sample) || sample >= 400 {
                assert_eq!(actual, expected, "sample {sample}");
            }
        }
    }

    #[test]
    fn quiet_bursts_and_level_changes_stay_bounded_without_inverting_clipped_lobes() {
        for sign in [-1.0, 1.0] {
            for (gain, threshold) in [(4.0, 1.0), (16.0, 0.063)] {
                let mut clipper = SpectralClipper::new(512, 128);
                for sample in 0..12_288 {
                    let amplitude = match sample {
                        0..2048 | 4096..6144 | 8192.. => 0.0,
                        2048..4096 => 0.02,
                        _ => 0.12,
                    };
                    let quiet = amplitude
                        * (quiet_tone(sample, 512, 47) + 0.5 * quiet_tone(sample + 13, 512, 93));
                    let pending_quiet = clipper.quiet_output_ring[clipper.read_pos] as f32;
                    let output = clipper.process_sample(0.8 * sign + quiet, gain, threshold, 0.375);
                    assert!(clipper.detail_peak.held_peak >= pending_quiet.abs());
                    assert!(output.is_finite() && output.abs() <= threshold);
                    assert!(
                        output * sign >= -1e-6,
                        "sample {sample}, sign {sign}, gain {gain}, T {threshold}: {output}"
                    );
                }
            }
        }
    }

    #[test]
    fn fft_skip_and_reenable_do_not_retain_old_headroom_on_a_constant_signal() {
        let mut clipper = SpectralClipper::new(512, 128);
        for sample in 0..4096 {
            clipper.process_sample(0.8 + 0.02 * quiet_tone(sample, 512, 47), 4.0, 1.0, 0.0);
        }
        assert!(clipper.detail_peak.held_peak > 0.01);
        for _ in 0..2048 {
            assert_eq!(clipper.process_sample_skip_fft(0.8, 4.0, 1.0, 0.0), 1.0);
            assert_eq!(clipper.detail_peak.held_peak, 0.0);
        }
        for _ in 0..2048 {
            let output = clipper.process_sample(0.8, 4.0, 1.0, 0.0);
            assert!(
                (output - 1.0).abs() < 2e-6,
                "{output}, quiet peak {}",
                clipper.detail_peak.held_peak
            );
        }
    }

    #[test]
    fn a_quiet_level_drop_releases_the_clip_line_smoothly_to_its_new_level() {
        for sign in [-1.0, 1.0] {
            let mut clipper = SpectralClipper::new(512, 128);
            let mut previous_line = 1.0;
            for sample in 0..10_240 {
                let amplitude = if sample < 4096 { 0.02 } else { 0.005 };
                let input = 0.8 * sign + amplitude * quiet_tone(sample, 512, 47);
                let quiet = clipper.quiet_output_ring[clipper.read_pos] as f32;
                let output = clipper.process_sample(input, 4.0, 1.0, 0.0);
                let line = sign * (output - 4.0 * quiet);
                if sample >= 5120 {
                    assert!(
                        line - previous_line < 0.001,
                        "sample {sample}: clip line jumped {previous_line} -> {line}"
                    );
                }
                if sample >= 8192 {
                    let expected = 0.98 * sign + 0.02 * quiet_tone(sample - 512, 512, 47);
                    assert!((output - expected).abs() < 1e-5, "{output} vs {expected}");
                }
                previous_line = line;
            }
        }
    }

    #[test]
    fn balanced_multitone_detail_keeps_a_steady_clip_line_and_equal_excursions() {
        let period: Vec<f32> = (0..512)
            .map(|sample| {
                0.02 * quiet_tone(sample, 512, 17)
                    + 0.014 * quiet_tone(sample, 512, 51)
                    + 0.009 * quiet_tone(sample, 512, 85)
            })
            .collect();
        let peak = period.iter().map(|q| q.abs()).fold(0.0_f32, f32::max);
        for sign in [-1.0, 1.0] {
            for detail in [0.25, 1.0] {
                let mut clipper = SpectralClipper::new(512, 128);
                let center = sign * (1.0 - 4.0 * detail * peak);
                let mut low = f32::INFINITY;
                let mut high = f32::NEG_INFINITY;
                for sample in 0..8192 {
                    let quiet = period[sample % period.len()];
                    let output = clipper.process_sample_with_detail(
                        0.8 * sign + quiet,
                        4.0,
                        1.0,
                        detail,
                        0.0,
                    );
                    if sample >= 6144 {
                        let expected = center + 4.0 * detail * quiet;
                        assert!(
                            (output - expected).abs() < 5e-6,
                            "sign {sign}, Detail {detail}, sample {sample}: {output} vs {expected}"
                        );
                        assert!(output.abs() <= 1.0);
                        low = low.min(output);
                        high = high.max(output);
                    }
                }
                assert!(((high - center) - (center - low)).abs() < 5e-6);
            }
        }
    }

    #[test]
    fn naturally_asymmetric_detail_keeps_its_original_waveform() {
        let period: Vec<f32> = (0..512)
            .map(|sample| {
                let phase = std::f64::consts::TAU * 17.0 * sample as f64 / 512.0;
                (0.02 * phase.sin() + 0.012 * (2.0 * phase).cos()) as f32
            })
            .collect();
        let peak = period.iter().map(|q| q.abs()).fold(0.0_f32, f32::max);
        let mut clipper = SpectralClipper::new(512, 128);
        for sample in 0..8192 {
            let quiet = period[sample % period.len()];
            let output = clipper.process_sample(0.8 + quiet, 4.0, 1.0, 0.0);
            if sample >= 6144 {
                let expected = 1.0 - 4.0 * peak + 4.0 * quiet;
                assert!((output - expected).abs() < 5e-6, "{output} vs {expected}");
            }
        }
    }

    #[test]
    fn quiet_waveform_reconstructs_a_known_sine_and_holds_its_peak() {
        let mut clipper = SpectralClipper::new(2048, 512);
        for sample in 0..12_288 {
            if sample >= 8192 {
                let angle = std::f64::consts::TAU * 215.0 * (sample - 2048) as f64 / 2048.0;
                let real = clipper.quiet_output_ring[clipper.read_pos] as f32;
                assert!((real - 0.02 * angle.sin() as f32).abs() < 2e-6);
                assert!((clipper.detail_peak.held_peak - 0.02).abs() < 2e-6);
            }
            clipper.process_sample(0.8 + 0.02 * quiet_tone(sample, 2048, 215), 1.0, 1.0, 0.0);
        }
    }

    #[test]
    fn quiet_nyquist_is_preserved_at_its_original_amplitude() {
        let mut clipper = SpectralClipper::new(64, 16);
        for sample in 0..1024 {
            let parity = if sample % 2 == 0 { 1.0 } else { -1.0 };
            if sample >= 256 {
                let real = clipper.quiet_output_ring[clipper.read_pos] as f32;
                assert!((real - 0.02 * parity).abs() < 1e-6);
            }
            clipper.process_sample(0.8 + 0.02 * parity, 4.0, 1.0, 0.0);
        }
    }

    #[test]
    fn zero_detail_skips_spectral_frames_but_keeps_the_input_ring_ready() {
        let mut clipper = SpectralClipper::new(512, 128);
        for sample in 0..2048 {
            let input = 0.8 + 0.02 * quiet_tone(sample, 512, 47);
            clipper.process_sample_with_detail(input, 4.0, 0.5, 0.0, 0.0);
        }
        assert!(clipper.mag_buf.iter().all(|&x| x == 0.0));
        assert!(clipper.quiet_output_ring.iter().all(|&x| x == 0.0));
        for sample in 2048..2176 {
            let input = 0.8 + 0.02 * quiet_tone(sample, 512, 47);
            clipper.process_sample(input, 4.0, 0.5, 0.0);
        }
        assert!(clipper.mag_buf.iter().any(|&x| x > 0.0));
    }

    #[test]
    fn partial_clipping_blends_the_headroom_curve_smoothly_for_all_knees() {
        for threshold in [0.4_f32, 0.8] {
            for sign in [-1.0, 1.0] {
                for knee in [0.0_f32, 0.375, 1.0] {
                    let mut clipper = SpectralClipper::new(512, 128);
                    let gain = 4.0;
                    let detail = 0.75;
                    let amplitude = 0.02 * threshold;
                    for sample in 0..4096 {
                        let tone = quiet_tone(sample, 512, 47);
                        let input = (sign * 1.5 * threshold + amplitude * tone) / gain;
                        let actual = clipper
                            .process_sample_with_detail(input, gain, threshold, detail, knee);
                        if sample >= 2048 {
                            let t = f64::from(threshold);
                            let gained = f64::from(input) * f64::from(gain);
                            let protected = f64::from(detail * amplitude * tone);
                            let inner = t - f64::from(detail * amplitude);
                            let shape = |x: f64, ceiling: f64| {
                                let hard = x.clamp(-ceiling, ceiling);
                                let soft = ceiling * (x / ceiling).tanh();
                                hard * (1.0 - f64::from(knee)) + soft * f64::from(knee)
                            };
                            let ordinary = shape(gained, t);
                            let candidate = shape(gained - protected, inner) + protected;
                            let over = (gained.abs() - t) / t;
                            let blend = 3.0 * over.powi(2) - 2.0 * over.powi(3);
                            let expected = ordinary * (1.0 - blend) + candidate * blend;
                            assert!(
                                (f64::from(actual) - expected).abs() < 1e-6,
                                "T {threshold}, sign {sign}, Knee {knee}: {actual} vs {expected}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn restored_tone_tracks_the_internal_clip_line_and_reaches_the_final_ceiling() {
        for sign in [-1.0, 1.0] {
            for detail in [0.0, 0.25, 0.5, 1.0] {
                for knee in [0.0, 0.375, 1.0] {
                    let mut clipper = SpectralClipper::new(512, 128);
                    let amplitude = 10.0 * 0.02 * detail;
                    let mut peak = 0.0_f32;
                    for sample in 0..4096 {
                        let tone = quiet_tone(sample, 512, 47);
                        let output = clipper.process_sample_with_detail(
                            0.8 * sign + 0.02 * tone,
                            10.0,
                            1.0,
                            detail,
                            knee,
                        );
                        if sample >= 2048 {
                            let expected = sign * (1.0 - amplitude) + amplitude * tone;
                            assert!(
                                (output - expected).abs() < 3e-6,
                                "sign {sign}, Detail {detail}, Knee {knee}: {output} vs {expected}"
                            );
                            assert!(output.abs() <= 1.0);
                            peak = peak.max(output.abs());
                        }
                    }
                    assert!((peak - 1.0).abs() < 3e-6);
                }
            }
        }
    }

    #[test]
    fn oscillating_carrier_preserves_the_quiet_waveform_without_flat_ceiling_sections() {
        let mut clipper = SpectralClipper::new(512, 128);
        let mut compared = 0;
        let mut near_ceiling = 0;
        for sample in 0..16_384 {
            let loud = 0.8 * quiet_tone(sample, 512, 8);
            let quiet = 0.02 * quiet_tone(sample, 512, 47);
            let output = clipper.process_sample(loud + quiet, 8.0, 1.0, 0.0);
            if sample >= 8192 && (8.0 * loud).abs() > 3.0 {
                let expected = loud.signum() * 0.84 + 8.0 * quiet;
                assert!((output - expected).abs() < 1e-5, "{output} vs {expected}");
                compared += 1;
                if output.abs() > 1.0 - 1e-5 {
                    near_ceiling += 1;
                }
            }
        }
        assert!(compared > 1000);
        assert!(
            near_ceiling < compared / 20,
            "{near_ceiling}/{compared} at ceiling"
        );
    }

    #[test]
    fn subthreshold_detail_and_fft_reenable_do_not_colour_the_delayed_signal() {
        for knee in [0.0, 0.375, 1.0] {
            let mut active = SpectralClipper::new(512, 128);
            let mut switched = SpectralClipper::new(512, 128);
            let mut ordinary = SpectralClipper::new(512, 128);
            for sample in 0..8192 {
                let input = 0.8 * quiet_tone(sample, 512, 8) + 0.02 * quiet_tone(sample, 512, 47);
                let expected = ordinary.process_sample_skip_fft(input, 1.0, 1.0, knee);
                assert_eq!(active.process_sample(input, 1.0, 1.0, knee), expected);
                let detail = if (2000..5000).contains(&sample) {
                    0.0
                } else {
                    1.0
                };
                assert_eq!(
                    switched.process_sample_with_detail(input, 1.0, 1.0, detail, knee),
                    expected
                );
            }
        }
    }

    #[test]
    fn live_headroom_parameters_remain_finite_bounded_and_match_the_fast_api() {
        let mut reference = SpectralClipper::new(512, 128);
        let mut fast = SpectralClipper::new(512, 128);
        let settings = [
            (1.0, 1.0, 0.0, 0.0),
            (4.0, 0.5, 0.25, 0.375),
            (16.0, 0.063, 1.0, 1.0),
            (2.0, 0.1, 0.75, 0.0),
            (1.0, 1.0, 0.0, 1.0),
        ];
        for sample in 0..12_288 {
            let (gain, threshold, detail, knee) = settings[(sample / 701) % settings.len()];
            let input = 0.8 * quiet_tone(sample, 512, 3)
                + 0.15 * quiet_tone(sample, 512, 47)
                + 0.02 * quiet_tone(sample, 512, 93);
            let expected =
                reference.process_sample_with_detail(input, gain, threshold, detail, knee);
            let actual = fast.process_sample_with_detail_fast(
                input,
                gain,
                threshold,
                1.0 / threshold,
                detail,
                knee,
            );
            assert_eq!(actual, expected);
            assert!(actual.is_finite() && actual.abs() <= threshold);
        }
    }

    #[test]
    fn reset_reuses_storage_and_clears_the_quiet_signal_and_headroom_history() {
        let mut clipper = SpectralClipper::new(512, 128);
        let pointers = (
            clipper.quiet_output_ring.as_ptr(),
            clipper.detail_peak.block_peaks.as_ptr(),
            clipper.quiet_buf.as_ptr(),
        );
        for sample in 0..2048 {
            clipper.process_sample(0.8 + 0.02 * quiet_tone(sample, 512, 47), 4.0, 0.5, 0.375);
        }
        assert!(clipper.detail_peak.held_peak > 0.001);
        clipper.reset();
        assert_eq!(
            pointers,
            (
                clipper.quiet_output_ring.as_ptr(),
                clipper.detail_peak.block_peaks.as_ptr(),
                clipper.quiet_buf.as_ptr()
            )
        );
        assert!(clipper.quiet_output_ring.iter().all(|&x| x == 0.0));
        assert_eq!(clipper.detail_peak.held_peak, 0.0);
        assert!(clipper.detail_peak.block_peaks.iter().all(|&x| x == 0.0));
        for _ in 0..2048 {
            assert_eq!(clipper.process_sample(0.0, 4.0, 0.5, 0.375), 0.0);
        }
    }
}
