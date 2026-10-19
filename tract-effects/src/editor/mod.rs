//! Effect and MSEG editing independent of plugin window and sequencer state.

use crate::effects::{self, EffectKind, MAX_EFFECT_PARAMS, ParamSpec, TrackEffect};
use crate::modulation::{ModulationTarget, TrackModulation, TriggerSource};
use tiny_skia_widgets as widgets;

pub mod view;
use view as effect_editor;
use view::{EffectHit, EffectLayout};

/// A trigger menu entry; DSP settings remain in `TriggerSource`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerChoice {
    Free,
    CellLight,
    CellStep,
    FreeHz,
    Transient,
}

impl TriggerChoice {
    pub const ALL: &'static [Self] = &[
        Self::Free,
        Self::CellLight,
        Self::CellStep,
        Self::FreeHz,
        Self::Transient,
    ];

    fn item(self) -> usize {
        match self {
            Self::Free => 0,
            Self::CellLight => 1,
            Self::CellStep => 2,
            Self::FreeHz => 3,
            Self::Transient => 4,
        }
    }
}

/// Host-supplied geometry, in physical pixels except for the scale factor.
#[derive(Clone, Copy, Debug)]
pub struct EditorGeometry {
    pub layout: EffectLayout,
    pub scale: f32,
    pub window_size: (f32, f32),
}

/// One input event's edits and navigation requests. The host owns persistence,
/// undo grouping, randomization policy, and audio-thread notifications.
#[derive(Default, Debug)]
pub struct EditorResponse {
    pub captured: bool,
    pub changed: bool,
    pub back: bool,
    pub randomize: bool,
}

/// Host-owned configuration and rendering context for one input event.
pub struct EditorContext<'a> {
    pub pointer_position: (f32, f32),
    pub effect: &'a mut TrackEffect,
    pub modulation: &'a mut TrackModulation,
    pub text_renderer: &'a mut widgets::TextRenderer,
    pub geometry: EditorGeometry,
}

/// All interaction state for one effect editor; it does not own plugin state.
pub struct EffectEditor {
    selected_mseg: usize,
    effect_dropdown: widgets::dropdown::DropdownState<EffectAction>,
    effect_dial_drag: widgets::DragState<EffectHit>,
    text_edit: widgets::TextEditState<EffectHit>,
    mseg_edit: widgets::mseg::MsegEditState,
    mseg_last_click_time: std::time::Instant,
    mseg_last_click_pos: (f32, f32),
    effect_click: ClickTracker<EffectHit>,
    mouse_pos: (f32, f32),
    trigger_choices: &'static [TriggerChoice],
}

impl Default for EffectEditor {
    fn default() -> Self {
        Self::new(TriggerChoice::ALL)
    }
}

impl EffectEditor {
    pub fn new(trigger_choices: &'static [TriggerChoice]) -> Self {
        Self {
            selected_mseg: 1,
            effect_dropdown: widgets::dropdown::DropdownState::new(),
            effect_dial_drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            mseg_edit: widgets::mseg::MsegEditState::new(),
            mseg_last_click_time: std::time::Instant::now(),
            mseg_last_click_pos: (-999.0, -999.0),
            effect_click: ClickTracker::new(),
            mouse_pos: (0.0, 0.0),
            trigger_choices,
        }
    }

    pub fn clear_selection(&mut self) {
        self.mseg_edit.clear_selection();
    }

    /// Cancel gestures tied to configuration that the host has replaced.
    pub fn cancel_interaction(&mut self) {
        let selected_mseg = self.selected_mseg;
        *self = Self::new(self.trigger_choices);
        self.selected_mseg = selected_mseg;
    }

    /// Commit a typed value before focus moves to a host-owned control.
    pub fn finish_text_edit(&mut self, context: EditorContext<'_>) -> EditorResponse {
        let mut session = Session::new(self, context);
        session.finalize_dial_edit_for_press(f32::NEG_INFINITY, f32::NEG_INFINITY);
        session.response
    }

    pub fn captures_keyboard(&self) -> bool {
        self.effect_dropdown.is_open() || self.text_edit.is_active()
    }

    pub fn dropdown_open(&self) -> bool {
        self.effect_dropdown.is_open()
    }

    pub fn handle_event(
        &mut self,
        event: &baseview::Event,
        context: EditorContext<'_>,
    ) -> EditorResponse {
        let mut session = Session::new(self, context);
        session.handle_event(event);
        session.response
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pixmap: &mut tiny_skia::Pixmap,
        tr: &mut widgets::TextRenderer,
        effect: &TrackEffect,
        modulation: &TrackModulation,
        phases: [f32; 4],
        geometry: EditorGeometry,
    ) {
        let mut effect = *effect;
        let mut modulation = modulation.clone();
        let pointer_position = self.mouse_pos;
        let mut session = Session::new(
            self,
            EditorContext {
                effect: &mut effect,
                modulation: &mut modulation,
                text_renderer: tr,
                geometry,
                pointer_position,
            },
        );
        session.phases = phases;
        session.draw_effect_view(pixmap);
    }

    pub fn draw_popup(
        &self,
        pixmap: &mut tiny_skia::Pixmap,
        tr: &mut widgets::TextRenderer,
        effect: &TrackEffect,
        geometry: EditorGeometry,
    ) {
        if self.effect_dropdown.is_open() {
            let items = dropdown_items(self, effect);
            widgets::dropdown::draw_dropdown_popup(
                pixmap,
                tr,
                &self.effect_dropdown,
                &items,
                geometry.window_size,
            );
        }
    }
}

struct Session<'a> {
    state: &'a mut EffectEditor,
    effect: &'a mut TrackEffect,
    modulation: &'a mut TrackModulation,
    text_renderer: &'a mut widgets::TextRenderer,
    layout: EffectLayout,
    scale_factor: f32,
    physical_width: f32,
    physical_height: f32,
    mouse_pos: (f32, f32),
    phases: [f32; 4],
    response: EditorResponse,
}

impl<'a> Session<'a> {
    fn new(state: &'a mut EffectEditor, context: EditorContext<'a>) -> Self {
        state.mouse_pos = context.pointer_position;
        state
            .effect_dial_drag
            .set_mouse(context.pointer_position.0, context.pointer_position.1);
        Self {
            mouse_pos: context.pointer_position,
            state,
            effect: context.effect,
            modulation: context.modulation,
            text_renderer: context.text_renderer,
            layout: context.geometry.layout,
            scale_factor: context.geometry.scale,
            physical_width: context.geometry.window_size.0,
            physical_height: context.geometry.window_size.1,
            phases: [0.0; 4],
            response: EditorResponse::default(),
        }
    }

    fn effect_config(&self) -> TrackEffect {
        *self.effect
    }
    fn modulation_config(&self) -> &TrackModulation {
        self.modulation
    }

    fn trigger_items(&self) -> Vec<&'static str> {
        let all = view::trigger_items();
        self.state
            .trigger_choices
            .iter()
            .map(|c| all[c.item()])
            .collect()
    }

    fn trigger_index(&self, source: TriggerSource) -> usize {
        self.state
            .trigger_choices
            .iter()
            .position(|c| c.item() == view::trigger_to_item(source))
            .unwrap_or(0)
    }

    fn dropdown_items(&self) -> Vec<&'static str> {
        dropdown_items(self.state, self.effect)
    }

    fn apply_selection(&mut self, action: EffectAction, idx: usize) {
        match action {
            EffectAction::Kind => {
                self.apply_kind_switch(EffectKind::ALL[idx.min(EffectKind::ALL.len() - 1)])
            }
            EffectAction::Target => self.apply_target_selection(idx),
            EffectAction::Trigger => self.apply_trigger_selection(idx),
            EffectAction::ParamDropdown(i) => self.apply_param_dropdown_selection(i, idx),
        }
    }

    fn handle_event(&mut self, event: &baseview::Event) {
        use baseview::{Event, MouseButton, MouseEvent};
        use keyboard_types::{Key, KeyState, Modifiers};
        let win = (self.physical_width, self.physical_height);
        match event {
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                let (x, y) = (position.x as f32, position.y as f32);
                self.mouse_pos = (x, y);
                self.state.mouse_pos = (x, y);
                self.state.effect_dial_drag.set_mouse(x, y);
                let shift = modifiers.contains(Modifiers::SHIFT);
                if let Some(hit) = self.state.effect_dial_drag.active_action().copied() {
                    let current = self.normalized_for_hit(hit);
                    if let Some(norm) = self.state.effect_dial_drag.update_drag(shift, current) {
                        self.apply_drag(hit, norm);
                    }
                }
                if self.state.effect_dropdown.is_open() {
                    let items = self.dropdown_items();
                    self.state.effect_dropdown.on_mouse_move(x, y, &items, win);
                }
                let sel = self.state.selected_mseg.min(3);
                if self.state.mseg_edit.on_mouse_move(
                    x,
                    y,
                    &mut self.modulation.msegs[sel],
                    self.layout.mseg_pane,
                    self.scale_factor,
                    shift,
                ) == Some(widgets::mseg::MsegEdit::Changed)
                {
                    self.response.changed = true;
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers,
            }) => {
                let (x, y) = self.mouse_pos;
                self.finalize_dial_edit_for_press(x, y);
                if self.state.effect_dropdown.is_open() {
                    let items = self.dropdown_items();
                    if let Some(widgets::dropdown::DropdownEvent::Selected(action, idx)) =
                        self.state.effect_dropdown.on_mouse_down(x, y, &items, win)
                    {
                        self.apply_selection(action, idx);
                    }
                    self.response.captured = true;
                } else {
                    self.response.captured =
                        self.on_effect_press(x, y, modifiers.contains(Modifiers::CONTROL));
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                self.response.captured =
                    matches!(self.on_right_press(), baseview::EventStatus::Captured);
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                self.state.effect_dial_drag.end_drag();
                self.state.effect_dropdown.on_mouse_up();
                let sel = self.state.selected_mseg.min(3);
                if self.state.mseg_edit.on_mouse_up(
                    &mut self.modulation.msegs[sel],
                    self.layout.mseg_pane,
                    self.scale_factor,
                ) == Some(widgets::mseg::MsegEdit::Changed)
                {
                    self.response.changed = true;
                }
            }
            Event::Mouse(MouseEvent::WheelScrolled { delta, .. })
                if self.state.effect_dropdown.is_open() =>
            {
                let dy = match delta {
                    baseview::ScrollDelta::Lines { y, .. } => *y,
                    baseview::ScrollDelta::Pixels { y, .. } => *y * 0.05,
                };
                let items = self.dropdown_items();
                self.state.effect_dropdown.on_wheel(dy, &items, win);
                self.response.captured = true;
            }
            Event::Keyboard(ev) if self.state.effect_dropdown.is_open() => {
                self.response.captured = true;
                if ev.state != KeyState::Down {
                    return;
                }
                let items = self.dropdown_items();
                use widgets::dropdown::DropdownKey as D;
                let key = match ev.key {
                    Key::ArrowUp => Some(D::Up),
                    Key::ArrowDown => Some(D::Down),
                    Key::Enter => Some(D::Enter),
                    Key::Escape => Some(D::Esc),
                    Key::Backspace => Some(D::Backspace),
                    Key::PageUp => Some(D::PageUp),
                    Key::PageDown => Some(D::PageDown),
                    Key::Home => Some(D::Home),
                    Key::End => Some(D::End),
                    _ => None,
                };
                let selected = if let Some(key) = key {
                    self.state.effect_dropdown.on_key(key, &items, win)
                } else if let Key::Character(s) = &ev.key {
                    let mut last = None;
                    for c in s.chars() {
                        if let Some(event) = self.state.effect_dropdown.on_char(c, &items) {
                            last = Some(event);
                        }
                    }
                    last
                } else {
                    None
                };
                if let Some(widgets::dropdown::DropdownEvent::Selected(action, idx)) = selected {
                    self.apply_selection(action, idx);
                }
            }
            Event::Keyboard(ev) if self.state.text_edit.is_active() => {
                self.response.captured = true;
                if ev.state != KeyState::Down {
                    return;
                }
                match &ev.key {
                    Key::Character(s) => {
                        for c in s.chars() {
                            self.state.text_edit.insert_char(c);
                        }
                    }
                    Key::Backspace => self.state.text_edit.backspace(),
                    Key::Escape => self.state.text_edit.cancel(),
                    Key::Enter => match self.state.text_edit.commit() {
                        Some((EffectHit::Dial(i), text)) => self.commit_dial_text_edit(i, &text),
                        Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                        _ => {}
                    },
                    _ => self.response.captured = false,
                }
            }
            Event::Keyboard(ev)
                if ev.state == KeyState::Down && matches!(ev.key, Key::Delete | Key::Backspace) =>
            {
                let sel = self.state.selected_mseg.min(3);
                if self
                    .state
                    .mseg_edit
                    .delete_selection(&mut self.modulation.msegs[sel])
                    == Some(widgets::mseg::MsegEdit::Changed)
                {
                    self.response.changed = true;
                    self.response.captured = true;
                }
            }
            _ => {}
        }
    }

    fn normalized_for_hit(&self, hit: EffectHit) -> f32 {
        match hit {
            EffectHit::Dial(i) => self
                .param_spec(i)
                .map(|s| effects::value_to_norm(self.effect.params[i], s.min, s.max, s.scaling))
                .unwrap_or(0.0),
            EffectHit::Depth if self.state.selected_mseg != 0 => {
                ((self.modulation.depths[self.state.selected_mseg - 1] + 1.0) / 2.0).clamp(0.0, 1.0)
            }
            EffectHit::TriggerRate => match self.modulation.trigger {
                TriggerSource::FreeHz { hz } => effects::value_to_norm(
                    hz,
                    view::TRIGGER_RATE_MIN_HZ,
                    view::TRIGGER_RATE_MAX_HZ,
                    effects::ParamScaling::Log,
                ),
                TriggerSource::Transient { threshold, .. } => {
                    1.0 - effects::value_to_norm(
                        threshold,
                        crate::modulation::TRANSIENT_THRESHOLD_MIN,
                        crate::modulation::TRANSIENT_THRESHOLD_MAX,
                        effects::ParamScaling::Log,
                    )
                }
                _ => 0.5,
            },
            EffectHit::TriggerAux => match self.modulation.trigger {
                TriggerSource::Transient { hold_ms, .. } => effects::value_to_norm(
                    hold_ms,
                    crate::modulation::TRANSIENT_HOLD_MS_MIN,
                    crate::modulation::TRANSIENT_HOLD_MS_MAX,
                    effects::ParamScaling::Log,
                ),
                _ => 0.5,
            },
            EffectHit::MsegLength => self.active_mseg_length_norm(),
            EffectHit::Mix => self.effect.mix,
            _ => 0.5,
        }
    }

    fn apply_drag(&mut self, hit: EffectHit, norm: f32) {
        match hit {
            EffectHit::Dial(i) => self.apply_effect_dial(i, norm),
            EffectHit::Depth => self.apply_depth_drag(norm),
            EffectHit::TriggerRate => self.apply_trigger_rate_drag(norm),
            EffectHit::TriggerAux => self.apply_trigger_aux_drag(norm),
            EffectHit::MsegLength => self.apply_mseg_length_drag(norm),
            EffectHit::Mix => self.apply_mix(norm),
            _ => {}
        }
    }

    fn compute_modulated_norms(&self) -> view::ModulationDisplay {
        let mut result = view::ModulationDisplay::default();
        for k in 1..4 {
            let Some(target) = self.modulation.targets[k - 1] else {
                continue;
            };
            let Some(spec) = target.spec(self.effect.kind) else {
                continue;
            };
            let mseg = &self.modulation.msegs[k];
            let value = widgets::mseg::value_at_phase(mseg, self.phases[k]);
            let modulated = crate::modulation::assignable_value(
                value,
                target.base_value(self.effect),
                self.modulation.depths[k - 1],
                spec,
                mseg.polarity,
            );
            let display = Some((
                effects::value_to_norm(modulated, spec.min, spec.max, spec.scaling),
                k as u8,
            ));
            match target {
                ModulationTarget::EffectParameter(index) => result.parameters[index] = display,
                ModulationTarget::Mix => result.mix = display,
            }
        }
        result
    }
    fn on_right_press(&mut self) -> baseview::EventStatus {
        let (px, py) = self.mouse_pos;
        if self.state.effect_dial_drag.active_action().is_some() {
            return baseview::EventStatus::Captured;
        }
        // First, hit-test for a param dial — only `EffectHit::Dial(i)`
        // gets text entry. Depth, trigger, and trigger-rate stay
        // drag-only.
        let param_count = self.effect_param_count();
        let trigger = self.modulation_config().trigger;
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            &self.layout,
            param_count,
            self.state.selected_mseg,
            trigger,
        );
        if let Some(EffectHit::Dial(i)) = hit {
            // Enum params are dropdowns, not editable text fields — let the
            // right-click fall through (left-click opens the dropdown).
            if self.param_enum_labels(i).is_some() {
                return baseview::EventStatus::Ignored;
            }
            // Right-clicking a *different* dial while one is already
            // being edited would silently discard the prior edit if we
            // jumped straight to `begin` (which clears the buffer).
            // Commit-and-apply the prior edit first.
            match self.state.text_edit.commit() {
                Some((EffectHit::Dial(prev), text)) => self.commit_dial_text_edit(prev, &text),
                Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                _ => {}
            }
            if let Some(spec) = self.param_spec(i) {
                let value = self.effect_config().params[i];
                self.state.text_edit.begin(
                    EffectHit::Dial(i),
                    &effects::format_value_bare(value, spec.format),
                );
            }
            return baseview::EventStatus::Captured;
        }
        if let Some(EffectHit::Mix) = hit {
            // Commit any prior edit before seeding a new one.
            match self.state.text_edit.commit() {
                Some((EffectHit::Dial(prev), text)) => self.commit_dial_text_edit(prev, &text),
                Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                _ => {}
            }
            let pct = (self.effect_config().mix * 100.0).round() as i32;
            self.state
                .text_edit
                .begin(EffectHit::Mix, &format!("{pct}"));
            return baseview::EventStatus::Captured;
        }
        let lay = self.layout;
        if effect_editor::in_rect(lay.mseg_pane, px, py) {
            let sel = self.state.selected_mseg.min(3);
            let changed = self.state.mseg_edit.on_right_click(
                px,
                py,
                &mut self.modulation.msegs[sel],
                lay.mseg_pane,
                self.scale_factor,
            );
            if changed == Some(widgets::mseg::MsegEdit::Changed) {
                self.response.changed = true;
            }
        }
        baseview::EventStatus::Captured
    }

    fn draw_effect_view(&mut self, pixmap: &mut tiny_skia::Pixmap) {
        let track = self.effect_config();
        let lay = self.layout;
        // If a dial-text edit is active, hand the buffer + caret state to the
        // section drawer so it can render in place of the formatted value.
        let editing_dial: Option<(usize, &str, bool)> = match self.state.text_edit.active_for_any()
        {
            Some(EffectHit::Dial(i)) => {
                let caret_on = self.state.text_edit.caret_visible();
                self.state
                    .text_edit
                    .active_for(&EffectHit::Dial(i))
                    .map(|buf| (i, buf, caret_on))
            }
            _ => None,
        };
        let editing_mix: Option<(&str, bool)> = match self.state.text_edit.active_for_any() {
            Some(EffectHit::Mix) => {
                let caret_on = self.state.text_edit.caret_visible();
                self.state
                    .text_edit
                    .active_for(&EffectHit::Mix)
                    .map(|buf| (buf, caret_on))
            }
            _ => None,
        };
        // Which Enum-format param has its dropdown open, if any.
        let open_param_dropdown = (0..crate::effects::MAX_EFFECT_PARAMS).find(|&i| {
            self.state
                .effect_dropdown
                .is_open_for(EffectAction::ParamDropdown(i))
        });
        let modulated_norms = self.compute_modulated_norms();
        effect_editor::draw_effect_section(
            pixmap,
            self.text_renderer,
            &track,
            self.state.effect_dropdown.is_open_for(EffectAction::Kind),
            editing_dial,
            editing_mix,
            open_param_dropdown,
            &modulated_norms,
            self.scale_factor,
            &self.layout,
        );
        effect_editor::draw_section_header(
            pixmap,
            self.text_renderer,
            lay.effect_header,
            "EFFECT",
            self.scale_factor,
        );
        effect_editor::draw_section_header(
            pixmap,
            self.text_renderer,
            lay.modulation_header,
            "MODULATION",
            self.scale_factor,
        );
        // MODULATION section.
        let modu = self.modulation.clone();
        let sel = self.state.selected_mseg.min(3);
        // Build the hover-node tooltip for the active MSEG. Amp (slot 0) shows
        // a dB readout; assignable MSEGs format through the target's ParamSpec
        // if one is set, or fall back to the raw 0..1 level.
        let tooltip: Option<(usize, String)> = self.state.mseg_edit.hovered_node().map(|idx| {
            let (spec, base, depth_polarity) = if sel == 0 {
                (None, 0.0, None)
            } else {
                let k = sel - 1;
                let target = modu.targets[k];
                let depth = modu.depths[k];
                let polarity = modu.msegs[sel].polarity;
                let spec = target.and_then(|target| target.spec(track.kind));
                let base = target
                    .map(|target| target.base_value(&track))
                    .unwrap_or(0.0);
                (spec, base, Some((depth, polarity)))
            };
            let text =
                mseg_node_tooltip_text(sel, &modu.msegs[sel], idx, spec, base, depth_polarity);
            (idx, text)
        });
        let tooltip_ref = tooltip.as_ref().map(|(i, t)| (*i, t.as_str()));
        // Active MSEG first — `draw_mseg` opens with an opaque canvas fill
        // that would otherwise wipe the ghosts. Ghosts are then drawn on top
        // at low alpha (≈ 38%) so they read as faint context, not foreground.
        widgets::mseg::draw_mseg(
            pixmap,
            self.text_renderer,
            lay.mseg_pane,
            &modu.msegs[sel],
            &self.state.mseg_edit,
            self.scale_factor,
            mseg_color(sel),
            tooltip_ref,
        );
        for (m, mseg) in modu.msegs.iter().enumerate() {
            if m != sel {
                let c = mseg_color(m);
                // Pack the slot colour with ghost alpha (~0x60).
                let r = (c.red() * 255.0).round() as u32;
                let g = (c.green() * 255.0).round() as u32;
                let b = (c.blue() * 255.0).round() as u32;
                let packed = (r << 24) | (g << 16) | (b << 8) | 0x60;
                widgets::mseg::draw_mseg_ghost(
                    pixmap,
                    lay.mseg_pane,
                    mseg,
                    &self.state.mseg_edit,
                    self.scale_factor,
                    packed,
                );
            }
        }
        // Playhead overlay: a thin vertical line at the active MSEG's current
        // phase, drawn over the active curve + ghosts.
        let phase = self.phases[sel];
        effect_editor::draw_mseg_playhead(pixmap, lay.mseg_pane, phase, self.scale_factor);
        // Open MSEG dropdown (Grid / Style / right-click Transform menu) goes
        // last so it sits above every preceding layer — `draw_mseg` no longer
        // paints it inline because the ghost loop above would otherwise bury
        // it.
        widgets::mseg::draw_mseg_dropdown(
            pixmap,
            self.text_renderer,
            &self.state.mseg_edit,
            lay.mseg_pane,
        );
        // Selector + target + depth (when on an assignable MSEG).
        let (target, depth) = if sel == 0 {
            (None, 0.0)
        } else {
            let k = sel - 1;
            (modu.targets[k], modu.depths[k])
        };
        effect_editor::draw_modulation_controls(
            pixmap,
            self.text_renderer,
            sel,
            track.kind,
            target,
            depth,
            self.state.effect_dropdown.is_open_for(EffectAction::Target),
            self.scale_factor,
            &self.layout,
        );
        // Per-track trigger control + (conditionally) rate dial.
        let trigger = self.modulation_config().trigger;
        effect_editor::draw_trigger_controls(
            pixmap,
            self.text_renderer,
            trigger,
            self.state
                .effect_dropdown
                .is_open_for(EffectAction::Trigger),
            self.scale_factor,
            &self.layout,
        );
        // Active MSEG's sync/length controls (sit on the modulation row to
        // the right of the depth dial).
        effect_editor::draw_mseg_controls(
            pixmap,
            self.text_renderer,
            &modu.msegs[sel],
            self.scale_factor,
            &self.layout,
        );
    }

    fn on_effect_press(&mut self, px: f32, py: f32, ctrl: bool) -> bool {
        let params = self.effect_param_count();
        let trigger = self.modulation_config().trigger;
        let Some(hit) = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            &self.layout,
            params,
            self.state.selected_mseg,
            trigger,
        ) else {
            return false;
        };
        let lay = self.layout;
        match hit {
            EffectHit::Back => {
                self.response.back = true;
            }
            EffectHit::Kind => {
                let kind_items = effect_editor::kind_items();
                let current = EffectKind::ALL
                    .iter()
                    .position(|k| *k == self.effect_config().kind)
                    .unwrap_or(0);
                let win = (self.physical_width, self.physical_height);
                self.state.effect_dropdown.open(
                    EffectAction::Kind,
                    lay.kind,
                    widgets::dropdown::DropdownList::sectioned(
                        &kind_items.items,
                        &kind_items.sections,
                    ),
                    current,
                    true,
                    win,
                );
            }
            EffectHit::Randomize => {
                self.response.randomize = true;
            }
            EffectHit::Dial(i) => {
                // Enum-format params open a dropdown over the dial slot instead
                // of being draggable. Double-click reset/text-edit/drag are
                // skipped for them — a discrete selector doesn't have a
                // continuous default to reset toward, and dragging a binary
                // value is meaningless.
                if let Some(labels) = self.param_enum_labels(i) {
                    let (dx, dy, dw, dh) = lay.dials[i];
                    let trigger_h = (dh * 0.32).max(22.0 * self.scale_factor);
                    // Match the renderer's per-label width so the popup
                    // anchors flush under the visible trigger rect.
                    let spec_format = self.effect_param_format(i);
                    let trigger_w = effect_editor::enum_trigger_width_for(
                        self.text_renderer,
                        spec_format,
                        trigger_h,
                        dw,
                        self.scale_factor,
                    );
                    let trigger_x = dx + (dw - trigger_w) * 0.5;
                    let trigger_y = dy + dh * 0.46;
                    let current = self.effect_config().params[i].round() as usize;
                    let win = (self.physical_width, self.physical_height);
                    let items: Vec<&'static str> = labels.to_vec();
                    self.state.effect_dropdown.open(
                        EffectAction::ParamDropdown(i),
                        (trigger_x, trigger_y, trigger_w, trigger_h),
                        widgets::dropdown::DropdownList::flat(&items),
                        current.min(labels.len().saturating_sub(1)),
                        false,
                        win,
                    );
                } else if self.state.effect_click.check_and_update(EffectHit::Dial(i)) {
                    self.reset_effect_dial_to_default(i);
                } else if let Some(spec) = self.param_spec(i) {
                    let value = self.effect_config().params[i];
                    let norm = effects::value_to_norm(value, spec.min, spec.max, spec.scaling);
                    self.state
                        .effect_dial_drag
                        .begin_drag(EffectHit::Dial(i), norm, false);
                }
            }
            EffectHit::MsegSelector(seg) => {
                let old = self.state.selected_mseg;
                self.state.selected_mseg = seg.min(3);
                // Only clear when the selection actually changed; re-clicking
                // the active MSEG must not discard the node selection.
                if self.state.selected_mseg != old {
                    self.state.mseg_edit.clear_selection();
                }
            }
            EffectHit::Target => {
                let kind = self.effect_config().kind;
                let items: Vec<&'static str> = effect_editor::target_items(kind);
                let current = if self.state.selected_mseg == 0 {
                    0
                } else {
                    let modu = self.modulation.clone();
                    effect_editor::target_to_item(kind, modu.targets[self.state.selected_mseg - 1])
                };
                let win = (self.physical_width, self.physical_height);
                self.state.effect_dropdown.open(
                    EffectAction::Target,
                    lay.target,
                    widgets::dropdown::DropdownList::flat(&items),
                    current,
                    false,
                    win,
                );
            }
            EffectHit::Depth => {
                if self.state.selected_mseg != 0 {
                    if self.state.effect_click.check_and_update(EffectHit::Depth) {
                        self.reset_depth_to_default();
                    } else {
                        let modu = self.modulation.clone();
                        let depth = modu.depths[self.state.selected_mseg - 1];
                        let norm = ((depth + 1.0) / 2.0).clamp(0.0, 1.0);
                        self.state
                            .effect_dial_drag
                            .begin_drag(EffectHit::Depth, norm, false);
                    }
                }
            }
            EffectHit::MsegPane => {
                let sel = self.state.selected_mseg.min(3);
                let is_double = self.mseg_double_click_check(px, py);
                let changed = if is_double {
                    self.state.mseg_edit.on_double_click(
                        px,
                        py,
                        &mut self.modulation.msegs[sel],
                        lay.mseg_pane,
                        self.scale_factor,
                    )
                } else {
                    self.state.mseg_edit.on_mouse_down(
                        px,
                        py,
                        &mut self.modulation.msegs[sel],
                        lay.mseg_pane,
                        self.scale_factor,
                        ctrl,
                    )
                };
                if changed == Some(widgets::mseg::MsegEdit::Changed) {
                    self.response.changed = true;
                }
            }
            EffectHit::Trigger => {
                let trigger = self.modulation_config().trigger;
                let current = self.trigger_index(trigger);
                let items = self.trigger_items();
                let win = (self.physical_width, self.physical_height);
                self.state.effect_dropdown.open(
                    EffectAction::Trigger,
                    lay.trigger,
                    widgets::dropdown::DropdownList::flat(&items),
                    current,
                    false,
                    win,
                );
            }
            EffectHit::TriggerRate => {
                let trigger = self.modulation_config().trigger;
                let current_norm = match trigger {
                    TriggerSource::FreeHz { hz } => effects::value_to_norm(
                        hz,
                        effect_editor::TRIGGER_RATE_MIN_HZ,
                        effect_editor::TRIGGER_RATE_MAX_HZ,
                        effects::ParamScaling::Log,
                    ),
                    TriggerSource::Transient { threshold, .. } => {
                        // Sensitivity = 1 − normalised threshold.
                        1.0 - effects::value_to_norm(
                            threshold,
                            crate::modulation::TRANSIENT_THRESHOLD_MIN,
                            crate::modulation::TRANSIENT_THRESHOLD_MAX,
                            effects::ParamScaling::Log,
                        )
                    }
                    _ => return false,
                };
                if self
                    .state
                    .effect_click
                    .check_and_update(EffectHit::TriggerRate)
                {
                    self.reset_trigger_rate_to_default();
                } else {
                    self.state.effect_dial_drag.begin_drag(
                        EffectHit::TriggerRate,
                        current_norm,
                        false,
                    );
                }
            }
            EffectHit::TriggerAux => {
                let trigger = self.modulation_config().trigger;
                let TriggerSource::Transient { hold_ms, .. } = trigger else {
                    return false;
                };
                if self
                    .state
                    .effect_click
                    .check_and_update(EffectHit::TriggerAux)
                {
                    self.reset_trigger_aux_to_default();
                } else {
                    let current_norm = effects::value_to_norm(
                        hold_ms,
                        crate::modulation::TRANSIENT_HOLD_MS_MIN,
                        crate::modulation::TRANSIENT_HOLD_MS_MAX,
                        effects::ParamScaling::Log,
                    );
                    self.state.effect_dial_drag.begin_drag(
                        EffectHit::TriggerAux,
                        current_norm,
                        false,
                    );
                }
            }
            EffectHit::MsegSync(seg) => {
                self.apply_mseg_sync(seg);
            }
            EffectHit::MsegLength => {
                let norm = self.active_mseg_length_norm();
                self.state
                    .effect_dial_drag
                    .begin_drag(EffectHit::MsegLength, norm, false);
            }
            EffectHit::Mix => {
                if self.state.effect_click.check_and_update(EffectHit::Mix) {
                    self.reset_mix_to_default();
                } else {
                    let norm = self.effect_config().mix;
                    self.state
                        .effect_dial_drag
                        .begin_drag(EffectHit::Mix, norm, false);
                }
            }
        }
        true
    }

    fn mseg_double_click_check(&mut self, x: f32, y: f32) -> bool {
        let now = std::time::Instant::now();
        let elapsed_ms = now
            .duration_since(self.state.mseg_last_click_time)
            .as_millis();
        let (px, py) = self.state.mseg_last_click_pos;
        let dist_sq = (x - px) * (x - px) + (y - py) * (y - py);
        let is_double = elapsed_ms < 400 && dist_sq < 64.0;
        self.state.mseg_last_click_time = now;
        self.state.mseg_last_click_pos = (x, y);
        is_double
    }

    fn active_mseg_length_norm(&self) -> f32 {
        let modu = self.modulation.clone();
        let sel = self.state.selected_mseg.min(3);
        let mseg = modu.msegs[sel];
        match mseg.sync_mode {
            tiny_skia_widgets::SyncMode::Time => effects::value_to_norm(
                mseg.time_seconds,
                effect_editor::MSEG_LENGTH_TIME_MIN,
                effect_editor::MSEG_LENGTH_TIME_MAX,
                effects::ParamScaling::Log,
            ),
            tiny_skia_widgets::SyncMode::Beat => effect_editor::beats_value_to_norm(mseg.beats),
        }
    }

    fn apply_mseg_sync(&mut self, seg: usize) {
        let new_mode = if seg == 0 {
            tiny_skia_widgets::SyncMode::Time
        } else {
            tiny_skia_widgets::SyncMode::Beat
        };
        let k = self.state.selected_mseg.min(3);

        self.modulation.msegs[k].sync_mode = new_mode;

        self.response.changed = true;
    }

    fn apply_mseg_length_drag(&mut self, norm: f32) {
        let k = self.state.selected_mseg.min(3);

        let mseg = &mut self.modulation.msegs[k];
        match mseg.sync_mode {
            tiny_skia_widgets::SyncMode::Time => {
                mseg.time_seconds = effects::norm_to_value(
                    norm,
                    effect_editor::MSEG_LENGTH_TIME_MIN,
                    effect_editor::MSEG_LENGTH_TIME_MAX,
                    effects::ParamScaling::Log,
                );
            }
            tiny_skia_widgets::SyncMode::Beat => {
                mseg.beats = effect_editor::beats_norm_to_value(norm);
            }
        }

        self.response.changed = true;
    }

    fn effect_param_count(&self) -> usize {
        crate::effects::param_count(self.effect_config().kind)
    }

    fn param_spec(&self, i: usize) -> Option<ParamSpec> {
        crate::effects::parameter_specs(self.effect_config().kind)
            .get(i)
            .copied()
    }

    fn commit_dial_text_edit(&mut self, i: usize, text: &str) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };
        let Some(v) = effects::parse_value(text, spec.format) else {
            return;
        };
        let clamped = v.clamp(spec.min, spec.max);

        self.effect.params[i] = clamped;

        self.response.changed = true;
    }

    fn commit_mix_text_edit(&mut self, text: &str) {
        let cleaned = text.trim();
        if let Ok(pct) = cleaned.parse::<f32>()
            && pct.is_finite()
        {
            let value = (pct / 100.0).clamp(0.0, 1.0);

            self.effect.mix = value;

            self.response.changed = true;
        }
    }

    fn finalize_dial_edit_for_press(&mut self, px: f32, py: f32) {
        let Some(active) = self.state.text_edit.active_for_any() else {
            return;
        };
        // Resolve the press to an effect-editor hit, if any.
        let trigger = self.modulation_config().trigger;
        let param_count = self.effect_param_count();
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            &self.layout,
            param_count,
            self.state.selected_mseg,
            trigger,
        );
        if hit == Some(active) {
            // Click on the active dial → cancel; the normal effect-press path
            // then starts a drag.
            self.state.text_edit.cancel();
        } else {
            match self.state.text_edit.commit() {
                Some((EffectHit::Dial(i), text)) => self.commit_dial_text_edit(i, &text),
                Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                // None: no edit was active — cancel() is a no-op. A new text-editable
                // EffectHit variant needs its own Some(...) arm above, not this.
                _ => self.state.text_edit.cancel(),
            }
        }
    }

    fn apply_effect_dial(&mut self, i: usize, norm: f32) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };
        let value = effects::norm_to_value(norm, spec.min, spec.max, spec.scaling);

        self.effect.params[i] = value;

        self.response.changed = true;
    }

    fn apply_kind_switch(&mut self, kind: EffectKind) {
        crate::modulation::switch_effect_kind(self.effect, self.modulation, kind);
        self.response.changed = true;
    }

    fn apply_depth_drag(&mut self, norm: f32) {
        if self.state.selected_mseg == 0 {
            return;
        }
        let depth = (norm.clamp(0.0, 1.0) * 2.0 - 1.0).clamp(-1.0, 1.0);
        let k = self.state.selected_mseg - 1;

        self.modulation.depths[k] = depth;

        self.response.changed = true;
    }

    fn param_enum_labels(&self, i: usize) -> Option<&'static [&'static str]> {
        let kind = self.effect_config().kind;
        let specs = crate::effects::parameter_specs(kind);
        let spec = specs.get(i)?;
        match spec.format {
            crate::effects::ParamFormat::Enum { labels } => Some(labels),
            _ => None,
        }
    }

    fn effect_param_format(&self, i: usize) -> crate::effects::ParamFormat {
        let kind = self.effect_config().kind;
        let specs = crate::effects::parameter_specs(kind);
        specs
            .get(i)
            .map(|s| s.format)
            .unwrap_or(crate::effects::ParamFormat::Number {
                decimals: 0,
                unit: "",
            })
    }

    fn apply_param_dropdown_selection(&mut self, param_idx: usize, item: usize) {
        if param_idx >= crate::effects::MAX_EFFECT_PARAMS {
            return;
        }

        self.effect.params[param_idx] = item as f32;

        self.response.changed = true;
    }

    fn apply_target_selection(&mut self, item: usize) {
        if self.state.selected_mseg == 0 {
            return;
        }
        let target = effect_editor::target_from_item(self.effect.kind, item);
        let k = self.state.selected_mseg - 1;

        self.modulation.targets[k] = target;

        self.response.changed = true;
    }

    fn reset_effect_dial_to_default(&mut self, i: usize) {
        let Some(spec) = self.param_spec(i) else {
            return;
        };

        self.effect.params[i] = spec.default;

        self.response.changed = true;
    }

    fn reset_depth_to_default(&mut self) {
        if self.state.selected_mseg == 0 {
            return;
        }
        let k = self.state.selected_mseg - 1;

        self.modulation.depths[k] = 0.0;

        self.response.changed = true;
    }

    fn apply_mix(&mut self, norm: f32) {
        let value = norm.clamp(0.0, 1.0);

        self.effect.mix = value;

        self.response.changed = true;
    }

    fn reset_mix_to_default(&mut self) {
        self.effect.mix = 1.0;

        self.response.changed = true;
    }

    fn reset_trigger_rate_to_default(&mut self) {
        match &mut self.modulation.trigger {
            TriggerSource::FreeHz { hz } => *hz = 1.0,
            TriggerSource::Transient { threshold, .. } => {
                *threshold = crate::modulation::TRANSIENT_THRESHOLD_DEFAULT;
            }
            _ => return,
        }
        self.response.changed = true;
    }

    fn apply_trigger_rate_drag(&mut self, norm: f32) {
        match &mut self.modulation.trigger {
            TriggerSource::FreeHz { hz } => {
                *hz = effects::norm_to_value(
                    norm,
                    effect_editor::TRIGGER_RATE_MIN_HZ,
                    effect_editor::TRIGGER_RATE_MAX_HZ,
                    effects::ParamScaling::Log,
                );
            }
            TriggerSource::Transient { threshold, .. } => {
                *threshold = effects::norm_to_value(
                    1.0 - norm,
                    crate::modulation::TRANSIENT_THRESHOLD_MIN,
                    crate::modulation::TRANSIENT_THRESHOLD_MAX,
                    effects::ParamScaling::Log,
                );
            }
            _ => return,
        }
        self.response.changed = true;
    }

    fn reset_trigger_aux_to_default(&mut self) {
        if let TriggerSource::Transient { hold_ms, .. } = &mut self.modulation.trigger {
            *hold_ms = crate::modulation::TRANSIENT_HOLD_MS_DEFAULT;
            self.response.changed = true;
        }
    }

    fn apply_trigger_aux_drag(&mut self, norm: f32) {
        let new_hold_ms = effects::norm_to_value(
            norm,
            crate::modulation::TRANSIENT_HOLD_MS_MIN,
            crate::modulation::TRANSIENT_HOLD_MS_MAX,
            effects::ParamScaling::Log,
        );
        if let TriggerSource::Transient { hold_ms, .. } = &mut self.modulation.trigger {
            *hold_ms = new_hold_ms;
            self.response.changed = true;
        }
    }

    fn apply_trigger_selection(&mut self, idx: usize) {
        let (carried_hz, carried_threshold, carried_hold_ms) =
            match self.modulation_config().trigger {
                TriggerSource::FreeHz { hz } => (
                    hz,
                    crate::modulation::TRANSIENT_THRESHOLD_DEFAULT,
                    crate::modulation::TRANSIENT_HOLD_MS_DEFAULT,
                ),
                TriggerSource::Transient { threshold, hold_ms } => (1.0, threshold, hold_ms),
                _ => (
                    1.0,
                    crate::modulation::TRANSIENT_THRESHOLD_DEFAULT,
                    crate::modulation::TRANSIENT_HOLD_MS_DEFAULT,
                ),
            };
        let new_trigger = effect_editor::trigger_from_item(
            self.state
                .trigger_choices
                .get(idx)
                .map(|c| c.item())
                .unwrap_or(0),
            carried_hz,
            carried_threshold,
            carried_hold_ms,
        );

        self.modulation.trigger = new_trigger;
        self.response.changed = true;
    }
}

fn dropdown_items(state: &EffectEditor, effect: &TrackEffect) -> Vec<&'static str> {
    if state.effect_dropdown.is_open_for(EffectAction::Target) {
        view::target_items(effect.kind)
    } else if state.effect_dropdown.is_open_for(EffectAction::Trigger) {
        let all = view::trigger_items();
        state
            .trigger_choices
            .iter()
            .map(|c| all[c.item()])
            .collect()
    } else if let Some(i) = (0..MAX_EFFECT_PARAMS).find(|&i| {
        state
            .effect_dropdown
            .is_open_for(EffectAction::ParamDropdown(i))
    }) {
        match effects::parameter_specs(effect.kind)
            .get(i)
            .map(|s| s.format)
        {
            Some(effects::ParamFormat::Enum { labels }) => labels.to_vec(),
            _ => Vec::new(),
        }
    } else {
        view::kind_items().items
    }
}
/// Amp, amber, purple, and mint slot colours; out-of-range slots clamp to 3.
pub fn mseg_color(slot: usize) -> tiny_skia::Color {
    match slot.min(3) {
        0 => tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xff), // Amp — sky blue
        1 => tiny_skia::Color::from_rgba8(0xff, 0xc8, 0x58, 0xff), // MSEG 1 — amber
        2 => tiny_skia::Color::from_rgba8(0xc3, 0x78, 0xff, 0xff), // MSEG 2 — purple
        _ => tiny_skia::Color::from_rgba8(0x66, 0xd9, 0xa0, 0xff), // MSEG 3 — mint
    }
}

/// Time window for double-click-to-reset on sliders/dials.
const DOUBLE_CLICK_MS: u128 = 400;

/// Two consecutive presses on the same control within `DOUBLE_CLICK_MS`
/// register as a double-click. The control's identity is carried in `A`.
pub struct ClickTracker<A: Copy + PartialEq> {
    last_time: std::time::Instant,
    last_action: Option<A>,
}

impl<A: Copy + PartialEq> Default for ClickTracker<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Copy + PartialEq> ClickTracker<A> {
    pub fn new() -> Self {
        Self {
            last_time: std::time::Instant::now(),
            last_action: None,
        }
    }

    /// Record this press; return `true` if it is a double-click on the same
    /// control as the previous press within the time window.
    pub fn check_and_update(&mut self, action: A) -> bool {
        self.check_and_update_at(action, std::time::Instant::now())
    }

    fn check_and_update_at(&mut self, action: A, now: std::time::Instant) -> bool {
        let is_double = self.last_action == Some(action)
            && now.duration_since(self.last_time).as_millis() < DOUBLE_CLICK_MS;
        self.last_time = now;
        self.last_action = Some(action);
        is_double
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectAction {
    Kind,
    Target,
    Trigger,
    ParamDropdown(usize),
}

/// Format a node's position and its amplitude or routed parameter value.
pub fn mseg_node_tooltip_text(
    slot: usize,
    data: &widgets::MsegData,
    node_idx: usize,
    spec: Option<crate::effects::ParamSpec>,
    base: f32,
    depth_polarity: Option<(f32, widgets::Polarity)>,
) -> String {
    if node_idx >= data.node_count {
        return String::new();
    }
    let value = data.nodes[node_idx].value;
    let time = data.nodes[node_idx].time;

    // Position line — Y display flips polarity-aware: bipolar shows
    // `(value - 0.5) * 2` so the midline reads `0.00` and the extremes
    // read `±1.00`; unipolar prints the raw `0..1` value.
    let y_str = match data.polarity {
        widgets::Polarity::Unipolar => format!("{value:.2}"),
        widgets::Polarity::Bipolar => format!("{:+.2}", (value - 0.5) * 2.0),
    };
    let pos_line = format!("X {:.0}%  Y {}", time * 100.0, y_str);

    // Dial-value line — slot-dependent.
    let dial_line: Option<String> = if slot == 0 {
        const FLOOR_DB: f32 = -80.0;
        if value <= 1e-4 {
            Some("-\u{221e} dB".to_string())
        } else {
            let db = (20.0 * value.log10()).max(FLOOR_DB);
            Some(format!("{db:.1} dB"))
        }
    } else {
        match (spec, depth_polarity) {
            (Some(spec), Some((depth, polarity))) => {
                let v = crate::modulation::assignable_value(value, base, depth, spec, polarity);
                Some(crate::effects::format_value(v, spec.format))
            }
            _ => None,
        }
    };

    match dial_line {
        Some(d) => format!("{pos_line}\n{d}"),
        None => pos_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_session(
        choices: &'static [TriggerChoice],
        kind: EffectKind,
        check: impl FnOnce(&mut Session<'_>),
    ) {
        let mut editor = EffectEditor::new(choices);
        let mut effect = TrackEffect {
            kind,
            params: effects::default_params_for_kind(kind),
            ..Default::default()
        };
        let mut modulation = TrackModulation::default();
        let mut tr = widgets::TextRenderer::new(widgets::DEFAULT_FONT);
        let geometry = EditorGeometry {
            layout: view::effect_layout((16.0, 16.0, 1_304.0, 640.0), 1.0),
            scale: 1.0,
            window_size: (1_336.0, 680.0),
        };
        let mut session = Session::new(
            &mut editor,
            EditorContext {
                pointer_position: (0.0, 0.0),
                effect: &mut effect,
                modulation: &mut modulation,
                text_renderer: &mut tr,
                geometry,
            },
        );
        check(&mut session);
    }

    mod editor_helpers {
        use super::*;

        fn open_menu(editor: &mut EffectEditor, action: EffectAction) {
            editor.effect_dropdown.open(
                action,
                (20.0, 20.0, 140.0, 30.0),
                widgets::dropdown::DropdownList {
                    items: &["one", "two"],
                    sections: &[],
                },
                0,
                true,
                (800.0, 600.0),
            );
        }

        #[test]
        fn keyboard_capture_tracks_popup_and_text_edit_independently() {
            for (popup, text) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut editor = EffectEditor::default();
                if popup {
                    open_menu(&mut editor, EffectAction::Kind);
                }
                if text {
                    editor.text_edit.begin(EffectHit::Mix, "25");
                }
                assert_eq!(editor.dropdown_open(), popup);
                assert_eq!(editor.captures_keyboard(), popup || text);
            }
        }

        #[test]
        fn trigger_menus_preserve_host_order_and_missing_sources_fall_back_to_zero() {
            for (choice, index) in [
                (TriggerChoice::Free, 0),
                (TriggerChoice::CellLight, 1),
                (TriggerChoice::CellStep, 2),
                (TriggerChoice::FreeHz, 3),
                (TriggerChoice::Transient, 4),
            ] {
                assert_eq!(choice.item(), index);
            }
            let cases: [(&'static [TriggerChoice], &[&str], [usize; 5]); 4] = [
                (
                    TriggerChoice::ALL,
                    &[
                        "Free run",
                        "Cell light",
                        "Cell step",
                        "Free Hz",
                        "Transient",
                    ],
                    [0, 1, 2, 3, 4],
                ),
                (
                    &[
                        TriggerChoice::Transient,
                        TriggerChoice::FreeHz,
                        TriggerChoice::Free,
                    ],
                    &["Transient", "Free Hz", "Free run"],
                    [2, 0, 0, 1, 0],
                ),
                (&[TriggerChoice::FreeHz], &["Free Hz"], [0; 5]),
                (&[], &[], [0; 5]),
            ];
            for (choices, labels, indices) in cases {
                with_session(choices, EffectKind::None, |session| {
                    assert_eq!(session.trigger_items(), labels);
                    for (source, index) in [
                        TriggerSource::Free,
                        TriggerSource::CellLight,
                        TriggerSource::CellStep,
                        TriggerSource::FreeHz { hz: 7.25 },
                        TriggerSource::Transient {
                            threshold: 2.5,
                            hold_ms: 80.0,
                        },
                    ]
                    .into_iter()
                    .zip(indices)
                    {
                        assert_eq!(session.trigger_index(source), index, "{source:?}");
                    }
                });
            }
        }

        #[test]
        fn dropdowns_follow_active_action_and_parameter_format() {
            with_session(
                &[TriggerChoice::Transient, TriggerChoice::Free],
                EffectKind::Svf,
                |session| {
                    let kinds = view::kind_items().items;
                    assert_eq!(dropdown_items(session.state, session.effect), kinds);
                    assert_eq!(session.dropdown_items(), kinds);
                    for (action, expected) in [
                        (EffectAction::Kind, kinds),
                        (
                            EffectAction::Target,
                            vec!["(none)", "Cutoff", "Resonance", "Type", "Poles", "Mix"],
                        ),
                        (EffectAction::Trigger, vec!["Transient", "Free run"]),
                        (
                            EffectAction::ParamDropdown(2),
                            vec!["Lowpass", "Bandpass", "Highpass"],
                        ),
                        (EffectAction::ParamDropdown(3), vec!["2", "4", "6", "8"]),
                        (EffectAction::ParamDropdown(0), vec![]),
                        (EffectAction::ParamDropdown(4), vec![]),
                    ] {
                        open_menu(session.state, action);
                        assert_eq!(
                            dropdown_items(session.state, session.effect),
                            expected,
                            "{action:?}"
                        );
                        assert_eq!(session.dropdown_items(), expected, "{action:?}");
                    }
                },
            );
            with_session(TriggerChoice::ALL, EffectKind::None, |session| {
                open_menu(session.state, EffectAction::Target);
                assert_eq!(session.dropdown_items(), ["(none)", "Mix"]);
                open_menu(session.state, EffectAction::ParamDropdown(0));
                assert!(session.dropdown_items().is_empty());
            });
        }

        #[test]
        fn slot_palette_is_opaque_and_extreme_indices_use_the_last_hue() {
            for slot in [0, 1, 2, 3, 4, usize::MAX] {
                assert_eq!(mseg_color(slot).alpha(), 1.0);
            }
            assert_eq!(mseg_color(usize::MAX), mseg_color(3));
        }

        #[test]
        fn amp_tooltips_show_exact_positions_and_the_silence_threshold() {
            let above_floor = f32::from_bits(1e-4_f32.to_bits() + 1);
            for (value, expected) in [
                (0.0, "X 42%  Y 0.00\n-∞ dB"),
                (1e-4, "X 42%  Y 0.00\n-∞ dB"),
                (above_floor, "X 42%  Y 0.00\n-80.0 dB"),
                (0.01, "X 42%  Y 0.01\n-40.0 dB"),
                (0.5, "X 42%  Y 0.50\n-6.0 dB"),
                (1.0, "X 42%  Y 1.00\n0.0 dB"),
            ] {
                let mut data = widgets::MsegData::default();
                let index = data.insert_node(0.42, value).unwrap();
                assert_eq!(
                    mseg_node_tooltip_text(0, &data, index, None, 0.0, None),
                    expected
                );
            }
        }

        #[test]
        fn routed_tooltips_require_both_metadata_inputs_and_preserve_units_and_polarity() {
            let spec = effects::ParamSpec {
                name: "Gain",
                min: -10.0,
                max: 30.0,
                default: 0.0,
                scaling: effects::ParamScaling::Linear,
                format: effects::ParamFormat::Number {
                    decimals: 1,
                    unit: "dB",
                },
            };
            for (polarity, depth, expected) in [
                (widgets::Polarity::Unipolar, 0.5, "X 25%  Y 0.75\n15.0 dB"),
                (widgets::Polarity::Bipolar, 0.5, "X 25%  Y +0.50\n10.0 dB"),
                (widgets::Polarity::Bipolar, -0.5, "X 25%  Y +0.50\n-10.0 dB"),
            ] {
                let mut data = widgets::MsegData {
                    polarity,
                    ..Default::default()
                };
                let index = data.insert_node(0.25, 0.75).unwrap();
                assert_eq!(
                    mseg_node_tooltip_text(
                        3,
                        &data,
                        index,
                        Some(spec),
                        0.0,
                        Some((depth, polarity))
                    ),
                    expected
                );
                let position = expected.split('\n').next().unwrap();
                for (metadata, modulation) in [
                    (None, None),
                    (Some(spec), None),
                    (None, Some((depth, polarity))),
                ] {
                    assert_eq!(
                        mseg_node_tooltip_text(3, &data, index, metadata, 0.0, modulation),
                        position
                    );
                }
            }
        }

        #[test]
        fn tooltip_indices_at_or_past_the_active_node_count_are_empty() {
            let data = widgets::MsegData::default();
            for index in [data.node_count, data.node_count + 1, usize::MAX] {
                assert_eq!(mseg_node_tooltip_text(0, &data, index, None, 0.0, None), "");
            }
        }

        #[test]
        fn public_click_tracking_records_the_first_press_with_the_current_time() {
            let mut tracker = ClickTracker::<EffectHit>::default();
            assert_eq!(tracker.last_action, None);
            let before = std::time::Instant::now();
            assert!(!tracker.check_and_update(EffectHit::Mix));
            let after = std::time::Instant::now();
            assert_eq!(tracker.last_action, Some(EffectHit::Mix));
            assert!((before..=after).contains(&tracker.last_time));
        }

        #[test]
        fn double_clicks_require_the_same_action_strictly_before_four_hundred_ms() {
            use std::time::{Duration, Instant};

            let anchor = Instant::now();
            let a = EffectHit::Dial(0);
            let b = EffectHit::Dial(1);
            for (previous, next, elapsed, expected) in [
                (None, a, Duration::ZERO, false),
                (Some(a), a, Duration::ZERO, true),
                (Some(a), a, Duration::from_millis(399), true),
                (Some(a), a, Duration::from_micros(399_999), true),
                (Some(a), a, Duration::from_millis(400), false),
                (Some(a), a, Duration::from_micros(400_001), false),
                (Some(a), a, Duration::from_secs(1), false),
                (Some(a), b, Duration::from_millis(100), false),
            ] {
                let mut tracker = ClickTracker {
                    last_time: anchor,
                    last_action: previous,
                };
                let now = anchor + elapsed;
                assert_eq!(
                    tracker.check_and_update_at(next, now),
                    expected,
                    "{previous:?} -> {next:?} after {elapsed:?}"
                );
                assert_eq!(tracker.last_time, now);
                assert_eq!(tracker.last_action, Some(next));
            }
        }

        #[test]
        fn click_tracking_updates_the_baseline_after_every_action_and_result() {
            use std::time::{Duration, Instant};

            let mut tracker = ClickTracker::<EffectHit>::new();
            let anchor = Instant::now();
            for (millis, action, expected) in [
                (0, EffectHit::Mix, false),
                (100, EffectHit::Mix, true),
                (399, EffectHit::Mix, true),
                (500, EffectHit::Depth, false),
                (600, EffectHit::Depth, true),
                (1_000, EffectHit::Depth, false),
                (1_001, EffectHit::Mix, false),
                (1_002, EffectHit::Mix, true),
            ] {
                let now = anchor + Duration::from_millis(millis);
                assert_eq!(
                    tracker.check_and_update_at(action, now),
                    expected,
                    "press at {millis} ms"
                );
                assert_eq!(tracker.last_time, now);
                assert_eq!(tracker.last_action, Some(action));
            }
        }
    }

    mod editor_text {
        use super::*;

        fn center(rect: (f32, f32, f32, f32)) -> (f32, f32) {
            (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0)
        }

        fn select_interior_node(session: &mut Session<'_>) -> (f32, f32) {
            let slot = session.state.selected_mseg.min(3);
            let data = &mut session.modulation.msegs[slot];
            let index = data.insert_node(0.5, 0.35).unwrap();
            let node = data.nodes[index];
            let layout =
                widgets::mseg::mseg_layout(session.layout.mseg_pane, false, session.scale_factor);
            let point = (
                widgets::mseg::phase_to_x(&layout, node.time),
                widgets::mseg::value_to_y(&layout, node.value),
            );
            session.state.mseg_edit.on_mouse_down(
                point.0,
                point.1,
                data,
                session.layout.mseg_pane,
                session.scale_factor,
                false,
            );
            session.state.mseg_edit.on_mouse_up(
                data,
                session.layout.mseg_pane,
                session.scale_factor,
            );
            assert!(session.state.mseg_edit.is_node_selected(index));
            point
        }

        fn open_menu(editor: &mut EffectEditor) {
            editor.effect_dropdown.open(
                EffectAction::Kind,
                (20.0, 20.0, 140.0, 30.0),
                widgets::dropdown::DropdownList::flat(&["one", "two"]),
                0,
                true,
                (800.0, 600.0),
            );
        }

        #[test]
        fn dial_text_parses_units_and_clamps_only_the_edited_parameter() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (index, text, value) in [
                    (0, "800", 800.0),
                    (0, " 1.5 kHz ", 1_500.0),
                    (0, "2K", 2_000.0),
                    (0, "0", 20.0),
                    (0, "30000", 20_000.0),
                    (1, ".25", 0.25),
                    (1, "-1", 0.0),
                    (1, "2", 1.0),
                ] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.commit_dial_text_edit(index, text);
                    let mut expected = original;
                    expected.params[index] = value;
                    assert_eq!(*session.effect, expected, "parameter {index}, text {text}");
                    assert_eq!(*session.modulation, modulation);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn invalid_dial_text_and_missing_parameters_preserve_configuration_and_dirty_flags() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                for (index, text) in [
                    (0, ""),
                    (0, "bad"),
                    (0, "NaN"),
                    (0, "inf"),
                    (0, "-inf"),
                    (0, "1e100"),
                    (1, "NaN"),
                    (4, "0.75"),
                    (MAX_EFFECT_PARAMS, "0.75"),
                    (usize::MAX, "0.75"),
                ] {
                    for dirty in [false, true] {
                        session.response.changed = dirty;
                        session.commit_dial_text_edit(index, text);
                        assert_eq!(*session.effect, effect);
                        assert_eq!(*session.modulation, modulation);
                        assert_eq!(session.response.changed, dirty);
                    }
                }
            });
        }

        #[test]
        fn mix_text_accepts_finite_percentages_and_clamps_the_blend() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                for (text, mix) in [
                    ("25", 0.25),
                    (" 12.5 ", 0.125),
                    ("-20", 0.0),
                    ("250", 1.0),
                    ("1e2", 1.0),
                    ("0", 0.0),
                ] {
                    session.response = EditorResponse::default();
                    session.commit_mix_text_edit(text);
                    assert_eq!(*session.effect, TrackEffect { mix, ..effect });
                    assert_eq!(*session.modulation, modulation);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn invalid_mix_text_preserves_the_blend_and_existing_dirty_flags() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.mix = 0.37;
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                for text in ["", "bad", "25 %", "NaN", "inf", "-inf", "1e100"] {
                    for dirty in [false, true] {
                        session.response.changed = dirty;
                        session.commit_mix_text_edit(text);
                        assert_eq!(*session.effect, effect, "{text}");
                        assert_eq!(*session.modulation, modulation);
                        assert_eq!(session.response.changed, dirty);
                    }
                }
            });
        }

        #[test]
        fn pressing_the_edited_control_cancels_while_other_presses_commit() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (action, text, rect) in [
                    (EffectHit::Dial(0), "800", session.layout.dials[0]),
                    (EffectHit::Mix, "25", session.layout.mix),
                ] {
                    for same_control in [true, false] {
                        *session.effect = original;
                        session.state.text_edit.begin(action, text);
                        session.response = EditorResponse::default();
                        let (x, y) = if same_control {
                            center(rect)
                        } else {
                            (-100.0, -100.0)
                        };
                        session.finalize_dial_edit_for_press(x, y);
                        let mut expected = original;
                        if !same_control {
                            match action {
                                EffectHit::Dial(0) => expected.params[0] = 800.0,
                                EffectHit::Mix => expected.mix = 0.25,
                                _ => unreachable!(),
                            }
                        }
                        assert_eq!(*session.effect, expected);
                        assert_eq!(*session.modulation, modulation);
                        assert!(!session.state.text_edit.is_active());
                        assert_eq!(session.response.changed, !same_control);
                        assert!(!session.response.captured);
                    }
                }
            });
        }

        #[test]
        fn idle_and_unsupported_text_finalization_leave_host_state_unchanged() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                for active in [false, true] {
                    if active {
                        session.state.text_edit.begin(EffectHit::Depth, "0.75");
                    }
                    for dirty in [false, true] {
                        session.response.changed = dirty;
                        session.finalize_dial_edit_for_press(-100.0, -100.0);
                        assert_eq!(*session.effect, effect);
                        assert_eq!(*session.modulation, modulation);
                        assert!(!session.state.text_edit.is_active());
                        assert_eq!(session.response.changed, dirty);
                    }
                }
            });
        }

        #[test]
        fn public_finish_commits_before_host_focus_moves_even_when_pointer_stays_on_the_dial() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (action, text, rect, cutoff, mix, changed) in [
                    (
                        EffectHit::Dial(0),
                        "800",
                        session.layout.dials[0],
                        800.0,
                        1.0,
                        true,
                    ),
                    (
                        EffectHit::Mix,
                        "25",
                        session.layout.mix,
                        2_000.0,
                        0.25,
                        true,
                    ),
                    (
                        EffectHit::Dial(0),
                        "NaN",
                        session.layout.dials[0],
                        2_000.0,
                        1.0,
                        false,
                    ),
                ] {
                    *session.effect = original;
                    session.state.text_edit.begin(action, text);
                    let response = session.state.finish_text_edit(EditorContext {
                        pointer_position: center(rect),
                        effect: session.effect,
                        modulation: session.modulation,
                        text_renderer: session.text_renderer,
                        geometry: EditorGeometry {
                            layout: session.layout,
                            scale: session.scale_factor,
                            window_size: (session.physical_width, session.physical_height),
                        },
                    });
                    let mut expected = original;
                    expected.params[0] = cutoff;
                    expected.mix = mix;
                    assert_eq!(*session.effect, expected);
                    assert_eq!(*session.modulation, modulation);
                    assert!(!session.state.captures_keyboard());
                    assert_eq!(
                        (
                            response.changed,
                            response.captured,
                            response.back,
                            response.randomize
                        ),
                        (changed, false, false, false)
                    );
                }
                let before = *session.effect;
                let response = session.state.finish_text_edit(EditorContext {
                    pointer_position: (0.0, 0.0),
                    effect: session.effect,
                    modulation: session.modulation,
                    text_renderer: session.text_renderer,
                    geometry: EditorGeometry {
                        layout: session.layout,
                        scale: session.scale_factor,
                        window_size: (session.physical_width, session.physical_height),
                    },
                });
                assert_eq!(*session.effect, before);
                assert!(!response.changed);
            });
        }

        #[test]
        fn right_press_commits_every_dial_mix_transition_before_seeding_the_next_edit() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.mix = 0.374;
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (previous, text, next, seed, cutoff, mix) in [
                    (
                        EffectHit::Dial(0),
                        "800",
                        EffectHit::Dial(1),
                        "0.10",
                        800.0,
                        0.374,
                    ),
                    (
                        EffectHit::Mix,
                        "25",
                        EffectHit::Dial(1),
                        "0.10",
                        2_000.0,
                        0.25,
                    ),
                    (
                        EffectHit::Dial(0),
                        "800",
                        EffectHit::Mix,
                        "37",
                        800.0,
                        0.374,
                    ),
                    (EffectHit::Mix, "25", EffectHit::Mix, "25", 2_000.0, 0.25),
                ] {
                    *session.effect = original;
                    session.state.text_edit.begin(previous, text);
                    session.mouse_pos = center(if next == EffectHit::Mix {
                        session.layout.mix
                    } else {
                        session.layout.dials[1]
                    });
                    session.response = EditorResponse::default();
                    assert!(matches!(
                        session.on_right_press(),
                        baseview::EventStatus::Captured
                    ));
                    let mut expected = original;
                    expected.params[0] = cutoff;
                    expected.mix = mix;
                    assert_eq!(*session.effect, expected);
                    assert_eq!(*session.modulation, modulation);
                    assert_eq!(session.state.text_edit.active_for(&next), Some(seed));
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn right_press_seeds_bare_parameter_values_and_rounded_mix_percentages() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.params[0] = 1_500.0;
                session.effect.params[1] = 0.1234;
                for (index, seed) in [(0, "1500"), (1, "0.12")] {
                    session.state.text_edit.cancel();
                    session.mouse_pos = center(session.layout.dials[index]);
                    session.response = EditorResponse::default();
                    assert!(matches!(
                        session.on_right_press(),
                        baseview::EventStatus::Captured
                    ));
                    assert_eq!(
                        session.state.text_edit.active_for(&EffectHit::Dial(index)),
                        Some(seed)
                    );
                    assert!(!session.response.changed);
                }
                for (mix, seed) in [(0.0, "0"), (0.374, "37"), (0.376, "38"), (1.0, "100")] {
                    session.state.text_edit.cancel();
                    session.effect.mix = mix;
                    let before = *session.effect;
                    session.mouse_pos = center(session.layout.mix);
                    session.response = EditorResponse::default();
                    assert!(matches!(
                        session.on_right_press(),
                        baseview::EventStatus::Captured
                    ));
                    assert_eq!(
                        session.state.text_edit.active_for(&EffectHit::Mix),
                        Some(seed)
                    );
                    assert_eq!(*session.effect, before);
                    assert!(!session.response.changed);
                }
            });
        }

        #[test]
        fn right_press_during_drag_and_on_enum_controls_preserves_an_active_edit() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.state.text_edit.begin(EffectHit::Mix, "25");
                session
                    .state
                    .effect_dial_drag
                    .begin_drag(EffectHit::Dial(0), 0.5, false);
                session.mouse_pos = center(session.layout.dials[1]);
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                assert!(matches!(
                    session.on_right_press(),
                    baseview::EventStatus::Captured
                ));
                assert_eq!(
                    session.state.effect_dial_drag.active_action(),
                    Some(&EffectHit::Dial(0))
                );
                assert_eq!(
                    session.state.text_edit.active_for(&EffectHit::Mix),
                    Some("25")
                );
                assert!(!session.response.changed);
                session.state.effect_dial_drag.end_drag();
                session.mouse_pos = center(session.layout.dials[2]);
                assert!(matches!(
                    session.on_right_press(),
                    baseview::EventStatus::Ignored
                ));
                assert_eq!(
                    session.state.text_edit.active_for(&EffectHit::Mix),
                    Some("25")
                );
                assert_eq!(*session.effect, effect);
                assert_eq!(*session.modulation, modulation);
                assert!(!session.response.changed);
            });
        }

        #[test]
        fn right_press_on_non_text_controls_or_outside_preserves_the_pending_edit() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.state.text_edit.begin(EffectHit::Mix, "25");
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                for point in [
                    center(session.layout.depth),
                    center(session.layout.trigger),
                    (-100.0, -100.0),
                ] {
                    session.mouse_pos = point;
                    assert!(matches!(
                        session.on_right_press(),
                        baseview::EventStatus::Captured
                    ));
                    assert_eq!(
                        session.state.text_edit.active_for(&EffectHit::Mix),
                        Some("25")
                    );
                    assert_eq!(*session.effect, effect);
                    assert_eq!(*session.modulation, modulation);
                    assert!(!session.response.changed);
                }
            });
        }

        #[test]
        fn right_press_toggles_only_the_selected_mseg_segment() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = session.modulation.clone();
                let effect = *session.effect;
                let layout = widgets::mseg::mseg_layout(
                    session.layout.mseg_pane,
                    false,
                    session.scale_factor,
                );
                session.mouse_pos = (
                    widgets::mseg::phase_to_x(&layout, 0.33),
                    widgets::mseg::value_to_y(&layout, 0.2),
                );
                for (slot, index, segment) in [
                    (0, 0, 0),
                    (1, 1, 0),
                    (2, 2, 0),
                    (3, 3, 1),
                    (usize::MAX, 3, 1),
                ] {
                    session.state.selected_mseg = slot;
                    *session.modulation = original.clone();
                    session.response = EditorResponse::default();
                    assert!(matches!(
                        session.on_right_press(),
                        baseview::EventStatus::Captured
                    ));
                    let mut expected = original.clone();
                    expected.msegs[index].nodes[segment].stepped =
                        !original.msegs[index].nodes[segment].stepped;
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(*session.effect, effect);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn right_press_on_a_selected_node_opens_transforms_without_editing_the_curve() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.mouse_pos = select_interior_node(session);
                let original = session.modulation.clone();
                assert!(matches!(
                    session.on_right_press(),
                    baseview::EventStatus::Captured
                ));
                assert!(
                    session
                        .state
                        .mseg_edit
                        .dropdown_is_open_for(widgets::mseg::StripId::Transform)
                );
                assert_eq!(*session.modulation, original);
                assert!(!session.response.changed);
            });
        }

        #[test]
        fn clearing_node_selection_preserves_other_editor_interactions() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                select_interior_node(session);
                open_menu(session.state);
                session.state.text_edit.begin(EffectHit::Mix, "25");
                session
                    .state
                    .effect_dial_drag
                    .begin_drag(EffectHit::Dial(0), 0.5, false);
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                session.state.clear_selection();
                assert_eq!(session.state.mseg_edit.selection_count(), 0);
                assert_eq!(session.state.selected_mseg, 1);
                assert!(session.state.dropdown_open());
                assert_eq!(
                    session.state.text_edit.active_for(&EffectHit::Mix),
                    Some("25")
                );
                assert_eq!(
                    session.state.effect_dial_drag.active_action(),
                    Some(&EffectHit::Dial(0))
                );
                assert_eq!(*session.effect, effect);
                assert_eq!(*session.modulation, modulation);
            });
        }

        #[test]
        fn cancellation_resets_gestures_and_click_history_but_preserves_host_choices_and_active_slot()
         {
            with_session(
                &[TriggerChoice::FreeHz, TriggerChoice::Transient],
                EffectKind::Svf,
                |session| {
                    session.state.selected_mseg = 3;
                    let point = select_interior_node(session);
                    open_menu(session.state);
                    session.state.text_edit.begin(EffectHit::Mix, "25");
                    session.state.effect_dial_drag.set_mouse(100.0, 200.0);
                    session
                        .state
                        .effect_dial_drag
                        .begin_drag(EffectHit::Dial(0), 0.5, false);
                    session.state.mouse_pos = point;
                    session.state.mseg_last_click_pos = point;
                    session.state.effect_click.last_action = Some(EffectHit::Mix);
                    let effect = *session.effect;
                    let modulation = session.modulation.clone();
                    let before = std::time::Instant::now();
                    session.state.cancel_interaction();
                    let after = std::time::Instant::now();
                    assert_eq!(session.state.selected_mseg, 3);
                    assert_eq!(
                        session.state.trigger_choices,
                        &[TriggerChoice::FreeHz, TriggerChoice::Transient]
                    );
                    assert!(!session.state.captures_keyboard());
                    assert!(!session.state.dropdown_open());
                    assert_eq!(session.state.effect_dial_drag.active_action(), None);
                    assert_eq!(session.state.effect_dial_drag.mouse_pos(), (0.0, 0.0));
                    assert!(!session.state.effect_dial_drag.mouse_in_window());
                    assert_eq!(session.state.mseg_edit.selection_count(), 0);
                    assert_eq!(session.state.mouse_pos, (0.0, 0.0));
                    assert_eq!(session.state.mseg_last_click_pos, (-999.0, -999.0));
                    assert_eq!(session.state.effect_click.last_action, None);
                    assert!((before..=after).contains(&session.state.mseg_last_click_time));
                    assert!((before..=after).contains(&session.state.effect_click.last_time));
                    assert_eq!(*session.effect, effect);
                    assert_eq!(*session.modulation, modulation);
                },
            );
        }
    }

    mod editor_timing {
        use super::*;

        #[test]
        fn normalized_dials_and_mix_read_host_values_and_missing_controls_use_fallbacks() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.params[0] = 200.0;
                session.effect.params[1] = 0.27;
                session.effect.mix = 0.23;
                let effect = *session.effect;
                let modulation = session.modulation.clone();
                assert!((session.normalized_for_hit(EffectHit::Dial(0)) - 1.0 / 3.0).abs() < 1e-6);
                assert_eq!(session.normalized_for_hit(EffectHit::Dial(1)), 0.27);
                assert_eq!(session.normalized_for_hit(EffectHit::Mix), 0.23);
                for index in [4, MAX_EFFECT_PARAMS, usize::MAX] {
                    assert_eq!(session.normalized_for_hit(EffectHit::Dial(index)), 0.0);
                }
                for hit in [
                    EffectHit::Back,
                    EffectHit::Kind,
                    EffectHit::Randomize,
                    EffectHit::Target,
                    EffectHit::Trigger,
                    EffectHit::MsegSelector(3),
                ] {
                    assert_eq!(session.normalized_for_hit(hit), 0.5, "{hit:?}");
                }
                assert_eq!(*session.effect, effect);
                assert_eq!(*session.modulation, modulation);
                assert!(!session.response.changed);
            });
        }

        #[test]
        fn normalized_depth_uses_each_assignable_slot_and_amp_stays_at_the_midpoint() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                for slot in 0..4 {
                    session.state.selected_mseg = slot;
                    for (depth, expected) in [
                        (-2.0, 0.0),
                        (-1.0, 0.0),
                        (-0.5, 0.25),
                        (0.0, 0.5),
                        (0.5, 0.75),
                        (1.0, 1.0),
                        (2.0, 1.0),
                    ] {
                        session.modulation.depths = [0.1, 0.2, 0.3];
                        if slot != 0 {
                            session.modulation.depths[slot - 1] = depth;
                        }
                        let modulation = session.modulation.clone();
                        assert_eq!(
                            session.normalized_for_hit(EffectHit::Depth),
                            if slot == 0 { 0.5 } else { expected }
                        );
                        assert_eq!(*session.modulation, modulation);
                        assert!(!session.response.changed);
                    }
                }
            });
        }

        #[test]
        fn trigger_norms_read_logarithmic_rate_inverse_sensitivity_and_hold() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                for (hz, expected) in [(0.05, 0.0), (0.223_606_8, 0.25), (1.0, 0.5), (20.0, 1.0)] {
                    session.modulation.trigger = TriggerSource::FreeHz { hz };
                    assert!(
                        (session.normalized_for_hit(EffectHit::TriggerRate) - expected).abs()
                            < 1e-6
                    );
                    assert_eq!(session.normalized_for_hit(EffectHit::TriggerAux), 0.5);
                }
                for (threshold, hold_ms, sensitivity, hold) in [
                    (6.0, 5.0, 0.0, 0.0),
                    (2.509_980_2, 50.0, 0.5, 0.5),
                    (1.05, 500.0, 1.0, 1.0),
                ] {
                    session.modulation.trigger = TriggerSource::Transient { threshold, hold_ms };
                    let before = session.modulation.clone();
                    assert!(
                        (session.normalized_for_hit(EffectHit::TriggerRate) - sensitivity).abs()
                            < 1e-6
                    );
                    assert!(
                        (session.normalized_for_hit(EffectHit::TriggerAux) - hold).abs() < 1e-6
                    );
                    assert_eq!(*session.modulation, before);
                }
                for trigger in [
                    TriggerSource::Free,
                    TriggerSource::CellLight,
                    TriggerSource::CellStep,
                ] {
                    session.modulation.trigger = trigger;
                    assert_eq!(session.normalized_for_hit(EffectHit::TriggerRate), 0.5);
                    assert_eq!(session.normalized_for_hit(EffectHit::TriggerAux), 0.5);
                }
                assert!(!session.response.changed);
            });
        }

        #[test]
        fn length_norm_reads_the_selected_clock_and_clamps_oversized_slots() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                for (slot, index) in [(0, 0), (1, 1), (2, 2), (3, 3), (usize::MAX, 3)] {
                    session.state.selected_mseg = slot;
                    for (seconds, expected) in [(0.05, 0.0), (1.264_911, 0.5), (32.0, 1.0)] {
                        let mseg = &mut session.modulation.msegs[index];
                        mseg.sync_mode = widgets::SyncMode::Time;
                        mseg.time_seconds = seconds;
                        let before = session.modulation.clone();
                        assert!((session.active_mseg_length_norm() - expected).abs() < 1e-6);
                        assert!(
                            (session.normalized_for_hit(EffectHit::MsegLength) - expected).abs()
                                < 1e-6
                        );
                        assert_eq!(*session.modulation, before);
                    }
                    for (beats, expected) in [
                        (0.25, 0.0),
                        (1.0, 0.2),
                        (4.0, 0.4),
                        (8.0, 0.5),
                        (256.0, 1.0),
                    ] {
                        let mseg = &mut session.modulation.msegs[index];
                        mseg.sync_mode = widgets::SyncMode::Beat;
                        mseg.beats = beats;
                        assert_eq!(session.active_mseg_length_norm(), expected);
                        assert_eq!(session.normalized_for_hit(EffectHit::MsegLength), expected);
                    }
                    assert!(!session.response.changed);
                }
            });
        }

        #[test]
        fn sync_selection_changes_only_the_active_mseg_mode_and_reports_reselection() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = session.modulation.clone();
                let effect = *session.effect;
                for (slot, index) in [(0, 0), (1, 1), (2, 2), (3, 3), (usize::MAX, 3)] {
                    session.state.selected_mseg = slot;
                    for (segment, mode) in [
                        (0, widgets::SyncMode::Time),
                        (1, widgets::SyncMode::Beat),
                        (usize::MAX, widgets::SyncMode::Beat),
                    ] {
                        *session.modulation = original.clone();
                        let mut expected = original.clone();
                        expected.msegs[index].sync_mode = mode;
                        for _ in 0..2 {
                            session.response = EditorResponse::default();
                            session.apply_mseg_sync(segment);
                            assert_eq!(*session.modulation, expected);
                            assert_eq!(*session.effect, effect);
                            assert!(session.response.changed);
                        }
                    }
                }
            });
        }

        #[test]
        fn time_length_drag_preserves_beats_and_every_other_envelope() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = session.modulation.clone();
                let effect = *session.effect;
                for (slot, index) in [(0, 0), (1, 1), (2, 2), (3, 3), (usize::MAX, 3)] {
                    session.state.selected_mseg = slot;
                    for (norm, seconds) in [
                        (-1.0, 0.05),
                        (0.0, 0.05),
                        (0.5, 1.264_911),
                        (1.0, 32.0),
                        (2.0, 32.0),
                    ] {
                        *session.modulation = original.clone();
                        session.modulation.msegs[index].sync_mode = widgets::SyncMode::Time;
                        let mut expected = session.modulation.clone();
                        session.response = EditorResponse::default();
                        session.apply_mseg_length_drag(norm);
                        let actual = session.modulation.msegs[index].time_seconds;
                        assert!(
                            (actual - seconds).abs() < 1e-4,
                            "slot {slot}, norm {norm}: {actual}"
                        );
                        expected.msegs[index].time_seconds = actual;
                        assert_eq!(*session.modulation, expected);
                        assert_eq!(*session.effect, effect);
                        assert!(session.response.changed);
                    }
                }
            });
        }

        #[test]
        fn beat_length_drag_snaps_to_musical_steps_and_preserves_seconds() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = session.modulation.clone();
                let effect = *session.effect;
                for (slot, index) in [(0, 0), (1, 1), (2, 2), (3, 3), (usize::MAX, 3)] {
                    session.state.selected_mseg = slot;
                    for (norm, beats) in [
                        (-1.0, 0.25),
                        (0.0, 0.25),
                        (0.4, 4.0),
                        (0.449, 4.0),
                        (0.451, 8.0),
                        (1.0, 256.0),
                        (2.0, 256.0),
                    ] {
                        *session.modulation = original.clone();
                        session.modulation.msegs[index].sync_mode = widgets::SyncMode::Beat;
                        let mut expected = session.modulation.clone();
                        expected.msegs[index].beats = beats;
                        session.response = EditorResponse::default();
                        session.apply_mseg_length_drag(norm);
                        assert_eq!(*session.modulation, expected, "slot {slot}, norm {norm}");
                        assert_eq!(*session.effect, effect);
                        assert!(session.response.changed);
                    }
                }
            });
        }

        #[test]
        fn free_hz_rate_drag_uses_the_log_range_and_preserves_other_configuration() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.modulation.trigger = TriggerSource::FreeHz { hz: 3.7 };
                let original = session.modulation.clone();
                let effect = *session.effect;
                for (norm, expected_hz) in [
                    (-0.5, 0.05),
                    (0.0, 0.05),
                    (0.25, 0.223_606_8),
                    (0.5, 1.0),
                    (1.0, 20.0),
                    (1.5, 20.0),
                ] {
                    *session.modulation = original.clone();
                    session.response = EditorResponse::default();
                    session.apply_trigger_rate_drag(norm);
                    let TriggerSource::FreeHz { hz } = session.modulation.trigger else {
                        panic!("rate drag changed trigger kind")
                    };
                    assert!((hz - expected_hz).abs() < 1e-5);
                    let mut expected = original.clone();
                    expected.trigger = TriggerSource::FreeHz { hz };
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(*session.effect, effect);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn sensitivity_drag_lowers_the_threshold_as_it_increases_and_preserves_hold() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.modulation.trigger = TriggerSource::Transient {
                    threshold: 3.2,
                    hold_ms: 73.0,
                };
                let original = session.modulation.clone();
                let effect = *session.effect;
                for (norm, expected_threshold) in [
                    (-0.5, 6.0),
                    (0.0, 6.0),
                    (0.5, 2.509_980_2),
                    (1.0, 1.05),
                    (1.5, 1.05),
                ] {
                    *session.modulation = original.clone();
                    session.response = EditorResponse::default();
                    session.apply_trigger_rate_drag(norm);
                    let TriggerSource::Transient { threshold, hold_ms } =
                        session.modulation.trigger
                    else {
                        panic!("sensitivity drag changed trigger kind")
                    };
                    assert!((threshold - expected_threshold).abs() < 1e-5);
                    assert_eq!(hold_ms, 73.0);
                    let mut expected = original.clone();
                    expected.trigger = TriggerSource::Transient {
                        threshold,
                        hold_ms: 73.0,
                    };
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(*session.effect, effect);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn hold_drag_uses_the_log_range_without_changing_sensitivity() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.modulation.trigger = TriggerSource::Transient {
                    threshold: 2.5,
                    hold_ms: 73.0,
                };
                let original = session.modulation.clone();
                let effect = *session.effect;
                for (norm, expected_hold) in [
                    (-0.5, 5.0),
                    (0.0, 5.0),
                    (0.25, 15.811_388),
                    (0.5, 50.0),
                    (1.0, 500.0),
                    (1.5, 500.0),
                ] {
                    *session.modulation = original.clone();
                    session.response = EditorResponse::default();
                    session.apply_trigger_aux_drag(norm);
                    let TriggerSource::Transient { threshold, hold_ms } =
                        session.modulation.trigger
                    else {
                        panic!("hold drag changed trigger kind")
                    };
                    assert_eq!(threshold, 2.5);
                    assert!((hold_ms - expected_hold).abs() < 1e-4);
                    let mut expected = original.clone();
                    expected.trigger = TriggerSource::Transient {
                        threshold: 2.5,
                        hold_ms,
                    };
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(*session.effect, effect);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn trigger_resets_restore_only_the_supported_control_default() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                for (before, rate, aux, rate_changed, aux_changed) in [
                    (
                        TriggerSource::Free,
                        TriggerSource::Free,
                        TriggerSource::Free,
                        false,
                        false,
                    ),
                    (
                        TriggerSource::CellLight,
                        TriggerSource::CellLight,
                        TriggerSource::CellLight,
                        false,
                        false,
                    ),
                    (
                        TriggerSource::CellStep,
                        TriggerSource::CellStep,
                        TriggerSource::CellStep,
                        false,
                        false,
                    ),
                    (
                        TriggerSource::FreeHz { hz: 3.7 },
                        TriggerSource::FreeHz { hz: 1.0 },
                        TriggerSource::FreeHz { hz: 3.7 },
                        true,
                        false,
                    ),
                    (
                        TriggerSource::Transient {
                            threshold: 2.5,
                            hold_ms: 73.0,
                        },
                        TriggerSource::Transient {
                            threshold: 1.5,
                            hold_ms: 73.0,
                        },
                        TriggerSource::Transient {
                            threshold: 2.5,
                            hold_ms: 50.0,
                        },
                        true,
                        true,
                    ),
                ] {
                    session.modulation.trigger = before;
                    let original = session.modulation.clone();
                    let effect = *session.effect;
                    session.response = EditorResponse::default();
                    session.reset_trigger_rate_to_default();
                    let mut expected = original.clone();
                    expected.trigger = rate;
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(session.response.changed, rate_changed);

                    *session.modulation = original.clone();
                    session.response = EditorResponse::default();
                    session.reset_trigger_aux_to_default();
                    expected.trigger = aux;
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(*session.effect, effect);
                    assert_eq!(session.response.changed, aux_changed);
                }
            });
        }

        #[test]
        fn inactive_trigger_drags_and_non_draggable_hits_preserve_state_and_dirty_flags() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                for (trigger, hits) in [
                    (
                        TriggerSource::Free,
                        &[EffectHit::TriggerRate, EffectHit::TriggerAux][..],
                    ),
                    (
                        TriggerSource::CellLight,
                        &[EffectHit::TriggerRate, EffectHit::TriggerAux][..],
                    ),
                    (
                        TriggerSource::CellStep,
                        &[EffectHit::TriggerRate, EffectHit::TriggerAux][..],
                    ),
                    (
                        TriggerSource::FreeHz { hz: 3.7 },
                        &[EffectHit::TriggerAux][..],
                    ),
                ] {
                    session.modulation.trigger = trigger;
                    let before = session.modulation.clone();
                    let effect = *session.effect;
                    for &hit in hits.iter().chain(
                        [
                            EffectHit::Back,
                            EffectHit::Kind,
                            EffectHit::Randomize,
                            EffectHit::Target,
                            EffectHit::Trigger,
                            EffectHit::MsegSelector(2),
                        ]
                        .iter(),
                    ) {
                        for dirty in [false, true] {
                            session.response.changed = dirty;
                            session.apply_drag(hit, 0.75);
                            assert_eq!(*session.modulation, before);
                            assert_eq!(*session.effect, effect);
                            assert_eq!(session.response.changed, dirty);
                        }
                    }
                }
            });
        }

        #[test]
        fn trigger_selection_carries_settings_within_a_family_and_defaults_across_families() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let effect = *session.effect;
                for (before, hz, threshold, hold_ms) in [
                    (TriggerSource::Free, 1.0, 1.5, 50.0),
                    (TriggerSource::CellLight, 1.0, 1.5, 50.0),
                    (TriggerSource::CellStep, 1.0, 1.5, 50.0),
                    (TriggerSource::FreeHz { hz: 3.7 }, 3.7, 1.5, 50.0),
                    (
                        TriggerSource::Transient {
                            threshold: 2.5,
                            hold_ms: 73.0,
                        },
                        1.0,
                        2.5,
                        73.0,
                    ),
                ] {
                    session.modulation.trigger = before;
                    let original = session.modulation.clone();
                    for (index, trigger) in [
                        TriggerSource::Free,
                        TriggerSource::CellLight,
                        TriggerSource::CellStep,
                        TriggerSource::FreeHz { hz },
                        TriggerSource::Transient { threshold, hold_ms },
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        *session.modulation = original.clone();
                        session.response = EditorResponse::default();
                        session.apply_trigger_selection(index);
                        let mut expected = original.clone();
                        expected.trigger = trigger;
                        assert_eq!(*session.modulation, expected);
                        assert_eq!(*session.effect, effect);
                        assert!(session.response.changed);
                    }
                }
            });
        }

        #[test]
        fn filtered_trigger_selection_uses_host_order_and_missing_items_fall_back_to_free() {
            with_session(
                &[
                    TriggerChoice::Transient,
                    TriggerChoice::FreeHz,
                    TriggerChoice::CellStep,
                ],
                EffectKind::Svf,
                |session| {
                    for (index, trigger) in [
                        (
                            0,
                            TriggerSource::Transient {
                                threshold: 1.5,
                                hold_ms: 50.0,
                            },
                        ),
                        (1, TriggerSource::FreeHz { hz: 2.0 }),
                        (2, TriggerSource::CellStep),
                        (3, TriggerSource::Free),
                        (usize::MAX, TriggerSource::Free),
                    ] {
                        session.modulation.trigger = TriggerSource::FreeHz { hz: 2.0 };
                        session.response = EditorResponse::default();
                        session.apply_trigger_selection(index);
                        assert_eq!(session.modulation.trigger, trigger);
                        assert!(session.response.changed);
                    }
                },
            );
            with_session(&[], EffectKind::Svf, |session| {
                session.modulation.trigger = TriggerSource::FreeHz { hz: 2.0 };
                session.apply_trigger_selection(0);
                assert_eq!(session.modulation.trigger, TriggerSource::Free);
                assert!(session.response.changed);
            });
        }

        #[test]
        fn drag_dispatch_applies_each_supported_control() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.apply_drag(EffectHit::Dial(1), 0.25);
                assert_eq!(session.effect.params[1], 0.25);
                assert!(session.response.changed);

                session.state.selected_mseg = 3;
                session.response = EditorResponse::default();
                session.apply_drag(EffectHit::Depth, 0.75);
                assert_eq!(session.modulation.depths[2], 0.5);
                assert!(session.response.changed);

                session.modulation.trigger = TriggerSource::FreeHz { hz: 3.7 };
                session.response = EditorResponse::default();
                session.apply_drag(EffectHit::TriggerRate, 0.5);
                let TriggerSource::FreeHz { hz } = session.modulation.trigger else {
                    panic!("rate drag changed trigger kind")
                };
                assert!((hz - 1.0).abs() < 1e-6);
                assert!(session.response.changed);

                session.modulation.trigger = TriggerSource::Transient {
                    threshold: 2.5,
                    hold_ms: 73.0,
                };
                session.response = EditorResponse::default();
                session.apply_drag(EffectHit::TriggerAux, 0.5);
                assert_eq!(
                    session.modulation.trigger,
                    TriggerSource::Transient {
                        threshold: 2.5,
                        hold_ms: 50.0
                    }
                );
                assert!(session.response.changed);

                session.modulation.msegs[3].sync_mode = widgets::SyncMode::Beat;
                session.response = EditorResponse::default();
                session.apply_drag(EffectHit::MsegLength, 0.4);
                assert_eq!(session.modulation.msegs[3].beats, 4.0);
                assert!(session.response.changed);

                session.response = EditorResponse::default();
                session.apply_drag(EffectHit::Mix, 0.37);
                assert_eq!(session.effect.mix, 0.37);
                assert!(session.response.changed);
            });
        }
    }

    mod editor_edits {
        use super::*;

        #[test]
        fn configuration_accessors_return_the_current_host_state() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.params[0] = 731.0;
                session.effect.mix = 0.37;
                session.effect.muted = true;
                session.effect.soloed = true;
                session.modulation.depths = [-0.2, 0.5, 0.9];
                session.modulation.targets[2] = Some(ModulationTarget::Mix);
                session.modulation.trigger = TriggerSource::Transient {
                    threshold: 2.5,
                    hold_ms: 80.0,
                };
                session.modulation.msegs[2].nodes[0].value = 0.3;
                let effect = *session.effect;
                let modulation = session.modulation.clone();

                assert_eq!(session.effect_config(), effect);
                assert_eq!(session.modulation_config(), &modulation);
                assert!(!session.response.changed);
            });
        }

        #[test]
        fn parameter_count_and_specs_follow_the_selected_effect() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                assert_eq!(session.effect_param_count(), 4);
                let cutoff = session.param_spec(0).unwrap();
                assert_eq!(cutoff.name, "Cutoff");
                assert_eq!(
                    (cutoff.min, cutoff.max, cutoff.default),
                    (20.0, 20_000.0, 2_000.0)
                );
                assert_eq!(cutoff.scaling, effects::ParamScaling::Log);
                assert_eq!(cutoff.format, effects::ParamFormat::Hertz);
                let poles = session.param_spec(3).unwrap();
                assert_eq!(poles.name, "Poles");
                assert_eq!((poles.min, poles.max, poles.default), (0.0, 3.0, 0.0));
                for index in [4, MAX_EFFECT_PARAMS, usize::MAX] {
                    assert!(session.param_spec(index).is_none());
                }
                session.effect.kind = EffectKind::None;
                assert_eq!(session.effect_param_count(), 0);
                assert!(session.param_spec(0).is_none());
            });
        }

        #[test]
        fn parameter_formats_distinguish_continuous_enum_and_missing_slots() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                use effects::ParamFormat;

                let types = &["Lowpass", "Bandpass", "Highpass"][..];
                let poles = &["2", "4", "6", "8"][..];
                assert_eq!(session.param_enum_labels(2), Some(types));
                assert_eq!(session.param_enum_labels(3), Some(poles));
                assert_eq!(
                    session.effect_param_format(2),
                    ParamFormat::Enum { labels: types }
                );
                assert_eq!(
                    session.effect_param_format(3),
                    ParamFormat::Enum { labels: poles }
                );
                assert_eq!(session.effect_param_format(0), ParamFormat::Hertz);
                assert_eq!(
                    session.effect_param_format(1),
                    ParamFormat::Number {
                        decimals: 2,
                        unit: ""
                    }
                );
                for index in [0, 1, 4, MAX_EFFECT_PARAMS, usize::MAX] {
                    assert_eq!(session.param_enum_labels(index), None);
                }
                let fallback = ParamFormat::Number {
                    decimals: 0,
                    unit: "",
                };
                for index in [4, MAX_EFFECT_PARAMS, usize::MAX] {
                    assert_eq!(session.effect_param_format(index), fallback);
                }
                session.effect.kind = EffectKind::None;
                assert_eq!(session.param_enum_labels(0), None);
                assert_eq!(session.effect_param_format(0), fallback);
            });
        }

        #[test]
        fn dial_edits_apply_linear_and_log_ranges_without_touching_other_state() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (index, norm, value) in [
                    (0, 0.0, 20.0),
                    (0, 1.0 / 3.0, 200.0),
                    (0, 1.0, 20_000.0),
                    (1, -0.5, 0.0),
                    (1, 0.25, 0.25),
                    (1, 1.5, 1.0),
                ] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.apply_effect_dial(index, norm);
                    assert!((session.effect.params[index] - value).abs() < 0.01);
                    let mut expected = original;
                    expected.params[index] = session.effect.params[index];
                    assert_eq!(*session.effect, expected);
                    assert_eq!(*session.modulation, modulation);
                    assert!(session.response.changed);
                }
                for index in [4, MAX_EFFECT_PARAMS, usize::MAX] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.apply_effect_dial(index, 0.75);
                    assert_eq!(*session.effect, original);
                    assert!(!session.response.changed);
                }
            });
        }

        #[test]
        fn dial_resets_restore_the_parameter_default_and_ignore_missing_specs() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.params = [7.0; MAX_EFFECT_PARAMS];
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (index, value) in [(0, 2_000.0), (1, 0.1), (2, 0.0), (3, 0.0)] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.reset_effect_dial_to_default(index);
                    let mut expected = original;
                    expected.params[index] = value;
                    assert_eq!(*session.effect, expected);
                    assert_eq!(*session.modulation, modulation);
                    assert!(session.response.changed);
                }
                for index in [4, MAX_EFFECT_PARAMS, usize::MAX] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.reset_effect_dial_to_default(index);
                    assert_eq!(*session.effect, original);
                    assert!(!session.response.changed);
                }
            });
        }

        #[test]
        fn parameter_selection_accepts_the_last_storage_slot_and_rejects_larger_indices() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (index, item) in [(2, 1), (3, 3), (MAX_EFFECT_PARAMS - 1, 2)] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.apply_param_dropdown_selection(index, item);
                    let mut expected = original;
                    expected.params[index] = item as f32;
                    assert_eq!(*session.effect, expected);
                    assert_eq!(*session.modulation, modulation);
                    assert!(session.response.changed);
                }
                for index in [MAX_EFFECT_PARAMS, usize::MAX] {
                    *session.effect = original;
                    session.response = EditorResponse::default();
                    session.apply_param_dropdown_selection(index, 1);
                    assert_eq!(*session.effect, original);
                    assert!(!session.response.changed);
                }
            });
        }

        #[test]
        fn depth_drag_maps_both_polarities_to_only_the_selected_assignable_slot() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.modulation.depths = [0.1, 0.2, 0.3];
                let original = session.modulation.clone();
                let effect = *session.effect;
                for slot in 0..4 {
                    session.state.selected_mseg = slot;
                    for (norm, depth) in [
                        (-0.25, -1.0),
                        (0.0, -1.0),
                        (0.25, -0.5),
                        (0.5, 0.0),
                        (0.75, 0.5),
                        (1.0, 1.0),
                        (1.25, 1.0),
                    ] {
                        *session.modulation = original.clone();
                        session.response = EditorResponse::default();
                        session.apply_depth_drag(norm);
                        let mut expected = original.clone();
                        if slot != 0 {
                            expected.depths[slot - 1] = depth;
                        }
                        assert_eq!(*session.modulation, expected, "slot {slot}, norm {norm}");
                        assert_eq!(*session.effect, effect);
                        assert_eq!(session.response.changed, slot != 0);
                    }
                }
            });
        }

        #[test]
        fn depth_reset_neutralizes_each_assignable_slot_and_leaves_amp_unchanged() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.modulation.depths = [-0.5, 0.25, 0.75];
                let original = session.modulation.clone();
                let effect = *session.effect;
                for slot in 0..4 {
                    session.state.selected_mseg = slot;
                    *session.modulation = original.clone();
                    session.response = EditorResponse::default();
                    session.reset_depth_to_default();
                    let mut expected = original.clone();
                    if slot != 0 {
                        expected.depths[slot - 1] = 0.0;
                    }
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(*session.effect, effect);
                    assert_eq!(session.response.changed, slot != 0);
                }
            });
        }

        #[test]
        fn target_selection_changes_only_the_selected_route_including_mix_and_none() {
            for kind in [EffectKind::Svf, EffectKind::None] {
                with_session(TriggerChoice::ALL, kind, |session| {
                    session.modulation.targets = [Some(ModulationTarget::Mix); 3];
                    let original = session.modulation.clone();
                    let effect = *session.effect;
                    let cases = if kind == EffectKind::Svf {
                        vec![
                            (0, None),
                            (1, Some(ModulationTarget::EffectParameter(0))),
                            (4, Some(ModulationTarget::EffectParameter(3))),
                            (5, Some(ModulationTarget::Mix)),
                            (6, None),
                            (usize::MAX, None),
                        ]
                    } else {
                        vec![(0, None), (1, Some(ModulationTarget::Mix)), (2, None)]
                    };
                    for slot in 0..4 {
                        session.state.selected_mseg = slot;
                        for &(item, target) in &cases {
                            *session.modulation = original.clone();
                            session.response = EditorResponse::default();
                            session.apply_target_selection(item);
                            let mut expected = original.clone();
                            if slot != 0 {
                                expected.targets[slot - 1] = target;
                            }
                            assert_eq!(*session.modulation, expected, "slot {slot}, item {item}");
                            assert_eq!(*session.effect, effect);
                            assert_eq!(session.response.changed, slot != 0);
                        }
                    }
                });
            }
        }

        #[test]
        fn mix_edits_clamp_to_the_blend_range_without_changing_routing() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (norm, mix) in [
                    (-1.0, 0.0),
                    (0.0, 0.0),
                    (0.25, 0.25),
                    (1.0, 1.0),
                    (2.0, 1.0),
                ] {
                    session.response = EditorResponse::default();
                    session.apply_mix(norm);
                    assert_eq!(*session.effect, TrackEffect { mix, ..original });
                    assert_eq!(*session.modulation, modulation);
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn mix_reset_restores_full_wet_and_reports_the_edit() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.mix = 0.2;
                let original = *session.effect;
                let modulation = session.modulation.clone();
                session.reset_mix_to_default();
                assert_eq!(
                    *session.effect,
                    TrackEffect {
                        mix: 1.0,
                        ..original
                    }
                );
                assert_eq!(*session.modulation, modulation);
                assert!(session.response.changed);
            });
        }

        #[test]
        fn kind_switch_resets_parameters_and_prunes_only_unavailable_routes() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                session.effect.params = [0.7; MAX_EFFECT_PARAMS];
                session.effect.mix = 0.4;
                session.effect.muted = true;
                session.effect.soloed = true;
                session.state.selected_mseg = 3;
                session.modulation.targets = [
                    Some(ModulationTarget::EffectParameter(0)),
                    Some(ModulationTarget::EffectParameter(4)),
                    Some(ModulationTarget::Mix),
                ];
                session.modulation.depths = [-0.25, 0.5, 0.75];
                session.modulation.trigger = TriggerSource::FreeHz { hz: 3.5 };
                let original = *session.effect;
                let modulation = session.modulation.clone();
                for (kind, targets) in [
                    (
                        EffectKind::Svf,
                        [
                            Some(ModulationTarget::EffectParameter(0)),
                            None,
                            Some(ModulationTarget::Mix),
                        ],
                    ),
                    (EffectKind::None, [None, None, Some(ModulationTarget::Mix)]),
                ] {
                    *session.effect = original;
                    *session.modulation = modulation.clone();
                    session.response = EditorResponse::default();
                    session.apply_kind_switch(kind);
                    assert_eq!(
                        *session.effect,
                        TrackEffect {
                            kind,
                            params: effects::default_params_for_kind(kind),
                            ..original
                        }
                    );
                    let mut expected = modulation.clone();
                    expected.targets = targets;
                    assert_eq!(*session.modulation, expected);
                    assert_eq!(session.state.selected_mseg, 3);
                    assert_eq!(
                        (
                            session.response.changed,
                            session.response.captured,
                            session.response.back,
                            session.response.randomize
                        ),
                        (true, false, false, false)
                    );
                }
            });
        }

        #[test]
        fn kind_menu_selection_clamps_indices_to_the_last_registered_effect() {
            with_session(TriggerChoice::ALL, EffectKind::Svf, |session| {
                let last = *EffectKind::ALL.last().unwrap();
                for (index, kind) in [
                    (0, EffectKind::None),
                    (1, EffectKind::ALL[1]),
                    (EffectKind::ALL.len() - 1, last),
                    (EffectKind::ALL.len(), last),
                    (usize::MAX, last),
                ] {
                    session.effect.params = [0.7; MAX_EFFECT_PARAMS];
                    session.response = EditorResponse::default();
                    session.apply_selection(EffectAction::Kind, index);
                    assert_eq!(session.effect.kind, kind);
                    assert_eq!(
                        session.effect.params,
                        effects::default_params_for_kind(kind)
                    );
                    assert!(session.response.changed);
                }
            });
        }

        #[test]
        fn selection_dispatch_routes_target_trigger_and_parameter_actions() {
            with_session(
                &[
                    TriggerChoice::Transient,
                    TriggerChoice::FreeHz,
                    TriggerChoice::Free,
                ],
                EffectKind::Svf,
                |session| {
                    session.state.selected_mseg = 3;
                    session.apply_selection(EffectAction::Target, 5);
                    assert_eq!(session.modulation.targets[2], Some(ModulationTarget::Mix));
                    assert!(session.response.changed);

                    session.response = EditorResponse::default();
                    session.apply_selection(EffectAction::ParamDropdown(2), 2);
                    assert_eq!(session.effect.params[2], 2.0);
                    assert!(session.response.changed);

                    session.response = EditorResponse::default();
                    session.apply_selection(EffectAction::Trigger, 0);
                    assert_eq!(
                        session.modulation.trigger,
                        TriggerSource::Transient {
                            threshold: crate::modulation::TRANSIENT_THRESHOLD_DEFAULT,
                            hold_ms: crate::modulation::TRANSIENT_HOLD_MS_DEFAULT
                        }
                    );
                    assert!(session.response.changed);
                },
            );
        }
    }

    #[test]
    fn mseg_color_returns_the_four_slot_hues_and_clamps_oob() {
        use tiny_skia::Color;
        let amp = mseg_color(0);
        let m1 = mseg_color(1);
        let m2 = mseg_color(2);
        let m3 = mseg_color(3);
        // Four distinct colours.
        let all = [amp, m1, m2, m3];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(rgb8(all[i]), rgb8(all[j]), "slots {i} and {j} share a hue");
            }
        }
        // Amp matches the existing accent (sky blue 0x4fc3f7).
        assert_eq!(rgb8(amp), (0x4f, 0xc3, 0xf7));
        // MSEG 1 amber.
        assert_eq!(rgb8(m1), (0xff, 0xc8, 0x58));
        // MSEG 2 purple.
        assert_eq!(rgb8(m2), (0xc3, 0x78, 0xff));
        // MSEG 3 mint.
        assert_eq!(rgb8(m3), (0x66, 0xd9, 0xa0));
        // OOB clamps to MSEG 3 (the highest valid slot).
        assert_eq!(rgb8(mseg_color(99)), rgb8(m3));

        fn rgb8(c: Color) -> (u8, u8, u8) {
            let r = (c.red() * 255.0).round() as u8;
            let g = (c.green() * 255.0).round() as u8;
            let b = (c.blue() * 255.0).round() as u8;
            (r, g, b)
        }
    }

    #[test]
    fn mix_node_tooltip_formats_the_effective_blend_as_a_percentage() {
        let data = widgets::MsegData::default();
        let track = TrackEffect {
            mix: 0.25,
            ..TrackEffect::default()
        };
        let target = ModulationTarget::Mix;
        let text = mseg_node_tooltip_text(
            1,
            &data,
            1,
            target.spec(track.kind),
            target.base_value(&track),
            Some((0.5, widgets::Polarity::Unipolar)),
        );
        assert!(text.ends_with("75 %"), "unexpected Mix tooltip: {text}");
    }

    #[test]
    fn node_tooltip_text_for_amp_has_position_line_and_db_readout() {
        let mut data = widgets::MsegData::default();
        // node 1 sits at time = 1.0, value = 1.0 by default.
        data.nodes[1].value = 1.0;
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        // Two lines: position then dial.
        assert_eq!(text, "X 100%  Y 1.00\n0.0 dB");

        data.nodes[1].value = 0.5;
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        // Position: X = 100% (default last-node time); Y = 0.50 (unipolar).
        // Dial: 20·log10(0.5) ≈ -6.02 dB.
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines[0], "X 100%  Y 0.50");
        assert!(
            lines[1].starts_with("-6.0"),
            "expected ~-6.0 dB on the dial line, got {}",
            lines[1]
        );

        data.nodes[1].value = 0.00001;
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        // The floor case keeps the position line and emits -∞ dB on the dial line.
        assert_eq!(text, "X 100%  Y 0.00\n-\u{221e} dB");
    }

    #[test]
    fn node_tooltip_text_bipolar_y_maps_value_to_minus_one_plus_one() {
        let mut data = widgets::MsegData {
            polarity: widgets::Polarity::Bipolar,
            ..widgets::MsegData::default()
        };
        data.nodes[1].value = 0.5; // midline → 0.00
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        let pos_line = text.split('\n').next().unwrap();
        assert!(
            pos_line.contains("Y +0.00"),
            "midline should display as +0.00 in bipolar, got {pos_line}"
        );

        data.nodes[1].value = 0.0; // bottom → -1.00
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        assert!(
            text.split('\n').next().unwrap().contains("Y -1.00"),
            "value 0 should display as -1.00 in bipolar, got {text}"
        );

        data.nodes[1].value = 1.0; // top → +1.00
        let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
        assert!(
            text.split('\n').next().unwrap().contains("Y +1.00"),
            "value 1 should display as +1.00 in bipolar, got {text}"
        );
    }

    #[test]
    fn node_tooltip_text_x_reflects_node_time() {
        let mut data = widgets::MsegData::default();
        // Insert a middle node at time 0.42; it lands at active-array index 1
        // and pushes the original endpoint to index 2.
        let i = data.insert_node(0.42, 0.5).unwrap();
        let text = mseg_node_tooltip_text(0, &data, i, None, 0.0, None);
        let pos_line = text.split('\n').next().unwrap();
        assert!(
            pos_line.starts_with("X 42%"),
            "X should reflect the node's time, got {pos_line}"
        );
    }

    #[test]
    fn node_tooltip_text_assignable_no_target_omits_the_dial_line() {
        let mut data = widgets::MsegData::default();
        data.nodes[1].value = 0.742;
        // Slot 1, no spec → position line only, no second line.
        let text = mseg_node_tooltip_text(1, &data, 1, None, 0.0, None);
        assert!(
            !text.contains('\n'),
            "no-target tooltip should be a single position line, got {text}"
        );
        assert_eq!(text, "X 100%  Y 0.74");
    }

    #[test]
    fn node_tooltip_text_out_of_range_node_returns_empty() {
        let data = widgets::MsegData::default();
        // Default MsegData has node_count == 2, so index 99 is OOB.
        let text = mseg_node_tooltip_text(0, &data, 99, None, 0.0, None);
        assert_eq!(text, "");
    }
}

#[cfg(test)]
mod interaction_tests;
