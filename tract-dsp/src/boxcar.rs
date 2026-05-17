//! Fixed-window running-sum accumulator (boxcar).

/// A sliding window over the last `window` pushed values, maintaining an
/// `f64` running sum so the windowed mean is O(1) per sample and free of the
/// drift an `f32` re-summation would accumulate.
///
/// Generic over the stored element type `T` (`f32` or `f64`); the running sum
/// is always `f64`. The backing ring is pre-allocated to a fixed maximum
/// capacity at construction — `push`, `set_window`, and `reset` never allocate.
pub struct RunningSumWindow<T> {
    ring: Vec<T>,
    /// Index where the next value is written (and the oldest currently sits).
    pos: usize,
    /// Number of values currently in the window (`<= window`).
    filled: usize,
    /// Logical window length (`<= ring.len()`).
    window: usize,
    /// Running sum of the values currently in the window.
    sum: f64,
}

impl<T: Copy + Default + Into<f64>> RunningSumWindow<T> {
    /// Create a window backed by a ring of `max_capacity` elements (at least
    /// 1), with the logical window set to `window` (clamped to
    /// `[1, max_capacity]`).
    pub fn new(max_capacity: usize, window: usize) -> Self {
        let max_capacity = max_capacity.max(1);
        Self {
            ring: vec![T::default(); max_capacity],
            pos: 0,
            filled: 0,
            window: window.clamp(1, max_capacity),
            sum: 0.0,
        }
    }

    /// Push one value: evict the oldest if the window is full, then add the new.
    #[inline]
    pub fn push(&mut self, x: T) {
        if self.filled == self.window {
            self.sum -= self.ring[self.pos].into();
        }
        self.ring[self.pos] = x;
        self.sum += x.into();
        self.pos += 1;
        if self.pos >= self.window {
            self.pos = 0;
        }
        if self.filled < self.window {
            self.filled += 1;
        }
    }

    /// Running sum of the values currently in the window.
    pub fn sum(&self) -> f64 {
        self.sum
    }

    /// Number of values currently in the window (`<= window()`).
    pub fn filled(&self) -> usize {
        self.filled
    }

    /// Current logical window length.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Mean of the values currently in the window; `0.0` when empty. The sum
    /// is clamped at `0.0` first to absorb any f64 drift slightly below zero.
    pub fn mean(&self) -> f64 {
        if self.filled == 0 {
            0.0
        } else {
            self.sum.max(0.0) / self.filled as f64
        }
    }

    /// Change the logical window length without reallocating (clamped to
    /// `[1, max_capacity]`). A no-op if unchanged; otherwise the window is
    /// cleared.
    pub fn set_window(&mut self, window: usize) {
        let window = window.clamp(1, self.ring.len());
        if self.window != window {
            self.window = window;
            self.reset();
        }
    }

    /// Clear the window: zero the ring, the sum, and the counters.
    pub fn reset(&mut self) {
        self.ring.fill(T::default());
        self.pos = 0;
        self.filled = 0;
        self.sum = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_then_evicts() {
        let mut w = RunningSumWindow::<f32>::new(10, 4);
        for v in [1.0, 2.0, 3.0, 4.0] {
            w.push(v);
        }
        assert_eq!(w.filled(), 4);
        assert_eq!(w.sum(), 10.0);
        w.push(5.0); // evicts 1.0
        assert_eq!(w.filled(), 4);
        assert_eq!(w.sum(), 14.0); // 2 + 3 + 4 + 5
    }

    #[test]
    fn mean_of_dc() {
        let mut w = RunningSumWindow::<f32>::new(100, 50);
        for _ in 0..50 {
            w.push(0.5);
        }
        assert!((w.mean() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn running_sum_stable_over_many_cycles() {
        let mut w = RunningSumWindow::<f32>::new(100, 100);
        for _ in 0..100_100 {
            w.push(0.5);
        }
        assert!(
            (w.mean() - 0.5).abs() < 1e-6,
            "running sum drifted: {}",
            w.mean()
        );
    }

    #[test]
    fn set_window_resets_only_on_change() {
        let mut w = RunningSumWindow::<f64>::new(100, 10);
        for _ in 0..10 {
            w.push(1.0);
        }
        assert_eq!(w.sum(), 10.0);
        w.set_window(10); // unchanged → no reset
        assert_eq!(w.sum(), 10.0);
        w.set_window(20); // changed → reset
        assert_eq!(w.sum(), 0.0);
        assert_eq!(w.filled(), 0);
        assert_eq!(w.window(), 20);
    }

    #[test]
    fn reset_clears() {
        let mut w = RunningSumWindow::<f64>::new(50, 10);
        for _ in 0..10 {
            w.push(2.0);
        }
        w.reset();
        assert_eq!(w.sum(), 0.0);
        assert_eq!(w.filled(), 0);
        assert_eq!(w.mean(), 0.0);
    }

    #[test]
    fn window_of_one() {
        let mut w = RunningSumWindow::<f32>::new(10, 1);
        w.push(0.5);
        assert_eq!(w.mean(), 0.5);
        w.push(0.3);
        assert!((w.mean() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn empty_window_mean_is_zero() {
        let w = RunningSumWindow::<f64>::new(10, 5);
        assert_eq!(w.mean(), 0.0);
        assert_eq!(w.filled(), 0);
    }

    #[test]
    fn window_clamped_to_capacity() {
        let w = RunningSumWindow::<f32>::new(8, 999);
        assert_eq!(w.window(), 8);
    }
}
