//! Lock-free audio→GUI mirror of the propagation wavefront.
//!
//! The audio thread publishes the lit-set once per process block; the editor
//! reads it each frame to draw the live wavefront. One `AtomicU32` per row
//! (bit `col` = cell lit) — 16 stores per publish, `Relaxed` ordering; a torn
//! read is sub-frame and visually irrelevant.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §3.1.

use crate::grid::{COLS, ROWS};
use crate::propagation::Wavefront;
use std::sync::atomic::{AtomicU32, Ordering};

/// The audio→GUI wavefront mirror: one `AtomicU32` per grid row, bit `col`
/// set when cell `(row, col)` is lit.
pub struct WavefrontDisplay {
    rows: [AtomicU32; ROWS],
}

impl WavefrontDisplay {
    /// A display with every cell dark.
    pub fn new() -> Self {
        Self {
            rows: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    /// Audio thread: publish the current wavefront. One `Relaxed` store per row.
    pub fn publish(&self, wf: &Wavefront) {
        for r in 0..ROWS {
            let mut word = 0u32;
            for c in 0..COLS {
                if wf.is_lit(r, c) {
                    word |= 1 << c;
                }
            }
            self.rows[r].store(word, Ordering::Relaxed);
        }
    }

    /// GUI thread: is cell `(row, col)` lit in the last published wavefront?
    pub fn is_lit(&self, row: usize, col: usize) -> bool {
        (self.rows[row].load(Ordering::Relaxed) >> col) & 1 != 0
    }
}

impl Default for WavefrontDisplay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_display_is_all_dark() {
        let d = WavefrontDisplay::new();
        for r in 0..ROWS {
            for c in 0..COLS {
                assert!(!d.is_lit(r, c));
            }
        }
    }

    #[test]
    fn publish_then_read_round_trips_the_wavefront() {
        let d = WavefrontDisplay::new();
        let mut wf = Wavefront::empty();
        wf.set(0, 0, true);
        wf.set(5, 17, true);
        wf.set(15, 31, true);
        d.publish(&wf);
        assert!(d.is_lit(0, 0));
        assert!(d.is_lit(5, 17));
        assert!(d.is_lit(15, 31));
        assert!(!d.is_lit(5, 18));
        assert!(!d.is_lit(8, 8));
    }

    #[test]
    fn publish_overwrites_the_previous_wavefront() {
        let d = WavefrontDisplay::new();
        let mut a = Wavefront::empty();
        a.set(3, 3, true);
        d.publish(&a);
        assert!(d.is_lit(3, 3));
        let b = Wavefront::empty();
        d.publish(&b);
        assert!(!d.is_lit(3, 3));
    }
}
