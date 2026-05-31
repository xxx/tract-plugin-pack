//! Sinusoidal LFO phasor. Phase is tracked in *turns* `[0, 1)` so it is
//! sample-rate independent — a sample-rate change does not desync it. Used by
//! Hyper voices and Dimension taps.

use std::f32::consts::TAU;

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
        (TAU * self.phase).sin()
    }

    /// Sine value at the present phase plus `offset_turns` (no state change).
    #[inline]
    pub fn sine_at_offset(&self, offset_turns: f32) -> f32 {
        (TAU * (self.phase + offset_turns)).sin()
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
}
