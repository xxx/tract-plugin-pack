//! The wavefront propagation engine: the lit-set, the per-step routing rule,
//! and the Initial/Running/Stopped lifecycle.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §5.

use crate::grid::{Direction, Grid, CELL_COUNT, COLS, ROWS};

/// The set of currently-lit cells. `Copy` so it crosses the GUI/audio
/// boundary cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wavefront {
    lit: [bool; CELL_COUNT],
}

impl Wavefront {
    /// An empty wavefront — no cells lit.
    pub fn empty() -> Self {
        Self {
            lit: [false; CELL_COUNT],
        }
    }

    /// Is the cell at `(row, col)` lit?
    pub fn is_lit(&self, row: usize, col: usize) -> bool {
        self.lit[Grid::index(row, col)]
    }

    /// Light or clear the cell at `(row, col)`.
    pub fn set(&mut self, row: usize, col: usize, on: bool) {
        self.lit[Grid::index(row, col)] = on;
    }

    /// Are no cells lit?
    pub fn is_empty(&self) -> bool {
        !self.lit.iter().any(|&l| l)
    }

    /// How many cells are lit.
    pub fn count(&self) -> usize {
        self.lit.iter().filter(|&&l| l).count()
    }
}

impl Default for Wavefront {
    fn default() -> Self {
        Self::empty()
    }
}

/// Compute the next wavefront from `current` by following every lit cell's
/// send directions through `grid.next_cell`. Pure — this is the seam a future
/// Game-of-Life routing mode plugs into (see the design doc §1.3).
pub fn step_manual(grid: &Grid, current: &Wavefront) -> Wavefront {
    let mut next = Wavefront::empty();
    for r in 0..ROWS {
        for c in 0..COLS {
            if !current.is_lit(r, c) {
                continue;
            }
            let cell = *grid.cell(r, c);
            for dir in Direction::ALL {
                if cell.sends_to(dir) {
                    let (nr, nc) = grid.next_cell(r, c, dir);
                    next.set(nr, nc, true);
                }
            }
        }
    }
    next
}

/// Lifecycle state of the sequence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SequenceState {
    /// Fresh or reset — the next tick arms the start cells.
    Initial,
    /// A non-empty wavefront is propagating.
    Running,
    /// Kept for compatibility — unreachable in normal operation.
    Stopped,
}

/// Drives the wavefront through its Initial/Running/Stopped lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Propagator {
    pub state: SequenceState,
    pub wavefront: Wavefront,
    /// Steps since the wavefront was last armed — for display.
    pub step: u64,
}

impl Propagator {
    /// A fresh propagator in the `Initial` state.
    pub fn new() -> Self {
        Self {
            state: SequenceState::Initial,
            wavefront: Wavefront::empty(),
            step: 0,
        }
    }

    /// Clear the wavefront and return to `Initial`. Triggered by the manual
    /// Reset button and the host transport's stopped→playing edge.
    pub fn reset(&mut self) {
        self.state = SequenceState::Initial;
        self.wavefront = Wavefront::empty();
        self.step = 0;
    }

    /// Advance the sequence one step. A dead end (every lit cell routed
    /// nowhere) restarts the sequence.
    pub fn tick(&mut self, grid: &Grid) {
        match self.state {
            SequenceState::Initial => {
                let mut wf = Wavefront::empty();
                for r in 0..ROWS {
                    for c in 0..COLS {
                        if grid.cell(r, c).is_start {
                            wf.set(r, c, true);
                        }
                    }
                }
                self.wavefront = wf;
                self.step = 0;
                if !wf.is_empty() {
                    self.state = SequenceState::Running;
                }
                // No start cells → stay Initial with an empty wavefront.
            }
            SequenceState::Running => {
                let next = step_manual(grid, &self.wavefront);
                if next.is_empty() {
                    self.wavefront = Wavefront::empty();
                    self.state = SequenceState::Initial;
                    self.step = 0;
                } else {
                    self.wavefront = next;
                    self.step += 1;
                }
            }
            SequenceState::Stopped => {}
        }
    }
}

impl Default for Propagator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavefront_empty_has_nothing_lit() {
        let wf = Wavefront::empty();
        assert!(wf.is_empty());
        assert_eq!(wf.count(), 0);
        assert!(!wf.is_lit(0, 0));
    }

    #[test]
    fn wavefront_set_and_read_back() {
        let mut wf = Wavefront::empty();
        wf.set(3, 7, true);
        wf.set(10, 0, true);
        assert!(wf.is_lit(3, 7));
        assert!(wf.is_lit(10, 0));
        assert!(!wf.is_lit(3, 8));
        assert!(!wf.is_empty());
        assert_eq!(wf.count(), 2);

        wf.set(3, 7, false);
        assert!(!wf.is_lit(3, 7));
        assert_eq!(wf.count(), 1);
    }

    use crate::grid::{Cell, LoopRegion};

    /// A grid where every cell sends nowhere — a blank slate for tests.
    fn blank_grid() -> Grid {
        let mut g = Grid::default_routing();
        for cell in g.cells.iter_mut() {
            *cell = Cell::default(); // enabled, not start, no sends
        }
        g.loop_region = LoopRegion::full();
        g
    }

    /// A grid with exactly one start cell at `(row, col)` and no sends — it
    /// dead-ends one tick after arming.
    fn one_start_no_sends(row: usize, col: usize) -> Grid {
        let mut g = blank_grid();
        g.cell_mut(row, col).is_start = true;
        g
    }

    #[test]
    fn propagator_new_is_initial_and_empty() {
        let p = Propagator::new();
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
        assert_eq!(p.step, 0);
    }

    #[test]
    fn tick_from_initial_arms_every_start_cell() {
        let g = Grid::default_routing(); // left column = 16 start cells
        let mut p = Propagator::new();
        p.tick(&g);
        assert_eq!(p.state, SequenceState::Running);
        assert_eq!(p.wavefront.count(), ROWS);
        for r in 0..ROWS {
            assert!(p.wavefront.is_lit(r, 0));
        }
    }

    #[test]
    fn tick_from_initial_with_no_start_cells_stays_initial() {
        let g = blank_grid(); // nothing is a start cell
        let mut p = Propagator::new();
        p.tick(&g);
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
    }

    #[test]
    fn tick_running_propagates_the_wavefront() {
        let g = Grid::default_routing(); // every cell sends East
        let mut p = Propagator::new();
        p.tick(&g); // arm: column 0
        p.tick(&g); // propagate East: column 1
        assert_eq!(p.state, SequenceState::Running);
        assert_eq!(p.step, 1);
        for r in 0..ROWS {
            assert!(p.wavefront.is_lit(r, 1));
            assert!(!p.wavefront.is_lit(r, 0));
        }
    }

    #[test]
    fn dead_end_always_returns_to_initial() {
        let g = one_start_no_sends(0, 0);
        let mut p = Propagator::new();
        p.tick(&g); // arm (0,0) -> Running
        assert_eq!(p.state, SequenceState::Running);
        p.tick(&g); // (0,0) sends nowhere -> dead end -> restart
        assert_eq!(p.state, SequenceState::Initial);
        assert!(p.wavefront.is_empty());
        // The next tick re-arms the start cells.
        p.tick(&g);
        assert_eq!(p.state, SequenceState::Running);
        assert!(p.wavefront.is_lit(0, 0));
    }

    #[test]
    fn propagator_loops_forever_in_a_1x1_self_routing_region() {
        // A 1×1 loop region whose single cell is a start and sends East: East
        // wraps onto itself, so the wavefront never dead-ends.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 5,
            row1: 5,
            col0: 6,
            col1: 6,
        };
        g.cell_mut(5, 6).is_start = true;
        g.cell_mut(5, 6).set_send(Direction::E, true);
        let mut p = Propagator::new();
        p.tick(&g); // arm
        for _ in 0..50 {
            p.tick(&g);
            assert_eq!(p.state, SequenceState::Running);
            assert_eq!(p.wavefront.count(), 1);
            assert!(p.wavefront.is_lit(5, 6));
        }
    }

    #[test]
    fn step_manual_follows_a_single_send() {
        let mut g = blank_grid();
        g.cell_mut(5, 5).set_send(Direction::E, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1);
        assert!(next.is_lit(5, 6));
    }

    #[test]
    fn step_manual_splits_the_wavefront() {
        let mut g = blank_grid();
        g.cell_mut(5, 5).set_send(Direction::E, true);
        g.cell_mut(5, 5).set_send(Direction::S, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 2);
        assert!(next.is_lit(5, 6));
        assert!(next.is_lit(6, 5));
    }

    #[test]
    fn step_manual_merges_two_cells_onto_one() {
        let mut g = blank_grid();
        // (5,5) sends E and (5,7) sends W — both land on (5,6).
        g.cell_mut(5, 5).set_send(Direction::E, true);
        g.cell_mut(5, 7).set_send(Direction::W, true);
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        wf.set(5, 7, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1, "merge collapses onto one cell");
        assert!(next.is_lit(5, 6));
    }

    #[test]
    fn step_manual_with_no_sends_yields_empty() {
        let g = blank_grid(); // no cell sends anywhere
        let mut wf = Wavefront::empty();
        wf.set(5, 5, true);
        assert!(step_manual(&g, &wf).is_empty());
    }

    #[test]
    fn step_manual_on_empty_input_is_empty() {
        let g = Grid::default_routing();
        assert!(step_manual(&g, &Wavefront::empty()).is_empty());
    }

    #[test]
    fn step_manual_loops_within_a_one_cell_region() {
        // A 1×1 loop region: a cell sending E wraps onto itself.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 4,
            col0: 9,
            col1: 9,
        };
        g.cell_mut(4, 9).set_send(Direction::E, true);
        let mut wf = Wavefront::empty();
        wf.set(4, 9, true);
        let next = step_manual(&g, &wf);
        assert!(next.is_lit(4, 9), "1×1 region send loops back onto itself");
        assert_eq!(next.count(), 1);
    }

    #[test]
    fn step_manual_1x1_region_collapses_every_send_onto_self() {
        // In a 1×1 region a cell sending in all 8 directions still lights only
        // itself — every send wraps back onto the one cell.
        let mut g = blank_grid();
        g.loop_region = LoopRegion {
            row0: 8,
            row1: 8,
            col0: 2,
            col1: 2,
        };
        for dir in Direction::ALL {
            g.cell_mut(8, 2).set_send(dir, true);
        }
        let mut wf = Wavefront::empty();
        wf.set(8, 2, true);
        let next = step_manual(&g, &wf);
        assert_eq!(next.count(), 1);
        assert!(next.is_lit(8, 2));
    }
}
