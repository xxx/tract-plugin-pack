//! Ring buffer with atomic write position and hierarchical mipmap.
//!
//! Single-writer (audio thread), non-consuming reader (GUI thread).
//! The reader copies data out — it never modifies write_pos.
//!
//! `level1_pos` and `level2_pos` are plain `usize` (not atomic) because:
//! - `push()` takes `&mut self` (exclusive access, behind RwLock write guard)
//! - `read_most_recent_l1/l2()` takes `&self` (shared access, behind RwLock read guard)
//! - The RwLock on the store provides memory synchronization between writer and readers.

use fearless_simd::{Level, dispatch, f32x16, prelude::*};
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

/// Compute min/max of a slice using SIMD (f32x16).
fn minmax_slice(s: &[f32]) -> MinMax {
    dispatch!(Level::new(), simd => {
        let mut vmin = f32x16::splat(simd, f32::MAX);
        let mut vmax = f32x16::splat(simd, f32::MIN);
        let (chunks, remainder) = s.as_chunks::<16>();
        for chunk in chunks {
            let v = f32x16::from_slice(simd, chunk);
            vmin = vmin.min_precise(v);
            vmax = vmax.max_precise(v);
        }
        let mut min = vmin.reduce_min_precise();
        let mut max = vmax.reduce_max_precise();
        for &x in remainder {
            min = min.min(x);
            max = max.max(x);
        }
        MinMax { min, max }
    })
}

fn copy_ring_range<T: Copy>(buffer: &[T], start_pos: usize, out: &mut [T]) {
    if out.is_empty() {
        return;
    }
    let start = start_pos % buffer.len();
    let first = (buffer.len() - start).min(out.len());
    let (prefix, rest) = out.split_at_mut(first);
    prefix.copy_from_slice(&buffer[start..start + first]);
    for chunk in rest.chunks_mut(buffer.len()) {
        chunk.copy_from_slice(&buffer[..chunk.len()]);
    }
}

fn read_recent<T: Copy>(buffer: &[T], write_pos: usize, out: &mut [T]) -> usize {
    let count = out.len().min(write_pos).min(buffer.len());
    copy_ring_range(buffer, write_pos - count, &mut out[..count]);
    count
}

/// A fixed-size circular buffer for audio samples.
///
/// The writer pushes samples sequentially. The reader can read any
/// historical window without consuming data.
pub struct RingBuffer {
    buffer: Vec<f32>,
    /// Monotonically increasing. Index into buffer is `write_pos % capacity`.
    write_pos: AtomicUsize,
    // Mipmap Level 1: min/max per BLOCK_SIZE samples
    level1: Vec<MinMax>,
    level1_pos: usize,
    // Accumulator for current L1 block being built
    l1_accum: MinMax,
    l1_accum_count: usize,
    // Mipmap Level 2: min/max per SUPER_BLOCK_SIZE samples
    level2: Vec<MinMax>,
    level2_pos: usize,
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
            level1: vec![MinMax::default(); l1_cap],
            level1_pos: 0,
            l1_accum: MinMax {
                min: f32::MAX,
                max: f32::MIN,
            },
            l1_accum_count: 0,
            level2: vec![MinMax::default(); l2_cap],
            level2_pos: 0,
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
        self.buffer.len()
    }

    /// Push a slice of samples into the ring buffer.
    /// Called from the audio thread only (single writer).
    ///
    /// Raw storage retains at most `capacity()` samples. The mipmaps reduce the
    /// full input in block order using SIMD min/max.
    pub fn push(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let next_pos = self.total_written() + samples.len();
        let cap = self.buffer.len();
        let retained = &samples[samples.len().saturating_sub(cap)..];
        let start = (next_pos - retained.len()) % cap;

        let end = start + retained.len();
        if end <= cap {
            self.buffer[start..end].copy_from_slice(retained);
        } else {
            let first = cap - start;
            self.buffer[start..cap].copy_from_slice(&retained[..first]);
            self.buffer[..end - cap].copy_from_slice(&retained[first..]);
        }

        // Reduce the input in absolute mipmap blocks, independent of the raw ring wrap.
        let mut remaining = samples;
        while !remaining.is_empty() {
            let count = remaining.len().min(BLOCK_SIZE - self.l1_accum_count);
            let (chunk, rest) = remaining.split_at(count);
            remaining = rest;

            let mm = minmax_slice(chunk);
            self.l1_accum.min = self.l1_accum.min.min(mm.min);
            self.l1_accum.max = self.l1_accum.max.max(mm.max);
            self.l1_accum_count += count;

            if self.l1_accum_count == BLOCK_SIZE {
                // Flush L1 block
                let l1_idx = self.level1_pos % self.level1.len();
                self.level1[l1_idx] = self.l1_accum;
                self.level1_pos += 1;

                // Update L2 accumulator
                self.l2_accum.min = self.l2_accum.min.min(self.l1_accum.min);
                self.l2_accum.max = self.l2_accum.max.max(self.l1_accum.max);
                self.l2_block_count += 1;

                if self.l2_block_count >= BLOCKS_PER_SUPER {
                    let l2_idx = self.level2_pos % self.level2.len();
                    self.level2[l2_idx] = self.l2_accum;
                    self.level2_pos += 1;
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

        self.write_pos.store(next_pos, Ordering::Relaxed);
    }

    /// Read the most recent `count` samples into `out`.
    /// Returns the number of samples actually copied (may be less than
    /// `count` if fewer have been written).
    /// Called from the GUI thread (non-consuming reader).
    pub fn read_most_recent(&self, out: &mut [f32]) -> usize {
        read_recent(&self.buffer, self.total_written(), out)
    }

    /// Read samples from an absolute position range [start_pos, start_pos + count).
    /// Reads whatever is in the circular buffer at each index. Positions older
    /// than `capacity` contain audio from a previous wrap — valid for looping
    /// content. Pre-filled zeros cover positions never written.
    /// Returns the number of samples copied (always == out.len()).
    pub fn read_range(&self, start_pos: usize, out: &mut [f32]) -> usize {
        copy_ring_range(&self.buffer, start_pos, out);
        out.len()
    }

    /// Read the most recent `count` blocks from Level 1 mipmap.
    pub fn read_most_recent_l1(&self, out: &mut [MinMax]) -> usize {
        read_recent(&self.level1, self.level1_pos, out)
    }

    /// Read the most recent `count` blocks from Level 2 mipmap.
    pub fn read_most_recent_l2(&self, out: &mut [MinMax]) -> usize {
        read_recent(&self.level2, self.level2_pos, out)
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
    fn minmax_ignores_nan_lanes_without_losing_extrema() {
        let mut samples = [0.0; 49];
        samples[0] = -0.75;
        samples[1] = 0.875;
        samples[16] = f32::NAN;
        samples[33] = f32::NAN;
        samples[48] = f32::NAN;
        let result = minmax_slice(&samples);
        assert_eq!((result.min, result.max), (-0.75, 0.875));
    }

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

        let mut out = [-1.0; 5];
        let n = rb.read_most_recent(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out, [1.0, 2.0, -1.0, -1.0, -1.0]);
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
    fn test_read_range_beyond_written_returns_prefill() {
        let mut rb = RingBuffer::new(1024);
        rb.push(&[1.0, 2.0, 3.0]);

        let mut out = [0.0f32; 3];
        let n = rb.read_range(2, &mut out);
        // pos 2 is valid (3.0), pos 3 and 4 are pre-filled zeros
        assert_eq!(n, 3);
        assert_eq!(out[0], 3.0);
        assert_eq!(out[1], 0.0); // pre-fill zero
        assert_eq!(out[2], 0.0); // pre-fill zero
    }

    #[test]
    fn test_read_range_overwritten_returns_current_data() {
        let mut rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]); // 1,2 overwritten by 5,6

        let mut out = [0.0f32; 2];
        let n = rb.read_range(0, &mut out);
        assert_eq!(n, 2);
        // Positions 0,1 in the circular buffer now contain 5.0, 6.0
        assert_eq!(out[0], 5.0);
        assert_eq!(out[1], 6.0);
    }

    #[test]
    fn read_range_copies_through_multiple_wraps() {
        let mut rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        let mut out = [0.0; 11];
        assert_eq!(rb.read_range(3, &mut out), out.len());
        assert_eq!(out, [4.0, 5.0, 6.0, 3.0, 4.0, 5.0, 6.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(rb.read_range(3, &mut []), 0);
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
    fn push_larger_than_two_turns_keeps_latest_raw_samples_and_mipmap_blocks() {
        let mut rb = RingBuffer::new(BLOCK_SIZE);
        rb.push(&[2.0]);
        let mut input = [0.0; BLOCK_SIZE * 3];
        input[BLOCK_SIZE - 1..2 * BLOCK_SIZE - 1].fill(1.0);
        input[2 * BLOCK_SIZE - 1..].fill(3.0);
        input[2 * BLOCK_SIZE - 1] = 2.0;
        rb.push(&input);

        let mut raw = [0.0; BLOCK_SIZE];
        assert_eq!(rb.total_written(), 3 * BLOCK_SIZE + 1);
        assert_eq!(rb.read_most_recent(&mut raw), raw.len());
        assert_eq!(raw, [3.0; BLOCK_SIZE]);
        let mut mipmap = [MinMax::default(); 2];
        assert_eq!(rb.read_most_recent_l1(&mut mipmap), 2);
        assert_eq!(
            mipmap.map(|pair| (pair.min, pair.max)),
            [(1.0, 1.0), (2.0, 3.0)]
        );
    }

    #[test]
    fn recent_mipmaps_follow_absolute_blocks_across_ring_wraps() {
        let capacity = SUPER_BLOCK_SIZE + 19;
        let mut rb = RingBuffer::new(capacity);
        let mut input: Vec<f32> = (0..SUPER_BLOCK_SIZE * 5 + 13)
            .map(|i| if i % 2 == 0 { i as f32 } else { -(i as f32) })
            .collect();
        input[SUPER_BLOCK_SIZE * 4 + 20] = f32::INFINITY;
        input[SUPER_BLOCK_SIZE * 4 + 21] = f32::NEG_INFINITY;
        input[SUPER_BLOCK_SIZE * 4 + 22] = f32::NAN;

        for samples in input.chunks(73) {
            rb.push(samples);
        }

        for width in [BLOCK_SIZE, SUPER_BLOCK_SIZE] {
            let count = capacity / width + 1;
            let mut out = vec![MinMax::default(); count + 1];
            let n = if width == BLOCK_SIZE {
                rb.read_most_recent_l1(&mut out)
            } else {
                rb.read_most_recent_l2(&mut out)
            };
            assert_eq!(n, count);
            let blocks = input.chunks_exact(width);
            let expected = blocks.skip(input.len() / width - n).map(|samples| {
                samples
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(min, max), &sample| {
                        (min.min(sample), max.max(sample))
                    })
            });
            assert!(
                out[..n]
                    .iter()
                    .map(|pair| (pair.min, pair.max))
                    .eq(expected)
            );
            assert_eq!((out[n].min, out[n].max), (0.0, 0.0));
        }
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
