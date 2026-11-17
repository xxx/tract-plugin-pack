use super::*;
use crate::modulation::{
    AudioTriggerSettings, DetectorInput, FILTER_MAX_HZ, FILTER_MIN_HZ, LEVEL_MAX_DB, LEVEL_MIN_DB,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorField {
    Level,
    Highpass,
    Lowpass,
}
impl DetectorField {
    pub(super) const ALL: [Self; 3] = [Self::Level, Self::Highpass, Self::Lowpass];
    fn label(self) -> &'static str {
        match self {
            Self::Level => "Threshold",
            Self::Highpass => "High-pass",
            Self::Lowpass => "Low-pass",
        }
    }
    pub(super) fn value(self, settings: AudioTriggerSettings) -> Option<f32> {
        match self {
            Self::Level => settings.level_db,
            Self::Highpass => settings.highpass_hz,
            Self::Lowpass => settings.lowpass_hz,
        }
    }
    pub(super) fn set(self, settings: &mut AudioTriggerSettings, value: Option<f32>) {
        match self {
            Self::Level => settings.level_db = value,
            Self::Highpass => settings.highpass_hz = value,
            Self::Lowpass => settings.lowpass_hz = value,
        }
    }
    pub(super) fn norm(self, value: Option<f32>) -> f32 {
        value.map_or(0.0, |value| {
            0.05 + 0.95
                * match self {
                    Self::Level => (value - LEVEL_MIN_DB) / (LEVEL_MAX_DB - LEVEL_MIN_DB),
                    _ => effects::value_to_norm(
                        value,
                        FILTER_MIN_HZ,
                        FILTER_MAX_HZ,
                        effects::ParamScaling::Log,
                    ),
                }
        })
    }
    pub(super) fn value_at_norm(self, norm: f32) -> Option<f32> {
        if norm < 0.025 {
            return None;
        }
        let value = ((norm - 0.05) / 0.95).clamp(0.0, 1.0);
        Some(match self {
            Self::Level => LEVEL_MIN_DB + value * (LEVEL_MAX_DB - LEVEL_MIN_DB),
            _ => effects::norm_to_value(
                value,
                FILTER_MIN_HZ,
                FILTER_MAX_HZ,
                effects::ParamScaling::Log,
            ),
        })
    }
    pub(super) fn parse(self, text: &str) -> Option<Option<f32>> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("off") {
            return Some(None);
        }
        let (format, lo, hi) = match self {
            Self::Level => (
                effects::ParamFormat::Number {
                    decimals: 1,
                    unit: "dB",
                },
                LEVEL_MIN_DB,
                LEVEL_MAX_DB,
            ),
            _ => (effects::ParamFormat::Hertz, FILTER_MIN_HZ, FILTER_MAX_HZ),
        };
        let value = effects::parse_value(text, format).filter(|value| value.is_finite())?;
        if self != Self::Level && value == 0.0 {
            return Some(None);
        }
        (lo..=hi).contains(&value).then_some(Some(value))
    }
    pub(super) fn display(self, value: Option<f32>) -> String {
        value.map_or_else(
            || "Off".into(),
            |value| match self {
                Self::Level => format!("{value:.1} dB"),
                _ => effects::format_value(value, effects::ParamFormat::Hertz),
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DetectorBinding {
    kind: EffectKind,
    source: (u32, u32),
    settings: AudioTriggerSettings,
    live: Option<(usize, usize, u64)>,
}

#[derive(Clone, Copy)]
pub(super) struct DetectorLayout {
    pub rect: (f32, f32, f32, f32),
    pub fields: [(f32, f32, f32, f32); 3],
    pub close: (f32, f32, f32, f32),
    pub input: (f32, f32, f32, f32),
    pub reset: (f32, f32, f32, f32),
    pub listen: (f32, f32, f32, f32),
    pub meter: (f32, f32, f32, f32),
    scale: f32,
}
pub(super) fn detector_layout(geometry: EditorGeometry) -> DetectorLayout {
    let (ww, wh) = geometry.window_size;
    let s = geometry
        .scale
        .min(((ww - 8.0) / 392.0).max(0.01))
        .min(((wh - 8.0) / 286.0).max(0.01))
        .max(0.01);
    let (w, h) = (392.0 * s, 286.0 * s);
    let anchor = geometry.layout.detector_setup;
    let x = anchor.0.clamp(4.0 * s, (ww - w - 4.0 * s).max(4.0 * s));
    let mut y = anchor.1 + anchor.3 + 4.0 * s;
    if y + h > wh - 4.0 * s {
        y = (anchor.1 - h - 4.0 * s).max(4.0 * s);
    }
    DetectorLayout {
        rect: (x, y, w, h),
        fields: std::array::from_fn(|i| {
            (
                x + 12.0 * s,
                y + (78.0 + i as f32 * 42.0) * s,
                w - 24.0 * s,
                34.0 * s,
            )
        }),
        close: (x + w - 32.0 * s, y + 8.0 * s, 24.0 * s, 20.0 * s),
        input: (x + 12.0 * s, y + 36.0 * s, w - 24.0 * s, 34.0 * s),
        reset: (x + 12.0 * s, y + 254.0 * s, 88.0 * s, 24.0 * s),
        listen: (x + w - 100.0 * s, y + 254.0 * s, 88.0 * s, 24.0 * s),
        meter: (x + 12.0 * s, y + 220.0 * s, w - 24.0 * s, 8.0 * s),
        scale: s,
    }
}

impl EffectEditor {
    pub fn detector_panel_open(&self) -> bool {
        self.detector_open
    }
    fn detector_binding(&self, kind: EffectKind, config: &TrackModulation) -> DetectorBinding {
        DetectorBinding {
            kind,
            source: match config.trigger {
                TriggerSource::TransientSample { threshold, hold_ms } => {
                    (threshold.to_bits(), hold_ms.to_bits())
                }
                _ => (0, 0),
            },
            settings: config.audio_trigger.normalized(),
            live: self.live.as_ref().map(|live| live.binding_key()),
        }
    }
    /// Discard detector input when the host replaces its binding.
    pub fn cancel_detector(&mut self) {
        self.stop_listening_source(crate::modulation::AuditionSource::Trigger);
        self.detector_open = false;
        self.detector_owner = None;
        if matches!(
            self.effect_dial_drag.active_action(),
            Some(EffectHit::Detector(_))
        ) {
            self.effect_dial_drag.end_drag();
        }
        if DetectorField::ALL.iter().any(|field| {
            self.text_edit
                .active_for(&EffectHit::Detector(*field))
                .is_some()
        }) {
            self.text_edit.cancel();
        }
    }
    pub(super) fn check_detector_binding(
        &mut self,
        kind: EffectKind,
        config: &TrackModulation,
    ) -> bool {
        if self.detector_open
            && (!matches!(config.trigger, TriggerSource::TransientSample { .. })
                || self.detector_owner != Some(self.detector_binding(kind, config)))
        {
            self.cancel_detector();
            true
        } else {
            false
        }
    }

    pub(super) fn detector_level(&self) -> Option<f32> {
        let binding = self.detector_owner?;
        self.live
            .as_ref()
            .and_then(|live| live.frame)
            .filter(|frame| frame.kind == binding.kind)
            .and_then(|frame| frame.detector)
            .filter(|reading| {
                reading.settings == binding.settings
                    && reading.level.is_finite()
                    && reading.level >= 0.0
            })
            .map(|reading| reading.level)
    }
    pub(super) fn draw_detector(
        &self,
        pm: &mut tiny_skia::Pixmap,
        tr: &mut widgets::TextRenderer,
        effect: &TrackEffect,
        geometry: EditorGeometry,
    ) {
        if !self.detector_open {
            return;
        }
        let Some(binding) = self.detector_owner else {
            return;
        };
        let layout = detector_layout(geometry);
        let (x, y, w, h) = layout.rect;
        let s = layout.scale;
        widgets::draw_rect(pm, x, y, w, h, widgets::color_bg());
        widgets::draw_rect_outline(pm, x, y, w, h, widgets::color_accent(), 1.0);
        tr.draw_text(
            pm,
            x + 12.0 * s,
            y + 23.0 * s,
            "Audio detector",
            (15.0 * s).max(10.0),
            widgets::color_text(),
        );
        let r = layout.close;
        widgets::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "X", false, false);
        let r = layout.input;
        let label = match binding.settings.input {
            DetectorInput::Main => "Input: Main",
            DetectorInput::Sidechain => "Input: Sidechain",
        };
        widgets::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, label, false, false);
        for (field, r) in DetectorField::ALL.into_iter().zip(layout.fields) {
            let value = field.value(binding.settings);
            let editing = self.text_edit.active_for(&EffectHit::Detector(field));
            let font = (r.3 * 0.5).max(10.0);
            let edit_width = (tr.text_width("-999.99", font) - 2.0).max(0.0);
            let editing = editing.map(|mut text| {
                while tr.text_width(text, font) > edit_width && !text.is_empty() {
                    let first = text.chars().next().map_or(0, char::len_utf8);
                    text = &text[first..];
                }
                text
            });
            widgets::draw_slider(
                pm,
                tr,
                r.0,
                r.1,
                r.2,
                r.3,
                field.label(),
                &field.display(value),
                field.norm(value),
                editing,
                self.text_edit.caret_visible(),
            );
        }
        let reading = (binding.kind == effect.kind)
            .then(|| self.detector_level())
            .flatten();
        let label = reading.map_or_else(
            || "Waiting for audio".into(),
            |reading| {
                if reading == 0.0 {
                    "Detector: -inf dB".into()
                } else {
                    format!("Detector: {:.1} dB", 20.0 * reading.log10())
                }
            },
        );
        tr.draw_text(
            pm,
            x + 12.0 * s,
            y + 212.0 * s,
            &label,
            (12.0 * s).max(10.0),
            widgets::color_text(),
        );
        let r = layout.meter;
        widgets::draw_rect(pm, r.0, r.1, r.2, r.3, widgets::color_control_bg());
        if let Some(reading) = reading {
            let db = if reading > 0.0 {
                20.0 * reading.log10()
            } else {
                -80.0
            };
            widgets::draw_rect(
                pm,
                r.0,
                r.1,
                r.2 * ((db + 80.0) / 80.0).clamp(0.0, 1.0),
                r.3,
                widgets::color_accent(),
            );
        }
        if let Some(level) = binding.settings.level_db {
            let marker = r.0 + r.2 * ((level + 80.0) / 80.0).clamp(0.0, 1.0);
            widgets::draw_rect(
                pm,
                marker,
                r.1 - 2.0 * s,
                1.0,
                r.3 + 4.0 * s,
                widgets::color_text(),
            );
        }
        tr.draw_text(
            pm,
            x + 12.0 * s,
            y + 245.0 * s,
            if self.listen_label() == "Listen" {
                "L: listen to filtered input"
            } else {
                "Audio monitoring unavailable"
            },
            (11.0 * s).max(10.0),
            widgets::color_muted(),
        );
        let r = layout.reset;
        widgets::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "Reset", false, false);
        let r = layout.listen;
        widgets::draw_button(
            pm,
            tr,
            r.0,
            r.1,
            r.2,
            r.3,
            self.listen_label(),
            self.listening(crate::modulation::AuditionSource::Trigger),
            false,
        );
    }
}

impl Session<'_> {
    pub(super) fn open_detector(&mut self) {
        if !matches!(
            self.modulation.trigger,
            TriggerSource::TransientSample { .. }
        ) {
            return;
        }
        self.state.detector_open = true;
        self.state.detector_owner = Some(
            self.state
                .detector_binding(self.effect.kind, self.modulation),
        );
    }
    pub(super) fn apply_detector(&mut self, field: DetectorField, value: Option<f32>) {
        let mut settings = self.modulation.audio_trigger.normalized();
        field.set(&mut settings, value);
        self.apply_detector_settings(settings);
    }

    fn apply_detector_settings(&mut self, settings: AudioTriggerSettings) {
        if !matches!(
            self.modulation.trigger,
            TriggerSource::TransientSample { .. }
        ) {
            return;
        }
        let settings = settings.normalized();
        if settings != self.modulation.audio_trigger {
            self.modulation.audio_trigger = settings;
            self.response.changed = true;
        }
        if self.state.detector_open {
            self.state.detector_owner = Some(
                self.state
                    .detector_binding(self.effect.kind, self.modulation),
            );
        }
        self.refresh_listening_settings();
    }
    pub(super) fn detector_norm(&self, field: DetectorField) -> f32 {
        field.norm(field.value(self.modulation.audio_trigger.normalized()))
    }
    pub(super) fn apply_detector_norm(&mut self, field: DetectorField, norm: f32) {
        if norm != self.detector_norm(field) {
            self.apply_detector(field, field.value_at_norm(norm));
        }
    }
    pub(super) fn commit_detector_text(&mut self, field: DetectorField, text: &str) {
        if let Some(value) = field.parse(text) {
            self.apply_detector(field, value);
        }
    }
    pub(super) fn handle_detector_event(&mut self, event: &baseview::Event) -> bool {
        use baseview::{Event, MouseButton, MouseEvent, WindowEvent};
        use keyboard_types::{Key, KeyState, Modifiers};
        if !self.state.detector_open || matches!(event, Event::Window(WindowEvent::Unfocused)) {
            return false;
        }
        let layout = detector_layout(EditorGeometry {
            layout: self.layout,
            scale: self.scale_factor,
            window_size: (self.physical_width, self.physical_height),
        });
        let hit = |p: (f32, f32)| {
            DetectorField::ALL
                .into_iter()
                .zip(layout.fields)
                .find_map(|(field, r)| view::in_rect(r, p.0, p.1).then_some(field))
        };
        self.response.captured = true;
        match event {
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.state.mouse_pos = self.mouse_pos;
                self.state
                    .effect_dial_drag
                    .set_mouse(self.mouse_pos.0, self.mouse_pos.1);
                if let Some(EffectHit::Detector(field)) =
                    self.state.effect_dial_drag.active_action().copied()
                {
                    let current = self.detector_norm(field);
                    if let Some(norm) = self
                        .state
                        .effect_dial_drag
                        .update_drag(modifiers.contains(Modifiers::SHIFT), current)
                    {
                        self.apply_detector_norm(field, norm);
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => {
                self.commit_pending_text();
                if let Some(field) = hit(self.mouse_pos) {
                    let action = EffectHit::Detector(field);
                    if self.state.effect_click.check_and_update(action) {
                        self.apply_detector(field, None);
                    } else {
                        let norm = self.detector_norm(field);
                        self.state.effect_dial_drag.begin_drag(action, norm, false);
                    }
                } else if view::in_rect(layout.input, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.effect_dial_drag.end_drag();
                    let mut settings = self.modulation.audio_trigger;
                    settings.input = match settings.input {
                        DetectorInput::Main => DetectorInput::Sidechain,
                        DetectorInput::Sidechain => DetectorInput::Main,
                    };
                    self.apply_detector_settings(settings);
                    self.state.effect_click = ClickTracker::new();
                } else if view::in_rect(layout.listen, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.effect_dial_drag.end_drag();
                    self.toggle_listening(crate::modulation::AuditionSource::Trigger);
                } else if view::in_rect(layout.reset, self.mouse_pos.0, self.mouse_pos.1) {
                    self.apply_detector_settings(AudioTriggerSettings {
                        input: self.modulation.audio_trigger.input,
                        ..AudioTriggerSettings::default()
                    });
                } else if view::in_rect(layout.close, self.mouse_pos.0, self.mouse_pos.1)
                    || !view::in_rect(layout.rect, self.mouse_pos.0, self.mouse_pos.1)
                {
                    self.state.cancel_detector();
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                if self.state.effect_dial_drag.active_action().is_none() {
                    self.commit_pending_text();
                    if let Some(field) = hit(self.mouse_pos) {
                        let text = field
                            .value(self.modulation.audio_trigger.normalized())
                            .map_or_else(|| "Off".into(), |value| value.to_string());
                        self.state
                            .text_edit
                            .begin(EffectHit::Detector(field), &text);
                        self.state.effect_click = ClickTracker::new();
                    } else if !view::in_rect(layout.input, self.mouse_pos.0, self.mouse_pos.1) {
                        self.state.cancel_detector();
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                self.state.effect_dial_drag.end_drag();
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Middle,
                ..
            }) => {
                self.commit_pending_text();
                if !view::in_rect(layout.rect, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.cancel_detector();
                }
            }
            Event::Keyboard(event) if event.state == KeyState::Down => {
                if self.state.text_edit.is_active() {
                    match &event.key {
                        Key::Character(text)
                            if !event
                                .modifiers
                                .intersects(Modifiers::CONTROL | Modifiers::META) =>
                        {
                            for c in text.chars() {
                                self.state.text_edit.insert_unit_char(c);
                            }
                        }
                        Key::Backspace => self.state.text_edit.backspace(),
                        Key::Enter => self.commit_pending_text(),
                        Key::Escape => self.state.text_edit.cancel(),
                        _ => {}
                    }
                } else if self.state.effect_dial_drag.active_action().is_none()
                    && !event
                        .modifiers
                        .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
                    && matches!(&event.key, Key::Character(text) if text.eq_ignore_ascii_case("l"))
                {
                    self.toggle_listening(crate::modulation::AuditionSource::Trigger);
                } else if event.key == Key::Escape {
                    self.state.cancel_detector();
                }
            }
            _ => {}
        }
        true
    }
}
