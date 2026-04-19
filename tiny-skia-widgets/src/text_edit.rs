//! Shared numeric text-entry state for softbuffer-based nih-plug editors.
//!
//! Mirrors the role of `DragState<A>`: a single in-flight edit, driven by
//! right-click + keyboard events in the host editor. The widget draw
//! functions (`draw_dial`, `draw_slider`) query `active_for(&A)` to decide
//! whether to render a buffer + caret in place of the formatted value.

use std::time::Instant;

/// Shared numeric text-entry state. One in-flight edit at most, tagged by
/// the same action type used for `DragState` hit regions.
pub struct TextEditState<A: Clone + PartialEq> {
    active: Option<A>,
    buffer: String,
    started_at: Instant,
}

/// Maximum buffer length (defensive cap — typed values are short).
const MAX_BUFFER_LEN: usize = 16;

impl<A: Clone + PartialEq> TextEditState<A> {
    pub fn new() -> Self {
        Self {
            active: None,
            buffer: String::new(),
            started_at: Instant::now(),
        }
    }

    /// Open an edit on `action` with `initial` as the starting buffer.
    /// Replaces any in-flight edit (the editor is expected to have called
    /// `commit()` first if it wanted to preserve the previous value).
    pub fn begin(&mut self, action: A, initial: &str) {
        self.active = Some(action);
        self.buffer.clear();
        self.buffer.push_str(initial);
        if self.buffer.len() > MAX_BUFFER_LEN {
            self.buffer.truncate(MAX_BUFFER_LEN);
        }
        self.started_at = Instant::now();
    }

    /// Returns `Some(&buffer)` iff `action` matches the currently active edit.
    pub fn active_for(&self, action: &A) -> Option<&str> {
        match &self.active {
            Some(a) if a == action => Some(&self.buffer),
            _ => None,
        }
    }

    /// Discard the in-flight edit without committing.
    pub fn cancel(&mut self) {
        self.active = None;
        self.buffer.clear();
    }
}

impl<A: Clone + PartialEq> Default for TextEditState<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    enum A {
        Gain,
        Freq,
    }

    #[test]
    fn new_reports_no_active_edit() {
        let s: TextEditState<A> = TextEditState::new();
        assert!(s.active_for(&A::Gain).is_none());
        assert!(s.active_for(&A::Freq).is_none());
    }

    #[test]
    fn active_for_other_action_is_none() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        assert_eq!(s.active_for(&A::Gain), Some("-6.0"));
        assert!(s.active_for(&A::Freq).is_none());
    }

    #[test]
    fn cancel_clears_state() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.cancel();
        assert!(s.active_for(&A::Gain).is_none());
    }

    #[test]
    fn begin_replaces_active_edit() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.begin(A::Freq, "440");
        assert!(s.active_for(&A::Gain).is_none());
        assert_eq!(s.active_for(&A::Freq), Some("440"));
    }
}
