//! Bounded editor undo history, independent of plugin state.

use std::collections::VecDeque;

/// Upper bound on the undo stack. Pushing past this drops the oldest entry.
pub const UNDO_DEPTH: usize = 128;

/// A bounded linear undo/redo history of snapshots, plus a one-slot capture
/// window. Generic over the snapshot type so the stack logic is unit-testable
/// without constructing plugin state.
pub struct UndoHistory<S> {
    undo: VecDeque<S>,
    redo: Vec<S>,
    pending: Option<S>,
}

impl<S: PartialEq> UndoHistory<S> {
    /// An empty history.
    pub fn new() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            pending: None,
        }
    }

    /// Open a capture window holding the pre-edit snapshot `before`, returning
    /// `true`. If a capture is already open this is a no-op returning `false`
    /// — so a subordinate bracket (a keyboard or right-click handler firing
    /// inside a longer mouse gesture) can tell it did not open the window and
    /// must not close it.
    pub fn begin_capture(&mut self, before: S) -> bool {
        if self.pending.is_none() {
            self.pending = Some(before);
            true
        } else {
            false
        }
    }

    /// Close the capture window. If a capture is open and `after` differs from
    /// the captured pre-state, push the pre-state as an undo entry and clear
    /// the redo stack. Otherwise the pending capture is discarded.
    pub fn commit_capture(&mut self, after: &S) {
        if let Some(before) = self.pending.take()
            && before != *after
        {
            self.push_undo(before);
            self.redo.clear();
        }
    }

    /// Undo: record `current` on the redo stack and return the snapshot to
    /// restore, or `None` if the undo stack is empty.
    pub fn undo(&mut self, current: S) -> Option<S> {
        let snapshot = self.undo.pop_back()?;
        self.redo.push(current);
        Some(snapshot)
    }

    /// Redo: record `current` on the undo stack and return the snapshot to
    /// restore, or `None` if the redo stack is empty.
    pub fn redo(&mut self, current: S) -> Option<S> {
        let snapshot = self.redo.pop()?;
        self.push_undo(current);
        Some(snapshot)
    }

    /// Whether an undo is available.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether a redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Whether a capture window is currently open — a `begin_capture` not yet
    /// matched by `commit_capture`.
    pub fn is_capturing(&self) -> bool {
        self.pending.is_some()
    }

    /// Push onto the undo stack, dropping the oldest entry past `UNDO_DEPTH`.
    fn push_undo(&mut self, snapshot: S) {
        if self.undo.len() >= UNDO_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
    }
}

impl<S: PartialEq> Default for UndoHistory<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_with_a_change_pushes_an_undo_entry() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(1);
        h.commit_capture(&2);
        assert!(h.can_undo());
        assert_eq!(h.undo(2), Some(1));
    }

    #[test]
    fn commit_with_no_change_pushes_nothing() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(5);
        h.commit_capture(&5);
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(1);
        h.commit_capture(&2);
        // Current state is 2: undo returns the pre-state 1, stashes 2 to redo.
        assert_eq!(h.undo(2), Some(1));
        assert!(h.can_redo());
        // Current state is now 1: redo returns 2.
        assert_eq!(h.redo(1), Some(2));
        assert!(!h.can_redo());
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(1);
        h.commit_capture(&2);
        assert_eq!(h.undo(2), Some(1));
        assert!(h.can_redo());
        // A fresh committed edit while undone clears the redo stack.
        h.begin_capture(1);
        h.commit_capture(&9);
        assert!(!h.can_redo());
    }

    #[test]
    fn begin_capture_keeps_the_first_pre_state() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        assert!(h.begin_capture(10), "first begin_capture opens the window");
        assert!(!h.begin_capture(20), "a second begin_capture does not open");
        h.commit_capture(&30);
        assert_eq!(h.undo(30), Some(10));
    }

    #[test]
    fn is_capturing_tracks_the_open_window() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        assert!(!h.is_capturing());
        h.begin_capture(1);
        assert!(h.is_capturing());
        h.commit_capture(&2);
        assert!(!h.is_capturing());
    }

    #[test]
    fn undo_and_redo_are_none_on_empty_stacks() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        assert_eq!(h.undo(0), None);
        assert_eq!(h.redo(0), None);
    }

    #[test]
    fn the_depth_bound_drops_the_oldest_entry() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        // Push UNDO_DEPTH + 1 distinct entries (pre-states 0..=UNDO_DEPTH).
        for i in 0..=UNDO_DEPTH as i32 {
            h.begin_capture(i);
            h.commit_capture(&(i + 1000)); // always a change
        }
        // The stack holds exactly UNDO_DEPTH entries; the oldest (pre-state 0)
        // was dropped, so the deepest retained pre-state is 1.
        let mut count = 0;
        let mut last = None;
        while let Some(s) = h.undo(0) {
            last = Some(s);
            count += 1;
        }
        assert_eq!(count, UNDO_DEPTH);
        assert_eq!(last, Some(1));
    }
}
