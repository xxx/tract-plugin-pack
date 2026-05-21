//! Lock-free audio→GUI mirror of the sequencer playhead column.
//!
//! The audio thread publishes the playhead's column once per process block;
//! the editor reads it each frame to draw the column highlight. One
//! `AtomicU32`, `Relaxed` ordering — a torn read is sub-frame and visually
//! irrelevant.

use std::sync::atomic::{AtomicU32, Ordering};

/// The audio→GUI playhead-column mirror.
pub struct PlayheadDisplay {
    column: AtomicU32,
}

impl PlayheadDisplay {
    /// A display with the playhead at column 0.
    pub fn new() -> Self {
        Self { column: AtomicU32::new(0) }
    }

    /// Audio thread: publish the current playhead column.
    pub fn publish(&self, column: usize) {
        self.column.store(column as u32, Ordering::Relaxed);
    }

    /// GUI thread: the last published playhead column.
    pub fn column(&self) -> usize {
        self.column.load(Ordering::Relaxed) as usize
    }
}

impl Default for PlayheadDisplay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_display_reads_column_zero() {
        assert_eq!(PlayheadDisplay::new().column(), 0);
    }

    #[test]
    fn publish_round_trips_the_column() {
        let d = PlayheadDisplay::new();
        d.publish(17);
        assert_eq!(d.column(), 17);
        d.publish(3);
        assert_eq!(d.column(), 3);
    }
}
