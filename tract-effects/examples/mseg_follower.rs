//! cargo run -p tract-effects --example mseg_follower -- /tmp/mseg-follower.png [playing|panel|audition|stopped] [scale]
use baseview::{Event, MouseButton, MouseEvent, Point};
use keyboard_types::Modifiers;
use std::sync::Arc;
use tiny_skia_widgets as widgets;
use tract_effects::editor::{AudioView, EditorContext, EditorGeometry, EffectEditor, view};
use tract_effects::effects::{
    Effect, EffectInstance, EffectKind, TrackEffect, default_params_for_kind,
};
use tract_effects::modulation::{HostPosition, ModulationState, TrackModulation, TriggerSource};
use tract_effects::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
use tract_effects::waveform::{WaveformClock, WaveformTiming};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .unwrap_or_else(|| "/tmp/mseg-follower.png".into());
    let mode = args.next().unwrap_or_else(|| "playing".into());
    if !["playing", "panel", "audition", "stopped"].contains(&mode.as_str()) {
        return Err("Mode must be playing, panel, audition or stopped".into());
    }
    let scale = args.next().map_or(Ok(1.0), |value| value.parse::<f32>())?;
    if !scale.is_finite() || !(0.5..=2.0).contains(&scale) {
        return Err("Scale must be .5 to 2".into());
    }
    let bounds = (1140.0 * scale, 704.0 * scale);
    let geometry = EditorGeometry {
        layout: view::effect_layout((30.0, 48.0, 1100.0, 640.0), scale),
        scale,
        window_size: bounds,
    };
    let mut pm = tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32)
        .ok_or("Pixmap allocation failed")?;
    let mut text = widgets::TextRenderer::new(widgets::DEFAULT_FONT);
    let mut track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        ..TrackEffect::default()
    };
    let mut config = TrackModulation {
        trigger: TriggerSource::Free,
        ..TrackModulation::default()
    };
    config.smooth_ms[1] = 20.0;
    config.follower.routes[1].enabled = true;
    config.follower.routes[1].mode = tract_effects::modulation::FollowerMode::Multiply;
    config.follower.routes[1].amount = 1.5;
    config.follower.attack_ms = 8.0;
    config.follower.hold_ms = 30.0;
    config.follower.release_ms = 150.0;
    config.follower.gain_db = 6.0;
    widgets::mseg::factory_shape(widgets::mseg::FactoryShape::Sine)
        .apply(&mut config.msegs[1], false)?;
    let telemetry = Arc::new(ModulationTelemetry::default());
    let mut editor = EffectEditor::default();
    editor.connect_live(&telemetry, 0);
    editor.set_trace_enabled(true);
    editor.set_audio_view(AudioView::Input);
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    let mut state = ModulationState::default();
    let mut effect = EffectInstance::new(track.kind);
    effect.set_sample_rate(48000.0);
    for (i, value) in track.params.iter().enumerate() {
        if i < effect.parameters().len() {
            effect.set_param(i, *value);
        }
    }
    let mut trace = TraceClock::default();
    let mut waveform = WaveformClock::default();
    let mut frame = HostPosition::default();
    for block in 0..146 {
        let stopped = mode == "stopped" && block == 145;
        let elapsed = block as f64 * 0.02;
        frame = HostPosition {
            playing: !stopped,
            beats: Some(8.0 + elapsed * 2.0),
            seconds: Some(4.0 + elapsed),
            bpm: 120.0,
            ..HostPosition::default()
        };
        let input: [f32; 960] = std::array::from_fn(|i| {
            let t = (block * 960 + i) as f32 / 48000.0;
            0.6 * (std::f32::consts::TAU * 70.0 * t).sin()
                * (0.65 + 0.35 * (std::f32::consts::TAU * 2.0 * t).sin())
        });
        trace.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        state.set_host_position(frame);
        state.begin_block(&config, &input, &input, 48000.0);
        waveform.begin_block(&telemetry.waveform);
        let slot = waveform.request().slot;
        if state.waveform_running(&config, slot) {
            waveform.prepare_segment_with_curve(
                &telemetry.waveform,
                0,
                WaveformTiming {
                    config: &config,
                    kind: track.kind,
                    phase: state.waveform_phase(&config, slot),
                    trigger_epoch: state.capture_epoch(&config, slot),
                    bpm: 120.0,
                    sample_rate: 48000.0,
                },
                state.effective_mseg(&config, slot),
            );
        } else {
            waveform.pause();
        }
        state.advance_segment(&config, &track, &mut effect, input.len(), 120.0, 48000.0);
        for value in input {
            state.process_modulation_sample(&config, &mut effect, [value; 2], [0.0; 2]);
            let wet = effect.process_sample(value, value);
            state.next_mix(track.mix);
            waveform.sync_position(
                &telemetry.waveform,
                state.capture_epoch(&config, slot),
                state.precise_phase(slot),
            );
            waveform.record_sample(
                &telemetry.waveform,
                [value; 2],
                [wet.0, wet.1],
                state.precise_phase(slot),
            );
            if trace.tick() {
                trace.record(
                    &telemetry,
                    0,
                    &ModulationFrame::capture(track.kind, &config, &state),
                );
            }
        }
        waveform.finish_block(&telemetry.waveform);
        trace.publish(
            &telemetry,
            0,
            &ModulationFrame::capture(track.kind, &config, &state),
        );
    }
    if mode == "panel" || mode == "audition" {
        let strip = widgets::mseg::mseg_layout(geometry.layout.mseg_pane, false, scale).strip;
        let rect = widgets::mseg::follower_button_rect(strip, scale);
        let at = (rect.0 + rect.2 * 0.5, rect.1 + rect.3 * 0.5);
        for event in [
            Event::Mouse(MouseEvent::CursorMoved {
                position: Point::new(at.0 as f64, at.1 as f64),
                modifiers: Modifiers::empty(),
            }),
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers: Modifiers::empty(),
            }),
        ] {
            editor.handle_event(
                &event,
                EditorContext {
                    pointer_position: at,
                    effect: &mut track,
                    modulation: &mut config,
                    text_renderer: &mut text,
                    geometry,
                },
            );
        }
    }
    if mode == "audition" {
        editor.handle_event(
            &Event::Keyboard(keyboard_types::KeyboardEvent {
                state: keyboard_types::KeyState::Down,
                key: keyboard_types::Key::Character("l".into()),
                ..keyboard_types::KeyboardEvent::default()
            }),
            EditorContext {
                pointer_position: (0.0, 0.0),
                effect: &mut track,
                modulation: &mut config,
                text_renderer: &mut text,
                geometry,
            },
        );
    }
    pm.fill(widgets::color_bg());
    text.draw_text(
        &mut pm,
        24.0 * scale,
        25.0 * scale,
        &format!(
            "Envelope follower: {:.2} beats; {}",
            frame.beats.unwrap_or(0.0),
            if frame.playing { "120 BPM" } else { "stopped" }
        ),
        18.0 * scale,
        widgets::color_text(),
    );
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    editor.draw_popup(&mut pm, &mut text, &track, geometry);
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
