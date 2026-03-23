# Gain Brain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a lightweight gain utility plugin with cross-instance group linking via memory-mapped IPC, CPU-rendered GUI.

**Architecture:** New workspace crate `gain-brain/`. The group IPC layer (`groups.rs`) uses `memmap2` to share a 272-byte file across all instances. The plugin (`lib.rs`) reads/writes group state once per buffer. The GUI (`editor.rs`) follows GS Meter's softbuffer + tiny-skia pattern. Widgets are copied from GS Meter.

**Tech Stack:** Rust nightly, nih-plug, memmap2, softbuffer, tiny-skia, fontdue, baseview

**Spec:** `docs/superpowers/specs/2026-03-22-gain-brain-design.md`

---

## File Structure

```
gain-brain/
├── Cargo.toml              — crate manifest, dependencies
├── src/
│   ├── lib.rs              — GainBrain plugin struct, params, process()
│   ├── main.rs             — standalone entry point (4 lines)
│   ├── groups.rs           — mmap IPC: GroupFile, read/write slots, init
│   ├── editor.rs           — softbuffer GUI: window, draw, events
│   └── widgets.rs          — copied from gs-meter/src/widgets.rs
```

Also modified:
- `Cargo.toml` (workspace root) — add `gain-brain` to members
- `CLAUDE.md` — already updated with unsafe exception
- `.github/workflows/build.yml` — add gain-brain bundle step

---

### Task 1: Scaffold the crate and verify it compiles

**Files:**
- Create: `gain-brain/Cargo.toml`
- Create: `gain-brain/src/lib.rs`
- Create: `gain-brain/src/main.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create `gain-brain/Cargo.toml`**

```toml
[package]
name = "gain-brain"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]

[[bin]]
name = "gain-brain"
path = "src/main.rs"

[dependencies]
nih_plug = { git = "https://github.com/davemollen/nih-plug.git", branch = "finish-vst3-pr", features = ["standalone"] }
memmap2 = "0.9"
baseview = { git = "https://github.com/RustAudio/baseview.git", rev = "9a0b42c09d712777b2edb4c5e0cb6baf21e988f0", features = ["opengl"] }
softbuffer = { version = "0.4", default-features = false, features = ["kms", "x11"] }
raw-window-handle = "0.5"
raw-window-handle-06 = { package = "raw-window-handle", version = "0.6" }
tiny-skia = "0.11"
fontdue = "0.9"
keyboard-types = "0.6"
crossbeam = "0.8"
serde = { version = "1.0", features = ["derive"] }

[package.metadata.bundler]
name = "Gain Brain"
company = "mpd"
description = "A gain utility with cross-instance group linking"
license = "GPL-3.0-or-later"
version = "0.1.0"
```

- [ ] **Step 2: Create minimal `gain-brain/src/lib.rs`**

A bare-minimum plugin that passes audio through unchanged:

```rust
use nih_plug::prelude::*;
use std::sync::Arc;

mod editor;
pub mod groups;
pub mod widgets;

pub struct GainBrain {
    params: Arc<GainBrainParams>,
}

#[derive(Params)]
pub struct GainBrainParams {
    #[id = "gain"]
    pub gain: FloatParam,
}

impl Default for GainBrain {
    fn default() -> Self {
        Self {
            params: Arc::new(GainBrainParams::new()),
        }
    }
}

impl GainBrainParams {
    fn new() -> Self {
        Self {
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-60.0),
                    max: util::db_to_gain(60.0),
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

impl Plugin for GainBrain {
    const NAME: &'static str = "Gain Brain";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }

        #[allow(clippy::needless_range_loop)]
        for i in 0..num_samples {
            let gain = self.params.gain.smoothed.next();
            channel_slices[0][i] *= gain;
            channel_slices[1][i] *= gain;
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for GainBrain {
    const CLAP_ID: &'static str = "com.mpd.gain-brain";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A gain utility with cross-instance group linking");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for GainBrain {
    const VST3_CLASS_ID: [u8; 16] = *b"GainBrainMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Tools,
    ];
}

nih_export_clap!(GainBrain);
nih_export_vst3!(GainBrain);
```

- [ ] **Step 3: Create `gain-brain/src/main.rs`**

```rust
use nih_plug::prelude::*;

fn main() {
    nih_export_standalone::<gain_brain::GainBrain>();
}
```

- [ ] **Step 4: Create stub `gain-brain/src/groups.rs`**

```rust
//! Cross-instance group IPC via memory-mapped file.
```

- [ ] **Step 5: Create stub `gain-brain/src/editor.rs`**

```rust
//! Softbuffer-based editor for gain-brain. CPU rendering via tiny-skia.
```

- [ ] **Step 6: Copy `gs-meter/src/widgets.rs` to `gain-brain/src/widgets.rs`**

Copy the entire file. Also copy `gs-meter/src/fonts/DejaVuSans.ttf` to `gain-brain/src/fonts/DejaVuSans.ttf`.

- [ ] **Step 7: Add `gain-brain` to workspace root `Cargo.toml`**

Change the members line:
```toml
members = ["nih-plug-widgets", "wavetable-filter", "gs-meter", "gain-brain", "xtask"]
```

- [ ] **Step 8: Verify it compiles**

Run: `cargo check --package gain-brain`
Expected: compiles (editor module is a stub, no GUI yet)

- [ ] **Step 9: Commit**

```
feat: scaffold gain-brain crate with minimal pass-through plugin
```

---

### Task 2: Implement groups.rs — mmap IPC layer with tests (TDD)

**Files:**
- Create/modify: `gain-brain/src/groups.rs`

This is the core IPC mechanism. Build it test-first. The `GroupFile` struct wraps the mmap and provides safe read/write methods for group slots.

- [ ] **Step 1: Write the tests first**

```rust
//! Cross-instance group IPC via memory-mapped file.

use memmap2::MmapMut;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// File layout constants.
const MAGIC: &[u8; 4] = b"GBRN";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 16;
const SLOT_SIZE: usize = 16;
const NUM_GROUPS: usize = 16;
const FILE_SIZE: usize = HEADER_SIZE + NUM_GROUPS * SLOT_SIZE;

/// A group slot's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupSlot {
    pub gain_millibels: i32,
    pub generation: u32,
}

/// Handle to the shared memory-mapped group file.
pub struct GroupFile {
    mmap: MmapMut,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a GroupFile backed by a temp file.
    fn temp_group_file() -> (GroupFile, PathBuf) {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gain-brain-test-{}", std::process::id()));
        // Clean up any leftover file
        let _ = fs::remove_file(&path);
        let gf = GroupFile::open(&path).expect("failed to open group file");
        (gf, path)
    }

    #[test]
    fn test_create_and_verify_header() {
        let (gf, path) = temp_group_file();
        assert_eq!(&gf.mmap[0..4], MAGIC);
        let version = u32::from_le_bytes(gf.mmap[4..8].try_into().unwrap());
        assert_eq!(version, VERSION);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_read_default_slot_is_zero() {
        let (gf, path) = temp_group_file();
        let slot = gf.read_slot(1);
        assert_eq!(slot.gain_millibels, 0);
        assert_eq!(slot.generation, 0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_write_and_read_slot() {
        let (mut gf, path) = temp_group_file();
        gf.write_slot(3, 1450); // +14.50 dB
        let slot = gf.read_slot(3);
        assert_eq!(slot.gain_millibels, 1450);
        assert_eq!(slot.generation, 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_generation_increments() {
        let (mut gf, path) = temp_group_file();
        gf.write_slot(1, 100);
        gf.write_slot(1, 200);
        gf.write_slot(1, 300);
        let slot = gf.read_slot(1);
        assert_eq!(slot.gain_millibels, 300);
        assert_eq!(slot.generation, 3);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_slots_are_independent() {
        let (mut gf, path) = temp_group_file();
        gf.write_slot(1, 100);
        gf.write_slot(2, 200);
        assert_eq!(gf.read_slot(1).gain_millibels, 100);
        assert_eq!(gf.read_slot(2).gain_millibels, 200);
        assert_eq!(gf.read_slot(3).gain_millibels, 0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_two_handles_share_state() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gain-brain-test-shared-{}", std::process::id()));
        let _ = fs::remove_file(&path);

        let mut gf1 = GroupFile::open(&path).unwrap();
        let gf2 = GroupFile::open(&path).unwrap();

        gf1.write_slot(5, -1400); // -14.00 dB
        let slot = gf2.read_slot(5);
        assert_eq!(slot.gain_millibels, -1400);
        assert_eq!(slot.generation, 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_group_0_panics_on_read() {
        let (gf, path) = temp_group_file();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gf.read_slot(0)));
        assert!(result.is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_group_17_panics_on_read() {
        let (gf, path) = temp_group_file();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gf.read_slot(17)));
        assert!(result.is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_version_mismatch_returns_error() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gain-brain-test-ver-{}", std::process::id()));
        let _ = fs::remove_file(&path);

        // Create a file with wrong version
        let file = OpenOptions::new()
            .read(true).write(true).create(true).truncate(true)
            .open(&path).unwrap();
        file.set_len(FILE_SIZE as u64).unwrap();
        let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
        mmap[0..4].copy_from_slice(MAGIC);
        mmap[4..8].copy_from_slice(&99u32.to_le_bytes()); // wrong version
        mmap.flush().unwrap();
        drop(mmap);
        drop(file);

        let result = GroupFile::open(&path);
        assert!(result.is_err());
        let _ = fs::remove_file(&path);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package gain-brain`
Expected: FAIL — `GroupFile::open`, `read_slot`, `write_slot` not implemented

- [ ] **Step 3: Implement GroupFile**

```rust
impl GroupFile {
    /// Open or create the shared group file at the given path.
    /// Returns an error if the file exists but has an incompatible version.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("failed to open group file: {e}"))?;

        let metadata = file.metadata().map_err(|e| format!("failed to read metadata: {e}"))?;

        if metadata.len() == 0 {
            // New file — initialize it
            file.set_len(FILE_SIZE as u64)
                .map_err(|e| format!("failed to set file size: {e}"))?;
            let mut mmap = unsafe { MmapMut::map_mut(&file) }
                .map_err(|e| format!("failed to mmap: {e}"))?;
            mmap[0..4].copy_from_slice(MAGIC);
            mmap[4..8].copy_from_slice(&VERSION.to_le_bytes());
            // Rest is already zeroed by set_len
            mmap.flush().map_err(|e| format!("failed to flush: {e}"))?;
            Ok(Self { mmap })
        } else {
            // Existing file — validate header
            if (metadata.len() as usize) < FILE_SIZE {
                return Err("group file too small".to_string());
            }
            let mmap = unsafe { MmapMut::map_mut(&file) }
                .map_err(|e| format!("failed to mmap: {e}"))?;
            if &mmap[0..4] != MAGIC {
                return Err("group file has invalid magic".to_string());
            }
            let version = u32::from_le_bytes(mmap[4..8].try_into().unwrap());
            if version != VERSION {
                return Err(format!("group file version {version} != expected {VERSION}"));
            }
            Ok(Self { mmap })
        }
    }

    /// Get the default file path for this platform.
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
                return PathBuf::from(dir).join("gain-brain-groups");
            }
            PathBuf::from("/tmp/gain-brain-groups")
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(dir) = std::env::var("TMPDIR") {
                return PathBuf::from(dir).join("gain-brain-groups");
            }
            PathBuf::from("/tmp/gain-brain-groups")
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(dir) = std::env::var("LOCALAPPDATA") {
                return PathBuf::from(dir).join("Temp").join("gain-brain-groups");
            }
            PathBuf::from("C:\\Temp\\gain-brain-groups")
        }
    }

    fn slot_offset(group: u8) -> usize {
        assert!(group >= 1 && group <= 16, "group must be 1-16, got {group}");
        HEADER_SIZE + (group as usize - 1) * SLOT_SIZE
    }

    /// Read a group slot. Group must be 1-16.
    pub fn read_slot(&self, group: u8) -> GroupSlot {
        let off = Self::slot_offset(group);
        let gain_millibels = i32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap());
        let generation = u32::from_le_bytes(self.mmap[off + 4..off + 8].try_into().unwrap());
        GroupSlot { gain_millibels, generation }
    }

    /// Write a gain value to a group slot and increment generation.
    /// Group must be 1-16.
    pub fn write_slot(&mut self, group: u8, gain_millibels: i32) {
        let off = Self::slot_offset(group);
        let old_gen = u32::from_le_bytes(self.mmap[off + 4..off + 8].try_into().unwrap());
        self.mmap[off..off + 4].copy_from_slice(&gain_millibels.to_le_bytes());
        self.mmap[off + 4..off + 8].copy_from_slice(&(old_gen + 1).to_le_bytes());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --package gain-brain`
Expected: all tests pass

- [ ] **Step 5: Commit**

```
feat: implement groups.rs mmap IPC layer with tests
```

---

### Task 3: Add group and link_mode parameters to the plugin

**Files:**
- Modify: `gain-brain/src/lib.rs`

- [ ] **Step 1: Add LinkMode enum and parameters**

Add above the `GainBrain` struct:

```rust
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum LinkMode {
    #[id = "off"]
    #[name = "Off"]
    Off,

    #[id = "absolute"]
    #[name = "Abs"]
    Absolute,

    #[id = "relative"]
    #[name = "Rel"]
    Relative,
}
```

Add to `GainBrainParams`:

```rust
#[id = "group"]
pub group: IntParam,

#[id = "link_mode"]
pub link_mode: EnumParam<LinkMode>,
```

Initialize in `GainBrainParams::new()`:

```rust
group: IntParam::new("Group", 0, IntRange::Linear { min: 0, max: 16 }),
link_mode: EnumParam::new("Link", LinkMode::Off),
```

- [ ] **Step 2: Add group IPC state to GainBrain struct**

```rust
pub struct GainBrain {
    params: Arc<GainBrainParams>,
    group_file: Option<groups::GroupFile>,
    last_seen_generation: u32,
    last_sent_gain_millibels: i32,
    last_group: i32,
    last_link_mode: LinkMode,
}
```

Update `Default`:

```rust
impl Default for GainBrain {
    fn default() -> Self {
        let group_file = groups::GroupFile::open(&groups::GroupFile::default_path()).ok();
        Self {
            params: Arc::new(GainBrainParams::new()),
            group_file,
            last_seen_generation: 0,
            last_sent_gain_millibels: 0,
            last_group: 0,
            last_link_mode: LinkMode::Off,
        }
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --package gain-brain`

- [ ] **Step 4: Commit**

```
feat: add group, link_mode parameters and GroupFile to GainBrain
```

---

### Task 4: Implement group sync logic in process()

**Files:**
- Modify: `gain-brain/src/lib.rs`

- [ ] **Step 1: Add helper to convert between dB and millibels**

```rust
fn db_to_millibels(db: f32) -> i32 {
    (db * 100.0).round() as i32
}

fn millibels_to_db(mb: i32) -> f32 {
    mb as f32 / 100.0
}
```

- [ ] **Step 2: Write tests for the sync logic helpers**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_to_millibels() {
        assert_eq!(db_to_millibels(3.5), 350);
        assert_eq!(db_to_millibels(-14.0), -1400);
        assert_eq!(db_to_millibels(0.0), 0);
    }

    #[test]
    fn test_millibels_to_db() {
        assert!((millibels_to_db(350) - 3.5).abs() < 0.01);
        assert!((millibels_to_db(-1400) - (-14.0)).abs() < 0.01);
        assert!((millibels_to_db(0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_millibels_roundtrip() {
        for db in [-60.0, -14.0, 0.0, 3.5, 60.0] {
            let mb = db_to_millibels(db);
            let back = millibels_to_db(mb);
            assert!((back - db).abs() < 0.01, "roundtrip failed for {db}");
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --package gain-brain`
Expected: all pass

- [ ] **Step 4: Implement group sync in process()**

Replace the process() method with the full group-aware version. Key logic:

```rust
fn process(
    &mut self,
    buffer: &mut Buffer,
    _aux: &mut AuxiliaryBuffers,
    _context: &mut impl ProcessContext<Self>,
) -> ProcessStatus {
    let num_samples = buffer.samples();
    let channel_slices = buffer.as_slice();
    if channel_slices.len() < 2 {
        return ProcessStatus::Normal;
    }

    // ── Group sync (once per buffer, before gain application) ──
    let group = self.params.group.value();
    let link_mode = self.params.link_mode.value();

    if let Some(ref mut gf) = self.group_file {
        // Detect group or mode change
        let group_changed = group != self.last_group;
        let mode_changed = link_mode != self.last_link_mode;

        if group_changed || mode_changed {
            self.handle_group_change(gf, group, link_mode);
            self.last_group = group;
            self.last_link_mode = link_mode;
        }

        // Sync from group if active
        if group >= 1 && group <= 16 && link_mode != LinkMode::Off {
            let slot = gf.read_slot(group as u8);
            if slot.generation != self.last_seen_generation {
                if slot.gain_millibels != self.last_sent_gain_millibels {
                    match link_mode {
                        LinkMode::Absolute => {
                            let db = millibels_to_db(slot.gain_millibels);
                            let linear = util::db_to_gain(db.clamp(-60.0, 60.0));
                            self.params.gain.set_from_normalized(
                                self.params.gain.preview_normalized(linear),
                            );
                        }
                        LinkMode::Relative => {
                            let delta_mb = slot.gain_millibels - self.last_sent_gain_millibels;
                            let current_db = util::gain_to_db(self.params.gain.value());
                            let new_db = (current_db + millibels_to_db(delta_mb)).clamp(-60.0, 60.0);
                            let linear = util::db_to_gain(new_db);
                            self.params.gain.set_from_normalized(
                                self.params.gain.preview_normalized(linear),
                            );
                        }
                        LinkMode::Off => {}
                    }
                    self.last_sent_gain_millibels = slot.gain_millibels;
                }
                self.last_seen_generation = slot.generation;
            }
        }
    }

    // ── Apply gain ──
    #[allow(clippy::needless_range_loop)]
    for i in 0..num_samples {
        let gain = self.params.gain.smoothed.next();
        channel_slices[0][i] *= gain;
        channel_slices[1][i] *= gain;
    }

    ProcessStatus::Normal
}
```

- [ ] **Step 5: Implement handle_group_change helper**

```rust
impl GainBrain {
    fn handle_group_change(&mut self, gf: &mut groups::GroupFile, group: i32, link_mode: LinkMode) {
        if group < 1 || group > 16 || link_mode == LinkMode::Off {
            // Leaving group or turning off linking — keep current gain
            return;
        }

        let slot = gf.read_slot(group as u8);

        match link_mode {
            LinkMode::Absolute => {
                // Adopt the group's current gain
                let db = millibels_to_db(slot.gain_millibels);
                let linear = util::db_to_gain(db.clamp(-60.0, 60.0));
                self.params.gain.set_from_normalized(
                    self.params.gain.preview_normalized(linear),
                );
                self.last_sent_gain_millibels = slot.gain_millibels;
            }
            LinkMode::Relative => {
                // Keep current gain, baseline to group's value
                self.last_sent_gain_millibels = slot.gain_millibels;
            }
            LinkMode::Off => {}
        }
        self.last_seen_generation = slot.generation;
    }
}
```

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `cargo test --package gain-brain`
Expected: all pass

Note: `set_from_normalized` may not exist on `FloatParam` in this nih-plug fork. If so, the implementer should check the nih-plug API and find the correct method for programmatically setting a parameter value from the audio thread. Alternatives include storing the target gain in a separate atomic and reading it in the smoother, or using an `Arc<AtomicF32>` sidecar.

- [ ] **Step 7: Commit**

```
feat: implement group sync logic in process()
```

---

### Task 5: Implement gain-change-to-group write path

**Files:**
- Modify: `gain-brain/src/lib.rs`

The read path (Task 4) handles incoming group changes. This task adds the write path: when this instance's gain changes, write it to the group slot.

- [ ] **Step 1: Detect local gain changes and write to group**

At the end of the group sync section in process() (after reading from the group, before applying gain), add:

```rust
// Write local gain changes to group
if group >= 1 && group <= 16 && link_mode != LinkMode::Off {
    let current_db = util::gain_to_db(self.params.gain.value());
    let current_mb = db_to_millibels(current_db);
    if current_mb != self.last_sent_gain_millibels {
        gf.write_slot(group as u8, current_mb);
        let slot = gf.read_slot(group as u8);
        self.last_sent_gain_millibels = current_mb;
        self.last_seen_generation = slot.generation;
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --package gain-brain`

- [ ] **Step 3: Commit**

```
feat: write local gain changes to group slot
```

---

### Task 6: Build the editor (GUI)

**Files:**
- Create/modify: `gain-brain/src/editor.rs`
- Modify: `gain-brain/src/lib.rs` (wire up editor)

Follow the exact same pattern as `gs-meter/src/editor.rs`. The editor.rs file is large (~800-1000 lines) so the implementer should use gs-meter's editor.rs as a direct template, adapting it for gain-brain's simpler layout.

- [ ] **Step 1: Implement the editor module**

The editor should follow the gs-meter pattern exactly:
- `GainBrainEditorState` with `AtomicCell<(u32, u32)>` for persisted window size
- `GainBrainWindow` struct with softbuffer surface, pixmap, hit regions, text renderer
- `draw()` method rendering:
  - Title "Gain Brain" with scale +/- buttons
  - Group stepped selector: "X", "1"-"16" (17 segments)
  - Link mode stepped selector: "Off", "Abs", "Rel" (3 segments)
  - Gain slider with value readout in dB
  - Reset button (resets gain to 0 dB)
- `on_event()` with hit testing, slider dragging, stepped segment clicks, double-click to reset
- `GainBrainEditor` implementing nih-plug's `Editor` trait

Reference: `gs-meter/src/editor.rs` — copy the structure, adapt the draw() layout and parameter mappings.

Window size: 300 x 280 pixels at 1x.

ParamIds needed: Gain, Group, LinkMode.

- [ ] **Step 2: Wire editor into lib.rs**

Add to GainBrain's Plugin impl:

```rust
fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
    editor::create(self.params.clone())
}
```

- [ ] **Step 3: Verify it compiles and builds standalone**

Run: `cargo build --bin gain-brain`
Expected: compiles, standalone binary runs and shows the GUI

- [ ] **Step 4: Commit**

```
feat: implement gain-brain CPU-rendered editor
```

---

### Task 7: Add to CI and final verification

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Add gain-brain bundle step to build.yml**

After the gs-meter bundle step, add:

```yaml
      - name: Bundle gain-brain
        run: cargo nih-plug bundle gain-brain --release
```

- [ ] **Step 2: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Build release bundle**

Run: `cargo nih-plug bundle gain-brain --release`
Expected: creates `target/bundled/gain-brain.clap` and `target/bundled/gain-brain.vst3`

- [ ] **Step 5: Commit**

```
feat: add gain-brain to CI build workflow
```

---

### Task 8: Dispatch review agents

- [ ] **Step 1: Dispatch parallel review agents**

Use `superpowers:code-reviewer` and `feature-dev:code-reviewer` agents to review all gain-brain code. Focus areas:
- No allocations on audio thread (process() must not allocate)
- Mmap safety (only constructor is unsafe, well-justified)
- Group sync correctness (read/write/echo detection)
- Joining behavior matches spec for all mode transitions
- All parameters handled in editor hit regions
- Widget code is a clean copy (no leftover gs-meter references)
