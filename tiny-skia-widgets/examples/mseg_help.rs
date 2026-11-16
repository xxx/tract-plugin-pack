//! Preview: cargo run -p tiny-skia-widgets --example mseg_help -- /tmp/mseg-help.png [full|static] [points|select|shapes|view|hover] [scale]

use keyboard_types::{Key, KeyState, KeyboardEvent};
use tiny_skia_widgets::mseg::*;
use tiny_skia_widgets::{TextRenderer, color_bg, color_text};

fn key(key: Key) -> KeyboardEvent {
    KeyboardEvent {
        key,
        state: KeyState::Down,
        ..KeyboardEvent::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args.next().unwrap_or_else(|| "/tmp/mseg-help.png".into());
    let curve_only = match args.next().as_deref() {
        None | Some("full") => false,
        Some("static") => true,
        _ => return Err("Editor must be full or static".into()),
    };
    let page = args.next().unwrap_or_else(|| "points".into());
    let turns = match page.as_str() {
        "points" | "hover" => 0,
        "select" => 1,
        "shapes" => 2,
        "view" => 3,
        _ => return Err("Page must be points, select, shapes, view, or hover".into()),
    };
    let scale = args.next().map_or(Ok(1.0), |value| value.parse::<f32>())?;
    if !scale.is_finite() || !(0.5..=2.0).contains(&scale) {
        return Err("Scale must be between 0.5 and 2".into());
    }
    let bounds = (1280.0 * scale, 760.0 * scale);
    let mut pm = tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32)
        .ok_or("Pixmap allocation failed")?;
    let mut tr = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    let mut state = if curve_only {
        MsegEditState::new_curve_only()
    } else {
        MsegEditState::new()
    };
    state.set_help_bounds(bounds);
    let mut data = MsegData::default();
    factory_shape(FactoryShape::Triangle).apply(&mut data, false)?;
    let rect = (24.0 * scale, 70.0 * scale, 1232.0 * scale, 640.0 * scale);
    let layout = state.layout(rect, scale);
    if page == "hover" {
        let help = help_button_rect(layout.strip, scale);
        let _ = state.on_mouse_move(
            help.0 + help.2 * 0.5,
            help.1 + help.3 * 0.5,
            &mut data,
            rect,
            scale,
            false,
        );
    } else {
        state.on_key(&key(Key::F1), &mut data, rect, scale);
        for _ in 0..turns {
            state.on_key(&key(Key::ArrowRight), &mut data, rect, scale);
        }
    }
    pm.fill(color_bg());
    tr.draw_text(
        &mut pm,
        24.0 * scale,
        30.0 * scale,
        if curve_only {
            "Shared MSEG guide - static curves"
        } else {
            "Shared MSEG guide - modulation curves"
        },
        20.0 * scale,
        color_text(),
    );
    tr.draw_text(
        &mut pm,
        24.0 * scale,
        52.0 * scale,
        "Hover for hints. ? / F1 opens the guide; Escape closes it. The curve stays unchanged.",
        14.0 * scale,
        color_text(),
    );
    draw_mseg(
        &mut pm,
        &mut tr,
        rect,
        &data,
        &state,
        scale,
        tiny_skia::Color::from_rgba8(255, 200, 88, 255),
        None,
    );
    draw_mseg_dropdown(&mut pm, &mut tr, &state, rect);
    pm.save_png(&output)?;
    println!("{output}");
    Ok(())
}
