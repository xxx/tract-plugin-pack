//! Offscreen preview: cargo run -p tiny-skia-widgets --example mseg_shapes -- /tmp/mseg-shapes.png

use tiny_skia_widgets::mseg::*;
use tiny_skia_widgets::{TextRenderer, color_bg, color_text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/mseg-shapes.png".into());
    let mut pm = tiny_skia::Pixmap::new(1280, 760).ok_or("Pixmap allocation failed")?;
    pm.fill(color_bg());
    let mut text = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    text.draw_text(
        &mut pm,
        24.0,
        34.0,
        "Native S-curves: two endpoints per segment",
        20.0,
        color_text(),
    );
    text.draw_text(
        &mut pm,
        24.0,
        403.0,
        "Point weights: Hard (amber), Medium (purple), Soft (teal)",
        20.0,
        color_text(),
    );
    let first = (24.0, 52.0, 1232.0, 310.0);
    let second = (24.0, 421.0, 1232.0, 310.0);
    let mut data = MsegData {
        snap: false,
        sync_mode: SyncMode::Beat,
        beats: 4.0,
        ..MsegData::default()
    };
    factory_shape(FactoryShape::FlatCenter).apply(&mut data, false)?;
    let mut state = MsegEditState::new();
    state.set_tool(MsegTool::SCurve);
    let layout = state.layout(first, 1.0);
    let start = (phase_to_x(&layout, 0.125), value_to_y(&layout, 0.25));
    let end = (phase_to_x(&layout, 0.3125), value_to_y(&layout, 0.75));
    let _ = state.on_mouse_down(start.0, start.1, &mut data, first, 1.0, false);
    let _ = state.on_mouse_move(end.0, end.1, &mut data, first, 1.0, true);
    let _ = state.on_mouse_up(&mut data, first, 1.0);
    for _ in 0..2 {
        let _ = state.on_mouse_down(end.0, end.1, &mut data, first, 1.0, false);
        let _ = state.on_mouse_up(&mut data, first, 1.0);
    }
    state.set_tool(MsegTool::Pointer);
    state.clear_selection();
    let point = data.nodes[1];
    let _ = state.on_mouse_down(
        phase_to_x(&layout, point.time),
        value_to_y(&layout, point.value),
        &mut data,
        first,
        1.0,
        false,
    );
    let _ = state.on_mouse_up(&mut data, first, 1.0);
    state.open_point_inspector(&data);
    state.set_inspector_readout(&format!(
        "{} points, including repeat seams",
        data.node_count
    ));
    draw_mseg(
        &mut pm,
        &mut text,
        first,
        &data,
        &state,
        1.0,
        tiny_skia::Color::from_rgba8(255, 200, 88, 255),
        None,
    );
    draw_mseg_dropdown(&mut pm, &mut text, &state, first);

    let mut soft = MsegData {
        snap: false,
        ..MsegData::default()
    };
    factory_shape(FactoryShape::Triangle).apply(&mut soft, false)?;
    for node in &mut soft.nodes[..soft.node_count] {
        node.weight = PointWeight::Soft;
    }
    let mut hard = soft;
    for node in &mut hard.nodes[..hard.node_count] {
        node.weight = PointWeight::Hard;
    }
    let mut medium = soft;
    for node in &mut medium.nodes[..medium.node_count] {
        node.weight = PointWeight::Medium;
    }
    let mut state = MsegEditState::new_curve_only();
    state.set_show_polarity(false);
    draw_mseg(
        &mut pm,
        &mut text,
        second,
        &soft,
        &state,
        1.0,
        tiny_skia::Color::from_rgba8(102, 217, 160, 255),
        None,
    );
    draw_mseg_ghost(&mut pm, second, &hard, &state, 1.0, 0xffc858a0);
    draw_mseg_ghost(&mut pm, second, &medium, &state, 1.0, 0xc378ffa0);
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
