//! Native CPU-rendered editor and the shared offscreen preview renderer.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};
use tiny_skia_widgets as widgets;

use crate::dsp::RESOLUTIONS;
use crate::pattern::{MAX_STEPS, Pattern, PatternBank, Random};
use crate::{GatedParams, Telemetry};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 840;
const RING_X: f32 = 240.0;
const RING_Y: f32 = 284.0;
const OUTER: f32 = 180.0;
const INNER: f32 = 137.0;

pub fn default_editor_state() -> Arc<widgets::EditorState> {
    widgets::EditorState::from_size(WIDTH, HEIGHT)
}

fn accent() -> Color {
    Color::from_rgba8(100, 201, 183, 255)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dial {
    Attack,
    Decay,
    Sustain,
    Release,
    Gate,
    Swing,
    Mix,
    Density,
    Ties,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Dial(Dial),
    Slot(usize),
    Length(usize),
    Copy,
    Paste,
    Rotate(bool),
    Undo,
    Redo,
    Randomize,
    Resolution,
    Bypass,
}

fn param(params: &GatedParams, dial: Dial) -> &FloatParam {
    match dial {
        Dial::Attack => &params.attack,
        Dial::Decay => &params.decay,
        Dial::Sustain => &params.sustain,
        Dial::Release => &params.release,
        Dial::Gate => &params.gate,
        Dial::Swing => &params.swing,
        Dial::Mix => &params.mix,
        Dial::Density => &params.density,
        Dial::Ties => &params.tie_probability,
    }
}

#[derive(Default)]
struct History {
    undo: Vec<PatternBank>,
    redo: Vec<PatternBank>,
    pending: Option<PatternBank>,
}

impl History {
    fn begin(&mut self, before: PatternBank) {
        self.pending.get_or_insert(before);
    }
    fn finish(&mut self, after: PatternBank) -> bool {
        if let Some(before) = self.pending.take()
            && before != after
        {
            if self.undo.len() == 128 {
                self.undo.remove(0);
            }
            self.undo.push(before);
            self.redo.clear();
            return true;
        }
        false
    }
    fn restore(&mut self, current: PatternBank, redo: bool) -> Option<PatternBank> {
        let (source, destination) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let before = source.pop()?;
        destination.push(current);
        Some(before)
    }
}

#[derive(Clone, Copy)]
struct RingGesture {
    slot: usize,
    first: usize,
    last: usize,
    moved: bool,
    erase: bool,
}

/// Coordinates are transformed once, identically for drawing and hit testing.
#[derive(Clone, Copy)]
struct Layout {
    scale: f32,
    x: f32,
    y: f32,
}

impl Layout {
    fn new(width: u32, height: u32) -> Self {
        let scale = (width as f32 / WIDTH as f32)
            .min(height as f32 / HEIGHT as f32)
            .max(0.01);
        Self {
            scale,
            x: (width as f32 - WIDTH as f32 * scale) * 0.5,
            y: (height as f32 - HEIGHT as f32 * scale) * 0.5,
        }
    }
    fn point(self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y + y * self.scale)
    }
    fn rect(self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let (x, y) = self.point(x, y);
        (x, y, w * self.scale, h * self.scale)
    }
    fn ring_step(self, x: f32, y: f32, length: usize) -> Option<usize> {
        let dx = (x - self.x) / self.scale - RING_X;
        let dy = (y - self.y) / self.scale - RING_Y;
        if !(INNER..=OUTER).contains(&dx.hypot(dy)) {
            return None;
        }
        let phase = (dy.atan2(dx) + FRAC_PI_2).rem_euclid(TAU) / TAU;
        Some(((phase * length as f32) as usize).min(length - 1))
    }
}

struct Panel {
    params: Arc<GatedParams>,
    telemetry: Arc<Telemetry>,
    text: widgets::TextRenderer,
    drag: widgets::DragState<Action>,
    text_edit: widgets::TextEditState<Action>,
    dropdown: widgets::DropdownState<Action>,
    history: History,
    clipboard: Option<Pattern>,
    gesture: Option<RingGesture>,
    observed_bank: PatternBank,
    observed_generation: u64,
    cancelled_dial: Option<Dial>,
    rng: Random,
    layout: Layout,
    size: (f32, f32),
}

impl Panel {
    fn new(params: Arc<GatedParams>, telemetry: Arc<Telemetry>) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(42, |time| time.as_nanos() as u64);
        let observed_bank = *params
            .bank
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let observed_generation = params.state_generation.load(Ordering::Acquire);
        Self {
            params,
            telemetry,
            text: widgets::TextRenderer::new(widgets::DEFAULT_FONT),
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            dropdown: widgets::DropdownState::new(),
            history: History::default(),
            clipboard: None,
            gesture: None,
            observed_bank,
            observed_generation,
            cancelled_dial: None,
            rng: Random::new(seed),
            layout: Layout::new(WIDTH, HEIGHT),
            size: (WIDTH as f32, HEIGHT as f32),
        }
    }

    fn bank(&self) -> PatternBank {
        *self
            .params
            .bank
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
    fn slot(&self) -> usize {
        (self.params.pattern.value() - 1) as usize
    }

    fn reconcile(&mut self) {
        let bank = self.bank();
        let generation = self.params.state_generation.load(Ordering::Acquire);
        if bank != self.observed_bank || generation != self.observed_generation {
            self.history = History::default();
            self.gesture = None;
            self.text_edit.cancel();
            self.dropdown.close();
            if let Some(Action::Dial(dial)) = self.drag.end_drag() {
                self.cancelled_dial = Some(dial);
            }
            self.observed_bank = bank;
            self.observed_generation = generation;
        }
    }

    fn finish_cancelled_dial(&mut self, context: &dyn GuiContext) {
        if let Some(dial) = self.cancelled_dial.take() {
            ParamSetter::new(context).end_set_parameter(param(&self.params, dial));
        }
    }

    fn text(&mut self, image: &mut Pixmap, x: f32, y: f32, label: &str, size: f32, color: Color) {
        let (x, y) = self.layout.point(x, y);
        self.text
            .draw_text(image, x, y, label, size * self.layout.scale, color);
    }

    fn centered(
        &mut self,
        image: &mut Pixmap,
        x: f32,
        y: f32,
        label: &str,
        size: f32,
        color: Color,
    ) {
        let width = self.text.text_width(label, size * self.layout.scale) / self.layout.scale;
        self.text(image, x - width * 0.5, y, label, size, color);
    }

    fn button(
        &mut self,
        image: &mut Pixmap,
        rect: (f32, f32, f32, f32),
        label: &str,
        action: Action,
        selected: bool,
        enabled: bool,
    ) {
        let (x, y, w, h) = self.layout.rect(rect.0, rect.1, rect.2, rect.3);
        let color = if selected {
            accent()
        } else {
            widgets::color_control_bg()
        };
        widgets::draw_rect(image, x, y, w, h, color);
        let ink = if !enabled {
            widgets::color_border()
        } else if selected {
            widgets::color_bg()
        } else {
            widgets::color_text()
        };
        self.centered(
            image,
            rect.0 + rect.2 * 0.5,
            rect.1 + rect.3 * 0.5 + 4.5,
            label,
            13.0,
            ink,
        );
        if enabled {
            self.drag.push_region(x, y, w, h, action);
        }
    }

    fn sector(
        &self,
        image: &mut Pixmap,
        first: f32,
        last: f32,
        inner: f32,
        outer: f32,
        color: Color,
    ) {
        let mut path = PathBuilder::new();
        let pieces = ((last - first) * 45.0).ceil().max(2.0) as usize;
        for i in 0..=pieces {
            let angle = first + (last - first) * i as f32 / pieces as f32 - FRAC_PI_2;
            let (x, y) = self
                .layout
                .point(RING_X + outer * angle.cos(), RING_Y + outer * angle.sin());
            if i == 0 {
                path.move_to(x, y);
            } else {
                path.line_to(x, y);
            }
        }
        for i in (0..=pieces).rev() {
            let angle = first + (last - first) * i as f32 / pieces as f32 - FRAC_PI_2;
            let (x, y) = self
                .layout
                .point(RING_X + inner * angle.cos(), RING_Y + inner * angle.sin());
            path.line_to(x, y);
        }
        path.close();
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            paint.set_color(color);
            image.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn draw(&mut self, image: &mut Pixmap) {
        self.layout = Layout::new(image.width(), image.height());
        self.size = (image.width() as f32, image.height() as f32);
        self.drag.clear_regions();
        self.reconcile();
        let bank = self.observed_bank;
        let slot = self.slot();
        let pattern = bank.patterns[slot];
        image.fill(widgets::color_bg());
        self.text(image, 24.0, 34.0, "gated", 25.0, widgets::color_text());
        self.button(
            image,
            (382.0, 13.0, 74.0, 27.0),
            "Bypass",
            Action::Bypass,
            self.params.bypass.value(),
            true,
        );
        for index in 0..8 {
            self.button(
                image,
                (24.0 + index as f32 * 54.0, 56.0, 50.0, 31.0),
                &(index + 1).to_string(),
                Action::Slot(index),
                index == slot,
                true,
            );
        }
        let sounding = self.telemetry.step.load(Ordering::Relaxed);
        let step_angle = TAU / pattern.len() as f32;
        for i in 0..pattern.len() {
            let first = i as f32 * step_angle;
            let last = first + step_angle;
            let gap = 0.012_f32.min(step_angle * 0.12);
            let left_gap = if pattern.tied((i + pattern.len() - 1) % pattern.len()) {
                0.0
            } else {
                gap
            };
            let right_gap = if pattern.tied(i) { 0.0 } else { gap };
            let color = if pattern.is_on(i) {
                accent()
            } else {
                Color::from_rgba8(57, 64, 72, 255)
            };
            self.sector(
                image,
                first + left_gap,
                last - right_gap,
                INNER,
                OUTER,
                color,
            );
            if sounding == i as u32 {
                self.sector(
                    image,
                    first + gap,
                    last - gap,
                    OUTER + 5.0,
                    OUTER + 9.0,
                    Color::from_rgba8(235, 232, 208, 255),
                );
            }
            if pattern.len() <= 32 || i % 4 == 0 {
                let angle = (first + last) * 0.5 - FRAC_PI_2;
                self.centered(
                    image,
                    RING_X + 120.0 * angle.cos(),
                    RING_Y + 120.0 * angle.sin() + 4.0,
                    &(i + 1).to_string(),
                    10.0,
                    widgets::color_muted(),
                );
            }
        }
        for i in 0..pattern.len() {
            if pattern.tied(i) {
                let boundary = (i + 1) as f32 * step_angle;
                self.sector(
                    image,
                    boundary - 0.012,
                    boundary + 0.012,
                    INNER,
                    OUTER,
                    accent(),
                );
            }
        }
        let (x, y, w, h) = self.layout.rect(172.0, 218.0, 136.0, 92.0);
        self.drag.push_region(x, y, w, h, Action::Length(slot));
        let length_hovered = self.drag.mouse_in_window()
            && self
                .drag
                .hit_test()
                .is_some_and(|region| matches!(region.action, Action::Length(_)));
        let length_dragging = matches!(self.drag.active_action(), Some(Action::Length(_)));
        self.centered(
            image,
            RING_X,
            278.0,
            &pattern.len().to_string(),
            57.0,
            if length_hovered || length_dragging {
                accent()
            } else {
                widgets::color_text()
            },
        );
        self.centered(image, RING_X, 300.0, "steps", 13.0, widgets::color_muted());
        if length_hovered || length_dragging {
            self.centered(
                image,
                RING_X,
                333.0,
                "drag ↕ · Shift for fine",
                11.0,
                widgets::color_muted(),
            );
        }
        let status = if sounding == u32::MAX {
            "Stopped · audio passes through"
        } else if self.telemetry.song_position.load(Ordering::Relaxed) == 0 {
            "Tempo sync · no song position"
        } else {
            "DAW sync"
        };
        self.centered(image, RING_X, 369.0, status, 11.0, widgets::color_muted());
        let buttons = [("←", Action::Rotate(false)), ("→", Action::Rotate(true))];
        for (i, (label, action)) in buttons.into_iter().enumerate() {
            self.button(
                image,
                (168.0 + i as f32 * 76.0, 486.0, 68.0, 27.0),
                label,
                action,
                false,
                true,
            );
        }
        let dials = [
            (Dial::Attack, "Attack", 66.0, 566.0, 28.0),
            (Dial::Decay, "Decay", 182.0, 566.0, 28.0),
            (Dial::Sustain, "Sustain", 298.0, 566.0, 28.0),
            (Dial::Release, "Release", 414.0, 566.0, 28.0),
            (Dial::Gate, "Gate", 242.0, 670.0, 25.0),
            (Dial::Swing, "Swing", 330.0, 670.0, 25.0),
            (Dial::Mix, "Mix", 418.0, 670.0, 25.0),
            (Dial::Density, "Density", 242.0, 776.0, 24.0),
            (Dial::Ties, "Tie probability", 384.0, 776.0, 24.0),
        ];
        for (dial, label, x, y, radius) in dials {
            let p = param(&self.params, dial);
            let normalized = p.modulated_normalized_value();
            let value = p.normalized_value_to_string(normalized, true);
            let editing = self
                .text_edit
                .active_for(&Action::Dial(dial))
                .map(str::to_owned);
            let (cx, cy) = self.layout.point(x, y);
            let r = radius * self.layout.scale;
            widgets::draw_dial_ex(
                image,
                &mut self.text,
                cx,
                cy,
                r,
                label,
                &value,
                normalized,
                None,
                editing.as_deref(),
                self.text_edit.caret_visible(),
                accent(),
            );
            let (rx, ry, rw, rh) = self.layout.rect(
                x - radius - 10.0,
                y - radius - 20.0,
                2.0 * radius + 20.0,
                2.0 * radius + 45.0,
            );
            self.drag.push_region(rx, ry, rw, rh, Action::Dial(dial));
        }
        self.text(
            image,
            24.0,
            646.0,
            "Resolution",
            13.0,
            widgets::color_text(),
        );
        let resolution_rect = self.layout.rect(24.0, 658.0, 150.0, 29.0);
        widgets::draw_dropdown_trigger(
            image,
            &mut self.text,
            resolution_rect,
            RESOLUTIONS[self.params.resolution.value() as usize],
            self.dropdown.is_open(),
        );
        self.drag.push_region(
            resolution_rect.0,
            resolution_rect.1,
            resolution_rect.2,
            resolution_rect.3,
            Action::Resolution,
        );
        let (x, y, w, h) = self.layout.rect(24.0, 719.0, 432.0, 1.0);
        widgets::draw_rect(image, x, y, w, h, widgets::color_control_bg());
        self.button(
            image,
            (24.0, 759.0, 142.0, 34.0),
            "Randomize",
            Action::Randomize,
            true,
            true,
        );
        self.text(
            image,
            24.0,
            813.0,
            "Current pattern",
            11.0,
            widgets::color_muted(),
        );
        widgets::draw_dropdown_popup(
            image,
            &mut self.text,
            &self.dropdown,
            &RESOLUTIONS,
            self.size,
        );
    }

    fn notify_edit(&self, context: &dyn GuiContext) {
        let setter = ParamSetter::new(context);
        let revision = &self.params.revision;
        setter.begin_set_parameter(revision);
        setter.set_parameter(
            revision,
            if revision.value() == i32::MAX {
                0
            } else {
                revision.value() + 1
            },
        );
        setter.end_set_parameter(revision);
    }

    fn update(&mut self, slot: usize, edit: impl FnOnce(&mut Pattern)) {
        let mut bank = self
            .params
            .bank
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *bank != self.observed_bank
            || self.params.state_generation.load(Ordering::Acquire) != self.observed_generation
        {
            drop(bank);
            self.reconcile();
            return;
        }
        edit(&mut bank.patterns[slot]);
        self.observed_bank = *bank;
    }

    fn commit(&mut self, context: &dyn GuiContext) {
        if self.history.finish(self.bank()) {
            self.notify_edit(context);
        }
    }

    fn begin_length_drag(&mut self, slot: usize, shift: bool) {
        let bank = self.bank();
        self.history.begin(bank);
        let normalized = (bank.patterns[slot].len() - 1) as f32 / (MAX_STEPS - 1) as f32;
        self.drag
            .begin_drag(Action::Length(slot), normalized, shift);
    }

    fn update_length_drag(&mut self, slot: usize, shift: bool) {
        let length = self.bank().patterns[slot].len();
        let normalized = (length - 1) as f32 / (MAX_STEPS - 1) as f32;
        let distance = 4.0 * (MAX_STEPS - 1) as f32 * self.layout.scale;
        if let Some(value) = self
            .drag
            .update_drag_with_sensitivity(shift, normalized, distance)
        {
            let length = 1 + (value * (MAX_STEPS - 1) as f32).round() as usize;
            self.update(slot, |pattern| pattern.set_length(length));
        }
    }

    fn end_gesture(&mut self, context: &dyn GuiContext, click: bool) {
        if let Some(gesture) = self.gesture.take() {
            if click && !gesture.moved && !gesture.erase {
                self.update(gesture.slot, |p| {
                    p.set_on(gesture.first, !p.is_on(gesture.first))
                });
            }
            self.commit(context);
        }
        match self.drag.end_drag() {
            Some(Action::Dial(dial)) => {
                ParamSetter::new(context).end_set_parameter(param(&self.params, dial))
            }
            Some(Action::Length(_)) => self.commit(context),
            _ => {}
        }
        self.dropdown.on_mouse_up();
    }

    fn commit_text(&mut self, context: &dyn GuiContext) {
        if let Some((Action::Dial(dial), text)) = self.text_edit.commit() {
            let p = param(&self.params, dial);
            if let Some(value) = p.string_to_normalized_value(&text) {
                let setter = ParamSetter::new(context);
                setter.begin_set_parameter(p);
                setter.set_parameter_normalized(p, value);
                setter.end_set_parameter(p);
            }
        }
    }

    fn select_resolution(
        &mut self,
        event: Option<widgets::DropdownEvent<Action>>,
        context: &dyn GuiContext,
    ) {
        if let Some(widgets::DropdownEvent::Selected(_, index)) = event {
            let setter = ParamSetter::new(context);
            setter.begin_set_parameter(&self.params.resolution);
            setter.set_parameter(&self.params.resolution, index as i32);
            setter.end_set_parameter(&self.params.resolution);
        }
    }

    fn action(&mut self, action: Action, context: &dyn GuiContext, shift: bool) {
        let setter = ParamSetter::new(context);
        let slot = self.slot();
        match action {
            Action::Length(slot) => self.begin_length_drag(slot, shift),
            Action::Dial(dial) => {
                let p = param(&self.params, dial);
                setter.begin_set_parameter(p);
                if self.drag.check_double_click(&action) {
                    setter.set_parameter_normalized(p, p.default_normalized_value());
                    setter.end_set_parameter(p);
                } else {
                    self.drag
                        .begin_drag(action, p.modulated_normalized_value(), shift);
                }
            }
            Action::Slot(index) => {
                setter.begin_set_parameter(&self.params.pattern);
                setter.set_parameter(&self.params.pattern, index as i32 + 1);
                setter.end_set_parameter(&self.params.pattern);
            }
            Action::Bypass => {
                setter.begin_set_parameter(&self.params.bypass);
                setter.set_parameter(&self.params.bypass, !self.params.bypass.value());
                setter.end_set_parameter(&self.params.bypass);
            }
            Action::Resolution => {
                self.dropdown.open(
                    action,
                    self.layout.rect(24.0, 658.0, 150.0, 29.0),
                    widgets::DropdownList::flat(&RESOLUTIONS),
                    self.params.resolution.value() as usize,
                    false,
                    self.size,
                );
            }
            Action::Copy => {
                self.clipboard = Some(self.bank().patterns[slot]);
            }
            Action::Undo | Action::Redo => {
                let mut current = self
                    .params
                    .bank
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if *current != self.observed_bank
                    || self.params.state_generation.load(Ordering::Acquire)
                        != self.observed_generation
                {
                    drop(current);
                    self.reconcile();
                    return;
                }
                if let Some(bank) = self.history.restore(*current, action == Action::Redo) {
                    *current = bank;
                    self.observed_bank = bank;
                    drop(current);
                    self.notify_edit(context);
                }
            }
            _ => {
                self.history.begin(self.bank());
                match action {
                    Action::Paste => {
                        if let Some(pattern) = self.clipboard {
                            self.update(slot, |p| *p = pattern);
                        }
                    }
                    Action::Rotate(right) => self.update(slot, |p| p.rotate(right)),
                    Action::Randomize => {
                        let density = self.params.density.value();
                        let ties = self.params.tie_probability.value();
                        let mut p = self.bank().patterns[slot];
                        p.randomize(&mut self.rng, density, ties);
                        self.update(slot, |pattern| *pattern = p);
                    }
                    _ => {}
                }
                self.commit(context);
            }
        }
    }

    fn event(
        &mut self,
        event: &baseview::Event,
        context: &dyn GuiContext,
    ) -> baseview::EventStatus {
        self.reconcile();
        self.finish_cancelled_dial(context);
        use baseview::{Event, MouseButton, MouseEvent, WindowEvent};
        use keyboard_types::{Key, KeyState, Modifiers};
        match event {
            Event::Mouse(MouseEvent::CursorEntered) => self.drag.on_cursor_entered(),
            Event::Mouse(MouseEvent::CursorLeft) => self.drag.on_cursor_left(),
            Event::Window(WindowEvent::Unfocused | WindowEvent::WillClose) => {
                self.end_gesture(context, false);
                self.commit_text(context);
                self.dropdown.close();
            }
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                let (x, y) = (position.x as f32, position.y as f32);
                self.drag.set_mouse(x, y);
                if self.dropdown.is_open() {
                    self.dropdown.on_mouse_move(x, y, &RESOLUTIONS, self.size);
                } else if let Some(Action::Dial(dial)) = self.drag.active_action().copied() {
                    let p = param(&self.params, dial);
                    if let Some(value) = self.drag.update_drag(
                        modifiers.contains(Modifiers::SHIFT),
                        p.modulated_normalized_value(),
                    ) {
                        ParamSetter::new(context).set_parameter_normalized(p, value);
                    }
                } else if let Some(Action::Length(slot)) = self.drag.active_action().copied() {
                    self.update_length_drag(slot, modifiers.contains(Modifiers::SHIFT));
                } else if let Some(mut gesture) = self.gesture {
                    let len = self.bank().patterns[gesture.slot].len();
                    if let Some(step) = self.layout.ring_step(x, y, len)
                        && step != gesture.last
                    {
                        self.update(gesture.slot, |p| {
                            paint_path(p, gesture.last, step, gesture.erase)
                        });
                        if self.gesture.is_some() {
                            gesture.last = step;
                            gesture.moved = true;
                            self.gesture = Some(gesture);
                        }
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers,
            }) => {
                self.commit_text(context);
                let (x, y) = self.drag.mouse_pos();
                if self.dropdown.is_open() {
                    let selected = self.dropdown.on_mouse_down(x, y, &RESOLUTIONS, self.size);
                    self.select_resolution(selected, context);
                } else {
                    self.end_gesture(context, false);
                    if let Some(region) = self.drag.hit_test().cloned() {
                        self.action(region.action, context, modifiers.contains(Modifiers::SHIFT));
                    } else {
                        let slot = self.slot();
                        let len = self.bank().patterns[slot].len();
                        if let Some(step) = self.layout.ring_step(x, y, len) {
                            let erase = modifiers.contains(Modifiers::SHIFT);
                            self.history.begin(self.bank());
                            self.gesture = Some(RingGesture {
                                slot,
                                first: step,
                                last: step,
                                moved: false,
                                erase,
                            });
                            if erase {
                                self.update(slot, |p| p.set_on(step, false));
                            }
                        }
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                if self.drag.active_action().is_some()
                    || self.gesture.is_some()
                    || self.dropdown.is_open()
                {
                    return baseview::EventStatus::Captured;
                }
                self.commit_text(context);
                if let Some(region) = self.drag.hit_test().cloned() {
                    if let Action::Dial(dial) = region.action {
                        let p = param(&self.params, dial);
                        self.text_edit.begin(
                            region.action,
                            &p.normalized_value_to_string(p.modulated_normalized_value(), false),
                        );
                    }
                } else {
                    let (x, y) = self.drag.mouse_pos();
                    let slot = self.slot();
                    if let Some(step) =
                        self.layout
                            .ring_step(x, y, self.bank().patterns[slot].len())
                    {
                        self.history.begin(self.bank());
                        self.update(slot, |p| p.set_tie(step, !p.tied(step)));
                        self.commit(context);
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => self.end_gesture(context, true),
            Event::Mouse(MouseEvent::WheelScrolled { delta, .. }) if self.dropdown.is_open() => {
                let y = match delta {
                    baseview::ScrollDelta::Lines { y, .. } => *y,
                    baseview::ScrollDelta::Pixels { y, .. } => *y / 32.0,
                };
                self.dropdown.on_wheel(y, &RESOLUTIONS, self.size);
            }
            Event::Keyboard(ev) => {
                if self.text_edit.is_active() {
                    if ev.state == KeyState::Down {
                        match &ev.key {
                            Key::Character(text) => {
                                for c in text.chars() {
                                    self.text_edit.insert_char(c);
                                }
                            }
                            Key::Backspace => self.text_edit.backspace(),
                            Key::Escape => self.text_edit.cancel(),
                            Key::Enter => self.commit_text(context),
                            _ => {}
                        }
                    }
                } else if self.dropdown.is_open() {
                    if ev.state == KeyState::Down {
                        let key = match ev.key {
                            Key::ArrowUp => Some(widgets::DropdownKey::Up),
                            Key::ArrowDown => Some(widgets::DropdownKey::Down),
                            Key::Enter => Some(widgets::DropdownKey::Enter),
                            Key::Escape => Some(widgets::DropdownKey::Esc),
                            Key::PageUp => Some(widgets::DropdownKey::PageUp),
                            Key::PageDown => Some(widgets::DropdownKey::PageDown),
                            _ => None,
                        };
                        if let Some(key) = key {
                            let selected = self.dropdown.on_key(key, &RESOLUTIONS, self.size);
                            self.select_resolution(selected, context);
                        }
                    }
                } else if ev.state == KeyState::Down
                    && ev
                        .modifiers
                        .intersects(Modifiers::CONTROL | Modifiers::META)
                {
                    if let Key::Character(text) = &ev.key {
                        let action = match text.to_ascii_lowercase().as_str() {
                            "z" if ev.modifiers.contains(Modifiers::SHIFT) => Some(Action::Redo),
                            "z" => Some(Action::Undo),
                            "y" => Some(Action::Redo),
                            "c" => Some(Action::Copy),
                            "v" => Some(Action::Paste),
                            _ => None,
                        };
                        if let Some(action) = action {
                            self.end_gesture(context, false);
                            self.action(action, context, false);
                        }
                    }
                } else {
                    return baseview::EventStatus::Ignored;
                }
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

fn paint_path(pattern: &mut Pattern, from: usize, to: usize, erase: bool) {
    let len = pattern.len();
    let clockwise = (to + len - from) % len;
    let counter = (from + len - to) % len;
    let right = clockwise <= counter;
    let count = clockwise.min(counter);
    pattern.set_on(from, !erase);
    let mut previous = from;
    for _ in 0..count {
        let next = if right {
            (previous + 1) % len
        } else {
            (previous + len - 1) % len
        };
        pattern.set_on(next, !erase);
        if !erase {
            pattern.set_tie(if right { previous } else { next }, true);
        }
        previous = next;
    }
}

struct GatedWindow {
    context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    panel: Panel,
    pending_resize: Arc<AtomicU64>,
}

impl baseview::WindowHandler for GatedWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        widgets::consume_pending_resize(
            &self.pending_resize,
            (self.surface.pixmap.width(), self.surface.pixmap.height()),
            window,
        );
        self.panel.draw(&mut self.surface.pixmap);
        self.panel.finish_cancelled_dial(self.context.as_ref());
        self.surface.present();
    }
    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        if let baseview::Event::Window(baseview::WindowEvent::Resized(info)) = &event {
            self.surface.resize_and_persist(
                info.physical_size().width,
                info.physical_size().height,
                &self.panel.params.editor_state,
            );
        }
        self.panel.event(&event, self.context.as_ref())
    }
}

struct GatedEditor {
    params: Arc<GatedParams>,
    telemetry: Arc<Telemetry>,
    pending_resize: Arc<AtomicU64>,
}

pub fn create(params: Arc<GatedParams>, telemetry: Arc<Telemetry>) -> Option<Box<dyn Editor>> {
    Some(Box::new(GatedEditor {
        params,
        telemetry,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for GatedEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (width, height) = self.params.editor_state.size();
        let params = self.params.clone();
        let telemetry = self.telemetry.clone();
        let pending_resize = self.pending_resize.clone();
        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: "Gated".to_owned(),
                size: baseview::Size::new(width as f64, height as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| GatedWindow {
                context,
                surface: widgets::SoftbufferSurface::new(window, width, height),
                panel: Panel::new(params, telemetry),
                pending_resize,
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
    fn param_values_changed(&self) {
        self.params.state_generation.fetch_add(1, Ordering::Release);
    }
}

pub fn render_preview(path: &std::path::Path, width: u32, height: u32) -> Result<(), String> {
    let mut image = Pixmap::new(width, height).ok_or("Invalid preview dimensions")?;
    let mut panel = Panel::new(
        Arc::new(GatedParams::default()),
        Arc::new(Telemetry::default()),
    );
    panel.draw(&mut image);
    image.save_png(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_drag_is_bounded_preserves_hidden_steps_and_creates_one_undo_entry() {
        let params = Arc::new(GatedParams::default());
        {
            let mut bank = params.bank.lock().unwrap();
            bank.patterns[0].set_length(64);
            bank.patterns[0].set_on(63, true);
            bank.patterns[0].set_length(16);
        }
        let mut panel = Panel::new(params, Arc::new(Telemetry::default()));
        let before = panel.bank();
        panel.drag.set_mouse(240.0, 250.0);
        panel.begin_length_drag(0, false);
        panel.drag.set_mouse(240.0, -1000.0);
        panel.update_length_drag(0, false);
        assert_eq!(panel.bank().patterns[0].len(), 64);
        panel.drag.set_mouse(240.0, 1000.0);
        panel.update_length_drag(0, false);
        assert_eq!(panel.bank().patterns[0].len(), 1);
        panel.drag.set_mouse(240.0, 218.0);
        panel.update_length_drag(0, false);
        let after = panel.bank();
        assert_eq!(after.patterns[0].len(), 24);
        assert_eq!(after.patterns[0].active, before.patterns[0].active);
        assert_eq!(after.patterns[1], before.patterns[1]);
        assert_eq!(panel.drag.end_drag(), Some(Action::Length(0)));
        assert!(panel.history.finish(after));
        assert_eq!(panel.history.undo.len(), 1);
        assert_eq!(panel.history.restore(after, false), Some(before));
        assert_eq!(panel.history.restore(before, true), Some(after));
    }

    #[test]
    fn length_drag_fine_control_and_sensitivity_follow_editor_scale() {
        for (width, height) in [(480, 840), (400, 700), (960, 1680)] {
            let mut panel = Panel::new(
                Arc::new(GatedParams::default()),
                Arc::new(Telemetry::default()),
            );
            panel.layout = Layout::new(width, height);
            let scale = panel.layout.scale;
            panel.drag.set_mouse(240.0 * scale, 250.0 * scale);
            panel.begin_length_drag(0, false);
            panel.drag.set_mouse(240.0 * scale, 218.0 * scale);
            panel.update_length_drag(0, false);
            assert_eq!(panel.bank().patterns[0].len(), 24);
            panel.update_length_drag(0, true);
            assert_eq!(panel.bank().patterns[0].len(), 24);
            panel.drag.set_mouse(240.0 * scale, 178.0 * scale);
            panel.update_length_drag(0, true);
            assert_eq!(panel.bank().patterns[0].len(), 25);
            panel.update_length_drag(0, false);
            panel.drag.set_mouse(240.0 * scale, 174.0 * scale);
            panel.update_length_drag(0, false);
            assert_eq!(panel.bank().patterns[0].len(), 26);
        }
    }

    #[test]
    fn length_click_without_motion_does_not_create_an_undo_entry() {
        let mut panel = Panel::new(
            Arc::new(GatedParams::default()),
            Arc::new(Telemetry::default()),
        );
        panel.begin_length_drag(0, false);
        panel.drag.end_drag();
        assert!(!panel.history.finish(panel.bank()));
        assert!(panel.history.undo.is_empty());
    }

    #[test]
    fn external_state_replacement_cancels_a_length_drag() {
        let params = Arc::new(GatedParams::default());
        let mut panel = Panel::new(params.clone(), Arc::new(Telemetry::default()));
        panel.begin_length_drag(0, false);
        panel.drag.set_mouse(0.0, -32.0);
        panel.update_length_drag(0, false);
        params.state_generation.fetch_add(1, Ordering::Release);
        panel.reconcile();
        assert!(panel.drag.active_action().is_none());
        assert!(panel.history.pending.is_none());
        assert!(panel.history.undo.is_empty());
        assert_eq!(panel.cancelled_dial, None);
    }

    #[test]
    fn ring_hit_testing_matches_drawing_at_different_aspect_ratios() {
        for (width, height) in [(480, 840), (960, 840), (360, 900)] {
            let layout = Layout::new(width, height);
            for len in [1, 3, 16, 64] {
                for step in 0..len {
                    let angle = TAU * (step as f32 + 0.5) / len as f32 - FRAC_PI_2;
                    let (x, y) =
                        layout.point(RING_X + 160.0 * angle.cos(), RING_Y + 160.0 * angle.sin());
                    assert_eq!(layout.ring_step(x, y, len), Some(step));
                }
            }
            let (x, y) = layout.point(RING_X, RING_Y);
            assert_eq!(layout.ring_step(x, y, 16), None);
        }
    }

    #[test]
    fn painting_skipped_steps_ties_in_both_directions_across_wrap() {
        let mut p = Pattern {
            length: 8,
            active: [0, 0],
            ties: [0, 0],
        };
        paint_path(&mut p, 6, 1, false);
        assert!(p.is_on(6) && p.is_on(7) && p.is_on(0) && p.is_on(1));
        assert!(p.tied(6) && p.tied(7) && p.tied(0));
        paint_path(&mut p, 1, 7, true);
        assert!(!p.is_on(0) && !p.is_on(7) && !p.tied(6));
        paint_path(&mut p, 1, 7, false);
        assert!(p.tied(7) && p.tied(0));
    }

    #[test]
    fn a_multi_edit_gesture_is_one_undo_action_and_redo_restores_it() {
        let mut history = History::default();
        let before = PatternBank::default();
        history.begin(before);
        let mut after = before;
        paint_path(&mut after.patterns[3], 14, 2, false);
        after.patterns[3].rotate(true);
        assert!(history.finish(after));
        assert_eq!(history.undo.len(), 1);
        assert_eq!(history.restore(after, false), Some(before));
        assert_eq!(history.restore(before, true), Some(after));
    }

    #[test]
    fn identical_external_state_replacement_cancels_pending_edits_and_history() {
        let params = Arc::new(GatedParams::default());
        let mut panel = Panel::new(params.clone(), Arc::new(Telemetry::default()));
        let bank = panel.bank();
        let mut other = bank;
        other.patterns[0].set_on(0, false);
        panel.history.begin(other);
        panel.history.finish(bank);
        panel.history.begin(bank);
        panel.gesture = Some(RingGesture {
            slot: 0,
            first: 0,
            last: 0,
            moved: false,
            erase: false,
        });
        panel.text_edit.begin(Action::Dial(Dial::Attack), "500");
        panel.drag.begin_drag(Action::Dial(Dial::Swing), 0.5, false);
        panel.dropdown.open(
            Action::Resolution,
            (0.0, 0.0, 100.0, 20.0),
            widgets::DropdownList::flat(&RESOLUTIONS),
            0,
            false,
            panel.size,
        );
        params.state_generation.fetch_add(1, Ordering::Release);
        panel.reconcile();
        assert!(panel.gesture.is_none());
        assert!(panel.history.undo.is_empty());
        assert!(panel.history.pending.is_none());
        assert!(!panel.text_edit.is_active());
        assert!(!panel.dropdown.is_open());
        assert!(panel.drag.active_action().is_none());
        assert_eq!(panel.cancelled_dial, Some(Dial::Swing));
        assert_eq!(panel.bank(), bank);
    }
}
