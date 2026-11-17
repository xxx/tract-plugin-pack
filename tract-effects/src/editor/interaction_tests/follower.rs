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
