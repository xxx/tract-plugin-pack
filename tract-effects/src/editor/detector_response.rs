use super::*;
use crate::modulation::{TransientAlgorithm, TransientSettings};
use widgets::dropdown::{DropdownEvent, DropdownKey, DropdownList};

pub(super) const ALGORITHM_ITEMS: &[&str] = &["Envelope", "Peak rise", "RMS rise"];
pub(super) fn index(algorithm: TransientAlgorithm) -> usize {
    match algorithm {
        TransientAlgorithm::Envelope => 0,
        TransientAlgorithm::Peak => 1,
        TransientAlgorithm::Rms => 2,
    }
}
impl Session<'_> {
    pub(super) fn apply_detector_response(&mut self, response: TransientSettings) {
        if !matches!(
            self.modulation.trigger,
            TriggerSource::TransientSample { .. }
        ) {
            return;
        }
        let response = response.normalized();
        if response != self.modulation.detector_response {
            self.modulation.detector_response = response;
            self.response.changed = true;
        }
        self.state.detector_owner = Some(
            self.state
                .detector_binding(self.effect.kind, self.modulation),
        );
    }
    pub(super) fn apply_detector_algorithm(&mut self, selected: usize) {
        if let Some(algorithm) = [
            TransientAlgorithm::Envelope,
            TransientAlgorithm::Peak,
            TransientAlgorithm::Rms,
        ]
        .get(selected)
        {
            self.apply_detector_response(TransientSettings {
                algorithm: *algorithm,
                ..self.modulation.detector_response
            });
        }
    }
    pub(super) fn apply_detector_detail(&mut self, detail: f32) {
        self.apply_detector_response(TransientSettings {
            detail,
            ..self.modulation.detector_response
        });
    }
    pub(super) fn commit_detector_detail(&mut self, text: &str) {
        if let Some(value) = effects::parse_value(
            text,
            effects::ParamFormat::Number {
                decimals: 1,
                unit: "%",
            },
        )
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        {
            self.apply_detector_detail(value);
        } else {
            self.state
                .mseg_edit
                .set_status_message("Invalid detector Detail");
        }
    }
    pub(super) fn open_detector_algorithm(&mut self, rect: (f32, f32, f32, f32)) {
        self.state.effect_dial_drag.end_drag();
        self.state.effect_dropdown.open(
            EffectAction::DetectorAlgorithm,
            rect,
            DropdownList::flat(ALGORITHM_ITEMS),
            index(self.modulation.detector_response.algorithm),
            false,
            (self.physical_width, self.physical_height),
        );
    }
    pub(super) fn handle_detector_dropdown(&mut self, event: &baseview::Event) -> bool {
        if !self
            .state
            .effect_dropdown
            .is_open_for(EffectAction::DetectorAlgorithm)
        {
            return false;
        }
        use baseview::{Event, MouseButton, MouseEvent};
        use keyboard_types::{Key, KeyState};
        let window = (self.physical_width, self.physical_height);
        let selected = match event {
            Event::Mouse(MouseEvent::CursorMoved { position, .. }) => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                self.state.mouse_pos = self.mouse_pos;
                self.state.effect_dropdown.on_mouse_move(
                    self.mouse_pos.0,
                    self.mouse_pos.1,
                    ALGORITHM_ITEMS,
                    window,
                );
                None
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => self.state.effect_dropdown.on_mouse_down(
                self.mouse_pos.0,
                self.mouse_pos.1,
                ALGORITHM_ITEMS,
                window,
            ),
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                ..
            }) => {
                self.state.effect_dropdown.close();
                None
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                self.state.effect_dropdown.on_mouse_up();
                None
            }
            Event::Mouse(MouseEvent::WheelScrolled { delta, .. }) => {
                let amount = match delta {
                    baseview::ScrollDelta::Lines { y, .. } => *y,
                    baseview::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                self.state
                    .effect_dropdown
                    .on_wheel(amount, ALGORITHM_ITEMS, window);
                None
            }
            Event::Keyboard(key) if key.state == KeyState::Down => {
                let action = match key.key {
                    Key::ArrowUp => Some(DropdownKey::Up),
                    Key::ArrowDown => Some(DropdownKey::Down),
                    Key::Home => Some(DropdownKey::Home),
                    Key::End => Some(DropdownKey::End),
                    Key::PageUp => Some(DropdownKey::PageUp),
                    Key::PageDown => Some(DropdownKey::PageDown),
                    Key::Enter => Some(DropdownKey::Enter),
                    Key::Escape => Some(DropdownKey::Esc),
                    _ => None,
                };
                action.and_then(|key| {
                    self.state
                        .effect_dropdown
                        .on_key(key, ALGORITHM_ITEMS, window)
                })
            }
            _ => None,
        };
        if let Some(DropdownEvent::Selected(EffectAction::DetectorAlgorithm, selected)) = selected {
            self.apply_detector_algorithm(selected);
        }
        true
    }
}
