mod context_help;
mod curve_browser;
mod semitone_grid;

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

mod loop_seams {
    use super::*;
    use widgets::mseg::{MsegData, PointWeight, SegmentShape};

    fn open_edit(h: &mut Harness, node_index: usize) {
        let node = h.modulation.msegs[h.editor.selected_mseg].nodes[node_index];
        let layout = h
            .editor
            .mseg_edit
            .layout(h.geometry.layout.mseg_pane, h.geometry.scale);
        h.move_to(
            widgets::mseg::phase_to_x(&layout, node.time),
            widgets::mseg::value_to_y(&layout, node.value),
            Modifiers::empty(),
        );
        h.press(MouseButton::Left);
        h.release();
        h.press(MouseButton::Right);
        assert!(h.editor.mseg_edit.has_open_menu());
    }

    fn choose(h: &mut Harness, index: usize) -> EditorResponse {
        h.key(Key::Home);
        for _ in 0..index {
            assert!(!h.key(Key::ArrowDown).changed);
        }
        h.key(Key::Enter)
    }

    #[test]
    fn seam_commands_are_saved_one_step_edits_and_smoothing_preserves_point_identity() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.resize(scale);
            let slot = h.editor.selected_mseg;
            h.modulation.msegs[slot] = MsegData::default();
            h.modulation.msegs[slot].nodes[0].value = 0.2;
            h.modulation.msegs[slot].nodes[1].value = 0.8;
            open_edit(&mut h, 1);
            let before = h.modulation.clone();
            let mut history = crate::undo::UndoHistory::new();
            history.begin_capture(before.clone());
            let response = choose(&mut h, 29);
            assert!(response.captured && response.changed);
            history.commit_capture(&h.modulation);
            let after = h.modulation.clone();
            let curve = &after.msegs[slot];
            assert_eq!(curve.node_count, 3);
            assert_eq!(curve.nodes[0].value, curve.nodes[2].value);
            assert_eq!(curve.nodes[1].value, 0.8);
            assert_eq!(curve.nodes[1].shape, SegmentShape::SCurve);
            assert_eq!(curve.nodes[0].weight, PointWeight::Soft);
            assert!(h.editor.mseg_edit.is_node_selected(2));
            let mut geometry_only = before.clone();
            geometry_only.msegs[slot] = *curve;
            assert_eq!(after, geometry_only);
            assert_eq!(history.undo(after.clone()), Some(before.clone()));
            assert!(history.undo(before.clone()).is_none());
            assert_eq!(history.redo(before), Some(after.clone()));
            assert_eq!(
                serde_json::from_str::<TrackModulation>(&serde_json::to_string(&after).unwrap())
                    .unwrap(),
                after
            );
            open_edit(&mut h, 2);
            assert!(!choose(&mut h, 29).changed);
            assert_eq!(h.modulation, after);
        }
    }

    #[test]
    fn endpoint_matches_reach_the_correct_hold_loop_without_touching_other_routing() {
        for (index, target, value) in [(27, 2, 0.2), (28, 1, 0.8)] {
            let mut h = Harness::new();
            let slot = h.editor.selected_mseg;
            let curve = &mut h.modulation.msegs[slot];
            *curve = MsegData::default();
            curve.insert_node(0.2, 0.2).unwrap();
            curve.insert_node(0.8, 0.8).unwrap();
            curve.hold = widgets::mseg::HoldMode::Loop { start: 1, end: 2 };
            open_edit(&mut h, 1);
            let mut expected = h.modulation.clone();
            expected.msegs[slot].nodes[target].value = value;
            let response = choose(&mut h, index);
            assert!(response.captured && response.changed);
            assert_eq!(h.modulation, expected);
        }
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
    fn native_shape_and_weight_commands_persist_as_separate_undo_entries() {
        use widgets::mseg::{PointWeight, SegmentShape};
        let mut h = Harness::new();
        let slot = h.editor.selected_mseg;
        assert!(
            h.send(shortcut(Key::Character("a".into()), Modifiers::CONTROL))
                .captured
        );
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        let node = h.modulation.msegs[slot].nodes[1];
        let layout = h
            .editor
            .mseg_edit
            .layout(h.geometry.layout.mseg_pane, h.geometry.scale);
        for shape in [true, false] {
            h.move_to(
                widgets::mseg::phase_to_x(&layout, node.time),
                widgets::mseg::value_to_y(&layout, node.value),
                Modifiers::empty(),
            );
            h.press(MouseButton::Right);
            assert!(h.editor.captures_keyboard());
            if shape {
                h.key(Key::Home);
                for _ in 0..22 {
                    h.key(Key::ArrowDown);
                }
            } else {
                h.key(Key::Home);
                for _ in 0..26 {
                    h.key(Key::ArrowDown);
                }
            }
            undo.begin_capture(h.modulation.clone());
            assert!(h.key(Key::Enter).changed);
            undo.commit_capture(&h.modulation);
        }
        let after = h.modulation.clone();
        for node in &after.msegs[slot].active()[..after.msegs[slot].node_count - 1] {
            assert_eq!(node.shape, SegmentShape::SCurve);
        }
        assert!(
            after.msegs[slot]
                .active()
                .iter()
                .all(|node| node.weight == PointWeight::Soft)
        );
        let shape_only = undo.undo(after.clone()).unwrap();
        assert!(
            shape_only.msegs[slot]
                .active()
                .iter()
                .all(|node| node.weight == PointWeight::Hard)
        );
        assert_eq!(undo.undo(shape_only), Some(before));
        assert_eq!(after.targets, h.modulation.targets);
        assert_eq!(after.depths, h.modulation.depths);
        let project = serde_json::to_string(&after).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&project).unwrap(),
            after
        );
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

mod detector_controls {
    use super::*;
    use crate::modulation::{AudioTriggerSettings, DetectorInput, DetectorReading};
    use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
    use std::sync::Arc;

    fn layout(h: &Harness) -> detector::DetectorLayout {
        detector::detector_layout(h.geometry)
    }
    fn open(h: &mut Harness) {
        h.modulation.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        h.click(h.geometry.layout.detector_setup);
        assert!(h.editor.detector_panel_open());
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
    fn type_value(h: &mut Harness, field: DetectorField, text: &str) {
        let r = layout(h).fields[field as usize];
        h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
        h.press(MouseButton::Right);
        assert!(h.editor.text_edit.is_active());
        h.type_replacement(text);
    }
    fn numeric(h: &mut Harness, field: DetectorField, text: &str) -> EditorResponse {
        type_value(h, field, text);
        h.key(Key::Enter)
    }

    #[test]
    fn sidechain_input_commits_numeric_text_and_resets_filters_without_changing_the_route() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.resize(scale);
            open(&mut h);
            type_value(&mut h, DetectorField::Level, "-18 dB");
            let before = h.modulation.clone();
            let mut history = crate::undo::UndoHistory::new();
            history.begin_capture(before.clone());
            let input = layout(&h).input;
            let response = h.click(input);
            assert!(response.captured && response.changed);
            assert!(h.editor.detector_panel_open() && !h.editor.text_edit.is_active());
            assert!(!h.editor.pointer_gesture_active());
            assert_eq!(h.modulation.audio_trigger.input, DetectorInput::Sidechain);
            assert_eq!(h.modulation.audio_trigger.level_db, Some(-18.0));
            history.commit_capture(&h.modulation);
            assert_eq!(history.undo(h.modulation.clone()), Some(before));
            assert!(h.press(MouseButton::Right).captured);
            assert!(h.editor.detector_panel_open() && !h.editor.text_edit.is_active());
            let reset = layout(&h).reset;
            assert!(h.click(reset).changed);
            assert_eq!(
                h.modulation.audio_trigger,
                AudioTriggerSettings {
                    input: DetectorInput::Sidechain,
                    ..AudioTriggerSettings::default()
                }
            );
            let saved = serde_json::to_string(&h.modulation).unwrap();
            assert_eq!(
                serde_json::from_str::<TrackModulation>(&saved).unwrap(),
                h.modulation
            );
            assert!(h.click(input).changed);
            assert_eq!(h.modulation.audio_trigger, AudioTriggerSettings::default());
        }
    }

    #[test]
    fn changing_detector_input_hides_readings_from_the_previous_source() {
        let mut h = Harness::new();
        let shared = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        open(&mut h);
        let mut frame = ModulationFrame::capture(
            h.effect.kind,
            &h.modulation,
            &crate::modulation::ModulationState::default(),
        );
        frame.detector = Some(DetectorReading {
            level: 0.8,
            settings: h.modulation.audio_trigger,
        });
        h.editor.live.as_mut().unwrap().frame = Some(frame);
        assert_eq!(h.editor.detector_level(), Some(0.8));
        let input = layout(&h).input;
        h.click(input);
        assert_eq!(h.editor.detector_level(), None);
        frame.detector = Some(DetectorReading {
            level: 0.0,
            settings: h.modulation.audio_trigger,
        });
        h.editor.live.as_mut().unwrap().frame = Some(frame);
        assert_eq!(h.editor.detector_level(), Some(0.0));
    }

    #[test]
    fn detector_numeric_settings_save_and_reset_as_one_undo_without_changing_geometry() {
        let mut h = Harness::new();
        let geometry = h.modulation.msegs;
        assert!(
            !serde_json::to_string(&h.modulation)
                .unwrap()
                .contains("audio_trigger")
        );
        open(&mut h);
        assert!(numeric(&mut h, DetectorField::Level, "-24 dB").changed);
        assert!(numeric(&mut h, DetectorField::Highpass, "200 Hz").changed);
        assert!(numeric(&mut h, DetectorField::Lowpass, "5 kHz").changed);
        assert_eq!(
            h.modulation.audio_trigger,
            AudioTriggerSettings {
                level_db: Some(-24.0),
                highpass_hz: Some(200.0),
                lowpass_hz: Some(5000.0),
                ..AudioTriggerSettings::default()
            }
        );
        assert_eq!(h.modulation.msegs, geometry);
        let saved = serde_json::to_string(&h.modulation).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&saved).unwrap(),
            h.modulation
        );
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        assert!(h.click(layout(&h).reset).changed);
        let after = h.modulation.clone();
        undo.commit_capture(&after);
        assert_eq!(after.audio_trigger, AudioTriggerSettings::default());
        assert_eq!(undo.undo(after), Some(before));
    }

    #[test]
    fn detector_minima_survive_zero_move_and_typed_value_commits_before_same_slider_drag() {
        let mut h = Harness::new();
        open(&mut h);
        for (field, text) in [
            (DetectorField::Level, "-60"),
            (DetectorField::Highpass, "20"),
            (DetectorField::Lowpass, "20"),
        ] {
            numeric(&mut h, field, text);
            let before = h.modulation.audio_trigger;
            h.editor.effect_click = ClickTracker::new();
            let r = layout(&h).fields[field as usize];
            h.move_to(r.0 + 20.0, r.1 + 16.0, Modifiers::empty());
            h.press(MouseButton::Left);
            h.move_to(r.0 + 20.0, r.1 + 16.0, Modifiers::empty());
            h.move_to(r.0 + 20.0, r.1 + 16.0, Modifiers::SHIFT);
            h.release();
            assert_eq!(h.modulation.audio_trigger, before, "field {field:?}");
        }
        type_value(&mut h, DetectorField::Highpass, "200");
        let r = layout(&h).fields[1];
        assert!(h.press(MouseButton::Left).changed);
        assert_eq!(h.modulation.audio_trigger.highpass_hz, Some(200.0));
        h.move_to(h.pointer.0, h.pointer.1 - 30.0, Modifiers::empty());
        h.release();
        assert!(h.modulation.audio_trigger.highpass_hz.unwrap() > 200.0);
        assert!(r.2 > 0.0);
        let before = h.modulation.audio_trigger;
        type_value(&mut h, DetectorField::Lowpass, "1000");
        h.key(Key::Escape);
        assert!(h.editor.detector_panel_open() && !h.editor.text_edit.is_active());
        assert_eq!(h.modulation.audio_trigger, before);
        h.key(Key::Escape);
        assert!(!h.editor.detector_panel_open());
    }

    #[test]
    fn detector_modal_draws_last_and_swallows_underlying_inputs_at_each_scale() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.geometry = EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                scale,
                window_size: (1336.0 * scale, 680.0 * scale),
            };
            open(&mut h);
            let rect = layout(&h).rect;
            assert!(
                rect.0 >= 0.0
                    && rect.1 >= 0.0
                    && rect.0 + rect.2 <= h.geometry.window_size.0
                    && rect.1 + rect.3 <= h.geometry.window_size.1
            );
            let before = (h.effect, h.modulation.clone());
            let viewport = h.editor.mseg_edit.viewport();
            let mut expected = tiny_skia::Pixmap::new(
                h.geometry.window_size.0 as u32,
                h.geometry.window_size.1 as u32,
            )
            .unwrap();
            h.editor
                .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
            let mut actual = expected.clone();
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
            let mut count = 0;
            for (a, b) in expected.pixels().iter().zip(actual.pixels()) {
                if a.alpha() == 255 {
                    count += 1;
                    assert_eq!(a, b);
                }
            }
            assert!(count > 100);
            assert!(
                h.send(Event::Mouse(MouseEvent::WheelScrolled {
                    delta: baseview::ScrollDelta::Lines { x: 0.0, y: 4.0 },
                    modifiers: Modifiers::empty()
                }))
                .captured
            );
            assert_eq!(h.editor.mseg_edit.viewport(), viewport);
            let dial = h.geometry.layout.dials[0];
            h.move_to(
                dial.0 + dial.2 * 0.5,
                dial.1 + dial.3 * 0.5,
                Modifiers::empty(),
            );
            assert!(h.press(MouseButton::Right).captured);
            assert!(!h.editor.text_edit.is_active() && !h.editor.detector_panel_open());
            assert_eq!((h.effect, h.modulation), before);
        }
    }

    #[test]
    fn detector_meter_uses_matching_dsp_settings_and_binding_changes_discard_pending_text() {
        let mut h = Harness::new();
        h.modulation.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        let shared = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        draw(&mut h);
        open(&mut h);
        assert_eq!(h.editor.detector_level(), None);
        let mut clock = TraceClock::default();
        clock.begin_block(&shared, 48000.0);
        let mut frame = ModulationFrame {
            kind: h.effect.kind,
            targets: h.modulation.targets,
            phases: [0.0; 4],
            outputs: [0.0; 4],
            parameters: [None; MAX_EFFECT_PARAMS],
            mix: None,
            detector: Some(DetectorReading {
                level: 0.0,
                settings: AudioTriggerSettings::default(),
            }),
        };
        clock.publish(&shared, 0, &frame);
        draw(&mut h);
        assert_eq!(h.editor.detector_level(), Some(0.0));
        numeric(&mut h, DetectorField::Highpass, "200");
        draw(&mut h);
        assert_eq!(h.editor.detector_level(), None);
        frame.detector = Some(DetectorReading {
            level: 0.4,
            settings: h.modulation.audio_trigger,
        });
        clock.publish(&shared, 0, &frame);
        draw(&mut h);
        assert_eq!(h.editor.detector_level(), Some(0.4));
        type_value(&mut h, DetectorField::Lowpass, "3000");
        h.editor.connect_live(&shared, 1);
        draw(&mut h);
        assert!(!h.editor.detector_panel_open() && !h.editor.text_edit.is_active());
        assert_eq!(h.modulation.audio_trigger.lowpass_hz, None);
        open(&mut h);
        type_value(&mut h, DetectorField::Lowpass, "4000");
        shared.invalidate();
        draw(&mut h);
        assert!(!h.editor.detector_panel_open() && !h.editor.text_edit.is_active());
        assert_eq!(h.modulation.audio_trigger.lowpass_hz, None);
    }

    #[test]
    fn detector_focus_commit_closes_panel_and_keeps_frozen_view_preferences() {
        let mut h = Harness::new();
        h.editor.set_audio_view(AudioView::Both);
        h.editor.set_audio_frozen(true);
        open(&mut h);
        type_value(&mut h, DetectorField::Level, "-12");
        let before = h.modulation.clone();
        let mut history = crate::undo::UndoHistory::new();
        history.begin_capture(before.clone());
        let response = h.send(Event::Window(baseview::WindowEvent::Unfocused));
        assert!(response.changed && response.captured);
        assert!(!h.editor.detector_panel_open() && !h.editor.text_edit.is_active());
        assert!(h.editor.audio_frozen());
        let after = h.modulation.clone();
        history.commit_capture(&after);
        assert_eq!(after.audio_trigger.level_db, Some(-12.0));
        assert_eq!(history.undo(after), Some(before));
        open(&mut h);
        h.resize(2.0);
        assert!(!h.editor.detector_panel_open());
        assert!(h.editor.audio_frozen());
    }

    #[test]
    fn detector_long_valid_unit_input_scrolls_inside_the_modal_without_truncating_its_value() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.geometry = EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                scale,
                window_size: (1336.0 * scale, 680.0 * scale),
            };
            open(&mut h);
            type_value(&mut h, DetectorField::Highpass, "20000.0000000 Hz");
            assert_eq!(
                h.editor
                    .text_edit
                    .active_for(&EffectHit::Detector(DetectorField::Highpass)),
                Some("20000.0000000 Hz")
            );
            let mut pm = tiny_skia::Pixmap::new(
                h.geometry.window_size.0 as u32,
                h.geometry.window_size.1 as u32,
            )
            .unwrap();
            h.editor
                .draw_popup(&mut pm, &mut h.tr, &h.effect, h.geometry);
            let r = layout(&h).rect;
            let width = pm.width() as usize;
            for (index, pixel) in pm
                .pixels()
                .iter()
                .enumerate()
                .filter(|(_, p)| p.alpha() > 0)
            {
                let (x, y) = ((index % width) as f32, (index / width) as f32);
                assert!(
                    x >= r.0 - 1.0
                        && x <= r.0 + r.2 + 1.0
                        && y >= r.1 - 1.0
                        && y <= r.1 + r.3 + 1.0,
                    "modal pixels escaped at {x},{y}, scale {scale}"
                );
                assert!(pixel.alpha() > 0);
            }
            assert!(h.key(Key::Enter).changed);
            assert_eq!(h.modulation.audio_trigger.highpass_hz, Some(20000.0));
        }
    }
}

mod precise_triggers {
    use super::*;
    fn choose(h: &mut Harness, index: usize) {
        h.click(h.geometry.layout.trigger);
        h.key(Key::Home);
        for _ in 0..index {
            h.key(Key::ArrowDown);
        }
        assert!(h.key(Key::Enter).changed);
    }
    #[test]
    fn precise_choices_carry_rate_and_sensitivity_controls_with_local_undo() {
        let mut h = Harness::new();
        let curves = h.modulation.msegs;
        h.modulation.trigger = TriggerSource::FreeHz { hz: 3.7 };
        choose(&mut h, 6);
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::FreeHzSample { hz: 3.7 }
        );
        let before = h.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        let r = h.geometry.layout.trigger_rate;
        h.move_to(r.0 + 32.0, r.1 + 48.0, Modifiers::empty());
        h.press(MouseButton::Left);
        h.move_to(r.0 + 32.0, r.1 + 18.0, Modifiers::empty());
        h.release();
        let after = h.modulation.clone();
        undo.commit_capture(&after);
        assert!(matches!(after.trigger,TriggerSource::FreeHzSample {hz} if hz>3.7));
        assert_eq!(undo.undo(after), Some(before));
        h.modulation.trigger = TriggerSource::Transient {
            threshold: 3.0,
            hold_ms: 80.0,
        };
        choose(&mut h, 7);
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::TransientSample {
                threshold: 3.0,
                hold_ms: 80.0
            }
        );
        h.editor.effect_click = ClickTracker::new();
        h.move_to(r.0 + 32.0, r.1 + 48.0, Modifiers::empty());
        h.press(MouseButton::Left);
        h.move_to(r.0 + 32.0, r.1 + 18.0, Modifiers::empty());
        h.release();
        assert!(
            matches!(h.modulation.trigger,TriggerSource::TransientSample {threshold,hold_ms:80.0} if threshold<3.0)
        );
        let r = h.geometry.layout.trigger_aux;
        h.move_to(r.0 + 32.0, r.1 + 48.0, Modifiers::empty());
        h.press(MouseButton::Left);
        h.move_to(r.0 + 32.0, r.1 + 18.0, Modifiers::empty());
        h.release();
        assert!(
            matches!(h.modulation.trigger,TriggerSource::TransientSample {hold_ms,..} if hold_ms>80.0)
        );
        assert_eq!(h.modulation.msegs, curves);
        let json = serde_json::to_string(&h.modulation).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&json).unwrap(),
            h.modulation
        );
    }
    #[test]
    fn precise_trigger_menu_fits_and_keeps_popup_input_priority_at_each_scale() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.geometry = EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                scale,
                window_size: (1336.0 * scale, 680.0 * scale),
            };
            h.click(h.geometry.layout.trigger);
            let items = dropdown_items(&h.editor, &h.effect);
            assert_eq!(items.len(), 8);
            let popup = widgets::dropdown::dropdown_popup_layout(
                &h.editor.effect_dropdown,
                &items,
                h.geometry.window_size,
            )
            .unwrap();
            let font = (h.geometry.layout.trigger.3 * 0.5).max(10.0);
            let widest = items
                .iter()
                .map(|label| h.tr.text_width(label, font))
                .fold(0.0_f32, f32::max);
            assert!(widest + 12.0 <= popup.visible_rows[0].rect.2 + 0.01);
            let mut expected = tiny_skia::Pixmap::new(
                h.geometry.window_size.0 as u32,
                h.geometry.window_size.1 as u32,
            )
            .unwrap();
            h.editor
                .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
            let mut actual = expected.clone();
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
            for (a, b) in expected.pixels().iter().zip(actual.pixels()) {
                if a.alpha() == 255 {
                    assert_eq!(a, b);
                }
            }
            let r = h.geometry.layout.dials[0];
            h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
            assert!(h.press(MouseButton::Right).captured);
            assert!(!h.editor.text_edit.is_active());
            choose(&mut h, 7);
            assert!(matches!(
                h.modulation.trigger,
                TriggerSource::TransientSample { .. }
            ));
        }
    }
}

mod midi_retrigger {
    use super::*;

    fn select_source(h: &mut Harness) {
        h.click(h.geometry.layout.trigger);
        h.key(Key::Home);
        for _ in 0..5 {
            h.key(Key::ArrowDown);
        }
        assert!(h.key(Key::Enter).changed);
        assert_eq!(
            h.modulation.trigger,
            TriggerSource::MidiNote {
                note: None,
                channel: None
            }
        );
    }

    #[test]
    fn midi_filters_are_stepped_saved_and_undoable_without_changing_curves() {
        let mut h = Harness::new();
        select_source(&mut h);
        let curves = h.modulation.msegs;
        let mut history = crate::undo::UndoHistory::new();
        let any = h.modulation.clone();
        h.click(MidiField::Note.rect(&h.geometry.layout, 1.0));
        assert!(h.editor.dropdown_open());
        h.key(Key::Character("60".into()));
        history.begin_capture(h.modulation.clone());
        assert!(h.key(Key::Enter).changed);
        let note = h.modulation.clone();
        history.commit_capture(&note);
        assert_eq!(
            note.trigger,
            TriggerSource::MidiNote {
                note: Some(60),
                channel: None
            }
        );
        h.click(MidiField::Channel.rect(&h.geometry.layout, 1.0));
        h.key(Key::End);
        history.begin_capture(h.modulation.clone());
        assert!(h.key(Key::Enter).changed);
        let channel = h.modulation.clone();
        history.commit_capture(&channel);
        assert_eq!(
            channel.trigger,
            TriggerSource::MidiNote {
                note: Some(60),
                channel: Some(15)
            }
        );
        assert_eq!(h.modulation.msegs, curves);
        assert_eq!(history.undo(channel.clone()), Some(note.clone()));
        assert_eq!(history.undo(note), Some(any));
        let json = serde_json::to_string(&channel).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&json).unwrap(),
            channel
        );
        h.click(h.geometry.layout.trigger);
        h.key(Key::Home);
        for _ in 0..5 {
            h.key(Key::ArrowDown);
        }
        h.key(Key::Enter);
        assert_eq!(
            h.modulation.trigger, channel.trigger,
            "selecting MIDI again lost its filters"
        );
        for field in [MidiField::Note, MidiField::Channel] {
            let r = field.rect(&h.geometry.layout, 1.0);
            h.move_to(r.0 + 10.0, r.1 + 10.0, Modifiers::empty());
            h.press(MouseButton::Right);
            assert!(!h.editor.text_edit.is_active());
        }
    }

    #[test]
    fn midi_menus_keep_popup_keyboard_and_wheel_priority_at_each_scale() {
        for scale in [0.5, 1.0, 2.0] {
            for field in [MidiField::Note, MidiField::Channel] {
                let mut h = Harness::new();
                h.geometry = EditorGeometry {
                    layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                    scale,
                    window_size: (1336.0 * scale, 680.0 * scale),
                };
                h.modulation.trigger = TriggerSource::MidiNote {
                    note: Some(127),
                    channel: Some(15),
                };
                let before = h.modulation.clone();
                h.click(field.rect(&h.geometry.layout, scale));
                let items = dropdown_items(&h.editor, &h.effect);
                let popup = widgets::dropdown::dropdown_popup_layout(
                    &h.editor.effect_dropdown,
                    &items,
                    h.geometry.window_size,
                )
                .unwrap();
                let font = (field.rect(&h.geometry.layout, scale).3 * 0.5).max(10.0);
                let width = items
                    .iter()
                    .map(|label| h.tr.text_width(label, font))
                    .fold(0.0_f32, f32::max);
                let row_width = popup
                    .visible_rows
                    .iter()
                    .map(|row| row.rect.2)
                    .fold(f32::INFINITY, f32::min);
                assert!(width <= row_width - 12.0 + 0.01);
                let mut expected = tiny_skia::Pixmap::new(
                    h.geometry.window_size.0 as u32,
                    h.geometry.window_size.1 as u32,
                )
                .unwrap();
                h.editor
                    .draw_popup(&mut expected, &mut h.tr, &h.effect, h.geometry);
                let mut actual = expected.clone();
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
                // Ignore the empty filter's caret; its blink can change between renders.
                let caret = popup
                    .filter_rect
                    .map(|(x, y, _, h)| (x + 7.0, y + 3.0, 1.0, h - 6.0));
                for (index, (old, new)) in expected.pixels().iter().zip(actual.pixels()).enumerate()
                {
                    let x = (index % expected.width() as usize) as f32;
                    let y = (index / expected.width() as usize) as f32;
                    let blinking = caret.is_some_and(|(cx, cy, cw, ch)| {
                        x >= cx.floor()
                            && x < (cx + cw).ceil()
                            && y >= cy.floor()
                            && y < (cy + ch).ceil()
                    });
                    if old.alpha() == 255 && !blinking {
                        assert_eq!(old, new, "{field:?} at {scale}x, pixel ({x}, {y})");
                    }
                }
                assert!(
                    h.send(Event::Mouse(MouseEvent::WheelScrolled {
                        delta: baseview::ScrollDelta::Lines { x: 0.0, y: 1.0 },
                        modifiers: Modifiers::empty()
                    }))
                    .captured
                );
                assert!(h.key(Key::Escape).captured);
                assert_eq!(h.modulation, before);
                h.click(field.rect(&h.geometry.layout, scale));
                let r = h.geometry.layout.dials[0];
                h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
                assert!(h.press(MouseButton::Right).captured);
                assert!(!h.editor.text_edit.is_active());
                assert!(!h.editor.dropdown_open());
            }
        }
    }
}

mod audio_backdrop {
    use super::*;
    use crate::modulation_telemetry::ModulationTelemetry;
    use crate::waveform::{WaveformClock, WaveformTiming};
    use std::sync::Arc;

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
    }

    fn capture(h: &Harness, shared: &ModulationTelemetry, input: f32, output: f32) {
        let mut clock = WaveformClock::default();
        clock.begin_block(&shared.waveform);
        clock.prepare_segment(
            &shared.waveform,
            0,
            WaveformTiming {
                config: &h.modulation,
                kind: h.effect.kind,
                phase: 0.0,
                trigger_epoch: 0,
                bpm: 120.0,
                sample_rate: 48000.0,
            },
        );
        clock.record_sample(&shared.waveform, [input; 2], [output; 2], 0.0);
        clock.finish_block(&shared.waveform);
    }

    #[test]
    fn sidechain_view_switches_capture_families_and_preserves_its_own_frozen_display() {
        let mut h = Harness::new();
        let shared = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        h.editor.set_audio_view(AudioView::Both);
        draw(&mut h);
        capture(&h, &shared, 0.9, 0.3);
        draw(&mut h);
        h.editor.set_audio_frozen(true);
        draw(&mut h);
        assert!(
            h.editor
                .live
                .as_ref()
                .unwrap()
                .waveform
                .iter()
                .any(Option::is_some)
        );
        let before = (h.effect, h.modulation.clone());
        h.click(h.geometry.layout.audio);
        h.key(Key::Home);
        for _ in 0..4 {
            h.key(Key::ArrowDown);
        }
        assert!(!h.key(Key::Enter).changed);
        assert_eq!(h.editor.audio_view(), AudioView::Sidechain);
        assert!(!h.editor.audio_frozen());
        assert!(
            h.editor
                .live
                .as_ref()
                .unwrap()
                .waveform
                .iter()
                .all(Option::is_none)
        );
        draw(&mut h);
        let mut clock = WaveformClock::default();
        clock.begin_block(&shared.waveform);
        assert!(clock.request().sidechain);
        clock.prepare_segment(
            &shared.waveform,
            0,
            WaveformTiming {
                config: &h.modulation,
                kind: h.effect.kind,
                phase: 0.0,
                trigger_epoch: 0,
                bpm: 120.0,
                sample_rate: 48000.0,
            },
        );
        clock.record_sample_with_sidechain(&shared.waveform, [0.9; 2], [0.3; 2], [-0.7, 0.4], 0.0);
        clock.finish_block(&shared.waveform);
        draw(&mut h);
        let bins = h.editor.live.as_ref().unwrap().waveform;
        let measured = bins.iter().flatten().next().unwrap();
        assert_eq!(measured.input[0].min, -0.7);
        assert_eq!(measured.input[1].max, 0.4);
        h.editor.set_audio_frozen(true);
        draw(&mut h);
        for display in [
            AudioDisplay::Magnitude,
            AudioDisplay::Combined,
            AudioDisplay::Stereo,
        ] {
            h.editor.set_audio_display(display);
            h.editor.set_audio_gain(AudioGain::Plus12);
            draw(&mut h);
            assert!(h.editor.audio_frozen());
            assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        }
        h.editor.set_audio_view(AudioView::Output);
        assert!(!h.editor.audio_frozen());
        assert!(
            h.editor
                .live
                .as_ref()
                .unwrap()
                .waveform
                .iter()
                .all(Option::is_none)
        );
        draw(&mut h);
        clock.begin_block(&shared.waveform);
        assert!(!clock.request().sidechain);
        assert_eq!((h.effect, h.modulation), before);
    }

    #[test]
    fn audio_menu_and_freeze_are_view_only_and_capture_input_priority() {
        let mut h = Harness::new();
        let before = (h.effect, h.modulation.clone());
        assert!(h.click(h.geometry.layout.audio).captured);
        assert!(h.editor.dropdown_open());
        h.key(Key::Home);
        for _ in 0..3 {
            h.key(Key::ArrowDown);
        }
        let response = h.key(Key::Enter);
        assert!(response.captured && !response.changed);
        assert_eq!(h.editor.audio_view(), AudioView::Both);
        h.click(h.geometry.layout.audio);
        h.key(Key::End);
        assert!(!h.key(Key::Enter).changed);
        assert!(h.editor.audio_frozen());
        h.click(h.geometry.layout.audio);
        h.move_to(
            h.geometry.layout.dials[0].0 + 30.0,
            h.geometry.layout.dials[0].1 + 30.0,
            Modifiers::empty(),
        );
        assert!(h.press(MouseButton::Right).captured);
        assert!(!h.editor.dropdown_open());
        assert!(!h.editor.text_edit.is_active());
        assert_eq!((h.effect, h.modulation), before);
    }

    #[test]
    fn focus_loss_closes_midi_overlay_and_commits_text_without_losing_frozen_history_or_zoom() {
        let mut h = Harness::new();
        h.modulation.trigger = TriggerSource::MidiNote {
            note: None,
            channel: None,
        };
        let shared = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        h.editor.set_audio_view(AudioView::Both);
        draw(&mut h);
        capture(&h, &shared, 0.8, 0.3);
        draw(&mut h);
        let bins = h.editor.live.as_ref().unwrap().waveform;
        assert!(bins.iter().any(Option::is_some));
        h.editor.set_audio_frozen(true);
        draw(&mut h);
        let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
        h.move_to(
            widgets::mseg::phase_to_x(&layout, 0.5),
            widgets::mseg::value_to_y(&layout, 0.5),
            Modifiers::empty(),
        );
        h.send(Event::Mouse(MouseEvent::WheelScrolled {
            delta: baseview::ScrollDelta::Lines { x: 0.0, y: 6.0 },
            modifiers: Modifiers::empty(),
        }));
        let zoom = h.editor.mseg_edit.viewport();
        h.click(MidiField::Note.rect(&h.geometry.layout, 1.0));
        assert!(h.editor.dropdown_open());
        assert!(
            h.send(Event::Window(baseview::WindowEvent::Unfocused))
                .captured
        );
        assert!(!h.editor.dropdown_open() && !h.editor.pointer_gesture_active());
        assert_eq!(h.editor.mseg_edit.viewport(), zoom);
        assert!(h.editor.audio_frozen());
        assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        let rect = h.geometry.layout.dials[0];
        h.move_to(rect.0 + 30.0, rect.1 + 30.0, Modifiers::empty());
        h.press(MouseButton::Right);
        h.type_replacement("800");
        assert!(
            h.send(Event::Window(baseview::WindowEvent::Unfocused))
                .changed
        );
        assert_eq!(h.effect.params[0], 800.0);
        assert!(!h.editor.text_edit.is_active());
        assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        let before = h.modulation.clone();
        let r = h.geometry.layout.smooth;
        h.move_to(r.0 + 10.0, r.1 + 10.0, Modifiers::empty());
        h.press(MouseButton::Right);
        h.type_replacement("40");
        let mut history = crate::undo::UndoHistory::new();
        history.begin_capture(before.clone());
        assert!(
            h.send(Event::Window(baseview::WindowEvent::Unfocused))
                .changed
        );
        let after = h.modulation.clone();
        history.commit_capture(&after);
        assert_eq!(after.smooth_ms[h.editor.selected_mseg], 40.0);
        assert_eq!(history.undo(after), Some(before));
    }

    #[test]
    fn stereo_display_and_gain_reuse_live_and_frozen_bins_without_changing_capture() {
        let mut h = Harness::new();
        let shared = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        h.editor.set_audio_view(AudioView::Both);
        draw(&mut h);
        capture(&h, &shared, 0.8, -0.3);
        draw(&mut h);
        let bins = h.editor.live.as_ref().unwrap().waveform;
        let mut clock = WaveformClock::default();
        clock.begin_block(&shared.waveform);
        let token = clock.request().token;
        let before = (h.effect, h.modulation.clone());
        for (index, display, gain) in [
            (6, AudioDisplay::Combined, AudioGain::Unity),
            (10, AudioDisplay::Combined, AudioGain::Plus12),
            (7, AudioDisplay::Stereo, AudioGain::Plus12),
        ] {
            h.click(h.geometry.layout.audio);
            h.key(Key::Home);
            for _ in 0..index {
                h.key(Key::ArrowDown);
            }
            let response = h.key(Key::Enter);
            assert!(response.captured && !response.changed);
            draw(&mut h);
            clock.begin_block(&shared.waveform);
            assert_eq!(clock.request().token, token);
            assert_eq!(h.editor.audio_display(), display);
            assert_eq!(h.editor.audio_gain(), gain);
            assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        }
        h.editor.set_audio_frozen(true);
        draw(&mut h);
        for display in [
            AudioDisplay::Magnitude,
            AudioDisplay::Combined,
            AudioDisplay::Stereo,
        ] {
            h.editor.set_audio_display(display);
            h.editor.set_audio_gain(AudioGain::Minus12);
            draw(&mut h);
            clock.begin_block(&shared.waveform);
            assert_eq!(clock.request().token, 0);
            assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        }
        h.editor.cancel_interaction();
        h.resize(2.0);
        draw(&mut h);
        assert_eq!(h.editor.audio_display(), AudioDisplay::Stereo);
        assert_eq!(h.editor.audio_gain(), AudioGain::Minus12);
        assert!(h.editor.audio_frozen());
        assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        assert_eq!((h.effect, h.modulation), before);
    }

    #[test]
    fn expanded_audio_menu_renders_last_and_owns_input_at_each_scale() {
        for scale in [0.5, 1.0, 2.0] {
            let mut h = Harness::new();
            h.geometry = EditorGeometry {
                layout: view::effect_layout((16.0, 16.0, 1304.0, 640.0), scale),
                scale,
                window_size: (1336.0 * scale, 680.0 * scale),
            };
            let before = (h.effect, h.modulation.clone());
            assert!(h.click(h.geometry.layout.audio).captured);
            let items = dropdown_items(&h.editor, &h.effect);
            assert_eq!(items.len(), 12);
            let popup = widgets::dropdown::dropdown_popup_layout(
                &h.editor.effect_dropdown,
                &items,
                h.geometry.window_size,
            )
            .unwrap();
            let font_size = (h.geometry.layout.audio.3 * 0.5).max(10.0);
            let widest = items
                .iter()
                .map(|label| h.tr.text_width(label, font_size))
                .fold(0.0_f32, f32::max);
            assert!(
                widest <= popup.list_viewport.2 - 12.0 + 0.01,
                "Audio labels truncated at scale {scale}: need {widest}, have {}",
                popup.list_viewport.2 - 12.0
            );
            let mut expected = tiny_skia::Pixmap::new(
                h.geometry.window_size.0 as u32,
                h.geometry.window_size.1 as u32,
            )
            .unwrap();
            widgets::dropdown::draw_dropdown_popup(
                &mut expected,
                &mut h.tr,
                &h.editor.effect_dropdown,
                &items,
                h.geometry.window_size,
            );
            let mut actual = expected.clone();
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
            let mut compared = 0;
            for (expected, actual) in expected.pixels().iter().zip(actual.pixels()) {
                if expected.alpha() == 255 {
                    compared += 1;
                    assert_eq!(actual, expected, "scale {scale}");
                }
            }
            assert!(compared > 100);
            assert!(h.key(Key::ArrowDown).captured);
            assert!(h.key(Key::Escape).captured);
            assert!(!h.editor.dropdown_open());
            h.click(h.geometry.layout.audio);
            let r = h.geometry.layout.dials[0];
            h.move_to(r.0 + r.2 * 0.5, r.1 + r.3 * 0.5, Modifiers::empty());
            assert!(h.press(MouseButton::Right).captured);
            assert!(!h.editor.dropdown_open() && !h.editor.text_edit.is_active());
            assert_eq!((h.effect, h.modulation), before);
        }
    }

    #[test]
    fn freeze_survives_focus_and_resize_but_replacements_clear_it() {
        let mut h = Harness::new();
        let shared = Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        h.editor.set_audio_view(AudioView::Both);
        draw(&mut h);
        capture(&h, &shared, 0.8, 0.3);
        draw(&mut h);
        let bins = h.editor.live.as_ref().unwrap().waveform;
        assert!(bins.iter().any(Option::is_some));
        h.editor.set_audio_frozen(true);
        draw(&mut h);
        let mut clock = WaveformClock::default();
        clock.begin_block(&shared.waveform);
        assert_eq!(clock.request().token, 0);
        h.editor.cancel_interaction();
        h.resize(2.0);
        h.editor.set_audio_view(AudioView::Output);
        draw(&mut h);
        assert_eq!(h.editor.live.as_ref().unwrap().waveform, bins);
        assert!(h.editor.audio_frozen());
        shared.invalidate();
        draw(&mut h);
        assert!(
            h.editor
                .live
                .as_ref()
                .unwrap()
                .waveform
                .iter()
                .all(Option::is_none)
        );
        h.editor.set_audio_frozen(false);
        draw(&mut h);
        capture(&h, &shared, 0.4, 0.2);
        draw(&mut h);
        assert!(
            h.editor
                .live
                .as_ref()
                .unwrap()
                .waveform
                .iter()
                .any(Option::is_some)
        );
        h.editor.set_audio_frozen(true);
        h.editor.selected_mseg = 2;
        draw(&mut h);
        assert!(
            h.editor
                .live
                .as_ref()
                .unwrap()
                .waveform
                .iter()
                .all(Option::is_none)
        );
        h.editor.set_audio_view(AudioView::Off);
        draw(&mut h);
        assert!(!h.editor.audio_frozen());
        clock.begin_block(&shared.waveform);
        assert_eq!(clock.request().token, 0);
    }
}

mod smoothing_controls {
    use super::*;

    #[test]
    fn smooth_drag_supports_fine_adjustment_reset_and_one_undo_entry() {
        fn drag(modifiers: Modifiers) -> Harness {
            let mut h = Harness::new();
            h.modulation.smooth_ms[h.editor.selected_mseg] = 25.0;
            let rect = h.geometry.layout.smooth;
            h.move_to(rect.0 + 20.0, rect.1 + 20.0, Modifiers::empty());
            h.press(MouseButton::Left);
            h.move_to(rect.0 + 20.0, rect.1 + 10.0, modifiers);
            assert!(h.move_to(rect.0 + 20.0, rect.1 - 20.0, modifiers).changed);
            h.release();
            h
        }
        let mut coarse = drag(Modifiers::empty());
        let fine = drag(Modifiers::SHIFT);
        let slot = coarse.editor.selected_mseg;
        assert!(25.0 < fine.modulation.smooth_ms[slot]);
        assert!(fine.modulation.smooth_ms[slot] < coarse.modulation.smooth_ms[slot]);
        let mut before = coarse.modulation.clone();
        before.smooth_ms[slot] = 25.0;
        let after = coarse.modulation.clone();
        let mut undo = crate::undo::UndoHistory::new();
        undo.begin_capture(before.clone());
        undo.commit_capture(&after);
        assert_eq!(undo.undo(after.clone()), Some(before.clone()));
        assert!(undo.undo(before.clone()).is_none());
        assert_eq!(undo.redo(before), Some(after));
        let rect = coarse.geometry.layout.smooth;
        coarse.move_to(rect.0 + 20.0, rect.1 + 10.0, Modifiers::empty());
        coarse.editor.effect_click.last_action = Some(EffectHit::Smooth(slot));
        coarse.editor.effect_click.last_time = std::time::Instant::now();
        assert!(coarse.press(MouseButton::Left).changed);
        coarse.release();
        assert_eq!(coarse.modulation.smooth_ms[slot], 0.0);
    }

    #[test]
    fn smooth_numeric_entry_is_saved_per_slot_and_trace_toggle_is_view_only() {
        let mut h = Harness::new();
        let before = h.modulation.clone();
        let slot = h.editor.selected_mseg;
        let smooth = h.geometry.layout.smooth;
        h.move_to(smooth.0 + 10.0, smooth.1 + 10.0, Modifiers::empty());
        h.press(MouseButton::Right);
        h.type_replacement("25");
        assert!(h.key(Key::Enter).changed);
        assert_eq!(h.modulation.smooth_ms[slot], 25.0);
        assert_eq!(h.modulation.msegs, before.msegs);
        let saved = serde_json::to_string(&h.modulation).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&saved).unwrap(),
            h.modulation
        );
        let config = h.modulation.clone();
        let trace = h.geometry.layout.trace;
        let response = h.click(trace);
        assert!(response.captured && !response.changed);
        assert!(!h.editor.trace_enabled());
        assert_eq!(h.modulation, config);
        h.editor.cancel_interaction();
        assert!(!h.editor.trace_enabled());
    }

    #[test]
    fn smooth_numeric_entry_commits_before_pan_and_escape_restores_the_view() {
        let mut h = Harness::new();
        let smooth = h.geometry.layout.smooth;
        h.move_to(smooth.0 + 10.0, smooth.1 + 10.0, Modifiers::empty());
        h.press(MouseButton::Right);
        h.type_replacement("40 ms");
        let layout = h.editor.mseg_edit.layout(h.geometry.layout.mseg_pane, 1.0);
        let view = h.editor.mseg_edit.viewport();
        let (x, y) = (
            widgets::mseg::phase_to_x(&layout, 0.5),
            widgets::mseg::value_to_y(&layout, 0.5),
        );
        h.move_to(x, y, Modifiers::empty());
        assert!(h.press(MouseButton::Middle).changed);
        assert!(!h.editor.text_edit.is_active());
        assert!(h.editor.mseg_edit.is_panning());
        assert_eq!(h.modulation.smooth_ms[h.editor.selected_mseg], 40.0);
        h.move_to(x + 30.0, y + 20.0, Modifiers::empty());
        assert!(h.key(Key::Escape).captured);
        assert!(!h.editor.mseg_edit.is_panning());
        assert_eq!(h.editor.mseg_edit.viewport(), view);
        assert_eq!(h.modulation.smooth_ms[h.editor.selected_mseg], 40.0);
    }
    #[test]
    fn selector_target_disables_smooth_edits_without_losing_its_saved_setting() {
        let mut h = Harness::new();
        let slot = h.editor.selected_mseg;
        h.modulation.smooth_ms[slot] = 30.0;
        let index = effects::parameter_specs(h.effect.kind)
            .iter()
            .position(|s| matches!(s.format, effects::ParamFormat::Enum { .. }))
            .unwrap();
        h.modulation.targets[slot - 1] = Some(ModulationTarget::EffectParameter(index));
        let before = h.modulation.clone();
        let rect = h.geometry.layout.smooth;
        h.move_to(rect.0 + 10.0, rect.1 + 10.0, Modifiers::empty());
        h.press(MouseButton::Right);
        assert!(!h.editor.text_edit.is_active());
        assert!(!h.click(rect).changed);
        h.release();
        assert_eq!(h.modulation, before);
    }
    #[test]
    fn live_arcs_use_published_values_and_selection_changes_invalidate_history() {
        use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TraceClock};
        let mut h = Harness::new();
        let shared = std::sync::Arc::new(ModulationTelemetry::default());
        h.editor.connect_live(&shared, 0);
        let mut pm = tiny_skia::Pixmap::new(1336, 680).unwrap();
        h.editor.draw(
            &mut pm,
            &mut h.tr,
            &h.effect,
            &h.modulation,
            [0.75; 4],
            h.geometry,
        );
        {
            let session = Session::new(
                &mut h.editor,
                EditorContext {
                    pointer_position: h.pointer,
                    effect: &mut h.effect,
                    modulation: &mut h.modulation,
                    text_renderer: &mut h.tr,
                    geometry: h.geometry,
                },
            );
            assert_eq!(
                session.compute_modulated_norms().parameters,
                [None; MAX_EFFECT_PARAMS]
            );
        }
        let mut clock = TraceClock::default();
        let request = clock.begin_block(&shared, 48000.0);
        let mut frame = ModulationFrame {
            kind: h.effect.kind,
            targets: h.modulation.targets,
            phases: [0.2; 4],
            outputs: [0.3; 4],
            parameters: [None; MAX_EFFECT_PARAMS],
            mix: None,
            detector: None,
        };
        frame.parameters[0] = Some((0.123, 1));
        clock.publish(&shared, 0, &frame);
        clock.record(&shared, 0, &frame);
        h.editor.draw(
            &mut pm,
            &mut h.tr,
            &h.effect,
            &h.modulation,
            [0.75; 4],
            h.geometry,
        );
        assert_eq!(h.editor.live.as_ref().unwrap().frame, Some(frame));
        {
            let session = Session::new(
                &mut h.editor,
                EditorContext {
                    pointer_position: h.pointer,
                    effect: &mut h.effect,
                    modulation: &mut h.modulation,
                    text_renderer: &mut h.tr,
                    geometry: h.geometry,
                },
            );
            assert_eq!(
                session.compute_modulated_norms().parameters[0],
                Some((0.123, 1))
            );
        }
        assert!(shared.frame(request.token).is_some());
        h.editor.invalidate_trace();
        assert!(h.editor.live.as_ref().unwrap().frame.is_none());
        h.editor.draw(
            &mut pm,
            &mut h.tr,
            &h.effect,
            &h.modulation,
            [0.75; 4],
            h.geometry,
        );
        assert!(h.editor.live.as_ref().unwrap().frame.is_none());
        h.editor.disconnect_live();
    }
}
