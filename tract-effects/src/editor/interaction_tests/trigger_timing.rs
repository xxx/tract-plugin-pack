use super::*;
use crate::editor::timing as timing_ui;
use crate::modulation::TriggerTiming;
use crate::modulation_telemetry::ModulationTelemetry;
use std::sync::Arc;

fn setup(scale: f32) -> (Harness, Arc<ModulationTelemetry>) {
    let mut h = Harness::new();
    h.geometry = EditorGeometry {
        layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
        scale,
        window_size: (1336.0 * scale, 680.0 * scale),
    };
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    let shared = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&shared, 0);
    (h, shared)
}
fn open(h: &mut Harness) -> EditorResponse {
    h.click(h.geometry.layout.trigger);
    h.key(Key::End);
    h.key(Key::Enter)
}
fn draw(h: &mut Harness) {
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
}
fn text(h: &mut Harness, field: usize, value: &str) {
    let r = timing_ui::layout(h.geometry).fields[field];
    h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
    h.press(MouseButton::Right);
    for _ in 0..16 {
        h.key(Key::Backspace);
    }
    h.key(Key::Character(value.into()));
}

#[test]
fn timing_command_preserves_source_and_numeric_fields_save_reset_and_undo_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let (mut h, _) = setup(scale);
        let before = h.modulation.clone();
        let response = open(&mut h);
        assert!(response.captured && !response.changed);
        assert!(h.editor.timing_panel_open());
        assert!(h.editor.captures_keyboard());
        assert!(h.editor.dropdown_open());
        assert_eq!(h.modulation, before);
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        text(&mut h, 0, "1.5 ms");
        h.key(Key::Enter);
        text(&mut h, 1, "8ms");
        h.key(Key::Enter);
        assert_eq!(
            h.modulation.trigger_timing,
            TriggerTiming {
                audio_delay_ms: 1.5,
                blend_ms: 8.0
            }
        );
        assert_eq!(h.modulation.trigger, before.trigger);
        assert_eq!(h.modulation.msegs, before.msegs);
        undo.commit_capture(&h.modulation);
        assert_eq!(undo.undo(h.modulation.clone()), Some(before.clone()));
        let wire = serde_json::to_string(&h.modulation).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&wire).unwrap(),
            h.modulation
        );
        draw(&mut h);
        h.click(timing_ui::layout(h.geometry).reset);
        assert_eq!(h.modulation, before);
        h.key(Key::Escape);
        assert!(!h.editor.timing_panel_open());
    }
}
#[test]
fn timing_text_commits_before_drag_escape_restores_drag_and_focus_loss_finishes_pending_text() {
    let (mut h, _) = setup(1.0);
    open(&mut h);
    text(&mut h, 0, "2 ms");
    let r = timing_ui::layout(h.geometry).fields[1];
    h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.editor.pointer_gesture_active());
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 2.0);
    h.move_to(r.0 + r.2 * 0.5, r.1 - 20.0, Modifiers::empty());
    assert!(h.modulation.trigger_timing.blend_ms > 0.0);
    h.key(Key::Escape);
    h.release();
    assert!(!h.editor.pointer_gesture_active());
    assert_eq!(h.modulation.trigger_timing.blend_ms, 0.0);
    text(&mut h, 1, "6 ms");
    h.send(Event::Window(baseview::WindowEvent::Unfocused));
    assert_eq!(h.modulation.trigger_timing.blend_ms, 6.0);
    assert!(!h.editor.timing_panel_open());
}
#[test]
fn timing_stale_bindings_resize_and_unusable_hidpi_layouts_discard_hidden_input() {
    for scale in [0.5, 1.0] {
        let (mut h, _) = setup(scale);
        open(&mut h);
        text(&mut h, 0, "3ms");
        h.geometry.window_size = (800.0 * scale, 200.0 * scale);
        draw(&mut h);
        h.key(Key::Enter);
        assert_eq!(h.modulation.trigger_timing, TriggerTiming::default());
        assert!(!h.editor.pointer_gesture_active());
        h.key(Key::Escape);
        assert!(!h.editor.timing_panel_open());
    }
    let (mut h, shared) = setup(1.0);
    open(&mut h);
    text(&mut h, 0, "3ms");
    h.modulation.trigger_timing.blend_ms = 9.0;
    let response = h.key(Key::Enter);
    assert!(response.captured && !response.changed);
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
    assert!(!h.editor.timing_panel_open());
    open(&mut h);
    shared.invalidate();
    draw(&mut h);
    assert!(!h.editor.timing_panel_open());
    open(&mut h);
    h.editor.prepare_resize(1.0);
    assert!(!h.editor.timing_panel_open());
    open(&mut h);
    text(&mut h, 0, "3ms");
    h.editor.invalidate_trace();
    assert!(!h.editor.timing_panel_open());
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
}

#[test]
fn timing_same_variant_source_replacements_discard_old_drafts() {
    for case in 0..4 {
        let (mut h, _) = setup(1.0);
        if case == 1 {
            h.modulation.trigger = TriggerSource::MidiPitch {
                transpose: 0,
                channel: None,
            };
        }
        if case == 2 {
            h.modulation.trigger = TriggerSource::MidiNote {
                note: Some(60),
                channel: None,
            };
        }
        if case == 3 {
            h.modulation.trigger = TriggerSource::FreeHzSample { hz: 1.0 };
        }
        open(&mut h);
        text(&mut h, 0, "3ms");
        match case {
            0 => h.modulation.audio_trigger.input = crate::modulation::DetectorInput::Sidechain,
            1 => {
                h.modulation.trigger = TriggerSource::MidiPitch {
                    transpose: 12,
                    channel: Some(3),
                }
            }
            2 => {
                h.modulation.trigger = TriggerSource::MidiNote {
                    note: Some(61),
                    channel: Some(3),
                }
            }
            _ => h.modulation.trigger = TriggerSource::FreeHzSample { hz: 2.0 },
        }
        let response = h.key(Key::Enter);
        assert!(response.captured && !response.changed);
        assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
        assert!(!h.editor.timing_panel_open());
    }
}

#[test]
fn host_focus_transfer_consumes_a_stale_timing_edit() {
    let (mut h, _) = setup(1.0);
    open(&mut h);
    text(&mut h, 0, "3 ms");
    h.modulation.trigger_timing.blend_ms = 9.0;
    let replacement = h.modulation.clone();

    let response = h.finish_text_edit();

    assert!(response.captured && !response.changed);
    assert!(!h.editor.timing_panel_open());
    assert_eq!(h.modulation, replacement);
}

#[test]
fn timing_draft_survives_a_redraw_with_room_for_the_panel() {
    let (mut h, _) = setup(1.0);
    open(&mut h);
    text(&mut h, 0, "3 ms");

    draw(&mut h);

    let response = h.key(Key::Enter);
    assert!(response.captured && response.changed);
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 3.0);
}

#[test]
fn timing_rejects_typed_values_outside_the_visible_slider_ranges() {
    let (mut h, _) = setup(1.0);
    open(&mut h);
    let original = h.modulation.trigger_timing;
    for (field, value) in [(0, "-1 ms"), (0, "9 ms"), (1, "-3 ms"), (1, "35 ms")] {
        text(&mut h, field, value);
        assert!(!h.key(Key::Enter).changed);
        assert_eq!(h.modulation.trigger_timing, original);
    }
}

#[test]
fn a_timing_draft_is_discarded_when_audio_trigger_settings_or_available_height_change() {
    let (mut h, _) = setup(1.0);
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    open(&mut h);
    text(&mut h, 0, "3 ms");
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 2.5,
        hold_ms: 50.0,
    };
    assert!(!h.key(Key::Enter).changed);
    assert!(!h.editor.timing_panel_open());
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);

    open(&mut h);
    text(&mut h, 0, "3 ms");
    let enough_room = h.geometry.window_size;
    h.geometry.window_size = (215.0, 140.0);
    assert!(h.move_to(10.0, 10.0, Modifiers::empty()).captured);
    assert!(h.editor.timing_panel_open());
    h.geometry.window_size = enough_room;
    assert!(!h.key(Key::Enter).changed);
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
}

#[test]
fn clicking_and_typing_timing_fields_leaves_keyboard_focus_on_the_edited_field() {
    let (mut h, _) = setup(1.0);
    open(&mut h);
    let layout = timing::layout(h.geometry);
    h.click(layout.fields[1]);
    h.key(Key::ArrowRight);
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
    let blend = h.modulation.trigger_timing.blend_ms;
    assert!((blend - 0.2).abs() < 1e-6);
    text(&mut h, 0, "1 ms");
    h.key(Key::Enter);
    h.key(Key::ArrowRight);
    assert!((h.modulation.trigger_timing.audio_delay_ms - 1.04).abs() < 1e-6);
    assert_eq!(h.modulation.trigger_timing.blend_ms, blend);
}

#[test]
fn timing_panel_centers_its_hit_regions_and_admits_the_minimum_scale() {
    let mut geometry = EditorGeometry {
        layout: view::effect_layout((0.0, 0.0, 1000.0, 800.0), 1.0),
        scale: 1.0,
        window_size: (1000.0, 800.0),
    };
    let full = timing_ui::layout(geometry);
    assert!(full.usable);
    assert_eq!(full.rect, (304.0, 240.0, 392.0, 320.0));
    assert_eq!(full.close, (664.0, 248.0, 24.0, 20.0));
    assert_eq!(full.lookahead, (316.0, 294.0, 368.0, 34.0));
    assert_eq!(
        full.fields,
        [(316.0, 338.0, 368.0, 34.0), (316.0, 382.0, 368.0, 34.0)]
    );
    assert_eq!(full.reset, (316.0, 526.0, 88.0, 24.0));

    geometry.scale = 0.5;
    geometry.window_size = (204.0, 168.0);
    let compact = timing_ui::layout(geometry);
    assert!(compact.usable);
    assert_eq!(compact.rect, (4.0, 4.0, 196.0, 160.0));
    assert_eq!(compact.close, (184.0, 8.0, 12.0, 10.0));
    assert_eq!(compact.lookahead, (10.0, 31.0, 184.0, 17.0));
    assert_eq!(
        compact.fields,
        [(10.0, 53.0, 184.0, 17.0), (10.0, 75.0, 184.0, 17.0)]
    );
    assert_eq!(compact.reset, (10.0, 147.0, 44.0, 12.0));
    for size in [(203.0, 168.0), (204.0, 167.0), (6.0, 6.0)] {
        geometry.window_size = size;
        assert!(!timing_ui::layout(geometry).usable);
    }
}

#[test]
fn timing_keyboard_navigation_adjusts_only_the_focused_field_and_keeps_command_keys_safe() {
    let (mut h, _) = setup(1.0);
    h.modulation.lookahead = true;
    h.modulation.trigger_timing.audio_delay_ms = -1.0;
    h.modulation.trigger_timing.blend_ms = 7.0;
    open(&mut h);

    assert!(h.key(Key::ArrowRight).changed);
    assert!((h.modulation.trigger_timing.audio_delay_ms + 0.92).abs() < 1e-6);
    assert!(
        h.key_with(Key::ArrowLeft, KeyState::Down, Modifiers::SHIFT)
            .changed
    );
    assert!((h.modulation.trigger_timing.audio_delay_ms + 0.928).abs() < 1e-6);
    let before = h.modulation.clone();
    for (state, modifiers) in [
        (KeyState::Up, Modifiers::empty()),
        (KeyState::Down, Modifiers::CONTROL),
        (KeyState::Down, Modifiers::META),
        (KeyState::Down, Modifiers::ALT),
    ] {
        assert!(!h.key_with(Key::ArrowRight, state, modifiers).changed);
        assert_eq!(h.modulation, before);
    }

    h.key(Key::ArrowDown);
    assert!(h.key(Key::ArrowRight).changed);
    assert!((h.modulation.trigger_timing.blend_ms - 7.2).abs() < 1e-6);
    assert!((h.modulation.trigger_timing.audio_delay_ms + 0.928).abs() < 1e-6);
    h.key(Key::ArrowUp);
    assert!(h.key(Key::ArrowLeft).changed);
    assert!((h.modulation.trigger_timing.audio_delay_ms + 1.008).abs() < 1e-6);

    h.key(Key::Tab);
    h.key(Key::Enter);
    h.type_replacement("9 ms");
    assert!(
        h.key_with(Key::Tab, KeyState::Down, Modifiers::SHIFT)
            .changed
    );
    assert_eq!(h.modulation.trigger_timing.blend_ms, 9.0);
    h.key(Key::Enter);
    h.type_replacement("2 ms");
    assert!(h.key(Key::Tab).changed);
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 2.0);
    h.key(Key::Tab);
    assert!(!h.key(Key::ArrowRight).changed);
    assert!(h.key(Key::Enter).changed);
    assert!(!h.modulation.lookahead);

    h.key(Key::ArrowDown);
    h.key(Key::Home);
    h.key(Key::Enter);
    h.type_replacement("3 ms");
    assert!(!h.key(Key::Escape).changed);
    assert!(h.editor.timing_panel_open());
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 2.0);
    h.key(Key::End);
    assert!(!h.key(Key::ArrowLeft).changed);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.modulation.trigger_timing, TriggerTiming::default());
    h.key_with(Key::Tab, KeyState::Down, Modifiers::SHIFT);
    assert!(h.key(Key::Enter).changed);
    assert!(h.modulation.lookahead);
    h.key(Key::ArrowDown);
    h.key(Key::ArrowDown);
    assert!(h.key(Key::ArrowRight).changed);
    assert!((h.modulation.trigger_timing.audio_delay_ms - 0.08).abs() < 1e-6);
}

#[test]
fn timing_pointer_and_resize_events_commit_or_discard_the_owned_draft() {
    let (mut h, _) = setup(1.0);
    h.modulation.lookahead = true;
    h.modulation.trigger_timing.audio_delay_ms = -2.0;
    open(&mut h);
    let layout = timing_ui::layout(h.geometry);
    let (x, y, w, height) = layout.fields[0];
    let pointer = (x + w * 0.5, y + height * 0.5);
    h.move_to(pointer.0, pointer.1, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.editor.pointer_gesture_active());
    assert!(!h.press(MouseButton::Right).changed);
    assert!(
        h.move_to(pointer.0, pointer.1 - 16.0, Modifiers::empty())
            .changed
    );
    assert!(h.modulation.trigger_timing.audio_delay_ms > -2.0);
    assert!(h.modulation.trigger_timing.audio_delay_ms < 0.0);
    h.release();
    assert!(!h.editor.pointer_gesture_active());
    let committed = h.modulation.clone();
    assert!(
        !h.move_to(pointer.0, pointer.1 - 24.0, Modifiers::empty())
            .changed
    );
    assert_eq!(h.modulation, committed);

    text(&mut h, 1, "6 ms");
    assert!(h.press(MouseButton::Middle).changed);
    assert!(h.editor.timing_panel_open());
    assert_eq!(h.modulation.trigger_timing.blend_ms, 6.0);
    text(&mut h, 1, "12 ms");
    h.move_to(1.0, 1.0, Modifiers::empty());
    assert!(h.press(MouseButton::Middle).changed);
    assert_eq!(h.modulation.trigger_timing.blend_ms, 12.0);
    assert!(!h.editor.timing_panel_open());

    open(&mut h);
    text(&mut h, 1, "16 ms");
    let resized = baseview::WindowInfo::from_logical_size(baseview::Size::new(640.0, 300.0), 1.0);
    h.send(Event::Window(baseview::WindowEvent::Resized(resized)));
    assert_eq!(h.modulation.trigger_timing.blend_ms, 12.0);
    assert!(!h.editor.timing_panel_open());
    open(&mut h);
    h.click(timing_ui::layout(h.geometry).close);
    assert!(!h.editor.timing_panel_open());
}

#[test]
fn unusable_timing_panel_waits_for_an_explicit_dismissal() {
    let (mut h, _) = setup(1.0);
    let normal = h.geometry.window_size;
    let before = h.modulation.clone();
    for close in 0..4 {
        h.geometry.window_size = normal;
        open(&mut h);
        text(&mut h, 0, "3 ms");
        h.geometry.window_size = (205.0, 136.0);
        assert!(!timing_ui::layout(h.geometry).usable);
        assert!(!h.key(Key::Enter).changed);
        assert!(
            !h.key_with(Key::Escape, KeyState::Up, Modifiers::empty())
                .changed
        );
        assert!(h.editor.timing_panel_open());
        let response = match close {
            0 => h.key(Key::Escape),
            1 => h.press(MouseButton::Left),
            2 => h.send(Event::Window(baseview::WindowEvent::Unfocused)),
            _ => {
                let resized =
                    baseview::WindowInfo::from_logical_size(baseview::Size::new(205.0, 136.0), 1.0);
                h.send(Event::Window(baseview::WindowEvent::Resized(resized)))
            }
        };
        assert!(response.captured && !response.changed);
        assert!(!h.editor.timing_panel_open());
        assert_eq!(h.modulation, before);
    }
}
