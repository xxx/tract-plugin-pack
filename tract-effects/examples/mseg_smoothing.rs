//! Measured preview: cargo run -p tract-effects --example mseg_smoothing -- /tmp/mseg-smoothing.png

use std::sync::Arc;
use tiny_skia_widgets::mseg::{FactoryShape, factory_shape};
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode, TextRenderer, color_bg, color_text};
use tract_effects::editor::{EditorGeometry, EffectEditor, view::effect_layout};
use tract_effects::effects::{
    Effect, EffectInstance, EffectKind, TrackEffect, default_params_for_kind,
};
use tract_effects::modulation::{ModulationState, TrackModulation, TriggerSource};
use tract_effects::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/mseg-smoothing.png".into());
    let mut pm = tiny_skia::Pixmap::new(1336, 730).ok_or("Pixmap allocation failed")?;
    let mut text = TextRenderer::new(include_bytes!(
        "../../tiny-skia-widgets/test_data/DejaVuSans.ttf"
    ));
    let track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        ..TrackEffect::default()
    };
    let mut config = TrackModulation {
        trigger: TriggerSource::Free,
        ..TrackModulation::default()
    };
    config.msegs[1] = MsegData {
        play_mode: PlayMode::Cyclic,
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        ..MsegData::default()
    };
    factory_shape(FactoryShape::Square).apply(&mut config.msegs[1], false)?;
    config.smooth_ms[1] = 75.0;
    config.depths[0] = 0.7;
    let mut effect = EffectInstance::new(track.kind);
    effect.set_sample_rate(48000.0);
    for (index, _) in effect.parameters().iter().enumerate() {
        effect.set_param(index, track.params[index]);
    }
    let mut state = ModulationState::default();
    let telemetry = Arc::new(ModulationTelemetry::default());
    let mut clock = TraceClock::default();
    let mut editor = EffectEditor::default();
    editor.connect_live(&telemetry, 0);
    let geometry = EditorGeometry {
        layout: effect_layout((16.0, 60.0, 1304.0, 640.0), 1.0),
        scale: 1.0,
        window_size: (1336.0, 730.0),
    };
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    let input = [0.1; 960];
    for _ in 0..145 {
        clock.begin_block(&telemetry, 48000.0);
        state.begin_block(&config, &input, &input, 48000.0);
        state.advance_segment(&config, &track, &mut effect, input.len(), 120.0, 48000.0);
        for _ in &input {
            state.process_smoothing_sample(&config, &mut effect);
            let _ = effect.process_sample(0.1, 0.1);
            state.next_mix(track.mix);
            if clock.tick() {
                clock.record(
                    &telemetry,
                    0,
                    &ModulationFrame::capture(track.kind, &config, &state),
                );
            }
        }
        clock.publish(
            &telemetry,
            0,
            &ModulationFrame::capture(track.kind, &config, &state),
        );
        pm.fill(color_bg());
        text.draw_text(
            &mut pm,
            24.0,
            30.0,
            "75 ms source smoothing — measured audio output",
            20.0,
            color_text(),
        );
        text.draw_text(
            &mut pm,
            24.0,
            52.0,
            "White: DSP source trace before Depth. Colored dial arc: delivered target value.",
            14.0,
            color_text(),
        );
        editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    }
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
