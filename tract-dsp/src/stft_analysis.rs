//! STFT analysis front-end: input ring, hop windowing, forward FFT, COLA window.
//!
//! [`StftAnalyzer`] owns the input-side STFT scaffolding shared by `satch`'s
//! spectral clipper and `warp-zone`'s phase vocoder: a circular input ring, the
//! periodic-Hann analysis window, the COLA-derived synthesis window, and the
//! forward FFT plan. The caller owns the synthesis half — inverse FFT, output
//! ring(s), overlap-add, `1/N` normalisation, and the per-bin transform — and
//! its own hop counter. It calls [`StftAnalyzer::write`] once per input sample
//! and [`StftAnalyzer::analyze`] once per hop.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

use crate::window::hann_periodic;

/// One analysis frame handed to the caller by [`StftAnalyzer::analyze`].
///
/// `spectrum` and `synthesis_window` are disjoint fields of the analyzer, so
/// the caller can hold this whole struct live across its entire frame block —
/// reading and transforming the spectrum, then using the synthesis window for
/// overlap-add — without a borrow conflict.
pub struct StftFrame<'a> {
    /// `fft_size` complex bins — the forward FFT of the latest windowed frame.
    /// Not normalised; the caller applies whatever `1/N` scaling it needs.
    pub spectrum: &'a mut [Complex<f32>],
    /// The COLA-normalised synthesis window (`analysis_window / cola_factor`),
    /// `fft_size` samples — multiply by this during overlap-add.
    pub synthesis_window: &'a [f32],
}

/// Per-channel STFT analysis front-end. Owns the input ring, the periodic-Hann
/// analysis window, the COLA-derived synthesis window, and the forward FFT.
///
/// The caller owns the hop counter and the synthesis half (inverse FFT, output
/// ring(s), overlap-add, normalisation, per-bin transform). It calls
/// [`write`](Self::write) each sample and [`analyze`](Self::analyze) once per
/// hop.
pub struct StftAnalyzer {
    fft_size: usize,
    fft_forward: Arc<dyn Fft<f32>>,
    /// Forward-FFT in-place scratch.
    scratch: Vec<Complex<f32>>,
    analysis_window: Vec<f32>,
    /// Pre-multiplied synthesis window: `analysis_window[i] / cola_factor`.
    synthesis_window: Vec<f32>,
    /// Circular buffer of the most recent `fft_size` input samples.
    input_ring: Vec<f32>,
    /// Write cursor into `input_ring`; also the oldest sample for the next frame.
    input_pos: usize,
    /// Pre-allocated FFT workspace; holds the spectrum returned by `analyze`.
    fft_buf: Vec<Complex<f32>>,
}

impl StftAnalyzer {
    /// Create an `fft_size`-point analyzer. `hop_size` is used only to compute
    /// the COLA synthesis window. `fft_size` must be a power of two, and
    /// `fft_size` must be an exact multiple of `hop_size`.
    pub fn new(fft_size: usize, hop_size: usize) -> Self {
        assert!(
            fft_size > 0
                && hop_size > 0
                && fft_size >= hop_size
                && fft_size.is_multiple_of(hop_size)
        );

        let mut planner = FftPlanner::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let scratch_len = fft_forward.get_inplace_scratch_len();

        let analysis_window: Vec<f32> = hann_periodic(fft_size);

        // COLA normalization for a Hann window: the sum of squared window
        // values across the `fft_size / hop_size` overlapping frames is
        // constant. Dividing the synthesis window by that constant makes
        // overlap-add reconstruct unity gain.
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

        Self {
            fft_size,
            fft_forward,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            analysis_window,
            synthesis_window,
            input_ring: vec![0.0; fft_size],
            input_pos: 0,
            fft_buf: vec![Complex::new(0.0, 0.0); fft_size],
        }
    }

    /// Write one input sample into the ring and advance. Skip this call to
    /// hold the ring frozen (e.g. `warp-zone`'s freeze).
    pub fn write(&mut self, input: f32) {
        self.input_ring[self.input_pos] = input;
        self.input_pos = (self.input_pos + 1) % self.fft_size;
    }

    /// Extract the latest `fft_size` samples (oldest-first, Hann-windowed) and
    /// forward-FFT them; return the spectrum plus the synthesis window. Call
    /// this once per hop. Skip it to suppress frame work (e.g. `satch`'s
    /// `skip_fft`).
    pub fn analyze(&mut self) -> StftFrame<'_> {
        let n = self.fft_size;
        for i in 0..n {
            let idx = (self.input_pos + i) % n;
            let windowed = self.input_ring[idx] * self.analysis_window[i];
            self.fft_buf[i] = Complex::new(windowed, 0.0);
        }
        self.fft_forward
            .process_with_scratch(&mut self.fft_buf, &mut self.scratch);
        StftFrame {
            spectrum: &mut self.fft_buf,
            synthesis_window: &self.synthesis_window,
        }
    }

    /// Zero the input ring, the position cursor, and the FFT workspace.
    pub fn reset(&mut self) {
        self.input_ring.fill(0.0);
        self.input_pos = 0;
        for bin in self.fft_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
    }

    /// Inherent latency in samples (`= fft_size`).
    pub fn latency_samples(&self) -> usize {
        self.fft_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::hann_periodic;

    #[test]
    fn latency_is_fft_size() {
        assert_eq!(StftAnalyzer::new(2048, 512).latency_samples(), 2048);
        assert_eq!(StftAnalyzer::new(4096, 1024).latency_samples(), 4096);
    }

    #[test]
    fn synthesis_window_is_analysis_over_cola() {
        // Periodic Hann at 75% overlap (hop = N/4): COLA factor = 1.5, so the
        // synthesis window is the analysis window scaled by 1/1.5.
        let mut a = StftAnalyzer::new(2048, 512);
        let analysis = hann_periodic(2048);
        let frame = a.analyze();
        for i in 0..2048 {
            let expected = analysis[i] / 1.5;
            assert!(
                (frame.synthesis_window[i] - expected).abs() < 1e-6,
                "synthesis[{i}] = {}, expected {expected}",
                frame.synthesis_window[i],
            );
        }
    }

    #[test]
    fn reset_clears_the_input_ring() {
        let mut a = StftAnalyzer::new(64, 16);
        for _ in 0..64 {
            a.write(0.9);
        }
        a.reset();
        // After reset the ring is silent: analysing it yields an all-zero spectrum.
        let frame = a.analyze();
        for bin in frame.spectrum.iter() {
            assert!(bin.norm() < 1e-6, "expected silent spectrum, got {bin}");
        }
    }

    #[test]
    fn dc_input_concentrates_energy_in_bin_zero() {
        // A windowed DC signal has its energy in the DC bin (and the two
        // adjacent bins from the Hann window's transform); every other bin,
        // and bin 1, sits strictly below the DC bin.
        let n = 2048;
        let mut a = StftAnalyzer::new(n, n / 4);
        for _ in 0..n {
            a.write(0.5);
        }
        let frame = a.analyze();
        let dc = frame.spectrum[0].norm();
        assert!(dc > 0.0, "DC bin should be non-zero for DC input");
        for k in 1..n / 2 {
            assert!(
                frame.spectrum[k].norm() < dc,
                "bin {k} ({}) should be below the DC bin ({dc})",
                frame.spectrum[k].norm(),
            );
        }
    }
}
