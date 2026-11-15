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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiField {
    Note,
    Channel,
}

impl MidiField {
    pub(super) fn items(self) -> &'static [&'static str] {
        match self {
            Self::Note => NOTE_ITEMS,
            Self::Channel => CHANNEL_ITEMS,
        }
    }
    pub(super) fn rect(self, layout: &EffectLayout, scale: f32) -> (f32, f32, f32, f32) {
        let r = match self {
            Self::Note => layout.trigger_rate,
            Self::Channel => layout.trigger_aux,
        };
        (r.0, r.1 + 28.0 * scale, r.2, 34.0 * scale)
    }
}

impl Session<'_> {
    pub(super) fn draw_midi_controls(&mut self, pm: &mut tiny_skia::Pixmap) {
        let TriggerSource::MidiNote { note, channel } = self.modulation.trigger else {
            return;
        };
        for (field, value, label) in [
            (MidiField::Note, note, "Note"),
            (MidiField::Channel, channel, "Ch."),
        ] {
            let r = field.rect(&self.layout, self.scale_factor);
            let index = value.map_or(0, |value| value as usize + 1);
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
        let TriggerSource::MidiNote { note, channel } = self.modulation.trigger else {
            return;
        };
        let value = match field {
            MidiField::Note => note,
            MidiField::Channel => channel,
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
            value
                .map_or(0, |value| value as usize + 1)
                .min(items.len() - 1),
            field == MidiField::Note,
            (self.physical_width, self.physical_height),
        );
    }

    pub(super) fn apply_midi_filter(&mut self, field: MidiField, index: usize) {
        if index >= field.items().len() {
            return;
        }
        if let TriggerSource::MidiNote { note, channel } = &mut self.modulation.trigger {
            let dest = match field {
                MidiField::Note => note,
                MidiField::Channel => channel,
            };
            let value = index.checked_sub(1).map(|value| value as u8);
            if *dest != value {
                *dest = value;
                self.response.changed = true;
            }
        }
    }
}
