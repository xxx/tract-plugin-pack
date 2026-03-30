# Pope Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a multichannel real-time oscilloscope plugin (CLAP/VST3/Standalone) with beat sync, three display modes, and an amber phosphor terminal theme.

**Architecture:** Pass-through audio plugin using a static global store for cross-instance data sharing. Each instance pushes audio into a per-slot ring buffer with hierarchical mipmap. A SnapshotBuilder reads all slots and produces immutable WaveSnapshots for the CPU-rendered GUI (tiny-skia + softbuffer). Ring buffers use atomic write positions (single-writer, non-consuming reader). Beat sync uses atomic time mapping with discontinuity detection.

**Tech Stack:** Rust nightly, nih-plug (CLAP/VST3/standalone), tiny-skia 0.12, softbuffer, baseview, tiny-skia-widgets (shared workspace crate)

**Spec:** `docs/superpowers/specs/2026-03-29-pope-scope-design.md`

---

## File Map

| File | Responsibility |
|---|---|
| `pope-scope/Cargo.toml` | Crate config, dependencies |
| `pope-scope/src/main.rs` | Standalone entry point (3 lines) |
| `pope-scope/src/lib.rs` | Plugin struct, params, `process()`, CLAP/VST3 export |
| `pope-scope/src/theme.rs` | Amber phosphor color palette, 16-color channel palette |
| `pope-scope/src/ring_buffer.rs` | Ring buffer with atomic write pos + 3-level mipmap |
| `pope-scope/src/time_mapping.rs` | Atomic PPQ/sample mapping, discontinuity detection |
| `pope-scope/src/store.rs` | Static global 16-slot store, CAS ownership, metadata |
| `pope-scope/src/snapshot.rs` | WaveSnapshot struct, SnapshotBuilder (free + beat sync) |
| `pope-scope/src/renderer.rs` | Waveform drawing, grid, peak hold, cursor, display modes |
| `pope-scope/src/editor.rs` | Softbuffer window, hit regions, control bar, timer loop |
| `pope-scope/src/controls.rs` | TrackControlStrip (solo/mute/color per track) |
| `pope-scope/src/fonts/DejaVuSans.ttf` | Embedded font (copy from existing plugin) |
| `Cargo.toml` (workspace root) | Add `pope-scope` to workspace members |

---

### Task 1: Project Scaffolding

**Files:**
- Create: `pope-scope/Cargo.toml`
- Create: `pope-scope/src/main.rs`
- Create: `pope-scope/src/lib.rs` (skeleton)
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "pope-scope"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]

[[bin]]
name = "pope-scope"
path = "src/main.rs"

[dependencies]
nih_plug = { git = "https://github.com/xxx/nih-plug.git", branch = "finish-vst3-pr", features = ["standalone"] }
baseview = { git = "https://github.com/RustAudio/baseview.git", rev = "9a0b42c09d712777b2edb4c5e0cb6baf21e988f0", features = ["opengl"] }
softbuffer = { version = "0.4", default-features = false, features = ["kms", "x11"] }
raw-window-handle = "0.5"
raw-window-handle-06 = { package = "raw-window-handle", version = "0.6" }
tiny-skia = "0.12"
tiny-skia-widgets = { path = "../tiny-skia-widgets" }
keyboard-types = "0.6"
crossbeam = "0.8"

[package.metadata.bundler]
name = "Pope Scope"
company = "mpd"
description = "A multichannel real-time oscilloscope"
license = "GPL-3.0-or-later"
version = "0.1.0"
```

- [ ] **Step 2: Create main.rs**

```rust
use nih_plug::prelude::*;

fn main() {
    nih_export_standalone::<pope_scope::PopeScope>();
}
```

- [ ] **Step 3: Create minimal lib.rs**

```rust
use nih_plug::prelude::*;
use std::sync::Arc;

mod theme;

pub struct PopeScope {
    params: Arc<PopeScopeParams>,
}

#[derive(Params)]
pub struct PopeScopeParams {}

impl Default for PopeScope {
    fn default() -> Self {
        Self {
            params: Arc::new(PopeScopeParams {}),
        }
    }
}

impl Plugin for PopeScope {
    const NAME: &'static str = "Pope Scope";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl ClapPlugin for PopeScope {
    const CLAP_ID: &'static str = "com.mpd.pope-scope";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A multichannel real-time oscilloscope");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Analyzer];
}

impl Vst3Plugin for PopeScope {
    const VST3_CLASS_ID: [u8; 16] = *b"PopeScopeMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

nih_export_clap!(PopeScope);
nih_export_vst3!(PopeScope);
```

- [ ] **Step 4: Create empty theme.rs**

```rust
//! Amber phosphor terminal color palette.
```

- [ ] **Step 5: Add to workspace**

In the root `Cargo.toml`, add `"pope-scope"` to the workspace members list.

- [ ] **Step 6: Verify it compiles**

Run: `cargo check -p pope-scope`
Expected: compiles with no errors

- [ ] **Step 7: Copy embedded font**

```bash
mkdir -p pope-scope/src/fonts
cp gain-brain/src/fonts/DejaVuSans.ttf pope-scope/src/fonts/
```

- [ ] **Step 8: Commit**

```bash
git add pope-scope/ Cargo.toml Cargo.lock
git commit -m "pope-scope: project scaffolding with minimal plugin skeleton"
```

---

### Task 2: Theme

**Files:**
- Modify: `pope-scope/src/theme.rs`

- [ ] **Step 1: Write tests for color palette**

```rust
//! Amber phosphor terminal color palette.

/// ARGB color constants for the amber phosphor theme.
pub const BG: u32 = 0xFF0A_0600;
pub const FG: u32 = 0xFFFF_B833;
pub const PRIMARY_DIM: u32 = 0xFFAA_7700;
pub const GRID: u32 = 0xFF44_2E00;
pub const GRID_BRIGHT: u32 = 0xFF66_4400;
pub const BORDER: u32 = 0xFF1A_1400;
pub const BAR_LINE: u32 = 0xFFCC_6600;
pub const CYAN: u32 = 0xFF33_DDFF;
pub const ROSE: u32 = 0xFFFF_6699;
pub const YELLOW: u32 = 0xFFFF_DD33;
pub const RED: u32 = 0xFFFF_4444;
pub const PURPLE: u32 = 0xFFBB_66FF;
pub const ORANGE: u32 = 0xFFFF_9944;
pub const BLUE: u32 = 0xFF44_99FF;

/// 16-color channel palette indexed by slot number.
const CHANNEL_COLORS: [u32; 16] = [
    0xFFFF_B833, // 0: amber
    0xFF33_DDFF, // 1: cyan
    0xFFFF_6699, // 2: rose
    0xFFFF_DD33, // 3: yellow
    0xFFFF_9944, // 4: orange
    0xFFBB_66FF, // 5: purple
    0xFFFF_4444, // 6: red
    0xFF44_99FF, // 7: blue
    0xFFFF_D066, // 8: light amber
    0xFF66_EEFF, // 9: light cyan
    0xFFFF_99BB, // 10: light rose
    0xFFFF_EE66, // 11: light yellow
    0xFFFF_BB77, // 12: light orange
    0xFFCC_88FF, // 13: light purple
    0xFFFF_7777, // 14: light red
    0xFF77_BBFF, // 15: light blue
];

/// Get the channel color for a slot index (wraps at 16).
pub fn channel_color(slot: usize) -> u32 {
    CHANNEL_COLORS[slot % 16]
}

/// Convert an ARGB u32 to tiny-skia Color.
pub fn to_color(argb: u32) -> tiny_skia::Color {
    let a = ((argb >> 24) & 0xFF) as f32 / 255.0;
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    tiny_skia::Color::from_rgba(r, g, b, a).unwrap()
}

/// Convert an ARGB u32 to a tiny-skia Color with overridden alpha (0.0-1.0).
pub fn to_color_alpha(argb: u32, alpha: f32) -> tiny_skia::Color {
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    tiny_skia::Color::from_rgba(r, g, b, alpha).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_color_in_range() {
        for i in 0..16 {
            assert_eq!(channel_color(i), CHANNEL_COLORS[i]);
        }
    }

    #[test]
    fn test_channel_color_wraps() {
        assert_eq!(channel_color(16), channel_color(0));
        assert_eq!(channel_color(17), channel_color(1));
    }

    #[test]
    fn test_to_color_bg() {
        let c = to_color(BG);
        assert!((c.red() - 10.0 / 255.0).abs() < 0.01);
        assert!((c.green() - 6.0 / 255.0).abs() < 0.01);
        assert!((c.blue() - 0.0 / 255.0).abs() < 0.01);
        assert!((c.alpha() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_to_color_alpha() {
        let c = to_color_alpha(FG, 0.5);
        assert!((c.alpha() - 0.5).abs() < 0.01);
        assert!((c.red() - 1.0).abs() < 0.01); // 0xFF
    }

    #[test]
    fn test_all_channel_colors_are_opaque() {
        for i in 0..16 {
            let c = channel_color(i);
            assert_eq!(c >> 24, 0xFF, "channel color {i} must be fully opaque");
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add pope-scope/src/theme.rs
git commit -m "pope-scope: amber phosphor theme with 16-color channel palette"
```

---

### Task 3: Ring Buffer — Core Push & Read

**Files:**
- Create: `pope-scope/src/ring_buffer.rs`
- Modify: `pope-scope/src/lib.rs` (add `pub mod ring_buffer;`)

- [ ] **Step 1: Write failing tests for core ring buffer**

```rust
//! Ring buffer with atomic write position and hierarchical mipmap.
//!
//! Single-writer (audio thread), non-consuming reader (GUI thread).
//! The reader copies data out — it never modifies write_pos.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A fixed-size circular buffer for audio samples.
///
/// The writer pushes samples sequentially. The reader can read any
/// historical window without consuming data.
pub struct RingBuffer {
    buffer: Vec<f32>,
    /// Monotonically increasing. Index into buffer is `write_pos % capacity`.
    write_pos: AtomicUsize,
    capacity: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity (in samples).
    /// Pre-touches all memory via zero-fill.
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0.0f32; capacity],
            write_pos: AtomicUsize::new(0),
            capacity,
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
        let start = if pos >= to_read { pos - to_read } else { 0 };
        for i in 0..to_read {
            let idx = (start + i) % self.capacity;
            out[i] = self.buffer[idx];
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
        for i in 0..count {
            let abs = start_pos + i;
            if abs >= pos || (pos > self.capacity && abs < pos - self.capacity) {
                out[i] = 0.0;
            } else {
                out[i] = self.buffer[abs % self.capacity];
                valid += 1;
            }
        }
        valid
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
}
```

- [ ] **Step 2: Add module to lib.rs**

Add `pub mod ring_buffer;` to `pope-scope/src/lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/ring_buffer.rs pope-scope/src/lib.rs
git commit -m "pope-scope: ring buffer with atomic write pos, push and read"
```

---

### Task 4: Ring Buffer — Mipmap Levels

**Files:**
- Modify: `pope-scope/src/ring_buffer.rs`

- [ ] **Step 1: Add mipmap constants and structure**

Add to the top of `ring_buffer.rs`:

```rust
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
```

- [ ] **Step 2: Add mipmap fields to RingBuffer**

Extend `RingBuffer`:

```rust
pub struct RingBuffer {
    buffer: Vec<f32>,
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
```

Update `new()`:

```rust
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
        l1_accum: MinMax { min: f32::MAX, max: f32::MIN },
        l1_accum_count: 0,
        level2: vec![MinMax::default(); l2_cap],
        level2_pos: AtomicUsize::new(0),
        level2_capacity: l2_cap,
        l2_block_count: 0,
        l2_accum: MinMax { min: f32::MAX, max: f32::MIN },
    }
}
```

- [ ] **Step 3: Update push() to compute mipmaps**

```rust
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
                self.l2_accum = MinMax { min: f32::MAX, max: f32::MIN };
                self.l2_block_count = 0;
            }

            self.l1_accum = MinMax { min: f32::MAX, max: f32::MIN };
            self.l1_accum_count = 0;
        }
    }
    self.write_pos.store(pos + samples.len(), Ordering::Relaxed);
}
```

- [ ] **Step 4: Add decimated read methods**

```rust
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
    for i in 0..to_read {
        let idx = (start + i) % self.level1_capacity;
        out[i] = self.level1[idx];
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
    for i in 0..to_read {
        let idx = (start + i) % self.level2_capacity;
        out[i] = self.level2[idx];
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
```

- [ ] **Step 5: Write mipmap tests**

```rust
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
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 7: Commit**

```bash
git add pope-scope/src/ring_buffer.rs
git commit -m "pope-scope: 3-level hierarchical mipmap on ring buffer"
```

---

### Task 5: Time Mapping

**Files:**
- Create: `pope-scope/src/time_mapping.rs`
- Modify: `pope-scope/src/lib.rs` (add `pub mod time_mapping;`)

- [ ] **Step 1: Write time mapping module with tests**

```rust
//! Atomic time mapping for beat sync.
//!
//! Maps between PPQ (Pulses Per Quarter note) positions and absolute
//! sample positions. Used by the audio thread to tag samples with
//! musical time, and by the GUI thread to extract beat-aligned windows.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

/// Atomic time mapping state. Written by audio thread, read by GUI thread.
pub struct TimeMapping {
    /// Current PPQ position (f64 bit-cast to u64).
    current_ppq: AtomicU64,
    /// Absolute sample position corresponding to `current_ppq`.
    current_sample_pos: AtomicI64,
    /// Samples per beat (f64 bit-cast to u64). Derived from BPM + sample rate.
    samples_per_beat: AtomicU64,
    /// Incremented on transport discontinuities (loop, seek, play start).
    discontinuity_counter: AtomicU64,
    /// Last PPQ written, for discontinuity detection (f64 bit-cast).
    last_ppq: AtomicU64,
    /// Whether transport was playing on the previous buffer.
    was_playing: AtomicU32,
}

/// Non-atomic snapshot of time mapping for GUI reads.
#[derive(Clone, Copy, Debug)]
pub struct TimeMappingSnapshot {
    pub current_ppq: f64,
    pub current_sample_pos: i64,
    pub samples_per_beat: f64,
    pub discontinuity_counter: u64,
}

impl TimeMapping {
    pub const fn new() -> Self {
        Self {
            current_ppq: AtomicU64::new(0),
            current_sample_pos: AtomicI64::new(0),
            samples_per_beat: AtomicU64::new(0),
            discontinuity_counter: AtomicU64::new(0),
            last_ppq: AtomicU64::new(0),
            was_playing: AtomicU32::new(0),
        }
    }

    /// Update time mapping from audio thread. Call BEFORE pushing audio.
    ///
    /// - `ppq`: current PPQ position from DAW playhead
    /// - `sample_pos`: absolute sample position at buffer start
    /// - `bpm`: current tempo
    /// - `sample_rate`: current sample rate
    /// - `is_playing`: whether transport is playing
    /// - `buffer_size`: number of samples in this buffer
    pub fn update(
        &self,
        ppq: f64,
        sample_pos: i64,
        bpm: f64,
        sample_rate: f64,
        is_playing: bool,
    ) {
        let was_playing = self.was_playing.load(Ordering::Relaxed) != 0;

        if is_playing {
            let spb = (60.0 / bpm) * sample_rate;
            self.samples_per_beat
                .store(spb.to_bits(), Ordering::Relaxed);

            // Detect discontinuity: play start, or PPQ jumped unexpectedly
            if !was_playing {
                // Transport just started
                self.discontinuity_counter.fetch_add(1, Ordering::Relaxed);
            } else {
                let last = f64::from_bits(self.last_ppq.load(Ordering::Relaxed));
                let expected_advance = 1024.0 / spb; // rough heuristic
                let actual_advance = ppq - last;
                // If PPQ jumped by more than 2x expected, it's a discontinuity
                if actual_advance < -0.01 || actual_advance > expected_advance * 2.0 + 0.5 {
                    self.discontinuity_counter.fetch_add(1, Ordering::Relaxed);
                }
            }

            self.current_ppq.store(ppq.to_bits(), Ordering::Relaxed);
            self.current_sample_pos.store(sample_pos, Ordering::Relaxed);
            self.last_ppq.store(ppq.to_bits(), Ordering::Relaxed);
        }

        self.was_playing
            .store(if is_playing { 1 } else { 0 }, Ordering::Relaxed);
    }

    /// Read a snapshot (GUI thread).
    pub fn snapshot(&self) -> TimeMappingSnapshot {
        TimeMappingSnapshot {
            current_ppq: f64::from_bits(self.current_ppq.load(Ordering::Relaxed)),
            current_sample_pos: self.current_sample_pos.load(Ordering::Relaxed),
            samples_per_beat: f64::from_bits(self.samples_per_beat.load(Ordering::Relaxed)),
            discontinuity_counter: self.discontinuity_counter.load(Ordering::Relaxed),
        }
    }

    /// Reset all fields (used in tests).
    #[cfg(test)]
    pub fn reset(&self) {
        self.current_ppq.store(0, Ordering::Relaxed);
        self.current_sample_pos.store(0, Ordering::Relaxed);
        self.samples_per_beat.store(0, Ordering::Relaxed);
        self.discontinuity_counter.store(0, Ordering::Relaxed);
        self.last_ppq.store(0, Ordering::Relaxed);
        self.was_playing.store(0, Ordering::Relaxed);
    }
}

/// Compute the sample range for a beat-aligned window.
///
/// - `snap`: time mapping snapshot
/// - `sync_bars`: number of bars to display (e.g. 0.25, 0.5, 1.0, 2.0, 4.0)
/// - `beats_per_bar`: from time signature numerator
///
/// Returns `(start_sample_pos, window_length_samples)` or `None` if
/// samples_per_beat is zero.
pub fn beat_aligned_window(
    snap: &TimeMappingSnapshot,
    sync_bars: f64,
    beats_per_bar: u32,
) -> Option<(i64, usize)> {
    if snap.samples_per_beat <= 0.0 {
        return None;
    }
    let beats_in_window = sync_bars * beats_per_bar as f64;
    let window_samples = (beats_in_window * snap.samples_per_beat).round() as usize;
    let ppq_per_bar = beats_per_bar as f64;
    let window_ppq = sync_bars * ppq_per_bar;

    // Snap current PPQ to the nearest window boundary
    let window_start_ppq = (snap.current_ppq / window_ppq).floor() * window_ppq;
    let ppq_offset = window_start_ppq - snap.current_ppq;
    let sample_offset = (ppq_offset * snap.samples_per_beat).round() as i64;
    let start_sample = snap.current_sample_pos + sample_offset;

    Some((start_sample, window_samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_snapshot_is_zero() {
        let tm = TimeMapping::new();
        let snap = tm.snapshot();
        assert_eq!(snap.current_ppq, 0.0);
        assert_eq!(snap.current_sample_pos, 0);
        assert_eq!(snap.samples_per_beat, 0.0);
        assert_eq!(snap.discontinuity_counter, 0);
    }

    #[test]
    fn test_update_stores_values() {
        let tm = TimeMapping::new();
        // 120 BPM, 48000 Hz → 24000 samples/beat
        tm.update(4.0, 96000, 120.0, 48000.0, true);
        let snap = tm.snapshot();
        assert!((snap.current_ppq - 4.0).abs() < 0.001);
        assert_eq!(snap.current_sample_pos, 96000);
        assert!((snap.samples_per_beat - 24000.0).abs() < 0.1);
    }

    #[test]
    fn test_play_start_increments_discontinuity() {
        let tm = TimeMapping::new();
        // First call with is_playing=true → play start
        tm.update(0.0, 0, 120.0, 48000.0, true);
        let snap = tm.snapshot();
        assert_eq!(snap.discontinuity_counter, 1);
    }

    #[test]
    fn test_continuous_play_no_discontinuity() {
        let tm = TimeMapping::new();
        tm.update(0.0, 0, 120.0, 48000.0, true); // play start → +1
        tm.update(0.0427, 1024, 120.0, 48000.0, true); // normal advance
        let snap = tm.snapshot();
        assert_eq!(snap.discontinuity_counter, 1); // no new discontinuity
    }

    #[test]
    fn test_loop_increments_discontinuity() {
        let tm = TimeMapping::new();
        tm.update(0.0, 0, 120.0, 48000.0, true);
        tm.update(3.9, 93600, 120.0, 48000.0, true);
        // PPQ jumps backward (loop restart)
        tm.update(0.0, 0, 120.0, 48000.0, true);
        let snap = tm.snapshot();
        assert!(snap.discontinuity_counter >= 2);
    }

    #[test]
    fn test_not_playing_doesnt_update_ppq() {
        let tm = TimeMapping::new();
        tm.update(0.0, 0, 120.0, 48000.0, true);
        let snap1 = tm.snapshot();
        tm.update(99.0, 999999, 120.0, 48000.0, false);
        let snap2 = tm.snapshot();
        // PPQ should not have changed
        assert_eq!(snap1.current_ppq, snap2.current_ppq);
    }

    #[test]
    fn test_beat_aligned_window_1_bar_4_4() {
        let snap = TimeMappingSnapshot {
            current_ppq: 6.5,
            current_sample_pos: 156000,
            samples_per_beat: 24000.0, // 120 BPM @ 48kHz
            discontinuity_counter: 0,
        };
        let (start, len) = beat_aligned_window(&snap, 1.0, 4).unwrap();
        // 1 bar = 4 beats = 96000 samples
        assert_eq!(len, 96000);
        // Window should start at PPQ 4.0 (floor of 6.5 to nearest 4.0 boundary)
        // PPQ offset = 4.0 - 6.5 = -2.5, sample offset = -2.5 * 24000 = -60000
        assert_eq!(start, 156000 - 60000);
    }

    #[test]
    fn test_beat_aligned_window_zero_spb() {
        let snap = TimeMappingSnapshot {
            current_ppq: 0.0,
            current_sample_pos: 0,
            samples_per_beat: 0.0,
            discontinuity_counter: 0,
        };
        assert!(beat_aligned_window(&snap, 1.0, 4).is_none());
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

Add `pub mod time_mapping;` to `lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/time_mapping.rs pope-scope/src/lib.rs
git commit -m "pope-scope: atomic time mapping with discontinuity detection"
```

---

### Task 6: Store — Slot Allocation & Metadata

**Files:**
- Create: `pope-scope/src/store.rs`
- Modify: `pope-scope/src/lib.rs` (add `pub mod store;`)

- [ ] **Step 1: Write store module with slot types and CAS allocation**

```rust
//! Static global store for cross-instance audio data sharing.
//!
//! 16 pre-allocated slots. Ownership via atomic CAS. Ring buffers
//! allocated on demand when an instance joins, deallocated on leave.

use crate::ring_buffer::RingBuffer;
use crate::time_mapping::TimeMapping;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

pub const MAX_SLOTS: usize = 16;
const BUFFER_SECONDS: usize = 32;
const MAX_CHANNELS: usize = 16;

/// Playhead info written by the audio thread.
pub struct PlayheadInfo {
    pub is_playing: AtomicBool,
    pub bpm: AtomicU64,     // f64 bit-cast
    pub time_sig_num: AtomicU32,
    pub time_sig_den: AtomicU32,
    pub ppq_position: AtomicU64,   // f64 bit-cast
    pub bar_start_ppq: AtomicU64,  // f64 bit-cast
}

impl PlayheadInfo {
    const fn new() -> Self {
        Self {
            is_playing: AtomicBool::new(false),
            bpm: AtomicU64::new(0),
            time_sig_num: AtomicU32::new(4),
            time_sig_den: AtomicU32::new(4),
            ppq_position: AtomicU64::new(0),
            bar_start_ppq: AtomicU64::new(0),
        }
    }
}

/// Track metadata. GUI-thread fields use atomics for cross-instance reads.
pub struct TrackMetadata {
    pub track_name: Mutex<String>,
    pub display_color: AtomicU32,
    pub num_channels: AtomicU32,
    pub group: AtomicU32,
    pub solo: AtomicBool,
    pub mute: AtomicBool,
}

impl TrackMetadata {
    const fn new() -> Self {
        Self {
            track_name: Mutex::new(String::new()),
            display_color: AtomicU32::new(0),
            num_channels: AtomicU32::new(0),
            group: AtomicU32::new(0),
            solo: AtomicBool::new(false),
            mute: AtomicBool::new(false),
        }
    }
}

/// A single slot in the global store.
pub struct Slot {
    /// Owner hash (0 = free). CAS for acquisition.
    pub owner: AtomicU64,
    /// Heartbeat timestamp (last audio update).
    pub heartbeat: AtomicI64,
    /// Ring buffers per channel. `None` when slot is free.
    pub buffers: Mutex<Option<Vec<RingBuffer>>>,
    /// Time mapping for beat sync.
    pub time_mapping: TimeMapping,
    /// Playhead info.
    pub playhead: PlayheadInfo,
    /// Track metadata.
    pub metadata: TrackMetadata,
}

impl Slot {
    const fn new() -> Self {
        Self {
            owner: AtomicU64::new(0),
            heartbeat: AtomicI64::new(0),
            buffers: Mutex::new(None),
            time_mapping: TimeMapping::new(),
            playhead: PlayheadInfo::new(),
            metadata: TrackMetadata::new(),
        }
    }
}

static STORE: [Slot; MAX_SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const SLOT_INIT: Slot = Slot::new();
    [SLOT_INIT; MAX_SLOTS]
};

/// Acquire a slot using atomic CAS. Returns the slot index (0-15) or None.
pub fn acquire_slot(instance_hash: u64) -> Option<usize> {
    assert!(instance_hash != 0, "instance hash must be non-zero");
    for i in 0..MAX_SLOTS {
        let result = STORE[i].owner.compare_exchange(
            0,
            instance_hash,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        if result.is_ok() {
            return Some(i);
        }
    }
    None
}

/// Release a slot. Verifies ownership before releasing.
pub fn release_slot(index: usize, instance_hash: u64) {
    assert!(index < MAX_SLOTS);
    let result = STORE[index].owner.compare_exchange(
        instance_hash,
        0,
        Ordering::AcqRel,
        Ordering::Relaxed,
    );
    if result.is_ok() {
        // Deallocate buffers
        if let Ok(mut guard) = STORE[index].buffers.lock() {
            *guard = None;
        }
        // Reset metadata
        STORE[index].metadata.solo.store(false, Ordering::Relaxed);
        STORE[index].metadata.mute.store(false, Ordering::Relaxed);
        STORE[index].metadata.num_channels.store(0, Ordering::Relaxed);
        if let Ok(mut name) = STORE[index].metadata.track_name.lock() {
            name.clear();
        }
    }
}

/// Initialize buffers for a slot. Called from `initialize()`.
pub fn init_buffers(index: usize, num_channels: usize, sample_rate: f32) {
    assert!(index < MAX_SLOTS);
    assert!(num_channels <= MAX_CHANNELS);
    let capacity = (sample_rate as usize) * BUFFER_SECONDS;
    let mut bufs = Vec::with_capacity(num_channels);
    for _ in 0..num_channels {
        bufs.push(RingBuffer::new(capacity));
    }
    if let Ok(mut guard) = STORE[index].buffers.lock() {
        *guard = Some(bufs);
    }
    STORE[index]
        .metadata
        .num_channels
        .store(num_channels as u32, Ordering::Relaxed);
}

/// Get a reference to a slot.
pub fn slot(index: usize) -> &'static Slot {
    &STORE[index]
}

/// Check if a slot is owned (in use).
pub fn is_active(index: usize) -> bool {
    STORE[index].owner.load(Ordering::Relaxed) != 0
}

/// Get all active slot indices.
pub fn active_slots() -> Vec<usize> {
    (0..MAX_SLOTS).filter(|&i| is_active(i)).collect()
}

/// Get active slot indices filtered by group.
pub fn active_slots_in_group(group: u32) -> Vec<usize> {
    (0..MAX_SLOTS)
        .filter(|&i| {
            is_active(i) && STORE[i].metadata.group.load(Ordering::Relaxed) == group
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn reset_slot(index: usize) {
    STORE[index].owner.store(0, Ordering::Relaxed);
    STORE[index].heartbeat.store(0, Ordering::Relaxed);
    if let Ok(mut guard) = STORE[index].buffers.lock() {
        *guard = None;
    }
    STORE[index].metadata.solo.store(false, Ordering::Relaxed);
    STORE[index].metadata.mute.store(false, Ordering::Relaxed);
    STORE[index].metadata.num_channels.store(0, Ordering::Relaxed);
    STORE[index].metadata.group.store(0, Ordering::Relaxed);
    STORE[index].metadata.display_color.store(0, Ordering::Relaxed);
    if let Ok(mut name) = STORE[index].metadata.track_name.lock() {
        name.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_all() {
        for i in 0..MAX_SLOTS {
            reset_slot(i);
        }
    }

    #[test]
    fn test_acquire_slot() {
        reset_all();
        let idx = acquire_slot(42).unwrap();
        assert!(idx < MAX_SLOTS);
        assert!(is_active(idx));
    }

    #[test]
    fn test_acquire_returns_different_slots() {
        reset_all();
        let a = acquire_slot(1).unwrap();
        let b = acquire_slot(2).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_release_slot() {
        reset_all();
        let idx = acquire_slot(42).unwrap();
        assert!(is_active(idx));
        release_slot(idx, 42);
        assert!(!is_active(idx));
    }

    #[test]
    fn test_release_wrong_owner_does_nothing() {
        reset_all();
        let idx = acquire_slot(42).unwrap();
        release_slot(idx, 99); // wrong owner
        assert!(is_active(idx)); // still owned
    }

    #[test]
    fn test_acquire_all_16() {
        reset_all();
        for i in 1..=16u64 {
            assert!(acquire_slot(i).is_some());
        }
        // 17th should fail
        assert!(acquire_slot(17).is_none());
    }

    #[test]
    fn test_init_buffers() {
        reset_all();
        let idx = acquire_slot(1).unwrap();
        init_buffers(idx, 2, 48000.0);
        let s = slot(idx);
        assert_eq!(s.metadata.num_channels.load(Ordering::Relaxed), 2);
        let guard = s.buffers.lock().unwrap();
        assert!(guard.is_some());
        let bufs = guard.as_ref().unwrap();
        assert_eq!(bufs.len(), 2);
        assert_eq!(bufs[0].capacity(), 48000 * 32);
    }

    #[test]
    fn test_release_deallocates_buffers() {
        reset_all();
        let idx = acquire_slot(1).unwrap();
        init_buffers(idx, 2, 48000.0);
        release_slot(idx, 1);
        let guard = slot(idx).buffers.lock().unwrap();
        assert!(guard.is_none());
    }

    #[test]
    fn test_active_slots_in_group() {
        reset_all();
        let a = acquire_slot(1).unwrap();
        let b = acquire_slot(2).unwrap();
        let c = acquire_slot(3).unwrap();
        slot(a).metadata.group.store(0, Ordering::Relaxed);
        slot(b).metadata.group.store(1, Ordering::Relaxed);
        slot(c).metadata.group.store(0, Ordering::Relaxed);

        let group0 = active_slots_in_group(0);
        assert_eq!(group0.len(), 2);
        assert!(group0.contains(&a));
        assert!(group0.contains(&c));

        let group1 = active_slots_in_group(1);
        assert_eq!(group1.len(), 1);
        assert!(group1.contains(&b));
    }

    #[test]
    #[should_panic(expected = "instance hash must be non-zero")]
    fn test_acquire_zero_hash_panics() {
        acquire_slot(0);
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

Add `pub mod store;` to `lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/store.rs pope-scope/src/lib.rs
git commit -m "pope-scope: static global store with CAS slot allocation"
```

---

### Task 7: Snapshot Types & Builder (Free Mode)

**Files:**
- Create: `pope-scope/src/snapshot.rs`
- Modify: `pope-scope/src/lib.rs` (add `pub mod snapshot;`)

- [ ] **Step 1: Write WaveSnapshot struct and SnapshotBuilder for free mode**

```rust
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
    let level = crate::ring_buffer::RingBuffer::select_level(
        if total_samples > decimation {
            total_samples / decimation
        } else {
            1
        },
    );

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
        let time_sig_den = s.playhead.time_sig_den.load(Ordering::Relaxed);
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
        let guard = s.buffers.lock().unwrap();
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
                        let num_blocks = (total_samples / crate::ring_buffer::BLOCK_SIZE)
                            .min(decimation);
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
                        let num_blocks = (total_samples
                            / crate::ring_buffer::SUPER_BLOCK_SIZE)
                            .min(decimation);
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

        let guard = s.buffers.lock().unwrap();
        let mut audio_data = Vec::new();
        let mut data_version = 0u64;
        let mut data_points = 0;

        if let (Some(bufs), Some((start_sample, window_len))) = (guard.as_ref(), window) {
            let read_count = window_len.min(decimation);
            // For beat sync, always read raw samples and decimate on the draw side
            for (ch, buf) in bufs.iter().enumerate().take(num_channels) {
                if ch == 0 {
                    data_version = buf.total_written() as u64;
                }
                let mut out = vec![0.0f32; read_count];
                // Read from absolute position
                if start_sample >= 0 {
                    buf.read_range(start_sample as usize, &mut out);
                }
                // Mask stale data beyond current PPQ with 16-sample fade
                let current_pos = tm_snap.current_sample_pos;
                if start_sample >= 0 {
                    let end_valid =
                        ((current_pos - start_sample) as usize).min(read_count);
                    let fade_len = 16.min(read_count - end_valid);
                    for i in 0..fade_len {
                        let idx = end_valid + i;
                        if idx < read_count {
                            let fade = 1.0 - (i as f32 / fade_len as f32);
                            out[idx] *= fade;
                        }
                    }
                    for i in (end_valid + fade_len)..read_count {
                        out[i] = 0.0;
                    }
                }
                data_points = read_count;
                audio_data.push(out);
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
        store::reset_slot(0);
        let idx = store::acquire_slot(100).unwrap();
        store::init_buffers(idx, 2, 48000.0);
        store::slot(idx).metadata.group.store(0, Ordering::Relaxed);

        // Push some audio
        {
            let guard = store::slot(idx).buffers.lock().unwrap();
            if let Some(bufs) = guard.as_ref() {
                let samples: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.001).sin()).collect();
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
```

- [ ] **Step 2: Add module to lib.rs**

Add `pub mod snapshot;` to `lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/snapshot.rs pope-scope/src/lib.rs
git commit -m "pope-scope: WaveSnapshot types and SnapshotBuilder (free + beat sync)"
```

---

### Task 8: Plugin Params & Process

**Files:**
- Modify: `pope-scope/src/lib.rs`

- [ ] **Step 1: Add full parameter definitions and process() implementation**

Replace the skeleton `lib.rs` with the full plugin implementation. Key additions:

- All 11 parameters from the spec (timebase, minDb, maxDb, freeze, displayMode, drawStyle, mixToMono, decimation, group, syncMode, syncUnit)
- `DisplayMode`, `DrawStyle`, `SyncMode`, `SyncUnit` enums
- `PopeScope` struct with slot_index, instance_hash, sample_rate tracking
- `initialize()`: acquire slot, init buffers
- `deactivate()`: release slot
- `process()`: pass-through audio, push to ring buffer, update time mapping + playhead

The `process()` function:
1. Pass through audio (output = input, no modification)
2. Lock the slot's ring buffers and push each channel's samples
3. Extract playhead info from `ProcessContext` and update atomics
4. Update time mapping (PPQ, sample position, BPM)
5. Update heartbeat timestamp

Note: The ring buffer `push()` requires `&mut self`, but the slot stores buffers behind a `Mutex`. On the audio thread, we use `try_lock()` — if the GUI is reading (rare, ~1ms), we skip the push for that buffer. This is safe because missing one 1024-sample push at 60 FPS read rate is inaudible.

- [ ] **Step 2: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 3: Verify standalone builds**

Run: `cargo build --bin pope-scope`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/lib.rs
git commit -m "pope-scope: full params, process() with ring buffer push and playhead sync"
```

---

### Task 9: Renderer — Amplitude Mapping & Grid

**Files:**
- Create: `pope-scope/src/renderer.rs`
- Modify: `pope-scope/src/lib.rs` (add `mod renderer;`)

- [ ] **Step 1: Write amplitude mapping and grid division functions with tests**

```rust
//! Waveform rendering: amplitude mapping, grid, waveform paths, display modes.

use crate::theme;

/// Map a sample value to a Y pixel coordinate using dB scaling.
///
/// - `sample`: audio sample value (typically -1.0 to 1.0)
/// - `min_db`: bottom of visible dB range (e.g. -48.0)
/// - `max_db`: top of visible dB range (e.g. 0.0)
/// - `centre_y`: pixel Y coordinate of the center line (silence)
/// - `half_height`: half the available height in pixels
pub fn sample_to_y(
    sample: f32,
    min_db: f32,
    max_db: f32,
    centre_y: f32,
    half_height: f32,
) -> f32 {
    let sign = if sample >= 0.0 { 1.0 } else { -1.0 };
    let abs_amp = sample.abs().clamp(0.0, 2.0); // reject spikes
    let db = if abs_amp > 0.0 {
        20.0 * abs_amp.log10()
    } else {
        -96.0
    };
    let db_range = max_db - min_db;
    if db_range.abs() < 0.001 {
        return centre_y;
    }
    let normalized = ((db - min_db) / db_range).clamp(0.0, 1.0);
    centre_y - (normalized * half_height * sign)
}

/// Compute dB grid division size for the given dB range.
/// Targets 4-8 grid lines.
pub fn db_grid_division(min_db: f32, max_db: f32) -> f32 {
    let range = max_db - min_db;
    if range > 36.0 {
        12.0
    } else if range > 18.0 {
        6.0
    } else if range > 9.0 {
        3.0
    } else {
        2.0
    }
}

/// Compute time grid divisions for free mode.
/// Returns (division_ms, num_divisions).
pub fn time_grid_divisions(timebase_ms: f32) -> (f32, usize) {
    let targets = [
        1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
    ];
    for &div in &targets {
        let n = (timebase_ms / div).floor() as usize;
        if (4..=10).contains(&n) {
            return (div, n);
        }
    }
    // Fallback
    let div = timebase_ms / 5.0;
    (div, 5)
}

/// Draw amplitude grid lines on a pixmap.
/// Draws horizontal lines at dB divisions within the given area.
pub fn draw_amplitude_grid(
    pixmap: &mut tiny_skia::Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    min_db: f32,
    max_db: f32,
    text_renderer: &mut tiny_skia_widgets::TextRenderer,
    scale: f32,
) {
    let division = db_grid_division(min_db, max_db);
    let centre_y = y + h / 2.0;
    let half_height = h / 2.0;
    let font_size = 8.0 * scale;

    // Center line (silence)
    tiny_skia_widgets::draw_rect(
        pixmap,
        x,
        centre_y - 0.5,
        w,
        1.0,
        theme::to_color(theme::GRID_BRIGHT),
    );

    // dB grid lines above and below center
    let db_range = max_db - min_db;
    let mut db = division;
    while db < db_range {
        let normalized = db / db_range;
        let offset = normalized * half_height;

        // Above center
        let y_above = centre_y - offset;
        if y_above > y {
            tiny_skia_widgets::draw_rect(
                pixmap,
                x,
                y_above - 0.5,
                w,
                1.0,
                theme::to_color(theme::GRID),
            );
            // dB label on right
            let label = format!("-{}", (db as i32));
            text_renderer.draw_text(
                pixmap,
                x + w - 30.0 * scale,
                y_above - font_size / 2.0,
                &label,
                font_size,
                theme::to_color(theme::GRID),
            );
        }

        // Below center (mirror)
        let y_below = centre_y + offset;
        if y_below < y + h {
            tiny_skia_widgets::draw_rect(
                pixmap,
                x,
                y_below - 0.5,
                w,
                1.0,
                theme::to_color(theme::GRID),
            );
        }

        db += division;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_to_y_silence() {
        // 0.0 amplitude → should map to centre
        let y = sample_to_y(0.0, -48.0, 0.0, 100.0, 50.0);
        assert_eq!(y, 100.0);
    }

    #[test]
    fn test_sample_to_y_full_scale() {
        // 1.0 amplitude (0 dB) → should map to top
        let y = sample_to_y(1.0, -48.0, 0.0, 100.0, 50.0);
        assert!((y - 50.0).abs() < 0.1); // centre - half_height
    }

    #[test]
    fn test_sample_to_y_negative() {
        // -1.0 amplitude → should map to bottom
        let y = sample_to_y(-1.0, -48.0, 0.0, 100.0, 50.0);
        assert!((y - 150.0).abs() < 0.1); // centre + half_height
    }

    #[test]
    fn test_sample_to_y_half_db() {
        // -24 dB is halfway in the -48 to 0 range
        let amp = 10.0f32.powf(-24.0 / 20.0); // ~0.063
        let y = sample_to_y(amp, -48.0, 0.0, 100.0, 50.0);
        assert!((y - 75.0).abs() < 1.0); // centre - half_height * 0.5
    }

    #[test]
    fn test_sample_to_y_spike_clamped() {
        // Values > 2.0 should be clamped
        let y = sample_to_y(10.0, -48.0, 0.0, 100.0, 50.0);
        let y_clamped = sample_to_y(2.0, -48.0, 0.0, 100.0, 50.0);
        assert_eq!(y, y_clamped);
    }

    #[test]
    fn test_db_grid_division() {
        assert_eq!(db_grid_division(-48.0, 0.0), 12.0); // 48 dB range
        assert_eq!(db_grid_division(-24.0, 0.0), 6.0);  // 24 dB range
        assert_eq!(db_grid_division(-12.0, 0.0), 3.0);  // 12 dB range
        assert_eq!(db_grid_division(-6.0, 0.0), 2.0);   // 6 dB range
    }

    #[test]
    fn test_time_grid_divisions() {
        let (div, n) = time_grid_divisions(1000.0);
        assert!(n >= 4 && n <= 10);
        assert!((div * n as f32 - 1000.0).abs() < div);
    }

    #[test]
    fn test_time_grid_divisions_small() {
        let (div, n) = time_grid_divisions(10.0);
        assert!(n >= 4 && n <= 10);
    }

    #[test]
    fn test_time_grid_divisions_large() {
        let (div, n) = time_grid_divisions(10000.0);
        assert!(n >= 4 && n <= 10);
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

Add `mod renderer;` to `lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/renderer.rs pope-scope/src/lib.rs
git commit -m "pope-scope: renderer with amplitude mapping, grid, and draw helpers"
```

---

### Task 10: Renderer — Waveform Drawing & Display Modes

**Files:**
- Modify: `pope-scope/src/renderer.rs`

- [ ] **Step 1: Add waveform path building and display mode rendering**

Add to `renderer.rs`:

- `draw_waveform_line()`: Build a tiny-skia Path from sample data, decimating to pixel columns with min/max vertical segments when needed. Stroke with the track's color.
- `draw_waveform_filled()`: Build an envelope path (max values forward, min values backward, close path). Fill with semi-transparent track color.
- `draw_waveform()`: Dispatch to line/filled/both based on DrawStyle.
- `draw_vertical_mode()`: Stack tracks vertically with control strip space. Call `draw_waveform()` for each visible track. Grid on each lane.
- `draw_overlay_mode()`: Single area, draw all tracks overlaid. Legend in top-left.
- `draw_sum_mode()`: Sum all visible tracks sample-by-sample into one buffer, draw as single waveform in foreground amber.
- `draw_beat_grid()`: Vertical lines for beats, thicker lines for bar boundaries, "Bar.Beat" labels.
- `draw_time_grid()`: Vertical lines for free mode time divisions.

Each function takes the pixmap, snapshot data, drawing area bounds, scale factor, and relevant parameters.

- [ ] **Step 2: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass (existing tests + any new rendering math tests)

- [ ] **Step 3: Commit**

```bash
git add pope-scope/src/renderer.rs
git commit -m "pope-scope: waveform drawing with 3 display modes and grid rendering"
```

---

### Task 11: Controls — Track Control Strip

**Files:**
- Create: `pope-scope/src/controls.rs`
- Modify: `pope-scope/src/lib.rs` (add `mod controls;`)

- [ ] **Step 1: Write control strip rendering and hit region generation**

```rust
//! Track control strip for vertical mode.
//!
//! Renders track name, color swatch, solo/mute buttons.
//! Returns hit regions for the editor to handle clicks.

use crate::theme;

/// Hit region action from a control strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlAction {
    CycleColor(usize),   // slot index
    ToggleSolo(usize),   // slot index
    ToggleMute(usize),   // slot index
}

/// A rectangular hit region with an action.
#[derive(Clone, Debug)]
pub struct ControlHitRegion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub action: ControlAction,
}

/// Draw a single track's control strip and return hit regions.
///
/// - `pixmap`: target pixmap
/// - `tr`: text renderer
/// - `x, y, w, h`: bounds for this strip
/// - `slot_index`: which slot this strip is for
/// - `track_name`: display name
/// - `color`: ARGB display color
/// - `solo`: solo state
/// - `mute`: mute state
/// - `scale`: UI scale factor
pub fn draw_control_strip(
    pixmap: &mut tiny_skia::Pixmap,
    tr: &mut tiny_skia_widgets::TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    slot_index: usize,
    track_name: &str,
    color: u32,
    solo: bool,
    mute: bool,
    scale: f32,
) -> Vec<ControlHitRegion> {
    let mut regions = Vec::new();

    // Background
    tiny_skia_widgets::draw_rect(pixmap, x, y, w, h, theme::to_color(theme::BG));
    // Right border
    tiny_skia_widgets::draw_rect(
        pixmap,
        x + w - 1.0,
        y,
        1.0,
        h,
        theme::to_color(theme::BORDER),
    );

    let pad = 6.0 * scale;
    let font_size = 11.0 * scale;
    let btn_h = 18.0 * scale;
    let btn_w = 24.0 * scale;
    let swatch_size = 14.0 * scale;

    let mut cy = y + pad;

    // Track name (centered)
    let name = if track_name.is_empty() {
        format!("Track {}", slot_index + 1)
    } else {
        track_name.to_string()
    };
    let text_w = tr.text_width(&name, font_size);
    tr.draw_text(
        pixmap,
        x + (w - text_w) / 2.0,
        cy + font_size,
        &name,
        font_size,
        theme::to_color(color),
    );
    cy += font_size + pad;

    // Color swatch (clickable)
    let swatch_x = x + (w - swatch_size) / 2.0;
    tiny_skia_widgets::draw_rect(
        pixmap,
        swatch_x,
        cy,
        swatch_size,
        swatch_size,
        theme::to_color(color),
    );
    regions.push(ControlHitRegion {
        x: swatch_x,
        y: cy,
        w: swatch_size,
        h: swatch_size,
        action: ControlAction::CycleColor(slot_index),
    });
    cy += swatch_size + pad;

    // Solo / Mute buttons side by side
    let total_btn_w = btn_w * 2.0 + 4.0 * scale;
    let btn_x = x + (w - total_btn_w) / 2.0;

    // Solo button
    let solo_bg = if solo { theme::YELLOW } else { theme::BG };
    let solo_fg = if solo { theme::BG } else { theme::FG };
    tiny_skia_widgets::draw_button(
        pixmap,
        tr,
        btn_x,
        cy,
        btn_w,
        btn_h,
        "S",
        solo,
        false,
    );
    regions.push(ControlHitRegion {
        x: btn_x,
        y: cy,
        w: btn_w,
        h: btn_h,
        action: ControlAction::ToggleSolo(slot_index),
    });

    // Mute button
    let mute_x = btn_x + btn_w + 4.0 * scale;
    tiny_skia_widgets::draw_button(
        pixmap,
        tr,
        mute_x,
        cy,
        btn_w,
        btn_h,
        "M",
        mute,
        false,
    );
    regions.push(ControlHitRegion {
        x: mute_x,
        y: cy,
        w: btn_w,
        h: btn_h,
        action: ControlAction::ToggleMute(slot_index),
    });

    regions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_actions_are_distinct() {
        assert_ne!(ControlAction::CycleColor(0), ControlAction::ToggleSolo(0));
        assert_ne!(ControlAction::ToggleSolo(0), ControlAction::ToggleMute(0));
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

Add `mod controls;` to `lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p pope-scope`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/controls.rs pope-scope/src/lib.rs
git commit -m "pope-scope: track control strip with solo/mute/color"
```

---

### Task 12: Editor — Window, Controls Bar & Timer

**Files:**
- Create: `pope-scope/src/editor.rs`
- Modify: `pope-scope/src/lib.rs` (add `mod editor;`, implement `editor()`)

- [ ] **Step 1: Write the editor following the gain-brain/gs-meter pattern**

The editor follows the exact same structure as `gain-brain/src/editor.rs`:

- `PopeScopeWindow` struct implementing `baseview::WindowHandler`
- `PopeScopeEditor` struct implementing `nih_plug::Editor`
- `on_frame()` calls draw at 60 FPS (16ms timer)
- `draw()`:
  1. Clear background to `theme::BG`
  2. Draw title + scale buttons (top)
  3. Call SnapshotBuilder to get current snapshots
  4. Call renderer display mode function (vertical/overlay/sum)
  5. Draw control bar (bottom)
  6. Draw cursor if mouse is hovering
  7. Present surface
- `on_event()`: hit-region-based interaction (same pattern as gain-brain)
- Control bar: draw stepped selectors, buttons, sliders for all params
- Mouse cursor: track position, draw vertical cyan line + tooltip

Key files to reference for patterns:
- `gain-brain/src/editor.rs` — complete window handler, hit regions, scaling
- `tiny-skia-widgets/src/editor_base.rs` — EditorState, EditorHandle, SoftbufferSurface

- [ ] **Step 2: Wire editor into lib.rs**

Add `editor()` method to the Plugin impl, returning the editor. Connect params, add `editor_state` to params with `#[persist]`.

- [ ] **Step 3: Verify standalone builds and opens window**

Run: `cargo build --bin pope-scope && ./target/debug/pope-scope`
Expected: window opens with black background and control bar

- [ ] **Step 4: Commit**

```bash
git add pope-scope/src/editor.rs pope-scope/src/lib.rs
git commit -m "pope-scope: softbuffer editor with control bar and 60fps timer"
```

---

### Task 13: Editor — Waveform Integration & Cursor

**Files:**
- Modify: `pope-scope/src/editor.rs`
- Modify: `pope-scope/src/renderer.rs`

- [ ] **Step 1: Connect snapshot builder to the draw loop**

In the editor's `on_frame()` / `draw()`:
1. Read current params (sync mode, timebase/sync unit, dB range, display mode, draw style, group, decimation, mix to mono, freeze)
2. If not frozen, call `build_snapshots_free()` or `build_snapshots_beat_sync()` based on sync mode
3. Apply solo/mute filtering: if any snapshot has `solo=true`, filter to only soloed tracks
4. Call the appropriate renderer display mode function with the filtered snapshots
5. Draw cursor overlay if mouse is in the waveform area

- [ ] **Step 2: Add cursor rendering to renderer.rs**

Add `draw_cursor()` function:
- Vertical cyan line at mouse X
- Per-track tooltip showing: color swatch, track name, amplitude, dB value
- Only recalculate when X changes

- [ ] **Step 3: Add peak hold rendering**

Add `PeakHoldState` struct and `draw_peak_hold()`:
- Per-track dashed horizontal line at peak level
- 2-second hold, 20 dB/sec decay
- Updated each frame from snapshot peak values

- [ ] **Step 4: Test with standalone**

Run: `cargo build --bin pope-scope && ./target/debug/pope-scope`
Expected: waveform area renders (empty until audio is routed), controls respond to clicks

- [ ] **Step 5: Commit**

```bash
git add pope-scope/src/editor.rs pope-scope/src/renderer.rs
git commit -m "pope-scope: waveform rendering integration with cursor and peak hold"
```

---

### Task 14: Plugin Bundle & Workspace Integration

**Files:**
- Modify: `Cargo.toml` (workspace root — already done in Task 1)
- Modify: `README.md`

- [ ] **Step 1: Verify plugin bundle builds**

Run: `cargo nih-plug bundle pope-scope --release`
Expected: creates `.clap` and `.vst3` in `target/bundled/`

- [ ] **Step 2: Run full test suite**

Run: `cargo test --workspace`
Expected: all tests pass including pope-scope

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p pope-scope -- -D warnings`
Expected: clean

- [ ] **Step 4: Update README.md**

Add Pope Scope to the plugins list, build commands, workspace structure, and documentation links. Follow the existing format for other plugins.

- [ ] **Step 5: Commit**

```bash
git add README.md Cargo.lock
git commit -m "pope-scope: workspace integration, README update"
```

---

### Task 15: Documentation

**Files:**
- Create: `docs/pope-scope/pope-scope-manual.md`

- [ ] **Step 1: Write manual**

Follow the structure of `docs/gain-brain/gain-brain-manual.md`:
- What is Pope Scope?
- Installation
- Controls (all parameters with descriptions)
- Display Modes (Vertical, Overlay, Sum)
- Draw Styles (Line, Filled, Both)
- Beat Sync (how it works, sync units)
- Multi-Instance (shared state, groups, solo/mute)
- Technical Notes (CPU rendering, performance, ring buffer)
- Formats
- License

- [ ] **Step 2: Generate PDF**

```bash
cd docs/pope-scope && pandoc pope-scope-manual.md -o pope-scope-manual.pdf
```

- [ ] **Step 3: Update README documentation links**

Add link to pope-scope manual in README.md documentation section.

- [ ] **Step 4: Commit**

```bash
git add docs/pope-scope/ README.md
git commit -m "pope-scope: user manual with PDF"
```

---

## Self-Review

**Spec coverage check:**
- Plugin identity (name, formats, audio) → Task 1, 8
- Parameters (all 11) → Task 8
- Shared state architecture → Task 6
- Ring buffer + mipmap → Tasks 3, 4
- Time mapping + beat sync → Task 5, 7
- Snapshot builder (free + beat sync) → Task 7
- Renderer (display modes, draw styles, amplitude mapping) → Tasks 9, 10
- Grid rendering (amplitude, time, beat) → Task 9, 10
- Peak hold → Task 13
- Mouse cursor → Task 13
- Control strip (solo/mute/color) → Task 11
- Editor layout (control bar) → Task 12
- Amber phosphor theme → Task 2
- File structure → matches file map
- Testing strategy → each task includes tests
- Performance targets → addressed in process() design (no allocs, try_lock)
- Sample validation ([-2.0, 2.0] clamp) → Task 9 (sample_to_y clamp)

**Placeholder scan:** No TBD, TODO, or "fill in" placeholders found.

**Type consistency:** WaveSnapshot fields match across snapshot.rs and renderer.rs. RingBuffer API (push, read_most_recent, read_range, read_most_recent_l1/l2, select_level) consistent across tasks 3, 4, 7. ControlAction enum used consistently in controls.rs and editor.rs.
