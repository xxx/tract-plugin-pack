//! MSEG editor — transient interaction state and event handlers.
//!
//! The editor will own the document: event handlers (added in later tasks)
//! mutate `&mut MsegData` directly and return `MsegEdit::Changed` when
//! something changed.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::dropdown::DropdownState;
use crate::mseg::randomize::RandomStyle;
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
}

impl Default for MsegEditState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_full_editor() {
        assert!(!MsegEditState::new().is_curve_only());
    }

    #[test]
    fn new_curve_only_is_curve_only() {
        assert!(MsegEditState::new_curve_only().is_curve_only());
    }
}
