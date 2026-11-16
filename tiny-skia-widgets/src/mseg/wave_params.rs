//! Stable host controls for Project-wave playback, separate from authored curves.
use nih_plug::prelude::*;
use nih_plug::wrapper::state::ParamValue;
use std::sync::Arc;

pub const WAVE_AUTOMATION_ITEMS: [&str; 9] = [
    "Drawn/MIDI",
    "Project 1",
    "Project 2",
    "Project 3",
    "Project 4",
    "Project 5",
    "Project 6",
    "Project 7",
    "Project 8",
];
const IDS: [&str; 4] = ["wave_amp", "wave_1", "wave_2", "wave_3"];

#[derive(Params)]
pub struct WaveControls {
    #[id = "wave_amp"]
    pub amp: IntParam,
    #[id = "wave_1"]
    pub one: IntParam,
    #[id = "wave_2"]
    pub two: IntParam,
    #[id = "wave_3"]
    pub three: IntParam,
}
impl WaveControls {
    pub fn new(prefix: &str) -> Self {
        let make = |curve| {
            IntParam::new(
                format!("{prefix} {curve} Wave"),
                0,
                IntRange::Linear { min: 0, max: 8 },
            )
            .with_value_to_string(Arc::new(|value| {
                WAVE_AUTOMATION_ITEMS[value as usize].to_owned()
            }))
            .with_string_to_value(Arc::new(|text| {
                let text = text.trim();
                WAVE_AUTOMATION_ITEMS
                    .iter()
                    .position(|item| item.eq_ignore_ascii_case(text))
                    .map(|value| value as i32)
                    .or_else(|| {
                        text.parse::<i32>()
                            .ok()
                            .filter(|value| (0..=8).contains(value))
                    })
            }))
        };
        Self {
            amp: make("Amp"),
            one: make("1"),
            two: make("2"),
            three: make("3"),
        }
    }
    pub fn controls(&self) -> [&IntParam; 4] {
        [&self.amp, &self.one, &self.two, &self.three]
    }
    pub fn values(&self) -> [u8; 4] {
        self.controls().map(|param| param.value() as u8)
    }
    /// NIH-plug preserves missing parameters on hotload; old states must clear new lanes.
    pub fn prepare_state(state: &mut PluginState, banks: usize) {
        for bank in 1..=banks {
            for id in IDS {
                state
                    .params
                    .entry(format!("{id}_{bank}"))
                    .or_insert(ParamValue::I32(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_are_stepped_automatable_and_format_all_project_slots() {
        let controls = WaveControls::new("Row 1");
        assert_eq!(controls.values(), [0; 4]);
        for param in controls.controls() {
            assert!(!param.flags().contains(ParamFlags::NON_AUTOMATABLE));
            assert_eq!(param.step_count(), Some(8));
            for (value, label) in WAVE_AUTOMATION_ITEMS.iter().enumerate() {
                let norm = param.preview_normalized(value as i32);
                assert_eq!(param.normalized_value_to_string(norm, false), *label);
                assert_eq!(param.string_to_normalized_value(label), Some(norm));
                assert_eq!(
                    param.string_to_normalized_value(&value.to_string()),
                    Some(norm)
                );
            }
            for invalid in ["-1", "9", "nan", "Project 9"] {
                assert_eq!(param.string_to_normalized_value(invalid), None);
            }
        }
    }
    #[test]
    fn migration_fills_only_missing_selectors_and_preserves_other_state() {
        let mut state = PluginState {
            version: "legacy".into(),
            params: [
                ("mix".into(), ParamValue::F32(0.25)),
                ("wave_3_2".into(), ParamValue::I32(8)),
            ]
            .into(),
            fields: [("other".into(), "saved".into())].into(),
        };
        WaveControls::prepare_state(&mut state, 2);
        assert_eq!(state.params.len(), 9);
        assert!(matches!(state.params["wave_3_2"], ParamValue::I32(8)));
        assert!(matches!(state.params["wave_amp_1"], ParamValue::I32(0)));
        assert!(matches!(state.params["mix"], ParamValue::F32(0.25)));
        assert_eq!(state.fields["other"], "saved");
    }
}
