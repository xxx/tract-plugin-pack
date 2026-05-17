//! Raw + Phaseless convolution engines, adapted from wavetable-filter's
//! proven DSP. A self-contained module — no plugin or GUI types.

use crate::kernel::{Kernel, MAX_KERNEL};

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

/// Per-channel STFT magnitude-only convolution state. A thin wrapper over
/// `tract_dsp::stft::StftConvolver` with a fixed `STFT_FRAME`-point transform.
pub struct PhaselessChannel {
    conv: tract_dsp::stft::StftConvolver,
}

impl PhaselessChannel {
    pub fn new() -> Self {
        Self {
            conv: tract_dsp::stft::StftConvolver::new(STFT_FRAME),
        }
    }

    /// Zero all state.
    pub fn reset(&mut self) {
        self.conv.reset();
    }

    /// Process one sample. Output is always delayed by `PHASELESS_HOP` samples.
    ///
    /// A zero kernel maps to per-bin gain 1.0 (identity) — a *delayed* dry
    /// passthrough, never a 0-delay bypass.
    pub fn process(&mut self, sample: f32, kernel: &Kernel) -> f32 {
        self.conv.process(sample, &kernel.mags, !kernel.is_zero)
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
