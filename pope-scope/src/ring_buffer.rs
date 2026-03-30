//! Ring buffer with atomic write position and hierarchical mipmap.
//!
//! Single-writer (audio thread), non-consuming reader (GUI thread).
//! The reader copies data out — it never modifies write_pos.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Level 1 mipmap: min/max per 64-sample block.
pub const BLOCK_SIZE: usize = 64;
/// Level 2 mipmap: min/max per 256-sample block (4 Level 1 blocks).
pub const BLOCKS_PER_SUPER: usize = 4;
pub const SUPER_BLOCK_SIZE: usize = BLOCK_SIZE * BLOCKS_PER_SUPER; // 256

/// Min/max pair for mipmap levels.
#[derive(Clone, Copy, Debug, Default)]
pub struct MinMax {
    pub min: f32,
    pub max: f32,
}

/// A fixed-size circular buffer for audio samples.
///
/// The writer pushes samples sequentially. The reader can read any
/// historical window without consuming data.
pub struct RingBuffer {
    buffer: Vec<f32>,
    /// Monotonically increasing. Index into buffer is `write_pos % capacity`.
    write_pos: AtomicUsize,
    capacity: usize,
    // Mipmap Level 1: min/max per BLOCK_SIZE samples
    level1: Vec<MinMax>,
    level1_pos: AtomicUsize,
    level1_capacity: usize,
    // Accumulator for current L1 block being built
    l1_accum: MinMax,
    l1_accum_count: usize,
    // Mipmap Level 2: min/max per SUPER_BLOCK_SIZE samples
    level2: Vec<MinMax>,
    level2_pos: AtomicUsize,
    level2_capacity: usize,
    // Accumulator for current L2 block
    l2_block_count: usize,
    l2_accum: MinMax,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity (in samples).
    /// Pre-touches all memory via zero-fill.
    pub fn new(capacity: usize) -> Self {
        let l1_cap = capacity / BLOCK_SIZE + 1;
        let l2_cap = capacity / SUPER_BLOCK_SIZE + 1;
        Self {
            buffer: vec![0.0f32; capacity],
            write_pos: AtomicUsize::new(0),
            capacity,
            level1: vec![MinMax::default(); l1_cap],
            level1_pos: AtomicUsize::new(0),
            level1_capacity: l1_cap,
            l1_accum: MinMax {
                min: f32::MAX,
                max: f32::MIN,
            },
            l1_accum_count: 0,
            level2: vec![MinMax::default(); l2_cap],
            level2_pos: AtomicUsize::new(0),
            level2_capacity: l2_cap,
            l2_block_count: 0,
            l2_accum: MinMax {
                min: f32::MAX,
                max: f32::MIN,
            },
        }
    }

    /// Number of samples that have been written in total.
    pub fn total_written(&self) -> usize {
        self.write_pos.load(Ordering::Relaxed)
    }

    /// Buffer capacity in samples.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Push a slice of samples into the ring buffer.
    /// Called from the audio thread only (single writer).
    pub fn push(&mut self, samples: &[f32]) {
        let pos = self.write_pos.load(Ordering::Relaxed);
        for (i, &sample) in samples.iter().enumerate() {
            let idx = (pos + i) % self.capacity;
            self.buffer[idx] = sample;

            // Update L1 accumulator
            self.l1_accum.min = self.l1_accum.min.min(sample);
            self.l1_accum.max = self.l1_accum.max.max(sample);
            self.l1_accum_count += 1;

            if self.l1_accum_count >= BLOCK_SIZE {
                let l1_pos = self.level1_pos.load(Ordering::Relaxed);
                let l1_idx = l1_pos % self.level1_capacity;
                self.level1[l1_idx] = self.l1_accum;
                self.level1_pos.store(l1_pos + 1, Ordering::Relaxed);

                // Update L2 accumulator
                self.l2_accum.min = self.l2_accum.min.min(self.l1_accum.min);
                self.l2_accum.max = self.l2_accum.max.max(self.l1_accum.max);
                self.l2_block_count += 1;

                if self.l2_block_count >= BLOCKS_PER_SUPER {
                    let l2_pos = self.level2_pos.load(Ordering::Relaxed);
                    let l2_idx = l2_pos % self.level2_capacity;
                    self.level2[l2_idx] = self.l2_accum;
                    self.level2_pos.store(l2_pos + 1, Ordering::Relaxed);
                    self.l2_accum = MinMax {
                        min: f32::MAX,
                        max: f32::MIN,
                    };
                    self.l2_block_count = 0;
                }

                self.l1_accum = MinMax {
                    min: f32::MAX,
                    max: f32::MIN,
                };
                self.l1_accum_count = 0;
            }
        }
        self.write_pos
            .store(pos + samples.len(), Ordering::Relaxed);
    }

    /// Read the most recent `count` samples into `out`.
    /// Returns the number of samples actually copied (may be less than
    /// `count` if fewer have been written).
    /// Called from the GUI thread (non-consuming reader).
    pub fn read_most_recent(&self, out: &mut [f32]) -> usize {
        let pos = self.write_pos.load(Ordering::Relaxed);
        let count = out.len();
        let available = pos.min(self.capacity);
        let to_read = count.min(available);
        if to_read == 0 {
            return 0;
        }
        let start = pos.saturating_sub(to_read);
        for (i, slot) in out.iter_mut().enumerate().take(to_read) {
            let idx = (start + i) % self.capacity;
            *slot = self.buffer[idx];
        }
        to_read
    }

    /// Read samples from an absolute position range [start_pos, start_pos + count).
    /// Positions that haven't been written yet or have been overwritten return 0.0.
    /// Returns the number of valid samples copied.
    pub fn read_range(&self, start_pos: usize, out: &mut [f32]) -> usize {
        let pos = self.write_pos.load(Ordering::Relaxed);
        let count = out.len();
        let mut valid = 0;
        for (i, slot) in out.iter_mut().enumerate().take(count) {
            let abs = start_pos + i;
            if abs >= pos || (pos > self.capacity && abs < pos - self.capacity) {
                *slot = 0.0;
            } else {
                *slot = self.buffer[abs % self.capacity];
                valid += 1;
            }
        }
        valid
    }

    /// Read the most recent `count` blocks from Level 1 mipmap.
    pub fn read_most_recent_l1(&self, out: &mut [MinMax]) -> usize {
        let pos = self.level1_pos.load(Ordering::Relaxed);
        let count = out.len();
        let available = pos.min(self.level1_capacity);
        let to_read = count.min(available);
        if to_read == 0 {
            return 0;
        }
        let start = pos - to_read;
        for (i, slot) in out.iter_mut().enumerate().take(to_read) {
            let idx = (start + i) % self.level1_capacity;
            *slot = self.level1[idx];
        }
        to_read
    }

    /// Read the most recent `count` blocks from Level 2 mipmap.
    pub fn read_most_recent_l2(&self, out: &mut [MinMax]) -> usize {
        let pos = self.level2_pos.load(Ordering::Relaxed);
        let count = out.len();
        let available = pos.min(self.level2_capacity);
        let to_read = count.min(available);
        if to_read == 0 {
            return 0;
        }
        let start = pos - to_read;
        for (i, slot) in out.iter_mut().enumerate().take(to_read) {
            let idx = (start + i) % self.level2_capacity;
            *slot = self.level2[idx];
        }
        to_read
    }

    /// Select the appropriate mipmap level based on decimation factor.
    /// Returns 0 (raw), 1, or 2.
    pub fn select_level(decimation: usize) -> u8 {
        if decimation < BLOCK_SIZE {
            0
        } else if decimation < SUPER_BLOCK_SIZE {
            1
        } else {
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_buffer_is_zeroed() {
        let rb = RingBuffer::new(1024);
        assert_eq!(rb.capacity(), 1024);
        assert_eq!(rb.total_written(), 0);
    }

    #[test]
    fn test_push_and_read() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(rb.total_written(), 4);

        let mut out = [0.0f32; 4];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 4);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_read_fewer_than_available() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);

        let mut out = [0.0f32; 3];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 3);
        assert_eq!(out, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn test_read_more_than_written() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0]);

        let mut out = [0.0f32; 5];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[1], 2.0);
    }

    #[test]
    fn test_wrap_around() {
        let mut rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0]); // fills buffer
        rb.push(&[5.0, 6.0]); // wraps around

        let mut out = [0.0f32; 4];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 4);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_read_empty_buffer() {
        let rb = RingBuffer::new(1024);
        let mut out = [0.0f32; 4];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 0);
    }

    #[test]
    fn test_read_range_basic() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[10.0, 20.0, 30.0, 40.0, 50.0]);

        let mut out = [0.0f32; 3];
        let n = rb.read_range(1, &mut out);
        assert_eq!(n, 3);
        assert_eq!(out, [20.0, 30.0, 40.0]);
    }

    #[test]
    fn test_read_range_beyond_written() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0, 3.0]);

        let mut out = [0.0f32; 3];
        let n = rb.read_range(2, &mut out);
        // pos 2 is valid (3.0), pos 3 and 4 are beyond written
        assert_eq!(n, 1);
        assert_eq!(out[0], 3.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.0);
    }

    #[test]
    fn test_read_range_overwritten() {
        let mut rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]); // 1,2 overwritten

        let mut out = [0.0f32; 2];
        let n = rb.read_range(0, &mut out);
        assert_eq!(n, 0); // positions 0,1 have been overwritten
    }

    #[test]
    fn test_multiple_pushes() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0]);
        rb.push(&[3.0, 4.0]);
        rb.push(&[5.0]);
        assert_eq!(rb.total_written(), 5);

        let mut out = [0.0f32; 5];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 5);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn test_mipmap_level1_produced_after_block() {
        let mut rb = RingBuffer::new(1024);
        // Push exactly one block of BLOCK_SIZE samples
        let samples: Vec<f32> = (0..BLOCK_SIZE).map(|i| i as f32).collect();
        rb.push(&samples);

        let mut out = [MinMax::default(); 1];
        let n = rb.read_most_recent_l1(&mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0].min, 0.0);
        assert_eq!(out[0].max, (BLOCK_SIZE - 1) as f32);
    }

    #[test]
    fn test_mipmap_level1_not_produced_before_block() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0, 3.0]); // less than BLOCK_SIZE

        let mut out = [MinMax::default(); 1];
        let n = rb.read_most_recent_l1(&mut out);
        assert_eq!(n, 0);
    }

    #[test]
    fn test_mipmap_level2_produced_after_super_block() {
        let mut rb = RingBuffer::new(4096);
        let samples: Vec<f32> = (0..SUPER_BLOCK_SIZE).map(|i| i as f32).collect();
        rb.push(&samples);

        let mut out = [MinMax::default(); 1];
        let n = rb.read_most_recent_l2(&mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0].min, 0.0);
        assert_eq!(out[0].max, (SUPER_BLOCK_SIZE - 1) as f32);
    }

    #[test]
    fn test_mipmap_negative_samples() {
        let mut rb = RingBuffer::new(1024);
        let mut samples = vec![0.0f32; BLOCK_SIZE];
        samples[10] = -5.0;
        samples[20] = 3.0;
        rb.push(&samples);

        let mut out = [MinMax::default(); 1];
        rb.read_most_recent_l1(&mut out);
        assert_eq!(out[0].min, -5.0);
        assert_eq!(out[0].max, 3.0);
    }

    #[test]
    fn test_select_level() {
        assert_eq!(RingBuffer::select_level(1), 0);
        assert_eq!(RingBuffer::select_level(63), 0);
        assert_eq!(RingBuffer::select_level(64), 1);
        assert_eq!(RingBuffer::select_level(255), 1);
        assert_eq!(RingBuffer::select_level(256), 2);
        assert_eq!(RingBuffer::select_level(4096), 2);
    }
}
