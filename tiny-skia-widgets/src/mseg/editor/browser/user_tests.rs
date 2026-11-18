use super::*;
use crate::mseg::library::CurveLibrary;
use crate::mseg::{HoldMode, MsegTool, PlayMode, Polarity, SyncMode};
use std::sync::atomic::{AtomicU64, Ordering};

const RECT: Rect = (20.0, 70.0, 1000.0, 420.0);
static NEXT: AtomicU64 = AtomicU64::new(0);

fn setup() -> (MsegEditState, CurveLibrary, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "tract-user-browser-{}-{}",
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
fn user_save_rename_delete_manage_geometry_without_changing_authored_state() {
    let (mut state, library, root) = setup();
    let mut data = MsegData {
        snap: false,
        ..MsegData::default()
    };
    state.set_tool(MsegTool::Line);
    state.selection = 3;
    let before = data;
    let view = state.viewport();
    user(&mut state, &mut data);
    for value in [
        Key::Character("s".into()),
        Key::Character("My 曲線".into()),
        Key::Enter,
    ] {
        assert_eq!(
            state.on_key(&key(value), &mut data, RECT, 1.0),
            (true, None)
        );
        assert_eq!(data, before);
    }
    let saved = library.user_presets().unwrap().remove(0);
    assert_eq!(saved.name, "My 曲線");
    assert_eq!(saved.shape, CurveShape::capture(&before));
    for value in [Key::F2, Key::Character("Soft S/R".into()), Key::Enter] {
        assert_eq!(
            state.on_key(&key(value), &mut data, RECT, 1.0),
            (true, None)
        );
    }
    let renamed = library.user_presets().unwrap().remove(0);
    assert_eq!(renamed.id, saved.id);
    assert_eq!(renamed.name, "Soft S/R");
    state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
    assert_eq!(library.user_presets().unwrap(), [renamed.clone()]);
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(state.curve_browser_open());
    state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert!(library.user_presets().unwrap().is_empty());
    assert_eq!(data, before);
    assert_eq!(state.selection_count(), 2);
    assert_eq!(state.viewport(), view);
    assert_eq!(state.tool(), MsegTool::Line);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn named_load_fit_and_noop_use_existing_recall_contract() {
    for fit in [false, true] {
        let (mut state, library, root) = setup();
        let shape = factory_shape(FactoryShape::Duck);
        library.save_user_preset("Duck", &shape).unwrap();
        let mut data = MsegData {
            sync_mode: SyncMode::Beat,
            beats: 7.0,
            time_seconds: 2.5,
            polarity: Polarity::Bipolar,
            play_mode: PlayMode::OneShot,
            hold: HoldMode::Sustain(0),
            ..MsegData::default()
        };
        data.nodes[0].value = 0.3;
        data.nodes[1].value = 0.7;
        let mut expected = data;
        shape.apply(&mut expected, fit).unwrap();
        user(&mut state, &mut data);
        assert_eq!(
            state.on_key(
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
                1.0
            ),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(data, expected);
        assert!(!state.curve_browser_open());
        user(&mut state, &mut data);
        assert_eq!(
            state.on_key(
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
                1.0
            ),
            (true, None)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stale_named_actions_refresh_before_loading_renaming_or_deleting() {
    for action in [Key::Enter, Key::F2, Key::Delete] {
        let (mut state, library, root) = setup();
        let shape = factory_shape(FactoryShape::Duck);
        let saved = library.save_user_preset("Original", &shape).unwrap();
        let mut data = MsegData::default();
        let before = data;
        user(&mut state, &mut data);
        let renamed = library.rename_user_preset(&saved, "External").unwrap();
        state.on_key(&key(action), &mut data, RECT, 1.0);
        assert_eq!(data, before);
        assert!(state.curve_browser_open());
        assert!(state.browser.message.contains("changed"));
        assert_eq!(library.user_presets().unwrap(), [renamed]);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn named_pages_reach_every_entry_and_clamp_after_last_page_delete() {
    let (mut state, library, root) = setup();
    let shape = factory_shape(FactoryShape::Sine);
    for i in 0..17 {
        library
            .save_user_preset(&format!("Curve {i:02}"), &shape)
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    state.on_key(&key(Key::PageDown), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(8));
    state.on_key(&key(Key::PageDown), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(16));
    state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(15));
    state.on_key(&key(Key::Home), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(0));
    state.on_key(&key(Key::End), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(15));
    state.on_key(&key(Key::PageUp), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(7));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn page_boundary_buttons_and_keys_preserve_the_current_card_selection() {
    let (mut state, library, root) = setup();
    for i in 0..21 {
        library
            .save_user_preset(&format!("Curve {i:02}"), &factory_shape(FactoryShape::Duck))
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    for _ in 0..3 {
        state.on_key(&key(Key::ArrowRight), &mut data, RECT, 1.0);
    }
    state.on_key(&key(Key::PageUp), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(3));
    let previous = state.browser_layout(RECT, 1.0).unwrap().pager.unwrap()[0];
    state.on_mouse_down(
        previous.0 + 1.0,
        previous.1 + 1.0,
        &mut data,
        RECT,
        1.0,
        false,
    );
    assert_eq!(state.browser.view.selected_index(), Some(3));
    state.on_key(&key(Key::End), &mut data, RECT, 1.0);
    for _ in 0..3 {
        state.on_key(&key(Key::ArrowLeft), &mut data, RECT, 1.0);
    }
    assert_eq!(state.browser.view.selected_index(), Some(17));
    state.on_key(&key(Key::PageDown), &mut data, RECT, 1.0);
    assert_eq!(state.browser.view.selected_index(), Some(17));
    let next = state.browser_layout(RECT, 1.0).unwrap().pager.unwrap()[1];
    state.on_mouse_down(next.0 + 1.0, next.1 + 1.0, &mut data, RECT, 1.0, false);
    assert_eq!(state.browser.view.selected_index(), Some(17));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_entry_uses_unicode_boundaries_and_keeps_management_keys_inside_the_field() {
    let (mut state, library, root) = setup();
    let mut data = MsegData::default();
    let before = data;
    user(&mut state, &mut data);
    for value in [
        Key::Character("s".into()),
        Key::Character("Aé曲Z".into()),
        Key::ArrowLeft,
        Key::Backspace,
        Key::Character("Δ".into()),
        Key::Home,
        Key::ArrowRight,
        Key::Delete,
        Key::End,
        Key::Backspace,
        Key::Enter,
    ] {
        assert_eq!(
            state.on_key(&key(value), &mut data, RECT, 1.0),
            (true, None)
        );
    }
    assert_eq!(library.user_presets().unwrap()[0].name, "AΔ");
    state.on_key(&key(Key::F2), &mut data, RECT, 1.0);
    state.on_key(
        &KeyboardEvent {
            modifiers: Modifiers::CONTROL,
            ..key(Key::Character("a".into()))
        },
        &mut data,
        RECT,
        1.0,
    );
    state.on_key(&key(Key::Character("SR / Δ".into())), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(library.user_presets().unwrap()[0].name, "SR / Δ");
    state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Character("曲".repeat(65))), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(library.user_presets().unwrap()[1].name.chars().count(), 64);
    assert_eq!(data, before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn option_generated_text_is_saved_in_both_save_and_rename_dialogs() {
    for (modifiers, text) in [
        (Modifiers::ALT, "å"),
        (Modifiers::ALT | Modifiers::SHIFT, "Å"),
        (Modifiers::ALT, "é"),
        (Modifiers::ALT, "ß€"),
    ] {
        for rename in [false, true] {
            let (mut state, library, root) = setup();
            let original = library
                .save_user_preset("Original", &factory_shape(FactoryShape::Duck))
                .unwrap();
            let mut data = MsegData::default();
            let before = data;
            user(&mut state, &mut data);
            state.on_key(
                &key(if rename {
                    Key::F2
                } else {
                    Key::Character("s".into())
                }),
                &mut data,
                RECT,
                1.0,
            );
            for state_kind in [KeyState::Down, KeyState::Up] {
                assert_eq!(
                    state.on_key(
                        &KeyboardEvent {
                            modifiers,
                            state: state_kind,
                            ..key(Key::Character(text.into()))
                        },
                        &mut data,
                        RECT,
                        1.0
                    ),
                    (true, None)
                );
            }
            assert_eq!(
                state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                (true, None)
            );
            let presets = library.user_presets().unwrap();
            let saved = presets
                .iter()
                .find(|preset| preset.name == text)
                .expect("Option-generated text was discarded");
            assert_eq!(presets.len(), if rename { 1 } else { 2 });
            if rename {
                assert_eq!(saved.id, original.id);
                assert_eq!(saved.shape, original.shape);
            } else {
                assert_ne!(saved.id, original.id);
                assert_eq!(saved.shape, CurveShape::capture(&before));
                assert!(presets.contains(&original));
            }
            assert_eq!(data, before);
            assert!(state.curve_browser_open());
            assert!(state.browser.user.dialog.is_none());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn option_text_keeps_commands_dead_keys_and_nonprintable_events_inside_the_name_field() {
    let (mut state, library, root) = setup();
    let mut data = MsegData::default();
    let before = data;
    user(&mut state, &mut data);
    state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
    for (modifiers, value) in [
        (Modifiers::ALT, Key::Dead),
        (Modifiers::ALT, Key::Character("\n".into())),
        (Modifiers::CONTROL, Key::Character("a".into())),
        (Modifiers::ALT, Key::Character("é".into())),
        (Modifiers::ALT, Key::ArrowLeft),
        (Modifiers::ALT, Key::Backspace),
        (Modifiers::ALT, Key::Delete),
        (Modifiers::META, Key::Character("x".into())),
        (Modifiers::CONTROL, Key::Character("z".into())),
        (Modifiers::ALT, Key::Character("s".into())),
        (
            Modifiers::ALT | Modifiers::SHIFT,
            Key::Character("R".into()),
        ),
    ] {
        assert_eq!(
            state.on_key(
                &KeyboardEvent {
                    modifiers,
                    ..key(value)
                },
                &mut data,
                RECT,
                1.0
            ),
            (true, None)
        );
        assert!(library.user_presets().unwrap().is_empty());
        assert_eq!(data, before);
    }
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert_eq!(library.user_presets().unwrap()[0].name, "ésR");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_errors_keep_the_name_draft_and_cancel_never_writes() {
    let (mut state, library, root) = setup();
    let saved = library
        .save_user_preset("Existing", &factory_shape(FactoryShape::Duck))
        .unwrap();
    let before = std::fs::read(root.join("user-presets.json")).unwrap();
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    for value in [
        Key::Character("s".into()),
        Key::Character("existing".into()),
        Key::Enter,
    ] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
    }
    assert!(state.browser.user.dialog.is_some());
    assert!(
        state
            .context_hint(&data, RECT, 1.0)
            .unwrap()
            .contains("Preset dialog")
    );
    assert_eq!(
        std::fs::read(root.join("user-presets.json")).unwrap(),
        before
    );
    state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
    assert!(state.curve_browser_open());
    assert!(state.browser.user.dialog.is_none());
    state.on_key(&key(Key::Delete), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
    assert_eq!(library.user_presets().unwrap(), [saved]);
    state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
    assert!(state.browser.user.dialog.is_none());
    assert_eq!(
        std::fs::read(root.join("user-presets.json")).unwrap(),
        before
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_changes_during_a_rename_or_delete_draft_cannot_write_stale_data() {
    for action in [Key::F2, Key::Delete] {
        let (mut state, library, root) = setup();
        let saved = library
            .save_user_preset("Original", &factory_shape(FactoryShape::Duck))
            .unwrap();
        let mut data = MsegData::default();
        user(&mut state, &mut data);
        state.on_key(&key(action), &mut data, RECT, 1.0);
        let replaced = library.rename_user_preset(&saved, "External").unwrap();
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
        assert_eq!(library.user_presets().unwrap(), [replaced]);
        assert!(state.browser.user.dialog.is_none());
        assert!(state.browser.message.contains("changed"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn authored_document_replacement_cancels_pending_user_actions_before_keyboard_or_mouse_commit() {
    for action in [Key::Character("s".into()), Key::F2, Key::Delete] {
        for mouse in [false, true] {
            let (mut state, library, root) = setup();
            let saved = library
                .save_user_preset("Saved", &factory_shape(FactoryShape::Duck))
                .unwrap();
            let mut data = MsegData::default();
            user(&mut state, &mut data);
            state.on_key(&key(action.clone()), &mut data, RECT, 1.0);
            assert!(state.browser.user.dialog.is_some());
            factory_shape(FactoryShape::Square)
                .apply(&mut data, false)
                .unwrap();
            data.time_seconds = 2.5;
            let replacement = data;
            if mouse {
                let panel = state.browser_layout(RECT, 1.0).unwrap().panel;
                state.on_mouse_down(
                    panel.0 + 30.0,
                    panel.1 + panel.3 - 30.0,
                    &mut data,
                    RECT,
                    1.0,
                    false,
                );
            } else {
                assert_eq!(
                    state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                    (true, None)
                );
            }
            assert!(!state.curve_browser_open());
            assert!(state.browser.user.dialog.is_none());
            assert_eq!(data, replacement);
            assert_eq!(library.user_presets().unwrap(), [saved]);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn resize_rebind_outside_and_right_press_discard_name_drafts_without_writing() {
    for cleanup in 0..5 {
        let (mut state, library, root) = setup();
        let mut data = MsegData::default();
        let before = data;
        user(&mut state, &mut data);
        state.on_key(&key(Key::Character("s".into())), &mut data, RECT, 1.0);
        state.on_key(&key(Key::Character("Pending".into())), &mut data, RECT, 1.0);
        match cleanup {
            0 => state.set_overlay_bounds((400.0, 300.0)),
            1 => state.set_curve_library_directory(root.join("other")),
            2 => state.cancel_interaction(),
            3 => {
                state.on_right_click(0.0, 0.0, &mut data, RECT, 1.0);
            }
            _ => {
                state.on_mouse_down(0.0, 0.0, &mut data, RECT, 1.0, false);
            }
        }
        assert!(!state.curve_browser_open());
        assert!(state.browser.user.dialog.is_none());
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
            (false, None)
        );
        assert!(library.user_presets().unwrap().is_empty());
        assert!(!root.join("user-presets.json").exists());
        assert_eq!(data, before);
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn user_pages_and_dialogs_fit_scaled_bounds_or_close_with_an_enlargement_message() {
    for (scale, bounds) in [
        (0.5, (570.0, 352.0)),
        (1.0, (1140.0, 704.0)),
        (2.0, (2280.0, 1408.0)),
        (1.0, (300.0, 260.0)),
    ] {
        let (mut state, library, root) = setup();
        let preset = library
            .save_user_preset(
                &"Long é name ".repeat(5),
                &factory_shape(FactoryShape::Duck),
            )
            .unwrap();
        state.set_overlay_bounds(bounds);
        let rect = (
            10.0 * scale,
            bounds.1 * 0.65,
            bounds.0 - 20.0 * scale,
            bounds.1 * 0.3,
        );
        let mut data = MsegData::default();
        assert!(state.open_curve_browser(rect, scale));
        for _ in 1..state.browser_pages().len() {
            state.on_key(&key(Key::Tab), &mut data, rect, scale);
        }
        if !state.curve_browser_open() {
            assert!(state.status_message().contains("Enlarge"));
            assert_eq!(bounds, (300.0, 260.0));
        } else {
            let layout = state.browser_layout(rect, scale).unwrap();
            assert_eq!(layout.buttons.len(), 6);
            assert!(layout.pager.is_some());
            for r in layout
                .tabs
                .iter()
                .chain(layout.cards.iter())
                .chain(layout.buttons.iter())
            {
                assert!(r.0 >= layout.panel.0 && r.1 >= layout.panel.1);
                assert!(r.0 + r.2 <= layout.panel.0 + layout.panel.2 + 0.01);
                assert!(r.1 + r.3 <= layout.panel.1 + layout.panel.3 + 0.01);
            }
            let mut pm = Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
            let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
            for value in [
                Key::F2,
                Key::Escape,
                Key::Delete,
                Key::Escape,
                Key::Character("s".into()),
            ] {
                state.on_key(&key(value), &mut data, rect, scale);
                state.draw_curve_browser(&mut pm, &mut tr, rect, scale);
            }
            assert!(state.browser.user.dialog.is_some());
        }
        assert_eq!(library.user_presets().unwrap(), [preset]);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn navigation_and_drawing_use_the_snapshot_until_explicit_refresh() {
    let (mut state, library, root) = setup();
    for i in 0..9 {
        library
            .save_user_preset(&format!("Curve {i}"), &factory_shape(FactoryShape::Duck))
            .unwrap();
    }
    let mut data = MsegData::default();
    user(&mut state, &mut data);
    let hidden = root.with_extension("hidden");
    std::fs::rename(&root, &hidden).unwrap();
    std::fs::write(&root, "unavailable").unwrap();
    let mut pm = Pixmap::new(1040, 720).unwrap();
    let mut tr = TextRenderer::new(crate::DEFAULT_FONT);
    for value in [Key::ArrowRight, Key::PageDown, Key::Home, Key::End] {
        state.on_key(&key(value), &mut data, RECT, 1.0);
        state.draw_curve_browser(&mut pm, &mut tr, RECT, 1.0);
        assert_eq!(state.browser.user.entries.len(), 9);
        assert!(state.browser.user.error.is_none());
    }
    state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
    assert!(state.browser.user.error.is_some());
    assert!(state.browser.user.entries.is_empty());
    std::fs::remove_file(&root).unwrap();
    std::fs::rename(hidden, &root).unwrap();
    state.on_key(&key(Key::Character("r".into())), &mut data, RECT, 1.0);
    assert_eq!(state.browser.user.entries.len(), 9);
    assert!(state.browser.user.error.is_none());
    std::fs::remove_dir_all(root).unwrap();
}
