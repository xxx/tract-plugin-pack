//! Raw + Phaseless convolution engines, adapted from wavetable-filter's
//! proven DSP. A self-contained module — no plugin or GUI types.

use crate::kernel::{Kernel, MAG_BINS, MAX_KERNEL};
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use std::sync::Arc;

/// Per-channel time-domain convolution state. A thin wrapper over
/// `tract_dsp::fir::FirRing`.
///
/// The silence fast-path only re-arms on `reset()`; a host `process()` loop
/// should call `reset()` after sustained input silence.
pub struct RawChannel {
    ring: tract_dsp::fir::FirRing,
}

impl RawChannel {
    /// A channel sized for kernels up to `MAX_KERNEL` taps.
    pub fn new() -> Self {
        Self {
            ring: tract_dsp::fir::FirRing::new(MAX_KERNEL),
        }
    }

    /// Zero the history.
    pub fn reset(&mut self) {
        self.ring.reset();
    }

    /// Process one sample through `kernel`; returns the filtered output.
    /// All-zero kernel -> the input is returned unchanged (dry passthrough).
    pub fn process(&mut self, sample: f32, kernel: &Kernel) -> f32 {
        self.ring.push(sample);
        if kernel.is_zero {
            return sample; // dry passthrough — see the miff spec
        }
        if self.ring.is_silent() {
            return 0.0; // silence fast-path: history is all zero
        }
        self.ring.mac(&kernel.rev_taps[..kernel.len])
    }
}

impl Default for RawChannel {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Phaseless STFT engine ───────────────────────────────────────────────────

/// Phaseless STFT frame size (fixed — does not track kernel length).
pub const STFT_FRAME: usize = MAX_KERNEL; // 4096
/// Overlap-add hop (50% overlap) and the reported Phaseless latency.
pub const PHASELESS_HOP: usize = STFT_FRAME / 2; // 2048
/// Reported plugin latency in Phaseless mode, in samples.
pub const PHASELESS_LATENCY: u32 = PHASELESS_HOP as u32;

/// Per-channel STFT magnitude-only convolution state. Fixed `STFT_FRAME`-point
/// transform; the kernel's magnitude spectrum is applied to each frame, phase
/// preserved. Adapted from wavetable-filter's STFT path.
///
/// The forward FFT input is the latest `STFT_FRAME` input samples, windowed
/// by a Hann window. Each bin is multiplied by the kernel's magnitude (real
/// scalar — phase preserved). The result is IFFT'd and overlap-add'd with
/// scale `1/STFT_FRAME`. Output is delayed by `PHASELESS_HOP` samples.
pub struct PhaselessChannel {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// Circular input buffer: STFT_FRAME samples.
    in_buf: Vec<f32>,
    /// Position where the NEXT sample will be written (oldest is `in_pos`).
    in_pos: usize,
    /// Overlap-add output accumulator: STFT_FRAME samples.
    out_buf: Vec<f32>,
    /// Read/write position within the current hop (0..PHASELESS_HOP).
    out_pos: usize,
    scratch_time: Vec<f32>,
    scratch_freq: Vec<Complex<f32>>,
    /// Pre-allocated FFT scratch buffers. `realfft`'s short-form `process()`
    /// heap-allocates a scratch vec per call; `process_with_scratch` reuses
    /// these so the audio-thread frame path never allocates. The forward and
    /// inverse transforms can require different scratch sizes — kept separate.
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
}

impl PhaselessChannel {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(STFT_FRAME);
        let ifft = planner.plan_fft_inverse(STFT_FRAME);
        let window: Vec<f32> = tract_dsp::window::hann_periodic(STFT_FRAME);
        let scratch_fwd = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        Self {
            fft,
            ifft,
            window,
            in_buf: vec![0.0; STFT_FRAME],
            in_pos: 0,
            out_buf: vec![0.0; STFT_FRAME],
            out_pos: 0,
            scratch_time: vec![0.0; STFT_FRAME],
            scratch_freq: vec![Complex::new(0.0, 0.0); MAG_BINS],
            scratch_fwd,
            scratch_inv,
        }
    }

    /// Zero all state.
    pub fn reset(&mut self) {
        self.in_buf.iter_mut().for_each(|s| *s = 0.0);
        self.out_buf.iter_mut().for_each(|s| *s = 0.0);
        self.in_pos = 0;
        self.out_pos = 0;
    }

    /// Process one sample. Output is always delayed by `PHASELESS_HOP` samples.
    ///
    /// Both zero and non-zero kernels route through the STFT — the engine's
    /// inherent `PHASELESS_HOP` latency is uniform regardless of kernel, so it
    /// matches the fixed latency miff reports to the host in Phaseless mode.
    /// A zero kernel maps to per-bin gain 1.0 (identity) inside `process_frame`,
    /// so it becomes a *delayed* dry passthrough — never a 0-delay bypass,
    /// which would play 2048 samples early against the host's delay
    /// compensation.
    pub fn process(&mut self, sample: f32, kernel: &Kernel) -> f32 {
        // When out_pos is 0 (start of a new hop), process the next STFT frame.
        if self.out_pos == 0 {
            // Shift the second hop of out_buf into the first half, clear second.
            self.out_buf.copy_within(PHASELESS_HOP..STFT_FRAME, 0);
            self.out_buf[PHASELESS_HOP..].fill(0.0);
            Self::process_frame_static(
                &self.in_buf,
                self.in_pos,
                &mut self.out_buf,
                kernel,
                &self.window,
                self.fft.as_ref(),
                self.ifft.as_ref(),
                &mut self.scratch_time,
                &mut self.scratch_freq,
                &mut self.scratch_fwd,
                &mut self.scratch_inv,
            );
        }

        // Write the new input sample into the circular buffer.
        self.in_buf[self.in_pos] = sample;
        // Read the current (delayed) output sample.
        let out = self.out_buf[self.out_pos];

        // Advance positions.
        self.in_pos = (self.in_pos + 1) & (STFT_FRAME - 1);
        self.out_pos += 1;
        if self.out_pos >= PHASELESS_HOP {
            self.out_pos = 0;
        }

        out
    }

    /// STFT frame: window → FFT → magnitude multiply → IFFT → overlap-add.
    ///
    /// Faithfully reproduced from wavetable-filter's `process_stft_frame`.
    /// Scale factor: `1/N` where N = STFT_FRAME. With a Hann window at 50%
    /// overlap the OLA sum is `N/2` (half the window's squared-sum), and the
    /// `1/N` scale reduces this to a flat `0.5` gain — which is the correct
    /// normalization for Hann-windowed 50% OLA.
    ///
    /// Per-bin gain: `1.0` when `kernel.is_zero` (identity — a delayed dry
    /// passthrough), otherwise `kernel.mags[bin]`. A zero kernel has all-zero
    /// `mags`, so blindly multiplying would produce silence — the `is_zero`
    /// branch is what keeps the zero-kernel path a passthrough.
    #[allow(clippy::too_many_arguments)]
    fn process_frame_static(
        in_buf: &[f32],
        in_pos: usize,
        out_buf: &mut [f32],
        kernel: &Kernel,
        window: &[f32],
        fft: &dyn RealToComplex<f32>,
        ifft: &dyn ComplexToReal<f32>,
        scratch_time: &mut [f32],
        scratch_freq: &mut [Complex<f32>],
        scratch_fwd: &mut [Complex<f32>],
        scratch_inv: &mut [Complex<f32>],
    ) {
        let n = STFT_FRAME;
        let mask = n - 1;
        // Copy the circular buffer into scratch, oldest-first, and apply window.
        for i in 0..n {
            scratch_time[i] = in_buf[(in_pos + i) & mask] * window[i];
        }
        if fft
            .process_with_scratch(scratch_time, scratch_freq, scratch_fwd)
            .is_err()
        {
            return;
        }
        // Multiply each bin by the kernel magnitude (real scalar, preserves
        // phase). A zero kernel is unity gain everywhere — the identity — so
        // skip the multiply entirely and leave the bins unchanged.
        if !kernel.is_zero {
            for (bin, &mag) in scratch_freq.iter_mut().zip(kernel.mags.iter()) {
                *bin *= mag;
            }
        }
        if ifft
            .process_with_scratch(scratch_freq, scratch_time, scratch_inv)
            .is_err()
        {
            return;
        }
        // Overlap-add with 1/N normalization (matches wavetable-filter exactly).
        let scale = 1.0 / n as f32;
        for i in 0..n {
            out_buf[i] += scratch_time[i] * scale;
        }
    }
}

impl Default for PhaselessChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{bake, MAG_BINS};
    use tiny_skia_widgets::mseg::MsegData;

    /// Build a `Kernel` directly from explicit taps, bypassing the curve bake.
    fn kernel_from_taps(taps: &[f32]) -> Kernel {
        let mut k = Kernel::default();
        k.is_zero = false;
        k.len = taps.len();
        k.taps[..taps.len()].copy_from_slice(taps);
        for j in 0..taps.len() {
            k.rev_taps[j] = taps[taps.len() - 1 - j];
        }
        k
    }

    #[test]
    fn unit_impulse_kernel_passes_audio_through() {
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        let k = kernel_from_taps(&taps);
        let mut ch = RawChannel::new();
        let input = [0.5, -0.3, 0.9, 0.1];
        for &s in &input {
            let out = ch.process(s, &k);
            assert!(
                (out - s).abs() < 1e-6,
                "impulse kernel must pass {s} through, got {out}"
            );
        }
    }

    #[test]
    fn zero_kernel_is_dry_passthrough() {
        let k = Kernel::default(); // is_zero
        let mut ch = RawChannel::new();
        for &s in &[0.7, -0.4, 0.2] {
            assert!(
                (ch.process(s, &k) - s).abs() < 1e-6,
                "zero kernel must pass through"
            );
        }
    }

    #[test]
    fn silence_fast_path_outputs_exact_zero() {
        let k = bake(&MsegData::default(), 256); // non-zero kernel
        let mut ch = RawChannel::new();
        for _ in 0..10 {
            assert_eq!(ch.process(0.0, &k), 0.0);
        }
    }

    #[test]
    fn phaseless_reports_fixed_hop_latency() {
        assert_eq!(crate::convolution::PHASELESS_LATENCY, 2048);
    }

    #[test]
    fn phaseless_zero_kernel_is_dry_passthrough() {
        let k = Kernel::default(); // is_zero
        let mut ch = PhaselessChannel::new();
        let mut last = 0.0;
        for _ in 0..8192 {
            last = ch.process(0.5, &k);
        }
        assert!(
            (last - 0.5).abs() < 1e-3,
            "zero kernel must pass through, got {last}"
        );
    }

    #[test]
    fn phaseless_flat_magnitude_preserves_signal_energy() {
        // A kernel whose magnitude spectrum is all-ones (flat) must, after the
        // pipeline fills, reproduce a steady input within a small tolerance.
        let mut k = Kernel::default();
        k.is_zero = false;
        k.len = 4096;
        k.mags = [1.0; MAG_BINS];
        let mut ch = PhaselessChannel::new();
        let mut last = 0.0;
        for _ in 0..16384 {
            last = ch.process(0.5, &k);
        }
        assert!(
            (last - 0.5).abs() < 5e-3,
            "flat magnitude ~unity, got {last}"
        );
    }

    #[test]
    fn known_kernel_yields_known_output() {
        // ASYMMETRIC 2-tap kernel h = [1.0, 0.5] padded to 16. The FIR
        // convolution is y[n] = taps[0]*x[n] + taps[1]*x[n-1]
        //                     = 1.0*x[n] + 0.5*x[n-1].
        // Asymmetric on purpose: a kernel/window reversal bug (computing
        // 0.5*x[n] + 1.0*x[n-1] instead) fails this test.
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        taps[1] = 0.5;
        let k = kernel_from_taps(&taps);
        let mut ch = RawChannel::new();
        assert!((ch.process(1.0, &k) - 1.0).abs() < 1e-6); // 1*1 + 0.5*0
        assert!((ch.process(1.0, &k) - 1.5).abs() < 1e-6); // 1*1 + 0.5*1
        assert!((ch.process(0.0, &k) - 0.5).abs() < 1e-6); // 1*0 + 0.5*1
    }
}
