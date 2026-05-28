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

/// A snapshot of the three DAW-opaque persisted config structs.
///
/// `modulation` is boxed: `[TrackModulation; ROWS]` is ~138 KB
/// (`[MsegNode; 128]` x 4 MSEGs x 16 rows). Held inline, a `ConfigSnapshot`
/// move would push that whole array across the stack; release builds elide
/// the copy but debug builds do not, overflowing the editor thread's stack
/// the moment the editor snapshots for undo. Boxing keeps the bulk on the
/// heap so only a pointer moves.
#[derive(Clone, PartialEq, Debug)]
pub struct ConfigSnapshot {
    pub grid: Grid,
    pub effects: [TrackEffect; ROWS],
    pub modulation: Box<[TrackModulation; ROWS]>,
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
        // Clone the modulation array straight onto the heap. Collecting into a
        // Vec clones element by element into a heap buffer, then the
        // boxed-slice -> boxed-array conversion is a pointer cast (no realloc).
        // This avoids ever materialising the full ~138 KB array as a stack
        // temporary, which a debug build would not elide.
        let modulation: Box<[TrackModulation; ROWS]> = params
            .track_modulation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .try_into()
            .expect("track_modulation always has exactly ROWS entries");
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
        // Clone element-by-element into the live array rather than cloning the
        // whole ~138 KB array onto the stack first (see `capture`).
        params
            .track_modulation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from_slice(&self.modulation[..]);
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
