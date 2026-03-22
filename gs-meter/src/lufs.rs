//! EBU R128 LUFS loudness metering DSP.
//!
//! Implements K-weighted filtering, momentary/short-term/integrated loudness,
//! and loudness range (LRA) per the EBU R128 / ITU-R BS.1770-4 standard.
//!
//! All accumulation uses f64 for numerical stability. No allocations occur
//! after construction (audio-thread safe).

/// Maximum supported sample rate for pre-allocation sizing.
const MAX_SAMPLE_RATE: f64 = 192_000.0;

/// Maximum block entries for integrated loudness gating (~2 hours at 100ms hop).
const MAX_BLOCK_ENTRIES: usize = 7200;

/// Maximum entries for LRA short-term energies (~2 hours at 1s hop).
const MAX_ST_ENTRIES: usize = 7200;

/// Absolute gate threshold in LUFS.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;

/// Convert mean-square energy to LUFS.
#[inline]
fn energy_to_loudness(energy: f64) -> f64 {
    if energy <= 0.0 {
        return f64::NEG_INFINITY;
    }
    10.0 * energy.log10() - 0.691
}

/// Compute the energy threshold corresponding to a LUFS value.
/// Inverse of energy_to_loudness: energy = 10^((lufs + 0.691) / 10)
#[inline]
fn loudness_to_energy(lufs: f64) -> f64 {
    f64::powf(10.0, (lufs + 0.691) / 10.0)
}

/// Compute combined 4th-order K-weighting filter coefficients for a given sample rate.
///
/// Two cascaded biquad stages (high-shelf + high-pass) convolved into a single
/// 4th-order IIR filter. Coefficients match the sdroege/ebur128 reference implementation.
fn filter_coefficients(rate: f64) -> ([f64; 5], [f64; 5]) {
    use std::f64::consts::PI;

    // Stage 1: High-shelf (+4 dB above ~1.5 kHz)
    let f0 = 1681.974450955533;
    #[allow(non_snake_case)]
    let G = 3.999843853973347;
    #[allow(non_snake_case)]
    let Q = 0.7071752369554196;
    #[allow(non_snake_case)]
    let K = f64::tan(PI * f0 / rate);
    #[allow(non_snake_case)]
    let Vh = f64::powf(10.0, G / 20.0);
    #[allow(non_snake_case)]
    let Vb = f64::powf(Vh, 0.4996667741545416);
    let a0 = 1.0 + K / Q + K * K;
    let pb = [
        (Vh + Vb * K / Q + K * K) / a0,
        2.0 * (K * K - Vh) / a0,
        (Vh - Vb * K / Q + K * K) / a0,
    ];
    let pa = [1.0, 2.0 * (K * K - 1.0) / a0, (1.0 - K / Q + K * K) / a0];

    // Stage 2: High-pass (~38 Hz)
    let f0 = 38.13547087602444;
    #[allow(non_snake_case)]
    let Q = 0.5003270373238773;
    #[allow(non_snake_case)]
    let K = f64::tan(PI * f0 / rate);
    let rb = [1.0, -2.0, 1.0];
    let a0_hp = 1.0 + K / Q + K * K;
    let ra = [
        1.0,
        2.0 * (K * K - 1.0) / a0_hp,
        (1.0 - K / Q + K * K) / a0_hp,
    ];

    // Convolve the two 2nd-order stages into a single 4th-order filter
    let b = [
        pb[0] * rb[0],
        pb[0] * rb[1] + pb[1] * rb[0],
        pb[0] * rb[2] + pb[1] * rb[1] + pb[2] * rb[0],
        pb[1] * rb[2] + pb[2] * rb[1],
        pb[2] * rb[2],
    ];
    let a = [
        pa[0] * ra[0],
        pa[0] * ra[1] + pa[1] * ra[0],
        pa[0] * ra[2] + pa[1] * ra[1] + pa[2] * ra[0],
        pa[1] * ra[2] + pa[2] * ra[1],
        pa[2] * ra[2],
    ];
    (b, a)
}

/// Per-channel K-weighting filter (4th-order IIR, Direct Form II transposed).
struct KWeightFilter {
    b: [f64; 5],
    a: [f64; 5],
    z: [f64; 4], // filter state
}

impl KWeightFilter {
    /// Create a new K-weighting filter for the given sample rate.
    fn new(sample_rate: f64) -> Self {
        let (b, a) = filter_coefficients(sample_rate);
        Self { b, a, z: [0.0; 4] }
    }

    /// Reset the filter state to zero.
    fn reset(&mut self) {
        self.z = [0.0; 4];
    }

    /// Recompute coefficients for a new sample rate and reset state.
    fn set_sample_rate(&mut self, sample_rate: f64) {
        let (b, a) = filter_coefficients(sample_rate);
        self.b = b;
        self.a = a;
        self.z = [0.0; 4];
    }

    /// Filter one sample through the 4th-order K-weighting IIR.
    /// Returns the K-weighted output as f64.
    #[inline]
    fn process_sample(&mut self, x: f32) -> f64 {
        let x = x as f64;
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[1] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[2] * y + self.z[2];
        self.z[2] = self.b[3] * x - self.a[3] * y + self.z[3];
        self.z[3] = self.b[4] * x - self.a[4] * y;
        y
    }
}

/// EBU R128 LUFS loudness meter for stereo audio.
///
/// Provides momentary (400ms), short-term (3s), integrated (gated), and
/// loudness range (LRA) measurements per the EBU R128 specification.
///
/// All ring buffers are pre-allocated at construction for the maximum supported
/// sample rate (192 kHz). No allocations occur during `process_sample`.
pub struct LufsMeter {
    // K-weighting filters (one per channel)
    filter_l: KWeightFilter,
    filter_r: KWeightFilter,

    // Momentary loudness: 400ms sliding window of K-weighted squared samples.
    // At 192kHz, 400ms = 76800 samples. Uses O(1) running sum.
    momentary_ring: Vec<f64>,
    momentary_ring_pos: usize,
    momentary_ring_filled: usize,
    momentary_ring_sum: f64,
    momentary_window_size: usize,
    momentary_max: f64,

    // Short-term loudness: 3000ms sliding window.
    short_term_ring: Vec<f64>,
    short_term_ring_pos: usize,
    short_term_ring_filled: usize,
    short_term_ring_sum: f64,
    short_term_window_size: usize,
    short_term_max: f64,

    // Integrated loudness: gated block energies.
    // 400ms blocks with 75% overlap (100ms hop).
    block_ring: Vec<f64>,
    block_ring_pos: usize,
    block_ring_count: usize,
    block_sample_count: usize,
    samples_per_block: usize,
    samples_per_hop: usize,
    hop_counter: usize,

    // LRA: short-term (3s) block energies with 1s hop (2/3 overlap).
    st_block_energies: Vec<f64>,
    st_block_count: usize,
    st_hop_counter: usize,
    samples_per_st_block: usize,
    samples_per_st_hop: usize,

    // Cached integrated loudness to avoid O(n) scans every buffer
    cached_integrated: f64,
    cached_integrated_block_count: usize,

    // Cached LRA to avoid recomputation on audio thread
    cached_lra: f64,
    cached_lra_block_count: usize,

    // Pre-allocated scratch buffer for O(n log n) LRA percentile sort
    lra_scratch: Vec<f64>,

    sample_rate: f64,
}

impl LufsMeter {
    /// Create a new LUFS meter for the given sample rate.
    ///
    /// Pre-allocates all ring buffers for 192 kHz maximum sample rate.
    pub fn new(sample_rate: f64) -> Self {
        let momentary_max_size = (MAX_SAMPLE_RATE * 0.4) as usize; // 400ms at 192kHz
        let short_term_max_size = (MAX_SAMPLE_RATE * 3.0) as usize; // 3000ms at 192kHz

        let momentary_window_size = (sample_rate * 0.4) as usize;
        let short_term_window_size = (sample_rate * 3.0) as usize;
        let samples_per_block = (sample_rate * 0.4) as usize;
        let samples_per_hop = (sample_rate * 0.1) as usize;
        let samples_per_st_block = (sample_rate * 3.0) as usize;
        let samples_per_st_hop = (sample_rate * 1.0) as usize;

        Self {
            filter_l: KWeightFilter::new(sample_rate),
            filter_r: KWeightFilter::new(sample_rate),

            momentary_ring: vec![0.0; momentary_max_size],
            momentary_ring_pos: 0,
            momentary_ring_filled: 0,
            momentary_ring_sum: 0.0,
            momentary_window_size,
            momentary_max: 0.0,

            short_term_ring: vec![0.0; short_term_max_size],
            short_term_ring_pos: 0,
            short_term_ring_filled: 0,
            short_term_ring_sum: 0.0,
            short_term_window_size,
            short_term_max: 0.0,

            block_ring: vec![0.0; MAX_BLOCK_ENTRIES],
            block_ring_pos: 0,
            block_ring_count: 0,
            block_sample_count: 0,
            samples_per_block,
            samples_per_hop,
            hop_counter: 0,

            st_block_energies: vec![0.0; MAX_ST_ENTRIES],
            st_block_count: 0,
            st_hop_counter: 0,
            samples_per_st_block,
            samples_per_st_hop,

            cached_integrated: f64::NEG_INFINITY,
            cached_integrated_block_count: 0,

            cached_lra: f64::NEG_INFINITY,
            cached_lra_block_count: 0,

            lra_scratch: vec![0.0; MAX_ST_ENTRIES],

            sample_rate,
        }
    }

    /// Reset all accumulated state (filters, windows, gating blocks).
    pub fn reset(&mut self) {
        self.filter_l.reset();
        self.filter_r.reset();

        // Momentary
        self.momentary_ring[..self.momentary_window_size].fill(0.0);
        self.momentary_ring_pos = 0;
        self.momentary_ring_filled = 0;
        self.momentary_ring_sum = 0.0;
        self.momentary_max = 0.0;

        // Short-term
        self.short_term_ring[..self.short_term_window_size].fill(0.0);
        self.short_term_ring_pos = 0;
        self.short_term_ring_filled = 0;
        self.short_term_ring_sum = 0.0;
        self.short_term_max = 0.0;

        // Integrated
        self.block_ring[..self.block_ring_count.min(MAX_BLOCK_ENTRIES)].fill(0.0);
        self.block_ring_pos = 0;
        self.block_ring_count = 0;
        self.block_sample_count = 0;
        self.hop_counter = 0;

        // LRA
        self.st_block_energies[..self.st_block_count.min(MAX_ST_ENTRIES)].fill(0.0);
        self.st_block_count = 0;
        self.st_hop_counter = 0;
        self.cached_integrated = f64::NEG_INFINITY;
        self.cached_integrated_block_count = 0;
        self.cached_lra = f64::NEG_INFINITY;
        self.cached_lra_block_count = 0;
    }

    /// Reconfigure for a new sample rate. Resets all state.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.filter_l.set_sample_rate(sample_rate);
        self.filter_r.set_sample_rate(sample_rate);

        self.momentary_window_size = (sample_rate * 0.4) as usize;
        self.short_term_window_size = (sample_rate * 3.0) as usize;
        self.samples_per_block = (sample_rate * 0.4) as usize;
        self.samples_per_hop = (sample_rate * 0.1) as usize;
        self.samples_per_st_block = (sample_rate * 3.0) as usize;
        self.samples_per_st_hop = (sample_rate * 1.0) as usize;

        self.reset();
    }

    /// Process one stereo sample pair. Call for every sample.
    #[inline]
    pub fn process_sample(&mut self, left: f32, right: f32) {
        // K-weight both channels
        let kl = self.filter_l.process_sample(left);
        let kr = self.filter_r.process_sample(right);

        // Sum of K-weighted squared samples (both channels)
        let sq = kl * kl + kr * kr;

        // ── Momentary (400ms) sliding window ──
        if self.momentary_ring_filled == self.momentary_window_size {
            self.momentary_ring_sum -= self.momentary_ring[self.momentary_ring_pos];
        }
        self.momentary_ring[self.momentary_ring_pos] = sq;
        self.momentary_ring_sum += sq;
        self.momentary_ring_pos += 1;
        if self.momentary_ring_pos >= self.momentary_window_size {
            self.momentary_ring_pos = 0;
        }
        if self.momentary_ring_filled < self.momentary_window_size {
            self.momentary_ring_filled += 1;
        }

        // ── Short-term (3000ms) sliding window ──
        if self.short_term_ring_filled == self.short_term_window_size {
            self.short_term_ring_sum -= self.short_term_ring[self.short_term_ring_pos];
        }
        self.short_term_ring[self.short_term_ring_pos] = sq;
        self.short_term_ring_sum += sq;
        self.short_term_ring_pos += 1;
        if self.short_term_ring_pos >= self.short_term_window_size {
            self.short_term_ring_pos = 0;
        }
        if self.short_term_ring_filled < self.short_term_window_size {
            self.short_term_ring_filled += 1;
        }

        // ── Integrated loudness: accumulate block energy ──
        self.block_sample_count += 1;
        self.hop_counter += 1;

        // Every 100ms (hop), if we have a full 400ms block, emit a block energy
        if self.hop_counter >= self.samples_per_hop {
            self.hop_counter = 0;

            if self.block_sample_count >= self.samples_per_block {
                // Mean-square energy over the 400ms block.
                // Use the momentary ring's running sum for the 400ms block energy.
                let block_energy = if self.momentary_ring_filled >= self.momentary_window_size {
                    self.momentary_ring_sum.max(0.0) / self.momentary_window_size as f64
                } else {
                    0.0
                };

                self.block_ring[self.block_ring_pos] = block_energy;
                self.block_ring_pos = (self.block_ring_pos + 1) % MAX_BLOCK_ENTRIES;
                if self.block_ring_count < MAX_BLOCK_ENTRIES {
                    self.block_ring_count += 1;
                }
            }
        }

        // ── LRA: short-term block energies ──
        self.st_hop_counter += 1;
        if self.st_hop_counter >= self.samples_per_st_hop {
            self.st_hop_counter = 0;

            if self.short_term_ring_filled >= self.short_term_window_size {
                let st_energy =
                    self.short_term_ring_sum.max(0.0) / self.short_term_window_size as f64;

                let st_pos = if self.st_block_count < MAX_ST_ENTRIES {
                    self.st_block_count
                } else {
                    self.st_block_count % MAX_ST_ENTRIES
                };
                self.st_block_energies[st_pos] = st_energy;
                self.st_block_count += 1;
            }
        }
    }

    /// Current momentary loudness in LUFS (400ms window).
    /// Returns -inf until a full 400ms window has been accumulated.
    pub fn momentary_lufs(&self) -> f64 {
        if self.momentary_ring_filled < self.momentary_window_size {
            return f64::NEG_INFINITY;
        }
        let mean_sq = self.momentary_ring_sum.max(0.0) / self.momentary_window_size as f64;
        energy_to_loudness(mean_sq)
    }

    /// Highest momentary loudness (energy) since last reset, in LUFS.
    pub fn momentary_max_lufs(&self) -> f64 {
        energy_to_loudness(self.momentary_max)
    }

    /// Current short-term loudness in LUFS (3000ms window).
    /// Returns -inf until a full 3000ms window has been accumulated.
    pub fn short_term_lufs(&self) -> f64 {
        if self.short_term_ring_filled < self.short_term_window_size {
            return f64::NEG_INFINITY;
        }
        let mean_sq = self.short_term_ring_sum.max(0.0) / self.short_term_window_size as f64;
        energy_to_loudness(mean_sq)
    }

    /// Highest short-term loudness (energy) since last reset, in LUFS.
    pub fn short_term_max_lufs(&self) -> f64 {
        energy_to_loudness(self.short_term_max)
    }

    /// Integrated loudness with EBU R128 two-stage gating, in LUFS.
    ///
    /// Cached: only recomputes when new blocks have been added.
    /// Step 1: Absolute gate at -70 LUFS.
    /// Step 2: Relative gate at -10 LU below the absolute-gated mean.
    pub fn integrated_lufs(&mut self) -> f64 {
        if self.block_ring_count == self.cached_integrated_block_count {
            return self.cached_integrated;
        }
        self.cached_integrated = self.compute_integrated_lufs();
        self.cached_integrated_block_count = self.block_ring_count;
        self.cached_integrated
    }

    fn compute_integrated_lufs(&self) -> f64 {
        if self.block_ring_count == 0 {
            return f64::NEG_INFINITY;
        }

        let abs_gate_energy = loudness_to_energy(ABSOLUTE_GATE_LUFS);
        let count = self.block_ring_count.min(MAX_BLOCK_ENTRIES);

        // Step 1: mean of blocks above absolute gate
        let mut abs_sum = 0.0_f64;
        let mut abs_count = 0_u64;
        for i in 0..count {
            let e = self.block_ring[i];
            if e > abs_gate_energy {
                abs_sum += e;
                abs_count += 1;
            }
        }

        if abs_count == 0 {
            return f64::NEG_INFINITY;
        }

        let abs_mean = abs_sum / abs_count as f64;

        // Step 2: relative gate = abs_mean - 10 LU = abs_mean * 10^(-10/10) = abs_mean * 0.1
        let rel_gate_energy = abs_mean * 0.1;

        let mut rel_sum = 0.0_f64;
        let mut rel_count = 0_u64;
        for i in 0..count {
            let e = self.block_ring[i];
            if e > abs_gate_energy && e > rel_gate_energy {
                rel_sum += e;
                rel_count += 1;
            }
        }

        if rel_count == 0 {
            return f64::NEG_INFINITY;
        }

        energy_to_loudness(rel_sum / rel_count as f64)
    }

    /// Loudness Range (LRA) in LU.
    ///
    /// Uses short-term (3s) block energies gated at -70 LUFS absolute
    /// and -20 LU relative. LRA = loudness(95th percentile) - loudness(10th percentile).
    pub fn loudness_range(&mut self) -> f64 {
        // Cache: only recompute when new blocks have been added
        if self.st_block_count == self.cached_lra_block_count {
            return self.cached_lra;
        }
        self.cached_lra = self.compute_loudness_range();
        self.cached_lra_block_count = self.st_block_count;
        self.cached_lra
    }

    fn compute_loudness_range(&mut self) -> f64 {
        if self.st_block_count < 2 {
            return 0.0;
        }

        let abs_gate_energy = loudness_to_energy(ABSOLUTE_GATE_LUFS);
        let count = self.st_block_count.min(MAX_ST_ENTRIES);

        // Step 1: mean of blocks above absolute gate
        let mut abs_sum = 0.0_f64;
        let mut abs_count = 0_u64;
        for i in 0..count {
            let e = self.st_block_energies[i];
            if e > abs_gate_energy {
                abs_sum += e;
                abs_count += 1;
            }
        }

        if abs_count == 0 {
            return 0.0;
        }

        let abs_mean = abs_sum / abs_count as f64;

        // Step 2: relative gate at -20 LU (not -10 LU like integrated)
        // rel_gate = abs_mean * 10^(-20/10) = abs_mean * 0.01
        let rel_gate_energy = abs_mean * 0.01;

        // Copy gated energies into pre-allocated scratch buffer and sort.
        // O(n log n) instead of the previous O(n^2) rank-counting approach.
        let mut gated_count = 0_usize;
        for i in 0..count {
            let e = self.st_block_energies[i];
            if e > abs_gate_energy && e > rel_gate_energy {
                self.lra_scratch[gated_count] = e;
                gated_count += 1;
            }
        }

        if gated_count < 2 {
            return 0.0;
        }

        // Sort the gated subset for percentile lookup
        self.lra_scratch[..gated_count].sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

        // Target ranks (0-based) for 10th and 95th percentiles
        let rank_10 = ((gated_count as f64 * 0.10).ceil() as usize).saturating_sub(1);
        let rank_95 = ((gated_count as f64 * 0.95).ceil() as usize).saturating_sub(1);

        let energy_10 = self.lra_scratch[rank_10];
        let energy_95 = self.lra_scratch[rank_95];

        let loudness_10 = energy_to_loudness(energy_10);
        let loudness_95 = energy_to_loudness(energy_95);

        (loudness_95 - loudness_10).max(0.0)
    }

    /// Update peak (max) momentary and short-term values.
    /// Call once per audio buffer.
    pub fn update_maxes(&mut self) {
        // Momentary max: track highest energy, not LUFS, to avoid log in hot path
        if self.momentary_ring_filled > 0 {
            let mean_sq = self.momentary_ring_sum.max(0.0) / self.momentary_ring_filled as f64;
            if mean_sq > self.momentary_max {
                self.momentary_max = mean_sq;
            }
        }

        // Short-term max
        if self.short_term_ring_filled > 0 {
            let mean_sq = self.short_term_ring_sum.max(0.0) / self.short_term_ring_filled as f64;
            if mean_sq > self.short_term_max {
                self.short_term_max = mean_sq;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f64 = 48000.0;

    /// Helper: approximate equality for f64 within a tolerance.
    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn test_k_weight_filter_dc_gain() {
        // The high-shelf boosts by ~+4 dB, but the high-pass at 38 Hz blocks DC.
        // For DC (0 Hz), the high-pass output should be 0.
        let mut filter = KWeightFilter::new(SAMPLE_RATE);
        // Feed a long DC signal and check output settles to 0
        let mut last_output = 0.0;
        for _ in 0..480000 {
            last_output = filter.process_sample(1.0);
        }
        // DC should be blocked by the high-pass stage
        assert!(
            last_output.abs() < 0.001,
            "DC output should be ~0 (high-pass blocks DC), got {}",
            last_output
        );
    }

    #[test]
    fn test_k_weight_filter_coefficients() {
        // Verify coefficients at 48kHz match known reference values from sdroege/ebur128
        let (b, a) = filter_coefficients(48000.0);

        // b[0] should be the product of the high-shelf and high-pass numerators
        // From known reference: b[0] ≈ 1.53512485958697
        assert!(
            approx_eq(b[0], 1.53512485958697, 1e-6),
            "b[0] = {}, expected ~1.535125",
            b[0]
        );

        // a[0] should be 1.0 (product of two normalized filters)
        assert!(
            approx_eq(a[0], 1.0, 1e-10),
            "a[0] = {}, expected 1.0",
            a[0]
        );

        // a[1] is the convolution of pa[1] and ra[1] (4th-order combined filter).
        // Actual value at 48kHz: ≈ -3.6807067480
        assert!(
            approx_eq(a[1], -3.68070674801639, 1e-6),
            "a[1] = {}, expected ~-3.680707",
            a[1]
        );
    }

    #[test]
    fn test_energy_to_loudness() {
        // energy 1.0 → -0.691 LUFS
        let lufs = energy_to_loudness(1.0);
        assert!(
            approx_eq(lufs, -0.691, 1e-3),
            "energy 1.0 → {} LUFS, expected -0.691",
            lufs
        );

        // energy 0.0 → -inf
        assert!(
            energy_to_loudness(0.0).is_infinite() && energy_to_loudness(0.0) < 0.0,
            "energy 0.0 should give -inf LUFS"
        );

        // Roundtrip: loudness_to_energy(energy_to_loudness(e)) == e
        let e = 0.5;
        let rt = loudness_to_energy(energy_to_loudness(e));
        assert!(
            approx_eq(rt, e, 1e-10),
            "roundtrip: {} != {}",
            rt,
            e
        );
    }

    #[test]
    fn test_momentary_loudness_sine() {
        // 1kHz sine at 0 dBFS. K-weighting at 1kHz is approximately 0 dB.
        // RMS of a sine = 1/sqrt(2), so mean_sq = 0.5 per channel.
        // Stereo sum energy = 0.5 + 0.5 = 1.0 (both channels same signal).
        // Wait — the spec says energy = mean_sq_L + mean_sq_R for stereo.
        // LUFS = 10*log10(1.0) - 0.691 = -0.691 LUFS.
        //
        // But with a unit sine: each channel has mean_sq = 0.5,
        // so stereo energy = 1.0, giving -0.691 LUFS.
        //
        // However, a "0 dBFS" sine is peak 1.0, which gives -3.01 dBFS RMS.
        // The expected LUFS for a 0 dBFS stereo sine at 1kHz is approximately
        // -3.01 LUFS for a single channel, but with stereo sum it's -0.691 LUFS.
        //
        // For a mono sine panned center (same signal both channels), the EBU
        // spec gives energy = mean_sq_L + mean_sq_R = 0.5 + 0.5 = 1.0.
        // LUFS = 10*log10(1.0) - 0.691 = -0.691 LUFS.
        //
        // Let's test with a single channel having signal, other silent:
        // energy = 0.5 + 0 = 0.5, LUFS = 10*log10(0.5) - 0.691 = -3.01 - 0.691 = -3.70 LUFS.
        //
        // Test: stereo 1kHz sine → ~-0.691 LUFS momentary (after window fills).

        let mut meter = LufsMeter::new(SAMPLE_RATE);
        let freq = 1000.0;
        let n_samples = (SAMPLE_RATE * 0.5) as usize; // 500ms to fill 400ms window

        for i in 0..n_samples {
            let t = i as f64 / SAMPLE_RATE;
            let sample = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            meter.process_sample(sample, sample);
        }

        let lufs = meter.momentary_lufs();
        // For stereo 1kHz sine at 0 dBFS, without K-weighting: mean_sq per channel = 0.5,
        // stereo energy = 1.0, LUFS = -0.691. But K-weighting at 1kHz boosts by ~+0.7 dB,
        // so the actual energy is ~1.17 and LUFS ≈ +0.0 LUFS. Accept within 1.0 dB of 0.
        assert!(
            lufs > -1.5 && lufs < 1.5,
            "stereo 1kHz sine momentary: {} LUFS, expected near 0 LUFS (K-weighted)",
            lufs
        );

        // Single-channel test: only left channel has signal, right is silent.
        // K-weighted energy ≈ 0.5 * 1.17 (K-weight boost) = 0.585.
        // LUFS = 10*log10(0.585) - 0.691 ≈ -2.33 - 0.691 ≈ -3.02 LUFS.
        let mut meter2 = LufsMeter::new(SAMPLE_RATE);
        for i in 0..n_samples {
            let t = i as f64 / SAMPLE_RATE;
            let sample = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            meter2.process_sample(sample, 0.0);
        }
        let lufs_mono = meter2.momentary_lufs();
        // With K-weight gain at 1kHz, mono LUFS ≈ -3.0 LUFS (vs -3.70 without K-weight)
        assert!(
            lufs_mono > -4.5 && lufs_mono < -2.0,
            "mono 1kHz sine momentary: {} LUFS, expected ~-3.0 LUFS",
            lufs_mono
        );
    }

    #[test]
    fn test_integrated_gating_silence() {
        // Pure silence should give -inf LUFS integrated.
        let mut meter = LufsMeter::new(SAMPLE_RATE);
        // Process enough for several blocks (2 seconds)
        let n = (SAMPLE_RATE * 2.0) as usize;
        for _ in 0..n {
            meter.process_sample(0.0, 0.0);
        }
        let integrated = meter.integrated_lufs();
        assert!(
            integrated.is_infinite() && integrated < 0.0,
            "silence should give -inf LUFS integrated, got {}",
            integrated
        );
    }

    #[test]
    fn test_momentary_window_size() {
        // At 48kHz, 400ms = 19200 samples
        let meter = LufsMeter::new(48000.0);
        assert_eq!(
            meter.momentary_window_size, 19200,
            "400ms at 48kHz should be 19200 samples, got {}",
            meter.momentary_window_size
        );
    }

    #[test]
    fn test_short_term_window_size() {
        // At 48kHz, 3000ms = 144000 samples
        let meter = LufsMeter::new(48000.0);
        assert_eq!(
            meter.short_term_window_size, 144000,
            "3000ms at 48kHz should be 144000 samples, got {}",
            meter.short_term_window_size
        );
    }

    #[test]
    fn test_reset_clears_everything() {
        let mut meter = LufsMeter::new(SAMPLE_RATE);
        // Process some signal
        for i in 0..48000 {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32;
            meter.process_sample(s, s);
        }
        meter.update_maxes();
        assert!(meter.momentary_lufs().is_finite());
        assert!(meter.momentary_max > 0.0);

        meter.reset();
        assert!(meter.momentary_lufs().is_infinite());
        assert_eq!(meter.momentary_max, 0.0);
        assert_eq!(meter.short_term_max, 0.0);
        assert_eq!(meter.block_ring_count, 0);
        assert_eq!(meter.st_block_count, 0);
    }

    #[test]
    fn test_set_sample_rate_updates_windows() {
        let mut meter = LufsMeter::new(48000.0);
        assert_eq!(meter.momentary_window_size, 19200);

        meter.set_sample_rate(96000.0);
        assert_eq!(
            meter.momentary_window_size, 38400,
            "400ms at 96kHz should be 38400 samples"
        );
        assert_eq!(
            meter.short_term_window_size, 288000,
            "3000ms at 96kHz should be 288000 samples"
        );
    }

    #[test]
    fn test_loudness_to_energy_roundtrip() {
        // Verify that loudness_to_energy is the inverse of energy_to_loudness
        for &lufs in &[-70.0, -23.0, -14.0, -3.0, 0.0] {
            let energy = loudness_to_energy(lufs);
            let rt_lufs = energy_to_loudness(energy);
            assert!(
                approx_eq(rt_lufs, lufs, 1e-10),
                "roundtrip failed for {} LUFS: got {}",
                lufs,
                rt_lufs
            );
        }
    }

    #[test]
    fn test_absolute_gate_threshold() {
        // The absolute gate at -70 LUFS should correspond to a specific energy
        let energy = loudness_to_energy(-70.0);
        // -70 LUFS → energy = 10^((-70+0.691)/10) = 10^(-6.9309) ≈ 1.17e-7
        assert!(
            approx_eq(energy, 1.17e-7, 1e-8),
            "absolute gate energy: {}, expected ~1.17e-7",
            energy
        );
    }

    #[test]
    fn test_integrated_loudness_constant_signal() {
        // A constant-level signal should have integrated loudness close to momentary.
        let mut meter = LufsMeter::new(SAMPLE_RATE);
        let freq = 1000.0;
        // Process 5 seconds to get multiple blocks
        let n = (SAMPLE_RATE * 5.0) as usize;
        for i in 0..n {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            meter.process_sample(s, s);
        }

        let momentary = meter.momentary_lufs();
        let integrated = meter.integrated_lufs();

        // For a constant signal, integrated should be very close to momentary
        assert!(
            momentary.is_finite() && integrated.is_finite(),
            "both readings should be finite: momentary={}, integrated={}",
            momentary,
            integrated
        );
        assert!(
            approx_eq(momentary, integrated, 0.5),
            "constant signal: momentary={} LUFS, integrated={} LUFS should be close",
            momentary,
            integrated
        );
    }

    #[test]
    fn test_short_term_needs_3_seconds() {
        // Short-term should be -inf until we have 3s of data
        let mut meter = LufsMeter::new(SAMPLE_RATE);

        // At 1 second, short-term window not yet full
        let n_1s = SAMPLE_RATE as usize;
        for i in 0..n_1s {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32;
            meter.process_sample(s, s);
        }
        // Per EBU spec, short-term should return -inf until the full 3s window is filled
        let st_1s = meter.short_term_lufs();
        assert!(st_1s.is_infinite(), "short-term at 1s should be -inf (window not full)");

        // At 4 seconds, should be fully valid
        let n_more = (SAMPLE_RATE * 3.0) as usize;
        for i in 0..n_more {
            let t = (n_1s + i) as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32;
            meter.process_sample(s, s);
        }
        let st_4s = meter.short_term_lufs();
        assert!(st_4s.is_finite(), "short-term at 4s should be finite");
    }

    #[test]
    fn test_momentary_max_tracks_peak() {
        let mut meter = LufsMeter::new(SAMPLE_RATE);

        // Loud signal for 1 second
        let n = SAMPLE_RATE as usize;
        for i in 0..n {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32;
            meter.process_sample(s, s);
        }
        meter.update_maxes();
        let max_after_loud = meter.momentary_max;
        assert!(max_after_loud > 0.0);

        // Quiet signal for 1 second
        for i in 0..n {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() as f32 * 0.01;
            meter.process_sample(s, s);
        }
        meter.update_maxes();
        // Max should not decrease
        assert!(
            meter.momentary_max >= max_after_loud,
            "momentary max should not decrease: {} < {}",
            meter.momentary_max,
            max_after_loud
        );
    }

    #[test]
    fn test_filter_at_1khz_near_unity() {
        // K-weighting at 1kHz should be close to 0 dB (unity gain).
        // Feed a 1kHz sine and check the output RMS vs input RMS.
        let mut filter = KWeightFilter::new(SAMPLE_RATE);
        let freq = 1000.0;
        let n = 48000_usize;
        let mut sum_sq_in = 0.0_f64;
        let mut sum_sq_out = 0.0_f64;

        // Let the filter settle for 1000 samples, then measure
        for i in 0..1000 {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            filter.process_sample(s);
        }

        for i in 1000..n {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            let out = filter.process_sample(s);
            sum_sq_in += (s as f64) * (s as f64);
            sum_sq_out += out * out;
        }

        let gain_db = 10.0 * (sum_sq_out / sum_sq_in).log10();
        // K-weighting at 1kHz has a small boost (~+0.7 dB) from the high-shelf.
        // This is correct per the EBU R128 spec.
        assert!(
            approx_eq(gain_db, 0.0, 1.0),
            "K-weight gain at 1kHz: {} dB, expected near 0 dB (within 1 dB)",
            gain_db
        );
    }

    #[test]
    fn test_filter_high_frequency_boost() {
        // K-weighting should boost high frequencies (~+4 dB at ~4kHz).
        let mut filter = KWeightFilter::new(SAMPLE_RATE);
        let freq = 4000.0;
        let n = 48000_usize;
        let mut sum_sq_in = 0.0_f64;
        let mut sum_sq_out = 0.0_f64;

        for i in 0..1000 {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            filter.process_sample(s);
        }

        for i in 1000..n {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            let out = filter.process_sample(s);
            sum_sq_in += (s as f64) * (s as f64);
            sum_sq_out += out * out;
        }

        let gain_db = 10.0 * (sum_sq_out / sum_sq_in).log10();
        // Should be boosted, roughly +2 to +4 dB at 4kHz
        assert!(
            gain_db > 1.5 && gain_db < 5.0,
            "K-weight gain at 4kHz: {} dB, expected ~+2 to +4 dB",
            gain_db
        );
    }

    #[test]
    fn test_lra_constant_signal() {
        // A constant-level signal should have LRA ≈ 0.
        let mut meter = LufsMeter::new(SAMPLE_RATE);
        let freq = 1000.0;
        // Need enough time for multiple short-term blocks: at least 5s + 1s hops
        let n = (SAMPLE_RATE * 10.0) as usize;
        for i in 0..n {
            let t = i as f64 / SAMPLE_RATE;
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            meter.process_sample(s, s);
        }

        let lra = meter.loudness_range();
        assert!(
            lra < 1.0,
            "constant signal LRA should be ~0 LU, got {} LU",
            lra
        );
    }

    #[test]
    fn test_hop_sizes() {
        let meter = LufsMeter::new(48000.0);
        // 100ms hop for integrated: 4800 samples
        assert_eq!(meter.samples_per_hop, 4800);
        // 400ms block for integrated: 19200 samples
        assert_eq!(meter.samples_per_block, 19200);
        // 1s hop for LRA: 48000 samples
        assert_eq!(meter.samples_per_st_hop, 48000);
        // 3s block for LRA: 144000 samples
        assert_eq!(meter.samples_per_st_block, 144000);
    }
}
