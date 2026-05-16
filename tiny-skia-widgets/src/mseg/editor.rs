//! MSEG editor — transient interaction state and event handlers.
//!
//! The editor will own the document: event handlers (added in later tasks)
//! mutate `&mut MsegData` directly and return `MsegEdit::Changed` when
//! something changed.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::dropdown::DropdownState;
use crate::mseg::randomize::RandomStyle;
use crate::mseg::MsegData;
use crate::text_edit::TextEditState;

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
#[allow(dead_code)] // fields wired up across Tasks 2-11
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
    /// Style-selector dropdown state.
    style_dropdown: DropdownState<StripId>,
    /// Numeric strip-field text entry.
    text_edit: TextEditState<StripId>,
    /// Bumped on each Randomize click so successive clicks differ.
    seed: u32,
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
        Self {
            curve_only,
            drag: None,
            hover: None,
            step_last_cell: None,
            stepped_draw_held: false,
            style: RandomStyle::Smooth,
            style_dropdown: DropdownState::new(),
            text_edit: TextEditState::new(),
            seed: 0,
        }
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

    /// Advance the randomizer style to the next of the five variants.
    pub fn cycle_style(&mut self) {
        self.style = match self.style {
            RandomStyle::Smooth => RandomStyle::Ramps,
            RandomStyle::Ramps => RandomStyle::Stepped,
            RandomStyle::Stepped => RandomStyle::Spiky,
            RandomStyle::Spiky => RandomStyle::Chaos,
            RandomStyle::Chaos => RandomStyle::Smooth,
        };
    }

    /// Primary-button press. Returns `MsegEdit::Changed` when the document
    /// changed. With the stepped-draw modifier held, begins a stepped-draw on
    /// empty canvas; otherwise adds a node on empty canvas, or begins a node
    /// or tension drag on a hit handle.
    pub fn on_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
        _fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
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
                let phase = x_to_phase(&layout, x);
                let value = y_to_value(&layout, y);
                let inserted = data.insert_node(phase, value);
                if let Some(idx) = inserted {
                    self.drag = Some(DragTarget::Node(idx));
                    Some(MsegEdit::Changed)
                } else {
                    None
                }
            }
            MsegHit::None => None,
            // Strip / Randomize / MarkerLane handled in later tasks.
            _ => None,
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
        _fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        // Hover highlight (only when not dragging).
        if self.drag.is_none() {
            self.hover = match mseg_hit_test(&layout, data, self.curve_only, scale, x, y) {
                MsegHit::Node(i) => Some(i),
                _ => None,
            };
        }
        match self.drag {
            Some(DragTarget::Node(i)) => {
                let phase = x_to_phase(&layout, x);
                let value = y_to_value(&layout, y);
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

    /// Primary-button release. Ends any drag.
    pub fn on_mouse_up(&mut self, _data: &mut MsegData) -> Option<MsegEdit> {
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
        state.on_mouse_down(phase_to_x(&l, 0.1), value_to_y(&l, 0.8),
                            &mut data, RECT, 1.0, false);
        for &p in &[0.3_f32, 0.5, 0.7, 0.9] {
            state.on_mouse_move(phase_to_x(&l, p), value_to_y(&l, 0.6),
                                &mut data, RECT, 1.0, false);
        }
        state.on_mouse_up(&mut data);
        assert!(data.node_count > before, "stepped-draw inserted no nodes");
        // Painted nodes are stepped.
        assert!(data.active().iter().take(data.node_count - 1).any(|n| n.stepped));
    }

    #[test]
    fn stepped_draw_inactive_when_modifier_not_held() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new(); // modifier NOT held
        let l = mseg_layout(RECT, false, 1.0);
        // Click at phase 0.3, value 0.7 — well away from any existing node or
        // tension handle — so the hit lands on Canvas and triggers a single
        // ordinary insert (not stepped-draw).
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.7),
                            &mut data, RECT, 1.0, false);
        // Without the modifier this is an ordinary single-node insert.
        assert_eq!(data.node_count, 3);
    }
}
