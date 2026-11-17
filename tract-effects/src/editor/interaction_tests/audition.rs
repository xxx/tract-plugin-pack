use super::*;
use crate::editor::follower as follower_ui;
use crate::modulation::{AuditionSettings, AuditionSource};
use crate::modulation_telemetry::ModulationTelemetry;
use std::sync::Arc;

fn setup(scale: f32) -> (Harness, Arc<ModulationTelemetry>) {
    let mut h = Harness::new();
    h.geometry = EditorGeometry {
        layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
        scale,
        window_size: (1336.0 * scale, 680.0 * scale),
    };
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    (h, telemetry)
}
fn open_follower(h: &mut Harness) {
    let strip = h
        .editor
        .mseg_edit
        .layout(h.geometry.layout.mseg_pane, h.geometry.scale)
        .strip;
    h.click(widgets::mseg::follower_button_rect(strip, h.geometry.scale));
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
#[test]
fn detector_listen_is_temporary_does_not_edit_state_and_numeric_fields_keep_keyboard_priority() {
    for scale in [0.5, 1.0, 2.0] {
        let (mut h, telemetry) = setup(scale);
        h.modulation.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        let before = h.modulation.clone();
        h.click(h.geometry.layout.detector_setup);
        let lay = detector::detector_layout(h.geometry);
        let response = h.click(lay.listen);
        assert!(response.captured && !response.changed);
        assert!(h.editor.listening(AuditionSource::Trigger));
        assert_eq!(h.modulation, before);
        assert!(telemetry.audition.snapshot().unwrap().matches(
            0,
            h.effect.kind,
            &h.modulation,
            telemetry.waveform.generation()
        ));
        h.key(Key::Character("l".into()));
        assert!(telemetry.audition.snapshot().is_none());
        h.key(Key::Character("L".into()));
        assert!(h.editor.listening(AuditionSource::Trigger));
        let r = lay.fields[0];
        h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
        h.press(MouseButton::Right);
        h.key(Key::Character("l".into()));
        assert!(h.editor.listening(AuditionSource::Trigger));
        h.key(Key::Escape);
        assert!(h.editor.detector_panel_open());
        h.key(Key::Escape);
        assert!(telemetry.audition.snapshot().is_none());
        assert_eq!(h.modulation, before);
    }
}
#[test]
fn follower_listen_works_with_routes_off_and_tracks_own_edits_and_popup_priority() {
    for scale in [0.5, 1.0, 2.0] {
        let (mut h, telemetry) = setup(scale);
        open_follower(&mut h);
        let lay = follower_ui::layout(h.geometry, h.editor.follower.scroll).unwrap();
        let before = h.modulation.clone();
        let response = h.click(lay.listen);
        assert!(response.captured && !response.changed);
        assert!(h.editor.listening(AuditionSource::Follower));
        assert_eq!(h.modulation, before);
        assert_eq!(h.modulation.follower.mask(), 0);
        let token = telemetry.audition.snapshot().unwrap().token;
        h.key(Key::Home);
        for _ in 0..8 {
            h.key(Key::ArrowDown);
        }
        h.key(Key::Enter);
        for _ in 0..16 {
            h.key(Key::Backspace);
        }
        h.key(Key::Character("6 dB".into()));
        h.key(Key::Enter);
        assert_eq!(telemetry.audition.snapshot().unwrap().settings.gain_db, 6.0);
        assert_eq!(telemetry.audition.snapshot().unwrap().token, token);
        h.key(Key::Home);
        h.key(Key::ArrowDown);
        h.key(Key::Enter);
        h.key(Key::Character("l".into()));
        assert!(h.editor.listening(AuditionSource::Follower));
        h.key(Key::Escape);
        assert!(h.editor.follower_panel_open());
        draw(&mut h);
        h.key(Key::Escape);
        assert!(telemetry.audition.snapshot().is_none());
    }
}
#[test]
fn listen_cleanup_releases_on_focus_resize_replacement_disconnect_and_editor_drop() {
    for case in 0..9 {
        let (mut h, telemetry) = setup(1.0);
        open_follower(&mut h);
        h.key(Key::Character("l".into()));
        assert!(telemetry.audition.snapshot().is_some());
        match case {
            0 => {
                h.send(Event::Window(baseview::WindowEvent::Unfocused));
            }
            1 => h.editor.prepare_resize(1.0),
            2 => h.editor.cancel_interaction(),
            3 => h.editor.disconnect_live(),
            4 => h.editor.connect_live(&telemetry, 1),
            5 => {
                h.modulation.follower.gain_db = 6.0;
                draw(&mut h);
            }
            6 => {
                h.move_to(0.0, 0.0, Modifiers::empty());
                h.press(MouseButton::Left);
            }
            7 => h.editor.invalidate_trace(),
            _ => {
                drop(h.editor);
                assert!(telemetry.audition.snapshot().is_none());
                continue;
            }
        }
        assert!(
            telemetry.audition.snapshot().is_none(),
            "cleanup case {case}"
        );
    }
}

#[test]
fn disconnected_listen_reports_unavailability_inside_the_small_panel() {
    for detector_panel in [false, true] {
        let mut h = Harness::new();
        h.geometry.window_size = (300.0, 180.0);
        if detector_panel {
            h.modulation.trigger = TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 50.0,
            };
            h.click(h.geometry.layout.detector_setup);
        } else {
            open_follower(&mut h);
        }
        h.key(Key::Character("l".into()));
        assert_eq!(h.editor.listen_label(), "Offline");
        assert!(!h.editor.listening(AuditionSource::Trigger));
        assert!(!h.editor.listening(AuditionSource::Follower));
        draw(&mut h);
    }
}
#[test]
fn invalidated_leases_do_not_republish_and_old_editor_drop_cannot_stop_a_new_owner() {
    let (mut h, telemetry) = setup(1.0);
    open_follower(&mut h);
    h.key(Key::Character("l".into()));
    telemetry.invalidate();
    draw(&mut h);
    assert!(telemetry.audition.snapshot().is_none());
    assert!(!h.editor.listening(AuditionSource::Follower));
    open_follower(&mut h);
    h.key(Key::Character("l".into()));
    let settings = AuditionSettings::for_source(&h.modulation, AuditionSource::Follower).unwrap();
    let token = telemetry.audition.begin(
        1,
        h.effect.kind,
        AuditionSource::Follower,
        settings,
        telemetry.waveform.generation(),
    );
    drop(h.editor);
    assert!(telemetry.audition.is_current(token));
}
