use super::{DragTarget, MsegEdit, MsegEditState};
use crate::mseg::render::{MsegLayout, in_rect, mseg_layout};
use crate::mseg::{MsegData, MsegViewport, Polarity, SyncMode, tools};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};

pub(crate) const VIEW_ITEMS: &[&str] = &["Fit all", "Fit selection"];

pub(super) struct PrecisionState {
    viewport: MsegViewport,
    pan: Option<(MsegViewport, (f32, f32))>,
    inspector: Option<PointInspector>,
    scale: f32,
    readout: String,
}

impl Default for PrecisionState {
    fn default() -> Self {
        Self {
            viewport: MsegViewport::default(),
            pan: None,
            inspector: None,
            scale: 1.0,
            readout: String::new(),
        }
    }
}

struct PointInspector {
    node: usize,
    fields: [String; 3],
    active: usize,
    editable: [bool; 3],
    replace: bool,
    dirty: bool,
    error: String,
    time_scale: f32,
    time_unit: &'static str,
    bipolar: bool,
}

impl PointInspector {
    fn new(node: usize, data: &MsegData, static_curve: bool) -> Self {
        let (time_scale, time_unit) = if static_curve {
            (100.0, "%")
        } else if data.sync_mode == SyncMode::Beat {
            (data.beats, "beats")
        } else {
            (data.time_seconds * 1000.0, "ms")
        };
        let editable = [
            node > 0 && node + 1 < data.node_count,
            true,
            node + 1 < data.node_count && !data.nodes[node].stepped,
        ];
        let mut inspector = Self {
            node,
            fields: std::array::from_fn(|_| String::new()),
            active: if editable[0] { 0 } else { 1 },
            editable,
            replace: true,
            dirty: false,
            error: String::new(),
            time_scale,
            time_unit,
            bipolar: data.polarity == Polarity::Bipolar,
        };
        inspector.refresh(data);
        inspector
    }

    fn refresh(&mut self, data: &MsegData) {
        let n = data.nodes[self.node];
        self.fields = [
            (n.time * self.time_scale).to_string(),
            (if self.bipolar {
                n.value * 2.0 - 1.0
            } else {
                n.value
            })
            .to_string(),
            n.tension.to_string(),
        ];
        self.dirty = false;
        self.replace = true;
        self.error.clear();
    }

    fn commit(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        if !self.dirty {
            return None;
        }
        let Ok(value) = self.fields[self.active].trim().parse::<f64>() else {
            self.error = "Enter a finite number".into();
            return None;
        };
        if !value.is_finite() {
            self.error = "Enter a finite number".into();
            return None;
        }
        let (minimum, maximum) = match self.active {
            0 => (0.0, self.time_scale as f64),
            1 if !self.bipolar => (0.0, 1.0),
            _ => (-1.0, 1.0),
        };
        if !maximum.is_finite() || maximum <= minimum || value < minimum || value > maximum {
            self.error = "Value is outside its allowed range".into();
            return None;
        }
        let mut candidate = *data;
        let n = &mut candidate.nodes[self.node];
        match self.active {
            0 => n.time = (value / self.time_scale as f64) as f32,
            1 => {
                n.value = if self.bipolar {
                    ((value + 1.0) * 0.5) as f32
                } else {
                    value as f32
                }
            }
            _ => n.tension = value as f32,
        }
        if !candidate.is_valid() {
            self.error = if self.active == 0 {
                "Time must stay between neighboring points"
            } else {
                "Value is outside its allowed range"
            }
            .into();
            return None;
        }
        let changed = candidate != *data;
        *data = candidate;
        self.refresh(data);
        changed.then_some(MsegEdit::Changed)
    }
}

impl MsegEditState {
    pub fn viewport(&self) -> MsegViewport {
        self.precision.viewport
    }

    pub fn layout(&self, rect: (f32, f32, f32, f32), scale: f32) -> MsegLayout {
        let mut layout = mseg_layout(rect, self.curve_only, scale);
        layout.viewport = self.viewport();
        layout
    }

    pub(crate) fn cancel_precision(&mut self) {
        self.precision.pan = None;
        self.precision.inspector = None;
        self.precision.readout.clear();
    }

    pub(crate) fn prepare_precision(&mut self, data: &MsegData, scale: f32) {
        self.precision.scale = scale;
        self.selection &= tools::all_nodes(data.node_count);
        if self
            .inspector_node()
            .is_some_and(|node| node >= data.node_count)
        {
            self.precision.inspector = None;
        }
    }

    pub fn inspector_node(&self) -> Option<usize> {
        self.precision.inspector.as_ref().map(|i| i.node)
    }

    pub fn set_inspector_readout(&mut self, readout: &str) {
        if self.precision.readout != readout {
            self.precision.readout.clear();
            self.precision.readout.push_str(readout);
        }
    }

    pub fn inspector_readout(&self) -> &str {
        &self.precision.readout
    }

    pub fn open_point_inspector(&mut self, data: &MsegData) -> bool {
        if self.selection.count_ones() != 1 || self.is_dragging() {
            self.set_status_message("Select one point to inspect");
            return false;
        }
        let node = self.selection.trailing_zeros() as usize;
        if node >= data.node_count {
            return false;
        }
        self.dropdown.close();
        self.precision.inspector = Some(PointInspector::new(node, data, self.curve_only));
        self.power.preview.clear();
        true
    }

    pub fn fit_all(&mut self) {
        self.precision.viewport = MsegViewport::default();
        self.power.preview.clear();
    }

    pub fn fit_selection(&mut self, data: &MsegData) -> bool {
        let nodes: Vec<_> = data
            .active()
            .iter()
            .enumerate()
            .filter(|(i, _)| self.is_node_selected(*i))
            .map(|(_, n)| n)
            .collect();
        let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
            self.set_status_message("Select points to fit");
            return false;
        };
        let (lo, hi) = nodes.iter().fold((1.0_f32, 0.0_f32), |(lo, hi), n| {
            (lo.min(n.value), hi.max(n.value))
        });
        self.precision
            .viewport
            .fit((first.time, last.time), (lo, hi));
        self.power.preview.clear();
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta: f32,
        vertical: bool,
        data: &MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> bool {
        self.prepare_precision(data, scale);
        if self.has_open_menu() {
            return self.on_wheel(delta, rect);
        }
        if self.is_dragging() {
            return false;
        }
        let layout = self.layout(rect, scale);
        if !in_rect(layout.canvas, x, y) || layout.plot.2 <= 0.0 || layout.plot.3 <= 0.0 {
            return false;
        }
        self.precision.viewport.zoom(
            delta,
            (x - layout.plot.0) / layout.plot.2,
            1.0 - (y - layout.plot.1) / layout.plot.3,
            vertical,
        );
        self.hover = None;
        self.power.preview.clear();
        true
    }

    /// Whether a middle press can begin a pan rather than being owned by an overlay or drag.
    pub fn can_start_pan(&self, x: f32, y: f32, rect: (f32, f32, f32, f32), scale: f32) -> bool {
        !self.has_open_menu()
            && !self.is_dragging()
            && in_rect(self.layout(rect, scale).canvas, x, y)
    }

    pub fn on_pan_start(&mut self, x: f32, y: f32, rect: (f32, f32, f32, f32), scale: f32) -> bool {
        if self.has_open_menu() {
            return true;
        }
        if !self.can_start_pan(x, y, rect, scale) {
            return false;
        }
        self.precision.pan = Some((self.viewport(), (x, y)));
        self.drag = Some(DragTarget::Pan);
        self.hover = None;
        self.power.preview.clear();
        true
    }

    pub fn on_pan_end(&mut self) -> bool {
        if self.precision.pan.take().is_none() {
            return false;
        }
        if self.drag == Some(DragTarget::Pan) {
            self.drag = None;
        }
        true
    }

    pub fn is_panning(&self) -> bool {
        self.precision.pan.is_some()
    }

    pub(crate) fn move_pan(&mut self, x: f32, y: f32, rect: (f32, f32, f32, f32), scale: f32) {
        let Some((start, (px, py))) = self.precision.pan else {
            return;
        };
        let layout = self.layout(rect, scale);
        if layout.plot.2 > 0.0 && layout.plot.3 > 0.0 {
            self.precision.viewport.pan_from(
                start,
                (px - x) / layout.plot.2,
                (y - py) / layout.plot.3,
            );
        }
    }

    pub(crate) fn cancel_pan(&mut self) -> bool {
        let Some((start, _)) = self.precision.pan else {
            return false;
        };
        self.precision.viewport = start;
        self.on_pan_end();
        true
    }

    pub(crate) fn inspector_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
    ) -> Option<MsegEdit> {
        let scale = self.precision.scale;
        let panel = inspector_rect(rect, scale);
        let inspector = self.precision.inspector.as_mut()?;
        let field = (0..3).find(|&i| in_rect(field_rect(panel, i, scale), x, y));
        if let Some(field) = field {
            if !inspector.editable[field] || inspector.active == field {
                return None;
            }
            let changed = inspector.commit(data);
            if inspector.error.is_empty() {
                inspector.active = field;
                inspector.replace = true;
            }
            changed
        } else if !in_rect(panel, x, y) {
            let changed = inspector.commit(data);
            if inspector.error.is_empty() {
                self.precision.inspector = None;
            }
            changed
        } else {
            None
        }
    }

    pub(crate) fn inspector_key(
        &mut self,
        event: &KeyboardEvent,
        data: &mut MsegData,
    ) -> (bool, Option<MsegEdit>) {
        let Some(inspector) = self.precision.inspector.as_mut() else {
            return (false, None);
        };
        if event.state != KeyState::Down {
            return (true, None);
        }
        match &event.key {
            Key::Escape => {
                self.precision.inspector = None;
                (true, None)
            }
            Key::Enter | Key::Tab => {
                let changed = inspector.commit(data);
                if event.key == Key::Tab && inspector.error.is_empty() {
                    let delta = if event.modifiers.contains(Modifiers::SHIFT) {
                        2
                    } else {
                        1
                    };
                    loop {
                        inspector.active = (inspector.active + delta) % 3;
                        if inspector.editable[inspector.active] {
                            break;
                        }
                    }
                    inspector.replace = true;
                }
                (true, changed)
            }
            Key::Character(text) => {
                if event
                    .modifiers
                    .intersects(Modifiers::CONTROL | Modifiers::META)
                {
                    if text.eq_ignore_ascii_case("a") {
                        inspector.replace = true;
                    }
                } else {
                    let field = &mut inspector.fields[inspector.active];
                    if inspector.replace {
                        field.clear();
                        inspector.replace = false;
                    }
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if field.len() < 64 {
                            field.push(c);
                        }
                    }
                    inspector.dirty = true;
                    inspector.error.clear();
                }
                (true, None)
            }
            Key::Backspace | Key::Delete => {
                let field = &mut inspector.fields[inspector.active];
                if inspector.replace || event.key == Key::Delete {
                    field.clear();
                } else {
                    field.pop();
                }
                inspector.replace = false;
                inspector.dirty = true;
                inspector.error.clear();
                (true, None)
            }
            _ => (true, None),
        }
    }

    pub(crate) fn dismiss_inspector(&mut self) {
        self.precision.inspector = None;
    }

    pub(crate) fn draw_precision_overlay(
        &self,
        pixmap: &mut tiny_skia::Pixmap,
        tr: &mut crate::TextRenderer,
        rect: (f32, f32, f32, f32),
    ) {
        use crate::primitives::*;
        let Some(inspector) = &self.precision.inspector else {
            return;
        };
        let scale = self.precision.scale;
        let panel = inspector_rect(rect, scale);
        draw_rect(
            pixmap,
            panel.0,
            panel.1,
            panel.2,
            panel.3,
            color_control_bg(),
        );
        draw_rect_outline(
            pixmap,
            panel.0,
            panel.1,
            panel.2,
            panel.3,
            color_border(),
            1.0,
        );
        tr.draw_text(
            pixmap,
            panel.0 + 10.0 * scale,
            panel.1 + 20.0 * scale,
            &format!("Point {}", inspector.node + 1),
            13.0 * scale,
            color_text(),
        );
        for (i, label) in [
            format!("Time ({})", inspector.time_unit),
            "Value".into(),
            "Tension".into(),
        ]
        .iter()
        .enumerate()
        {
            let field = field_rect(panel, i, scale);
            let color = if inspector.editable[i] {
                color_text()
            } else {
                color_muted()
            };
            tr.draw_text(
                pixmap,
                panel.0 + 10.0 * scale,
                field.1 + 18.0 * scale,
                label,
                12.0 * scale,
                color,
            );
            draw_rect(pixmap, field.0, field.1, field.2, field.3, color_bg());
            draw_rect_outline(
                pixmap,
                field.0,
                field.1,
                field.2,
                field.3,
                if i == inspector.active {
                    color_accent()
                } else {
                    color_border()
                },
                1.0,
            );
            let text = if inspector.editable[i] || i != 2 {
                inspector.fields[i].as_str()
            } else {
                "-"
            };
            let mut shown = String::new();
            for ch in text.chars() {
                shown.push(ch);
                if tr.text_width(&shown, 12.0 * scale) > (field.2 - 10.0 * scale).max(0.0) {
                    shown.pop();
                    break;
                }
            }
            let text_width = tr.text_width(&shown, 12.0 * scale);
            if i == inspector.active && inspector.replace {
                draw_rect(
                    pixmap,
                    field.0 + 4.0 * scale,
                    field.1 + 3.0 * scale,
                    text_width + 2.0 * scale,
                    20.0 * scale,
                    color_border(),
                );
            }
            tr.draw_text(
                pixmap,
                field.0 + 5.0 * scale,
                field.1 + 18.0 * scale,
                &shown,
                12.0 * scale,
                color,
            );
            if i == inspector.active && !inspector.replace {
                draw_rect(
                    pixmap,
                    field.0 + 5.0 * scale + text_width,
                    field.1 + 4.0 * scale,
                    scale,
                    18.0 * scale,
                    color_text(),
                );
            }
        }
        let text = if inspector.error.is_empty() {
            self.precision.readout.as_str()
        } else {
            inspector.error.as_str()
        };
        let width = (panel.2 - 20.0 * scale).max(0.0);
        let mut shown = String::new();
        for ch in text.chars() {
            shown.push(ch);
            if tr.text_width(&shown, 11.0 * scale) > width {
                shown.pop();
                break;
            }
        }
        tr.draw_text(
            pixmap,
            panel.0 + 10.0 * scale,
            panel.1 + 142.0 * scale,
            &shown,
            11.0 * scale,
            color_text(),
        );
        tr.draw_text(
            pixmap,
            panel.0 + 10.0 * scale,
            panel.1 + 161.0 * scale,
            "Enter: apply  Tab: next  Esc: close",
            10.0 * scale,
            color_muted(),
        );
    }

    pub(crate) fn copy_selected_phrase(&mut self, data: &MsegData) -> Option<MsegEdit> {
        match tools::capture_selection(data, self.selection) {
            Ok(shape) => match self
                .curve_library()
                .and_then(|library| library.copy(&shape))
            {
                Ok(()) => self.set_status_message("Selection copied"),
                Err(err) => self.set_status_message(format!("Copy failed: {err}")),
            },
            Err(err) => self.set_status_message(err.to_string()),
        }
        None
    }

    pub(crate) fn paste_selected_phrase(
        &mut self,
        data: &mut MsegData,
        merge: bool,
    ) -> Option<MsegEdit> {
        let result = match self.curve_library().and_then(|library| library.paste()) {
            Ok(shape) => tools::paste_selection(data, &shape, self.selection, merge),
            Err(err) => {
                self.set_status_message(format!("Paste failed: {err}"));
                return None;
            }
        };
        match result {
            Ok((changed, selected)) => {
                self.selection = selected;
                self.hover = None;
                changed.then_some(MsegEdit::Changed)
            }
            Err(err) => self.finish_power_edit(Err(err)),
        }
    }
}

fn inspector_rect(rect: (f32, f32, f32, f32), scale: f32) -> (f32, f32, f32, f32) {
    let width = (340.0 * scale).min((rect.2 - 12.0 * scale).max(0.0));
    (
        rect.0 + rect.2 - width - 6.0 * scale,
        (rect.1 + rect.3 - 74.0 * scale - 168.0 * scale).max(rect.1),
        width,
        168.0 * scale,
    )
}
fn field_rect(panel: (f32, f32, f32, f32), field: usize, scale: f32) -> (f32, f32, f32, f32) {
    (
        panel.0 + 90.0 * scale,
        panel.1 + (30.0 + 32.0 * field as f32) * scale,
        (panel.2 - 100.0 * scale).max(0.0),
        26.0 * scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::render::{phase_to_x, value_to_y, x_to_phase, y_to_value};
    use crate::mseg::{FactoryShape, factory_shape};
    const RECT: (f32, f32, f32, f32) = (10.0, 10.0, 1100.0, 330.0);
    fn key(key: Key) -> KeyboardEvent {
        KeyboardEvent {
            key,
            state: KeyState::Down,
            ..KeyboardEvent::default()
        }
    }
    fn fixture(static_curve: bool) -> (MsegEditState, MsegData) {
        let mut data = MsegData::default();
        factory_shape(FactoryShape::Triangle)
            .apply(&mut data, false)
            .unwrap();
        data.snap = false;
        let mut state = if static_curve {
            MsegEditState::new_curve_only()
        } else {
            MsegEditState::new()
        };
        state.selection = 1 << 1;
        (state, data)
    }
    fn type_text(state: &mut MsegEditState, data: &mut MsegData, text: &str) {
        assert_eq!(
            state.on_key(&key(Key::Character(text.into())), data, RECT, 1.0),
            (true, None)
        );
    }
    #[test]
    fn numeric_time_commits_in_beats_milliseconds_and_percent() {
        for (static_curve, mode, duration, text, unit) in [
            (false, SyncMode::Beat, 4.0, "1", "beats"),
            (false, SyncMode::Time, 2.0, "500", "ms"),
            (true, SyncMode::Time, 2.0, "25", "%"),
        ] {
            let (mut state, mut data) = fixture(static_curve);
            data.sync_mode = mode;
            data.beats = duration;
            data.time_seconds = duration;
            let original = data;
            assert!(state.open_point_inspector(&data));
            assert_eq!(state.precision.inspector.as_ref().unwrap().time_unit, unit);
            type_text(&mut state, &mut data, text);
            assert_eq!(data, original);
            assert_eq!(
                state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                (true, Some(MsegEdit::Changed))
            );
            assert_eq!(data.nodes[1].time, 0.25);
            assert_eq!(
                state.on_key(&key(Key::Enter), &mut data, RECT, 1.0),
                (true, None)
            );
        }
    }
    #[test]
    fn inspector_edits_bipolar_value_and_tension_and_skips_readonly_fields() {
        let (mut state, mut data) = fixture(false);
        data.polarity = Polarity::Bipolar;
        state.open_point_inspector(&data);
        state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
        type_text(&mut state, &mut data, "-0.5");
        assert_eq!(
            state.on_key(&key(Key::Tab), &mut data, RECT, 1.0).1,
            Some(MsegEdit::Changed)
        );
        assert_eq!(data.nodes[1].value, 0.25);
        type_text(&mut state, &mut data, "0.3");
        state.on_key(&key(Key::Enter), &mut data, RECT, 1.0);
        assert_eq!(data.nodes[1].tension, 0.3);
        state.dismiss_inspector();
        state.selection = 1 << (data.node_count - 1);
        state.open_point_inspector(&data);
        assert_eq!(state.precision.inspector.as_ref().unwrap().active, 1);
        state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
        assert_eq!(state.precision.inspector.as_ref().unwrap().active, 1);
    }
    #[test]
    fn invalid_numeric_input_and_escape_never_modify_the_document() {
        for text in ["NaN", "inf", "oops", "-100", "100000"] {
            let (mut state, mut data) = fixture(false);
            let original = data;
            state.open_point_inspector(&data);
            type_text(&mut state, &mut data, text);
            assert_eq!(state.on_key(&key(Key::Enter), &mut data, RECT, 1.0).1, None);
            assert!(!state.precision.inspector.as_ref().unwrap().error.is_empty());
            state.on_mouse_down(0.0, 0.0, &mut data, RECT, 1.0, false);
            assert!(state.has_open_menu());
            state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
            assert!(!state.has_open_menu());
            assert_eq!(data, original);
        }
    }

    #[test]
    fn display_bounds_are_checked_before_f32_mapping_can_round_them_into_range() {
        for bipolar in [false, true] {
            for text in ["1.0000001", "1.0000000000001", "-1.0000001"] {
                let (mut state, mut data) = fixture(false);
                data.polarity = if bipolar {
                    Polarity::Bipolar
                } else {
                    Polarity::Unipolar
                };
                data.nodes[1].value = 0.5;
                let before = data;
                state.open_point_inspector(&data);
                state.on_key(&key(Key::Tab), &mut data, RECT, 1.0);
                type_text(&mut state, &mut data, text);
                assert_eq!(state.on_key(&key(Key::Enter), &mut data, RECT, 1.0).1, None);
                assert_eq!(data, before);
                assert!(!state.precision.inspector.as_ref().unwrap().error.is_empty());
            }
        }
    }

    #[test]
    fn primary_and_double_clicks_cannot_steal_an_active_pan() {
        let (mut state, mut data) = fixture(false);
        let before = data;
        let layout = state.layout(RECT, 1.0);
        let (x, y) = (phase_to_x(&layout, 0.5), value_to_y(&layout, 0.5));
        state.on_scroll(x, y, 6.0, false, &data, RECT, 1.0);
        state.on_pan_start(x, y, RECT, 1.0);
        state.on_mouse_move(x + 40.0, y, &mut data, RECT, 1.0, false);
        let view = state.viewport();
        assert_eq!(state.on_mouse_down(x, y, &mut data, RECT, 1.0, false), None);
        assert_eq!(state.on_double_click(x, y, &mut data, RECT, 1.0), None);
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(state.is_panning());
        assert!(state.on_pan_end());
        assert!(!state.is_panning());
        state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
        assert_eq!(state.viewport(), view);
        assert_eq!(data, before);
    }
    #[test]
    fn exact_numeric_time_does_not_use_the_mouse_drag_clearance() {
        let (mut state, mut data) = fixture(false);
        data.beats = 1.0;
        data.sync_mode = SyncMode::Beat;
        data.insert_node(0.499, 0.3).unwrap();
        data.insert_node(0.501, 0.6).unwrap();
        data.nodes[1].time = 0.49999;
        data.nodes[3].time = 0.50001;
        state.selection = 1 << 2;
        state.open_point_inspector(&data);
        type_text(&mut state, &mut data, "0.500001");
        assert_eq!(
            state.on_key(&key(Key::Enter), &mut data, RECT, 1.0).1,
            Some(MsegEdit::Changed)
        );
        assert_eq!(data.nodes[2].time, 0.500001);
    }
    #[test]
    fn outside_click_commits_and_inspector_owns_wheel_and_key_releases() {
        let (mut state, mut data) = fixture(true);
        state.open_point_inspector(&data);
        let view = state.viewport();
        assert!(state.on_scroll(100.0, 100.0, 10.0, false, &data, RECT, 1.0));
        assert_eq!(state.viewport(), view);
        let mut release = key(Key::Enter);
        release.state = KeyState::Up;
        assert_eq!(state.on_key(&release, &mut data, RECT, 1.0), (true, None));
        type_text(&mut state, &mut data, "25");
        assert_eq!(
            state.on_mouse_down(0.0, 0.0, &mut data, RECT, 1.0, false),
            Some(MsegEdit::Changed)
        );
        assert_eq!(data.nodes[1].time, 0.25);
        assert!(!state.has_open_menu());
    }

    #[test]
    fn pan_preflight_distinguishes_a_real_pan_from_overlay_capture_or_an_active_drag() {
        let (mut state, mut data) = fixture(false);
        let layout = state.layout(RECT, 1.0);
        let (x, y) = (phase_to_x(&layout, 0.5), value_to_y(&layout, 0.5));
        assert!(state.can_start_pan(x, y, RECT, 1.0));
        assert!(!state.can_start_pan(0.0, 0.0, RECT, 1.0));
        state.open_point_inspector(&data);
        assert!(!state.can_start_pan(x, y, RECT, 1.0));
        assert!(state.on_pan_start(x, y, RECT, 1.0));
        assert!(!state.is_panning());
        state.on_right_click(x, y, &mut data, RECT, 1.0);
        assert!(state.can_start_pan(x, y, RECT, 1.0));
        assert!(state.on_pan_start(x, y, RECT, 1.0));
        assert!(!state.can_start_pan(x, y, RECT, 1.0));
        state.on_pan_end();
        assert!(state.can_start_pan(x, y, RECT, 1.0));
    }
    #[test]
    fn zoom_pan_and_resize_are_view_only_and_use_document_coordinates() {
        for scale in [0.5, 1.0, 2.0] {
            let (mut state, mut data) = fixture(false);
            let original = data;
            let rect = (
                RECT.0 * scale,
                RECT.1 * scale,
                RECT.2 * scale,
                RECT.3 * scale,
            );
            let layout = state.layout(rect, scale);
            let point = (phase_to_x(&layout, 0.5), value_to_y(&layout, 0.5));
            assert!(state.on_scroll(point.0, point.1, 6.0, false, &data, rect, scale));
            assert!(state.on_scroll(point.0, point.1, 4.0, true, &data, rect, scale));
            let layout = state.layout(rect, scale);
            assert!((x_to_phase(&layout, point.0) - 0.5).abs() < 1e-6);
            assert!((y_to_value(&layout, point.1) - 0.5).abs() < 1e-6);
            let view = state.viewport();
            assert!(state.on_pan_start(point.0, point.1, rect, scale));
            assert_eq!(
                state.on_mouse_move(
                    point.0 + 40.0 * scale,
                    point.1 + 10.0 * scale,
                    &mut data,
                    rect,
                    scale,
                    false
                ),
                None
            );
            assert_ne!(state.viewport(), view);
            assert_eq!(
                state.on_key(&key(Key::Escape), &mut data, rect, scale),
                (true, None)
            );
            assert_eq!(state.viewport(), view);
            state.cancel_interaction();
            assert_eq!(state.viewport(), view);
            assert_eq!(data, original);
        }
    }
    #[test]
    fn drawing_clips_offscreen_curves_and_selection_handles() {
        let (mut state, data) = fixture(false);
        state.selection = 0b111;
        state.precision.viewport.fit((0.4, 0.6), (0.4, 0.6));
        let mut pm = tiny_skia::Pixmap::new(1120, 350).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(12, 24, 36, 255));
        let original = pm.clone();
        let mut text = crate::TextRenderer::new(crate::DEFAULT_FONT);
        crate::mseg::draw_mseg(
            &mut pm,
            &mut text,
            RECT,
            &data,
            &state,
            1.0,
            crate::color_accent(),
            None,
        );
        crate::mseg::draw_mseg_ghost(&mut pm, RECT, &data, &state, 1.0, 0xffaabb80);
        for y in 0..350 {
            for x in 0..1120 {
                if !in_rect(RECT, x as f32, y as f32) {
                    let i = (y * 1120 + x) * 4;
                    assert_eq!(
                        &pm.data()[i..i + 4],
                        &original.data()[i..i + 4],
                        "({x},{y})"
                    );
                }
            }
        }
    }
    #[test]
    fn zoomed_node_drag_snaps_in_document_space() {
        let (mut state, mut data) = fixture(false);
        state.selection = 0;
        data.snap = true;
        state.precision.viewport.fit((0.3, 0.7), (0.0, 1.0));
        let layout = state.layout(RECT, 1.0);
        state.on_mouse_down(
            phase_to_x(&layout, 0.5),
            value_to_y(&layout, 1.0),
            &mut data,
            RECT,
            1.0,
            false,
        );
        state.on_mouse_move(
            phase_to_x(&layout, 0.61),
            value_to_y(&layout, 0.75),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert_eq!(data.nodes[1].time, 0.625);
    }
}
