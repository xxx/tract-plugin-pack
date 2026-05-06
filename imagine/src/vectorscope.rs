//! SPSC ring buffer of (L, R) samples for the vectorscope display.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

pub const RING_CAPACITY: usize = 32_768;

struct Inner {
    samples_l: Vec<AtomicU32>,
    samples_r: Vec<AtomicU32>,
    write_pos: AtomicUsize,
}

pub struct VectorProducer {
    inner: Arc<Inner>,
}

pub struct VectorConsumer {
    inner: Arc<Inner>,
}

pub fn ring_pair() -> (VectorProducer, VectorConsumer) {
    let inner = Arc::new(Inner {
        samples_l: (0..RING_CAPACITY).map(|_| AtomicU32::new(0)).collect(),
        samples_r: (0..RING_CAPACITY).map(|_| AtomicU32::new(0)).collect(),
        write_pos: AtomicUsize::new(0),
    });
    (
        VectorProducer {
            inner: inner.clone(),
        },
        VectorConsumer { inner },
    )
}

impl VectorProducer {
    /// Audio thread: push one sample.
    #[inline]
    pub fn push(&self, l: f32, r: f32) {
        let idx = self.inner.write_pos.load(Ordering::Relaxed);
        let slot = idx % RING_CAPACITY;
        self.inner.samples_l[slot].store(l.to_bits(), Ordering::Relaxed);
        self.inner.samples_r[slot].store(r.to_bits(), Ordering::Relaxed);
        self.inner
            .write_pos
            .store(idx.wrapping_add(1), Ordering::Release);
    }
}

impl VectorConsumer {
    /// GUI thread: snapshot up to `count` most-recent samples into the provided buffers.
    /// Returns the number of samples actually copied.
    pub fn snapshot(&self, count: usize, l_out: &mut [f32], r_out: &mut [f32]) -> usize {
        let count = count.min(RING_CAPACITY).min(l_out.len()).min(r_out.len());
        let write_pos = self.inner.write_pos.load(Ordering::Acquire);
        let available = write_pos.min(RING_CAPACITY);
        let n = count.min(available);
        if n == 0 {
            return 0;
        }
        let start_logical = write_pos.wrapping_sub(n);
        for i in 0..n {
            let logical = start_logical.wrapping_add(i);
            let slot = logical % RING_CAPACITY;
            l_out[i] = f32::from_bits(self.inner.samples_l[slot].load(Ordering::Relaxed));
            r_out[i] = f32::from_bits(self.inner.samples_r[slot].load(Ordering::Relaxed));
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_preserves_order() {
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
    fn snapshot_smaller_than_history_returns_recent() {
        let (prod, cons) = ring_pair();
        for i in 0..50 {
            prod.push(i as f32, 0.0);
        }
        let mut l = vec![0.0; 10];
        let mut r = vec![0.0; 10];
        let n = cons.snapshot(10, &mut l, &mut r);
        assert_eq!(n, 10);
        for i in 0..10 {
            assert_eq!(l[i], (40 + i) as f32);
        }
    }

    #[test]
    fn snapshot_more_than_history_returns_partial() {
        let (prod, cons) = ring_pair();
        for i in 0..5 {
            prod.push(i as f32, 0.0);
        }
        let mut l = vec![0.0; 100];
        let mut r = vec![0.0; 100];
        let n = cons.snapshot(100, &mut l, &mut r);
        assert_eq!(n, 5);
        for i in 0..5 {
            assert_eq!(l[i], i as f32);
        }
    }

    #[test]
    fn wraparound_continuous() {
        let (prod, cons) = ring_pair();
        let total = RING_CAPACITY + RING_CAPACITY / 2;
        for i in 0..total {
            prod.push(i as f32, 0.0);
        }
        let mut l = vec![0.0; 100];
        let mut r = vec![0.0; 100];
        let n = cons.snapshot(100, &mut l, &mut r);
        assert_eq!(n, 100);
        let start = total - 100;
        for i in 0..100 {
            assert_eq!(l[i], (start + i) as f32, "i={i}");
        }
    }

    #[test]
    fn empty_snapshot() {
        let (_, cons) = ring_pair();
        let mut l = vec![0.0; 10];
        let mut r = vec![0.0; 10];
        let n = cons.snapshot(10, &mut l, &mut r);
        assert_eq!(n, 0);
    }
}
