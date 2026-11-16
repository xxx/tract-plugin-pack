use super::*;
use crate::mseg::render::{phase_to_x, power_buttons, strip_buttons, value_to_y};
use crate::mseg::{FactoryShape, MsegTool, factory_shape};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};

const RECT: (f32, f32, f32, f32) = (20.0, 40.0, 1000.0, 420.0);
fn key(key: Key) -> KeyboardEvent {
    KeyboardEvent {
        key,
        state: KeyState::Down,
        ..KeyboardEvent::default()
    }
}

#[test]
fn playback_view_blocks_geometry_and_clipboard_commands_but_keeps_navigation_and_help() {
    let mut state = MsegEditState::new();
    let mut author = crate::mseg::MsegData::default();
    let before = author;
    let mut playing = author;
    factory_shape(FactoryShape::Triangle)
        .apply(&mut playing, false)
        .unwrap();
    state.set_read_only_curve(Some(playing));
    let layout = state.layout(RECT, 1.0);
    let point = (phase_to_x(&layout, 0.4), value_to_y(&layout, 0.4));
    assert_eq!(
        state.on_mouse_down(point.0, point.1, &mut author, RECT, 1.0, false),
        None
    );
    assert_eq!(
        state.on_mouse_move(
            point.0 + 40.0,
            point.1 - 30.0,
            &mut author,
            RECT,
            1.0,
            false
        ),
        None
    );
    assert_eq!(
        state.on_double_click(point.0, point.1, &mut author, RECT, 1.0),
        None
    );
    assert_eq!(
        state.on_right_click(point.0, point.1, &mut author, RECT, 1.0),
        None
    );
    let power = power_buttons(layout.strip, 1.0);
    let basic = strip_buttons(layout.strip, 1.0, true);
    for button in [
        power.tool,
        power.shapes,
        power.edit,
        power.point,
        basic.randomize,
    ] {
        assert_eq!(
            state.on_mouse_down(
                button.0 + 2.0,
                button.1 + 2.0,
                &mut author,
                RECT,
                1.0,
                false
            ),
            None
        );
        assert!(!state.has_open_menu() && !state.is_dragging());
    }
    for value in ["a", "c", "v"] {
        let event = KeyboardEvent {
            modifiers: Modifiers::CONTROL,
            ..key(Key::Character(value.into()))
        };
        assert_eq!(state.on_key(&event, &mut author, RECT, 1.0), (true, None));
    }
    assert_eq!(
        state.on_key(&key(Key::Delete), &mut author, RECT, 1.0),
        (true, None)
    );
    assert_eq!(author, before);
    assert!(state.on_scroll(point.0, point.1, 1.0, false, &author, RECT, 1.0));
    let viewport = state.viewport();
    assert!(state.on_pan_start(point.0, point.1, RECT, 1.0));
    state.on_mouse_move(point.0 + 20.0, point.1, &mut author, RECT, 1.0, false);
    state.on_key(&key(Key::Escape), &mut author, RECT, 1.0);
    assert_eq!(state.viewport(), viewport);
    assert_eq!(
        state.on_key(&key(Key::F1), &mut author, RECT, 1.0),
        (true, None)
    );
    assert!(state.help_open());
    state.set_read_only_curve(Some(playing));
    assert!(state.help_open());
    state.on_key(&key(Key::Escape), &mut author, RECT, 1.0);
    state.set_read_only_curve(None);
    state.set_tool(MsegTool::Pointer);
    let layout = state.layout(RECT, 1.0);
    let point = (phase_to_x(&layout, 0.4), value_to_y(&layout, 0.6));
    assert_eq!(
        state.on_double_click(point.0, point.1, &mut author, RECT, 1.0),
        Some(MsegEdit::Changed)
    );
}

#[test]
fn playback_curve_drawing_uses_the_override_and_omits_edit_handles() {
    let mut state = MsegEditState::new();
    let author = crate::mseg::MsegData::default();
    let mut display = author;
    factory_shape(FactoryShape::Triangle)
        .apply(&mut display, false)
        .unwrap();
    display.nodes[1].value = 0.8;
    state.set_read_only_curve(Some(display));
    let mut pm = tiny_skia::Pixmap::new(1040, 500).unwrap();
    let mut text = crate::TextRenderer::new(crate::DEFAULT_FONT);
    crate::mseg::draw_mseg(
        &mut pm,
        &mut text,
        RECT,
        &author,
        &state,
        1.0,
        crate::color_accent(),
        None,
    );
    let layout = state.layout(RECT, 1.0);
    let x = phase_to_x(&layout, 0.5) as u32;
    let y = value_to_y(&layout, 0.8) as u32;
    assert!((y.saturating_sub(1)..=y + 1).any(|y| {
        pm.pixel(x, y)
            .unwrap()
            .blue()
            .saturating_sub(pm.pixel(x, y).unwrap().red())
            > 50
    }));
    assert!(
        pm.pixel(x, y + 4)
            .unwrap()
            .blue()
            .saturating_sub(pm.pixel(x, y + 4).unwrap().red())
            < 50
    );
    assert_eq!(state.display_data(&author), &display);
    assert_eq!(author, crate::mseg::MsegData::default());
}
