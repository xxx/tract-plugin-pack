//! Offscreen preview: cargo run -p tiny-skia-widgets --example mseg_loop_seams -- /tmp/mseg-loop-seams.png

use tiny_skia_widgets::mseg::*;
use tiny_skia_widgets::{TextRenderer, color_bg, color_text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/mseg-loop-seams.png".into());
    let mut pm = tiny_skia::Pixmap::new(1280, 1080).ok_or("Pixmap allocation failed")?;
    pm.fill(color_bg());
    let mut text = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    let mut original = MsegData {
        play_mode: PlayMode::Cyclic,
        snap: false,
        ..MsegData::default()
    };
    original.nodes[0].value = 0.2;
    original.nodes[1].value = 0.95;
    original.insert_node(0.22, 0.9).ok_or("Insert failed")?;
    original.insert_node(0.6, 0.5).ok_or("Insert failed")?;
    original.nodes[2].stepped = true;
    let mut matched = original;
    let mut state = MsegEditState::new();
    let _ = state.match_loop_end_to_start(&mut matched);
    let mut smooth = matched;
    let _ = state.smooth_loop_seam(&mut smooth);
    for (i, (label, data, color)) in [
        (
            "Original: stepped tail wraps with a level jump",
            original,
            tiny_skia::Color::from_rgba8(255, 200, 88, 255),
        ),
        (
            "Match end to start: endpoints match, but the stepped tail still jumps",
            matched,
            tiny_skia::Color::from_rgba8(195, 120, 255, 255),
        ),
        (
            "Smooth seam: native return with matched levels and a flat wrap",
            smooth,
            tiny_skia::Color::from_rgba8(102, 217, 160, 255),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let top = i as f32 * 354.0;
        text.draw_text(&mut pm, 24.0, top + 34.0, label, 20.0, color_text());
        let rect = (24.0, top + 52.0, 1232.0, 294.0);
        draw_mseg(&mut pm, &mut text, rect, &data, &state, 1.0, color, None);
        draw_mseg_dropdown(&mut pm, &mut text, &state, rect);
    }
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
