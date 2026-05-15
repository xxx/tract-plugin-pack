# Dropdown Menu Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a generic dropdown (popup-list) widget to the `tiny-skia-widgets` crate, with scroll, typeahead filter, full keyboard navigation, and full test coverage.

**Architecture:** A single `dropdown.rs` module. State lives in a `DropdownState<A>` struct (one open dropdown at a time, parameterized over the caller's action enum — the same pattern as `TextEditState<A>`). One pure `dropdown_popup_layout` function backs both drawing and hit testing. Two drawing functions render the collapsed trigger and the open popup. Event handler methods process mouse/keyboard/wheel input and return `DropdownEvent<A>`.

**Tech Stack:** Rust (nightly), `tiny-skia` (CPU rendering), `fontdue` (via the crate's `TextRenderer`). No new dependencies.

**Deviations from the design spec** (refinements found during planning — all minor, all improve correctness/consistency):

1. `open`, `on_key`, `on_char`, and `on_wheel` take extra parameters (`items` and/or `window_size`) that the spec's illustrative signatures omitted. They are needed to recompute the filtered list and to clamp/scroll-into-view. The editor always has `window_size` on hand.
2. The spec's `tick()` + `caret_blink_phase` are replaced by an elapsed-time `caret_visible()` method, exactly matching `TextEditState::caret_visible()`. No per-frame toggle call is needed.
3. The generic bound is `A: Copy + PartialEq` (Copy for ergonomic event construction; PartialEq matches the crate's comparison style).

**Reference reading before starting:**
- `tiny-skia-widgets/src/text_edit.rs` — the `State<A>` pattern this mirrors.
- `tiny-skia-widgets/src/grid_selector.rs` — `*_layout` function + drawing split, test style.
- `tiny-skia-widgets/src/primitives.rs` — `draw_rect`, `draw_rect_outline`, `color_*` helpers.
- `tiny-skia-widgets/src/text.rs` — `TextRenderer::{text_width, draw_text}`.
- `docs/superpowers/specs/2026-05-15-dropdown-widget-design.md` — the design spec.

**Test command convention:** run a single test with
`cargo nextest run -p tiny-skia-widgets <test_name_substring>`.
Run the module's whole suite with
`cargo nextest run -p tiny-skia-widgets dropdown`.

---

## File Structure

- **Create:** `tiny-skia-widgets/src/dropdown.rs` — the entire widget: types, layout function, drawing functions, state methods, tests.
- **Modify:** `tiny-skia-widgets/src/lib.rs` — register and re-export the module.

One file is correct here: the widget is ~600–700 lines including tests, matching `grid_selector.rs` (648 lines) and `param_dial.rs` (638 lines). No split needed.

---

## Task 1: Module scaffold and core types

**Files:**
- Create: `tiny-skia-widgets/src/dropdown.rs`
- Modify: `tiny-skia-widgets/src/lib.rs:13` (add `pub mod dropdown;`) and `:26` (add `pub use dropdown::*;`)

- [ ] **Step 1: Create `dropdown.rs` with types and a construction test**

```rust
//! Dropdown (popup-list) widget for softbuffer-based nih-plug editors.
//!
//! Fills the niche above `draw_stepped_selector` (small enums) and
//! `grid_selector` (~9 inline options): many options, or scarce horizontal
//! space. State lives in `DropdownState<A>`, mirroring `TextEditState<A>`:
//! at most one dropdown open at a time, tagged by the caller's action enum.
//!
//! See `docs/superpowers/specs/2026-05-15-dropdown-widget-design.md`.

use std::time::Instant;

use tiny_skia::Pixmap;

use crate::primitives::{
    color_accent, color_border, color_control_bg, color_edit_bg, color_muted, color_text,
    draw_rect, draw_rect_outline,
};
use crate::text::TextRenderer;

/// Maximum rows shown before the list scrolls.
pub const MAX_VISIBLE_ROWS: usize = 12;

/// Filter-text buffer cap (defensive — typed filters are short).
const MAX_FILTER_LEN: usize = 64;

/// Popup border thickness, in physical pixels.
const BORDER: f32 = 1.0;

/// Scrollbar strip width, in physical pixels.
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
struct Active<A> {
    action: A,
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
#[derive(Clone, PartialEq, Debug)]
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
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `tiny-skia-widgets/src/lib.rs`, add `pub mod dropdown;` in the `pub mod` block (alphabetical order — after `pub mod drag;`) and `pub use dropdown::*;` in the `pub use` block (after `pub use drag::*;`).

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — `new_reports_closed` passes and the crate compiles. The
`primitives` / `TextRenderer` imports are unused until drawing lands in
Tasks 9-10, which produces dead-code warnings (not errors — `cargo nextest`
still passes). Add `#[allow(unused_imports)]` directly above the
`use crate::primitives::{...}` and `use crate::text::TextRenderer;` lines to
silence them. Task 9 removes both attributes once every symbol is used.

- [ ] **Step 4: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs tiny-skia-widgets/src/lib.rs
git commit -m "feat(widgets): scaffold dropdown module with core types"
```

---

## Task 2: Filter matcher

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing tests for `filter_matches` and `filtered_indices`**

Add inside `mod tests`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `filter_matches` / `filtered_indices` not defined.

- [ ] **Step 3: Implement the matcher**

Add to `dropdown.rs` at module scope (after the constants, before `impl`):

```rust
/// Case-insensitive substring match. An empty filter matches everything.
fn filter_matches(item: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    item.to_lowercase().contains(&filter.to_lowercase())
}

/// UNFILTERED indices of items matching `filter`, in original order.
/// Allocates — fine here, this runs on the editor thread, never `process()`.
fn filtered_indices(items: &[&str], filter: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| filter_matches(item, filter))
        .map(|(i, _)| i)
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 6 new tests green.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown filter matcher"
```

---

## Task 3: open / close / is_open_for

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
const WIN: (f32, f32) = (800.0, 600.0);
const ANCHOR: (f32, f32, f32, f32) = (100.0, 100.0, 160.0, 24.0);

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `open`, `close`, `is_open_for`, `highlight_for_test` not defined.

- [ ] **Step 3: Implement open/close/is_open_for plus a test accessor**

Add these methods to the existing `impl<A: Copy + PartialEq> DropdownState<A>` block:

```rust
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
```

Also add a private placeholder for the scroll helper (the real body lands in
Task 4, once layout exists — for now it must compile and be a safe no-op):

```rust
    /// Scroll so the highlight is visible, assuming an empty filter.
    /// Real implementation lands in Task 4; no-op stub keeps Task 3 compiling.
    fn scroll_highlight_into_view_empty_filter(&mut self, _window_size: (f32, f32)) {}
```

And a test-only accessor, gated so it never ships in release builds:

```rust
    #[cfg(test)]
    fn highlight_for_test(&self) -> Option<usize> {
        self.active.as_ref().map(|a| a.highlight)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 5 new tests green.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown open/close lifecycle"
```

---

## Task 4: Popup layout function

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

This task implements `dropdown_popup_layout` and replaces the Task 3 scroll stub with the real implementation.

- [ ] **Step 1: Write failing layout tests**

Add inside `mod tests`:

```rust
// Helper: build an open state with N items and a given filter.
fn open_state(item_count: usize, filter_enabled: bool) -> DropdownState<A> {
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, item_count, 0, filter_enabled, WIN);
    s
}

#[test]
fn layout_opens_downward_when_room_below() {
    let s = open_state(5, false);
    let items: Vec<&str> = vec!["a"; 5];
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    assert!(!l.opens_upward);
    // popup starts just below the anchor bottom (100 + 24)
    assert_eq!(l.popup_rect.1, 124.0);
}

#[test]
fn layout_flips_upward_when_no_room_below() {
    // Anchor near the bottom of an 600px window; 12 rows of 24px won't fit.
    let mut s: DropdownState<A> = DropdownState::new();
    let low_anchor = (100.0, 560.0, 160.0, 24.0);
    s.open(A::Wavetable, low_anchor, 20, 0, false, WIN);
    let items: Vec<&str> = vec!["a"; 20];
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    assert!(l.opens_upward);
    // popup bottom touches the anchor top (560)
    assert!((l.popup_rect.1 + l.popup_rect.3 - 560.0).abs() < 0.01);
}

#[test]
fn layout_clamps_to_right_window_edge() {
    let mut s: DropdownState<A> = DropdownState::new();
    let edge_anchor = (760.0, 100.0, 160.0, 24.0); // 760 + 160 = 920 > 800
    s.open(A::Wavetable, edge_anchor, 3, 0, false, WIN);
    let items: Vec<&str> = vec!["a"; 3];
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    assert!(l.popup_rect.0 + l.popup_rect.2 <= 800.0 + 0.01);
}

#[test]
fn layout_no_scrollbar_when_few_items() {
    let s = open_state(5, false);
    let items: Vec<&str> = vec!["a"; 5];
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    assert!(l.scrollbar.is_none());
}

#[test]
fn layout_has_scrollbar_when_overflowing() {
    let s = open_state(40, false);
    let items: Vec<&str> = vec!["a"; 40];
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    assert!(l.scrollbar.is_some());
    // content height = 40 rows * 24px
    assert!((l.content_height - 40.0 * 24.0).abs() < 0.01);
}

#[test]
fn layout_filter_rect_present_only_when_enabled() {
    let with = open_state(5, true);
    let without = open_state(5, false);
    let items: Vec<&str> = vec!["a"; 5];
    assert!(dropdown_popup_layout(&with, &items, WIN).unwrap().filter_rect.is_some());
    assert!(dropdown_popup_layout(&without, &items, WIN).unwrap().filter_rect.is_none());
}

#[test]
fn layout_visible_rows_map_to_unfiltered_indices() {
    // 4 items; filter "bra" -> matches indices 1,2,3.
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 4, 0, true, WIN);
    s.set_filter_for_test("bra");
    let items = ["alpha", "bravo", "bravado", "bract"];
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    let mapped: Vec<usize> = l.visible_rows.iter().map(|r| r.item_index).collect();
    assert_eq!(mapped, vec![1, 2, 3]);
}

#[test]
fn layout_returns_none_when_closed() {
    let s: DropdownState<A> = DropdownState::new();
    let items: Vec<&str> = vec!["a"; 3];
    assert!(dropdown_popup_layout(&s, &items, WIN).is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `dropdown_popup_layout` and `set_filter_for_test` not defined.

- [ ] **Step 3: Implement the layout function**

Add at module scope (after `filtered_indices`):

```rust
/// Compute popup geometry for the open dropdown. Returns `None` when closed.
///
/// Pure: this is the single source of truth for both drawing and hit
/// testing. `items` is the UNFILTERED list; rows are filtered internally
/// against the active filter.
pub fn dropdown_popup_layout<A: Copy + PartialEq>(
    state: &DropdownState<A>,
    items: &[&str],
    window_size: (f32, f32),
) -> Option<DropdownPopupLayout> {
    let active = state.active.as_ref()?;
    let (win_w, win_h) = window_size;
    let (ax, ay, aw, ah) = active.anchor;

    let item_h = ah;
    let filter_h = if active.filter_enabled { ah } else { 0.0 };

    let matches = filtered_indices(items, &active.filter);
    let n = matches.len();
    let content_height = n as f32 * item_h;

    let visible = n.min(MAX_VISIBLE_ROWS);
    let desired_list_h = visible.max(1) as f32 * item_h;
    let desired_popup_h = filter_h + desired_list_h + 2.0 * BORDER;

    let space_below = win_h - (ay + ah);
    let space_above = ay;

    let (popup_y, popup_h, opens_upward) = if desired_popup_h <= space_below {
        (ay + ah, desired_popup_h, false)
    } else if space_above > space_below {
        let h = desired_popup_h.min(space_above);
        (ay - h, h, true)
    } else {
        let h = desired_popup_h.min(space_below);
        (ay + ah, h, false)
    };

    let popup_w = aw;
    let popup_x = ax.min(win_w - popup_w).max(0.0);

    // Filter row sits at the top of the popup interior.
    let filter_rect = if active.filter_enabled {
        Some((popup_x + BORDER, popup_y + BORDER, popup_w - 2.0 * BORDER, filter_h))
    } else {
        None
    };

    let lv_x = popup_x + BORDER;
    let lv_y = popup_y + BORDER + filter_h;
    let lv_w = popup_w - 2.0 * BORDER;
    let lv_h = popup_h - 2.0 * BORDER - filter_h;
    let list_viewport = (lv_x, lv_y, lv_w, lv_h);

    let has_scrollbar = content_height > lv_h + 0.01;
    let row_w = if has_scrollbar { lv_w - SCROLLBAR_W } else { lv_w };

    // Visible rows: filtered position k -> screen y. Keep rows that intersect
    // the viewport at all.
    let mut visible_rows = Vec::new();
    for (k, &item_index) in matches.iter().enumerate() {
        let row_y = lv_y - active.scroll_px + k as f32 * item_h;
        if row_y + item_h <= lv_y || row_y >= lv_y + lv_h {
            continue;
        }
        visible_rows.push(RowRect {
            rect: (lv_x, row_y, row_w, item_h),
            item_index,
        });
    }

    let scrollbar = if has_scrollbar {
        let track = (lv_x + lv_w - SCROLLBAR_W, lv_y, SCROLLBAR_W, lv_h);
        let thumb_h = (lv_h * (lv_h / content_height)).max(8.0);
        let max_scroll = (content_height - lv_h).max(0.0);
        let frac = if max_scroll > 0.0 {
            active.scroll_px / max_scroll
        } else {
            0.0
        };
        let thumb_y = lv_y + frac * (lv_h - thumb_h);
        Some(ScrollbarRect {
            track,
            thumb: (track.0, thumb_y, SCROLLBAR_W, thumb_h),
        })
    } else {
        None
    };

    Some(DropdownPopupLayout {
        popup_rect: (popup_x, popup_y, popup_w, popup_h),
        filter_rect,
        list_viewport,
        visible_rows,
        scrollbar,
        content_height,
        opens_upward,
    })
}
```

- [ ] **Step 4: Replace the Task 3 scroll stub with the real implementation**

Replace the `scroll_highlight_into_view_empty_filter` stub body, and add a
general scroll-into-view helper plus a scroll clamp, in the `impl` block:

```rust
    /// Clamp `scroll_px` into `[0, content_height - viewport_h]`.
    fn clamp_scroll(&mut self, items: &[&str], window_size: (f32, f32)) {
        let Some(layout) = dropdown_popup_layout(self, items, window_size) else {
            return;
        };
        let max_scroll = (layout.content_height - layout.list_viewport.3).max(0.0);
        if let Some(active) = self.active.as_mut() {
            active.scroll_px = active.scroll_px.clamp(0.0, max_scroll);
        }
    }

    /// Scroll so the highlighted item's row is fully inside the viewport.
    fn scroll_highlight_into_view(&mut self, items: &[&str], window_size: (f32, f32)) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let matches = filtered_indices(items, &active.filter);
        let Some(k) = matches.iter().position(|&i| i == active.highlight) else {
            return;
        };
        let Some(layout) = dropdown_popup_layout(self, items, window_size) else {
            return;
        };
        let item_h = active.anchor.3;
        let viewport_h = layout.list_viewport.3;
        let row_top = k as f32 * item_h;
        let row_bot = row_top + item_h;
        let active = self.active.as_mut().unwrap();
        if row_top < active.scroll_px {
            active.scroll_px = row_top;
        } else if row_bot > active.scroll_px + viewport_h {
            active.scroll_px = row_bot - viewport_h;
        }
    }
```

Then change `open()` to call the real helper. Replace the stub call
`self.scroll_highlight_into_view_empty_filter(window_size);` with — and delete
the stub method entirely:

```rust
        // (in open(), after constructing self.active)
        // Empty filter at open time, so the unfiltered `items` are unavailable
        // here; build a trivial slice of the right length for the scroll math.
        let dummy: Vec<&str> = vec![""; item_count];
        self.scroll_highlight_into_view(&dummy, window_size);
```

This works because with an empty filter every item matches regardless of text,
so `filtered_indices` over the dummy slice yields `0..item_count` — the same
positions the real items would.

- [ ] **Step 5: Add the `set_filter_for_test` accessor**

Add to the `impl` block:

```rust
    #[cfg(test)]
    fn set_filter_for_test(&mut self, f: &str) {
        if let Some(active) = self.active.as_mut() {
            active.filter = f.to_string();
        }
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 8 layout tests plus the Task 3 tests green.

- [ ] **Step 7: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown popup layout + scroll math"
```

---

## Task 5: Keyboard navigation (`on_key`)

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn key_down_moves_highlight_forward() {
    let mut s = open_state(5, false);
    let items: Vec<&str> = vec!["a"; 5];
    let ev = s.on_key(DropdownKey::Down, &items, WIN);
    assert_eq!(ev, Some(DropdownEvent::HighlightChanged(A::Wavetable, 1)));
    assert_eq!(s.highlight_for_test(), Some(1));
}

#[test]
fn key_up_moves_highlight_back() {
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 5, 3, false, WIN);
    let items: Vec<&str> = vec!["a"; 5];
    s.on_key(DropdownKey::Up, &items, WIN);
    assert_eq!(s.highlight_for_test(), Some(2));
}

#[test]
fn key_down_clamps_at_last_item() {
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 3, 2, false, WIN);
    let items: Vec<&str> = vec!["a"; 3];
    s.on_key(DropdownKey::Down, &items, WIN);
    assert_eq!(s.highlight_for_test(), Some(2));
}

#[test]
fn key_up_clamps_at_first_item() {
    let mut s = open_state(3, false);
    let items: Vec<&str> = vec!["a"; 3];
    s.on_key(DropdownKey::Up, &items, WIN);
    assert_eq!(s.highlight_for_test(), Some(0));
}

#[test]
fn key_home_and_end_jump() {
    let mut s = open_state(10, false);
    let items: Vec<&str> = vec!["a"; 10];
    s.on_key(DropdownKey::End, &items, WIN);
    assert_eq!(s.highlight_for_test(), Some(9));
    s.on_key(DropdownKey::Home, &items, WIN);
    assert_eq!(s.highlight_for_test(), Some(0));
}

#[test]
fn key_enter_emits_selected_and_closes() {
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 5, 2, false, WIN);
    let items: Vec<&str> = vec!["a"; 5];
    let ev = s.on_key(DropdownKey::Enter, &items, WIN);
    assert_eq!(ev, Some(DropdownEvent::Selected(A::Wavetable, 2)));
    assert!(!s.is_open());
}

#[test]
fn key_esc_emits_closed() {
    let mut s = open_state(5, false);
    let items: Vec<&str> = vec!["a"; 5];
    let ev = s.on_key(DropdownKey::Esc, &items, WIN);
    assert_eq!(ev, Some(DropdownEvent::Closed(A::Wavetable)));
    assert!(!s.is_open());
}

#[test]
fn key_arrows_navigate_filtered_set() {
    // 4 items, filter "bra" -> visible indices 1,2,3. Down from highlight=1
    // (filtered position 0) should land on 2, the next *visible* item.
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 4, 1, true, WIN);
    s.set_filter_for_test("bra");
    let items = ["alpha", "bravo", "bravado", "bract"];
    s.on_key(DropdownKey::Down, &items, WIN);
    assert_eq!(s.highlight_for_test(), Some(2));
}

#[test]
fn key_enter_noop_when_no_matches() {
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 4, 0, true, WIN);
    s.set_filter_for_test("zzz");
    let items = ["alpha", "bravo", "bravado", "bract"];
    let ev = s.on_key(DropdownKey::Enter, &items, WIN);
    assert_eq!(ev, None);
    assert!(s.is_open(), "Enter with no matches must not close");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `on_key` not defined.

- [ ] **Step 3: Implement `on_key`**

Add to the `impl` block:

```rust
    /// Handle a key event for the open dropdown. Returns `None` when closed
    /// or the key has no effect. Arrow/Page/Home/End move the highlight over
    /// the *visible* (filtered) items; Enter selects; Esc closes.
    pub fn on_key(
        &mut self,
        key: DropdownKey,
        items: &[&str],
        window_size: (f32, f32),
    ) -> Option<DropdownEvent<A>> {
        let active = self.active.as_ref()?;
        let action = active.action;
        let matches = filtered_indices(items, &active.filter);

        match key {
            DropdownKey::Esc => {
                self.close();
                Some(DropdownEvent::Closed(action))
            }
            DropdownKey::Enter => {
                if matches.is_empty() {
                    return None;
                }
                let highlight = active.highlight;
                self.close();
                Some(DropdownEvent::Selected(action, highlight))
            }
            DropdownKey::Backspace => self.on_char_internal(None, items),
            DropdownKey::Up
            | DropdownKey::Down
            | DropdownKey::PageUp
            | DropdownKey::PageDown
            | DropdownKey::Home
            | DropdownKey::End => {
                if matches.is_empty() {
                    return None;
                }
                // Current filtered position of the highlight (fall back to 0).
                let cur = matches
                    .iter()
                    .position(|&i| i == active.highlight)
                    .unwrap_or(0);
                let last = matches.len() - 1;
                let page = MAX_VISIBLE_ROWS.saturating_sub(1).max(1);
                let next = match key {
                    DropdownKey::Up => cur.saturating_sub(1),
                    DropdownKey::Down => (cur + 1).min(last),
                    DropdownKey::PageUp => cur.saturating_sub(page),
                    DropdownKey::PageDown => (cur + page).min(last),
                    DropdownKey::Home => 0,
                    DropdownKey::End => last,
                    _ => cur,
                };
                let new_highlight = matches[next];
                self.active.as_mut().unwrap().highlight = new_highlight;
                self.scroll_highlight_into_view(items, window_size);
                Some(DropdownEvent::HighlightChanged(action, new_highlight))
            }
        }
    }
```

`on_key` references `on_char_internal` for Backspace — that helper is
implemented in Task 6. To keep this task compiling, add a minimal stub now and
replace it in Task 6:

```rust
    /// Stub — real body lands in Task 6.
    fn on_char_internal(
        &mut self,
        _c: Option<char>,
        _items: &[&str],
    ) -> Option<DropdownEvent<A>> {
        None
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 9 keyboard tests green.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown keyboard navigation"
```

---

## Task 6: Typeahead filter input (`on_char` + Backspace)

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn char_appends_to_filter_when_enabled() {
    let mut s = open_state(4, true);
    let items = ["sine", "saw", "square", "sample"];
    s.on_char('s', &items);
    s.on_char('a', &items);
    assert_eq!(s.filter_for_test(), Some("sa"));
}

#[test]
fn char_ignored_when_filter_disabled() {
    let mut s = open_state(4, false);
    let items = ["sine", "saw", "square", "sample"];
    s.on_char('s', &items);
    assert_eq!(s.filter_for_test(), Some(""));
}

#[test]
fn char_resets_highlight_to_first_match() {
    let mut s: DropdownState<A> = DropdownState::new();
    s.open(A::Wavetable, ANCHOR, 4, 3, true, WIN);
    let items = ["sine", "saw", "square", "sample"];
    // typing "sq" filters to index 2 only -> highlight should become 2.
    s.on_char('s', &items);
    s.on_char('q', &items);
    assert_eq!(s.highlight_for_test(), Some(2));
}

#[test]
fn char_resets_scroll_to_top() {
    let mut s = open_state(40, true);
    let items: Vec<&str> = vec!["wave"; 40];
    s.set_scroll_for_test(200.0);
    s.on_char('w', &items);
    assert_eq!(s.scroll_for_test(), Some(0.0));
}

#[test]
fn backspace_removes_last_filter_char() {
    let mut s = open_state(4, true);
    let items = ["sine", "saw", "square", "sample"];
    s.on_char('s', &items);
    s.on_char('a', &items);
    s.on_key(DropdownKey::Backspace, &items, WIN);
    assert_eq!(s.filter_for_test(), Some("s"));
}

#[test]
fn char_respects_filter_length_cap() {
    let mut s = open_state(4, true);
    let items: Vec<&str> = vec!["x"; 4];
    for _ in 0..100 {
        s.on_char('x', &items);
    }
    assert_eq!(s.filter_for_test().unwrap().len(), MAX_FILTER_LEN);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `on_char`, `filter_for_test`, `set_scroll_for_test`,
`scroll_for_test` not defined.

- [ ] **Step 3: Replace the `on_char_internal` stub with the real body and add `on_char`**

Delete the Task 5 `on_char_internal` stub and replace it with:

```rust
    /// Public char-input handler. Only effective when the filter is enabled.
    pub fn on_char(&mut self, c: char, items: &[&str]) -> Option<DropdownEvent<A>> {
        self.on_char_internal(Some(c), items)
    }

    /// Shared filter-edit logic. `Some(c)` appends; `None` is a backspace.
    /// Resets scroll to the top and the highlight to the first match.
    fn on_char_internal(
        &mut self,
        c: Option<char>,
        items: &[&str],
    ) -> Option<DropdownEvent<A>> {
        let active = self.active.as_mut()?;
        if !active.filter_enabled {
            return None;
        }
        match c {
            Some(ch) => {
                if ch.is_control() || active.filter.len() >= MAX_FILTER_LEN {
                    return None;
                }
                active.filter.push(ch);
            }
            None => {
                active.filter.pop();
            }
        }
        active.scroll_px = 0.0;
        self.last_filter_change = Instant::now();

        let action = active.action;
        let matches = filtered_indices(items, &active.filter);
        let first = matches.first().copied();
        let active = self.active.as_mut().unwrap();
        match first {
            Some(idx) => {
                active.highlight = idx;
                Some(DropdownEvent::HighlightChanged(action, idx))
            }
            None => None,
        }
    }
```

- [ ] **Step 4: Add the test accessors**

Add to the `impl` block:

```rust
    #[cfg(test)]
    fn filter_for_test(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.filter.as_str())
    }

    #[cfg(test)]
    fn scroll_for_test(&self) -> Option<f32> {
        self.active.as_ref().map(|a| a.scroll_px)
    }

    #[cfg(test)]
    fn set_scroll_for_test(&mut self, v: f32) {
        if let Some(a) = self.active.as_mut() {
            a.scroll_px = v;
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 6 filter tests plus `backspace_removes_last_filter_char`
(which exercises the now-real `on_char_internal` via `on_key`) green.

- [ ] **Step 6: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown typeahead filter input"
```

---

## Task 7: Mouse and wheel handlers

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn mouse_down_on_item_selects_and_closes() {
    let s_items: Vec<&str> = vec!["a"; 5];
    let mut s = open_state(5, false);
    let l = dropdown_popup_layout(&s, &s_items, WIN).unwrap();
    let row = l.visible_rows[2];
    let (rx, ry, rw, rh) = row.rect;
    let ev = s.on_mouse_down(rx + rw * 0.5, ry + rh * 0.5, &s_items, WIN);
    assert_eq!(ev, Some(DropdownEvent::Selected(A::Wavetable, row.item_index)));
    assert!(!s.is_open());
}

#[test]
fn mouse_down_outside_closes() {
    let s_items: Vec<&str> = vec!["a"; 5];
    let mut s = open_state(5, false);
    // Far from the popup (popup is around x=100..260, y=124..).
    let ev = s.on_mouse_down(5.0, 5.0, &s_items, WIN);
    assert_eq!(ev, Some(DropdownEvent::Closed(A::Wavetable)));
    assert!(!s.is_open());
}

#[test]
fn mouse_down_on_filter_row_keeps_open() {
    let s_items: Vec<&str> = vec!["a"; 5];
    let mut s = open_state(5, true);
    let l = dropdown_popup_layout(&s, &s_items, WIN).unwrap();
    let (fx, fy, fw, fh) = l.filter_rect.unwrap();
    let ev = s.on_mouse_down(fx + fw * 0.5, fy + fh * 0.5, &s_items, WIN);
    assert_eq!(ev, None);
    assert!(s.is_open());
}

#[test]
fn mouse_move_over_row_updates_highlight() {
    let s_items: Vec<&str> = vec!["a"; 5];
    let mut s = open_state(5, false);
    let l = dropdown_popup_layout(&s, &s_items, WIN).unwrap();
    let row = l.visible_rows[3];
    let (rx, ry, rw, rh) = row.rect;
    let ev = s.on_mouse_move(rx + rw * 0.5, ry + rh * 0.5, &s_items, WIN);
    assert_eq!(
        ev,
        Some(DropdownEvent::HighlightChanged(A::Wavetable, row.item_index))
    );
    assert_eq!(s.highlight_for_test(), Some(row.item_index));
}

#[test]
fn wheel_scrolls_and_clamps() {
    let s_items: Vec<&str> = vec!["a"; 40];
    let mut s = open_state(40, false);
    // Scroll way past the end; must clamp to max_scroll, not exceed it.
    s.on_wheel(-100000.0, &s_items, WIN);
    let l = dropdown_popup_layout(&s, &s_items, WIN).unwrap();
    let max_scroll = l.content_height - l.list_viewport.3;
    assert!((s.scroll_for_test().unwrap() - max_scroll).abs() < 0.01);
    // Scroll back past zero; must clamp to 0.
    s.on_wheel(100000.0, &s_items, WIN);
    assert_eq!(s.scroll_for_test(), Some(0.0));
}

#[test]
fn scrollbar_thumb_drag_scrolls() {
    let s_items: Vec<&str> = vec!["a"; 40];
    let mut s = open_state(40, false);
    let l = dropdown_popup_layout(&s, &s_items, WIN).unwrap();
    let thumb = l.scrollbar.unwrap().thumb;
    // Press on the thumb, drag down by 50px, release.
    s.on_mouse_down(thumb.0 + 2.0, thumb.1 + 2.0, &s_items, WIN);
    s.on_mouse_move(thumb.0 + 2.0, thumb.1 + 52.0, &s_items, WIN);
    assert!(s.scroll_for_test().unwrap() > 0.0, "thumb drag should scroll");
    s.on_mouse_up();
    // After release a plain move must not keep scrolling.
    let after = s.scroll_for_test().unwrap();
    s.on_mouse_move(thumb.0 + 2.0, thumb.1 + 200.0, &s_items, WIN);
    assert_eq!(s.scroll_for_test(), Some(after));
}

#[test]
fn mouse_handlers_noop_when_closed() {
    let s_items: Vec<&str> = vec!["a"; 5];
    let mut s: DropdownState<A> = DropdownState::new();
    assert_eq!(s.on_mouse_down(10.0, 10.0, &s_items, WIN), None);
    assert_eq!(s.on_mouse_move(10.0, 10.0, &s_items, WIN), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `on_mouse_down`, `on_mouse_move`, `on_mouse_up`, `on_wheel`
not defined.

- [ ] **Step 3: Implement the mouse and wheel handlers**

Add to the `impl` block. Add a small private hit-test helper first:

```rust
    /// `true` when `(x, y)` lies inside `rect` (half-open).
    fn point_in(rect: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
        x >= rect.0 && x < rect.0 + rect.2 && y >= rect.1 && y < rect.1 + rect.3
    }

    /// Handle a primary-button press. Inside an item row -> select + close.
    /// On the scrollbar thumb -> begin a drag. Outside the popup -> close.
    pub fn on_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        items: &[&str],
        window_size: (f32, f32),
    ) -> Option<DropdownEvent<A>> {
        let layout = dropdown_popup_layout(self, items, window_size)?;
        let action = self.active.as_ref()?.action;

        // Scrollbar thumb -> start dragging.
        if let Some(sb) = layout.scrollbar {
            if Self::point_in(sb.thumb, x, y) {
                if let Some(active) = self.active.as_mut() {
                    active.scrollbar_drag = Some(y - sb.thumb.1);
                }
                return None;
            }
        }

        // Item row -> select and close.
        for row in &layout.visible_rows {
            if Self::point_in(row.rect, x, y) {
                let idx = row.item_index;
                self.close();
                return Some(DropdownEvent::Selected(action, idx));
            }
        }

        // Inside the popup but not on a row (filter row, scrollbar track,
        // padding) -> consume, keep open.
        if Self::point_in(layout.popup_rect, x, y) {
            return None;
        }

        // Outside the popup -> close. The click is consumed by the editor.
        self.close();
        Some(DropdownEvent::Closed(action))
    }

    /// Handle pointer motion. Updates the highlight on row hover, or scrolls
    /// when a scrollbar-thumb drag is in progress.
    pub fn on_mouse_move(
        &mut self,
        x: f32,
        y: f32,
        items: &[&str],
        window_size: (f32, f32),
    ) -> Option<DropdownEvent<A>> {
        let layout = dropdown_popup_layout(self, items, window_size)?;
        let active = self.active.as_ref()?;
        let action = active.action;

        // Scrollbar drag in progress.
        if let Some(grab) = active.scrollbar_drag {
            let lv = layout.list_viewport;
            let thumb_h = layout
                .scrollbar
                .map(|sb| sb.thumb.3)
                .unwrap_or(lv.3);
            let travel = (lv.3 - thumb_h).max(1.0);
            let thumb_y = (y - grab).clamp(lv.1, lv.1 + travel);
            let frac = (thumb_y - lv.1) / travel;
            let max_scroll = (layout.content_height - lv.3).max(0.0);
            if let Some(active) = self.active.as_mut() {
                active.scroll_px = frac * max_scroll;
            }
            return None;
        }

        // Row hover -> move highlight.
        for row in &layout.visible_rows {
            if Self::point_in(row.rect, x, y) {
                let idx = row.item_index;
                if self.active.as_ref().unwrap().highlight != idx {
                    self.active.as_mut().unwrap().highlight = idx;
                    return Some(DropdownEvent::HighlightChanged(action, idx));
                }
                return None;
            }
        }
        None
    }

    /// End any in-progress scrollbar-thumb drag.
    pub fn on_mouse_up(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.scrollbar_drag = None;
        }
    }

    /// Scroll the list. `delta_y` follows the usual convention: positive
    /// scrolls toward the top of the list.
    pub fn on_wheel(&mut self, delta_y: f32, items: &[&str], window_size: (f32, f32)) {
        const WHEEL_STEP: f32 = 32.0;
        if let Some(active) = self.active.as_mut() {
            active.scroll_px -= delta_y * WHEEL_STEP;
        }
        self.clamp_scroll(items, window_size);
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 7 mouse/wheel tests green.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown mouse and wheel handlers"
```

---

## Task 8: Caret-blink helper

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing tests**

Add inside `mod tests`:

```rust
#[test]
fn caret_hidden_when_closed() {
    let s: DropdownState<A> = DropdownState::new();
    assert!(!s.caret_visible());
}

#[test]
fn caret_hidden_when_filter_disabled() {
    let s = open_state(5, false);
    assert!(!s.caret_visible());
}

#[test]
fn caret_visible_at_open_when_filter_enabled() {
    let s = open_state(5, true);
    // Freshly opened -> within the first 500ms "on" half of the cycle.
    assert!(s.caret_visible());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `caret_visible` not defined.

- [ ] **Step 3: Implement `caret_visible`**

Add to the `impl` block:

```rust
    /// `true` during the "on" half of the 1000ms blink cycle, but only when
    /// a filter-enabled dropdown is open. The blink phase is measured from
    /// the last filter edit (or open), matching `TextEditState::caret_visible`.
    pub fn caret_visible(&self) -> bool {
        match &self.active {
            Some(a) if a.filter_enabled => {
                (self.last_filter_change.elapsed().as_millis() % 1000) < 500
            }
            _ => false,
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 3 caret tests green.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown filter-row caret blink"
```

---

## Task 9: Trigger drawing (`draw_dropdown_trigger`)

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write a failing render smoke test**

Add inside `mod tests` — note the new imports at the top of the test module:

```rust
    use crate::test_font::test_font_data;
    use tiny_skia::Pixmap;

    fn px_alpha(pm: &Pixmap, x: u32, y: u32) -> u8 {
        pm.pixels()[(y * pm.width() + x) as usize].alpha()
    }
```

(If `mod tests` already has `use super::*;`, keep it and add the two `use`
lines below it.)

Then the test:

```rust
#[test]
fn draw_trigger_paints_something() {
    let mut pm = Pixmap::new(300, 80).unwrap();
    let mut tr = TextRenderer::new(&test_font_data());
    draw_dropdown_trigger(&mut pm, &mut tr, (10.0, 10.0, 200.0, 28.0), "Sine.wt", false);
    // The trigger background fills its rect — interior pixel must be painted.
    assert!(px_alpha(&pm, 100, 24) > 0, "trigger background not drawn");
}

#[test]
fn draw_trigger_open_does_not_panic() {
    let mut pm = Pixmap::new(300, 80).unwrap();
    let mut tr = TextRenderer::new(&test_font_data());
    draw_dropdown_trigger(&mut pm, &mut tr, (10.0, 10.0, 200.0, 28.0), "Sine.wt", true);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `draw_dropdown_trigger` not defined.

- [ ] **Step 3: Implement `draw_dropdown_trigger`**

Add at module scope. It also needs a text-truncation helper — add that first:

```rust
/// Truncate `text` with a trailing "…" so it fits within `max_w` at `size`.
/// Returns `text` unchanged when it already fits.
fn truncate_to_width(
    tr: &mut TextRenderer,
    text: &str,
    size: f32,
    max_w: f32,
) -> String {
    if tr.text_width(text, size) <= max_w {
        return text.to_string();
    }
    let ellipsis = "…";
    let ell_w = tr.text_width(ellipsis, size);
    let mut out = String::new();
    let mut w = 0.0;
    for ch in text.chars() {
        let cw = tr.text_width(&ch.to_string(), size);
        if w + cw + ell_w > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push_str(ellipsis);
    out
}

/// Draw the collapsed dropdown trigger: bordered rect, selected-item label
/// (ellipsis-truncated), and a chevron at the right. `is_open` brightens the
/// border to the accent color and flips the chevron.
pub fn draw_dropdown_trigger(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    rect: (f32, f32, f32, f32),
    label: &str,
    is_open: bool,
) {
    let (x, y, w, h) = rect;
    draw_rect(pixmap, x, y, w, h, color_control_bg());
    let border = if is_open { color_accent() } else { color_border() };
    draw_rect_outline(pixmap, x, y, w, h, border, 1.0);

    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;
    let pad = 6.0;
    let chevron_w = h * 0.6;

    // Label region: from the left pad to the start of the chevron.
    let label_max_w = (w - 2.0 * pad - chevron_w).max(0.0);
    let shown = truncate_to_width(text_renderer, label, text_size, label_max_w);
    text_renderer.draw_text(pixmap, x + pad, text_y, &shown, text_size, color_text());

    // Chevron: a small filled triangle made of stacked 1px rows.
    let cx = x + w - pad - chevron_w * 0.5;
    let cy = y + h * 0.5;
    let tri_h = chevron_w * 0.4;
    let steps = tri_h.max(1.0) as i32;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let half = (chevron_w * 0.5) * (1.0 - t);
        // Down chevron when closed, up chevron when open.
        let row_y = if is_open { cy + tri_h * 0.5 - i as f32 } else { cy - tri_h * 0.5 + i as f32 };
        draw_rect(pixmap, cx - half, row_y, half * 2.0, 1.0, color_muted());
    }
}
```

Remove the `#[allow(unused_imports)]` added in Task 1 — every imported symbol
is now used (`color_muted` by the chevron, the rest by trigger/popup drawing).
If `cargo clippy` later flags a still-unused import, delete just that import.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — both trigger render tests green.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown trigger drawing"
```

---

## Task 10: Popup drawing (`draw_dropdown_popup`)

**Files:**
- Modify: `tiny-skia-widgets/src/dropdown.rs`

- [ ] **Step 1: Write failing render smoke tests**

Add inside `mod tests`:

```rust
#[test]
fn draw_popup_filter_disabled_paints_rows() {
    let mut pm = Pixmap::new(800, 600).unwrap();
    let mut tr = TextRenderer::new(&test_font_data());
    let s = open_state(5, false);
    let items = ["sine", "saw", "square", "triangle", "noise"];
    draw_dropdown_popup(&mut pm, &mut tr, &s, &items, WIN);
    let l = dropdown_popup_layout(&s, &items, WIN).unwrap();
    let (px, py, _, _) = l.popup_rect;
    // Popup interior must be painted.
    assert!(px_alpha(&pm, (px + 20.0) as u32, (py + 20.0) as u32) > 0);
}

#[test]
fn draw_popup_filter_enabled_does_not_panic() {
    let mut pm = Pixmap::new(800, 600).unwrap();
    let mut tr = TextRenderer::new(&test_font_data());
    let mut s = open_state(40, true);
    let items: Vec<&str> = vec!["wavetable-frame"; 40];
    s.set_filter_for_test("wave");
    draw_dropdown_popup(&mut pm, &mut tr, &s, &items, WIN);
}

#[test]
fn draw_popup_no_matches_does_not_panic() {
    let mut pm = Pixmap::new(800, 600).unwrap();
    let mut tr = TextRenderer::new(&test_font_data());
    let mut s = open_state(5, true);
    s.set_filter_for_test("zzzzz");
    let items = ["sine", "saw", "square", "triangle", "noise"];
    draw_dropdown_popup(&mut pm, &mut tr, &s, &items, WIN);
}

#[test]
fn draw_popup_closed_is_noop() {
    let mut pm = Pixmap::new(800, 600).unwrap();
    let mut tr = TextRenderer::new(&test_font_data());
    let s: DropdownState<A> = DropdownState::new();
    let items = ["a", "b"];
    draw_dropdown_popup(&mut pm, &mut tr, &s, &items, WIN);
    // Nothing open -> nothing drawn.
    assert_eq!(px_alpha(&pm, 400, 300), 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: FAIL — `draw_dropdown_popup` not defined.

- [ ] **Step 3: Implement `draw_dropdown_popup`**

Add at module scope:

```rust
/// Draw the open dropdown popup: bordered panel, optional filter row, the
/// scrollable item list (highlight tint + selected marker), and a scrollbar
/// when the list overflows. No-op when the dropdown is closed.
///
/// Call this LAST in the editor's draw pass so the popup paints over
/// everything else.
pub fn draw_dropdown_popup<A: Copy + PartialEq>(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    state: &DropdownState<A>,
    items: &[&str],
    window_size: (f32, f32),
) {
    let Some(layout) = dropdown_popup_layout(state, items, window_size) else {
        return;
    };
    let Some(active) = state.active.as_ref() else {
        return;
    };

    // Panel.
    let (px, py, pw, ph) = layout.popup_rect;
    draw_rect(pixmap, px, py, pw, ph, color_control_bg());
    draw_rect_outline(pixmap, px, py, pw, ph, color_accent(), 1.0);

    let item_h = active.anchor.3;
    let text_size = (item_h * 0.5).max(10.0);
    let pad = 6.0;

    // Filter row.
    if let Some((fx, fy, fw, fh)) = layout.filter_rect {
        draw_rect(pixmap, fx, fy, fw, fh, color_edit_bg());
        let ty = fy + (fh + text_size) * 0.5 - 2.0;
        let shown = truncate_to_width(
            text_renderer,
            &active.filter,
            text_size,
            (fw - 2.0 * pad).max(0.0),
        );
        text_renderer.draw_text(pixmap, fx + pad, ty, &shown, text_size, color_text());
        if state.caret_visible() {
            let caret_x = fx + pad + text_renderer.text_width(&shown, text_size) + 1.0;
            draw_rect(pixmap, caret_x, fy + 3.0, 1.0, fh - 6.0, color_text());
        }
    }

    // Empty-result message.
    if layout.visible_rows.is_empty() {
        let (lx, ly, lw, lh) = layout.list_viewport;
        let ty = ly + (lh.min(item_h) + text_size) * 0.5 - 2.0;
        let msg = "No matches";
        let mw = text_renderer.text_width(msg, text_size);
        text_renderer.draw_text(
            pixmap,
            lx + (lw - mw) * 0.5,
            ty,
            msg,
            text_size,
            color_muted(),
        );
        return;
    }

    // Item rows.
    for row in &layout.visible_rows {
        let (rx, ry, rw, rh) = row.rect;
        if row.item_index == active.highlight {
            draw_rect(pixmap, rx, ry, rw, rh, color_accent());
        }
        // Marker bar for the originally-selected item is omitted: `open()`
        // seeds the highlight from the current selection, so the highlight
        // tint already shows it on open. (A separate marker would only
        // matter after the highlight moves; deliberately kept minimal.)
        let label = items.get(row.item_index).copied().unwrap_or("");
        let shown = truncate_to_width(
            text_renderer,
            label,
            text_size,
            (rw - 2.0 * pad).max(0.0),
        );
        let color = if row.item_index == active.highlight {
            // Dark text on the accent fill, matching `draw_button`.
            tiny_skia::Color::from_rgba8(0x1a, 0x1c, 0x22, 0xff)
        } else {
            color_text()
        };
        let ty = ry + (rh + text_size) * 0.5 - 2.0;
        text_renderer.draw_text(pixmap, rx + pad, ty, &shown, text_size, color);
    }

    // Scrollbar.
    if let Some(sb) = layout.scrollbar {
        let (tx, ty, tw, th) = sb.track;
        draw_rect(pixmap, tx, ty, tw, th, color_border());
        let (hx, hy, hw, hh) = sb.thumb;
        draw_rect(pixmap, hx, hy, hw, hh, color_muted());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets dropdown`
Expected: PASS — all 4 popup render tests green.

- [ ] **Step 5: Run the full crate suite, clippy, and fmt**

Run: `cargo nextest run -p tiny-skia-widgets`
Expected: PASS — every test in the crate (existing 29 + the new dropdown
tests).

Run: `cargo clippy -p tiny-skia-widgets -- -D warnings`
Expected: no warnings. If an unused import remains, delete it.

Run: `cargo fmt -p tiny-skia-widgets`
Expected: no diff (or apply the formatting it produces).

- [ ] **Step 6: Commit**

```bash
git add tiny-skia-widgets/src/dropdown.rs
git commit -m "feat(widgets): add dropdown popup drawing"
```

---

## Task 11: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Run the whole workspace test suite**

Run: `cargo nextest run --workspace`
Expected: PASS — all pre-existing tests plus the new dropdown tests. No
regressions in other crates.

- [ ] **Step 2: Run workspace clippy and fmt**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --check`
Expected: no diff.

- [ ] **Step 3: Confirm the public API is re-exported**

Run: `cargo doc -p tiny-skia-widgets --no-deps`
Expected: builds clean. The crate docs list `DropdownState`, `DropdownEvent`,
`DropdownKey`, `DropdownPopupLayout`, `RowRect`, `ScrollbarRect`,
`dropdown_popup_layout`, `draw_dropdown_trigger`, `draw_dropdown_popup`, and
`MAX_VISIBLE_ROWS` at the crate root (via `pub use dropdown::*;`).

- [ ] **Step 4: No commit**

Verification only — nothing to commit if Tasks 1-10 are clean.

---

## Self-Review Notes

**Spec coverage** — every spec section maps to a task:

- Public API (state, events, key enum, layout, drawing, methods) → Tasks 1, 4, 9, 10; methods spread across 3, 5, 6, 7, 8.
- Layout & placement (downward/upward/clamp/scrollbar) → Task 4.
- Drawing detail (trigger chevron, filter row, highlight tint, scrollbar, no-matches) → Tasks 9, 10.
- Behavior — opening editor-driven → Task 3; mouse → Task 7; keyboard → Task 5; filtering → Tasks 2, 6; caret blink → Task 8.
- Testing (layout, state machine, filter matcher, render smoke) → tests embedded in every task; full suite + lint in Tasks 10-11.

**Deliberate spec simplification** — the spec's "originally-selected item marked
with a left accent bar" is dropped. Rationale: `open()` seeds the highlight from
the current selection, so the highlight tint already marks it; a separate bar
only diverges from the highlight after the user moves it, and showing two
markers at once is more confusing than helpful. Documented inline in Task 10's
code comment. If the user wants the persistent marker, it is a small additive
follow-up (store `selected: usize` in `Active`, draw a 2px bar).

**Type consistency** — `DropdownState<A>`, `Active<A>`, `DropdownEvent<A>`,
`RowRect`, `ScrollbarRect`, `DropdownPopupLayout` field names and the method
signatures (`open`, `close`, `on_key`, `on_char`, `on_mouse_*`, `on_wheel`,
`caret_visible`, `dropdown_popup_layout`) are used identically across all
tasks. The `on_char_internal` helper is stubbed in Task 5 and given its real
body in Task 6 — flagged explicitly in both tasks.
