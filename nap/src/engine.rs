//! Audio-thread playback: sparse signed tapped-delay-line convolution into the
//! coloration filter bank, then post-filter + DC blocker, pre-delay, dry/wet.
//!
//! Two convolution paths produce identical output:
//! * [`ReverbChannel::process`] — the per-sample reference: for each output
//!   sample it gathers every pulse's tap from scattered ring positions. Simple,
//!   but the random gather is cache-bound (~L2 latency per pulse).
//! * [`ReverbChannel::process_block`] — the hot path used in production. It
//!   convolves a whole block at once: per pulse, a *contiguous* ring window is
//!   SIMD-multiply-accumulated into its filter's excitation block. Same
//!   per-`(filter, sample)` summation order as the reference (so the output
//!   matches within FMA rounding), but sequential memory + `f32x16` vectorisation
//!   make it several times faster. The `block_matches_per_sample_reference`
//!   test gates the equivalence.

use std::simd::{f32x16, StdFloat};

use crate::coloration::{Dictionary, OnePole, Q};
use crate::sequence::VelvetSequence;

/// Max tail length in seconds (matches the Size param's maximum). The ring is
/// sized for this plus a little headroom for the per-pulse right-channel jitter
/// (Width up to 30 ms).
pub const MAX_TAIL_SECONDS: f32 = 10.0;

/// Convolution sub-block length (a multiple of the SIMD width). Bounds the
/// excitation scratch and keeps the per-block working set cache-resident.
pub const BLOCK: usize = 512;

/// DC blocker pole radius (one-pole high-pass `y = x - x1 + R·y1`).
const DC_BLOCKER_R: f32 = 0.995;

/// `dst[i] += c * src[i]` over equal-length slices, as a true fused
/// multiply-add (`vfmadd`), `f32x16`-vectorised with a scalar tail. Both slices
/// are contiguous — the cache-friendly, auto-prefetchable core of the block
/// convolution. `mul_add` lowers to one hardware FMA on the pack's `haswell`
/// release target (vs. the separate `vmulps`+`vaddps` a plain `d + c*s` emits).
#[inline]
fn fma_into(dst: &mut [f32], src: &[f32], c: f32) {
    debug_assert_eq!(dst.len(), src.len());
    let cv = f32x16::splat(c);
    let lanes = dst.len() / 16 * 16;
    let mut i = 0;
    while i < lanes {
        let s = f32x16::from_slice(&src[i..i + 16]);
        let d = f32x16::from_slice(&dst[i..i + 16]);
        cv.mul_add(s, d).copy_to_slice(&mut dst[i..i + 16]);
        i += 16;
    }
    for j in lanes..dst.len() {
        dst[j] = c.mul_add(src[j], dst[j]);
    }
}

/// One reverb channel: an input ring + the dictionary + post/DC state.
pub struct ReverbChannel {
    ring: Vec<f32>,
    mask: usize,
    write: usize,
    dict: Dictionary,
    post: OnePole,
    // DC blocker state (one-pole high-pass): y = x - x1 + R*y1
    dc_x1: f32,
    dc_y1: f32,
    /// Per-filter excitation scratch for `process_block`: `Q` blocks of `BLOCK`,
    /// laid out `[filter q][sample i]` at `exc[q * BLOCK + i]`. Pre-allocated.
    exc: Vec<f32>,
    /// Summed-wet scratch for `process_block` (`BLOCK` samples). Pre-allocated.
    wet: Vec<f32>,
}

impl ReverbChannel {
    /// Build a channel whose input ring is sized to hold the maximum tail
    /// (`MAX_TAIL_SECONDS` + jitter headroom) at `sample_rate`, rounded up to a
    /// power of two so tap reads can mask instead of branch. Sizing by the real
    /// sample rate is what keeps a 10 s tail from aliasing at 96/192 kHz.
    pub fn new(sample_rate: f32) -> Self {
        let cap = (((MAX_TAIL_SECONDS + 0.1) * sample_rate) as usize).next_power_of_two();
        Self {
            ring: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
            dict: Dictionary::new(sample_rate),
            post: OnePole::new(12_000.0, sample_rate),
            dc_x1: 0.0,
            dc_y1: 0.0,
            exc: vec![0.0; Q * BLOCK],
            wet: vec![0.0; BLOCK],
        }
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write = 0;
        self.dict.reset();
        self.post.reset();
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }

    /// Push one input sample and return the wet reverb sample, reading taps at
    /// `location` (use `location` for L, `location_r` for R). `coeff`/
    /// `filter_idx` are shared between channels.
    #[inline]
    pub fn process(&mut self, x: f32, seq: &VelvetSequence, location: &[u32]) -> f32 {
        debug_assert!(
            location.len() >= seq.count,
            "location slice shorter than pulse count"
        );

        // Write newest input.
        self.ring[self.write] = x;

        // Scatter pulses into per-filter excitation accumulators.
        let mut acc = [0.0f32; Q];
        for m in 0..seq.count {
            debug_assert!(
                (seq.filter_idx[m] as usize) < Q,
                "filter index out of dictionary range"
            );
            let idx = (self.write.wrapping_sub(location[m] as usize)) & self.mask;
            acc[seq.filter_idx[m] as usize] += seq.coeff[m] * self.ring[idx];
        }

        // Run the Q coloration filters, sum.
        let mut wet = 0.0f32;
        for (filt, &excitation) in self.dict.filters.iter_mut().zip(acc.iter()) {
            wet += filt.process(excitation);
        }

        // Post LP: gently tames the top octave (air absorption above the
        // dictionary's brightest filter) so the tail can't sparkle past ~12 kHz.
        wet = self.post.process(wet);
        // DC block.
        let y = wet - self.dc_x1 + DC_BLOCKER_R * self.dc_y1;
        self.dc_x1 = wet;
        self.dc_y1 = y;

        self.write = (self.write + 1) & self.mask;
        y
    }

    /// Block convolution — the production hot path. Writes `output[i]` = wet
    /// reverb for `input[i]`, for a block of up to `BLOCK` samples, reading taps
    /// at `location` (use `seq.location` for L, `seq.location_r` for R). Produces
    /// the same result as feeding the samples one at a time through [`process`]
    /// (within FMA rounding), but with sequential, SIMD-vectorised ring reads.
    ///
    /// Contract: `input.len() == output.len()` and the block length must be
    /// `<= BLOCK` (the scratch size); callers split larger host buffers into
    /// `BLOCK`-sized sub-blocks. Debug-asserted.
    pub fn process_block(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        seq: &VelvetSequence,
        location: &[u32],
    ) {
        let b = input.len();
        debug_assert_eq!(b, output.len());
        debug_assert!(b <= BLOCK, "block longer than scratch");
        debug_assert!(
            location.len() >= seq.count,
            "location slice shorter than pulse count"
        );
        let cap = self.mask + 1;
        let write_start = self.write;

        // 1. Write the input block into the ring (sequential).
        for (i, &x) in input.iter().enumerate() {
            self.ring[(write_start + i) & self.mask] = x;
        }

        // 2. Scatter each pulse into its filter's excitation block via a single
        //    contiguous (SIMD) multiply-accumulate. The window for pulse m at
        //    output sample i is ring[(write_start + i) - location[m]] — contiguous
        //    in i — matching `process`'s per-sample read exactly.
        for q in 0..Q {
            self.exc[q * BLOCK..q * BLOCK + b].fill(0.0);
        }
        for (m, &loc) in location[..seq.count].iter().enumerate() {
            let q = seq.filter_idx[m] as usize;
            debug_assert!(q < Q, "filter index out of dictionary range");
            let c = seq.coeff[m];
            let base = write_start.wrapping_sub(loc as usize) & self.mask;
            let exc = &mut self.exc[q * BLOCK..q * BLOCK + b];
            if base + b <= cap {
                fma_into(exc, &self.ring[base..base + b], c);
            } else {
                let first = cap - base;
                fma_into(&mut exc[..first], &self.ring[base..cap], c);
                fma_into(&mut exc[first..], &self.ring[..b - first], c);
            }
        }

        // 3. Run each coloration filter over its excitation block; sum into wet.
        self.wet[..b].fill(0.0);
        for q in 0..Q {
            let exc = &self.exc[q * BLOCK..q * BLOCK + b];
            let filt = &mut self.dict.filters[q];
            for (w, &e) in self.wet[..b].iter_mut().zip(exc.iter()) {
                *w += filt.process(e);
            }
        }

        // 4. Post LP + DC block per sample (cheap recursive tail), write output.
        for (out, &w) in output.iter_mut().zip(self.wet[..b].iter()) {
            let p = self.post.process(w);
            let y = p - self.dc_x1 + DC_BLOCKER_R * self.dc_y1;
            self.dc_x1 = p;
            self.dc_y1 = y;
            *out = y;
        }

        // 5. Advance the write pointer past the block.
        self.write = (write_start + b) & self.mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sequence with `Q=1`-routed pulses and an identity-ish setup to check
    /// the sparse convolution math in isolation. We bypass the filters by
    /// using a near-allpass (very high cutoff) and comparing relative shape.
    fn seq_from(locations: &[u32], coeffs: &[f32], filt: &[u8]) -> VelvetSequence {
        let mut s = VelvetSequence::new();
        s.count = locations.len();
        s.location[..s.count].copy_from_slice(locations);
        s.location_r[..s.count].copy_from_slice(locations);
        s.coeff[..s.count].copy_from_slice(coeffs);
        s.filter_idx[..s.count].copy_from_slice(filt);
        s.tail_len = *locations.iter().max().unwrap() as usize + 1;
        s
    }

    #[test]
    fn impulse_response_places_pulses_at_locations() {
        // Route everything through one filter; check the wet IR has energy
        // arriving at the pulse locations (post-filter smears, so check the
        // cumulative energy crosses thresholds at the right times).
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 100, 200], &[1.0, 0.5, 0.25], &[0, 0, 0]);
        let n = 400;
        let mut ir = vec![0.0f32; n];
        ir[0] = ch.process(1.0, &seq, &seq.location);
        for s in ir.iter_mut().skip(1) {
            *s = ch.process(0.0, &seq, &seq.location);
        }
        // Energy before sample 50 should come only from the first pulse.
        let e0: f32 = ir[..50].iter().map(|v| v * v).sum();
        let e1: f32 = ir[100..150].iter().map(|v| v * v).sum();
        let e2: f32 = ir[200..250].iter().map(|v| v * v).sum();
        assert!(e0 > 0.0 && e1 > 0.0 && e2 > 0.0);
        // Decaying coeffs → decaying per-pulse energy.
        assert!(e0 > e1 && e1 > e2, "energy should decay: {e0} {e1} {e2}");
    }

    #[test]
    fn silent_input_decays_to_silence() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 50, 100], &[1.0, 0.5, 0.25], &[2, 2, 2]);
        // Excite once.
        let _ = ch.process(1.0, &seq, &seq.location);
        // Run long enough for the tail + filters to settle.
        let mut last = 0.0;
        for _ in 0..20_000 {
            last = ch.process(0.0, &seq, &seq.location);
        }
        assert!(last.abs() < 1e-4, "tail should settle to ~0, got {last}");
    }

    #[test]
    fn empty_sequence_is_silent() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = VelvetSequence::new(); // count = 0
        let mut out = 0.0;
        for _ in 0..100 {
            out += ch.process(1.0, &seq, &seq.location).abs();
        }
        assert!(out < 1e-6, "no pulses → no wet output");
    }

    #[test]
    fn ring_covers_max_tail_at_high_sample_rate() {
        // At 192 kHz a 10 s tail needs ~1.92M samples; the ring must be large
        // enough that those taps don't alias through the mask.
        let ch = ReverbChannel::new(192_000.0);
        assert!(
            ch.ring.len() >= (MAX_TAIL_SECONDS * 192_000.0) as usize,
            "ring {} too small for a 10 s tail at 192 kHz",
            ch.ring.len()
        );
    }

    #[test]
    fn block_matches_per_sample_reference() {
        use crate::rng::Rng;
        // A non-trivial sequence: pulses spread across all Q filters with
        // distinct, jittered L/R locations and signed gains.
        let mut seq = VelvetSequence::new();
        let n = 300;
        seq.count = n;
        let mut rng = Rng::new(99);
        for m in 0..n {
            seq.location[m] = (rng.next_u64() % 4000) as u32;
            seq.location_r[m] = (rng.next_u64() % 4000) as u32;
            seq.coeff[m] = rng.next_f32() * 2.0 - 1.0;
            seq.filter_idx[m] = (rng.next_u64() % Q as u64) as u8;
        }
        seq.tail_len = 4001;

        // Random input long enough to span several full SIMD blocks plus a
        // ragged final chunk (1030 = 512 + 512 + 6) so the scalar tail runs too.
        let total = 1030;
        let mut rng = Rng::new(7);
        let input: Vec<f32> = (0..total).map(|_| rng.next_f32() * 2.0 - 1.0).collect();

        // Reference: one sample at a time.
        let mut refc = ReverbChannel::new(48_000.0);
        let ref_out: Vec<f32> = input
            .iter()
            .map(|&x| refc.process(x, &seq, &seq.location))
            .collect();

        // Block path, in BLOCK-sized chunks with a ragged tail.
        let mut blkc = ReverbChannel::new(48_000.0);
        let mut blk_out = vec![0.0f32; total];
        let mut off = 0;
        while off < total {
            let b = (total - off).min(BLOCK);
            blkc.process_block(
                &input[off..off + b],
                &mut blk_out[off..off + b],
                &seq,
                &seq.location,
            );
            off += b;
        }

        for i in 0..total {
            assert!(
                (ref_out[i] - blk_out[i]).abs() <= 1e-4 + 1e-4 * ref_out[i].abs(),
                "sample {i}: block {} vs per-sample reference {}",
                blk_out[i],
                ref_out[i]
            );
        }
    }

    #[test]
    fn block_matches_reference_across_ring_wrap() {
        use crate::rng::Rng;
        // A small ring (sample_rate 1000 → ring 16384) so processing ~17k
        // samples wraps the write pointer, exercising process_block's two-span
        // wrap split against the per-sample reference (which wraps via & mask).
        let sr = 1000.0;
        let mut seq = VelvetSequence::new();
        let n = 120;
        seq.count = n;
        let mut rng = Rng::new(5);
        for m in 0..n {
            let loc = (rng.next_u64() % 2000) as u32;
            seq.location[m] = loc;
            seq.location_r[m] = loc;
            seq.coeff[m] = rng.next_f32() * 2.0 - 1.0;
            seq.filter_idx[m] = (rng.next_u64() % Q as u64) as u8;
        }
        seq.tail_len = 2001;

        let total = 17_000; // > ring (16384) → the write pointer wraps
        let mut rng = Rng::new(11);
        let input: Vec<f32> = (0..total).map(|_| rng.next_f32() * 2.0 - 1.0).collect();

        let mut refc = ReverbChannel::new(sr);
        let ref_out: Vec<f32> = input
            .iter()
            .map(|&x| refc.process(x, &seq, &seq.location))
            .collect();

        let mut blkc = ReverbChannel::new(sr);
        let mut blk_out = vec![0.0f32; total];
        let mut off = 0;
        while off < total {
            let b = (total - off).min(BLOCK);
            blkc.process_block(
                &input[off..off + b],
                &mut blk_out[off..off + b],
                &seq,
                &seq.location,
            );
            off += b;
        }

        for i in 0..total {
            assert!(
                (ref_out[i] - blk_out[i]).abs() <= 1e-4 + 1e-4 * ref_out[i].abs(),
                "wrap mismatch at sample {i}: block {} vs reference {}",
                blk_out[i],
                ref_out[i]
            );
        }
    }

    #[test]
    fn reset_clears_tail() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 10], &[1.0, 1.0], &[0, 0]);
        let _ = ch.process(1.0, &seq, &seq.location);
        ch.reset();
        let after = ch.process(0.0, &seq, &seq.location);
        assert!(
            after.abs() < 1e-9,
            "reset should zero the ring + filter state"
        );
    }
}
