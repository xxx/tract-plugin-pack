//! Cross-instance group IPC via memory-mapped file.
//!
//! Layout (272 bytes total):
//!   Header (16 bytes): magic [u8;4] = b"GBRN", version u32 LE = 1, _reserved [u8;8]
//!   16 group slots × 16 bytes each:
//!     gain_millibels i32 LE, generation u32 LE, _reserved [u8;8]

use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

// ── Layout constants ────────────────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"GBRN";
const VERSION: u32 = 1;

const FILE_SIZE: u64 = 272;
const HEADER_SIZE: usize = 16;
const SLOT_SIZE: usize = 16;
const NUM_GROUPS: usize = 16;

// Offsets within a slot
const SLOT_GAIN_OFFSET: usize = 0;
const SLOT_GEN_OFFSET: usize = 4;
const SLOT_BASELINE_GEN_OFFSET: usize = 8;

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct GroupFile {
    mmap: MmapMut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupSlot {
    pub gain_millibels: i32,
    pub generation: u32,
    /// Incremented when an invert toggle or similar event requires readers
    /// to re-baseline without applying a delta.
    pub baseline_generation: u32,
}

// ── Implementation ──────────────────────────────────────────────────────────

impl GroupFile {
    /// Open or create the shared group file.
    ///
    /// Returns `Err` on I/O failure or if the file contains an incompatible version.
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("failed to open group file: {e}"))?;

        let len = file
            .metadata()
            .map_err(|e| format!("failed to stat group file: {e}"))?
            .len();

        if len == 0 {
            // New file — initialise to FILE_SIZE and write header.
            file.set_len(FILE_SIZE)
                .map_err(|e| format!("failed to set group file length: {e}"))?;
        } else if len < FILE_SIZE {
            return Err(format!(
                "group file is too short ({len} bytes, expected {FILE_SIZE})"
            ));
        }

        // SAFETY: mmap of a regular file we own. The file lives for the duration
        // of this struct. Concurrent writes by other instances write the same
        // idempotent header bytes or slot values — no UB arises from aliasing
        // because all accesses go through the byte slice and we do not hold
        // references across writes. This is an approved unsafe exception (CLAUDE.md).
        let mut mmap = unsafe {
            MmapMut::map_mut(&file).map_err(|e| format!("failed to mmap group file: {e}"))?
        };

        if len == 0 {
            // Write header into the freshly zeroed mapping.
            mmap[0..4].copy_from_slice(MAGIC);
            mmap[4..8].copy_from_slice(&VERSION.to_le_bytes());
            // bytes 8-15 remain zero (_reserved)
            mmap.flush()
                .map_err(|e| format!("failed to flush group file header: {e}"))?;
        } else {
            // Validate existing header.
            if &mmap[0..4] != MAGIC {
                return Err(format!(
                    "group file has wrong magic (expected {:?})",
                    MAGIC
                ));
            }
            let file_version = u32::from_le_bytes(mmap[4..8].try_into().unwrap());
            if file_version != VERSION {
                return Err(format!(
                    "group file version mismatch: file={file_version}, expected={VERSION}"
                ));
            }
        }

        Ok(Self { mmap })
    }

    /// Platform-appropriate default path for the shared group file.
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            let base = std::env::var("XDG_RUNTIME_DIR")
                .unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(base).join("gain-brain-groups.bin")
        }
        #[cfg(target_os = "macos")]
        {
            let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(base).join("gain-brain-groups.bin")
        }
        #[cfg(target_os = "windows")]
        {
            let base = std::env::var("LOCALAPPDATA")
                .unwrap_or_else(|_| std::env::var("TEMP").unwrap_or_else(|_| "C:\\Temp".to_string()));
            PathBuf::from(base).join("Temp").join("gain-brain-groups.bin")
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            PathBuf::from("/tmp/gain-brain-groups.bin")
        }
    }

    /// Read a group slot. `group` must be 1–16 (panics otherwise).
    pub fn read_slot(&self, group: u8) -> GroupSlot {
        let offset = Self::slot_offset(group);
        let gain_millibels =
            i32::from_le_bytes(self.mmap[offset + SLOT_GAIN_OFFSET..offset + SLOT_GAIN_OFFSET + 4]
                .try_into()
                .unwrap());
        let generation =
            u32::from_le_bytes(self.mmap[offset + SLOT_GEN_OFFSET..offset + SLOT_GEN_OFFSET + 4]
                .try_into()
                .unwrap());
        let baseline_generation =
            u32::from_le_bytes(self.mmap[offset + SLOT_BASELINE_GEN_OFFSET..offset + SLOT_BASELINE_GEN_OFFSET + 4]
                .try_into()
                .unwrap());
        GroupSlot {
            gain_millibels,
            generation,
            baseline_generation,
        }
    }

    /// Write gain to a group slot and increment generation. `group` must be 1–16.
    pub fn write_slot(&mut self, group: u8, gain_millibels: i32) {
        let offset = Self::slot_offset(group);

        let old_gen =
            u32::from_le_bytes(self.mmap[offset + SLOT_GEN_OFFSET..offset + SLOT_GEN_OFFSET + 4]
                .try_into()
                .unwrap());
        let new_gen = old_gen.wrapping_add(1);

        self.mmap[offset + SLOT_GAIN_OFFSET..offset + SLOT_GAIN_OFFSET + 4]
            .copy_from_slice(&gain_millibels.to_le_bytes());
        self.mmap[offset + SLOT_GEN_OFFSET..offset + SLOT_GEN_OFFSET + 4]
            .copy_from_slice(&new_gen.to_le_bytes());
    }

    /// Write gain and increment BOTH generation and baseline_generation.
    /// Used for invert toggles: readers should re-baseline without applying a delta.
    pub fn write_slot_rebaseline(&mut self, group: u8, gain_millibels: i32) {
        let offset = Self::slot_offset(group);

        let old_gen =
            u32::from_le_bytes(self.mmap[offset + SLOT_GEN_OFFSET..offset + SLOT_GEN_OFFSET + 4]
                .try_into()
                .unwrap());
        let old_baseline =
            u32::from_le_bytes(self.mmap[offset + SLOT_BASELINE_GEN_OFFSET..offset + SLOT_BASELINE_GEN_OFFSET + 4]
                .try_into()
                .unwrap());

        self.mmap[offset + SLOT_GAIN_OFFSET..offset + SLOT_GAIN_OFFSET + 4]
            .copy_from_slice(&gain_millibels.to_le_bytes());
        self.mmap[offset + SLOT_GEN_OFFSET..offset + SLOT_GEN_OFFSET + 4]
            .copy_from_slice(&old_gen.wrapping_add(1).to_le_bytes());
        self.mmap[offset + SLOT_BASELINE_GEN_OFFSET..offset + SLOT_BASELINE_GEN_OFFSET + 4]
            .copy_from_slice(&old_baseline.wrapping_add(1).to_le_bytes());
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    fn slot_offset(group: u8) -> usize {
        assert!(group >= 1 && group <= NUM_GROUPS as u8, "group must be 1-16, got {group}");
        HEADER_SIZE + (group as usize - 1) * SLOT_SIZE
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gain-brain-test-{}-{}",
            name,
            std::process::id()
        ))
    }

    // 1. Create file and verify magic + version bytes in the raw file.
    #[test]
    fn test_create_and_verify_header() {
        let path = test_path("create-header");
        let _ = fs::remove_file(&path);

        GroupFile::open(&path).expect("open failed");

        let bytes = fs::read(&path).expect("read failed");
        assert_eq!(bytes.len(), FILE_SIZE as usize);
        assert_eq!(&bytes[0..4], b"GBRN");
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(version, 1);

        let _ = fs::remove_file(&path);
    }

    // 2. New file → slot 1 has gain=0 and generation=0.
    #[test]
    fn test_read_default_slot_is_zero() {
        let path = test_path("default-slot");
        let _ = fs::remove_file(&path);

        let gf = GroupFile::open(&path).expect("open failed");
        let slot = gf.read_slot(1);
        assert_eq!(slot.gain_millibels, 0);
        assert_eq!(slot.generation, 0);

        let _ = fs::remove_file(&path);
    }

    // 3. Write gain to slot 3, read it back.
    #[test]
    fn test_write_and_read_slot() {
        let path = test_path("write-read");
        let _ = fs::remove_file(&path);

        let mut gf = GroupFile::open(&path).expect("open failed");
        gf.write_slot(3, 350);
        let slot = gf.read_slot(3);
        assert_eq!(slot.gain_millibels, 350);

        let _ = fs::remove_file(&path);
    }

    // 4. Write to same slot 3 times → generation == 3.
    #[test]
    fn test_generation_increments() {
        let path = test_path("generation");
        let _ = fs::remove_file(&path);

        let mut gf = GroupFile::open(&path).expect("open failed");
        gf.write_slot(3, 100);
        gf.write_slot(3, 200);
        gf.write_slot(3, 300);
        let slot = gf.read_slot(3);
        assert_eq!(slot.generation, 3);
        assert_eq!(slot.gain_millibels, 300);

        let _ = fs::remove_file(&path);
    }

    // 5. Write different values to slots 1 and 2; slot 3 remains zero.
    #[test]
    fn test_slots_are_independent() {
        let path = test_path("independent");
        let _ = fs::remove_file(&path);

        let mut gf = GroupFile::open(&path).expect("open failed");
        gf.write_slot(1, 100);
        gf.write_slot(2, 200);

        let s1 = gf.read_slot(1);
        let s2 = gf.read_slot(2);
        let s3 = gf.read_slot(3);

        assert_eq!(s1.gain_millibels, 100);
        assert_eq!(s2.gain_millibels, 200);
        assert_eq!(s3.gain_millibels, 0);
        assert_eq!(s3.generation, 0);

        let _ = fs::remove_file(&path);
    }

    // 6. Two handles to the same file see each other's writes.
    #[test]
    fn test_two_handles_share_state() {
        let path = test_path("two-handles");
        let _ = fs::remove_file(&path);

        let mut gf1 = GroupFile::open(&path).expect("open gf1 failed");
        let gf2 = GroupFile::open(&path).expect("open gf2 failed");

        gf1.write_slot(5, 750);

        // gf2 reads directly from the shared mmap — no flush needed for same process.
        let slot = gf2.read_slot(5);
        assert_eq!(slot.gain_millibels, 750);
        assert_eq!(slot.generation, 1);

        let _ = fs::remove_file(&path);
    }

    // 7. Reading slot 0 panics.
    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_group_0_panics() {
        let path = test_path("group0-panic");
        let _ = fs::remove_file(&path);
        let gf = GroupFile::open(&path).expect("open failed");
        let _ = gf.read_slot(0);
        let _ = fs::remove_file(&path);
    }

    // 8. Reading slot 17 panics.
    #[test]
    #[should_panic(expected = "group must be 1-16")]
    fn test_group_17_panics() {
        let path = test_path("group17-panic");
        let _ = fs::remove_file(&path);
        let gf = GroupFile::open(&path).expect("open failed");
        let _ = gf.read_slot(17);
        let _ = fs::remove_file(&path);
    }

    // 9. File with version=99 → open returns Err.
    #[test]
    fn test_version_mismatch_returns_error() {
        let path = test_path("version-mismatch");
        let _ = fs::remove_file(&path);

        // Build a 272-byte buffer with magic GBRN but version=99.
        let mut buf = vec![0u8; FILE_SIZE as usize];
        buf[0..4].copy_from_slice(b"GBRN");
        buf[4..8].copy_from_slice(&99u32.to_le_bytes());
        fs::write(&path, &buf).expect("write failed");

        let result = GroupFile::open(&path);
        assert!(result.is_err(), "expected Err for version mismatch");
        let msg = result.unwrap_err();
        assert!(msg.contains("version mismatch"), "unexpected error: {msg}");

        let _ = fs::remove_file(&path);
    }

    // 10. Negative gain -1400 (-14.00 dB) round-trips correctly.
    #[test]
    fn test_negative_gain_millibels() {
        let path = test_path("negative-gain");
        let _ = fs::remove_file(&path);

        let mut gf = GroupFile::open(&path).expect("open failed");
        gf.write_slot(8, -1400);
        let slot = gf.read_slot(8);
        assert_eq!(slot.gain_millibels, -1400);

        let _ = fs::remove_file(&path);
    }

    // 11. Extreme values ±6000 (±60.00 dB) round-trip correctly.
    #[test]
    fn test_extreme_values() {
        let path = test_path("extreme-values");
        let _ = fs::remove_file(&path);

        let mut gf = GroupFile::open(&path).expect("open failed");

        gf.write_slot(1, 6000);
        assert_eq!(gf.read_slot(1).gain_millibels, 6000);

        gf.write_slot(2, -6000);
        assert_eq!(gf.read_slot(2).gain_millibels, -6000);

        let _ = fs::remove_file(&path);
    }

    // 12. Simulate project switch: stale data is overwritten on first write.
    //
    // Scenario: "Project A" writes gain=+6dB to group 1. Project A is closed
    // (instances destroyed). "Project B" loads with a fresh instance in group 1
    // at 0dB. On its first write, it overwrites the stale +6dB value.
    #[test]
    fn test_stale_data_overwritten_on_project_switch() {
        let path = test_path("project-switch");
        let _ = fs::remove_file(&path);

        // "Project A" instance writes +6dB (600 millibels) to group 1
        {
            let mut gf = GroupFile::open(&path).expect("open failed");
            gf.write_slot(1, 600);
            let slot = gf.read_slot(1);
            assert_eq!(slot.gain_millibels, 600);
            assert_eq!(slot.generation, 1);
        }
        // Project A instances are destroyed (gf dropped), file persists on disk.

        // "Project B" instance opens the same file — sees stale data.
        let mut gf_b = GroupFile::open(&path).expect("open failed");
        let stale = gf_b.read_slot(1);
        assert_eq!(stale.gain_millibels, 600, "stale data should be visible");

        // Project B's instance has its own param at 0dB. On first sync_group
        // call in process(), it writes its own value, overwriting the stale data.
        // Simulate: instance writes 0 millibels (its own default gain).
        gf_b.write_slot(1, 0);
        let fresh = gf_b.read_slot(1);
        assert_eq!(fresh.gain_millibels, 0, "stale data should be overwritten");
        assert_eq!(fresh.generation, 2, "generation should advance");

        // Other slots should still have stale data = 0 (never written by project A)
        let slot2 = gf_b.read_slot(2);
        assert_eq!(slot2.gain_millibels, 0);
        assert_eq!(slot2.generation, 0);

        let _ = fs::remove_file(&path);
    }

    // 13. The stale read before the first write cannot cause permanent pollution.
    //
    // Even if Project B reads the stale +6dB value from Project A on its first
    // buffer, the instance's internal state (last_seen_generation=0) differs
    // from the file's generation (>0), triggering a read. But the read applies
    // a gain override. The NEXT buffer's write path then writes the instance's
    // own param value, correcting it. After 2 buffers, the slot reflects
    // Project B's state.
    #[test]
    fn test_stale_read_corrected_within_two_writes() {
        let path = test_path("stale-correction");
        let _ = fs::remove_file(&path);

        // Project A writes to groups 1 and 5
        {
            let mut gf = GroupFile::open(&path).expect("open failed");
            gf.write_slot(1, 600);   // +6dB
            gf.write_slot(5, -1200); // -12dB
        }

        // Project B opens — fresh instance state (last_seen_generation=0)
        let mut gf_b = GroupFile::open(&path).expect("open failed");

        // Instance reads stale slot — generation mismatch detected
        let slot1 = gf_b.read_slot(1);
        assert_ne!(slot1.generation, 0, "stale generation should be >0");
        // Instance would apply this stale value as an override...

        // ...but on the SAME or NEXT buffer, the write path fires because
        // the instance's param value (0dB = 0mb) differs from last_sent.
        // This overwrites the stale value:
        gf_b.write_slot(1, 0); // Instance's actual param value
        let corrected = gf_b.read_slot(1);
        assert_eq!(corrected.gain_millibels, 0, "should be corrected to instance's value");

        // Group 5 also gets corrected when its instance writes
        gf_b.write_slot(5, 300); // Project B's group 5 instance has +3dB
        let corrected5 = gf_b.read_slot(5);
        assert_eq!(corrected5.gain_millibels, 300);

        let _ = fs::remove_file(&path);
    }
}
