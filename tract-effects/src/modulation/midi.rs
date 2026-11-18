//! Host-independent MIDI note validation and streaming trigger boundaries.

use super::{ModulationState, TrackModulation, TriggerSource};
use tiny_skia_widgets::mseg::WavePaletteSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiNoteOn {
    pub timing: usize,
    pub channel: u8,
    pub note: u8,
}

impl MidiNoteOn {
    pub fn new(timing: u32, channel: u8, note: u8, velocity: f32) -> Option<Self> {
        (channel < 16 && note < 128 && velocity.is_finite() && velocity > 0.0).then_some(Self {
            timing: timing as usize,
            channel,
            note,
        })
    }
}

impl TriggerSource {
    pub fn accepts_midi(self, event: MidiNoteOn) -> bool {
        match self {
            Self::MidiNote { note, channel } => {
                event.note < 128
                    && event.channel < 16
                    && note.is_none_or(|note| note == event.note)
                    && channel.is_none_or(|channel| channel == event.channel)
            }
            Self::MidiPitch { channel, .. } => {
                event.note < 128
                    && event.channel < 16
                    && channel.is_none_or(|channel| channel == event.channel)
            }
            _ => false,
        }
    }
}

impl TrackModulation {
    /// Match notes that can stage a wave or retrigger this track.
    pub fn accepts_playback_note(&self, bank: &WavePaletteSnapshot, event: MidiNoteOn) -> bool {
        self.trigger.accepts_midi(event)
            || self
                .wave_switch
                .and_then(|mapping| mapping.slot(event))
                .is_some_and(|slot| bank.get(slot).is_some())
    }
}

/// DSP and scheduling changes caused by one note.
pub struct MidiPlaybackResult {
    pub wave_staged: bool,
    pub retriggered: bool,
}

impl ModulationState {
    /// Stage the note's wave before applying its retrigger. Wave refresh waits for the sample.
    pub fn dispatch_playback_note(
        &mut self,
        config: &TrackModulation,
        bank: &WavePaletteSnapshot,
        event: MidiNoteOn,
        within_span: bool,
    ) -> MidiPlaybackResult {
        let wave_staged = self.select_midi_wave(config, bank, event);
        let retriggered = self.midi_note_on(config, event, within_span);
        MidiPlaybackResult {
            wave_staged,
            retriggered,
        }
    }
}

#[derive(Default)]
pub struct MidiCursor {
    last_timing: usize,
}

impl MidiCursor {
    pub fn next_matching(
        &mut self,
        events: &mut impl Iterator<Item = MidiNoteOn>,
        frames: usize,
        mut accepts: impl FnMut(MidiNoteOn) -> bool,
    ) -> Option<MidiNoteOn> {
        for event in events.by_ref() {
            if event.timing >= frames
                || event.timing < self.last_timing
                || event.channel >= 16
                || event.note >= 128
            {
                continue;
            }
            self.last_timing = event.timing;
            if accepts(event) {
                return Some(event);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_out_of_range_note_and_channel_are_rejected_before_matching() {
        let valid = MidiNoteOn {
            timing: 2,
            channel: 15,
            note: 127,
        };
        let invalid_channel = MidiNoteOn {
            channel: 16,
            ..valid
        };
        let invalid_note = MidiNoteOn { note: 128, ..valid };
        for source in [
            TriggerSource::MidiNote {
                note: None,
                channel: None,
            },
            TriggerSource::MidiPitch {
                transpose: 0,
                channel: None,
            },
        ] {
            assert!(source.accepts_midi(valid));
            assert!(!source.accepts_midi(invalid_channel));
            assert!(!source.accepts_midi(invalid_note));
        }
        let mut cursor = MidiCursor::default();
        let mut events = [invalid_channel, invalid_note, valid].into_iter();
        assert_eq!(cursor.next_matching(&mut events, 3, |_| true), Some(valid));
        assert_eq!(cursor.next_matching(&mut events, 3, |_| true), None);
    }

    #[test]
    fn filters_accept_only_matching_valid_positive_note_ons() {
        let all = TriggerSource::MidiNote {
            note: None,
            channel: None,
        };
        let filtered = TriggerSource::MidiNote {
            note: Some(60),
            channel: Some(2),
        };
        for channel in [0, 2, 15] {
            for note in [0, 60, 127] {
                let event = MidiNoteOn::new(17, channel, note, 0.5).unwrap();
                assert!(all.accepts_midi(event));
                assert_eq!(filtered.accepts_midi(event), channel == 2 && note == 60);
                assert!(!TriggerSource::Free.accepts_midi(event));
            }
        }
        for velocity in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(MidiNoteOn::new(0, 0, 60, velocity).is_none());
        }
        assert!(MidiNoteOn::new(0, 16, 60, 1.0).is_none());
        assert!(MidiNoteOn::new(0, 0, 128, 1.0).is_none());
    }

    #[test]
    fn streaming_cursor_keeps_simultaneous_events_and_ignores_late_invalid_offsets() {
        let times = [0, 0, 17, 16, 31, 32, usize::MAX];
        let mut events = times.into_iter().map(|timing| MidiNoteOn {
            timing,
            channel: 0,
            note: 60,
        });
        let mut cursor = MidiCursor::default();
        let actual: Vec<_> = std::iter::from_fn(|| cursor.next_matching(&mut events, 32, |_| true))
            .map(|event| event.timing)
            .collect();
        assert_eq!(actual, [0, 0, 17, 31]);
        let mut events = (0..10000).map(|_| MidiNoteOn {
            timing: 0,
            channel: 0,
            note: 60,
        });
        let mut cursor = MidiCursor::default();
        assert_eq!(
            std::iter::from_fn(|| cursor.next_matching(&mut events, 1, |_| true)).count(),
            10000
        );
    }

    #[test]
    fn midi_filter_state_round_trips_and_old_triggers_keep_their_wire_format() {
        for source in [
            TriggerSource::MidiNote {
                note: None,
                channel: None,
            },
            TriggerSource::MidiNote {
                note: Some(127),
                channel: Some(15),
            },
        ] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(
                serde_json::from_str::<TriggerSource>(&json).unwrap(),
                source
            );
        }
        assert_eq!(
            serde_json::to_string(&TriggerSource::Free).unwrap(),
            "\"Free\""
        );
        assert_eq!(
            serde_json::from_str::<TriggerSource>("{\"FreeHz\":{\"hz\":2.0}}").unwrap(),
            TriggerSource::FreeHz { hz: 2.0 }
        );
    }

    #[test]
    fn midi_interruption_restarts_mix_from_the_consumed_value_not_the_future_endpoint() {
        use crate::effects::{EffectInstance, EffectKind, TrackEffect};
        use crate::modulation::{ModulationState, ModulationTarget, TrackModulation};
        let mut config = TrackModulation {
            trigger: TriggerSource::MidiNote {
                note: None,
                channel: None,
            },
            targets: [Some(ModulationTarget::Mix), None, None],
            depths: [1.0, 0.0, 0.0],
            ..TrackModulation::default()
        };
        for node in &mut config.msegs[1].nodes {
            node.value = 1.0;
        }
        let track = TrackEffect {
            mix: 0.0,
            ..TrackEffect::default()
        };
        let mut state = ModulationState::default();
        let mut effect = EffectInstance::new(EffectKind::None);
        state.advance_segment(&config, &track, &mut effect, 100, 120.0, 48000.0);
        let mut before = 0.0;
        for _ in 0..50 {
            before = state.next_mix(0.0);
        }
        assert!((before - 0.5).abs() < 1e-6);
        state.trigger_midi_in_span();
        state.advance_segment(&config, &track, &mut effect, 50, 120.0, 48000.0);
        assert!((state.next_mix(0.0) - 0.51).abs() < 1e-6);
        for _ in 0..49 {
            state.next_mix(0.0);
        }
        assert_eq!(state.next_mix(0.0), 1.0);
    }
}
