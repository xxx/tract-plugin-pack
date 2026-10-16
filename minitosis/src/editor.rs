//! Native window and host bindings around the shared effect editor.

use crate::{
    Minitosis, Shared,
    params::MinitosisParams,
    prepare_key,
    state::{EditSnapshot, bank_index, neutral_modulation},
};
use baseview::{
    Event, EventStatus, MouseButton, MouseEvent, WindowEvent, WindowOpenOptions, WindowScalePolicy,
};
use keyboard_types::{Key, KeyState, Modifiers};
use nih_plug::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tiny_skia_widgets as widgets;
use tract_effects::editor::{
    ClickTracker, EditorContext, EditorGeometry, EffectEditor, TriggerChoice, view,
};
use tract_effects::effects::{self, EffectKind, MAX_EFFECT_PARAMS, TrackEffect};
use tract_effects::undo::UndoHistory;

pub const WINDOW_WIDTH: u32 = 1140;
pub const WINDOW_HEIGHT: u32 = 704;
const TRIGGERS: &[TriggerChoice] = &[
    TriggerChoice::Free,
    TriggerChoice::FreeHz,
    TriggerChoice::Transient,
];
type Rect = (f32, f32, f32, f32);

#[derive(Default)]
pub struct FrameState<'a> {
    pub kind: Option<EffectKind>,
    pub output_edit: Option<(&'a str, bool)>,
}

struct SelectionState {
    confirmed: EffectKind,
    pending: Option<EffectKind>,
}

#[derive(PartialEq, Debug)]
enum SelectionChange {
    Unchanged,
    Acknowledged,
    External,
}

impl SelectionState {
    fn new(kind: EffectKind) -> Self {
        Self {
            confirmed: kind,
            pending: None,
        }
    }
    fn displayed(&self) -> EffectKind {
        self.pending.unwrap_or(self.confirmed)
    }
    fn request(&mut self, kind: EffectKind) -> bool {
        if kind == self.displayed() {
            return false;
        }
        self.pending = Some(kind);
        true
    }
    fn reconcile(&mut self, actual: EffectKind) -> SelectionChange {
        if self.pending == Some(actual) {
            self.pending = None;
            self.confirmed = actual;
            SelectionChange::Acknowledged
        } else if self.pending.is_some() {
            self.confirmed = actual;
            SelectionChange::Unchanged
        } else if self.confirmed != actual {
            self.confirmed = actual;
            SelectionChange::External
        } else {
            SelectionChange::Unchanged
        }
    }
}

pub fn geometry(scale: f32, width: u32, height: u32) -> EditorGeometry {
    let mut layout = view::effect_layout((16.0, 48.0, WINDOW_WIDTH as f32 - 32.0, 640.0), scale);
    layout.back = (0.0, 0.0, 0.0, 0.0);
    EditorGeometry {
        layout,
        scale,
        window_size: (width as f32, height as f32),
    }
}

fn output_rect(scale: f32) -> Rect {
    (230.0 * scale, 12.0 * scale, 220.0 * scale, 30.0 * scale)
}
fn reset_rect(scale: f32) -> Rect {
    (474.0 * scale, 12.0 * scale, 96.0 * scale, 30.0 * scale)
}
fn retrig_rect(scale: f32) -> Rect {
    (590.0 * scale, 12.0 * scale, 180.0 * scale, 30.0 * scale)
}
fn bypass_rect(scale: f32) -> Rect {
    (790.0 * scale, 12.0 * scale, 100.0 * scale, 30.0 * scale)
}

#[allow(clippy::too_many_arguments)]
pub fn draw_frame(
    pixmap: &mut tiny_skia::Pixmap,
    tr: &mut widgets::TextRenderer,
    params: &MinitosisParams,
    shared: &Shared,
    editor: &mut EffectEditor,
    scale: f32,
    frame: FrameState<'_>,
) {
    widgets::fill_pixmap_opaque(pixmap, widgets::color_bg());
    let geo = geometry(scale, pixmap.width(), pixmap.height());
    widgets::draw_rect(
        pixmap,
        16.0 * scale,
        48.0 * scale,
        (WINDOW_WIDTH as f32 - 32.0) * scale,
        640.0 * scale,
        widgets::color_control_bg(),
    );
    let kind = frame.kind.unwrap_or_else(|| params.selected_kind());
    let track = params
        .controls_for(kind)
        .track(params.mix.unmodulated_normalized_value(), true);
    let modulation = params
        .modulation
        .lock()
        .map(|bank| bank.get(track.kind).clone())
        .unwrap_or_else(|_| neutral_modulation());
    let phases = std::array::from_fn(|i| f32::from_bits(shared.phases[i].load(Ordering::Relaxed)));
    editor.draw(pixmap, tr, &track, &modulation, phases, geo);
    tr.draw_text(
        pixmap,
        24.0 * scale,
        34.0 * scale,
        "Minitosis",
        23.0 * scale,
        widgets::color_text(),
    );
    let r = output_rect(scale);
    widgets::controls::draw_slider(
        pixmap,
        tr,
        r.0,
        r.1,
        r.2,
        r.3,
        "Output",
        &params.output.to_string(),
        params.output.unmodulated_normalized_value(),
        frame.output_edit.map(|(text, _)| text),
        frame.output_edit.is_some_and(|(_, caret)| caret),
    );
    for (rect, label, active) in [
        (reset_rect(scale), "Reset", false),
        (
            retrig_rect(scale),
            "Retrig on play",
            params.retrig_on_play.value(),
        ),
        (bypass_rect(scale), "Bypass", params.bypass.value()),
    ] {
        widgets::controls::draw_button(
            pixmap, tr, rect.0, rect.1, rect.2, rect.3, label, active, false,
        );
    }
    if kind != params.selected_kind()
        || shared.active_generation.load(Ordering::Acquire)
            != params.selection_generation.load(Ordering::Acquire)
    {
        tr.draw_text(
            pixmap,
            916.0 * scale,
            32.0 * scale,
            "Preparing…",
            13.0 * scale,
            widgets::color_muted(),
        );
    }
    editor.draw_popup(pixmap, tr, &track, geo);
}

pub fn effect_editor() -> EffectEditor {
    EffectEditor::new(TRIGGERS)
}

struct MinitosisWindow {
    surface: widgets::SoftbufferSurface,
    width: u32,
    height: u32,
    scale: f32,
    pending_resize: Arc<AtomicU64>,
    params: Arc<MinitosisParams>,
    shared: Arc<Shared>,
    executor: AsyncExecutor<Minitosis>,
    gui_context: Arc<dyn GuiContext>,
    renderer: widgets::TextRenderer,
    editor: EffectEditor,
    pointer: (f32, f32),
    selection: SelectionState,
    undo: UndoHistory<EditSnapshot>,
    gestures: [bool; MAX_EFFECT_PARAMS],
    gesture_kind: EffectKind,
    mix_gesture: bool,
    output_drag: widgets::DragState<()>,
    output_text: widgets::TextEditState<()>,
    output_click: ClickTracker<()>,
    rng_seed: u32,
}

impl MinitosisWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        params: Arc<MinitosisParams>,
        shared: Arc<Shared>,
        executor: AsyncExecutor<Minitosis>,
        gui_context: Arc<dyn GuiContext>,
        pending_resize: Arc<AtomicU64>,
    ) -> Self {
        let (width, height) = params.editor_state.size();
        let scale = (width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
        let last_kind = params.selected_kind();
        let rng_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1)
            .max(1);
        Self {
            surface: widgets::SoftbufferSurface::new(window, width, height),
            width,
            height,
            scale,
            pending_resize,
            params,
            shared,
            executor,
            gui_context,
            renderer: widgets::TextRenderer::new(widgets::DEFAULT_FONT),
            editor: effect_editor(),
            pointer: (0.0, 0.0),
            selection: SelectionState::new(last_kind),
            undo: UndoHistory::new(),
            gestures: [false; MAX_EFFECT_PARAMS],
            gesture_kind: last_kind,
            mix_gesture: false,
            output_drag: widgets::DragState::new(),
            output_text: widgets::TextEditState::new(),
            output_click: ClickTracker::new(),
            rng_seed,
        }
    }

    fn snapshot(&self) -> EditSnapshot {
        let kind = self.selection.displayed();
        let modulation = self
            .params
            .modulation
            .lock()
            .map(|b| b.get(kind).clone())
            .unwrap_or_else(|_| neutral_modulation());
        EditSnapshot { kind, modulation }
    }

    fn restore(&mut self, snapshot: EditSnapshot) {
        if let Ok(mut bank) = self.params.modulation.lock() {
            bank.set(snapshot.kind, snapshot.modulation);
        }
        self.shared.modulation_dirty.store(true, Ordering::Release);
        self.select_kind(snapshot.kind);
        self.editor.cancel_interaction();
    }

    fn select_kind(&mut self, kind: EffectKind) {
        self.end_effect_gestures();
        if self.selection.request(kind) {
            let setter = ParamSetter::new(self.gui_context.as_ref());
            setter.begin_set_parameter(&self.params.effect);
            setter.set_parameter(&self.params.effect, bank_index(kind) as i32);
            setter.end_set_parameter(&self.params.effect);
            self.editor.cancel_interaction();
            self.sync_selection();
        }
    }

    fn end_effect_gestures(&mut self) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        for (active, param) in self
            .gestures
            .iter_mut()
            .zip(self.params.controls_for(self.gesture_kind).controls())
        {
            if *active {
                setter.end_set_parameter(param);
                *active = false;
            }
        }
        if self.mix_gesture {
            setter.end_set_parameter(&self.params.mix);
            self.mix_gesture = false;
        }
    }

    fn sync_selection(&mut self) {
        let current = self.params.selected_kind();
        match self.selection.reconcile(current) {
            SelectionChange::External => {
                self.end_effect_gestures();
                self.editor.cancel_interaction();
            }
            SelectionChange::Acknowledged => {
                self.executor
                    .execute_background(prepare_key(&self.params, &self.shared));
            }
            SelectionChange::Unchanged => {}
        }
    }

    fn write_controls(&mut self, before: &TrackEffect, after: &TrackEffect, dragging: bool) {
        if self.gesture_kind != after.kind {
            self.end_effect_gestures();
            self.gesture_kind = after.kind;
        }
        let setter = ParamSetter::new(self.gui_context.as_ref());
        for (i, (param, spec)) in self
            .params
            .controls_for(after.kind)
            .controls()
            .iter()
            .zip(effects::parameter_specs(after.kind))
            .enumerate()
        {
            if before.params[i] == after.params[i] {
                continue;
            }
            if !self.gestures[i] {
                setter.begin_set_parameter(*param);
            }
            setter.set_parameter(
                *param,
                effects::value_to_norm(after.params[i], spec.min, spec.max, spec.scaling),
            );
            if dragging {
                self.gestures[i] = true;
            } else {
                setter.end_set_parameter(*param);
                self.gestures[i] = false;
            }
        }
        if before.mix != after.mix {
            if !self.mix_gesture {
                setter.begin_set_parameter(&self.params.mix);
            }
            setter.set_parameter(&self.params.mix, after.mix);
            if dragging {
                self.mix_gesture = true;
            } else {
                setter.end_set_parameter(&self.params.mix);
                self.mix_gesture = false;
            }
        }
    }

    fn handle_effect(&mut self, event: &Event, finish_text: bool) -> bool {
        let before = self
            .params
            .controls_for(self.selection.displayed())
            .track(self.params.mix.unmodulated_normalized_value(), true);
        let mut track = before;
        let original_modulation = self.snapshot().modulation;
        let mut modulation = original_modulation.clone();
        let context = EditorContext {
            pointer_position: self.pointer,
            effect: &mut track,
            modulation: &mut modulation,
            text_renderer: &mut self.renderer,
            geometry: geometry(self.scale, self.width, self.height),
        };
        let response = if finish_text {
            self.editor.finish_text_edit(context)
        } else {
            self.editor.handle_event(event, context)
        };
        if response.randomize {
            self.rng_seed = self.rng_seed.wrapping_add(1);
            tract_effects::randomize::randomize_track_effect(
                &mut track,
                &mut modulation,
                &[],
                self.rng_seed,
            );
            if track.kind != before.kind {
                modulation = self
                    .params
                    .modulation
                    .lock()
                    .map(|b| b.get(track.kind).clone())
                    .unwrap_or_else(|_| neutral_modulation());
                let old_values = self.params.controls_for(track.kind).track(before.mix, true);
                self.write_controls(&old_values, &track, false);
            }
        }
        if track.kind != before.kind {
            self.select_kind(track.kind);
        } else {
            self.write_controls(
                &before,
                &track,
                matches!(event, Event::Mouse(MouseEvent::CursorMoved { .. })),
            );
            if modulation != original_modulation {
                if let Ok(mut bank) = self.params.modulation.lock() {
                    bank.set(track.kind, modulation);
                }
                self.shared.modulation_dirty.store(true, Ordering::Release);
            }
        }
        response.captured
    }

    fn commit_output_text(&mut self) {
        if let Some(((), text)) = self.output_text.commit()
            && let Some(norm) = self
                .params
                .output
                .string_to_normalized_value(&text)
                .filter(|v| v.is_finite())
        {
            let setter = ParamSetter::new(self.gui_context.as_ref());
            setter.begin_set_parameter(&self.params.output);
            setter.set_parameter_normalized(&self.params.output, norm);
            setter.end_set_parameter(&self.params.output);
        }
    }

    fn end_output_drag(&mut self) {
        if self.output_drag.end_drag().is_some() {
            ParamSetter::new(self.gui_context.as_ref()).end_set_parameter(&self.params.output);
        }
    }

    fn toolbar_press(&mut self) -> bool {
        let (x, y) = self.pointer;
        let setter = ParamSetter::new(self.gui_context.as_ref());
        if view::in_rect(output_rect(self.scale), x, y) {
            if self.output_click.check_and_update(()) {
                setter.begin_set_parameter(&self.params.output);
                setter.set_parameter(&self.params.output, 1.0);
                setter.end_set_parameter(&self.params.output);
            } else {
                setter.begin_set_parameter(&self.params.output);
                self.output_drag.begin_drag(
                    (),
                    self.params.output.unmodulated_normalized_value(),
                    false,
                );
            }
        } else if view::in_rect(reset_rect(self.scale), x, y) {
            self.shared.reset_modulation.store(true, Ordering::Release);
        } else if view::in_rect(retrig_rect(self.scale), x, y) {
            setter.begin_set_parameter(&self.params.retrig_on_play);
            setter.set_parameter(
                &self.params.retrig_on_play,
                !self.params.retrig_on_play.value(),
            );
            setter.end_set_parameter(&self.params.retrig_on_play);
        } else if view::in_rect(bypass_rect(self.scale), x, y) {
            setter.begin_set_parameter(&self.params.bypass);
            setter.set_parameter(&self.params.bypass, !self.params.bypass.value());
            setter.end_set_parameter(&self.params.bypass);
        } else {
            return false;
        }
        true
    }
}

impl baseview::WindowHandler for MinitosisWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        widgets::consume_pending_resize(&self.pending_resize, (self.width, self.height), window);
        self.sync_selection();
        let output_edit = self
            .output_text
            .active_for(&())
            .map(|text| (text, self.output_text.caret_visible()));
        draw_frame(
            &mut self.surface.pixmap,
            &mut self.renderer,
            &self.params,
            &self.shared,
            &mut self.editor,
            self.scale,
            FrameState {
                kind: Some(self.selection.displayed()),
                output_edit,
            },
        );
        self.surface.present();
    }

    fn on_event(&mut self, _window: &mut baseview::Window, event: Event) -> EventStatus {
        self.sync_selection();
        match &event {
            Event::Window(WindowEvent::Resized(info)) => {
                self.width = info.physical_size().width;
                self.height = info.physical_size().height;
                self.scale = (self.width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.surface
                    .resize_and_persist(self.width, self.height, &self.params.editor_state);
            }
            Event::Window(WindowEvent::Unfocused) => {
                self.end_effect_gestures();
                self.end_output_drag();
                self.commit_output_text();
                self.handle_effect(&event, true);
                self.editor.cancel_interaction();
                let after = self.snapshot();
                self.undo.commit_capture(&after);
            }
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.pointer = (position.x as f32, position.y as f32);
                self.output_drag.set_mouse(self.pointer.0, self.pointer.1);
                if self.output_drag.active_action().is_some()
                    && let Some(norm) = self.output_drag.update_drag(
                        modifiers.contains(Modifiers::SHIFT),
                        self.params.output.unmodulated_normalized_value(),
                    )
                {
                    ParamSetter::new(self.gui_context.as_ref())
                        .set_parameter_normalized(&self.params.output, norm);
                }
                self.handle_effect(&event, false);
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => {
                self.undo.begin_capture(self.snapshot());
                if self.output_text.is_active() {
                    if view::in_rect(output_rect(self.scale), self.pointer.0, self.pointer.1) {
                        self.output_text.cancel();
                    } else {
                        self.commit_output_text();
                    }
                }
                if !self.handle_effect(&event, false) {
                    self.toolbar_press();
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                let opened = self.undo.begin_capture(self.snapshot());
                if view::in_rect(output_rect(self.scale), self.pointer.0, self.pointer.1) {
                    if self.output_drag.active_action().is_none() {
                        self.handle_effect(&event, true);
                        self.commit_output_text();
                        self.output_text.begin(
                            (),
                            &format!("{:.1}", util::gain_to_db(self.params.output.value())),
                        );
                    }
                } else {
                    self.commit_output_text();
                    self.handle_effect(&event, false);
                }
                if opened {
                    let after = self.snapshot();
                    self.undo.commit_capture(&after);
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                self.handle_effect(&event, false);
                self.end_effect_gestures();
                self.end_output_drag();
                let after = self.snapshot();
                self.undo.commit_capture(&after);
            }
            Event::Keyboard(ev) if self.output_text.is_active() => {
                if ev.state != KeyState::Down {
                    return EventStatus::Captured;
                }
                match &ev.key {
                    Key::Character(s) => {
                        for c in s.chars() {
                            self.output_text.insert_char(c);
                        }
                    }
                    Key::Backspace => self.output_text.backspace(),
                    Key::Escape => self.output_text.cancel(),
                    Key::Enter => self.commit_output_text(),
                    _ => return EventStatus::Ignored,
                }
            }
            Event::Keyboard(ev)
                if !self.editor.captures_keyboard()
                    && ev.state == KeyState::Down
                    && ev.modifiers.contains(Modifiers::CONTROL) =>
            {
                let is_z = matches!(&ev.key, Key::Character(s) if s.eq_ignore_ascii_case("z"));
                let is_y = matches!(&ev.key, Key::Character(s) if s.eq_ignore_ascii_case("y"));
                if (is_z || is_y) && !self.undo.is_capturing() {
                    let current = self.snapshot();
                    let saved = if is_y || ev.modifiers.contains(Modifiers::SHIFT) {
                        self.undo.redo(current)
                    } else {
                        self.undo.undo(current)
                    };
                    if let Some(snapshot) = saved {
                        self.restore(snapshot);
                    }
                } else if !is_z && !is_y {
                    return EventStatus::Ignored;
                }
            }
            Event::Keyboard(_) => {
                let opened = self.undo.begin_capture(self.snapshot());
                let captured = self.handle_effect(&event, false);
                if opened {
                    let after = self.snapshot();
                    self.undo.commit_capture(&after);
                }
                if !captured {
                    return EventStatus::Ignored;
                }
            }
            _ => {
                self.handle_effect(&event, false);
            }
        }
        EventStatus::Captured
    }
}

impl Drop for MinitosisWindow {
    fn drop(&mut self) {
        self.end_effect_gestures();
        self.end_output_drag();
    }
}

struct MinitosisEditor {
    params: Arc<MinitosisParams>,
    shared: Arc<Shared>,
    executor: AsyncExecutor<Minitosis>,
    pending_resize: Arc<AtomicU64>,
}

pub fn create(
    params: Arc<MinitosisParams>,
    shared: Arc<Shared>,
    executor: AsyncExecutor<Minitosis>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MinitosisEditor {
        params,
        shared,
        executor,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for MinitosisEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let params = self.params.clone();
        let shared = self.shared.clone();
        let executor = self.executor.clone();
        let pending_resize = self.pending_resize.clone();
        let (width, height) = self.params.editor_state.size();
        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: "Minitosis".into(),
                size: baseview::Size::new(width as f64, height as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                MinitosisWindow::new(window, params, shared, executor, context, pending_resize)
            },
        );
        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }
    fn size(&self) -> (u32, u32) {
        self.params.editor_state.size()
    }
    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }
    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.pending_resize
            .store(((width as u64) << 32) | height as u64, Ordering::Relaxed);
        true
    }
    fn param_value_changed(&self, _id: &str, _value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _offset: f32) {}
    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_removes_navigation_and_fits_all_controls() {
        let geo = geometry(1.0, WINDOW_WIDTH, WINDOW_HEIGHT);
        assert_eq!(geo.layout.back, (0.0, 0.0, 0.0, 0.0));
        for rect in [geo.layout.mseg_length, geo.layout.mix, geo.layout.mseg_pane] {
            assert!(rect.0 + rect.2 <= WINDOW_WIDTH as f32);
            assert!(rect.1 + rect.3 <= WINDOW_HEIGHT as f32);
        }
        let larger = geometry(2.0, WINDOW_WIDTH * 2, WINDOW_HEIGHT * 2);
        assert_eq!(larger.layout.mseg_pane.2, geo.layout.mseg_pane.2 * 2.0);
    }
    #[test]
    fn deferred_host_selection_is_displayed_and_undoable_before_echo() {
        let mut selection = SelectionState::new(EffectKind::None);
        let mut history = UndoHistory::new();
        history.begin_capture(selection.displayed());
        assert!(selection.request(EffectKind::Svf));
        assert_eq!(
            selection.reconcile(EffectKind::None),
            SelectionChange::Unchanged
        );
        assert_eq!(selection.displayed(), EffectKind::Svf);
        history.commit_capture(&selection.displayed());
        assert_eq!(history.undo(selection.displayed()), Some(EffectKind::None));
        assert_eq!(
            selection.reconcile(EffectKind::Svf),
            SelectionChange::Acknowledged
        );
        assert_eq!(selection.displayed(), EffectKind::Svf);
    }

    #[test]
    fn intermediate_host_echo_does_not_override_the_latest_gui_selection() {
        let mut selection = SelectionState::new(EffectKind::None);
        selection.request(EffectKind::Svf);
        selection.request(EffectKind::Delay);
        assert_eq!(
            selection.reconcile(EffectKind::Svf),
            SelectionChange::Unchanged
        );
        assert_eq!(selection.displayed(), EffectKind::Delay);
        assert_eq!(
            selection.reconcile(EffectKind::Delay),
            SelectionChange::Acknowledged
        );
        assert_eq!(
            selection.reconcile(EffectKind::Reverb),
            SelectionChange::External
        );
        assert_eq!(selection.displayed(), EffectKind::Reverb);
    }
}
