use super::*;
use crate::editor::timing as timing_ui;
use crate::modulation::TriggerTiming;

fn setup(scale: f32) -> Harness {
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
    h.click(h.geometry.layout.trigger);
    h.key(Key::End);
    h.key(Key::Enter);
    h
}
fn text(h: &mut Harness, value: &str) {
    let r = timing_ui::layout(h.geometry).fields[0];
    h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
    h.press(MouseButton::Right);
    for _ in 0..16 {
        h.key(Key::Backspace);
    }
    h.key(Key::Character(value.into()));
}
#[test]
fn lookahead_toggle_commits_pending_input_enables_signed_shift_and_resets_as_one_setting_group() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = setup(scale);
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        text(&mut h, "1.5 ms");
        h.click(timing_ui::layout(h.geometry).lookahead);
        assert!(h.modulation.lookahead);
        assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 1.5);
        text(&mut h, "-3 ms");
        h.key(Key::Enter);
        assert_eq!(h.modulation.trigger_timing.audio_delay_ms, -3.0);
        assert_eq!(h.modulation.trigger, before.trigger);
        assert_eq!(h.modulation.msegs, before.msegs);
        let wire = serde_json::to_string(&h.modulation).unwrap();
        let loaded: TrackModulation = serde_json::from_str(&wire).unwrap();
        assert!(loaded.lookahead);
        assert_eq!(loaded.trigger_timing.audio_delay_ms, -3.0);
        undo.commit_capture(&h.modulation);
        assert_eq!(undo.undo(h.modulation.clone()), Some(before.clone()));
        h.click(timing_ui::layout(h.geometry).lookahead);
        assert!(!h.modulation.lookahead);
        assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
        text(&mut h, "-2 ms");
        h.key(Key::Enter);
        assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
        h.click(timing_ui::layout(h.geometry).lookahead);
        text(&mut h, "-2 ms");
        h.key(Key::Enter);
        h.click(timing_ui::layout(h.geometry).reset);
        assert!(!h.modulation.lookahead);
        assert_eq!(h.modulation.trigger_timing, TriggerTiming::default());
    }
}
#[test]
fn external_lookahead_replacement_discards_a_signed_numeric_draft() {
    let mut h = setup(1.0);
    h.click(timing_ui::layout(h.geometry).lookahead);
    text(&mut h, "-3 ms");
    h.modulation.lookahead = false;
    let response = h.key(Key::Enter);
    assert!(response.captured && !response.changed);
    assert_eq!(h.modulation.trigger_timing.audio_delay_ms, 0.0);
    assert!(!h.editor.timing_panel_open());
}
