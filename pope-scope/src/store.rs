//! Static global store for cross-instance audio data sharing.
//!
//! 16 pre-allocated slots. Ownership via atomic CAS. Ring buffers
//! allocated on demand when an instance joins, deallocated on leave.

use crate::ring_buffer::RingBuffer;
use crate::time_mapping::TimeMapping;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

pub const MAX_SLOTS: usize = 16;
const BUFFER_SECONDS: usize = 32;
const MAX_CHANNELS: usize = 16;

/// Playhead info written by the audio thread.
pub struct PlayheadInfo {
    pub is_playing: AtomicBool,
    pub bpm: AtomicU64, // f64 bit-cast
    pub time_sig_num: AtomicU32,
    pub time_sig_den: AtomicU32,
    pub ppq_position: AtomicU64,  // f64 bit-cast
    pub bar_start_ppq: AtomicU64, // f64 bit-cast
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
    /// RwLock: audio thread takes brief write lock for push(),
    /// GUI thread takes shared read lock for snapshot reads.
    pub buffers: RwLock<Option<Vec<RingBuffer>>>,
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
            buffers: RwLock::new(None),
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
#[allow(clippy::needless_range_loop)]
pub fn acquire_slot(instance_hash: u64) -> Option<usize> {
    debug_assert!(instance_hash != 0, "instance hash must be non-zero");
    if instance_hash == 0 {
        return None;
    }
    for i in 0..MAX_SLOTS {
        let result =
            STORE[i]
                .owner
                .compare_exchange(0, instance_hash, Ordering::AcqRel, Ordering::Relaxed);
        if result.is_ok() {
            return Some(i);
        }
    }
    None
}

/// Release a slot. Verifies ownership before releasing.
pub fn release_slot(index: usize, instance_hash: u64) {
    assert!(index < MAX_SLOTS);
    let result =
        STORE[index]
            .owner
            .compare_exchange(instance_hash, 0, Ordering::AcqRel, Ordering::Relaxed);
    if result.is_ok() {
        // Deallocate buffers — recover from poisoned lock to ensure cleanup.
        let mut guard = STORE[index]
            .buffers
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = None;
        // Reset metadata
        STORE[index].metadata.solo.store(false, Ordering::Relaxed);
        STORE[index].metadata.mute.store(false, Ordering::Relaxed);
        STORE[index]
            .metadata
            .num_channels
            .store(0, Ordering::Relaxed);
        if let Ok(mut name) = STORE[index].metadata.track_name.lock() {
            name.clear();
        }
    }
}

/// Initialize buffers for a slot. Called from `initialize()`.
pub fn init_buffers(index: usize, num_channels: usize, sample_rate: f32) {
    debug_assert!(index < MAX_SLOTS);
    debug_assert!(num_channels <= MAX_CHANNELS);
    if index >= MAX_SLOTS || num_channels > MAX_CHANNELS {
        return;
    }
    let capacity = (sample_rate as usize) * BUFFER_SECONDS;
    let mut bufs = Vec::with_capacity(num_channels);
    for _ in 0..num_channels {
        bufs.push(RingBuffer::new(capacity));
    }
    if let Ok(mut guard) = STORE[index].buffers.write() {
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

/// Get active slot indices filtered by group.
/// Returns a fixed-size array and the count of valid entries (no heap allocation).
#[allow(clippy::needless_range_loop)]
pub fn active_slots_in_group(group: u32) -> ([usize; MAX_SLOTS], usize) {
    let mut result = [0usize; MAX_SLOTS];
    let mut count = 0;
    for i in 0..MAX_SLOTS {
        if is_active(i) && STORE[i].metadata.group.load(Ordering::Relaxed) == group {
            result[count] = i;
            count += 1;
        }
    }
    (result, count)
}

#[cfg(test)]
pub(crate) fn reset_slot(index: usize) {
    STORE[index].owner.store(0, Ordering::Relaxed);
    STORE[index].heartbeat.store(0, Ordering::Relaxed);
    if let Ok(mut guard) = STORE[index].buffers.write() {
        *guard = None;
    }
    STORE[index].metadata.solo.store(false, Ordering::Relaxed);
    STORE[index].metadata.mute.store(false, Ordering::Relaxed);
    STORE[index]
        .metadata
        .num_channels
        .store(0, Ordering::Relaxed);
    STORE[index].metadata.group.store(0, Ordering::Relaxed);
    STORE[index]
        .metadata
        .display_color
        .store(0, Ordering::Relaxed);
    if let Ok(mut name) = STORE[index].metadata.track_name.lock() {
        name.clear();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Serialize store tests since they share global static state.
    pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_all() {
        for i in 0..MAX_SLOTS {
            reset_slot(i);
        }
    }

    #[test]
    fn test_acquire_slot() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let idx = acquire_slot(42).unwrap();
        assert!(idx < MAX_SLOTS);
        assert!(is_active(idx));
    }

    #[test]
    fn test_acquire_returns_different_slots() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let a = acquire_slot(1).unwrap();
        let b = acquire_slot(2).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_release_slot() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let idx = acquire_slot(42).unwrap();
        assert!(is_active(idx));
        release_slot(idx, 42);
        assert!(!is_active(idx));
    }

    #[test]
    fn test_release_wrong_owner_does_nothing() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let idx = acquire_slot(42).unwrap();
        release_slot(idx, 99); // wrong owner
        assert!(is_active(idx)); // still owned
    }

    #[test]
    fn test_acquire_all_16() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        for i in 1..=16u64 {
            assert!(acquire_slot(i).is_some());
        }
        // 17th should fail
        assert!(acquire_slot(17).is_none());
    }

    #[test]
    fn test_init_buffers() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let idx = acquire_slot(1).unwrap();
        init_buffers(idx, 2, 48000.0);
        let s = slot(idx);
        assert_eq!(s.metadata.num_channels.load(Ordering::Relaxed), 2);
        let guard = s.buffers.read().unwrap();
        assert!(guard.is_some());
        let bufs = guard.as_ref().unwrap();
        assert_eq!(bufs.len(), 2);
        assert_eq!(bufs[0].capacity(), 48000 * 32);
    }

    #[test]
    fn test_release_deallocates_buffers() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let idx = acquire_slot(1).unwrap();
        init_buffers(idx, 2, 48000.0);
        release_slot(idx, 1);
        let guard = slot(idx).buffers.read().unwrap();
        assert!(guard.is_none());
    }

    #[test]
    fn test_active_slots_in_group() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let a = acquire_slot(1).unwrap();
        let b = acquire_slot(2).unwrap();
        let c = acquire_slot(3).unwrap();
        slot(a).metadata.group.store(0, Ordering::Relaxed);
        slot(b).metadata.group.store(1, Ordering::Relaxed);
        slot(c).metadata.group.store(0, Ordering::Relaxed);

        let (group0, group0_count) = active_slots_in_group(0);
        assert_eq!(group0_count, 2);
        assert!(group0[..group0_count].contains(&a));
        assert!(group0[..group0_count].contains(&c));

        let (group1, group1_count) = active_slots_in_group(1);
        assert_eq!(group1_count, 1);
        assert!(group1[..group1_count].contains(&b));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "instance hash must be non-zero")]
    fn test_acquire_zero_hash_panics() {
        // Note: no TEST_LOCK here — should_panic tests poison the mutex
        acquire_slot(0);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn test_acquire_zero_hash_returns_none() {
        assert!(acquire_slot(0).is_none());
    }
}
