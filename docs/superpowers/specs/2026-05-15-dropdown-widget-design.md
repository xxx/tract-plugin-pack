# Dropdown Menu Widget — Design

**Date:** 2026-05-15
**Status:** Approved
**Crate:** `tiny-skia-widgets`

## Purpose

Add a dropdown (combo-box / popup-list) widget to `tiny-skia-widgets`. It fills
the niche the existing selectors do not:

- `draw_stepped_selector` — small discrete enums (~2–4 options), inline.
- `grid_selector` — up to ~9 options shown as an inline grid.
- **dropdown** — many options (tens to hundreds), or cases where horizontal
  space is too scarce for the above.

The widget is generic: callers supply the option list and a selection callback.
It is not coupled to nih-plug. A param-backed adapter (e.g. for an `EnumParam`)
is trivial caller-side code and is out of scope here.

## Scope

In scope:

- The dropdown widget in `tiny-skia-widgets/src/dropdown.rs` (state struct,
  layout function, drawing functions, event handlers).
- Full unit tests and a few render smoke tests, inline `#[cfg(test)]`.

Out of scope:

- Wiring the widget into any plugin (a separate, later task).
- A nih-plug `Param` adapter.
- Hierarchical / grouped / multi-column lists.
- Fuzzy filtering, multi-selection, disabled items.

## Design Principles

- **Consistency with the crate.** `tiny-skia-widgets` is "pure drawing
  functions; no event handling lives here" — except that `text_edit.rs`
  already established a `State<A>` pattern for shared focus/typing state. The
  dropdown follows `TextEditState<A>` exactly: a per-editor state struct
  parameterized over the caller's action enum.
- **One open dropdown at a time** per editor. Opening one auto-closes any other.
- **Caller owns the option list** as `&[&str]`. The widget never owns or clones
  it. The caller's backing storage (`Vec<String>`, `&'static [&'static str]`,
  …) is its own concern.
- **GUI-thread allocation is acceptable.** This widget runs on the editor
  thread, never in `process()`. The CLAUDE.md "no allocations on the audio
  thread" rule does not apply; the filter may allocate a `Vec<usize>` of
  matching indices per layout call.
- **Layout is the single source of truth for hit testing.** One pure
  `dropdown_popup_layout` function backs both drawing and event handling, the
  same way `grid_selector_layout` backs `grid_selector`.

## Public API

All in `tiny-skia-widgets/src/dropdown.rs`, re-exported from `lib.rs`.

### State

```rust
/// Per-editor focus + transient state for at most one open dropdown.
/// `A` is the caller's action enum, same pattern as `TextEditState<A>`.
pub struct DropdownState<A: Copy + Eq> {
    active: Option<Active<A>>,      // None when closed
    last_filter_change: Instant,    // drives filter-row caret blink
}

struct Active<A> {
    action: A,                      // which dropdown is open
    anchor: (f32, f32, f32, f32),   // trigger rect captured at open time
    item_count: usize,              // unfiltered count
    highlight: usize,               // index into the UNFILTERED list
    scroll_px: f32,                 // viewport top, in popup-content pixels
    filter: String,                 // empty when filter disabled
    filter_enabled: bool,
    scrollbar_drag: Option<f32>,    // Some(grab_offset) while dragging thumb
    caret_blink_phase: bool,
}
```

### Events

```rust
/// Returned from event handlers. Indices are always into the UNFILTERED list.
/// There is no `Opened` variant — opening is editor-driven via `open()`, so
/// the editor already knows.
pub enum DropdownEvent<A> {
    Closed(A),
    HighlightChanged(A, usize),     // mouse hover or arrow key
    Selected(A, usize),             // Enter or click; popup auto-closes
}
```

### Crate-local key enum

The widget never depends on baseview. The editor maps host key events to:

```rust
pub enum DropdownKey { Up, Down, Enter, Esc, Backspace, PageUp, PageDown, Home, End }
```

### Layout

```rust
pub struct RowRect { pub rect: (f32, f32, f32, f32), pub item_index: usize }
pub struct ScrollbarRect { pub track: (f32,f32,f32,f32), pub thumb: (f32,f32,f32,f32) }

pub struct DropdownPopupLayout {
    pub popup_rect: (f32, f32, f32, f32),
    pub filter_rect: Option<(f32, f32, f32, f32)>,  // None when filter disabled
    pub list_viewport: (f32, f32, f32, f32),        // clipped scroll region
    pub visible_rows: Vec<RowRect>,                 // filtered rows, with mapping
    pub scrollbar: Option<ScrollbarRect>,           // None when no overflow
    pub content_height: f32,
    pub opens_upward: bool,
}

/// Pure. Backs both drawing and hit testing.
pub fn dropdown_popup_layout<A: Copy + Eq>(
    state: &DropdownState<A>,
    items: &[&str],
    window_size: (f32, f32),
) -> DropdownPopupLayout;
```

### Drawing

```rust
/// Collapsed trigger — always visible.
pub fn draw_dropdown_trigger(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    rect: (f32, f32, f32, f32),
    label: &str,            // currently-selected item text
    is_open: bool,
);

/// Open popup. Caller invokes this LAST in its draw pass so the popup
/// paints over everything else.
pub fn draw_dropdown_popup<A: Copy + Eq>(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    state: &DropdownState<A>,
    items: &[&str],         // unfiltered; widget filters per state.filter
    window_size: (f32, f32),
);
```

### State methods

```rust
impl<A: Copy + Eq> DropdownState<A> {
    pub fn new() -> Self;
    pub fn is_open(&self) -> bool;
    pub fn is_open_for(&self, a: A) -> bool;

    /// Opens `a`, auto-closing any other open dropdown. `current` seeds the
    /// highlight and is marked in the popup; the popup scrolls it into view.
    pub fn open(&mut self, a: A, anchor: (f32,f32,f32,f32),
                item_count: usize, current: usize, filter_enabled: bool);
    pub fn close(&mut self);

    // Event handlers. Return None when nothing happened.
    pub fn on_mouse_down(&mut self, x: f32, y: f32, items: &[&str],
                         window_size: (f32,f32)) -> Option<DropdownEvent<A>>;
    pub fn on_mouse_move(&mut self, x: f32, y: f32, items: &[&str],
                         window_size: (f32,f32)) -> Option<DropdownEvent<A>>;
    pub fn on_mouse_up(&mut self);                   // ends scrollbar drag
    pub fn on_wheel(&mut self, delta_y: f32);
    pub fn on_key(&mut self, key: DropdownKey,
                  items: &[&str]) -> Option<DropdownEvent<A>>;
    pub fn on_char(&mut self, c: char) -> Option<DropdownEvent<A>>;
    pub fn tick(&mut self);                          // caret blink
}
```

## Layout & Placement

All sizing keys off the trigger rect's height: `item_h = filter_row_h =
anchor.h`. Callers tune one number.

- Popup width = trigger width; x clamped so the popup never runs off the right
  window edge.
- Desired height = `filter_row_h (if enabled) + min(match_count,
  MAX_VISIBLE_ROWS) * item_h + borders`. `MAX_VISIBLE_ROWS ≈ 12`.
- **Downward** from `anchor.bottom` if it fits in `window_h - anchor.bottom`.
- Else **upward** from `anchor.top` if there is more room above than below
  (`opens_upward = true`).
- Else clamp to whichever side; the list scrolls within the clamped viewport.
- The list always scrolls internally when `content_height > viewport_h`;
  clamping only bounds the viewport.

`RowRect.item_index` carries the unfiltered index through filtering, so
`Selected` / `HighlightChanged` always report unfiltered indices regardless of
the active filter.

## Drawing Detail

- **Trigger:** bordered rect; selected-item text left-aligned and
  ellipsis-truncated to fit; chevron at right (`▾` closed, `▴` open); open state
  uses a `color_accent` border.
- **Popup:** bordered panel.
  - Optional filter row at top: typed filter text + 1px blinking caret, on the
    `color_edit_bg` highlight reused from `text_edit`.
  - Item rows: the `highlight` row (hover or arrow target) tinted
    `color_accent`; the originally-selected item marked with a left accent bar.
  - Scrollbar: thin (~5px) thumb on the right edge, present only when
    `content_height > viewport_h`.
  - List content clipped to `list_viewport`.
  - Zero filter matches: list area shows a dimmed "No matches" line; the popup
    still draws.

## Behavior

### Opening / closing

Opening is **editor-driven**: the editor hit-tests its own trigger rects and
calls `state.open(...)`. Closing is **widget-driven**: emitted as
`DropdownEvent::Closed`. The widget's `on_mouse_down` only handles interaction
inside an open popup.

### Mouse (popup open)

- Click an item row → `Selected(action, item_index)`, close.
- Click the filter row → no-op (already keyboard-focused).
- Click the scrollbar thumb → begin thumb-drag; `on_mouse_move` updates
  `scroll_px`; `on_mouse_up` ends it.
- Click outside the popup → `Closed(action)`, close. **The click is consumed**
  — it does not also activate whatever is under it (standard dropdown
  behavior; differs deliberately from `text_edit`'s pass-through
  click-outside-commit).
- `on_mouse_move` over an item row → sets `highlight`, emits
  `HighlightChanged`.
- `on_wheel` → scrolls; `scroll_px` clamped to
  `[0, content_height - viewport_h]`.

### Keyboard (popup open)

While `is_open()`, the editor routes all key events to `DropdownState` and
swallows key-ups from the host — the exact pattern CLAUDE.md documents for
right-click text entry.

- Up / Down → move `highlight` over the visible (filtered) items, clamped at
  the ends (no wrap), auto-scrolling to keep the highlight visible →
  `HighlightChanged`.
- PageUp / PageDown / Home / End → bulk highlight moves, clamped.
- Enter → `Selected(action, highlight)`, close. No-op when the filter has zero
  matches.
- Esc → `Closed(action)`, close, no selection.
- Char / Backspace → only when `filter_enabled`; edits `filter`, resets
  `scroll_px` to 0 and `highlight` to the first match. When the filter is
  disabled (the small-menu case) char input is ignored entirely — arrow keys
  only.

### Filtering

Case-insensitive substring match against the item label. Empty filter = all
items. Not fuzzy — predictable and cheap; fuzzy matching is a possible later
refinement.

### Caret blink

`tick()` toggles `caret_blink_phase` on the same 1000ms cadence as
`text_edit`, relevant only when the filter row is visible.

## Testing

Inline `#[cfg(test)]` in `dropdown.rs`, consistent with the crate's existing 29
widget tests.

- **Layout** (`dropdown_popup_layout`): downward placement; upward flip when no
  room below; horizontal clamp at the right window edge; scrollbar appears once
  match count exceeds `MAX_VISIBLE_ROWS`; filtered visible-row count; correct
  row→unfiltered-index mapping under an active filter.
- **State machine**: open/close; `open()` auto-closes a prior dropdown;
  highlight clamps at list ends (no wrap); Enter emits `Selected` with the
  correct unfiltered index; Enter is a no-op with zero matches; Esc emits
  `Closed`; click-outside closes and is consumed; wheel scroll clamps; filter
  edit resets scroll and highlight; char input ignored when filter disabled.
- **Filter matcher**: case-insensitivity; substring (not prefix-only); empty
  filter = all; no-match = empty.
- **Render smoke tests**: draw the trigger and the popup (filter-enabled and
  filter-disabled) into a `Pixmap`; assert non-panic and a sentinel pixel.

## File Changes

- New: `tiny-skia-widgets/src/dropdown.rs`.
- Edit: `tiny-skia-widgets/src/lib.rs` — `pub mod dropdown;` + `pub use
  dropdown::*;`.
