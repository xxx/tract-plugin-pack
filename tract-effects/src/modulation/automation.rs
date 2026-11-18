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

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia_widgets::mseg::{FactoryShape, WavePalette, factory_shape};

    #[test]
    fn handoff_inherits_the_audible_waves_and_pending_automation_in_each_lane() {
        let mut palette = WavePalette::default();
        let duck = factory_shape(FactoryShape::Duck);
        let square = factory_shape(FactoryShape::Square);
        palette.store(0, &duck).unwrap();
        palette.store(1, &square).unwrap();
        let mut bank = WavePaletteSnapshot::default();
        bank.refresh(&palette);
        let cfg = TrackModulation::default();
        let midi = MidiWaveRuntime::default();
        let mut old = AutomatedWaves {
            requested: [1, 0, 2, 0],
            ..AutomatedWaves::default()
        };
        assert_eq!(old.prepare(&cfg, &midi, &bank), 0b0101);
        old.requested = [1, 2, 0, 0];

        let mut next = AutomatedWaves::default();
        next.inherit(&old);
        assert_eq!(next.curve(&cfg.msegs[0], 0).active(), duck.nodes());
        assert_eq!(next.curve(&cfg.msegs[2], 2).active(), square.nodes());
        assert_eq!(next.prepare(&cfg, &midi, &bank), 0b0110);
        assert_eq!(next.curve(&cfg.msegs[1], 1).active(), square.nodes());
        assert!(!next.active(2));

        next.reset();
        assert!((0..4).all(|slot| !next.active(slot)));
        assert_eq!(next.prepare(&cfg, &midi, &bank), 0b0011);
    }
}
