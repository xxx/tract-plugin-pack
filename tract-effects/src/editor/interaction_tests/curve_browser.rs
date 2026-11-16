use super::*;

fn at_scale(scale: f32) -> Harness {
    let mut h = Harness::new();
    h.resize(scale);
    h.editor.mseg_edit.set_curve_library_directory(
        std::env::temp_dir().join(format!("tract-browser-effect-{}", std::process::id())),
    );
    h
}

#[test]
fn browser_owns_all_input_and_renders_above_effect_controls_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = at_scale(scale);
        h.editor.set_audio_view(AudioView::Both);
        h.editor.set_audio_frozen(true);
        let before = (h.effect, h.modulation.clone());
        let viewport = h.editor.mseg_edit.viewport();
        assert!(
            h.editor
                .mseg_edit
                .open_curve_browser(h.geometry.layout.mseg_pane, scale)
        );
        assert!(h.editor.captures_keyboard() && h.editor.dropdown_open());
        assert!(
            h.move_to(668.0 * scale, 340.0 * scale, Modifiers::empty())
                .captured
        );
        let mut expected = tiny_skia::Pixmap::new(
            h.geometry.window_size.0 as u32,
            h.geometry.window_size.1 as u32,
        )
        .unwrap();
        h.editor
            .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
        let mut actual = expected.clone();
        actual.fill(widgets::color_bg());
        h.editor.draw(
            &mut actual,
            &mut h.tr,
            &h.effect,
            &h.modulation,
            [0.0; 4],
            h.geometry,
        );
        h.editor
            .draw_popup(&mut actual, &mut h.tr, &h.effect, h.geometry);
        let mut compared = 0;
        for (old, new) in expected.pixels().iter().zip(actual.pixels()) {
            if old.alpha() == 255 {
                assert_eq!(old, new);
                compared += 1;
            }
        }
        assert!(compared > 1000);
        assert!(h.press(MouseButton::Middle).captured);
        assert!(!h.editor.mseg_edit.is_panning());
        assert!(
            h.send(Event::Mouse(MouseEvent::WheelScrolled {
                delta: baseview::ScrollDelta::Lines { x: 0.0, y: -2.0 },
                modifiers: Modifiers::empty()
            }))
            .captured
        );
        for key in [Key::ArrowRight, Key::ArrowDown, Key::Tab, Key::End, Key::F1] {
            let response = h.key(key);
            assert!(response.captured && !response.changed);
        }
        let response = h.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character("z".into()),
            modifiers: Modifiers::CONTROL,
            ..KeyboardEvent::default()
        }));
        assert!(response.captured && !response.changed);
        assert_eq!(h.editor.mseg_edit.viewport(), viewport);
        assert_eq!((h.effect, h.modulation.clone()), before);
        assert!(h.editor.audio_frozen());
        let dial = h.geometry.layout.dials[0];
        h.move_to(
            dial.0 + dial.2 * 0.5,
            dial.1 + dial.3 * 0.5,
            Modifiers::empty(),
        );
        assert!(h.press(MouseButton::Right).captured);
        assert!(!h.editor.mseg_edit.curve_browser_open() && !h.editor.text_edit.is_active());
        assert_eq!((h.effect, h.modulation.clone()), before);
        assert!(
            h.editor
                .mseg_edit
                .open_curve_browser(h.geometry.layout.mseg_pane, scale)
        );
        let response = h.click((0.0, 0.0, 4.0 * scale, 4.0 * scale));
        assert!(response.captured && !response.changed);
        assert!(!h.editor.dropdown_open() && !h.editor.pointer_gesture_active());
        assert_eq!((h.effect, h.modulation), before);
    }
}

#[test]
fn browser_load_reports_one_curve_edit_preserving_routing_smoothing_and_other_envelopes() {
    let mut h = at_scale(1.0);
    let before = (h.effect, h.modulation.clone());
    let slot = h.editor.selected_mseg;
    let mut expected = h.modulation.clone();
    widgets::mseg::factory_shape(widgets::mseg::FactoryShape::Duck)
        .apply(&mut expected.msegs[slot], false)
        .unwrap();
    assert!(
        h.editor
            .mseg_edit
            .open_curve_browser(h.geometry.layout.mseg_pane, 1.0)
    );
    assert!(!h.key(Key::End).changed);
    let response = h.key(Key::Enter);
    assert!(response.captured && response.changed);
    assert_eq!(h.modulation, expected);
    assert_eq!(h.effect, before.0);
    assert!(!h.editor.dropdown_open());
    assert!(!h.release().changed);
    assert!(
        h.editor
            .mseg_edit
            .open_curve_browser(h.geometry.layout.mseg_pane, 1.0)
    );
    h.key(Key::End);
    let response = h.key(Key::Enter);
    assert!(response.captured && !response.changed);
    assert_eq!(h.modulation, expected);
}

#[test]
fn shapes_entry_finalizes_numeric_input_and_browser_closes_on_focus_loss_or_resize() {
    let mut h = at_scale(1.0);
    let dial = h.geometry.layout.dials[0];
    h.move_to(
        dial.0 + dial.2 * 0.5,
        dial.1 + dial.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    h.type_replacement("800");
    let curves = h.modulation.msegs;
    let strip = h
        .editor
        .mseg_edit
        .layout(h.geometry.layout.mseg_pane, 1.0)
        .strip;
    let response = h.click((strip.0 + 170.0, strip.1 + 46.0, 30.0, 20.0));
    assert!(response.captured && response.changed);
    assert!(!h.editor.text_edit.is_active());
    assert_eq!(h.effect.params[0], 800.0);
    h.key(Key::Home);
    let response = h.key(Key::Enter);
    assert!(response.captured && !response.changed);
    assert!(h.editor.mseg_edit.curve_browser_open());
    assert_eq!(h.modulation.msegs, curves);
    let before = (h.effect, h.modulation.clone());
    h.send(Event::Window(baseview::WindowEvent::Unfocused));
    assert!(!h.editor.dropdown_open());
    assert!(
        h.editor
            .mseg_edit
            .open_curve_browser(h.geometry.layout.mseg_pane, 1.0)
    );
    h.editor.prepare_resize(2.0);
    assert!(!h.editor.dropdown_open());
    assert_eq!((h.effect, h.modulation), before);
}
