use super::*;
use crate::mseg::render::{phase_to_x, value_to_y};
use crate::mseg::{HoldMode, MsegTool, PointWeight, Polarity, SegmentShape, SyncMode};
use std::sync::atomic::{AtomicU64, Ordering};

const RECT: Rect = (20.0, 70.0, 1000.0, 420.0);
static NEXT: AtomicU64 = AtomicU64::new(0);

fn state(palette: Arc<Mutex<WavePalette>>) -> MsegEditState {
    let mut state = MsegEditState::new();
    state.set_curve_library_directory(std::env::temp_dir().join(format!(
        "tract-project-waves-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    state.set_overlay_bounds((1040.0, 720.0));
    state.bind_wave_palette(palette);
    state
}

fn key(key: Key) -> KeyboardEvent {
    KeyboardEvent {
        key,
        state: KeyState::Down,
        ..KeyboardEvent::default()
    }
}

fn project(state: &mut MsegEditState, data: &mut MsegData) {
    assert!(state.open_curve_browser(RECT, 1.0));
    state.on_key(&key(Key::Tab), data, RECT, 1.0);
    state.on_key(&key(Key::Tab), data, RECT, 1.0);
    assert!(state.browser.page == Page::Project);
    assert_eq!(state.browser_layout(RECT, 1.0).unwrap().tabs.len(), 3);
}

#[test]
fn storing_and_clearing_project_waves_preserve_the_curve_selection_view_and_pen_repeat() {
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    let mut state = state(palette.clone());
    let mut data = MsegData {
        snap: false,
        ..MsegData::default()
    };
    state.set_tool(MsegTool::Line);
    let layout = state.layout(RECT, 1.0);
    let point = (phase_to_x(&layout, 0.1), value_to_y(&layout, 0.2));
    state.on_mouse_move(point.0, point.1, &mut data, RECT, 1.0, false);
    state.on_mouse_down(point.0, point.1, &mut data, RECT, 1.0, false);
    state.on_mouse_up(&mut data, RECT, 1.0);
    state.selection = 3;
    let before = data;
    let preview = state.pen_preview().to_vec();
    let viewport = state.viewport();
    project(&mut state, &mut data);
    for value in ["s", "s"] {
        assert_eq!(
            state.on_key(&key(Key::Character(value.into())), &mut data, RECT, 1.0),
            (true, None)
        );
        assert_eq!(
            palette.lock().unwrap().get(0),
            Some(&CurveShape::capture(&before))
        );
        assert_eq!(data, before);
    }
    for _ in 0..2 {
        assert_eq!(
            state.on_key(&key(Key::Delete), &mut data, RECT, 1.0),
            (true, None)
        );
        assert!(palette.lock().unwrap().get(0).is_none());
    }
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert_eq!(data, before);
    assert_eq!(state.selection_count(), 2);
    assert_eq!(state.viewport(), viewport);
    assert_eq!(state.tool(), MsegTool::Line);
    assert_eq!(state.pen_preview(), preview);
    assert_eq!(
        state.on_mouse_down(point.0, point.1, &mut data, RECT, 1.0, false),
        Some(MsegEdit::Changed)
    );
    let end = preview.last().unwrap();
    assert!(
        data.active().iter().any(
            |node| (node.time - end.time).abs() < 1e-5 && (node.value - end.value).abs() < 1e-5
        )
    );
}

#[test]
fn project_slots_share_between_curves_and_recall_only_geometry_into_the_destination() {
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    let mut source = state(palette.clone());
    let mut destination = state(palette.clone());
    let mut data = MsegData::default();
    data.nodes[0].shape = SegmentShape::SCurve;
    data.nodes[0].tension = -0.4;
    data.nodes[1].weight = PointWeight::Soft;
    let original = data;
    project(&mut source, &mut data);
    source.on_key(&key(Key::End), &mut data, RECT, 1.0);
    assert_eq!(
        source.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0),
        (true, None)
    );
    source.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert_eq!(data, original);
    let mut target = MsegData {
        beats: 7.5,
        time_seconds: 2.3,
        sync_mode: SyncMode::Beat,
        snap: false,
        time_divisions: 12,
        value_steps: 5,
        polarity: Polarity::Bipolar,
        hold: HoldMode::Sustain(0),
        ..MsegData::default()
    };
    let mut expected = target;
    expected.nodes[..original.node_count].copy_from_slice(original.active());
    expected.node_count = original.node_count;
    expected.hold = HoldMode::None;
    project(&mut destination, &mut target);
    destination.on_key(&key(Key::End), &mut target, RECT, 1.0);
    assert_eq!(
        destination.on_key(&key(Key::Enter), &mut target, RECT, 1.0),
        (true, Some(MsegEdit::Changed))
    );
    assert_eq!(target, expected);
    assert!(!destination.has_open_menu());
    assert_eq!(
        palette.lock().unwrap().get(7),
        Some(&CurveShape::capture(&original))
    );
    project(&mut destination, &mut target);
    destination.on_key(&key(Key::End), &mut target, RECT, 1.0);
    assert_eq!(
        destination.on_key(&key(Key::Enter), &mut target, RECT, 1.0),
        (true, None)
    );
}

#[test]
fn project_fit_double_click_and_reverse_tab_navigation_use_the_existing_browser_contract() {
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    palette
        .lock()
        .unwrap()
        .store(7, &factory_shape(FactoryShape::Triangle))
        .unwrap();
    let mut state = state(palette);
    let mut data = MsegData::default();
    data.nodes[0].value = 0.3;
    data.nodes[1].value = 0.7;
    assert!(state.open_curve_browser(RECT, 1.0));
    let backwards = KeyboardEvent {
        modifiers: Modifiers::SHIFT,
        ..key(Key::Tab)
    };
    state.on_key(&backwards, &mut data, RECT, 1.0);
    assert!(state.browser.page == Page::Project);
    state.on_key(&key(Key::End), &mut data, RECT, 1.0);
    let fit = KeyboardEvent {
        modifiers: Modifiers::SHIFT,
        ..key(Key::Enter)
    };
    assert_eq!(
        state.on_key(&fit, &mut data, RECT, 1.0),
        (true, Some(MsegEdit::Changed))
    );
    assert_eq!(
        data.active()
            .iter()
            .map(|node| node.value)
            .collect::<Vec<_>>(),
        vec![0.3, 0.7, 0.3]
    );
    project(&mut state, &mut data);
    let card = state.browser_layout(RECT, 1.0).unwrap().cards[7];
    let at = (card.0 + card.2 * 0.5, card.1 + card.3 * 0.5);
    assert_eq!(
        state.on_mouse_down(at.0, at.1, &mut data, RECT, 1.0, false),
        None
    );
    state.on_mouse_up(&mut data, RECT, 1.0);
    assert_eq!(
        state.on_mouse_down(at.0, at.1, &mut data, RECT, 1.0, false),
        Some(MsegEdit::Changed)
    );
    assert_eq!(data.active(), factory_shape(FactoryShape::Triangle).nodes());
}

#[test]
fn externally_replaced_project_slots_refresh_before_loading_storing_or_clearing() {
    for event in [
        key(Key::Enter),
        KeyboardEvent {
            modifiers: Modifiers::SHIFT,
            ..key(Key::Enter)
        },
        key(Key::Character("s".into())),
        key(Key::Delete),
    ] {
        let palette = Arc::new(Mutex::new(WavePalette::default()));
        palette
            .lock()
            .unwrap()
            .store(0, &factory_shape(FactoryShape::Triangle))
            .unwrap();
        let mut state = state(palette.clone());
        let mut data = MsegData::default();
        let before = data;
        project(&mut state, &mut data);
        palette
            .lock()
            .unwrap()
            .store(0, &factory_shape(FactoryShape::Duck))
            .unwrap();
        palette
            .lock()
            .unwrap()
            .store(5, &factory_shape(FactoryShape::Square))
            .unwrap();
        let external = palette.lock().unwrap().clone();
        assert_eq!(state.on_key(&event, &mut data, RECT, 1.0), (true, None));
        assert!(state.browser.message.contains("changed"));
        assert_eq!(data, before);
        assert_eq!(*palette.lock().unwrap(), external);
        let edit = state.on_key(&event, &mut data, RECT, 1.0).1;
        if event.key == Key::Enter {
            assert_eq!(edit, Some(MsegEdit::Changed));
            assert_eq!(data.active(), factory_shape(FactoryShape::Duck).nodes());
        } else {
            assert_eq!(edit, None);
            assert_eq!(data, before);
            if event.key == Key::Delete {
                assert!(palette.lock().unwrap().get(0).is_none());
            } else {
                assert_eq!(
                    palette.lock().unwrap().get(0),
                    Some(&CurveShape::capture(&before))
                );
            }
        }
        assert_eq!(
            palette.lock().unwrap().get(5),
            Some(&factory_shape(FactoryShape::Square))
        );
    }
}

#[test]
fn entering_or_refreshing_project_reads_current_bank_and_rebinding_closes_the_old_browser() {
    let first = Arc::new(Mutex::new(WavePalette::default()));
    let second = Arc::new(Mutex::new(WavePalette::default()));
    second
        .lock()
        .unwrap()
        .store(0, &factory_shape(FactoryShape::Duck))
        .unwrap();
    let mut state = state(first.clone());
    let mut data = MsegData::default();
    let before = data;
    project(&mut state, &mut data);
    first
        .lock()
        .unwrap()
        .store(0, &factory_shape(FactoryShape::Triangle))
        .unwrap();
    state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
    assert_eq!(
        state.browser_shape(),
        Some(&factory_shape(FactoryShape::Triangle))
    );
    state.bind_wave_palette(first.clone());
    assert!(state.curve_browser_open());
    state.bind_wave_palette(second.clone());
    assert!(!state.curve_browser_open());
    project(&mut state, &mut data);
    assert_eq!(
        state.browser_shape(),
        Some(&factory_shape(FactoryShape::Duck))
    );
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    assert!(state.browser.page == Page::Factory);
    second
        .lock()
        .unwrap()
        .store(0, &factory_shape(FactoryShape::Sine))
        .unwrap();
    state.on_key(
        &KeyboardEvent {
            modifiers: Modifiers::SHIFT,
            ..key(Key::Tab)
        },
        &mut data,
        RECT,
        1.0,
    );
    assert_eq!(
        state.browser_shape(),
        Some(&factory_shape(FactoryShape::Sine))
    );
    assert_eq!(data, before);
    assert_eq!(
        first.lock().unwrap().get(0),
        Some(&factory_shape(FactoryShape::Triangle))
    );
}

#[test]
fn poisoned_project_storage_reports_an_error_and_does_not_prevent_factory_loading() {
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    let other = palette.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = other.lock().unwrap();
            panic!("poison project bank");
        })
        .join()
        .is_err()
    );
    let mut state = state(palette);
    let mut data = MsegData::default();
    let before = data;
    project(&mut state, &mut data);
    assert_eq!(
        state.browser.project_error,
        Some("Project waves unavailable")
    );
    for event in [
        Key::Enter,
        Key::Character("s".into()),
        Key::Delete,
        Key::Character("r".into()),
    ] {
        assert_eq!(
            state.on_key(&key(event), &mut data, RECT, 1.0),
            (true, None)
        );
        assert_eq!(data, before);
    }
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    state.on_key(&key(Key::End), &mut data, RECT, 1.0);
    assert_eq!(
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
        (true, Some(MsegEdit::Changed))
    );
}

#[test]
fn project_thumbnails_and_three_tabs_fit_supported_native_bounds() {
    for (scale, bounds) in [
        (0.5, (570.0, 352.0)),
        (1.0, (1140.0, 704.0)),
        (2.0, (2280.0, 1408.0)),
        (1.0, (300.0, 260.0)),
    ] {
        let palette = Arc::new(Mutex::new(WavePalette::default()));
        for (slot, kind) in FactoryShape::ALL.into_iter().enumerate() {
            palette
                .lock()
                .unwrap()
                .store(slot, &factory_shape(kind))
                .unwrap();
        }
        let mut state = state(palette.clone());
        state.set_overlay_bounds(bounds);
        let rect = (
            10.0 * scale,
            bounds.1 * 0.65,
            bounds.0 - 20.0 * scale,
            bounds.1 * 0.3,
        );
        let mut data = MsegData::default();
        assert!(state.open_curve_browser(rect, scale));
        for _ in 0..2 {
            state.on_key(&key(Key::Tab), &mut data, rect, scale);
        }
        let layout = state.browser_layout(rect, scale).unwrap();
        assert_eq!(layout.tabs.len(), 3);
        let mut pm = Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
        state.draw_curve_browser(
            &mut pm,
            &mut TextRenderer::new(crate::DEFAULT_FONT),
            rect,
            scale,
        );
        for card in layout.cards {
            let mut ink = 0;
            for y in card.1.ceil() as u32 + 3
                ..(card.1 + card.3 - (12.0 * scale).max(10.0) - 6.0).floor() as u32
            {
                for x in card.0.ceil() as u32 + 3..(card.0 + card.2).floor() as u32 - 3 {
                    let pixel = pm.pixel(x, y).unwrap();
                    if pixel.blue().saturating_sub(pixel.red()) > 50 {
                        ink += 1;
                    }
                }
            }
            assert!(ink > 10, "No project preview at {scale}x");
        }
        for tab in layout.tabs {
            assert!(
                tab.0 >= 0.0
                    && tab.1 >= 0.0
                    && tab.0 + tab.2 <= bounds.0
                    && tab.1 + tab.3 <= bounds.1
            );
        }
        assert_eq!(data, MsegData::default());
    }
}

#[test]
fn narrow_project_tabs_keep_caption_pixels_inside_each_button() {
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    let mut state = state(palette);
    state.set_overlay_bounds((124.0, 300.0));
    let rect = (0.0, 0.0, 124.0, 159.0);
    let mut data = MsegData::default();
    assert!(state.open_curve_browser(rect, 0.5));
    let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
    for _ in 0..3 {
        let layout = state.browser_layout(rect, 0.5).unwrap();
        let mut pm = Pixmap::new(124, 300).unwrap();
        state.draw_curve_browser(&mut pm, &mut tr, rect, 0.5);
        let background = color_bg().to_color_u8();
        for pair in layout.tabs.windows(2) {
            for x in (pair[0].0 + pair[0].2).ceil() as u32 + 1..pair[1].0.floor() as u32 {
                for y in pair[0].1.ceil() as u32 + 2..(pair[0].1 + pair[0].3).floor() as u32 - 2 {
                    let pixel = pm.pixel(x, y).unwrap();
                    assert_eq!(
                        (pixel.red(), pixel.green(), pixel.blue()),
                        (background.red(), background.green(), background.blue()),
                        "caption spilled into the tab gap at {x},{y}"
                    );
                }
            }
        }
        state.on_key(&key(Key::Tab), &mut data, rect, 0.5);
    }
    assert_eq!(data, MsegData::default());
}
