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
