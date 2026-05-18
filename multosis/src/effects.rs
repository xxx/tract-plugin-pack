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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_bank_variants_distinct() {
        assert_ne!(EffectBank::Lowpass, EffectBank::Bitcrush);
    }
}
