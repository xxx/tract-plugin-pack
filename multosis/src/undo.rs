//! Editor-side undo/redo for the DAW-opaque persisted config.
//!
//! The host's own undo covers the automatable parameters; it cannot see into
//! the `grid` / `track_effects` / `track_modulation` persisted blobs. This
//! module captures snapshots of those three structs and restores them.
//! See `docs/superpowers/specs/2026-05-22-multosis-undo-redo-design.md`.

use std::sync::PoisonError;

use crate::MultosisParams;
use crate::effects::TrackEffect;
use crate::grid::{Grid, ROWS};
use crate::modulation::TrackModulation;

pub use tract_effects::undo::{UNDO_DEPTH, UndoHistory};

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
