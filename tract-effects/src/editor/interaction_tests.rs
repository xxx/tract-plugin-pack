use super::*;
use baseview::{Event, MouseButton, MouseEvent, Point};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};

type Rect = (f32, f32, f32, f32);

struct Harness {
    editor: EffectEditor,
    effect: TrackEffect,
    modulation: TrackModulation,
    tr: widgets::TextRenderer,
    geometry: EditorGeometry,
    pointer: (f32, f32),
}

impl Harness {
    fn new() -> Self {
        let kind = EffectKind::Svf;
        Self {
            editor: EffectEditor::default(),
            effect: TrackEffect {
                kind,
                params: effects::default_params_for_kind(kind),
                ..TrackEffect::default()
            },
            modulation: TrackModulation::default(),
            tr: widgets::TextRenderer::new(include_bytes!(
                "../../../tiny-skia-widgets/test_data/DejaVuSans.ttf"
            )),
            geometry: EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), 1.0),
                scale: 1.0,
                window_size: (1336.0, 680.0),
            },
            pointer: (0.0, 0.0),
        }
    }

    fn send(&mut self, event: Event) -> EditorResponse {
        if let Event::Mouse(MouseEvent::CursorMoved { position, .. }) = &event {
            self.pointer = (position.x as f32, position.y as f32);
        }
        self.editor.handle_event(
            &event,
            EditorContext {
                pointer_position: self.pointer,
                effect: &mut self.effect,
                modulation: &mut self.modulation,
                text_renderer: &mut self.tr,
                geometry: self.geometry,
            },
        )
    }

    fn move_to(&mut self, x: f32, y: f32, modifiers: Modifiers) -> EditorResponse {
        self.send(Event::Mouse(MouseEvent::CursorMoved {
            position: Point::new(x as f64, y as f64),
            modifiers,
        }))
    }

    fn press(&mut self, button: MouseButton) -> EditorResponse {
        self.send(Event::Mouse(MouseEvent::ButtonPressed {
            button,
            modifiers: Modifiers::empty(),
        }))
    }

    fn release(&mut self) -> EditorResponse {
        self.send(Event::Mouse(MouseEvent::ButtonReleased {
            button: MouseButton::Left,
            modifiers: Modifiers::empty(),
        }))
    }

    fn click(&mut self, rect: Rect) -> EditorResponse {
        self.move_to(
            rect.0 + rect.2 / 2.0,
            rect.1 + rect.3 / 2.0,
            Modifiers::empty(),
        );
        let result = self.press(MouseButton::Left);
        self.release();
        result
    }

    fn key(&mut self, key: Key) -> EditorResponse {
        self.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Down,
            key,
            ..KeyboardEvent::default()
        }))
    }

    fn type_replacement(&mut self, text: &str) {
        for _ in 0..16 {
            self.key(Key::Backspace);
        }
        self.key(Key::Character(text.into()));
    }
}

#[test]
fn continuous_dial_drag_changes_value_and_release_ends_gesture() {
    let mut h = Harness::new();
    let rect = h.geometry.layout.dials[0];
    let before = h.effect.params[0];
    h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.move_to(rect.0 + 30.0, rect.1, Modifiers::empty()).changed);
    assert!(h.effect.params[0] > before);
    h.release();
    let after = h.effect;
    h.move_to(rect.0 + 30.0, rect.1 - 30.0, Modifiers::empty());
    assert_eq!(h.effect, after);
}

#[test]
fn shift_drag_has_finer_resolution_than_normal_drag() {
    fn drag(modifiers: Modifiers) -> f32 {
        let mut h = Harness::new();
        let r = h.geometry.layout.dials[0];
        h.move_to(r.0 + 30.0, r.1 + 60.0, Modifiers::empty());
        h.press(MouseButton::Left);
        h.move_to(r.0 + 30.0, r.1 + 50.0, modifiers);
        h.move_to(r.0 + 30.0, r.1 + 20.0, modifiers);
        h.effect.params[0]
    }
    assert!(drag(Modifiers::SHIFT) < drag(Modifiers::empty()));
}

#[test]
fn text_entry_commits_parsed_value_and_captures_key_releases() {
    let mut h = Harness::new();
    let r = h.geometry.layout.dials[0];
    h.move_to(r.0 + 30.0, r.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("800");
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.effect.params[0], 800.0);
    h.press(MouseButton::Right);
    assert!(
        h.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Up,
            key: Key::Enter,
            ..KeyboardEvent::default()
        }))
        .captured
    );
    h.key(Key::Escape);
    assert_eq!(h.effect.params[0], 800.0);
}

#[test]
fn mix_is_selectable_as_an_mseg_target_without_editing_the_base_mix() {
    let mut h = Harness::new();
    let rect = h.geometry.layout.target;
    h.move_to(rect.0 + 20.0, rect.1 + 15.0, Modifiers::empty());
    h.press(MouseButton::Left);
    h.key(Key::End);
    h.key(Key::Enter);
    assert_eq!(
        h.modulation.targets[0],
        Some(crate::modulation::ModulationTarget::Mix)
    );
    assert_eq!(h.effect.mix, 1.0);
}

#[test]
fn click_outside_commits_mix_text_without_consuming_host_control() {
    let mut h = Harness::new();
    let r = h.geometry.layout.mix;
    h.move_to(r.0 + 30.0, r.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("25");
    h.move_to(0.0, 0.0, Modifiers::empty());
    let response = h.press(MouseButton::Left);
    assert!(response.changed);
    assert!(!response.captured);
    assert_eq!(h.effect.mix, 0.25);
}

#[test]
fn enum_dropdown_changes_discrete_value_and_right_click_does_not_edit_it() {
    let mut h = Harness::new();
    let rect = h.geometry.layout.dials[2];
    h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
    assert!(!h.press(MouseButton::Right).captured);
    assert!(!h.editor.captures_keyboard());
    h.click(rect);
    assert!(h.editor.dropdown_open());
    h.key(Key::ArrowDown);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.effect.params[2], 1.0);
}

#[test]
fn filtered_trigger_menu_maps_to_the_same_saved_trigger_variants() {
    let mut h = Harness::new();
    h.editor = EffectEditor::new(&[
        TriggerChoice::Free,
        TriggerChoice::FreeHz,
        TriggerChoice::Transient,
    ]);
    h.click(h.geometry.layout.trigger);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    assert_eq!(h.modulation.trigger, TriggerSource::FreeHz { hz: 1.0 });
    h.click(h.geometry.layout.trigger);
    h.key(Key::End);
    h.key(Key::Enter);
    assert!(matches!(
        h.modulation.trigger,
        TriggerSource::Transient { .. }
    ));
}

#[test]
fn mseg_edit_and_delete_change_only_the_selected_curve() {
    let mut h = Harness::new();
    let original = h.modulation.clone();
    let r = h.geometry.layout.mseg_pane;
    h.click((r.0 + r.2 * 0.5, r.1 + r.3 * 0.4, 1.0, 1.0));
    h.click((r.0 + r.2 * 0.5, r.1 + r.3 * 0.4, 1.0, 1.0));
    assert!(h.modulation.msegs[1].node_count > original.msegs[1].node_count);
    for slot in [0, 2, 3] {
        assert_eq!(h.modulation.msegs[slot], original.msegs[slot]);
    }
    let plot = widgets::mseg::mseg_layout(r, false, 1.0);
    let node = original.msegs[1].nodes[1];
    h.click((
        widgets::mseg::phase_to_x(&plot, node.time),
        widgets::mseg::value_to_y(&plot, node.value),
        1.0,
        1.0,
    ));
    assert!(h.key(Key::Delete).changed);
    assert_eq!(
        h.modulation.msegs[1].node_count,
        original.msegs[1].node_count
    );
}

#[test]
fn navigation_and_randomize_are_host_requests_without_config_changes() {
    let mut h = Harness::new();
    let before = h.effect;
    assert!(h.click(h.geometry.layout.randomize).randomize);
    assert!(h.click(h.geometry.layout.back).back);
    assert_eq!(h.effect, before);
}

#[test]
fn translated_layout_routes_edits_to_the_same_parameter() {
    let mut h = Harness::new();
    h.geometry.layout = view::effect_layout((216.0, 116.0, 1304.0, 640.0), 1.0);
    let rect = h.geometry.layout.mix;
    h.click(rect);
    h.effect.mix = 0.2;
    assert!(h.click(rect).changed);
    assert_eq!(h.effect.mix, 1.0);
}

#[test]
fn overflowing_mix_text_is_rejected_without_changing_the_base_value() {
    let mut h = Harness::new();
    h.effect.mix = 0.42;
    let rect = h.geometry.layout.mix;
    h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("1e100");
    assert!(!h.key(Key::Enter).changed);
    assert_eq!(h.effect.mix, 0.42);
}

#[test]
fn canceled_interaction_rebases_a_dial_drag_without_another_pointer_move() {
    fn drag(cancel: bool) -> f32 {
        let mut h = Harness::new();
        let rect = h.geometry.layout.dials[0];
        h.move_to(rect.0 + 30.0, rect.1 + 50.0, Modifiers::empty());
        if cancel {
            h.editor.cancel_interaction();
        }
        h.press(MouseButton::Left);
        h.move_to(rect.0 + 30.0, rect.1 + 49.0, Modifiers::empty());
        h.effect.params[0]
    }
    assert_eq!(drag(true), drag(false));
}
