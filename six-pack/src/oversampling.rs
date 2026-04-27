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

/// Run one upsampling stage: read `src_len` samples from `src`, write
/// `2 * src_len` samples to `dst`. No allocation.
fn upsample_one_stage(stage: &mut HalfBandStage, src: &[f32], dst: &mut [f32]) {
    debug_assert!(dst.len() >= src.len() * 2);
    let mut wr = 0;
    for &x in src {
        let (a, b) = stage.upsample(x);
        dst[wr] = a;
        dst[wr + 1] = b;
        wr += 2;
    }
}

/// Run one downsampling stage: read `src_len` samples from `src` (must be
/// even-length), write `src_len / 2` samples to `dst`. No allocation.
fn downsample_one_stage(stage: &mut HalfBandStage, src: &[f32], dst: &mut [f32]) {
    debug_assert!(src.len().is_multiple_of(2));
    debug_assert!(dst.len() >= src.len() / 2);
    let mut wr = 0;
    let mut i = 0;
    while i + 1 < src.len() {
        dst[wr] = stage.downsample(src[i], src[i + 1]);
        wr += 1;
        i += 2;
    }
}

/// Two-channel cascaded half-band oversampler.
///
/// Real-time-safe: all working buffers (`stage_scratch_*`, `down_scratch_*`,
/// `scratch_*`) are pre-allocated by `set_factor`. `upsample_block` /
/// `downsample_block` perform zero allocations on the hot path.
pub struct StereoOversampler {
    factor: usize, // 1, 4, 8, or 16
    up_l: Vec<HalfBandStage>,
    up_r: Vec<HalfBandStage>,
    down_l: Vec<HalfBandStage>,
    down_r: Vec<HalfBandStage>,
    /// Per-stage scratch for the upsampler cascade.
    /// `stage_scratch_*[i]` holds the signal *after* stage `i`.
    stage_scratch_l: Vec<Vec<f32>>,
    stage_scratch_r: Vec<Vec<f32>>,
    /// Final-rate scratch for processing.
    pub scratch_l: Vec<f32>,
    pub scratch_r: Vec<f32>,
    /// Per-stage scratch for the downsampler cascade.
    /// `down_scratch_*[i]` holds the signal *after* downsample stage `i`.
    down_scratch_l: Vec<Vec<f32>>,
    down_scratch_r: Vec<Vec<f32>>,
}

impl StereoOversampler {
    pub fn new() -> Self {
        Self {
            factor: 1,
            up_l: Vec::new(),
            up_r: Vec::new(),
            down_l: Vec::new(),
            down_r: Vec::new(),
            stage_scratch_l: Vec::new(),
            stage_scratch_r: Vec::new(),
            scratch_l: Vec::new(),
            scratch_r: Vec::new(),
            down_scratch_l: Vec::new(),
            down_scratch_r: Vec::new(),
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

        // Pre-allocate stage scratch: stage i holds 2^(i+1) * max_block samples.
        self.stage_scratch_l.clear();
        self.stage_scratch_r.clear();
        self.down_scratch_l.clear();
        self.down_scratch_r.clear();
        let mut size = max_block;
        for _ in 0..n_stages {
            size *= 2;
            self.stage_scratch_l.push(vec![0.0; size]);
            self.stage_scratch_r.push(vec![0.0; size]);
            self.down_scratch_l.push(vec![0.0; size]);
            self.down_scratch_r.push(vec![0.0; size]);
        }
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
        for v in &mut self.stage_scratch_l {
            v.fill(0.0);
        }
        for v in &mut self.stage_scratch_r {
            v.fill(0.0);
        }
        for v in &mut self.down_scratch_l {
            v.fill(0.0);
        }
        for v in &mut self.down_scratch_r {
            v.fill(0.0);
        }
    }

    /// Upsample one input block of (L, R) into the internal scratch.
    /// Real-time-safe: no allocations.
    pub fn upsample_block(&mut self, input_l: &[f32], input_r: &[f32]) -> (&mut [f32], &mut [f32]) {
        let n = input_l.len();
        if self.factor == 1 {
            self.scratch_l[..n].copy_from_slice(input_l);
            self.scratch_r[..n].copy_from_slice(input_r);
            return (&mut self.scratch_l[..n], &mut self.scratch_r[..n]);
        }

        upsample_channel(
            &mut self.up_l,
            &mut self.stage_scratch_l,
            input_l,
            &mut self.scratch_l,
        );
        upsample_channel(
            &mut self.up_r,
            &mut self.stage_scratch_r,
            input_r,
            &mut self.scratch_r,
        );

        let cap = n * self.factor;
        (&mut self.scratch_l[..cap], &mut self.scratch_r[..cap])
    }

    /// Downsample the scratch back into output buffers.
    /// Real-time-safe: no allocations.
    pub fn downsample_block(&mut self, output_l: &mut [f32], output_r: &mut [f32]) {
        let n = output_l.len();
        if self.factor == 1 {
            output_l.copy_from_slice(&self.scratch_l[..n]);
            output_r.copy_from_slice(&self.scratch_r[..n]);
            return;
        }

        let total_os = n * self.factor;
        downsample_channel(
            &mut self.down_l,
            &self.scratch_l[..total_os],
            &mut self.down_scratch_l,
            output_l,
        );
        downsample_channel(
            &mut self.down_r,
            &self.scratch_r[..total_os],
            &mut self.down_scratch_r,
            output_r,
        );
    }
}

/// Run the full upsampling cascade for one channel.
///
/// Stage 0 reads from `input` and writes to `stage_scratch[0]`.
/// Stage k (k > 0) reads from `stage_scratch[k - 1]` and writes to
/// `stage_scratch[k]`. After all stages, the final stage's slice is copied
/// into `final_scratch`.
fn upsample_channel(
    stages: &mut [HalfBandStage],
    stage_scratch: &mut [Vec<f32>],
    input: &[f32],
    final_scratch: &mut [f32],
) {
    let n_stages = stages.len();
    debug_assert!(n_stages > 0);
    debug_assert_eq!(stage_scratch.len(), n_stages);

    let mut current_len = input.len();

    // Stage 0: input → stage_scratch[0].
    {
        let new_len = current_len * 2;
        let dst = &mut stage_scratch[0][..new_len];
        upsample_one_stage(&mut stages[0], input, dst);
        current_len = new_len;
    }

    // Stages 1..n_stages: stage_scratch[idx-1] → stage_scratch[idx].
    // Use split_at_mut to borrow source (immutable) and destination
    // (mutable) simultaneously without allocation.
    for idx in 1..n_stages {
        let new_len = current_len * 2;
        let (left_part, right_part) = stage_scratch.split_at_mut(idx);
        let src = &left_part[idx - 1][..current_len];
        let dst = &mut right_part[0][..new_len];
        upsample_one_stage(&mut stages[idx], src, dst);
        current_len = new_len;
    }

    // Copy final-stage output into the shared final-rate scratch.
    final_scratch[..current_len].copy_from_slice(&stage_scratch[n_stages - 1][..current_len]);
}

/// Run the full downsampling cascade for one channel.
///
/// Stage 0 reads from `final_scratch` and writes to `down_scratch[0]`
/// (or directly to `output` if it's the only stage).
/// Stage k (k > 0) reads from `down_scratch[k - 1]` and writes to
/// `down_scratch[k]` (or directly to `output` for the last stage).
fn downsample_channel(
    stages: &mut [HalfBandStage],
    final_scratch: &[f32],
    down_scratch: &mut [Vec<f32>],
    output: &mut [f32],
) {
    let n_stages = stages.len();
    debug_assert!(n_stages > 0);
    debug_assert_eq!(down_scratch.len(), n_stages);

    let mut current_len = final_scratch.len();

    for idx in 0..n_stages {
        let new_len = current_len / 2;
        let is_last = idx == n_stages - 1;

        // Source slice: either the final-rate scratch (idx==0) or the
        // previous stage's output (idx>0). split_at_mut isolates the
        // mutable destination borrow when reading down_scratch[idx-1].
        if idx == 0 {
            if is_last {
                downsample_one_stage(&mut stages[0], &final_scratch[..current_len], output);
            } else {
                let dst = &mut down_scratch[0][..new_len];
                downsample_one_stage(&mut stages[0], &final_scratch[..current_len], dst);
            }
        } else {
            let (left_part, right_part) = down_scratch.split_at_mut(idx);
            let src = &left_part[idx - 1][..current_len];
            if is_last {
                downsample_one_stage(&mut stages[idx], src, output);
            } else {
                let dst = &mut right_part[0][..new_len];
                downsample_one_stage(&mut stages[idx], src, dst);
            }
        }

        current_len = new_len;
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
