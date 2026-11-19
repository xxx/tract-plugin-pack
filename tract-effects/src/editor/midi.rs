use super::*;

pub(super) const NOTE_ITEMS: &[&str] = &[
    "Any",
    "C-1 (0)",
    "C#-1 (1)",
    "D-1 (2)",
    "D#-1 (3)",
    "E-1 (4)",
    "F-1 (5)",
    "F#-1 (6)",
    "G-1 (7)",
    "G#-1 (8)",
    "A-1 (9)",
    "A#-1 (10)",
    "B-1 (11)",
    "C0 (12)",
    "C#0 (13)",
    "D0 (14)",
    "D#0 (15)",
    "E0 (16)",
    "F0 (17)",
    "F#0 (18)",
    "G0 (19)",
    "G#0 (20)",
    "A0 (21)",
    "A#0 (22)",
    "B0 (23)",
    "C1 (24)",
    "C#1 (25)",
    "D1 (26)",
    "D#1 (27)",
    "E1 (28)",
    "F1 (29)",
    "F#1 (30)",
    "G1 (31)",
    "G#1 (32)",
    "A1 (33)",
    "A#1 (34)",
    "B1 (35)",
    "C2 (36)",
    "C#2 (37)",
    "D2 (38)",
    "D#2 (39)",
    "E2 (40)",
    "F2 (41)",
    "F#2 (42)",
    "G2 (43)",
    "G#2 (44)",
    "A2 (45)",
    "A#2 (46)",
    "B2 (47)",
    "C3 (48)",
    "C#3 (49)",
    "D3 (50)",
    "D#3 (51)",
    "E3 (52)",
    "F3 (53)",
    "F#3 (54)",
    "G3 (55)",
    "G#3 (56)",
    "A3 (57)",
    "A#3 (58)",
    "B3 (59)",
    "C4 (60)",
    "C#4 (61)",
    "D4 (62)",
    "D#4 (63)",
    "E4 (64)",
    "F4 (65)",
    "F#4 (66)",
    "G4 (67)",
    "G#4 (68)",
    "A4 (69)",
    "A#4 (70)",
    "B4 (71)",
    "C5 (72)",
    "C#5 (73)",
    "D5 (74)",
    "D#5 (75)",
    "E5 (76)",
    "F5 (77)",
    "F#5 (78)",
    "G5 (79)",
    "G#5 (80)",
    "A5 (81)",
    "A#5 (82)",
    "B5 (83)",
    "C6 (84)",
    "C#6 (85)",
    "D6 (86)",
    "D#6 (87)",
    "E6 (88)",
    "F6 (89)",
    "F#6 (90)",
    "G6 (91)",
    "G#6 (92)",
    "A6 (93)",
    "A#6 (94)",
    "B6 (95)",
    "C7 (96)",
    "C#7 (97)",
    "D7 (98)",
    "D#7 (99)",
    "E7 (100)",
    "F7 (101)",
    "F#7 (102)",
    "G7 (103)",
    "G#7 (104)",
    "A7 (105)",
    "A#7 (106)",
    "B7 (107)",
    "C8 (108)",
    "C#8 (109)",
    "D8 (110)",
    "D#8 (111)",
    "E8 (112)",
    "F8 (113)",
    "F#8 (114)",
    "G8 (115)",
    "G#8 (116)",
    "A8 (117)",
    "A#8 (118)",
    "B8 (119)",
    "C9 (120)",
    "C#9 (121)",
    "D9 (122)",
    "D#9 (123)",
    "E9 (124)",
    "F9 (125)",
    "F#9 (126)",
    "G9 (127)",
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

pub(super) fn note_matches(label: &str, query: &str) -> bool {
    if query.bytes().all(|byte| byte.is_ascii_digit()) {
        return label
            .split_once(" (")
            .is_some_and(|(_, number)| number.contains(query));
    }
    label
        .to_ascii_lowercase()
        .contains(&query.to_ascii_lowercase())
}

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
            let value = if field == MidiField::Note {
                value.split_once(' ').map_or(value, |(name, _)| name)
            } else {
                value
            };
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
        let list = widgets::dropdown::DropdownList::flat(items);
        let list = if field == MidiField::Note {
            list.with_filter(note_matches)
        } else {
            list
        };
        self.state.effect_dropdown.open(
            EffectAction::MidiFilter(field),
            anchor,
            list,
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
