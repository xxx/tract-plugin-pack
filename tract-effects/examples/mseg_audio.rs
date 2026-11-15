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
    ModulationState, ModulationTarget, TrackModulation, TriggerSource,
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
    let caption = format!("{caption} — display gain {gain_label}");
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
        trigger: TriggerSource::Free,
        targets: [Some(ModulationTarget::Mix), None, None],
        depths: [1.0, 0.0, 0.0],
        ..TrackModulation::default()
    };
    config.msegs[1] = MsegData {
        play_mode: PlayMode::Cyclic,
        sync_mode: SyncMode::Time,
        time_seconds: 1.0,
        ..MsegData::default()
    };
    factory_shape(FactoryShape::Square).apply(&mut config.msegs[1], false)?;
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
    editor.set_audio_view(AudioView::Both);
    editor.set_audio_display(display);
    editor.set_audio_gain(gain);
    let geometry = EditorGeometry {
        layout: effect_layout((16.0, 60.0, 1304.0, 640.0), 1.0),
        scale: 1.0,
        window_size: (1336.0, 730.0),
    };
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    let mut input = [0.0; 960];
    let block_samples = input.len();
    for block in 0..145 {
        for (index, sample) in input.iter_mut().enumerate() {
            let time = (block * block_samples + index) as f32 / 48000.0;
            let envelope = (-((time * 4.0).fract()) * 18.0).exp();
            *sample = envelope
                * ((std::f32::consts::TAU * 2200.0 * time).sin() * 0.65
                    + (std::f32::consts::TAU * 60.0 * time).sin() * 0.25);
        }
        trace.begin_block(&shared, 48000.0);
        audio.begin_block(&shared.waveform);
        state.begin_block(&config, &input, &input, 48000.0);
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
        for &sample in &input {
            state.process_smoothing_sample(&config, &mut effect);
            let wet = effect.process_sample(sample, -sample * 0.8);
            let mix = state.next_mix(track.mix);
            let output = [
                sample * (1.0 - mix) + wet.0 * mix * state.amplitude(),
                -sample * 0.8 * (1.0 - mix) + wet.1 * mix * state.amplitude(),
            ];
            audio.record_sample(
                &shared.waveform,
                [sample, -sample * 0.8],
                output,
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
            "Gray: input. Amber: processed output. White: smoothed source trace.",
            14.0,
            color_text(),
        );
        editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    }
    editor.set_audio_frozen(true);
    editor.draw(&mut pm, &mut text, &track, &config, [0.0; 4], geometry);
    pm.save_png(&path)?;
    println!("{path}");
    Ok(())
}
