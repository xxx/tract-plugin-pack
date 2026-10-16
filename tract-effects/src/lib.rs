//! Shared effects, modulation, and editing for the Tract plugin family.

pub mod compressor;
pub mod editor;
pub mod effects;
pub mod modulation;
pub mod randomize;
pub mod undo;

#[cfg(test)]
mod compatibility_tests {
    use super::effects::{EffectKind, TrackEffect, default_params_for_kind};
    use super::modulation::{TrackModulation, TriggerSource};

    fn saved_state() -> serde_json::Value {
        serde_json::from_str(include_str!("../test-data/multosis-state.json")).unwrap()
    }

    #[test]
    fn pre_refactor_effect_and_modulation_state_keeps_its_wire_format() {
        let state = saved_state();
        let effects: Vec<TrackEffect> = serde_json::from_value(state["effects"].clone()).unwrap();
        assert_eq!(effects.len(), EffectKind::ALL.len());
        for (effect, kind) in effects.iter().zip(EffectKind::ALL) {
            assert_eq!(effect.kind, kind);
            assert_eq!(effect.params, default_params_for_kind(kind));
            assert_eq!(effect.mix, 1.0);
            assert!(!effect.muted);
            assert!(!effect.soloed);
        }
        let effects_json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&effects).unwrap()).unwrap();
        assert_eq!(effects_json, state["effects"]);
        let modulation: TrackModulation =
            serde_json::from_value(state["modulation"].clone()).unwrap();
        assert_eq!(modulation, TrackModulation::default());
        let modulation_json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&modulation).unwrap()).unwrap();
        assert_eq!(modulation_json, state["modulation"]);
    }

    #[test]
    fn legacy_three_mseg_state_loads_with_a_spare_slot_and_default_trigger() {
        let mut legacy = saved_state()["modulation"].clone();
        legacy["msegs"].as_array_mut().unwrap().truncate(3);
        legacy["targets"].as_array_mut().unwrap().truncate(2);
        legacy["depths"].as_array_mut().unwrap().truncate(2);
        legacy.as_object_mut().unwrap().remove("trigger");
        let modulation: TrackModulation = serde_json::from_value(legacy).unwrap();
        assert_eq!(modulation.trigger, TriggerSource::Free);
        assert_eq!(modulation.targets[2], None);
        assert_eq!(modulation.depths[2], 0.0);
        assert_eq!(
            &modulation.msegs[..3],
            &TrackModulation::default().msegs[..3]
        );
    }

    #[test]
    fn legacy_effect_state_defaults_mix_and_track_flags() {
        let effect: TrackEffect =
            serde_json::from_str(r#"{"kind":"None","params":[0,0,0,0,0]}"#).unwrap();
        assert_eq!(effect, TrackEffect::default());
    }
}
