use super::*;
use crate::modulation::MidiWaveMapping;
use keyboard_types::{Key, KeyState, Modifiers};
use widgets::dropdown::{DropdownEvent, DropdownKey, DropdownList, DropdownState};

type Rect = (f32, f32, f32, f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Enabled,
    Envelope,
    FirstNote,
    Channel,
    Wave,
}
impl Field {
    const ALL: [Self; 5] = [
        Self::Enabled,
        Self::Envelope,
        Self::FirstNote,
        Self::Channel,
        Self::Wave,
    ];
    fn items(self) -> &'static [&'static str] {
        match self {
            Self::Enabled => &["Off", "On"],
            Self::Envelope => &["Amp", "1", "2", "3"],
            Self::FirstNote => &super::midi::NOTE_ITEMS[1..122],
            Self::Channel => super::midi::CHANNEL_ITEMS,
            Self::Wave => &crate::modulation::WAVE_AUTOMATION_ITEMS,
        }
    }
}

pub(super) struct Panel {
    open: bool,
    owner: Option<(EffectKind, Option<MidiWaveMapping>)>,
    draft: MidiWaveMapping,
    focus: usize,
    dropdown: DropdownState<Field>,
    wave: u8,
    wave_envelope: usize,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            open: false,
            owner: None,
            draft: MidiWaveMapping {
                envelope: 1,
                first_note: 36,
                channel: None,
            },
            focus: 0,
            dropdown: DropdownState::new(),
            wave: 0,
            wave_envelope: 1,
        }
    }
}
impl Panel {
    fn open_field(&self) -> Option<Field> {
        Field::ALL
            .into_iter()
            .find(|field| self.dropdown.is_open_for(*field))
    }
    pub(super) fn close(&mut self) {
        self.open = false;
        self.owner = None;
        self.dropdown.close();
    }
    pub(super) fn open(&self) -> bool {
        self.open
    }
    fn enabled(&self) -> bool {
        self.owner.is_some_and(|(_, mapping)| mapping.is_some())
    }
    fn index(&self, field: Field) -> usize {
        match field {
            Field::Enabled => usize::from(self.enabled()),
            Field::Envelope => self.draft.envelope as usize,
            Field::FirstNote => self.draft.first_note as usize,
            Field::Channel => self.draft.channel.map_or(0, |channel| channel as usize + 1),
            Field::Wave => self.wave as usize,
        }
    }
}

pub(super) struct Layout {
    pub panel: Rect,
    pub close: Rect,
    pub values: [Rect; 5],
    labels: [Rect; 5],
    footer: Rect,
}
pub(super) fn layout(window: (f32, f32), pane: Rect, scale: f32) -> Option<Layout> {
    let pad = (8.0 * scale).max(4.0);
    let w = (420.0 * scale).max(240.0).min(window.0 - 2.0 * pad);
    let h = (308.0 * scale).max(220.0).min(window.1 - 2.0 * pad);
    let header = (34.0 * scale).max(26.0);
    let footer = (60.0 * scale).max(50.0);
    if w < 216.0 || h < header + footer + 5.0 * (24.0 * scale).max(24.0) {
        return None;
    }
    let x = (pane.0 + (pane.2 - w) * 0.5).clamp(pad, (window.0 - w - pad).max(pad));
    let y = (pane.1 + (pane.3 - h) * 0.5).clamp(pad, (window.1 - h - pad).max(pad));
    let row = (h - header - footer) / 5.0;
    let label_w = ((w - 2.0 * pad) * 0.4).min(130.0 * scale).max(32.0);
    let values = std::array::from_fn(|i| {
        (
            x + pad + label_w,
            y + header + i as f32 * row,
            (w - 2.0 * pad - label_w).max(0.0),
            (row - 4.0 * scale).min((34.0 * scale).max(20.0)),
        )
    });
    let labels = std::array::from_fn(|i| (x + pad, values[i].1, label_w - 4.0, values[i].3));
    Some(Layout {
        panel: (x, y, w, h),
        close: (
            x + w - pad - (22.0 * scale).max(14.0),
            y + pad * 0.5,
            (22.0 * scale).max(14.0),
            (20.0 * scale).max(14.0),
        ),
        values,
        labels,
        footer: (x + pad, y + h - footer, w - 2.0 * pad, footer),
    })
}

fn text(
    tr: &mut widgets::TextRenderer,
    pm: &mut tiny_skia::Pixmap,
    at: (f32, f32),
    value: &str,
    size: f32,
    width: f32,
    color: tiny_skia::Color,
) {
    let mut visible = String::new();
    for character in value.chars() {
        visible.push(character);
        if tr.text_width(&visible, size) > width {
            visible.pop();
            break;
        }
    }
    tr.draw_text(pm, at.0, at.1, &visible, size, color);
}

impl EffectEditor {
    /// Bind current host values; edits are returned as host parameter requests.
    pub fn set_wave_automation(&mut self, values: [u8; 4]) {
        let values = values.map(|value| if value <= 8 { value } else { 0 });
        if self.wave_automation != Some(values) {
            self.wave_automation = Some(values);
            self.clear_modulation_trace();
        }
        let slot = if self.wave_midi.open {
            self.wave_midi.wave_envelope
        } else {
            self.selected_mseg.min(3)
        };
        self.wave_midi.wave = values[slot];
    }
    pub(super) fn check_wave_midi_binding(
        &mut self,
        effect: &TrackEffect,
        config: &TrackModulation,
    ) -> bool {
        if self.wave_midi.open && self.wave_midi.owner != Some((effect.kind, config.wave_switch)) {
            self.wave_midi.close();
            true
        } else {
            false
        }
    }
    pub fn wave_midi_panel_open(&self) -> bool {
        self.wave_midi.open()
    }
    pub(super) fn wave_display_curve(
        &self,
        effect: &TrackEffect,
        config: &TrackModulation,
        slot: usize,
    ) -> Option<widgets::MsegData> {
        let automated = self.wave_automation.unwrap_or([0; 4])[slot];
        let mapping = config
            .wave_switch
            .filter(|mapping| mapping.is_valid() && mapping.envelope as usize == slot);
        if automated == 0 && mapping.is_none() {
            return None;
        }
        let mut display = config.msegs[slot];
        display.hold = widgets::HoldMode::None;
        let midi = self
            .live
            .as_ref()
            .and_then(|source| source.frame)
            .filter(|frame| frame.matches(effect.kind, config))
            .and_then(|frame| frame.wave)
            .filter(|(value, _)| Some(*value) == mapping)
            .and_then(|(_, slot)| slot);
        let active = automated.checked_sub(1).or(midi);
        if let Some(active) = active
            && let Some(binding) = self.mseg_edit.wave_palette()
            && let Ok(palette) = binding.lock()
            && let Some(shape) = palette.get(active as usize)
        {
            let _ = shape.apply(&mut display, false);
        }
        Some(display)
    }
    pub(super) fn sync_wave_view(&mut self, effect: &TrackEffect, config: &TrackModulation) {
        if self.wave_midi.open && self.wave_midi.owner != Some((effect.kind, config.wave_switch)) {
            self.wave_midi.close();
        }
        let display = self.wave_display_curve(effect, config, self.selected_mseg.min(3));
        self.mseg_edit.set_read_only_curve(display);
    }
    pub(super) fn draw_wave_midi_popup(
        &self,
        pm: &mut tiny_skia::Pixmap,
        tr: &mut widgets::TextRenderer,
        geometry: EditorGeometry,
    ) {
        if !self.wave_midi.open {
            return;
        }
        let Some(lay) = layout(
            geometry.window_size,
            geometry.layout.mseg_pane,
            geometry.scale,
        ) else {
            return;
        };
        let s = geometry.scale;
        let p = lay.panel;
        widgets::draw_rect(pm, p.0, p.1, p.2, p.3, widgets::color_bg());
        widgets::draw_rect_outline(pm, p.0, p.1, p.2, p.3, widgets::color_accent(), 1.0);
        text(
            tr,
            pm,
            (p.0 + 6.0, p.1 + (22.0 * s).max(17.0)),
            "Project waves",
            (16.0 * s).max(11.0),
            (lay.close.0 - p.0 - 10.0).max(0.0),
            widgets::color_text(),
        );
        let r = lay.close;
        widgets::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "X", false, false);
        for (index, field) in Field::ALL.into_iter().enumerate() {
            let r = lay.labels[index];
            let font = (12.0 * s).max(10.0);
            let wave_label = format!(
                "Wave {}",
                ["Amp", "1", "2", "3"][self.wave_midi.wave_envelope]
            );
            text(
                tr,
                pm,
                (r.0, r.1 + (r.3 + font) * 0.5 - 2.0),
                ["MIDI", "MIDI curve", "First", "Ch.", &wave_label][index],
                font,
                r.2,
                widgets::color_muted(),
            );
            let r = lay.values[index];
            widgets::controls::draw_button(
                pm,
                tr,
                r.0,
                r.1,
                r.2,
                r.3,
                "",
                self.wave_midi.focus == index,
                false,
            );
            let label = field.items()[self.wave_midi.index(field).min(field.items().len() - 1)];
            let width = tr.text_width(label, font).min((r.2 - 8.0).max(0.0));
            text(
                tr,
                pm,
                (r.0 + (r.2 - width) * 0.5, r.1 + (r.3 + font) * 0.5 - 2.0),
                label,
                font,
                (r.2 - 8.0).max(0.0),
                if self.wave_midi.focus == index {
                    widgets::color_bg()
                } else {
                    widgets::color_text()
                },
            );
        }
        let font = (11.0 * s).max(10.0);
        let range = format!(
            "Notes {}-{}: Project 1-8",
            self.wave_midi.draft.first_note,
            self.wave_midi.draft.first_note + 7
        );
        text(
            tr,
            pm,
            (lay.footer.0, lay.footer.1 + font),
            &range,
            font,
            lay.footer.2,
            widgets::color_text(),
        );
        text(
            tr,
            pm,
            (lay.footer.0, lay.footer.1 + font * 2.3),
            "Project overrides MIDI",
            font,
            lay.footer.2,
            widgets::color_muted(),
        );
        text(
            tr,
            pm,
            (lay.footer.0, lay.footer.1 + font * 3.6),
            "Drawn/MIDI restores normal playback",
            font,
            lay.footer.2,
            widgets::color_muted(),
        );
        let items = self
            .wave_midi
            .open_field()
            .map_or(Field::Enabled.items(), Field::items);
        widgets::dropdown::draw_dropdown_popup(
            pm,
            tr,
            &self.wave_midi.dropdown,
            items,
            geometry.window_size,
        );
    }
}

impl Session<'_> {
    pub(super) fn open_wave_midi(&mut self) {
        if layout(
            (self.physical_width, self.physical_height),
            self.layout.mseg_pane,
            self.scale_factor,
        )
        .is_none()
        {
            self.state
                .mseg_edit
                .set_status_message("Enlarge the window for wave setup");
            return;
        }
        self.commit_pending_text();
        self.state.mseg_edit.cancel_interaction();
        self.state.wave_midi.open = true;
        self.state.wave_midi.owner = Some((self.effect.kind, self.modulation.wave_switch));
        self.state.wave_midi.draft = self
            .modulation
            .wave_switch
            .filter(|mapping| mapping.is_valid())
            .unwrap_or(MidiWaveMapping {
                envelope: self.state.selected_mseg.min(3) as u8,
                first_note: 36,
                channel: None,
            });
        self.state.wave_midi.focus = 0;
        self.state.wave_midi.wave_envelope = self.state.selected_mseg.min(3);
        self.state.wave_midi.wave =
            self.state.wave_automation.unwrap_or([0; 4])[self.state.wave_midi.wave_envelope];
        self.state.wave_midi.dropdown.close();
    }
    fn apply_wave_field(&mut self, field: Field, index: usize) {
        if index >= field.items().len() {
            return;
        }
        if field == Field::Wave {
            if let Some(mut values) = self.state.wave_automation {
                let slot = self.state.wave_midi.wave_envelope;
                if values[slot] != index as u8 {
                    values[slot] = index as u8;
                    self.state.set_wave_automation(values);
                    self.response.wave_automation = Some((slot, index as u8));
                }
            }
            return;
        }
        let panel = &mut self.state.wave_midi;
        let previous = self.modulation.wave_switch;
        match field {
            Field::Enabled => {
                if index == 1 && self.state.mseg_edit.wave_palette().is_none() {
                    self.state
                        .mseg_edit
                        .set_status_message("Project waves unavailable");
                    return;
                }
                self.modulation.wave_switch = (index == 1).then_some(panel.draft);
            }
            Field::Envelope => panel.draft.envelope = index as u8,
            Field::FirstNote => panel.draft.first_note = index as u8,
            Field::Channel => {
                panel.draft.channel = index.checked_sub(1).map(|channel| channel as u8)
            }
            Field::Wave => {}
        }
        if field != Field::Enabled && self.modulation.wave_switch.is_some() {
            self.modulation.wave_switch = Some(panel.draft);
        }
        self.response.changed |= previous != self.modulation.wave_switch;
        panel.owner = Some((self.effect.kind, self.modulation.wave_switch));
    }
    fn open_wave_field(&mut self, field: Field, lay: &Layout) {
        if field == Field::Enabled {
            self.apply_wave_field(field, usize::from(!self.state.wave_midi.enabled()));
            return;
        }
        let index = Field::ALL
            .iter()
            .position(|value| *value == field)
            .unwrap_or(0);
        self.state.wave_midi.dropdown.open(
            field,
            lay.values[index],
            DropdownList::flat(field.items()),
            self.state.wave_midi.index(field),
            field == Field::FirstNote,
            (self.physical_width, self.physical_height),
        );
    }
    pub(super) fn handle_wave_midi_event(&mut self, event: &baseview::Event) -> bool {
        use baseview::{Event, MouseButton, MouseEvent, WindowEvent};
        if !self.state.wave_midi.open {
            return false;
        }
        self.response.captured = true;
        if self.state.wave_midi.owner != Some((self.effect.kind, self.modulation.wave_switch)) {
            self.state.wave_midi.close();
            return true;
        }
        let Some(lay) = layout(
            (self.physical_width, self.physical_height),
            self.layout.mseg_pane,
            self.scale_factor,
        ) else {
            self.state.wave_midi.close();
            return true;
        };
        let window = (self.physical_width, self.physical_height);
        match event {
            Event::Window(WindowEvent::Unfocused | WindowEvent::Resized(_)) => {
                self.state.wave_midi.close()
            }
            Event::Mouse(MouseEvent::CursorMoved { position, .. }) => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.state.mouse_pos = self.mouse_pos;
                if let Some(field) = self.state.wave_midi.open_field() {
                    self.state.wave_midi.dropdown.on_mouse_move(
                        self.mouse_pos.0,
                        self.mouse_pos.1,
                        field.items(),
                        window,
                    );
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => {
                let (x, y) = self.mouse_pos;
                if let Some(field) = self.state.wave_midi.open_field() {
                    if let Some(DropdownEvent::Selected(field, index)) = self
                        .state
                        .wave_midi
                        .dropdown
                        .on_mouse_down(x, y, field.items(), window)
                    {
                        self.apply_wave_field(field, index);
                    }
                } else if view::in_rect(lay.close, x, y) || !view::in_rect(lay.panel, x, y) {
                    self.state.wave_midi.close();
                } else if let Some(index) = lay
                    .values
                    .iter()
                    .position(|rect| view::in_rect(*rect, x, y))
                {
                    self.state.wave_midi.focus = index;
                    self.open_wave_field(Field::ALL[index], &lay);
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                if self.state.wave_midi.dropdown.is_open() {
                    self.state.wave_midi.dropdown.close();
                } else {
                    self.state.wave_midi.close();
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => self.state.wave_midi.dropdown.on_mouse_up(),
            Event::Mouse(MouseEvent::WheelScrolled { delta, .. }) => {
                if let Some(field) = self.state.wave_midi.open_field() {
                    let y = match delta {
                        baseview::ScrollDelta::Lines { y, .. } => *y,
                        baseview::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                    };
                    self.state
                        .wave_midi
                        .dropdown
                        .on_wheel(y, field.items(), window);
                }
            }
            Event::Keyboard(key) if key.state == KeyState::Down => {
                if key
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
                {
                    return true;
                }
                if let Some(field) = self.state.wave_midi.open_field() {
                    let action = match key.key {
                        Key::ArrowUp => Some(DropdownKey::Up),
                        Key::ArrowDown => Some(DropdownKey::Down),
                        Key::Home => Some(DropdownKey::Home),
                        Key::End => Some(DropdownKey::End),
                        Key::PageUp => Some(DropdownKey::PageUp),
                        Key::PageDown => Some(DropdownKey::PageDown),
                        Key::Enter => Some(DropdownKey::Enter),
                        Key::Escape => Some(DropdownKey::Esc),
                        Key::Backspace => Some(DropdownKey::Backspace),
                        _ => None,
                    };
                    let selected = if let Some(action) = action {
                        self.state
                            .wave_midi
                            .dropdown
                            .on_key(action, field.items(), window)
                    } else if let Key::Character(value) = &key.key {
                        let mut result = None;
                        for character in value.chars() {
                            result = self
                                .state
                                .wave_midi
                                .dropdown
                                .on_char(character, field.items())
                                .or(result);
                        }
                        result
                    } else {
                        None
                    };
                    if let Some(DropdownEvent::Selected(field, index)) = selected {
                        self.apply_wave_field(field, index);
                    }
                } else {
                    match &key.key {
                        Key::Escape => self.state.wave_midi.close(),
                        Key::Tab => {
                            let delta = if key.modifiers.contains(Modifiers::SHIFT) {
                                Field::ALL.len() - 1
                            } else {
                                1
                            };
                            self.state.wave_midi.focus =
                                (self.state.wave_midi.focus + delta) % Field::ALL.len();
                        }
                        Key::ArrowUp => {
                            self.state.wave_midi.focus =
                                (self.state.wave_midi.focus + Field::ALL.len() - 1)
                                    % Field::ALL.len()
                        }
                        Key::ArrowDown => {
                            self.state.wave_midi.focus =
                                (self.state.wave_midi.focus + 1) % Field::ALL.len()
                        }
                        Key::ArrowLeft | Key::ArrowRight => {
                            let field = Field::ALL[self.state.wave_midi.focus];
                            let count = field.items().len();
                            let delta = if key.key == Key::ArrowLeft {
                                count - 1
                            } else {
                                1
                            };
                            let index = (self.state.wave_midi.index(field) + delta) % count;
                            self.apply_wave_field(field, index);
                        }
                        Key::Enter => {
                            self.open_wave_field(Field::ALL[self.state.wave_midi.focus], &lay)
                        }
                        Key::Character(value) if value == " " => {
                            self.open_wave_field(Field::ALL[self.state.wave_midi.focus], &lay)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        true
    }
    pub(super) fn draw_wave_midi_button(&mut self, pm: &mut tiny_skia::Pixmap) {
        if self.state.mseg_edit.wave_palette().is_none() && self.modulation.wave_switch.is_none() {
            return;
        }
        let strip = self
            .state
            .mseg_edit
            .layout(self.layout.mseg_pane, self.scale_factor)
            .strip;
        let r = widgets::mseg::wave_midi_button_rect(strip, self.scale_factor);
        let active = self.modulation.wave_switch.is_some();
        let slot = self
            .state
            .live
            .as_ref()
            .and_then(|live| live.frame)
            .filter(|frame| frame.matches(self.effect.kind, self.modulation))
            .and_then(|frame| frame.wave)
            .and_then(|(_, slot)| slot);
        let automated =
            self.state.wave_automation.unwrap_or([0; 4])[self.state.selected_mseg.min(3)];
        let label = if automated > 0 {
            format!("Wave P{automated}")
        } else if active {
            slot.map_or_else(
                || "MIDI on".to_owned(),
                |slot| format!("MIDI P{}", slot + 1),
            )
        } else {
            "Waves...".to_owned()
        };
        widgets::controls::draw_button(
            pm,
            self.text_renderer,
            r.0,
            r.1,
            r.2,
            r.3,
            "",
            active || automated > 0,
            false,
        );
        let font = (r.3 * 0.5).max(10.0);
        let width = self
            .text_renderer
            .text_width(&label, font)
            .min((r.2 - 6.0).max(0.0));
        text(
            self.text_renderer,
            pm,
            (r.0 + (r.2 - width) * 0.5, r.1 + (r.3 + font) * 0.5 - 2.0),
            &label,
            font,
            (r.2 - 6.0).max(0.0),
            if active || automated > 0 {
                widgets::color_bg()
            } else {
                widgets::color_text()
            },
        );
    }
}
