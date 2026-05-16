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
    /// changed. Adds a node on empty canvas, begins a node or tension drag on
    /// a hit handle.
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
        let _ = fine;
        match mseg_hit_test(&layout, data, self.curve_only, scale, x, y) {
            MsegHit::Node(i) => {
                self.drag = Some(DragTarget::Node(i));
                None
            }
            MsegHit::Tension(i) => {
                self.drag = Some(DragTarget::Tension(i));
                None
            }
            MsegHit::Canvas => {
                // Stepped-draw (a later task) takes over when its modifier is held.
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
        fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        let _ = fine;
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
                let delta = (cur - straight_mid) * if rising { -2.0 } else { 2.0 };
                data.nodes[i].tension = delta.clamp(-1.0, 1.0);
                data.debug_assert_valid();
                Some(MsegEdit::Changed)
            }
            _ => None,
        }
    }

    /// Primary-button release. Ends any drag.
    pub fn on_mouse_up(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        let _ = data;
        self.drag = None;
        self.step_last_cell = None;
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
    }
}
