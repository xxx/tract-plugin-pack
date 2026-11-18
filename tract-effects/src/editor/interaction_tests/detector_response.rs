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
    assert!(h.editor.dropdown_open());
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
fn algorithm_navigation_starts_from_current_choice_and_only_key_down_selects() {
    let (mut h, _) = setup(1.0);
    let button = detector::detector_layout(h.geometry).algorithm;
    let select = |h: &mut Harness, key: Key, expected| {
        h.click(button);
        assert!(h.key(key).captured);
        assert!(h.key(Key::Enter).captured);
        assert_eq!(h.modulation.detector_response.algorithm, expected);
    };
    select(&mut h, Key::ArrowDown, TransientAlgorithm::Peak);
    select(&mut h, Key::ArrowDown, TransientAlgorithm::Rms);
    select(&mut h, Key::ArrowUp, TransientAlgorithm::Peak);
    select(&mut h, Key::Home, TransientAlgorithm::Envelope);
    select(&mut h, Key::PageDown, TransientAlgorithm::Rms);
    select(&mut h, Key::PageUp, TransientAlgorithm::Envelope);
    h.click(button);
    h.key_with(Key::ArrowDown, KeyState::Up, Modifiers::empty());
    h.key_with(Key::Enter, KeyState::Up, Modifiers::empty());
    assert!(h.editor.effect_dropdown.is_open());
    h.key(Key::Enter);
    assert_eq!(
        h.modulation.detector_response.algorithm,
        TransientAlgorithm::Envelope
    );
    h.click(button);
    h.key(Key::Escape);
    assert!(!h.editor.effect_dropdown.is_open());
    assert!(h.editor.detector_panel_open());

    for invalid in ["-10%", "175%"] {
        detail_text(&mut h, invalid);
        assert!(!h.key(Key::Enter).changed);
        assert_eq!(h.modulation.detector_response.detail, 50.0);
    }
}

#[test]
fn detector_algorithm_popup_responds_to_hover_click_wheel_and_scrollbar_release() {
    use super::super::detector_response::ALGORITHM_ITEMS;
    use widgets::dropdown::{DropdownList, dropdown_popup_layout};

    let (mut h, _) = setup(0.5);
    let layout = detector::detector_layout(h.geometry);
    let popup = |h: &Harness| {
        dropdown_popup_layout(
            &h.editor.effect_dropdown,
            ALGORITHM_ITEMS,
            h.geometry.window_size,
        )
        .unwrap()
    };
    h.click(layout.algorithm);
    let peak = popup(&h).visible_rows[1].rect;
    h.move_to(
        peak.0 + peak.2 * 0.5,
        peak.1 + peak.3 * 0.5,
        Modifiers::empty(),
    );
    h.key(Key::Enter);
    assert_eq!(
        h.modulation.detector_response.algorithm,
        TransientAlgorithm::Peak
    );
    h.click(layout.algorithm);
    let rms = popup(&h).visible_rows[2].rect;
    assert!(h.click(rms).changed);
    assert_eq!(
        h.modulation.detector_response.algorithm,
        TransientAlgorithm::Rms
    );

    h.geometry.window_size = (640.0, 193.0);
    let anchor = (100.0, 73.0, 95.0, 48.0);
    let open = |h: &mut Harness| {
        h.editor.effect_dropdown.open(
            EffectAction::DetectorAlgorithm,
            anchor,
            DropdownList::flat(ALGORITHM_ITEMS),
            0,
            false,
            h.geometry.window_size,
        )
    };
    let mut expected = None;
    for delta in [
        baseview::ScrollDelta::Lines { x: 0.0, y: -1.0 },
        baseview::ScrollDelta::Pixels { x: 0.0, y: -40.0 },
    ] {
        open(&mut h);
        let initial = popup(&h).scrollbar.unwrap().thumb;
        assert!(
            h.send(Event::Mouse(MouseEvent::WheelScrolled {
                delta,
                modifiers: Modifiers::empty()
            }))
            .captured
        );
        let thumb = popup(&h).scrollbar.unwrap().thumb;
        assert!(thumb.1 > initial.1);
        if let Some(expected) = expected {
            assert_eq!(thumb, expected);
        }
        expected = Some(thumb);
    }
    let bar = popup(&h).scrollbar.unwrap();
    let x = bar.thumb.0 + bar.thumb.2 * 0.5;
    let y = bar.thumb.1 + bar.thumb.3 * 0.5;
    h.move_to(x, y, Modifiers::empty());
    h.press(MouseButton::Left);
    h.move_to(x, y - 12.0, Modifiers::empty());
    let position = popup(&h).scrollbar.unwrap().thumb.1;
    assert!(position < bar.thumb.1);
    h.release();
    h.move_to(x, y + 24.0, Modifiers::empty());
    assert_eq!(popup(&h).scrollbar.unwrap().thumb.1, position);
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

#[test]
fn short_detector_popup_draw_does_not_paint_its_previous_algorithm_menu() {
    let (mut h, _) = setup(1.0);
    let small = EditorGeometry {
        window_size: (h.geometry.window_size.0, 270.0),
        ..h.geometry
    };
    assert!(!detector::detector_layout(small).usable);
    let mut expected = tiny_skia::Pixmap::new(small.window_size.0 as u32, 270).unwrap();
    h.editor
        .draw_popup(&mut expected, &mut h.tr, &h.effect, small);

    h.click(detector::detector_layout(h.geometry).algorithm);
    assert!(
        h.editor
            .effect_dropdown
            .is_open_for(EffectAction::DetectorAlgorithm)
    );
    let mut actual = tiny_skia::Pixmap::new(small.window_size.0 as u32, 270).unwrap();
    h.editor
        .draw_popup(&mut actual, &mut h.tr, &h.effect, small);

    assert!(actual.pixels() == expected.pixels());
}

#[test]
fn detector_panel_places_controls_below_the_trigger_or_above_it_near_the_window_edge() {
    let mut geometry = EditorGeometry {
        layout: view::effect_layout((0.0, 0.0, 1000.0, 800.0), 1.0),
        scale: 1.0,
        window_size: (1000.0, 800.0),
    };
    geometry.layout.detector_setup = (80.0, 90.0, 40.0, 20.0);
    let below = detector::detector_layout(geometry);
    assert!(below.usable);
    assert_eq!(below.rect, (80.0, 114.0, 392.0, 370.0));
    assert_eq!(below.close, (440.0, 122.0, 24.0, 20.0));
    assert_eq!(below.input, (92.0, 150.0, 368.0, 34.0));
    assert_eq!(below.algorithm, (92.0, 192.0, 368.0, 34.0));
    assert_eq!(
        below.fields,
        [
            (92.0, 234.0, 368.0, 34.0),
            (92.0, 276.0, 368.0, 34.0),
            (92.0, 318.0, 368.0, 34.0),
        ]
    );
    assert_eq!(below.detail, (92.0, 360.0, 368.0, 34.0));
    assert_eq!(below.meter, (92.0, 418.0, 368.0, 8.0));
    assert_eq!(below.reset, (92.0, 452.0, 88.0, 24.0));
    assert_eq!(below.listen, (372.0, 452.0, 88.0, 24.0));

    geometry.layout.detector_setup = (700.0, 520.0, 40.0, 20.0);
    let above = detector::detector_layout(geometry);
    assert!(above.usable);
    assert_eq!(above.rect, (604.0, 146.0, 392.0, 370.0));

    geometry.scale = 0.5;
    geometry.layout.detector_setup = (80.0, 90.0, 40.0, 20.0);
    geometry.window_size = (204.0, 193.0);
    let compact = detector::detector_layout(geometry);
    assert!(compact.usable);
    assert_eq!(compact.rect, (6.0, 2.0, 196.0, 185.0));
    for size in [(203.0, 193.0), (204.0, 192.0), (6.0, 6.0)] {
        geometry.window_size = size;
        assert!(!detector::detector_layout(geometry).usable);
    }
}
