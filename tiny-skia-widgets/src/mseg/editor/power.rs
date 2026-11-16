use super::{DragTarget, MsegEdit, MsegEditState, StripId, transform_menu_items};
use crate::dropdown::{DropdownEvent, DropdownKey, DropdownList};
use crate::mseg::library::{CurveLibrary, FAVORITE_COUNT};
use crate::mseg::render::{MsegLayout, in_rect, phase_to_x, value_to_y, x_to_phase, y_to_value};
use crate::mseg::shape::{pen_nodes, value_range};
use crate::mseg::{
    CurveShape, FactoryShape, MsegData, MsegNode, MsegTool, ShapeError, factory_shape, tools,
};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectionHandle {
    NW,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
}

impl SelectionHandle {
    const ALL: [Self; 8] = [
        Self::NW,
        Self::N,
        Self::NE,
        Self::E,
        Self::SE,
        Self::S,
        Self::SW,
        Self::W,
    ];
    fn axes(self) -> (f32, f32) {
        match self {
            Self::NW => (-1.0, 1.0),
            Self::N => (0.0, 1.0),
            Self::NE => (1.0, 1.0),
            Self::E => (1.0, 0.0),
            Self::SE => (1.0, -1.0),
            Self::S => (0.0, -1.0),
            Self::SW => (-1.0, -1.0),
            Self::W => (-1.0, 0.0),
        }
    }
}

pub(super) struct PowerState {
    tool: MsegTool,
    pub(super) gesture_start: Option<MsegData>,
    press: (f32, f32),
    pen_size: (f32, f32),
    pen_pointer: (f32, f32),
    pen_cursor: (f32, f32),
    last_pen_pointer: Option<(f32, f32)>,
    next_pen_start: Option<(f32, f32)>,
    pen_repeating: bool,
    pub(super) preview: Vec<MsegNode>,
    shape_labels: Vec<String>,
    message: String,
    alt: bool,
    control: bool,
    library_directory: Option<std::path::PathBuf>,
}

impl Default for PowerState {
    fn default() -> Self {
        Self {
            tool: MsegTool::Pointer,
            gesture_start: None,
            press: (0.0, 0.0),
            pen_size: (0.125, 0.5),
            pen_pointer: (0.0, 0.0),
            pen_cursor: (0.0, 0.0),
            last_pen_pointer: None,
            next_pen_start: None,
            pen_repeating: false,
            preview: Vec::new(),
            shape_labels: Vec::new(),
            message: String::new(),
            alt: false,
            control: false,
            library_directory: None,
        }
    }
}

impl MsegEditState {
    /// Override shared curve storage for an embedded host or isolated editor.
    pub fn set_curve_library_directory(&mut self, directory: std::path::PathBuf) {
        self.power.library_directory = Some(directory);
    }

    pub(super) fn curve_library(&self) -> std::io::Result<CurveLibrary> {
        match &self.power.library_directory {
            Some(path) => Ok(CurveLibrary::at(path.clone())),
            None => CurveLibrary::user_library(),
        }
    }
    pub fn tool(&self) -> MsegTool {
        self.power.tool
    }
    pub fn set_tool(&mut self, tool: MsegTool) {
        self.cancel_interaction();
        self.power.tool = tool;
    }
    pub(crate) fn reset_pen_repeat(&mut self) {
        self.power.last_pen_pointer = None;
        self.power.next_pen_start = None;
    }

    pub(crate) fn finish_power_gesture(&mut self, data: &MsegData) {
        self.power.gesture_start = None;
        self.power.preview = if self.tool() != MsegTool::Pointer {
            self.power
                .next_pen_start
                .map(|start| self.pen_points(data, start, self.pen_end(start), true))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    }
    pub(crate) fn set_status_message(&mut self, text: impl Into<String>) {
        self.power.message = text.into();
    }
    pub fn status_message(&self) -> &str {
        &self.power.message
    }
    pub fn has_open_menu(&self) -> bool {
        self.dropdown.is_open() || self.inspector_node().is_some()
    }
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn set_transform_modifiers(&mut self, alt: bool, control: bool) {
        self.power.alt = alt;
        self.power.control = control;
    }
    pub fn pen_preview(&self) -> &[MsegNode] {
        &self.power.preview
    }

    pub(crate) fn dropdown_labels(&self) -> Vec<String> {
        let static_items = if self.dropdown.is_open_for(StripId::View) {
            super::precision::VIEW_ITEMS
        } else if self.dropdown.is_open_for(StripId::TimeGrid) {
            return self.grid_labels.clone();
        } else if self.dropdown.is_open_for(StripId::Shapes) {
            return self.power.shape_labels.clone();
        } else if self.dropdown.is_open_for(StripId::Tool) {
            &MsegTool::LABELS[..]
        } else if self.dropdown.is_open_for(StripId::Transform) {
            transform_menu_items()
        } else {
            super::style_items()
        };
        static_items.iter().map(|s| (*s).to_owned()).collect()
    }

    pub(crate) fn open_power_menu(
        &mut self,
        id: StripId,
        anchor: (f32, f32, f32, f32),
        window: (f32, f32),
    ) {
        if id == StripId::Shapes {
            self.power.shape_labels = FactoryShape::LABELS
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            self.power
                .shape_labels
                .extend((1..=FAVORITE_COUNT).map(|i| format!("Load favorite {i}")));
            self.power
                .shape_labels
                .extend((1..=FAVORITE_COUNT).map(|i| format!("Store favorite {i}")));
            self.power
                .shape_labels
                .extend(["Import shape...".to_owned(), "Export curve...".to_owned()]);
        }
        let labels: Vec<String> = match id {
            StripId::View => super::precision::VIEW_ITEMS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            StripId::Tool => MsegTool::LABELS.iter().map(|s| (*s).to_owned()).collect(),
            StripId::Shapes => self.power.shape_labels.clone(),
            _ => transform_menu_items()
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        };
        let refs: Vec<_> = labels.iter().map(String::as_str).collect();
        let selected = if id == StripId::Tool {
            MsegTool::ALL
                .iter()
                .position(|t| *t == self.tool())
                .unwrap_or(0)
        } else {
            0
        };
        self.dropdown.open(
            id,
            (
                anchor.0,
                anchor.1,
                if id == StripId::Transform {
                    anchor.2.max(anchor.3 * 9.0).max(160.0)
                } else {
                    anchor.2.max(anchor.3 * 9.0)
                },
                anchor.3,
            ),
            DropdownList::flat(&refs),
            selected,
            false,
            window,
        );
    }

    pub(crate) fn apply_menu(
        &mut self,
        id: StripId,
        index: usize,
        data: &mut MsegData,
    ) -> Option<MsegEdit> {
        self.power.message.clear();
        let result = match id {
            StripId::TimeGrid => {
                if let Some(&(t, v)) = self.grid_options.get(index) {
                    let changed = data.time_divisions != t || data.value_steps != v;
                    data.time_divisions = t;
                    data.value_steps = v;
                    Ok(changed)
                } else {
                    Ok(false)
                }
            }
            StripId::Style => {
                self.set_style(crate::mseg::RandomStyle::from_index(index));
                Ok(false)
            }
            StripId::Tool => {
                self.set_tool(MsegTool::ALL[index.min(3)]);
                Ok(false)
            }
            StripId::Shapes => return self.apply_shape_menu(index, data),
            StripId::View => {
                if index == 0 {
                    self.fit_all();
                } else {
                    self.fit_selection(data);
                }
                Ok(false)
            }
            StripId::Transform => match index {
                0 => return self.compress_values(data),
                1 => return self.expand_values(data),
                2 => return self.compress_times(data),
                3 => return self.expand_times(data),
                4 => return self.copy_curve(data),
                5 | 6 => return self.paste_curve(data, index == 6),
                7 => Ok(tools::flip_values(data, self.selection)),
                8 | 12 | 13 => {
                    let edit = if index == 8 {
                        tools::reverse(data, self.selection)
                    } else {
                        tools::repeat(data, self.selection, index - 10)
                    };
                    edit.map(|(changed, selected)| {
                        self.selection = selected;
                        self.hover = None;
                        changed
                    })
                }
                9 | 10 => tools::nudge(
                    data,
                    self.selection,
                    if index == 9 { -1.0 } else { 1.0 } / data.time_divisions.max(1) as f32,
                ),
                11 => tools::quantize(data, self.selection),
                14 | 15 => {
                    self.seed = self.seed.wrapping_add(1);
                    Ok(tools::mutate(data, self.selection, self.seed, index == 14))
                }
                16 => {
                    self.selection = tools::all_nodes(data.node_count);
                    Ok(false)
                }
                17 => {
                    self.open_point_inspector(data);
                    Ok(false)
                }
                18 => return self.copy_selected_phrase(data),
                19 | 20 => return self.paste_selected_phrase(data, index == 20),
                21..=23 => Ok(tools::set_segment_shape(
                    data,
                    self.selection,
                    if index == 22 {
                        crate::mseg::SegmentShape::SCurve
                    } else {
                        crate::mseg::SegmentShape::Curve
                    },
                    index == 23,
                )),
                24..=26 => Ok(tools::set_point_weight(
                    data,
                    self.selection,
                    [
                        crate::mseg::PointWeight::Hard,
                        crate::mseg::PointWeight::Medium,
                        crate::mseg::PointWeight::Soft,
                    ][index - 24],
                )),
                27 => return self.match_loop_end_to_start(data),
                28 => return self.match_loop_start_to_end(data),
                29 => return self.smooth_loop_seam(data),
                _ => Ok(false),
            },
            _ => Ok(false),
        };
        self.finish_power_edit(result)
    }

    /// Match the loop's terminal node level to its initial node level.
    pub fn match_loop_end_to_start(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        self.match_loop_endpoint(data, true)
    }

    /// Match the loop's initial node level to its terminal node level.
    pub fn match_loop_start_to_end(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        self.match_loop_endpoint(data, false)
    }

    fn match_loop_endpoint(&mut self, data: &mut MsegData, end_to_start: bool) -> Option<MsegEdit> {
        self.power.message.clear();
        let changed = crate::mseg::seam::match_endpoint(data, self.curve_only, end_to_start);
        if changed {
            self.cancel_interaction();
        }
        self.finish_power_edit(Ok(changed))
    }

    /// Add an authored S-curve return and flatten the loop's starting tangent.
    pub fn smooth_loop_seam(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        self.power.message.clear();
        let result = crate::mseg::seam::smooth(data, self.curve_only, self.selection).map(
            |(changed, selection)| {
                if changed {
                    self.cancel_interaction();
                    self.selection = selection;
                }
                changed
            },
        );
        self.finish_power_edit(result)
    }

    pub(super) fn finish_power_edit(
        &mut self,
        result: Result<bool, ShapeError>,
    ) -> Option<MsegEdit> {
        match result {
            Ok(true) => Some(MsegEdit::Changed),
            Ok(false) => None,
            Err(err) => {
                self.power.message = err.to_string();
                None
            }
        }
    }

    fn apply_shape(
        &mut self,
        data: &mut MsegData,
        shape: CurveShape,
        fit: bool,
    ) -> Option<MsegEdit> {
        let result = shape.apply(data, fit);
        if result.is_ok() {
            self.cancel_interaction();
            self.clear_selection();
        }
        self.finish_power_edit(result)
    }

    fn copy_curve(&mut self, data: &MsegData) -> Option<MsegEdit> {
        match self
            .curve_library()
            .and_then(|library| library.copy(&CurveShape::capture(data)))
        {
            Ok(()) => self.power.message = "Curve copied".to_owned(),
            Err(err) => self.power.message = format!("Copy failed: {err}"),
        }
        None
    }

    fn paste_curve(&mut self, data: &mut MsegData, fit: bool) -> Option<MsegEdit> {
        match self.curve_library().and_then(|library| library.paste()) {
            Ok(shape) => self.apply_shape(data, shape, fit),
            Err(err) => {
                self.power.message = format!("Paste failed: {err}");
                None
            }
        }
    }

    fn apply_shape_menu(&mut self, index: usize, data: &mut MsegData) -> Option<MsegEdit> {
        if let Some(kind) = FactoryShape::ALL.get(index) {
            return self.apply_shape(data, factory_shape(*kind), false);
        }
        let slot = index.saturating_sub(FactoryShape::ALL.len());
        if slot < FAVORITE_COUNT {
            match self.curve_library().and_then(|l| l.load_favorite(slot)) {
                Ok(shape) => return self.apply_shape(data, shape, false),
                Err(err) => {
                    self.power.message = format!("Favorite {} unavailable: {err}", slot + 1)
                }
            }
        } else if slot < 2 * FAVORITE_COUNT {
            match self
                .curve_library()
                .and_then(|l| l.store_favorite(slot - FAVORITE_COUNT, &CurveShape::capture(data)))
            {
                Ok(()) => {
                    self.power.message = format!("Favorite {} stored", slot - FAVORITE_COUNT + 1)
                }
                Err(err) => self.power.message = format!("Store failed: {err}"),
            }
        } else if slot == 2 * FAVORITE_COUNT {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Tract curve", &["json"])
                .pick_file()
            {
                match CurveLibrary::import(&path) {
                    Ok(shape) => return self.apply_shape(data, shape, false),
                    Err(err) => self.power.message = format!("Import failed: {err}"),
                }
            }
        } else if slot == 2 * FAVORITE_COUNT + 1
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Tract curve", &["json"])
                .set_file_name("curve.json")
                .save_file()
        {
            match CurveLibrary::export(&path, &CurveShape::capture(data)) {
                Ok(()) => self.power.message = "Curve exported".to_owned(),
                Err(err) => self.power.message = format!("Export failed: {err}"),
            }
        }
        None
    }

    pub fn on_wheel(&mut self, delta: f32, rect: (f32, f32, f32, f32)) -> bool {
        if !self.has_open_menu() {
            return false;
        }
        if self.inspector_node().is_some() {
            return true;
        }
        let labels = self.dropdown_labels();
        let refs: Vec<_> = labels.iter().map(String::as_str).collect();
        self.dropdown
            .on_wheel(delta, &refs, (rect.0 + rect.2, rect.1 + rect.3));
        true
    }

    pub fn on_key(
        &mut self,
        event: &KeyboardEvent,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> (bool, Option<MsegEdit>) {
        self.prepare_precision(data, scale);
        if self.inspector_node().is_some() {
            return self.inspector_key(event, data);
        }
        if self.has_open_menu() {
            if event.state != KeyState::Down {
                return (true, None);
            }
            let labels = self.dropdown_labels();
            let refs: Vec<_> = labels.iter().map(String::as_str).collect();
            let key = match event.key {
                Key::ArrowUp => Some(DropdownKey::Up),
                Key::ArrowDown => Some(DropdownKey::Down),
                Key::Enter => Some(DropdownKey::Enter),
                Key::Escape => Some(DropdownKey::Esc),
                Key::Home => Some(DropdownKey::Home),
                Key::End => Some(DropdownKey::End),
                Key::PageUp => Some(DropdownKey::PageUp),
                Key::PageDown => Some(DropdownKey::PageDown),
                _ => None,
            };
            let result = key.and_then(|k| {
                self.dropdown
                    .on_key(k, &refs, (rect.0 + rect.2, rect.1 + rect.3))
            });
            let edit = if let Some(DropdownEvent::Selected(id, index)) = result {
                self.apply_menu(id, index, data)
            } else {
                None
            };
            return (true, edit);
        }
        if event.state != KeyState::Down {
            return (false, None);
        }
        if event.key == Key::Escape {
            if self.cancel_pan() {
                return (true, None);
            }
            let changed = if let Some(start) = self.power.gesture_start {
                let changed = *data != start;
                *data = start;
                changed
            } else {
                false
            };
            let captured =
                self.drag.is_some() || self.selection != 0 || self.tool() != MsegTool::Pointer;
            self.cancel_interaction();
            self.clear_selection();
            self.power.tool = MsegTool::Pointer;
            return (captured, changed.then_some(MsegEdit::Changed));
        }
        if self.is_dragging() {
            return (false, None);
        }
        let command = event
            .modifiers
            .intersects(Modifiers::CONTROL | Modifiers::META);
        if command && let Key::Character(key) = &event.key {
            if event.modifiers.contains(Modifiers::ALT) {
                let edit = match key.to_ascii_lowercase().as_str() {
                    "c" => self.copy_selected_phrase(data),
                    "v" => {
                        self.paste_selected_phrase(data, event.modifiers.contains(Modifiers::SHIFT))
                    }
                    _ => return (false, None),
                };
                return (true, edit);
            }
            let edit = match key.to_ascii_lowercase().as_str() {
                "a" => self.apply_menu(StripId::Transform, 16, data),
                "c" => self.copy_curve(data),
                "v" => self.paste_curve(data, event.modifiers.contains(Modifiers::SHIFT)),
                _ => return (false, None),
            };
            return (true, edit);
        }
        match event.key {
            Key::Enter if command && self.selection_count() == 1 => {
                (self.open_point_inspector(data), None)
            }
            Key::Delete | Key::Backspace if self.selection != 0 => {
                let edit = self.delete_selection(data);
                (edit.is_some(), edit)
            }
            Key::ArrowLeft | Key::ArrowRight if self.selection != 0 => {
                let delta = if event.key == Key::ArrowLeft {
                    -1.0
                } else {
                    1.0
                } / data.time_divisions.max(1) as f32
                    / if event.modifiers.contains(Modifiers::SHIFT) {
                        16.0
                    } else {
                        1.0
                    };
                let result = tools::nudge(data, self.selection, delta);
                (true, self.finish_power_edit(result))
            }
            _ => (false, None),
        }
    }

    fn selected_bounds(&self, data: &MsegData) -> Option<(f32, f32, f32, f32)> {
        if self.selection_count() < 2 {
            return None;
        }
        let selected: Vec<_> = data
            .active()
            .iter()
            .enumerate()
            .filter(|(i, _)| self.is_node_selected(*i))
            .map(|(_, n)| *n)
            .collect();
        if selected.len() < 2 {
            return None;
        }
        let (lo, hi) = value_range(&selected);
        Some((selected.first()?.time, selected.last()?.time, lo, hi))
    }

    pub(crate) fn selection_handles(
        &self,
        data: &MsegData,
        layout: &MsegLayout,
        scale: f32,
    ) -> Vec<(SelectionHandle, (f32, f32))> {
        if self.tool() != MsegTool::Pointer {
            return Vec::new();
        }
        let Some((left, right, low, high)) = self.selected_bounds(data) else {
            return Vec::new();
        };
        let pad = 10.0 * scale;
        let x0 = phase_to_x(layout, left) - pad;
        let x1 = phase_to_x(layout, right) + pad;
        let y0 = value_to_y(layout, high) - pad;
        let y1 = value_to_y(layout, low) + pad;
        SelectionHandle::ALL
            .iter()
            .map(|handle| {
                let (x, y) = handle.axes();
                (
                    *handle,
                    (
                        (x0 + x1) * 0.5 + x * (x1 - x0) * 0.5,
                        (y0 + y1) * 0.5 - y * (y1 - y0) * 0.5,
                    ),
                )
            })
            .collect()
    }

    pub(crate) fn begin_power_drag(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        layout: &MsegLayout,
        scale: f32,
    ) -> Option<Option<MsegEdit>> {
        if self.tool() == MsegTool::Pointer {
            let hit = self
                .selection_handles(data, layout, scale)
                .into_iter()
                .find(|(_, (hx, hy))| {
                    in_rect(layout.canvas, *hx, *hy)
                        && (x - hx).abs() <= 5.0 * scale
                        && (y - hy).abs() <= 5.0 * scale
                });
            if let Some((handle, _)) = hit {
                self.power.gesture_start = Some(*data);
                self.power.press = (x_to_phase(layout, x), y_to_value(layout, y));
                self.drag = Some(DragTarget::Transform(handle));
                return Some(None);
            }
        } else if in_rect(layout.canvas, x, y) {
            self.power.gesture_start = Some(*data);
            self.power.pen_cursor = (x_to_phase(layout, x), y_to_value(layout, y));
            let pointer =
                super::snap_point(x_to_phase(layout, x), y_to_value(layout, y), data, false);
            self.power.pen_pointer = pointer;
            self.power.pen_repeating = self.power.last_pen_pointer == Some(self.power.pen_cursor);
            self.power.press = if self.power.pen_repeating {
                self.power.next_pen_start.unwrap_or(pointer)
            } else {
                pointer
            };
            self.drag = Some(DragTarget::Pen);
            let end = self.pen_end(self.power.press);
            return Some(self.stamp_pen(data, end));
        }
        None
    }

    fn pen_end(&self, start: (f32, f32)) -> (f32, f32) {
        let mut end = (
            start.0 + self.power.pen_size.0,
            start.1 + self.power.pen_size.1,
        );
        end.0 = end.0.clamp(0.0, 1.0);
        end.1 = end.1.clamp(0.0, 1.0);
        (end.0, end.1)
    }

    fn pen_points(
        &self,
        data: &MsegData,
        start: (f32, f32),
        end: (f32, f32),
        repeating: bool,
    ) -> Vec<MsegNode> {
        let mut points = pen_nodes(self.tool(), start, end);
        if repeating
            && let Some(boundary) = data.active().iter().find(|n| n.time == start.0).copied()
        {
            let local_span = if end.0 > start.0 {
                points[1].time - points[0].time
            } else {
                points[points.len() - 1].time - points[points.len() - 2].time
            };
            let gap = (local_span * 0.25).min(1e-4);
            if end.0 > start.0 && boundary.value != points[0].value {
                points[0].time += gap;
                points.insert(
                    0,
                    MsegNode {
                        tension: 0.0,
                        stepped: false,
                        shape: crate::mseg::SegmentShape::Curve,
                        ..boundary
                    },
                );
            } else if end.0 < start.0 && boundary.value != points[points.len() - 1].value {
                let last = points.len() - 1;
                points[last].time -= gap;
                points[last].tension = 0.0;
                points[last].stepped = false;
                points[last].shape = crate::mseg::SegmentShape::Curve;
                points.push(boundary);
            }
        }
        tools::reconcile_interval_metadata(data, &mut points);
        points
    }

    fn stamp_pen(&mut self, data: &mut MsegData, end: (f32, f32)) -> Option<MsegEdit> {
        let mut candidate = self.power.gesture_start?;
        let points = self.pen_points(&candidate, self.power.press, end, self.power.pen_repeating);
        if (end.0 - self.power.press.0).abs() < 1e-4 {
            self.power.message = "No room for another pen shape".to_owned();
            return None;
        }
        match tools::replace_interval(&mut candidate, &points) {
            Ok((_, selection)) => {
                let changed = candidate != *data;
                *data = candidate;
                self.selection = selection;
                self.hover = None;
                self.power.preview = points;
                self.power.message.clear();
                self.power.last_pen_pointer = Some(self.power.pen_cursor);
                self.power.next_pen_start = Some((end.0, self.power.press.1));
                changed.then_some(MsegEdit::Changed)
            }
            Err(err) => {
                self.power.message = err.to_string();
                None
            }
        }
    }

    pub(crate) fn move_power_drag(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        layout: &MsegLayout,
        fine: bool,
    ) -> Option<MsegEdit> {
        self.power.pen_cursor = (x_to_phase(layout, x), y_to_value(layout, y));
        let pointer = super::snap_point(x_to_phase(layout, x), y_to_value(layout, y), data, fine);
        if self.drag == Some(DragTarget::Pen) {
            let end = (
                (self.power.press.0 + pointer.0 - self.power.pen_pointer.0).clamp(0.0, 1.0),
                (self.power.press.1 + pointer.1 - self.power.pen_pointer.1).clamp(0.0, 1.0),
            );
            if (end.0 - self.power.press.0).abs() > 1e-4 {
                let result = self.stamp_pen(data, end);
                if self.power.message.is_empty() {
                    self.power.pen_size = (end.0 - self.power.press.0, end.1 - self.power.press.1);
                }
                return result;
            }
        } else if let Some(DragTarget::Transform(handle)) = self.drag {
            let mut pointer = (x_to_phase(layout, x), y_to_value(layout, y));
            if data.snap
                && !fine
                && let Some(original) = self.power.gesture_start
                && let Some((lo, hi, bottom, top)) = self.selected_bounds(&original)
            {
                let (sx, sy) = handle.axes();
                let dx = pointer.0 - self.power.press.0;
                let dy = pointer.1 - self.power.press.1;
                if sx != 0.0 && dx.abs() > 1e-6 {
                    let edge = if sx < 0.0 { lo } else { hi };
                    let cols = data.time_divisions.max(1) as f32;
                    pointer.0 = self.power.press.0 + ((edge + dx) * cols).round() / cols - edge;
                }
                if sy != 0.0 && dy.abs() > 1e-6 {
                    let edge = if sy < 0.0 { bottom } else { top };
                    let rows = data.value_steps.max(1) as f32;
                    pointer.1 = self.power.press.1 + ((edge + dy) * rows).round() / rows - edge;
                }
            }
            return self.transform_selection(handle, pointer, data);
        } else if self.drag.is_none() {
            if self.power.last_pen_pointer != Some(self.power.pen_cursor) {
                self.reset_pen_repeat();
            }
            let start = self.power.next_pen_start.unwrap_or(pointer);
            self.power.preview = if self.tool() != MsegTool::Pointer && in_rect(layout.canvas, x, y)
            {
                self.pen_points(
                    data,
                    start,
                    self.pen_end(start),
                    self.power.next_pen_start.is_some(),
                )
            } else {
                Vec::new()
            };
        }
        None
    }

    fn transform_selection(
        &mut self,
        handle: SelectionHandle,
        pointer: (f32, f32),
        data: &mut MsegData,
    ) -> Option<MsegEdit> {
        let original = self.power.gesture_start?;
        let (lo, hi, bottom, top) = self.selected_bounds(&original)?;
        let (sx, sy) = handle.axes();
        let dx = pointer.0 - self.power.press.0;
        let dy = pointer.1 - self.power.press.1;
        if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
            let changed = *data != original;
            *data = original;
            return changed.then_some(MsegEdit::Changed);
        }
        let x_pivot = if self.power.alt {
            (lo + hi) * 0.5
        } else if sx < 0.0 {
            hi
        } else {
            lo
        };
        let y_pivot = if self.power.alt {
            (bottom + top) * 0.5
        } else if sy < 0.0 {
            top
        } else {
            bottom
        };
        let edge_x = if sx < 0.0 { lo } else { hi };
        let edge_y = if sy < 0.0 { bottom } else { top };
        let x_scale = if sx != 0.0 && (edge_x - x_pivot).abs() > 1e-6 {
            ((edge_x + dx - x_pivot) / (edge_x - x_pivot)).max(0.001)
        } else {
            1.0
        };
        let y_scale = if sy != 0.0 && (edge_y - y_pivot).abs() > 1e-6 {
            ((edge_y + dy - y_pivot) / (edge_y - y_pivot)).max(0.001)
        } else {
            1.0
        };
        let mut candidate = original;
        for (i, n) in candidate.nodes[..candidate.node_count]
            .iter_mut()
            .enumerate()
        {
            if !self.is_node_selected(i) {
                continue;
            }
            if self.power.control && (sx == 0.0 || sy == 0.0) {
                if sx != 0.0 {
                    let width = edge_x - x_pivot;
                    if width.abs() > 1e-6 {
                        n.value += dy * (n.time - x_pivot) / width;
                    }
                } else {
                    let height = edge_y - y_pivot;
                    if height.abs() > 1e-6 {
                        n.time += dx * (n.value - y_pivot) / height;
                    }
                }
            } else {
                if sx != 0.0 {
                    n.time = x_pivot + (n.time - x_pivot) * x_scale;
                }
                if sy != 0.0 {
                    n.value = if top - bottom > 1e-6 {
                        y_pivot + (n.value - y_pivot) * y_scale
                    } else {
                        n.value + dy
                    };
                }
            }
            n.value = n.value.clamp(0.0, 1.0);
        }
        candidate.nodes[0].time = 0.0;
        candidate.nodes[candidate.node_count - 1].time = 1.0;
        let mut amount = 1.0_f32;
        for i in 1..candidate.node_count {
            let gap = original.nodes[i].time - original.nodes[i - 1].time;
            let next_gap = candidate.nodes[i].time - candidate.nodes[i - 1].time;
            let minimum = gap.min(1e-4) * 0.5;
            if next_gap < minimum {
                amount = amount.min((gap - minimum) / (gap - next_gap));
            }
        }
        for i in 1..candidate.node_count - 1 {
            candidate.nodes[i].time = original.nodes[i].time
                + (candidate.nodes[i].time - original.nodes[i].time) * amount;
        }
        if candidate.is_valid() && candidate != *data {
            *data = candidate;
            Some(MsegEdit::Changed)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::render::{mseg_layout, power_buttons};

    const RECT: (f32, f32, f32, f32) = (0.0, 0.0, 1100.0, 330.0);

    fn key(key: Key, modifiers: Modifiers) -> KeyboardEvent {
        KeyboardEvent {
            key,
            modifiers,
            state: KeyState::Down,
            ..KeyboardEvent::default()
        }
    }

    fn fixture() -> MsegData {
        let mut data = MsegData {
            snap: false,
            ..MsegData::default()
        };
        data.nodes[0].value = 0.4;
        data.nodes[1].value = 0.4;
        for (t, v) in [(0.25, 0.2), (0.5, 0.8), (0.75, 0.4)] {
            data.insert_node(t, v).unwrap();
        }
        data
    }

    #[test]
    fn pens_stamp_drag_preview_and_repeat_without_double_click_deletion() {
        for tool in [MsegTool::Line, MsegTool::Arc, MsegTool::SCurve] {
            let mut data = fixture();
            let mut state = MsegEditState::new_curve_only();
            state.set_tool(tool);
            let layout = mseg_layout(RECT, true, 1.0);
            let (x, y) = (phase_to_x(&layout, 0.1), value_to_y(&layout, 0.2));
            assert_eq!(state.on_mouse_move(x, y, &mut data, RECT, 1.0, false), None);
            assert!(state.pen_preview().len() >= 2);
            assert_eq!(
                state.on_mouse_down(x, y, &mut data, RECT, 1.0, false),
                Some(MsegEdit::Changed)
            );
            let end = (phase_to_x(&layout, 0.3), value_to_y(&layout, 0.9));
            assert_eq!(
                state.on_mouse_move(end.0, end.1, &mut data, RECT, 1.0, false),
                Some(MsegEdit::Changed)
            );
            state.on_mouse_up(&mut data, RECT, 1.0);
            assert!(data.is_valid());
            assert!(
                data.active()
                    .iter()
                    .any(|n| (n.time - 0.3).abs() < 1e-5 && (n.value - 0.9).abs() < 1e-5)
            );
            let x = phase_to_x(&layout, 0.55);
            assert_eq!(
                state.on_double_click(x, y, &mut data, RECT, 1.0),
                Some(MsegEdit::Changed)
            );
            state.on_mouse_up(&mut data, RECT, 1.0);
            assert!(
                data.active()
                    .iter()
                    .any(|n| (n.time - 0.75).abs() < 1e-5 && (n.value - 0.9).abs() < 1e-5)
            );
            assert!(data.is_valid());
        }
    }

    #[test]
    fn hover_drag_and_remembered_pen_previews_match_preserved_boundary_weights() {
        fn preview_at(points: &[MsegNode], phase: f32) -> f32 {
            for pair in points.windows(2) {
                if phase < pair[1].time {
                    return crate::mseg::segment_value(&pair[0], &pair[1], phase);
                }
            }
            points.last().unwrap().value
        }
        let mut data = MsegData {
            snap: false,
            ..MsegData::default()
        };
        factory_shape(FactoryShape::Triangle)
            .apply(&mut data, false)
            .unwrap();
        data.nodes[1].weight = crate::mseg::PointWeight::Soft;
        let mut state = MsegEditState::new();
        state.set_tool(MsegTool::Line);
        state.power.pen_size = (0.5, 1.0);
        let layout = mseg_layout(RECT, false, 1.0);
        let start = (phase_to_x(&layout, 0.0), value_to_y(&layout, 0.0));
        let end = (phase_to_x(&layout, 0.5), value_to_y(&layout, 1.0));
        state.on_mouse_move(start.0, start.1, &mut data, RECT, 1.0, false);
        assert_eq!(preview_at(state.pen_preview(), 0.25), 0.625);
        state.on_mouse_down(start.0, start.1, &mut data, RECT, 1.0, false);
        state.on_mouse_move(end.0, end.1, &mut data, RECT, 1.0, false);
        assert_eq!(
            preview_at(state.pen_preview(), 0.25),
            crate::mseg::value_at_phase(&data, 0.25)
        );
        state.on_mouse_up(&mut data, RECT, 1.0);
        let remembered = state.pen_preview().to_vec();
        state.on_mouse_down(end.0, end.1, &mut data, RECT, 1.0, false);
        for phase in [0.50005, 0.75] {
            assert_eq!(
                preview_at(&remembered, phase),
                crate::mseg::value_at_phase(&data, phase)
            );
        }
    }

    #[test]
    fn pen_overflow_preserves_curve_and_selection() {
        let mut data = MsegData {
            node_count: 128,
            snap: false,
            ..MsegData::default()
        };
        for (i, n) in data.nodes.iter_mut().enumerate() {
            n.time = i as f32 / 127.0;
        }
        let before = data;
        let mut state = MsegEditState::new();
        state.selection = 0b110;
        state.set_tool(MsegTool::SCurve);
        state.power.pen_size = (0.001, 0.5);
        let layout = mseg_layout(RECT, false, 1.0);
        assert_eq!(
            state.on_mouse_down(
                phase_to_x(&layout, 0.1),
                value_to_y(&layout, 0.2),
                &mut data,
                RECT,
                1.0,
                false
            ),
            None
        );
        assert_eq!(data, before);
        assert_eq!(state.selection, 0b110);
        assert!(state.status_message().contains("128"));
    }

    #[test]
    fn repeated_pen_clicks_at_the_same_position_advance_without_erasing_previous_shapes() {
        let mut data = fixture();
        let mut state = MsegEditState::new_curve_only();
        state.set_tool(MsegTool::Line);
        let layout = mseg_layout(RECT, true, 1.0);
        let x = phase_to_x(&layout, 0.1);
        let y = value_to_y(&layout, 0.2);
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        state.on_mouse_up(&mut data, RECT, 1.0);
        let first_sample = crate::mseg::value_at_phase(&data, 0.16);
        for end in [0.35, 0.475] {
            assert_eq!(
                state.on_double_click(x, y, &mut data, RECT, 1.0),
                Some(MsegEdit::Changed)
            );
            state.on_mouse_up(&mut data, RECT, 1.0);
            assert!((crate::mseg::value_at_phase(&data, 0.16) - first_sample).abs() < 1e-6);
            assert!(
                data.active()
                    .iter()
                    .any(|n| (n.time - end).abs() < 1e-5 && (n.value - 0.7).abs() < 1e-5)
            );
            assert!(data.is_valid());
        }
    }

    #[test]
    fn short_s_curve_pens_repeat_in_both_directions_and_remain_editable() {
        for direction in [-1.0, 1.0] {
            let mut data = fixture();
            let mut state = MsegEditState::new_curve_only();
            state.set_tool(MsegTool::SCurve);
            let layout = mseg_layout(RECT, true, 1.0);
            let start = (phase_to_x(&layout, 0.4), value_to_y(&layout, 0.2));
            state.on_mouse_down(start.0, start.1, &mut data, RECT, 1.0, false);
            state.on_mouse_move(
                phase_to_x(&layout, 0.4 + direction * 0.001),
                value_to_y(&layout, 0.8),
                &mut data,
                RECT,
                1.0,
                true,
            );
            state.on_mouse_up(&mut data, RECT, 1.0);
            assert_eq!(
                state.on_mouse_down(
                    phase_to_x(&layout, 0.4 + direction * 0.001),
                    value_to_y(&layout, 0.8),
                    &mut data,
                    RECT,
                    1.0,
                    false
                ),
                Some(MsegEdit::Changed)
            );
            state.on_mouse_up(&mut data, RECT, 1.0);
            state.set_tool(MsegTool::Pointer);
            assert_eq!(state.compress_times(&mut data), Some(MsegEdit::Changed));
            assert_eq!(state.expand_times(&mut data), Some(MsegEdit::Changed));
            assert!(data.is_valid());
            let node = state.selection.trailing_zeros() as usize + 1;
            data.move_node(node, 1.0, 0.5);
            assert!(data.is_valid());
        }
    }

    #[test]
    fn pen_repeats_after_a_drag_at_the_released_pointer_position() {
        let mut data = fixture();
        let mut state = MsegEditState::new_curve_only();
        state.set_tool(MsegTool::Line);
        let layout = mseg_layout(RECT, true, 1.0);
        let start = (phase_to_x(&layout, 0.1), value_to_y(&layout, 0.2));
        let end = (phase_to_x(&layout, 0.3), value_to_y(&layout, 0.9));
        state.on_mouse_down(start.0, start.1, &mut data, RECT, 1.0, false);
        state.on_mouse_move(end.0, end.1, &mut data, RECT, 1.0, true);
        state.on_mouse_up(&mut data, RECT, 1.0);
        state.on_mouse_move(end.0, end.1, &mut data, RECT, 1.0, true);
        assert_eq!(
            state.on_mouse_down(end.0, end.1, &mut data, RECT, 1.0, false),
            Some(MsegEdit::Changed)
        );
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(
            data.active()
                .iter()
                .any(|n| (n.time - 0.5).abs() < 1e-5 && (n.value - 0.9).abs() < 1e-5)
        );
        assert!((crate::mseg::value_at_phase(&data, 0.4) - 0.55).abs() < 0.001);
    }

    #[test]
    fn pen_repeat_keeps_its_shape_when_fine_adjustment_is_released() {
        let mut data = fixture();
        data.snap = true;
        let mut state = MsegEditState::new_curve_only();
        state.set_tool(MsegTool::Line);
        let layout = mseg_layout(RECT, true, 1.0);
        let start = (phase_to_x(&layout, 0.125), value_to_y(&layout, 0.25));
        let end = (phase_to_x(&layout, 0.37), value_to_y(&layout, 0.9));
        state.on_mouse_down(start.0, start.1, &mut data, RECT, 1.0, false);
        state.on_mouse_move(end.0, end.1, &mut data, RECT, 1.0, true);
        state.on_mouse_up(&mut data, RECT, 1.0);
        state.on_mouse_move(end.0, end.1, &mut data, RECT, 1.0, false);
        assert_eq!(
            state.on_mouse_down(end.0, end.1, &mut data, RECT, 1.0, false),
            Some(MsegEdit::Changed)
        );
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(
            data.active()
                .iter()
                .any(|n| (n.time - 0.615).abs() < 1e-5 && (n.value - 0.9).abs() < 1e-5)
        );
    }

    #[test]
    fn selection_handles_scale_from_snapshot_and_return_exactly_after_clamping() {
        for scale in [0.5, 1.0, 2.0] {
            let rect = (0.0, 0.0, RECT.2 * scale, RECT.3 * scale);
            let mut data = fixture();
            let original = data;
            let mut state = MsegEditState::new();
            state.selection = 0b1110;
            let layout = mseg_layout(rect, false, scale);
            let (_, point) = state
                .selection_handles(&data, &layout, scale)
                .into_iter()
                .find(|(h, _)| *h == SelectionHandle::E)
                .unwrap();
            state.on_mouse_down(point.0, point.1, &mut data, rect, scale, false);
            assert_eq!(state.drag, Some(DragTarget::Transform(SelectionHandle::E)));
            assert_eq!(
                state.on_mouse_move(point.0 + 10000.0, point.1, &mut data, rect, scale, false),
                Some(MsegEdit::Changed)
            );
            assert!(data.nodes[3].time < 1.0);
            assert!(data.is_valid());
            assert_eq!(
                state.on_mouse_move(point.0, point.1, &mut data, rect, scale, false),
                Some(MsegEdit::Changed)
            );
            assert_eq!(data, original);
            state.on_mouse_up(&mut data, rect, scale);
            assert!(!state.is_dragging());
        }
    }

    #[test]
    fn centered_scaling_and_skew_preserve_unselected_nodes() {
        for skew in [false, true] {
            let mut data = fixture();
            let before = data;
            let mut state = MsegEditState::new();
            state.selection = 0b1110;
            state.set_transform_modifiers(!skew, skew);
            let layout = mseg_layout(RECT, false, 1.0);
            let (_, p) = state
                .selection_handles(&data, &layout, 1.0)
                .into_iter()
                .find(|(h, _)| *h == SelectionHandle::E)
                .unwrap();
            state.on_mouse_down(p.0, p.1, &mut data, RECT, 1.0, false);
            state.on_mouse_move(
                p.0 + layout.plot.2 * 0.05,
                p.1 - layout.plot.3 * 0.1,
                &mut data,
                RECT,
                1.0,
                true,
            );
            assert!(data.is_valid());
            assert_eq!(data.nodes[0], before.nodes[0]);
            assert_eq!(data.nodes[4], before.nodes[4]);
            if skew {
                assert_eq!(data.nodes[1].time, before.nodes[1].time);
                assert!(data.nodes[3].value > before.nodes[3].value);
            } else {
                assert!(data.nodes[1].time < before.nodes[1].time);
                assert!(data.nodes[3].time > before.nodes[3].time);
            }
        }
    }

    #[test]
    fn escape_rolls_back_a_pen_gesture_and_cancels_its_tool() {
        let mut data = fixture();
        let original = data;
        let mut state = MsegEditState::new();
        state.set_tool(MsegTool::Arc);
        let layout = mseg_layout(RECT, false, 1.0);
        state.on_mouse_down(
            phase_to_x(&layout, 0.2),
            value_to_y(&layout, 0.7),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert_ne!(data, original);
        assert_eq!(
            state.on_key(&key(Key::Escape, Modifiers::empty()), &mut data, RECT, 1.0),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(data, original);
        assert!(!state.is_dragging());
        assert_eq!(state.tool(), MsegTool::Pointer);
    }

    #[test]
    fn shapes_and_edit_menus_accept_keyboard_and_wheel_input() {
        let mut state = MsegEditState::new();
        let mut data = fixture();
        let original = data;
        let buttons = power_buttons(mseg_layout(RECT, false, 1.0).strip, 1.0);
        state.on_mouse_down(
            buttons.shapes.0 + 4.0,
            buttons.shapes.1 + 4.0,
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(state.has_open_menu());
        assert!(state.on_wheel(-5.0, RECT));
        state.on_key(&key(Key::Home, Modifiers::empty()), &mut data, RECT, 1.0);
        assert_eq!(
            state.on_key(&key(Key::Enter, Modifiers::empty()), &mut data, RECT, 1.0),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(data.active(), factory_shape(FactoryShape::FlatHigh).nodes());
        assert_eq!(data.beats, original.beats);
        assert!(!state.has_open_menu());
    }

    #[test]
    fn loop_seam_commands_use_the_shared_menu_in_full_and_static_editors() {
        for curve_only in [false, true] {
            for scale in [0.5, 1.0, 2.0] {
                let rect = (10.0 * scale, 10.0 * scale, 1200.0 * scale, 400.0 * scale);
                let mut state = MsegEditState::with_curve_only(curve_only);
                let mut data = MsegData::default();
                let buttons = power_buttons(state.layout(rect, scale).strip, scale);
                assert_eq!(
                    state.on_mouse_down(
                        buttons.edit.0 + scale,
                        buttons.edit.1 + scale,
                        &mut data,
                        rect,
                        scale,
                        false
                    ),
                    None
                );
                assert!(state.has_open_menu());
                assert!(state.on_wheel(-3.0, rect));
                assert_eq!(
                    state.on_key(&key(Key::End, Modifiers::empty()), &mut data, rect, scale),
                    (true, None)
                );
                assert_eq!(
                    state.on_key(&key(Key::Enter, Modifiers::empty()), &mut data, rect, scale),
                    (true, Some(MsegEdit::Changed))
                );
                assert!(!state.has_open_menu());
                assert_eq!(data.node_count, 3);
                assert_eq!(data.nodes[0].value, data.nodes[2].value);
                assert_eq!(state.smooth_loop_seam(&mut data), None);
            }
        }
    }

    #[test]
    fn loop_seam_edits_remap_selection_and_fail_without_losing_it() {
        let mut state = MsegEditState::new();
        let mut data = MsegData::default();
        state.toggle_selected_for_test(1);
        assert_eq!(state.smooth_loop_seam(&mut data), Some(MsegEdit::Changed));
        assert!(state.is_node_selected(2));
        assert_eq!(state.selection_count(), 1);
        assert!(!state.is_node_selected(1));
        data.nodes[1].time = 1.0_f32.next_down();
        data.nodes[2].value = 1.0;
        let before = data;
        assert_eq!(state.smooth_loop_seam(&mut data), None);
        assert_eq!(data, before);
        assert!(state.is_node_selected(2));
        assert!(!state.status_message().is_empty());
    }

    #[test]
    fn loop_seam_labels_fit_toolbar_and_context_menus_at_each_scale() {
        use crate::dropdown::{RowKind, dropdown_popup_layout};
        let mut text =
            crate::TextRenderer::new(include_bytes!("../../../test_data/DejaVuSans.ttf"));
        for scale in [0.5, 1.0, 2.0] {
            for context in [false, true] {
                let rect = (0.0, 0.0, 1200.0 * scale, 500.0 * scale);
                let mut state = MsegEditState::new();
                let mut data = fixture();
                if context {
                    state.select_only(1);
                    let layout = state.layout(rect, scale);
                    let node = data.nodes[1];
                    assert_eq!(
                        state.on_right_click(
                            phase_to_x(&layout, node.time),
                            value_to_y(&layout, node.value),
                            &mut data,
                            rect,
                            scale
                        ),
                        None
                    );
                } else {
                    let buttons = power_buttons(state.layout(rect, scale).strip, scale);
                    state.open_power_menu(StripId::Transform, buttons.edit, (rect.2, rect.3));
                }
                state.on_key(&key(Key::End, Modifiers::empty()), &mut data, rect, scale);
                let labels = state.dropdown_labels();
                let refs: Vec<_> = labels.iter().map(String::as_str).collect();
                let popup =
                    dropdown_popup_layout(state.dropdown_state(), &refs, (rect.2, rect.3)).unwrap();
                let mut found = 0;
                for row in popup
                    .visible_rows
                    .iter()
                    .filter(|row| row.kind == RowKind::Item && row.item_index >= 27)
                {
                    let size = (row.rect.3 * 0.5).max(10.0);
                    assert!(
                        text.text_width(&labels[row.item_index], size) <= row.rect.2 - 12.0,
                        "{} at {scale}x",
                        labels[row.item_index]
                    );
                    found += 1;
                }
                assert_eq!(found, 3);
            }
        }
    }

    #[test]
    fn separate_editors_copy_paste_and_store_favorites_through_the_shared_directory() {
        let root = std::env::temp_dir().join(format!(
            "tract-editor-curves-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut first = MsegEditState::new();
        let mut second = MsegEditState::new_curve_only();
        first.set_curve_library_directory(root.clone());
        second.set_curve_library_directory(root.clone());
        let mut source = fixture();
        let mut destination = MsegData {
            beats: 17.0,
            time_divisions: 24,
            ..MsegData::default()
        };
        let copy = key(Key::Character("c".into()), Modifiers::CONTROL);
        assert_eq!(first.on_key(&copy, &mut source, RECT, 1.0), (true, None));
        let paste = key(Key::Character("v".into()), Modifiers::META);
        assert_eq!(
            second.on_key(&paste, &mut destination, RECT, 1.0),
            (true, Some(MsegEdit::Changed))
        );
        assert_eq!(destination.active(), source.active());
        assert_eq!((destination.beats, destination.time_divisions), (17.0, 24));
        first.apply_menu(
            StripId::Shapes,
            FactoryShape::ALL.len() + FAVORITE_COUNT + 2,
            &mut source,
        );
        factory_shape(FactoryShape::RampDown)
            .apply(&mut destination, false)
            .unwrap();
        assert_eq!(
            second.apply_menu(
                StripId::Shapes,
                FactoryShape::ALL.len() + 2,
                &mut destination
            ),
            Some(MsegEdit::Changed)
        );
        assert_eq!(destination.active(), source.active());
        let before = destination;
        second.apply_menu(
            StripId::Shapes,
            FactoryShape::ALL.len() + 7,
            &mut destination,
        );
        assert_eq!(destination, before);
        assert!(second.status_message().contains("unavailable"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
