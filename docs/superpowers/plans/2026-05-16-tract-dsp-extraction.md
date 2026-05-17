# tract-dsp Shared DSP Crate — Extraction Plan (Pass 1: Safe Wins)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a shared, GUI-free `tract-dsp` crate and move the highest-confidence duplicated DSP into it — the verbatim-copied ITU true-peak detector and the near-duplicated lock-free SPSC ring — with zero behaviour change.

**Architecture:** A new pure-DSP library crate (`tract-dsp/`) with no `nih-plug`, `tiny-skia`, `softbuffer`, or editor dependencies — only `std` and `std::simd`. Three modules this pass: `true_peak` (ITU-R BS.1770-4 polyphase detector), `spsc` (generic 2-channel lock-free ring), `db` (decibel conversions). Consumer crates (`gs-meter`, `tinylimit`, `imagine`) delete their local copies and import from `tract-dsp`. Each module ships with the moved test suite; each migration leaves the workspace green.

**Tech Stack:** Rust (nightly, workspace-pinned via `rust-toolchain.toml`), `std::simd` portable SIMD, `cargo nextest`, `cargo clippy`.

**Scope boundary:** This is Pass 1 of a planned multi-pass extraction. Pass 1 covers *only* the dependency-free, zero-behaviour-change extractions. Explicitly **deferred** to later passes (see "Deferred Work" at the end): the one-pole envelope follower, the running-sum boxcar window, the Hann-window helper, the FIR/STFT convolution engines, and the wavetable-filter `lib.rs` carve-out.

---

## Background

A four-agent audit (reports in `/tmp/dsp-audit/`) found the true-peak detector is **byte-identical** between `gs-meter/src/meter.rs:9-206` and `tinylimit/src/true_peak.rs:1-218` (confirmed by the CLAUDE.md note "`tinylimit/src/true_peak.rs` was copied from `gs-meter/src/meter.rs`"), and the lock-free SPSC ring is near-duplicated between `imagine/src/vectorscope.rs` and `imagine/src/polar_rays.rs`. The memory entry `project_dsp_duplication_refactor.md` records that per-crate DSP duplication is unrefactored debt, not a convention.

Key facts the worker must respect:

- The workspace is nightly-pinned. `std::simd` (`f32x16`) is available; consumer crates already enable `#![feature(portable_simd)]`.
- `tract-dsp` must stay GUI-free and `nih-plug`-free. Pure `std` only.
- `gs-meter` is "designed for 100+ instances" (CLAUDE.md) — do not increase its per-instance memory footprint. Pass 1 does not touch `gs-meter`'s ring buffers (deferred).
- Never commit unless the executing workflow's review checkpoint says to. This plan's "Commit" steps are part of the subagent-driven / executing-plans workflow and are expected; they are not ad-hoc commits.
- `tinylimit/src/true_peak.rs` is already the clean, self-contained form of the detector (it is what `gs-meter/src/meter.rs` was copied *into*, plus one extra method `process_sample_peak`). It is the canonical source for the extracted module.
- `gs-meter`'s `linear_to_db` returns `f32::NEG_INFINITY` for non-positive input; `tinylimit`'s private `linear_to_db` (in `lib.rs:478`) floors at `-100.0`. These are **semantically different**. Pass 1 migrates only `gs-meter`'s variant into `tract-dsp::db`; `tinylimit`'s local `linear_to_db` is **left untouched**.

---

## File Structure

**New files:**

- `tract-dsp/Cargo.toml` — package manifest. No dependencies. No `[lib]` section (default rlib).
- `tract-dsp/src/lib.rs` — crate root: `#![feature(portable_simd)]` + module declarations + crate doc.
- `tract-dsp/src/true_peak.rs` — ITU-R BS.1770-4 polyphase true-peak detector. Verbatim from `tinylimit/src/true_peak.rs`.
- `tract-dsp/src/spsc.rs` — generic lock-free single-producer/single-consumer ring of paired `f32` samples.
- `tract-dsp/src/db.rs` — decibel ↔ linear-amplitude conversions.

**Modified files:**

- `Cargo.toml` (workspace) — add `tract-dsp` to `members`.
- `gs-meter/Cargo.toml` — add `tract-dsp` path dependency.
- `gs-meter/src/meter.rs` — delete the true-peak block (`:9-206`) and the `linear_to_db`/`db_to_linear` fns (`:530-544`); import from `tract-dsp`.
- `gs-meter/src/lib.rs` — update the `linear_to_db` import.
- `tinylimit/Cargo.toml` — add `tract-dsp` path dependency.
- `tinylimit/src/true_peak.rs` — **deleted** (replaced by the `tract-dsp` module).
- `tinylimit/src/lib.rs` — drop `pub mod true_peak;`, import `TruePeakDetector` from `tract-dsp`.
- `tinylimit/src/limiter.rs` — update the `TruePeakDetector` path in `process_block`'s signature.
- `imagine/Cargo.toml` — add `tract-dsp` path dependency.
- `imagine/src/vectorscope.rs` — reduce to a thin typed wrapper over `tract_dsp::spsc`.
- `imagine/src/polar_rays.rs` — reduce to a thin typed wrapper over `tract_dsp::spsc`.

---

## Task 1: Scaffold the `tract-dsp` crate

**Files:**
- Create: `tract-dsp/Cargo.toml`
- Create: `tract-dsp/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Create the package manifest**

Create `tract-dsp/Cargo.toml`:

```toml
[package]
name = "tract-dsp"
version = "0.1.0"
edition = "2021"
description = "Shared GUI-free DSP primitives for the tract-plugin-pack workspace"
license = "GPL-3.0-or-later"

# No [lib] section: default rlib. No cdylib, no bin — this is a library only.
# No [dependencies]: pure std + std::simd (nightly, pinned by rust-toolchain.toml).
```

- [ ] **Step 2: Create the crate root**

Create `tract-dsp/src/lib.rs`:

```rust
//! `tract-dsp` — shared, GUI-free DSP primitives for the tract-plugin-pack
//! workspace.
//!
//! This crate contains pure signal-processing code only. It has no
//! `nih-plug`, `tiny-skia`, `softbuffer`, or editor dependency, and no
//! external crates at all — just `std` and `std::simd`.
//!
//! Requires nightly Rust for `std::simd`; the workspace already pins nightly
//! via `rust-toolchain.toml`.
#![feature(portable_simd)]

pub mod db;
pub mod spsc;
pub mod true_peak;
```

Note: this references `db`, `spsc`, and `true_peak` modules created in Tasks 2–4. The crate will not compile until those files exist — that is expected; do not build until Task 4.

- [ ] **Step 3: Register the crate in the workspace**

In `Cargo.toml` (workspace root), add `"tract-dsp"` to the `members` array. The current line is:

```toml
members = ["wavetable-filter", "gs-meter", "gain-brain", "tinylimit", "satch", "pope-scope", "warp-zone", "six-pack", "imagine", "miff", "tiny-skia-widgets", "xtask", "bench-suite"]
```

Change it to:

```toml
members = ["wavetable-filter", "gs-meter", "gain-brain", "tinylimit", "satch", "pope-scope", "warp-zone", "six-pack", "imagine", "miff", "tiny-skia-widgets", "tract-dsp", "xtask", "bench-suite"]
```

Leave the `exclude` array unchanged.

- [ ] **Step 4: Commit**

```bash
git add tract-dsp/Cargo.toml tract-dsp/src/lib.rs Cargo.toml
git commit -m "build(tract-dsp): scaffold shared DSP crate"
```

(Co-author trailer per repo convention; see CLAUDE.md / commit workflow.)

---

## Task 2: `db` module — decibel conversions

**Files:**
- Create: `tract-dsp/src/db.rs`

- [ ] **Step 1: Create the module with implementation and tests**

Create `tract-dsp/src/db.rs`:

```rust
//! Decibel ↔ linear-amplitude conversions for raw DSP / metering code.
//!
//! Parameter-side conversions should keep using `nih_plug::util::db_to_gain`
//! / `gain_to_db` — those are correct and framework-integrated. These helpers
//! are for DSP modules that have no `nih-plug` dependency.

/// Convert linear amplitude to dB. Returns `f32::NEG_INFINITY` for a
/// non-positive input (silence has no finite dB value).
#[inline]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * linear.log10()
    }
}

/// Convert dB to linear amplitude: `10^(dB / 20)`.
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Convert dB to linear amplitude using `exp()` instead of `powf()`.
///
/// `10^(dB/20)` is rewritten as `exp(dB · ln(10) / 20)`. `exp()` is roughly
/// twice as fast as `powf()`; prefer this on hot per-sample paths. The result
/// matches [`db_to_linear`] to within f32 rounding.
#[inline]
pub fn db_to_linear_fast(db: f32) -> f32 {
    (db * (std::f32::consts::LN_10 / 20.0)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn linear_to_db_known_values() {
        assert!(approx_eq(linear_to_db(1.0), 0.0));
        assert!(approx_eq(linear_to_db(0.5), -6.0206));
        assert_eq!(linear_to_db(0.0), f32::NEG_INFINITY);
        assert_eq!(linear_to_db(-1.0), f32::NEG_INFINITY);
    }

    #[test]
    fn db_to_linear_known_values() {
        assert!(approx_eq(db_to_linear(0.0), 1.0));
        assert!(approx_eq(db_to_linear(-6.0206), 0.5));
    }

    #[test]
    fn db_roundtrip() {
        for db in [-40.0, -20.0, -6.0, 0.0, 6.0, 20.0] {
            let rt = linear_to_db(db_to_linear(db));
            assert!((rt - db).abs() < 0.001, "roundtrip failed for {db} dB: got {rt}");
        }
    }

    #[test]
    fn db_to_linear_fast_matches_powf() {
        for db in [-60.0, -24.0, -6.0, -0.1, 0.0, 0.1, 6.0, 24.0] {
            let slow = db_to_linear(db);
            let fast = db_to_linear_fast(db);
            assert!(
                (slow - fast).abs() < 1e-4 * slow.max(1.0),
                "mismatch at {db} dB: powf={slow} exp={fast}"
            );
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add tract-dsp/src/db.rs
git commit -m "feat(tract-dsp): add db conversion module"
```

(Tests run as part of Task 4's first build, once the crate compiles end to end.)

---

## Task 3: `spsc` module — generic lock-free ring

This generalises `imagine/src/vectorscope.rs`'s ring. It stores two `f32` channels per slot, with runtime capacity, and offers both an oldest-first read (for the vectorscope) and a newest-first read (for the polar rays).

**Files:**
- Create: `tract-dsp/src/spsc.rs`

- [ ] **Step 1: Create the module with implementation and tests**

Create `tract-dsp/src/spsc.rs`:

```rust
//! Lock-free single-producer / single-consumer ring of paired `f32` samples.
//!
//! Each slot holds two `f32` values stored as `AtomicU32` bit patterns
//! (`f32::to_bits`). The producer (audio thread) writes the two halves with
//! `Relaxed` ordering, then publishes `write_pos` with `Release`. The consumer
//! (GUI thread) loads `write_pos` with `Acquire`, then reads slots with
//! `Relaxed`. The Acquire/Release pair establishes a happens-before edge so
//! the consumer never reads a slot whose writes have not completed.
//!
//! **Per-slot tear:** the two reads are independent. If the producer writes a
//! new pair between the consumer's two reads, the consumer can observe one
//! half from frame N and the other from N+1. Callers that decimate many
//! samples per GUI frame (vectorscopes) treat one torn pair as sub-pixel.
//!
//! This is the shared engine behind `imagine`'s vectorscope and polar-ray
//! rings; capacity and payload semantics are fixed by the calling wrapper.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner {
    a: Vec<AtomicU32>,
    b: Vec<AtomicU32>,
    capacity: usize,
    write_pos: AtomicUsize,
}

/// Audio-thread producer half. Created by [`channel`].
pub struct Producer {
    inner: Arc<Inner>,
}

/// GUI-thread consumer half. Created by [`channel`].
pub struct Consumer {
    inner: Arc<Inner>,
}

/// Create a producer/consumer pair backed by a ring of `capacity` slots.
///
/// # Panics
/// Panics if `capacity` is zero.
pub fn channel(capacity: usize) -> (Producer, Consumer) {
    assert!(capacity > 0, "SPSC ring capacity must be non-zero");
    let inner = Arc::new(Inner {
        a: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
        b: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
        capacity,
        write_pos: AtomicUsize::new(0),
    });
    (
        Producer {
            inner: inner.clone(),
        },
        Consumer { inner },
    )
}

impl Producer {
    /// Push one `(a, b)` pair. Audio thread; lock-free and allocation-free.
    #[inline]
    pub fn push(&self, a: f32, b: f32) {
        let idx = self.inner.write_pos.load(Ordering::Relaxed);
        let slot = idx % self.inner.capacity;
        self.inner.a[slot].store(a.to_bits(), Ordering::Relaxed);
        self.inner.b[slot].store(b.to_bits(), Ordering::Relaxed);
        self.inner
            .write_pos
            .store(idx.wrapping_add(1), Ordering::Release);
    }
}

impl Consumer {
    /// Ring capacity (slot count).
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Copy up to `count` most-recent pairs into `a_out` / `b_out`, **oldest
    /// first**: the last written entry is the most recent. Returns the number
    /// of pairs written. Never reads more than `capacity`, `a_out.len()`, or
    /// `b_out.len()` entries.
    pub fn snapshot_oldest_first(
        &self,
        count: usize,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) -> usize {
        let count = count
            .min(self.inner.capacity)
            .min(a_out.len())
            .min(b_out.len());
        let write_pos = self.inner.write_pos.load(Ordering::Acquire);
        let available = write_pos.min(self.inner.capacity);
        let n = count.min(available);
        if n == 0 {
            return 0;
        }
        let start = write_pos.wrapping_sub(n);
        for i in 0..n {
            let slot = start.wrapping_add(i) % self.inner.capacity;
            a_out[i] = f32::from_bits(self.inner.a[slot].load(Ordering::Relaxed));
            b_out[i] = f32::from_bits(self.inner.b[slot].load(Ordering::Relaxed));
        }
        n
    }

    /// Copy up to `count` most-recent pairs into `a_out` / `b_out`, **newest
    /// first**: `out[0]` is the most recent pair, `out[n-1]` the oldest still
    /// visible. Returns the number of pairs written.
    pub fn snapshot_newest_first(
        &self,
        count: usize,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) -> usize {
        let count = count
            .min(self.inner.capacity)
            .min(a_out.len())
            .min(b_out.len());
        let write_pos = self.inner.write_pos.load(Ordering::Acquire);
        let available = write_pos.min(self.inner.capacity);
        let n = count.min(available);
        for i in 0..n {
            let logical = write_pos.wrapping_sub(i + 1);
            let slot = logical % self.inner.capacity;
            a_out[i] = f32::from_bits(self.inner.a[slot].load(Ordering::Relaxed));
            b_out[i] = f32::from_bits(self.inner.b[slot].load(Ordering::Relaxed));
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oldest_first_preserves_order() {
        let (prod, cons) = channel(1024);
        for i in 0..1000 {
            prod.push(i as f32, -(i as f32));
        }
        let mut a = vec![0.0; 100];
        let mut b = vec![0.0; 100];
        let n = cons.snapshot_oldest_first(100, &mut a, &mut b);
        assert_eq!(n, 100);
        for i in 0..100 {
            assert_eq!(a[i], (900 + i) as f32);
            assert_eq!(b[i], -((900 + i) as f32));
        }
    }

    #[test]
    fn oldest_first_smaller_than_history_returns_recent() {
        let (prod, cons) = channel(1024);
        for i in 0..50 {
            prod.push(i as f32, 0.0);
        }
        let mut a = vec![0.0; 10];
        let mut b = vec![0.0; 10];
        let n = cons.snapshot_oldest_first(10, &mut a, &mut b);
        assert_eq!(n, 10);
        for i in 0..10 {
            assert_eq!(a[i], (40 + i) as f32);
        }
    }

    #[test]
    fn oldest_first_more_than_history_returns_partial() {
        let (prod, cons) = channel(1024);
        for i in 0..5 {
            prod.push(i as f32, 0.0);
        }
        let mut a = vec![0.0; 100];
        let mut b = vec![0.0; 100];
        let n = cons.snapshot_oldest_first(100, &mut a, &mut b);
        assert_eq!(n, 5);
        for i in 0..5 {
            assert_eq!(a[i], i as f32);
        }
    }

    #[test]
    fn wraparound_continuous() {
        let cap = 4096;
        let (prod, cons) = channel(cap);
        let total = cap + cap / 2;
        for i in 0..total {
            prod.push(i as f32, 0.0);
        }
        let mut a = vec![0.0; 100];
        let mut b = vec![0.0; 100];
        let n = cons.snapshot_oldest_first(100, &mut a, &mut b);
        assert_eq!(n, 100);
        let start = total - 100;
        for i in 0..100 {
            assert_eq!(a[i], (start + i) as f32, "i={i}");
        }
    }

    #[test]
    fn empty_snapshot() {
        let (_, cons) = channel(64);
        let mut a = vec![0.0; 10];
        let mut b = vec![0.0; 10];
        assert_eq!(cons.snapshot_oldest_first(10, &mut a, &mut b), 0);
        assert_eq!(cons.snapshot_newest_first(10, &mut a, &mut b), 0);
    }

    #[test]
    fn newest_first_ordering() {
        let (prod, cons) = channel(32);
        prod.push(0.1, 0.5);
        prod.push(0.2, 0.6);
        prod.push(0.3, 0.7);
        let mut a = vec![0.0; 8];
        let mut b = vec![0.0; 8];
        let n = cons.snapshot_newest_first(8, &mut a, &mut b);
        assert_eq!(n, 3);
        assert_eq!((a[0], b[0]), (0.3, 0.7));
        assert_eq!((a[1], b[1]), (0.2, 0.6));
        assert_eq!((a[2], b[2]), (0.1, 0.5));
    }

    #[test]
    fn newest_first_caps_at_capacity() {
        let cap = 32;
        let (prod, cons) = channel(cap);
        for i in 0..(cap * 2) {
            prod.push(i as f32, i as f32);
        }
        let mut a = vec![0.0; cap * 2];
        let mut b = vec![0.0; cap * 2];
        let n = cons.snapshot_newest_first(cap * 2, &mut a, &mut b);
        assert_eq!(n, cap);
        assert_eq!(a[0], (cap * 2 - 1) as f32);
        assert_eq!(a[cap - 1], cap as f32);
    }

    #[test]
    #[should_panic(expected = "capacity must be non-zero")]
    fn zero_capacity_panics() {
        let _ = channel(0);
    }

    // Concurrency stress test. Consolidated here from imagine's vectorscope.rs
    // (a memory note records it as occasionally flaky under heavy load — a
    // lone failure is not a regression; re-run before treating it as one).
    #[test]
    fn concurrent_writer_reader_no_torn_index() {
        use std::sync::atomic::AtomicBool;
        use std::thread;
        use std::time::Duration;

        let (prod, cons) = channel(65_536);
        let stop = Arc::new(AtomicBool::new(false));

        let stop_w = stop.clone();
        let writer = thread::spawn(move || {
            let mut i: u64 = 0;
            while !stop_w.load(Ordering::Relaxed) {
                prod.push(i as f32, -(i as f32));
                i = i.wrapping_add(1);
                if i & 0xfff == 0 {
                    std::hint::spin_loop();
                }
            }
            i
        });

        let mut a_buf = vec![0.0_f32; 256];
        let mut b_buf = vec![0.0_f32; 256];
        let mut snapshots_taken = 0;
        let mut max_n = 0;
        for _ in 0..1000 {
            let n = cons.snapshot_oldest_first(256, &mut a_buf, &mut b_buf);
            if n > 0 {
                snapshots_taken += 1;
                max_n = max_n.max(n);
                for i in 1..n {
                    let diff = a_buf[i] - a_buf[i - 1];
                    assert!(
                        (diff - 1.0).abs() < 0.5 || a_buf[i] == a_buf[i - 1],
                        "non-monotone window at i={i}: prev={} cur={}",
                        a_buf[i - 1],
                        a_buf[i]
                    );
                }
            }
            thread::sleep(Duration::from_micros(50));
        }
        stop.store(true, Ordering::Relaxed);
        let final_count = writer.join().unwrap();
        assert!(
            snapshots_taken > 0,
            "snapshots never saw data; final writer count: {final_count}"
        );
        assert!(max_n > 0);
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add tract-dsp/src/spsc.rs
git commit -m "feat(tract-dsp): add generic lock-free SPSC ring"
```

---

## Task 4: `true_peak` module — ITU-R BS.1770-4 detector

The canonical source is `tinylimit/src/true_peak.rs`: it is the clean, self-contained form of the detector and already includes the superset method `process_sample_peak`. This task copies it verbatim, drops the now-unneeded `#![allow(dead_code)]`, and adds a crate-module doc line.

**Files:**
- Create: `tract-dsp/src/true_peak.rs`

- [ ] **Step 1: Copy the canonical detector verbatim**

```bash
cp tinylimit/src/true_peak.rs tract-dsp/src/true_peak.rs
```

- [ ] **Step 2: Drop the file-level `dead_code` allow**

In `tract-dsp/src/true_peak.rs`, delete line 3:

```rust
#![allow(dead_code)] // Not all methods may be used yet
```

In the shared crate every method is `pub` API, so there is no dead code; the allow is no longer needed. (If Task 6's clippy step surprises you with a dead-code warning, restore a *targeted* `#[allow]` on the specific item — but it should not happen.)

- [ ] **Step 3: Update the module doc comment**

The file's first line is currently:

```rust
//! ITU-R BS.1770-4 true peak detector.
```

Replace it with:

```rust
//! ITU-R BS.1770-4 true-peak detector (polyphase oversampling).
//!
//! Exact 48-tap, 4-phase reference coefficients from ITU-R BS.1770-4 Annex 2.
//! Sample-rate-aware: 4× oversampling below 96 kHz, 2× from 96–192 kHz,
//! bypass at/above 192 kHz. Uses a double-buffered history so the SIMD dot
//! product always reads a contiguous 12-element slice.
//!
//! Extracted verbatim from the copy that previously lived in both
//! `gs-meter/src/meter.rs` and `tinylimit/src/true_peak.rs`.
```

- [ ] **Step 4: Build the crate and run its tests**

Run: `cargo build -p tract-dsp`
Expected: compiles clean (this is the first compile of the whole crate — `db`, `spsc`, `true_peak` all present now).

Run: `cargo nextest run -p tract-dsp`
Expected: PASS — all tests from `db`, `spsc`, and `true_peak` (the `true_peak` suite came with the copied file: `test_true_peak_detects_intersample`, `test_true_peak_reset`, `test_true_peak_quiet_signal`).

Run: `cargo clippy -p tract-dsp -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add tract-dsp/src/true_peak.rs
git commit -m "feat(tract-dsp): add ITU-R BS.1770-4 true-peak detector"
```

---

## Task 5: Migrate `gs-meter` onto `tract-dsp`

`gs-meter/src/meter.rs` currently *defines* `TruePeakDetector` (and its consts, `ITU_COEFFS`, `ITU_COEFFS_PADDED`, `dot12_simd`, `TruePeakMode`) at lines 9–206, and `linear_to_db` / `db_to_linear` at lines 530–544. It keeps `simd_peak_sumsq` (lines 67–93) — that is RMS code and stays. This task deletes the duplicated definitions and imports them from `tract-dsp`.

**Files:**
- Modify: `gs-meter/Cargo.toml`
- Modify: `gs-meter/src/meter.rs`
- Modify: `gs-meter/src/lib.rs`

- [ ] **Step 1: Add the dependency**

In `gs-meter/Cargo.toml`, under `[dependencies]`, add (after the `serde` line):

```toml
tract-dsp = { path = "../tract-dsp" }
```

- [ ] **Step 2: Delete the duplicated true-peak block from `meter.rs`**

In `gs-meter/src/meter.rs`, delete lines 9–63 — the entire block from the comment `// ── True Peak: 4x oversampling per ITU-R BS.1770-4, Annex 2 ─────────────` down to and including the closing `}` of `fn dot12_simd`. This removes: the `// ── True Peak` comment, `TRUE_PEAK_TAPS`, `TRUE_PEAK_PHASES`, `ITU_COEFFS`, `ITU_COEFFS_PADDED`, and `dot12_simd`.

**Keep** `simd_peak_sumsq` (currently lines 67–93) — it is RMS code, not true-peak.

Then delete lines 95–206 — the block from the comment `/// Oversampling mode based on input sample rate...` down to and including the closing `}` of `impl TruePeakDetector` (the `pub fn true_peak_max` accessor and its enclosing brace). This removes `TruePeakMode`, the `TruePeakDetector` struct, its `impl Default`, and its `impl`.

After these two deletions, `meter.rs` no longer defines anything true-peak-related; `ChannelMeter` still has a `true_peak: TruePeakDetector` field that now needs the import.

- [ ] **Step 3: Delete `linear_to_db` / `db_to_linear` from `meter.rs`**

In `gs-meter/src/meter.rs`, delete the two functions defined at (original) lines 530–544 — from the comment `/// Convert linear amplitude to dB. Returns -f32::INFINITY for zero.` through the closing `}` of `pub fn db_to_linear`.

- [ ] **Step 4: Add the imports to `meter.rs`**

At the top of `gs-meter/src/meter.rs`, just below the existing `use std::simd::{f32x16, num::SimdFloat};` line, add:

```rust
use tract_dsp::db::linear_to_db;
use tract_dsp::true_peak::TruePeakDetector;
```

`meter.rs` uses `linear_to_db` in `ChannelMeter::crest_factor_db` and `StereoMeter::crest_factor_db_stereo`, and `TruePeakDetector` in the `ChannelMeter` struct — both now resolve via the imports. `db_to_linear` was only used by the deleted tests (see Step 6).

- [ ] **Step 5: Re-export `linear_to_db` for `lib.rs`**

`gs-meter/src/lib.rs` imports `linear_to_db` via `use meter::{linear_to_db, StereoMeter};`. Keep that working by re-exporting from `meter.rs`. Change the import line added in Step 4 from:

```rust
use tract_dsp::db::linear_to_db;
```

to:

```rust
pub use tract_dsp::db::linear_to_db;
```

Now `meter::linear_to_db` still resolves and `lib.rs` needs no change. (Leave `gs-meter/src/lib.rs` unmodified — verify in Step 7 that it still compiles.)

- [ ] **Step 6: Remove the migrated tests from `meter.rs`**

In `gs-meter/src/meter.rs`'s `#[cfg(test)] mod tests`, delete these three test functions (their behaviour is now covered by `tract-dsp/src/db.rs`'s tests): `test_linear_to_db`, `test_db_to_linear`, `test_db_roundtrip`.

Do **not** delete any other test. In particular keep `test_true_peak_ge_sample_peak`, `test_true_peak_detects_intersample`, `test_true_peak_2x_mode`, `test_true_peak_bypass_mode`, `test_dot12_simd_matches_scalar`, `test_simd_peak_sumsq*`, etc. — they exercise `ChannelMeter` / `StereoMeter` / `simd_peak_sumsq` through the public API and remain valid.

Note: `test_true_peak_reset` in `meter.rs` constructs `TruePeakDetector::new()` directly — this still works via the new import. Keep it.

- [ ] **Step 7: Build, test, lint `gs-meter`**

Run: `cargo build -p gs-meter`
Expected: compiles clean.

Run: `cargo nextest run -p gs-meter`
Expected: PASS — all remaining `meter.rs` and `lufs.rs` tests.

Run: `cargo clippy -p gs-meter -- -D warnings`
Expected: no warnings. (If clippy flags `dot12_simd` or similar as unused — it should not, it was deleted — re-check Step 2.)

- [ ] **Step 8: Commit**

```bash
git add gs-meter/Cargo.toml gs-meter/src/meter.rs
git commit -m "refactor(gs-meter): use tract-dsp true-peak and db modules"
```

---

## Task 6: Migrate `tinylimit` onto `tract-dsp`

`tinylimit/src/true_peak.rs` is the canonical source already lifted in Task 4. This task deletes it and repoints the two references (`lib.rs` and `limiter.rs`).

**Files:**
- Modify: `tinylimit/Cargo.toml`
- Delete: `tinylimit/src/true_peak.rs`
- Modify: `tinylimit/src/lib.rs`
- Modify: `tinylimit/src/limiter.rs`

- [ ] **Step 1: Add the dependency**

In `tinylimit/Cargo.toml`, under `[dependencies]`, add (after the `serde` line):

```toml
tract-dsp = { path = "../tract-dsp" }
```

- [ ] **Step 2: Delete the local copy**

```bash
git rm tinylimit/src/true_peak.rs
```

- [ ] **Step 3: Repoint `lib.rs`**

In `tinylimit/src/lib.rs`, delete the module declaration line:

```rust
pub mod true_peak;
```

Change the import line:

```rust
use true_peak::TruePeakDetector;
```

to:

```rust
use tract_dsp::true_peak::TruePeakDetector;
```

No other change to `lib.rs` is needed — `TruePeakDetector` is used in the `Tinylimit` struct field `true_peak_detectors: [TruePeakDetector; 2]` and in `Default`/`initialize`/`reset`, all of which resolve through the updated import.

- [ ] **Step 4: Repoint `limiter.rs`**

`tinylimit/src/limiter.rs`'s `Limiter::process_block` signature names the type by path:

```rust
        true_peak: Option<&mut [crate::true_peak::TruePeakDetector; 2]>,
```

Change `crate::true_peak::TruePeakDetector` to `tract_dsp::true_peak::TruePeakDetector`:

```rust
        true_peak: Option<&mut [tract_dsp::true_peak::TruePeakDetector; 2]>,
```

That is the only occurrence in `limiter.rs`. (The envelope follower `EnvelopeFilter` in this file is **not** touched in Pass 1 — see Deferred Work.)

- [ ] **Step 5: Build, test, lint `tinylimit`**

Run: `cargo build -p tinylimit`
Expected: compiles clean.

Run: `cargo nextest run -p tinylimit`
Expected: PASS — all `limiter.rs` tests. (The three true-peak tests that lived in the deleted `true_peak.rs` now live in `tract-dsp`; that crate's suite covers them.)

Run: `cargo clippy -p tinylimit -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add tinylimit/Cargo.toml tinylimit/src/lib.rs tinylimit/src/limiter.rs
git commit -m "refactor(tinylimit): use tract-dsp true-peak module"
```

---

## Task 7: Migrate `imagine` onto `tract-dsp::spsc`

`imagine/src/vectorscope.rs` and `imagine/src/polar_rays.rs` each contain a full copy of the lock-free SPSC ring. This task reduces both to thin typed wrappers over `tract_dsp::spsc`. The public types (`VectorProducer`, `VectorConsumer`, `ring_pair`, `PolarRayProducer`, `PolarRayConsumer`, `Ray`, `RING_CAPACITY`) keep their exact names and signatures, so `imagine/src/lib.rs`, `imagine/src/editor*`, and `imagine/src/spectrum.rs` need **no** changes.

**Files:**
- Modify: `imagine/Cargo.toml`
- Modify: `imagine/src/vectorscope.rs`
- Modify: `imagine/src/polar_rays.rs`

- [ ] **Step 1: Add the dependency**

In `imagine/Cargo.toml`, under `[dependencies]`, add (after the `rustfft` line):

```toml
tract-dsp = { path = "../tract-dsp" }
```

- [ ] **Step 2: Rewrite `vectorscope.rs` as a wrapper**

Replace the entire contents of `imagine/src/vectorscope.rs` with:

```rust
//! SPSC ring of (L, R) samples for the vectorscope display.
//!
//! Thin typed wrapper over [`tract_dsp::spsc`] — the lock-free ring engine
//! lives there and is shared with `polar_rays`. This module fixes the
//! capacity and the (L, R) payload naming.
//!
//! `RING_CAPACITY` holds ~340 ms at 192 kHz — far longer than any realistic
//! GUI frame interval, so the audio thread cannot lap the GUI in one frame.

use tract_dsp::spsc::{self, Consumer, Producer};

pub const RING_CAPACITY: usize = 65_536;

/// Audio-thread producer half of the vectorscope ring.
pub struct VectorProducer {
    inner: Producer,
}

/// GUI-thread consumer half of the vectorscope ring.
pub struct VectorConsumer {
    inner: Consumer,
}

/// Create a paired vectorscope producer/consumer.
pub fn ring_pair() -> (VectorProducer, VectorConsumer) {
    let (p, c) = spsc::channel(RING_CAPACITY);
    (VectorProducer { inner: p }, VectorConsumer { inner: c })
}

impl VectorProducer {
    /// Audio thread: push one (L, R) sample.
    #[inline]
    pub fn push(&self, l: f32, r: f32) {
        self.inner.push(l, r);
    }
}

impl VectorConsumer {
    /// GUI thread: snapshot up to `count` most-recent samples into the
    /// provided buffers, oldest first. Returns the number of samples copied.
    pub fn snapshot(&self, count: usize, l_out: &mut [f32], r_out: &mut [f32]) -> usize {
        self.inner.snapshot_oldest_first(count, l_out, r_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_snapshot_preserves_recent_order() {
        let (prod, cons) = ring_pair();
        for i in 0..1000 {
            prod.push(i as f32, -(i as f32));
        }
        let mut l = vec![0.0; 100];
        let mut r = vec![0.0; 100];
        let n = cons.snapshot(100, &mut l, &mut r);
        assert_eq!(n, 100);
        for i in 0..100 {
            assert_eq!(l[i], (900 + i) as f32);
            assert_eq!(r[i], -((900 + i) as f32));
        }
    }

    #[test]
    fn empty_snapshot_returns_zero() {
        let (_, cons) = ring_pair();
        let mut l = vec![0.0; 10];
        let mut r = vec![0.0; 10];
        assert_eq!(cons.snapshot(10, &mut l, &mut r), 0);
    }
}
```

(The full behavioural matrix — wraparound, partial fills, the concurrency stress test — now lives in `tract-dsp/src/spsc.rs`. The two smoke tests above confirm the wrapper wiring.)

- [ ] **Step 3: Rewrite `polar_rays.rs` as a wrapper**

Replace the entire contents of `imagine/src/polar_rays.rs` with:

```rust
//! SPSC ring of recent (angle, amplitude) emits for the Polar Level
//! vectorscope mode.
//!
//! Thin typed wrapper over [`tract_dsp::spsc`]. The audio thread emits one
//! ray (the average M/S vector over the most recent emit interval); the GUI
//! reads the ring newest-first and renders each ray with age-scaled opacity.
//!
//! `RING_CAPACITY` is sized for the longest decay window divided by the
//! shortest emit interval, with headroom (~17 needed at 30 ms emit / 500 ms
//! decay; rounded up to 32).

use tract_dsp::spsc::{self, Consumer, Producer};

pub const RING_CAPACITY: usize = 32;

/// Audio-thread producer half of the polar-ray ring.
pub struct PolarRayProducer {
    inner: Producer,
}

/// GUI-thread consumer half of the polar-ray ring.
pub struct PolarRayConsumer {
    inner: Consumer,
}

/// Create a paired polar-ray producer/consumer.
pub fn ring_pair() -> (PolarRayProducer, PolarRayConsumer) {
    let (p, c) = spsc::channel(RING_CAPACITY);
    (PolarRayProducer { inner: p }, PolarRayConsumer { inner: c })
}

impl PolarRayProducer {
    /// Audio thread: emit one ray. `angle` in radians (typically `[0, π]` for
    /// the half-disc), `amp` the magnitude (rendered as a ray-length fraction
    /// of the disc radius after the consumer clamps it).
    #[inline]
    pub fn emit(&self, angle: f32, amp: f32) {
        self.inner.push(angle, amp);
    }
}

/// One ray entry produced by [`PolarRayConsumer::snapshot`].
///
/// `age_normalised` is `0.0` for the most recent emit and approaches `1.0`
/// for the oldest still-visible emit; the renderer turns it into an
/// opacity / colour decay.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ray {
    pub angle: f32,
    pub amp: f32,
    pub age_normalised: f32,
}

impl PolarRayConsumer {
    /// GUI thread: snapshot up to `out.len()` most-recent emits into `out`,
    /// newest first (`out[0]` is the most recent, `age_normalised = 0`).
    /// Returns the number of rays written.
    pub fn snapshot(&self, out: &mut [Ray]) -> usize {
        let cap = out.len().min(RING_CAPACITY);
        if cap == 0 {
            return 0;
        }
        let mut angle = [0.0_f32; RING_CAPACITY];
        let mut amp = [0.0_f32; RING_CAPACITY];
        let n = self
            .inner
            .snapshot_newest_first(cap, &mut angle, &mut amp);
        let denom = (n.saturating_sub(1) as f32).max(1.0);
        for (i, slot_out) in out.iter_mut().enumerate().take(n) {
            *slot_out = Ray {
                angle: angle[i],
                amp: amp[i],
                age_normalised: i as f32 / denom,
            };
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Ray {
        Ray {
            angle: 0.0,
            amp: 0.0,
            age_normalised: 0.0,
        }
    }

    #[test]
    fn empty_snapshot_returns_zero() {
        let (_, cons) = ring_pair();
        let mut out = [blank(); 8];
        assert_eq!(cons.snapshot(&mut out), 0);
    }

    #[test]
    fn newest_first_ordering_and_age() {
        let (prod, cons) = ring_pair();
        prod.emit(0.1, 0.5);
        prod.emit(0.2, 0.6);
        prod.emit(0.3, 0.7);
        let mut out = [blank(); 8];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, 3);
        assert_eq!((out[0].angle, out[0].amp), (0.3, 0.7));
        assert_eq!(out[0].age_normalised, 0.0);
        assert_eq!(out[1].angle, 0.2);
        assert_eq!(out[2].angle, 0.1);
        assert!((out[2].age_normalised - 1.0).abs() < 1e-6);
    }

    #[test]
    fn caps_at_ring_capacity() {
        let (prod, cons) = ring_pair();
        for i in 0..(RING_CAPACITY * 2) {
            prod.emit(i as f32, i as f32);
        }
        let mut out = [blank(); RING_CAPACITY * 2];
        let n = cons.snapshot(&mut out);
        assert_eq!(n, RING_CAPACITY);
        assert_eq!(out[0].angle, (RING_CAPACITY * 2 - 1) as f32);
        assert_eq!(out[RING_CAPACITY - 1].angle, RING_CAPACITY as f32);
    }
}
```

- [ ] **Step 4: Build, test, lint `imagine`**

Run: `cargo build -p imagine`
Expected: compiles clean — `lib.rs`, `editor`, `spectrum.rs` are unchanged because the wrapper types kept their names and signatures.

Run: `cargo nextest run -p imagine`
Expected: PASS — including `vectorscope` and `polar_rays` wrapper tests and all `plugin_tests`.

Run: `cargo clippy -p imagine -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add imagine/Cargo.toml imagine/src/vectorscope.rs imagine/src/polar_rays.rs
git commit -m "refactor(imagine): use tract-dsp SPSC ring for vectorscope and polar rays"
```

---

## Task 8: Workspace-wide verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace`
Expected: every crate compiles clean.

- [ ] **Step 2: Full workspace test run**

Run: `cargo nextest run --workspace`
Expected: PASS, no failures. If `concurrent_writer_reader_no_torn_index` fails, re-run once — it is a known occasionally-flaky concurrency test (see the test's own comment); a single failure under load is not a regression.

- [ ] **Step 3: Full workspace lint**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Format check**

Run: `cargo fmt --check`
Expected: clean. If it reports diffs in the new/modified files, run `cargo fmt` and amend the relevant commit.

- [ ] **Step 5: Confirm the duplication is gone**

Run: `rg -l "ITU_COEFFS" --type rust`
Expected: exactly one file — `tract-dsp/src/true_peak.rs`. (Before this plan it appeared in `gs-meter/src/meter.rs` and `tinylimit/src/true_peak.rs`.)

Run: `rg -l "write_pos" imagine/src`
Expected: no matches in `imagine/src/vectorscope.rs` or `imagine/src/polar_rays.rs` (the ring internals moved to `tract-dsp`).

---

## Self-Review (completed by plan author)

**Spec coverage:** Pass-1 scope = `tract-dsp` crate + `true_peak` + `spsc` + `db` modules + migration of the three consumers. Task 1 = crate; Tasks 2/3/4 = the three modules; Tasks 5/6/7 = gs-meter / tinylimit / imagine migrations; Task 8 = verification. Covered.

**Placeholder scan:** No `TBD`/`TODO`/"similar to". The one verbatim move (Task 4) names its exact source file and the exact two edits — not a placeholder.

**Type consistency:** `tract_dsp::spsc::{channel, Producer, Consumer}` used consistently in Tasks 3/7. `Producer::push`, `Consumer::snapshot_oldest_first`, `Consumer::snapshot_newest_first` named identically across definition and both wrappers. `tract_dsp::true_peak::TruePeakDetector` and `tract_dsp::db::linear_to_db` paths consistent across Tasks 4/5/6. Wrapper public names (`VectorProducer`, `VectorConsumer`, `ring_pair`, `PolarRayProducer`, `PolarRayConsumer`, `Ray`, `RING_CAPACITY`) preserved exactly, so untouched consumers still compile.

---

## Deferred Work (later passes — NOT in this plan)

These were found by the audit but are deliberately out of Pass 1's "safe wins" scope. Each needs its own plan.

- **`envelope` module** — one-pole `EnvelopeFilter` (`tinylimit/src/limiter.rs:37-83`). `imagine/src/spectrum.rs` re-derives the same one-pole. Extracting now would be relocation-only (one consumer) until imagine adopts it; bundle both in a later pass. `DualStageEnvelope` stays in tinylimit (product-specific).
- **`boxcar` module** — O(1) running-sum sliding window, duplicated ~4× in `gs-meter` (`meter.rs`, `lufs.rs`). Deferred because `gs-meter` is built for 100+ instances and the ring element type differs (`meter.rs` stores `f32`, `lufs.rs` stores `f64`); a shared type must stay generic over the element type to avoid doubling `meter.rs`'s per-instance memory. Needs careful design + benchmarking.
- **`window` module** — Hann-window generator, duplicated 6× workspace-wide with an inconsistency (`/N` vs `/(N-1)` denominator). Belongs with the FFT extraction pass.
- **`fir` + `stft` modules** — the FIR ring convolver (`miff` `RawChannel` ↔ `wavetable-filter` `FilterState`) and magnitude-STFT convolver (`miff` `PhaselessChannel` ↔ `wavetable-filter` `process_stft_frame`). These are real near-copies, but extraction is gated on a `realfft`/`rustfft` feature (`stft`) on `tract-dsp`, an API generalisation away from `miff`'s `Kernel` type, and — the hard part — carving `FilterState`/`process_stft_frame` out of `wavetable-filter`'s 2933-line `lib.rs`. The audit also found `wavetable-filter`'s `process_stft_frame` still allocates on the audio thread (`realfft::process()` short form) where `miff`'s copy fixed it — the extraction should adopt `miff`'s scratch-buffer variant. This is the next pass.
- **STFT overlap-add engine** (`satch` ↔ `warp-zone`) and **GUI spectrum analyzer** (`six-pack` ↔ `imagine`) — parallel reimplementations; the user chose "safe wins first" over "all duplication", so these wait.
