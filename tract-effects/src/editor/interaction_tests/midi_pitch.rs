use super::*;
use crate::modulation::{MidiNoteOn, ModulationState};
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
use std::sync::Arc;

fn setup(scale: f32) -> Harness {
    let mut h = Harness::new();
    h.geometry.scale = scale;
    h.geometry.layout = view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale);
    h.geometry.window_size = (1336.0 * scale, 680.0 * scale);
    h.modulation.trigger = TriggerSource::MidiPitch {
        transpose: 0,
        channel: None,
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
fn midi_pitch_selectors_keep_popup_priority_and_preserve_inactive_lengths_at_each_scale() {
    for scale in [0.5, 1.0, 2.0] {
        let mut h = setup(scale);
        let curves = h.modulation.msegs;
        h.modulation.trigger = TriggerSource::Free;
        h.click(h.geometry.layout.trigger);
        h.key(Key::End);
        h.key(Key::Enter);
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::MidiPitch {
                transpose: 0,
                channel: None
            }
        );
        let transpose = MidiField::Transpose.rect(&h.geometry.layout, scale);
        let channel = MidiField::Channel.rect(&h.geometry.layout, scale);
        h.click(transpose);
        assert!(
            h.editor
                .effect_dropdown
                .is_open_for(EffectAction::MidiFilter(MidiField::Transpose))
        );
        h.key(Key::Home);
        h.key(Key::Enter);
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::MidiPitch {
                transpose: -36,
                channel: None
            }
        );
        h.click(channel);
        h.key(Key::End);
        h.key(Key::Enter);
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::MidiPitch {
                transpose: -36,
                channel: Some(15)
            }
        );
        h.click(transpose);
        h.key(Key::End);
        draw(&mut h);
        h.press(MouseButton::Right);
        assert!(!h.editor.effect_dropdown.is_open());
        assert!(!h.editor.text_edit.is_active());
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::MidiPitch {
                transpose: -36,
                channel: Some(15)
            }
        );
        h.click(h.geometry.layout.mseg_sync);
        h.click(h.geometry.layout.mseg_length);
        assert_eq!(h.modulation.msegs, curves);
        assert!(h.editor.effect_dial_drag.active_action().is_none());
        h.click(transpose);
        h.editor.prepare_resize(1.0);
        assert!(!h.editor.effect_dropdown.is_open());
    }
}
#[test]
fn midi_pitch_trace_rejects_the_previous_clock_and_readouts_require_prepared_settings() {
    let mut h = setup(1.0);
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    h.editor.set_audio_view(AudioView::Input);
    h.editor.set_audio_frozen(true);
    draw(&mut h);
    let mut clock = TraceClock::default();
    clock.begin_block(&telemetry, 48000.0);
    let mut old_config = h.modulation.clone();
    old_config.trigger = TriggerSource::Free;
    let old = ModulationFrame::capture(h.effect.kind, &old_config, &ModulationState::default());
    clock.record(&telemetry, 0, &old);
    clock.publish(&telemetry, 0, &old);
    assert_eq!(telemetry.head(), 0);
    let mut state = ModulationState::default();
    let mut effect = crate::effects::EffectInstance::new(h.effect.kind);
    state.begin_block(&h.modulation, &[0.0], &[0.0], 48000.0);
    state.midi_note_on(
        &h.modulation,
        MidiNoteOn::new(0, 0, 69, 1.0).unwrap(),
        false,
    );
    state.advance_segment(&h.modulation, &h.effect, &mut effect, 1, 120.0, 48000.0);
    state.process_modulation_sample(&h.modulation, &mut effect, [0.0; 2], [0.0; 2]);
    let actual = ModulationFrame::capture(h.effect.kind, &h.modulation, &state);
    clock.publish(&telemetry, 0, &actual);
    draw(&mut h);
    assert_eq!(
        h.editor
            .live
            .as_ref()
            .unwrap()
            .frame
            .unwrap()
            .pitch
            .unwrap()
            .hz,
        440.0
    );
    h.modulation.trigger = TriggerSource::MidiPitch {
        transpose: 12,
        channel: None,
    };
    draw(&mut h);
    assert!(h.editor.live.as_ref().unwrap().frame.is_none());
    assert!(h.editor.audio_frozen);
}

#[test]
fn external_pitch_mode_changes_finish_length_drags_before_they_can_edit_saved_lengths() {
    let mut h = setup(1.0);
    h.modulation.trigger = TriggerSource::Free;
    let rect = h.geometry.layout.mseg_length;
    h.move_to(
        rect.0 + rect.2 * 0.5,
        rect.1 + rect.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Left);
    assert_eq!(
        h.editor.effect_dial_drag.active_action(),
        Some(&EffectHit::MsegLength)
    );
    let curves = h.modulation.msegs;
    h.modulation.trigger = TriggerSource::MidiPitch {
        transpose: 0,
        channel: None,
    };
    let result = h.move_to(
        rect.0 + rect.2 * 0.5,
        rect.1 + rect.3 * 0.5 - 50.0,
        Modifiers::empty(),
    );
    assert!(!result.changed);
    assert_eq!(h.modulation.msegs, curves);
    assert!(h.editor.effect_dial_drag.active_action().is_none());
}

#[test]
fn complete_extreme_pitch_readouts_fit_and_render_with_units_at_each_scale() {
    use crate::modulation::{MidiPitchReading, MidiPitchSettings};
    for scale in [0.5, 1.0, 2.0] {
        let mut h = setup(scale);
        h.modulation.trigger = TriggerSource::MidiPitch {
            transpose: 36,
            channel: Some(15),
        };
        let telemetry = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&telemetry, 0);
        draw(&mut h);
        let mut frame =
            ModulationFrame::capture(h.effect.kind, &h.modulation, &ModulationState::default());
        frame.pitch = Some(MidiPitchReading {
            settings: MidiPitchSettings {
                transpose: 36,
                channel: Some(15),
            },
            note: Some(127),
            hz: 440.0 * 2.0_f64.powf(94.0 / 12.0),
        });
        let mut clock = TraceClock::default();
        clock.begin_block(&telemetry, 48000.0);
        clock.publish(&telemetry, 0, &frame);
        let actual = draw(&mut h);
        let mut expected = actual.clone();
        for (r, label) in [
            (h.geometry.layout.mseg_sync, "Note 127"),
            (h.geometry.layout.mseg_length, "100.4 kHz"),
        ] {
            assert!(h.tr.text_width(label, (r.3 * 0.5).max(10.0)) <= r.2 - 8.0 * scale);
            widgets::draw_button(
                &mut expected,
                &mut h.tr,
                r.0,
                r.1,
                r.2,
                r.3,
                label,
                false,
                false,
            );
            let x0 = r.0.ceil() as u32;
            let x1 = (r.0 + r.2).floor() as u32;
            let y0 = r.1.ceil() as u32;
            let y1 = (r.1 + r.3).floor() as u32;
            assert!((x0..x1).all(|x| (y0..y1).all(|y| actual.pixel(x, y) == expected.pixel(x, y))));
        }
    }
}

#[test]
fn source_replacement_closes_midi_selectors_before_they_can_modify_the_new_source() {
    for field in [MidiField::Channel, MidiField::Transpose] {
        let mut h = setup(1.0);
        if field == MidiField::Channel {
            h.modulation.trigger = TriggerSource::MidiNote {
                note: Some(60),
                channel: None,
            };
        }
        h.click(field.rect(&h.geometry.layout, 1.0));
        assert!(h.editor.effect_dropdown.is_open());
        let replacement = TriggerSource::MidiPitch {
            transpose: 12,
            channel: Some(3),
        };
        h.modulation.trigger = replacement;
        let response = h.key(Key::End);
        assert!(response.captured && !response.changed);
        assert!(!h.editor.effect_dropdown.is_open());
        h.key(Key::Enter);
        assert_eq!(h.modulation.trigger, replacement);
    }
}
