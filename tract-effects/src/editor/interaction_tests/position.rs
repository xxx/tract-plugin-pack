use super::*;
use crate::modulation::{HostPosition, ModulationState};
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
fn daw_position_menu_choice_is_bounded_undoable_and_keeps_curve_geometry_editable() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = setup(scale);
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        h.click(h.geometry.layout.trigger);
        h.key(Key::End);
        h.key(Key::ArrowUp);
        h.key(Key::ArrowUp);
        let items = dropdown_items(&h.editor, &h.effect);
        assert_eq!(items.get(8), Some(&"DAW position"));
        let popup = widgets::dropdown::dropdown_popup_layout(
            &h.editor.effect_dropdown,
            &items,
            h.geometry.window_size,
        )
        .unwrap();
        assert!(popup.visible_rows.iter().any(|row| row.item_index == 8));
        let r = popup.popup_rect;
        assert!(
            r.0 >= 0.0
                && r.1 >= 0.0
                && r.0 + r.2 <= h.geometry.window_size.0
                && r.1 + r.3 <= h.geometry.window_size.1
        );
        let response = h.key(Key::Enter);
        assert!(response.changed && response.captured);
        assert_eq!(h.modulation.trigger, TriggerSource::DawPosition);
        assert_eq!(h.modulation.msegs, before.msegs);
        assert!(!h.editor.mseg_edit.geometry_read_only());
        undo.commit_capture(&h.modulation);
        let selected = h.modulation.clone();
        assert_eq!(undo.undo(selected), Some(before.clone()));
        draw(&mut h);
        let lay = h
            .editor
            .mseg_edit
            .layout(h.geometry.layout.mseg_pane, scale);
        let node = h.modulation.msegs[1].nodes[1];
        h.move_to(
            widgets::mseg::phase_to_x(&lay, node.time),
            widgets::mseg::value_to_y(&lay, node.value),
            Modifiers::empty(),
        );
        h.press(MouseButton::Left);
        h.move_to(
            widgets::mseg::phase_to_x(&lay, 0.6),
            widgets::mseg::value_to_y(&lay, 0.7),
            Modifiers::empty(),
        );
        h.release();
        assert_ne!(h.modulation.msegs[1], before.msegs[1]);
        assert_eq!(h.modulation.trigger, TriggerSource::DawPosition);
    }
}

#[test]
fn a_clock_mode_change_discards_existing_and_late_trace_points_from_the_previous_clock() {
    let mut h = setup(1.0);
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    let baseline = draw(&mut h);
    let mut frame =
        ModulationFrame::capture(h.effect.kind, &h.modulation, &ModulationState::default());
    frame.phases[1] = 0.25;
    frame.outputs[1] = 0.8;
    let mut clock = TraceClock::default();
    clock.begin_block(&telemetry, 48000.0);
    clock.record(&telemetry, 0, &frame);
    clock.publish(&telemetry, 0, &frame);
    let old = draw(&mut h);
    let lay = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
    let x = widgets::mseg::phase_to_x(&lay, 0.25) as u32;
    let y = widgets::mseg::value_to_y(&lay, 0.8) as u32;
    let region_equal = |a: &tiny_skia::Pixmap, b: &tiny_skia::Pixmap| {
        (x - 3..=x + 3).all(|col| (y - 3..=y + 3).all(|row| a.pixel(col, row) == b.pixel(col, row)))
    };
    assert!(!region_equal(&old, &baseline));
    h.click(h.geometry.layout.trigger);
    h.key(Key::End);
    h.key(Key::ArrowUp);
    h.key(Key::ArrowUp);
    h.key(Key::Enter);
    let switched = draw(&mut h);
    assert!(region_equal(&switched, &baseline));
    clock.begin_block(&telemetry, 48000.0);
    clock.record(&telemetry, 0, &frame);
    clock.publish(&telemetry, 0, &frame);
    let late = draw(&mut h);
    assert!(region_equal(&late, &baseline));
    assert!(h.editor.live.as_ref().unwrap().frame.is_none());
    frame.position_sync = true;
    clock.publish(&telemetry, 0, &frame);
    draw(&mut h);
    assert!(h.editor.live.as_ref().unwrap().frame.unwrap().position_sync);
}

#[test]
fn daw_editor_uses_measured_position_and_rejects_previous_free_clock_frames() {
    let mut h = setup(1.0);
    h.modulation.trigger = TriggerSource::DawPosition;
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    draw(&mut h);
    let mut clock = TraceClock::default();
    clock.begin_block(&telemetry, 48000.0);
    let mut old_config = h.modulation.clone();
    old_config.trigger = TriggerSource::Free;
    clock.publish(
        &telemetry,
        0,
        &ModulationFrame::capture(h.effect.kind, &old_config, &ModulationState::default()),
    );
    let old = draw(&mut h);
    assert!(h.editor.live.as_ref().unwrap().frame.is_none());
    let mut state = ModulationState::default();
    let mut effect = crate::effects::EffectInstance::new(h.effect.kind);
    use crate::effects::Effect;
    effect.set_sample_rate(48000.0);
    state.set_host_position(HostPosition {
        playing: true,
        beats: Some(3.0),
        seconds: Some(1.5),
        bpm: 120.0,
        ..HostPosition::default()
    });
    state.begin_block(&h.modulation, &[0.0], &[0.0], 48000.0);
    state.advance_segment(&h.modulation, &h.effect, &mut effect, 1, 120.0, 48000.0);
    state.process_smoothing_sample(&h.modulation, &mut effect);
    clock.publish(
        &telemetry,
        0,
        &ModulationFrame::capture(h.effect.kind, &h.modulation, &state),
    );
    let synced = draw(&mut h);
    assert_eq!(
        h.editor.live.as_ref().unwrap().frame.unwrap().phases[1],
        0.75
    );
    let lay = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
    let x = widgets::mseg::phase_to_x(&lay, 0.75) as u32;
    let y = (lay.canvas.1 + lay.canvas.3 * 0.22) as u32;
    assert_ne!(old.pixel(x, y), synced.pixel(x, y));
    assert!(!h.editor.mseg_edit.geometry_read_only());
}
