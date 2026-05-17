//! FFT-based spectral clipper with detail preservation.
//!
//! Architecture inspired by Newfangled Audio Saturate:
//!
//! **Step 1 — Spectral split (loud/quiet separation):**
//! STFT the input, split bins into loud (dominant, above -6 dB of peak) and
//! quiet (detail, below -20 dB of peak) paths with a smooth crossfade in the
//! transition band. ISTFT each path separately via overlap-add.
//!
//! **Step 2 — Time-domain clip + detail addition:**
//! Apply gain boost and clip at ±threshold to the full reconstructed signal
//! (loud + quiet = delayed input). This produces flat-top clipping at
//! ±threshold. Then ADD the quiet (detail) component on top. The detail rides
//! symmetrically around the clip level, producing the characteristic waveform:
//! flat tops with small ripple from preserved spectral detail.
//!
//! `gain` boosts the signal. `threshold` sets the clip ceiling.
//! Small signals pass through unchanged; loud signals clip at ±threshold.
//!
//! FFT size 2048, hop size 512 (75% overlap, 4x redundancy), Hann window.

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

/// Like `saturate_td` but also returns `tanh(gained / threshold)` for reuse
/// in the clip mask (avoids computing tanh twice per sample).
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

/// STFT-based spectral clipper with detail preservation.
///
/// Feed one sample at a time via [`process_sample`]. Internally it accumulates
/// samples, triggers FFT processing every `hop_size` samples, and reads from
/// overlap-add output buffers.
///
/// Architecture:
///
/// - `input_ring` (size `fft_size`): circular buffer of recent input samples.
/// - `loud_output_ring` / `quiet_output_ring` (size `2 * fft_size` each):
///   separate overlap-add accumulation buffers for the loud and quiet spectral
///   paths. The waveshaper clips the FULL signal (loud + quiet = delayed input)
///   at ±threshold, then the quiet (detail) component is added on top. This
///   produces flat-top clipping with symmetric detail ripple around the clip
///   level.
/// - `read_pos` advances one sample at a time; `write_pos` leads by `fft_size`.
/// - Every `hop_size` samples, an FFT frame is extracted from `input_ring`,
///   split into loud/quiet bins, and each path is ISTFT'd and overlap-added
///   into its respective output ring buffer.
pub struct SpectralClipper {
    fft_size: usize,
    hop_size: usize,

    // FFT plans
    fft_forward: Arc<dyn Fft<f32>>,
    fft_inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,

    // Windows & normalization
    analysis_window: Vec<f32>,
    /// Pre-multiplied synthesis window: analysis_window[i] / cola_factor.
    synthesis_window: Vec<f32>,
    // Pre-allocated workspace for per-bin magnitudes (avoids recomputing norm)
    mag_buf: Vec<f32>,

    // Ring buffers
    input_ring: Vec<f32>,
    /// Overlap-add accumulation buffer for loud bins (above threshold). Size = 2 x fft_size.
    loud_output_ring: Vec<f64>,
    /// Overlap-add accumulation buffer for quiet bins (below threshold). Size = 2 x fft_size.
    quiet_output_ring: Vec<f64>,
    /// Current read position in input_ring (also indexes input_ring).
    input_pos: usize,
    /// Current read position in output rings.
    read_pos: usize,
    /// Sample counter within current hop.
    hop_counter: usize,

    // Pre-allocated FFT workspace
    fft_buf: Vec<Complex<f32>>,
    /// Loud bins (above threshold) for separate ISTFT.
    loud_buf: Vec<Complex<f32>>,
    /// Quiet bins (below threshold) for separate ISTFT.
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
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);
        let scratch_len = fft_forward
            .get_inplace_scratch_len()
            .max(fft_inverse.get_inplace_scratch_len());

        // Hann window
        let analysis_window: Vec<f32> = tract_dsp::window::hann_periodic(fft_size);

        // COLA normalization for Hann window at 75% overlap (hop = N/4):
        // sum of Hann[i]² across 4 overlapping frames = 1.5 (constant).
        let num_frames = fft_size / hop_size;
        let mut cola_check = vec![0.0_f64; hop_size];
        for frame in 0..num_frames {
            let offset = frame * hop_size;
            for p in 0..hop_size {
                let w = analysis_window[p + offset] as f64;
                cola_check[p] += w * w;
            }
        }
        let cola_factor = cola_check[0] as f32;
        let inv_cola = 1.0 / cola_factor;
        let synthesis_window: Vec<f32> = analysis_window.iter().map(|&w| w * inv_cola).collect();

        let out_ring_size = 2 * fft_size;

        Self {
            fft_size,
            hop_size,
            fft_forward,
            fft_inverse,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            analysis_window,
            synthesis_window,
            mag_buf: vec![0.0; fft_size],
            input_ring: vec![0.0; fft_size],
            loud_output_ring: vec![0.0; out_ring_size],
            quiet_output_ring: vec![0.0; out_ring_size],
            input_pos: 0,
            read_pos: 0,
            hop_counter: 0,
            fft_buf: vec![Complex::new(0.0, 0.0); fft_size],
            loud_buf: vec![Complex::new(0.0, 0.0); fft_size],
            quiet_buf: vec![Complex::new(0.0, 0.0); fft_size],
        }
    }

    /// Latency in samples introduced by the STFT process.
    pub fn latency_samples(&self) -> usize {
        self.fft_size
    }

    /// Reset all internal state (ring buffers, counters) to zero.
    pub fn reset(&mut self) {
        self.input_ring.fill(0.0);
        self.loud_output_ring.fill(0.0);
        self.quiet_output_ring.fill(0.0);
        self.input_pos = 0;
        self.read_pos = 0;
        self.hop_counter = 0;
        for bin in self.fft_buf.iter_mut() {
            *bin = rustfft::num_complex::Complex::new(0.0, 0.0);
        }
        for bin in self.loud_buf.iter_mut() {
            *bin = rustfft::num_complex::Complex::new(0.0, 0.0);
        }
        for bin in self.quiet_buf.iter_mut() {
            *bin = rustfft::num_complex::Complex::new(0.0, 0.0);
        }
    }

    /// Process a single input sample and return the corresponding output sample.
    ///
    /// `gain` boosts the input. `threshold` sets the clip ceiling (linear).
    /// `knee` controls hard/soft clipping blend.
    ///
    /// When `skip_fft` is true, the FFT frame processing is skipped (the input
    /// ring and counters are still maintained). This is used when the spectral
    /// output won't be used (e.g. detail=0 or mix=0) to avoid FFT overhead.
    /// When `skip_fft` transitions back to false, there will be a brief settling
    /// period (~4 hops) as the output rings refill.
    pub fn process_sample(&mut self, input: f32, gain: f32, threshold: f32, knee: f32) -> f32 {
        let inv_threshold = 1.0 / threshold;
        self.process_sample_inner(input, gain, threshold, inv_threshold, knee, false)
    }

    /// Like `process_sample` but allows skipping FFT frame processing.
    pub fn process_sample_skip_fft(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        knee: f32,
    ) -> f32 {
        let inv_threshold = 1.0 / threshold;
        self.process_sample_inner(input, gain, threshold, inv_threshold, knee, true)
    }

    /// Fast variant of [`process_sample`] that takes a precomputed
    /// `inv_threshold = 1.0 / threshold`, avoiding a division per call.
    /// Used from the plugin `process()` loop where `inv_threshold` is
    /// computed once per sample and shared across both channels.
    #[inline]
    pub fn process_sample_fast(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        inv_threshold: f32,
        knee: f32,
    ) -> f32 {
        self.process_sample_inner(input, gain, threshold, inv_threshold, knee, false)
    }

    /// Fast variant of [`process_sample_skip_fft`] with precomputed `inv_threshold`.
    #[inline]
    pub fn process_sample_skip_fft_fast(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        inv_threshold: f32,
        knee: f32,
    ) -> f32 {
        self.process_sample_inner(input, gain, threshold, inv_threshold, knee, true)
    }

    fn process_sample_inner(
        &mut self,
        input: f32,
        gain: f32,
        threshold: f32,
        inv_threshold: f32,
        knee: f32,
        skip_fft: bool,
    ) -> f32 {
        let out_len = self.loud_output_ring.len();

        // Write input into the input ring
        self.input_ring[self.input_pos] = input;
        self.input_pos = (self.input_pos + 1) % self.fft_size;

        // Read from both output ring buffers (properly reconstructed via COLA)
        let loud_td = self.loud_output_ring[self.read_pos] as f32;
        let quiet_td = self.quiet_output_ring[self.read_pos] as f32;

        // Clear after reading
        self.loud_output_ring[self.read_pos] = 0.0;
        self.quiet_output_ring[self.read_pos] = 0.0;

        let original = loud_td + quiet_td;

        // Clip the FULL reconstructed signal at ±threshold, then ADD the
        // quiet (detail) component back on top.
        //
        // This produces the correct waveform shape: the waveshaper clips the
        // full signal to flat tops at ±threshold, then the detail rides
        // symmetrically on top as ripple.
        //
        // Why not clip just the loud path? Because loud_td and quiet_td are
        // correlated (they sum to the original), so clipping only the loud
        // path leaves a "bias" from leaked fundamental energy in quiet_td.
        // Clipping the full signal first eliminates this correlation.
        let gained_full = gain * original;
        let hard_full = gained_full.clamp(-threshold, threshold);
        let soft_full = threshold * (gained_full * inv_threshold).tanh();
        let clipped_full = hard_full + knee * (soft_full - hard_full);
        let processed = clipped_full + quiet_td;

        // Safety clip: detail can ride above the threshold level, but bound
        // it to prevent extreme values. Allow 50% overshoot above threshold.
        let safety = threshold * 1.5;
        let output = processed.clamp(-safety, safety);

        self.read_pos = (self.read_pos + 1) % out_len;

        self.hop_counter += 1;

        // Process an FFT frame every hop_size samples.
        // When skip_fft is true, reset the counter but don't run the FFT —
        // the output rings will read as zeros (from the clearing above).
        if self.hop_counter >= self.hop_size {
            self.hop_counter = 0;
            if !skip_fft {
                self.process_frame();
            }
        }

        output
    }

    /// Loud/quiet split spectral clipper with detail preservation.
    ///
    /// **Step 1:** Forward FFT of windowed input frame.
    ///
    /// **Step 2:** Split bins into loud (above threshold) and quiet (below).
    /// Threshold is peak-relative: loud bins are within 6 dB of the frame's
    /// spectral peak; quiet bins are more than 20 dB below the peak.
    ///
    /// **Step 3:** ISTFT both paths separately.
    ///
    /// **Step 4:** Overlap-add each path linearly (with synthesis window and
    /// COLA normalization) into separate output ring buffers. No nonlinear
    /// processing here — tanh is applied AFTER reconstruction in
    /// `process_sample()` to preserve COLA normalization.
    ///
    /// The split threshold is peak-relative (not drive-dependent).
    /// The actual tanh saturation happens post-reconstruction in
    /// `process_sample()`.
    fn process_frame(&mut self) {
        let n = self.fft_size;
        let out_len = self.loud_output_ring.len();

        // 1. Extract frame from input ring with analysis window.
        for i in 0..n {
            let idx = (self.input_pos + i) % n;
            let windowed = self.input_ring[idx] * self.analysis_window[i];
            self.fft_buf[i] = Complex::new(windowed, 0.0);
        }

        // 2. Forward FFT (in-place), normalize by 1/N, and cache magnitudes.
        //    Merging normalization with magnitude computation avoids a separate
        //    pass over the data. Using norm_sqr (no sqrt) for threshold comparisons
        //    eliminates ~4096 sqrt calls per frame; sqrt is only computed for the
        //    small number of bins in the transition band.
        self.fft_forward
            .process_with_scratch(&mut self.fft_buf, &mut self.scratch);

        let inv_n = 1.0 / n as f32;
        let mut max_mag_sq = 0.0_f32;
        for k in 0..n {
            self.fft_buf[k] *= inv_n;
            let mag_sq = self.fft_buf[k].norm_sqr();
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
                self.loud_buf[k] = self.fft_buf[k];
                self.quiet_buf[k] = Complex::new(0.0, 0.0);
            } else if mag_sq <= lo_sq {
                // Clearly quiet — 100% to quiet path (detail preserved)
                self.loud_buf[k] = Complex::new(0.0, 0.0);
                self.quiet_buf[k] = self.fft_buf[k];
            } else {
                // Transition band — smooth crossfade (sqrt only here).
                // quiet = fft - loud avoids a second complex multiply.
                let mag = mag_sq.sqrt();
                let t = (mag - lo) * inv_band; // 0 at lo, 1 at hi
                self.loud_buf[k] = self.fft_buf[k] * t;
                self.quiet_buf[k] = self.fft_buf[k] - self.loud_buf[k];
            }
        }

        // 4. ISTFT both paths.
        // Since we divided by N above, rustfft's unnormalized IFFT
        // produces correctly scaled time-domain signals.
        self.fft_inverse
            .process_with_scratch(&mut self.loud_buf, &mut self.scratch);
        self.fft_inverse
            .process_with_scratch(&mut self.quiet_buf, &mut self.scratch);

        // 5. Overlap-add both paths LINEARLY into their respective output rings.
        //    Synthesis window (= analysis_window / cola_factor) is pre-computed
        //    at construction to avoid a per-sample multiply.
        //    No nonlinear processing here — tanh is applied post-reconstruction
        //    in process_sample() to preserve COLA normalization.
        for i in 0..n {
            let out_idx = (self.read_pos + i) % out_len;
            let w = self.synthesis_window[i];
            self.loud_output_ring[out_idx] += (self.loud_buf[i].re * w) as f64;
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
    fn test_reconstruction_sine() {
        // At gain=1, threshold=1: small signal should reconstruct well.
        let input = make_sine(1000.0, 0.1, 16384);
        let output = run_spectral(&input, 1.0, 1.0);

        let latency = 2048;
        let skip = latency + 2048;
        let mut rms_error = 0.0_f64;
        let mut rms_signal = 0.0_f64;
        let compare_len = input.len() - skip - latency;
        for i in 0..compare_len {
            let inp = input[i + skip - latency] as f64;
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
        assert!(
            snr > 20.0,
            "SNR {snr:.1} too low — reconstruction is broken"
        );
    }

    #[test]
    fn test_reconstruction_dc() {
        let input = vec![0.05_f32; 16384];
        let output = run_spectral(&input, 1.0, 1.0);

        let skip = 4096;
        for (i, &s) in output.iter().enumerate().skip(skip) {
            assert!(
                (s - 0.05).abs() < 0.02,
                "DC reconstruction failed at {i}: got {s}",
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
        // Output should be bounded by the safety clip at ±threshold*1.5.
        let input = make_sine(440.0, 0.8, 32768);
        let threshold = 1.0_f32;
        let mut sc = SpectralClipper::new(2048, 512);
        for &s in &input {
            let out = sc.process_sample(s, 10.0, threshold, 1.0);
            assert!(
                out.abs() <= threshold * 1.5 + 0.01,
                "output {out} exceeds safety clip"
            );
        }
    }

    #[test]
    fn test_spectral_preserves_quiet_detail() {
        let num = 32768;
        let sr = 48000.0;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let mut output = Vec::new();
        for &s in &input {
            output.push(sc.process_sample(s, 10.0, 1.0, 1.0));
        }

        let skip = 8192;
        let mut hf_energy = 0.0_f64;
        for i in (skip + 1)..output.len() {
            let diff = (output[i] - output[i - 1]) as f64;
            hf_energy += diff * diff;
        }
        hf_energy = (hf_energy / (output.len() - skip - 1) as f64).sqrt();

        assert!(
            hf_energy > 0.001,
            "5kHz detail should survive spectral clipping, hf_energy={hf_energy}"
        );
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
    fn test_spectral_preserves_detail_across_gain_levels() {
        let num = 32768;
        let sr = 48000.0;
        let skip = 8192;
        let threshold = 1.0_f32;

        for gain_db in [6.0, 12.0, 18.0, 24.0] {
            let gain = 10.0_f32.powf(gain_db / 20.0);

            let input: Vec<f32> = (0..num)
                .map(|i| {
                    let t = i as f32 / sr;
                    0.8 * (2.0 * PI * 100.0 * t).sin() + 0.03 * (2.0 * PI * 3000.0 * t).sin()
                })
                .collect();

            let mut sc = SpectralClipper::new(2048, 512);
            let sp_output: Vec<f32> = input
                .iter()
                .map(|&s| sc.process_sample(s, gain, threshold, 1.0))
                .collect();

            let sp_hf = hf_energy_rms(&sp_output, skip);

            assert!(
                sp_hf > 0.001,
                "at {gain_db} dB, spectral HF energy ({sp_hf:.6}) should be non-trivial"
            );
        }
    }

    /// Compute RMS of sample-to-sample differences (high-frequency energy proxy).
    fn hf_energy_rms(signal: &[f32], skip: usize) -> f64 {
        let mut energy = 0.0_f64;
        for i in (skip + 1)..signal.len() {
            let d = (signal[i] - signal[i - 1]) as f64;
            energy += d * d;
        }
        (energy / (signal.len() - skip - 1) as f64).sqrt()
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

    #[test]
    fn test_spectral_preserves_detail_on_flat_tops() {
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, 10.0, 1.0);

        let skip = 8192;
        let out_peak = peak_after(&output, skip);

        let flat_threshold = out_peak * 0.90;
        let mut flat_samples: Vec<f32> = Vec::new();
        for &s in &output[skip..] {
            if s.abs() > flat_threshold {
                flat_samples.push(s);
            }
        }

        let mut variation = 0.0_f64;
        for i in 1..flat_samples.len() {
            let diff = (flat_samples[i] - flat_samples[i - 1]).abs() as f64;
            variation += diff;
        }
        let avg_variation = if flat_samples.len() > 1 {
            variation / (flat_samples.len() - 1) as f64
        } else {
            0.0
        };

        assert!(
            flat_samples.len() > 100,
            "should have enough flat-top samples to measure, got {}",
            flat_samples.len()
        );
        assert!(
            avg_variation > 0.001,
            "flat sections should have detail ripple: avg_variation={avg_variation:.6} (need >0.001)"
        );
    }

    #[test]
    fn test_td_destroys_detail_on_flat_tops() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, gain, threshold, 1.0))
            .collect();

        let out_peak = peak_after(&td_output, 0);
        let flat_threshold = out_peak * 0.90;

        let mut flat_samples: Vec<f32> = Vec::new();
        for &s in &td_output {
            if s.abs() > flat_threshold {
                flat_samples.push(s);
            }
        }

        let mut variation = 0.0_f64;
        for i in 1..flat_samples.len() {
            let diff = (flat_samples[i] - flat_samples[i - 1]).abs() as f64;
            variation += diff;
        }
        let avg_variation = if flat_samples.len() > 1 {
            variation / (flat_samples.len() - 1) as f64
        } else {
            0.0
        };

        assert!(
            avg_variation < 0.02,
            "TD flat sections should be smooth: avg_variation={avg_variation:.6} (need <0.02)"
        );
    }

    #[test]
    fn test_spectral_detail_better_than_td() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let sp_output = run_spectral(&input, gain, threshold);
        let sp_skip = 8192;
        let sp_peak = peak_after(&sp_output, sp_skip);

        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, gain, threshold, 1.0))
            .collect();
        let td_peak = peak_after(&td_output, 0);

        fn flat_variation(signal: &[f32], peak: f32) -> f64 {
            let threshold = peak * 0.90;
            let flat: Vec<f32> = signal
                .iter()
                .filter(|&&s| s.abs() > threshold)
                .copied()
                .collect();
            if flat.len() < 2 {
                return 0.0;
            }
            let mut var = 0.0_f64;
            for i in 1..flat.len() {
                var += (flat[i] - flat[i - 1]).abs() as f64;
            }
            var / (flat.len() - 1) as f64
        }

        let sp_var = flat_variation(&sp_output[sp_skip..], sp_peak);
        let td_var = flat_variation(&td_output, td_peak);

        assert!(
            sp_var > td_var * 2.0,
            "spectral should preserve much more detail than TD: sp_var={sp_var:.6}, td_var={td_var:.6}"
        );
    }

    #[test]
    fn test_spectral_output_bounded_with_detail() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.1 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, gain, threshold);
        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.abs() <= threshold * 1.5 + 0.01,
                "output {s} at sample {i} exceeds safety clip bound"
            );
        }
    }

    #[test]
    fn test_spectral_peak_from_tanh_not_safety_clip() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, gain, threshold);
        let skip = 8192;
        let out_peak = peak_after(&output, skip);

        assert!(
            out_peak > 0.8,
            "output peak {out_peak:.4} too low — gain should boost signal"
        );
        assert!(
            out_peak < threshold * 1.45,
            "output peak {out_peak:.4} is at the safety clip ceiling — \
             tanh should be doing the clipping, not the safety clip"
        );
    }

    #[test]
    fn test_spectral_no_samples_at_safety_clip() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, gain, threshold);
        let skip = 8192;

        let safety_clip = threshold * 1.5;
        let at_clip = output[skip..]
            .iter()
            .filter(|&&s| (s.abs() - safety_clip).abs() < 1e-6)
            .count();

        assert!(
            at_clip == 0,
            "found {at_clip} samples at exact safety clip level — \
             safety clip is destroying detail (should be 0)"
        );
    }

    // ── Clip-aware detail blend tests (mirrors lib.rs pipeline) ─────────

    /// Simulate the full plugin pipeline: TD saturation + spectral + clip-aware blend.
    fn blend_with_clip_mask(input: &[f32], gain: f32, threshold: f32, detail: f32) -> Vec<f32> {
        let mut sc = SpectralClipper::new(2048, 512);

        let delay_len = 2048;
        let mut dry_delay = vec![0.0_f32; delay_len];
        let mut delay_pos = 0;

        let mut output = Vec::with_capacity(input.len());

        for &sample in input {
            let dry = dry_delay[delay_pos];
            dry_delay[delay_pos] = sample;
            delay_pos = (delay_pos + 1) % delay_len;

            let (td, tanh_val) = saturate_td_with_tanh(dry, gain, threshold, 1.0);
            let sp = sc.process_sample(sample, gain, threshold, 1.0);

            let clip_mask = tanh_val * tanh_val;
            let lost = sp - td;
            let wet = td + detail * clip_mask * lost;

            output.push(wet);
        }

        output
    }

    #[test]
    fn test_clip_blend_unclipped_matches_td() {
        // Quiet signal at high gain: clip mask activates but (sp - td) ≈ 0
        // for linear-region signals, so blended ≈ td.
        let gain = 10.0_f32;
        let threshold = 1.0_f32;
        let detail = 1.0;

        let input = make_sine(440.0, 0.05, 32768);

        let delay_len = 2048;
        let mut dry_delay = vec![0.0_f32; delay_len];
        let mut delay_pos = 0;
        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| {
                let dry = dry_delay[delay_pos];
                dry_delay[delay_pos] = s;
                delay_pos = (delay_pos + 1) % delay_len;
                saturate_td(dry, gain, threshold, 1.0)
            })
            .collect();

        let blended = blend_with_clip_mask(&input, gain, threshold, detail);

        let skip = 8192;
        let mut rms_diff = 0.0_f64;
        let mut rms_signal = 0.0_f64;
        let count = blended.len() - skip;
        for i in skip..blended.len() {
            let diff = (blended[i] - td_output[i]).abs();
            rms_diff += (diff as f64).powi(2);
            rms_signal += (td_output[i] as f64).powi(2);
        }
        rms_diff = (rms_diff / count as f64).sqrt();
        rms_signal = (rms_signal / count as f64).sqrt();

        let relative_error = if rms_signal > 1e-10 {
            rms_diff / rms_signal
        } else {
            rms_diff
        };
        assert!(
            relative_error < 0.05,
            "unclipped material should match TD within 5%: relative_error={relative_error:.6}"
        );
    }

    #[test]
    fn test_clip_blend_adds_detail_in_clipped_regions() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;

        let num = 65536;
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.15 * (2.0 * PI * 1000.0 * t).sin()
            })
            .collect();

        let delay_len = 2048;
        let mut dry_delay = vec![0.0_f32; delay_len];
        let mut delay_pos = 0;
        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| {
                let dry = dry_delay[delay_pos];
                dry_delay[delay_pos] = s;
                delay_pos = (delay_pos + 1) % delay_len;
                saturate_td(dry, gain, threshold, 1.0)
            })
            .collect();

        let blended = blend_with_clip_mask(&input, gain, threshold, 1.0);

        let skip = 8192;

        fn flat_variation(signal: &[f32], skip: usize) -> f64 {
            let peak = signal[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
            let threshold = peak * 0.90;
            let flat: Vec<f32> = signal[skip..]
                .iter()
                .filter(|&&s| s.abs() > threshold)
                .copied()
                .collect();
            if flat.len() < 2 {
                return 0.0;
            }
            let mut var = 0.0_f64;
            for i in 1..flat.len() {
                var += (flat[i] - flat[i - 1]).abs() as f64;
            }
            var / (flat.len() - 1) as f64
        }

        let td_var = flat_variation(&td_output, skip);
        let blended_var = flat_variation(&blended, skip);

        assert!(
            blended_var > td_var * 1.5,
            "clip-aware blend should add detail in clipped regions: blended_var={blended_var:.6}, td_var={td_var:.6}, ratio={:.2}x",
            blended_var / td_var.max(1e-10)
        );
    }

    #[test]
    fn test_clip_blend_peaks_match_td() {
        let gain = 10.0_f32;
        let threshold = 1.0_f32;

        let num = 65536;
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.15 * (2.0 * PI * 1000.0 * t).sin()
            })
            .collect();

        let delay_len = 2048;
        let mut dry_delay = vec![0.0_f32; delay_len];
        let mut delay_pos = 0;
        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| {
                let dry = dry_delay[delay_pos];
                dry_delay[delay_pos] = s;
                delay_pos = (delay_pos + 1) % delay_len;
                saturate_td(dry, gain, threshold, 1.0)
            })
            .collect();

        let blended = blend_with_clip_mask(&input, gain, threshold, 1.0);

        let skip = 8192;
        let td_peak = td_output[skip..]
            .iter()
            .fold(0.0_f32, |m, &s| m.max(s.abs()));
        let blended_peak = blended[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));

        let ratio = blended_peak / td_peak;
        // In the new architecture (no amount crossfade), spectral detail rides
        // above the threshold clip level, producing up to ~20% overshoot with
        // 0.15 amplitude detail content. The safety clip bounds this at 1.5x.
        assert!(
            (0.85..=1.25).contains(&ratio),
            "blended peak should be within +25%/-15% of TD peak: td_peak={td_peak:.4}, blended_peak={blended_peak:.4}, ratio={ratio:.4}"
        );
    }

    // ── Detail at threshold level (spec test 5) ────���─────────────────────

    #[test]
    fn test_detail_at_threshold_level() {
        // gain=1, threshold=0.5, detail=100%, composite signal
        // Detail variation should be > 1.5x TD at the ±0.5 flat sections.
        let gain = 1.0_f32;
        let threshold = 0.5_f32;

        let num = 65536;
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        // TD-only path (with delay alignment)
        let delay_len = 2048;
        let mut dry_delay_td = vec![0.0_f32; delay_len];
        let mut pos_td = 0;
        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| {
                let dry = dry_delay_td[pos_td];
                dry_delay_td[pos_td] = s;
                pos_td = (pos_td + 1) % delay_len;
                saturate_td(dry, gain, threshold, 1.0)
            })
            .collect();

        // Full blend pipeline
        let blended = blend_with_clip_mask(&input, gain, threshold, 1.0);

        let skip = 8192;

        fn flat_variation(signal: &[f32], skip: usize) -> f64 {
            let peak = signal[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
            let thresh = peak * 0.90;
            let flat: Vec<f32> = signal[skip..]
                .iter()
                .filter(|&&s| s.abs() > thresh)
                .copied()
                .collect();
            if flat.len() < 2 {
                return 0.0;
            }
            let mut var = 0.0_f64;
            for i in 1..flat.len() {
                var += (flat[i] - flat[i - 1]).abs() as f64;
            }
            var / (flat.len() - 1) as f64
        }

        let td_var = flat_variation(&td_output, skip);
        let blended_var = flat_variation(&blended, skip);

        assert!(
            blended_var > td_var * 1.5,
            "detail at threshold level should preserve variation: blended_var={blended_var:.6}, td_var={td_var:.6}"
        );
    }

    // ── Detail at gain level (spec test 6) ──────────────────────────────

    #[test]
    fn test_detail_at_gain_level() {
        // gain=10, threshold=1.0, detail=100%
        // Detail variation should be > 1.5x TD at the ±1.0 flat sections.
        let gain = 10.0_f32;
        let threshold = 1.0_f32;

        let num = 65536;
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin() + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let delay_len = 2048;
        let mut dry_delay_td = vec![0.0_f32; delay_len];
        let mut pos_td = 0;
        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| {
                let dry = dry_delay_td[pos_td];
                dry_delay_td[pos_td] = s;
                pos_td = (pos_td + 1) % delay_len;
                saturate_td(dry, gain, threshold, 1.0)
            })
            .collect();

        let blended = blend_with_clip_mask(&input, gain, threshold, 1.0);

        let skip = 8192;

        fn flat_variation(signal: &[f32], skip: usize) -> f64 {
            let peak = signal[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
            let thresh = peak * 0.90;
            let flat: Vec<f32> = signal[skip..]
                .iter()
                .filter(|&&s| s.abs() > thresh)
                .copied()
                .collect();
            if flat.len() < 2 {
                return 0.0;
            }
            let mut var = 0.0_f64;
            for i in 1..flat.len() {
                var += (flat[i] - flat[i - 1]).abs() as f64;
            }
            var / (flat.len() - 1) as f64
        }

        let td_var = flat_variation(&td_output, skip);
        let blended_var = flat_variation(&blended, skip);

        assert!(
            blended_var > td_var * 1.5,
            "detail at gain level should preserve variation: blended_var={blended_var:.6}, td_var={td_var:.6}"
        );
    }

    // ── Full pipeline tests with threshold ──────────────────────────────

    #[test]
    fn test_pipeline_peak_clamped_at_threshold() {
        // gain=8 (~18dB), threshold=0.5 — output peak should be at threshold.
        let gain = 8.0_f32;
        let threshold = 0.5_f32;
        let knee = 1.0;

        let num_samples = 32768;
        let freq = 440.0_f32;
        let sr = 48000.0_f32;
        let amplitude = 0.8_f32;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let spectral_out: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, knee))
            .collect();

        let latency = 2048;
        let mut dry_delay = vec![0.0_f32; latency];
        let mut dry_pos = 0;
        let mut output = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let dry = dry_delay[dry_pos];
            dry_delay[dry_pos] = input[i];
            dry_pos = (dry_pos + 1) % latency;

            let (td, tanh_val) = saturate_td_with_tanh(dry, gain, threshold, knee);
            let sp = spectral_out[i];

            let clip_mask = tanh_val * tanh_val;
            let lost = sp - td;
            let wet = td + 1.0 * clip_mask * lost;
            output.push(wet);
        }

        let skip = latency + 4096;
        let peak = output[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()));

        // TD clips at threshold, spectral detail can ride slightly above
        assert!(
            peak < threshold * 1.5 + 0.01,
            "output peak {peak} should not far exceed threshold {threshold}"
        );
        assert!(
            peak > threshold * 0.85,
            "output peak {peak} should reach near threshold {threshold}"
        );
    }

    #[test]
    fn test_pipeline_detail_variation_with_gain() {
        // At high gain (18dB), the clip mask is near 1.0 for loud signals,
        // so the spectral detail term adds variation in clipped regions.
        let gain = 8.0_f32;
        let threshold = 1.0_f32;
        let knee = 1.0;

        let num_samples = 32768;
        let freq = 440.0_f32;
        let sr = 48000.0_f32;
        let amplitude = 0.8_f32;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let spectral_out: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, gain, threshold, knee))
            .collect();

        let latency = 2048;
        let mut dry_delay = vec![0.0_f32; latency];
        let mut dry_pos = 0;

        let mut output_td = Vec::with_capacity(num_samples);
        let mut output_detail = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let dry = dry_delay[dry_pos];
            dry_delay[dry_pos] = input[i];
            dry_pos = (dry_pos + 1) % latency;

            let (td, tanh_val) = saturate_td_with_tanh(dry, gain, threshold, knee);
            let sp = spectral_out[i];
            let clip_mask = tanh_val * tanh_val;

            let lost = sp - td;
            output_td.push(td);
            output_detail.push(td + 1.0 * clip_mask * lost);
        }

        let skip = latency + 4096;

        let mut td_var = 0.0_f64;
        let mut detail_var = 0.0_f64;
        let mut count = 0usize;
        for i in skip..num_samples.saturating_sub(1) {
            let orig = input[i.saturating_sub(latency)];
            if orig.abs() > 0.7 {
                let d_td = (output_td[i + 1] - output_td[i]).abs() as f64;
                let d_detail = (output_detail[i + 1] - output_detail[i]).abs() as f64;
                td_var += d_td;
                detail_var += d_detail;
                count += 1;
            }
        }
        assert!(
            count > 100,
            "should have enough clipped samples: count={count}"
        );
        assert!(
            detail_var > td_var * 1.05,
            "detail output should have more variation than TD in clipped regions: \
             detail_var={detail_var:.6}, td_var={td_var:.6}, count={count}"
        );
    }
}
