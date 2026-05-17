//! SPSC ring of recent (angle, amplitude) emits for the Polar Level
//! vectorscope mode.
//!
//! Thin typed wrapper over [`tract_dsp::spsc`]. The audio thread emits one
//! ray (the average M/S vector over the most recent emit interval); the GUI
//! reads the ring newest-first and renders each ray with age-scaled opacity.
//!
//! `RING_CAPACITY` is sized for the longest decay window divided by the
//! shortest emit interval, with headroom (~17 needed at 30 ms emit / 500 ms
//! decay; rounded up to 32).

use tract_dsp::spsc::{self, Consumer, Producer};

pub const RING_CAPACITY: usize = 32;

/// Audio-thread producer half of the polar-ray ring.
pub struct PolarRayProducer {
    inner: Producer,
}

/// GUI-thread consumer half of the polar-ray ring.
pub struct PolarRayConsumer {
    inner: Consumer,
}

/// Create a paired polar-ray producer/consumer.
pub fn ring_pair() -> (PolarRayProducer, PolarRayConsumer) {
    let (p, c) = spsc::channel(RING_CAPACITY);
    (PolarRayProducer { inner: p }, PolarRayConsumer { inner: c })
}

impl PolarRayProducer {
    /// Audio thread: emit one ray. `angle` in radians (typically `[0, π]` for
    /// the half-disc), `amp` the magnitude (rendered as a ray-length fraction
    /// of the disc radius after the consumer clamps it).
    #[inline]
    pub fn emit(&self, angle: f32, amp: f32) {
        self.inner.push(angle, amp);
    }
}

/// One ray entry produced by [`PolarRayConsumer::snapshot`].
///
/// `age_normalised` is `0.0` for the most recent emit and approaches `1.0`
/// for the oldest still-visible emit; the renderer turns it into an
/// opacity / colour decay.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ray {
    pub angle: f32,
    pub amp: f32,
    pub age_normalised: f32,
}

impl PolarRayConsumer {
    /// GUI thread: snapshot up to `out.len()` most-recent emits into `out`,
    /// newest first (`out[0]` is the most recent, `age_normalised = 0`).
    /// Returns the number of rays written.
    pub fn snapshot(&self, out: &mut [Ray]) -> usize {
        let cap = out.len().min(RING_CAPACITY);
        if cap == 0 {
            return 0;
        }
        let mut angle = [0.0_f32; RING_CAPACITY];
        let mut amp = [0.0_f32; RING_CAPACITY];
        let n = self.inner.snapshot_newest_first(cap, &mut angle, &mut amp);
        let denom = (n.saturating_sub(1) as f32).max(1.0);
        for (i, slot_out) in out.iter_mut().enumerate().take(n) {
            *slot_out = Ray {
                angle: angle[i],
                amp: amp[i],
                age_normalised: i as f32 / denom,
            };
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Ray {
        Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }
    }

    #[test]
    fn empty_snapshot_returns_zero() {
        let (_, cons) = ring_pair();
        let mut out = [blank(); 8];
        assert_eq!(cons.snapshot(&mut out), 0);
    }

    #[test]
    fn newest_first_ordering_and_age() {
        let (prod, cons) = ring_pair();
        prod.emit(0.1, 0.5);
        prod.emit(0.2, 0.6);
        prod.emit(0.3, 0.7);
        let mut out = [blank(); 8];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, 3);
        assert_eq!((out[0].angle, out[0].amp), (0.3, 0.7));
        assert_eq!(out[0].age_normalised, 0.0);
        assert_eq!(out[1].angle, 0.2);
        assert_eq!(out[2].angle, 0.1);
        assert!((out[2].age_normalised - 1.0).abs() < 1e-6);
    }

    #[test]
    fn caps_at_ring_capacity() {
        let (prod, cons) = ring_pair();
        for i in 0..(RING_CAPACITY * 2) {
            prod.emit(i as f32, i as f32);
        }
        let mut out = [blank(); RING_CAPACITY * 2];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, RING_CAPACITY);
        assert_eq!(out[0].angle, (RING_CAPACITY * 2 - 1) as f32);
        assert_eq!(out[RING_CAPACITY - 1].angle, RING_CAPACITY as f32);
    }
}
