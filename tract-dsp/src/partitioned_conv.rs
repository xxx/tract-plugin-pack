//! Uniformly-partitioned overlap-save (UPOLS) real convolution.
//!
//! Convolves a streaming input with a fixed impulse response that has been
//! pre-partitioned into `P`-sample blocks and forward-FFT'd elsewhere (the
//! audio thread never transforms the IR). Owns input/output FIFOs so it accepts
//! arbitrary block lengths; the I/O introduces exactly `P` samples of latency.

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use std::sync::Arc;

/// Partition / latency size and FFT size (`N = 2*P` for linear convolution).
pub const P: usize = 512;
pub const N: usize = 2 * P;
/// Real-FFT bin count for an `N`-point transform.
pub const BINS: usize = N / 2 + 1;

pub struct PartitionedConvolver {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    /// IR partition spectra, `max_k` blocks of `BINS` (only `k` active). Newest
    /// input pairs with partition 0.
    ir: Vec<Complex<f32>>,
    k: usize,
    max_k: usize,
    /// Frequency-domain delay line: `max_k` past input-block spectra (ring).
    fdl: Vec<Complex<f32>>,
    fdl_pos: usize,
    /// Time window [prev P | current P] and FFT/accumulator scratch.
    time: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    acc: Vec<Complex<f32>>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    prev_in: Vec<f32>,
    /// P-sample input/output FIFOs (output primed with P zeros → P latency).
    in_fifo: Vec<f32>,
    in_len: usize,
    out_fifo: Vec<f32>, // ring of length P
    out_read: usize,
}

impl PartitionedConvolver {
    /// `max_ir_len` bounds the IR length; `max_k = ceil(max_ir_len / P)`.
    pub fn new(max_ir_len: usize) -> Self {
        let max_k = max_ir_len.div_ceil(P).max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let ifft = planner.plan_fft_inverse(N);
        let scratch_fwd = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        Self {
            fft,
            ifft,
            ir: vec![Complex::new(0.0, 0.0); max_k * BINS],
            k: 0,
            max_k,
            fdl: vec![Complex::new(0.0, 0.0); max_k * BINS],
            fdl_pos: 0,
            time: vec![0.0; N],
            spectrum: vec![Complex::new(0.0, 0.0); BINS],
            acc: vec![Complex::new(0.0, 0.0); BINS],
            scratch_fwd,
            scratch_inv,
            prev_in: vec![0.0; P],
            in_fifo: vec![0.0; P],
            in_len: 0,
            // out_fifo primed with P zeros (out_read starts at 0) → P latency.
            out_fifo: vec![0.0; P],
            out_read: 0,
        }
    }

    pub fn latency(&self) -> usize {
        P
    }

    /// Install `k` IR partition spectra (`k * BINS` complex values, partition 0
    /// = first P taps). Clears the FDL so the new IR starts clean. GUI/setup or
    /// reset only.
    pub fn set_ir(&mut self, spectra: &[Complex<f32>], k: usize) {
        debug_assert!(k <= self.max_k);
        debug_assert_eq!(spectra.len(), k * BINS);
        self.ir[..k * BINS].copy_from_slice(spectra);
        for s in self.ir[k * BINS..].iter_mut() {
            *s = Complex::new(0.0, 0.0);
        }
        self.k = k;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.fdl
            .iter_mut()
            .for_each(|c| *c = Complex::new(0.0, 0.0));
        self.fdl_pos = 0;
        self.prev_in.fill(0.0);
        self.in_len = 0;
        self.out_fifo.fill(0.0);
        self.out_read = 0;
    }

    /// Convolve `input` → `output` (equal lengths, any size). Allocation-free.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (inp, out) in input.iter().zip(output.iter_mut()) {
            // Emit one sample from the output FIFO first (primed with P zeros).
            *out = self.out_fifo[self.out_read];
            self.out_read = (self.out_read + 1) % P;
            // Accumulate input; when a full P-sample block is ready, run the
            // partition and overwrite out_fifo with the next block's output.
            self.in_fifo[self.in_len] = *inp;
            self.in_len += 1;
            if self.in_len == P {
                self.run_partition();
                self.in_len = 0;
            }
        }
    }

    /// One P-sample UPOLS step: FFT the [prev|current] window, push to the FDL,
    /// multiply-accumulate against the IR partitions, inverse-FFT, and queue the
    /// last P samples (overlap-save discards the first P).
    fn run_partition(&mut self) {
        // window = [prev_in | in_fifo]
        self.time[..P].copy_from_slice(&self.prev_in);
        self.time[P..].copy_from_slice(&self.in_fifo);
        self.fft
            .process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch_fwd)
            .expect("forward FFT length matches planner");

        // Store newest spectrum into the FDL.
        let pos = self.fdl_pos;
        self.fdl[pos * BINS..pos * BINS + BINS].copy_from_slice(&self.spectrum);

        // acc = Σ_{j<k} ir[j] · fdl[(pos - j) mod k]
        for a in self.acc.iter_mut() {
            *a = Complex::new(0.0, 0.0);
        }
        for j in 0..self.k {
            let slot = (pos + self.k - j) % self.k;
            let ir = &self.ir[j * BINS..j * BINS + BINS];
            let x = &self.fdl[slot * BINS..slot * BINS + BINS];
            for (a, (h, xx)) in self.acc.iter_mut().zip(ir.iter().zip(x.iter())) {
                *a += *h * *xx;
            }
        }

        // realfft's ComplexToReal requires the DC and Nyquist bins to be purely
        // real; force them so the inverse transform can never return Err. (Their
        // imaginary parts are 0 anyway for this N, but this removes the latent
        // panic vector if N ever changes.)
        self.acc[0].im = 0.0;
        self.acc[BINS - 1].im = 0.0;
        // inverse FFT → time (realfft mutates the spectrum input).
        self.ifft
            .process_with_scratch(&mut self.acc, &mut self.time, &mut self.scratch_inv)
            .expect("inverse FFT length matches planner");
        let scale = 1.0 / N as f32;
        for (slot, &y) in self.out_fifo.iter_mut().zip(self.time[P..].iter()) {
            *slot = y * scale; // last P samples (overlap-save)
        }
        // The output FIFO is a simple P-ring read in lockstep with writes here;
        // out_read continues from where it was, out_fifo now holds this block.

        self.prev_in.copy_from_slice(&self.in_fifo);
        if self.k > 0 {
            self.fdl_pos = (self.fdl_pos + 1) % self.k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct time-domain convolution reference: out[n] = Σ_k ir[k]·in[n-k].
    fn direct_conv(input: &[f32], ir: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; input.len()];
        for n in 0..input.len() {
            let mut acc = 0.0f32;
            for (k, &h) in ir.iter().enumerate() {
                if n >= k {
                    acc += h * input[n - k];
                }
            }
            out[n] = acc;
        }
        out
    }

    /// Partition `ir` into k blocks of P, zero-pad each to N, forward-FFT.
    fn bake_spectra(ir: &[f32]) -> (Vec<Complex<f32>>, usize) {
        let k = ir.len().div_ceil(P).max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let mut spectra = vec![Complex::new(0.0, 0.0); k * BINS];
        let mut block = vec![0.0f32; N];
        let mut sp = fft.make_output_vec();
        for j in 0..k {
            block.iter_mut().for_each(|x| *x = 0.0);
            let start = j * P;
            let end = (start + P).min(ir.len());
            block[..end - start].copy_from_slice(&ir[start..end]);
            fft.process(&mut block, &mut sp).unwrap();
            spectra[j * BINS..j * BINS + BINS].copy_from_slice(&sp);
        }
        (spectra, k)
    }

    #[test]
    fn upols_matches_direct_convolution_delayed_by_p() {
        use std::f32::consts::PI;
        // IR spanning several partitions; input a tone.
        let ir: Vec<f32> = (0..1500)
            .map(|n| (-(n as f32) / 400.0).exp() * (0.13 * n as f32).sin())
            .collect();
        let total = 6000;
        let input: Vec<f32> = (0..total)
            .map(|n| (2.0 * PI * 300.0 * n as f32 / 48000.0).sin())
            .collect();
        let reference = direct_conv(&input, &ir);

        let (spectra, k) = bake_spectra(&ir);
        let mut conv = PartitionedConvolver::new(ir.len());
        conv.set_ir(&spectra, k);
        let mut out = vec![0.0f32; total];
        // Feed in odd-sized chunks to exercise the FIFO with arbitrary blocks.
        let mut off = 0;
        for &chunk in [100usize, 512, 333, 1024, 7].iter().cycle() {
            if off >= total {
                break;
            }
            let b = chunk.min(total - off);
            let (i, o) = (&input[off..off + b], &mut out[off..off + b]);
            conv.process(i, o);
            off += b;
        }
        // UPOLS output is the convolution delayed by the convolver's latency.
        let lat = conv.latency();
        for n in lat..total - 10 {
            let got = out[n];
            let want = reference[n - lat];
            assert!(
                (got - want).abs() <= 1e-3 + 1e-3 * want.abs(),
                "n={n}: {got} vs {want}"
            );
        }
    }

    #[test]
    fn impulse_in_yields_the_ir() {
        let ir: Vec<f32> = (0..900)
            .map(|n| (if n % 7 == 0 { 0.5 } else { -0.2 }) * (-(n as f32) / 300.0).exp())
            .collect();
        let (spectra, k) = bake_spectra(&ir);
        let mut conv = PartitionedConvolver::new(ir.len());
        conv.set_ir(&spectra, k);
        let total = 2048;
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;
        let mut out = vec![0.0f32; total];
        conv.process(&input, &mut out);
        let lat = conv.latency();
        for n in 0..ir.len() {
            assert!(
                (out[n + lat] - ir[n]).abs() <= 1e-3,
                "tap {n}: {} vs {}",
                out[n + lat],
                ir[n]
            );
        }
    }

    #[test]
    fn reset_clears_state() {
        let ir = vec![1.0f32; 600];
        let (spectra, k) = bake_spectra(&ir);
        let mut conv = PartitionedConvolver::new(ir.len());
        conv.set_ir(&spectra, k);
        let mut out = vec![0.0f32; 2000];
        let mut input = vec![0.0f32; 2000];
        input[0] = 1.0;
        conv.process(&input, &mut out);
        conv.reset();
        let mut z = vec![0.0f32; 2000];
        conv.process(&vec![0.0f32; 2000], &mut z);
        assert!(z.iter().all(|&v| v.abs() <= 1e-6), "reset should silence");
    }
}
