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
fn contextual_hints_follow_the_control_and_clock_under_the_pointer() {
    let mut h = at_scale(1.0);
    let layout = h.geometry.layout;
    let hint_at = |h: &mut Harness, rect: Rect| {
        h.move_to(
            rect.0 + rect.2 * 0.5,
            rect.1 + rect.3 * 0.5,
            Modifiers::empty(),
        );
        h.editor.mseg_edit.context_hint(
            &h.modulation.msegs[h.editor.selected_mseg],
            layout.mseg_pane,
            1.0,
        )
    };
    for (rect, expected) in [
        (layout.mseg_selector, "other curves"),
        (layout.target, "destination"),
        (layout.depth, "Scale modulation"),
        (layout.trigger, "Choose a clock"),
        (layout.mseg_sync, "seconds or host beats"),
        (layout.mseg_length, "playback length"),
        (layout.smooth, "milliseconds"),
        (layout.trace, "measured modulation"),
    ] {
        let hint = hint_at(&mut h, rect).unwrap();
        assert!(
            hint.contains(expected),
            "{hint:?} must contain {expected:?}"
        );
    }
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    assert!(
        hint_at(&mut h, layout.trigger_rate)
            .unwrap()
            .contains("onset sensitivity")
    );
    assert!(
        hint_at(&mut h, layout.trigger_aux)
            .unwrap()
            .contains("audio triggers")
    );

    let channel = MidiField::Channel.rect(&layout, 1.0);
    h.modulation.trigger = TriggerSource::MidiNote {
        note: Some(60),
        channel: None,
    };
    assert!(hint_at(&mut h, channel).unwrap().contains("MIDI restarts"));
    h.modulation.trigger = TriggerSource::MidiPitch {
        transpose: 0,
        channel: None,
    };
    assert!(hint_at(&mut h, channel).unwrap().contains("pitch channel"));
    assert!(
        hint_at(&mut h, layout.mseg_length)
            .unwrap()
            .contains("controls cycle rate")
    );
    let transpose = MidiField::Transpose.rect(&layout, 1.0);
    assert!(
        hint_at(&mut h, transpose)
            .unwrap()
            .contains("Transpose MIDI pitch")
    );

    let strip = h.editor.mseg_edit.layout(layout.mseg_pane, 1.0).strip;
    let waves = widgets::mseg::wave_midi_button_rect(strip, 1.0);
    h.modulation.wave_switch = Some(crate::modulation::MidiWaveMapping {
        envelope: 1,
        first_note: 60,
        channel: None,
    });
    assert!(hint_at(&mut h, waves).unwrap().contains("Project wave"));
    assert!(
        hint_at(&mut h, layout.target)
            .unwrap()
            .contains("destination")
    );
    h.click(layout.trigger);
    assert!(h.editor.effect_dropdown.is_open());
    assert_eq!(hint_at(&mut h, layout.target), None);
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
