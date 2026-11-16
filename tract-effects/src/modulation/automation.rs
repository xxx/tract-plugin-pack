use super::{TrackModulation, wave::MidiWaveRuntime};
use tiny_skia_widgets::mseg::{HoldMode, MsegData, WavePaletteSnapshot};

pub use tiny_skia_widgets::mseg::WAVE_AUTOMATION_ITEMS;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AutomatedWaves {
    pub(super) requested: [u8; 4],
    selected: [u8; 4],
    curves: Box<[MsegData; 4]>,
}
impl AutomatedWaves {
    pub(super) fn curve<'a>(&'a self, fallback: &'a MsegData, slot: usize) -> &'a MsegData {
        if self.selected[slot] > 0 {
            &self.curves[slot]
        } else {
            fallback
        }
    }
    pub(super) fn active(&self, slot: usize) -> bool {
        self.selected[slot] > 0
    }
    pub(super) fn reset(&mut self) {
        self.selected = [0; 4];
    }
    pub(super) fn inherit(&mut self, previous: &Self) {
        self.requested = previous.requested;
        self.selected = previous.selected;
        *self.curves = *previous.curves;
    }
    pub(super) fn prepare(
        &mut self,
        config: &TrackModulation,
        midi: &MidiWaveRuntime,
        bank: &WavePaletteSnapshot,
    ) -> u8 {
        let mut changed = 0;
        for slot in 0..4 {
            if self.requested[slot] == 0 && self.selected[slot] == 0 {
                continue;
            }
            let fallback = midi.curve(config, slot);
            let mut next = *fallback;
            if let Some(project) = self.requested[slot].checked_sub(1) {
                next = config.msegs[slot];
                next.hold = HoldMode::None;
                if let Some(shape) = bank.get(project as usize) {
                    shape.apply_to(&mut next);
                }
            }
            let before = self.curve(fallback, slot);
            if before.active() != next.active() || before.hold != next.hold {
                changed |= 1 << slot;
            }
            self.curves[slot] = next;
            self.selected[slot] = self.requested[slot];
        }
        changed
    }
}

impl super::ModulationState {
    pub fn set_wave_automation(&mut self, values: [u8; 4]) {
        self.automated.requested = values.map(|value| if value <= 8 { value } else { 0 });
    }
    pub fn wave_automation(&self) -> [u8; 4] {
        self.automated.requested
    }
}
