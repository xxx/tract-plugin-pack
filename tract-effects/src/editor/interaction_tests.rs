use super::*;
use baseview::{Event, MouseButton, MouseEvent, Point};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};

type Rect = (f32, f32, f32, f32);

struct Harness {
    editor: EffectEditor,
    effect: TrackEffect,
    modulation: TrackModulation,
    tr: widgets::TextRenderer,
    geometry: EditorGeometry,
    pointer: (f32, f32),
}

impl Harness {
    fn new() -> Self {
        let kind = EffectKind::Svf;
        Self {
            editor: EffectEditor::default(),
            effect: TrackEffect {
                kind,
                params: effects::default_params_for_kind(kind),
                ..TrackEffect::default()
            },
            modulation: TrackModulation::default(),
            tr: widgets::TextRenderer::new(include_bytes!(
                "../../../tiny-skia-widgets/test_data/DejaVuSans.ttf"
            )),
            geometry: EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), 1.0),
                scale: 1.0,
                window_size: (1336.0, 680.0),
            },
            pointer: (0.0, 0.0),
        }
    }

    fn send(&mut self, event: Event) -> EditorResponse {
        if let Event::Mouse(MouseEvent::CursorMoved { position, .. }) = &event {
            self.pointer = (position.x as f32, position.y as f32);
        }
        self.editor.handle_event(
            &event,
            EditorContext {
                pointer_position: self.pointer,
                effect: &mut self.effect,
                modulation: &mut self.modulation,
                text_renderer: &mut self.tr,
                geometry: self.geometry,
            },
        )
    }

    fn move_to(&mut self, x: f32, y: f32, modifiers: Modifiers) -> EditorResponse {
        self.send(Event::Mouse(MouseEvent::CursorMoved {
            position: Point::new(x as f64, y as f64),
            modifiers,
        }))
    }

    fn press(&mut self, button: MouseButton) -> EditorResponse {
        self.send(Event::Mouse(MouseEvent::ButtonPressed {
            button,
            modifiers: Modifiers::empty(),
        }))
    }

    fn release(&mut self) -> EditorResponse {
        self.send(Event::Mouse(MouseEvent::ButtonReleased {
            button: MouseButton::Left,
            modifiers: Modifiers::empty(),
        }))
    }

    fn click(&mut self, rect: Rect) -> EditorResponse {
        self.move_to(
            rect.0 + rect.2 / 2.0,
            rect.1 + rect.3 / 2.0,
            Modifiers::empty(),
        );
        let result = self.press(MouseButton::Left);
        self.release();
        result
    }

    fn key(&mut self, key: Key) -> EditorResponse {
        self.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Down,
            key,
            ..KeyboardEvent::default()
        }))
    }

    fn type_replacement(&mut self, text: &str) {
        for _ in 0..16 {
            self.key(Key::Backspace);
        }
        self.key(Key::Character(text.into()));
    }
}

#[test]
fn continuous_dial_drag_changes_value_and_release_ends_gesture() {
    let mut h = Harness::new();
    let rect = h.geometry.layout.dials[0];
    let before = h.effect.params[0];
    h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Left);
    assert!(h.move_to(rect.0 + 30.0, rect.1, Modifiers::empty()).changed);
    assert!(h.effect.params[0] > before);
    h.release();
    let after = h.effect;
    h.move_to(rect.0 + 30.0, rect.1 - 30.0, Modifiers::empty());
    assert_eq!(h.effect, after);
}

#[test]
fn shift_drag_has_finer_resolution_than_normal_drag() {
    fn drag(modifiers: Modifiers) -> f32 {
        let mut h = Harness::new();
        let r = h.geometry.layout.dials[0];
        h.move_to(r.0 + 30.0, r.1 + 60.0, Modifiers::empty());
        h.press(MouseButton::Left);
        h.move_to(r.0 + 30.0, r.1 + 50.0, modifiers);
        h.move_to(r.0 + 30.0, r.1 + 20.0, modifiers);
        h.effect.params[0]
    }
    assert!(drag(Modifiers::SHIFT) < drag(Modifiers::empty()));
}

#[test]
fn text_entry_commits_parsed_value_and_captures_key_releases() {
    let mut h = Harness::new();
    let r = h.geometry.layout.dials[0];
    h.move_to(r.0 + 30.0, r.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("800");
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.effect.params[0], 800.0);
    h.press(MouseButton::Right);
    assert!(
        h.send(Event::Keyboard(KeyboardEvent {
            state: KeyState::Up,
            key: Key::Enter,
            ..KeyboardEvent::default()
        }))
        .captured
    );
    h.key(Key::Escape);
    assert_eq!(h.effect.params[0], 800.0);
}

#[test]
fn mix_is_selectable_as_an_mseg_target_without_editing_the_base_mix() {
    let mut h = Harness::new();
    let rect = h.geometry.layout.target;
    h.move_to(rect.0 + 20.0, rect.1 + 15.0, Modifiers::empty());
    h.press(MouseButton::Left);
    h.key(Key::End);
    h.key(Key::Enter);
    assert_eq!(
        h.modulation.targets[0],
        Some(crate::modulation::ModulationTarget::Mix)
    );
    assert_eq!(h.effect.mix, 1.0);
}

#[test]
fn click_outside_commits_mix_text_without_consuming_host_control() {
    let mut h = Harness::new();
    let r = h.geometry.layout.mix;
    h.move_to(r.0 + 30.0, r.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("25");
    h.move_to(0.0, 0.0, Modifiers::empty());
    let response = h.press(MouseButton::Left);
    assert!(response.changed);
    assert!(!response.captured);
    assert_eq!(h.effect.mix, 0.25);
}

#[test]
fn enum_dropdown_changes_discrete_value_and_right_click_does_not_edit_it() {
    let mut h = Harness::new();
    let rect = h.geometry.layout.dials[2];
    h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
    assert!(!h.press(MouseButton::Right).captured);
    assert!(!h.editor.captures_keyboard());
    h.click(rect);
    assert!(h.editor.dropdown_open());
    h.key(Key::ArrowDown);
    assert!(h.key(Key::Enter).changed);
    assert_eq!(h.effect.params[2], 1.0);
}

#[test]
fn filtered_trigger_menu_maps_to_the_same_saved_trigger_variants() {
    let mut h = Harness::new();
    h.editor = EffectEditor::new(&[
        TriggerChoice::Free,
        TriggerChoice::FreeHz,
        TriggerChoice::Transient,
    ]);
    h.click(h.geometry.layout.trigger);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    assert_eq!(h.modulation.trigger, TriggerSource::FreeHz { hz: 1.0 });
    h.click(h.geometry.layout.trigger);
    h.key(Key::End);
    h.key(Key::Enter);
    assert!(matches!(
        h.modulation.trigger,
        TriggerSource::Transient { .. }
    ));
}

#[test]
fn mseg_edit_and_delete_change_only_the_selected_curve() {
    let mut h = Harness::new();
    let original = h.modulation.clone();
    let r = h.geometry.layout.mseg_pane;
    h.click((r.0 + r.2 * 0.5, r.1 + r.3 * 0.4, 1.0, 1.0));
    h.click((r.0 + r.2 * 0.5, r.1 + r.3 * 0.4, 1.0, 1.0));
    assert!(h.modulation.msegs[1].node_count > original.msegs[1].node_count);
    for slot in [0, 2, 3] {
        assert_eq!(h.modulation.msegs[slot], original.msegs[slot]);
    }
    let plot = widgets::mseg::mseg_layout(r, false, 1.0);
    let node = original.msegs[1].nodes[1];
    h.click((
        widgets::mseg::phase_to_x(&plot, node.time),
        widgets::mseg::value_to_y(&plot, node.value),
        1.0,
        1.0,
    ));
    assert!(h.key(Key::Delete).changed);
    assert_eq!(
        h.modulation.msegs[1].node_count,
        original.msegs[1].node_count
    );
}

#[test]
fn navigation_and_randomize_are_host_requests_without_config_changes() {
    let mut h = Harness::new();
    let before = h.effect;
    assert!(h.click(h.geometry.layout.randomize).randomize);
    assert!(h.click(h.geometry.layout.back).back);
    assert_eq!(h.effect, before);
}

#[test]
fn translated_layout_routes_edits_to_the_same_parameter() {
    let mut h = Harness::new();
    h.geometry.layout = view::effect_layout((216.0, 116.0, 1304.0, 640.0), 1.0);
    let rect = h.geometry.layout.mix;
    h.click(rect);
    h.effect.mix = 0.2;
    assert!(h.click(rect).changed);
    assert_eq!(h.effect.mix, 1.0);
}

#[test]
fn overflowing_mix_text_is_rejected_without_changing_the_base_value() {
    let mut h = Harness::new();
    h.effect.mix = 0.42;
    let rect = h.geometry.layout.mix;
    h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
    h.press(MouseButton::Right);
    h.type_replacement("1e100");
    assert!(!h.key(Key::Enter).changed);
    assert_eq!(h.effect.mix, 0.42);
}

#[test]
fn canceled_interaction_rebases_a_dial_drag_without_another_pointer_move() {
    fn drag(cancel: bool) -> f32 {
        let mut h = Harness::new();
        let rect = h.geometry.layout.dials[0];
        h.move_to(rect.0 + 30.0, rect.1 + 50.0, Modifiers::empty());
        if cancel {
            h.editor.cancel_interaction();
        }
        h.press(MouseButton::Left);
        h.move_to(rect.0 + 30.0, rect.1 + 49.0, Modifiers::empty());
        h.effect.params[0]
    }
    assert_eq!(drag(true), drag(false));
}

mod event_dispatch {
    use super::*;
    use widgets::dropdown::{DropdownPopupLayout, RowKind, dropdown_popup_layout};

    fn popup(h: &Harness) -> DropdownPopupLayout {
        let items = view::kind_items().items;
        dropdown_popup_layout(&h.editor.effect_dropdown, &items, h.geometry.window_size).unwrap()
    }

    fn open_kind_menu(h: &mut Harness) {
        h.click(h.geometry.layout.kind);
        assert!(h.editor.effect_dropdown.is_open_for(EffectAction::Kind));
    }

    fn key_event(h: &mut Harness, key: Key, state: KeyState) -> EditorResponse {
        h.send(Event::Keyboard(KeyboardEvent {
            key,
            state,
            ..KeyboardEvent::default()
        }))
    }

    fn select_node(h: &mut Harness, slot: usize, node_index: usize) -> usize {
        h.editor.selected_mseg = slot;
        h.editor.mseg_edit = widgets::mseg::MsegEditState::new();
        let index = slot.min(3);
        h.modulation.msegs[index] = widgets::MsegData {
            snap: false,
            ..Default::default()
        };
        h.modulation.msegs[index].insert_node(0.4, 0.35).unwrap();
        let node = h.modulation.msegs[index].nodes[node_index];
        let layout =
            widgets::mseg::mseg_layout(h.geometry.layout.mseg_pane, false, h.geometry.scale);
        h.move_to(
            widgets::mseg::phase_to_x(&layout, node.time),
            widgets::mseg::value_to_y(&layout, node.value),
            Modifiers::empty(),
        );
        h.editor.mseg_last_click_pos = (-999.0, -999.0);
        assert!(h.press(MouseButton::Left).captured);
        let release = h.release();
        assert!(!release.changed);
        assert!(!release.captured);
        assert!(h.editor.mseg_edit.is_node_selected(node_index));
        index
    }

    #[test]
    fn popup_navigation_keys_select_the_expected_registry_item() {
        let last = EffectKind::ALL.len() - 1;
        for (keys, index) in [
            (vec![Key::Home], 0),
            (vec![Key::End], last),
            (vec![Key::Home, Key::ArrowDown], 1),
            (vec![Key::End, Key::ArrowUp], last - 1),
            (vec![Key::Home, Key::PageDown], 11),
            (vec![Key::End, Key::PageUp], last - 11),
        ] {
            let mut h = Harness::new();
            let modulation = h.modulation.clone();
            open_kind_menu(&mut h);
            for key in keys {
                let response = h.key(key);
                assert!(response.captured);
                assert!(!response.changed);
            }
            let response = h.key(Key::Enter);
            assert!(response.captured);
            assert!(response.changed);
            assert!(!h.editor.dropdown_open());
            assert_eq!(h.effect.kind, EffectKind::ALL[index]);
            assert_eq!(
                h.effect.params,
                effects::default_params_for_kind(EffectKind::ALL[index])
            );
            assert_eq!(h.modulation.msegs, modulation.msegs);
            assert_eq!(h.modulation.depths, modulation.depths);
            assert_eq!(h.modulation.trigger, modulation.trigger);
        }
    }

    #[test]
    fn popup_key_releases_are_captured_without_navigation_or_dismissal() {
        let mut h = Harness::new();
        let original = h.effect;
        open_kind_menu(&mut h);
        for key in [
            Key::ArrowDown,
            Key::Enter,
            Key::Escape,
            Key::Backspace,
            Key::Character("svf".into()),
        ] {
            let response = key_event(&mut h, key, KeyState::Up);
            assert!(response.captured);
            assert!(!response.changed);
            assert!(h.editor.dropdown_open());
            assert_eq!(h.effect, original);
        }
        assert!(h.key(Key::Enter).changed);
        assert_eq!(h.effect, original);
        assert!(!h.editor.dropdown_open());
        open_kind_menu(&mut h);
        let response = h.key(Key::Escape);
        assert!(response.captured);
        assert!(!response.changed);
        assert!(!h.editor.captures_keyboard());
        assert_eq!(h.effect, original);
    }

    #[test]
    fn popup_filter_backspace_restores_a_match_after_a_multi_character_paste() {
        let mut h = Harness::new();
        open_kind_menu(&mut h);
        assert!(h.key(Key::Character("svfx".into())).captured);
        assert!(popup(&h).visible_rows.is_empty());
        assert!(h.key(Key::Backspace).captured);
        let kinds: Vec<_> = popup(&h)
            .visible_rows
            .iter()
            .filter(|row| row.kind == RowKind::Item)
            .map(|row| EffectKind::ALL[row.item_index])
            .collect();
        assert_eq!(kinds, [EffectKind::Svf]);
        let response = h.key(Key::Enter);
        assert!(response.captured);
        assert!(response.changed);
        assert_eq!(h.effect.kind, EffectKind::Svf);
        assert!(!h.editor.dropdown_open());
    }

    #[test]
    fn pixel_and_line_wheels_scroll_equally_and_closed_menus_ignore_wheels() {
        for delta in [
            baseview::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            baseview::ScrollDelta::Pixels { x: 0.0, y: -20.0 },
        ] {
            let mut h = Harness::new();
            let effect = h.effect;
            let modulation = h.modulation.clone();
            let event = Event::Mouse(MouseEvent::WheelScrolled {
                delta,
                modifiers: Modifiers::empty(),
            });
            let ignored = h.send(event.clone());
            assert!(!ignored.captured);
            assert!(!ignored.changed);
            open_kind_menu(&mut h);
            h.key(Key::Home);
            let initial = popup(&h);
            let before_y = initial
                .visible_rows
                .iter()
                .find(|row| row.kind == RowKind::Item && row.item_index == 0)
                .unwrap()
                .rect
                .1;
            let response = h.send(event);
            assert!(response.captured);
            assert!(!response.changed);
            let after = popup(&h);
            let after_y = after
                .visible_rows
                .iter()
                .find(|row| row.kind == RowKind::Item && row.item_index == 0)
                .map(|row| row.rect.1);
            assert_eq!(after_y, Some(before_y - 32.0));
            assert_eq!(h.effect, effect);
            assert_eq!(h.modulation, modulation);
        }
    }

    #[test]
    fn text_paste_backspace_and_enter_commit_to_the_active_control() {
        for (hit, text, shortened) in [
            (EffectHit::Dial(0), "1250", "125"),
            (EffectHit::Mix, "12.5", "12."),
        ] {
            let mut h = Harness::new();
            let original = h.effect;
            let modulation = h.modulation.clone();
            h.editor.text_edit.begin(hit, "");
            assert!(h.key(Key::Character(text.into())).captured);
            assert_eq!(h.editor.text_edit.active_for(&hit), Some(text));
            assert_eq!(h.effect, original);
            assert!(h.key(Key::Backspace).captured);
            assert_eq!(h.editor.text_edit.active_for(&hit), Some(shortened));
            let response = h.key(Key::Enter);
            assert!(response.captured);
            assert!(response.changed);
            let mut expected = original;
            match hit {
                EffectHit::Dial(0) => expected.params[0] = 125.0,
                EffectHit::Mix => expected.mix = 0.12,
                _ => unreachable!(),
            }
            assert_eq!(h.effect, expected);
            assert_eq!(h.modulation, modulation);
            assert!(!h.editor.captures_keyboard());
        }
    }

    #[test]
    fn text_key_releases_and_unhandled_keys_preserve_the_pending_edit() {
        let mut h = Harness::new();
        let effect = h.effect;
        let modulation = h.modulation.clone();
        h.editor.text_edit.begin(EffectHit::Mix, "25");
        for key in [
            Key::Character("999".into()),
            Key::Backspace,
            Key::Enter,
            Key::Escape,
        ] {
            let response = key_event(&mut h, key, KeyState::Up);
            assert!(response.captured);
            assert!(!response.changed);
            assert_eq!(h.editor.text_edit.active_for(&EffectHit::Mix), Some("25"));
        }
        let response = h.key(Key::Tab);
        assert!(!response.captured);
        assert!(!response.changed);
        assert_eq!(h.editor.text_edit.active_for(&EffectHit::Mix), Some("25"));
        let response = h.key(Key::Escape);
        assert!(response.captured);
        assert!(!response.changed);
        assert!(!h.editor.captures_keyboard());
        assert_eq!(h.effect, effect);
        assert_eq!(h.modulation, modulation);
        assert!(!h.key(Key::Enter).captured);
    }

    #[test]
    fn popup_keyboard_focus_takes_priority_over_a_pending_text_edit() {
        let mut h = Harness::new();
        let original = h.effect;
        open_kind_menu(&mut h);
        h.editor.text_edit.begin(EffectHit::Mix, "25");
        let response = h.key(Key::Escape);
        assert!(response.captured);
        assert!(!response.changed);
        assert!(!h.editor.dropdown_open());
        assert_eq!(h.editor.text_edit.active_for(&EffectHit::Mix), Some("25"));
        assert_eq!(h.effect, original);
        assert!(h.key(Key::Enter).changed);
        assert_eq!(
            h.effect,
            TrackEffect {
                mix: 0.25,
                ..original
            }
        );
    }

    #[test]
    fn delete_and_backspace_remove_only_selected_interior_nodes_on_key_down() {
        for key in [Key::Delete, Key::Backspace] {
            for slot in [0, 1, 2, 3, usize::MAX] {
                let mut h = Harness::new();
                let index = select_node(&mut h, slot, 1);
                let effect = h.effect;
                let mut expected = h.modulation.clone();
                let response = h.key(key.clone());
                assert!(response.captured);
                assert!(response.changed);
                assert_eq!(h.modulation.msegs[index].node_count, 2);
                let nodes = h.modulation.msegs[index].active();
                assert_eq!(
                    (nodes[0].time, nodes[0].value, nodes[1].time, nodes[1].value),
                    (0.0, 0.0, 1.0, 1.0)
                );
                expected.msegs[index] = h.modulation.msegs[index];
                assert_eq!(h.modulation, expected);
                assert_eq!(h.effect, effect);
                assert_eq!(h.editor.mseg_edit.selection_count(), 0);
            }
        }
    }

    #[test]
    fn key_up_and_unrelated_keys_do_not_delete_selected_nodes() {
        let mut h = Harness::new();
        select_node(&mut h, 1, 1);
        let effect = h.effect;
        let modulation = h.modulation.clone();
        for (key, state) in [
            (Key::Delete, KeyState::Up),
            (Key::Backspace, KeyState::Up),
            (Key::Enter, KeyState::Down),
            (Key::Character("x".into()), KeyState::Down),
            (Key::ArrowLeft, KeyState::Down),
        ] {
            let response = key_event(&mut h, key, state);
            assert!(!response.captured);
            assert!(!response.changed);
            assert_eq!(h.effect, effect);
            assert_eq!(h.modulation, modulation);
            assert!(h.editor.mseg_edit.is_node_selected(1));
        }
    }

    #[test]
    fn delete_keys_report_no_edit_for_empty_selection_or_pinned_endpoints() {
        for selected in [None, Some(0), Some(2)] {
            let mut h = Harness::new();
            select_node(&mut h, 1, selected.unwrap_or(1));
            if selected.is_none() {
                h.editor.clear_selection();
            }
            let effect = h.effect;
            let modulation = h.modulation.clone();
            for key in [Key::Delete, Key::Backspace] {
                let response = h.key(key);
                assert!(!response.captured);
                assert!(!response.changed);
                assert_eq!(h.effect, effect);
                assert_eq!(h.modulation, modulation);
            }
        }
    }

    #[test]
    fn mseg_motion_reports_edits_and_release_stops_the_gesture_without_another_edit() {
        for slot in [0, 1, 2, 3, usize::MAX] {
            let mut h = Harness::new();
            let index = select_node(&mut h, slot, 1);
            let layout =
                widgets::mseg::mseg_layout(h.geometry.layout.mseg_pane, false, h.geometry.scale);
            let node = h.modulation.msegs[index].nodes[1];
            h.move_to(
                widgets::mseg::phase_to_x(&layout, node.time),
                widgets::mseg::value_to_y(&layout, node.value),
                Modifiers::empty(),
            );
            h.editor.mseg_last_click_pos = (-999.0, -999.0);
            h.press(MouseButton::Left);
            let response = h.move_to(
                widgets::mseg::phase_to_x(&layout, 0.6),
                widgets::mseg::value_to_y(&layout, 0.7),
                Modifiers::empty(),
            );
            assert!(response.changed);
            assert!(!response.captured);
            assert!((h.modulation.msegs[index].nodes[1].time - 0.6).abs() < 1e-5);
            assert!((h.modulation.msegs[index].nodes[1].value - 0.7).abs() < 1e-5);
            let response = h.release();
            assert!(!response.changed);
            assert!(!response.captured);
            let after = h.modulation.clone();
            let response = h.move_to(
                widgets::mseg::phase_to_x(&layout, 0.8),
                widgets::mseg::value_to_y(&layout, 0.2),
                Modifiers::empty(),
            );
            assert!(!response.changed);
            assert_eq!(h.modulation, after);
        }
    }

    #[test]
    fn popup_pointer_motion_changes_the_highlight_used_by_enter() {
        let mut h = Harness::new();
        open_kind_menu(&mut h);
        h.key(Key::Home);
        let row = popup(&h)
            .visible_rows
            .into_iter()
            .find(|row| row.kind == RowKind::Item && row.item_index == 2)
            .unwrap();
        let response = h.move_to(
            row.rect.0 + row.rect.2 / 2.0,
            row.rect.1 + row.rect.3 / 2.0,
            Modifiers::empty(),
        );
        assert!(!response.changed);
        assert_eq!(h.editor.mouse_pos, h.pointer);
        let response = h.key(Key::Enter);
        assert!(response.captured);
        assert!(response.changed);
        assert_eq!(h.effect.kind, EffectKind::ALL[2]);
    }

    #[test]
    fn popup_mouse_selection_applies_the_item_and_outside_clicks_are_consumed() {
        let mut h = Harness::new();
        open_kind_menu(&mut h);
        h.key(Key::Home);
        let row = popup(&h)
            .visible_rows
            .into_iter()
            .find(|row| row.kind == RowKind::Item && row.item_index == 1)
            .unwrap();
        let response = h.click(row.rect);
        assert!(response.captured);
        assert!(response.changed);
        assert_eq!(h.effect.kind, EffectKind::ALL[1]);
        assert!(!h.editor.dropdown_open());
        open_kind_menu(&mut h);
        let before = h.effect;
        let response = h.click(h.geometry.layout.back);
        assert!(response.captured);
        assert!(!response.changed);
        assert!(!response.back);
        assert!(!h.editor.dropdown_open());
        assert_eq!(h.effect, before);
        assert!(h.click(h.geometry.layout.back).back);
    }
}
