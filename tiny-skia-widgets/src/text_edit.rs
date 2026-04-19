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

/// Check if a character is valid for numeric entry: 0-9, . - + e E
fn is_numeric_char(c: char) -> bool {
    matches!(c, '0'..='9' | '.' | '-' | '+' | 'e' | 'E')
}

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
            // Truncate on the nearest char boundary at or below MAX_BUFFER_LEN.
            // Callers pass ASCII numeric strings, but this keeps `begin` panic-safe
            // if that ever changes (insert_char filters non-numeric in Task 2).
            let end = (0..=MAX_BUFFER_LEN)
                .rev()
                .find(|&i| self.buffer.is_char_boundary(i))
                .unwrap_or(0);
            self.buffer.truncate(end);
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

    /// Insert a character at the end of the buffer if there's an active edit.
    /// Only accepts numeric characters (0-9 . - + e E). Respects the 16-char cap.
    /// No-op if no edit is active.
    pub fn insert_char(&mut self, c: char) {
        if self.active.is_none() {
            return;
        }
        if !is_numeric_char(c) {
            return;
        }
        if self.buffer.len() < MAX_BUFFER_LEN {
            self.buffer.push(c);
        }
    }

    /// Remove the last character from the buffer if there's an active edit
    /// and the buffer is not empty. No-op otherwise.
    pub fn backspace(&mut self) {
        if self.active.is_none() {
            return;
        }
        self.buffer.pop();
    }

    /// Finalize the edit and return the action and buffer contents.
    /// Returns `None` if no edit is active. Clears the state after returning.
    pub fn commit(&mut self) -> Option<(A, String)> {
        let action = self.active.take()?;
        let buffer = std::mem::take(&mut self.buffer);
        Some((action, buffer))
    }

    /// Check if an edit is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// `true` during the "on" half of the 1000 ms blink cycle. Returns
    /// `false` when no edit is active.
    pub fn caret_visible(&self) -> bool {
        if self.active.is_none() {
            return false;
        }
        let elapsed_ms = self.started_at.elapsed().as_millis();
        (elapsed_ms % 1000) < 500
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

    #[test]
    fn begin_on_same_action_replaces_buffer() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.begin(A::Gain, "-12.0");
        assert_eq!(s.active_for(&A::Gain), Some("-12.0"));
    }

    #[test]
    fn insert_char_accepts_digits() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        s.insert_char('5');
        assert_eq!(s.active_for(&A::Gain), Some("5"));
    }

    #[test]
    fn insert_char_accepts_numeric_chars() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6");
        s.insert_char('.');
        s.insert_char('5');
        s.insert_char('+');
        assert_eq!(s.active_for(&A::Gain), Some("-6.5+"));
    }

    #[test]
    fn insert_char_rejects_non_numeric() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "10");
        s.insert_char('a');
        s.insert_char(' ');
        assert_eq!(s.active_for(&A::Gain), Some("10"));
    }

    #[test]
    fn insert_char_respects_max_buffer_len() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        for _ in 0..20 {
            s.insert_char('5');
        }
        let buf = s.active_for(&A::Gain).unwrap();
        assert_eq!(buf.len(), MAX_BUFFER_LEN);
        assert_eq!(buf, "5555555555555555");
    }

    #[test]
    fn insert_char_noop_when_inactive() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.insert_char('5');
        assert!(s.active_for(&A::Gain).is_none());
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "123");
        s.backspace();
        assert_eq!(s.active_for(&A::Gain), Some("12"));
    }

    #[test]
    fn backspace_on_empty_buffer_is_noop() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        s.backspace();
        assert_eq!(s.active_for(&A::Gain), Some(""));
    }

    #[test]
    fn backspace_when_inactive_is_noop() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.backspace();
        assert!(s.active_for(&A::Gain).is_none());
    }

    #[test]
    fn commit_returns_action_and_buffer() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        s.insert_char('5');
        let (action, buffer) = s.commit().unwrap();
        assert_eq!(action, A::Gain);
        assert_eq!(buffer, "5");
    }

    #[test]
    fn commit_clears_state() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.commit();
        assert!(s.active_for(&A::Gain).is_none());
        assert!(s.commit().is_none());
    }

    #[test]
    fn is_active_matches_state() {
        let mut s: TextEditState<A> = TextEditState::new();
        assert!(!s.is_active());
        s.begin(A::Gain, "10");
        assert!(s.is_active());
        s.cancel();
        assert!(!s.is_active());
    }

    #[test]
    fn caret_visible_false_when_inactive() {
        let s: TextEditState<A> = TextEditState::new();
        assert!(!s.caret_visible());
    }

    #[test]
    fn caret_visible_flips_after_half_second() {
        use std::thread;
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        thread::sleep(std::time::Duration::from_millis(550));
        assert!(!s.caret_visible(), "hidden ~550ms in (second half of 1000ms cycle)");
        thread::sleep(std::time::Duration::from_millis(500));
        assert!(s.caret_visible(), "visible again ~1050ms in (start of next cycle)");
    }

    #[test]
    fn caret_visible_resets_on_begin() {
        use std::thread;
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "10");
        thread::sleep(std::time::Duration::from_millis(550));
        assert!(!s.caret_visible());
        s.begin(A::Freq, "440");
        assert!(s.caret_visible());
    }
}
