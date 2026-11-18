//! Preview: cargo run -p tract-effects --example mseg_midi_waves -- /tmp/waves.png [playing|setup|automation|automation-setup] [scale]
use baseview::{Event, MouseButton, MouseEvent, Point};
use keyboard_types::Modifiers;
use std::sync::{Arc, Mutex};
use tiny_skia_widgets::mseg::{
    FactoryShape, PlayMode, SyncMode, WavePalette, WavePaletteSnapshot, factory_shape,
};
use tiny_skia_widgets::{TextRenderer, color_bg, color_text};
use tract_effects::editor::{EditorContext, EditorGeometry, EffectEditor, view::effect_layout};
use tract_effects::effects::{
    Effect, EffectInstance, EffectKind, TrackEffect, default_params_for_kind,
};
use tract_effects::modulation::{MidiNoteOn, MidiWaveMapping, ModulationState, TrackModulation};
use tract_effects::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .unwrap_or_else(|| "/tmp/mseg-midi-waves.png".into());
    let (setup, automation) = match args.next().as_deref() {
        None | Some("playing") => (false, false),
        Some("setup") => (true, false),
        Some("automation") => (false, true),
        Some("automation-setup") => (true, true),
        _ => return Err("View must be playing, setup, automation, or automation-setup".into()),
    };
    let scale = args.next().map_or(Ok(1.0), |value| value.parse::<f32>())?;
    if !scale.is_finite() || !(0.5..=2.0).contains(&scale) {
        return Err("Scale must be between .5 and 2".into());
    }
    let bounds = (1140.0 * scale, 704.0 * scale);
    let geometry = EditorGeometry {
        layout: effect_layout((30.0, 48.0, 1100.0, 640.0), scale),
        scale,
        window_size: bounds,
    };
    let mut pm = tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32)
        .ok_or("Pixmap allocation failed")?;
    let mut text = TextRenderer::new(tiny_skia_widgets::DEFAULT_FONT);
    let mut track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        ..TrackEffect::default()
    };
    let mut config = TrackModulation {
        wave_switch: Some(MidiWaveMapping {
            envelope: 1,
            first_note: 36,
            channel: Some(2),
        }),
        ..TrackModulation::default()
    };
    for curve in &mut config.msegs {
        curve.sync_mode = SyncMode::Time;
        curve.time_seconds = 1.0;
        curve.play_mode = PlayMode::Cyclic;
    }
    config.smooth_ms[1] = 50.0;
    config.depths[0] = 0.4;
    let mut palette = WavePalette::default();
    palette.store(0, &factory_shape(FactoryShape::Sine))?;
    palette.store(1, &factory_shape(FactoryShape::Square))?;
    palette.store(2, &factory_shape(FactoryShape::Duck))?;
    let mut bank = WavePaletteSnapshot::default();
    bank.refresh(&palette);
    let mut effect = EffectInstance::new(track.kind);
    effect.set_sample_rate(48000.0);
    for (index, _) in effect.parameters().iter().enumerate() {
        effect.set_param(index, track.params[index]);
    }
    let mut state = ModulationState::default();
    let wave_values = if automation { [0, 2, 0, 0] } else { [0; 4] };
    state.set_wave_automation(wave_values);
    let telemetry = Arc::new(ModulationTelemetry::default());
    let mut trace = TraceClock::default();
    let mut editor = EffectEditor::default();
    editor.bind_wave_palette(Arc::new(Mutex::new(palette)));
    editor.set_wave_automation(wave_values);
    editor.connect_live(&telemetry, 0);
    editor.set_trace_enabled(true);
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    let input = [0.1; 960];
    for block in 0..145 {
        trace.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        state.prepare_wave_bank(&config, &bank);
        state.begin_block(&config, &input, &input, 48000.0);
        state.advance_segment(&config, &track, &mut effect, input.len(), 120.0, 48000.0);
        for sample in 0..input.len() {
            if block == 1 && sample == 240 {
                state.select_midi_wave(
                    &config,
                    &bank,
                    MidiNoteOn {
                        timing: sample,
                        channel: 2,
                        note: 38,
                    },
                );
            }
            state.refresh_pending_wave(&config, &track, &mut effect, input.len() - sample);
            state.process_smoothing_sample(&config, &mut effect);
            effect.process_sample(0.1, 0.1);
            state.next_mix(track.mix);
            if trace.tick() {
                trace.record(
                    &telemetry,
                    0,
                    &ModulationFrame::capture(track.kind, &config, &state),
                );
            }
        }
        trace.publish(
            &telemetry,
            0,
            &ModulationFrame::capture(track.kind, &config, &state),
        );
    }
    if setup {
        let strip =
            tiny_skia_widgets::mseg::mseg_layout(geometry.layout.mseg_pane, false, scale).strip;
        let button = tiny_skia_widgets::mseg::wave_midi_button_rect(strip, scale);
        let position = (button.0 + button.2 * 0.5, button.1 + button.3 * 0.5);
        for event in [
            Event::Mouse(MouseEvent::CursorMoved {
                position: Point::new(position.0 as f64, position.1 as f64),
                modifiers: Modifiers::empty(),
            }),
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers: Modifiers::empty(),
            }),
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                modifiers: Modifiers::empty(),
            }),
        ] {
            editor.handle_event(
                &event,
                EditorContext {
                    pointer_position: position,
                    effect: &mut track,
                    modulation: &mut config,
                    text_renderer: &mut text,
                    geometry,
                },
            );
        }
    }
    pm.fill(color_bg());
    if setup && !editor.wave_midi_panel_open() {
        return Err("Wave setup did not open".into());
    }
    text.draw_text(
        &mut pm,
        24.0 * scale,
        25.0 * scale,
        if automation {
            "Automated Project wave 2 — overrides MIDI wave 3"
        } else {
            "MIDI Project wave 3 — phase preserved; authored curve retained"
        },
        18.0 * scale,
        color_text(),
    );
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    editor.draw_popup(&mut pm, &mut text, &track, geometry);
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
