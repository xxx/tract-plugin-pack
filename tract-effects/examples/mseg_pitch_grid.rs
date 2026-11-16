//! Preview: cargo run -p tract-effects --example mseg_pitch_grid -- /tmp/mseg-pitch-grid.png [frequency]

use baseview::{Event, MouseButton, MouseEvent, Point};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};
use tiny_skia_widgets as widgets;
use tract_effects::editor::{EditorContext, EditorGeometry, EffectEditor, view::effect_layout};
use tract_effects::effects::{EffectKind, TrackEffect, default_params_for_kind, parameter_specs};
use tract_effects::modulation::{ModulationTarget, TrackModulation, assignable_value};
use widgets::mseg::{MsegData, PlayMode, Polarity, SyncMode, ValueGrid};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .unwrap_or_else(|| "/tmp/mseg-pitch-grid.png".into());
    let frequency = match args.next().as_deref() {
        None => false,
        Some("frequency") => true,
        _ => return Err("Optional mode must be frequency".into()),
    };
    let kind = if frequency {
        EffectKind::Svf
    } else {
        EffectKind::PitchShift
    };
    let mut effect = TrackEffect {
        kind,
        params: default_params_for_kind(kind),
        ..TrackEffect::default()
    };
    effect.params[0] = if frequency { 443.0 } else { 0.3 };
    let mut config = TrackModulation::default();
    config.targets[0] = Some(ModulationTarget::EffectParameter(0));
    config.depths[0] = 0.5;
    config.msegs[1] = MsegData {
        play_mode: PlayMode::OneShot,
        sync_mode: SyncMode::Beat,
        beats: 4.0,
        polarity: Polarity::Bipolar,
        ..MsegData::default()
    };
    config.msegs[1].nodes[0].value = 0.49;
    config.msegs[1].nodes[1].value = 0.49;
    for (i, value) in [0.64, 0.76, 0.59, 0.47, 0.36, 0.25, 0.42]
        .into_iter()
        .enumerate()
    {
        config.msegs[1]
            .insert_node((i + 1) as f32 / 8.0, value)
            .ok_or("Insert failed")?;
    }
    for node in &mut config.msegs[1].nodes[..config.msegs[1].node_count - 1] {
        node.stepped = true;
    }
    let mut editor = EffectEditor::default();
    let mut tr = widgets::TextRenderer::new(widgets::DEFAULT_FONT);
    let mut pm = tiny_skia::Pixmap::new(1336, 750).ok_or("Pixmap allocation failed")?;
    let geometry = EditorGeometry {
        layout: effect_layout((16.0, 70.0, 1304.0, 640.0), 1.0),
        scale: 1.0,
        window_size: (1336.0, 750.0),
    };
    editor.draw(&mut pm, &mut tr, &effect, &config, [0.0; 4], geometry);
    let layout = widgets::mseg::mseg_layout(geometry.layout.mseg_pane, false, 1.0);
    let grid = widgets::mseg::grid_button_rect(layout.strip, 1.0);
    let mut pointer = (grid.0 + grid.2 * 0.5, grid.1 + grid.3 * 0.5);
    {
        let mut send = |event, position| {
            editor.handle_event(
                &event,
                EditorContext {
                    pointer_position: position,
                    effect: &mut effect,
                    modulation: &mut config,
                    text_renderer: &mut tr,
                    geometry,
                },
            )
        };
        send(
            Event::Mouse(MouseEvent::CursorMoved {
                position: Point::new(pointer.0 as f64, pointer.1 as f64),
                modifiers: Modifiers::empty(),
            }),
            pointer,
        );
        send(
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                modifiers: Modifiers::empty(),
            }),
            pointer,
        );
        send(
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                modifiers: Modifiers::empty(),
            }),
            pointer,
        );
        for key in [Key::End, Key::Enter] {
            send(
                Event::Keyboard(KeyboardEvent {
                    key,
                    state: KeyState::Down,
                    ..KeyboardEvent::default()
                }),
                pointer,
            );
        }
        send(
            Event::Keyboard(KeyboardEvent {
                key: Key::Character("a".into()),
                state: KeyState::Down,
                modifiers: Modifiers::CONTROL,
                ..KeyboardEvent::default()
            }),
            pointer,
        );
        pointer = (
            widgets::mseg::phase_to_x(&layout, 0.0),
            widgets::mseg::value_to_y(&layout, 0.49),
        );
        send(
            Event::Mouse(MouseEvent::CursorMoved {
                position: Point::new(pointer.0 as f64, pointer.1 as f64),
                modifiers: Modifiers::empty(),
            }),
            pointer,
        );
        send(
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Right,
                modifiers: Modifiers::empty(),
            }),
            pointer,
        );
        send(
            Event::Keyboard(KeyboardEvent {
                key: Key::Home,
                state: KeyState::Down,
                ..KeyboardEvent::default()
            }),
            pointer,
        );
        for _ in 0..11 {
            send(
                Event::Keyboard(KeyboardEvent {
                    key: Key::ArrowDown,
                    state: KeyState::Down,
                    ..KeyboardEvent::default()
                }),
                pointer,
            );
        }
        send(
            Event::Keyboard(KeyboardEvent {
                key: Key::Enter,
                state: KeyState::Down,
                ..KeyboardEvent::default()
            }),
            pointer,
        );
        send(
            Event::Keyboard(KeyboardEvent {
                key: Key::Escape,
                state: KeyState::Down,
                ..KeyboardEvent::default()
            }),
            pointer,
        );
    }
    assert_eq!(config.msegs[1].value_grid, ValueGrid::Semitones);
    let spec = parameter_specs(kind)[0];
    for node in config.msegs[1].active() {
        let value = assignable_value(node.value, effect.params[0], 0.5, spec, Polarity::Bipolar);
        let pitch = if frequency {
            69.0 + 12.0 * (value as f64 / 440.0).log2()
        } else {
            value as f64
        };
        assert!((pitch - pitch.round()).abs() < 2e-4);
    }
    pm.fill(widgets::color_bg());
    tr.draw_text(
        &mut pm,
        24.0,
        30.0,
        if frequency {
            "Frequency note grid: 443 Hz base, 50% depth, bipolar"
        } else {
            "Semitone grid: +0.30 st base, 50% depth, bipolar"
        },
        20.0,
        widgets::color_text(),
    );
    tr.draw_text(&mut pm, 24.0, 52.0, "Grid selection preserves the curve; Quantize points moves it to the mapped notes. Shift-drag bypasses snap.", 14.0, widgets::color_text());
    editor.draw(&mut pm, &mut tr, &effect, &config, [0.0; 4], geometry);
    editor.draw_popup(&mut pm, &mut tr, &effect, geometry);
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
