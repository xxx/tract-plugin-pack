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
    /// Trigger widget rect captured at open time.
    anchor: (f32, f32, f32, f32),
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
        Some((
            popup_x + BORDER,
            popup_y + BORDER,
            popup_w - 2.0 * BORDER,
            filter_h,
        ))
    } else {
        None
    };

    let lv_x = popup_x + BORDER;
    let lv_y = popup_y + BORDER + filter_h;
    let lv_w = popup_w - 2.0 * BORDER;
    let lv_h = popup_h - 2.0 * BORDER - filter_h;
    let list_viewport = (lv_x, lv_y, lv_w, lv_h);

    let has_scrollbar = content_height > lv_h + 0.01;
    let row_w = if has_scrollbar {
        lv_w - SCROLLBAR_W
    } else {
        lv_w
    };

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

/// Truncate `text` with a trailing "…" so it fits within `max_w` at `size`.
/// Returns `text` unchanged when it already fits.
fn truncate_to_width(tr: &mut TextRenderer, text: &str, size: f32, max_w: f32) -> String {
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
    let border = if is_open {
        color_accent()
    } else {
        color_border()
    };
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
        let row_y = if is_open {
            cy + tri_h * 0.5 - i as f32
        } else {
            cy - tri_h * 0.5 + i as f32
        };
        draw_rect(pixmap, cx - half, row_y, half * 2.0, 1.0, color_muted());
    }
}

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
        let shown = truncate_to_width(text_renderer, label, text_size, (rw - 2.0 * pad).max(0.0));
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
            highlight,
            scroll_px: 0.0,
            filter: String::new(),
            filter_enabled,
            scrollbar_drag: None,
        });
        self.last_filter_change = Instant::now();
        // `scroll_highlight_into_view` needs an items slice only to size the
        // layout and locate the highlight's filtered position. The filter was
        // just cleared above, so an empty filter matches every item: filtered
        // position == unfiltered index, and item *text* is irrelevant — only
        // the slice *length* matters. A dummy slice of the right length is
        // therefore sufficient and correct. (If `open()` ever pre-seeds a
        // non-empty filter, this trick breaks — use the real items instead.)
        let dummy: Vec<&str> = vec![""; item_count];
        self.scroll_highlight_into_view(&dummy, window_size);
    }

    /// Close the dropdown.
    pub fn close(&mut self) {
        self.active = None;
    }

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
        // `active` is still `Some`: the early-returns above already proved it,
        // and nothing between them closed the dropdown.
        let active = self.active.as_mut().unwrap();
        if row_top < active.scroll_px {
            active.scroll_px = row_top;
        } else if row_bot > active.scroll_px + viewport_h {
            active.scroll_px = row_bot - viewport_h;
        }
    }

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

    /// Public char-input handler. Only effective when the filter is enabled.
    pub fn on_char(&mut self, c: char, items: &[&str]) -> Option<DropdownEvent<A>> {
        self.on_char_internal(Some(c), items)
    }

    /// Shared filter-edit logic. `Some(c)` appends; `None` is a backspace.
    /// Resets scroll to the top and the highlight to the first match.
    fn on_char_internal(&mut self, c: Option<char>, items: &[&str]) -> Option<DropdownEvent<A>> {
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
        let action = active.action;
        let first = filtered_indices(items, &active.filter).first().copied();
        if let Some(idx) = first {
            active.highlight = idx;
        }
        self.last_filter_change = Instant::now();
        first.map(|idx| DropdownEvent::HighlightChanged(action, idx))
    }

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
            let thumb_h = layout.scrollbar.map(|sb| sb.thumb.3).unwrap_or(lv.3);
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

    #[cfg(test)]
    fn highlight_for_test(&self) -> Option<usize> {
        self.active.as_ref().map(|a| a.highlight)
    }

    #[cfg(test)]
    fn set_filter_for_test(&mut self, f: &str) {
        if let Some(active) = self.active.as_mut() {
            active.filter = f.to_string();
        }
    }

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
}

impl<A: Copy + PartialEq> Default for DropdownState<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_font::test_font_data;
    use tiny_skia::Pixmap;

    fn px_alpha(pm: &Pixmap, x: u32, y: u32) -> u8 {
        pm.pixels()[(y * pm.width() + x) as usize].alpha()
    }

    #[test]
    fn draw_trigger_paints_something() {
        let mut pm = Pixmap::new(300, 80).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        draw_dropdown_trigger(
            &mut pm,
            &mut tr,
            (10.0, 10.0, 200.0, 28.0),
            "Sine.wt",
            false,
        );
        // The trigger background fills its rect — interior pixel must be painted.
        assert!(px_alpha(&pm, 100, 24) > 0, "trigger background not drawn");
    }

    #[test]
    fn draw_trigger_open_does_not_panic() {
        let mut pm = Pixmap::new(300, 80).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        draw_dropdown_trigger(&mut pm, &mut tr, (10.0, 10.0, 200.0, 28.0), "Sine.wt", true);
    }

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
        assert!(dropdown_popup_layout(&with, &items, WIN)
            .unwrap()
            .filter_rect
            .is_some());
        assert!(dropdown_popup_layout(&without, &items, WIN)
            .unwrap()
            .filter_rect
            .is_none());
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

    #[test]
    fn key_arrows_noop_when_no_matches() {
        let mut s: DropdownState<A> = DropdownState::new();
        s.open(A::Wavetable, ANCHOR, 4, 0, true, WIN);
        s.set_filter_for_test("zzz");
        let items = ["alpha", "bravo", "bravado", "bract"];
        // Every arrow/page/home/end key must be a no-op when nothing matches.
        for key in [
            DropdownKey::Down,
            DropdownKey::Up,
            DropdownKey::PageDown,
            DropdownKey::PageUp,
            DropdownKey::Home,
            DropdownKey::End,
        ] {
            assert_eq!(s.on_key(key, &items, WIN), None);
        }
        assert!(
            s.is_open(),
            "no-match navigation must not close the dropdown"
        );
    }

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

    #[test]
    fn mouse_down_on_item_selects_and_closes() {
        let s_items: Vec<&str> = vec!["a"; 5];
        let mut s = open_state(5, false);
        let l = dropdown_popup_layout(&s, &s_items, WIN).unwrap();
        let row = l.visible_rows[2];
        let (rx, ry, rw, rh) = row.rect;
        let ev = s.on_mouse_down(rx + rw * 0.5, ry + rh * 0.5, &s_items, WIN);
        assert_eq!(
            ev,
            Some(DropdownEvent::Selected(A::Wavetable, row.item_index))
        );
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
            Some(DropdownEvent::HighlightChanged(
                A::Wavetable,
                row.item_index
            ))
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
        assert!(
            s.scroll_for_test().unwrap() > 0.0,
            "thumb drag should scroll"
        );
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
}
