//! SPSC ring of (L, R) samples for the vectorscope display.
//!
//! Thin typed wrapper over [`tract_dsp::spsc`] — the lock-free ring engine
//! lives there and is shared with `polar_rays`. This module fixes the
//! capacity and the (L, R) payload naming.
//!
//! `RING_CAPACITY` holds ~340 ms at 192 kHz — far longer than any realistic
//! GUI frame interval, so the audio thread cannot lap the GUI in one frame.

use tract_dsp::spsc::{self, Consumer, Producer};

pub const RING_CAPACITY: usize = 65_536;

/// Audio-thread producer half of the vectorscope ring.
pub struct VectorProducer {
    inner: Producer,
}

/// GUI-thread consumer half of the vectorscope ring.
pub struct VectorConsumer {
    inner: Consumer,
}

/// Create a paired vectorscope producer/consumer.
pub fn ring_pair() -> (VectorProducer, VectorConsumer) {
    let (p, c) = spsc::channel(RING_CAPACITY);
    (VectorProducer { inner: p }, VectorConsumer { inner: c })
}

impl VectorProducer {
    /// Audio thread: push one (L, R) sample.
    #[inline]
    pub fn push(&self, l: f32, r: f32) {
        self.inner.push(l, r);
    }
}

impl VectorConsumer {
    /// GUI thread: snapshot up to `count` most-recent samples into the
    /// provided buffers, oldest first. Returns the number of samples copied.
    pub fn snapshot(&self, count: usize, l_out: &mut [f32], r_out: &mut [f32]) -> usize {
        self.inner.snapshot_oldest_first(count, l_out, r_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_snapshot_preserves_recent_order() {
        let (prod, cons) = ring_pair();
        for i in 0..1000 {
            prod.push(i as f32, -(i as f32));
        }
        let mut l = vec![0.0; 100];
        let mut r = vec![0.0; 100];
        let n = cons.snapshot(100, &mut l, &mut r);
        assert_eq!(n, 100);
        for i in 0..100 {
            assert_eq!(l[i], (900 + i) as f32);
            assert_eq!(r[i], -((900 + i) as f32));
        }
    }

    #[test]
    fn empty_snapshot_returns_zero() {
        let (_, cons) = ring_pair();
        let mut l = vec![0.0; 10];
        let mut r = vec![0.0; 10];
        assert_eq!(cons.snapshot(10, &mut l, &mut r), 0);
    }
}
