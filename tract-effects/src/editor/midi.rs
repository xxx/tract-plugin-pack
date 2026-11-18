use super::*;

pub(super) const NOTE_ITEMS: &[&str] = &[
    "Any", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
    "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31",
    "32", "33", "34", "35", "36", "37", "38", "39", "40", "41", "42", "43", "44", "45", "46", "47",
    "48", "49", "50", "51", "52", "53", "54", "55", "56", "57", "58", "59", "60", "61", "62", "63",
    "64", "65", "66", "67", "68", "69", "70", "71", "72", "73", "74", "75", "76", "77", "78", "79",
    "80", "81", "82", "83", "84", "85", "86", "87", "88", "89", "90", "91", "92", "93", "94", "95",
    "96", "97", "98", "99", "100", "101", "102", "103", "104", "105", "106", "107", "108", "109",
    "110", "111", "112", "113", "114", "115", "116", "117", "118", "119", "120", "121", "122",
    "123", "124", "125", "126", "127",
];
pub(super) const CHANNEL_ITEMS: &[&str] = &[
    "Any", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
];
pub(super) const TRANSPOSE_ITEMS: &[&str] = &[
    "-36", "-35", "-34", "-33", "-32", "-31", "-30", "-29", "-28", "-27", "-26", "-25", "-24",
    "-23", "-22", "-21", "-20", "-19", "-18", "-17", "-16", "-15", "-14", "-13", "-12", "-11",
    "-10", "-9", "-8", "-7", "-6", "-5", "-4", "-3", "-2", "-1", "0", "1", "2", "3", "4", "5", "6",
    "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22",
    "23", "24", "25", "26", "27", "28", "29", "30", "31", "32", "33", "34", "35", "36",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiField {
    Note,
    Channel,
    Transpose,
}
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Binding {
    kind: EffectKind,
    trigger: TriggerSource,
    live: Option<(usize, usize, u64)>,
}

impl MidiField {
    pub(super) fn items(self) -> &'static [&'static str] {
        match self {
            Self::Note => NOTE_ITEMS,
            Self::Channel => CHANNEL_ITEMS,
            Self::Transpose => TRANSPOSE_ITEMS,
        }
    }
    pub(super) fn rect(self, layout: &EffectLayout, scale: f32) -> (f32, f32, f32, f32) {
        let r = match self {
            Self::Note | Self::Transpose => layout.trigger_rate,
            Self::Channel => layout.trigger_aux,
        };
        (r.0, r.1 + 28.0 * scale, r.2, 34.0 * scale)
    }
    fn selected(self, trigger: TriggerSource) -> Option<usize> {
        match (self, trigger) {
            (Self::Note, TriggerSource::MidiNote { note, .. }) => {
                Some(note.map_or(0, |n| n as usize + 1))
            }
            (
                Self::Channel,
                TriggerSource::MidiNote { channel, .. } | TriggerSource::MidiPitch { channel, .. },
            ) => Some(channel.map_or(0, |c| c as usize + 1)),
            (Self::Transpose, source) => source
                .pitch_settings()
                .map(|s| (s.transpose as i16 + 36) as usize),
            _ => None,
        }
    }
}

impl Session<'_> {
    pub(super) fn draw_midi_controls(&mut self, pm: &mut tiny_skia::Pixmap) {
        for field in [MidiField::Note, MidiField::Transpose, MidiField::Channel] {
            let Some(index) = field.selected(self.modulation.trigger) else {
                continue;
            };
            let label = match field {
                MidiField::Note => "Note",
                MidiField::Channel => "Ch.",
                MidiField::Transpose => "Semi",
            };
            let r = field.rect(&self.layout, self.scale_factor);
            let value = field.items().get(index).copied().unwrap_or("Invalid");
            self.text_renderer.draw_text(
                pm,
                r.0 + 4.0 * self.scale_factor,
                r.1 - 5.0 * self.scale_factor,
                label,
                (11.0 * self.scale_factor).max(10.0),
                widgets::color_muted(),
            );
            widgets::controls::draw_button(
                pm,
                self.text_renderer,
                r.0,
                r.1,
                r.2,
                r.3,
                value,
                self.state
                    .effect_dropdown
                    .is_open_for(EffectAction::MidiFilter(field)),
                false,
            );
        }
    }

    pub(super) fn open_midi_filter(&mut self, field: MidiField) {
        let Some(index) = field.selected(self.modulation.trigger) else {
            return;
        };
        let items = field.items();
        let r = field.rect(&self.layout, self.scale_factor);
        let font_size = (r.3 * 0.5).max(10.0);
        let width = (items
            .iter()
            .map(|label| self.text_renderer.text_width(label, font_size))
            .fold(0.0_f32, f32::max)
            + 19.0)
            .max(r.2);
        let anchor = (r.0, r.1, width, r.3);
        self.state.effect_dropdown.open(
            EffectAction::MidiFilter(field),
            anchor,
            widgets::dropdown::DropdownList::flat(items),
            index.min(items.len() - 1),
            field == MidiField::Note,
            (self.physical_width, self.physical_height),
        );
        self.state.midi_filter_owner =
            Some(self.state.midi_filter_binding(self.effect, self.modulation));
    }

    pub(super) fn apply_midi_filter(&mut self, field: MidiField, index: usize) {
        if index >= field.items().len() {
            return;
        }
        match (field, &mut self.modulation.trigger) {
            (MidiField::Transpose, TriggerSource::MidiPitch { transpose, .. }) => {
                let value = index as i8 - 36;
                if *transpose != value {
                    *transpose = value;
                    self.response.changed = true;
                }
            }
            (MidiField::Note, TriggerSource::MidiNote { note: dest, .. })
            | (
                MidiField::Channel,
                TriggerSource::MidiNote { channel: dest, .. }
                | TriggerSource::MidiPitch { channel: dest, .. },
            ) => {
                let value = index.checked_sub(1).map(|value| value as u8);
                if *dest != value {
                    *dest = value;
                    self.response.changed = true;
                }
            }
            _ => {}
        }
    }
    pub(super) fn draw_pitch_rate(&mut self, pm: &mut tiny_skia::Pixmap) {
        let reading = self
            .state
            .live
            .as_ref()
            .and_then(|live| live.frame)
            .filter(|frame| frame.matches(self.effect.kind, self.modulation))
            .and_then(|frame| frame.pitch);
        let (note_label, rate_label) = match reading {
            Some(reading) => match reading.note {
                Some(note) => (
                    format!("Note {note}"),
                    if reading.hz >= 1000.0 {
                        format!("{:.1} kHz", reading.hz / 1000.0)
                    } else if reading.hz < 10.0 {
                        format!("{:.2} Hz", reading.hz)
                    } else {
                        format!("{:.1} Hz", reading.hz)
                    },
                ),
                None => ("MIDI pitch".into(), "Waiting".into()),
            },
            None => ("MIDI pitch".into(), "No data".into()),
        };
        for (r, label) in [
            (self.layout.mseg_sync, note_label),
            (self.layout.mseg_length, rate_label),
        ] {
            widgets::draw_button(
                pm,
                self.text_renderer,
                r.0,
                r.1,
                r.2,
                r.3,
                &label,
                false,
                false,
            );
        }
    }
}

impl EffectEditor {
    fn midi_filter_binding(&self, effect: &TrackEffect, config: &TrackModulation) -> Binding {
        Binding {
            kind: effect.kind,
            trigger: config.trigger,
            live: self.live.as_ref().map(|source| source.binding_key()),
        }
    }
    pub(super) fn check_midi_filter_binding(
        &mut self,
        effect: &TrackEffect,
        config: &TrackModulation,
    ) -> bool {
        let open = [MidiField::Note, MidiField::Channel, MidiField::Transpose]
            .iter()
            .any(|field| {
                self.effect_dropdown
                    .is_open_for(EffectAction::MidiFilter(*field))
            });
        if open && self.midi_filter_owner != Some(self.midi_filter_binding(effect, config)) {
            self.effect_dropdown.close();
            self.midi_filter_owner = None;
            true
        } else {
            if !open {
                self.midi_filter_owner = None;
            }
            false
        }
    }
    pub(super) fn finish_inactive_pitch_controls(&mut self, config: &TrackModulation) {
        if config.trigger.pitch_settings().is_some()
            && self.effect_dial_drag.active_action() == Some(&EffectHit::MsegLength)
        {
            self.effect_dial_drag.end_drag();
        }
    }
}
