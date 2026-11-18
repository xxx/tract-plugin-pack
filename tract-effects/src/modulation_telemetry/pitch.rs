use super::*;
use crate::modulation::{MidiPitchReading, MidiPitchSettings};

#[derive(Default)]
pub(super) struct AtomicPitch {
    active: AtomicBool,
    settings: AtomicU32,
    note: AtomicU32,
    hz: AtomicU64,
}
impl AtomicPitch {
    pub(super) fn store(&self, reading: Option<MidiPitchReading>) {
        self.active.store(reading.is_some(), Ordering::Release);
        let Some(reading) = reading else {
            return;
        };
        self.settings.store(
            reading.settings.channel.map_or(0, |c| c as u32 + 1)
                | (((reading.settings.transpose as i16 + 128) as u32) << 16),
            Ordering::Release,
        );
        self.note.store(
            reading.note.map_or(0, |note| note as u32 + 1),
            Ordering::Release,
        );
        self.hz.store(reading.hz.to_bits(), Ordering::Release);
    }
    pub(super) fn load(&self) -> Option<MidiPitchReading> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let settings = self.settings.load(Ordering::Acquire);
        let note = self.note.load(Ordering::Acquire);
        let channel = settings & 0xffff;
        Some(MidiPitchReading {
            settings: MidiPitchSettings {
                channel: channel.checked_sub(1).map(|c| c as u8),
                transpose: ((settings >> 16) as i16 - 128) as i8,
            },
            note: note.checked_sub(1).map(|note| note as u8),
            hz: f64::from_bits(self.hz.load(Ordering::Acquire)),
        })
    }
}
