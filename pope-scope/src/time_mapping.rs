//! Atomic time mapping for beat sync.
//!
//! Maps between PPQ (Pulses Per Quarter note) positions and absolute
//! sample positions. Used by the audio thread to tag samples with
//! musical time, and by the GUI thread to extract beat-aligned windows.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

/// Atomic time mapping state. Written by audio thread, read by GUI thread.
pub struct TimeMapping {
    /// Current PPQ position (f64 bit-cast to u64).
    current_ppq: AtomicU64,
    /// Absolute sample position corresponding to `current_ppq` (DAW transport coordinates).
    current_sample_pos: AtomicI64,
    /// Ring buffer write_pos at the time of current_sample_pos.
    /// Used to convert DAW transport positions to ring buffer positions.
    ring_buffer_pos: AtomicU64,
    /// Samples per beat (f64 bit-cast to u64). Derived from BPM + sample rate.
    samples_per_beat: AtomicU64,
    /// Incremented on transport discontinuities (loop, seek, play start).
    discontinuity_counter: AtomicU64,
    /// Last PPQ written, for discontinuity detection (f64 bit-cast).
    last_ppq: AtomicU64,
    /// Whether transport was playing on the previous buffer.
    was_playing: AtomicU32,
}

/// Non-atomic snapshot of time mapping for GUI reads.
#[derive(Clone, Copy, Debug)]
pub struct TimeMappingSnapshot {
    pub current_ppq: f64,
    pub current_sample_pos: i64,
    /// Ring buffer write_pos at the time of current_sample_pos.
    pub ring_buffer_pos: u64,
    pub samples_per_beat: f64,
    pub discontinuity_counter: u64,
}

#[allow(clippy::new_without_default)]
impl TimeMapping {
    pub const fn new() -> Self {
        Self {
            current_ppq: AtomicU64::new(0),
            current_sample_pos: AtomicI64::new(0),
            ring_buffer_pos: AtomicU64::new(0),
            samples_per_beat: AtomicU64::new(0),
            discontinuity_counter: AtomicU64::new(0),
            last_ppq: AtomicU64::new(0),
            was_playing: AtomicU32::new(0),
        }
    }

    /// Update time mapping from audio thread. Call BEFORE pushing audio.
    ///
    /// - `ppq`: current PPQ position from DAW playhead
    /// - `sample_pos`: absolute sample position at buffer start (DAW transport coordinates)
    /// - `ring_buf_pos`: ring buffer write_pos at this moment (ring buffer coordinates)
    /// - `bpm`: current tempo
    /// - `sample_rate`: current sample rate
    /// - `buffer_size`: number of samples in this buffer
    /// - `is_playing`: whether transport is playing
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        ppq: f64,
        sample_pos: i64,
        ring_buf_pos: u64,
        bpm: f64,
        sample_rate: f64,
        buffer_size: usize,
        is_playing: bool,
    ) {
        let was_playing = self.was_playing.load(Ordering::Relaxed) != 0;

        if is_playing {
            let spb = (60.0 / bpm) * sample_rate;
            self.samples_per_beat
                .store(spb.to_bits(), Ordering::Relaxed);

            // Detect discontinuity: play start, or PPQ jumped unexpectedly
            if !was_playing {
                // Transport just started
                self.discontinuity_counter.fetch_add(1, Ordering::Relaxed);
            } else {
                let last = f64::from_bits(self.last_ppq.load(Ordering::Relaxed));
                let expected_advance = buffer_size as f64 / spb;
                let actual_advance = ppq - last;
                // If PPQ jumped by more than 2x expected, it's a discontinuity
                if actual_advance < -0.01 || actual_advance > expected_advance * 2.0 + 0.5 {
                    self.discontinuity_counter.fetch_add(1, Ordering::Relaxed);
                }
            }

            self.current_ppq.store(ppq.to_bits(), Ordering::Relaxed);
            self.current_sample_pos.store(sample_pos, Ordering::Relaxed);
            self.ring_buffer_pos
                .store(ring_buf_pos, Ordering::Relaxed);
            self.last_ppq.store(ppq.to_bits(), Ordering::Relaxed);
        }

        self.was_playing
            .store(if is_playing { 1 } else { 0 }, Ordering::Relaxed);
    }

    /// Read a snapshot (GUI thread).
    pub fn snapshot(&self) -> TimeMappingSnapshot {
        TimeMappingSnapshot {
            current_ppq: f64::from_bits(self.current_ppq.load(Ordering::Relaxed)),
            current_sample_pos: self.current_sample_pos.load(Ordering::Relaxed),
            ring_buffer_pos: self.ring_buffer_pos.load(Ordering::Relaxed),
            samples_per_beat: f64::from_bits(self.samples_per_beat.load(Ordering::Relaxed)),
            discontinuity_counter: self.discontinuity_counter.load(Ordering::Relaxed),
        }
    }

    /// Reset all fields (used in tests).
    #[cfg(test)]
    pub fn reset(&self) {
        self.current_ppq.store(0, Ordering::Relaxed);
        self.current_sample_pos.store(0, Ordering::Relaxed);
        self.ring_buffer_pos.store(0, Ordering::Relaxed);
        self.samples_per_beat.store(0, Ordering::Relaxed);
        self.discontinuity_counter.store(0, Ordering::Relaxed);
        self.last_ppq.store(0, Ordering::Relaxed);
        self.was_playing.store(0, Ordering::Relaxed);
    }
}

/// Compute a beat-aligned window using PPQ deltas relative to ring_buffer_pos.
///
/// Uses only the PPQ *offset* within the current window (0..window_ppq),
/// anchored to the monotonic ring buffer write position. This is correct
/// across DAW loops because:
/// - PPQ may wrap, but the delta (current_ppq − window_start_ppq) is always
///   small (0..window_ppq) regardless of transport jumps.
/// - ring_buffer_pos is monotonic and never resets on loop.
///
/// Returns `(rb_start, window_len, playhead_fraction)` where `rb_start` is
/// the ring buffer position of the window start, `window_len` is the window
/// size in samples, and `playhead_fraction` (0.0..1.0) is how far through
/// the window the playhead is. Returns `None` if samples_per_beat is zero.
pub fn beat_aligned_window(
    snap: &TimeMappingSnapshot,
    sync_bars: f64,
    beats_per_bar: u32,
) -> Option<(usize, usize, f64)> {
    if snap.samples_per_beat <= 0.0 {
        return None;
    }
    let beats_in_window = sync_bars * beats_per_bar as f64;
    let window_len = (beats_in_window * snap.samples_per_beat).round() as usize;
    let ppq_per_bar = beats_per_bar as f64;
    let window_ppq = sync_bars * ppq_per_bar;

    // How far into the current window are we (in PPQ)?
    // Use rem_euclid to handle negative PPQ values correctly.
    let ppq_offset = snap.current_ppq.rem_euclid(window_ppq);

    // Playhead fraction: 0.0 at window start, ~1.0 at window end.
    let playhead_fraction = ppq_offset / window_ppq;

    // Convert to samples
    let sample_offset = (ppq_offset * snap.samples_per_beat).round() as usize;

    // Window start in ring buffer space: walk back from current write position
    let rb_start = snap.ring_buffer_pos.saturating_sub(sample_offset as u64) as usize;

    Some((rb_start, window_len, playhead_fraction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_snapshot_is_zero() {
        let tm = TimeMapping::new();
        let snap = tm.snapshot();
        assert_eq!(snap.current_ppq, 0.0);
        assert_eq!(snap.current_sample_pos, 0);
        assert_eq!(snap.ring_buffer_pos, 0);
        assert_eq!(snap.samples_per_beat, 0.0);
        assert_eq!(snap.discontinuity_counter, 0);
    }

    #[test]
    fn test_update_stores_values() {
        let tm = TimeMapping::new();
        // 120 BPM, 48000 Hz -> 24000 samples/beat
        tm.update(4.0, 96000, 50000, 120.0, 48000.0, 1024, true);
        let snap = tm.snapshot();
        assert!((snap.current_ppq - 4.0).abs() < 0.001);
        assert_eq!(snap.current_sample_pos, 96000);
        assert_eq!(snap.ring_buffer_pos, 50000);
        assert!((snap.samples_per_beat - 24000.0).abs() < 0.1);
    }

    #[test]
    fn test_play_start_increments_discontinuity() {
        let tm = TimeMapping::new();
        // First call with is_playing=true -> play start
        tm.update(0.0, 0, 0, 120.0, 48000.0, 1024, true);
        let snap = tm.snapshot();
        assert_eq!(snap.discontinuity_counter, 1);
    }

    #[test]
    fn test_continuous_play_no_discontinuity() {
        let tm = TimeMapping::new();
        tm.update(0.0, 0, 0, 120.0, 48000.0, 1024, true); // play start -> +1
        tm.update(0.0427, 1024, 1024, 120.0, 48000.0, 1024, true); // normal advance
        let snap = tm.snapshot();
        assert_eq!(snap.discontinuity_counter, 1); // no new discontinuity
    }

    #[test]
    fn test_loop_increments_discontinuity() {
        let tm = TimeMapping::new();
        tm.update(0.0, 0, 0, 120.0, 48000.0, 1024, true);
        tm.update(3.9, 93600, 93600, 120.0, 48000.0, 1024, true);
        // PPQ jumps backward (loop restart)
        tm.update(0.0, 0, 94624, 120.0, 48000.0, 1024, true);
        let snap = tm.snapshot();
        assert!(snap.discontinuity_counter >= 2);
    }

    #[test]
    fn test_not_playing_doesnt_update_ppq() {
        let tm = TimeMapping::new();
        tm.update(0.0, 0, 0, 120.0, 48000.0, 1024, true);
        let snap1 = tm.snapshot();
        tm.update(99.0, 999999, 999999, 120.0, 48000.0, 1024, false);
        let snap2 = tm.snapshot();
        // PPQ should not have changed
        assert_eq!(snap1.current_ppq, snap2.current_ppq);
    }

    #[test]
    fn test_beat_aligned_window_1_bar_4_4() {
        let snap = TimeMappingSnapshot {
            current_ppq: 6.5,
            current_sample_pos: 156000,
            ring_buffer_pos: 156000,
            samples_per_beat: 24000.0, // 120 BPM @ 48kHz
            discontinuity_counter: 0,
        };
        let (rb_start, len, frac) = beat_aligned_window(&snap, 1.0, 4).unwrap();
        // 1 bar = 4 beats = 96000 samples
        assert_eq!(len, 96000);
        // PPQ offset into window = 6.5 % 4.0 = 2.5
        // sample_offset = 2.5 * 24000 = 60000
        // rb_start = 156000 - 60000 = 96000
        assert_eq!(rb_start, 96000);
        // playhead_fraction = 2.5 / 4.0 = 0.625
        assert!((frac - 0.625).abs() < 0.001);
    }

    #[test]
    fn test_beat_aligned_window_zero_spb() {
        let snap = TimeMappingSnapshot {
            current_ppq: 0.0,
            current_sample_pos: 0,
            ring_buffer_pos: 0,
            samples_per_beat: 0.0,
            discontinuity_counter: 0,
        };
        assert!(beat_aligned_window(&snap, 1.0, 4).is_none());
    }

    #[test]
    fn test_beat_aligned_window_at_bar_boundary() {
        // Exactly at a bar boundary: offset should be 0, rb_start == ring_buffer_pos
        let snap = TimeMappingSnapshot {
            current_ppq: 8.0,
            current_sample_pos: 192000,
            ring_buffer_pos: 192000,
            samples_per_beat: 24000.0,
            discontinuity_counter: 0,
        };
        let (rb_start, len, frac) = beat_aligned_window(&snap, 1.0, 4).unwrap();
        assert_eq!(len, 96000);
        // PPQ 8.0 % 4.0 = 0.0, so rb_start == ring_buffer_pos
        assert_eq!(rb_start, 192000);
        // At bar boundary, playhead_fraction should be 0.0
        assert!((frac - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_beat_aligned_window_loop_scenario() {
        // After a DAW loop: PPQ wraps back to 0.5 but ring_buffer_pos
        // keeps advancing. The delta approach should still work.
        let snap = TimeMappingSnapshot {
            current_ppq: 0.5, // PPQ wrapped back after loop
            current_sample_pos: 12000,
            ring_buffer_pos: 500000, // Monotonic, much larger
            samples_per_beat: 24000.0,
            discontinuity_counter: 2,
        };
        let (rb_start, len, frac) = beat_aligned_window(&snap, 1.0, 4).unwrap();
        assert_eq!(len, 96000);
        // PPQ offset = 0.5 % 4.0 = 0.5, sample_offset = 0.5 * 24000 = 12000
        // rb_start = 500000 - 12000 = 488000
        assert_eq!(rb_start, 488000);
        // playhead_fraction = 0.5 / 4.0 = 0.125
        assert!((frac - 0.125).abs() < 0.001);
    }
}
