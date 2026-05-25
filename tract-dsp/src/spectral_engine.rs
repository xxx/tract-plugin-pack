//! Audio-thread-safe per-channel STFT analysis/synthesis with switchable
//! FFT size.
//!
//! [`SpectralEngine`] pre-allocates all four supported FFT sizes
//! (512 / 1024 / 2048 / 4096) at construction. [`SpectralEngine::set_fft_size`]
//! latches a switch that takes effect at the next hop boundary of the new
//! slot, costing zero allocations on the audio thread.
//!
//! Effects implement [`SpectralTransform`] and pass an instance to
//! [`SpectralEngine::process_sample`] per call; the engine drives input
//! ring -> hop boundary -> analyze -> caller transform -> IFFT -> overlap-add
//! -> output sample.
//!
//! Hop ratio is fixed at 50% (`hop = fft_size / 2`), matching the periodic-Hann
//! analysis window's natural COLA point. Effects that need 75% overlap
//! (phase vocoders) hold their own analyzer outside the engine.

use crate::stft_analysis::StftAnalyzer;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The four supported FFT sizes, in display order. Effect param 0 selects
/// an index into this array.
pub const FFT_SIZES: [usize; 4] = [512, 1024, 2048, 4096];

/// A spectrum transform driven by [`SpectralEngine`]. The engine calls
/// [`transform`](Self::transform) once per hop with the freshest analysis
/// spectrum; the implementer mutates it in place. Magnitude AND phase are
/// fair game.
pub trait SpectralTransform {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32);
}

struct Slot {
    fft_size: usize,
    hop_size: usize,
    analyzer: StftAnalyzer,
    ifft: Arc<dyn Fft<f32>>,
    output_ring: Vec<f32>,
    output_pos: usize,
    hop_counter: usize,
    spectrum_scratch: Vec<Complex<f32>>,
    ifft_scratch: Vec<Complex<f32>>,
}

impl Slot {
    fn new(fft_size: usize, planner: &mut FftPlanner<f32>) -> Self {
        let hop_size = fft_size / 2;
        let ifft = planner.plan_fft_inverse(fft_size);
        let scratch_len = ifft.get_inplace_scratch_len();
        Self {
            fft_size,
            hop_size,
            analyzer: StftAnalyzer::new(fft_size, hop_size),
            ifft,
            output_ring: vec![0.0; fft_size],
            output_pos: 0,
            hop_counter: 0,
            spectrum_scratch: vec![Complex::default(); fft_size],
            ifft_scratch: vec![Complex::default(); scratch_len],
        }
    }

    fn reset(&mut self) {
        self.analyzer.reset();
        self.output_ring.fill(0.0);
        self.output_pos = 0;
        self.hop_counter = 0;
    }
}

/// Per-channel STFT engine. Construct one per audio channel.
pub struct SpectralEngine {
    slots: [Slot; 4],
    active: usize,
    pending: Option<usize>,
    sample_rate: f32,
}

impl SpectralEngine {
    /// Build a new engine with all four FFT sizes pre-allocated. Active
    /// FFT size defaults to 2048 (index 2 in [`FFT_SIZES`]).
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let slots = [
            Slot::new(FFT_SIZES[0], &mut planner),
            Slot::new(FFT_SIZES[1], &mut planner),
            Slot::new(FFT_SIZES[2], &mut planner),
            Slot::new(FFT_SIZES[3], &mut planner),
        ];
        Self {
            slots,
            active: 2,
            pending: None,
            sample_rate,
        }
    }

    /// Latch an FFT-size switch. Takes effect at the next hop boundary of the
    /// new slot. Calling with the currently active size cancels any pending
    /// switch. Unknown sizes are silently ignored.
    pub fn set_fft_size(&mut self, fft_size: usize) {
        if let Some(idx) = FFT_SIZES.iter().position(|&s| s == fft_size) {
            if idx == self.active {
                self.pending = None;
            } else {
                self.pending = Some(idx);
            }
        }
    }

    /// Current active FFT size.
    pub fn fft_size(&self) -> usize {
        self.slots[self.active].fft_size
    }

    /// Algorithmic latency in samples — equal to the active slot's hop size.
    pub fn latency_samples(&self) -> usize {
        self.slots[self.active].hop_size
    }

    /// Zero all ring buffers in all four slots. Used by `Effect::reset`.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.reset();
        }
        self.pending = None;
    }

    /// Push one input sample, optionally drive an analysis + transform +
    /// synthesis hop, pull and return one output sample. Allocation-free.
    pub fn process_sample<T: SpectralTransform>(&mut self, input: f32, t: &mut T) -> f32 {
        let slot = &mut self.slots[self.active];

        // Output read first — matches spectral_shifter and gives the engine
        // its full latency = hop_size (the just-overlap-added samples sit in
        // the ring until the read catches up).
        let out = slot.output_ring[slot.output_pos];
        slot.output_ring[slot.output_pos] = 0.0;
        slot.output_pos = (slot.output_pos + 1) % slot.fft_size;

        slot.analyzer.write(input);
        slot.hop_counter += 1;

        if slot.hop_counter >= slot.hop_size {
            slot.hop_counter = 0;

            let sample_rate = self.sample_rate;
            let fft_size = slot.fft_size;
            let frame = slot.analyzer.analyze();

            // Copy spectrum out, transform, IFFT back into spectrum_scratch.
            slot.spectrum_scratch.copy_from_slice(frame.spectrum);
            t.transform(&mut slot.spectrum_scratch, fft_size, sample_rate);

            // In-place IFFT.
            slot.ifft
                .process_with_scratch(&mut slot.spectrum_scratch, &mut slot.ifft_scratch);

            // 1/N normalisation + window + overlap-add.
            let inv_n = 1.0 / fft_size as f32;
            let synth = frame.synthesis_window;
            let ring = &mut slot.output_ring;
            let pos = slot.output_pos;
            let n = slot.fft_size;
            for (i, (&w, c)) in synth.iter().zip(slot.spectrum_scratch.iter()).enumerate() {
                let ring_idx = (pos + i) % n;
                ring[ring_idx] += c.re * inv_n * w;
            }

            // Apply pending FFT-size switch at hop boundary.
            if let Some(new_active) = self.pending.take() {
                self.active = new_active;
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex;

    /// An identity transform — leaves the spectrum untouched. Lets the tests
    /// validate the engine's analysis/synthesis path on its own.
    struct Identity;
    impl SpectralTransform for Identity {
        fn transform(&mut self, _s: &mut [Complex<f32>], _n: usize, _sr: f32) {}
    }

    /// A DC-bin-only transform — sets only `spectrum[0]` so the IFFT produces a
    /// constant time-domain frame, which the synthesis window then shapes to
    /// non-trivial OLA output. Used to confirm the synthesis path is wired.
    struct DcConstant;
    impl SpectralTransform for DcConstant {
        fn transform(&mut self, s: &mut [Complex<f32>], n: usize, _sr: f32) {
            s.fill(Complex::default());
            // IFFT of a single non-zero DC bin produces a flat time-domain
            // frame of value `s[0].re / n` after 1/N normalisation. Setting
            // s[0] = N gives a flat 1.0 in time domain, which the Hann
            // synthesis window shapes to a non-zero OLA contribution.
            s[0] = Complex::new(n as f32, 0.0);
        }
    }

    /// Drive `n_samples` of an input function through the engine with the
    /// given transform, returning the collected output.
    fn drive<F: FnMut(usize) -> f32, T: SpectralTransform>(
        engine: &mut SpectralEngine,
        n_samples: usize,
        mut input: F,
        t: &mut T,
    ) -> Vec<f32> {
        (0..n_samples)
            .map(|i| engine.process_sample(input(i), t))
            .collect()
    }

    #[test]
    fn fft_sizes_constant_matches_doc() {
        assert_eq!(FFT_SIZES, [512, 1024, 2048, 4096]);
    }

    #[test]
    fn default_active_is_2048() {
        let e = SpectralEngine::new(48_000.0);
        assert_eq!(e.fft_size(), 2048);
        assert_eq!(e.latency_samples(), 1024);
    }

    #[test]
    fn set_fft_size_latches_change_until_next_hop() {
        let mut e = SpectralEngine::new(48_000.0);
        e.set_fft_size(512);
        // Active is still 2048 right after the call — the switch is latched
        // until the next hop boundary completes inside process_sample.
        assert_eq!(e.fft_size(), 2048);

        // Drive enough samples to cross at least one hop_size (= 1024 for
        // 2048-pt). The pending switch must be consumed inside that window.
        let mut id = Identity;
        let _ = drive(&mut e, 1100, |_| 0.0, &mut id);
        assert_eq!(e.fft_size(), 512);
        assert_eq!(e.latency_samples(), 256);
    }

    #[test]
    fn set_fft_size_unknown_is_noop() {
        let mut e = SpectralEngine::new(48_000.0);
        e.set_fft_size(777); // not in FFT_SIZES
        assert!(e.pending.is_none());
        assert_eq!(e.fft_size(), 2048);
    }

    #[test]
    fn set_fft_size_to_active_cancels_pending_switch() {
        let mut e = SpectralEngine::new(48_000.0);
        // Latch a switch away from 2048.
        e.set_fft_size(512);
        assert_eq!(e.pending, Some(0));
        // Change our mind and set it back to the currently active 2048.
        e.set_fft_size(2048);
        assert_eq!(
            e.pending, None,
            "set_fft_size to active size should clear pending"
        );
        // Driving samples must not switch sizes.
        let mut id = Identity;
        let _ = (0..2200).map(|_| e.process_sample(0.0, &mut id)).count();
        assert_eq!(e.fft_size(), 2048);
    }

    #[test]
    fn identity_passes_sine_within_3db_after_latency() {
        let sr = 48_000.0;
        let mut e = SpectralEngine::new(sr);
        let f = 1000.0;
        let n = 8192_usize;
        let mut id = Identity;
        let out = drive(
            &mut e,
            n,
            |i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin(),
            &mut id,
        );

        // Skip the first 2 * latency samples while the ring fills (warm-up
        // until first analysis frame is OLA-deposited, then read).
        let warmup = 2 * e.latency_samples();
        let peak: f32 = out[warmup..].iter().cloned().fold(0.0, f32::max);
        let trough: f32 = out[warmup..].iter().cloned().fold(0.0, |a, x| a.min(x));
        let amp = (peak - trough) / 2.0;

        // Identity should reconstruct the input sine within 3 dB amplitude.
        // 3 dB linear = 10^(-3/20) = 0.708.
        assert!(
            amp > 0.708,
            "identity sine amplitude {amp} fell below 0.708 (3 dB below unity)"
        );
        assert!(
            amp < 1.0 / 0.708,
            "identity sine amplitude {amp} exceeded 1.41 (3 dB above unity)"
        );
    }

    #[test]
    fn impulse_response_finite_under_identity() {
        let mut e = SpectralEngine::new(48_000.0);
        let mut id = Identity;
        let out = drive(&mut e, 4096, |i| if i == 0 { 1.0 } else { 0.0 }, &mut id);
        assert!(out.iter().all(|x| x.is_finite()));
        // Identity must produce SOME non-zero output after the latency.
        let energy: f32 = out.iter().map(|x| x * x).sum();
        assert!(
            energy > 0.01,
            "identity impulse response energy {energy} too low"
        );
    }

    #[test]
    fn reset_zeros_all_slots() {
        let mut e = SpectralEngine::new(48_000.0);
        let mut id = Identity;
        // Run some content through.
        let _ = drive(&mut e, 4096, |i| ((i as f32) * 0.1).sin(), &mut id);
        // Switch slots so all four get exercised.
        for &size in &FFT_SIZES {
            e.set_fft_size(size);
            let _ = drive(&mut e, 2048, |i| ((i as f32) * 0.1).sin(), &mut id);
        }
        e.reset();
        // Drive silence — output must be exactly zero for the first sample
        // (no leftover ring content).
        let first = e.process_sample(0.0, &mut id);
        assert_eq!(first, 0.0);
    }

    #[test]
    fn dc_only_transform_produces_constant_output() {
        // A DC-bin-only spectrum IFFTs to a flat time-domain frame. After the
        // Hann synthesis window and overlap-add, the engine should output
        // a steady positive value (not zero, not NaN). Confirms the
        // synthesis path is wired correctly.
        let mut e = SpectralEngine::new(48_000.0);
        let mut t = DcConstant;
        let out = drive(&mut e, 4096, |_| 1.0, &mut t);
        // After warm-up, the output should be a roughly-steady non-zero value.
        let warmup = 2 * e.latency_samples();
        let tail = &out[warmup..];
        let mean: f32 = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(
            mean.abs() > 0.1,
            "dc-only transform produced near-zero mean output {mean}"
        );
        assert!(out.iter().all(|x| x.is_finite()));
    }
}
