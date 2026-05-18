//! The two throwaway effects for Milestone 1b — a per-row lowpass and a
//! per-row bitcrush. Hardwired, with no shared abstraction; the standardised
//! effect trait is Phase 2. Each effect's character is mapped from the row
//! index so the wavefront's vertical motion is immediately audible.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.1.

use nih_plug::prelude::Enum;

/// Which throwaway effect every row uses. A host parameter.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum EffectBank {
    #[id = "lowpass"]
    #[name = "Lowpass"]
    Lowpass,
    #[id = "bitcrush"]
    #[name = "Bitcrush"]
    Bitcrush,
}

use crate::grid::ROWS;

/// A one-pole lowpass per row. Throwaway effect: the cutoff is mapped from the
/// row index — row 0 is darkest (~200 Hz), row `ROWS - 1` is open (~18 kHz) —
/// so the wavefront's vertical motion is audible. State is one running value
/// per (row, channel); it persists across steps so the filter does not click.
pub struct LowpassBank {
    /// Running output value per `[row][channel]` (2 channels).
    state: [[f32; 2]; ROWS],
    /// One-pole coefficient per row, set by `set_sample_rate`.
    coeff: [f32; ROWS],
}

impl LowpassBank {
    /// A bank with cleared state and zeroed coefficients. Call
    /// `set_sample_rate` before processing.
    pub fn new() -> Self {
        Self {
            state: [[0.0; 2]; ROWS],
            coeff: [0.0; ROWS],
        }
    }

    /// Recompute the per-row coefficients for `sample_rate` (Hz). Row 0 maps
    /// to ~200 Hz, row `ROWS - 1` to ~18 kHz, log-spaced in between.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        for r in 0..ROWS {
            let t = r as f32 / (ROWS - 1) as f32;
            let cutoff = 200.0 * (18_000.0_f32 / 200.0).powf(t);
            let alpha = 1.0 - (-2.0 * std::f32::consts::PI * cutoff / sr).exp();
            self.coeff[r] = alpha.clamp(0.0, 1.0);
        }
    }

    /// Clear all filter state.
    pub fn reset(&mut self) {
        self.state = [[0.0; 2]; ROWS];
    }

    /// Process one sample of `channel` (0 or 1) for `row`.
    pub fn process(&mut self, row: usize, channel: usize, x: f32) -> f32 {
        let a = self.coeff[row];
        let prev = self.state[row][channel];
        let y = prev + a * (x - prev);
        self.state[row][channel] = y;
        y
    }
}

impl Default for LowpassBank {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_bank_variants_distinct() {
        assert_ne!(EffectBank::Lowpass, EffectBank::Bitcrush);
    }

    #[test]
    fn lowpass_open_row_passes_a_constant() {
        // The brightest row (ROWS-1) has a very high cutoff; a constant input
        // settles to ~itself within a few samples.
        let mut lp = LowpassBank::new();
        lp.set_sample_rate(48_000.0);
        let row = crate::grid::ROWS - 1;
        let mut y = 0.0;
        for _ in 0..256 {
            y = lp.process(row, 0, 1.0);
        }
        assert!(y > 0.9, "open row should pass a constant, got {y}");
    }

    #[test]
    fn lowpass_dark_row_attenuates_alternating_signal() {
        // The darkest row (0) has a low cutoff; a fast ±1 alternation is
        // heavily attenuated relative to the open row.
        let mut lp = LowpassBank::new();
        lp.set_sample_rate(48_000.0);
        let mut dark_peak = 0.0_f32;
        let mut open_peak = 0.0_f32;
        for i in 0..512 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            dark_peak = dark_peak.max(lp.process(0, 0, x).abs());
            open_peak = open_peak.max(lp.process(crate::grid::ROWS - 1, 0, x).abs());
        }
        assert!(
            dark_peak < open_peak,
            "dark row ({dark_peak}) should attenuate more than open ({open_peak})"
        );
    }

    #[test]
    fn lowpass_reset_clears_state() {
        let mut lp = LowpassBank::new();
        lp.set_sample_rate(48_000.0);
        for _ in 0..100 {
            lp.process(0, 0, 1.0);
        }
        lp.reset();
        // After reset the first output is just the first filtered step from 0.
        let y = lp.process(0, 0, 1.0);
        assert!(y < 0.5, "state should be cleared, got {y}");
    }
}
