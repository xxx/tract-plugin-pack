//! The two throwaway effects for Milestone 1b — a per-row lowpass and a
//! per-row bitcrush. Hardwired, with no shared abstraction; the standardised
//! effect trait is Phase 2. Each effect's character is mapped from the row
//! index so the wavefront's vertical motion is immediately audible.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.1.

use nih_plug::prelude::Enum;

/// A modulatable parameter of an effect: its name and value range. Static per
/// effect kind; used by the 2b modulation engine and the 2c effect editor.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

/// The standardized audio-effect contract. Implemented by each effect struct;
/// dispatched allocation-free through `EffectInstance` (no `dyn`). Audio-thread
/// methods (`process_sample`, `set_param`, `reset`) must not allocate.
pub trait Effect {
    /// Process one stereo sample, returning the wet `(left, right)`. DSP state
    /// persists across calls so the effect does not click on reactivation.
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32);

    /// Recompute sample-rate-dependent coefficients.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// Clear all DSP state.
    fn reset(&mut self);

    /// The effect's modulatable parameters, in `set_param` index order.
    fn parameters(&self) -> &'static [ParamSpec];

    /// Set parameter `index` to `value` (clamped to the spec's range). An
    /// out-of-range `index` is ignored.
    fn set_param(&mut self, index: usize, value: f32);
}

/// A resonant lowpass — a TPT state-variable filter, lowpass output.
pub struct LowpassEffect {
    cutoff: f32,
    resonance: f32,
    sample_rate: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl LowpassEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
        },
        ParamSpec {
            name: "Resonance",
            min: 0.0,
            max: 1.0,
            default: 0.1,
        },
    ];

    /// A `LowpassEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut lp = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            sample_rate: 48_000.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
        };
        lp.recompute();
        lp
    }

    /// Recompute the TPT-SVF coefficients from cutoff / resonance / SR.
    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let q = 0.5 + self.resonance.clamp(0.0, 1.0) * 9.5;
        let k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Process one sample for a single channel using the TPT-SVF integrator form.
    fn svf_step(&mut self, x: f32, ch: usize) -> f32 {
        let v3 = x - self.ic2[ch];
        let v1 = self.a1 * self.ic1[ch] + self.a2 * v3;
        let v2 = self.ic2[ch] + self.a2 * self.ic1[ch] + self.a3 * v3;
        self.ic1[ch] = 2.0 * v1 - self.ic1[ch];
        self.ic2[ch] = 2.0 * v2 - self.ic2[ch];
        v2
    }
}

impl Default for LowpassEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for LowpassEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.svf_step(left, 0), self.svf_step(right, 1))
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => return,
        }
        self.recompute();
    }
}

/// Bit-depth reduction plus sample-rate reduction (sample-and-hold).
pub struct BitcrushEffect {
    bit_depth: f32,
    rate_reduction: f32,
    held: [f32; 2],
    phase: [f32; 2],
}

impl BitcrushEffect {
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Bit Depth",
            min: 1.0,
            max: 16.0,
            default: 16.0,
        },
        ParamSpec {
            name: "Rate Reduction",
            min: 1.0,
            max: 50.0,
            default: 1.0,
        },
    ];

    /// A `BitcrushEffect` at its default (near-clean) parameters.
    pub fn new() -> Self {
        Self {
            bit_depth: Self::PARAMS[0].default,
            rate_reduction: Self::PARAMS[1].default,
            held: [0.0; 2],
            phase: [Self::PARAMS[1].default; 2],
        }
    }

    /// Quantize `x` to the current bit depth.
    fn quantize(&self, x: f32) -> f32 {
        let levels = 2.0_f32.powf(self.bit_depth);
        let step = 2.0 / levels;
        (x / step).round() * step
    }

    /// One channel's sample-and-hold + quantization step.
    fn crush_step(&mut self, x: f32, ch: usize) -> f32 {
        self.phase[ch] += 1.0;
        if self.phase[ch] >= self.rate_reduction {
            self.phase[ch] -= self.rate_reduction;
            self.held[ch] = self.quantize(x);
        }
        self.held[ch]
    }
}

impl Default for BitcrushEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for BitcrushEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.crush_step(left, 0), self.crush_step(right, 1))
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn reset(&mut self) {
        self.held = [0.0; 2];
        self.phase = [self.rate_reduction; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.bit_depth = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.rate_reduction = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => {}
        }
    }
}

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

/// Per-row bit-depth reduction. Throwaway effect: the bit depth is mapped from
/// the row index — row 0 is heavily crushed (~2 bits), row `ROWS - 1` is
/// nearly clean (~16 bits). Stateless — bit quantisation has no memory.
pub struct BitcrushBank {
    /// Quantisation step per row.
    step: [f32; ROWS],
}

impl BitcrushBank {
    /// A bank with per-row quantisation steps computed.
    pub fn new() -> Self {
        let mut step = [0.0; ROWS];
        for (r, s) in step.iter_mut().enumerate() {
            let t = r as f32 / (ROWS - 1) as f32;
            let bits = 2.0 + t * 14.0; // 2..16 bits
            *s = 2.0_f32.powf(1.0 - bits);
        }
        Self { step }
    }

    /// Process one sample for `row`. Stateless; both channels use this.
    pub fn process(&self, row: usize, x: f32) -> f32 {
        let s = self.step[row];
        (x / s).round() * s
    }
}

impl Default for BitcrushBank {
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

    #[test]
    fn bitcrush_dark_row_quantizes_coarsely() {
        // Row 0 (~2 bits) snaps a small value hard; row ROWS-1 (~16 bits)
        // barely moves it.
        let bc = BitcrushBank::new();
        let x = 0.1_f32;
        let crushed = bc.process(0, x);
        let clean = bc.process(crate::grid::ROWS - 1, x);
        assert!(
            (crushed - x).abs() > (clean - x).abs(),
            "dark row should distort more (crushed={crushed}, clean={clean})"
        );
    }

    #[test]
    fn bitcrush_is_bounded() {
        // Quantisation never blows the signal far past its input range.
        let bc = BitcrushBank::new();
        for r in 0..crate::grid::ROWS {
            for &x in &[-1.0_f32, -0.3, 0.0, 0.42, 1.0] {
                let y = bc.process(r, x);
                assert!(y.abs() <= 1.5, "row {r}, x {x} -> {y} out of range");
            }
        }
    }

    #[test]
    fn lowpass_effect_parameters_are_declared() {
        let lp = LowpassEffect::new();
        let specs = lp.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert!(specs[0].min < specs[0].max);
    }

    #[test]
    fn lowpass_effect_dark_cutoff_attenuates_highs() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 200.0);
        lp.set_param(1, 0.0);
        let mut peak = 0.0_f32;
        for i in 0..2048 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = lp.process_sample(x, x);
            if i > 256 {
                peak = peak.max(l.abs());
            }
        }
        assert!(peak < 0.5, "a 200 Hz lowpass should kill a fast alternation, got {peak}");
    }

    #[test]
    fn lowpass_effect_open_cutoff_passes_a_constant() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 18_000.0);
        lp.set_param(1, 0.0);
        let mut y = 0.0;
        for _ in 0..2048 {
            y = lp.process_sample(1.0, 1.0).0;
        }
        assert!(y > 0.9, "an open lowpass should pass a constant, got {y}");
    }

    #[test]
    fn lowpass_effect_reset_clears_state() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 300.0);
        for _ in 0..512 {
            lp.process_sample(1.0, 1.0);
        }
        lp.reset();
        let y = lp.process_sample(1.0, 1.0).0;
        assert!(y.abs() < 0.5, "reset should clear filter state, got {y}");
    }

    #[test]
    fn lowpass_effect_set_param_out_of_range_is_ignored() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(99, 1.0);
        let y = lp.process_sample(0.25, 0.25);
        assert!(y.0.is_finite());
    }

    #[test]
    fn bitcrush_effect_parameters_are_declared() {
        let bc = BitcrushEffect::new();
        let specs = bc.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Bit Depth");
        assert_eq!(specs[1].name, "Rate Reduction");
    }

    #[test]
    fn bitcrush_effect_low_bit_depth_quantizes_coarsely() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 2.0);
        bc.set_param(1, 1.0);
        let crushed = bc.process_sample(0.1, 0.1).0;
        bc.set_param(0, 16.0);
        let clean = bc.process_sample(0.1, 0.1).0;
        assert!(
            (crushed - 0.1).abs() > (clean - 0.1).abs(),
            "2-bit ({crushed}) should distort more than 16-bit ({clean})"
        );
    }

    #[test]
    fn bitcrush_effect_rate_reduction_holds_samples() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 16.0);
        bc.set_param(1, 4.0);
        let first = bc.process_sample(1.0, 1.0).0;
        let held = bc.process_sample(-1.0, -1.0).0;
        assert!((first - held).abs() < 1e-6, "rate reduction should hold the sample");
    }

    #[test]
    fn bitcrush_effect_output_is_bounded() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 3.0);
        for &x in &[-1.0_f32, -0.3, 0.0, 0.42, 1.0] {
            let (l, r) = bc.process_sample(x, x);
            assert!(l.abs() <= 1.5 && r.abs() <= 1.5, "x {x} -> ({l},{r}) out of range");
        }
    }

    #[test]
    fn bitcrush_effect_reset_clears_hold_state() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(1, 8.0);
        bc.process_sample(0.7, 0.7);
        bc.reset();
        let y = bc.process_sample(0.25, 0.25).0;
        assert!((y - 0.25).abs() < 0.1, "reset should re-sample, got {y}");
    }
}
