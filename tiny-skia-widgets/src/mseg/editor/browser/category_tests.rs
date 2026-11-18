use super::categories::{CategoryFilter, CategoryMenu};
use super::*;
use crate::mseg::library::CurveLibrary;
use std::sync::atomic::{AtomicU64, Ordering};

const RECT: Rect = (20.0, 70.0, 1000.0, 420.0);
static NEXT: AtomicU64 = AtomicU64::new(0);

fn setup() -> (MsegEditState, CurveLibrary, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "tract-browser-category-{}-{}",
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

#[test]
fn every_factory_category_maps_cards_and_recall_to_original_shape_indices() {
    for (label, indices) in [
        ("Basic", vec![0, 1]),
        ("Ramps", vec![2, 3]),
        ("Modulation", vec![4, 5]),
        ("Rhythmic", vec![6, 7]),
    ] {
        let (mut state, _, root) = setup();
        let mut data = MsegData::default();
        assert!(state.open_curve_browser(RECT, 1.0));
        let before = data;
        state.select_browser_category(CategoryFilter::Category(label.to_owned()));
        assert_eq!(state.browser_visible_indices(), indices);
        assert_eq!(data, before);
        state.on_key(&key(Key::End), &mut data, RECT, 1.0);
        let index = *indices.last().unwrap();
        let mut expected = before;
        factory_shape(FactoryShape::ALL[index])
            .apply(&mut expected, false)
            .unwrap();
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
        assert_eq!(data, expected);
        assert!(!state.curve_browser_open());
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn sparse_user_categories_use_visible_positions_for_pages_cards_and_recall() {
    let (mut state, library, root) = setup();
    for i in 0..28 {
        let mut source = MsegData::default();
        source.nodes[0].value = (i + 1) as f32 / 50.0;
        source.nodes[1].value = source.nodes[0].value;
        library
            .save_user_preset_with_category(
                &format!("Curve {i:02}"),
                if i % 2 == 0 { "Bass" } else { "Lead" },
                &CurveShape::capture(&source),
            )
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    state.select_browser_category(CategoryFilter::Category("BASS".into()));
    assert_eq!(
        state.browser_visible_indices(),
        (0..28).step_by(2).collect::<Vec<_>>()
    );
    state.on_key(&key(Key::End), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(26));
    state.on_key(&key(Key::PageUp), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(10));
    state.on_key(&key(Key::PageDown), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(26));
    let card = state.browser_layout(RECT, 1.0).unwrap().cards[0];
    state.on_mouse_down(card.0 + 2.0, card.1 + 2.0, &mut data, RECT, 1.0, false);
    assert_eq!(state.browser.view.selected_index(), Some(16));
    let expected = state.browser.user.entries[16].clone();
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(data.active(), expected.shape.nodes());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recategorizing_follows_the_stable_id_and_reveals_its_new_category_without_curve_edits() {
    let (mut state, library, root) = setup();
    let original = library
        .save_user_preset_with_category("A", "Bass", &factory_shape(FactoryShape::Duck))
        .unwrap();
    library
        .save_user_preset_with_category("Z", "Lead", &factory_shape(FactoryShape::Sine))
        .unwrap();
    let mut data = MsegData::default();
    let before = data;
    user(&mut state, &mut data);
    state.select_browser_category(CategoryFilter::Category("Bass".into()));
    for value in [Key::F2, Key::Tab, Key::Character("Lead".into()), Key::Enter] {
        assert_eq!(
            state.on_key(&key(value), &mut data, RECT, 1.0),
            (true, None)
        );
    }
    let selected = &state.browser.user.entries[state.browser.view.selected_index().unwrap()];
    assert_eq!(selected.id, original.id);
    assert_eq!(selected.category, "Lead");
    assert_eq!(selected.shape, original.shape);
    assert!(state.browser.categories.filter.matches("Lead"));
    assert!(!state.browser.categories.filter.matches("Bass"));
    assert_eq!(state.browser_visible_indices(), [0, 1]);
    assert_eq!(data, before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_filters_disable_hidden_actions_and_new_saves_inherit_the_filter() {
    let (mut state, library, root) = setup();
    library
        .save_user_preset("Other", &factory_shape(FactoryShape::Duck))
        .unwrap();
    let mut data = MsegData::default();
    let before = data;
    user(&mut state, &mut data);
    state.select_browser_category(CategoryFilter::Category("Ghost".into()));
    assert!(state.browser_visible_indices().is_empty());
    for value in [Key::Enter, Key::F2, Key::Delete, Key::PageDown] {
        assert_eq!(
            state.on_key(&key(value), &mut data, RECT, 1.0),
            (true, None)
        );
        assert!(state.browser.user.dialog.is_none());
    }
    for value in [
        Key::Character("s".into()),
        Key::Character("New".into()),
        Key::Enter,
    ] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    let saved = library
        .user_presets()
        .unwrap()
        .into_iter()
        .find(|preset| preset.name == "New")
        .unwrap();
    assert_eq!(saved.category, "Ghost");
    assert_eq!(state.browser_visible_indices().len(), 1);
    assert_eq!(data, before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn category_popups_consume_outside_presses_and_preserve_uncommitted_form_data() {
    let (mut state, _, root) = setup();
    let mut data = MsegData::default();
    assert!(state.open_curve_browser(RECT, 1.0));
    state.on_key(&key(Key::Character("c".into())), &mut data, RECT, 1.0);
    assert!(
        state
            .browser
            .categories
            .menu
            .is_open_for(CategoryMenu::Filter)
    );
    let card = state.browser_layout(RECT, 1.0).unwrap().cards[7];
    state.on_mouse_down(
        card.0 + 1.0,
        card.1 + card.3 - 1.0,
        &mut data,
        RECT,
        1.0,
        false,
    );
    assert!(!state.browser.categories.menu.is_open());
    assert!(state.curve_browser_open());
    assert_eq!(state.browser.view.selected_index(), Some(0));
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    user(&mut state, &mut data);
    for value in [Key::Character("s".into()), Key::Tab, Key::F4] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    assert!(
        state
            .browser
            .categories
            .menu
            .is_open_for(CategoryMenu::Assign)
    );
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(!state.browser.categories.menu.is_open());
    assert!(state.browser.user.dialog.is_some());
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(state.browser.user.dialog.is_none());
    assert_eq!(data, MsegData::default());
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn special_filters_remain_distinct_from_identically_named_custom_categories() {
    let (mut state, library, root) = setup();
    let shape = factory_shape(FactoryShape::Duck);
    for (name, category) in [
        ("A", ""),
        ("B", "All categories"),
        ("C", "Uncategorized"),
        ("D", "bass"),
        ("E", "BASS"),
        ("F", "Lead"),
    ] {
        library
            .save_user_preset_with_category(name, category, &shape)
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    for (choice, indices) in [
        (1, vec![0]),
        (2, vec![1]),
        (5, vec![2]),
        (3, vec![3, 4]),
        (0, (0..6).collect()),
    ] {
        state.on_key(&key(Key::Character("c".into())), &mut data, RECT, 1.0);
        state.on_key(&key(Key::Home), &mut data, RECT, 1.0);
        for _ in 0..choice {
            state.on_key(&key(Key::ArrowDown), &mut data, RECT, 1.0);
        }
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
        assert_eq!(state.browser_visible_indices(), indices);
        assert!(!state.browser.categories.menu.is_open());
    }
    assert_eq!(data, MsegData::default());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn category_input_and_assignment_preserve_option_text_and_submit_only_once() {
    let (mut state, library, root) = setup();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Character("Saved".into())), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    assert!(
        state
            .context_hint(&data, RECT, 1.0)
            .unwrap()
            .contains("Uncategorized")
    );
    state.on_key(
        &KeyboardEvent {
            modifiers: Modifiers::ALT | Modifiers::SHIFT,
            ..key(Key::Character("Åß€".into()))
        },
        &mut data,
        RECT,
        1.0,
    );
    assert_eq!(state.user_category_text(), Some("Åß€"));
    state.on_key(
        &KeyboardEvent {
            modifiers: Modifiers::ALT,
            ..key(Key::ArrowDown)
        },
        &mut data,
        RECT,
        1.0,
    );
    assert!(
        state
            .browser
            .categories
            .menu
            .is_open_for(CategoryMenu::Assign)
    );
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert_eq!(state.user_category_text(), Some("Åß€"));
    assert!(library.user_presets().unwrap().is_empty());
    state.on_key(&key(Key::F4), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Home), &mut data, RECT, 1.0);
    state.on_key(&key(Key::ArrowDown), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(state.user_category_text(), Some("Basic"));
    assert!(state.browser.user.dialog.is_some());
    assert!(library.user_presets().unwrap().is_empty());
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    let saved = library.user_presets().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].category, "Basic");
    assert_eq!(saved[0].name, "Saved");
    assert_eq!(data, MsegData::default());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn filtered_rename_delete_and_external_recategorization_keep_stable_record_ownership() {
    let (mut state, library, root) = setup();
    let bass = library
        .save_user_preset_with_category("Bass", "Bass", &factory_shape(FactoryShape::RampDown))
        .unwrap();
    let lead = library
        .save_user_preset_with_category("Lead", "Lead", &factory_shape(FactoryShape::Sine))
        .unwrap();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    state.select_browser_category(CategoryFilter::Category("Lead".into()));
    for value in [Key::F2, Key::Character("000 Lead".into()), Key::Enter] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    assert_eq!(state.browser.view.selected_index(), Some(0));
    let selected = state.browser.user.entries[state.browser.view.selected_index().unwrap()].clone();
    assert_eq!(selected.id, lead.id);
    assert_eq!(selected.category, "Lead");
    let changed = library
        .update_user_preset(&selected, &selected.name, "Bass")
        .unwrap();
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert!(state.browser_visible_indices().is_empty());
    assert!(state.browser.message.contains("changed"));
    assert_eq!(data, MsegData::default());
    state.select_browser_category(CategoryFilter::Category("Bass".into()));
    assert_eq!(
        state.browser.user.entries[state.browser.view.selected_index().unwrap()].id,
        changed.id
    );
    state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(library.user_presets().unwrap(), [bass]);
    assert_eq!(state.browser_visible_indices(), [0]);
    assert_eq!(data, MsegData::default());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn filtered_double_click_recalls_the_displayed_record_and_keeps_its_category() {
    let (mut state, library, root) = setup();
    library
        .save_user_preset_with_category("A", "Bass", &factory_shape(FactoryShape::FlatCenter))
        .unwrap();
    let lead = library
        .save_user_preset_with_category("Z", "Lead", &factory_shape(FactoryShape::Duck))
        .unwrap();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    state.select_browser_category(CategoryFilter::Category("Lead".into()));
    let card = state.browser_layout(RECT, 1.0).unwrap().cards[0];
    assert_eq!(
        state.on_double_click(card.0 + 2.0, card.1 + 2.0, &mut data, RECT, 1.0),
        Some(MsegEdit::Changed)
    );
    assert_eq!(data.active(), lead.shape.nodes());
    assert_eq!(library.user_presets().unwrap()[1], lead);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn filtering_and_popup_navigation_use_only_the_cached_snapshot() {
    let (mut state, library, root) = setup();
    for (name, category) in [("A", "Bass"), ("B", "Lead")] {
        library
            .save_user_preset_with_category(name, category, &factory_shape(FactoryShape::Duck))
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    let hidden = root.with_extension("hidden");
    std::fs::rename(&root, &hidden).unwrap();
    std::fs::write(&root, "not a directory").unwrap();
    state.select_browser_category(CategoryFilter::Category("Bass".into()));
    state.on_key(&key(Key::Character("c".into())), &mut data, RECT, 1.0);
    for value in [Key::End, Key::Home, Key::Enter] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    let mut pm = Pixmap::new(1040, 720).unwrap();
    state.draw_curve_browser(
        &mut pm,
        &mut TextRenderer::new(crate::DEFAULT_FONT),
        RECT,
        1.0,
    );
    assert!(state.browser.user.error.is_none());
    assert_eq!(state.browser_visible_indices(), [0, 1]);
    state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
    assert!(state.browser.user.error.is_some());
    assert!(state.browser_visible_indices().is_empty());
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(hidden, &root).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mouse_opened_metadata_forms_close_immediately_when_browsing_fits_but_two_fields_do_not() {
    for action in [2, 3] {
        let (mut state, library, root) = setup();
        let saved = library
            .save_user_preset("Saved", &factory_shape(FactoryShape::Duck))
            .unwrap();
        let bytes = std::fs::read(root.join("user-presets.json")).unwrap();
        let mut data = MsegData::default();
        state.set_overlay_bounds((290.0, 205.0));
        let rect = (10.0, 10.0, 270.0, 185.0);
        assert!(state.open_curve_browser(rect, 0.5));
        for _ in 1..state.browser_pages().len() {
            state.on_key(&key(Key::Tab), &mut data, rect, 0.5);
        }
        assert!(state.curve_browser_open());
        let button = state.browser_layout(rect, 0.5).unwrap().buttons[action];
        assert_eq!(
            state.on_mouse_down(button.0 + 1.0, button.1 + 1.0, &mut data, rect, 0.5, false),
            None
        );
        assert!(!state.curve_browser_open());
        assert!(state.browser.user.dialog.is_none());
        assert!(state.status_message().contains("Enlarge"));
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
fn admitted_category_headers_reserve_a_readable_compact_caption_at_every_scale() {
    let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
    for scale in [0.5, 1.0, 2.0] {
        let mut accepted = 0;
        for width in (120..501).step_by(4) {
            let mut state = MsegEditState::new();
            state.set_overlay_bounds((width as f32, 760.0 * scale));
            let rect = (0.0, 0.0, width as f32, 700.0 * scale);
            let Some(layout) = state.browser_layout(rect, scale) else {
                continue;
            };
            let category = layout.category.unwrap();
            let font = (category.3 * 0.5).max(10.0);
            let label_width = category.2 - 12.0 - category.3 * 0.6;
            assert!(
                tr.text_width("Cat", font) <= label_width,
                "Compact caption is clipped at scale {scale}, width {width}"
            );
            accepted += 1;
        }
        assert!(accepted > 0);
    }
}

#[test]
fn compact_user_category_popups_show_distinct_labels_and_hit_test_the_wider_rows() {
    use crate::dropdown::dropdown_popup_layout;
    let labels = [
        "All categories",
        "Uncategorized",
        "Category: Basic",
        "Category: Ramps",
        "Category: Rhythmic",
    ];
    let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
    for (scale, bounds) in [
        (1.0, (320.0, 720.0)),
        (0.5, (320.0, 360.0)),
        (2.0, (640.0, 1440.0)),
    ] {
        let (mut state, library, root) = setup();
        for (name, category) in [("A", "Basic"), ("B", "Ramps"), ("C", "Rhythmic")] {
            library
                .save_user_preset_with_category(name, category, &factory_shape(FactoryShape::Duck))
                .unwrap();
        }
        state.set_overlay_bounds(bounds);
        let rect = (0.0, 0.0, bounds.0, bounds.1);
        let mut data = MsegData::default();
        assert!(state.open_curve_browser(rect, scale));
        for _ in 1..state.browser_pages().len() {
            state.on_key(&key(Key::Tab), &mut data, rect, scale);
        }
        let trigger = state.browser_layout(rect, scale).unwrap().category.unwrap();
        for source in 0..3 {
            state.on_key(&key(Key::Character("c".into())), &mut data, rect, scale);
            let popup =
                dropdown_popup_layout(&state.browser.categories.menu, &labels, bounds).unwrap();
            for row in &popup.visible_rows {
                let text = labels[row.item_index];
                let font = (row.rect.3 * 0.5).max(10.0);
                assert!(
                    tr.text_width(text, font) <= row.rect.2 - 12.0,
                    "{text} is clipped in popup width {} at scale {scale}",
                    popup.popup_rect.2
                );
            }
            assert!(popup.popup_rect.2 > trigger.2);
            assert!(popup.popup_rect.0 >= 0.0 && popup.popup_rect.1 >= 0.0);
            assert!(popup.popup_rect.0 + popup.popup_rect.2 <= bounds.0);
            assert!(popup.popup_rect.1 + popup.popup_rect.3 <= bounds.1);
            let row = popup
                .visible_rows
                .iter()
                .find(|row| row.item_index == source + 2)
                .unwrap()
                .rect;
            assert_eq!(
                state.on_mouse_down(
                    row.0 + row.2 * 0.5,
                    row.1 + row.3 * 0.5,
                    &mut data,
                    rect,
                    scale,
                    false
                ),
                None
            );
            assert!(!state.browser.categories.menu.is_open());
            assert!(state.curve_browser_open());
            assert_eq!(state.browser_visible_indices(), [source]);
            assert_eq!(
                state.browser_layout(rect, scale).unwrap().category,
                Some(trigger)
            );
            assert_eq!(data, MsegData::default());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn category_popup_width_clamps_to_a_narrow_factory_window() {
    use crate::dropdown::dropdown_popup_layout;
    let (mut state, _, root) = setup();
    let bounds = (124.0, 300.0);
    let rect = (0.0, 0.0, 124.0, 159.0);
    state.set_overlay_bounds(bounds);
    let mut data = MsegData::default();
    assert!(state.open_curve_browser(rect, 0.5));
    let trigger = state.browser_layout(rect, 0.5).unwrap().category.unwrap();
    state.on_key(&key(Key::Character("c".into())), &mut data, rect, 0.5);
    let labels = [
        "All categories",
        "Category: Basic",
        "Category: Modulation",
        "Category: Ramps",
        "Category: Rhythmic",
    ];
    let popup = dropdown_popup_layout(&state.browser.categories.menu, &labels, bounds).unwrap();
    assert_eq!(popup.popup_rect.0, 0.0);
    assert_eq!(popup.popup_rect.2, bounds.0);
    assert!(trigger.2 < popup.popup_rect.2);
    let mut pm = Pixmap::new(124, 300).unwrap();
    state.draw_curve_browser(
        &mut pm,
        &mut TextRenderer::new(crate::DEFAULT_FONT),
        rect,
        0.5,
    );
    state.on_key(&key(Key::End), &mut data, rect, 0.5);
    state.on_key(&key(Key::Enter), &mut data, rect, 0.5);
    assert_eq!(state.browser_visible_indices(), [6, 7]);
    assert_eq!(data, MsegData::default());
    if root.exists() {
        std::fs::remove_dir_all(root).unwrap();
    }
}
