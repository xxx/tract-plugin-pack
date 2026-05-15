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

/// Case-insensitive substring match. An empty filter matches everything.
#[allow(dead_code)]
fn filter_matches(item: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    item.to_lowercase().contains(&filter.to_lowercase())
}

/// UNFILTERED indices of items matching `filter`, in original order.
/// Allocates — fine here, this runs on the editor thread, never `process()`.
#[allow(dead_code)]
fn filtered_indices(items: &[&str], filter: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| filter_matches(item, filter))
        .map(|(i, _)| i)
        .collect()
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

    /// `true` when the dropdown for `action` is the one currently open.
    pub fn is_open_for(&self, action: A) -> bool {
        matches!(&self.active, Some(a) if a.action == action)
    }

    /// Open the dropdown for `action`, auto-closing any other. `current` is
    /// the currently-selected UNFILTERED index; it seeds the highlight and is
    /// scrolled into view. `window_size` is the editor pixmap size.
    pub fn open(
        &mut self,
        action: A,
        anchor: (f32, f32, f32, f32),
        item_count: usize,
        current: usize,
        filter_enabled: bool,
        window_size: (f32, f32),
    ) {
        let highlight = current.min(item_count.saturating_sub(1));
        self.active = Some(Active {
            action,
            anchor,
            item_count,
            highlight,
            scroll_px: 0.0,
            filter: String::new(),
            filter_enabled,
            scrollbar_drag: None,
        });
        self.last_filter_change = Instant::now();
        // Scroll the seeded highlight into view. With an empty filter the
        // filtered position equals the unfiltered index.
        self.scroll_highlight_into_view_empty_filter(window_size);
    }

    /// Close the dropdown.
    pub fn close(&mut self) {
        self.active = None;
    }

    /// Scroll so the highlight is visible, assuming an empty filter.
    /// Real implementation lands in Task 4; no-op stub keeps Task 3 compiling.
    fn scroll_highlight_into_view_empty_filter(&mut self, _window_size: (f32, f32)) {}

    #[cfg(test)]
    fn highlight_for_test(&self) -> Option<usize> {
        self.active.as_ref().map(|a| a.highlight)
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

    const WIN: (f32, f32) = (800.0, 600.0);
    const ANCHOR: (f32, f32, f32, f32) = (100.0, 100.0, 160.0, 24.0);

    #[test]
    fn new_reports_closed() {
        let s: DropdownState<A> = DropdownState::new();
        assert!(!s.is_open());
    }

    #[test]
    fn filter_matches_empty_filter_matches_all() {
        assert!(filter_matches("anything", ""));
    }

    #[test]
    fn filter_matches_is_case_insensitive() {
        assert!(filter_matches("SineWave.wt", "sine"));
        assert!(filter_matches("sinewave.wt", "SINE"));
    }

    #[test]
    fn filter_matches_is_substring_not_prefix() {
        assert!(filter_matches("deep-bass.wt", "bass"));
        assert!(!filter_matches("deep-bass.wt", "treble"));
    }

    #[test]
    fn filtered_indices_empty_filter_returns_all() {
        let items = ["a", "b", "c"];
        assert_eq!(filtered_indices(&items, ""), vec![0, 1, 2]);
    }

    #[test]
    fn filtered_indices_returns_unfiltered_positions() {
        let items = ["alpha", "bravo", "bravado", "bract"];
        // "bra" matches bravo(1), bravado(2), bract(3) — alpha(0) does not.
        assert_eq!(filtered_indices(&items, "bra"), vec![1, 2, 3]);
    }

    #[test]
    fn filtered_indices_no_match_is_empty() {
        let items = ["a", "b"];
        assert!(filtered_indices(&items, "z").is_empty());
    }

    #[test]
    fn open_marks_dropdown_open() {
        let mut s: DropdownState<A> = DropdownState::new();
        s.open(A::Wavetable, ANCHOR, 50, 3, true, WIN);
        assert!(s.is_open());
        assert!(s.is_open_for(A::Wavetable));
        assert!(!s.is_open_for(A::Algorithm));
    }

    #[test]
    fn open_auto_closes_previous() {
        let mut s: DropdownState<A> = DropdownState::new();
        s.open(A::Wavetable, ANCHOR, 50, 3, true, WIN);
        s.open(A::Algorithm, ANCHOR, 6, 0, false, WIN);
        assert!(!s.is_open_for(A::Wavetable));
        assert!(s.is_open_for(A::Algorithm));
    }

    #[test]
    fn open_seeds_highlight_from_current() {
        let mut s: DropdownState<A> = DropdownState::new();
        s.open(A::Wavetable, ANCHOR, 50, 7, true, WIN);
        assert_eq!(s.highlight_for_test(), Some(7));
    }

    #[test]
    fn close_clears_state() {
        let mut s: DropdownState<A> = DropdownState::new();
        s.open(A::Wavetable, ANCHOR, 50, 3, true, WIN);
        s.close();
        assert!(!s.is_open());
        assert!(!s.is_open_for(A::Wavetable));
    }

    #[test]
    fn is_open_for_false_when_closed() {
        let s: DropdownState<A> = DropdownState::new();
        assert!(!s.is_open_for(A::Wavetable));
    }
}
