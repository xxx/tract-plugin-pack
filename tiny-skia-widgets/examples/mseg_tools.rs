//! Offscreen preview: cargo run -p tiny-skia-widgets --example mseg_tools -- /tmp/mseg-tools.png

use tiny_skia_widgets::mseg::*;
use tiny_skia_widgets::{TextRenderer, color_bg, color_text, fill_pixmap_opaque};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/mseg-tools.png".to_owned());
    let mut pixmap = tiny_skia::Pixmap::new(1280, 760).ok_or("Pixmap allocation failed")?;
    fill_pixmap_opaque(&mut pixmap, color_bg());
    let mut text = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    text.draw_text(
        &mut pixmap,
        24.0,
        34.0,
        "Selection: stretch, center-scale, and skew",
        20.0,
        color_text(),
    );
    text.draw_text(
        &mut pixmap,
        24.0,
        403.0,
        "S-curve pen: drag a shape, click again to repeat",
        20.0,
        color_text(),
    );
    let first = (24.0, 52.0, 1232.0, 310.0);
    let second = (24.0, 421.0, 1232.0, 310.0);
    let mut data = MsegData {
        snap: false,
        ..MsegData::default()
    };
    factory_shape(FactoryShape::Sine).apply(&mut data, false)?;
    let mut selection = MsegEditState::new();
    let layout = mseg_layout(first, false, 1.0);
    let start = (phase_to_x(&layout, 0.18), value_to_y(&layout, 0.97));
    let end = (phase_to_x(&layout, 0.7), value_to_y(&layout, 0.03));
    selection.on_mouse_down(start.0, start.1, &mut data, first, 1.0, false);
    selection.on_mouse_move(end.0, end.1, &mut data, first, 1.0, false);
    selection.on_mouse_up(&mut data, first, 1.0);
    draw_mseg(
        &mut pixmap,
        &mut text,
        first,
        &data,
        &selection,
        1.0,
        tiny_skia::Color::from_rgba8(255, 200, 88, 255),
        None,
    );

    factory_shape(FactoryShape::FlatCenter).apply(&mut data, false)?;
    let mut pen = MsegEditState::new_curve_only();
    pen.set_show_polarity(false);
    pen.set_tool(MsegTool::SCurve);
    let layout = mseg_layout(second, true, 1.0);
    let start = (phase_to_x(&layout, 0.08), value_to_y(&layout, 0.15));
    let end = (phase_to_x(&layout, 0.28), value_to_y(&layout, 0.85));
    pen.on_mouse_down(start.0, start.1, &mut data, second, 1.0, false);
    pen.on_mouse_move(end.0, end.1, &mut data, second, 1.0, true);
    pen.on_mouse_up(&mut data, second, 1.0);
    for _ in 0..2 {
        pen.on_mouse_down(end.0, end.1, &mut data, second, 1.0, false);
        pen.on_mouse_up(&mut data, second, 1.0);
    }
    pen.on_mouse_move(
        phase_to_x(&layout, 0.76),
        start.1,
        &mut data,
        second,
        1.0,
        true,
    );
    draw_mseg(
        &mut pixmap,
        &mut text,
        second,
        &data,
        &pen,
        1.0,
        tiny_skia::Color::from_rgba8(195, 120, 255, 255),
        None,
    );
    pixmap.save_png(&output)?;
    println!("{output}");
    Ok(())
}
