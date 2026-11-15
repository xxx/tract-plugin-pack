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

    fn resize(&mut self, backing_scale: f32) {
        self.editor.prepare_resize(backing_scale);
        self.geometry = EditorGeometry {
            layout: view::effect_layout(
                (
                    16.0 * backing_scale,
                    16.0 * backing_scale,
                    1304.0 * backing_scale,
                    640.0 * backing_scale,
                ),
                backing_scale,
            ),
            scale: backing_scale,
            window_size: (1336.0 * backing_scale, 680.0 * backing_scale),
        };
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

mod mseg_popup_regressions {
    use super::*;

    fn open_overlapping_edit_menu(scale: f32) -> Harness {
        let mut h = Harness::new();
        h.geometry = EditorGeometry {
            layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
            scale,
            window_size: (1336.0 * scale, 680.0 * scale),
        };
        let curve = &mut h.modulation.msegs[h.editor.selected_mseg];
        widgets::mseg::factory_shape(widgets::mseg::FactoryShape::Triangle)
            .apply(curve, false)
            .unwrap();
        curve.snap = false;
        curve.nodes[1].time = 0.25;
        curve.nodes[1].value = 0.5;
        let layout = widgets::mseg::mseg_layout(h.geometry.layout.mseg_pane, false, scale);
        h.move_to(
            widgets::mseg::phase_to_x(&layout, 0.25),
            widgets::mseg::value_to_y(&layout, 0.5),
            Modifiers::empty(),
        );
        h.press(MouseButton::Left);
        h.release();
        h.press(MouseButton::Right);
        assert!(h.editor.mseg_edit.has_open_menu());
        h
    }

    fn popup_only(h: &mut Harness) -> tiny_skia::Pixmap {
        let (width, height) = h.geometry.window_size;
        let mut pm = tiny_skia::Pixmap::new(width as u32, height as u32).unwrap();
        widgets::mseg::draw_mseg_dropdown(
            &mut pm,
            &mut h.tr,
            &h.editor.mseg_edit,
            h.geometry.layout.mseg_pane,
        );
        pm
    }

    #[test]
    fn expanded_mseg_popup_renders_above_modulation_controls() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = open_overlapping_edit_menu(scale);
            let mut actual = popup_only(&mut h);
            actual.fill(widgets::color_bg());
            h.editor.draw(
                &mut actual,
                &mut h.tr,
                &h.effect,
                &h.modulation,
                [0.0; 4],
                h.geometry,
            );
            h.editor
                .draw_popup(&mut actual, &mut h.tr, &h.effect, h.geometry);
            let expected = popup_only(&mut h);
            let (x, y, w, height) = h.geometry.layout.mseg_selector;
            let mut compared = 0;
            for row in y.ceil() as usize..(y + height).floor() as usize {
                for col in x.ceil() as usize..(x + w).floor() as usize {
                    let offset = (row * actual.width() as usize + col) * 4;
                    if expected.data()[offset + 3] == 255 {
                        compared += 1;
                        assert_eq!(
                            &actual.data()[offset..offset + 4],
                            &expected.data()[offset..offset + 4],
                            "menu obscured at ({col}, {row}), scale {scale}"
                        );
                    }
                }
            }
            assert!(
                compared > 0,
                "menu must overlap the selector at scale {scale}"
            );
        }
    }

    #[test]
    fn right_click_on_mseg_popup_dismisses_it_without_editing_the_underlying_dial() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = open_overlapping_edit_menu(scale);
            let popup = popup_only(&mut h);
            let dial = h.geometry.layout.dials[1];
            let (x, y) = (dial.0 + dial.2 * 0.5, dial.1 + dial.3 - 8.0 * scale);
            let offset = (y as usize * popup.width() as usize + x as usize) * 4;
            assert_eq!(popup.data()[offset + 3], 255, "menu must cover the dial");
            let effect = h.effect;
            let modulation = h.modulation.clone();
            h.move_to(x, y, Modifiers::empty());
            let response = h.press(MouseButton::Right);
            assert!(response.captured);
            assert!(!response.changed);
            assert!(!h.editor.mseg_edit.has_open_menu());
            assert!(!h.editor.text_edit.is_active());
            assert!(!h.key(Key::Character("999".into())).captured);
            assert!(!h.key(Key::Enter).captured);
            assert_eq!(h.effect, effect);
            assert_eq!(h.modulation, modulation);
        }
    }
}

#[test]
fn mseg_menu_captures_keyboard_wheel_and_clicks_outside_the_curve_pane() {
    let mut h = Harness::new();
    let slot = h.editor.selected_mseg;
    h.modulation.msegs[slot].snap = false;
    let before = h.modulation.clone();
    let command = Event::Keyboard(KeyboardEvent {
        key: Key::Character("a".into()),
        state: KeyState::Down,
        modifiers: Modifiers::CONTROL,
        ..KeyboardEvent::default()
    });
    assert!(h.send(command).captured);
    let layout = widgets::mseg::mseg_layout(h.geometry.layout.mseg_pane, false, 1.0);
    let node = h.modulation.msegs[slot].nodes[1];
    h.move_to(
        widgets::mseg::phase_to_x(&layout, node.time),
        widgets::mseg::value_to_y(&layout, node.value),
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    assert!(h.editor.captures_keyboard());
    assert!(
        h.send(Event::Mouse(MouseEvent::WheelScrolled {
            delta: baseview::ScrollDelta::Lines { x: 0.0, y: -2.0 },
            modifiers: Modifiers::empty()
        }))
        .captured
    );
    h.key(Key::Home);
    for _ in 0..7 {
        h.key(Key::ArrowDown);
    }
    assert!(h.key(Key::Enter).changed);
    assert!(!h.editor.captures_keyboard());
    for (old, new) in before.msegs[slot]
        .active()
        .iter()
        .zip(h.modulation.msegs[slot].active())
    {
        assert_eq!(new.value, 1.0 - old.value);
    }
    let node = h.modulation.msegs[slot].nodes[1];
    h.move_to(
        widgets::mseg::phase_to_x(&layout, node.time),
        widgets::mseg::value_to_y(&layout, node.value),
        Modifiers::empty(),
    );
    h.press(MouseButton::Right);
    assert!(h.editor.captures_keyboard());
    let dial = h.geometry.layout.dials[0];
    h.click(dial);
    assert!(!h.editor.captures_keyboard());
    assert!(h.editor.effect_dial_drag.active_action().is_none());
}

#[test]
fn curve_shortcuts_share_shapes_across_editors_and_paste_is_one_undo_entry() {
    let root = std::env::temp_dir().join(format!("tract-effect-copy-{}", std::process::id()));
    let mut source = Harness::new();
    let mut destination = Harness::new();
    source
        .editor
        .mseg_edit
        .set_curve_library_directory(root.clone());
    destination
        .editor
        .mseg_edit
        .set_curve_library_directory(root.clone());
    let slot = destination.editor.selected_mseg;
    widgets::mseg::factory_shape(widgets::mseg::FactoryShape::RampDown)
        .apply(&mut destination.modulation.msegs[slot], false)
        .unwrap();
    destination.modulation.msegs[slot].beats = 13.0;
    destination.modulation.msegs[slot].time_divisions = 12;
    let before = destination.modulation.clone();
    let shortcut = |key: &str| {
        Event::Keyboard(KeyboardEvent {
            key: Key::Character(key.into()),
            state: KeyState::Down,
            modifiers: Modifiers::CONTROL,
            ..KeyboardEvent::default()
        })
    };
    assert!(source.send(shortcut("c")).captured);
    let mut undo = crate::undo::UndoHistory::new();
    undo.begin_capture(before.clone());
    let response = destination.send(shortcut("v"));
    assert!(response.captured && response.changed);
    let after = destination.modulation.clone();
    undo.commit_capture(&after);
    assert_eq!(
        after.msegs[slot].active(),
        source.modulation.msegs[source.editor.selected_mseg].active()
    );
    assert_eq!(
        (after.msegs[slot].beats, after.msegs[slot].time_divisions),
        (13.0, 12)
    );
    assert_eq!(after.depths, before.depths);
    assert_eq!(after.targets, before.targets);
    assert_eq!(undo.undo(after.clone()), Some(before.clone()));
    assert!(undo.undo(before.clone()).is_none());
    assert_eq!(undo.redo(before), Some(after));
    std::fs::remove_dir_all(root).unwrap();
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
    fn pixel_and_line_wheels_scroll_equally_in_logical_points_at_each_backing_scale() {
        for backing_scale in [1.0, 1.5, 2.0] {
            for delta in [
                baseview::ScrollDelta::Lines { x: 0.0, y: -1.0 },
                baseview::ScrollDelta::Pixels { x: 0.0, y: -20.0 },
            ] {
                let mut h = Harness::new();
                h.resize(backing_scale);
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
                assert_eq!(after_y, Some(before_y - 32.0 * backing_scale));
                assert_eq!(h.effect, effect);
                assert_eq!(h.modulation, modulation);
            }
        }
    }

    #[test]
    fn resize_ends_dial_drags_and_clears_click_history_before_the_next_press() {
        for backing_scale in [1.0, 1.5, 2.0] {
            let mut h = Harness::new();
            h.effect.params[0] = 800.0;
            let dial = h.geometry.layout.dials[0];
            h.move_to(dial.0 + 30.0, dial.1 + 30.0, Modifiers::empty());
            h.press(MouseButton::Left);
            assert_eq!(
                h.editor.effect_dial_drag.active_action(),
                Some(&EffectHit::Dial(0))
            );
            let effect = h.effect;
            h.resize(backing_scale);
            assert_eq!(h.editor.effect_dial_drag.active_action(), None);
            assert!(!h.move_to(0.0, 0.0, Modifiers::empty()).changed);
            assert_eq!(h.effect, effect);
            // A press after resizing starts a new gesture instead of resetting the dial.
            h.click(h.geometry.layout.dials[0]);
            assert_eq!(h.effect, effect);
        }
    }

    #[test]
    fn resize_dismisses_popup_and_text_focus_without_editing_the_document() {
        for text_entry in [false, true] {
            let mut h = Harness::new();
            if text_entry {
                let dial = h.geometry.layout.dials[0];
                h.move_to(dial.0 + 30.0, dial.1 + 30.0, Modifiers::empty());
                h.press(MouseButton::Right);
                h.type_replacement("800");
            } else {
                open_kind_menu(&mut h);
            }
            assert!(h.editor.captures_keyboard());
            let effect = h.effect;
            let modulation = h.modulation.clone();
            h.resize(2.0);
            assert!(!h.editor.captures_keyboard());
            assert!(!h.editor.dropdown_open());
            assert!(!h.key(Key::Enter).changed);
            assert_eq!(h.effect, effect);
            assert_eq!(h.modulation, modulation);
        }
    }

    #[test]
    fn resize_ends_mseg_drags_without_discarding_selection_or_strip_preferences() {
        let mut h = Harness::new();
        h.editor = EffectEditor::new(&[TriggerChoice::FreeHz, TriggerChoice::Transient]);
        select_node(&mut h, 3, 1);
        h.editor.mseg_edit.set_grid_options(&[(5, 7), (9, 11)]);
        h.editor
            .mseg_edit
            .set_style(widgets::mseg::randomize::RandomStyle::Spiky);
        h.editor.mseg_edit.set_show_polarity(false);
        h.editor.mseg_last_click_pos = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        h.press(MouseButton::Left);
        let effect = h.effect;
        let modulation = h.modulation.clone();
        h.resize(2.0);
        assert!(!h.move_to(700.0, 400.0, Modifiers::empty()).changed);
        assert!(!h.release().changed);
        assert_eq!(h.effect, effect);
        assert_eq!(h.modulation, modulation);
        assert_eq!(h.editor.selected_mseg, 3);
        assert!(h.editor.mseg_edit.is_node_selected(1));
        assert_eq!(h.editor.mseg_edit.grid_options(), &[(5, 7), (9, 11)]);
        assert_eq!(
            h.editor.mseg_edit.style(),
            widgets::mseg::randomize::RandomStyle::Spiky
        );
        assert!(!h.editor.mseg_edit.show_polarity());
        assert_eq!(
            h.editor.trigger_choices,
            &[TriggerChoice::FreeHz, TriggerChoice::Transient]
        );
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

mod precision_editing {
    use super::*;
    fn shortcut(key: Key, modifiers: Modifiers) -> Event {
        Event::Keyboard(KeyboardEvent {
            key,
            modifiers,
            state: KeyState::Down,
            ..KeyboardEvent::default()
        })
    }
    fn select_point(h: &mut Harness, index: usize, add: bool) {
        let node = h.modulation.msegs[h.editor.selected_mseg].nodes[index];
        let layout = h
            .editor
            .mseg_edit
            .layout(h.geometry.layout.mseg_pane, h.geometry.scale);
        h.move_to(
            widgets::mseg::phase_to_x(&layout, node.time),
            widgets::mseg::value_to_y(&layout, node.value),
            Modifiers::empty(),
        );
        h.send(Event::Mouse(MouseEvent::ButtonPressed {
            button: MouseButton::Left,
            modifiers: if add {
                Modifiers::CONTROL
            } else {
                Modifiers::empty()
            },
        }));
        h.release();
    }
    fn draw(h: &mut Harness) {
        let mut pm = tiny_skia::Pixmap::new(
            h.geometry.window_size.0 as u32,
            h.geometry.window_size.1 as u32,
        )
        .unwrap();
        h.editor.draw(
            &mut pm,
            &mut h.tr,
            &h.effect,
            &h.modulation,
            [0.0; 4],
            h.geometry,
        );
        h.editor
            .draw_popup(&mut pm, &mut h.tr, &h.effect, h.geometry);
    }

    #[test]
    fn pan_start_commits_numeric_text_and_escape_cancels_the_pan() {
        for scale in [0.5, 1.0, 2.0] {
            for mix in [false, true] {
                let mut h = Harness::new();
                h.geometry = EditorGeometry {
                    layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                    scale,
                    window_size: (1336.0 * scale, 680.0 * scale),
                };
                let layout = h
                    .editor
                    .mseg_edit
                    .layout(h.geometry.layout.mseg_pane, scale);
                let (x, y) = (
                    widgets::mseg::phase_to_x(&layout, 0.5),
                    widgets::mseg::value_to_y(&layout, 0.5),
                );
                h.move_to(x, y, Modifiers::empty());
                h.send(Event::Mouse(MouseEvent::WheelScrolled {
                    delta: baseview::ScrollDelta::Lines { x: 0.0, y: 6.0 },
                    modifiers: Modifiers::empty(),
                }));
                let view = h.editor.mseg_edit.viewport();
                let dial = if mix {
                    h.geometry.layout.mix
                } else {
                    h.geometry.layout.dials[0]
                };
                h.move_to(
                    dial.0 + dial.2 * 0.5,
                    dial.1 + dial.3 * 0.5,
                    Modifiers::empty(),
                );
                h.press(MouseButton::Right);
                h.type_replacement(if mix { "25" } else { "800" });
                let modulation = h.modulation.clone();
                let before = h.effect;
                let mut undo = crate::undo::UndoHistory::new();
                undo.begin_capture(before);
                h.move_to(x, y, Modifiers::empty());
                let response = h.press(MouseButton::Middle);
                assert!(response.captured && response.changed);
                assert!(!h.editor.text_edit.is_active());
                assert!(h.editor.mseg_edit.is_panning());
                let committed = h.effect;
                undo.commit_capture(&committed);
                assert_eq!(
                    if mix {
                        committed.mix
                    } else {
                        committed.params[0]
                    },
                    if mix { 0.25 } else { 800.0 }
                );
                h.move_to(x + 40.0 * scale, y + 20.0 * scale, Modifiers::empty());
                assert_ne!(h.editor.mseg_edit.viewport(), view);
                let response = h.key(Key::Escape);
                assert!(response.captured && !response.changed);
                assert!(!h.editor.mseg_edit.is_panning());
                assert_eq!(h.editor.mseg_edit.viewport(), view);
                assert_eq!(h.effect, committed);
                assert_eq!(h.modulation, modulation);
                assert_eq!(undo.undo(h.effect), Some(before));
                assert!(undo.undo(before).is_none());
            }
        }
    }

    #[test]
    fn pan_start_finalizes_invalid_numeric_text_without_changing_the_value() {
        let mut h = Harness::new();
        let dial = h.geometry.layout.mix;
        h.move_to(
            dial.0 + dial.2 * 0.5,
            dial.1 + dial.3 * 0.5,
            Modifiers::empty(),
        );
        h.press(MouseButton::Right);
        h.type_replacement("1e");
        let before = h.effect;
        let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
        h.move_to(
            widgets::mseg::phase_to_x(&layout, 0.5),
            widgets::mseg::value_to_y(&layout, 0.5),
            Modifiers::empty(),
        );
        let response = h.press(MouseButton::Middle);
        assert!(response.captured && !response.changed);
        assert!(!h.editor.text_edit.is_active());
        assert!(h.editor.mseg_edit.is_panning());
        assert!(h.key(Key::Escape).captured);
        assert!(!h.editor.mseg_edit.is_panning());
        assert_eq!(h.effect, before);
    }

    #[test]
    fn middle_press_outside_the_canvas_keeps_pending_numeric_input() {
        let mut h = Harness::new();
        let dial = h.geometry.layout.mix;
        h.move_to(
            dial.0 + dial.2 * 0.5,
            dial.1 + dial.3 * 0.5,
            Modifiers::empty(),
        );
        h.press(MouseButton::Right);
        h.type_replacement("25");
        let before = h.effect;
        h.move_to(0.0, 0.0, Modifiers::empty());
        let response = h.press(MouseButton::Middle);
        assert!(!response.captured && !response.changed);
        assert!(h.editor.text_edit.is_active());
        assert!(!h.editor.mseg_edit.is_panning());
        assert_eq!(h.effect, before);
    }

    #[test]
    fn dial_drag_inhibits_navigation_and_pan_blocks_control_presses() {
        let mut h = Harness::new();
        let dial = h.geometry.layout.dials[0];
        h.move_to(
            dial.0 + dial.2 * 0.5,
            dial.1 + dial.3 * 0.5,
            Modifiers::empty(),
        );
        h.press(MouseButton::Left);
        assert!(h.editor.effect_dial_drag.active_action().is_some());
        assert!(h.editor.pointer_gesture_active());
        let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
        let (x, y) = (
            widgets::mseg::phase_to_x(&layout, 0.5),
            widgets::mseg::value_to_y(&layout, 0.5),
        );
        h.move_to(x, y, Modifiers::empty());
        let view = h.editor.mseg_edit.viewport();
        h.send(Event::Mouse(MouseEvent::WheelScrolled {
            delta: baseview::ScrollDelta::Lines { x: 0.0, y: 6.0 },
            modifiers: Modifiers::empty(),
        }));
        h.press(MouseButton::Middle);
        assert_eq!(h.editor.mseg_edit.viewport(), view);
        assert!(!h.editor.mseg_edit.is_panning());
        h.release();
        assert!(!h.editor.pointer_gesture_active());
        h.send(Event::Mouse(MouseEvent::WheelScrolled {
            delta: baseview::ScrollDelta::Lines { x: 0.0, y: 6.0 },
            modifiers: Modifiers::empty(),
        }));
        h.press(MouseButton::Middle);
        assert!(h.editor.mseg_edit.is_panning());
        assert!(h.editor.pointer_gesture_active());
        h.move_to(
            dial.0 + dial.2 * 0.5,
            dial.1 + dial.3 * 0.5,
            Modifiers::empty(),
        );
        assert!(h.press(MouseButton::Left).captured);
        assert!(h.editor.effect_dial_drag.active_action().is_none());
        assert!(h.press(MouseButton::Right).captured);
        assert!(!h.editor.text_edit.is_active());
        h.send(Event::Mouse(MouseEvent::ButtonReleased {
            button: MouseButton::Middle,
            modifiers: Modifiers::empty(),
        }));
        assert!(!h.editor.mseg_edit.is_panning());
        assert!(!h.editor.pointer_gesture_active());
    }
    #[test]
    fn inspector_commit_is_one_undo_entry_and_readout_tracks_host_base_values() {
        let mut h = Harness::new();
        let slot = h.editor.selected_mseg;
        h.modulation.msegs[slot].sync_mode = widgets::mseg::SyncMode::Beat;
        h.modulation.msegs[slot].beats = 4.0;
        h.modulation.targets[slot - 1] = Some(ModulationTarget::Mix);
        h.modulation.depths[slot - 1] = 0.5;
        h.effect.mix = 0.0;
        select_point(&mut h, 1, false);
        assert!(h.send(shortcut(Key::Enter, Modifiers::CONTROL)).captured);
        assert!(h.editor.captures_keyboard());
        draw(&mut h);
        assert!(h.editor.mseg_edit.inspector_readout().ends_with("50 %"));
        h.effect.mix = 0.2;
        draw(&mut h);
        assert!(h.editor.mseg_edit.inspector_readout().ends_with("70 %"));
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        assert!(!h.key(Key::Character("1".into())).changed);
        assert_eq!(h.modulation, before);
        undo.begin_capture(before.clone());
        assert!(h.key(Key::Enter).changed);
        let after = h.modulation.clone();
        undo.commit_capture(&after);
        assert_eq!(after.msegs[slot].nodes[1].time, 0.25);
        assert_eq!(undo.undo(after), Some(before.clone()));
        assert!(undo.undo(before).is_none());
        h.key(Key::Escape);
        assert!(!h.editor.captures_keyboard());
    }
    #[test]
    fn wheel_and_middle_drag_update_only_the_view_and_cancel_cleanly() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.geometry = EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                scale,
                window_size: (1336.0 * scale, 680.0 * scale),
            };
            let before = h.modulation.clone();
            let layout = h
                .editor
                .mseg_edit
                .layout(h.geometry.layout.mseg_pane, scale);
            h.move_to(
                widgets::mseg::phase_to_x(&layout, 0.5),
                widgets::mseg::value_to_y(&layout, 0.5),
                Modifiers::empty(),
            );
            let response = h.send(Event::Mouse(MouseEvent::WheelScrolled {
                delta: baseview::ScrollDelta::Lines { x: 0.0, y: 6.0 },
                modifiers: Modifiers::empty(),
            }));
            assert!(response.captured && !response.changed);
            let zoom = h.editor.mseg_edit.viewport();
            assert_ne!(zoom, widgets::mseg::MsegViewport::default());
            assert!(h.press(MouseButton::Middle).captured);
            assert!(!h.move_to(0.0, 0.0, Modifiers::empty()).changed);
            assert_ne!(h.editor.mseg_edit.viewport(), zoom);
            assert!(!h.key(Key::Escape).changed);
            assert_eq!(h.editor.mseg_edit.viewport(), zoom);
            assert_eq!(h.modulation, before);
            assert!(h.press(MouseButton::Right).captured);
            assert_eq!(h.modulation, before);
        }
    }
    #[test]
    fn phrase_shortcuts_replace_only_the_selected_interval() {
        let root =
            std::env::temp_dir().join(format!("tract-precision-clip-{}", std::process::id()));
        let mut source = Harness::new();
        let mut target = Harness::new();
        for (h, value) in [(&mut source, 0.2), (&mut target, 0.7)] {
            let slot = h.editor.selected_mseg;
            let curve = &mut h.modulation.msegs[slot];
            *curve = widgets::mseg::MsegData::default();
            curve.snap = false;
            for phase in [0.25, 0.5, 0.75] {
                curve.insert_node(phase, value).unwrap();
            }
            h.editor.mseg_edit.set_curve_library_directory(root.clone());
            for (i, index) in [1, 2, 3].into_iter().enumerate() {
                select_point(h, index, i != 0);
            }
        }
        let source_slot = source.editor.selected_mseg;
        source.modulation.msegs[source_slot].nodes[2].value = 0.9;
        assert!(
            source
                .send(shortcut(
                    Key::Character("c".into()),
                    Modifiers::CONTROL | Modifiers::ALT
                ))
                .captured
        );
        let before = target.modulation.clone();
        let slot = target.editor.selected_mseg;
        let response = target.send(shortcut(
            Key::Character("v".into()),
            Modifiers::CONTROL | Modifiers::ALT,
        ));
        assert!(response.captured && response.changed);
        assert_eq!(
            target.modulation.msegs[slot].nodes[0],
            before.msegs[slot].nodes[0]
        );
        assert_eq!(
            target.modulation.msegs[slot].nodes[4],
            before.msegs[slot].nodes[4]
        );
        assert_eq!(target.modulation.msegs[slot].nodes[1].value, 0.2);
        assert_eq!(target.modulation.msegs[slot].nodes[2].value, 0.9);
        assert_eq!(target.modulation.msegs[slot].nodes[3].value, 0.2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
