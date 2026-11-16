use super::*;

fn at_scale(scale: f32) -> Harness {
    let mut h = Harness::new();
    h.geometry = EditorGeometry {
        layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
        scale,
        window_size: (1336.0 * scale, 680.0 * scale),
    };
    h
}

fn draw(h: &mut Harness) -> tiny_skia::Pixmap {
    let mut pm = tiny_skia::Pixmap::new(
        h.geometry.window_size.0 as u32,
        h.geometry.window_size.1 as u32,
    )
    .unwrap();
    h.editor.draw(
        &mut pm,
        &mut h.tr,
        &h.effect,
        &h.modulation,
        [0.0; 4],
        h.geometry,
    );
    h.editor
        .draw_popup(&mut pm, &mut h.tr, &h.effect, h.geometry);
    pm
}

#[test]
fn guide_renders_last_and_captures_all_inputs_without_changing_parameters_or_curves() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = at_scale(scale);
        h.editor.set_audio_view(AudioView::Both);
        h.editor.set_audio_frozen(true);
        let before = (h.effect, h.modulation.clone());
        let viewport = h.editor.mseg_edit.viewport();
        let response = h.key(Key::F1);
        assert!(response.captured && !response.changed);
        assert!(h.editor.mseg_edit.help_open() && h.editor.captures_keyboard());
        let mut expected = tiny_skia::Pixmap::new(
            h.geometry.window_size.0 as u32,
            h.geometry.window_size.1 as u32,
        )
        .unwrap();
        h.editor
            .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
        let actual = draw(&mut h);
        let mut compared = 0;
        for (old, new) in expected.pixels().iter().zip(actual.pixels()) {
            if old.alpha() == 255 {
                assert_eq!(old, new);
                compared += 1;
            }
        }
        assert!(compared > 1000);
        assert!(
            h.move_to(668.0 * scale, 340.0 * scale, Modifiers::empty())
                .captured
        );
        assert!(h.press(MouseButton::Middle).captured);
        assert!(h.editor.mseg_edit.help_open());
        assert!(
            h.send(Event::Mouse(MouseEvent::WheelScrolled {
                delta: baseview::ScrollDelta::Lines { x: 0.0, y: -2.0 },
                modifiers: Modifiers::empty()
            }))
            .captured
        );
        assert!(h.key(Key::ArrowRight).captured);
        assert!(h.key(Key::End).captured);
        assert!(
            h.send(Event::Keyboard(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Character("z".into()),
                modifiers: Modifiers::CONTROL,
                ..KeyboardEvent::default()
            }))
            .captured
        );
        assert_eq!(h.editor.mseg_edit.viewport(), viewport);
        assert_eq!((h.effect, h.modulation.clone()), before);
        assert!(h.editor.audio_frozen());
        assert!(h.press(MouseButton::Right).captured);
        assert!(!h.editor.mseg_edit.help_open() && !h.editor.text_edit.is_active());
        h.key(Key::F1);
        assert!(h.editor.mseg_edit.help_open());
        assert!(h.click(h.geometry.layout.dials[0]).captured);
        assert!(!h.editor.mseg_edit.help_open() && !h.editor.pointer_gesture_active());
        assert_eq!((h.effect, h.modulation), before);
    }
}

#[test]
fn guide_respects_numeric_and_dial_priority_and_closes_on_focus_loss_or_resize() {
    let mut h = at_scale(1.0);
    let dial = h.geometry.layout.dials[0];
    h.move_to(
        dial.0 + dial.2 * 0.5,
        dial.1 + dial.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    h.type_replacement("800");
    assert!(h.key(Key::F1).captured);
    assert!(h.editor.text_edit.is_active() && !h.editor.mseg_edit.help_open());
    let rect = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
    let help = widgets::mseg::help_button_rect(rect.strip, 1.0);
    let curves = h.modulation.msegs;
    let response = h.click(help);
    assert!(response.captured && response.changed);
    assert!(h.editor.mseg_edit.help_open());
    assert!(!h.editor.text_edit.is_active());
    assert_eq!(h.effect.params[0], 800.0);
    assert_eq!(h.modulation.msegs, curves);
    let before = (h.effect, h.modulation.clone());
    h.send(Event::Window(baseview::WindowEvent::Unfocused));
    assert!(!h.editor.mseg_edit.help_open());
    h.move_to(
        dial.0 + dial.2 * 0.5,
        dial.1 + dial.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Left);
    assert!(h.editor.pointer_gesture_active());
    assert!(h.key(Key::F1).captured);
    assert!(!h.editor.mseg_edit.help_open());
    h.release();
    h.key(Key::F1);
    assert!(h.editor.mseg_edit.help_open());
    h.editor.prepare_resize(2.0);
    assert!(!h.editor.mseg_edit.help_open());
    assert_eq!((h.effect, h.modulation), before);
}

#[test]
fn modulation_hover_hints_clear_on_cursor_leave_without_closing_help() {
    let mut h = at_scale(1.0);
    let slot = h.editor.selected_mseg;
    let pane = h.geometry.layout.mseg_pane;
    assert_eq!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0),
        None
    );
    let audio = h.geometry.layout.audio;
    h.move_to(audio.0 + 10.0, audio.1 + 10.0, Modifiers::empty());
    assert!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0)
            .unwrap()
            .contains("waveform")
    );
    h.send(Event::Mouse(MouseEvent::CursorLeft));
    assert_eq!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0),
        None
    );
    h.key(Key::F1);
    h.send(Event::Mouse(MouseEvent::CursorLeft));
    assert!(h.editor.mseg_edit.help_open());
    h.key(Key::Escape);
    assert_eq!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0),
        None
    );
}

#[test]
fn detector_modal_cursor_leave_clears_hints_before_the_panel_consumes_the_event() {
    let mut h = at_scale(1.0);
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    let slot = h.editor.selected_mseg;
    let pane = h.geometry.layout.mseg_pane;
    let detect = h.geometry.layout.detector_setup;
    h.move_to(
        detect.0 + detect.2 * 0.5,
        detect.1 + detect.3 * 0.5,
        Modifiers::empty(),
    );
    assert!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0)
            .is_some()
    );
    h.press(MouseButton::Left);
    h.release();
    assert!(h.editor.detector_panel_open());
    let before = (h.effect, h.modulation.clone());
    h.send(Event::Mouse(MouseEvent::CursorLeft));
    assert!(h.editor.detector_panel_open());
    h.key(Key::Escape);
    assert!(!h.editor.detector_panel_open());
    assert_eq!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0),
        None
    );
    assert_eq!((h.effect, h.modulation), before);
}

#[test]
fn detector_binding_change_does_not_bypass_cursor_leave_hint_cleanup() {
    let mut h = at_scale(1.0);
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    let slot = h.editor.selected_mseg;
    let pane = h.geometry.layout.mseg_pane;
    let detect = h.geometry.layout.detector_setup;
    h.click(detect);
    assert!(h.editor.detector_panel_open());
    assert!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0)
            .is_some()
    );
    h.modulation.audio_trigger.highpass_hz = Some(500.0);
    let before = (h.effect, h.modulation.clone());
    h.send(Event::Mouse(MouseEvent::CursorLeft));
    assert!(!h.editor.detector_panel_open());
    assert_eq!(
        h.editor
            .mseg_edit
            .context_hint(&h.modulation.msegs[slot], pane, 1.0),
        None
    );
    assert_eq!((h.effect, h.modulation), before);
}
