//! Dropdown (popup-list) widget for softbuffer-based nih-plug editors.
//!
//! Fills the niche above `draw_stepped_selector` (small enums) and
//! `grid_selector` (~9 inline options): many options, or scarce horizontal
//! space. State lives in `DropdownState<A>`, mirroring `TextEditState<A>`:
//! at most one dropdown open at a time, tagged by the caller's action enum.
//!
//! See `docs/superpowers/specs/2026-05-15-dropdown-widget-design.md`.

use std::time::Instant;

#[allow(unused_imports)]
use tiny_skia::Pixmap;

#[allow(unused_imports)]
use crate::primitives::{
    color_accent, color_border, color_control_bg, color_edit_bg, color_muted, color_text,
    draw_rect, draw_rect_outline,
};
#[allow(unused_imports)]
use crate::text::TextRenderer;

/// Maximum rows shown before the list scrolls.
pub const MAX_VISIBLE_ROWS: usize = 12;

/// Filter-text buffer cap (defensive — typed filters are short).
#[allow(dead_code)]
const MAX_FILTER_LEN: usize = 64;

/// Popup border thickness, in physical pixels.
#[allow(dead_code)]
const BORDER: f32 = 1.0;

/// Scrollbar strip width, in physical pixels.
#[allow(dead_code)]
const SCROLLBAR_W: f32 = 5.0;

/// Crate-local key enum. The editor maps host (baseview) key events to this,
/// so the widget never depends on baseview.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropdownKey {
    Up,
    Down,
    Enter,
    Esc,
    Backspace,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Result of an event handler. Indices are always into the UNFILTERED list.
/// There is no `Opened` variant — opening is editor-driven via `open()`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DropdownEvent<A> {
    Closed(A),
    HighlightChanged(A, usize),
    Selected(A, usize),
}

/// Transient state for the single open dropdown.
#[allow(dead_code)]
struct Active<A> {
    action: A,
    /// Trigger widget rect captured at open time.
    anchor: (f32, f32, f32, f32),
    item_count: usize,
    /// Index into the UNFILTERED list.
    highlight: usize,
    /// Viewport top, in popup-content pixels.
    scroll_px: f32,
    filter: String,
    filter_enabled: bool,
    /// `Some(grab_offset_y)` while the scrollbar thumb is being dragged.
    scrollbar_drag: Option<f32>,
}

/// Per-editor dropdown focus + transient state. At most one open at a time.
pub struct DropdownState<A: Copy + PartialEq> {
    active: Option<Active<A>>,
    /// Drives the filter-row caret blink; reset on every filter edit / open.
    #[allow(dead_code)]
    last_filter_change: Instant,
}

/// A single visible (post-filter) row: its on-screen rect and the index it
/// maps back to in the UNFILTERED list.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RowRect {
    pub rect: (f32, f32, f32, f32),
    pub item_index: usize,
}

/// Scrollbar geometry.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ScrollbarRect {
    pub track: (f32, f32, f32, f32),
    pub thumb: (f32, f32, f32, f32),
}

/// Computed popup geometry. Backs both drawing and hit testing.
#[derive(Clone, Debug)]
pub struct DropdownPopupLayout {
    pub popup_rect: (f32, f32, f32, f32),
    pub filter_rect: Option<(f32, f32, f32, f32)>,
    pub list_viewport: (f32, f32, f32, f32),
    pub visible_rows: Vec<RowRect>,
    pub scrollbar: Option<ScrollbarRect>,
    pub content_height: f32,
    pub opens_upward: bool,
}

impl<A: Copy + PartialEq> DropdownState<A> {
    pub fn new() -> Self {
        Self {
            active: None,
            last_filter_change: Instant::now(),
        }
    }

    /// `true` when any dropdown is open.
    pub fn is_open(&self) -> bool {
        self.active.is_some()
    }
}

impl<A: Copy + PartialEq> Default for DropdownState<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum A {
        Wavetable,
        Algorithm,
    }

    #[test]
    fn new_reports_closed() {
        let s: DropdownState<A> = DropdownState::new();
        assert!(!s.is_open());
    }
}
