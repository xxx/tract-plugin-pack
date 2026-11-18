use super::*;
use crate::editor::follower as follower_ui;
use crate::modulation::{FollowerMode, ModulationState};
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
use std::sync::Arc;

fn setup(scale: f32) -> Harness {
    let mut h = Harness::new();
    h.geometry = EditorGeometry {
        layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
        scale,
        window_size: (1336.0 * scale, 680.0 * scale),
    };
    h
}
fn button(h: &Harness) -> Rect {
    widgets::mseg::follower_button_rect(
        h.editor
            .mseg_edit
            .layout(h.geometry.layout.mseg_pane, h.geometry.scale)
            .strip,
        h.geometry.scale,
    )
}
fn layout(h: &Harness) -> follower_ui::Layout {
    follower_ui::layout(h.geometry, h.editor.follower.scroll).unwrap()
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
fn numeric(h: &mut Harness, row: usize, text: &str) {
    h.key(Key::Home);
    for _ in 0..row {
        h.key(Key::ArrowDown);
    }
    h.key(Key::Enter);
    for _ in 0..16 {
        h.key(Key::Backspace);
    }
    h.key(Key::Character(text.into()));
    h.key(Key::Enter);
}

#[test]
fn follower_input_choices_wrap_and_amount_drag_does_not_edit_attack() {
    use crate::modulation::DetectorInput;
    let mut h = setup(1.0);
    h.modulation.follower.routes[1].mode = FollowerMode::Add;
    h.click(button(&h));
    h.click(layout(&h).rows[2].unwrap());
    h.key(Key::ArrowDown);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(
        h.modulation.follower.detector.input,
        DetectorInput::Sidechain
    );
    h.key(Key::ArrowLeft);
    assert_eq!(h.modulation.follower.detector.input, DetectorInput::Main);
    h.key(Key::ArrowRight);
    assert_eq!(
        h.modulation.follower.detector.input,
        DetectorInput::Sidechain
    );

    h.key(Key::ArrowUp);
    assert_eq!(h.editor.follower.focus, 1);
    let before = h.modulation.follower;
    let amount = layout(&h).rows[3].unwrap();
    let x = amount.0 + amount.2 * 0.5;
    let y = amount.1 + amount.3 * 0.5;
    h.move_to(x, y, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.editor.follower.dragging());
    assert!(h.move_to(x, y - 30.0, Modifiers::empty()).changed);
    h.release();
    assert!(h.modulation.follower.routes[1].amount > before.routes[1].amount);
    assert_eq!(h.modulation.follower.attack_ms, before.attack_ms);

    h.key(Key::Home);
    h.key(Key::ArrowUp);
    assert_eq!(h.editor.follower.focus, 10);
}

#[test]
fn a_zero_wheel_event_clamps_scroll_after_the_body_gains_visible_rows() {
    let mut h = setup(0.5);
    h.click(button(&h));
    h.key(Key::End);
    assert!(h.editor.follower.scroll > 0);
    h.geometry.scale = 1.0;
    h.geometry.window_size = (1336.0, 680.0);
    assert_eq!(layout(&h).visible, 11);
    assert!(
        h.send(Event::Mouse(MouseEvent::WheelScrolled {
            delta: baseview::ScrollDelta::Lines { x: 0.0, y: 0.0 },
            modifiers: Modifiers::empty(),
        }))
        .captured
    );
    assert_eq!(h.editor.follower.scroll, 0);
}

#[test]
fn follower_modal_routes_the_selected_curve_and_scrolls_keyboard_focus_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = setup(scale);
        let before = h.modulation.clone();
        let response = h.click(button(&h));
        assert!(response.captured && !response.changed);
        assert!(h.editor.follower_panel_open());
        assert!(h.editor.captures_keyboard());
        assert!(h.editor.dropdown_open());
        assert!(h.key(Key::Enter).changed);
        assert!(h.modulation.follower.routes[1].enabled);
        assert!(!h.modulation.follower.routes[0].enabled);
        h.key(Key::End);
        assert!(layout(&h).rows[10].is_some());
        numeric(&mut h, 10, "5 kHz");
        assert_eq!(h.modulation.follower.detector.lowpass_hz, Some(5000.0));
        h.key(Key::Escape);
        assert!(!h.editor.follower_panel_open());
        assert!(!h.editor.mseg_edit.geometry_read_only());
        assert_eq!(h.modulation.msegs, before.msegs);
        draw(&mut h);
    }
}
#[test]
fn follower_numeric_commit_cancel_and_reset_preserve_route_scope_and_modal_priority() {
    let mut h = setup(1.0);
    h.click(button(&h));
    h.key(Key::Enter);
    numeric(&mut h, 4, "10 ms");
    assert_eq!(h.modulation.follower.attack_ms, 10.0);
    h.key(Key::Home);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Add);
    numeric(&mut h, 3, "-50%");
    assert_eq!(h.modulation.follower.routes[1].amount, -0.5);
    let before = h.modulation.follower;
    let lay = layout(&h);
    let row = lay.rows[4].unwrap();
    h.move_to(row.0 + row.2 * 0.5, row.1 + row.3 * 0.5, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.editor.pointer_gesture_active());
    h.move_to(
        row.0 + row.2 * 0.5,
        row.1 + row.3 * 0.5 - 20.0,
        Modifiers::empty(),
    );
    h.key(Key::Escape);
    h.release();
    assert!(!h.editor.pointer_gesture_active());
    assert_eq!(h.modulation.follower, before);
    h.key(Key::Home);
    for _ in 0..4 {
        h.key(Key::ArrowDown);
    }
    h.key(Key::Enter);
    h.key(Key::Backspace);
    h.key(Key::Character("9999".into()));
    h.key(Key::Escape);
    assert!(h.editor.follower_panel_open());
    assert_eq!(h.modulation.follower.attack_ms, 10.0);
    let lay = layout(&h);
    h.click(lay.reset);
    assert_eq!(h.modulation.follower.attack_ms, 5.0);
    assert_eq!(h.modulation.follower.routes, before.routes);
    let curves = h.modulation.msegs;
    assert!(h.key(Key::Delete).captured);
    assert!(h.press(MouseButton::Middle).captured);
    assert_eq!(h.modulation.msegs, curves);
    h.editor.prepare_resize(1.0);
    assert!(!h.editor.follower_panel_open());
}
#[test]
fn follower_meters_show_prepared_audio_and_trace_tokens_reject_the_previous_curve_source() {
    let mut h = setup(1.0);
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    draw(&mut h);
    let old = ModulationFrame::capture(h.effect.kind, &h.modulation, &ModulationState::default());
    h.click(button(&h));
    h.key(Key::Enter);
    numeric(&mut h, 4, "0");
    numeric(&mut h, 6, "0");
    draw(&mut h);
    assert!(h.editor.follower_reading().is_none());
    let mut clock = TraceClock::default();
    clock.begin_block(&telemetry, 48000.0);
    let head = telemetry.head();
    clock.record(&telemetry, 0, &old);
    clock.publish(&telemetry, 0, &old);
    assert_eq!(telemetry.head(), head);
    let mut state = ModulationState::default();
    let mut effect = crate::effects::EffectInstance::new(h.effect.kind);
    use crate::effects::Effect;
    effect.set_sample_rate(48000.0);
    state.begin_block(&h.modulation, &[0.0], &[0.0], 48000.0);
    state.advance_segment(&h.modulation, &h.effect, &mut effect, 1, 120.0, 48000.0);
    state.process_modulation_sample(&h.modulation, &mut effect, [0.5, -0.5], [0.0; 2]);
    let frame = ModulationFrame::capture(h.effect.kind, &h.modulation, &state);
    clock.publish(&telemetry, 0, &frame);
    draw(&mut h);
    let reading = h.editor.follower_reading().unwrap();
    assert_eq!(reading.level, 0.5);
    assert_eq!(reading.envelope, 0.5);
    assert_eq!(
        telemetry.frame(clock.begin_block(&telemetry, 48000.0).token),
        Some(frame)
    );
    h.modulation.follower.gain_db = 6.0;
    let before = h.modulation.clone();
    let response = h.key(Key::Enter);
    assert!(response.captured && !response.changed);
    assert_eq!(h.modulation, before);
    assert!(!h.editor.follower_panel_open());
}

#[test]
fn host_focus_transfer_consumes_a_stale_follower_edit() {
    let mut h = setup(1.0);
    h.click(button(&h));
    let attack = layout(&h).rows[4].unwrap();
    h.move_to(
        attack.0 + attack.2 * 0.5,
        attack.1 + attack.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    h.type_replacement("10 ms");
    h.modulation.follower.gain_db = 6.0;
    let replacement = h.modulation.clone();

    let response = h.finish_text_edit();

    assert!(response.captured && !response.changed);
    assert!(!h.editor.follower_panel_open());
    assert_eq!(h.modulation, replacement);
}

#[test]
fn follower_panel_places_every_hit_region_inside_the_full_and_scrolled_layouts() {
    let mut h = setup(1.0);
    h.geometry.window_size = (1000.0, 800.0);
    h.geometry.layout.mseg_pane = (100.0, 90.0, 800.0, 620.0);
    let full = follower_ui::layout(h.geometry, 10).unwrap();
    assert_eq!(full.panel, (260.0, 120.0, 480.0, 560.0));
    assert_eq!(full.close, (710.0, 128.0, 22.0, 22.0));
    assert_eq!(full.body, (268.0, 170.0, 464.0, 396.0));
    assert_eq!(full.footer, (268.0, 590.0, 464.0, 90.0));
    assert_eq!(full.reset, (268.0, 646.0, 130.0, 26.0));
    assert_eq!(full.listen, (602.0, 646.0, 130.0, 26.0));
    assert_eq!(full.visible, 11);
    for (row, y) in full.rows.into_iter().zip([
        170.0, 206.0, 242.0, 278.0, 314.0, 350.0, 386.0, 422.0, 458.0, 494.0, 530.0,
    ]) {
        assert_eq!(row, Some((268.0, y, 456.0, 32.0)));
    }

    h.geometry.scale = 0.5;
    h.geometry.window_size = (300.0, 236.0);
    h.geometry.layout.mseg_pane = (50.0, 55.0, 200.0, 100.0);
    let compact = follower_ui::layout(h.geometry, usize::MAX).unwrap();
    assert_eq!(compact.panel, (10.0, 4.0, 280.0, 228.0));
    assert_eq!(compact.close, (268.0, 8.0, 18.0, 18.0));
    assert_eq!(compact.body, (14.0, 44.0, 272.0, 84.0));
    assert_eq!(compact.footer, (14.0, 144.0, 272.0, 88.0));
    assert_eq!(compact.reset, (14.0, 204.0, 110.0, 24.0));
    assert_eq!(compact.listen, (176.0, 204.0, 110.0, 24.0));
    assert_eq!(compact.visible, 3);
    assert!(compact.rows[..8].iter().all(Option::is_none));
    assert_eq!(
        compact.rows[8..],
        [
            Some((14.0, 44.0, 264.0, 24.0)),
            Some((14.0, 72.0, 264.0, 24.0)),
            Some((14.0, 100.0, 264.0, 24.0)),
        ]
    );

    h.geometry.window_size = (224.0, 164.0);
    h.geometry.layout.mseg_pane = (500.0, 500.0, 200.0, 100.0);
    let minimum = follower_ui::layout(h.geometry, usize::MAX).unwrap();
    assert_eq!(minimum.panel, (4.0, 4.0, 216.0, 156.0));
    assert_eq!(minimum.body, (8.0, 44.0, 208.0, 28.0));
    assert_eq!(minimum.footer, (8.0, 72.0, 208.0, 88.0));
    assert_eq!(minimum.reset, (8.0, 132.0, 102.5, 24.0));
    assert_eq!(minimum.listen, (113.5, 132.0, 102.5, 24.0));
    assert_eq!(minimum.visible, 1);
    assert!(minimum.rows[..10].iter().all(Option::is_none));
    assert_eq!(minimum.rows[10], Some((8.0, 44.0, 200.0, 24.0)));

    for size in [(223.0, 164.0), (224.0, 163.0)] {
        h.geometry.window_size = size;
        assert!(follower_ui::layout(h.geometry, 0).is_none());
    }
}

#[test]
fn follower_shortcuts_follow_visible_rows_and_suppress_modified_keys_and_releases() {
    let mut h = setup(1.0);
    h.click(button(&h));
    h.geometry.window_size = (800.0, 350.0);
    assert_eq!(layout(&h).visible, 5);
    for (key, modifiers, expected) in [
        (Key::PageDown, Modifiers::empty(), 4),
        (Key::PageDown, Modifiers::empty(), 8),
        (Key::PageDown, Modifiers::empty(), 10),
        (Key::PageUp, Modifiers::empty(), 6),
        (Key::PageUp, Modifiers::empty(), 2),
        (Key::Home, Modifiers::empty(), 0),
        (Key::ArrowUp, Modifiers::empty(), 10),
        (Key::Tab, Modifiers::empty(), 0),
        (Key::Tab, Modifiers::SHIFT, 10),
        (Key::Home, Modifiers::empty(), 0),
    ] {
        let response = h.key_with(key, KeyState::Down, modifiers);
        assert!(response.captured && !response.changed);
        assert_eq!(h.editor.follower.focus, expected);
        assert!(layout(&h).rows[expected].is_some());
    }
    let before = h.modulation.clone();
    for (state, modifiers) in [
        (KeyState::Up, Modifiers::empty()),
        (KeyState::Down, Modifiers::CONTROL),
        (KeyState::Down, Modifiers::META),
        (KeyState::Down, Modifiers::ALT),
    ] {
        assert!(!h.key_with(Key::Enter, state, modifiers).changed);
    }
    assert_eq!(h.modulation, before);
    assert!(!h.key(Key::Character("x".into())).changed);
    assert!(h.key(Key::Character(" ".into())).changed);
    assert!(h.modulation.follower.routes[1].enabled);
    assert!(h.key(Key::ArrowLeft).changed);
    assert!(!h.modulation.follower.routes[1].enabled);
    h.key(Key::ArrowDown);
    assert!(h.key(Key::ArrowLeft).changed);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Multiply);
    assert!(h.key(Key::ArrowRight).changed);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Follow);
    h.key(Key::ArrowDown);
    assert!(h.key(Key::ArrowRight).changed);
    assert_eq!(
        h.modulation.follower.detector.input,
        crate::modulation::DetectorInput::Sidechain
    );
    assert!(h.key(Key::ArrowLeft).changed);
    assert_eq!(
        h.modulation.follower.detector.input,
        crate::modulation::DetectorInput::Main
    );
    h.key(Key::ArrowDown);
    assert!(h.key(Key::ArrowLeft).changed);
    assert!((h.modulation.follower.routes[1].amount - 0.98).abs() < 1e-6);
    assert!(
        h.key_with(Key::ArrowRight, KeyState::Down, Modifiers::SHIFT)
            .changed
    );
    assert!((h.modulation.follower.routes[1].amount - 0.982).abs() < 1e-6);
}

#[test]
fn follower_numeric_navigation_commits_tab_and_consumes_escape_inside_the_field() {
    let mut h = setup(1.0);
    h.click(button(&h));
    let amount = layout(&h).rows[3].unwrap();
    h.move_to(amount.0 + 10.0, amount.1 + 10.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("125%");
    assert!(h.key(Key::Tab).changed);
    assert_eq!(h.modulation.follower.routes[1].amount, 1.25);
    assert_eq!(h.editor.follower.focus, 4);
    h.key(Key::Enter);
    h.type_replacement("11 ms");
    assert!(
        h.key_with(Key::Tab, KeyState::Down, Modifiers::SHIFT)
            .changed
    );
    assert_eq!(h.modulation.follower.attack_ms, 11.0);
    assert_eq!(h.editor.follower.focus, 3);

    h.key(Key::Enter);
    h.type_replacement("170%");
    assert!(!h.key(Key::Escape).changed);
    assert!(h.editor.follower_panel_open());
    assert_eq!(h.modulation.follower.routes[1].amount, 1.25);
    assert!(h.key(Key::ArrowRight).changed);
    assert!((h.modulation.follower.routes[1].amount - 1.27).abs() < 1e-6);
}

#[test]
fn follower_wheel_drag_middle_click_and_resize_keep_the_correct_field_owner() {
    let mut h = setup(1.0);
    h.click(button(&h));
    h.geometry.window_size = (800.0, 350.0);
    let wheel = |delta| {
        Event::Mouse(MouseEvent::WheelScrolled {
            delta,
            modifiers: Modifiers::empty(),
        })
    };
    h.send(wheel(baseview::ScrollDelta::Pixels { x: 0.0, y: -80.0 }));
    assert_eq!(h.editor.follower.scroll, 2);
    h.send(wheel(baseview::ScrollDelta::Lines { x: 0.0, y: -20.0 }));
    assert_eq!(h.editor.follower.scroll, 6);
    h.send(wheel(baseview::ScrollDelta::Pixels { x: 0.0, y: 80.0 }));
    assert_eq!(h.editor.follower.scroll, 4);
    let attack = layout(&h).rows[4].unwrap();
    h.move_to(attack.0 + 10.0, attack.1 + 10.0, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.editor.pointer_gesture_active());
    h.send(wheel(baseview::ScrollDelta::Lines { x: 0.0, y: -2.0 }));
    assert_eq!(h.editor.follower.scroll, 4);
    assert!(
        h.move_to(attack.0 + 10.0, attack.1 - 6.0, Modifiers::empty())
            .changed
    );
    assert!(h.modulation.follower.attack_ms > 5.0);
    h.release();
    assert!(!h.editor.pointer_gesture_active());
    let committed = h.modulation.clone();
    assert!(
        !h.move_to(attack.0 + 10.0, attack.1 - 20.0, Modifiers::empty())
            .changed
    );
    assert_eq!(h.modulation, committed);

    h.move_to(attack.0 + 10.0, attack.1 + 10.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("6 ms");
    assert!(h.press(MouseButton::Middle).changed);
    assert!(h.editor.follower_panel_open());
    assert_eq!(h.modulation.follower.attack_ms, 6.0);
    h.press(MouseButton::Right);
    h.type_replacement("12 ms");
    h.move_to(1.0, 1.0, Modifiers::empty());
    assert!(h.press(MouseButton::Middle).changed);
    assert!(!h.editor.follower_panel_open());
    assert_eq!(h.modulation.follower.attack_ms, 12.0);

    h.geometry.window_size = (1336.0, 680.0);
    h.click(button(&h));
    let attack = layout(&h).rows[4].unwrap();
    h.move_to(attack.0 + 10.0, attack.1 + 10.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("99 ms");
    let resized = baseview::WindowInfo::from_logical_size(baseview::Size::new(800.0, 360.0), 1.0);
    h.send(Event::Window(baseview::WindowEvent::Resized(resized)));
    assert!(!h.editor.follower_panel_open());
    assert_eq!(h.modulation.follower.attack_ms, 12.0);
}

#[test]
fn follower_mode_picker_navigates_and_cancels_before_panel_dismissal() {
    let mut h = setup(1.0);
    h.click(button(&h));
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    h.key(Key::End);
    h.key(Key::ArrowUp);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Add);
    h.key(Key::Enter);
    h.key(Key::PageDown);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Multiply);
    h.key(Key::Enter);
    h.key(Key::PageUp);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Follow);
    h.key(Key::Enter);
    h.key(Key::End);
    h.key(Key::Home);
    assert!(!h.key(Key::Enter).changed);
    h.key(Key::Enter);
    h.key(Key::ArrowDown);
    h.key(Key::Escape);
    assert!(h.editor.follower_panel_open());
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Follow);
    assert!(h.key(Key::ArrowLeft).changed);
    assert_eq!(h.modulation.follower.routes[1].mode, FollowerMode::Multiply);
}
#[test]
fn follower_panel_rejects_too_short_layout_and_keeps_valid_rows_inside_its_body() {
    let mut h = setup(1.0);
    h.geometry.window_size = (300.0, 180.0);
    h.click(button(&h));
    assert!(h.editor.follower_panel_open());
    let lay = layout(&h);
    for r in lay.rows.into_iter().flatten() {
        assert!(r.1 >= lay.body.1 && r.1 + r.3 <= lay.body.1 + lay.body.3 + 0.01);
    }
    h.editor.prepare_resize(1.0);
    h.geometry.window_size = (200.0, 120.0);
    h.click(button(&h));
    assert!(!h.editor.follower_panel_open());
}
