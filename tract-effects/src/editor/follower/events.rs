use super::*;
use baseview::{Event, MouseButton, MouseEvent, WindowEvent};
use keyboard_types::{Key, KeyState, Modifiers};
use widgets::dropdown::{DropdownEvent, DropdownKey, DropdownList};

impl Session<'_> {
    fn follower_geometry(&self) -> EditorGeometry {
        EditorGeometry {
            layout: self.layout,
            scale: self.scale_factor,
            window_size: (self.physical_width, self.physical_height),
        }
    }
    pub(in crate::editor) fn open_follower(&mut self) {
        if layout(self.follower_geometry(), 0).is_none() {
            self.state
                .mseg_edit
                .set_status_message("Enlarge the window for follower setup");
            return;
        }
        self.commit_pending_text();
        self.state.mseg_edit.cancel_interaction();
        self.state.follower.close();
        self.state.follower.open = true;
        self.state.follower.focus = 0;
        self.state.follower.scroll = 0;
        self.state.follower.owner = Some(self.state.follower_binding(self.effect, self.modulation));
        self.state
            .follower
            .drag
            .set_input_scale(self.state.effect_dial_drag.input_scale());
        self.state
            .follower
            .drag
            .set_mouse(self.mouse_pos.0, self.mouse_pos.1);
    }
    fn apply_follower(&mut self, cfg: FollowerConfig) {
        let cfg = cfg.normalized();
        if cfg != self.modulation.follower {
            self.modulation.follower = cfg;
            self.response.changed = true;
        }
        self.state.follower.owner = Some(self.state.follower_binding(self.effect, self.modulation));
    }
    fn apply_field(&mut self, field: Field, value: Option<f32>) {
        let mut cfg = self.modulation.follower;
        let slot = self.state.selected_mseg.min(3);
        field.set(&mut cfg, slot, value);
        self.apply_follower(cfg);
    }
    pub(in crate::editor) fn commit_follower_text(&mut self) {
        if let Some((field, text)) = self.state.follower.text.commit() {
            if let Some(value) = field.parse(
                self.modulation.follower,
                self.state.selected_mseg.min(3),
                &text,
            ) {
                self.apply_field(field, value);
            } else {
                self.state
                    .mseg_edit
                    .set_status_message("Invalid follower value");
            }
        }
    }
    fn choice_index(&self, choice: Choice) -> usize {
        match choice {
            Choice::Mode => {
                match self.modulation.follower.routes[self.state.selected_mseg.min(3)].mode {
                    FollowerMode::Follow => 0,
                    FollowerMode::Add => 1,
                    FollowerMode::Multiply => 2,
                }
            }
            Choice::Input => {
                usize::from(self.modulation.follower.detector.input == DetectorInput::Sidechain)
            }
        }
    }
    fn apply_choice(&mut self, choice: Choice, index: usize) {
        let mut cfg = self.modulation.follower;
        match choice {
            Choice::Mode => {
                if let Some(mode) = [
                    FollowerMode::Follow,
                    FollowerMode::Add,
                    FollowerMode::Multiply,
                ]
                .get(index)
                {
                    cfg.routes[self.state.selected_mseg.min(3)].mode = *mode;
                }
            }
            Choice::Input => {
                cfg.detector.input = if index == 0 {
                    DetectorInput::Main
                } else {
                    DetectorInput::Sidechain
                }
            }
        }
        self.apply_follower(cfg);
    }
    fn begin_choice(&mut self, choice: Choice, rect: Rect) {
        let selected = self.choice_index(choice);
        self.state.follower.dropdown.open(
            choice,
            rect,
            DropdownList::flat(choice.items()),
            selected,
            false,
            (self.physical_width, self.physical_height),
        );
    }
    fn focus_follower(&mut self, row: usize, visible: usize) {
        self.state.follower.focus = row.min(ROWS - 1);
        self.state.follower.reveal(visible);
    }
    fn activate_follower_row(&mut self, row: usize, lay: &Layout) {
        let Some(rect) = lay.rows[row] else {
            return;
        };
        let slot = self.state.selected_mseg.min(3);
        match row {
            0 => {
                let mut cfg = self.modulation.follower;
                cfg.routes[slot].enabled = !cfg.routes[slot].enabled;
                self.apply_follower(cfg);
            }
            1 => self.begin_choice(Choice::Mode, rect),
            2 => self.begin_choice(Choice::Input, rect),
            _ => {
                let field = Field::ALL[row - 3];
                let initial = field
                    .value(self.modulation.follower, slot)
                    .map_or_else(|| "Off".into(), |value| value.to_string());
                self.state.follower.text.begin(field, &initial);
            }
        }
    }
    pub(in crate::editor) fn handle_follower_event(&mut self, event: &Event) -> bool {
        if !self.state.follower.open {
            return false;
        }
        self.response.captured = true;
        let Some(lay) = layout(self.follower_geometry(), self.state.follower.scroll) else {
            self.state.follower.close();
            return true;
        };
        let window = (self.physical_width, self.physical_height);
        match event {
            Event::Window(WindowEvent::Unfocused) => {
                self.commit_follower_text();
                self.state.follower.close();
            }
            Event::Window(WindowEvent::Resized(_)) => self.state.follower.close(),
            Event::Mouse(MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.state.mouse_pos = self.mouse_pos;
                self.state
                    .follower
                    .drag
                    .set_mouse(self.mouse_pos.0, self.mouse_pos.1);
                if let Some(choice) = self.state.follower.choice() {
                    self.state.follower.dropdown.on_mouse_move(
                        self.mouse_pos.0,
                        self.mouse_pos.1,
                        choice.items(),
                        window,
                    );
                } else if let Some(field) = self.state.follower.drag.active_action().copied() {
                    let norm =
                        field.norm(self.modulation.follower, self.state.selected_mseg.min(3));
                    if let Some(norm) = self
                        .state
                        .follower
                        .drag
                        .update_drag(modifiers.contains(Modifiers::SHIFT), norm)
                    {
                        self.apply_field(
                            field,
                            field.at_norm(
                                self.modulation.follower,
                                self.state.selected_mseg.min(3),
                                norm,
                            ),
                        );
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => {
                if let Some(choice) = self.state.follower.choice() {
                    if let Some(DropdownEvent::Selected(choice, index)) = self
                        .state
                        .follower
                        .dropdown
                        .on_mouse_down(self.mouse_pos.0, self.mouse_pos.1, choice.items(), window)
                    {
                        self.apply_choice(choice, index);
                    }
                    return true;
                }
                self.commit_follower_text();
                if crate::editor::view::in_rect(lay.close, self.mouse_pos.0, self.mouse_pos.1)
                    || !crate::editor::view::in_rect(lay.panel, self.mouse_pos.0, self.mouse_pos.1)
                {
                    self.state.follower.close();
                } else if crate::editor::view::in_rect(
                    lay.reset,
                    self.mouse_pos.0,
                    self.mouse_pos.1,
                ) {
                    let cfg = FollowerConfig {
                        routes: self.modulation.follower.routes,
                        ..FollowerConfig::default()
                    };
                    self.apply_follower(cfg);
                } else if let Some(row) = lay.rows.iter().position(|r| {
                    r.is_some_and(|r| {
                        crate::editor::view::in_rect(r, self.mouse_pos.0, self.mouse_pos.1)
                    })
                }) {
                    self.focus_follower(row, lay.visible);
                    if row < 3 {
                        self.activate_follower_row(row, &lay);
                    } else {
                        let field = Field::ALL[row - 3];
                        if self.state.follower.click.check_and_update(field) {
                            self.apply_field(
                                field,
                                field.value(
                                    FollowerConfig::default(),
                                    self.state.selected_mseg.min(3),
                                ),
                            );
                        } else {
                            self.state.follower.drag_start = Some(self.modulation.follower);
                            let norm = field
                                .norm(self.modulation.follower, self.state.selected_mseg.min(3));
                            self.state.follower.drag.begin_drag(field, norm, false);
                        }
                    }
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                if self.state.follower.dragging() {
                    return true;
                }
                if self.state.follower.dropdown.is_open() {
                    self.state.follower.dropdown.close();
                    return true;
                }
                self.commit_follower_text();
                if let Some(row) = lay.rows.iter().position(|r| {
                    r.is_some_and(|r| {
                        crate::editor::view::in_rect(r, self.mouse_pos.0, self.mouse_pos.1)
                    })
                }) {
                    if row >= 3 {
                        self.focus_follower(row, lay.visible);
                        self.activate_follower_row(row, &lay);
                        self.state.follower.click = ClickTracker::new();
                    }
                } else {
                    self.state.follower.close();
                }
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Middle,
                ..
            }) => {
                self.commit_follower_text();
                if !crate::editor::view::in_rect(lay.panel, self.mouse_pos.0, self.mouse_pos.1) {
                    self.state.follower.close();
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                self.state.follower.drag.end_drag();
                self.state.follower.drag_start = None;
                self.state.follower.dropdown.on_mouse_up();
            }
            Event::Mouse(MouseEvent::WheelScrolled { delta, .. }) => {
                let amount = match delta {
                    baseview::ScrollDelta::Lines { y, .. } => *y,
                    baseview::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                if let Some(choice) = self.state.follower.choice() {
                    self.state
                        .follower
                        .dropdown
                        .on_wheel(amount, choice.items(), window);
                } else if !self.state.follower.dragging() {
                    self.commit_follower_text();
                    let steps = amount.abs().ceil() as usize;
                    self.state.follower.scroll = if amount > 0.0 {
                        self.state.follower.scroll.saturating_sub(steps)
                    } else {
                        (self.state.follower.scroll + steps).min(ROWS - lay.visible)
                    };
                }
            }
            Event::Keyboard(key) if key.state == KeyState::Down => {
                if key
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
                {
                    return true;
                }
                if let Some(choice) = self.state.follower.choice() {
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
                    if let Some(action) = action
                        && let Some(DropdownEvent::Selected(choice, index)) = self
                            .state
                            .follower
                            .dropdown
                            .on_key(action, choice.items(), window)
                    {
                        self.apply_choice(choice, index);
                    }
                    return true;
                }
                if self.state.follower.text.is_active() {
                    match &key.key {
                        Key::Escape => self.state.follower.text.cancel(),
                        Key::Enter => self.commit_follower_text(),
                        Key::Tab => {
                            self.commit_follower_text();
                            let delta = if key.modifiers.contains(Modifiers::SHIFT) {
                                ROWS - 1
                            } else {
                                1
                            };
                            self.focus_follower(
                                (self.state.follower.focus + delta) % ROWS,
                                lay.visible,
                            );
                        }
                        Key::Backspace => self.state.follower.text.backspace(),
                        Key::Character(text) => {
                            for c in text.chars() {
                                self.state.follower.text.insert_unit_char(c);
                            }
                        }
                        _ => {}
                    }
                    return true;
                }
                if self.state.follower.dragging() {
                    if key.key == Key::Escape {
                        if let Some(cfg) = self.state.follower.drag_start.take() {
                            self.apply_follower(cfg);
                        }
                        self.state.follower.drag.end_drag();
                    }
                    return true;
                }
                let focus = self.state.follower.focus;
                match &key.key {
                    Key::Escape => self.state.follower.close(),
                    Key::Tab => {
                        let delta = if key.modifiers.contains(Modifiers::SHIFT) {
                            ROWS - 1
                        } else {
                            1
                        };
                        self.focus_follower((focus + delta) % ROWS, lay.visible);
                    }
                    Key::ArrowUp => self.focus_follower((focus + ROWS - 1) % ROWS, lay.visible),
                    Key::ArrowDown => self.focus_follower((focus + 1) % ROWS, lay.visible),
                    Key::Home => self.focus_follower(0, lay.visible),
                    Key::End => self.focus_follower(ROWS - 1, lay.visible),
                    Key::PageUp => self.focus_follower(
                        focus.saturating_sub(lay.visible.saturating_sub(1).max(1)),
                        lay.visible,
                    ),
                    Key::PageDown => self.focus_follower(
                        (focus + lay.visible.saturating_sub(1).max(1)).min(ROWS - 1),
                        lay.visible,
                    ),
                    Key::Enter => self.activate_follower_row(focus, &lay),
                    Key::Character(text) if text == " " => self.activate_follower_row(focus, &lay),
                    Key::ArrowLeft | Key::ArrowRight => {
                        if focus == 0 {
                            self.activate_follower_row(0, &lay);
                        } else if focus < 3 {
                            let choice = if focus == 1 {
                                Choice::Mode
                            } else {
                                Choice::Input
                            };
                            let n = choice.items().len();
                            let delta = if key.key == Key::ArrowLeft { n - 1 } else { 1 };
                            self.apply_choice(choice, (self.choice_index(choice) + delta) % n);
                        } else {
                            let field = Field::ALL[focus - 3];
                            let delta = if key.modifiers.contains(Modifiers::SHIFT) {
                                0.001
                            } else {
                                0.01
                            } * if key.key == Key::ArrowLeft { -1.0 } else { 1.0 };
                            let cfg = self.modulation.follower;
                            let slot = self.state.selected_mseg.min(3);
                            self.apply_field(
                                field,
                                field.at_norm(cfg, slot, field.norm(cfg, slot) + delta),
                            );
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}
