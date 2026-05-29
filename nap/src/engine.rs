//! Audio-thread playback: sparse signed tapped-delay-line convolution into the
//! coloration filter bank, then post-filter + DC blocker, pre-delay, dry/wet.

use crate::coloration::{Dictionary, OnePole, Q};
use crate::sequence::VelvetSequence;

/// Max tail length in seconds (matches the Size param's maximum). The ring is
/// sized for this plus a little headroom for the per-pulse right-channel jitter
/// (Width up to 30 ms).
pub const MAX_TAIL_SECONDS: f32 = 10.0;

/// DC blocker pole radius (one-pole high-pass `y = x - x1 + R·y1`).
const DC_BLOCKER_R: f32 = 0.995;

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
    fn reset_clears_tail() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 10], &[1.0, 1.0], &[0, 0]);
        let _ = ch.process(1.0, &seq, &seq.location);
        ch.reset();
        let after = ch.process(0.0, &seq, &seq.location);
        assert!(after.abs() < 1e-9, "reset should zero the ring + filter state");
    }
}
