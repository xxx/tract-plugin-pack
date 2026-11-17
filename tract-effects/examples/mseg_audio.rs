//! Measured preview: cargo run -p tract-effects --example mseg_audio -- /tmp/mseg-audio.png stereo

use std::sync::Arc;
use tiny_skia_widgets::mseg::{FactoryShape, factory_shape};
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode, TextRenderer, color_bg, color_text};
use tract_effects::editor::{
    AudioDisplay, AudioGain, AudioView, EditorGeometry, EffectEditor, view::effect_layout,
};
use tract_effects::effects::{
    Effect, EffectInstance, EffectKind, TrackEffect, default_params_for_kind,
};
use tract_effects::modulation::{
    MidiNoteOn, ModulationState, ModulationTarget, TrackModulation, TriggerSource,
};
use tract_effects::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
use tract_effects::waveform::{WaveformClock, WaveformTiming};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "/tmp/mseg-audio.png".into());
    let (display, caption) = match args.next().as_deref() {
        None | Some("magnitude") => (
            AudioDisplay::Magnitude,
            "Magnitude: greater absolute channel",
        ),
        Some("combined") => (
            AudioDisplay::Combined,
            "Combined: signed left/right overlays around zero",
        ),
        Some("stereo") => (AudioDisplay::Stereo, "Stereo: left above, right below"),
        _ => return Err("Display must be magnitude, combined, or stereo".into()),
    };
    let (gain, gain_label) = match args.next().as_deref() {
        None | Some("0") => (AudioGain::Unity, "0 dB"),
        Some("-12") => (AudioGain::Minus12, "-12 dB"),
        Some("12") => (AudioGain::Plus12, "+12 dB"),
        _ => return Err("Display gain must be -12, 0, or 12 dB".into()),
    };
    let demo = match args.next().as_deref() {
        None => 0,
        Some("midi") => 1,
        Some("audio") => 2,
        Some("hz") => 3,
        Some("sidechain") => 4,
        _ => return Err("Trigger demo must be midi, audio, hz, or sidechain".into()),
    };
    let midi_demo = demo == 1;
    let detector_mode = args.next();
    let lookahead_demo = detector_mode.as_deref() == Some("lookahead");
    let timing_demo = detector_mode.as_deref() == Some("timing") || lookahead_demo;
    let detector_demo = match detector_mode.as_deref() {
        None => false,
        Some("timing" | "lookahead") if demo == 2 || demo == 4 => false,
        Some("detector" | "listen" | "peak" | "rms") if demo == 2 || demo == 4 => true,
        _ => return Err("Detector/listen panel demo requires audio detector".into()),
    };
    let caption = format!(
        "{caption} — display gain {gain_label}{}",
        match demo {
            1 => " — MIDI note 60 / Ch. 10",
            2 => " — sample-timed audio",
            3 => " — sample-timed 4 Hz",
            4 => " — external sidechain retrigger",
            _ => "",
        }
    );
    let mut pm = tiny_skia::Pixmap::new(1336, 730).ok_or("Pixmap allocation failed")?;
    let mut text = TextRenderer::new(include_bytes!(
        "../../tiny-skia-widgets/test_data/DejaVuSans.ttf"
    ));
    let mut track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        mix: 0.0,
        ..TrackEffect::default()
    };
    track.params[0] = 250.0;
    let mut config = TrackModulation {
        trigger: match demo {
            1 => TriggerSource::MidiNote {
                note: Some(60),
                channel: Some(9),
            },
            2 | 4 => TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 50.0,
            },
            3 => TriggerSource::FreeHzSample { hz: 4.0 },
            _ => TriggerSource::Free,
        },
        targets: [Some(ModulationTarget::Mix), None, None],
        depths: [1.0, 0.0, 0.0],
        ..TrackModulation::default()
    };
    if detector_demo {
        config.audio_trigger = tract_effects::modulation::AudioTriggerSettings {
            level_db: Some(-36.0),
            highpass_hz: Some(120.0),
            lowpass_hz: Some(6000.0),
            ..tract_effects::modulation::AudioTriggerSettings::default()
        };
    }
    if let Some(algorithm) = match detector_mode.as_deref() {
        Some("peak") => Some(tract_effects::modulation::TransientAlgorithm::Peak),
        Some("rms") => Some(tract_effects::modulation::TransientAlgorithm::Rms),
        _ => None,
    } {
        config.detector_response = tract_effects::modulation::TransientSettings {
            algorithm,
            detail: 75.0,
        };
    }
    if demo == 4 {
        config.audio_trigger.input = tract_effects::modulation::DetectorInput::Sidechain;
    }
    if timing_demo {
        config.trigger_timing = tract_effects::modulation::TriggerTiming {
            audio_delay_ms: 2.0,
            blend_ms: 5.0,
        };
    }
    if lookahead_demo {
        config.lookahead = true;
        config.trigger_timing.audio_delay_ms = -2.0;
    }
    config.msegs[1] = MsegData {
        play_mode: PlayMode::Cyclic,
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        ..MsegData::default()
    };
    factory_shape(if demo != 0 {
        FactoryShape::Duck
    } else {
        FactoryShape::Square
    })
    .apply(&mut config.msegs[1], false)?;
    config.smooth_ms[1] = 20.0;
    let mut effect = EffectInstance::new(track.kind);
    effect.set_sample_rate(48000.0);
    for (index, _) in effect.parameters().iter().enumerate() {
        effect.set_param(index, track.params[index]);
    }
    let shared = Arc::new(ModulationTelemetry::default());
    let mut state = ModulationState::default();
    let mut trace = TraceClock::default();
    let mut audio = WaveformClock::default();
    let mut editor = EffectEditor::default();
    editor.connect_live(&shared, 0);
    editor.set_audio_view(if demo == 4 {
        AudioView::Sidechain
    } else {
        AudioView::Both
    });
    editor.set_audio_display(display);
    editor.set_audio_gain(gain);
    let geometry = EditorGeometry {
        layout: effect_layout((16.0, 60.0, 1304.0, 640.0), 1.0),
        scale: 1.0,
        window_size: (1336.0, 730.0),
    };
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    let mut input = [0.0; 960];
    let mut auxiliary = [0.0; 960];
    let block_samples = input.len();
    let mut ahead = tract_effects::lookahead::LookaheadDelay::new(
        tract_effects::lookahead::lookahead_samples(48000.0),
    );
    ahead.prepare(config.lookahead_samples(48000.0));
    for block in 0..if detector_demo { 138 } else { 145 } {
        for (index, sample) in input.iter_mut().enumerate() {
            let time = (block * block_samples + index) as f32 / 48000.0;
            let envelope = (-((time * 4.0).fract()) * 18.0).exp();
            auxiliary[index] = envelope
                * ((std::f32::consts::TAU * 2200.0 * time).sin() * 0.65
                    + (std::f32::consts::TAU * 60.0 * time).sin() * 0.25);
            *sample = if demo == 4 {
                (std::f32::consts::TAU * 220.0 * time).sin() * 0.5
            } else {
                auxiliary[index]
            };
        }
        trace.begin_block(&shared, 48000.0);
        audio.begin_block(&shared.waveform);
        state.begin_block(&config, &input, &input, 48000.0);
        if state.next_precise_trigger_with_sidechain(
            [input[0], -input[0] * 0.8],
            [auxiliary[0], auxiliary[0] * 0.8],
        ) {
            state.trigger();
        }
        audio.prepare_segment(
            &shared.waveform,
            0,
            WaveformTiming {
                config: &config,
                kind: track.kind,
                phase: state.precise_phase(1),
                trigger_epoch: state.trigger_epoch(),
                bpm: 120.0,
                sample_rate: 48000.0,
            },
        );
        state.advance_segment(&config, &track, &mut effect, input.len(), 120.0, 48000.0);
        for (index, &sample) in input.iter().enumerate() {
            let midi_event = if midi_demo {
                [
                    (6240, 60, 9),
                    (24000, 61, 9),
                    (29280, 60, 9),
                    (51360, 60, 9),
                    (70000, 60, 8),
                    (87840, 60, 9),
                    (115680, 60, 9),
                ]
                .iter()
                .find(|(sample, _, _)| *sample == block * block_samples + index)
                .and_then(|&(_, note, channel)| MidiNoteOn::new(index as u32, channel, note, 1.0))
            } else {
                None
            };
            let midi_fire = midi_event.is_some_and(|event| config.trigger.accepts_midi(event));
            let precise_fire = index > 0
                && state.next_precise_trigger_with_sidechain(
                    [sample, -sample * 0.8],
                    [auxiliary[index], auxiliary[index] * 0.8],
                );
            if midi_fire || precise_fire {
                if index > 0 {
                    state.trigger_in_span();
                } else {
                    state.trigger();
                }
                audio.prepare_segment(
                    &shared.waveform,
                    0,
                    WaveformTiming {
                        config: &config,
                        kind: track.kind,
                        phase: state.precise_phase(1),
                        trigger_epoch: state.trigger_epoch(),
                        bpm: 120.0,
                        sample_rate: 48000.0,
                    },
                );
                state.advance_segment(
                    &config,
                    &track,
                    &mut effect,
                    input.len() - index,
                    120.0,
                    48000.0,
                );
            }
            let frame = ahead.next(tract_effects::lookahead::LookaheadFrame {
                signal: [sample, -sample * 0.8],
                main: [sample, -sample * 0.8],
                sidechain: [auxiliary[index], auxiliary[index] * 0.8],
            });
            state.process_modulation_sample(&config, &mut effect, frame.main, frame.sidechain);
            let wet = effect.process_sample(frame.signal[0], frame.signal[1]);
            let mix = state.next_mix(track.mix);
            let output = [
                frame.signal[0] * (1.0 - mix) + wet.0 * mix * state.amplitude(),
                frame.signal[1] * (1.0 - mix) + wet.1 * mix * state.amplitude(),
            ];
            audio.record_sample_with_sidechain(
                &shared.waveform,
                frame.signal,
                output,
                frame.sidechain,
                state.precise_phase(1),
            );
            if trace.tick() {
                trace.record(
                    &shared,
                    0,
                    &ModulationFrame::capture(track.kind, &config, &state),
                );
            }
        }
        audio.finish_block(&shared.waveform);
        trace.publish(
            &shared,
            0,
            &ModulationFrame::capture(track.kind, &config, &state),
        );
        pm.fill(color_bg());
        text.draw_text(&mut pm, 24.0, 30.0, &caption, 20.0, color_text());
        text.draw_text(
            &mut pm,
            24.0,
            52.0,
            if demo == 4 {
                "Gray: raw sidechain. White: measured source trace. Main audio uses a separate tone."
            } else {
                "Gray: input. Amber: processed output. White: smoothed source trace."
            },
            14.0,
            color_text(),
        );
        editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    }
    editor.set_audio_frozen(true);
    if detector_demo {
        let r = geometry.layout.detector_setup;
        let point = (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5);
        for event in [
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position: baseview::Point::new(point.0 as f64, point.1 as f64),
                modifiers: keyboard_types::Modifiers::empty(),
            }),
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers: keyboard_types::Modifiers::empty(),
            }),
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                modifiers: keyboard_types::Modifiers::empty(),
            }),
        ] {
            editor.handle_event(
                &event,
                tract_effects::editor::EditorContext {
                    pointer_position: point,
                    effect: &mut track,
                    modulation: &mut config,
                    text_renderer: &mut text,
                    geometry,
                },
            );
        }
    }
    if detector_mode.as_deref() == Some("listen") {
        editor.handle_event(
            &baseview::Event::Keyboard(keyboard_types::KeyboardEvent {
                state: keyboard_types::KeyState::Down,
                key: keyboard_types::Key::Character("l".into()),
                ..keyboard_types::KeyboardEvent::default()
            }),
            tract_effects::editor::EditorContext {
                pointer_position: (0.0, 0.0),
                effect: &mut track,
                modulation: &mut config,
                text_renderer: &mut text,
                geometry,
            },
        );
    }
    if timing_demo {
        let r = geometry.layout.trigger;
        let point = (r.0 + r.2 * 0.5, r.1 + r.3 * 0.5);
        for event in [
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position: baseview::Point::new(point.0 as f64, point.1 as f64),
                modifiers: keyboard_types::Modifiers::empty(),
            }),
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers: keyboard_types::Modifiers::empty(),
            }),
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                modifiers: keyboard_types::Modifiers::empty(),
            }),
            baseview::Event::Keyboard(keyboard_types::KeyboardEvent {
                state: keyboard_types::KeyState::Down,
                key: keyboard_types::Key::End,
                ..keyboard_types::KeyboardEvent::default()
            }),
            baseview::Event::Keyboard(keyboard_types::KeyboardEvent {
                state: keyboard_types::KeyState::Down,
                key: keyboard_types::Key::Enter,
                ..keyboard_types::KeyboardEvent::default()
            }),
        ] {
            editor.handle_event(
                &event,
                tract_effects::editor::EditorContext {
                    pointer_position: point,
                    effect: &mut track,
                    modulation: &mut config,
                    text_renderer: &mut text,
                    geometry,
                },
            );
        }
    }
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    editor.draw_popup(&mut pm, &mut text, &track, geometry);
    pm.save_png(&path)?;
    println!("{path}");
    Ok(())
}
