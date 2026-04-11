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
    /// rb_start from the previous bar (saved at each bar boundary).
    prev_rb_start: AtomicUsize,
    /// Last observed playhead fraction (0.0-1.0 packed as u32 fixed-point).
    /// When frac < last_frac, a new bar has started.
    last_frac_u32: AtomicUsize,
}

impl BarLatch {
    const fn new() -> Self {
        Self {
            rb_start: AtomicUsize::new(0),
            prev_rb_start: AtomicUsize::new(0),
            last_frac_u32: AtomicUsize::new(0),
        }
    }

    /// Update the latch. Returns `(stable_rb_start, bar_boundary_occurred)`.
    fn update(&self, computed_rb_start: usize, frac: f64) -> (usize, bool) {
        let frac_u32 = (frac * 1_000_000.0) as usize;
        let prev_frac = self.last_frac_u32.load(Ordering::Relaxed);

        // A real bar boundary has frac dropping from >0.5 to <0.5.
        // Reject small backward jumps caused by torn atomic reads
        // (ppq and rb_pos from different audio callbacks).
        let is_real_wrap = frac_u32 < prev_frac && prev_frac > 500_000;
        let latched = self.rb_start.load(Ordering::Relaxed);

        if is_real_wrap || prev_frac == 0 {
            // New bar boundary or first frame — save old rb_start, latch the new one
            self.prev_rb_start.store(latched, Ordering::Relaxed);
            self.rb_start.store(computed_rb_start, Ordering::Relaxed);
            self.last_frac_u32.store(frac_u32, Ordering::Relaxed);
            (computed_rb_start, is_real_wrap)
        } else {
            self.last_frac_u32.store(frac_u32, Ordering::Relaxed);
            (latched, false)
        }
    }

    /// Get the rb_start from the previous bar (set at the last bar boundary).
    fn prev_rb_start(&self) -> usize {
        self.prev_rb_start.load(Ordering::Relaxed)
    }
}

static BAR_LATCHES: [BarLatch; store::MAX_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const LATCH_INIT: BarLatch = BarLatch::new();
    [LATCH_INIT; store::MAX_SLOTS]
};

// ── Per-slot stale detection ─────────────────────────────────────────────
//
// Each slot has a heartbeat counter incremented by process() every buffer.
// If the heartbeat hasn't changed between GUI frames, the slot's plugin
// is no longer running (track deleted, plugin removed, etc). Skip it.

static LAST_HEARTBEATS: [AtomicUsize; store::MAX_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const HB_INIT: AtomicUsize = AtomicUsize::new(0);
    [HB_INIT; store::MAX_SLOTS]
};

/// Consecutive stale frame count per slot.
static STALE_COUNTS: [AtomicUsize; store::MAX_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const SC_INIT: AtomicUsize = AtomicUsize::new(0);
    [SC_INIT; store::MAX_SLOTS]
};

/// Number of consecutive stale frames before a slot is considered dead.
/// At 60fps, 30 frames = 0.5 seconds of silence.
const STALE_THRESHOLD: usize = 30;

/// Check if a slot is stale (heartbeat hasn't changed for STALE_THRESHOLD frames).
fn is_slot_stale(idx: usize) -> bool {
    let current = store::slot(idx).heartbeat.load(Ordering::Relaxed) as usize;
    let prev = LAST_HEARTBEATS[idx].swap(current, Ordering::Relaxed);
    if current == prev && current != 0 {
        let count = STALE_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
        count >= STALE_THRESHOLD
    } else {
        STALE_COUNTS[idx].store(0, Ordering::Relaxed);
        false
    }
}

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

    /// Update the hold buffer with only the valid portion of the current frame's data.
    ///
    /// `valid_count` is the number of samples from the start of `current_data` that
    /// are behind the ring buffer write head (i.e., real audio, not stale/unwritten).
    /// Only `[0..valid_count]` is copied into the back buffer each frame; the rest
    /// accumulates from previous frames as the playhead advances through the bar.
    ///
    /// Returns an Arc to the front buffer (pointer-sized clone), or None.
    fn update(&self, current_data: &[Vec<f32>], frac: f64, valid_count: usize) -> Option<Arc<Vec<Vec<f32>>>> {
        let frac_u = (frac * 1_000_000.0) as usize;
        let prev_frac = self.last_frac.load(Ordering::Relaxed);

        // Lock back buffer once for both the bar-boundary promotion and the current write
        if let Ok(mut back) = self.back.lock() {
            if frac_u < prev_frac && prev_frac > 500_000 {
                // Bar boundary crossed (frac dropped from >0.5 to <0.5) —
                // wrap back in Arc and promote to front.
                if !back.is_empty() {
                    let completed = std::mem::take(&mut *back);
                    if let Ok(mut front) = self.front.lock() {
                        *front = Some(Arc::new(completed));
                    }
                }
                // Clear new back buffer to zeros so old bar data doesn't
                // persist into the next bar's accumulation.
                let window_len = current_data.first().map_or(0, |ch| ch.len());
                back.resize_with(current_data.len(), Vec::new);
                for dst in back.iter_mut() {
                    dst.resize(window_len, 0.0);
                    dst.fill(0.0);
                }
            }

            // Copy only the valid portion into the back buffer
            if back.len() != current_data.len() {
                back.resize_with(current_data.len(), Vec::new);
            }
            for (dst, src) in back.iter_mut().zip(current_data.iter()) {
                if dst.len() != src.len() {
                    dst.resize(src.len(), 0.0);
                }
                let n = valid_count.min(src.len()).min(dst.len());
                dst[..n].copy_from_slice(&src[..n]);
            }
        }
        self.last_frac.store(frac_u, Ordering::Relaxed);

        // Return Arc clone of front (pointer-sized, no data copy)
        self.front.lock().ok().and_then(|f| f.as_ref().map(Arc::clone))
    }

    /// Promote a complete bar directly to the front buffer.
    ///
    /// Called at bar boundary when we have a fresh, complete read of the
    /// previous bar from the ring buffer. Bypasses the incremental
    /// accumulation — overwrites front immediately.
    ///
    /// Also resets `last_frac` to 0 so the next `update()` call doesn't
    /// re-detect the bar boundary and overwrite front with the (incomplete)
    /// back buffer.
    fn promote_complete(&self, complete_bar: Vec<Vec<f32>>) {
        if let Ok(mut front) = self.front.lock() {
            *front = Some(Arc::new(complete_bar));
        }
        // Clear back buffer so update() starts fresh for the new bar
        if let Ok(mut back) = self.back.lock() {
            for dst in back.iter_mut() {
                dst.fill(0.0);
            }
        }
        // Reset last_frac to 0 so update() doesn't see a bar boundary
        self.last_frac.store(0, Ordering::Relaxed);
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

impl WaveSnapshot {
    /// Return the sample value at a normalized x position `[0.0, 1.0]`.
    ///
    /// This samples ONE element of the underlying buffer regardless of
    /// mipmap level. It matches what the user sees only when the buffer
    /// has roughly the same size as the rendered pixel width; when the
    /// renderer further decimates (because the window is narrower than
    /// the buffer), a single-element readout can disagree with the
    /// rendered envelope by several dB. Use `peak_at_column` instead to
    /// align with the renderer's actual per-pixel-column output.
    ///
    /// Picks the appropriate buffer:
    /// - If `mix_to_mono` is set and `mono_mix` is populated (only at mipmap L0),
    ///   read from `mono_mix`.
    /// - At mipmap level 0 (raw), read `audio_data[0]` (first channel — stereo
    ///   is not distinguished here; the tooltip only reports the first channel
    ///   unless mono mixing is enabled).
    /// - At mipmap level 1/2, `audio_data[0]` holds interleaved `(min, max)`
    ///   pairs — return whichever sample has the greater absolute magnitude so
    ///   the readout tracks the visible envelope peak.
    pub fn sample_at_normalized_x(&self, normalized_x: f32, mix_to_mono: bool) -> f32 {
        let n = self.data_points;
        if n == 0 {
            return 0.0;
        }
        let t = normalized_x.clamp(0.0, 1.0);
        // Clamp index to n - 1 to avoid off-by-one at t == 1.0
        let i = ((t * n as f32) as usize).min(n - 1);

        if mix_to_mono && !self.mono_mix.is_empty() {
            debug_assert!(
                self.mono_mix.len() >= n,
                "mono_mix length {} < data_points {}",
                self.mono_mix.len(),
                n
            );
            return self.mono_mix.get(i).copied().unwrap_or(0.0);
        }

        let Some(ch0) = self.audio_data.first() else {
            return 0.0;
        };

        if self.mipmap_level == 0 {
            ch0.get(i).copied().unwrap_or(0.0)
        } else {
            let lo = ch0.get(i * 2).copied().unwrap_or(0.0);
            let hi = ch0.get(i * 2 + 1).copied().unwrap_or(0.0);
            if lo.abs() > hi.abs() {
                lo
            } else {
                hi
            }
        }
    }

    /// Return the value visible at pixel column `col` of `num_cols`,
    /// matching what the renderer draws for the same column:
    ///
    /// - When `buf.len() > num_cols` (normal or decimated case), this is
    ///   the sign-preserving max-abs sample across the block of samples
    ///   the renderer assigns to `col` via `decimate_to_columns`.
    /// - When `buf.len() <= num_cols` (extreme zoom, renderer draws
    ///   line segments between samples), this is a linear interpolation
    ///   between the two adjacent samples at the cursor's pixel
    ///   position. Interpolation is done in sample space, not in
    ///   dB/pixel space — the renderer's segments are drawn in pixel
    ///   space so the two agree to within a fraction of a dB except at
    ///   zero-crossings.
    ///
    /// Sign is preserved so callers can still distinguish polarity.
    ///
    /// This is the function the cursor tooltip should use. For the raw
    /// "one element at this index" semantic, use `sample_at_normalized_x`.
    ///
    /// Precision note: column mapping uses integer arithmetic
    /// (`(col * n).div_ceil(num_cols)`). The renderer's
    /// `decimate_to_columns_into` uses f32 arithmetic, which begins to
    /// lose integer precision around `i * num_cols > 2^24` (~16.7M).
    /// The snapshot builder decimates raw data into L1/L2 mipmap levels
    /// well before any realistic combination of `data_points * num_cols`
    /// approaches that magnitude, so the two column mappings agree in
    /// practice. Tests `test_peak_at_column_matches_decimate_on_*`
    /// pin parity on representative small cases.
    pub fn peak_at_column(&self, col: usize, num_cols: usize, mix_to_mono: bool) -> f32 {
        if num_cols == 0 {
            return 0.0;
        }
        let buf: &[f32] = if mix_to_mono && !self.mono_mix.is_empty() {
            &self.mono_mix
        } else {
            match self.audio_data.first() {
                Some(ch) => ch,
                None => return 0.0,
            }
        };
        let n = buf.len();
        if n == 0 {
            return 0.0;
        }
        let col = col.min(num_cols - 1);

        // Sparse-samples path: when the buffer is smaller than the pixel
        // width the renderer draws line segments between adjacent samples.
        // The renderer places sample i at pixel `i * num_cols / n`
        // (equivalently, `i * step` where `step = w / n`), and draws a
        // line from there to sample i+1. At pixel `col`, the visible
        // value is therefore a linear interpolation between the two
        // samples bracketing `col * n / num_cols`. Beyond the last
        // sample the renderer draws nothing, so we return the last
        // sample value — which is also what the eye expects if the
        // cursor is past the end of the visible line.
        if n <= num_cols {
            if n == 1 {
                return buf[0];
            }
            let t = col as f32 * n as f32 / num_cols as f32;
            let i_lo_f = t.floor();
            let i_lo = (i_lo_f as usize).min(n - 1);
            if i_lo >= n - 1 {
                return buf[n - 1];
            }
            let frac = (t - i_lo_f).clamp(0.0, 1.0);
            return buf[i_lo] * (1.0 - frac) + buf[i_lo + 1] * frac;
        }

        // Dense-samples path: exact inverse of
        // `decimate_to_columns_into`'s `col = (i * num_cols / n) as usize`
        // mapping, in integer arithmetic. Sample i belongs to column c
        // iff `c * n <= i * num_cols < (c + 1) * n`, so:
        //   i_lo = ceil(col * n / num_cols)
        //   i_hi = ceil((col + 1) * n / num_cols)  (exclusive)
        //
        // At `col == num_cols - 1`, `((col + 1) * n).div_ceil(num_cols)`
        // equals `n` exactly, so no special case is needed for the last
        // column — `.min(n)` is belt-and-braces in case upstream math
        // ever rounds differently.
        let i_lo = (col * n).div_ceil(num_cols);
        let i_hi = ((col + 1) * n).div_ceil(num_cols).min(n);

        if i_lo >= i_hi {
            // Should be unreachable given `n > num_cols` above, but
            // degrade gracefully rather than panic.
            return buf.get(i_lo.min(n - 1)).copied().unwrap_or(0.0);
        }

        let mut best: f32 = 0.0;
        let mut best_abs: f32 = 0.0;
        for &s in &buf[i_lo..i_hi] {
            let a = s.abs();
            if a > best_abs {
                best_abs = a;
                best = s;
            }
        }
        best
    }
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
        // Skip stale slots (plugin no longer calling process())
        if is_slot_stale(idx) {
            continue;
        }

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

    // Compute staleness ONCE per slot per frame. is_slot_stale() has a
    // destructive side effect (swap) — calling it twice would cause the
    // second call to always see stale, incorrectly skipping active slots.
    let mut slot_stale = [false; store::MAX_SLOTS];
    for &idx in &slots[..slot_count] {
        slot_stale[idx] = is_slot_stale(idx);
    }

    // Read transport info from the FIRST active slot and use it for ALL slots.
    let first_active = slots[..slot_count].iter().copied().find(|&i| !slot_stale[i]);
    let (global_is_playing, global_bpm, global_beats_per_bar, global_ppq, global_bar_start, global_spb) =
        if let Some(fi) = first_active {
            let fs = store::slot(fi);
            let playing = fs.playhead.is_playing.load(Ordering::Relaxed);
            let bpm = f64::from_bits(fs.playhead.bpm.load(Ordering::Relaxed));
            let bpb = fs.playhead.time_sig_num.load(Ordering::Relaxed);
            let ppq = f64::from_bits(fs.playhead.ppq_position.load(Ordering::Relaxed));
            let bar_start = f64::from_bits(fs.playhead.bar_start_ppq.load(Ordering::Relaxed));
            let spb = if bpm > 0.0 { (60.0 / bpm) * sample_rate as f64 } else { 0.0 };
            (playing, bpm, bpb, ppq, bar_start, spb)
        } else {
            (false, 120.0, 4, 0.0, 0.0, 0.0)
        };
    let global_samples_per_bar = global_spb * global_beats_per_bar as f64;

    // Compute the global playhead fraction ONCE for consistent bar latch/hold
    // triggering across all slots.
    let global_window_ppq = sync_bars * global_beats_per_bar as f64;
    let global_ppq_offset = global_ppq.rem_euclid(global_window_ppq);
    let global_frac = if global_window_ppq > 0.0 { global_ppq_offset / global_window_ppq } else { 0.0 };



    // Read ALL time_mapping snapshots in a tight loop BEFORE any other
    // per-slot work. This prevents the audio thread from advancing one
    // slot's ring_buffer_pos between reads, which shifts that slot's
    // beat-aligned window by up to one buffer (~1024 samples).
    let mut tm_snapshots: [Option<crate::time_mapping::TimeMappingSnapshot>; store::MAX_SLOTS] =
        [const { None }; store::MAX_SLOTS];
    for &idx in &slots[..slot_count] {
        if !slot_stale[idx] {
            tm_snapshots[idx] = Some(store::slot(idx).time_mapping.snapshot());
        }
    }

    for &idx in &slots[..slot_count] {
        // Skip stale slots (using cached result — never call is_slot_stale twice)
        if slot_stale[idx] {
            continue;
        }

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

        // Use global transport values (consistent across slots)
        let is_playing = global_is_playing;
        let bpm = global_bpm;
        let beats_per_bar = global_beats_per_bar;
        let ppq_in_bar = global_ppq - global_bar_start;
        let samples_per_bar = global_samples_per_bar;

        // Use the pre-read time_mapping snapshot (read in a tight batch above
        // to minimize timing skew between slots).
        let tm_snap = tm_snapshots[idx].unwrap();

        // Compute beat-aligned window using each slot's OWN ppq and
        // ring_buffer_pos (both written in the same process() call, so the
        // subtraction rb_pos - sample_offset is consistent). Using global_ppq
        // here would introduce a shift equal to the transport-position
        // difference between the reference slot's and this slot's process()
        // calls (~251 samples in multi-threaded DAWs).
        // Tempo (samples_per_beat) is global since all tracks share tempo.
        let per_slot_tm = crate::time_mapping::TimeMappingSnapshot {
            current_ppq: tm_snap.current_ppq,
            current_sample_pos: tm_snap.current_sample_pos,
            ring_buffer_pos: tm_snap.ring_buffer_pos,
            samples_per_beat: global_spb,
            discontinuity_counter: tm_snap.discontinuity_counter,
        };
        let window = if is_playing {
            crate::time_mapping::beat_aligned_window(&per_slot_tm, sync_bars, beats_per_bar)
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

        if let (Some(bufs), Some((computed_rb_start, window_len, _per_slot_frac))) =
            (guard.as_ref(), window)
        {
            // Use global_frac for bar latch and hold buffer to ensure all
            // slots trigger bar boundaries on the same frame.
            let playhead_fraction = global_frac;
            // Latch rb_start at bar boundaries to eliminate per-buffer PPQ jitter
            let (rb_start, bar_boundary) = BAR_LATCHES[idx].update(computed_rb_start, playhead_fraction);

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
                // At bar boundary, the previous bar is now fully written in the
                // ring buffer. Re-read it completely and promote directly to
                // the front buffer, bypassing the incremental accumulation
                // (which may be 5-20% incomplete due to GUI frame timing).
                if bar_boundary {
                    let prev_start = BAR_LATCHES[idx].prev_rb_start();
                    if prev_start > 0 {
                        let mut complete_bar = Vec::with_capacity(num_channels);
                        for (ch, buf) in bufs.iter().enumerate().take(num_channels) {
                            if ch == 0 {
                                data_version = buf.total_written() as u64;
                            }
                            let mut out = vec![0.0f32; window_len];
                            buf.read_range(prev_start, &mut out);
                            complete_bar.push(out);
                        }
                        HOLD_BUFFERS[idx].promote_complete(complete_bar);
                    }
                }

                // How many samples in the window are behind the write head (valid data).
                let write_pos = bufs.first().map_or(0, |b| b.total_written());
                let valid_count = write_pos.saturating_sub(rb_start).min(window_len);

                // Hold mode: use the double buffer to show the last complete bar.
                // Only the valid portion is copied into the back buffer each frame;
                // samples accumulate across frames as the playhead advances.
                if let Some(front_arc) = HOLD_BUFFERS[idx].update(&raw_data, playhead_fraction, valid_count) {
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
        // First frame at frac=0.5, all 3 samples valid
        let result = hb.update(&bar_data, 0.5, 3);
        assert!(result.is_none());
    }

    #[test]
    fn test_hold_buffer_promotes_back_at_bar_boundary() {
        let hb = HoldBuffer::new();
        let bar1 = vec![vec![1.0, 2.0, 3.0]];
        let bar2 = vec![vec![4.0, 5.0, 6.0]];

        // Simulate bar 1 playing through (all samples valid)
        hb.update(&bar1, 0.1, 3);
        hb.update(&bar1, 0.5, 3);
        hb.update(&bar1, 0.9, 3);

        // Bar boundary: frac wraps to 0.05
        let result = hb.update(&bar2, 0.05, 3);
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
        hb.update(&bar1, 0.1, 2);
        hb.update(&bar1, 0.9, 2);
        hb.update(&bar2_early, 0.05, 2); // boundary → bar1 promoted to front

        // Mid-bar reads should return stable front (bar1)
        let r1 = hb.update(&bar2_mid, 0.3, 2).unwrap();
        let r2 = hb.update(&bar2_mid, 0.7, 2).unwrap();
        assert_eq!(r1[0], vec![1.0, 2.0]);
        assert_eq!(r2[0], vec![1.0, 2.0]);
    }

    #[test]
    fn test_hold_buffer_uses_last_read_before_boundary() {
        let hb = HoldBuffer::new();
        let early = vec![vec![0.1]];
        let mid = vec![vec![0.5]];
        let late = vec![vec![0.9]];
        let new_bar = vec![vec![1.0]];

        hb.update(&early, 0.1, 1);
        hb.update(&mid, 0.5, 1);
        hb.update(&late, 0.95, 1);

        let result = hb.update(&new_bar, 0.02, 1); // boundary
        let front = result.unwrap();
        // Front should be the LATE data (last back before swap), not new_bar
        assert_eq!(front[0], vec![0.9]);
    }

    #[test]
    fn test_hold_buffer_accumulates_valid_portion_only() {
        let hb = HoldBuffer::new();
        // Simulate a 4-sample window where valid_count grows over time.
        // Frame 1: frac=0.25, valid=1 → only sample[0] is written
        let data1 = vec![vec![1.0, 0.0, 0.0, 0.0]];
        hb.update(&data1, 0.25, 1);
        // Frame 2: frac=0.5, valid=2 → samples[0..2] are written
        let data2 = vec![vec![1.0, 2.0, 0.0, 0.0]];
        hb.update(&data2, 0.5, 2);
        // Frame 3: frac=0.75, valid=3
        let data3 = vec![vec![1.0, 2.0, 3.0, 0.0]];
        hb.update(&data3, 0.75, 3);
        // Frame 4: frac=0.95, valid=4 (full bar almost complete)
        let data4 = vec![vec![1.0, 2.0, 3.0, 4.0]];
        hb.update(&data4, 0.95, 4);

        // Bar boundary: promote back to front
        let new_bar = vec![vec![5.0, 0.0, 0.0, 0.0]];
        let front = hb.update(&new_bar, 0.05, 1).unwrap();
        // Front should have the full accumulated bar: [1, 2, 3, 4]
        assert_eq!(front[0], vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_hold_buffer_clears_back_after_promotion() {
        let hb = HoldBuffer::new();
        // Bar 1: fill with [1.0, 2.0, 3.0]
        hb.update(&vec![vec![1.0, 2.0, 3.0]], 0.5, 3);
        hb.update(&vec![vec![1.0, 2.0, 3.0]], 0.9, 3);
        // Promote bar 1 to front
        hb.update(&vec![vec![9.0, 0.0, 0.0]], 0.05, 1);

        // Bar 2: only write first sample (valid_count=1).
        // Back buffer should be zeros for [1..3], NOT leftover bar 1 data.
        hb.update(&vec![vec![9.0, 0.0, 0.0]], 0.5, 1);
        hb.update(&vec![vec![9.0, 0.0, 0.0]], 0.9, 1);
        // Promote bar 2
        let front = hb.update(&vec![vec![0.0, 0.0, 0.0]], 0.05, 0).unwrap();
        // Only sample[0] was ever written as valid; rest should be zeros
        assert_eq!(front[0], vec![9.0, 0.0, 0.0]);
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

    // ── sample_at_normalized_x ─────────────────────────────────────────

    fn empty_snap() -> WaveSnapshot {
        WaveSnapshot {
            slot_index: 0,
            track_name: String::new(),
            display_color: 0,
            num_channels: 1,
            group: 0,
            is_active: true,
            solo: false,
            mute: false,
            audio_data: Vec::new(),
            mipmap_level: 0,
            data_points: 0,
            data_version: 0,
            is_playing: false,
            bpm: 120.0,
            beats_per_bar: 4,
            samples_per_bar: 0.0,
            ppq_position_in_bar: 0.0,
            mono_mix: Vec::new(),
            peak_amplitude: 0.0,
            peak_db: -96.0,
        }
    }

    #[test]
    fn test_sample_at_normalized_x_empty_returns_zero() {
        let snap = empty_snap();
        assert_eq!(snap.sample_at_normalized_x(0.5, false), 0.0);
    }

    #[test]
    fn test_sample_at_normalized_x_raw_l0() {
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![0.1, 0.2, 0.3, 0.4]];
        snap.data_points = 4;
        assert!((snap.sample_at_normalized_x(0.0, false) - 0.1).abs() < 1e-6);
        assert!((snap.sample_at_normalized_x(0.25, false) - 0.2).abs() < 1e-6);
        // Clamp at t == 1.0 should pick the last sample, not panic
        assert!((snap.sample_at_normalized_x(1.0, false) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_sample_at_normalized_x_uses_mono_mix_when_requested() {
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![1.0, 1.0], vec![-1.0, -1.0]];
        snap.mono_mix = vec![0.5, -0.5];
        snap.data_points = 2;
        // mono_mix path
        assert!((snap.sample_at_normalized_x(0.0, true) - 0.5).abs() < 1e-6);
        assert!((snap.sample_at_normalized_x(0.75, true) + 0.5).abs() < 1e-6);
        // without mix_to_mono, falls back to first channel
        assert!((snap.sample_at_normalized_x(0.0, false) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_sample_at_normalized_x_decimated_min_max_pairs() {
        let mut snap = empty_snap();
        snap.mipmap_level = 1;
        // Two pairs: (min=-0.2, max=0.7) and (min=-0.9, max=0.1)
        snap.audio_data = vec![vec![-0.2, 0.7, -0.9, 0.1]];
        snap.data_points = 2; // number of pairs
        // First pair: |max| > |min|, so should return max (0.7)
        assert!((snap.sample_at_normalized_x(0.0, false) - 0.7).abs() < 1e-6);
        // Second pair: |min| > |max|, so should return min (-0.9)
        assert!((snap.sample_at_normalized_x(0.75, false) + 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_sample_at_normalized_x_out_of_range_clamps() {
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![1.0, 2.0, 3.0]];
        snap.data_points = 3;
        // Negative clamps to 0.0
        assert!((snap.sample_at_normalized_x(-0.5, false) - 1.0).abs() < 1e-6);
        // >1.0 clamps to last sample
        assert!((snap.sample_at_normalized_x(2.0, false) - 3.0).abs() < 1e-6);
    }

    // ── peak_at_column ─────────────────────────────────────────────────

    #[test]
    fn test_peak_at_column_empty() {
        let snap = empty_snap();
        assert_eq!(snap.peak_at_column(0, 10, false), 0.0);
        assert_eq!(snap.peak_at_column(0, 0, false), 0.0);
    }

    #[test]
    fn test_peak_at_column_matches_decimate_on_raw_l0() {
        // Construct a buffer longer than num_cols to force decimation.
        let samples: Vec<f32> =
            (0..20).map(|i| ((i as f32 * 0.37).sin()) * 0.9).collect();
        let mut snap = empty_snap();
        snap.audio_data = vec![samples.clone()];
        snap.data_points = samples.len();
        snap.mipmap_level = 0;

        let num_cols = 5;
        let (mins, maxs) = crate::renderer::decimate_to_columns(&samples, num_cols);

        for col in 0..num_cols {
            let tooltip = snap.peak_at_column(col, num_cols, false);
            let rendered_peak_abs = mins[col].abs().max(maxs[col].abs());
            // Whichever sample the tooltip returns must have the same
            // absolute magnitude as the column's envelope peak.
            assert!(
                (tooltip.abs() - rendered_peak_abs).abs() < 1e-5,
                "col {col}: tooltip {tooltip} vs envelope {rendered_peak_abs}"
            );
        }
    }

    #[test]
    fn test_peak_at_column_matches_decimate_on_interleaved_l1() {
        // Simulate an L1 mipmap buffer: `audio_data[0]` holds flat
        // [min, max, min, max, ...] pairs for many blocks. The renderer
        // treats this as one flat sample array and decimates it again
        // when num_cols < 2 * data_points.
        let n_blocks = 50;
        let samples: Vec<f32> = (0..n_blocks)
            .flat_map(|b| {
                let lo = -(b as f32 * 0.02);
                let hi = (b as f32 * 0.02) + 0.01;
                [lo, hi]
            })
            .collect();
        let mut snap = empty_snap();
        snap.audio_data = vec![samples.clone()];
        snap.data_points = n_blocks;
        snap.mipmap_level = 1;

        let num_cols = 10;
        let (mins, maxs) = crate::renderer::decimate_to_columns(&samples, num_cols);

        for col in 0..num_cols {
            let tooltip = snap.peak_at_column(col, num_cols, false);
            let rendered_peak_abs = mins[col].abs().max(maxs[col].abs());
            assert!(
                (tooltip.abs() - rendered_peak_abs).abs() < 1e-5,
                "col {col}: tooltip {tooltip} (abs {}) vs envelope {rendered_peak_abs}",
                tooltip.abs()
            );
        }
    }

    #[test]
    fn test_peak_at_column_preserves_sign() {
        // A buffer whose max-abs is negative; the tooltip should report
        // the negative number, not its absolute value.
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![0.1, -0.9, 0.2, 0.3]];
        snap.data_points = 4;
        let best = snap.peak_at_column(0, 1, false);
        assert!((best - (-0.9)).abs() < 1e-6);
    }

    #[test]
    fn test_peak_at_column_prefers_mono_mix_when_enabled() {
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![1.0, 1.0], vec![-1.0, -1.0]];
        snap.mono_mix = vec![0.5, -0.7];
        snap.data_points = 2;
        // With mix_to_mono, we read mono_mix → max-abs is -0.7
        let best = snap.peak_at_column(0, 1, true);
        assert!((best - (-0.7)).abs() < 1e-6);
        // Without mix_to_mono, we read audio_data[0] → max-abs is 1.0
        let best2 = snap.peak_at_column(0, 1, false);
        assert!((best2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_peak_at_column_wider_than_data_interpolates() {
        // num_cols > n: renderer draws line segments between samples at
        // pixels (i * num_cols / n). peak_at_column should return the
        // linearly interpolated value between the two adjacent samples,
        // not the nearest discrete sample.
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![0.0, 1.0, 2.0]]; // slope
        snap.data_points = 3;

        // With num_cols=9, step = 9/3 = 3. Samples drawn at pixels 0, 3, 6.
        // col 0: t=0.0 → samples[0]=0.0
        // col 1: t=0.333 → 0*(1-0.333) + 1*0.333 ≈ 0.333
        // col 2: t=0.666 → 0*(1-0.666) + 1*0.666 ≈ 0.666
        // col 3: t=1.0   → samples[1]=1.0 (frac=0 path)
        // col 4: t=1.333 → 1*(1-0.333) + 2*0.333 ≈ 1.333
        // col 6: t=2.0   → samples[2]=2.0 (clamped)
        // col 7: t=2.333 → past end, returns last sample = 2.0
        let num_cols = 9;
        assert!((snap.peak_at_column(0, num_cols, false) - 0.0).abs() < 1e-5);
        assert!((snap.peak_at_column(1, num_cols, false) - 0.333).abs() < 1e-3);
        assert!((snap.peak_at_column(2, num_cols, false) - 0.666).abs() < 1e-3);
        assert!((snap.peak_at_column(3, num_cols, false) - 1.0).abs() < 1e-5);
        assert!((snap.peak_at_column(4, num_cols, false) - 1.333).abs() < 1e-3);
        assert!((snap.peak_at_column(6, num_cols, false) - 2.0).abs() < 1e-5);
        assert_eq!(snap.peak_at_column(7, num_cols, false), 2.0);
        assert_eq!(snap.peak_at_column(8, num_cols, false), 2.0);
    }

    #[test]
    fn test_peak_at_column_single_sample_buffer() {
        // Pathological edge: buffer of length 1. Interpolation path
        // should return the single sample without panicking.
        let mut snap = empty_snap();
        snap.audio_data = vec![vec![0.42]];
        snap.data_points = 1;
        for col in 0..5 {
            assert_eq!(snap.peak_at_column(col, 5, false), 0.42);
        }
    }

    #[test]
    fn test_peak_at_column_dense_matches_renderer_at_last_col() {
        // Regression: verify parity at the last column specifically, to
        // pin the "no special case needed" claim in the implementation.
        let samples: Vec<f32> = (0..17).map(|i| (i as f32).sin()).collect();
        let mut snap = empty_snap();
        snap.audio_data = vec![samples.clone()];
        snap.data_points = samples.len();

        let num_cols = 4;
        let (mins, maxs) = crate::renderer::decimate_to_columns(&samples, num_cols);
        let last = num_cols - 1;
        let tooltip = snap.peak_at_column(last, num_cols, false);
        let envelope_peak_abs = mins[last].abs().max(maxs[last].abs());
        assert!(
            (tooltip.abs() - envelope_peak_abs).abs() < 1e-5,
            "last col: tooltip {tooltip} vs envelope {envelope_peak_abs}"
        );
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
