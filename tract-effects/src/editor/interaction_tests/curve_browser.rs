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

#[test]
fn named_user_management_preserves_audio_config_and_frozen_view_at_each_scale() {
    use widgets::mseg::library::CurveLibrary;
    use widgets::mseg::{CurveShape, WavePalette};
    for scale in [0.5, 1.0, 2.0] {
        let mut h = at_scale(scale);
        let root =
            std::env::temp_dir().join(format!("tract-effect-user-{}-{scale}", std::process::id()));
        let library = CurveLibrary::at(root.clone());
        h.editor.mseg_edit.set_curve_library_directory(root.clone());
        h.editor
            .bind_wave_palette(std::sync::Arc::new(std::sync::Mutex::new(
                WavePalette::default(),
            )));
        h.editor.set_audio_view(AudioView::Both);
        h.editor.set_audio_frozen(true);
        let before = (h.effect, h.modulation.clone());
        assert!(
            h.editor
                .mseg_edit
                .open_curve_browser(h.geometry.layout.mseg_pane, scale)
        );
        for _ in 0..3 {
            assert!(!h.key(Key::Tab).changed);
        }
        for value in [
            Key::Character("s".into()),
            Key::Character("Named curve".into()),
            Key::Enter,
            Key::F2,
            Key::Character("Renamed curve".into()),
            Key::Enter,
            Key::Delete,
            Key::Escape,
        ] {
            let response = h.key(value);
            assert!(response.captured && !response.changed);
            assert_eq!((h.effect, h.modulation.clone()), before);
            assert!(h.editor.audio_frozen());
        }
        let saved = library.user_presets().unwrap().remove(0);
        assert_eq!(saved.name, "Renamed curve");
        assert_eq!(
            saved.shape,
            CurveShape::capture(&before.1.msegs[h.editor.selected_mseg])
        );
        assert!(!h.key(Key::Character("s".into())).changed);
        assert!(
            !h.key(Key::Character("Discard on focus loss".into()))
                .changed
        );
        h.send(Event::Window(baseview::WindowEvent::Unfocused));
        assert!(!h.editor.dropdown_open());
        assert!(!h.key(Key::Enter).changed);
        assert_eq!(library.user_presets().unwrap(), [saved]);
        assert_eq!((h.effect, h.modulation.clone()), before);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn named_user_recall_reports_one_selected_curve_edit_and_keeps_the_library() {
    use widgets::mseg::library::CurveLibrary;
    let mut h = at_scale(1.0);
    let root =
        std::env::temp_dir().join(format!("tract-effect-user-recall-{}", std::process::id()));
    let library = CurveLibrary::at(root.clone());
    let shape = widgets::mseg::factory_shape(widgets::mseg::FactoryShape::Duck);
    let saved = library.save_user_preset("Duck", &shape).unwrap();
    h.editor.mseg_edit.set_curve_library_directory(root.clone());
    let before = (h.effect, h.modulation.clone());
    let mut expected = h.modulation.clone();
    shape
        .apply(&mut expected.msegs[h.editor.selected_mseg], false)
        .unwrap();
    for changed in [true, false] {
        assert!(
            h.editor
                .mseg_edit
                .open_curve_browser(h.geometry.layout.mseg_pane, 1.0)
        );
        for _ in 0..2 {
            assert!(!h.key(Key::Tab).changed);
        }
        let response = h.key(Key::Enter);
        assert!(response.captured);
        assert_eq!(response.changed, changed);
        assert_eq!(h.effect, before.0);
        assert_eq!(h.modulation, expected);
        assert!(!h.release().changed);
    }
    assert_eq!(library.user_presets().unwrap(), [saved]);
    std::fs::remove_dir_all(root).unwrap();
}
