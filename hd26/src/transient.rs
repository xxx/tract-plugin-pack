//! Audio onset detector driving HD26's Hyper Retrig. Compares a fast envelope
//! to a slow envelope; a rising crossing of the (Sensitivity-derived) ratio
//! threshold fires, then a refractory hold + re-arm gate prevent immediate
//! re-fires. Ratio-based, so it is robust to absolute input level.

pub struct TransientDetector {
    fast_env: f32,
    slow_env: f32,
    fast_atk: f32,
    fast_rel: f32,
    slow_coeff: f32,
    threshold: f32,
    refractory: u32,
    cooldown: u32,
    armed: bool,
}

impl TransientDetector {
    const EPS: f32 = 1e-6;
    /// Below this fast-envelope level we never fire (noise floor gate).
    const LEVEL_FLOOR: f32 = 1e-4;

    pub fn new(sample_rate: f32) -> Self {
        let mut d = Self {
            fast_env: 0.0,
            slow_env: 0.0,
            fast_atk: 0.0,
            fast_rel: 0.0,
            slow_coeff: 0.0,
            threshold: 2.0,
            refractory: 0,
            cooldown: 0,
            armed: true,
        };
        d.set_sample_rate(sample_rate);
        d.set_sensitivity(0.5);
        d
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        // One-pole smoothing coefficient for time constant tau: exp(-1/(tau*sr)).
        let coeff = |tau_s: f32| (-1.0 / (tau_s * sr)).exp();
        self.fast_atk = coeff(0.002); // 2 ms attack
        self.fast_rel = coeff(0.030); // 30 ms release
        self.slow_coeff = coeff(0.100); // 100 ms
        self.refractory = (0.030 * sr) as u32; // 30 ms refractory hold
        self.reset();
    }

    pub fn reset(&mut self) {
        self.fast_env = 0.0;
        self.slow_env = 0.0;
        self.cooldown = 0;
        self.armed = true;
    }

    /// Map Sensitivity `[0, 1]` to the ratio threshold. Higher sensitivity →
    /// lower threshold (fires more readily). Spans 4.0 (insensitive) → 1.15.
    pub fn set_sensitivity(&mut self, sensitivity: f32) {
        let s = sensitivity.clamp(0.0, 1.0);
        self.threshold = 4.0 + s * (1.15 - 4.0);
    }

    /// Current fast-envelope value (for the GUI level bar).
    #[inline]
    pub fn fast_env(&self) -> f32 {
        self.fast_env
    }

    /// Feed one sample. Returns `true` when a transient fires.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> bool {
        let a = x.abs();
        let fc = if a > self.fast_env {
            self.fast_atk
        } else {
            self.fast_rel
        };
        self.fast_env = a + fc * (self.fast_env - a);
        self.slow_env = a + self.slow_coeff * (self.slow_env - a);

        if self.cooldown > 0 {
            self.cooldown -= 1;
        }

        let ratio = self.fast_env / (self.slow_env + Self::EPS);
        let mut fired = false;
        if self.armed
            && self.cooldown == 0
            && ratio >= self.threshold
            && self.fast_env > Self::LEVEL_FLOOR
        {
            fired = true;
            self.cooldown = self.refractory;
            self.armed = false;
        } else if ratio < self.threshold * 0.7 {
            // Re-arm once the ratio falls back, so sustained tones don't refire.
            self.armed = true;
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_fires(det: &mut TransientDetector, input: &[f32]) -> u32 {
        input.iter().filter(|&&x| det.process_sample(x)).count() as u32
    }

    #[test]
    fn silence_never_fires() {
        let mut d = TransientDetector::new(48_000.0);
        assert_eq!(count_fires(&mut d, &[0.0; 4800]), 0);
    }

    #[test]
    fn step_fires_exactly_once() {
        let mut d = TransientDetector::new(48_000.0);
        // Settle on silence, then a sustained loud level.
        count_fires(&mut d, &[0.0; 4800]);
        let step = vec![1.0f32; 9600]; // 200 ms — well past refractory
        assert_eq!(count_fires(&mut d, &step), 1);
    }

    #[test]
    fn level_robust_quiet_step_still_fires() {
        let mut d = TransientDetector::new(48_000.0);
        count_fires(&mut d, &[0.0; 4800]);
        let quiet = vec![0.05f32; 9600]; // far above 1e-4 floor, ratio identical
        assert_eq!(count_fires(&mut d, &quiet), 1);
    }

    #[test]
    fn sensitivity_threshold_is_monotonic() {
        let mut d = TransientDetector::new(48_000.0);
        d.set_sensitivity(0.0);
        let lo = d.threshold;
        d.set_sensitivity(1.0);
        let hi = d.threshold;
        assert!(
            hi < lo,
            "higher sensitivity must lower threshold: {hi} < {lo}"
        );
    }

    #[test]
    fn fast_env_tracks_input() {
        let mut d = TransientDetector::new(48_000.0);
        count_fires(&mut d, &[0.8; 2400]);
        assert!(d.fast_env() > 0.5, "fast_env should track to ~0.8");
    }
}
