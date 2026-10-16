//! Per-effect modulation recall; host parameters store the base values.

use serde::{Deserialize, Deserializer, Serialize};
use tract_effects::effects::{EffectKind, parameter_specs};
use tract_effects::modulation::{TrackModulation, TriggerSource};

/// Bank order follows declaration order, independent of the picker layout.
/// EffectKind declarations and this ordering are append-only automation IDs.
pub fn bank_kinds() -> Vec<EffectKind> {
    let mut kinds = EffectKind::ALL.to_vec();
    kinds.sort_unstable_by_key(|kind| *kind as usize);
    kinds
}

pub fn bank_index(kind: EffectKind) -> usize {
    kind as usize
}

pub fn neutral_modulation() -> TrackModulation {
    let mut modulation = TrackModulation::default_for_row(0);
    modulation.depths = [0.0; 3];
    modulation
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModulationBank {
    entries: Vec<TrackModulation>,
}

impl Default for ModulationBank {
    fn default() -> Self {
        Self {
            entries: bank_kinds()
                .into_iter()
                .map(|kind| {
                    let mut m = neutral_modulation();
                    sanitize(kind, &mut m);
                    m
                })
                .collect(),
        }
    }
}

impl<'de> Deserialize<'de> for ModulationBank {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Saved {
            entries: Vec<TrackModulation>,
        }
        let saved = Saved::deserialize(deserializer)?;
        let mut bank = Self::default();
        for (destination, source) in bank.entries.iter_mut().zip(saved.entries) {
            *destination = source;
        }
        bank.sanitize();
        Ok(bank)
    }
}

impl ModulationBank {
    pub fn get(&self, kind: EffectKind) -> &TrackModulation {
        &self.entries[bank_index(kind)]
    }

    pub fn set(&mut self, kind: EffectKind, mut modulation: TrackModulation) {
        sanitize(kind, &mut modulation);
        self.entries[bank_index(kind)] = modulation;
    }

    fn sanitize(&mut self) {
        for kind in bank_kinds() {
            sanitize(kind, &mut self.entries[bank_index(kind)]);
        }
    }
}

fn sanitize(kind: EffectKind, modulation: &mut TrackModulation) {
    modulation.clamp_targets(parameter_specs(kind).len());
    if matches!(
        modulation.trigger,
        TriggerSource::CellLight | TriggerSource::CellStep
    ) {
        modulation.trigger = TriggerSource::Free;
    }
    for depth in &mut modulation.depths {
        *depth = if depth.is_finite() {
            depth.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditSnapshot {
    pub kind: EffectKind,
    pub modulation: TrackModulation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_effects_have_neutral_independent_defaults() {
        let mut bank = ModulationBank::default();
        let mut changed = bank.get(EffectKind::Svf).clone();
        changed.depths[0] = 0.75;
        bank.set(EffectKind::Svf, changed.clone());
        assert_eq!(bank.get(EffectKind::Svf), &changed);
        for kind in bank_kinds()
            .into_iter()
            .filter(|&kind| kind != EffectKind::Svf)
        {
            let modulation = bank.get(kind);
            assert_eq!(modulation.depths, [0.0; 3]);
            assert!(modulation.msegs[0].active().iter().all(|n| n.value == 1.0));
        }
    }

    #[test]
    fn saved_bank_round_trips_and_sanitizes_cell_triggers_and_targets() {
        let mut bank = ModulationBank::default();
        let mut modulation = bank.get(EffectKind::Svf).clone();
        modulation.trigger = TriggerSource::CellStep;
        modulation.targets = [Some(99); 3];
        bank.entries[bank_index(EffectKind::Svf)] = modulation;
        let restored: ModulationBank =
            serde_json::from_str(&serde_json::to_string(&bank).unwrap()).unwrap();
        assert_eq!(restored.get(EffectKind::Svf).trigger, TriggerSource::Free);
        assert_eq!(restored.get(EffectKind::Svf).targets, [None; 3]);
    }

    #[test]
    fn shorter_saved_banks_gain_default_entries() {
        let bank: ModulationBank = serde_json::from_str(r#"{"entries":[]}"#).unwrap();
        assert_eq!(bank, ModulationBank::default());
    }
}
