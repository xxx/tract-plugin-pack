//! Core limiter DSP: gain computer, envelope filters, lookahead.

/// Compute gain reduction in dB for a given input level.
///
/// The threshold is 0 dBFS (the signal is pre-boosted by the threshold
/// parameter before reaching the gain computer). `knee_db` is the soft
/// knee width in dB (0 = hard knee).
///
/// Returns a value <= 0 (gain reduction). Based on Giannoulis et al.
/// (2012) with ratio = infinity (brickwall limiter).
pub fn gain_computer_db(input_db: f32, knee_db: f32) -> f32 {
    if knee_db < 0.01 {
        // Hard knee: no reduction below 0, full limiting above
        if input_db <= 0.0 { 0.0 } else { -input_db }
    } else {
        let half_knee = knee_db / 2.0;
        if input_db < -half_knee {
            // Below knee region
            0.0
        } else if input_db <= half_knee {
            // In knee region: quadratic transition
            -(input_db + half_knee).powi(2) / (2.0 * knee_db)
        } else {
            // Above knee region: full limiting
            -input_db
        }
    }
}

/// One-pole IIR envelope follower with separate attack and release coefficients.
///
/// Based on Giannoulis et al. (2012). Tracks gain reduction (values <= 0 dB).
pub struct EnvelopeFilter {
    state: f32,
    alpha_attack: f32,
    alpha_release: f32,
}

impl EnvelopeFilter {
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self {
        let mut env = Self { state: 0.0, alpha_attack: 0.0, alpha_release: 0.0 };
        env.set_params(sample_rate, attack_ms, release_ms);
        env
    }

    pub fn set_params(&mut self, sample_rate: f32, attack_ms: f32, release_ms: f32) {
        self.alpha_attack = Self::compute_alpha(sample_rate, attack_ms);
        self.alpha_release = Self::compute_alpha(sample_rate, release_ms);
    }

    fn compute_alpha(sample_rate: f32, time_ms: f32) -> f32 {
        if time_ms <= 0.0 || sample_rate <= 0.0 {
            return 0.0;
        }
        let time_seconds = time_ms / 1000.0;
        (-1.0_f32 / (sample_rate * time_seconds)).exp()
    }

    /// Process one sample of gain reduction. Returns smoothed GR in dB (<= 0).
    #[inline]
    pub fn process(&mut self, gr_db: f32) -> f32 {
        if gr_db <= self.state {
            // Attack: gain reduction is increasing (going more negative)
            self.state = self.alpha_attack * self.state + (1.0 - self.alpha_attack) * gr_db;
        } else {
            // Release: gain reduction is decreasing (recovering toward 0)
            self.state = self.alpha_release * self.state + (1.0 - self.alpha_release) * gr_db;
        }
        self.state
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Two-stage envelope filter mixing transient (fast release) and dynamics (user release).
///
/// `transient_mix` 0.0 = dynamics only, 1.0 = transient only.
/// The brickwall guarantee ensures the output never exceeds (is less negative than) either stage.
pub struct DualStageEnvelope {
    transient: EnvelopeFilter,
    dynamics: EnvelopeFilter,
}

impl DualStageEnvelope {
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self {
        Self {
            // Transient: 0.1ms attack, release = attack_ms (very fast)
            transient: EnvelopeFilter::new(sample_rate, 0.1, attack_ms),
            // Dynamics: 0.1ms attack, user release
            dynamics: EnvelopeFilter::new(sample_rate, 0.1, release_ms),
        }
    }

    pub fn set_params(&mut self, sample_rate: f32, attack_ms: f32, release_ms: f32) {
        self.transient.set_params(sample_rate, 0.1, attack_ms);
        self.dynamics.set_params(sample_rate, 0.1, release_ms);
    }

    /// Process one sample. transient_mix: 0.0 = dynamics only, 1.0 = transient only.
    #[inline]
    pub fn process(&mut self, gr_db: f32, transient_mix: f32) -> f32 {
        let tr = self.transient.process(gr_db);
        let dy = self.dynamics.process(gr_db);
        // Mix but never allow output to be less negative than either stage
        // (brickwall guarantee): clamp to each stage weighted by mix
        let mixed = transient_mix * tr + (1.0 - transient_mix) * dy;
        let clamped = if transient_mix < 1.0 { mixed.min(dy) } else { mixed };
        if transient_mix > 0.0 { clamped.min(tr) } else { clamped }
    }

    pub fn reset(&mut self) {
        self.transient.reset();
        self.dynamics.reset();
    }
}

/// Apply lookahead by iterating backwards through the gain reduction buffer.
///
/// For each sample, if the gain reduction is deeper than a linear (in dB)
/// ramp from the current lookahead window, the ramp is extended backward.
/// Deeper peaks override shallower ramps. This ensures the gain reduction
/// is fully applied by the time the peak arrives in the delayed audio.
///
/// `gr` is modified in place. `lookahead_samples` is the number of samples
/// in the lookahead window (must be >= 1).
pub fn apply_lookahead_backward_pass(gr: &mut [f32], lookahead_samples: usize) {
    if gr.is_empty() || lookahead_samples == 0 {
        return;
    }

    let len = gr.len();
    let la = lookahead_samples;
    // Ramp spans la+1 samples: the peak itself plus la samples before it.
    // t = ramp_remaining / (la + 1) gives a linear ramp from ramp_target (at the
    // peak, t = 1) down to ramp_target/(la+1) (la samples before, t = 1/(la+1)).
    let ramp_len = (la + 1) as f32;

    // Track the "ramp target" — the deepest gain reduction we need to ramp toward.
    // Start from the end and work backward.
    let mut ramp_target = 0.0_f32;  // deepest GR we're ramping toward
    let mut ramp_remaining = 0_usize;  // samples remaining in the current ramp

    for i in (0..len).rev() {
        if gr[i] < ramp_target || ramp_remaining == 0 {
            // New deeper peak (or no active ramp) — start a new ramp
            ramp_target = gr[i];
            ramp_remaining = la + 1;
        }

        if ramp_remaining > 0 {
            // Linear ramp in dB: t=1 at the peak, decreasing toward 0 over la+1 steps
            let t = ramp_remaining as f32 / ramp_len;
            let ramped = ramp_target * t;
            // Take the deeper (more negative) of the current value and the ramp
            gr[i] = gr[i].min(ramped);
            ramp_remaining -= 1;
        }
    }
}

// ── Limiter struct ────────────────────────────────────────────────────────────

/// Top-level limiter DSP: owns delay lines, gain reduction buffer, and envelope.
///
/// Signal flow in `process_block`:
/// 1. Detect stereo peak (max of |L|, |R|)
/// 2. Compute gain reduction via `gain_computer_db`
/// 3. Apply lookahead backward pass (on raw gain computer output)
/// 4. Smooth via dual-stage envelope (on lookahead-shaped buffer)
/// 5. Apply gain reduction to delayed audio
/// 6. Safety clip at ceiling
pub struct Limiter {
    // Stereo delay line (ring buffer)
    delay_line_l: Vec<f32>,
    delay_line_r: Vec<f32>,
    delay_pos: usize,
    delay_len: usize,

    // Per-sample gain reduction buffer (reused each block)
    gr_buffer: Vec<f32>,

    // Dual-stage envelope
    envelope: DualStageEnvelope,

    // Tracking
    sample_rate: f32,
    max_lookahead_ms: f32,
    max_lookahead_samples: usize,
}

impl Limiter {
    /// Create a new limiter with pre-allocated buffers for the maximum lookahead.
    ///
    /// `max_lookahead_ms` determines the maximum delay line length and GR buffer size.
    pub fn new(sample_rate: f32, max_lookahead_ms: f32) -> Self {
        let max_lookahead_samples =
            (max_lookahead_ms / 1000.0 * sample_rate).ceil() as usize;
        // Default delay length is the max; set_params can reduce it.
        let delay_len = max_lookahead_samples;
        Self {
            delay_line_l: vec![0.0; max_lookahead_samples],
            delay_line_r: vec![0.0; max_lookahead_samples],
            delay_pos: 0,
            delay_len,
            // Pre-allocate GR buffer large enough for typical block sizes.
            // process_block will not allocate — it only uses existing capacity.
            gr_buffer: vec![0.0; 8192],
            envelope: DualStageEnvelope::new(sample_rate, 5.0, 200.0),
            sample_rate,
            max_lookahead_ms,
            max_lookahead_samples,
        }
    }

    /// Reconfigure for a new sample rate. Reallocates delay lines if needed.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let new_max =
            (self.max_lookahead_ms / 1000.0 * sample_rate).ceil() as usize;
        self.max_lookahead_samples = new_max;
        self.delay_line_l.resize(new_max, 0.0);
        self.delay_line_r.resize(new_max, 0.0);
        self.delay_len = self.delay_len.min(new_max).max(1);
        self.delay_pos = 0;
        self.delay_line_l.fill(0.0);
        self.delay_line_r.fill(0.0);
    }

    /// Update envelope parameters and recalculate lookahead length from attack time.
    ///
    /// The lookahead delay equals the attack time (clamped to max_lookahead_samples).
    pub fn set_params(&mut self, attack_ms: f32, release_ms: f32) {
        self.envelope.set_params(self.sample_rate, attack_ms, release_ms);
        let attack_samples = (attack_ms / 1000.0 * self.sample_rate).ceil() as usize;
        self.delay_len = attack_samples.min(self.max_lookahead_samples).max(1);
    }

    /// Pre-allocate `gr_buffer` to hold at least `max_samples` samples.
    ///
    /// Called from `initialize()` so that `process_block` never allocates.
    pub fn set_max_block_size(&mut self, max_samples: usize) {
        if self.gr_buffer.len() < max_samples {
            self.gr_buffer.resize(max_samples, 0.0);
        }
    }

    /// Zero all buffers and reset the envelope.
    pub fn reset(&mut self) {
        self.delay_line_l.fill(0.0);
        self.delay_line_r.fill(0.0);
        self.delay_pos = 0;
        self.envelope.reset();
    }

    /// Returns the current lookahead delay length in samples (= reported latency).
    pub fn latency_samples(&self) -> usize {
        self.delay_len
    }

    /// Process a stereo block in-place. Returns the deepest gain reduction applied (in dB, <= 0).
    ///
    /// Signal flow:
    /// 1. For each sample, detect peak (max of |L|, |R| with stereo link)
    /// 2. Compute gain reduction via gain_computer_db
    /// 3. Apply lookahead backward pass (on raw gain computer output)
    /// 4. Smooth via dual-stage envelope (on lookahead-shaped buffer)
    /// 5. Apply gain to delayed audio
    /// 6. Safety clip at ceiling
    #[allow(clippy::too_many_arguments)]
    pub fn process_block(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        knee_db: f32,
        transient_mix: f32,
        _stereo_link: f32, // reserved for future per-channel limiting; v1 always uses max(|L|,|R|)
        ceiling_linear: f32,
        true_peak: Option<&mut [crate::true_peak::TruePeakDetector; 2]>,
    ) -> f32 {
        let num_samples = left.len().min(right.len());
        if num_samples == 0 {
            return 0.0;
        }

        debug_assert!(
            num_samples <= self.gr_buffer.len(),
            "process_block called with {} samples but gr_buffer only has {} — \
             call set_max_block_size() in initialize()",
            num_samples,
            self.gr_buffer.len(),
        );

        // Step 1 & 2: Peak detection + gain computer
        if let Some(tp) = true_peak {
            for i in 0..num_samples {
                let peak_l = tp[0].process_sample_peak(left[i]);
                let peak_r = tp[1].process_sample_peak(right[i]);
                let peak = peak_l.max(peak_r);
                let peak_db = 20.0 * peak.max(1e-10).log10();
                self.gr_buffer[i] = gain_computer_db(peak_db, knee_db);
            }
        } else {
            for i in 0..num_samples {
                let peak = left[i].abs().max(right[i].abs());
                let peak_db = 20.0 * peak.max(1e-10).log10();
                self.gr_buffer[i] = gain_computer_db(peak_db, knee_db);
            }
        }

        // Step 3: Lookahead backward pass (on raw gain computer output)
        apply_lookahead_backward_pass(&mut self.gr_buffer[..num_samples], self.delay_len);

        // Step 4: Envelope smoothing (on lookahead-shaped buffer)
        for i in 0..num_samples {
            self.gr_buffer[i] = self.envelope.process(self.gr_buffer[i], transient_mix);
        }

        // Step 5 & 6: Apply gain to delayed audio with safety clip
        let mut deepest_gr = 0.0_f32;
        let dl_len = self.delay_len;

        for i in 0..num_samples {
            // Read the sample that is dl_len samples old (about to be overwritten)
            let delayed_l = self.delay_line_l[self.delay_pos];
            let delayed_r = self.delay_line_r[self.delay_pos];

            // Then write new sample
            self.delay_line_l[self.delay_pos] = left[i];
            self.delay_line_r[self.delay_pos] = right[i];

            // Apply gain reduction (dB to linear)
            let gr_linear = 10.0_f32.powf(self.gr_buffer[i] / 20.0);
            let out_l = (delayed_l * gr_linear).clamp(-ceiling_linear, ceiling_linear);
            let out_r = (delayed_r * gr_linear).clamp(-ceiling_linear, ceiling_linear);

            left[i] = out_l;
            right[i] = out_r;

            if self.gr_buffer[i] < deepest_gr {
                deepest_gr = self.gr_buffer[i];
            }

            // Advance delay position
            self.delay_pos = (self.delay_pos + 1) % dl_len;
        }

        deepest_gr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hard knee (knee = 0)
    #[test]
    fn test_gc_hard_below_threshold() {
        assert!((gain_computer_db(-10.0, 0.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_gc_hard_at_threshold() {
        assert!((gain_computer_db(0.0, 0.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_gc_hard_above_threshold() {
        assert!((gain_computer_db(6.0, 0.0) - (-6.0)).abs() < 0.01);
    }

    #[test]
    fn test_gc_hard_way_above() {
        assert!((gain_computer_db(20.0, 0.0) - (-20.0)).abs() < 0.01);
    }

    // Soft knee
    #[test]
    fn test_gc_soft_below_knee() {
        // Well below knee region — no reduction
        assert!((gain_computer_db(-20.0, 6.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_gc_soft_at_knee_start() {
        // At -W/2 = -3 dB — should be 0 reduction (knee starts here)
        assert!((gain_computer_db(-3.0, 6.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_gc_soft_in_knee() {
        // At threshold (0 dB) with 6 dB knee — partial reduction
        let gr = gain_computer_db(0.0, 6.0);
        assert!(gr < 0.0);         // some reduction
        assert!(gr > -3.0);        // but not full
    }

    #[test]
    fn test_gc_soft_at_knee_end() {
        // At +W/2 = +3 dB — should be full limiting (-3 dB)
        let gr = gain_computer_db(3.0, 6.0);
        assert!((gr - (-3.0)).abs() < 0.01);
    }

    #[test]
    fn test_gc_soft_above_knee() {
        // Well above knee — full limiting
        let gr = gain_computer_db(10.0, 6.0);
        assert!((gr - (-10.0)).abs() < 0.01);
    }

    #[test]
    fn test_gc_negative_infinity() {
        // Very quiet signal — no reduction
        assert!((gain_computer_db(-100.0, 0.0) - 0.0).abs() < 0.01);
        assert!((gain_computer_db(-100.0, 6.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_gc_soft_knee_is_monotonic() {
        // Gain reduction should increase monotonically with input level
        let mut prev_gr = 0.0_f32;
        for i in -100..200 {
            let db = i as f32 / 10.0;
            let gr = gain_computer_db(db, 6.0);
            assert!(gr <= prev_gr + 0.001, "non-monotonic at {db} dB: {gr} > {prev_gr}");
            prev_gr = gr;
        }
    }

    #[test]
    fn test_envelope_attack_fast() {
        let mut env = EnvelopeFilter::new(48000.0, 0.1, 200.0);
        // Feed -10 dB for 1ms (48 samples at 48kHz)
        let mut val = 0.0;
        for _ in 0..48 {
            val = env.process(-10.0);
        }
        // With 0.1ms attack at 48kHz, should be very close to -10 after 1ms
        assert!(val < -8.0, "expected < -8, got {val}");
    }

    #[test]
    fn test_envelope_release_slow() {
        let mut env = EnvelopeFilter::new(48000.0, 0.1, 200.0);
        // Drive to -10 dB
        for _ in 0..4800 {
            env.process(-10.0);
        }
        // Release (input 0)
        let mut val = 0.0;
        for _ in 0..9600 { // 200ms
            val = env.process(0.0);
        }
        // After one time constant, should have recovered ~63% (from -10 to ~-3.7)
        assert!(val > -5.0, "expected > -5, got {val}");
        assert!(val < 0.0, "should still be negative, got {val}");
    }

    #[test]
    fn test_envelope_no_positive_output() {
        let mut env = EnvelopeFilter::new(48000.0, 0.1, 200.0);
        for _ in 0..4800 {
            let val = env.process(-5.0);
            assert!(val <= 0.0);
        }
        for _ in 0..48000 {
            let val = env.process(0.0);
            assert!(val <= 0.001); // allow tiny float error
        }
    }

    #[test]
    fn test_dual_stage_transient_only() {
        let mut dual = DualStageEnvelope::new(48000.0, 5.0, 200.0);
        // Drive with -10 dB
        for _ in 0..480 {
            dual.process(-10.0, 1.0); // 100% transient
        }
        // Release — transient stage should release fast (5ms = attack time)
        let mut val = 0.0;
        for _ in 0..480 { // 10ms — 2x the transient release time
            val = dual.process(0.0, 1.0);
        }
        // Should have mostly recovered
        assert!(val > -3.0, "expected > -3, got {val}");
    }

    #[test]
    fn test_dual_stage_dynamics_only() {
        let mut dual = DualStageEnvelope::new(48000.0, 5.0, 200.0);
        for _ in 0..480 {
            dual.process(-10.0, 0.0); // 100% dynamics
        }
        // After 10ms of release, dynamics (200ms release) should still be deep
        let mut val = 0.0;
        for _ in 0..480 {
            val = dual.process(0.0, 0.0);
        }
        assert!(val < -7.0, "expected < -7, got {val}");
    }

    #[test]
    fn test_dual_stage_never_exceeds_either() {
        let mut dual = DualStageEnvelope::new(48000.0, 5.0, 200.0);
        let mut tr_env = EnvelopeFilter::new(48000.0, 0.1, 5.0);
        let mut dy_env = EnvelopeFilter::new(48000.0, 0.1, 200.0);
        // Feed varying signal
        for i in 0..4800 {
            let gr = if i % 100 < 50 { -10.0 } else { 0.0 };
            let dual_val = dual.process(gr, 0.5);
            let tr_val = tr_env.process(gr);
            let dy_val = dy_env.process(gr);
            assert!(dual_val <= tr_val + 0.01, "exceeded transient at {i}");
            assert!(dual_val <= dy_val + 0.01, "exceeded dynamics at {i}");
        }
    }

    #[test]
    fn test_lookahead_no_change_when_no_peaks() {
        let mut gr = vec![0.0_f32; 100];
        apply_lookahead_backward_pass(&mut gr, 10);
        for &v in &gr {
            assert_eq!(v, 0.0);
        }
    }

    #[test]
    fn test_lookahead_ramps_before_peak() {
        let lookahead = 10;
        let mut gr = vec![0.0_f32; 100];
        gr[50] = -10.0;  // peak at sample 50
        apply_lookahead_backward_pass(&mut gr, lookahead);
        // Gain reduction should start ramping before sample 50
        assert!(gr[40] < 0.0, "ramp should start at sample 40, got {}", gr[40]);
        assert!(gr[41] < gr[40], "ramp should deepen: {} vs {}", gr[41], gr[40]);
        // Before the ramp start, should be 0
        assert_eq!(gr[39], 0.0);
    }

    #[test]
    fn test_lookahead_peak_value_preserved() {
        let lookahead = 10;
        let mut gr = vec![0.0_f32; 100];
        gr[50] = -10.0;
        apply_lookahead_backward_pass(&mut gr, lookahead);
        // The peak itself should still be -10
        assert!((gr[50] - (-10.0)).abs() < 0.01);
    }

    #[test]
    fn test_lookahead_deeper_peak_overrides() {
        let lookahead = 20;
        let mut gr = vec![0.0_f32; 100];
        gr[50] = -5.0;   // shallow peak
        gr[60] = -10.0;  // deeper peak later
        apply_lookahead_backward_pass(&mut gr, lookahead);
        // The deeper peak's ramp should override the shallower one
        // At sample 50, the -10 ramp from sample 60 should be active
        assert!(gr[50] < -5.0, "deeper ramp should override: got {}", gr[50]);
    }

    #[test]
    fn test_lookahead_ramp_is_linear_in_db() {
        let lookahead = 10;
        let mut gr = vec![0.0_f32; 100];
        gr[50] = -10.0;
        apply_lookahead_backward_pass(&mut gr, lookahead);
        // Check linearity: each sample in the ramp should change by ~1 dB
        for i in 41..50 {
            let diff = gr[i + 1] - gr[i];
            // Should be approximately -1 dB per sample (total -10 over 10 samples)
            assert!((diff - (-1.0)).abs() < 0.2, "non-linear at {i}: diff={diff}");
        }
    }

    #[test]
    fn test_lookahead_empty_buffer() {
        let mut gr: Vec<f32> = vec![];
        apply_lookahead_backward_pass(&mut gr, 10);  // should not panic
    }

    #[test]
    fn test_lookahead_peak_at_start() {
        let lookahead = 10;
        let mut gr = vec![0.0_f32; 20];
        gr[0] = -10.0;  // peak at very start
        apply_lookahead_backward_pass(&mut gr, lookahead);
        // Peak at start — no room to ramp, but should not panic
        assert!((gr[0] - (-10.0)).abs() < 0.01);
    }

    #[test]
    fn test_lookahead_peak_at_end() {
        let lookahead = 10;
        let mut gr = vec![0.0_f32; 20];
        gr[19] = -10.0;  // peak at very end
        apply_lookahead_backward_pass(&mut gr, lookahead);
        // Ramp should extend backward from sample 19
        assert!(gr[9] < 0.0, "ramp should reach sample 9");
    }

    // ── Limiter integration tests ─────────────────────────────────────────

    #[test]
    fn test_limiter_output_below_ceiling() {
        let mut limiter = Limiter::new(48000.0, 10.0);
        limiter.set_params(5.0, 200.0);
        let mut left = vec![2.0_f32; 1024]; // +6 dBFS
        let mut right = vec![2.0_f32; 1024];
        let gr = limiter.process_block(&mut left, &mut right, 0.0, 0.5, 1.0, 1.0, None);
        // After lookahead settles, output should be at or below ceiling
        let la = (48000.0_f32 * 0.005) as usize + 10; // attack + margin
        for &s in &left[la..] {
            assert!(s.abs() <= 1.01, "output {s} exceeds ceiling");
        }
        assert!(gr < 0.0, "should have gain reduction");
    }

    #[test]
    fn test_limiter_quiet_signal_no_reduction() {
        let mut limiter = Limiter::new(48000.0, 10.0);
        limiter.set_params(5.0, 200.0);
        // Signal at -20 dBFS (well below 0 dBFS threshold)
        let level = 0.1_f32;
        let mut left = vec![level; 512];
        let mut right = vec![level; 512];
        let gr = limiter.process_block(&mut left, &mut right, 0.0, 0.5, 1.0, 1.0, None);
        assert!(
            (gr - 0.0).abs() < 0.01,
            "quiet signal should have no gain reduction, got {gr}"
        );
    }

    #[test]
    fn test_limiter_reset_clears_state() {
        let mut limiter = Limiter::new(48000.0, 10.0);
        limiter.set_params(5.0, 200.0);
        // Process a loud signal
        let mut left = vec![2.0_f32; 256];
        let mut right = vec![2.0_f32; 256];
        limiter.process_block(&mut left, &mut right, 0.0, 0.5, 1.0, 1.0, None);
        limiter.reset();
        // After reset, process silence — should get no gain reduction
        let mut left = vec![0.0_f32; 256];
        let mut right = vec![0.0_f32; 256];
        let gr = limiter.process_block(&mut left, &mut right, 0.0, 0.5, 1.0, 1.0, None);
        assert!(
            (gr - 0.0).abs() < 0.01,
            "after reset, silence should have no GR, got {gr}"
        );
    }

    #[test]
    fn test_limiter_latency_matches_attack() {
        let mut limiter = Limiter::new(48000.0, 10.0);
        limiter.set_params(5.0, 200.0);
        let expected = (5.0_f32 / 1000.0 * 48000.0).ceil() as usize;
        assert_eq!(limiter.latency_samples(), expected);
    }

    #[test]
    fn test_limiter_ceiling_below_zero() {
        let mut limiter = Limiter::new(48000.0, 10.0);
        limiter.set_params(5.0, 200.0);
        let ceiling_linear = 10.0_f32.powf(-3.0 / 20.0); // -3 dBFS
        let mut left = vec![2.0_f32; 1024];
        let mut right = vec![2.0_f32; 1024];
        limiter.process_block(
            &mut left,
            &mut right,
            0.0,
            0.5,
            1.0,
            ceiling_linear,
            None,
        );
        let la = (48000.0_f32 * 0.005) as usize + 10;
        for &s in &left[la..] {
            assert!(
                s.abs() <= ceiling_linear + 0.01,
                "output {s} exceeds ceiling {ceiling_linear}"
            );
        }
    }
}
