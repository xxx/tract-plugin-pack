# tract-dsp `window` + `boxcar` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the Hann window generator and the O(1) running-sum sliding window into the `tract-dsp` crate, and migrate every consumer — a pure DRY refactor with zero behaviour change.

**Architecture:** Two new dependency-free modules in the existing `tract-dsp` crate. `window` exposes `hann_periodic`/`hann_symmetric` (the two denominator variants the codebase uses — kept separate, not unified, so no plugin's output shifts). `boxcar` exposes `RunningSumWindow<T>`, generic over the stored element type (`f32`/`f64`) with an `f64` accumulator — generic is mandatory so `gs-meter`'s `meter.rs` keeps its `f32` ring within its 100-instance memory budget.

**Tech Stack:** Rust (nightly, workspace-pinned), `cargo nextest`, `cargo clippy`. `tract-dsp` stays pure `std`, zero external deps.

**Spec:** `docs/superpowers/specs/2026-05-16-tract-dsp-window-boxcar-design.md`.

**Hard constraint — zero behaviour change.** Every migrated consumer must produce bit-identical output. The two `hann` functions use the *exact* arithmetic form of the call sites they replace (worked out below — `hann_periodic` uses `0.5 * (1.0 - …)`, `hann_symmetric` uses `0.5 - 0.5 * …`; note `std::f32::consts::TAU == 2.0 * std::f32::consts::PI` bit-for-bit, so warp-zone's `TAU`-based site is covered). `RunningSumWindow::push` reproduces the consumers' evict/add/wrap/fill arithmetic exactly. Verification: every plugin's existing test suite stays green, plus workspace `build`/`nextest`/`clippy -D warnings`/`fmt --check`.

---

## File Structure

**New files:**
- `tract-dsp/src/window.rs` — `hann_periodic`, `hann_symmetric` + tests.
- `tract-dsp/src/boxcar.rs` — `RunningSumWindow<T>` + tests.

**Modified files:**
- `tract-dsp/src/lib.rs` — add `pub mod boxcar; pub mod window;`.
- `satch/Cargo.toml`, `warp-zone/Cargo.toml`, `miff/Cargo.toml`, `wavetable-filter/Cargo.toml`, `six-pack/Cargo.toml` — add `tract-dsp` path dependency. (`imagine` and `gs-meter` already have it from Pass 1.)
- `satch/src/spectral.rs`, `warp-zone/src/spectral.rs`, `miff/src/convolution.rs`, `wavetable-filter/src/lib.rs`, `six-pack/src/spectrum.rs`, `imagine/src/spectrum.rs` — replace the inline Hann loop with a `hann_*` call.
- `gs-meter/src/meter.rs`, `gs-meter/src/lufs.rs` — replace hand-coded running-sum rings with `RunningSumWindow`.
- `Cargo.lock` — updated by the dependency additions; include it in the relevant commits.

---

## Task 1: `window` module

**Files:**
- Create: `tract-dsp/src/window.rs`
- Modify: `tract-dsp/src/lib.rs`

- [ ] **Step 1: Create `tract-dsp/src/window.rs`**

```rust
//! Window functions for spectral analysis.

use std::f32::consts::PI;

/// Periodic (DFT) Hann window of `n` samples: `w[i] = 0.5·(1 − cos(2π·i/n))`.
///
/// The correct variant for STFT analysis windows — it gives clean
/// constant-overlap-add reconstruction. Returns an empty `Vec` for `n == 0`.
pub fn hann_periodic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n as f32).cos()))
        .collect()
}

/// Symmetric Hann window of `n` samples: `w[i] = 0.5 − 0.5·cos(2π·i/(n−1))`.
///
/// For one-shot spectral analysis. For `n < 2` the `n−1` denominator is
/// degenerate, so a flat `vec![1.0; n]` is returned.
pub fn hann_symmetric(n: usize) -> Vec<f32> {
    if n < 2 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (n - 1) as f32).cos())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_periodic_endpoints_and_midpoint() {
        let w = hann_periodic(8);
        assert_eq!(w.len(), 8);
        assert!(w[0].abs() < 1e-6, "periodic Hann starts at 0");
        assert!((w[4] - 1.0).abs() < 1e-6, "periodic Hann peaks at n/2");
    }

    #[test]
    fn hann_symmetric_endpoints_and_symmetry() {
        let w = hann_symmetric(9);
        assert_eq!(w.len(), 9);
        assert!(w[0].abs() < 1e-6, "symmetric Hann starts at 0");
        assert!(w[8].abs() < 1e-6, "symmetric Hann ends at 0");
        for i in 0..9 {
            assert!((w[i] - w[8 - i]).abs() < 1e-6, "not mirror-symmetric at {i}");
        }
    }

    #[test]
    fn periodic_and_symmetric_differ_by_the_denominator() {
        let p = hann_periodic(16);
        let s = hann_symmetric(16);
        assert!(
            p.iter().zip(&s).any(|(a, b)| (a - b).abs() > 1e-4),
            "the two variants must not be identical"
        );
    }

    #[test]
    fn degenerate_sizes() {
        assert!(hann_periodic(0).is_empty());
        assert!(hann_symmetric(0).is_empty());
        assert_eq!(hann_symmetric(1), vec![1.0]);
    }
}
```

- [ ] **Step 2: Declare the module**

In `tract-dsp/src/lib.rs`, the module declarations are currently:

```rust
pub mod db;
pub mod spsc;
pub mod true_peak;
```

Change to (keep alphabetical):

```rust
pub mod boxcar;
pub mod db;
pub mod spsc;
pub mod true_peak;
pub mod window;
```

(`boxcar` is declared now too; its file is created in Task 2. The crate will not compile between Task 1 and Task 2 — that is expected. Do not build until Task 2.)

- [ ] **Step 3: Commit**

```bash
git add tract-dsp/src/window.rs tract-dsp/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tract-dsp): add window module (Hann generators)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `boxcar` module

**Files:**
- Create: `tract-dsp/src/boxcar.rs`

- [ ] **Step 1: Create `tract-dsp/src/boxcar.rs`**

```rust
//! Fixed-window running-sum accumulator (boxcar).

/// A sliding window over the last `window` pushed values, maintaining an
/// `f64` running sum so the windowed mean is O(1) per sample and free of the
/// drift an `f32` re-summation would accumulate.
///
/// Generic over the stored element type `T` (`f32` or `f64`); the running sum
/// is always `f64`. The backing ring is pre-allocated to a fixed maximum
/// capacity at construction — `push`, `set_window`, and `reset` never allocate.
pub struct RunningSumWindow<T> {
    ring: Vec<T>,
    /// Index where the next value is written (and the oldest currently sits).
    pos: usize,
    /// Number of values currently in the window (`<= window`).
    filled: usize,
    /// Logical window length (`<= ring.len()`).
    window: usize,
    /// Running sum of the values currently in the window.
    sum: f64,
}

impl<T: Copy + Default + Into<f64>> RunningSumWindow<T> {
    /// Create a window backed by a ring of `max_capacity` elements (at least
    /// 1), with the logical window set to `window` (clamped to
    /// `[1, max_capacity]`).
    pub fn new(max_capacity: usize, window: usize) -> Self {
        let max_capacity = max_capacity.max(1);
        Self {
            ring: vec![T::default(); max_capacity],
            pos: 0,
            filled: 0,
            window: window.clamp(1, max_capacity),
            sum: 0.0,
        }
    }

    /// Push one value: evict the oldest if the window is full, then add the new.
    #[inline]
    pub fn push(&mut self, x: T) {
        if self.filled == self.window {
            self.sum -= self.ring[self.pos].into();
        }
        self.ring[self.pos] = x;
        self.sum += x.into();
        self.pos += 1;
        if self.pos >= self.window {
            self.pos = 0;
        }
        if self.filled < self.window {
            self.filled += 1;
        }
    }

    /// Running sum of the values currently in the window.
    pub fn sum(&self) -> f64 {
        self.sum
    }

    /// Number of values currently in the window (`<= window()`).
    pub fn filled(&self) -> usize {
        self.filled
    }

    /// Current logical window length.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Mean of the values currently in the window; `0.0` when empty. The sum
    /// is clamped at `0.0` first to absorb any f64 drift slightly below zero.
    pub fn mean(&self) -> f64 {
        if self.filled == 0 {
            0.0
        } else {
            self.sum.max(0.0) / self.filled as f64
        }
    }

    /// Change the logical window length without reallocating (clamped to
    /// `[1, max_capacity]`). A no-op if unchanged; otherwise the window is
    /// cleared.
    pub fn set_window(&mut self, window: usize) {
        let window = window.clamp(1, self.ring.len());
        if self.window != window {
            self.window = window;
            self.reset();
        }
    }

    /// Clear the window: zero the ring, the sum, and the counters.
    pub fn reset(&mut self) {
        self.ring.fill(T::default());
        self.pos = 0;
        self.filled = 0;
        self.sum = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_then_evicts() {
        let mut w = RunningSumWindow::<f32>::new(10, 4);
        for v in [1.0, 2.0, 3.0, 4.0] {
            w.push(v);
        }
        assert_eq!(w.filled(), 4);
        assert_eq!(w.sum(), 10.0);
        w.push(5.0); // evicts 1.0
        assert_eq!(w.filled(), 4);
        assert_eq!(w.sum(), 14.0); // 2 + 3 + 4 + 5
    }

    #[test]
    fn mean_of_dc() {
        let mut w = RunningSumWindow::<f32>::new(100, 50);
        for _ in 0..50 {
            w.push(0.5);
        }
        assert!((w.mean() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn running_sum_stable_over_many_cycles() {
        let mut w = RunningSumWindow::<f32>::new(100, 100);
        for _ in 0..100_100 {
            w.push(0.5);
        }
        assert!((w.mean() - 0.5).abs() < 1e-6, "running sum drifted: {}", w.mean());
    }

    #[test]
    fn set_window_resets_only_on_change() {
        let mut w = RunningSumWindow::<f64>::new(100, 10);
        for _ in 0..10 {
            w.push(1.0);
        }
        assert_eq!(w.sum(), 10.0);
        w.set_window(10); // unchanged → no reset
        assert_eq!(w.sum(), 10.0);
        w.set_window(20); // changed → reset
        assert_eq!(w.sum(), 0.0);
        assert_eq!(w.filled(), 0);
        assert_eq!(w.window(), 20);
    }

    #[test]
    fn reset_clears() {
        let mut w = RunningSumWindow::<f64>::new(50, 10);
        for _ in 0..10 {
            w.push(2.0);
        }
        w.reset();
        assert_eq!(w.sum(), 0.0);
        assert_eq!(w.filled(), 0);
        assert_eq!(w.mean(), 0.0);
    }

    #[test]
    fn window_of_one() {
        let mut w = RunningSumWindow::<f32>::new(10, 1);
        w.push(0.5);
        assert_eq!(w.mean(), 0.5);
        w.push(0.3);
        assert!((w.mean() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn empty_window_mean_is_zero() {
        let w = RunningSumWindow::<f64>::new(10, 5);
        assert_eq!(w.mean(), 0.0);
        assert_eq!(w.filled(), 0);
    }

    #[test]
    fn window_clamped_to_capacity() {
        let w = RunningSumWindow::<f32>::new(8, 999);
        assert_eq!(w.window(), 8);
    }
}
```

- [ ] **Step 2: Build, test, lint the crate**

Run: `cargo build -p tract-dsp` — expect clean (this is the first compile with both new modules).
Run: `cargo nextest run -p tract-dsp` — expect PASS, all `window` + `boxcar` + existing `db`/`spsc`/`true_peak` tests.
Run: `cargo clippy -p tract-dsp --tests -- -D warnings` — expect no warnings.
Run: `cargo fmt -p tract-dsp` then `git diff --stat` — confirm only `tract-dsp/src/boxcar.rs` shows (if `fmt` reformats it).

- [ ] **Step 3: Commit**

```bash
git add tract-dsp/src/boxcar.rs
git commit -m "$(cat <<'EOF'
feat(tract-dsp): add boxcar module (running-sum sliding window)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Tasks 3–8: `window` consumer migrations

Each task: add the `tract-dsp` dependency if the crate lacks it, replace the inline Hann loop with the matching `hann_*` call, then `cargo build -p <crate>` / `cargo nextest run -p <crate>` / `cargo clippy -p <crate> --all-targets -- -D warnings` (expect all green — the replacement is numerically bit-identical, so the existing STFT / spectrum tests pass unchanged) / `cargo fmt -p <crate>`, then commit `<crate's files> + Cargo.lock`.

The replacement is bit-identical because `hann_periodic` / `hann_symmetric` use the exact arithmetic of the loops they replace.

### Task 3: satch

**Files:** Modify `satch/Cargo.toml`, `satch/src/spectral.rs`.

- [ ] **Step 1:** In `satch/Cargo.toml` under `[dependencies]`, add: `tract-dsp = { path = "../tract-dsp" }`.

- [ ] **Step 2:** In `satch/src/spectral.rs`, replace this block (in `SpectralClipper::new`, preceded by the comment `// Hann window`):

```rust
        let analysis_window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / fft_size as f32).cos()))
            .collect();
```

with:

```rust
        let analysis_window: Vec<f32> = tract_dsp::window::hann_periodic(fft_size);
```

- [ ] **Step 3:** Check the `use std::f32::consts::PI;` import (top of file). If `PI` is now unused elsewhere in the file, remove the import; if still used, leave it. `cargo clippy` in Step 4 will flag an unused import — resolve it accordingly.

- [ ] **Step 4:** `cargo build -p satch` / `cargo nextest run -p satch` / `cargo clippy -p satch --all-targets -- -D warnings` / `cargo fmt -p satch`. All green.

- [ ] **Step 5: Commit**

```bash
git add satch/Cargo.toml satch/src/spectral.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(satch): use tract-dsp::window::hann_periodic

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4: warp-zone

**Files:** Modify `warp-zone/Cargo.toml`, `warp-zone/src/spectral.rs`.

- [ ] **Step 1:** In `warp-zone/Cargo.toml` under `[dependencies]`, add: `tract-dsp = { path = "../tract-dsp" }`.

- [ ] **Step 2:** In `warp-zone/src/spectral.rs`, replace this block:

```rust
        let analysis_window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (TAU * i as f32 / fft_size as f32).cos()))
            .collect();
```

with:

```rust
        let analysis_window: Vec<f32> = tract_dsp::window::hann_periodic(fft_size);
```

(`std::f32::consts::TAU == 2.0 * std::f32::consts::PI` as f32 bit patterns, so `hann_periodic` is bit-identical to this `TAU`-based loop.)

- [ ] **Step 3:** The import line is `use std::f32::consts::{PI, TAU};`. After Step 2, `TAU` is likely unused. Check whether `PI` and `TAU` are each still referenced elsewhere in the file and narrow or remove the import accordingly (e.g. to `use std::f32::consts::PI;`, or delete it entirely). `cargo clippy` in Step 4 will flag any unused import.

- [ ] **Step 4:** `cargo build -p warp-zone` / `cargo nextest run -p warp-zone` / `cargo clippy -p warp-zone --all-targets -- -D warnings` / `cargo fmt -p warp-zone`. All green.

- [ ] **Step 5: Commit**

```bash
git add warp-zone/Cargo.toml warp-zone/src/spectral.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(warp-zone): use tract-dsp::window::hann_periodic

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 5: miff

**Files:** Modify `miff/Cargo.toml`, `miff/src/convolution.rs`.

- [ ] **Step 1:** In `miff/Cargo.toml` under `[dependencies]`, add: `tract-dsp = { path = "../tract-dsp" }`.

- [ ] **Step 2:** In `miff/src/convolution.rs` (in `PhaselessChannel::new`), replace this block (preceded by the two `// Hann window …` comment lines — keep or drop the comment, your choice; it is now redundant):

```rust
        let window: Vec<f32> = (0..STFT_FRAME)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / STFT_FRAME as f32).cos())
            })
            .collect();
```

with:

```rust
        let window: Vec<f32> = tract_dsp::window::hann_periodic(STFT_FRAME);
```

- [ ] **Step 3:** `cargo build -p miff` / `cargo nextest run -p miff` / `cargo clippy -p miff --all-targets -- -D warnings` / `cargo fmt -p miff`. All green.

- [ ] **Step 4: Commit**

```bash
git add miff/Cargo.toml miff/src/convolution.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(miff): use tract-dsp::window::hann_periodic

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 6: wavetable-filter

**Files:** Modify `wavetable-filter/Cargo.toml`, `wavetable-filter/src/lib.rs`.

- [ ] **Step 1:** In `wavetable-filter/Cargo.toml` under `[dependencies]`, add: `tract-dsp = { path = "../tract-dsp" }`.

- [ ] **Step 2:** In `wavetable-filter/src/lib.rs`, in the `Default` impl, the `stft_window` struct-field initializer is currently a block:

```rust
            stft_window: {
                let mut w = vec![0.0f32; KERNEL_LEN];
                for (i, w_i) in w.iter_mut().enumerate() {
                    *w_i = 0.5
                        * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / KERNEL_LEN as f32).cos());
                }
                w
            },
```

Replace the whole initializer (keeping the field name and trailing comma) with:

```rust
            stft_window: tract_dsp::window::hann_periodic(KERNEL_LEN),
```

(The original `for` loop fills element `i` with the identical expression `hann_periodic` computes, so the resulting `Vec` is bit-identical.)

- [ ] **Step 3:** `cargo build -p wavetable-filter` / `cargo nextest run -p wavetable-filter` / `cargo clippy -p wavetable-filter --all-targets -- -D warnings` / `cargo fmt -p wavetable-filter`. All green. (The STFT tests — `test_stft_lowpass_attenuates_highs`, `test_stft_flat_preserves_amplitude`, `test_frame_sweep_regression`, etc. — must pass unchanged.)

- [ ] **Step 4: Commit**

```bash
git add wavetable-filter/Cargo.toml wavetable-filter/src/lib.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(wavetable-filter): use tract-dsp::window::hann_periodic

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 7: six-pack

**Files:** Modify `six-pack/Cargo.toml`, `six-pack/src/spectrum.rs`.

- [ ] **Step 1:** In `six-pack/Cargo.toml` under `[dependencies]`, add: `tract-dsp = { path = "../tract-dsp" }`.

- [ ] **Step 2:** In `six-pack/src/spectrum.rs`, replace this block (preceded by `// Hann window`):

```rust
        let window = (0..FFT_SIZE)
            .map(|n| {
                0.5 - 0.5
                    * (2.0 * std::f32::consts::PI * (n as f32) / ((FFT_SIZE - 1) as f32)).cos()
            })
            .collect();
```

with:

```rust
        let window = tract_dsp::window::hann_symmetric(FFT_SIZE);
```

(`FFT_SIZE` is a large const, so the `hann_symmetric` `n < 2` guard never triggers; the values are bit-identical to the `0.5 - 0.5·cos(2π·n/(N−1))` loop.)

- [ ] **Step 3:** `cargo build -p six-pack` / `cargo nextest run -p six-pack` / `cargo clippy -p six-pack --all-targets -- -D warnings` / `cargo fmt -p six-pack`. All green.

- [ ] **Step 4: Commit**

```bash
git add six-pack/Cargo.toml six-pack/src/spectrum.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(six-pack): use tract-dsp::window::hann_symmetric

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 8: imagine

**Files:** Modify `imagine/src/spectrum.rs`. (`imagine` already depends on `tract-dsp` from Pass 1 — no `Cargo.toml` change.)

- [ ] **Step 1:** In `imagine/src/spectrum.rs` (in `Analyzer`'s constructor), replace this block:

```rust
        let window = (0..FFT_SIZE)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos()
            })
            .collect();
```

with:

```rust
        let window = tract_dsp::window::hann_symmetric(FFT_SIZE);
```

- [ ] **Step 2:** `cargo build -p imagine` / `cargo nextest run -p imagine` / `cargo clippy -p imagine --all-targets -- -D warnings` / `cargo fmt -p imagine`. All green. (`imagine`'s `clippy --all-targets` has pre-existing unrelated test-code lint debt in `bands.rs`/`editor.rs`/`lib.rs` — see the note in Task 11; `spectrum.rs` itself must be clean.)

- [ ] **Step 3: Commit**

```bash
git add imagine/src/spectrum.rs
git commit -m "$(cat <<'EOF'
refactor(imagine): use tract-dsp::window::hann_symmetric

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `gs-meter` `meter.rs` → `boxcar`

`ChannelMeter` keeps a momentary-RMS ring as five fields. Replace them with one `RunningSumWindow<f32>`. `gs-meter` already depends on `tract-dsp` (Pass 1).

**Files:** Modify `gs-meter/src/meter.rs`.

Work by content (line numbers shift). Read `meter.rs` first.

- [ ] **Step 1: Add the import.** Below the existing `use tract_dsp::...` lines at the top of `meter.rs`, add:

```rust
use tract_dsp::boxcar::RunningSumWindow;
```

- [ ] **Step 2: Replace the `ChannelMeter` ring fields.** The struct currently has these five fields (with their doc comments):

```rust
    /// Ring buffer of squared samples, pre-allocated to MAX_WINDOW_SAMPLES.
    rms_ring: Vec<f32>,
    /// Running sum of squared samples in the ring (f64 for precision).
    rms_ring_sum: f64,
    /// Logical window size (may be smaller than rms_ring.len()).
    rms_window_size: usize,
    /// Write position in the ring buffer (0..rms_window_size-1).
    rms_ring_pos: usize,
    /// Number of valid samples in the ring buffer (up to rms_window_size).
    rms_ring_filled: usize,
```

Replace all five with one field:

```rust
    /// Sliding window of squared samples for momentary RMS.
    rms_window: RunningSumWindow<f32>,
```

Leave the other `ChannelMeter` fields (`peak_max`, `true_peak`, `rms_sum`, `rms_count`, `rms_momentary_max`) untouched.

- [ ] **Step 3: `ChannelMeter::new`.** It currently builds the ring fields:

```rust
            rms_ring: vec![0.0; MAX_WINDOW_SAMPLES],
            rms_ring_sum: 0.0,
            rms_window_size: size,
            rms_ring_pos: 0,
            rms_ring_filled: 0,
```

Replace with:

```rust
            rms_window: RunningSumWindow::new(MAX_WINDOW_SAMPLES, size),
```

(`size` is the existing `let size = window_samples.clamp(1, MAX_WINDOW_SAMPLES);` — `RunningSumWindow::new` clamps again, harmlessly.)

- [ ] **Step 4: `ChannelMeter::reset`.** It currently has:

```rust
        self.rms_ring[..self.rms_window_size].fill(0.0);
        self.rms_ring_sum = 0.0;
        self.rms_ring_pos = 0;
        self.rms_ring_filled = 0;
```

Replace those four lines with:

```rust
        self.rms_window.reset();
```

Leave the other `reset` lines (`peak_max`, `true_peak.reset()`, `rms_sum`, `rms_count`, `rms_momentary_max`) untouched.

- [ ] **Step 5: `ChannelMeter::set_window_size`.** It currently is:

```rust
    pub fn set_window_size(&mut self, window_samples: usize) {
        let size = window_samples.clamp(1, MAX_WINDOW_SAMPLES);
        if self.rms_window_size != size {
            self.rms_window_size = size;
            self.rms_ring[..size].fill(0.0);
            self.rms_ring_sum = 0.0;
            self.rms_ring_pos = 0;
            self.rms_ring_filled = 0;
            self.rms_momentary_max = 0.0;
        }
    }
```

Replace the body with:

```rust
    pub fn set_window_size(&mut self, window_samples: usize) {
        let size = window_samples.clamp(1, MAX_WINDOW_SAMPLES);
        if self.rms_window.window() != size {
            self.rms_window.set_window(size);
            self.rms_momentary_max = 0.0;
        }
    }
```

(`set_window` resets the window on change; the `rms_momentary_max = 0.0` reset stays gated on the size actually changing, matching the original.)

- [ ] **Step 6: `ChannelMeter::process_sample`.** The momentary-ring block is currently:

```rust
        // Momentary RMS ring buffer with O(1) running sum
        let sq_f32 = sample * sample;
        // Subtract the sample being evicted (if ring is full)
        if self.rms_ring_filled == self.rms_window_size {
            self.rms_ring_sum -= self.rms_ring[self.rms_ring_pos] as f64;
        }
        self.rms_ring[self.rms_ring_pos] = sq_f32;
        self.rms_ring_sum += sq_f32 as f64;
        self.rms_ring_pos += 1;
        if self.rms_ring_pos >= self.rms_window_size {
            self.rms_ring_pos = 0;
        }
        if self.rms_ring_filled < self.rms_window_size {
            self.rms_ring_filled += 1;
        }
```

Replace the whole block with:

```rust
        // Momentary RMS sliding window (O(1) running sum)
        self.rms_window.push(sample * sample);
```

- [ ] **Step 7: `ChannelMeter::process_buffer_channel`.** Its per-sample loop contains the same ring block:

```rust
            let sq_f32 = sample * sample;
            if self.rms_ring_filled == self.rms_window_size {
                self.rms_ring_sum -= self.rms_ring[self.rms_ring_pos] as f64;
            }
            self.rms_ring[self.rms_ring_pos] = sq_f32;
            self.rms_ring_sum += sq_f32 as f64;
            self.rms_ring_pos += 1;
            if self.rms_ring_pos >= self.rms_window_size {
                self.rms_ring_pos = 0;
            }
            if self.rms_ring_filled < self.rms_window_size {
                self.rms_ring_filled += 1;
            }
```

Replace it with:

```rust
            self.rms_window.push(sample * sample);
```

- [ ] **Step 8: `ChannelMeter::rms_momentary_linear`.** Currently:

```rust
    pub fn rms_momentary_linear(&self) -> f32 {
        if self.rms_ring_filled == 0 {
            return 0.0;
        }
        (self.rms_ring_sum.max(0.0) / self.rms_ring_filled as f64).sqrt() as f32
    }
```

Replace the body with:

```rust
    pub fn rms_momentary_linear(&self) -> f32 {
        (self.rms_window.mean().sqrt()) as f32
    }
```

(`mean()` returns `0.0` when empty and `sum.max(0.0)/filled` otherwise — `sqrt(0.0) == 0.0`, so this matches the original including the empty case.)

- [ ] **Step 9: `ChannelMeter::rms_momentary_raw`.** Currently:

```rust
    pub fn rms_momentary_raw(&self) -> (f64, usize) {
        let ms = if self.rms_ring_filled > 0 {
            self.rms_ring_sum.max(0.0) / self.rms_ring_filled as f64
        } else {
            0.0
        };
        (ms, self.rms_ring_filled)
    }
```

Replace the body with:

```rust
    pub fn rms_momentary_raw(&self) -> (f64, usize) {
        (self.rms_window.mean(), self.rms_window.filled())
    }
```

- [ ] **Step 10: Build, test, lint.**

Run: `cargo build -p gs-meter` — clean.
Run: `cargo nextest run -p gs-meter` — PASS. The momentary-RMS tests (`test_rms_momentary_window`, `test_rms_momentary_max_tracks`, `test_running_sum_accuracy`, `test_set_window_size`, `test_window_size_one`, `test_buffer_channel_matches_scalar`, `test_stereo_*`) all exercise this path and must stay green.
Run: `cargo clippy -p gs-meter --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt -p gs-meter`.

- [ ] **Step 11: Commit**

```bash
git add gs-meter/src/meter.rs
git commit -m "$(cat <<'EOF'
refactor(gs-meter): use tract-dsp boxcar for momentary RMS window

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `gs-meter` `lufs.rs` → `boxcar`

`LufsMeter` keeps two running-sum rings — momentary (400 ms) and short-term (3 s). Replace each five-field group with a `RunningSumWindow<f64>`.

**Files:** Modify `gs-meter/src/lufs.rs`.

Work by content. Read `lufs.rs` first.

- [ ] **Step 1: Add the import.** At the top of `lufs.rs`, add:

```rust
use tract_dsp::boxcar::RunningSumWindow;
```

- [ ] **Step 2: Replace the momentary + short-term ring fields.** `LufsMeter` currently has these ten fields (momentary group then short-term group):

```rust
    momentary_ring: Vec<f64>,
    momentary_ring_pos: usize,
    momentary_ring_filled: usize,
    momentary_ring_sum: f64,
    momentary_window_size: usize,
    momentary_max: f64,

    // Short-term loudness: 3000ms sliding window.
    short_term_ring: Vec<f64>,
    short_term_ring_pos: usize,
    short_term_ring_filled: usize,
    short_term_ring_sum: f64,
    short_term_window_size: usize,
    short_term_max: f64,
```

Replace the ring fields with one `RunningSumWindow<f64>` each, keeping `momentary_max` and `short_term_max` (they are separate peak trackers, not part of the ring):

```rust
    momentary: RunningSumWindow<f64>,
    momentary_max: f64,

    // Short-term loudness: 3000ms sliding window.
    short_term: RunningSumWindow<f64>,
    short_term_max: f64,
```

- [ ] **Step 3: `LufsMeter::new`.** It currently computes `momentary_max_size`/`short_term_max_size` and `momentary_window_size`/`short_term_window_size`, and initializes the ring fields:

```rust
            momentary_ring: vec![0.0; momentary_max_size],
            momentary_ring_pos: 0,
            momentary_ring_filled: 0,
            momentary_ring_sum: 0.0,
            momentary_window_size,
            momentary_max: 0.0,
            ...
            short_term_ring: vec![0.0; short_term_max_size],
            short_term_ring_pos: 0,
            short_term_ring_filled: 0,
            short_term_ring_sum: 0.0,
            short_term_window_size,
            short_term_max: 0.0,
```

Replace the momentary group with:

```rust
            momentary: RunningSumWindow::new(momentary_max_size, momentary_window_size),
            momentary_max: 0.0,
```

and the short-term group with:

```rust
            short_term: RunningSumWindow::new(short_term_max_size, short_term_window_size),
            short_term_max: 0.0,
```

The local `let momentary_window_size = …;` / `let short_term_window_size = …;` bindings are still used (passed to `new`); keep them. The `momentary_max_size`/`short_term_max_size` locals are still used too.

- [ ] **Step 4: `LufsMeter::reset`.** It currently has the two ring-clearing blocks:

```rust
        // Momentary
        self.momentary_ring[..self.momentary_window_size].fill(0.0);
        self.momentary_ring_pos = 0;
        self.momentary_ring_filled = 0;
        self.momentary_ring_sum = 0.0;
        self.momentary_max = 0.0;

        // Short-term
        self.short_term_ring[..self.short_term_window_size].fill(0.0);
        self.short_term_ring_pos = 0;
        self.short_term_ring_filled = 0;
        self.short_term_ring_sum = 0.0;
        self.short_term_max = 0.0;
```

Replace with:

```rust
        // Momentary
        self.momentary.reset();
        self.momentary_max = 0.0;

        // Short-term
        self.short_term.reset();
        self.short_term_max = 0.0;
```

- [ ] **Step 5: `LufsMeter::set_sample_rate`.** It currently recomputes `self.momentary_window_size` and `self.short_term_window_size` (and the block/hop sizes), then calls `self.reset()`. Replace the two window-size assignments:

```rust
        self.momentary_window_size = (sample_rate * 0.4) as usize;
        self.short_term_window_size = (sample_rate * 3.0) as usize;
```

with:

```rust
        self.momentary.set_window((sample_rate * 0.4) as usize);
        self.short_term.set_window((sample_rate * 3.0) as usize);
```

Leave the `samples_per_block`/`samples_per_hop`/`samples_per_st_block`/`samples_per_st_hop` assignments and the trailing `self.reset();` untouched. (`set_window` clears the window; the following `reset()` clears it again — harmless.)

- [ ] **Step 6: `LufsMeter::process_sample` — momentary ring.** The block is currently:

```rust
        // ── Momentary (400ms) sliding window ──
        if self.momentary_ring_filled == self.momentary_window_size {
            self.momentary_ring_sum -= self.momentary_ring[self.momentary_ring_pos];
        }
        self.momentary_ring[self.momentary_ring_pos] = sq;
        self.momentary_ring_sum += sq;
        self.momentary_ring_pos += 1;
        if self.momentary_ring_pos >= self.momentary_window_size {
            self.momentary_ring_pos = 0;
        }
        if self.momentary_ring_filled < self.momentary_window_size {
            self.momentary_ring_filled += 1;
        }
```

Replace with:

```rust
        // ── Momentary (400ms) sliding window ──
        self.momentary.push(sq);
```

- [ ] **Step 7: `LufsMeter::process_sample` — short-term ring.** The block is currently:

```rust
        // ── Short-term (3000ms) sliding window ──
        if self.short_term_ring_filled == self.short_term_window_size {
            self.short_term_ring_sum -= self.short_term_ring[self.short_term_ring_pos];
        }
        self.short_term_ring[self.short_term_ring_pos] = sq;
        self.short_term_ring_sum += sq;
        self.short_term_ring_pos += 1;
        if self.short_term_ring_pos >= self.short_term_window_size {
            self.short_term_ring_pos = 0;
        }
        if self.short_term_ring_filled < self.short_term_window_size {
            self.short_term_ring_filled += 1;
        }
```

Replace with:

```rust
        // ── Short-term (3000ms) sliding window ──
        self.short_term.push(sq);
```

- [ ] **Step 8: `LufsMeter::process_sample` — integrated block energy.** Further down, the integrated-loudness block reads the momentary ring directly:

```rust
                let block_energy = if self.momentary_ring_filled >= self.momentary_window_size {
                    self.momentary_ring_sum.max(0.0) / self.momentary_window_size as f64
                } else {
                    0.0
                };
```

Replace with:

```rust
                let block_energy = if self.momentary.filled() >= self.momentary.window() {
                    self.momentary.sum().max(0.0) / self.momentary.window() as f64
                } else {
                    0.0
                };
```

- [ ] **Step 9: `LufsMeter::process_sample` — LRA short-term energy.** The LRA block reads the short-term ring:

```rust
            if self.short_term_ring_filled >= self.short_term_window_size {
                let st_energy =
                    self.short_term_ring_sum.max(0.0) / self.short_term_window_size as f64;
```

Replace those two lines with:

```rust
            if self.short_term.filled() >= self.short_term.window() {
                let st_energy = self.short_term.sum().max(0.0) / self.short_term.window() as f64;
```

(Leave the rest of that block — the `st_pos`/`st_block_energies` handling — untouched.)

- [ ] **Step 10: `LufsMeter::momentary_lufs`.** Currently:

```rust
    pub fn momentary_lufs(&self) -> f64 {
        if self.momentary_ring_filled < self.momentary_window_size {
            return f64::NEG_INFINITY;
        }
        let mean_sq = self.momentary_ring_sum.max(0.0) / self.momentary_window_size as f64;
        energy_to_loudness(mean_sq)
    }
```

Replace the body with:

```rust
    pub fn momentary_lufs(&self) -> f64 {
        if self.momentary.filled() < self.momentary.window() {
            return f64::NEG_INFINITY;
        }
        let mean_sq = self.momentary.sum().max(0.0) / self.momentary.window() as f64;
        energy_to_loudness(mean_sq)
    }
```

- [ ] **Step 11: `LufsMeter::short_term_lufs`.** Currently:

```rust
    pub fn short_term_lufs(&self) -> f64 {
        if self.short_term_ring_filled < self.short_term_window_size {
            return f64::NEG_INFINITY;
        }
        let mean_sq = self.short_term_ring_sum.max(0.0) / self.short_term_window_size as f64;
        energy_to_loudness(mean_sq)
    }
```

Replace the body with:

```rust
    pub fn short_term_lufs(&self) -> f64 {
        if self.short_term.filled() < self.short_term.window() {
            return f64::NEG_INFINITY;
        }
        let mean_sq = self.short_term.sum().max(0.0) / self.short_term.window() as f64;
        energy_to_loudness(mean_sq)
    }
```

- [ ] **Step 12: `LufsMeter::update_maxes`.** Currently:

```rust
    pub fn update_maxes(&mut self) {
        // Momentary max: track highest energy, not LUFS, to avoid log in hot path
        if self.momentary_ring_filled > 0 {
            let mean_sq = self.momentary_ring_sum.max(0.0) / self.momentary_ring_filled as f64;
            if mean_sq > self.momentary_max {
                self.momentary_max = mean_sq;
            }
        }

        // Short-term max
        if self.short_term_ring_filled > 0 {
            let mean_sq = self.short_term_ring_sum.max(0.0) / self.short_term_ring_filled as f64;
            if mean_sq > self.short_term_max {
                self.short_term_max = mean_sq;
            }
        }
    }
```

Note these divide by `*_ring_filled` (not the window size). `RunningSumWindow::mean()` is exactly `sum.max(0.0)/filled` (or `0.0` when empty). Replace the body with:

```rust
    pub fn update_maxes(&mut self) {
        // Momentary max: track highest energy, not LUFS, to avoid log in hot path
        if self.momentary.filled() > 0 {
            let mean_sq = self.momentary.mean();
            if mean_sq > self.momentary_max {
                self.momentary_max = mean_sq;
            }
        }

        // Short-term max
        if self.short_term.filled() > 0 {
            let mean_sq = self.short_term.mean();
            if mean_sq > self.short_term_max {
                self.short_term_max = mean_sq;
            }
        }
    }
```

- [ ] **Step 13: Search for stragglers.** Run `rg -n 'momentary_ring|short_term_ring|momentary_window_size|short_term_window_size' gs-meter/src/lufs.rs`. Every hit must be gone except inside `#[cfg(test)]` — and check the test module too: tests reference `meter.momentary_window_size` / `meter.short_term_window_size` (`test_momentary_window_size`, `test_short_term_window_size`, `test_set_sample_rate_updates_windows`). Update those test reads to `meter.momentary.window()` / `meter.short_term.window()`. Do not change the asserted values. If a test references a removed ring field in a way that cannot be expressed via the `RunningSumWindow` API, STOP and report it.

- [ ] **Step 14: Build, test, lint.**

Run: `cargo build -p gs-meter` — clean.
Run: `cargo nextest run -p gs-meter` — PASS. The LUFS suite (`test_momentary_loudness_sine`, `test_integrated_*`, `test_short_term_needs_3_seconds`, `test_momentary_max_tracks_peak`, `test_lra_*`, `test_momentary_window_size`, `test_short_term_window_size`, `test_set_sample_rate_updates_windows`, `test_hop_sizes`, `test_reset_clears_everything`) must all stay green.
Run: `cargo clippy -p gs-meter --all-targets -- -D warnings` — no warnings.
Run: `cargo fmt -p gs-meter`.

- [ ] **Step 15: Commit**

```bash
git add gs-meter/src/lufs.rs
git commit -m "$(cat <<'EOF'
refactor(gs-meter): use tract-dsp boxcar for LUFS momentary/short-term windows

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Workspace verification

**Files:** none (verification only).

- [ ] **Step 1:** `cargo build --workspace` — every crate compiles clean.

- [ ] **Step 2:** `cargo nextest run --workspace` — all tests pass. Baseline before this plan is 757; this plan adds `tract-dsp`'s `window` (4) + `boxcar` (8) tests = **769 expected**, with no tests removed. If `concurrent_writer_reader_no_torn_index` fails, re-run once (known flaky stress test).

- [ ] **Step 3:** `cargo clippy --workspace -- -D warnings` — clean. (This is the CI lint command. Note: `cargo clippy --workspace --all-targets` surfaces *pre-existing* test-code lint debt in `tiny-skia-widgets` and `imagine` that this plan does not touch and is out of scope — the `--all-targets` form is not the CI gate.)

- [ ] **Step 4:** `cargo fmt --check` — clean. If it reports a diff in a file this plan created/modified, run `cargo fmt` and amend the relevant commit.

- [ ] **Step 5: Confirm the duplication is gone.**

Run: `rg -n 'cos\(\)' satch/src/spectral.rs warp-zone/src/spectral.rs miff/src/convolution.rs six-pack/src/spectrum.rs imagine/src/spectrum.rs` — no Hann-window generation loops remain (any `.cos()` hits should be unrelated DSP, not a `0.5·(1−cos)` window build).
Run: `rg -n 'rms_ring|momentary_ring|short_term_ring' gs-meter/src` — no matches (the hand-coded rings are gone).

---

## Self-Review (completed by plan author)

**Spec coverage:** spec's `window` module → Task 1; `boxcar` module → Task 2; 6 window migrations → Tasks 3–8; 3 boxcar rings (`meter.rs` momentary → Task 9; `lufs.rs` momentary + short-term → Task 10); zero-behaviour-change verification → Task 11. `envelope` correctly absent. Covered.

**Placeholder scan:** No `TBD`/`TODO`. Migration tasks show exact old and new code. The import-cleanup steps (satch Step 3, warp-zone Step 3) say to act on what clippy reports — that is a concrete instruction, not a placeholder, because the unused-import outcome depends on the rest of each file.

**Type consistency:** `RunningSumWindow<T>` with `new`/`push`/`sum`/`filled`/`window`/`mean`/`set_window`/`reset` — used consistently in Tasks 2, 9, 10. `hann_periodic`/`hann_symmetric` named identically in Tasks 1, 3–8. `meter.rs` uses `RunningSumWindow<f32>`, `lufs.rs` uses `RunningSumWindow<f64>` — matches the spec's element-type rule.
