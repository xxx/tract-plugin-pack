//! Linear-phase polyphase oversampling for factors 4 / 8 / 16.
//!
//! Implementation: cascaded half-band stages. Each stage upsamples 2× by
//! polyphase filtering, downsamples 2× the symmetrical way. Total factor
//! is 2^N for N stages.

const HALFBAND_TAPS: usize = 31;

/// Compile-time can't compute trig, so we recompute the half-band taps
/// once at struct construction. Hamming-windowed sinc, length 31, group
/// delay 15 OS samples per stage.
fn compute_halfband_taps() -> [f32; HALFBAND_TAPS] {
    let mut c = [0.0_f32; HALFBAND_TAPS];
    use std::f32::consts::PI;
    for (i, slot) in c.iter_mut().enumerate() {
        let n = i as isize - 15;
        if n == 0 {
            *slot = 0.5;
        } else if n % 2 == 0 {
            *slot = 0.0;
        } else {
            let nf = n as f32;
            let sinc = (PI * nf * 0.5).sin() / (PI * nf * 0.5);
            // Hamming window of length HALFBAND_TAPS:
            let w = 0.54 - 0.46 * (2.0 * PI * (i as f32) / ((HALFBAND_TAPS - 1) as f32)).cos();
            *slot = 0.5 * sinc * w;
        }
    }
    c
}

#[derive(Clone)]
pub struct HalfBandStage {
    taps: [f32; HALFBAND_TAPS],
    state: [f32; HALFBAND_TAPS],
    pos: usize,
}

impl Default for HalfBandStage {
    fn default() -> Self {
        Self {
            taps: compute_halfband_taps(),
            state: [0.0; HALFBAND_TAPS],
            pos: 0,
        }
    }
}

impl HalfBandStage {
    /// Push one sample, return two upsampled samples.
    pub fn upsample(&mut self, x: f32) -> (f32, f32) {
        // Two-sample output via polyphase decomposition (insert zero between
        // samples and convolve; even output uses center tap of original signal,
        // odd output uses the windowed sinc).
        self.state[self.pos] = x;
        self.pos = (self.pos + 1) % HALFBAND_TAPS;
        let mut sum_a = 0.0;
        let mut sum_b = 0.0;
        for i in 0..HALFBAND_TAPS {
            let s = self.state[(self.pos + HALFBAND_TAPS - 1 - i) % HALFBAND_TAPS];
            let t = self.taps[i];
            if i % 2 == 0 {
                sum_a += s * t;
            } else {
                sum_b += s * t;
            }
        }
        // sum_a captures the inserted-zero positions; sum_b captures the
        // direct-pass positions. Multiply by 2 to compensate for energy loss
        // at the inserted zeros.
        (2.0 * sum_a, 2.0 * sum_b)
    }

    /// Push two samples, return one downsampled sample.
    pub fn downsample(&mut self, a: f32, b: f32) -> f32 {
        // Mirrored decimating polyphase. Push both samples, output filtered.
        self.state[self.pos] = a;
        self.pos = (self.pos + 1) % HALFBAND_TAPS;
        self.state[self.pos] = b;
        self.pos = (self.pos + 1) % HALFBAND_TAPS;
        let mut sum = 0.0;
        for i in 0..HALFBAND_TAPS {
            let s = self.state[(self.pos + HALFBAND_TAPS - 1 - i) % HALFBAND_TAPS];
            sum += s * self.taps[i];
        }
        sum
    }

    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.pos = 0;
    }
}

/// Group delay (in OS samples per stage).
pub const HALFBAND_GROUP_DELAY: usize = (HALFBAND_TAPS - 1) / 2;

/// Two-channel cascaded half-band oversampler.
///
/// IMPORTANT: This Task 19 implementation is correctness-focused and uses
/// `Vec::with_capacity` / `push` per `upsample_block` / `downsample_block` call.
/// Task 20 replaces these with pre-allocated stage scratch buffers for
/// real-time safety.
pub struct StereoOversampler {
    factor: usize, // 1, 4, 8, or 16
    up_l: Vec<HalfBandStage>,
    up_r: Vec<HalfBandStage>,
    down_l: Vec<HalfBandStage>,
    down_r: Vec<HalfBandStage>,
    /// Final-rate scratch (after upsampling, before downsampling).
    pub scratch_l: Vec<f32>,
    pub scratch_r: Vec<f32>,
}

impl StereoOversampler {
    pub fn new() -> Self {
        Self {
            factor: 1,
            up_l: Vec::new(),
            up_r: Vec::new(),
            down_l: Vec::new(),
            down_r: Vec::new(),
            scratch_l: Vec::new(),
            scratch_r: Vec::new(),
        }
    }

    pub fn set_factor(&mut self, factor: usize, max_block: usize) {
        let n_stages = match factor {
            1 => 0,
            4 => 2,
            8 => 3,
            16 => 4,
            _ => panic!("unsupported factor {}", factor),
        };
        self.factor = factor;
        self.up_l = (0..n_stages).map(|_| HalfBandStage::default()).collect();
        self.up_r = (0..n_stages).map(|_| HalfBandStage::default()).collect();
        self.down_l = (0..n_stages).map(|_| HalfBandStage::default()).collect();
        self.down_r = (0..n_stages).map(|_| HalfBandStage::default()).collect();
        let cap = max_block * factor.max(1);
        self.scratch_l.resize(cap, 0.0);
        self.scratch_r.resize(cap, 0.0);
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    /// Total round-trip latency in native samples (estimate; will be refined
    /// against measurements).
    pub fn latency_samples(&self) -> usize {
        let n_stages = self.up_l.len();
        let mut latency = 0usize;
        for stage in 0..n_stages {
            latency += HALFBAND_GROUP_DELAY >> stage;
        }
        latency * 2
    }

    pub fn reset(&mut self) {
        for s in &mut self.up_l {
            s.reset();
        }
        for s in &mut self.up_r {
            s.reset();
        }
        for s in &mut self.down_l {
            s.reset();
        }
        for s in &mut self.down_r {
            s.reset();
        }
        self.scratch_l.fill(0.0);
        self.scratch_r.fill(0.0);
    }

    /// Upsample one input block of (L, R) into the internal scratch.
    /// Task 19 implementation: allocates Vec per call. Task 20 will fix.
    pub fn upsample_block(&mut self, input_l: &[f32], input_r: &[f32]) -> (&mut [f32], &mut [f32]) {
        let n = input_l.len();
        let f = self.factor;
        if f == 1 {
            self.scratch_l[..n].copy_from_slice(input_l);
            self.scratch_r[..n].copy_from_slice(input_r);
            return (&mut self.scratch_l[..n], &mut self.scratch_r[..n]);
        }
        for ch in 0..2 {
            let stages = if ch == 0 {
                &mut self.up_l
            } else {
                &mut self.up_r
            };
            let scratch = if ch == 0 {
                &mut self.scratch_l
            } else {
                &mut self.scratch_r
            };
            let inp = if ch == 0 { input_l } else { input_r };
            let mut current_len = n;
            scratch[..n].copy_from_slice(inp);
            let mut tmp: Vec<f32> = scratch[..n].to_vec();
            for stage in stages.iter_mut() {
                let mut out = Vec::with_capacity(current_len * 2);
                for &x in &tmp {
                    let (a, b) = stage.upsample(x);
                    out.push(a);
                    out.push(b);
                }
                tmp = out;
                current_len *= 2;
            }
            scratch[..current_len].copy_from_slice(&tmp);
        }
        let cap = n * f;
        (&mut self.scratch_l[..cap], &mut self.scratch_r[..cap])
    }

    /// Downsample the scratch back into output buffers.
    pub fn downsample_block(&mut self, output_l: &mut [f32], output_r: &mut [f32]) {
        let n = output_l.len();
        let f = self.factor;
        if f == 1 {
            output_l.copy_from_slice(&self.scratch_l[..n]);
            output_r.copy_from_slice(&self.scratch_r[..n]);
            return;
        }
        for ch in 0..2 {
            let stages = if ch == 0 {
                &mut self.down_l
            } else {
                &mut self.down_r
            };
            let scratch = if ch == 0 {
                &self.scratch_l
            } else {
                &self.scratch_r
            };
            let out = if ch == 0 {
                &mut *output_l
            } else {
                &mut *output_r
            };
            let mut tmp: Vec<f32> = scratch[..n * f].to_vec();
            for stage in stages.iter_mut() {
                let mut next = Vec::with_capacity(tmp.len() / 2);
                let mut i = 0;
                while i + 1 < tmp.len() {
                    next.push(stage.downsample(tmp[i], tmp[i + 1]));
                    i += 2;
                }
                tmp = next;
            }
            out.copy_from_slice(&tmp);
        }
    }
}

impl Default for StereoOversampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfband_roundtrip_dc() {
        let mut up = HalfBandStage::default();
        let mut down = HalfBandStage::default();

        for _ in 0..(2 * HALFBAND_TAPS) {
            let (a, b) = up.upsample(0.0);
            let _ = down.downsample(a, b);
        }

        let input = 0.5;
        let mut last = 0.0;
        for _ in 0..(2 * HALFBAND_TAPS) {
            let (a, b) = up.upsample(input);
            last = down.downsample(a, b);
        }
        assert!(
            (last - input).abs() < 0.05,
            "DC roundtrip: in={input} out={last}"
        );
    }

    #[test]
    fn oversampler_roundtrip_4x_dc() {
        let mut os = StereoOversampler::new();
        os.set_factor(4, 64);
        let n = 64;
        let mut in_l = vec![0.0_f32; n];
        let mut in_r = vec![0.0_f32; n];
        let mut out_l = vec![0.0_f32; n];
        let mut out_r = vec![0.0_f32; n];

        for _ in 0..16 {
            let _ = os.upsample_block(&in_l, &in_r);
            os.downsample_block(&mut out_l, &mut out_r);
        }

        in_l.fill(0.5);
        in_r.fill(-0.3);
        for _ in 0..16 {
            let _ = os.upsample_block(&in_l, &in_r);
            os.downsample_block(&mut out_l, &mut out_r);
        }

        let last_l = out_l[n - 1];
        let last_r = out_r[n - 1];
        assert!((last_l - 0.5).abs() < 0.05, "L: {last_l} != 0.5");
        assert!((last_r - (-0.3)).abs() < 0.05, "R: {last_r} != -0.3");
    }
}
