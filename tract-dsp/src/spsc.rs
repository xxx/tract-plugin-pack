//! Lock-free single-producer / single-consumer ring of paired `f32` samples.
//!
//! Each slot holds two `f32` values stored as `AtomicU32` bit patterns
//! (`f32::to_bits`). The producer (audio thread) writes the two halves with
//! `Relaxed` ordering, then publishes `write_pos` with `Release`. The consumer
//! (GUI thread) loads `write_pos` with `Acquire`, then reads slots with
//! `Relaxed`. The Acquire/Release pair establishes a happens-before edge so
//! the consumer never reads a slot whose writes have not completed.
//!
//! **Per-slot tear:** the two reads are independent. If the producer writes a
//! new pair between the consumer's two reads, the consumer can observe one
//! half from frame N and the other from N+1. Callers that decimate many
//! samples per GUI frame (vectorscopes) treat one torn pair as sub-pixel.
//!
//! This is the shared engine behind `imagine`'s vectorscope and polar-ray
//! rings; capacity and payload semantics are fixed by the calling wrapper.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner {
    a: Vec<AtomicU32>,
    b: Vec<AtomicU32>,
    capacity: usize,
    write_pos: AtomicUsize,
}

/// Audio-thread producer half. Created by [`channel`].
pub struct Producer {
    inner: Arc<Inner>,
}

/// GUI-thread consumer half. Created by [`channel`].
pub struct Consumer {
    inner: Arc<Inner>,
}

/// Create a producer/consumer pair backed by a ring of `capacity` slots.
///
/// # Panics
/// Panics if `capacity` is zero.
pub fn channel(capacity: usize) -> (Producer, Consumer) {
    assert!(capacity > 0, "SPSC ring capacity must be non-zero");
    let inner = Arc::new(Inner {
        a: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
        b: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
        capacity,
        write_pos: AtomicUsize::new(0),
    });
    (
        Producer {
            inner: inner.clone(),
        },
        Consumer { inner },
    )
}

impl Producer {
    /// Push one `(a, b)` pair. Audio thread; lock-free and allocation-free.
    #[inline]
    pub fn push(&self, a: f32, b: f32) {
        let idx = self.inner.write_pos.load(Ordering::Relaxed);
        let slot = idx % self.inner.capacity;
        self.inner.a[slot].store(a.to_bits(), Ordering::Relaxed);
        self.inner.b[slot].store(b.to_bits(), Ordering::Relaxed);
        self.inner
            .write_pos
            .store(idx.wrapping_add(1), Ordering::Release);
    }
}

impl Consumer {
    /// Ring capacity (slot count).
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Copy up to `count` most-recent pairs into `a_out` / `b_out`, **oldest
    /// first**: the last written entry is the most recent. Returns the number
    /// of pairs written. Never reads more than `capacity`, `a_out.len()`, or
    /// `b_out.len()` entries.
    pub fn snapshot_oldest_first(
        &self,
        count: usize,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) -> usize {
        let count = count
            .min(self.inner.capacity)
            .min(a_out.len())
            .min(b_out.len());
        let write_pos = self.inner.write_pos.load(Ordering::Acquire);
        let available = write_pos.min(self.inner.capacity);
        let n = count.min(available);
        if n == 0 {
            return 0;
        }
        let start = write_pos.wrapping_sub(n);
        for i in 0..n {
            let slot = start.wrapping_add(i) % self.inner.capacity;
            a_out[i] = f32::from_bits(self.inner.a[slot].load(Ordering::Relaxed));
            b_out[i] = f32::from_bits(self.inner.b[slot].load(Ordering::Relaxed));
        }
        n
    }

    /// Copy up to `count` most-recent pairs into `a_out` / `b_out`, **newest
    /// first**: `out[0]` is the most recent pair, `out[n-1]` the oldest still
    /// visible. Returns the number of pairs written.
    pub fn snapshot_newest_first(
        &self,
        count: usize,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) -> usize {
        let count = count
            .min(self.inner.capacity)
            .min(a_out.len())
            .min(b_out.len());
        let write_pos = self.inner.write_pos.load(Ordering::Acquire);
        let available = write_pos.min(self.inner.capacity);
        let n = count.min(available);
        for i in 0..n {
            let logical = write_pos.wrapping_sub(i + 1);
            let slot = logical % self.inner.capacity;
            a_out[i] = f32::from_bits(self.inner.a[slot].load(Ordering::Relaxed));
            b_out[i] = f32::from_bits(self.inner.b[slot].load(Ordering::Relaxed));
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oldest_first_preserves_order() {
        let (prod, cons) = channel(1024);
        for i in 0..1000 {
            prod.push(i as f32, -(i as f32));
        }
        let mut a = vec![0.0; 100];
        let mut b = vec![0.0; 100];
        let n = cons.snapshot_oldest_first(100, &mut a, &mut b);
        assert_eq!(n, 100);
        for i in 0..100 {
            assert_eq!(a[i], (900 + i) as f32);
            assert_eq!(b[i], -((900 + i) as f32));
        }
    }

    #[test]
    fn oldest_first_smaller_than_history_returns_recent() {
        let (prod, cons) = channel(1024);
        for i in 0..50 {
            prod.push(i as f32, 0.0);
        }
        let mut a = vec![0.0; 10];
        let mut b = vec![0.0; 10];
        let n = cons.snapshot_oldest_first(10, &mut a, &mut b);
        assert_eq!(n, 10);
        for i in 0..10 {
            assert_eq!(a[i], (40 + i) as f32);
        }
    }

    #[test]
    fn oldest_first_more_than_history_returns_partial() {
        let (prod, cons) = channel(1024);
        for i in 0..5 {
            prod.push(i as f32, 0.0);
        }
        let mut a = vec![0.0; 100];
        let mut b = vec![0.0; 100];
        let n = cons.snapshot_oldest_first(100, &mut a, &mut b);
        assert_eq!(n, 5);
        for i in 0..5 {
            assert_eq!(a[i], i as f32);
        }
    }

    #[test]
    fn wraparound_continuous() {
        let cap = 4096;
        let (prod, cons) = channel(cap);
        let total = cap + cap / 2;
        for i in 0..total {
            prod.push(i as f32, 0.0);
        }
        let mut a = vec![0.0; 100];
        let mut b = vec![0.0; 100];
        let n = cons.snapshot_oldest_first(100, &mut a, &mut b);
        assert_eq!(n, 100);
        let start = total - 100;
        for i in 0..100 {
            assert_eq!(a[i], (start + i) as f32, "i={i}");
        }
    }

    #[test]
    fn empty_snapshot() {
        let (_, cons) = channel(64);
        let mut a = vec![0.0; 10];
        let mut b = vec![0.0; 10];
        assert_eq!(cons.snapshot_oldest_first(10, &mut a, &mut b), 0);
        assert_eq!(cons.snapshot_newest_first(10, &mut a, &mut b), 0);
    }

    #[test]
    fn newest_first_ordering() {
        let (prod, cons) = channel(32);
        prod.push(0.1, 0.5);
        prod.push(0.2, 0.6);
        prod.push(0.3, 0.7);
        let mut a = vec![0.0; 8];
        let mut b = vec![0.0; 8];
        let n = cons.snapshot_newest_first(8, &mut a, &mut b);
        assert_eq!(n, 3);
        assert_eq!((a[0], b[0]), (0.3, 0.7));
        assert_eq!((a[1], b[1]), (0.2, 0.6));
        assert_eq!((a[2], b[2]), (0.1, 0.5));
    }

    #[test]
    fn newest_first_caps_at_capacity() {
        let cap = 32;
        let (prod, cons) = channel(cap);
        for i in 0..(cap * 2) {
            prod.push(i as f32, i as f32);
        }
        let mut a = vec![0.0; cap * 2];
        let mut b = vec![0.0; cap * 2];
        let n = cons.snapshot_newest_first(cap * 2, &mut a, &mut b);
        assert_eq!(n, cap);
        assert_eq!(a[0], (cap * 2 - 1) as f32);
        assert_eq!(a[cap - 1], cap as f32);
    }

    #[test]
    #[should_panic(expected = "capacity must be non-zero")]
    fn zero_capacity_panics() {
        let _ = channel(0);
    }

    // Concurrency stress test. Consolidated here from imagine's vectorscope.rs
    // (a memory note records it as occasionally flaky under heavy load — a
    // lone failure is not a regression; re-run before treating it as one).
    #[test]
    fn concurrent_writer_reader_no_torn_index() {
        use std::sync::atomic::AtomicBool;
        use std::thread;
        use std::time::Duration;

        let (prod, cons) = channel(65_536);
        let stop = Arc::new(AtomicBool::new(false));

        let stop_w = stop.clone();
        let writer = thread::spawn(move || {
            let mut i: u64 = 0;
            while !stop_w.load(Ordering::Relaxed) {
                prod.push(i as f32, -(i as f32));
                i = i.wrapping_add(1);
                if i & 0xfff == 0 {
                    std::hint::spin_loop();
                }
            }
            i
        });

        let mut a_buf = vec![0.0_f32; 256];
        let mut b_buf = vec![0.0_f32; 256];
        let mut snapshots_taken = 0;
        let mut max_n = 0;
        for _ in 0..1000 {
            let n = cons.snapshot_oldest_first(256, &mut a_buf, &mut b_buf);
            if n > 0 {
                snapshots_taken += 1;
                max_n = max_n.max(n);
                for i in 1..n {
                    let diff = a_buf[i] - a_buf[i - 1];
                    assert!(
                        (diff - 1.0).abs() < 0.5 || a_buf[i] == a_buf[i - 1],
                        "non-monotone window at i={i}: prev={} cur={}",
                        a_buf[i - 1],
                        a_buf[i]
                    );
                }
            }
            thread::sleep(Duration::from_micros(50));
        }
        stop.store(true, Ordering::Relaxed);
        let final_count = writer.join().unwrap();
        assert!(
            snapshots_taken > 0,
            "snapshots never saw data; final writer count: {final_count}"
        );
        assert!(max_n > 0);
    }
}
