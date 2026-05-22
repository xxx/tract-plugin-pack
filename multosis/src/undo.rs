//! Editor-side undo/redo for the DAW-opaque persisted config.
//!
//! The host's own undo covers the automatable parameters; it cannot see into
//! the `grid` / `track_effects` / `track_modulation` persisted blobs. This
//! module captures snapshots of those three structs and restores them.
//! See `docs/superpowers/specs/2026-05-22-multosis-undo-redo-design.md`.

use std::collections::VecDeque;
use std::sync::PoisonError;

use crate::effects::TrackEffect;
use crate::grid::{Grid, ROWS};
use crate::modulation::TrackModulation;
use crate::MultosisParams;

/// Upper bound on the undo stack. Pushing past this drops the oldest entry.
pub const UNDO_DEPTH: usize = 128;

/// A bounded linear undo/redo history of snapshots, plus a one-slot capture
/// window. Generic over the snapshot type so the stack logic is unit-testable
/// without constructing a full [`ConfigSnapshot`].
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

    /// Open a capture window holding the pre-edit snapshot `before`. A no-op
    /// if a capture is already open, so the earliest pre-state is the one
    /// kept.
    pub fn begin_capture(&mut self, before: S) {
        if self.pending.is_none() {
            self.pending = Some(before);
        }
    }

    /// Close the capture window. If a capture is open and `after` differs from
    /// the captured pre-state, push the pre-state as an undo entry and clear
    /// the redo stack. Otherwise the pending capture is discarded.
    pub fn commit_capture(&mut self, after: &S) {
        if let Some(before) = self.pending.take() {
            if before != *after {
                self.push_undo(before);
                self.redo.clear();
            }
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

/// A snapshot of the three DAW-opaque persisted config structs.
#[derive(Clone, PartialEq, Debug)]
pub struct ConfigSnapshot {
    pub grid: Grid,
    pub effects: [TrackEffect; ROWS],
    pub modulation: [TrackModulation; ROWS],
}

impl ConfigSnapshot {
    /// Clone the current `grid` / `track_effects` / `track_modulation` out of
    /// `params`. A poisoned mutex is recovered with `into_inner`, so a
    /// snapshot is always faithful (the editor is the only GUI writer and
    /// holds each lock only briefly).
    pub fn capture(params: &MultosisParams) -> Self {
        let grid = *params.grid.lock().unwrap_or_else(PoisonError::into_inner);
        let effects = *params
            .track_effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let modulation = params
            .track_modulation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Self {
            grid,
            effects,
            modulation,
        }
    }

    /// Write this snapshot's three structs back into `params`.
    pub fn restore(&self, params: &MultosisParams) {
        *params.grid.lock().unwrap_or_else(PoisonError::into_inner) = self.grid;
        *params
            .track_effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.effects;
        *params
            .track_modulation
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.modulation.clone();
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
        h.begin_capture(10);
        h.begin_capture(20); // ignored — a capture is already open
        h.commit_capture(&30);
        assert_eq!(h.undo(30), Some(10));
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

    #[test]
    fn config_snapshot_round_trips_a_grid_edit() {
        let params = MultosisParams::default();
        let original = ConfigSnapshot::capture(&params);
        // Flip a grid cell.
        params.grid.lock().unwrap().cell_mut(3, 5).enabled ^= true;
        let mutated = ConfigSnapshot::capture(&params);
        assert_ne!(original, mutated, "the grid edit changed the snapshot");
        // Restoring the original snapshot reverts the edit.
        original.restore(&params);
        assert_eq!(ConfigSnapshot::capture(&params), original);
    }

    #[test]
    fn config_snapshot_round_trips_effect_and_modulation_edits() {
        let params = MultosisParams::default();
        let original = ConfigSnapshot::capture(&params);
        params.track_effects.lock().unwrap()[2].mix = 0.25;
        params.track_modulation.lock().unwrap()[7].depths[0] = -0.5;
        let mutated = ConfigSnapshot::capture(&params);
        assert_ne!(original, mutated);
        original.restore(&params);
        assert_eq!(ConfigSnapshot::capture(&params), original);
    }
}
