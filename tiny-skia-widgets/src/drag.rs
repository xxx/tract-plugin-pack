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
        self.hit_regions.push(HitRegion {
            x,
            y,
            w,
            h,
            action,
        });
    }

    // ── Queries ─────────────────────────────────────────────────────

    pub fn mouse_pos(&self) -> (f32, f32) {
        (self.mouse_x, self.mouse_y)
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
    }

    /// Update drag value during CursorMoved. Returns the new normalized value
    /// if a drag is active, or None if no drag is in progress.
    ///
    /// `shift`: whether the shift key is held (enables granular/fine mode).
    /// `current_norm`: the current normalized parameter value (for shift transitions).
    pub fn update_drag(&mut self, shift: bool, current_norm: f32) -> Option<f32> {
        self.active.as_ref()?;

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
            (self.granular_start_value + delta_y / 600.0 * 0.1).clamp(0.0, 1.0)
        } else {
            let delta_y = self.start_y - self.mouse_y;
            (self.start_value + delta_y / 600.0).clamp(0.0, 1.0)
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
}

impl<A: Clone + PartialEq> Default for DragState<A> {
    fn default() -> Self {
        Self::new()
    }
}
