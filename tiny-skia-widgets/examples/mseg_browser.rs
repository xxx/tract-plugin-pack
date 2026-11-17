//! Preview: cargo run -p tiny-skia-widgets --example mseg_browser -- /tmp/mseg-browser.png [full|static] [factory|favorites|project|user] [scale] [browse|save|rename|delete|last|assign|filter] [all|uncategorized|category]

use keyboard_types::{Key, KeyState, KeyboardEvent};
use tiny_skia_widgets::mseg::library::CurveLibrary;
use tiny_skia_widgets::mseg::*;
use tiny_skia_widgets::{TextRenderer, color_bg, color_text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .unwrap_or_else(|| "/tmp/mseg-browser.png".into());
    let curve_only = match args.next().as_deref() {
        None | Some("full") => false,
        Some("static") => true,
        _ => return Err("Editor must be full or static".into()),
    };
    let turns = match args.next().as_deref() {
        None | Some("factory") => 0,
        Some("favorites") => 1,
        Some("project") => 2,
        Some("user") => 3,
        _ => return Err("Page must be factory, favorites, project or user".into()),
    };
    let scale = args.next().map_or(Ok(1.0), |value| value.parse::<f32>())?;
    if !scale.is_finite() || !(0.5..=2.0).contains(&scale) {
        return Err("Scale must be between 0.5 and 2".into());
    }
    let mode = args.next().unwrap_or_else(|| "browse".into());
    let category = args.next().unwrap_or_else(|| "all".into());
    if !matches!(
        mode.as_str(),
        "browse" | "save" | "rename" | "delete" | "last" | "assign" | "filter"
    ) {
        return Err("Unknown browser mode".into());
    }
    if mode != "browse" && mode != "filter" && turns != 3 {
        return Err("Dialog/page modes require the User tab".into());
    }
    let root = std::env::temp_dir().join(format!(
        "tract-browser-preview-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&root)?;
    let library = CurveLibrary::at(root.clone());
    library.store_favorite(0, &factory_shape(FactoryShape::Duck))?;
    library.store_favorite(3, &factory_shape(FactoryShape::Square))?;
    let mut data = MsegData::default();
    data.nodes[0].shape = SegmentShape::SCurve;
    data.nodes[0].tension = -0.5;
    data.nodes[1].weight = PointWeight::Soft;
    library.store_favorite(5, &CurveShape::capture(&data))?;
    for (i, name) in [
        "Airy rise",
        "Bright pulse",
        "Curved descent",
        "Deep duck",
        "Early gate",
        "Falling stairs",
        "Gentle swell",
        "Hard chop",
        "Irregular motion",
        "Late dip",
        "Long fade",
        "Midrange bounce",
        "Narrow peak",
        "Open ramp",
        "Rounded wave",
        "Soft S-curve",
        "Wide envelope with a longer descriptive name",
    ]
    .iter()
    .enumerate()
    {
        let kind = FactoryShape::ALL[i % 8];
        library.save_user_preset_with_category(name, kind.category(), &factory_shape(kind))?;
    }
    let mut palette = WavePalette::default();
    palette.store(0, &CurveShape::capture(&data))?;
    palette.store(3, &factory_shape(FactoryShape::Duck))?;
    palette.store(5, &factory_shape(FactoryShape::Square))?;
    palette.store(7, &factory_shape(FactoryShape::RampDown))?;
    factory_shape(FactoryShape::Triangle).apply(&mut data, false)?;
    let bounds = (1280.0 * scale, 760.0 * scale);
    let mut pm = tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32)
        .ok_or("Pixmap allocation failed")?;
    let mut tr = TextRenderer::new(tiny_skia_widgets::DEFAULT_FONT);
    let mut state = if curve_only {
        MsegEditState::new_curve_only()
    } else {
        MsegEditState::new()
    };
    state.set_curve_library_directory(root.clone());
    state.bind_wave_palette(std::sync::Arc::new(std::sync::Mutex::new(palette)));
    state.set_overlay_bounds(bounds);
    let rect = (24.0 * scale, 70.0 * scale, 1232.0 * scale, 640.0 * scale);
    state.open_curve_browser(rect, scale);
    for _ in 0..turns {
        state.on_key(
            &KeyboardEvent {
                key: Key::Tab,
                state: KeyState::Down,
                ..KeyboardEvent::default()
            },
            &mut data,
            rect,
            scale,
        );
    }
    if category != "all" {
        if !matches!(turns, 0 | 3) {
            return Err("Category filters require Factory or User".into());
        }
        let index = if category == "uncategorized" && turns == 3 {
            1
        } else {
            let mut labels: Vec<_> = FactoryShape::ALL
                .iter()
                .map(|shape| shape.category())
                .collect();
            labels.sort_unstable();
            labels.dedup();
            labels
                .iter()
                .position(|label| label.eq_ignore_ascii_case(&category))
                .ok_or("Unknown category")?
                + if turns == 3 { 2 } else { 1 }
        };
        for key in std::iter::once(Key::Character("c".into()))
            .chain(std::iter::once(Key::Home))
            .chain(std::iter::repeat_n(Key::ArrowDown, index))
            .chain(std::iter::once(Key::Enter))
        {
            state.on_key(
                &KeyboardEvent {
                    key,
                    state: KeyState::Down,
                    ..KeyboardEvent::default()
                },
                &mut data,
                rect,
                scale,
            );
        }
    }
    let key = match mode.as_str() {
        "browse" => None,
        "save" | "assign" => Some(Key::Character("s".into())),
        "rename" => Some(Key::F2),
        "delete" => Some(Key::Delete),
        "last" => Some(Key::End),
        "filter" => Some(Key::Character("c".into())),
        _ => return Err("Mode must be browse, save, rename, delete or last".into()),
    };
    if let Some(key) = key {
        state.on_key(
            &KeyboardEvent {
                key,
                state: KeyState::Down,
                ..KeyboardEvent::default()
            },
            &mut data,
            rect,
            scale,
        );
    }
    if mode == "assign" {
        for key in [Key::Tab, Key::F4] {
            state.on_key(
                &KeyboardEvent {
                    key,
                    state: KeyState::Down,
                    ..KeyboardEvent::default()
                },
                &mut data,
                rect,
                scale,
            );
        }
    }
    pm.fill(color_bg());
    tr.draw_text(
        &mut pm,
        24.0 * scale,
        30.0 * scale,
        "Shared MSEG curve browser",
        20.0 * scale,
        color_text(),
    );
    tr.draw_text(
        &mut pm,
        24.0 * scale,
        52.0 * scale,
        "Factory, Favorites, Project waves and named User presets. Load or Fit recalls geometry.",
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
    std::fs::remove_dir_all(root)?;
    println!("{output}");
    Ok(())
}
