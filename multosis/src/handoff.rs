//! Lock-free-ish GUI→audio handoff of the routing `Grid`.
//!
//! Mirrors miff's `KernelHandoff`: the GUI thread publishes with a blocking
//! lock; the audio thread reads with a non-blocking `try_lock` and keeps its
//! previous copy on contention. `Grid` is `Copy` (~1.5 KB), so a read is an
//! allocation-free stack copy.

use crate::grid::Grid;
use std::sync::Mutex;

/// Carries the latest `Grid` from the GUI thread to the audio thread.
pub struct GridHandoff {
    shared: Mutex<Grid>,
}

impl GridHandoff {
    /// A handoff seeded with `grid`.
    pub fn new(grid: Grid) -> Self {
        Self {
            shared: Mutex::new(grid),
        }
    }

    /// GUI thread: publish a new grid. Blocks briefly on the lock.
    pub fn publish(&self, grid: Grid) {
        if let Ok(mut slot) = self.shared.lock() {
            *slot = grid;
        }
    }

    /// Audio thread: read the latest grid without blocking. Returns `None` on
    /// lock contention — the caller keeps its previous copy.
    pub fn try_read(&self) -> Option<Grid> {
        self.shared.try_lock().ok().map(|slot| *slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_read_returns_the_initial_grid() {
        let handoff = GridHandoff::new(Grid::default());
        let g = handoff.try_read().expect("uncontended read");
        assert_eq!(g, Grid::default());
    }

    #[test]
    fn publish_then_read_sees_the_new_grid() {
        let handoff = GridHandoff::new(Grid::default());
        let mut edited = Grid::default();
        edited.cell_mut(4, 4).enabled = false;
        handoff.publish(edited);
        let g = handoff.try_read().expect("uncontended read");
        assert!(!g.cell(4, 4).enabled);
    }
}
