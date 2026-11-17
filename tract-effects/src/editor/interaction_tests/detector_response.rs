use super::*;
use crate::modulation::{AuditionSource, ModulationState, TransientAlgorithm, TransientSettings};
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
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
    h.click(h.geometry.layout.detector_setup);
    (h, shared)
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
fn detail_text(h: &mut Harness, text: &str) {
    let r = detector::detector_layout(h.geometry).detail;
    h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
    h.press(MouseButton::Right);
    for _ in 0..16 {
        h.key(Key::Backspace);
    }
    h.key(Key::Character(text.into()));
}
#[test]
fn algorithm_and_detail_edits_are_undoable_preserve_listen_and_own_popups_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let (mut h, shared) = setup(scale);
        h.key(Key::Character("l".into()));
        let lease = shared.audition.snapshot().unwrap();
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        let lay = detector::detector_layout(h.geometry);
        h.click(lay.algorithm);
        h.key(Key::End);
        h.key(Key::Enter);
        assert_eq!(
            h.modulation.detector_response.algorithm,
            TransientAlgorithm::Rms
        );
        detail_text(&mut h, "75%");
        h.key(Key::Character("l".into()));
        assert!(h.editor.listening(AuditionSource::Trigger));
        h.key(Key::Enter);
        assert_eq!(h.modulation.detector_response.detail, 75.0);
        assert_eq!(shared.audition.snapshot(), Some(lease));
        undo.commit_capture(&h.modulation);
        assert_eq!(undo.undo(h.modulation.clone()), Some(before.clone()));
        assert_eq!(h.modulation.msegs, before.msegs);
        h.click(lay.algorithm);
        h.key(Key::Character("l".into()));
        assert!(h.editor.listening(AuditionSource::Trigger));
        draw(&mut h);
        h.press(MouseButton::Right);
        assert!(!h.editor.effect_dropdown.is_open());
        assert!(!h.editor.text_edit.is_active());
        assert!(h.editor.detector_panel_open());
        h.click(lay.reset);
        assert_eq!(h.modulation.detector_response, TransientSettings::default());
        assert!(h.editor.listening(AuditionSource::Trigger));
        h.key(Key::Escape);
        assert!(shared.audition.snapshot().is_none());
    }
}
#[test]
fn detector_detail_autocommits_before_drag_and_focus_loss_and_external_response_cleans_up() {
    let (mut h, shared) = setup(1.0);
    detail_text(&mut h, "25%");
    let lay = detector::detector_layout(h.geometry);
    let r = lay.fields[1];
    h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
    h.press(MouseButton::Left);
    assert_eq!(h.modulation.detector_response.detail, 25.0);
    h.release();
    detail_text(&mut h, "90%");
    h.send(Event::Window(baseview::WindowEvent::Unfocused));
    assert_eq!(h.modulation.detector_response.detail, 90.0);
    assert!(!h.editor.detector_panel_open());
    h.click(h.geometry.layout.detector_setup);
    h.key(Key::Character("l".into()));
    h.click(lay.algorithm);
    let replacement = TransientSettings {
        algorithm: TransientAlgorithm::Peak,
        detail: 33.0,
    };
    h.modulation.detector_response = replacement;
    let result = h.key(Key::End);
    assert!(result.captured && !result.changed);
    assert!(!h.editor.detector_panel_open());
    assert!(!h.editor.effect_dropdown.is_open());
    assert!(shared.audition.snapshot().is_none());
    assert_eq!(h.modulation.detector_response, replacement);
}
#[test]
fn detector_meter_requires_the_actually_prepared_algorithm_and_detail() {
    let (mut h, shared) = setup(1.0);
    draw(&mut h);
    let mut state = ModulationState::default();
    state.begin_block(&h.modulation, &[0.0], &[0.0], 48000.0);
    state.next_precise_trigger([0.2; 2]);
    let old = ModulationFrame::capture(h.effect.kind, &h.modulation, &state);
    let mut clock = TraceClock::default();
    clock.begin_block(&shared, 48000.0);
    clock.publish(&shared, 0, &old);
    draw(&mut h);
    assert_eq!(h.editor.detector_level(), Some(old.detector.unwrap().level));
    h.click(detector::detector_layout(h.geometry).algorithm);
    h.key(Key::End);
    h.key(Key::Enter);
    draw(&mut h);
    assert!(h.editor.detector_level().is_none());
    state.begin_block(&h.modulation, &[0.0], &[0.0], 48000.0);
    state.next_precise_trigger([0.2; 2]);
    let current = ModulationFrame::capture(h.effect.kind, &h.modulation, &state);
    clock.publish(&shared, 0, &current);
    draw(&mut h);
    assert_eq!(
        h.editor.detector_level(),
        Some(current.detector.unwrap().level)
    );
    detail_text(&mut h, "25%");
    h.key(Key::Enter);
    draw(&mut h);
    assert!(h.editor.detector_level().is_none());
}

#[test]
fn too_short_detector_layout_has_no_active_controls_and_stops_existing_listen() {
    for scale in [0.5, 1.0] {
        for listening in [false, true] {
            let (mut h, shared) = setup(scale);
            if listening {
                h.key(Key::Character("l".into()));
            }
            let before = h.modulation.clone();
            h.geometry.window_size = (800.0 * scale, 270.0 * scale);
            draw(&mut h);
            assert!(!h.editor.listening(AuditionSource::Trigger));
            h.key(Key::Character("l".into()));
            assert!(shared.audition.snapshot().is_none());
            assert_eq!(h.modulation, before);
            assert!(h.editor.detector_panel_open());
            h.key(Key::Escape);
            assert!(!h.editor.detector_panel_open());
        }
    }
}
