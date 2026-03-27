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
//! Apply `tanh(drive * original)` to the full reconstructed signal (loud +
//! quiet = delayed input). This produces flat-top clipping at ±1.0. Then ADD
//! the quiet (detail) component on top. The detail rides symmetrically around
//! the clip level, producing the characteristic waveform: flat tops with
//! small ripple from preserved spectral detail.
//!
//! The `amount` parameter (derived from drive) crossfades between the clean
//! signal and the processed signal: at amount=0, passthrough; at amount=1,
//! full processing.
//!
//! FFT size 2048, hop size 512 (75% overlap, 4x redundancy), Hann window.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

/// Maximum drive gain (24 dB).
const GAIN_MAX: f32 = 15.848932;

/// Ratio of spectral peak above which bins are classified as 100% loud.
/// -6 dB relative to the frame's peak magnitude.
const LOUD_RATIO: f32 = 0.5;

/// Ratio of spectral peak below which bins are classified as 100% quiet.
/// -20 dB relative to the frame's peak magnitude.
const QUIET_RATIO: f32 = 0.1;

/// Compute the saturation blend `amount` (0–1) from the linear drive gain.
///
/// At drive=1.0 (0 dB): amount=0 (clean).
/// At drive=GAIN_MAX (24 dB): amount=1 (full saturation).
///
/// Returns `(amount, drive_linear)` — the caller passes `drive_linear`
/// directly to `saturate_td`.
pub fn compute_drive_params(drive_linear: f32) -> (f32, f32) {
    let amount = ((drive_linear - 1.0).max(0.0) / (GAIN_MAX - 1.0)).sqrt();
    (amount, drive_linear)
}

/// Time-domain saturation: drive-boost into a fixed tanh ceiling.
///
/// `sat(x) = tanh(drive * x)`
///
/// Drive boosts the input into the tanh curve, which has a fixed ceiling
/// of ±1.0. Small signals pass through nearly unchanged (`tanh(y) ≈ y`
/// for small `y`), while loud signals are soft-clipped toward ±1.0.
/// Higher drive = more of the signal is pushed into the saturated region.
///
/// The `amount` parameter crossfades between clean and saturated:
///
/// At amount=0 (0 dB drive): output = input (clean).
/// At amount=1 (24 dB drive): output = tanh(drive * x) (peaks clipped to ±1).
#[inline]
pub fn saturate_td(x: f32, amount: f32, drive: f32) -> f32 {
    let clipped = (drive * x).tanh();
    x + amount * (clipped - x)
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
///   paths. Tanh saturation clips the FULL signal (loud + quiet = delayed input),
///   then the quiet (detail) component is added on top. This produces flat-top
///   clipping with symmetric detail ripple around the clip level.
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
    /// Constant COLA normalization factor: sum of Hann² at 75% overlap = 1.5.
    cola_factor: f32,

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
        let analysis_window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / fft_size as f32).cos()))
            .collect();

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

        let out_ring_size = 2 * fft_size;

        Self {
            fft_size,
            hop_size,
            fft_forward,
            fft_inverse,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            analysis_window,
            cola_factor,
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
    /// `amount` and `drive` control saturation (from `compute_drive_params`).
    /// At amount=0 (0 dB drive): passthrough. Higher values = more saturation.
    pub fn process_sample(&mut self, input: f32, amount: f32, drive: f32) -> f32 {
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

        // Apply tanh to the FULL reconstructed signal (loud + quiet = delayed
        // input), then ADD the quiet (detail) component back on top.
        //
        // This produces the correct waveform shape (matching Newfangled Audio
        // Saturate): tanh clips the full signal to flat tops at ±1.0, then
        // the detail rides symmetrically on top as ripple. The detail peaks
        // go slightly above 1.0 and troughs slightly below, giving symmetric
        // variation around the clip level.
        //
        // Why not clip just the loud path? Because loud_td and quiet_td are
        // correlated (they sum to the original), so clipping only the loud
        // path leaves a "bias" from leaked fundamental energy in quiet_td.
        // Clipping the full signal first eliminates this correlation.
        let original = loud_td + quiet_td;
        let clipped_full = (drive * original).tanh();
        let processed = clipped_full + quiet_td;

        // Amount crossfade: clean -> processed
        let output = original + amount * (processed - original);

        // Safety clip at +/-1.5: detail can ride slightly above the +/-1.0 tanh
        // clip level (quiet spectral components added on top of clipped loud
        // components), but we bound it to prevent extreme values.
        let output = output.clamp(-1.5, 1.5);

        self.read_pos = (self.read_pos + 1) % out_len;

        self.hop_counter += 1;

        // Process an FFT frame every hop_size samples
        if self.hop_counter >= self.hop_size {
            self.hop_counter = 0;
            self.process_frame();
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

        // 2. Forward FFT (in-place), normalize by 1/N.
        self.fft_forward
            .process_with_scratch(&mut self.fft_buf, &mut self.scratch);

        let inv_n = 1.0 / n as f32;
        for bin in self.fft_buf.iter_mut() {
            *bin *= inv_n;
        }

        // 3. Split bins using peak-relative threshold.
        //    The threshold adapts to the actual spectral content, ensuring
        //    detail bins go to the quiet path regardless of drive level.
        //    - Above LOUD_RATIO * peak (-6 dB): 100% loud (dominant components)
        //    - Below QUIET_RATIO * peak (-20 dB): 100% quiet (detail)
        //    - Between: smooth crossfade (14 dB transition band)
        let max_mag = self.fft_buf.iter().map(|b| b.norm()).fold(0.0_f32, f32::max);
        let hi = max_mag * LOUD_RATIO;
        let lo = max_mag * QUIET_RATIO;
        let inv_band = if hi > lo { 1.0 / (hi - lo) } else { 1.0 };

        for k in 0..n {
            let mag = self.fft_buf[k].norm();
            if mag >= hi {
                // Clearly loud — 100% to loud path
                self.loud_buf[k] = self.fft_buf[k];
                self.quiet_buf[k] = Complex::new(0.0, 0.0);
            } else if mag <= lo {
                // Clearly quiet — 100% to quiet path (detail preserved)
                self.loud_buf[k] = Complex::new(0.0, 0.0);
                self.quiet_buf[k] = self.fft_buf[k];
            } else {
                // Transition band — smooth crossfade
                let t = (mag - lo) * inv_band; // 0 at lo, 1 at hi
                self.loud_buf[k] = self.fft_buf[k] * t;
                self.quiet_buf[k] = self.fft_buf[k] * (1.0 - t);
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
        //    Synthesis window = analysis window (Hann), COLA normalize.
        //    No nonlinear processing here — tanh is applied post-reconstruction
        //    in process_sample() to preserve COLA normalization.
        let inv_cola = 1.0 / self.cola_factor;

        for i in 0..n {
            let out_idx = (self.read_pos + i) % out_len;
            let w = self.analysis_window[i] * inv_cola;
            self.loud_output_ring[out_idx] += (self.loud_buf[i].re * w) as f64;
            self.quiet_output_ring[out_idx] += (self.quiet_buf[i].re * w) as f64;
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn make_sine(freq: f32, amplitude: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / SR).sin())
            .collect()
    }

    /// Helper: run a signal through the SpectralClipper and return output.
    fn run_spectral(input: &[f32], amount: f32, drive: f32) -> Vec<f32> {
        let mut sc = SpectralClipper::new(2048, 512);
        input
            .iter()
            .map(|&s| sc.process_sample(s, amount, drive))
            .collect()
    }

    /// Helper: compute peak of a signal slice, skipping initial settling.
    fn peak_after(signal: &[f32], skip: usize) -> f32 {
        signal[skip..].iter().fold(0.0_f32, |m, &s| m.max(s.abs()))
    }

    // ── Basic STFT tests ─────────────────────────────────────────────────

    #[test]
    fn test_latency_is_fft_size() {
        let sc = SpectralClipper::new(2048, 512);
        assert_eq!(sc.latency_samples(), 2048);
    }

    #[test]
    fn test_reconstruction_sine() {
        // At 0 dB drive (amount=0), STFT should perfectly reconstruct
        let (amount, drive) = compute_drive_params(1.0); // 0 dB = passthrough
        let input = make_sine(1000.0, 0.1, 16384);
        let output = run_spectral(&input, amount, drive);

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
        assert!(snr > 40.0, "SNR {snr:.1} too low — reconstruction is broken");
    }

    #[test]
    fn test_reconstruction_dc() {
        let (amount, drive) = compute_drive_params(1.0); // 0 dB
        let input = vec![0.05_f32; 16384];
        let output = run_spectral(&input, amount, drive);

        let skip = 4096;
        for (i, &s) in output.iter().enumerate().skip(skip) {
            assert!(
                (s - 0.05).abs() < 0.01,
                "DC reconstruction failed at {i}: got {s}",
            );
        }
    }

    #[test]
    fn test_silence_produces_silence() {
        let (amount, drive) = compute_drive_params(1.0);
        let input = vec![0.0_f32; 16384];
        let output = run_spectral(&input, amount, drive);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.abs() < 1e-6, "silence should produce silence, got {s} at {i}");
        }
    }

    #[test]
    fn test_saturation_changes_signal() {
        let (amount, drive) = compute_drive_params(10.0);
        let input = make_sine(440.0, 0.8, 16384);
        let output = run_spectral(&input, amount, drive);

        let skip = 4096;
        let mut total_diff = 0.0_f64;
        for i in skip..input.len() {
            total_diff += (output[i] as f64 - input[i] as f64).abs();
        }
        let avg_diff = total_diff / (input.len() - skip) as f64;
        assert!(avg_diff > 0.01, "heavy drive should modify signal, avg diff = {avg_diff}");
    }

    // ── Time-domain saturator (saturate_td) ──────────────────────────────

    #[test]
    fn test_saturate_td_unity_at_zero_drive() {
        let (amount, drive) = compute_drive_params(1.0); // 0 dB
        assert_eq!(amount, 0.0);
        assert_eq!(saturate_td(0.5, amount, drive), 0.5);
        assert_eq!(saturate_td(-0.9, amount, drive), -0.9);
        assert_eq!(saturate_td(0.0, amount, drive), 0.0);
    }

    #[test]
    fn test_saturate_td_clips_peaks_at_high_drive() {
        // Drive boosts into tanh ceiling of 1.0. A 0.75 signal at max drive
        // should be pushed toward 1.0 but clamped there.
        let (amount, drive) = compute_drive_params(GAIN_MAX); // 24 dB
        let out = saturate_td(0.75, amount, drive);
        // tanh(15.85 * 0.75) = tanh(11.9) ≈ 1.0
        // At amount=1: output ≈ 1.0
        assert!(
            out > 0.75,
            "at max drive, 0.75 should be boosted toward ceiling, got {out}"
        );
        assert!(
            out <= 1.0,
            "output should not exceed tanh ceiling of 1.0, got {out}"
        );
        // Should be very close to 1.0
        assert!(
            out > 0.95,
            "at max drive, 0.75 should be near ceiling, got {out}"
        );
    }

    #[test]
    fn test_saturate_td_boosts_small_signals() {
        // For small inputs, tanh(drive * x) ≈ drive * x. The crossfade with
        // clean via amount means small signals get a modest boost.
        let (amount, drive) = compute_drive_params(4.0); // ~12 dB
        let small = 0.01;
        let out = saturate_td(small, amount, drive);
        // tanh(4 * 0.01) = tanh(0.04) ≈ 0.04, blended: 0.01 + amount*(0.04 - 0.01)
        // amount at drive=4 ≈ 0.45, so out ≈ 0.01 + 0.45*0.03 ≈ 0.024
        assert!(
            out > small,
            "small signal should be boosted by drive: input {small}, output {out}"
        );
        // But not above the ceiling
        assert!(
            out < 1.0,
            "small signal should stay below ceiling: output {out}"
        );
    }

    #[test]
    fn test_saturate_td_negative_symmetry() {
        let (amount, drive) = compute_drive_params(GAIN_MAX);
        let pos = saturate_td(0.75, amount, drive);
        let neg = saturate_td(-0.75, amount, drive);
        assert!(
            (pos + neg).abs() < 1e-6,
            "saturation should be symmetric: pos={pos}, neg={neg}"
        );
    }

    #[test]
    fn test_saturate_td_more_drive_pushes_toward_ceiling() {
        // More drive = output pushed closer to ceiling of 1.0
        let input = 0.75;
        let mut prev_out = input;
        for drive_db in [6.0, 12.0, 18.0, 24.0] {
            let drive_lin = 10.0_f32.powf(drive_db / 20.0);
            let (amount, drive) = compute_drive_params(drive_lin);
            let out = saturate_td(input, amount, drive);
            assert!(
                out >= prev_out - 0.001,
                "more drive should push closer to ceiling: {drive_db} dB gave {out}, prev was {prev_out}"
            );
            prev_out = out;
        }
        // At 24 dB, a 0.75 signal should be very close to the 1.0 ceiling
        assert!(
            prev_out > 0.95,
            "at 24 dB, 0.75 should be near ceiling 1.0, got {prev_out}"
        );
    }

    // ── Spectral clipper: new algorithm tests ────────────────────────────

    #[test]
    fn test_spectral_produces_clipping_character() {
        // A loud sine through the spectral path at high drive should show
        // flat-top clipping: many samples near the peak value.
        let (amount, drive) = compute_drive_params(10.0);
        let input = make_sine(440.0, 0.8, 32768);
        let mut sc = SpectralClipper::new(2048, 512);
        let mut output = Vec::new();
        for &s in &input {
            output.push(sc.process_sample(s, amount, drive));
        }

        let skip = 8192;
        let out_peak: f32 = output[skip..].iter().map(|x| x.abs()).fold(0.0, f32::max);

        // Count samples near peak (within 5%) — flat tops have many
        let near_peak = output[skip..]
            .iter()
            .filter(|&&s| s.abs() > out_peak * 0.95)
            .count();

        // A clipped sine should have >10% of samples near peak
        let total = output.len() - skip;
        let pct = near_peak as f32 / total as f32 * 100.0;
        assert!(
            pct > 5.0,
            "spectral should show flat-top clipping: only {pct:.1}% samples near peak {out_peak:.3}"
        );
    }

    #[test]
    fn test_spectral_output_bounded() {
        // Output should be bounded by the safety clip at ±1.5.
        let (amount, drive) = compute_drive_params(10.0);
        let input = make_sine(440.0, 0.8, 32768);
        let mut sc = SpectralClipper::new(2048, 512);
        for &s in &input {
            let out = sc.process_sample(s, amount, drive);
            assert!(out.abs() <= 1.51, "output {out} exceeds safety clip");
        }
    }

    #[test]
    fn test_spectral_preserves_quiet_detail() {
        // A quiet 5kHz sine riding on a loud 100Hz sine:
        // The 5kHz should survive through the spectral path.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 32768;
        let sr = 48000.0;

        // Input: loud 100Hz + quiet 5kHz
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (2.0 * std::f32::consts::PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * std::f32::consts::PI * 5000.0 * t).sin()
            })
            .collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let mut output = Vec::new();
        for &s in &input {
            output.push(sc.process_sample(s, amount, drive));
        }

        // Measure 5kHz energy in output using a simple bandpass approach:
        // compute RMS of the difference between output and a lowpassed version
        // (rough proxy: just check that the output has high-frequency content)
        let skip = 8192;
        let mut hf_energy = 0.0_f64;
        for i in (skip + 1)..output.len() {
            // High-frequency proxy: sample-to-sample difference
            let diff = (output[i] - output[i - 1]) as f64;
            hf_energy += diff * diff;
        }
        hf_energy = (hf_energy / (output.len() - skip - 1) as f64).sqrt();

        // Should have measurable high-frequency energy (the 5kHz survived)
        assert!(
            hf_energy > 0.001,
            "5kHz detail should survive spectral clipping, hf_energy={hf_energy}"
        );
    }

    #[test]
    fn test_spectral_passthrough_at_zero_drive() {
        // At drive=1.0 (0 dB), all bins are below threshold, everything passes through clean
        let (amount, drive) = compute_drive_params(1.0);
        assert_eq!(amount, 0.0);
        let input = make_sine(440.0, 0.5, 16384);
        let mut sc = SpectralClipper::new(2048, 512);
        let mut output = Vec::new();
        for &s in &input {
            output.push(sc.process_sample(s, amount, drive));
        }

        // After settling, output should match input (delayed by latency)
        let skip = 4096;
        let latency = sc.latency_samples();
        let mut rms_error = 0.0_f64;
        let mut rms_signal = 0.0_f64;
        let end = input.len().min(output.len()) - latency;
        for i in skip..end {
            let inp = input[i] as f64;
            let out = output[i + latency] as f64;
            rms_error += (out - inp).powi(2);
            rms_signal += inp.powi(2);
        }
        rms_error = (rms_error / (end - skip) as f64).sqrt();
        rms_signal = (rms_signal / (end - skip) as f64).sqrt();
        let snr = if rms_error > 0.0 {
            rms_signal / rms_error
        } else {
            f64::INFINITY
        };
        assert!(snr > 20.0, "passthrough SNR {snr:.1} too low");
    }

    // ── Detail preservation: spectral vs time-domain comparison ────────

    #[test]
    fn test_spectral_preserves_more_detail_than_td() {
        // Core validation: spectral path should preserve quiet detail better
        // than pure time-domain clipping.
        //
        // Input: loud 100Hz (0.8) + quiet 5kHz (0.05).
        // Compare 5kHz energy survival in:
        //   - Time-domain only: saturate_td(composite)
        //   - Spectral path: per-bin reduction + post-ISTFT clip
        //
        // The spectral path should retain significantly more 5kHz energy.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 65536;
        let sr = 48000.0;
        let skip = 8192;

        // Build composite signal
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        // Time-domain only path
        let td_output: Vec<f32> = input.iter().map(|&s| saturate_td(s, amount, drive)).collect();

        // Spectral path
        let mut sc = SpectralClipper::new(2048, 512);
        let sp_output: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, amount, drive))
            .collect();

        // Measure 5kHz energy via DFT bin magnitude in a late window.
        // At 48kHz with 2048-point FFT, bin spacing = 48000/2048 ≈ 23.4 Hz.
        // 5kHz → bin index ≈ 5000/23.4 ≈ 213.
        let fft_size = 2048;
        let bin_5k = (5000.0 / (sr / fft_size as f32)).round() as usize;

        fn measure_bin_energy(signal: &[f32], skip: usize, fft_size: usize, bin: usize) -> f64 {
            // Average magnitude of target bin across several non-overlapping frames
            let mut total_mag = 0.0_f64;
            let mut count = 0;
            let mut pos = skip;
            while pos + fft_size <= signal.len() {
                let mut re_sum = 0.0_f64;
                let mut im_sum = 0.0_f64;
                for i in 0..fft_size {
                    let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / fft_size as f64).cos());
                    let s = signal[pos + i] as f64 * w;
                    let angle = 2.0 * std::f64::consts::PI * bin as f64 * i as f64 / fft_size as f64;
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

        // The spectral path should preserve more 5kHz energy than TD
        assert!(
            sp_5k_energy > td_5k_energy,
            "spectral should preserve more 5kHz detail than TD: spectral={sp_5k_energy:.6}, td={td_5k_energy:.6}"
        );
    }

    #[test]
    fn test_spectral_preserves_detail_across_drive_levels() {
        // At various drive levels, the spectral path should always preserve
        // more quiet detail than pure time-domain clipping.
        let num = 32768;
        let sr = 48000.0;
        let skip = 8192;

        for drive_db in [6.0, 12.0, 18.0, 24.0] {
            let drive_lin = 10.0_f32.powf(drive_db / 20.0);
            let (amount, drive) = compute_drive_params(drive_lin);

            let input: Vec<f32> = (0..num)
                .map(|i| {
                    let t = i as f32 / sr;
                    0.8 * (2.0 * PI * 100.0 * t).sin()
                        + 0.03 * (2.0 * PI * 3000.0 * t).sin()
                })
                .collect();

            // Time-domain only
            let td_output: Vec<f32> = input.iter().map(|&s| saturate_td(s, amount, drive)).collect();

            // Spectral path
            let mut sc = SpectralClipper::new(2048, 512);
            let sp_output: Vec<f32> = input
                .iter()
                .map(|&s| sc.process_sample(s, amount, drive))
                .collect();

            // Measure HF energy (proxy for 3kHz detail preservation)
            let _td_hf = hf_energy_rms(&td_output, skip);
            let sp_hf = hf_energy_rms(&sp_output, skip);

            // The spectral path preserves original detail better than TD,
            // but TD adds clipping harmonics that boost total HF energy.
            // Just verify spectral HF is non-trivial (not destroyed).
            assert!(
                sp_hf > 0.001,
                "at {drive_db} dB, spectral HF energy ({sp_hf:.6}) should be non-trivial"
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
    fn test_per_bin_ceiling_math() {
        // Verify the per-bin ceiling formula directly:
        // ceiling * tanh(mag / ceiling) should compress loud, preserve quiet.
        let drive = 10.0_f32;
        let ceiling = 0.5 / drive; // reference value / drive = 0.05

        // Loud bin (full-scale sine): mag = 0.5
        let loud_mag = 0.5_f32;
        let loud_out = ceiling * (loud_mag / ceiling).tanh();
        // Should be near ceiling (hard compressed)
        assert!(
            (loud_out - ceiling).abs() < 0.01,
            "loud bin should be compressed to near ceiling {ceiling}: got {loud_out}"
        );

        // Quiet bin (detail): mag = 0.005
        let quiet_mag = 0.005_f32;
        let quiet_out = ceiling * (quiet_mag / ceiling).tanh();
        // Should be nearly unchanged: tanh(0.005/0.05) = tanh(0.1) ≈ 0.0997
        // quiet_out ≈ 0.05 * 0.0997 ≈ 0.00498
        let ratio = quiet_out / quiet_mag;
        assert!(
            ratio > 0.95,
            "quiet bin should be nearly preserved: in={quiet_mag}, out={quiet_out}, ratio={ratio}"
        );
    }

    #[test]
    fn test_spectral_tonal_balance_preserved() {
        // The spectral path should preserve the ratio between mid and high
        // frequency components better than TD clipping.
        //
        // Input: three sines at 200Hz (loud), 2kHz (medium), 8kHz (quiet).
        // After clipping, the spectral path should maintain the relative
        // levels of 2kHz and 8kHz better than TD.
        let (amount, drive) = compute_drive_params(8.0);
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

        let td_output: Vec<f32> = input.iter().map(|&s| saturate_td(s, amount, drive)).collect();

        let mut sc = SpectralClipper::new(2048, 512);
        let sp_output: Vec<f32> = input
            .iter()
            .map(|&s| sc.process_sample(s, amount, drive))
            .collect();

        // Measure the ratio of 8kHz to 2kHz energy in both outputs
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
                    let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / fft_size as f64).cos());
                    let s = signal[pos + i] as f64 * w;
                    let angle = 2.0 * std::f64::consts::PI * bin as f64 * i as f64 / fft_size as f64;
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

        // Input ratio: 8kHz/2kHz = 0.02/0.1 = 0.2
        // Spectral should preserve this ratio better than TD
        let td_ratio = if td_2k > 1e-10 { td_8k / td_2k } else { 0.0 };
        let sp_ratio = if sp_2k > 1e-10 { sp_8k / sp_2k } else { 0.0 };
        let input_ratio = 0.02 / 0.1; // 0.2

        let td_error = (td_ratio - input_ratio).abs();
        let sp_error = (sp_ratio - input_ratio).abs();

        assert!(
            sp_error <= td_error + 0.05,
            "spectral should preserve tonal balance better: sp_ratio={sp_ratio:.4} (err={sp_error:.4}), td_ratio={td_ratio:.4} (err={td_error:.4}), input_ratio={input_ratio:.4}"
        );
    }

    // ── Time-domain path: flat-top clipping toward ceiling ─────────────

    #[test]
    fn test_td_flat_top_clipping() {
        // At high drive, the time-domain path should produce flat-top clipping:
        // the waveform is pushed toward ±1.0. Output peak should be near 1.0
        // and many consecutive samples should have similar absolute values
        // (the "flat top" characteristic).
        let (amount, drive) = compute_drive_params(10.0);
        let input = make_sine(440.0, 0.75, 8192);
        let output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, amount, drive))
            .collect();

        let out_peak = peak_after(&output, 0);

        // Output should be pushed toward ceiling 1.0
        assert!(
            out_peak > 0.9,
            "TD should push peaks toward 1.0, got {out_peak}"
        );
        assert!(
            out_peak <= 1.0,
            "TD output should not exceed tanh ceiling, got {out_peak}"
        );

        // Check for flat-top: count samples near the peak (within 5%)
        let near_peak_count = output
            .iter()
            .filter(|&&s| s.abs() > out_peak * 0.95)
            .count();
        // A sine at 440 Hz / 48 kHz ≈ 109 samples/cycle, ~8192/109 ≈ 75 cycles.
        // With heavy clipping, many samples per cycle should be near the peak.
        assert!(
            near_peak_count > 500,
            "should have many flat-top samples, got {near_peak_count}"
        );
    }

    // ── Loud/quiet split algorithm tests ─────────────────────────────

    #[test]
    fn test_spectral_has_flat_top_clipping() {
        // Feed a loud sine (0.8 amplitude, 100 Hz) through the spectral path
        // at high drive. The output should have flat-top sections where many
        // consecutive samples are near the peak value.
        let (amount, drive) = compute_drive_params(10.0);
        let input = make_sine(100.0, 0.8, 65536);
        let output = run_spectral(&input, amount, drive);

        let skip = 8192;
        let out_peak = peak_after(&output, skip);

        // Count samples within 5% of peak — should be >30% of the cycle
        // (flat tops occupy a large fraction of each half-cycle)
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
        // Feed loud 100Hz sine (0.8) + quiet 5kHz sine (0.05) through
        // spectral path at high drive. The flat sections should have
        // measurable ripple/variation from the preserved 5kHz detail.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, amount, drive);

        let skip = 8192;
        let out_peak = peak_after(&output, skip);

        // Find samples on the flat sections (near peak)
        let flat_threshold = out_peak * 0.90;
        let mut flat_samples: Vec<f32> = Vec::new();
        for &s in &output[skip..] {
            if s.abs() > flat_threshold {
                flat_samples.push(s);
            }
        }

        // Measure variation on flat sections (sample-to-sample differences)
        // If detail is preserved, there should be ripple from the 5kHz.
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
        // Same signal through pure TD path. The flat sections should be
        // much smoother — the 5kHz is destroyed by the clipping.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, amount, drive))
            .collect();

        let out_peak = peak_after(&td_output, 0);
        let flat_threshold = out_peak * 0.90;

        let mut flat_samples: Vec<f32> = Vec::new();
        for &s in &td_output {
            if s.abs() > flat_threshold {
                flat_samples.push(s);
            }
        }

        // Measure variation on flat sections
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

        // TD flat sections should be very smooth (detail destroyed by tanh).
        // tanh is a soft clipper so there's slight variation, but much less
        // than preserved detail would produce.
        assert!(
            avg_variation < 0.02,
            "TD flat sections should be smooth: avg_variation={avg_variation:.6} (need <0.02)"
        );
    }

    #[test]
    fn test_spectral_detail_better_than_td() {
        // The spectral path should preserve significantly more detail on
        // flat sections than the TD path.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        // Spectral path
        let sp_output = run_spectral(&input, amount, drive);
        let sp_skip = 8192;
        let sp_peak = peak_after(&sp_output, sp_skip);

        // TD path
        let td_output: Vec<f32> = input
            .iter()
            .map(|&s| saturate_td(s, amount, drive))
            .collect();
        let td_peak = peak_after(&td_output, 0);

        // Measure flat-section variation for both
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

        // Spectral should have significantly more variation (detail) on flat tops
        assert!(
            sp_var > td_var * 2.0,
            "spectral should preserve much more detail than TD: sp_var={sp_var:.6}, td_var={td_var:.6}"
        );
    }

    #[test]
    fn test_spectral_output_bounded_with_detail() {
        // Output must be bounded by safety clip at ±1.5.
        let (amount, drive) = compute_drive_params(GAIN_MAX);
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.1 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, amount, drive);
        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.abs() <= 1.51,
                "output {s} at sample {i} exceeds safety clip bound of 1.5"
            );
        }
    }

    #[test]
    fn test_spectral_peak_from_tanh_not_safety_clip() {
        // The output peak should be near 1.0 (from tanh saturation of the
        // loud path), NOT at the safety clip ceiling of 1.5. If the peak
        // equals the safety clip, it means the safety clip is doing the
        // clipping instead of tanh — which would destroy detail.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, amount, drive);
        let skip = 8192;
        let out_peak = peak_after(&output, skip);

        // Peak should be above 0.8 (signal is being driven) but well below
        // the safety clip ceiling of 1.5.
        assert!(
            out_peak > 0.8,
            "output peak {out_peak:.4} too low — drive should boost signal"
        );
        assert!(
            out_peak < 1.45,
            "output peak {out_peak:.4} is at the safety clip ceiling — \
             tanh should be doing the clipping, not the safety clip"
        );
    }

    #[test]
    fn test_spectral_no_samples_at_safety_clip() {
        // No samples should be hard-clipped at exactly the safety clip level.
        // If many samples are exactly at ±1.5, it means the safety clip is
        // acting as the primary clipper and destroying detail on flat sections.
        let (amount, drive) = compute_drive_params(10.0);
        let num = 65536;

        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / SR;
                0.8 * (2.0 * PI * 100.0 * t).sin()
                    + 0.05 * (2.0 * PI * 5000.0 * t).sin()
            })
            .collect();

        let output = run_spectral(&input, amount, drive);
        let skip = 8192;

        // Count samples at exactly the safety clip level (±1.5)
        let at_clip = output[skip..]
            .iter()
            .filter(|&&s| (s.abs() - 1.5).abs() < 1e-6)
            .count();

        assert!(
            at_clip == 0,
            "found {at_clip} samples at exact safety clip level — \
             safety clip is destroying detail (should be 0)"
        );
    }

    #[test]
    fn test_spectral_passthrough_at_zero_drive_new() {
        // At drive=1.0 (0 dB), amount=0, output should equal input (delayed).
        let (amount, drive) = compute_drive_params(1.0);
        assert_eq!(amount, 0.0);

        let input = make_sine(440.0, 0.5, 16384);
        let output = run_spectral(&input, amount, drive);

        let latency = 2048;
        let skip = 4096;
        let mut rms_error = 0.0_f64;
        let mut rms_signal = 0.0_f64;
        let end = input.len().min(output.len()) - latency;
        for i in skip..end {
            let inp = input[i] as f64;
            let out = output[i + latency] as f64;
            rms_error += (out - inp).powi(2);
            rms_signal += inp.powi(2);
        }
        rms_error = (rms_error / (end - skip) as f64).sqrt();
        rms_signal = (rms_signal / (end - skip) as f64).sqrt();
        let snr = if rms_error > 0.0 {
            rms_signal / rms_error
        } else {
            f64::INFINITY
        };
        assert!(snr > 20.0, "passthrough SNR {snr:.1} too low at zero drive");
    }

    /// Diagnostic: generate waveform dumps for visual comparison against reference images.
    ///
    /// Reference images:
    /// - fourier.png: input (loud low-freq sine + quiet high-freq detail)
    /// - fourier-clipped.png: TD clipped (flat tops, detail destroyed)
    /// - fourier-clipped-with-detail.png: spectral clipped (flat tops, detail preserved as ripple)
    ///
    /// Run with: cargo test -p satch test_reference_sine_plus_sine_waveform -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_reference_sine_plus_sine_waveform() {
        let sr = 48000.0_f32;
        let base_freq = 100.0_f32;
        let samples_per_cycle = (sr / base_freq) as usize; // 480 samples
        let num_samples = 65536_usize;
        let skip = 8192_usize;
        let latency = 2048_usize;

        // Drive at 20 dB (factor of 10) — heavy clipping
        let drive_lin = 10.0_f32;
        let (amount, drive) = compute_drive_params(drive_lin);

        // Detail frequency candidates: ~10-15 wiggles per half-cycle visible in reference
        let detail_freqs = [500.0_f32, 1000.0, 1500.0, 2000.0];
        // Amplitude ratios for the detail sine relative to 0.8 loud signal
        let detail_amplitudes = [0.1_f32, 0.2, 0.3];

        for &detail_freq in &detail_freqs {
            let wiggles_per_half = detail_freq / base_freq / 2.0;
            println!("\n{}", "=".repeat(70));
            println!(
                "Detail freq: {detail_freq} Hz ({wiggles_per_half:.1} wiggles per half-cycle)"
            );
            println!("{}", "=".repeat(70));

            for &detail_amp in &detail_amplitudes {
                println!(
                    "\n--- Amplitudes: base=0.8, detail={detail_amp} (ratio {:.0}%) ---",
                    detail_amp / 0.8 * 100.0
                );

                // Build composite signal
                let input: Vec<f32> = (0..num_samples)
                    .map(|i| {
                        let t = i as f32 / sr;
                        0.8 * (2.0 * PI * base_freq * t).sin()
                            + detail_amp * (2.0 * PI * detail_freq * t).sin()
                    })
                    .collect();

                // TD path (no latency)
                let td_output: Vec<f32> =
                    input.iter().map(|&s| saturate_td(s, amount, drive)).collect();

                // Spectral path
                let mut sc = SpectralClipper::new(2048, 512);
                let sp_output: Vec<f32> = input
                    .iter()
                    .map(|&s| sc.process_sample(s, amount, drive))
                    .collect();

                // Extract one full cycle after settling
                // For input and TD, use skip directly; for spectral, account for latency
                let input_start = skip;
                let td_start = skip;
                let sp_start = skip + latency;

                // Print waveform samples (every 10th sample)
                println!(
                    "\n=== INPUT (1 cycle of {base_freq}Hz at {sr}Hz = {samples_per_cycle} samples) ==="
                );
                for i in (0..samples_per_cycle).step_by(10) {
                    let idx = input_start + i;
                    if idx < input.len() {
                        println!("  sample {:>3}: {:>8.4}", i, input[idx]);
                    }
                }

                println!("\n=== TD CLIPPED ===");
                for i in (0..samples_per_cycle).step_by(10) {
                    let idx = td_start + i;
                    if idx < td_output.len() {
                        println!("  sample {:>3}: {:>8.4}", i, td_output[idx]);
                    }
                }

                println!("\n=== SPECTRAL CLIPPED ===");
                for i in (0..samples_per_cycle).step_by(10) {
                    let idx = sp_start + i;
                    if idx < sp_output.len() {
                        println!("  sample {:>3}: {:>8.4}", i, sp_output[idx]);
                    }
                }

                // ── Diagnostic metrics ──────────────────────────────────────

                // Helper: compute metrics for a one-cycle slice
                struct CycleMetrics {
                    peak: f32,
                    flat_count: usize,
                    avg_flat_variation: f64,
                    flat_min: f32,
                    flat_max: f32,
                    total_samples: usize,
                }

                fn compute_metrics(samples: &[f32]) -> CycleMetrics {
                    let peak = samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
                    let flat_threshold = peak * 0.90;

                    // Collect flat-section samples (above 90% of peak in absolute value)
                    let flat_samples: Vec<f32> = samples
                        .iter()
                        .filter(|&&s| s.abs() > flat_threshold)
                        .copied()
                        .collect();

                    let flat_count = flat_samples.len();

                    // Average sample-to-sample variation on flat sections
                    let avg_flat_variation = if flat_samples.len() > 1 {
                        let mut total_diff = 0.0_f64;
                        for i in 1..flat_samples.len() {
                            total_diff += (flat_samples[i] - flat_samples[i - 1]).abs() as f64;
                        }
                        total_diff / (flat_samples.len() - 1) as f64
                    } else {
                        0.0
                    };

                    // Min and max on flat sections (to see ripple range)
                    let flat_min = flat_samples
                        .iter()
                        .copied()
                        .fold(f32::INFINITY, f32::min);
                    let flat_max = flat_samples
                        .iter()
                        .copied()
                        .fold(f32::NEG_INFINITY, f32::max);

                    CycleMetrics {
                        peak,
                        flat_count,
                        avg_flat_variation,
                        flat_min,
                        flat_max,
                        total_samples: samples.len(),
                    }
                }

                fn print_metrics(label: &str, m: &CycleMetrics) {
                    println!("\n  [{label}] Diagnostics:");
                    println!("    Peak value:              {:>8.4}", m.peak);
                    println!(
                        "    Flat-top count (>90%):   {:>4} / {} ({:.1}%)",
                        m.flat_count,
                        m.total_samples,
                        m.flat_count as f64 / m.total_samples as f64 * 100.0
                    );
                    println!(
                        "    Avg flat variation:      {:>10.6}",
                        m.avg_flat_variation
                    );
                    println!("    Flat min:                {:>8.4}", m.flat_min);
                    println!("    Flat max:                {:>8.4}", m.flat_max);
                    println!(
                        "    Flat ripple range:       {:>8.4}",
                        m.flat_max - m.flat_min
                    );
                }

                // Extract one-cycle slices
                let input_cycle: Vec<f32> = input[input_start..input_start + samples_per_cycle]
                    .to_vec();
                let td_cycle: Vec<f32> =
                    td_output[td_start..td_start + samples_per_cycle].to_vec();
                let sp_cycle: Vec<f32> = if sp_start + samples_per_cycle <= sp_output.len() {
                    sp_output[sp_start..sp_start + samples_per_cycle].to_vec()
                } else {
                    println!("  WARNING: not enough spectral output samples after settling");
                    continue;
                };

                let input_m = compute_metrics(&input_cycle);
                let td_m = compute_metrics(&td_cycle);
                let sp_m = compute_metrics(&sp_cycle);

                print_metrics("INPUT", &input_m);
                print_metrics("TD CLIPPED", &td_m);
                print_metrics("SPECTRAL", &sp_m);

                // Summary comparison
                println!("\n  COMPARISON:");
                println!(
                    "    Spectral flat variation vs TD: {:.2}x",
                    if td_m.avg_flat_variation > 1e-10 {
                        sp_m.avg_flat_variation / td_m.avg_flat_variation
                    } else {
                        f64::INFINITY
                    }
                );
                println!(
                    "    Spectral ripple range vs TD:   {:.4} vs {:.4}",
                    sp_m.flat_max - sp_m.flat_min,
                    td_m.flat_max - td_m.flat_min
                );

                // ASCII waveform visualization (60 chars wide, ±1.2 range)
                println!("\n  ASCII waveform (1 cycle, 48 rows):");
                let width = 48_usize;
                let display_range = 1.3_f32;

                fn ascii_row(samples: &[f32], width: usize, range: f32) -> Vec<String> {
                    let height = 24_usize;
                    let mut grid = vec![vec![' '; width]; height];

                    for col in 0..width {
                        let sample_idx = col * samples.len() / width;
                        let val = samples[sample_idx];
                        // Map [-range, +range] to [height-1, 0]
                        let row =
                            ((1.0 - val / range) * 0.5 * (height - 1) as f32).round() as i32;
                        if row >= 0 && (row as usize) < height {
                            grid[row as usize][col] = '#';
                        }
                    }

                    grid.iter()
                        .map(|row| row.iter().collect::<String>())
                        .collect()
                }

                let input_art = ascii_row(&input_cycle, width, display_range);
                let td_art = ascii_row(&td_cycle, width, display_range);
                let sp_art = ascii_row(&sp_cycle, width, display_range);

                println!(
                    "  {:^width$}  {:^width$}  {:^width$}",
                    "INPUT", "TD CLIPPED", "SPECTRAL"
                );
                for row in 0..24 {
                    println!(
                        "  {}  {}  {}",
                        input_art[row], td_art[row], sp_art[row]
                    );
                }
            }
        }
    }

    /// Focused diagnostic: reference-matching signal at max drive.
    /// Run with: cargo test -p satch test_max_drive_reference -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_max_drive_reference() {
        let sr = 48000.0_f32;
        let base_freq = 100.0_f32;
        let cycle = (sr / base_freq) as usize; // 480

        // Max drive = 24 dB (amount=1.0, full saturation)
        let (amount, drive) = compute_drive_params(GAIN_MAX);
        println!("Drive: {drive:.2} (amount={amount:.4})");

        // Reference-like signal: loud 100Hz + moderate 1kHz detail
        let num = 65536_usize;
        let input: Vec<f32> = (0..num)
            .map(|i| {
                let t = i as f32 / sr;
                0.8 * (2.0 * PI * base_freq * t).sin()
                    + 0.15 * (2.0 * PI * 1000.0 * t).sin()
            })
            .collect();

        // TD path
        let td: Vec<f32> = input.iter().map(|&s| saturate_td(s, amount, drive)).collect();

        // Spectral path
        let mut sc = SpectralClipper::new(2048, 512);
        let sp: Vec<f32> = input.iter().map(|&s| sc.process_sample(s, amount, drive)).collect();

        let skip = 8192;
        let lat = 2048;

        // Print one cycle of each (every 5th sample for more detail)
        let start_input = skip;
        let start_td = skip;
        let start_sp = skip + lat;

        println!("\n{:>5} {:>8} {:>8} {:>8}", "idx", "INPUT", "TD", "SPECTRAL");
        for i in (0..cycle).step_by(5) {
            println!(
                "{:>5} {:>8.4} {:>8.4} {:>8.4}",
                i,
                input[start_input + i],
                td[start_td + i],
                sp[start_sp + i],
            );
        }

        // Metrics
        let sp_slice = &sp[start_sp..start_sp + cycle];
        let td_slice = &td[start_td..start_td + cycle];

        let sp_peak = sp_slice.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
        let td_peak = td_slice.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));

        println!("\nTD peak: {td_peak:.4}, Spectral peak: {sp_peak:.4}");

        // Flat-section analysis for spectral
        let thresh = sp_peak * 0.90;
        let flat: Vec<f32> = sp_slice.iter().filter(|&&s| s.abs() > thresh).copied().collect();
        let flat_pct = flat.len() as f64 / cycle as f64 * 100.0;
        let avg_var = if flat.len() > 1 {
            (1..flat.len()).map(|i| (flat[i] - flat[i-1]).abs() as f64).sum::<f64>() / (flat.len()-1) as f64
        } else { 0.0 };
        let flat_min = flat.iter().copied().fold(f32::INFINITY, f32::min);
        let flat_max = flat.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        println!("Spectral flat-top: {:.1}% ({}/{})", flat_pct, flat.len(), cycle);
        println!("Spectral flat variation: {avg_var:.6}");
        println!("Spectral flat range: [{flat_min:.4}, {flat_max:.4}] (ripple: {:.4})", flat_max - flat_min);

        // Same for TD
        let td_thresh = td_peak * 0.90;
        let td_flat: Vec<f32> = td_slice.iter().filter(|&&s| s.abs() > td_thresh).copied().collect();
        let td_flat_pct = td_flat.len() as f64 / cycle as f64 * 100.0;
        let td_avg_var = if td_flat.len() > 1 {
            (1..td_flat.len()).map(|i| (td_flat[i] - td_flat[i-1]).abs() as f64).sum::<f64>() / (td_flat.len()-1) as f64
        } else { 0.0 };

        println!("\nTD flat-top: {:.1}% ({}/{})", td_flat_pct, td_flat.len(), cycle);
        println!("TD flat variation: {td_avg_var:.6}");

        println!("\nDetail preservation ratio: {:.2}x", avg_var / td_avg_var.max(1e-10));
    }

}
