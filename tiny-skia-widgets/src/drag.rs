//! Shared hit-testing and drag state for softbuffer-based nih-plug editors.
//!
//! Extracts the common mouse interaction patterns (vertical dial dragging,
//! shift-for-granular control, double-click-to-reset, hit region management)
//! used by every softbuffer editor in the workspace.

use std::time::Instant;

// ── Hit region ──────────────────────────────────────────────────────────

/// A rectangular hit region with a generic action tag.
#[derive(Clone)]
pub struct HitRegion<A: Clone> {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub action: A,
}

// ── Drag state ──────────────────────────────────────────────────────────

/// Shared mouse/drag state for softbuffer editors.
///
/// Generic over `A`, the plugin's hit-action type. Tracks mouse position,
/// active drag, shift-for-granular state, and double-click timing.
pub struct DragState<A: Clone + PartialEq> {
    hit_regions: Vec<HitRegion<A>>,
    active: Option<A>,
    start_y: f32,
    start_value: f32,
    last_shift: bool,
    granular_start_y: f32,
    granular_start_value: f32,
    mouse_x: f32,
    mouse_y: f32,
    /// Whether the cursor is currently inside the editor window. Starts
    /// `false` (before the first CursorMoved) and is cleared on CursorLeft.
    /// Use `mouse_in_window()` to query — this avoids phantom hover state
    /// when the window first opens or after the cursor leaves.
    mouse_in_window: bool,
    last_click_time: Instant,
    last_click_action: Option<A>,
}

impl<A: Clone + PartialEq> DragState<A> {
    pub fn new() -> Self {
        Self {
            hit_regions: Vec::new(),
            active: None,
            start_y: 0.0,
            start_value: 0.0,
            last_shift: false,
            granular_start_y: 0.0,
            granular_start_value: 0.0,
            mouse_x: 0.0,
            mouse_y: 0.0,
            mouse_in_window: false,
            last_click_time: Instant::now(),
            last_click_action: None,
        }
    }

    // ── Hit region management ───────────────────────────────────────

    /// Clear all hit regions (call at the start of each draw frame).
    pub fn clear_regions(&mut self) {
        self.hit_regions.clear();
    }

    /// Add a hit region.
    pub fn push_region(&mut self, x: f32, y: f32, w: f32, h: f32, action: A) {
        self.hit_regions.push(HitRegion { x, y, w, h, action });
    }

    // ── Queries ─────────────────────────────────────────────────────

    pub fn mouse_pos(&self) -> (f32, f32) {
        (self.mouse_x, self.mouse_y)
    }

    /// Returns `true` after a `CursorMoved`/`CursorEntered` event has been
    /// seen and before a matching `CursorLeft`. Always `false` before the
    /// first mouse event, which prevents spurious hover state when the
    /// editor window first opens with the cursor outside the window.
    pub fn mouse_in_window(&self) -> bool {
        self.mouse_in_window
    }

    /// Returns a slice of all current hit regions.
    pub fn regions(&self) -> &[HitRegion<A>] {
        &self.hit_regions
    }

    /// Returns the currently active drag action, if any.
    pub fn active_action(&self) -> Option<&A> {
        self.active.as_ref()
    }

    // ── Mouse event handlers ────────────────────────────────────────

    /// Update mouse position. Call on CursorMoved.
    pub fn set_mouse(&mut self, x: f32, y: f32) {
        self.mouse_x = x;
        self.mouse_y = y;
        self.mouse_in_window = true;
    }

    /// Mark the cursor as inside the window. Call on CursorEntered.
    /// Not strictly required — `set_mouse` also flips this flag — but
    /// useful on platforms that fire CursorEntered before the first move.
    pub fn on_cursor_entered(&mut self) {
        self.mouse_in_window = true;
    }

    /// Mark the cursor as outside the window. Call on CursorLeft. Does
    /// not move the cursor position, so latched positions remain readable
    /// for anything that needs them — consumers should gate visibility
    /// queries on `mouse_in_window()`.
    pub fn on_cursor_left(&mut self) {
        self.mouse_in_window = false;
    }

    /// Update drag value during CursorMoved. Returns the new normalized value
    /// if a drag is active, or None if no drag is in progress.
    ///
    /// `shift`: whether the shift key is held (enables granular/fine mode).
    /// `current_norm`: the current normalized parameter value (for shift transitions).
    pub fn update_drag(&mut self, shift: bool, current_norm: f32) -> Option<f32> {
        self.update_drag_with_sensitivity(shift, current_norm, 600.0)
    }

    /// Update a drag with a custom physical-pixel distance for the full range.
    /// Shift uses ten times that distance, preserving the value on mode changes.
    pub fn update_drag_with_sensitivity(
        &mut self,
        shift: bool,
        current_norm: f32,
        pixels_per_range: f32,
    ) -> Option<f32> {
        self.active.as_ref()?;
        let pixels_per_range = pixels_per_range.max(1.0);

        // Handle shift transitions: snapshot value when entering/leaving granular mode
        if shift && !self.last_shift {
            self.granular_start_y = self.mouse_y;
            self.granular_start_value = current_norm;
        } else if !shift && self.last_shift {
            self.start_y = self.mouse_y;
            self.start_value = current_norm;
        }

        let target = if shift {
            let delta_y = self.granular_start_y - self.mouse_y;
            (self.granular_start_value + delta_y / pixels_per_range * 0.1).clamp(0.0, 1.0)
        } else {
            let delta_y = self.start_y - self.mouse_y;
            (self.start_value + delta_y / pixels_per_range).clamp(0.0, 1.0)
        };

        self.last_shift = shift;
        Some(target)
    }

    /// Start a drag for the given action. Call when a draggable region is clicked.
    pub fn begin_drag(&mut self, action: A, current_norm: f32, shift: bool) {
        self.start_y = self.mouse_y;
        self.start_value = current_norm;
        self.granular_start_y = self.mouse_y;
        self.granular_start_value = current_norm;
        self.last_shift = shift;
        self.active = Some(action);
    }

    /// End the current drag. Returns the action that was being dragged, if any.
    pub fn end_drag(&mut self) -> Option<A> {
        self.active.take()
    }

    /// Hit-test at the current mouse position. Returns the first matching region.
    pub fn hit_test(&self) -> Option<&HitRegion<A>> {
        let mx = self.mouse_x;
        let my = self.mouse_y;
        self.hit_regions
            .iter()
            .find(|r| mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h)
    }

    /// Check if clicking the given action right now would be a double-click.
    /// Also records the click for future double-click detection.
    pub fn check_double_click(&mut self, action: &A) -> bool {
        let now = Instant::now();
        let is_double = now.duration_since(self.last_click_time).as_millis() < 400
            && self.last_click_action.as_ref() == Some(action);
        self.last_click_time = now;
        self.last_click_action = Some(action.clone());
        is_double
    }

    /// Cancel a pending double-click pair after an intervening interaction.
    pub fn clear_double_click(&mut self) {
        self.last_click_action = None;
    }
}

impl<A: Clone + PartialEq> Default for DragState<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_double_click_requires_a_new_pair_on_the_same_action() {
        let mut drag: DragState<u8> = DragState::new();
        assert!(!drag.check_double_click(&1));
        drag.clear_double_click();
        assert!(!drag.check_double_click(&1));
        assert!(drag.check_double_click(&1));
        assert!(!drag.check_double_click(&2));
    }

    #[test]
    fn custom_sensitivity_supports_fine_control_and_shift_transitions() {
        let mut drag: DragState<()> = DragState::new();
        drag.set_mouse(0.0, 400.0);
        drag.begin_drag((), 0.25, false);
        drag.set_mouse(0.0, 350.0);
        assert_eq!(
            drag.update_drag_with_sensitivity(false, 0.25, 200.0),
            Some(0.5)
        );
        assert_eq!(
            drag.update_drag_with_sensitivity(true, 0.5, 200.0),
            Some(0.5)
        );
        drag.set_mouse(0.0, 150.0);
        let fine = drag.update_drag_with_sensitivity(true, 0.5, 200.0).unwrap();
        assert!((fine - 0.6).abs() < 1e-6);
        assert_eq!(
            drag.update_drag_with_sensitivity(false, fine, 200.0),
            Some(fine)
        );
        drag.set_mouse(0.0, 50.0);
        assert_eq!(
            drag.update_drag_with_sensitivity(false, fine, 200.0),
            Some(1.0)
        );
        drag.end_drag();
        assert_eq!(drag.update_drag_with_sensitivity(false, 1.0, 200.0), None);
    }

    #[test]
    fn default_drag_retains_its_existing_sensitivity() {
        let mut drag: DragState<()> = DragState::new();
        drag.set_mouse(0.0, 400.0);
        drag.begin_drag((), 0.5, false);
        drag.set_mouse(0.0, 340.0);
        assert!((drag.update_drag(false, 0.5).unwrap() - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_mouse_in_window_starts_false() {
        let d: DragState<()> = DragState::new();
        assert!(
            !d.mouse_in_window(),
            "new DragState must not claim the mouse is in window"
        );
    }

    #[test]
    fn test_set_mouse_marks_in_window() {
        let mut d: DragState<()> = DragState::new();
        d.set_mouse(10.0, 20.0);
        assert!(d.mouse_in_window());
        assert_eq!(d.mouse_pos(), (10.0, 20.0));
    }

    #[test]
    fn test_cursor_left_clears_flag_without_moving_position() {
        let mut d: DragState<()> = DragState::new();
        d.set_mouse(42.0, 84.0);
        d.on_cursor_left();
        assert!(!d.mouse_in_window());
        // Position is latched for consumers that still want it, but the
        // in-window flag is authoritative for visibility.
        assert_eq!(d.mouse_pos(), (42.0, 84.0));
    }

    #[test]
    fn test_cursor_entered_re_flags() {
        let mut d: DragState<()> = DragState::new();
        d.on_cursor_left();
        d.on_cursor_entered();
        assert!(d.mouse_in_window());
    }
}
