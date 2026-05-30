//! Analytic bake of the wet-path impulse response, and its UPOLS partition
//! spectra, for the Efficient (FFT) engine. GUI/setup thread only.

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;

use crate::coloration::{Dictionary, OnePole, Q};
use crate::engine::{DC_BLOCKER_R, MAX_TAIL_SECONDS, SETTLE_SAMPLES};
use crate::sequence::VelvetSequence;
use tract_dsp::partitioned_conv::{BINS, N, P};

/// Max IR length: max tail + filter settle, rounded up to a whole partition.
pub fn max_ir_len(sample_rate: f32) -> usize {
    (((MAX_TAIL_SECONDS + 0.1) * sample_rate) as usize + SETTLE_SAMPLES).div_ceil(P) * P
}

/// A baked per-channel IR as UPOLS partition spectra. Pre-allocated to the max.
pub struct IrSpectra {
    pub spectra: Vec<Complex<f32>>, // max_k * BINS, only k*BINS valid
    pub k: usize,
}

impl IrSpectra {
    pub fn new(sample_rate: f32) -> Self {
        let max_k = max_ir_len(sample_rate).div_ceil(P).max(1);
        Self {
            spectra: vec![Complex::new(0.0, 0.0); max_k * BINS],
            k: 0,
        }
    }
    pub fn copy_from(&mut self, other: &IrSpectra) {
        self.k = other.k;
        self.spectra[..self.k * BINS].copy_from_slice(&other.spectra[..self.k * BINS]);
    }

    /// Grow the spectra buffer in place to hold the max IR for `sample_rate`
    /// (no-op if already large enough). GUI/setup thread only — used to bring a
    /// default-sized (48 kHz) instance up to the real sample rate so a max-Size
    /// IR at 96/192 kHz fits. Resizing in place keeps any shared `Arc` valid.
    pub fn resize_for(&mut self, sample_rate: f32) {
        let want = max_ir_len(sample_rate).div_ceil(P).max(1) * BINS;
        if self.spectra.len() < want {
            self.spectra.resize(want, Complex::new(0.0, 0.0));
        }
    }
}

/// Reusable GUI-thread baker (owns the IR scratch + FFT planner).
pub struct IrBaker {
    sample_rate: f32,
    dict: Dictionary,
    post: OnePole,
    /// Per-filter sparse excitation buffer + the summed/filtered IR, length max_ir_len.
    band: Vec<f32>,
    h: Vec<f32>,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    fft_block: Vec<f32>,
    fft_out: Vec<Complex<f32>>,
}

impl IrBaker {
    pub fn new(sample_rate: f32) -> Self {
        let l = max_ir_len(sample_rate);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let fft_out = fft.make_output_vec();
        Self {
            sample_rate,
            dict: Dictionary::new(sample_rate),
            post: OnePole::new(12_000.0, sample_rate),
            band: vec![0.0; l],
            h: vec![0.0; l],
            fft,
            fft_block: vec![0.0; N],
            fft_out,
        }
    }

    /// Bake the IR for the channel whose taps are `location` (use `seq.location`
    /// for L, `seq.location_r` for R) into `out`. `coeff`/`filter_idx` shared.
    pub fn bake(&mut self, seq: &VelvetSequence, location: &[u32], out: &mut IrSpectra) {
        let l = (seq.tail_len + SETTLE_SAMPLES).min(self.h.len());
        self.h[..l].iter_mut().for_each(|x| *x = 0.0);

        // For each coloration filter: place its pulses, run the one-pole, add to h.
        for q in 0..Q {
            self.band[..l].iter_mut().for_each(|x| *x = 0.0);
            for ((loc, coeff), &fidx) in location[..seq.count]
                .iter()
                .zip(seq.coeff[..seq.count].iter())
                .zip(seq.filter_idx[..seq.count].iter())
            {
                if fidx as usize == q {
                    let loc = *loc as usize;
                    if loc < l {
                        self.band[loc] += coeff;
                    }
                }
            }
            // OnePole is Copy; copy the filter from dict and reset it.
            let mut f = self.dict.filters[q];
            f.reset();
            for i in 0..l {
                self.h[i] += f.process(self.band[i]);
            }
        }

        // Post-LP then DC blocker over h, exactly matching ReverbChannel.
        let mut post = self.post;
        post.reset();
        let (mut dc_x1, mut dc_y1) = (0.0f32, 0.0f32);
        for i in 0..l {
            let p = post.process(self.h[i]);
            let y = p - dc_x1 + DC_BLOCKER_R * dc_y1;
            dc_x1 = p;
            dc_y1 = y;
            self.h[i] = y;
        }

        // Partition h into k blocks of P, zero-pad to N, forward-FFT.
        let k = l.div_ceil(P).max(1);
        for j in 0..k {
            self.fft_block.iter_mut().for_each(|x| *x = 0.0);
            let start = j * P;
            let end = (start + P).min(l);
            self.fft_block[..end - start].copy_from_slice(&self.h[start..end]);
            self.fft
                .process(&mut self.fft_block, &mut self.fft_out)
                .unwrap();
            out.spectra[j * BINS..j * BINS + BINS].copy_from_slice(&self.fft_out);
        }
        out.k = k;
        let _ = self.sample_rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ReverbChannel;
    use crate::rng::Rng;

    #[test]
    fn resize_for_grows_to_higher_sample_rate() {
        let mut s = IrSpectra::new(48_000.0);
        let lo = s.spectra.len();
        s.resize_for(192_000.0);
        let want = max_ir_len(192_000.0).div_ceil(P).max(1) * BINS;
        assert_eq!(s.spectra.len(), want);
        assert!(s.spectra.len() > lo, "192k buffer must exceed 48k buffer");
        // Resizing back down (or to the same SR) never shrinks.
        s.resize_for(48_000.0);
        assert_eq!(s.spectra.len(), want, "resize_for never shrinks");
    }

    #[test]
    fn analytic_ir_matches_engine_impulse_response() {
        // Small seq so the engine impulse response is cheap to capture.
        let mut seq = VelvetSequence::new();
        let n = 50;
        seq.count = n;
        let mut rng = Rng::new(8);
        for m in 0..n {
            let loc = (rng.next_u64() % 300) as u32;
            seq.location[m] = loc;
            seq.location_r[m] = loc;
            seq.coeff[m] = (rng.next_f32() * 2.0 - 1.0) * 0.2;
            seq.filter_idx[m] = (rng.next_u64() % Q as u64) as u8;
        }
        seq.tail_len = 300;
        let l = seq.tail_len + SETTLE_SAMPLES;

        // Engine impulse response.
        let mut ch = ReverbChannel::new(48_000.0);
        let mut ir_engine = vec![0.0f32; l];
        ir_engine[0] = ch.process(1.0, &seq, &seq.location);
        for s in ir_engine.iter_mut().skip(1) {
            *s = ch.process(0.0, &seq, &seq.location);
        }

        // Analytic IR (inverse-FFT the baked spectra back, or compare h directly):
        // bake, then reconstruct h from partitions and compare.
        let mut baker = IrBaker::new(48_000.0);
        let mut spec = IrSpectra::new(48_000.0);
        baker.bake(&seq, &seq.location, &mut spec);
        // Reconstruct time-domain IR from the partition spectra via inverse FFT.
        let mut planner = RealFftPlanner::<f32>::new();
        let ifft = planner.plan_fft_inverse(N);
        let mut recon = vec![0.0f32; l];
        let mut tmp = ifft.make_output_vec();
        for j in 0..spec.k {
            let mut sp: Vec<Complex<f32>> = spec.spectra[j * BINS..j * BINS + BINS].to_vec();
            ifft.process(&mut sp, &mut tmp).unwrap();
            let scale = 1.0 / N as f32;
            for i in 0..P {
                let idx = j * P + i;
                if idx < l {
                    recon[idx] = tmp[i] * scale;
                }
            }
        }
        for n in 0..l.min(1000) {
            assert!(
                (recon[n] - ir_engine[n]).abs() <= 1e-3 + 1e-3 * ir_engine[n].abs(),
                "ir tap {n}: analytic {} vs engine {}",
                recon[n],
                ir_engine[n]
            );
        }
    }
}
