//! Offscreen fixtures for modal controls that share layout and input geometry.

use super::*;
use crate::editor::follower as follower_ui;
use crate::modulation::{
    AudioTriggerSettings, DetectorInput, DetectorReading, FollowerMode, FollowerReading,
    MidiWaveMapping, ModulationState, TransientAlgorithm,
};
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry};
use std::sync::{Arc, Mutex};
use tiny_skia::Pixmap;

fn raster(h: &Harness) -> Pixmap {
    let (width, height) = h.geometry.window_size;
    Pixmap::new(width as u32, height as u32).unwrap()
}

fn popup(h: &mut Harness) -> Pixmap {
    let mut pixmap = raster(h);
    h.editor
        .draw_popup(&mut pixmap, &mut h.tr, &h.effect, h.geometry);
    pixmap
}

fn assert_raster(case: &str, pixmap: Pixmap, expected: &[u64]) {
    let digest = pixmap
        .data()
        .chunks(4)
        .fold(0xcbf2_9ce4_8422_2325, |hash, rgba| {
            (hash ^ u32::from_le_bytes(rgba.try_into().unwrap()) as u64)
                .wrapping_mul(0x0000_0100_0000_01b3)
        });
    // Draft fixtures accept both phases of the blinking caret.
    assert!(
        expected.contains(&digest),
        "{case} raster was {digest:#018x}, expected {expected:x?}"
    );
}

#[test]
fn midi_filter_rows_render_the_selected_values_and_use_scaled_geometry() {
    let mut h = Harness::new();
    h.resize(1.25);
    let layout = h.geometry.layout;
    let scale = h.geometry.scale;
    let rate = layout.trigger_rate;
    let aux = layout.trigger_aux;
    for (trigger, fields) in [
        (
            TriggerSource::MidiNote {
                note: Some(79),
                channel: Some(12),
            },
            [(rate, "Note", "G5"), (aux, "Ch.", "13")],
        ),
        (
            TriggerSource::MidiPitch {
                transpose: -17,
                channel: None,
            },
            [(rate, "Semi", "-17"), (aux, "Ch.", "Any")],
        ),
    ] {
        h.modulation.trigger = trigger;
        let mut actual = raster(&h);
        let mut expected = actual.clone();
        with_session(&mut h, |session| session.draw_midi_controls(&mut actual));
        for (rect, label, value) in fields {
            let y = rect.1 + 35.0;
            h.tr.draw_text(
                &mut expected,
                rect.0 + 5.0,
                y - 6.25,
                label,
                13.75,
                widgets::color_muted(),
            );
            widgets::draw_button(
                &mut expected,
                &mut h.tr,
                rect.0,
                y,
                rect.2,
                42.5,
                value,
                false,
                false,
            );
        }
        assert!(
            actual.data() == expected.data(),
            "MIDI controls for {trigger:?}"
        );
    }
    h.modulation.trigger = TriggerSource::Free;
    let mut actual = raster(&h);
    with_session(&mut h, |session| session.draw_midi_controls(&mut actual));
    assert!(actual.data().iter().all(|byte| *byte == 0));

    let anchor = (50.0, 40.0, 17.0, 60.0);
    h.geometry.layout.trigger_rate = anchor;
    h.modulation.trigger = TriggerSource::MidiNote {
        note: Some(79),
        channel: Some(12),
    };
    with_session(&mut h, |session| session.open_midi_filter(MidiField::Note));
    let list = widgets::dropdown::dropdown_popup_layout(
        &h.editor.effect_dropdown,
        MidiField::Note.items(),
        h.geometry.window_size,
    )
    .unwrap();
    let rect = MidiField::Note.rect(&h.geometry.layout, scale);
    assert_eq!(rect, (50.0, 75.0, 17.0, 42.5));
    let longest_label = MidiField::Note
        .items()
        .iter()
        .map(|label| h.tr.text_width(label, 21.25))
        .fold(0.0_f32, f32::max);
    assert!((list.popup_rect.2 - (longest_label + 19.0)).abs() < 1e-5);
}

#[test]
fn midi_pitch_rate_switches_precision_at_ten_hertz_and_units_at_one_kilohertz() {
    use crate::modulation::{MidiPitchReading, MidiPitchSettings};

    let mut h = Harness::new();
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    h.modulation.trigger = TriggerSource::MidiPitch {
        transpose: 0,
        channel: None,
    };
    for (hz, label) in [
        (9.843, "9.84 Hz"),
        (10.0, "10.0 Hz"),
        (987.625, "987.6 Hz"),
        (1000.0, "1.0 kHz"),
    ] {
        let mut frame = frame(&h);
        frame.pitch = Some(MidiPitchReading {
            settings: MidiPitchSettings {
                transpose: 0,
                channel: None,
            },
            note: Some(60),
            hz,
        });
        set_frame(&mut h, frame);
        let mut actual = raster(&h);
        let mut expected = actual.clone();
        with_session(&mut h, |session| session.draw_pitch_rate(&mut actual));
        for (rect, text) in [
            (h.geometry.layout.mseg_sync, "Note 60"),
            (h.geometry.layout.mseg_length, label),
        ] {
            widgets::draw_button(
                &mut expected,
                &mut h.tr,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
                text,
                false,
                false,
            );
        }
        assert!(actual.data() == expected.data(), "MIDI rate label {label}");
    }
}

#[test]
fn live_controls_render_frozen_audio_and_supported_or_unavailable_smoothing() {
    let mut h = Harness::new();
    h.editor.set_audio_view(AudioView::Sidechain);
    h.editor.set_audio_frozen(true);
    h.editor.selected_mseg = 1;
    h.editor.trace_enabled = true;
    let layout = h.geometry.layout;
    let stepped = effects::parameter_specs(h.effect.kind)
        .iter()
        .position(|spec| matches!(spec.format, effects::ParamFormat::Enum { .. }))
        .unwrap();
    for (target, ms, label, level) in [
        (None, 0.0, "Off", 0.0),
        (None, 19.0, "19.0 ms", 20.0_f32.ln() / 1001.0_f32.ln()),
        (
            Some(ModulationTarget::EffectParameter(stepped)),
            19.0,
            "n/a",
            0.0,
        ),
    ] {
        h.modulation.targets[0] = target;
        h.modulation.smooth_ms[1] = ms;
        let mut actual = raster(&h);
        let mut expected = actual.clone();
        with_session(&mut h, |session| session.draw_live_controls(&mut actual));
        widgets::dropdown::draw_dropdown_trigger(
            &mut expected,
            &mut h.tr,
            layout.audio,
            "Frozen: SC",
            false,
        );
        let r = layout.smooth;
        widgets::draw_slider(
            &mut expected,
            &mut h.tr,
            r.0,
            r.1,
            r.2,
            r.3,
            "Smooth",
            label,
            level,
            None,
            false,
        );
        if target.is_some() {
            widgets::draw_rect_outline(
                &mut expected,
                r.0,
                r.1,
                r.2,
                r.3,
                widgets::color_muted(),
                1.0,
            );
        }
        let r = layout.trace;
        widgets::draw_button(
            &mut expected,
            &mut h.tr,
            r.0,
            r.1,
            r.2,
            r.3,
            "Trace",
            true,
            false,
        );
        assert!(actual.data() == expected.data(), "smoothing label {label}");
    }
    with_session(&mut h, |session| session.apply_smooth(1, 42.0));
    assert_eq!(h.modulation.smooth_ms[1], 19.0);
}

fn frame(h: &Harness) -> ModulationFrame {
    ModulationFrame::capture(h.effect.kind, &h.modulation, &ModulationState::default())
}

fn set_frame(h: &mut Harness, frame: ModulationFrame) {
    h.editor.live.as_mut().unwrap().frame = Some(frame);
}

#[test]
fn follower_meter_rejects_stale_configuration_and_nonfinite_readings() {
    let mut h = Harness::new();
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    h.click(widgets::mseg::follower_button_rect(strip(&h), 1.0));
    assert!(h.editor.follower_panel_open());
    let current = h.modulation.follower;
    for (config, level, envelope, visible) in [
        (current, 0.25, 0.5, true),
        (
            crate::modulation::FollowerConfig {
                gain_db: 6.0,
                ..current
            },
            0.25,
            0.5,
            false,
        ),
        (current, f32::NAN, 0.5, false),
        (current, 0.25, f32::INFINITY, false),
    ] {
        let mut reading = frame(&h);
        reading.follower = Some(FollowerReading {
            config,
            level,
            envelope,
        });
        set_frame(&mut h, reading);
        assert_eq!(h.editor.follower_reading().is_some(), visible);
    }
}

#[test]
fn narrow_follower_subtitle_and_footer_hint_stay_inside_the_available_space() {
    let mut h = Harness::new();
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    h.resize(0.5);
    h.geometry.window_size = (224.0, 350.0);
    with_session(&mut h, |session| session.open_follower());
    let layout = follower_ui::layout(h.geometry, 0).unwrap();
    let (x, y, w, height) = layout.panel;
    let label = "Mode/Amount: this curve; response: shared";
    let end = label
        .char_indices()
        .map(|(i, _)| i)
        .rev()
        .find(|&i| h.tr.text_width(&label[..i], 10.0) <= 200.0)
        .unwrap();
    let mut expected = raster(&h);
    widgets::draw_rect(&mut expected, x, y, w, height, widgets::color_bg());
    h.tr.draw_text(
        &mut expected,
        x + 8.0,
        y + 34.0,
        &label[..end],
        10.0,
        widgets::color_muted(),
    );
    let actual = popup(&mut h);
    assert!(((y as u32 + 24)..(y as u32 + 38)).all(|y| {
        ((x as u32 + 4)..((x + w) as u32 - 4)).all(|x| actual.pixel(x, y) == expected.pixel(x, y))
    }));

    h.resize(0.75);
    h.modulation.follower.routes[1].mode = FollowerMode::Multiply;
    with_session(&mut h, |session| session.open_follower());
    let layout = follower_ui::layout(h.geometry, 0).unwrap();
    let (x, y, w, height) = layout.panel;
    let mut expected = raster(&h);
    widgets::draw_rect(&mut expected, x, y, w, height, widgets::color_bg());
    let actual = popup(&mut h);
    let left = (layout.reset.0 + layout.reset.2 + 2.0).ceil() as u32;
    let right = (layout.listen.0 - 2.0).floor() as u32;
    let top = (layout.reset.1 + 3.0).ceil() as u32;
    let bottom = (layout.reset.1 + layout.reset.3 - 2.0).floor() as u32;
    assert!(
        (top..bottom).all(|y| (left..right).all(|x| actual.pixel(x, y) == expected.pixel(x, y)))
    );
}

fn strip(h: &Harness) -> Rect {
    h.editor
        .mseg_edit
        .layout(h.geometry.layout.mseg_pane, h.geometry.scale)
        .strip
}

fn with_session(h: &mut Harness, run: impl FnOnce(&mut Session<'_>)) {
    run(&mut Session::new(
        &mut h.editor,
        EditorContext {
            effect: &mut h.effect,
            modulation: &mut h.modulation,
            text_renderer: &mut h.tr,
            geometry: h.geometry,
            pointer_position: h.pointer,
        },
    ));
}

fn start_draft(h: &mut Harness, rect: Rect, value: &str) {
    h.move_to(
        rect.0 + rect.2 * 0.5,
        rect.1 + rect.3 * 0.5,
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    h.type_replacement(value);
}

fn assert_draft_pixels(h: &mut Harness, rect: Rect, label: &str, norm: f32, draft: &str) {
    let actual = popup(h);
    let (x, y, w, height) = rect;
    let columns = (x.ceil() as u32 + 2)..((x + w).floor() as u32 - 2);
    let rows = (y.ceil() as u32 + 2)..((y + height).floor() as u32 - 2);
    assert!(
        [false, true].into_iter().any(|caret| {
            let mut expected = raster(h);
            widgets::draw_slider(
                &mut expected,
                &mut h.tr,
                x,
                y,
                w,
                height,
                label,
                "",
                norm,
                Some(draft),
                caret,
            );
            rows.clone().all(|y| {
                columns
                    .clone()
                    .all(|x| actual.pixel(x, y) == expected.pixel(x, y))
            })
        }),
        "visible draft {draft:?} for {label}"
    );
}

#[test]
fn detector_filter_drafts_use_the_drawn_font_and_keep_an_exact_fit() {
    for (scale, visible) in [(0.5, "1e+20"), (0.8574392, "-1e+20")] {
        let mut h = Harness::new();
        h.resize(scale);
        h.modulation.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        h.click(h.geometry.layout.detector_setup);
        let layout = detector::detector_layout(h.geometry);
        let cutoff = layout.fields[2];
        start_draft(&mut h, cutoff, "-1e+20");
        assert_draft_pixels(&mut h, cutoff, "Low-pass", 0.0, visible);
    }
}

#[test]
fn detector_detail_and_timing_drafts_measure_with_the_same_font_as_the_field() {
    let mut h = Harness::new();
    h.resize(0.503);
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    h.click(h.geometry.layout.detector_setup);
    let detail = detector::detector_layout(h.geometry).detail;
    start_draft(&mut h, detail, "-9.9999");
    assert_draft_pixels(&mut h, detail, "Detail", 0.5, "-9.9999");
    h.key(Key::Escape);
    h.key(Key::Escape);
    open_timing(&mut h);
    let shift = timing::layout(h.geometry).fields[0];
    start_draft(&mut h, shift, "-9.9999");
    assert_draft_pixels(&mut h, shift, "Audio shift", 0.0, "-9.9999");
}

#[test]
fn follower_popup_rasters_cover_selected_routes_scrolling_meters_and_offline_copy() {
    let mut h = Harness::new();
    h.click(widgets::mseg::follower_button_rect(strip(&h), 1.0));
    assert!(h.editor.follower_panel_open());
    assert_raster("follower-offline", popup(&mut h), &[0x9d72_0abc_4091_d6e7]);

    h.resize(0.5);
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    h.editor.selected_mseg = 2;
    h.modulation.follower.routes[2].enabled = true;
    h.modulation.follower.routes[2].mode = FollowerMode::Add;
    h.modulation.follower.routes[2].amount = -0.375;
    h.modulation.follower.detector = AudioTriggerSettings {
        input: DetectorInput::Sidechain,
        level_db: Some(-32.0),
        highpass_hz: Some(400.0),
        lowpass_hz: Some(5630.0),
    };
    with_session(&mut h, |session| session.open_follower());
    assert!(h.editor.follower_panel_open());
    h.key(Key::Character("l".into()));
    let mut reading = frame(&h);
    reading.follower = Some(FollowerReading {
        config: h.modulation.follower.normalized(),
        level: 0.15,
        envelope: 0.425,
    });
    set_frame(&mut h, reading);
    assert_raster(
        "follower-listening",
        popup(&mut h),
        &[0x764c_24b5_827b_6936],
    );

    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    assert_raster(
        "follower-mode-menu",
        popup(&mut h),
        &[0x67a2_5301_3e62_5f31],
    );
    h.key(Key::Escape);
    h.key(Key::End);
    assert_raster("follower-scrolled", popup(&mut h), &[0x5542_7552_b208_550a]);
    let lowpass = follower_ui::layout(h.geometry, h.editor.follower.scroll)
        .unwrap()
        .rows[10]
        .unwrap();
    start_draft(&mut h, lowpass, "10000.000 Hz");
    assert_raster(
        "follower-draft",
        popup(&mut h),
        &[0x27c4_103f_7248_7593, 0x0c8b_075e_bf92_246b],
    );
    h.key(Key::Escape);
    start_draft(&mut h, lowpass, "-9999.99");
    assert_raster(
        "follower-exact-width-draft",
        popup(&mut h),
        &[0x6365_a1ee_6aae_762d, 0x5749_4b0c_2f23_fe99],
    );

    h.resize(1.0);
    h.editor.selected_mseg = 0;
    h.modulation.follower.routes[0].enabled = true;
    h.modulation.follower.routes[0].mode = FollowerMode::Multiply;
    h.geometry.layout.mseg_pane = (16.0, 20.0, 980.0, 820.0);
    h.geometry.window_size = (1050.0, 780.0);
    with_session(&mut h, |session| session.open_follower());
    let mut quiet = frame(&h);
    quiet.follower = Some(FollowerReading {
        config: h.modulation.follower.normalized(),
        level: 0.0,
        envelope: 0.0,
    });
    set_frame(&mut h, quiet);
    assert_raster("follower-silent", popup(&mut h), &[0xb073_a0cc_63fb_61c1]);

    h.resize(0.75);
    h.modulation.follower.routes[0].mode = FollowerMode::Follow;
    with_session(&mut h, |session| session.open_follower());
    assert_raster(
        "follower-compact-hint",
        popup(&mut h),
        &[0x958e_5dee_7df0_11b6],
    );
}

#[test]
fn detector_popup_rasters_cover_meter_states_algorithm_choice_and_window_warning() {
    let mut h = Harness::new();
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    h.click(h.geometry.layout.detector_setup);
    assert!(h.editor.detector_panel_open());
    assert_raster("detector-offline", popup(&mut h), &[0x3129_59d9_aefe_3547]);

    h.resize(0.5);
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    h.modulation.audio_trigger = AudioTriggerSettings {
        input: DetectorInput::Sidechain,
        level_db: Some(-38.0),
        highpass_hz: Some(160.0),
        lowpass_hz: Some(9620.0),
    };
    h.modulation.detector_response.algorithm = TransientAlgorithm::Rms;
    h.modulation.detector_response.detail = 65.0;
    h.click(h.geometry.layout.detector_setup);
    h.key(Key::Character("l".into()));
    let mut reading = frame(&h);
    reading.detector = Some(DetectorReading {
        settings: h.modulation.audio_trigger.normalized(),
        response: h.modulation.detector_response.normalized(),
        level: 0.16,
    });
    set_frame(&mut h, reading);
    assert_raster("detector-measured", popup(&mut h), &[0x749b_321f_7886_168f]);

    let mut silent = reading;
    silent.detector.as_mut().unwrap().level = 0.0;
    set_frame(&mut h, silent);
    let layout = detector::detector_layout(h.geometry);
    h.click(layout.algorithm);
    assert_raster(
        "detector-algorithm-menu",
        popup(&mut h),
        &[0x47b8_6da7_ad17_2689],
    );
    h.key(Key::Escape);
    start_draft(&mut h, layout.fields[2], "10000.000 Hz");
    assert_raster(
        "detector-filter-draft",
        popup(&mut h),
        &[0x0650_8d54_b2c2_6f8f, 0xc921_ef73_b980_740f],
    );
    h.key(Key::Escape);
    start_draft(&mut h, layout.detail, "100000.000%");
    assert_raster(
        "detector-detail-draft",
        popup(&mut h),
        &[0x2eb7_da8f_3d1a_0eaa, 0x04bb_b15a_078e_200a],
    );
    h.key(Key::Escape);
    start_draft(&mut h, layout.detail, "-999.99");
    assert_raster(
        "detector-exact-width-draft",
        popup(&mut h),
        &[0x4c5c_82d5_f8df_be4f, 0x4213_fafd_0e72_51cf],
    );
    h.key(Key::Escape);
    h.geometry.window_size = (220.0, 140.0);
    assert_raster("detector-short", popup(&mut h), &[0xf61e_62ca_dc53_5e81]);
    h.geometry.window_size = (90.0, 72.0);
    assert_raster(
        "detector-narrow-warning",
        popup(&mut h),
        &[0x29ac_f2ee_b921_09bb],
    );
}

fn open_timing(h: &mut Harness) {
    h.click(h.geometry.layout.trigger);
    h.key(Key::End);
    h.key(Key::Enter);
    assert!(h.editor.timing_panel_open());
}

#[test]
fn height_limited_setup_panels_center_each_warning_line() {
    for detector_panel in [false, true] {
        let mut h = Harness::new();
        if detector_panel {
            h.modulation.trigger = TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 50.0,
            };
            h.click(h.geometry.layout.detector_setup);
        } else {
            open_timing(&mut h);
        }
        for available_width in [640.0, 80.0] {
            h.geometry.window_size = (available_width, 300.0);
            let (rect, panel_line) = if detector_panel {
                let layout = detector::detector_layout(h.geometry);
                assert!(!layout.usable);
                (layout.rect, "for detector")
            } else {
                let layout = timing::layout(h.geometry);
                assert!(!layout.usable);
                (layout.rect, "for timing")
            };
            let (x, y, w, height) = rect;
            let mut expected = raster(&h);
            widgets::draw_rect(&mut expected, x, y, w, height, widgets::color_bg());
            widgets::draw_rect_outline(
                &mut expected,
                x,
                y,
                w,
                height,
                widgets::color_accent(),
                1.0,
            );
            let first_line = y + (height - 120.0).max(0.0) * 0.5 + 20.0;
            for (line, text) in ["Enlarge", "the window", panel_line, "setup", "Click / Esc"]
                .into_iter()
                .enumerate()
            {
                let width = h.tr.text_width(text, 20.0);
                h.tr.draw_text(
                    &mut expected,
                    (x + (w - width) * 0.5).max(x + 2.0),
                    first_line + line as f32 * 24.0,
                    text,
                    20.0,
                    widgets::color_text(),
                );
            }
            assert!(
                popup(&mut h).data() == expected.data(),
                "{panel_line} at width {available_width}"
            );
        }
    }
}

#[test]
fn timing_popup_rasters_cover_signed_shift_focus_and_window_warning() {
    let mut h = Harness::new();
    open_timing(&mut h);
    assert_raster("timing-off", popup(&mut h), &[0x19db_9272_297a_b747]);

    h.resize(2.0);
    h.modulation.trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 50.0,
    };
    h.modulation.lookahead = true;
    h.modulation.trigger_timing.audio_delay_ms = -1.75;
    h.modulation.trigger_timing.blend_ms = 8.5;
    open_timing(&mut h);
    h.key(Key::Home);
    h.key(Key::Tab);
    h.key(Key::Tab);
    assert_raster(
        "timing-lookahead-focus",
        popup(&mut h),
        &[0x604f_455a_adf1_e1a7],
    );
    h.key(Key::End);
    assert_raster(
        "timing-reset-focus",
        popup(&mut h),
        &[0x8be3_5b8f_7856_28d3],
    );
    let blend = timing::layout(h.geometry).fields[1];
    start_draft(&mut h, blend, "12345.000 ms");
    assert_raster(
        "timing-blend-draft",
        popup(&mut h),
        &[0x8b1e_afc4_6c83_fdeb, 0xee7f_d6f9_8f43_ea8b],
    );
    h.key(Key::Escape);
    start_draft(&mut h, blend, "-999.99");
    assert_raster(
        "timing-exact-width-draft",
        popup(&mut h),
        &[0x644a_4909_a96d_5504, 0xf88a_ca81_2260_45e4],
    );

    h.resize(0.5);
    h.modulation.trigger_timing.audio_delay_ms = 0.0;
    open_timing(&mut h);
    assert_raster("timing-zero-shift", popup(&mut h), &[0x1e50_8c9d_a317_b5ef]);
    h.geometry.window_size = (190.0, 156.0);
    assert_raster("timing-short", popup(&mut h), &[0x8606_6706_cde2_0f7f]);
    h.geometry.window_size = (95.0, 72.0);
    assert_raster(
        "timing-narrow-warning",
        popup(&mut h),
        &[0x1f9d_e72f_c298_4a60],
    );
}

#[test]
fn wave_popup_rasters_cover_route_controls_and_project_override_at_compact_scale() {
    let mut h = Harness::new();
    h.editor
        .bind_wave_palette(Arc::new(Mutex::new(widgets::mseg::WavePalette::default())));
    h.click(widgets::mseg::wave_midi_button_rect(strip(&h), 1.0));
    assert!(h.editor.wave_midi_panel_open());
    assert_raster("waves-off", popup(&mut h), &[0x5ba2_7363_8001_253b]);

    h.resize(0.5);
    h.editor.selected_mseg = 3;
    h.modulation.wave_switch = Some(MidiWaveMapping {
        envelope: 2,
        first_note: 114,
        channel: Some(13),
    });
    h.editor.set_wave_automation([0, 0, 0, 7]);
    with_session(&mut h, |session| session.open_wave_midi());
    assert!(h.editor.wave_midi_panel_open());
    for _ in 0..4 {
        h.key(Key::ArrowDown);
    }
    assert_raster("waves-project", popup(&mut h), &[0x9a7c_5a22_a90a_1449]);
    h.key(Key::Enter);
    assert_raster("waves-picker", popup(&mut h), &[0x35fe_6550_4a8f_e5ec]);

    h.resize(2.0);
    h.geometry.window_size = (290.0, 1200.0);
    h.editor.set_wave_automation([0; 4]);
    with_session(&mut h, |session| session.open_wave_midi());
    assert!(h.editor.wave_midi_panel_open());
    assert_raster("waves-narrow", popup(&mut h), &[0x83eb_f690_037b_eed9]);
}

fn wave_button(h: &mut Harness) -> Pixmap {
    let mut pixmap = raster(h);
    with_session(h, |session| session.draw_wave_midi_button(&mut pixmap));
    pixmap
}

#[test]
fn compact_wave_button_clips_and_centers_the_project_label_inside_its_padding() {
    let mut h = Harness::new();
    let scale = 0.35;
    h.resize(scale);
    h.geometry.layout.mseg_pane = (12.0, 12.0, 430.0, 204.0);
    h.editor
        .bind_wave_palette(Arc::new(Mutex::new(widgets::mseg::WavePalette::default())));
    h.editor.set_wave_automation([0, 3, 0, 0]);
    let actual = wave_button(&mut h);
    let mut expected = raster(&h);
    let (x, y, w, height) = widgets::mseg::wave_midi_button_rect(strip(&h), scale);
    let font = 10.0;
    let limit = w - 6.0;
    assert!(limit > 10.0 && limit < h.tr.text_width("Wave P3", font));
    let end = "Wave P3"
        .char_indices()
        .map(|(index, _)| index)
        .rev()
        .find(|&end| h.tr.text_width(&"Wave P3"[..end], font) <= limit)
        .unwrap();
    widgets::draw_button(&mut expected, &mut h.tr, x, y, w, height, "", true, false);
    h.tr.draw_text(
        &mut expected,
        x + 3.0,
        y + (height + font) * 0.5 - 2.0,
        &"Wave P3"[..end],
        font,
        widgets::color_bg(),
    );
    assert!(
        actual.data() == expected.data(),
        "clipped Project-wave label"
    );
}

#[test]
fn wave_button_rasters_distinguish_midi_pending_measured_stale_and_automated_selection() {
    let mut h = Harness::new();
    assert_raster(
        "wave-button-hidden",
        wave_button(&mut h),
        &[0xa66a_2c6b_9711_0225],
    );
    h.editor
        .bind_wave_palette(Arc::new(Mutex::new(widgets::mseg::WavePalette::default())));
    assert_raster(
        "wave-button-idle",
        wave_button(&mut h),
        &[0x1fbe_8d43_bb76_1d84],
    );
    let mapping = MidiWaveMapping {
        envelope: 1,
        first_note: 50,
        channel: Some(1),
    };
    h.modulation.wave_switch = Some(mapping);
    assert_raster(
        "wave-button-midi",
        wave_button(&mut h),
        &[0xeab8_6161_f236_0059],
    );
    let telemetry = Arc::new(ModulationTelemetry::default());
    h.editor.connect_live(&telemetry, 0);
    let mut measured = frame(&h);
    measured.wave = Some((mapping, Some(5)));
    set_frame(&mut h, measured);
    assert_raster(
        "wave-button-measured",
        wave_button(&mut h),
        &[0xb731_5503_8083_d6c1],
    );
    let mut stale = measured;
    stale.kind = EffectKind::Bitcrush;
    set_frame(&mut h, stale);
    assert_raster(
        "wave-button-stale",
        wave_button(&mut h),
        &[0xeab8_6161_f236_0059],
    );
    h.resize(2.0);
    h.modulation.wave_switch = None;
    h.editor.set_wave_automation([0, 3, 0, 0]);
    assert_raster(
        "wave-button-project",
        wave_button(&mut h),
        &[0x24ed_72cf_2922_2f84],
    );
}
