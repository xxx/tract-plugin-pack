use super::search::BrowserSearch;
use super::*;
use crate::mseg::library::CurveLibrary;
use std::sync::atomic::{AtomicU64, Ordering};

const RECT: Rect = (20.0, 70.0, 1000.0, 420.0);
static NEXT: AtomicU64 = AtomicU64::new(0);

fn setup() -> (MsegEditState, CurveLibrary, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "tract-browser-search-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut state = MsegEditState::new();
    state.set_curve_library_directory(root.clone());
    state.set_overlay_bounds((1040.0, 720.0));
    (state, CurveLibrary::at(root.clone()), root)
}

fn key(key: Key) -> KeyboardEvent {
    KeyboardEvent {
        key,
        state: KeyState::Down,
        ..KeyboardEvent::default()
    }
}

fn user(state: &mut MsegEditState, data: &mut MsegData) {
    assert!(state.open_curve_browser(RECT, 1.0));
    for _ in 1..state.browser_pages().len() {
        state.on_key(&key(Key::Tab), data, RECT, 1.0);
    }
    assert!(state.browser.page == Page::User);
}

fn find(state: &mut MsegEditState, data: &mut MsegData) {
    assert_eq!(
        state.on_key(
            &KeyboardEvent {
                modifiers: Modifiers::CONTROL,
                ..key(Key::Character("f".into()))
            },
            data,
            RECT,
            1.0
        ),
        (true, None)
    );
    assert!(state.browser.search.visible && state.browser.search.focused);
}

#[test]
fn query_matches_every_literal_term_across_name_and_category_using_unicode_lowercase() {
    let mut search = BrowserSearch::default();
    for query in ["", "  ", "\t\n"] {
        search.set_text(query);
        assert!(search.matches("Anything", ""));
    }
    search.set_text("rHY du");
    assert!(search.matches("Duck", "Rhythmic"));
    assert!(!search.matches("Sine", "Modulation"));
    search.set_text("  BASS   pulse  ");
    assert!(search.matches("Slow pulse", "Bass drive"));
    assert!(!search.matches("Bass ramp", "Lead"));
    search.set_text("Å Δ");
    assert!(search.matches("å envelope", "Δ filter"));
    search.set_text("uncategorized");
    assert!(search.matches("Legacy", ""));
    assert!(!search.matches("Legacy", "Bass"));
    search.set_text("amps");
    assert!(!search.matches("ram", "ps"));
    search.set_text(".*");
    assert!(!search.matches("Duck", "Rhythmic"));
    search.set_text(&"曲".repeat(300));
    assert_eq!(search.text().chars().count(), 256);
}

#[test]
fn factory_search_combines_categories_and_enter_recalls_the_visible_match() {
    for fit in [false, true] {
        let (mut state, _, root) = setup();
        let mut data = MsegData::default();
        data.nodes[0].value = 0.3;
        data.nodes[1].value = 0.7;
        assert!(state.open_curve_browser(RECT, 1.0));
        let before = data;
        state.select_browser_category(CategoryFilter::Category("Rhythmic".into()));
        find(&mut state, &mut data);
        assert_eq!(
            state.on_key(&key(Key::Character("RHY DU".into())), &mut data, RECT, 1.0),
            (true, None)
        );
        assert_eq!(state.browser_visible_indices(), [7]);
        assert_eq!(data, before);
        let mut expected = before;
        factory_shape(FactoryShape::Duck)
            .apply(&mut expected, fit)
            .unwrap();
        let response = state.on_key(
            &KeyboardEvent {
                modifiers: if fit {
                    Modifiers::SHIFT
                } else {
                    Modifiers::empty()
                },
                ..key(Key::Enter)
            },
            &mut data,
            RECT,
            1.0,
        );
        assert_eq!(response, (true, Some(MsegEdit::Changed)));
        assert_eq!(data, expected);
        assert!(!state.curve_browser_open());
        assert!(!state.browser.search.visible);
        assert!(state.browser.search.text().is_empty());
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn command_modified_enter_cannot_recall_from_focused_search() {
    for modifiers in [
        Modifiers::CONTROL,
        Modifiers::META,
        Modifiers::ALT,
        Modifiers::CONTROL | Modifiers::SHIFT,
        Modifiers::ALT | Modifiers::SHIFT,
    ] {
        let (mut state, _, root) = setup();
        let mut data = MsegData::default();
        let before = data;
        assert!(state.open_curve_browser(RECT, 1.0));
        find(&mut state, &mut data);
        state.on_key(&key(Key::Character("duck".into())), &mut data, RECT, 1.0);
        assert_eq!(state.browser_visible_indices(), [7]);
        for kind in [KeyState::Down, KeyState::Up] {
            assert_eq!(
                state.on_key(
                    &KeyboardEvent {
                        modifiers,
                        state: kind,
                        ..key(Key::Enter)
                    },
                    &mut data,
                    RECT,
                    1.0
                ),
                (true, None)
            );
        }
        assert_eq!(data, before);
        assert!(state.curve_browser_open() && state.browser.search.focused);
        assert_eq!(state.browser.search.text(), "duck");
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn sparse_user_search_pages_cards_and_double_click_follow_visible_source_indices() {
    let (mut state, library, root) = setup();
    for i in 0..30 {
        let mut source = MsegData::default();
        source.nodes[0].value = (i + 1) as f32 / 40.0;
        source.nodes[1].value = source.nodes[0].value;
        let (name, category) = if i % 3 == 0 {
            (format!("Preset {i:02} Pulse"), "Bass")
        } else {
            (format!("Preset {i:02} Other"), "Lead")
        };
        library
            .save_user_preset_with_category(&name, category, &CurveShape::capture(&source))
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    find(&mut state, &mut data);
    state.on_key(
        &key(Key::Character("pulse bass".into())),
        &mut data,
        RECT,
        1.0,
    );
    assert_eq!(
        state.browser_visible_indices(),
        (0..30).step_by(3).collect::<Vec<_>>()
    );
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    assert!(state.browser.page == Page::User && !state.browser.search.focused);
    state.on_key(&key(Key::PageDown), &mut data, RECT, 1.0);
    assert_eq!(state.browser.selected, 24);
    state.on_key(&key(Key::End), &mut data, RECT, 1.0);
    assert_eq!(state.browser.selected, 27);
    let card = state.browser_layout(RECT, 1.0).unwrap().cards[0];
    let expected = state.browser.user.entries[24].clone();
    state.on_double_click(card.0 + 2.0, card.1 + 2.0, &mut data, RECT, 1.0);
    assert_eq!(data.active(), expected.shape.nodes());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_search_disables_hidden_actions_and_metadata_success_reveals_its_id() {
    let (mut state, library, root) = setup();
    library
        .save_user_preset_with_category("Original", "Bass", &factory_shape(FactoryShape::Duck))
        .unwrap();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    find(&mut state, &mut data);
    state.on_key(&key(Key::Character("zzz".into())), &mut data, RECT, 1.0);
    assert!(state.browser_visible_indices().is_empty());
    assert_eq!(
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
        (true, None)
    );
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    for value in [Key::F2, Key::Delete] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
        assert!(state.browser.user.dialog.is_none());
    }
    state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
    state.on_key(
        &KeyboardEvent {
            modifiers: Modifiers::CONTROL,
            ..key(Key::Character("f".into()))
        },
        &mut data,
        RECT,
        1.0,
    );
    assert!(!state.browser.search.focused);
    state.on_key(&key(Key::Character("/".into())), &mut data, RECT, 1.0);
    state.on_key(
        &key(Key::Character("New record".into())),
        &mut data,
        RECT,
        1.0,
    );
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    let selected = &state.browser.user.entries[state.browser.selected];
    assert_eq!(selected.name, "/New record");
    assert!(state.browser.search.text().is_empty());
    assert!(!state.browser.search.focused);
    assert!(
        state
            .browser_visible_indices()
            .contains(&state.browser.selected)
    );
    assert_eq!(data, MsegData::default());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn popup_search_and_browser_escape_layers_do_not_steal_each_others_input() {
    let (mut state, _, root) = setup();
    let mut data = MsegData::default();
    assert!(state.open_curve_browser(RECT, 1.0));
    find(&mut state, &mut data);
    state.on_key(&key(Key::Character("duck".into())), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Character("c".into())), &mut data, RECT, 1.0);
    assert!(state.browser.categories.menu.is_open());
    state.on_key(
        &KeyboardEvent {
            modifiers: Modifiers::META,
            ..key(Key::Character("f".into()))
        },
        &mut data,
        RECT,
        1.0,
    );
    assert!(state.browser.categories.menu.is_open());
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(!state.browser.categories.menu.is_open());
    assert_eq!(state.browser.search.text(), "duck");
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(state.curve_browser_open() && !state.browser.search.visible);
    assert_eq!(state.browser_visible_indices(), (0..8).collect::<Vec<_>>());
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(!state.curve_browser_open());
    assert_eq!(data, MsegData::default());
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn search_typing_navigation_and_drawing_do_not_read_unavailable_storage() {
    let (mut state, library, root) = setup();
    for i in 0..10 {
        library
            .save_user_preset_with_category(
                &format!("Preset {i:02}"),
                if i % 2 == 0 { "Bass" } else { "Lead" },
                &factory_shape(FactoryShape::Duck),
            )
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    find(&mut state, &mut data);
    let hidden = root.with_extension("hidden");
    std::fs::rename(&root, &hidden).unwrap();
    std::fs::write(&root, "not a directory").unwrap();
    state.on_key(&key(Key::Character("bass".into())), &mut data, RECT, 1.0);
    assert_eq!(
        state.browser_visible_indices(),
        (0..10).step_by(2).collect::<Vec<_>>()
    );
    let mut pm = Pixmap::new(1040, 720).unwrap();
    state.draw_curve_browser(
        &mut pm,
        &mut TextRenderer::new(crate::DEFAULT_FONT),
        RECT,
        1.0,
    );
    assert!(state.browser.user.error.is_none());
    state.on_key(&key(Key::ArrowDown), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
    assert!(state.browser.user.error.is_some());
    assert_eq!(state.browser.search.text(), "bass");
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(hidden, &root).unwrap();
    state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
    assert_eq!(
        state.browser_visible_indices(),
        (0..10).step_by(2).collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn literal_slash_option_text_and_reset_paths_keep_search_limit_and_curve_state() {
    let (mut state, _, root) = setup();
    let mut data = MsegData::default();
    data.insert_node(0.5, 0.5).unwrap();
    state.selection = 3;
    let before = data;
    let viewport = state.viewport();
    assert!(state.open_curve_browser(RECT, 1.0));
    state.on_key(&key(Key::Character("/".into())), &mut data, RECT, 1.0);
    for text in ["S", "/", "R"] {
        state.on_key(&key(Key::Character(text.into())), &mut data, RECT, 1.0);
    }
    assert_eq!(state.browser.search.text(), "S/R");
    for command in ["a", "c", "v", "z"] {
        state.on_key(
            &KeyboardEvent {
                modifiers: Modifiers::CONTROL,
                ..key(Key::Character(command.into()))
            },
            &mut data,
            RECT,
            1.0,
        );
        assert_eq!(state.selection_count(), 2);
        assert_eq!(data, before);
    }
    assert!(!root.exists());
    find(&mut state, &mut data);
    for kind in [KeyState::Down, KeyState::Up] {
        state.on_key(
            &KeyboardEvent {
                modifiers: Modifiers::ALT | Modifiers::SHIFT,
                state: kind,
                ..key(Key::Character("Å".into()))
            },
            &mut data,
            RECT,
            1.0,
        );
    }
    assert_eq!(state.browser.search.text(), "Å");
    for round in 0..3 {
        find(&mut state, &mut data);
        state.on_key(&key(Key::Character("曲".repeat(300))), &mut data, RECT, 1.0);
        assert_eq!(state.browser.search.text().chars().count(), 256);
        if round == 0 {
            let clear = state.browser_layout(RECT, 1.0).unwrap().search.unwrap()[1];
            state.on_mouse_down(clear.0 + 1.0, clear.1 + 1.0, &mut data, RECT, 1.0, false);
            assert!(state.browser.search.visible && state.browser.search.focused);
        } else {
            state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
            assert!(!state.browser.search.visible);
        }
    }
    assert_eq!(data, before);
    assert_eq!(state.selection_count(), 2);
    assert_eq!(state.viewport(), viewport);
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn refresh_delete_and_stale_records_retain_query_and_never_load_a_hidden_source() {
    let (mut state, library, root) = setup();
    let a = library
        .save_user_preset_with_category("Pulse A", "Bass", &factory_shape(FactoryShape::Duck))
        .unwrap();
    library
        .save_user_preset_with_category("Pulse B", "Bass", &factory_shape(FactoryShape::Sine))
        .unwrap();
    library
        .save_user_preset_with_category("Other", "Lead", &factory_shape(FactoryShape::Square))
        .unwrap();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    find(&mut state, &mut data);
    state.on_key(&key(Key::Character("pulse".into())), &mut data, RECT, 1.0);
    for value in [Key::Tab, Key::End, Key::Delete, Key::Enter] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    assert_eq!(state.browser.search.text(), "pulse");
    assert_eq!(state.browser_visible_indices().len(), 1);
    assert_eq!(state.browser.user.entries[state.browser.selected].id, a.id);
    library.rename_user_preset(&a, "Other A").unwrap();
    assert_eq!(
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
        (true, None)
    );
    assert!(state.browser_visible_indices().is_empty());
    assert!(state.browser.message.contains("changed"));
    assert_eq!(state.browser.search.text(), "pulse");
    assert_eq!(data, MsegData::default());
    find(&mut state, &mut data);
    state.on_key(&key(Key::Character("other".into())), &mut data, RECT, 1.0);
    assert_eq!(state.browser_visible_indices().len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_reveal_clears_only_a_query_that_no_longer_matches() {
    for query in ["pulse", "bass"] {
        let (mut state, library, root) = setup();
        let saved = library
            .save_user_preset_with_category("Pulse", "Bass", &factory_shape(FactoryShape::Duck))
            .unwrap();
        let mut data = MsegData::default();
        user(&mut state, &mut data);
        state.select_browser_category(CategoryFilter::Category("Bass".into()));
        find(&mut state, &mut data);
        state.on_key(&key(Key::Character(query.into())), &mut data, RECT, 1.0);
        for value in [
            Key::Tab,
            Key::F2,
            Key::Character("Pulse renamed".into()),
            Key::Tab,
            Key::Character("Lead".into()),
            Key::Enter,
        ] {
            state.on_key(&key(value), &mut data, RECT, 1.0);
        }
        assert_eq!(
            state.browser.search.text(),
            if query == "pulse" { "pulse" } else { "" }
        );
        assert!(!state.browser.search.focused);
        assert_eq!(
            state.browser.user.entries[state.browser.selected].id,
            saved.id
        );
        assert!(state.browser.categories.filter.matches("Lead"));
        assert_eq!(data, MsegData::default());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn pure_search_closes_on_authored_replacement_before_recall_or_pointer_input() {
    for event in 0..5 {
        let (mut state, library, root) = setup();
        let saved = library
            .save_user_preset("Saved", &factory_shape(FactoryShape::Duck))
            .unwrap();
        let mut data = MsegData::default();
        user(&mut state, &mut data);
        find(&mut state, &mut data);
        state.on_key(&key(Key::Character("saved".into())), &mut data, RECT, 1.0);
        let layout = state.browser_layout(RECT, 1.0).unwrap();
        factory_shape(FactoryShape::Square)
            .apply(&mut data, false)
            .unwrap();
        data.time_seconds = 2.5;
        let replacement = data;
        match event {
            0 => {
                assert_eq!(
                    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                    (true, None)
                );
            }
            1 => {
                let r = layout.cards[0];
                assert_eq!(
                    state.on_double_click(r.0 + 2.0, r.1 + 2.0, &mut data, RECT, 1.0),
                    None
                );
            }
            2 => {
                assert!(state.on_scroll(100.0, 100.0, 1.0, false, &data, RECT, 1.0));
            }
            3 => {
                state.on_mouse_up(&mut data, RECT, 1.0);
            }
            _ => {
                state.on_mouse_move(100.0, 100.0, &mut data, RECT, 1.0, false);
            }
        }
        assert!(!state.curve_browser_open());
        assert!(!state.browser.search.visible && state.browser.search.text().is_empty());
        assert_eq!(data, replacement);
        assert_eq!(library.user_presets().unwrap(), [saved]);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn mouse_and_keyboard_search_entry_reject_invisible_expanded_layouts() {
    for mouse in [false, true] {
        let (mut state, library, root) = setup();
        let saved = library
            .save_user_preset("Saved", &factory_shape(FactoryShape::Duck))
            .unwrap();
        let bytes = std::fs::read(root.join("user-presets.json")).unwrap();
        state.set_overlay_bounds((290.0, 205.0));
        let rect = (10.0, 10.0, 270.0, 185.0);
        let mut data = MsegData::default();
        assert!(state.open_curve_browser(rect, 0.5));
        for _ in 1..state.browser_pages().len() {
            state.on_key(&key(Key::Tab), &mut data, rect, 0.5);
        }
        let layout = state.browser_layout(rect, 0.5).unwrap();
        if mouse {
            let find = layout.find.unwrap();
            state.on_mouse_down(find.0 + 1.0, find.1 + 1.0, &mut data, rect, 0.5, false);
        } else {
            state.on_key(
                &KeyboardEvent {
                    modifiers: Modifiers::CONTROL,
                    ..key(Key::Character("f".into()))
                },
                &mut data,
                rect,
                0.5,
            );
        }
        assert!(!state.curve_browser_open());
        assert!(state.status_message().contains("Enlarge"));
        assert!(!state.browser.search.visible);
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, rect, 0.5),
            (false, None)
        );
        assert_eq!(data, MsegData::default());
        assert_eq!(library.user_presets().unwrap(), [saved]);
        assert_eq!(
            std::fs::read(root.join("user-presets.json")).unwrap(),
            bytes
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn tab_changes_reset_query_and_fixed_slot_tabs_keep_their_original_layout() {
    let (mut state, _, root) = setup();
    state.bind_wave_palette(Arc::new(Mutex::new(WavePalette::default())));
    let mut data = MsegData::default();
    assert!(state.open_curve_browser(RECT, 1.0));
    find(&mut state, &mut data);
    state.on_key(&key(Key::Character("duck".into())), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    assert!(state.browser.page == Page::Factory);
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    for expected in [Page::Favorites, Page::Project] {
        assert!(state.browser.page == expected);
        let layout = state.browser_layout(RECT, 1.0).unwrap();
        assert!(layout.find.is_none() && layout.search.is_none());
        assert!(state.browser.search.text().is_empty());
        state.on_key(
            &KeyboardEvent {
                modifiers: Modifiers::META,
                ..key(Key::Character("f".into()))
            },
            &mut data,
            RECT,
            1.0,
        );
        assert!(!state.browser.search.visible);
        state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    }
    assert!(state.browser.page == Page::User);
    assert!(!state.browser.search.visible && state.browser.search.text().is_empty());
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn closing_search_with_a_double_click_cannot_load_the_card_reflowing_beneath_it() {
    let (mut state, _, root) = setup();
    let mut data = MsegData::default();
    assert!(state.open_curve_browser(RECT, 1.0));
    find(&mut state, &mut data);
    let close = state.browser_layout(RECT, 1.0).unwrap().search.unwrap()[2];
    let (x, y) = (close.0 + close.2 * 0.5, close.1 + close.3 * 0.5);
    state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
    assert!(state.curve_browser_open() && !state.browser.search.visible);
    let selected = state.browser.selected;
    assert_eq!(state.on_mouse_down(x, y, &mut data, RECT, 1.0, false), None);
    assert_eq!(state.browser.selected, selected);
    assert_eq!(state.on_double_click(x, y, &mut data, RECT, 1.0), None);
    assert_eq!(data, MsegData::default());
    assert!(state.curve_browser_open());
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn search_row_controls_and_thumbnails_fit_native_scaled_and_narrow_layouts() {
    for curve_only in [false, true] {
        for (scale, bounds) in [
            (0.5, (570.0, 352.0)),
            (1.0, (1140.0, 704.0)),
            (2.0, (2280.0, 1408.0)),
            (0.5, (124.0, 300.0)),
            (1.0, (320.0, 720.0)),
        ] {
            let (mut state, _, root) = setup();
            if curve_only {
                state = MsegEditState::new_curve_only();
                state.set_curve_library_directory(root.clone());
            }
            state.set_overlay_bounds(bounds);
            let rect = (0.0, 0.0, bounds.0, bounds.1);
            let mut data = MsegData::default();
            assert!(state.open_curve_browser(rect, scale));
            let original = state.browser_layout(rect, scale).unwrap();
            let find = original.find.unwrap();
            state.on_mouse_down(find.0 + 1.0, find.1 + 1.0, &mut data, rect, scale, false);
            assert!(state.browser.search.visible && state.browser.search.focused);
            let layout = state.browser_layout(rect, scale).unwrap();
            assert_eq!(layout.find, original.find);
            assert_eq!(layout.category, original.category);
            assert_eq!(layout.pager, original.pager);
            let [field, clear, close] = layout.search.unwrap();
            assert!(field.1 + field.3 <= layout.cards[0].1);
            let preview = thumbnail_rect(layout.cards[0], scale);
            assert!(preview.3 >= 8.0 * scale.max(1.0));
            let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
            for (r, label) in [(find, "Find"), (clear, "Clear"), (close, "X")] {
                assert!(tr.text_width(label, (r.3 * 0.5).max(10.0)) <= r.2 - 8.0);
            }
            for r in [field, clear, close] {
                assert!(r.0 >= layout.panel.0 && r.1 >= layout.panel.1);
                assert!(r.0 + r.2 <= layout.panel.0 + layout.panel.2 + 0.01);
                assert!(r.1 + r.3 <= layout.panel.1 + layout.panel.3 + 0.01);
            }
            state.on_key(
                &key(Key::Character("曲".repeat(200))),
                &mut data,
                rect,
                scale,
            );
            assert_eq!(state.browser.search.text().chars().count(), 200);
            let mut pm = Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
            state.draw_curve_browser(&mut pm, &mut tr, rect, scale);
            assert!(state.browser_visible_indices().is_empty());
            state.on_mouse_down(close.0 + 1.0, close.1 + 1.0, &mut data, rect, scale, false);
            assert!(state.curve_browser_open() && !state.browser.search.visible);
            assert_eq!(data, MsegData::default());
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}

#[test]
fn assignment_metadata_search_and_browser_have_separate_escape_layers() {
    let (mut state, library, root) = setup();
    let saved = library
        .save_user_preset("Saved", &factory_shape(FactoryShape::Duck))
        .unwrap();
    let bytes = std::fs::read(root.join("user-presets.json")).unwrap();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    find(&mut state, &mut data);
    state.on_key(&key(Key::Character("saved".into())), &mut data, RECT, 1.0);
    for value in [Key::Tab, Key::Character("s".into()), Key::Tab, Key::F4] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    assert!(state.browser.categories.menu.is_open());
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(!state.browser.categories.menu.is_open());
    assert!(state.browser.user.dialog.is_some());
    state.on_key(&key(Key::Character("/".into())), &mut data, RECT, 1.0);
    assert_eq!(state.user_category_text(), Some("/"));
    assert_eq!(state.browser.search.text(), "saved");
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(state.browser.user.dialog.is_none() && state.browser.search.visible);
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(state.curve_browser_open() && !state.browser.search.visible);
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(!state.curve_browser_open());
    assert_eq!(data, MsegData::default());
    assert_eq!(library.user_presets().unwrap(), [saved]);
    assert_eq!(
        std::fs::read(root.join("user-presets.json")).unwrap(),
        bytes
    );
    std::fs::remove_dir_all(root).unwrap();
}
