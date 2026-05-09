//! SPSC ring of recent (angle, amplitude) emits for the Polar Level
//! vectorscope mode.
//!
//! Per Ozone Imager's Polar Level mode (per the user's reverse-engineering):
//! the audio thread periodically emits ONE ray representing the average
//! M/S vector over the most recent emit interval. Each ray then fades
//! over its own lifetime — there is no per-pan-bin energy histogram. The
//! GUI iterates the ring of recent emits and draws each ray with its
//! age-scaled opacity.
//!
//! `RING_CAPACITY` is sized for the longest expected decay window divided
//! by the shortest expected emit interval, with headroom. At 30 ms emit
//! interval and 500 ms decay, ~17 slots are needed; round up to 32.
//!
//! Storage matches `vectorscope.rs`: two `AtomicU32` per slot (angle bits,
//! amp bits). Audio thread writes both with Relaxed; publishes `write_pos`
//! with Release. GUI reads `write_pos` with Acquire; iterates from
//! newest-back. Per-slot tear (angle from emit N, amp from emit N+1) is
//! benign — at most one emit per frame would be affected, sub-pixel.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

pub const RING_CAPACITY: usize = 32;

struct Inner {
    angle: Vec<AtomicU32>,
    amp: Vec<AtomicU32>,
    write_pos: AtomicUsize,
}

pub struct PolarRayProducer {
    inner: Arc<Inner>,
}

pub struct PolarRayConsumer {
    inner: Arc<Inner>,
}

pub fn ring_pair() -> (PolarRayProducer, PolarRayConsumer) {
    let inner = Arc::new(Inner {
        angle: (0..RING_CAPACITY).map(|_| AtomicU32::new(0)).collect(),
        amp: (0..RING_CAPACITY).map(|_| AtomicU32::new(0)).collect(),
        write_pos: AtomicUsize::new(0),
    });
    (
        PolarRayProducer {
            inner: inner.clone(),
        },
        PolarRayConsumer { inner },
    )
}

impl PolarRayProducer {
    /// Audio thread: emit one ray. `angle` in radians (typically `[0, π]`
    /// for the half-disc), `amp` is the magnitude (rendered as ray length
    /// fraction of disc radius after clamping by the consumer).
    #[inline]
    pub fn emit(&self, angle: f32, amp: f32) {
        let idx = self.inner.write_pos.load(Ordering::Relaxed);
        let slot = idx % RING_CAPACITY;
        self.inner.angle[slot].store(angle.to_bits(), Ordering::Relaxed);
        self.inner.amp[slot].store(amp.to_bits(), Ordering::Relaxed);
        self.inner
            .write_pos
            .store(idx.wrapping_add(1), Ordering::Release);
    }
}

/// One ray entry emitted by the GUI consumer.
///
/// `age_normalised` is `0.0` for the most recent emit and approaches `1.0`
/// for the oldest still-visible emit. The renderer turns this into an
/// opacity / colour decay.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ray {
    pub angle: f32,
    pub amp: f32,
    pub age_normalised: f32,
}

impl PolarRayConsumer {
    /// GUI thread: snapshot up to `max_rays` most-recent emits into the
    /// caller-provided slice. Returns the number of rays written.
    /// `out[0]` is the most-recent emit (`age_normalised = 0`); successive
    /// entries are progressively older.
    pub fn snapshot(&self, out: &mut [Ray]) -> usize {
        let cap = out.len().min(RING_CAPACITY);
        if cap == 0 {
            return 0;
        }
        let write_pos = self.inner.write_pos.load(Ordering::Acquire);
        let available = write_pos.min(RING_CAPACITY);
        let n = cap.min(available);
        if n == 0 {
            return 0;
        }
        let denom = (n.saturating_sub(1) as f32).max(1.0);
        for (i, slot_out) in out.iter_mut().enumerate().take(n) {
            // i = 0 is the most recent emit; i = n-1 is the oldest.
            let logical = write_pos.wrapping_sub(i + 1);
            let slot = logical % RING_CAPACITY;
            let angle = f32::from_bits(self.inner.angle[slot].load(Ordering::Relaxed));
            let amp = f32::from_bits(self.inner.amp[slot].load(Ordering::Relaxed));
            let age_normalised = (i as f32) / denom;
            *slot_out = Ray {
                angle,
                amp,
                age_normalised,
            };
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_returns_zero() {
        let (_, cons) = ring_pair();
        let mut out = [Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; 8];
        assert_eq!(cons.snapshot(&mut out), 0);
    }

    #[test]
    fn newest_first_ordering() {
        let (prod, cons) = ring_pair();
        prod.emit(0.1, 0.5);
        prod.emit(0.2, 0.6);
        prod.emit(0.3, 0.7);
        let mut out = [Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; 8];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, 3);
        assert_eq!(out[0].angle, 0.3);
        assert_eq!(out[0].amp, 0.7);
        assert_eq!(out[0].age_normalised, 0.0);
        assert_eq!(out[1].angle, 0.2);
        assert_eq!(out[2].angle, 0.1);
        assert!(out[2].age_normalised > out[0].age_normalised);
    }

    #[test]
    fn cap_at_ring_size() {
        let (prod, cons) = ring_pair();
        for i in 0..(RING_CAPACITY * 2) {
            prod.emit(i as f32, i as f32);
        }
        let mut out = [Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; RING_CAPACITY * 2];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, RING_CAPACITY);
        // Newest visible is the last emit.
        let expected_newest = (RING_CAPACITY * 2 - 1) as f32;
        assert_eq!(out[0].angle, expected_newest);
        // Oldest visible is RING_CAPACITY emits ago.
        let expected_oldest = (RING_CAPACITY * 2 - RING_CAPACITY) as f32;
        assert_eq!(out[RING_CAPACITY - 1].angle, expected_oldest);
    }

    #[test]
    fn age_runs_zero_to_one() {
        let (prod, cons) = ring_pair();
        for i in 0..5 {
            prod.emit(i as f32, 0.0);
        }
        let mut out = [Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }; 5];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, 5);
        assert_eq!(out[0].age_normalised, 0.0);
        assert!((out[4].age_normalised - 1.0).abs() < 1e-6);
        // Monotone non-decreasing.
        for i in 1..n {
            assert!(out[i].age_normalised >= out[i - 1].age_normalised);
        }
    }
}
