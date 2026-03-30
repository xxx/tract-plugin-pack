//! Immutable waveform snapshots and the builder that produces them.
//!
//! SnapshotBuilder is the ONLY component that reads from the shared store.
//! It produces immutable WaveSnapshots for the renderer.

use crate::ring_buffer::MinMax;
use crate::store;
use crate::time_mapping;
use std::sync::atomic::Ordering;

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
    let slots = store::active_slots_in_group(group);
    let total_samples = ((timebase_ms / 1000.0) * sample_rate).round() as usize;
    let level = crate::ring_buffer::RingBuffer::select_level(if total_samples > decimation {
        total_samples / decimation
    } else {
        1
    });

    let mut snapshots = Vec::with_capacity(slots.len());

    for &idx in &slots {
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

        // Read audio data
        let guard = s.buffers.read().unwrap();
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
                        let mut out = vec![0.0f32; total_samples.min(decimation)];
                        let n = buf.read_most_recent(&mut out);
                        out.truncate(n);
                        data_points = n;
                        audio_data.push(out);
                    }
                    1 => {
                        let num_blocks =
                            (total_samples / crate::ring_buffer::BLOCK_SIZE).min(decimation);
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
                            (total_samples / crate::ring_buffer::SUPER_BLOCK_SIZE).min(decimation);
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
/// - `group`: which group to filter for
/// - `sync_bars`: number of bars to display (0.25, 0.5, 1.0, 2.0, 4.0)
/// - `sample_rate`: current sample rate
/// - `decimation`: max output data points
/// - `mix_to_mono`: whether to compute mono mix
pub fn build_snapshots_beat_sync(
    group: u32,
    sync_bars: f64,
    sample_rate: f32,
    decimation: usize,
    mix_to_mono: bool,
) -> Vec<WaveSnapshot> {
    let slots = store::active_slots_in_group(group);
    let mut snapshots = Vec::with_capacity(slots.len());

    for &idx in &slots {
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

        // Compute beat-aligned window
        let window = if is_playing {
            time_mapping::beat_aligned_window(&tm_snap, sync_bars, beats_per_bar)
        } else {
            None
        };

        let guard = s.buffers.read().unwrap();
        let mut audio_data = Vec::new();
        let mut data_version = 0u64;
        let mut data_points = 0;

        if let (Some(bufs), Some((start_sample, window_len))) = (guard.as_ref(), window) {
            let read_count = window_len.min(decimation);

            // Convert DAW transport positions to ring buffer positions.
            // The time mapping stores the ring buffer write_pos that corresponds
            // to the DAW's current_sample_pos. We use this anchor to translate.
            let rb_pos = tm_snap.ring_buffer_pos as i64;
            let transport_pos = tm_snap.current_sample_pos;
            let rb_start = rb_pos - (transport_pos - start_sample);

            // For beat sync, always read raw samples and decimate on the draw side
            if rb_start >= 0 {
                for (ch, buf) in bufs.iter().enumerate().take(num_channels) {
                    if ch == 0 {
                        data_version = buf.total_written() as u64;
                    }
                    let mut out = vec![0.0f32; read_count];
                    buf.read_range(rb_start as usize, &mut out);

                    // Mask stale data beyond current playhead with 16-sample fade.
                    // end_valid = how many samples from window start to current playhead.
                    let end_valid =
                        ((transport_pos - start_sample) as usize).min(read_count);
                    let fade_len = 16.min(read_count - end_valid);
                    for i in 0..fade_len {
                        let idx = end_valid + i;
                        if idx < read_count {
                            let fade = 1.0 - (i as f32 / fade_len as f32);
                            out[idx] *= fade;
                        }
                    }
                    for slot in out.iter_mut().take(read_count).skip(end_valid + fade_len) {
                        *slot = 0.0;
                    }
                    data_points = read_count;
                    audio_data.push(out);
                }
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
