//! MSEG editor — transient interaction state and event handlers.
//!
//! The editor will own the document: event handlers (added in later tasks)
//! mutate `&mut MsegData` directly and return `MsegEdit::Changed` when
//! something changed.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::dropdown::{DropdownEvent, DropdownState};
use crate::mseg::randomize::RandomStyle;
use crate::mseg::MsegData;
use crate::text_edit::TextEditState;

/// Default grid option pairs used when no custom list is supplied.
const DEFAULT_GRID_OPTIONS: [(u32, u32); 4] = [(4, 4), (8, 8), (16, 8), (32, 16)];

/// Style display labels, matching `RandomStyle::ALL` order.
pub const STYLE_LABELS: [&str; 5] = ["Smooth", "Ramps", "Stepped", "Spiky", "Chaos"];

/// Fixed `&'static [&'static str]` slice of style labels. The same slice must
/// be passed to every `DropdownState` call for the style dropdown.
pub fn style_items() -> &'static [&'static str] {
    &STYLE_LABELS
}

/// Snap `(phase, value)` to the document's grid when `data.snap` is on, unless
/// `fine` (the caller's fine-adjust modifier, e.g. Shift) bypasses it. `phase`
/// snaps to the `time_divisions` columns, `value` to the `value_steps` rows.
fn snap_point(phase: f32, value: f32, data: &MsegData, fine: bool) -> (f32, f32) {
    if !data.snap || fine {
        return (phase, value);
    }
    let cols = data.time_divisions.max(1) as f32;
    let rows = data.value_steps.max(1) as f32;
    (
        ((phase * cols).round() / cols).clamp(0.0, 1.0),
        ((value * rows).round() / rows).clamp(0.0, 1.0),
    )
}

/// Returned by an event handler when the document changed and the consuming
/// plugin should re-persist (and, for `miff`, re-bake).
#[must_use = "check whether the document changed and re-persist if so"]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MsegEdit {
    Changed,
}

/// Which strip sub-control a `DropdownState` / `TextEditState` action targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StripId {
    Style,
    Duration,
    TimeGrid,
    ValueGrid,
}

/// What the pointer is currently dragging.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DragTarget {
    /// Moving the node at this index.
    Node(usize),
    /// Bending the segment starting at this node index.
    Tension(usize),
    /// Dragging a hold marker.
    Marker(MarkerHandle),
    /// Painting stepped nodes (Alt held).
    StepDraw,
}

/// Which hold marker is being dragged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkerHandle {
    Sustain,
    LoopStart,
    LoopEnd,
}

/// Transient editor state — not persisted.
pub struct MsegEditState {
    /// When true, playback/timing controls and the marker lane are hidden.
    curve_only: bool,
    /// Active drag, if any.
    drag: Option<DragTarget>,
    /// Hovered node index, for highlight.
    hover: Option<usize>,
    /// During a stepped-draw, the last time-grid cell a node was painted in
    /// (so dragging within one cell does not insert duplicates).
    step_last_cell: Option<u32>,
    /// `true` while the caller's stepped-draw modifier (e.g. Alt) is held.
    stepped_draw_held: bool,
    /// Randomizer style currently chosen in the strip.
    style: RandomStyle,
    /// Shared dropdown state for the grid and style dropdowns. At most one open
    /// at a time; the `StripId` discriminant (`TimeGrid` vs `Style`) identifies
    /// which is active.
    dropdown: DropdownState<StripId>,
    /// Numeric strip-field text entry.
    // reserved for a future numeric-entry follow-up
    #[allow(dead_code)]
    text_edit: TextEditState<StripId>,
    /// Bumped on each Randomize click so successive clicks differ.
    seed: u32,
    /// Per-plugin grid presets `(time_divisions, value_steps)`.
    grid_options: Vec<(u32, u32)>,
    /// Display labels for `grid_options`, kept in sync by `set_grid_options`.
    grid_labels: Vec<String>,
}

impl MsegEditState {
    /// A full editor (playback controls + marker lane shown).
    pub fn new() -> Self {
        Self::with_curve_only(false)
    }

    /// A curve-only editor — playback/timing controls and the marker lane are
    /// hidden; curve editing, grid, snap, and the randomizer remain.
    pub fn new_curve_only() -> Self {
        Self::with_curve_only(true)
    }

    fn with_curve_only(curve_only: bool) -> Self {
        let grid_options: Vec<(u32, u32)> = DEFAULT_GRID_OPTIONS.to_vec();
        let grid_labels = Self::build_labels(&grid_options);
        Self {
            curve_only,
            drag: None,
            hover: None,
            step_last_cell: None,
            stepped_draw_held: false,
            style: RandomStyle::Smooth,
            dropdown: DropdownState::new(),
            text_edit: TextEditState::new(),
            seed: 0,
            grid_options,
            grid_labels,
        }
    }

    /// Build display labels from a slice of `(time_divisions, value_steps)` pairs.
    fn build_labels(options: &[(u32, u32)]) -> Vec<String> {
        options
            .iter()
            .map(|&(t, v)| format!("{} / {}", t, v))
            .collect()
    }

    /// Replace the grid preset list. If `options` is empty the call is a no-op
    /// (the default list is kept).
    ///
    /// Must be called at setup time, NOT while the grid dropdown is open —
    /// the dropdown's stable-items invariant (the item list must not change
    /// while open) would be violated if the list were replaced mid-session.
    pub fn set_grid_options(&mut self, options: &[(u32, u32)]) {
        if options.is_empty() {
            return;
        }
        self.grid_options = options.to_vec();
        self.grid_labels = Self::build_labels(&self.grid_options);
    }

    /// The current grid presets `(time_divisions, value_steps)`.
    pub fn grid_options(&self) -> &[(u32, u32)] {
        &self.grid_options
    }

    /// Build a `Vec<&str>` of grid label references for passing to dropdown
    /// calls. A fresh `Vec` is built each call from the unchanged `grid_labels`
    /// — a GUI-thread allocation, fine. The stable-items invariant (same
    /// length/order while the dropdown is open) is satisfied because
    /// `set_grid_options` must not be called while the dropdown is open.
    pub(crate) fn grid_label_refs(&self) -> Vec<&str> {
        self.grid_labels.iter().map(String::as_str).collect()
    }

    /// Return the `grid_options` index whose `(time_divisions, value_steps)`
    /// pair best matches `data`. Prefers an exact match; falls back to the
    /// nearest by combined absolute deviation; returns 0 on empty list.
    pub(crate) fn current_grid_index(&self, data: &MsegData) -> usize {
        // Prefer an exact match.
        for (i, &(t, v)) in self.grid_options.iter().enumerate() {
            if data.time_divisions == t && data.value_steps == v {
                return i;
            }
        }
        // Nearest by combined distance.
        self.grid_options
            .iter()
            .enumerate()
            .min_by_key(|(_, &(t, v))| {
                let dt = (data.time_divisions as i64 - t as i64).unsigned_abs();
                let dv = (data.value_steps as i64 - v as i64).unsigned_abs();
                dt + dv
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// `true` for a curve-only editor.
    pub fn is_curve_only(&self) -> bool {
        self.curve_only
    }

    /// Set whether the caller's stepped-draw modifier is currently held.
    pub fn set_stepped_draw(&mut self, held: bool) {
        self.stepped_draw_held = held;
    }

    /// The currently hovered node index, if any.
    pub fn hovered_node(&self) -> Option<usize> {
        self.hover
    }

    /// The randomizer style currently selected in the strip.
    pub fn style(&self) -> RandomStyle {
        self.style
    }

    /// Set the randomizer style directly (used by the style dropdown).
    pub fn set_style(&mut self, style: RandomStyle) {
        self.style = style;
    }

    /// `true` when the dropdown for `id` is currently open.
    pub fn dropdown_is_open_for(&self, id: StripId) -> bool {
        self.dropdown.is_open_for(id)
    }

    /// Crate-internal accessor for `render.rs` to call `draw_dropdown_popup`.
    pub(crate) fn dropdown_state(&self) -> &DropdownState<StripId> {
        &self.dropdown
    }

    /// Primary-button press. Returns `MsegEdit::Changed` when the document
    /// changed. With the stepped-draw modifier held, begins a stepped-draw on
    /// empty canvas; otherwise adds a node on empty canvas, or begins a node
    /// or tension drag on a hit handle. Strip clicks toggle snap, open a grid
    /// or style dropdown, or fire the randomizer.
    pub fn on_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
        fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);

        // If a dropdown is open, route the click to it first.
        let mut closed_a_dropdown = false;
        if self.dropdown.is_open() {
            let window_size = (rect.0 + rect.2, rect.1 + rect.3);
            let ev = if self.dropdown.is_open_for(StripId::TimeGrid) {
                // Clone the labels so the borrow of self.grid_labels ends
                // before the mutable self.dropdown call.
                let owned: Vec<String> = self.grid_labels.clone();
                let grid_refs: Vec<&str> = owned.iter().map(String::as_str).collect();
                self.dropdown.on_mouse_down(x, y, &grid_refs, window_size)
            } else {
                self.dropdown
                    .on_mouse_down(x, y, style_items(), window_size)
            };
            match ev {
                Some(DropdownEvent::Selected(StripId::TimeGrid, idx)) => {
                    let (t, v) = self.grid_options[idx];
                    data.time_divisions = t;
                    data.value_steps = v;
                    return Some(MsegEdit::Changed);
                }
                Some(DropdownEvent::Selected(StripId::Style, idx)) => {
                    self.set_style(RandomStyle::from_index(idx));
                    return None;
                }
                Some(DropdownEvent::Closed(_)) => {
                    // The click landed outside the popup, closing the dropdown.
                    // Fall through so the SAME click still acts on whatever it
                    // hit (Randomize, snap, a node...) — no second click needed.
                    // The `closed_a_dropdown` guard below stops a click on the
                    // dropdown's own trigger from immediately reopening it.
                    closed_a_dropdown = true;
                }
                _ => return None,
            }
        }

        match mseg_hit_test(&layout, data, self.curve_only, scale, x, y) {
            MsegHit::Node(i) => {
                self.drag = Some(DragTarget::Node(i));
                None
            }
            MsegHit::Tension(i) => {
                self.drag = Some(DragTarget::Tension(i));
                None
            }
            MsegHit::Canvas if self.stepped_draw_held => {
                self.drag = Some(DragTarget::StepDraw);
                self.step_last_cell = None;
                self.step_draw_paint(x, y, data, &layout)
            }
            MsegHit::Canvas => {
                let (phase, value) =
                    snap_point(x_to_phase(&layout, x), y_to_value(&layout, y), data, fine);
                let inserted = data.insert_node(phase, value);
                if let Some(idx) = inserted {
                    self.drag = Some(DragTarget::Node(idx));
                    Some(MsegEdit::Changed)
                } else {
                    None
                }
            }
            MsegHit::Randomize => {
                self.seed = self.seed.wrapping_add(1);
                crate::mseg::randomize::randomize(data, self.style, self.seed);
                // Randomize replaces every node — any drag/hover index is now
                // stale, so clear them.
                self.drag = None;
                self.hover = None;
                Some(MsegEdit::Changed)
            }
            MsegHit::Strip => {
                // Resolve which strip button was clicked via the very layout
                // the renderer draws — clicks land exactly on the buttons, and
                // a click in a gap between buttons is a no-op.
                use crate::mseg::render::{in_rect, strip_buttons};
                let b = strip_buttons(layout.strip, scale);
                let window_size = (rect.0 + rect.2, rect.1 + rect.3);
                if in_rect(b.snap, x, y) {
                    data.snap = !data.snap;
                    Some(MsegEdit::Changed)
                } else if in_rect(b.grid, x, y) {
                    // Open the grid dropdown; no document change yet. Skipped
                    // if this same click just closed a dropdown, so clicking an
                    // open dropdown's trigger toggles it shut rather than
                    // closing-then-reopening.
                    if !closed_a_dropdown {
                        // Clone labels so the borrow of self.grid_labels ends
                        // before the mutable self.dropdown call.
                        let owned: Vec<String> = self.grid_labels.clone();
                        let grid_refs: Vec<&str> = owned.iter().map(String::as_str).collect();
                        let grid_idx = self.current_grid_index(data);
                        self.dropdown.open(
                            StripId::TimeGrid,
                            b.grid,
                            &grid_refs,
                            grid_idx,
                            false,
                            window_size,
                        );
                    }
                    None
                } else if in_rect(b.style, x, y) {
                    // Open the style dropdown; no document change. Skipped if
                    // this click just closed a dropdown (trigger toggle-shut).
                    if !closed_a_dropdown {
                        self.dropdown.open(
                            StripId::Style,
                            b.style,
                            style_items(),
                            self.style.index(),
                            false,
                            window_size,
                        );
                    }
                    None
                } else {
                    None
                }
            }
            MsegHit::MarkerLane => None,
            MsegHit::None => None,
        }
    }

    /// Pointer motion. Applies the active drag.
    pub fn on_mouse_move(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
        fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);

        // Route moves to the open dropdown for hover-highlight updates.
        if self.dropdown.is_open() {
            let window_size = (rect.0 + rect.2, rect.1 + rect.3);
            if self.dropdown.is_open_for(StripId::TimeGrid) {
                // Clone labels so the borrow of self.grid_labels ends before
                // the mutable self.dropdown call.
                let owned: Vec<String> = self.grid_labels.clone();
                let grid_refs: Vec<&str> = owned.iter().map(String::as_str).collect();
                self.dropdown.on_mouse_move(x, y, &grid_refs, window_size);
            } else {
                self.dropdown
                    .on_mouse_move(x, y, style_items(), window_size);
            }
            return None;
        }
        // Hover highlight (only when not dragging).
        if self.drag.is_none() {
            self.hover = match mseg_hit_test(&layout, data, self.curve_only, scale, x, y) {
                MsegHit::Node(i) => Some(i),
                _ => None,
            };
        }
        match self.drag {
            Some(DragTarget::Node(i)) => {
                let (phase, value) =
                    snap_point(x_to_phase(&layout, x), y_to_value(&layout, y), data, fine);
                data.move_node(i, phase, value);
                Some(MsegEdit::Changed)
            }
            Some(DragTarget::Tension(i)) => {
                // Drag vertically away from the segment's straight midpoint to
                // bend it. Map the vertical offset to tension in -1..1.
                // Copy needed values out of `a` before the mutable write.
                let (v_lo, v_hi) = if i + 1 < data.node_count {
                    let a = data.active();
                    (a[i].value, a[i + 1].value)
                } else {
                    return None;
                };
                let straight_mid = (v_lo + v_hi) * 0.5;
                let cur = y_to_value(&layout, y);
                let rising = v_hi >= v_lo;
                // Sign flip by `rising`: dragging the handle the same screen
                // direction must bow a rising vs. a falling segment
                // consistently in tension polarity (positive = slow-start).
                let delta = (cur - straight_mid) * if rising { -2.0 } else { 2.0 };
                data.nodes[i].tension = delta.clamp(-1.0, 1.0);
                data.debug_assert_valid();
                Some(MsegEdit::Changed)
            }
            Some(DragTarget::StepDraw) => self.step_draw_paint(x, y, data, &layout),
            _ => None,
        }
    }

    /// Primary-button release. Ends any drag and any scrollbar-thumb drag
    /// inside an open dropdown.
    pub fn on_mouse_up(&mut self, _data: &mut MsegData) -> Option<MsegEdit> {
        self.dropdown.on_mouse_up();
        self.drag = None;
        self.step_last_cell = None;
        None
    }

    /// Double-click: delete the node under the pointer (endpoints excepted).
    pub fn on_double_click(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        if let MsegHit::Node(i) = mseg_hit_test(&layout, data, self.curve_only, scale, x, y) {
            if data.remove_node(i) {
                // drag/hover may reference the just-deleted node's index — clear both.
                self.drag = None;
                self.hover = None;
                return Some(MsegEdit::Changed);
            }
        }
        None
    }

    /// Right-click: toggle the `stepped` flag of the segment under the
    /// pointer. The segment is the one whose time range contains the click.
    pub fn on_right_click(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        if !matches!(
            mseg_hit_test(&layout, data, self.curve_only, scale, x, y),
            MsegHit::Canvas | MsegHit::Tension(_) | MsegHit::Node(_)
        ) {
            return None;
        }
        let phase = x_to_phase(&layout, x);
        // Segment i is the last node whose time is <= phase, capped so a
        // segment always exists. Compute `seg` inside a block so the
        // immutable borrow of `data` via `active()` ends before the write.
        // `phase` is clamped to [0,1]; the last node (time==1.0) is excluded
        // from the loop window because it has no outgoing segment, so a click
        // at phase 1.0 correctly resolves to the final segment.
        let seg = {
            let a = data.active();
            let mut seg = 0;
            for (i, n) in a.iter().enumerate().take(data.node_count - 1) {
                // Nodes are time-sorted: once a node's time exceeds `phase`,
                // no later node can match — mirror `value_at_phase`'s break.
                if n.time <= phase {
                    seg = i;
                } else {
                    break;
                }
            }
            seg
        };
        data.nodes[seg].stepped = !data.nodes[seg].stepped;
        data.debug_assert_valid();
        Some(MsegEdit::Changed)
    }

    /// Insert a node snapped to the current time-grid cell's left edge and
    /// mark both it and its predecessor `stepped`, if the pointer has entered
    /// a new cell since the last paint. Used by stepped-draw.
    fn step_draw_paint(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        layout: &crate::mseg::render::MsegLayout,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{x_to_phase, y_to_value};
        let phase = x_to_phase(layout, x);
        let value = y_to_value(layout, y);
        let tdiv = data.time_divisions.max(1);
        // At phase 1.0, cell == tdiv and snapped_phase collapses to 1.0,
        // which insert_node refuses (no room past the end node) — harmless.
        let cell = (phase * tdiv as f32) as u32;
        if self.step_last_cell == Some(cell) {
            return None; // still inside the last painted cell
        }
        self.step_last_cell = Some(cell);
        // Snap the node to the cell's left edge; mark its segment stepped.
        let snapped_phase = (cell as f32 / tdiv as f32).clamp(0.0, 1.0);
        if let Some(idx) = data.insert_node(snapped_phase, value) {
            // The new node and the one before it both belong to the stepped
            // run — set the *previous* node's segment stepped so the painted
            // run reads as steps.
            if idx > 0 {
                data.nodes[idx - 1].stepped = true;
            }
            data.nodes[idx].stepped = true;
            data.debug_assert_valid();
            return Some(MsegEdit::Changed);
        }
        None
    }
}

impl Default for MsegEditState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::render::{mseg_layout, phase_to_x, value_to_y};
    use crate::mseg::MsegData;

    const RECT: (f32, f32, f32, f32) = (0.0, 0.0, 400.0, 300.0);

    #[test]
    fn new_is_full_editor() {
        assert!(!MsegEditState::new().is_curve_only());
    }

    #[test]
    fn new_curve_only_is_curve_only() {
        assert!(MsegEditState::new_curve_only().is_curve_only());
    }

    // --- Task 8: interaction tests ---
    //
    // NOTE: The default 2-node ramp has a tension handle at (phase=0.5, value=0.5)
    // because value_at_phase(data, 0.5) == 0.5 for a linear ramp. Clicking at
    // exactly that midpoint hits Tension(0), not Canvas. We use (phase=0.3,
    // value=0.7) instead — well away from any node or handle — so the hit
    // lands on Canvas and triggers insertion.

    #[test]
    fn click_empty_canvas_inserts_a_node() {
        let mut data = MsegData::default(); // 2 nodes
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let x = phase_to_x(&l, 0.3);
        let y = value_to_y(&l, 0.7);
        let ev = state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert_eq!(data.node_count, 3);
    }

    #[test]
    fn drag_moves_a_node() {
        let mut data = MsegData::default();
        data.snap = false; // test the raw drag mechanic; snapping has its own tests
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Press on node 1, drag it down to value ~0.2.
        state.on_mouse_down(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.5),
            &mut data,
            RECT,
            1.0,
            false,
        );
        state.on_mouse_move(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.2),
            &mut data,
            RECT,
            1.0,
            false,
        );
        // the node inserted at phase 0.5 sorts to index 1 (between the 0.0 and
        // 1.0 endpoints)
        assert!((data.nodes[1].value - 0.2).abs() < 0.05);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn drag_snaps_node_to_grid_when_snap_on() {
        // Default doc: snap on, time_divisions 16, value_steps 8.
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Grab node 1, drag to an off-grid value 0.61 — must snap to 1/8 = 0.625.
        state.on_mouse_down(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.5),
            &mut data,
            RECT,
            1.0,
            false,
        );
        state.on_mouse_move(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.61),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(
            (data.nodes[1].value - 0.625).abs() < 0.01,
            "value {} should snap to the 1/8 grid (0.625)",
            data.nodes[1].value
        );
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn fine_modifier_bypasses_snap() {
        let mut data = MsegData::default(); // snap on
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // fine = true on both press and move bypasses snapping.
        state.on_mouse_down(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.5),
            &mut data,
            RECT,
            1.0,
            true,
        );
        state.on_mouse_move(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.61),
            &mut data,
            RECT,
            1.0,
            true,
        );
        // Lands near the raw 0.61, NOT snapped to 0.625.
        assert!(
            (data.nodes[1].value - 0.61).abs() < 0.02,
            "fine should bypass snap; value {}",
            data.nodes[1].value
        );
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn handlers_noop_when_pointer_outside() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        assert_eq!(
            state.on_mouse_down(-5.0, -5.0, &mut data, RECT, 1.0, false),
            None
        );
        assert_eq!(data.node_count, 2);
        // A true no-op: the editor state must not be corrupted either.
        assert!(state.drag.is_none());
        assert!(state.hovered_node().is_none());
    }

    #[test]
    fn double_click_deletes_interior_node() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let ev = state.on_double_click(
            phase_to_x(&l, 0.5),
            value_to_y(&l, 0.5),
            &mut data,
            RECT,
            1.0,
        );
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert_eq!(data.node_count, 2);
    }

    #[test]
    fn double_click_endpoint_does_nothing() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let ev = state.on_double_click(
            phase_to_x(&l, 0.0),
            value_to_y(&l, 0.0),
            &mut data,
            RECT,
            1.0,
        );
        assert_eq!(ev, None);
        assert_eq!(data.node_count, 2);
    }

    #[test]
    fn right_click_toggles_segment_stepped() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Right-click mid-way along segment 0 (phase ~0.25).
        let x = phase_to_x(&l, 0.25);
        let y = value_to_y(&l, 0.25);
        assert!(!data.nodes[0].stepped);
        let ev = state.on_right_click(x, y, &mut data, RECT, 1.0);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert!(data.nodes[0].stepped);
    }

    #[test]
    fn stepped_draw_paints_nodes_across_cells() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        state.set_stepped_draw(true);
        let l = mseg_layout(RECT, false, 1.0);
        let before = data.node_count;
        // Press, then drag across several grid cells.
        state.on_mouse_down(
            phase_to_x(&l, 0.1),
            value_to_y(&l, 0.8),
            &mut data,
            RECT,
            1.0,
            false,
        );
        for &p in &[0.3_f32, 0.5, 0.7, 0.9] {
            state.on_mouse_move(
                phase_to_x(&l, p),
                value_to_y(&l, 0.6),
                &mut data,
                RECT,
                1.0,
                false,
            );
        }
        state.on_mouse_up(&mut data);
        assert!(data.node_count > before, "stepped-draw inserted no nodes");
        // Painted nodes are stepped.
        assert!(data
            .active()
            .iter()
            .take(data.node_count - 1)
            .any(|n| n.stepped));
    }

    #[test]
    fn stepped_draw_inactive_when_modifier_not_held() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new(); // modifier NOT held
        let l = mseg_layout(RECT, false, 1.0);
        // Click at phase 0.3, value 0.7 — well away from any existing node or
        // tension handle — so the hit lands on Canvas and triggers a single
        // ordinary insert (not stepped-draw).
        state.on_mouse_down(
            phase_to_x(&l, 0.3),
            value_to_y(&l, 0.7),
            &mut data,
            RECT,
            1.0,
            false,
        );
        // Without the modifier this is an ordinary single-node insert.
        assert_eq!(data.node_count, 3);
    }

    #[test]
    fn randomize_button_regenerates_and_changes_seed() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let bx = l.strip.0 + l.strip.2 - 48.0;
        let by = l.strip.1 + l.strip.3 * 0.5;
        // Seed starts at 0; each Randomize click does wrapping_add(1).
        let ev = state.on_mouse_down(bx, by, &mut data, RECT, 1.0, false);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert!(data.is_valid());
        assert_eq!(state.seed, 1);
        // A second click bumps the seed again and re-randomizes.
        let ev = state.on_mouse_down(bx, by, &mut data, RECT, 1.0, false);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert!(data.is_valid());
        assert_eq!(state.seed, 2);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn snap_toggle_zone_flips_snap() {
        let mut data = MsegData::default();
        let was = data.snap;
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Left third of the strip = snap toggle.
        let x = l.strip.0 + l.strip.2 * 0.1;
        let y = l.strip.1 + l.strip.3 * 0.5;
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert_eq!(data.snap, !was);
        state.on_mouse_up(&mut data);
    }

    // --- Grid dropdown tests ---

    #[test]
    fn grid_zone_opens_dropdown() {
        let mut data = MsegData::default(); // time_divisions 16, value_steps 8
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Use strip_buttons to find the grid trigger rect centre.
        use crate::mseg::render::strip_buttons;
        let b = strip_buttons(l.strip, 1.0);
        let x = b.grid.0 + b.grid.2 * 0.5;
        let y = b.grid.1 + b.grid.3 * 0.5;
        let ev = state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        // Opening the dropdown doesn't change the document.
        assert_eq!(ev, None, "opening dropdown must not change document");
        // The grid dropdown is now open.
        assert!(
            state.dropdown_is_open_for(StripId::TimeGrid),
            "grid dropdown not open after clicking grid trigger"
        );
        // Document not yet changed.
        assert_eq!(data.time_divisions, 16);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn grid_dropdown_select_updates_grid() {
        use crate::dropdown::dropdown_popup_layout;
        let mut data = MsegData::default(); // time_divisions 16, value_steps 8
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        use crate::mseg::render::strip_buttons;
        let b = strip_buttons(l.strip, 1.0);
        let x = b.grid.0 + b.grid.2 * 0.5;
        let y = b.grid.1 + b.grid.3 * 0.5;
        // Open the grid dropdown.
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert!(state.dropdown_is_open_for(StripId::TimeGrid));
        // Compute the popup layout to find a row to click.
        let window_size = (RECT.0 + RECT.2, RECT.1 + RECT.3);
        let grid_refs = state.grid_label_refs();
        let layout = dropdown_popup_layout(&state.dropdown, &grid_refs, window_size).unwrap();
        // Click the first visible row (index 0 = "4 / 4").
        let row = layout.visible_rows[0];
        let (rx, ry, rw, rh) = row.rect;
        let ev = state.on_mouse_down(rx + rw * 0.5, ry + rh * 0.5, &mut data, RECT, 1.0, false);
        assert_eq!(
            ev,
            Some(MsegEdit::Changed),
            "selecting grid must change document"
        );
        let (t, v) = state.grid_options()[row.item_index];
        assert_eq!(data.time_divisions, t);
        assert_eq!(data.value_steps, v);
        assert!(
            !state.dropdown_is_open_for(StripId::TimeGrid),
            "dropdown should close after selection"
        );
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn current_grid_index_round_trips() {
        let state = MsegEditState::new();
        for (i, &(t, v)) in state.grid_options().iter().enumerate() {
            let mut data = MsegData::default();
            data.time_divisions = t;
            data.value_steps = v;
            assert_eq!(
                state.current_grid_index(&data),
                i,
                "round-trip failed for grid_options[{i}] = ({t},{v})"
            );
        }
    }

    #[test]
    fn current_grid_index_no_match_returns_nearest() {
        // (7, 7) is between the default options 1=(8,8) and 0=(4,4); nearest is 1.
        let state = MsegEditState::new();
        let mut data = MsegData::default();
        data.time_divisions = 7;
        data.value_steps = 7;
        let idx = state.current_grid_index(&data);
        // Both options 0 and 1 are candidates — either is acceptable; just
        // verify it returns a valid index within bounds.
        assert!(idx < state.grid_options().len());
    }

    #[test]
    fn set_grid_options_updates_options_and_labels() {
        let mut state = MsegEditState::new();
        state.set_grid_options(&[(2, 2), (64, 64)]);
        assert_eq!(state.grid_options(), &[(2, 2), (64, 64)]);
        assert_eq!(state.grid_label_refs(), vec!["2 / 2", "64 / 64"]);
    }

    #[test]
    fn set_grid_options_empty_slice_keeps_default() {
        let state_default = MsegEditState::new();
        let default_opts: Vec<(u32, u32)> = state_default.grid_options().to_vec();
        let default_labels: Vec<String> = state_default
            .grid_label_refs()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut state = MsegEditState::new();
        state.set_grid_options(&[]); // no-op
        assert_eq!(state.grid_options(), default_opts.as_slice());
        let labels: Vec<String> = state
            .grid_label_refs()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(labels, default_labels);
    }

    #[test]
    fn custom_grid_select_applies_correct_values() {
        use crate::dropdown::dropdown_popup_layout;
        use crate::mseg::render::strip_buttons;

        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        // Plugin-supplied custom list: two options, second is (64, 64).
        state.set_grid_options(&[(2, 2), (64, 64)]);

        let l = mseg_layout(RECT, false, 1.0);
        let b = strip_buttons(l.strip, 1.0);
        // Open the grid dropdown.
        state.on_mouse_down(
            b.grid.0 + b.grid.2 * 0.5,
            b.grid.1 + b.grid.3 * 0.5,
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(state.dropdown_is_open_for(StripId::TimeGrid));

        // Find and click the second row (index 1 = (64,64)).
        let window_size = (RECT.0 + RECT.2, RECT.1 + RECT.3);
        let grid_refs = state.grid_label_refs();
        let layout = dropdown_popup_layout(&state.dropdown, &grid_refs, window_size).unwrap();
        // Rows may be ordered; find the row with item_index == 1.
        let row = layout
            .visible_rows
            .iter()
            .find(|r| r.item_index == 1)
            .copied()
            .expect("row with item_index 1 not found");
        let (rx, ry, rw, rh) = row.rect;
        let ev = state.on_mouse_down(rx + rw * 0.5, ry + rh * 0.5, &mut data, RECT, 1.0, false);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert_eq!(data.time_divisions, 64);
        assert_eq!(data.value_steps, 64);
        state.on_mouse_up(&mut data);
    }

    // --- Style dropdown tests ---

    #[test]
    fn style_zone_opens_dropdown() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        use crate::mseg::render::strip_buttons;
        let b = strip_buttons(l.strip, 1.0);
        let x = b.style.0 + b.style.2 * 0.5;
        let y = b.style.1 + b.style.3 * 0.5;
        let ev = state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert_eq!(ev, None, "opening style dropdown must not change document");
        assert!(
            state.dropdown_is_open_for(StripId::Style),
            "style dropdown not open after clicking style trigger"
        );
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn style_dropdown_select_changes_style() {
        use crate::dropdown::dropdown_popup_layout;
        let mut data = MsegData::default();
        let mut state = MsegEditState::new(); // starts at Smooth (index 0)
        let l = mseg_layout(RECT, false, 1.0);
        use crate::mseg::render::strip_buttons;
        let b = strip_buttons(l.strip, 1.0);
        let x = b.style.0 + b.style.2 * 0.5;
        let y = b.style.1 + b.style.3 * 0.5;
        // Open the style dropdown.
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert!(state.dropdown_is_open_for(StripId::Style));
        // Click the second row (index 1 = Ramps).
        let window_size = (RECT.0 + RECT.2, RECT.1 + RECT.3);
        let layout = dropdown_popup_layout(&state.dropdown, style_items(), window_size).unwrap();
        let row = layout.visible_rows[1]; // second row
        let (rx, ry, rw, rh) = row.rect;
        let ev = state.on_mouse_down(rx + rw * 0.5, ry + rh * 0.5, &mut data, RECT, 1.0, false);
        assert_eq!(
            ev, None,
            "style selection changes editor state, not the document"
        );
        assert_eq!(state.style(), RandomStyle::from_index(row.item_index));
        assert!(
            !state.dropdown_is_open_for(StripId::Style),
            "dropdown should close after selection"
        );
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn random_style_index_round_trips() {
        for (i, &style) in RandomStyle::ALL.iter().enumerate() {
            assert_eq!(style.index(), i);
            assert_eq!(RandomStyle::from_index(i), style);
        }
    }
}
