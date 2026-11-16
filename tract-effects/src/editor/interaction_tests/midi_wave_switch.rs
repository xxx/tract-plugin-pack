use super::*;
use crate::modulation::{MidiWaveMapping, TriggerSource};
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
use std::sync::{Arc, Mutex};
use widgets::mseg::{FactoryShape, WavePalette, factory_shape};

fn setup(scale: f32) -> Harness {
    let mut h = Harness::new();
    h.resize(scale);
    h.geometry = EditorGeometry {
        layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
        scale,
        window_size: (1336.0 * scale, 680.0 * scale),
    };
    let mut palette = WavePalette::default();
    palette
        .store(1, &factory_shape(FactoryShape::FlatHigh))
        .unwrap();
    h.editor.bind_wave_palette(Arc::new(Mutex::new(palette)));
    h
}
fn button(h: &Harness) -> Rect {
    widgets::mseg::wave_midi_button_rect(
        h.editor
            .mseg_edit
            .layout(h.geometry.layout.mseg_pane, h.geometry.scale)
            .strip,
        h.geometry.scale,
    )
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
fn wave_setup_changes_only_mapping_and_its_controls_have_keyboard_priority() {
    let mut h = setup(1.0);
    let before = (h.effect, h.modulation.clone());
    let response = h.click(button(&h));
    assert!(response.captured && !response.changed);
    assert!(h.editor.wave_midi_panel_open() && h.editor.captures_keyboard());
    assert!(!h.press(MouseButton::Middle).changed);
    let response = h.key(Key::Enter);
    assert!(response.captured && response.changed);
    assert_eq!(
        h.modulation.wave_switch,
        Some(MidiWaveMapping {
            envelope: 1,
            first_note: 36,
            channel: None
        })
    );
    assert_eq!(h.modulation.trigger, TriggerSource::Free);
    assert_eq!(h.modulation.msegs, before.1.msegs);
    assert_eq!(h.effect, before.0);
    let lay = wave_midi::layout(h.geometry.window_size, h.geometry.layout.mseg_pane, 1.0).unwrap();
    h.click(lay.values[2]);
    h.key(Key::Home);
    h.key(Key::Enter);
    assert_eq!(h.modulation.wave_switch.unwrap().first_note, 0);
    h.click(lay.values[3]);
    h.key(Key::End);
    h.key(Key::Enter);
    assert_eq!(h.modulation.wave_switch.unwrap().channel, Some(15));
    h.click(lay.values[1]);
    h.key(Key::Home);
    h.key(Key::Enter);
    assert_eq!(h.modulation.wave_switch.unwrap().envelope, 0);
    assert!(h.key(Key::F1).captured);
    assert!(!h.editor.mseg_edit.help_open());
    assert!(
        h.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Up,
            key: Key::Enter,
            ..KeyboardEvent::default()
        }))
        .captured
    );
    h.key(Key::Escape);
    assert!(!h.editor.wave_midi_panel_open());
    assert_eq!(h.modulation.msegs, before.1.msegs);
}

#[test]
fn enabled_wave_geometry_is_readonly_other_controls_work_and_off_restores_editing() {
    let mut h = setup(1.0);
    h.click(button(&h));
    h.key(Key::Enter);
    h.key(Key::Escape);
    assert!(h.editor.mseg_edit.geometry_read_only());
    let before = h.modulation.clone();
    let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
    h.move_to(
        widgets::mseg::phase_to_x(&layout, 0.3),
        widgets::mseg::value_to_y(&layout, 0.4),
        Modifiers::empty(),
    );
    assert!(!h.press(MouseButton::Left).changed);
    h.release();
    assert!(!h.key(Key::Delete).changed);
    assert!(
        !h.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character("v".into()),
            modifiers: Modifiers::CONTROL,
            ..KeyboardEvent::default()
        }))
        .changed
    );
    assert_eq!(h.modulation, before);
    let dial = h.geometry.layout.dials[0];
    h.move_to(
        dial.0 + dial.2 * 0.5,
        dial.1 + dial.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    assert!(h.editor.text_edit.is_active());
    h.key(Key::Escape);
    h.click(button(&h));
    h.key(Key::Enter);
    h.key(Key::Escape);
    assert_eq!(h.modulation.wave_switch, None);
    assert!(!h.editor.mseg_edit.geometry_read_only());
    let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
    assert_eq!(
        h.editor.mseg_edit.on_double_click(
            widgets::mseg::phase_to_x(&layout, 0.31),
            widgets::mseg::value_to_y(&layout, 0.43),
            &mut h.modulation.msegs[1],
            h.geometry.layout.mseg_pane,
            1.0
        ),
        Some(widgets::MsegEdit::Changed)
    );
}

#[test]
fn live_wave_display_keeps_authored_data_and_rejects_old_mapping_readings() {
    let mut h = setup(1.0);
    let mapping = MidiWaveMapping {
        envelope: 1,
        first_note: 36,
        channel: None,
    };
    h.modulation.wave_switch = Some(mapping);
    let before = h.modulation.clone();
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    let authored = draw(&mut h);
    let mut clock = TraceClock::default();
    clock.begin_block(&telemetry, 48000.0);
    let frame = ModulationFrame {
        kind: h.effect.kind,
        targets: h.modulation.targets,
        phases: [0.25; 4],
        outputs: [1.0; 4],
        parameters: [None; MAX_EFFECT_PARAMS],
        mix: None,
        detector: None,
        wave: Some((mapping, Some(1))),
    };
    clock.publish(&telemetry, 0, &frame);
    let playing = draw(&mut h);
    assert!(h.editor.mseg_edit.geometry_read_only());
    let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
    let x = widgets::mseg::phase_to_x(&layout, 0.35) as u32;
    let y = widgets::mseg::value_to_y(&layout, 1.0) as u32;
    assert!((y.saturating_sub(1)..=y + 1).any(|row| {
        playing
            .pixel(x, row)
            .unwrap()
            .red()
            .saturating_sub(authored.pixel(x, row).unwrap().red())
            > 20
    }));
    assert_eq!(h.modulation, before);
    h.modulation.wave_switch = Some(MidiWaveMapping {
        first_note: 40,
        ..mapping
    });
    draw(&mut h);
    assert!(h.editor.live.as_ref().unwrap().frame.is_none());
    assert_eq!(h.modulation.msegs, before.msegs);
}

#[test]
fn setup_is_last_and_bounds_input_and_layering_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = setup(scale);
        h.click(button(&h));
        assert!(h.editor.wave_midi_panel_open());
        let before = (h.effect, h.modulation.clone());
        let lay =
            wave_midi::layout(h.geometry.window_size, h.geometry.layout.mseg_pane, scale).unwrap();
        for rect in [lay.panel, lay.close].into_iter().chain(lay.values) {
            assert!(
                rect.0 >= 0.0
                    && rect.1 >= 0.0
                    && rect.0 + rect.2 <= h.geometry.window_size.0
                    && rect.1 + rect.3 <= h.geometry.window_size.1
            );
        }
        let actual = draw(&mut h);
        let mut expected = tiny_skia::Pixmap::new(actual.width(), actual.height()).unwrap();
        h.editor
            .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
        let mut count = 0;
        for (old, new) in expected.pixels().iter().zip(actual.pixels()) {
            if old.alpha() == 255 {
                assert_eq!(old, new);
                count += 1;
            }
        }
        assert!(count > 1000);
        assert!(h.press(MouseButton::Middle).captured);
        assert!(
            h.send(Event::Mouse(MouseEvent::WheelScrolled {
                delta: baseview::ScrollDelta::Lines { x: 0.0, y: -2.0 },
                modifiers: Modifiers::empty()
            }))
            .captured
        );
        h.move_to(1.0, 1.0, Modifiers::empty());
        assert!(h.press(MouseButton::Right).captured);
        assert!(!h.editor.wave_midi_panel_open() && !h.editor.text_edit.is_active());
        assert_eq!((h.effect, h.modulation), before);
    }
}

#[test]
fn setup_finalizes_numeric_fields_and_focus_resize_or_external_changes_dismiss_safely() {
    let mut h = setup(1.0);
    let dial = h.geometry.layout.dials[0];
    h.move_to(
        dial.0 + dial.2 * 0.5,
        dial.1 + dial.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    h.type_replacement("800");
    let response = h.click(button(&h));
    assert!(response.captured && response.changed);
    assert_eq!(h.effect.params[0], 800.0);
    h.send(Event::Window(baseview::WindowEvent::Unfocused));
    assert!(!h.editor.wave_midi_panel_open());
    h.click(button(&h));
    h.editor.prepare_resize(2.0);
    assert!(!h.editor.wave_midi_panel_open());
    h.click(button(&h));
    h.modulation.wave_switch = Some(MidiWaveMapping {
        envelope: 0,
        first_note: 40,
        channel: Some(2),
    });
    h.move_to(
        dial.0 + dial.2 * 0.5,
        dial.1 + dial.3 * 0.5,
        Modifiers::empty(),
    );
    assert!(!h.editor.wave_midi_panel_open());
    assert!(!h.editor.text_edit.is_active());
}
