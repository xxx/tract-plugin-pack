use super::*;
use crate::modulation::{MAX_AUDIO_DELAY_MS, MAX_RETRIGGER_BLEND_MS, TriggerTiming};
use baseview::{Event, MouseButton, MouseEvent, WindowEvent};
use keyboard_types::{Key, KeyState, Modifiers};

type Rect = (f32, f32, f32, f32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Delay,
    Blend,
}
impl Field {
    const ALL: [Self; 2] = [Self::Delay, Self::Blend];
    fn max(self) -> f32 {
        match self {
            Self::Delay => MAX_AUDIO_DELAY_MS,
            Self::Blend => MAX_RETRIGGER_BLEND_MS,
        }
    }
    fn value(self, settings: TriggerTiming) -> f32 {
        match self {
            Self::Delay => settings.audio_delay_ms,
            Self::Blend => settings.blend_ms,
        }
    }
    fn min(self, lookahead: bool) -> f32 {
        if self == Self::Delay && lookahead {
            -MAX_AUDIO_DELAY_MS
        } else {
            0.0
        }
    }
    fn norm(self, settings: TriggerTiming, lookahead: bool) -> f32 {
        (self.value(settings) - self.min(lookahead)) / (self.max() - self.min(lookahead))
    }
    fn label(self) -> &'static str {
        match self {
            Self::Delay => "Audio shift",
            Self::Blend => "Retrigger blend",
        }
    }
    fn set(self, settings: &mut TriggerTiming, value: f32) {
        match self {
            Self::Delay => settings.audio_delay_ms = value,
            Self::Blend => settings.blend_ms = value,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
struct Binding {
    kind: EffectKind,
    settings: TriggerTiming,
    lookahead: bool,
    source: (usize, [u32; 2]),
    detector: Option<(
        crate::modulation::AudioTriggerSettings,
        crate::modulation::TransientSettings,
    )>,
    live: Option<(usize, usize, u64)>,
}
pub(super) struct Panel {
    owner: Option<Binding>,
    text: widgets::TextEditState<Field>,
    drag: widgets::DragState<Field>,
    click: ClickTracker<Field>,
    start: Option<TriggerTiming>,
    focus: usize,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            owner: None,
            text: widgets::TextEditState::new(),
            drag: widgets::DragState::new(),
            click: ClickTracker::new(),
            start: None,
            focus: 0,
        }
    }
}
impl Panel {
    pub(super) fn open(&self) -> bool {
        self.owner.is_some()
    }
    pub(super) fn close(&mut self) {
        self.owner = None;
        self.text.cancel();
        self.drag.end_drag();
        self.start = None;
    }
    pub(super) fn dragging(&self) -> bool {
        self.drag.active_action().is_some()
    }
    pub(super) fn suspend(&mut self) {
        self.text.cancel();
        self.drag.end_drag();
        self.start = None;
    }
}
pub(super) struct Layout {
    pub rect: Rect,
    pub fields: [Rect; 2],
    pub lookahead: Rect,
    pub close: Rect,
    pub reset: Rect,
    pub usable: bool,
    scale: f32,
}
pub(super) fn layout(geometry: EditorGeometry) -> Layout {
    let (ww, wh) = geometry.window_size;
    let s = geometry
        .scale
        .min(((ww - 8.0) / 392.0).max(0.01))
        .min(((wh - 8.0) / 320.0).max(0.01))
        .max(0.01);
    let (w, h) = (392.0 * s, 320.0 * s);
    let x = ((ww - w) * 0.5).max(0.0);
    let y = ((wh - h) * 0.5).max(0.0);
    Layout {
        rect: (x, y, w, h),
        fields: std::array::from_fn(|i| {
            (
                x + 12.0 * s,
                y + (98.0 + 44.0 * i as f32) * s,
                w - 24.0 * s,
                34.0 * s,
            )
        }),
        close: (x + w - 32.0 * s, y + 8.0 * s, 24.0 * s, 20.0 * s),
        lookahead: (x + 12.0 * s, y + 54.0 * s, w - 24.0 * s, 34.0 * s),
        reset: (x + 12.0 * s, y + 286.0 * s, 88.0 * s, 24.0 * s),
        usable: s >= geometry.scale.max(0.5),
        scale: s,
    }
}
impl EffectEditor {
    fn timing_binding(&self, effect: &TrackEffect, cfg: &TrackModulation) -> Binding {
        Binding {
            kind: effect.kind,
            settings: cfg.trigger_timing.normalized_with_lookahead(cfg.lookahead),
            lookahead: cfg.lookahead,
            source: (
                view::trigger_to_item(cfg.trigger),
                match cfg.trigger {
                    TriggerSource::FreeHz { hz } | TriggerSource::FreeHzSample { hz } => {
                        [hz.to_bits(), 0]
                    }
                    TriggerSource::Transient { threshold, hold_ms }
                    | TriggerSource::TransientSample { threshold, hold_ms } => {
                        [threshold.to_bits(), hold_ms.to_bits()]
                    }
                    TriggerSource::MidiNote { note, channel } => {
                        [note.map_or(256, u32::from), channel.map_or(256, u32::from)]
                    }
                    TriggerSource::MidiPitch { transpose, channel } => {
                        [transpose as i32 as u32, channel.map_or(256, u32::from)]
                    }
                    _ => [0; 2],
                },
            ),
            detector: matches!(cfg.trigger, TriggerSource::TransientSample { .. }).then(|| {
                (
                    cfg.audio_trigger.normalized(),
                    cfg.detector_response.normalized(),
                )
            }),
            live: self.live.as_ref().map(|v| v.binding_key()),
        }
    }
    pub fn timing_panel_open(&self) -> bool {
        self.timing.open()
    }
    pub(super) fn check_timing_binding(
        &mut self,
        effect: &TrackEffect,
        cfg: &TrackModulation,
    ) -> bool {
        if self.timing.open() && self.timing.owner != Some(self.timing_binding(effect, cfg)) {
            self.timing.close();
            true
        } else {
            false
        }
    }
    pub(super) fn draw_timing_popup(
        &self,
        pm: &mut tiny_skia::Pixmap,
        tr: &mut widgets::TextRenderer,
        geometry: EditorGeometry,
    ) {
        let Some(owner) = self.timing.owner else {
            return;
        };
        let lay = layout(geometry);
        let (x, y, w, h) = lay.rect;
        let s = lay.scale;
        widgets::draw_rect(pm, x, y, w, h, widgets::color_bg());
        widgets::draw_rect_outline(pm, x, y, w, h, widgets::color_accent(), 1.0);
        if !lay.usable {
            let font = (20.0 * geometry.scale).max(10.0);
            let lh = font * 1.2;
            for (i, line) in [
                "Enlarge",
                "the window",
                "for timing",
                "setup",
                "Click / Esc",
            ]
            .iter()
            .enumerate()
            {
                let width = tr.text_width(line, font);
                tr.draw_text(
                    pm,
                    (x + (w - width) * 0.5).max(x + 2.0),
                    y + (h - 5.0 * lh).max(0.0) * 0.5 + font + i as f32 * lh,
                    line,
                    font,
                    widgets::color_text(),
                );
            }
            return;
        }
        tr.draw_text(
            pm,
            x + 12.0 * s,
            y + 23.0 * s,
            "Trigger timing",
            (15.0 * s).max(10.0),
            widgets::color_text(),
        );
        let r = lay.close;
        widgets::draw_button(pm, tr, r.0, r.1, r.2, r.3, "X", false, false);
        let r = lay.lookahead;
        widgets::draw_button(
            pm,
            tr,
            r.0,
            r.1,
            r.2,
            r.3,
            if owner.lookahead {
                "Lookahead: 4 ms"
            } else {
                "Lookahead: Off"
            },
            owner.lookahead,
            false,
        );
        if self.timing.focus == 2 {
            widgets::draw_rect_outline(pm, r.0, r.1, r.2, r.3, widgets::color_accent(), 1.0);
        }
        for (field, r) in Field::ALL.into_iter().zip(lay.fields) {
            let value = field.value(owner.settings);
            let font = (r.3 * 0.5).max(10.0);
            let edit = self.timing.text.active_for(&field).map(|mut text| {
                let max = tr.text_width("-999.99", font);
                while !text.is_empty() && tr.text_width(text, font) > max {
                    text = &text[text.chars().next().map_or(0, char::len_utf8)..];
                }
                text
            });
            let label = if value == 0.0 && !(field == Field::Delay && owner.lookahead) {
                "Off".into()
            } else {
                format!("{value:.2} ms")
            };
            widgets::draw_slider(
                pm,
                tr,
                r.0,
                r.1,
                r.2,
                r.3,
                field.label(),
                &label,
                field.norm(owner.settings, owner.lookahead),
                edit,
                self.timing.text.caret_visible(),
            );
            if self.timing.focus == usize::from(field == Field::Blend) {
                widgets::draw_rect_outline(pm, r.0, r.1, r.2, r.3, widgets::color_accent(), 1.0);
            }
        }
        for (offset, line) in [
            (201.0, "Shift/lookahead: Transient (sample)"),
            (224.0, "Adds 4 ms; FX latency stays separate"),
            (247.0, "Blend skips enums / Follow / DAW"),
        ] {
            tr.draw_text(
                pm,
                x + 12.0 * s,
                y + offset * s,
                line,
                (11.0 * s).max(10.0),
                widgets::color_muted(),
            );
        }
        let r = lay.reset;
        widgets::draw_button(pm, tr, r.0, r.1, r.2, r.3, "Reset", false, false);
        if self.timing.focus == 3 {
            widgets::draw_rect_outline(pm, r.0, r.1, r.2, r.3, widgets::color_accent(), 1.0);
        }
    }
}
impl Session<'_> {
    pub(super) fn open_timing(&mut self) {
        self.commit_pending_text();
        self.commit_follower_text();
        self.state.cancel_detector();
        self.state.follower.close();
        self.state.wave_midi.close();
        self.state.mseg_edit.cancel_interaction();
        self.state.effect_dropdown.close();
        self.state.timing.close();
        self.state.timing.owner = Some(self.state.timing_binding(self.effect, self.modulation));
        self.state.timing.focus = 0;
        self.state
            .timing
            .drag
            .set_input_scale(self.state.effect_dial_drag.input_scale());
        self.state
            .timing
            .drag
            .set_mouse(self.mouse_pos.0, self.mouse_pos.1);
    }
    fn apply_timing(&mut self, settings: TriggerTiming) {
        let settings = settings.normalized_with_lookahead(self.modulation.lookahead);
        if settings != self.modulation.trigger_timing {
            self.modulation.trigger_timing = settings;
            self.response.changed = true;
        }
        self.state.timing.owner = Some(self.state.timing_binding(self.effect, self.modulation));
    }
    fn apply_timing_field(&mut self, field: Field, value: f32) {
        let mut settings = self.modulation.trigger_timing;
        field.set(&mut settings, value);
        self.apply_timing(settings);
    }
    fn toggle_lookahead(&mut self) {
        self.state.timing.drag.end_drag();
        self.state.timing.start = None;
        self.modulation.lookahead = !self.modulation.lookahead;
        self.response.changed = true;
        self.apply_timing(self.modulation.trigger_timing);
        self.state.timing.click = ClickTracker::new();
    }
    fn reset_timing(&mut self) {
        if self.modulation.lookahead {
            self.modulation.lookahead = false;
            self.response.changed = true;
        }
        self.apply_timing(TriggerTiming::default());
    }
    pub(super) fn commit_timing_text(&mut self) {
        if let Some((field, text)) = self.state.timing.text.commit() {
            let value = if text.trim().eq_ignore_ascii_case("off") {
                Some(0.0)
            } else {
                effects::parse_value(
                    &text,
                    effects::ParamFormat::Number {
                        decimals: 2,
                        unit: "ms",
                    },
                )
            };
            if let Some(value) = value.filter(|v| {
                v.is_finite() && (field.min(self.modulation.lookahead)..=field.max()).contains(v)
            }) {
                self.apply_timing_field(field, value);
            } else {
                self.state
                    .mseg_edit
                    .set_status_message("Invalid trigger timing value");
            }
        }
    }
    fn timing_edit(&mut self, field: Field) {
        self.state.timing.click = ClickTracker::new();
        self.state.timing.focus = usize::from(field == Field::Blend);
        self.state.timing.text.begin(
            field,
            &field.value(self.modulation.trigger_timing).to_string(),
        );
    }
    pub(super) fn handle_timing_event(&mut self, event: &Event) -> bool {
        if !self.state.timing.open() {
            return false;
        }
        self.response.captured = true;
        let lay = layout(EditorGeometry {
            layout: self.layout,
            scale: self.scale_factor,
            window_size: (self.physical_width, self.physical_height),
        });
        if !lay.usable {
            self.state.timing.suspend();
            match event {
                Event::Mouse(MouseEvent::ButtonPressed { .. }) => self.state.timing.close(),
                Event::Keyboard(k) if k.state == KeyState::Down && k.key == Key::Escape => {
                    self.state.timing.close()
                }
                Event::Window(WindowEvent::Unfocused | WindowEvent::Resized(_)) => {
                    self.state.timing.close()
                }
                _ => {}
            }
            return true;
        }
        let hit = |point: (f32, f32)| {
            Field::ALL
                .into_iter()
                .zip(lay.fields)
                .find_map(|(f, r)| view::in_rect(r, point.0, point.1).then_some(f))
        };
        match event {
            Event::Window(WindowEvent::Unfocused) => {
                self.commit_timing_text();
                self.state.timing.close();
            }
            Event::Window(WindowEvent::Resized(_)) => self.state.timing.close(),
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.state.mouse_pos = self.mouse_pos;
                self.state
                    .timing
                    .drag
                    .set_mouse(self.mouse_pos.0, self.mouse_pos.1);
                if let Some(field) = self.state.timing.drag.active_action().copied() {
                    let norm =
                        field.norm(self.modulation.trigger_timing, self.modulation.lookahead);
                    if let Some(norm) = self
                        .state
                        .timing
                        .drag
                        .update_drag(modifiers.contains(Modifiers::SHIFT), norm)
                    {
                        let min = field.min(self.modulation.lookahead);
                        self.apply_timing_field(field, min + norm * (field.max() - min));
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => {
                self.commit_timing_text();
                if view::in_rect(lay.close, self.mouse_pos.0, self.mouse_pos.1)
                    || !view::in_rect(lay.rect, self.mouse_pos.0, self.mouse_pos.1)
                {
                    self.state.timing.close();
                } else if view::in_rect(lay.lookahead, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.timing.focus = 2;
                    self.toggle_lookahead();
                } else if view::in_rect(lay.reset, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.timing.drag.end_drag();
                    self.reset_timing();
                } else if let Some(field) = hit(self.mouse_pos) {
                    self.state.timing.focus = usize::from(field == Field::Blend);
                    if self.state.timing.click.check_and_update(field) {
                        self.apply_timing_field(field, 0.0);
                    } else {
                        self.state.timing.start = Some(self.modulation.trigger_timing);
                        let norm =
                            field.norm(self.modulation.trigger_timing, self.modulation.lookahead);
                        self.state.timing.drag.begin_drag(field, norm, false);
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                self.state.timing.drag.end_drag();
                self.state.timing.start = None;
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                if !self.state.timing.dragging() {
                    self.commit_timing_text();
                    if let Some(field) = hit(self.mouse_pos) {
                        self.timing_edit(field);
                        self.state.timing.click = ClickTracker::new();
                    } else {
                        self.state.timing.close();
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Middle,
                ..
            }) => {
                self.commit_timing_text();
                if !view::in_rect(lay.rect, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.timing.close();
                }
            }
            Event::Keyboard(key) if key.state == KeyState::Down => {
                if key
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
                {
                    return true;
                }
                if self.state.timing.text.is_active() {
                    match &key.key {
                        Key::Escape => self.state.timing.text.cancel(),
                        Key::Enter => self.commit_timing_text(),
                        Key::Backspace => self.state.timing.text.backspace(),
                        Key::Character(text) => {
                            for c in text.chars() {
                                self.state.timing.text.insert_unit_char(c);
                            }
                        }
                        Key::Tab => {
                            self.commit_timing_text();
                            self.state.timing.focus = (self.state.timing.focus
                                + if key.modifiers.contains(Modifiers::SHIFT) {
                                    3
                                } else {
                                    1
                                })
                                % 4;
                        }
                        _ => {}
                    }
                    return true;
                }
                if self.state.timing.dragging() {
                    if key.key == Key::Escape {
                        if let Some(start) = self.state.timing.start.take() {
                            self.apply_timing(start);
                        }
                        self.state.timing.drag.end_drag();
                    }
                    return true;
                }
                let focus = self.state.timing.focus;
                match &key.key {
                    Key::Escape => self.state.timing.close(),
                    Key::Tab => {
                        self.state.timing.focus = (focus
                            + if key.modifiers.contains(Modifiers::SHIFT) {
                                3
                            } else {
                                1
                            })
                            % 4
                    }
                    Key::ArrowDown => self.state.timing.focus = (focus + 1) % 4,
                    Key::ArrowUp => self.state.timing.focus = (focus + 3) % 4,
                    Key::Home => self.state.timing.focus = 0,
                    Key::End => self.state.timing.focus = 3,
                    Key::Enter => {
                        if focus < 2 {
                            self.timing_edit(Field::ALL[focus]);
                        } else if focus == 2 {
                            self.toggle_lookahead();
                        } else {
                            self.reset_timing();
                        }
                    }
                    Key::ArrowLeft | Key::ArrowRight if focus < 2 => {
                        let field = Field::ALL[focus];
                        let delta = if key.modifiers.contains(Modifiers::SHIFT) {
                            0.001
                        } else {
                            0.01
                        } * if key.key == Key::ArrowLeft { -1.0 } else { 1.0 };
                        let min = field.min(self.modulation.lookahead);
                        let value = (field.value(self.modulation.trigger_timing)
                            + delta * (field.max() - min))
                            .clamp(min, field.max());
                        self.apply_timing_field(field, value);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_audio_shift_and_unsigned_blend_map_to_distinct_slider_ranges() {
        let settings = TriggerTiming {
            audio_delay_ms: 1.25,
            blend_ms: 7.5,
        };
        assert_eq!(Field::Delay.label(), "Audio shift");
        assert_eq!(Field::Blend.label(), "Retrigger blend");
        assert_eq!(Field::Delay.value(settings), 1.25);
        assert_eq!(Field::Blend.value(settings), 7.5);
        assert_eq!(Field::Delay.norm(settings, false), 0.3125);
        assert_eq!(Field::Delay.norm(settings, true), 0.65625);
        assert_eq!(Field::Blend.norm(settings, false), 0.375);
        assert_eq!(Field::Blend.norm(settings, true), 0.375);
        let mut settings = settings;
        Field::Delay.set(&mut settings, -1.5);
        assert_eq!(Field::Delay.norm(settings, true), 0.3125);
    }
}
