//! Immutable waveform snapshots and the builder that produces them.
//!
//! SnapshotBuilder is the ONLY component that reads from the shared store.
//! It produces immutable WaveSnapshots for the renderer.

use crate::ring_buffer::MinMax;
use crate::store;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ── Per-slot bar latch for stable rb_start ──────────────────────────────────
//
// The DAW's PPQ and sample clocks have per-buffer jitter, causing rb_start
// to shift by ±1 audio buffer within a bar. We latch rb_start at each bar
// boundary and hold it for the bar's duration, eliminating the jitter.

struct BarLatch {
    /// Latched rb_start for the current bar.
    rb_start: AtomicUsize,
    /// Last observed playhead fraction (0.0-1.0 packed as u32 fixed-point).
    /// When frac < last_frac, a new bar has started.
    last_frac_u32: AtomicUsize,
}

impl BarLatch {
    const fn new() -> Self {
        Self {
            rb_start: AtomicUsize::new(0),
            last_frac_u32: AtomicUsize::new(0),
        }
    }

    /// Update the latch. Returns the stable rb_start to use.
    fn update(&self, computed_rb_start: usize, frac: f64) -> usize {
        let frac_u32 = (frac * 1_000_000.0) as usize;
        let prev_frac = self.last_frac_u32.load(Ordering::Relaxed);

        if frac_u32 < prev_frac || prev_frac == 0 {
            // New bar boundary (frac wrapped) or first frame — latch the new rb_start
            self.rb_start.store(computed_rb_start, Ordering::Relaxed);
            self.last_frac_u32.store(frac_u32, Ordering::Relaxed);
            computed_rb_start
        } else {
            self.last_frac_u32.store(frac_u32, Ordering::Relaxed);
            self.rb_start.load(Ordering::Relaxed)
        }
    }
}

static BAR_LATCHES: [BarLatch; store::MAX_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const LATCH_INIT: BarLatch = BarLatch::new();
    [LATCH_INIT; store::MAX_SLOTS]
};

// ── Per-slot hold buffer for Hold display mode ────────────────────────────
//
// In Hold mode, the renderer shows the last *complete* bar. During the
// current bar, audio is accumulated in the back buffer (current sweep read).
// At each bar boundary (frac wraps), the back buffer data becomes the front
// buffer. The renderer always displays the front buffer.

struct HoldBuffer {
    /// Front buffer: last complete bar, wrapped in Arc for cheap cloning.
    /// `None` until the first bar completes.
    front: Mutex<Option<Arc<Vec<Vec<f32>>>>>,
    /// Back buffer: accumulates the current bar's most recent read.
    /// At bar boundary, back is wrapped in Arc and moved to front.
    back: Mutex<Vec<Vec<f32>>>,
    /// Last observed frac for bar-boundary detection.
    last_frac: AtomicUsize,
}

impl HoldBuffer {
    const fn new() -> Self {
        Self {
            front: Mutex::new(None),
            back: Mutex::new(Vec::new()),
            last_frac: AtomicUsize::new(0),
        }
    }

    /// Update the hold buffer with the current frame's data.
    /// Returns an Arc to the front buffer (pointer-sized clone), or None.
    fn update(&self, current_data: &[Vec<f32>], frac: f64) -> Option<Arc<Vec<Vec<f32>>>> {
        let frac_u = (frac * 1_000_000.0) as usize;
        let prev_frac = self.last_frac.load(Ordering::Relaxed);

        // Lock back buffer once for both the bar-boundary promotion and the current write
        if let Ok(mut back) = self.back.lock() {
            if frac_u < prev_frac && prev_frac > 0 {
                // Bar boundary crossed — wrap back in Arc and promote to front.
                if !back.is_empty() {
                    let completed = std::mem::take(&mut *back);
                    if let Ok(mut front) = self.front.lock() {
                        *front = Some(Arc::new(completed));
                    }
                }
            }

            // Copy current data into the back buffer, reusing allocations
            if back.len() != current_data.len() {
                back.resize_with(current_data.len(), Vec::new);
            }
            for (dst, src) in back.iter_mut().zip(current_data.iter()) {
                if dst.len() != src.len() {
                    dst.resize(src.len(), 0.0);
                }
                dst.copy_from_slice(src);
            }
        }
        self.last_frac.store(frac_u, Ordering::Relaxed);

        // Return Arc clone of front (pointer-sized, no data copy)
        self.front.lock().ok().and_then(|f| f.as_ref().map(Arc::clone))
    }
}

static HOLD_BUFFERS: [HoldBuffer; store::MAX_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const HOLD_INIT: HoldBuffer = HoldBuffer::new();
    [HOLD_INIT; store::MAX_SLOTS]
};

/// Immutable snapshot of one track's state for rendering.
#[derive(Clone)]
pub struct WaveSnapshot {
    pub slot_index: usize,
    pub track_name: String,
    pub display_color: u32,
    pub num_channels: usize,
    pub group: u8,
    pub is_active: bool,
    pub solo: bool,
    pub mute: bool,

    /// Audio data: either raw samples or flattened min/max pairs.
    /// For raw: `audio_data[ch][sample]`
    /// For decimated: `audio_data[ch][i*2] = min, audio_data[ch][i*2+1] = max`
    pub audio_data: Vec<Vec<f32>>,
    /// Which mipmap level was used (0=raw, 1=L1, 2=L2).
    pub mipmap_level: u8,
    /// Number of data points (samples for L0, blocks for L1/L2).
    pub data_points: usize,
    /// Cache invalidation: total_written at read time.
    pub data_version: u64,

    // Beat sync info
    pub is_playing: bool,
    pub bpm: f64,
    pub beats_per_bar: u32,
    pub samples_per_bar: f64,
    pub ppq_position_in_bar: f64,

    // Pre-computed
    pub mono_mix: Vec<f32>,
    pub peak_amplitude: f32,
    pub peak_db: f32,
}

/// Compute mono mix by averaging all channels.
pub fn compute_mono_mix(audio_data: &[Vec<f32>]) -> Vec<f32> {
    if audio_data.is_empty() || audio_data[0].is_empty() {
        return Vec::new();
    }
    let len = audio_data[0].len();
    let num_ch = audio_data.len() as f32;
    let mut mono = vec![0.0f32; len];
    for ch in audio_data {
        for (i, &s) in ch.iter().enumerate() {
            if i < len {
                mono[i] += s;
            }
        }
    }
    for s in &mut mono {
        *s /= num_ch;
    }
    mono
}

/// Compute peak amplitude and dB across all channels.
pub fn compute_peak(audio_data: &[Vec<f32>]) -> (f32, f32) {
    let mut peak = 0.0f32;
    for ch in audio_data {
        for &s in ch {
            let abs = s.abs();
            if abs > peak {
                peak = abs;
            }
        }
    }
    let db = if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        -96.0
    };
    (peak, db)
}

/// Build snapshots for free (non-beat-sync) mode.
///
/// - `group`: which group to filter for
/// - `timebase_ms`: timebase in milliseconds
/// - `sample_rate`: current sample rate
/// - `decimation`: max number of output data points
/// - `mix_to_mono`: whether to compute mono mix
pub fn build_snapshots_free(
    group: u32,
    timebase_ms: f32,
    sample_rate: f32,
    decimation: usize,
    mix_to_mono: bool,
) -> Vec<WaveSnapshot> {
    let (slots, slot_count) = store::active_slots_in_group(group);
    let total_samples = ((timebase_ms / 1000.0) * sample_rate).round() as usize;
    let level = crate::ring_buffer::RingBuffer::select_level(if total_samples > decimation {
        total_samples / decimation
    } else {
        1
    });

    let mut snapshots = Vec::with_capacity(slot_count);

    for &idx in &slots[..slot_count] {
        let s = store::slot(idx);

        // Read metadata
        let track_name = s
            .metadata
            .track_name
            .lock()
            .map(|n| n.clone())
            .unwrap_or_default();
        let display_color = s.metadata.display_color.load(Ordering::Relaxed);
        let num_channels = s.metadata.num_channels.load(Ordering::Relaxed) as usize;
        let grp = s.metadata.group.load(Ordering::Relaxed) as u8;
        let solo = s.metadata.solo.load(Ordering::Relaxed);
        let mute = s.metadata.mute.load(Ordering::Relaxed);

        // Read playhead
        let is_playing = s.playhead.is_playing.load(Ordering::Relaxed);
        let bpm = f64::from_bits(s.playhead.bpm.load(Ordering::Relaxed));
        let beats_per_bar = s.playhead.time_sig_num.load(Ordering::Relaxed);
        let _time_sig_den = s.playhead.time_sig_den.load(Ordering::Relaxed);
        let spb = if bpm > 0.0 {
            (60.0 / bpm) * sample_rate as f64
        } else {
            0.0
        };
        let samples_per_bar = spb * beats_per_bar as f64;
        let ppq = f64::from_bits(s.playhead.ppq_position.load(Ordering::Relaxed));
        let bar_start = f64::from_bits(s.playhead.bar_start_ppq.load(Ordering::Relaxed));
        let ppq_in_bar = ppq - bar_start;

        // Read audio data — use try_read() to avoid blocking the GUI thread
        // if the audio thread holds the write lock. Skip this slot if contended.
        let guard = match s.buffers.try_read() {
            Ok(g) => g,
            Err(_) => continue,
        };
        let mut audio_data = Vec::new();
        let mut data_version = 0u64;
        let mut data_points = 0;

        if let Some(bufs) = guard.as_ref() {
            for (ch, buf) in bufs.iter().enumerate().take(num_channels) {
                if ch == 0 {
                    data_version = buf.total_written() as u64;
                }
                match level {
                    0 => {
                        let mut out = vec![0.0f32; total_samples];
                        let n = buf.read_most_recent(&mut out);
                        out.truncate(n);
                        data_points = n;
                        audio_data.push(out);
                    }
                    1 => {
                        let num_blocks =
                            total_samples / crate::ring_buffer::BLOCK_SIZE;
                        let mut blocks = vec![MinMax::default(); num_blocks];
                        let n = buf.read_most_recent_l1(&mut blocks);
                        let mut flat = Vec::with_capacity(n * 2);
                        for b in &blocks[..n] {
                            flat.push(b.min);
                            flat.push(b.max);
                        }
                        data_points = n;
                        audio_data.push(flat);
                    }
                    _ => {
                        let num_blocks =
                            total_samples / crate::ring_buffer::SUPER_BLOCK_SIZE;
                        let mut blocks = vec![MinMax::default(); num_blocks];
                        let n = buf.read_most_recent_l2(&mut blocks);
                        let mut flat = Vec::with_capacity(n * 2);
                        for b in &blocks[..n] {
                            flat.push(b.min);
                            flat.push(b.max);
                        }
                        data_points = n;
                        audio_data.push(flat);
                    }
                }
            }
        }
        drop(guard);

        let mono_mix = if mix_to_mono && level == 0 {
            compute_mono_mix(&audio_data)
        } else {
            Vec::new()
        };
        let (peak_amplitude, peak_db) = compute_peak(&audio_data);

        snapshots.push(WaveSnapshot {
            slot_index: idx,
            track_name,
            display_color,
            num_channels,
            group: grp,
            is_active: true,
            solo,
            mute,
            audio_data,
            mipmap_level: level,
            data_points,
            data_version,
            is_playing,
            bpm,
            beats_per_bar,
            samples_per_bar,
            ppq_position_in_bar: ppq_in_bar,
            mono_mix,
            peak_amplitude,
            peak_db,
        });
    }

    snapshots
}

/// Build snapshots for beat-sync mode.
///
/// Uses PPQ deltas relative to ring_buffer_pos for beat-aligned windows.
/// This works correctly across DAW loops because:
/// - We only use the PPQ *delta* (current_ppq − window_start_ppq),
///   which is always 0..window_ppq regardless of transport wraps.
/// - We anchor to ring_buffer_pos, which is monotonic and never resets.
///
/// - `group`: which group to filter for
/// - `sync_bars`: number of bars to display (0.25, 0.5, 1.0, 2.0, 4.0)
/// - `sample_rate`: current sample rate
/// - `mix_to_mono`: whether to compute mono mix
/// - `hold_mode`: if true, display the last complete bar instead of sweep
pub fn build_snapshots_beat_sync(
    group: u32,
    sync_bars: f64,
    sample_rate: f32,
    mix_to_mono: bool,
    hold_mode: bool,
) -> Vec<WaveSnapshot> {
    let (slots, slot_count) = store::active_slots_in_group(group);
    let mut snapshots = Vec::with_capacity(slot_count);

    for &idx in &slots[..slot_count] {
        let s = store::slot(idx);

        // Read metadata (same as free mode)
        let track_name = s
            .metadata
            .track_name
            .lock()
            .map(|n| n.clone())
            .unwrap_or_default();
        let display_color = s.metadata.display_color.load(Ordering::Relaxed);
        let num_channels = s.metadata.num_channels.load(Ordering::Relaxed) as usize;
        let grp = s.metadata.group.load(Ordering::Relaxed) as u8;
        let solo = s.metadata.solo.load(Ordering::Relaxed);
        let mute = s.metadata.mute.load(Ordering::Relaxed);

        let is_playing = s.playhead.is_playing.load(Ordering::Relaxed);
        let bpm = f64::from_bits(s.playhead.bpm.load(Ordering::Relaxed));
        let beats_per_bar = s.playhead.time_sig_num.load(Ordering::Relaxed);
        let ppq = f64::from_bits(s.playhead.ppq_position.load(Ordering::Relaxed));
        let bar_start = f64::from_bits(s.playhead.bar_start_ppq.load(Ordering::Relaxed));
        let ppq_in_bar = ppq - bar_start;
        let spb = if bpm > 0.0 {
            (60.0 / bpm) * sample_rate as f64
        } else {
            0.0
        };
        let samples_per_bar = spb * beats_per_bar as f64;

        let tm_snap = s.time_mapping.snapshot();

        // Compute beat-aligned window using PPQ delta approach.
        // beat_aligned_window returns (rb_start, window_len, playhead_fraction)
        // in ring buffer space.
        let window = if is_playing {
            crate::time_mapping::beat_aligned_window(&tm_snap, sync_bars, beats_per_bar)
        } else {
            None
        };

        // Use try_read() to avoid blocking — skip slot if contended.
        let guard = match s.buffers.try_read() {
            Ok(g) => g,
            Err(_) => continue,
        };
        let mut audio_data = Vec::new();
        let mut data_version = 0u64;
        let mut data_points = 0;

        if let (Some(bufs), Some((computed_rb_start, window_len, playhead_fraction))) =
            (guard.as_ref(), window)
        {
            // Latch rb_start at bar boundaries to eliminate per-buffer PPQ jitter
            let rb_start = BAR_LATCHES[idx].update(computed_rb_start, playhead_fraction);

            // Read the full beat-aligned window from the ring buffer
            let mut raw_data = Vec::with_capacity(num_channels);
            for (ch, buf) in bufs.iter().enumerate().take(num_channels) {
                if ch == 0 {
                    data_version = buf.total_written() as u64;
                }
                let mut out = vec![0.0f32; window_len];
                buf.read_range(rb_start, &mut out);
                raw_data.push(out);
            }

            if hold_mode {
                // Hold mode: use the double buffer to show the last complete bar
                if let Some(front_arc) = HOLD_BUFFERS[idx].update(&raw_data, playhead_fraction) {
                    data_points = front_arc.first().map_or(0, |ch| ch.len());
                    // Unwrap the Arc if we're the only holder, otherwise clone
                    audio_data = Arc::try_unwrap(front_arc).unwrap_or_else(|arc| (*arc).clone());
                } else {
                    // No complete bar yet — show partial data with sweep mask
                    // so user sees something while waiting for first bar
                    let end_valid =
                        (playhead_fraction * window_len as f64).round() as usize;
                    let end_valid = end_valid.min(window_len);
                    let fade_len = 16.min(window_len.saturating_sub(end_valid));
                    for out in &mut raw_data {
                        for i in 0..fade_len {
                            let fi = end_valid + i;
                            if fi < window_len {
                                out[fi] *= 1.0 - (i as f32 + 1.0) / (fade_len as f32 + 1.0);
                            }
                        }
                        for slot in out.iter_mut().take(window_len).skip(end_valid + fade_len) {
                            *slot = 0.0;
                        }
                    }
                    data_points = window_len;
                    audio_data = raw_data;
                }
            } else {
                // Sweep mode (original behavior): mask stale data ahead of playhead
                let end_valid =
                    (playhead_fraction * window_len as f64).round() as usize;
                let end_valid = end_valid.min(window_len);
                let fade_len = 16.min(window_len.saturating_sub(end_valid));
                for out in &mut raw_data {
                    for i in 0..fade_len {
                        let fi = end_valid + i;
                        if fi < window_len {
                            out[fi] *= 1.0 - (i as f32 + 1.0) / (fade_len as f32 + 1.0);
                        }
                    }
                    for slot in out.iter_mut().take(window_len).skip(end_valid + fade_len) {
                        *slot = 0.0;
                    }
                }
                data_points = window_len;
                audio_data = raw_data;
            }
        }
        drop(guard);

        let mono_mix = if mix_to_mono {
            compute_mono_mix(&audio_data)
        } else {
            Vec::new()
        };
        let (peak_amplitude, peak_db) = compute_peak(&audio_data);

        snapshots.push(WaveSnapshot {
            slot_index: idx,
            track_name,
            display_color,
            num_channels,
            group: grp,
            is_active: true,
            solo,
            mute,
            audio_data,
            mipmap_level: 0,
            data_points,
            data_version,
            is_playing,
            bpm,
            beats_per_bar,
            samples_per_bar,
            ppq_position_in_bar: ppq_in_bar,
            mono_mix,
            peak_amplitude,
            peak_db,
        });
    }

    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── HoldBuffer tests ───────────────────────────────────────────────

    #[test]
    fn test_hold_buffer_no_data_until_bar_completes() {
        let hb = HoldBuffer::new();
        let bar_data = vec![vec![1.0, 2.0, 3.0]];
        // First frame at frac=0.5 — no previous bar yet
        let result = hb.update(&bar_data, 0.5);
        assert!(result.is_none());
    }

    #[test]
    fn test_hold_buffer_promotes_back_at_bar_boundary() {
        let hb = HoldBuffer::new();
        let bar1 = vec![vec![1.0, 2.0, 3.0]];
        let bar2 = vec![vec![4.0, 5.0, 6.0]];

        // Simulate bar 1 playing through
        hb.update(&bar1, 0.1);
        hb.update(&bar1, 0.5);
        hb.update(&bar1, 0.9); // back buffer now has bar1

        // Bar boundary: frac wraps to 0.05
        let result = hb.update(&bar2, 0.05);
        // Should return bar1 (the PREVIOUS bar), not bar2
        assert!(result.is_some());
        let front = result.unwrap();
        assert_eq!(front[0], vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_hold_buffer_front_stable_within_bar() {
        let hb = HoldBuffer::new();
        let bar1 = vec![vec![1.0, 2.0]];
        let bar2_early = vec![vec![3.0, 4.0]];
        let bar2_mid = vec![vec![5.0, 6.0]];

        // Complete bar 1
        hb.update(&bar1, 0.1);
        hb.update(&bar1, 0.9);
        hb.update(&bar2_early, 0.05); // boundary → bar1 promoted to front

        // Mid-bar reads should return stable front (bar1)
        let r1 = hb.update(&bar2_mid, 0.3).unwrap();
        let r2 = hb.update(&bar2_mid, 0.7).unwrap();
        assert_eq!(r1[0], vec![1.0, 2.0]);
        assert_eq!(r2[0], vec![1.0, 2.0]);
    }

    #[test]
    fn test_hold_buffer_uses_last_read_before_boundary() {
        let hb = HoldBuffer::new();
        let early = vec![vec![0.1]];
        let mid = vec![vec![0.5]];
        let late = vec![vec![0.9]]; // this should be promoted — it's the last read
        let new_bar = vec![vec![1.0]];

        hb.update(&early, 0.1);
        hb.update(&mid, 0.5);
        hb.update(&late, 0.95); // back buffer = late

        let result = hb.update(&new_bar, 0.02); // boundary
        let front = result.unwrap();
        // Front should be the LATE data (last back before swap), not new_bar
        assert_eq!(front[0], vec![0.9]);
    }

    // ── Existing tests ────────────────────────────────────────────────

    #[test]
    fn test_compute_mono_mix_stereo() {
        let data = vec![vec![1.0, 2.0, 3.0], vec![3.0, 4.0, 5.0]];
        let mono = compute_mono_mix(&data);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 2.0).abs() < 0.001);
        assert!((mono[1] - 3.0).abs() < 0.001);
        assert!((mono[2] - 4.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_mono_mix_empty() {
        let data: Vec<Vec<f32>> = vec![];
        let mono = compute_mono_mix(&data);
        assert!(mono.is_empty());
    }

    #[test]
    fn test_compute_peak_basic() {
        let data = vec![vec![0.5, -0.8, 0.3], vec![0.1, 0.2, -0.9]];
        let (peak, db) = compute_peak(&data);
        assert!((peak - 0.9).abs() < 0.001);
        assert!((db - 20.0 * 0.9f32.log10()).abs() < 0.01);
    }

    #[test]
    fn test_compute_peak_silence() {
        let data = vec![vec![0.0, 0.0]];
        let (peak, db) = compute_peak(&data);
        assert_eq!(peak, 0.0);
        assert_eq!(db, -96.0);
    }

    #[test]
    fn test_build_snapshots_free_empty_store() {
        // No active slots in group 15
        let snaps = build_snapshots_free(15, 1000.0, 48000.0, 2048, false);
        assert!(snaps.is_empty());
    }

    #[test]
    fn test_build_snapshots_free_with_data() {
        // Serialize with store tests since they share global state
        let _g = crate::store::tests::TEST_LOCK.lock().unwrap();
        store::reset_slot(0);
        let idx = store::acquire_slot(100).unwrap();
        store::init_buffers(idx, 2, 48000.0);
        store::slot(idx).metadata.group.store(0, Ordering::Relaxed);

        // Push some audio
        {
            let guard = store::slot(idx).buffers.read().unwrap();
            if let Some(_bufs) = guard.as_ref() {
                // Can't push through immutable ref, need mutable access
                // In the real plugin, the audio thread has &mut via its cached pointer
            }
        }

        let snaps = build_snapshots_free(0, 100.0, 48000.0, 2048, true);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].slot_index, idx);
        assert_eq!(snaps[0].num_channels, 2);

        store::release_slot(idx, 100);
    }
}
