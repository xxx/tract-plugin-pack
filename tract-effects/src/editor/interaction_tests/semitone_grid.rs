use super::*;
use widgets::mseg::{MsegData, Polarity, ValueGrid};

fn pitched_harness(scale: f32) -> Harness {
    let mut h = Harness::new();
    h.resize(scale);
    h.effect.kind = EffectKind::PitchShift;
    h.effect.params = effects::default_params_for_kind(h.effect.kind);
    let slot = h.editor.selected_mseg;
    h.modulation.targets[slot - 1] = Some(ModulationTarget::EffectParameter(0));
    h.modulation.depths[slot - 1] = 0.5;
    h.modulation.msegs[slot] = MsegData {
        polarity: Polarity::Bipolar,
        ..MsegData::default()
    };
    h
}

fn grid_button(h: &Harness) -> Rect {
    let layout = h
        .editor
        .mseg_edit
        .layout(h.geometry.layout.mseg_pane, h.geometry.scale);
    widgets::mseg::grid_button_rect(layout.strip, h.geometry.scale)
}

fn point(h: &Harness, time: f32, level: f32) -> Rect {
    let layout = h
        .editor
        .mseg_edit
        .layout(h.geometry.layout.mseg_pane, h.geometry.scale);
    (
        widgets::mseg::phase_to_x(&layout, time),
        widgets::mseg::value_to_y(&layout, level),
        0.0,
        0.0,
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
fn musical_grid_menu_preserves_geometry_and_snaps_using_refreshed_routing_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = pitched_harness(scale);
        let before = h.modulation.clone();
        let button = grid_button(&h);
        h.click(button);
        h.key(Key::End);
        let mut history = crate::undo::UndoHistory::new();
        history.begin_capture(before.clone());
        assert!(h.key(Key::Enter).changed);
        history.commit_capture(&h.modulation);
        let slot = h.editor.selected_mseg;
        assert_eq!(h.modulation.msegs[slot].value_grid, ValueGrid::Semitones);
        assert_eq!(
            h.modulation.msegs[slot].active(),
            before.msegs[slot].active()
        );
        assert_eq!(history.undo(h.modulation.clone()), Some(before));
        let p = point(&h, 0.31, 0.18);
        h.click(p);
        assert!(h.click(p).changed);
        let spec = effects::parameter_specs(h.effect.kind)[0];
        let value = h.modulation.msegs[slot].nodes[1].value;
        let pitch = crate::modulation::assignable_value(
            value,
            h.effect.params[0],
            0.5,
            spec,
            Polarity::Bipolar,
        );
        assert!((pitch + 15.0).abs() < 1e-4);
        h.effect.params[0] = 0.37;
        h.modulation.depths[slot - 1] = -0.3;
        h.modulation.msegs[slot].polarity = Polarity::Unipolar;
        let p = point(&h, 0.69, 0.58);
        h.click(p);
        assert!(h.click(p).changed);
        let value = h.modulation.msegs[slot].nodes[2].value;
        let pitch =
            crate::modulation::assignable_value(value, 0.37, -0.3, spec, Polarity::Unipolar);
        assert!((pitch - pitch.round()).abs() < 1e-4);
        let geometry = h.modulation.msegs;
        h.modulation.targets[slot - 1] = Some(ModulationTarget::Mix);
        draw(&mut h);
        assert!(h.editor.mseg_edit.pitch_grid().is_none());
        assert_eq!(h.modulation.msegs, geometry);
    }
}

#[test]
fn pending_base_edit_is_committed_before_the_same_press_uses_the_musical_grid() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = pitched_harness(scale);
        let slot = h.editor.selected_mseg;
        h.modulation.msegs[slot].value_grid = ValueGrid::Semitones;
        h.modulation.msegs[slot].nodes[0].value = 0.5;
        h.modulation.msegs[slot].nodes[1].value = 0.5;
        let dial = h.geometry.layout.dials[0];
        h.move_to(
            dial.0 + dial.2 * 0.5,
            dial.1 + dial.3 * 0.5,
            Modifiers::empty(),
        );
        h.press(MouseButton::Right);
        h.type_replacement("0.3");
        assert!(h.editor.text_edit.is_active());
        let p = point(&h, 0.35, 0.73);
        h.move_to(p.0, p.1, Modifiers::ALT);
        assert!(
            h.send(Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers: Modifiers::ALT
            }))
            .changed
        );
        h.release();
        assert_eq!(h.effect.params[0], 0.3);
        assert!(!h.editor.text_edit.is_active());
        let value = h.modulation.msegs[slot].nodes[1].value;
        let pitch = crate::modulation::assignable_value(
            value,
            0.3,
            0.5,
            effects::parameter_specs(h.effect.kind)[0],
            Polarity::Bipolar,
        );
        assert!((pitch - 11.0).abs() < 1e-4, "{scale}x: {pitch}");
    }
}

#[test]
fn inactive_musical_grid_refresh_preserves_pen_preview_and_appends_repeats() {
    use widgets::mseg::MsegTool;
    for kind in [EffectKind::PitchShift, EffectKind::Svf] {
        for tool in [MsegTool::Line, MsegTool::Arc, MsegTool::SCurve] {
            for (value_grid, snap) in [
                (ValueGrid::Divisions, true),
                (ValueGrid::Divisions, false),
                (ValueGrid::Semitones, false),
            ] {
                let mut h = pitched_harness(1.0);
                h.effect.kind = kind;
                h.effect.params = effects::default_params_for_kind(kind);
                let slot = h.editor.selected_mseg;
                h.modulation.msegs[slot].value_grid = value_grid;
                h.modulation.msegs[slot].snap = snap;
                h.editor.mseg_edit.set_tool(tool);
                let p = point(&h, 0.125, 0.25);
                h.move_to(p.0, p.1, Modifiers::empty());
                assert!(h.press(MouseButton::Left).changed);
                h.release();
                let first = h.modulation.msegs[slot];
                let preview = h.editor.mseg_edit.pen_preview().to_vec();
                assert!(preview.len() >= 2);
                h.effect.params[0] = if kind == EffectKind::PitchShift {
                    0.3
                } else {
                    443.0
                };
                draw(&mut h);
                assert_eq!(h.modulation.msegs[slot], first);
                assert_eq!(
                    h.editor.mseg_edit.pen_preview(),
                    preview,
                    "{kind:?}, {tool:?}, {value_grid:?}, snap={snap}"
                );
                assert!(h.press(MouseButton::Left).changed);
                h.release();
                let second = h.modulation.msegs[slot];
                assert!(
                    second
                        .active()
                        .iter()
                        .any(|node| (node.time - 0.375).abs() < 1e-6)
                );
                for node in first.active() {
                    assert!(second.active().contains(node));
                }
                h.effect.params[0] = if kind == EffectKind::PitchShift {
                    0.6
                } else {
                    450.0
                };
                assert!(h.press(MouseButton::Left).changed);
                h.release();
                let third = h.modulation.msegs[slot];
                assert!(
                    third
                        .active()
                        .iter()
                        .any(|node| (node.time - 0.5).abs() < 1e-6)
                );
                for node in second.active() {
                    assert!(third.active().contains(node));
                }
            }
        }
    }
}

#[test]
fn grid_popup_keeps_its_full_labels_and_final_draw_priority_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = pitched_harness(scale);
        let button = grid_button(&h);
        h.click(button);
        h.key(Key::End);
        let mut expected = tiny_skia::Pixmap::new(
            h.geometry.window_size.0 as u32,
            h.geometry.window_size.1 as u32,
        )
        .unwrap();
        h.editor
            .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
        let actual = draw(&mut h);
        for (old, new) in expected.pixels().iter().zip(actual.pixels()) {
            if old.alpha() == 255 {
                assert_eq!(old, new);
            }
        }
        assert!(h.key(Key::Enter).changed);
        assert!(!h.editor.dropdown_open());
    }
}

#[test]
fn active_musical_grid_refresh_reanchors_the_pen_using_the_updated_mapping() {
    let mut h = pitched_harness(1.0);
    let slot = h.editor.selected_mseg;
    h.modulation.msegs[slot].value_grid = ValueGrid::Semitones;
    h.editor.mseg_edit.set_tool(widgets::mseg::MsegTool::Line);
    let p = point(&h, 0.125, 0.25);
    h.move_to(p.0, p.1, Modifiers::empty());
    assert!(h.press(MouseButton::Left).changed);
    h.release();
    let first = h.modulation.msegs[slot];
    assert!(!h.editor.mseg_edit.pen_preview().is_empty());
    h.effect.params[0] = 0.3;
    draw(&mut h);
    assert_eq!(h.modulation.msegs[slot], first);
    assert!(h.editor.mseg_edit.pen_preview().is_empty());
    assert!(h.press(MouseButton::Left).changed);
    h.release();
    let spec = effects::parameter_specs(h.effect.kind)[0];
    for node in h.modulation.msegs[slot]
        .active()
        .iter()
        .filter(|node| node.time > 0.0 && node.time < 1.0)
    {
        let pitch =
            crate::modulation::assignable_value(node.value, 0.3, 0.5, spec, Polarity::Bipolar);
        assert!((pitch - pitch.round()).abs() < 1e-4);
    }
}
