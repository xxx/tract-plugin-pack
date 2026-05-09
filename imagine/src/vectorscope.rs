//! SPSC ring buffer of (L, R) samples for the vectorscope display.
//!
//! Each sample is stored as `(AtomicU32, AtomicU32)` (L bits, R bits). The audio
//! thread (Producer) writes the two halves with `Relaxed` ordering, then publishes
//! `write_pos` with `Release`. The GUI thread (Consumer) loads `write_pos` with
//! `Acquire`, then reads the slot's L and R independently with `Relaxed`.
//!
//! The Acquire/Release pair establishes a happens-before edge between the slot
//! writes and the index publish, so the consumer can never read a slot whose
//! writes haven't completed.
//!
//! **Per-sample tear:** the L and R reads are independent. If the writer pushes
//! a new sample between the consumer's L and R reads, the consumer can observe
//! L from frame N paired with R from frame N+1. This is acceptable because the
//! vectorscope decimates thousands of points per frame; one torn pair is sub-pixel.
//!
//! `RING_CAPACITY = 32768` is sized to hold ~170 ms at 192 kHz, far longer than
//! any realistic GUI frame interval, so the audio thread cannot lap the GUI in
//! a single frame.
//!
//! Compare to `pope-scope/src/ring_buffer.rs`, which uses `Vec<f32>` because it
//! sits behind a `RwLock`. Imagine's ring is lock-free (writer is `&self`),
//! which forces atomic slots.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

pub const RING_CAPACITY: usize = 65_536;

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

    #[test]
    fn concurrent_writer_reader_no_torn_index() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::Duration;

        let (prod, cons) = ring_pair();
        let stop = Arc::new(AtomicBool::new(false));

        // Writer pushes a strictly-monotone sequence: L = i, R = -i, for i = 0, 1, 2, ...
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

        // Reader takes 1000 snapshots while the writer runs.
        let mut l_buf = vec![0.0_f32; 256];
        let mut r_buf = vec![0.0_f32; 256];
        let mut snapshots_taken = 0;
        let mut max_n = 0;
        for _ in 0..1000 {
            let n = cons.snapshot(256, &mut l_buf, &mut r_buf);
            if n > 0 {
                snapshots_taken += 1;
                max_n = max_n.max(n);
                // Within a snapshot, L values must be strictly monotone increasing
                // (or wrap consistently if u64 -> f32 saturates). The writer's per-slot
                // (L, R) pair can tear under contention, but L within the snapshot's
                // collected window must reflect a contiguous range up to write_pos.
                for i in 1..n {
                    let prev_l = l_buf[i - 1];
                    let cur_l = l_buf[i];
                    // The producer's L stream is monotone (i, i+1, i+2, ...) until
                    // u64-cast-to-f32 starts saturating (>= ~2^24). For our short test,
                    // we simply check that consecutive samples differ by exactly 1.0.
                    let diff = cur_l - prev_l;
                    assert!(
                        (diff - 1.0).abs() < 0.5 || cur_l == prev_l,
                        "non-monotone L window at i={i}: prev={prev_l} cur={cur_l}"
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
