//! Sinusoidal LFO phasor. Phase is tracked in *turns* `[0, 1)` so it is
//! sample-rate independent — a sample-rate change does not desync it. Used by
//! Hyper voices and Dimension taps.
//!
//! The sine itself is a refined parabolic approximation ([`fast_sin_turns`])
//! rather than libm `sinf`: profiling showed libm's f32→f64 range-reducing
//! `sinf` (called up to 14× per sample across the Hyper voices) was a major
//! per-sample cost. The approximation is inlined, branch-light, and accurate
//! to ~1.1e-3 — far below audibility for a sub-audio-rate LFO.

/// Refined parabolic sine of `turns` (one turn = 2π rad), i.e. ≈ `sin(2π·turns)`.
/// Accepts any real input (wraps internally). Max abs error ≈ 1.1e-3 vs the true
/// sine; output stays within `[-1, 1]`.
#[inline]
pub fn fast_sin_turns(turns: f32) -> f32 {
    // Reduce to q ∈ [-0.5, 0.5): sin is periodic in 1 turn, so this is exact.
    let q = turns - (turns + 0.5).floor();
    // Base parabola: 8q − 16q|q| ≈ sin(2πq), exact at q = 0, ±0.25, ±0.5.
    let y = 8.0 * q - 16.0 * q * q.abs();
    // One Newton-style refinement (P = 0.225) tightens the fit to < 1e-3.
    0.225 * (y * y.abs() - y) + y
}

#[derive(Clone, Copy)]
pub struct Phasor {
    phase: f32,
}

impl Phasor {
    /// Create a phasor at `initial_turns` (wrapped into `[0, 1)`).
    pub fn new(initial_turns: f32) -> Self {
        let mut p = Self { phase: 0.0 };
        p.reset_to(initial_turns);
        p
    }

    /// Reset the phase to `turns` (wrapped into `[0, 1)`).
    pub fn reset_to(&mut self, turns: f32) {
        self.phase = turns - turns.floor();
    }

    /// Current phase in turns `[0, 1)`.
    #[inline]
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Sine value in `[-1, 1]` at the present phase.
    #[inline]
    pub fn sine(&self) -> f32 {
        fast_sin_turns(self.phase)
    }

    /// Sine value at the present phase plus `offset_turns` (no state change).
    #[inline]
    pub fn sine_at_offset(&self, offset_turns: f32) -> f32 {
        fast_sin_turns(self.phase + offset_turns)
    }

    /// Advance the phase by one sample at `rate_hz`. Negative rates are
    /// supported (phase wraps in both directions).
    #[inline]
    pub fn advance(&mut self, rate_hz: f32, sample_rate: f32) {
        self.phase += rate_hz / sample_rate.max(1.0);
        self.phase -= self.phase.floor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_wraps_into_unit_range() {
        let p = Phasor::new(1.25);
        assert!((p.phase() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn advance_increments_by_rate_over_sr() {
        let mut p = Phasor::new(0.0);
        p.advance(100.0, 1000.0); // +0.1 turns
        assert!((p.phase() - 0.1).abs() < 1e-6);
    }

    #[test]
    fn phase_wraps_at_one() {
        let mut p = Phasor::new(0.95);
        p.advance(100.0, 1000.0); // 0.95 + 0.1 = 1.05 -> 0.05
        assert!((p.phase() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn sine_in_range() {
        let mut p = Phasor::new(0.0);
        for _ in 0..1000 {
            let v = p.sine();
            assert!((-1.0..=1.0).contains(&v));
            p.advance(440.0, 48_000.0);
        }
    }

    #[test]
    fn reset_to_zero() {
        let mut p = Phasor::new(0.3);
        p.reset_to(0.0);
        assert_eq!(p.phase(), 0.0);
        assert!(p.sine().abs() < 1e-6);
    }

    #[test]
    fn fast_sin_accuracy_below_audibility() {
        let mut max = 0.0f32;
        for i in 0..=200_000 {
            let t = i as f32 / 200_000.0; // [0, 1]
            let approx = fast_sin_turns(t);
            let truth = (std::f64::consts::TAU * t as f64).sin() as f32;
            max = max.max((approx - truth).abs());
        }
        // Refined-parabola worst case in f32 is ~1.09e-3 (textbook 9.19e-4 plus
        // f32 rounding). Far below audibility for a sub-audio-rate LFO.
        assert!(
            max < 1.2e-3,
            "fast_sin_turns max abs err {max} exceeds 1.2e-3"
        );
    }

    #[test]
    fn fast_sin_known_points() {
        assert!(fast_sin_turns(0.0).abs() < 1e-6);
        assert!((fast_sin_turns(0.25) - 1.0).abs() < 1e-3);
        assert!(fast_sin_turns(0.5).abs() < 1e-3);
        assert!((fast_sin_turns(0.75) + 1.0).abs() < 1e-3);
    }

    #[test]
    fn fast_sin_is_periodic_for_arbitrary_input() {
        for &t in &[0.1f32, 0.37, 0.6, 0.95] {
            let base = fast_sin_turns(t);
            for k in [-3.0f32, 1.0, 5.0, 12.0] {
                assert!(
                    (base - fast_sin_turns(t + k)).abs() < 2e-3,
                    "periodicity broken at t={t} k={k}"
                );
            }
        }
    }

    #[test]
    fn fast_sin_stays_in_unit_range() {
        for i in 0..=10_000 {
            let t = i as f32 / 10_000.0;
            let v = fast_sin_turns(t);
            assert!((-1.0..=1.0).contains(&v), "out of range at t={t}: {v}");
        }
    }
}
