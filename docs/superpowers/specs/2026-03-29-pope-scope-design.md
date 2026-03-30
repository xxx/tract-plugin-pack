# Pope Scope Design Spec

A multichannel real-time oscilloscope plugin for audio visualization. Displays waveforms from up to 16 simultaneous plugin instances with beat sync, multiple display modes, and a terminal green-on-black theme. Pass-through audio, CPU-rendered GUI.

Inspired by the JUCE oscilloscope at `/home/mpd/git-sources/oscilloscope`.

## Plugin Identity

- **Name:** Pope Scope
- **Crate:** `pope-scope`
- **Formats:** CLAP, VST3, Standalone
- **Audio:** Pure pass-through (input = output), no DSP
- **Latency:** Zero
- **Channel support:** 1-16 input channels per instance (matching host layout)
- **GUI:** Softbuffer + tiny-skia CPU rendering, 60 FPS when visible
- **Window:** Default 800x500, resizable 600x400 to 1920x1080, with +/- scaling

## Parameters

| Parameter | Type | Range | Default | Notes |
|---|---|---|---|---|
| timebase | float (skewed) | 1-10000 ms | 2000 ms | Only visible in Free mode |
| minDb | float | -96 to -6 dB | -48 dB | Bottom of visible range |
| maxDb | float | -48 to 0 dB | 0 dB | Top of visible range |
| freeze | bool | | false | Pause display updates |
| displayMode | enum | Vertical, Overlay, Sum | Vertical | |
| drawStyle | enum | Line, Filled, Both | Both | |
| mixToMono | bool | | true | Combine channels to mono |
| decimation | int | 128-4096 | 2048 | Max output samples for rendering |
| group | int | 0-15 | 0 | Track group filter |
| syncMode | enum | Free, BeatSync | BeatSync | |
| syncUnit | enum | 1/4, 1/2, 1, 2, 4 bars | 1 bar | Only visible in BeatSync mode |

All parameters are automatable and saved/restored via nih-plug's state management.

## Shared State Architecture

### Static Global Store

Same pattern as gain-brain: a `static` array of 16 slots, shared across all instances in the same host process.

Each slot contains:
- **Ring buffer:** One per channel (up to 16). Custom implementation with atomic write position (see Ring Buffer section). Allocated on `initialize()`, deallocated on `deactivate()`. Sized to 32 seconds at the instance's sample rate.
- **Mipmap levels:** Computed inline during `push()` by the audio thread. Level 1: min/max per 64-sample block. Level 2: min/max per 256-sample block.
- **Time mapping:** Atomic fields (`AtomicU64` bit-cast from `f64` for PPQ, `AtomicI64` for sample position, `AtomicU64` for samples-per-beat, `AtomicU64` for discontinuity counter). Written by the owning audio thread. Read by any GUI thread.
- **Playhead info:** Atomic fields for BPM, time signature numerator/denominator, transport playing state. Written by the owning audio thread.
- **Metadata:** Track name (`Mutex<String>`), display color (atomic u32), channel count (atomic), group (atomic), solo state (atomic bool), mute state (atomic bool). Written at registration time or by GUI interaction. Read by the rendering instance.
- **Slot ownership:** `AtomicU64` for owner hash (0 = free, CAS for acquisition). `AtomicI64` for heartbeat timestamp (stale detection).

### Allocation Strategy

Buffers are allocated on demand when an instance joins a slot (`initialize()`), and deallocated when it leaves (`deactivate()`). The slot struct itself is statically allocated, but the `Vec<f32>` ring buffers inside it are `Option`-wrapped and created at the appropriate sample rate and channel count when needed. Pre-touched via `vec![0.0f32; n]` to fault in pages at allocation time, not during `process()`.

At 48 kHz stereo, one slot's ring buffers are ~12 MB. Only active slots consume memory.

### Data Flow

```
Audio thread (instance N):
  process() → push samples to slot N's ring buffers
            → update slot N's time mapping atomics
            → update slot N's playhead atomics

GUI thread (whichever instance has the open window):
  60 FPS timer → SnapshotBuilder reads ALL active slots
               → produces Vec<WaveSnapshot> (immutable, owned data)
               → Renderer draws from snapshots only
```

The GUI never touches shared state directly. SnapshotBuilder is the sole reader.

## Ring Buffer Design

Custom single-writer ring buffer. NOT an SPSC queue — the reader does not consume.

### Structure (per channel)

```
RingBuffer {
    buffer: Vec<f32>,           // Pre-allocated, fixed size
    write_pos: AtomicUsize,     // Monotonically increasing, mod capacity for index
    capacity: usize,            // = sample_rate * 32 (seconds)
}
```

**Writer (audio thread):** Copies samples into buffer at `write_pos % capacity`, then advances `write_pos` with `Relaxed` store. Single writer — no CAS needed.

**Reader (GUI thread):** Reads `write_pos`, computes the range it needs, copies out. The reader never modifies `write_pos`. Safe because: (1) single writer, (2) `Relaxed` ordering is fine — stale reads just mean the GUI is one buffer behind, which is invisible at 60 FPS, (3) the reader copies data out before using it (snapshot pattern).

### Mipmap Levels

Stored alongside the raw ring buffer, same atomic-write-pos pattern:

- **Level 1:** `Vec<(f32, f32)>` — (min, max) per 64-sample block. Updated inline during `push()` after each 64th sample.
- **Level 2:** `Vec<(f32, f32)>` — (min, max) per 256-sample block. Updated after each 4th Level 1 block completes.

Mipmap indices are aligned to absolute buffer positions (block N always covers samples `[N*64, (N+1)*64)`), same as the JUCE version.

### Level Selection

- Decimation < 64: Read raw samples (Level 0)
- Decimation 64-255: Read Level 1 min/max blocks
- Decimation >= 256: Read Level 2 min/max blocks

## Time Mapping (Beat Sync)

### Atomic Fields per Slot

```
current_ppq: AtomicU64          // f64 bit-cast, current PPQ position
current_sample_pos: AtomicI64   // Absolute sample position at current_ppq
samples_per_beat: AtomicU64     // f64 bit-cast, derived from BPM + sample rate
discontinuity_counter: AtomicU64 // Incremented on loop/seek/play start
```

### Push with Time Mapping

The audio thread captures the sample position BEFORE pushing (DAW's PPQ refers to buffer start, not end). It detects discontinuities by comparing current PPQ with expected PPQ (based on last PPQ + samples advanced). On discontinuity (loop restart, seek, play start), it increments the counter.

### Beat-Aligned Reading

The SnapshotBuilder computes a beat-aligned window:
1. Read `current_ppq` and `current_sample_pos` atomics
2. Compute window start PPQ from sync unit (snap to bar/beat boundary)
3. Convert PPQ range to sample range via `samples_per_beat`
4. Read samples from ring buffer for that range
5. Mask stale data beyond current PPQ with 16-sample linear fade (prevents spike artifacts)

## Snapshot Builder

### WaveSnapshot (Immutable)

```
WaveSnapshot {
    // Metadata (copied)
    track_name: String,
    display_color: u32,         // ARGB
    num_channels: usize,
    group: u8,
    is_active: bool,
    solo: bool,
    mute: bool,

    // Audio data (owned, not shared)
    audio_data: Vec<Vec<f32>>,  // [channel][sample] or decimated min/max
    data_version: u64,          // For path cache invalidation

    // Beat sync info (copied from playhead)
    is_playing: bool,
    bpm: f64,
    beats_per_bar: u32,
    samples_per_bar: f64,
    ppq_position_in_bar: f64,

    // Pre-computed
    mono_mix: Vec<f32>,         // Average of all channels
    peak_amplitude: f32,        // Max |sample| across all channels
    peak_db: f32,               // 20 * log10(peak_amplitude)
}
```

### Build Paths

- `build_snapshots_free()`: Reads most recent N samples based on timebase, applies decimation via mipmap level selection.
- `build_snapshots_beat_sync()`: Reads beat-aligned windows using time mapping. Handles transport stop (clears stale data). Masks beyond-PPQ samples with fade.

Both paths filter by group assignment and produce only snapshots for the selected group.

## Renderer

### Display Modes

**Vertical:** Tracks stacked vertically. Control strip (110px) on left per track. Waveform fills remaining width. Amplitude grid (horizontal lines at dB divisions). Time/beat grid (vertical lines). Time labels only on bottom track. dB labels on right edge.

**Overlay:** All tracks overlaid in a single area. Color-coded legend in top-left corner. Single shared grid.

**Sum:** All visible (non-muted, passes solo filter) tracks summed sample-by-sample into one waveform. Rendered in the primary foreground green. Single shared grid.

### Draw Styles

- **Line:** Stroke path only, 1px width.
- **Filled:** Semi-transparent fill (75% alpha) from center line.
- **Both:** Fill at 30% alpha + stroke at 100% alpha.

### Amplitude Mapping

dB-scaled Y axis:
```
fn sample_to_y(sample: f32, min_db: f32, max_db: f32, centre_y: f32, half_height: f32) -> f32 {
    let sign = if sample >= 0.0 { 1.0 } else { -1.0 };
    let abs_amp = sample.abs();
    let db = if abs_amp > 0.0 { 20.0 * abs_amp.log10() } else { -96.0 };
    let normalized = (db - min_db) / (max_db - min_db);
    centre_y - (normalized * half_height * sign)
}
```

### Decimation in Drawing

When sample count exceeds display pixel columns: find min/max per pixel column, draw vertical segment for each column. For filled style, build an envelope path (max values forward, min values backward).

### Path Caching

Cache the `tiny_skia::Path` per track, keyed by:
- `data_version` (from snapshot)
- Bounds (width, height)
- dB range (min, max)
- Draw style

Rebuild only when any key changes.

### Grid Rendering

**Amplitude grid:** Horizontal lines at dB divisions. Division size auto-selected: 6 dB for ranges > 36 dB, 3 dB for 18-36 dB, etc. Lines drawn in grid color (`#004400`). Center line (0 dB silence) in brighter grid color (`#006600`).

**Time grid (Free mode):** Vertical lines with auto-calculated divisions targeting 4-10 lines. Labels in ms/s.

**Beat grid (Beat Sync mode):** Thin lines for beat subdivisions (`#004400`). Thicker purple lines for bar boundaries (`#9933ff`). Labels as "Bar.Beat" format. BPM indicator in top-right.

### Peak Hold

Per-track dashed horizontal line at the peak level. Updates when new peak detected. 2-second hold time, then decays at 20 dB/second.

### Mouse Cursor

Vertical cyan line at mouse X position. Tooltip showing:
- Time at cursor position (ms or Bar.Beat)
- Per-track: color swatch + track name + amplitude + dB reading
- Only recalculate dB values when X position changes

### Sample Validation

Reject samples outside [-2.0, 2.0] range (spike artifact prevention from corrupted data).

## Control Strip (Vertical Mode)

110px wide panel on the left side of each track:

- **Track name:** Centered, monospace, foreground color
- **Color button:** 16x16 swatch showing track color. Click cycles through the 16-color terminal palette.
- **Solo button:** "S" label. Off: foreground outline. On: yellow background with dark text. If any track is soloed, only soloed tracks render.
- **Mute button:** "M" label. Off: foreground outline. On: red background with dark text. Muted tracks don't render.

Solo/mute/color are stored in the shared metadata (atomic fields) so any instance's renderer can read them.

## Editor Layout

**Bottom control bar** containing (left to right):
- Sync mode selector (Free / Beat Sync)
- Sync unit selector (1/4, 1/2, 1, 2, 4 bars) — shown only in Beat Sync mode
- Timebase slider — shown only in Free mode
- Min dB / Max dB controls
- Display mode selector (Vertical / Overlay / Sum)
- Draw style selector (Line / Filled / Both)
- Mix to Mono toggle
- Group selector (0-15)
- Freeze button

**Upper right:** Scale +/- buttons with percentage label (same as other plugins).

**Main area:** Full waveform display area above the control bar.

All controls use the hit-region pattern from the existing softbuffer editors (gain-brain, gs-meter, tinylimit, satch).

## Amber Phosphor Theme

```
background:    #0a0600  (warm black)
foreground:    #ffb833  (amber phosphor)
primary_dim:   #aa7700  (dimmer amber)
grid:          #442e00  (dark amber)
grid_bright:   #664400  (brighter amber for center line)
border:        #1a1400  (very dark warm grey)
bar_line:      #cc6600  (bar boundary accent)
cyan:          #33ddff
magenta:       #ff6699
yellow:        #ffdd33
red:           #ff4444
purple:        #bb66ff
orange:        #ff9944
blue:          #4499ff
```

**16-color channel palette** (indexed by slot number):
```
 0: #ffb833  amber         8: #ffd066  light amber
 1: #33ddff  cyan          9: #66eeff  light cyan
 2: #ff6699  rose         10: #ff99bb  light rose
 3: #ffdd33  yellow       11: #ffee66  light yellow
 4: #ff9944  orange       12: #ffbb77  light orange
 5: #bb66ff  purple       13: #cc88ff  light purple
 6: #ff4444  red          14: #ff7777  light red
 7: #4499ff  blue         15: #77bbff  light blue
```

## File Structure

```
pope-scope/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Plugin struct, params, process()
│   ├── main.rs             # Standalone entry point
│   ├── store.rs            # Static global store, slot allocation, metadata
│   ├── ring_buffer.rs      # Ring buffer + mipmap levels
│   ├── time_mapping.rs     # Atomic PPQ/sample mapping, discontinuity detection
│   ├── snapshot.rs         # WaveSnapshot struct + SnapshotBuilder
│   ├── renderer.rs         # Display modes, waveform drawing, grid, cursor
│   ├── editor.rs           # Softbuffer editor, hit regions, mouse, controls bar
│   ├── controls.rs         # TrackControlStrip (solo/mute/color)
│   ├── theme.rs            # Terminal color palette
│   └── fonts/
│       └── DejaVuSans.ttf  # Embedded font
```

## Testing Strategy

- **ring_buffer.rs:** Push/read correctness, wrap-around, mipmap level computation, decimated reads at each level
- **time_mapping.rs:** PPQ-to-sample conversion, discontinuity detection, beat-aligned window calculation
- **snapshot.rs:** Mono mix computation, peak/dB calculation, group filtering, solo/mute filtering
- **store.rs:** Slot allocation via CAS, concurrent allocation, release/reuse, stale detection
- **renderer.rs:** Sample-to-Y mapping, grid division calculation, path building for each draw style
- **theme.rs:** Color palette indexing

Integration tests: multi-instance write/read through the shared store.

## Performance Targets

Based on existing plugin benchmarks in the workspace:
- Per-instance headless (no GUI): target < 1 MB RSS, < 0.05% CPU (pass-through audio + ring buffer push)
- GUI rendering: single window at 60 FPS with up to 16 tracks visible
- Audio thread `process()`: no allocations, no locks, just memcpy to ring buffer + atomic writes
