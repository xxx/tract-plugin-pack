//! Offscreen preview: cargo run -p tiny-skia-widgets --example mseg_precision -- /tmp/mseg-precision.png

use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};
use tiny_skia_widgets::mseg::*;
use tiny_skia_widgets::{TextRenderer, color_bg, color_text};

fn select(
    state: &mut MsegEditState,
    data: &mut MsegData,
    rect: (f32, f32, f32, f32),
    index: usize,
    add: bool,
) {
    let layout = state.layout(rect, 1.0);
    let node = data.nodes[index];
    let _ = state.on_mouse_down(
        phase_to_x(&layout, node.time),
        value_to_y(&layout, node.value),
        data,
        rect,
        1.0,
        add,
    );
    let _ = state.on_mouse_up(data, rect, 1.0);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/mseg-precision.png".into());
    let mut pm = tiny_skia::Pixmap::new(1280, 760).ok_or("Pixmap allocation failed")?;
    pm.fill(color_bg());
    let mut text = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    text.draw_text(
        &mut pm,
        24.0,
        34.0,
        "Point inspector: exact values in a zoomed view",
        20.0,
        color_text(),
    );
    text.draw_text(
        &mut pm,
        24.0,
        403.0,
        "Phrase paste: fit a saved shape inside the selected interval",
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
    factory_shape(FactoryShape::Sine).apply(&mut data, false)?;
    let mut state = MsegEditState::new();
    let point = 1;
    select(&mut state, &mut data, first, point, false);
    let layout = state.layout(first, 1.0);
    state.on_scroll(
        phase_to_x(&layout, 0.4),
        value_to_y(&layout, 0.5),
        5.0,
        false,
        &data,
        first,
        1.0,
    );
    state.open_point_inspector(&data);
    state.set_inspector_readout(&format!(
        "Amplitude: {:.2} dB",
        20.0 * data.nodes[point].value.log10()
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

    let root = std::env::temp_dir().join(format!("tract-precision-preview-{}", std::process::id()));
    let library = tiny_skia_widgets::mseg::library::CurveLibrary::at(root.clone());
    let mut source = MsegData::default();
    factory_shape(FactoryShape::Sine).apply(&mut source, false)?;
    library.copy(&CurveShape::capture(&source))?;
    let mut data = MsegData {
        snap: false,
        ..MsegData::default()
    };
    for phase in [0.25, 0.5, 0.75] {
        data.insert_node(phase, 0.5);
    }
    let mut state = MsegEditState::new_curve_only();
    state.set_curve_library_directory(root.clone());
    for (i, node) in [1, 2, 3].into_iter().enumerate() {
        select(&mut state, &mut data, second, node, i != 0);
    }
    let _ = state.on_key(
        &KeyboardEvent {
            key: Key::Character("v".into()),
            state: KeyState::Down,
            modifiers: Modifiers::CONTROL | Modifiers::ALT,
            ..KeyboardEvent::default()
        },
        &mut data,
        second,
        1.0,
    );
    draw_mseg(
        &mut pm,
        &mut text,
        second,
        &data,
        &state,
        1.0,
        tiny_skia::Color::from_rgba8(195, 120, 255, 255),
        None,
    );
    pm.save_png(&output)?;
    std::fs::remove_dir_all(root)?;
    println!("{output}");
    Ok(())
}
