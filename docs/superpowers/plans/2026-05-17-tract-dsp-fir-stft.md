# tract-dsp `fir` + `stft` Convolvers — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the time-domain SIMD FIR ring and the magnitude-only STFT convolver into the `tract-dsp` crate, migrate `miff`, and carve the equivalent DSP out of `wavetable-filter`'s `lib.rs` — a pure DRY refactor with zero behaviour change.

**Architecture:** Two new `tract-dsp` modules. `fir` (`FirRing` — double-buffered history ring with separate `push` / `mac`, so a caller can MAC one window against two kernels for a crossfade) is unconditional and `std`-only. `stft` (`StftConvolver` — fixed-frame Hann-windowed magnitude-multiply overlap-add) sits behind a `stft` cargo feature so `realfft`/`rustfft` stay opt-in. `miff`'s `RawChannel`/`PhaselessChannel` and `wavetable-filter`'s `FilterState`/`process_stft_frame` become consumers.

**Tech Stack:** Rust (nightly, workspace-pinned), `std::simd`, `realfft`/`rustfft` (feature-gated), `cargo nextest`, `cargo clippy`.

**Spec:** `docs/superpowers/specs/2026-05-16-tract-dsp-fir-stft-design.md`.

**Hard constraint — zero behaviour change.** The shared engines use the bit-identical arithmetic of the code they replace. `FirRing::mac` reproduces the consumers' `f32x16` MAC; `StftConvolver` reproduces the windowed-copy → FFT → magnitude-multiply → IFFT → 1/N overlap-add. For power-of-two frame sizes (all consumers use them) `realfft`'s `process` and `process_with_scratch` are equivalent. Verification: every plugin's existing suite stays green, plus workspace `build`/`nextest`/`clippy -D warnings`/`fmt --check`.

**Clippy note:** use `cargo clippy -p <crate> -- -D warnings` (the CI form) for per-crate checks — NOT `--all-targets`/`--tests`, which surfaces pre-existing test-code lint debt unrelated to this work.

---

## File Structure

**New files:**
- `tract-dsp/src/fir.rs` — `FirRing` + tests.
- `tract-dsp/src/stft.rs` — `StftConvolver` + tests (behind `stft` feature).

**Modified files:**
- `tract-dsp/Cargo.toml` — add `[features] stft` + optional `realfft`/`rustfft`.
- `tract-dsp/src/lib.rs` — `pub mod fir;` and `#[cfg(feature = "stft")] pub mod stft;`.
- `miff/Cargo.toml` — `tract-dsp` dep gains `features = ["stft"]`.
- `miff/src/convolution.rs` — `RawChannel`/`PhaselessChannel` reimplemented as wrappers; their own ring/STFT code deleted.
- `wavetable-filter/Cargo.toml` — `tract-dsp` dep gains `features = ["stft"]`.
- `wavetable-filter/src/lib.rs` — `FilterState` removed (→ `FirRing`); STFT state + `process_stft_frame` removed (→ `StftConvolver`); `process()` call sites updated.
- `Cargo.lock` — updated; include it in the relevant commits.

---

## Task 1: `fir` module

**Files:** Create `tract-dsp/src/fir.rs`; modify `tract-dsp/src/lib.rs`.

- [ ] **Step 1: Create `tract-dsp/src/fir.rs`**

```rust
//! Time-domain FIR convolution: a double-buffered history ring + SIMD MAC.

use std::simd::{num::SimdFloat, f32x16};

/// A per-channel FIR convolution history: a double-buffered ring so the SIMD
/// MAC always reads a contiguous window with no per-chunk wraparound.
///
/// `push` and `mac` are deliberately separate so a caller can MAC one window
/// against multiple kernels (e.g. a crossfade between two kernels) after a
/// single `push`.
///
/// The silence flag is only re-armed by `reset`; a host loop should `reset`
/// after sustained input silence.
pub struct FirRing {
    /// Double-buffered history: `2 * cap` samples. Each sample is written at
    /// both `write_pos` and `write_pos + cap`, so a contiguous window ending
    /// at the newest sample is always a single readable slice.
    history: Vec<f32>,
    write_pos: usize,
    mask: usize,
    is_silent: bool,
}

impl FirRing {
    /// A ring sized for kernels up to `max_len` taps. Capacity is rounded up
    /// to a power of two.
    pub fn new(max_len: usize) -> Self {
        let cap = max_len.next_power_of_two();
        Self {
            history: vec![0.0; cap * 2],
            write_pos: 0,
            mask: cap - 1,
            is_silent: true,
        }
    }

    /// Zero the history and re-arm the silence flag.
    pub fn reset(&mut self) {
        self.history.iter_mut().for_each(|s| *s = 0.0);
        self.write_pos = 0;
        self.is_silent = true;
    }

    /// Push one input sample (double-buffered write). Clears the silence flag
    /// when `sample.abs() > 1e-6`.
    #[inline]
    pub fn push(&mut self, sample: f32) {
        if sample.abs() > 1e-6 {
            self.is_silent = false;
        }
        let cap = self.mask + 1;
        self.history[self.write_pos] = sample;
        self.history[self.write_pos + cap] = sample;
        self.write_pos = (self.write_pos + 1) & self.mask;
    }

    /// `true` iff only (near-)zero samples have been pushed since the last
    /// `reset` — the MAC output is then guaranteed zero and may be skipped.
    #[inline]
    pub fn is_silent(&self) -> bool {
        self.is_silent
    }

    /// `f32x16` multiply-accumulate of the most-recent `rev_taps.len()` samples
    /// against `rev_taps` — the kernel pre-reversed so the MAC reads it
    /// contiguously. `rev_taps.len()` must be a non-zero multiple of 16 and
    /// must not exceed the ring capacity.
    ///
    /// The window is oldest-first: `window[0]` is the oldest of the
    /// `rev_taps.len()` most-recent samples, `window[len-1]` the newest.
    #[inline]
    pub fn mac(&self, rev_taps: &[f32]) -> f32 {
        let len = rev_taps.len();
        let cap = self.mask + 1;
        let start = (self.write_pos + cap - len) & self.mask;
        let window = &self.history[start..start + len];
        let mut acc = f32x16::splat(0.0);
        for c in 0..len / 16 {
            let w = f32x16::from_slice(&window[c * 16..c * 16 + 16]);
            let k = f32x16::from_slice(&rev_taps[c * 16..c * 16 + 16]);
            acc += w * k;
        }
        acc.reduce_sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a length-16 reversed-tap kernel from explicit taps (oldest-first
    /// convolution taps `taps[0..]`; reversed so `mac` reads contiguously).
    fn rev16(taps: [f32; 16]) -> Vec<f32> {
        let mut r = [0.0_f32; 16];
        for j in 0..16 {
            r[j] = taps[16 - 1 - j];
        }
        r.to_vec()
    }

    #[test]
    fn unit_impulse_passes_input_through() {
        // taps[0] = 1.0 → y[n] = x[n].
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        let rev = rev16(taps);
        let mut ring = FirRing::new(16);
        for &s in &[0.5, -0.3, 0.9, 0.1] {
            ring.push(s);
            assert!((ring.mac(&rev) - s).abs() < 1e-6);
        }
    }

    #[test]
    fn asymmetric_two_tap_kernel() {
        // taps = [1.0, 0.5] → y[n] = 1.0*x[n] + 0.5*x[n-1]. Asymmetric on
        // purpose: a window/kernel reversal bug fails this.
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0;
        taps[1] = 0.5;
        let rev = rev16(taps);
        let mut ring = FirRing::new(16);
        ring.push(1.0);
        assert!((ring.mac(&rev) - 1.0).abs() < 1e-6); // 1*1 + 0.5*0
        ring.push(1.0);
        assert!((ring.mac(&rev) - 1.5).abs() < 1e-6); // 1*1 + 0.5*1
        ring.push(0.0);
        assert!((ring.mac(&rev) - 0.5).abs() < 1e-6); // 1*0 + 0.5*1
    }

    #[test]
    fn silence_flag_arms_and_rearms() {
        let mut ring = FirRing::new(64);
        assert!(ring.is_silent());
        ring.push(0.0);
        assert!(ring.is_silent(), "a zero sample keeps silence");
        ring.push(0.5);
        assert!(!ring.is_silent(), "a non-zero sample clears silence");
        ring.reset();
        assert!(ring.is_silent(), "reset re-arms silence");
    }

    #[test]
    fn wraparound_keeps_window_contiguous() {
        // Push far more than capacity; the most-recent-16 MAC must still be
        // correct (double-buffer keeps the window a single slice).
        let mut taps = [0.0_f32; 16];
        taps[0] = 1.0; // newest sample only
        let rev = rev16(taps);
        let mut ring = FirRing::new(16); // cap 16
        for i in 0..1000 {
            ring.push(i as f32);
            assert!((ring.mac(&rev) - i as f32).abs() < 1e-6, "i={i}");
        }
    }
}
```

- [ ] **Step 2: Declare the module** — in `tract-dsp/src/lib.rs`, add `pub mod fir;` to the module declarations (alphabetical: after `db`, before `spsc`). The current block is `pub mod boxcar; pub mod db; pub mod spsc; pub mod true_peak; pub mod window;` → insert `pub mod fir;` after `pub mod db;`.

- [ ] **Step 3: Build, test, lint**

- `cargo build -p tract-dsp` — clean.
- `cargo nextest run -p tract-dsp` — PASS (the 4 new `fir` tests plus all existing).
- `cargo clippy -p tract-dsp -- -D warnings` — clean.
- `cargo fmt -p tract-dsp`, then `git diff --stat` — only `tract-dsp/src/fir.rs` + `tract-dsp/src/lib.rs`.

- [ ] **Step 4: Commit**

```bash
git add tract-dsp/src/fir.rs tract-dsp/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(tract-dsp): add fir module (double-buffered FIR convolution ring)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `stft` module + cargo feature

**Files:** Create `tract-dsp/src/stft.rs`; modify `tract-dsp/Cargo.toml`, `tract-dsp/src/lib.rs`.

- [ ] **Step 1: Add the feature and optional deps** — in `tract-dsp/Cargo.toml`, after the `[package]` section (the file currently has no `[features]` or `[dependencies]`), add:

```toml
[features]
# Gates the FFT-based `stft` module so non-FFT consumers don't pull realfft/rustfft.
stft = ["dep:realfft", "dep:rustfft"]

[dependencies]
realfft = { version = "3.3", optional = true }
rustfft = { version = "6.2", optional = true }
```

- [ ] **Step 2: Create `tract-dsp/src/stft.rs`**

```rust
//! Magnitude-only STFT convolution: fixed-frame Hann-windowed overlap-add.

use crate::window::hann_periodic;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use std::sync::Arc;

/// Per-channel magnitude-only STFT convolution. A fixed `frame`-point real
/// transform; each frame's spectrum has its per-bin magnitude scaled (phase
/// preserved), then inverse-transformed and overlap-added at 50% overlap with
/// `1/frame` normalisation. Output is delayed by `hop = frame / 2` samples.
pub struct StftConvolver {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    window: Vec<f32>,
    /// Circular input buffer, `frame` samples; oldest sample at `in_pos`.
    in_buf: Vec<f32>,
    in_pos: usize,
    /// Overlap-add output accumulator, `frame` samples.
    out_buf: Vec<f32>,
    /// Read/write position within the current hop (`0..hop`).
    out_pos: usize,
    scratch_time: Vec<f32>,
    scratch_freq: Vec<Complex<f32>>,
    /// Pre-allocated realfft scratch (forward and inverse can differ).
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    frame: usize,
    hop: usize,
}

impl StftConvolver {
    /// A convolver with a fixed `frame`-point transform and `hop = frame / 2`
    /// (50% overlap). `frame` must be even and a power of two. The analysis
    /// window is a periodic Hann window.
    pub fn new(frame: usize) -> Self {
        let hop = frame / 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(frame);
        let ifft = planner.plan_fft_inverse(frame);
        let scratch_fwd = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        Self {
            fft,
            ifft,
            window: hann_periodic(frame),
            in_buf: vec![0.0; frame],
            in_pos: 0,
            out_buf: vec![0.0; frame],
            out_pos: 0,
            scratch_time: vec![0.0; frame],
            scratch_freq: vec![Complex::new(0.0, 0.0); frame / 2 + 1],
            scratch_fwd,
            scratch_inv,
            frame,
            hop,
        }
    }

    /// Zero all state.
    pub fn reset(&mut self) {
        self.in_buf.iter_mut().for_each(|s| *s = 0.0);
        self.out_buf.iter_mut().for_each(|s| *s = 0.0);
        self.in_pos = 0;
        self.out_pos = 0;
    }

    /// Inherent latency in samples (`= hop`).
    pub fn latency(&self) -> usize {
        self.hop
    }

    /// Process one sample. `mags` is the per-bin magnitude gain
    /// (`frame / 2 + 1` bins). When `apply` is false the per-bin multiply is
    /// skipped — a delayed dry passthrough (identity). Output is delayed by
    /// `hop` samples.
    pub fn process(&mut self, sample: f32, mags: &[f32], apply: bool) -> f32 {
        if self.out_pos == 0 {
            self.out_buf.copy_within(self.hop..self.frame, 0);
            self.out_buf[self.hop..].fill(0.0);
            Self::process_frame(
                &self.in_buf,
                self.in_pos,
                &mut self.out_buf,
                mags,
                apply,
                &self.window,
                self.fft.as_ref(),
                self.ifft.as_ref(),
                &mut self.scratch_time,
                &mut self.scratch_freq,
                &mut self.scratch_fwd,
                &mut self.scratch_inv,
            );
        }
        self.in_buf[self.in_pos] = sample;
        let out = self.out_buf[self.out_pos];
        self.in_pos = (self.in_pos + 1) & (self.frame - 1);
        self.out_pos += 1;
        if self.out_pos >= self.hop {
            self.out_pos = 0;
        }
        out
    }

    /// STFT frame: window → FFT → per-bin magnitude multiply → IFFT →
    /// overlap-add with `1/n` normalisation (the correct gain for a
    /// Hann-windowed 50%-overlap reconstruction). `apply == false` skips the
    /// multiply (identity).
    #[allow(clippy::too_many_arguments)]
    fn process_frame(
        in_buf: &[f32],
        in_pos: usize,
        out_buf: &mut [f32],
        mags: &[f32],
        apply: bool,
        window: &[f32],
        fft: &dyn RealToComplex<f32>,
        ifft: &dyn ComplexToReal<f32>,
        scratch_time: &mut [f32],
        scratch_freq: &mut [Complex<f32>],
        scratch_fwd: &mut [Complex<f32>],
        scratch_inv: &mut [Complex<f32>],
    ) {
        let n = in_buf.len();
        let mask = n - 1;
        for i in 0..n {
            scratch_time[i] = in_buf[(in_pos + i) & mask] * window[i];
        }
        if fft
            .process_with_scratch(scratch_time, scratch_freq, scratch_fwd)
            .is_err()
        {
            return;
        }
        if apply {
            for (bin, &mag) in scratch_freq.iter_mut().zip(mags.iter()) {
                *bin *= mag;
            }
        }
        if ifft
            .process_with_scratch(scratch_freq, scratch_time, scratch_inv)
            .is_err()
        {
            return;
        }
        let scale = 1.0 / n as f32;
        for i in 0..n {
            out_buf[i] += scratch_time[i] * scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_is_half_the_frame() {
        assert_eq!(StftConvolver::new(4096).latency(), 2048);
        assert_eq!(StftConvolver::new(2048).latency(), 1024);
    }

    #[test]
    fn identity_passthrough_when_not_applied() {
        // apply = false → delayed dry passthrough.
        let frame = 2048;
        let mags = vec![0.0_f32; frame / 2 + 1]; // ignored when apply = false
        let mut c = StftConvolver::new(frame);
        let mut last = 0.0;
        for _ in 0..8 * frame {
            last = c.process(0.5, &mags, false);
        }
        assert!((last - 0.5).abs() < 1e-3, "identity passthrough, got {last}");
    }

    #[test]
    fn flat_magnitude_preserves_a_steady_signal() {
        // All-ones magnitude spectrum ≈ unity gain after the pipeline fills.
        let frame = 2048;
        let mags = vec![1.0_f32; frame / 2 + 1];
        let mut c = StftConvolver::new(frame);
        let mut last = 0.0;
        for _ in 0..16 * frame {
            last = c.process(0.5, &mags, true);
        }
        assert!((last - 0.5).abs() < 5e-3, "flat magnitude ~unity, got {last}");
    }

    #[test]
    fn reset_clears_state() {
        let frame = 2048;
        let mags = vec![1.0_f32; frame / 2 + 1];
        let mut c = StftConvolver::new(frame);
        for _ in 0..4 * frame {
            c.process(0.9, &mags, true);
        }
        c.reset();
        // First sample after reset: output is the freshly-zeroed delay line.
        assert_eq!(c.process(0.0, &mags, true), 0.0);
    }
}
```

- [ ] **Step 3: Declare the module** — in `tract-dsp/src/lib.rs`, add a feature-gated declaration. After the `pub mod spsc;` line, add:

```rust
#[cfg(feature = "stft")]
pub mod stft;
```

(Keep the other `pub mod` lines as they are. `stft` is the one gated module.)

- [ ] **Step 4: Build, test, lint** — `stft` only compiles with the feature on:

- `cargo build -p tract-dsp` — clean (stft not built).
- `cargo build -p tract-dsp --features stft` — clean (stft built).
- `cargo nextest run -p tract-dsp --features stft` — PASS (the 4 new `stft` tests + `fir` + all existing).
- `cargo nextest run -p tract-dsp` — PASS (without `stft`; the `stft` tests simply aren't compiled).
- `cargo clippy -p tract-dsp --features stft -- -D warnings` — clean.
- `cargo fmt -p tract-dsp`, then `git diff --stat` — only `tract-dsp/src/stft.rs`, `tract-dsp/src/lib.rs`, `tract-dsp/Cargo.toml`, `Cargo.lock`.

- [ ] **Step 5: Commit**

```bash
git add tract-dsp/src/stft.rs tract-dsp/src/lib.rs tract-dsp/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
feat(tract-dsp): add stft module behind the stft cargo feature

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Migrate `miff`'s `RawChannel` to `FirRing`

`miff/src/convolution.rs`'s `RawChannel` is the cleanest base for `FirRing` — it is reimplemented as a thin wrapper, keeping its public API (`new`, `reset`, `process`) so `miff/src/lib.rs` is untouched.

**Files:** Modify `miff/src/convolution.rs`.

- [ ] **Step 1: Replace the `RawChannel` struct and impl.** In `miff/src/convolution.rs`, the current `RawChannel` is a struct with `history`/`write_pos`/`mask`/`is_silent` and an `impl` with `new`/`reset`/`push`/`process`. Replace the entire struct definition and its `impl` block (the section from `pub struct RawChannel {` through the end of `impl Default for RawChannel`) with:

```rust
/// Per-channel time-domain convolution state. A thin wrapper over
/// `tract_dsp::fir::FirRing`.
///
/// The silence fast-path only re-arms on `reset()`; a host `process()` loop
/// should call `reset()` after sustained input silence.
pub struct RawChannel {
    ring: tract_dsp::fir::FirRing,
}

impl RawChannel {
    /// A channel sized for kernels up to `MAX_KERNEL` taps.
    pub fn new() -> Self {
        Self {
            ring: tract_dsp::fir::FirRing::new(MAX_KERNEL),
        }
    }

    /// Zero the history.
    pub fn reset(&mut self) {
        self.ring.reset();
    }

    /// Process one sample through `kernel`; returns the filtered output.
    /// All-zero kernel -> the input is returned unchanged (dry passthrough).
    pub fn process(&mut self, sample: f32, kernel: &Kernel) -> f32 {
        self.ring.push(sample);
        if kernel.is_zero {
            return sample; // dry passthrough — see the miff spec
        }
        if self.ring.is_silent() {
            return 0.0; // silence fast-path: history is all zero
        }
        self.ring.mac(&kernel.rev_taps[..kernel.len])
    }
}

impl Default for RawChannel {
    fn default() -> Self {
        Self::new()
    }
}
```

This preserves `RawChannel`'s exact behaviour: `push`, then the zero-kernel passthrough, then the silence fast-path, then `mac` over the kernel's reversed taps. (`Kernel`'s `rev_taps` is `[f32; MAX_KERNEL]` with `[..len]` the meaningful reversed taps — `FirRing::mac` reads exactly `len` taps. `len` is a multiple of 16, guaranteed by `bake_taps`.)

- [ ] **Step 2: Drop the now-unused SIMD import.** `convolution.rs`'s top has `use std::simd::{f32x16, num::SimdFloat};`. After Step 1, `RawChannel` no longer uses `f32x16` directly. Check whether `PhaselessChannel` (still present, migrated in Task 4) or anything else in the file still uses it (`rg -n 'f32x16|SimdFloat' miff/src/convolution.rs`). `PhaselessChannel` does NOT use SIMD, so after Step 1 the import is unused — remove the `use std::simd::{f32x16, num::SimdFloat};` line. (Task 4's clippy run will confirm; if for some reason a hit remains, keep the import.)

- [ ] **Step 3: Build, test, lint**

- `cargo build -p miff` — clean.
- `cargo nextest run -p miff` — PASS. `RawChannel`'s tests (`unit_impulse_kernel_passes_audio_through`, `zero_kernel_is_dry_passthrough`, `silence_fast_path_outputs_exact_zero`, `known_kernel_yields_known_output`) must stay green — they exercise the wrapper.
- `cargo clippy -p miff -- -D warnings` — clean.
- `cargo fmt -p miff`, then `git diff --stat` — only `miff/src/convolution.rs`.

- [ ] **Step 4: Commit**

```bash
git add miff/src/convolution.rs
git commit -m "$(cat <<'EOF'
refactor(miff): RawChannel wraps tract-dsp::fir::FirRing

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Migrate `miff`'s `PhaselessChannel` to `StftConvolver`

**Files:** Modify `miff/Cargo.toml`, `miff/src/convolution.rs`.

- [ ] **Step 1: Enable the `stft` feature.** In `miff/Cargo.toml`, `miff` currently has `tract-dsp = { path = "../tract-dsp" }`. Change it to:

```toml
tract-dsp = { path = "../tract-dsp", features = ["stft"] }
```

- [ ] **Step 2: Replace `PhaselessChannel`.** In `miff/src/convolution.rs`, replace the entire `PhaselessChannel` struct, its `impl` block (including the private `process_frame_static`), and `impl Default for PhaselessChannel` — everything from `pub struct PhaselessChannel {` through the end of `impl Default for PhaselessChannel` — with:

```rust
/// Per-channel STFT magnitude-only convolution state. A thin wrapper over
/// `tract_dsp::stft::StftConvolver` with a fixed `STFT_FRAME`-point transform.
pub struct PhaselessChannel {
    conv: tract_dsp::stft::StftConvolver,
}

impl PhaselessChannel {
    pub fn new() -> Self {
        Self {
            conv: tract_dsp::stft::StftConvolver::new(STFT_FRAME),
        }
    }

    /// Zero all state.
    pub fn reset(&mut self) {
        self.conv.reset();
    }

    /// Process one sample. Output is always delayed by `PHASELESS_HOP` samples.
    ///
    /// A zero kernel maps to per-bin gain 1.0 (identity) — a *delayed* dry
    /// passthrough, never a 0-delay bypass.
    pub fn process(&mut self, sample: f32, kernel: &Kernel) -> f32 {
        self.conv.process(sample, &kernel.mags, !kernel.is_zero)
    }
}

impl Default for PhaselessChannel {
    fn default() -> Self {
        Self::new()
    }
}
```

Keep the `STFT_FRAME` / `PHASELESS_HOP` / `PHASELESS_LATENCY` constants and their doc comments exactly as they are — they are part of `miff`'s public surface and `lib.rs` uses them. (`STFT_FRAME` = `MAX_KERNEL` = 4096; `StftConvolver::new(STFT_FRAME).latency()` = 2048 = `PHASELESS_HOP`, consistent with `PHASELESS_LATENCY`.)

- [ ] **Step 3: Remove now-unused imports.** After Tasks 3–4, `convolution.rs` no longer uses `realfft`, `rustfft::num_complex::Complex`, or `std::sync::Arc` directly (all FFT work is inside `tract-dsp`). Run `rg -n 'realfft|rustfft|Complex|Arc' miff/src/convolution.rs` and remove any import line that is now unused (`use realfft::...;`, `use rustfft::num_complex::Complex;`, `use std::sync::Arc;`). Keep `use crate::kernel::{Kernel, MAG_BINS, MAX_KERNEL};` — but check whether `MAG_BINS` is still referenced; if not, narrow that import to `{Kernel, MAX_KERNEL}`. `cargo clippy` in Step 4 flags any unused import or `use`.

- [ ] **Step 4: Build, test, lint**

- `cargo build -p miff` — clean.
- `cargo nextest run -p miff` — PASS. `PhaselessChannel`'s tests (`phaseless_reports_fixed_hop_latency`, `phaseless_zero_kernel_is_dry_passthrough`, `phaseless_flat_magnitude_preserves_signal_energy`) must stay green.
- `cargo clippy -p miff -- -D warnings` — clean.
- `cargo fmt -p miff`, then `git diff --stat` — only `miff/Cargo.toml`, `miff/src/convolution.rs`, `Cargo.lock`.

- [ ] **Step 5: Commit**

```bash
git add miff/Cargo.toml miff/src/convolution.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(miff): PhaselessChannel wraps tract-dsp::stft::StftConvolver

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Carve `wavetable-filter`'s Raw path onto `FirRing`

`wavetable-filter`'s `FilterState` is the same double-buffered ring as `FirRing`. Remove `FilterState` entirely; the plugin holds `[FirRing; 2]`. The Raw-mode crossfade (two-kernel blend) stays in `process()` — it now calls `FirRing::push` once and `FirRing::mac` once or twice.

**Files:** Modify `wavetable-filter/src/lib.rs`. Work by content (line numbers shift). Read the file's `FilterState`, `Default`, `process()` Raw branch, and the `#[cfg(test)] mod tests` first.

- [ ] **Step 1: Add the import.** Near the top of `wavetable-filter/src/lib.rs`, add `use tract_dsp::fir::FirRing;` with the other `use` lines.

- [ ] **Step 2: Delete the `FilterState` struct and its `impl`.** Remove the `struct FilterState { history, write_pos, len, mask, is_silent }` definition and the entire `impl FilterState { ... }` block (`new`, `reset`, `push`, `is_silent`, the `#[cfg(test)] get`, and `history_slice`). `FirRing` replaces all of it.

- [ ] **Step 3: Change the `filter_state` field type.** In `struct WavetableFilter`, the field is `filter_state: [FilterState; 2]`. Change it to `filter_state: [FirRing; 2]`.

- [ ] **Step 4: Update `Default`.** In `impl Default for WavetableFilter`, the field is initialised `filter_state: [FilterState::new(KERNEL_LEN), FilterState::new(KERNEL_LEN)]`. Change to `filter_state: [FirRing::new(KERNEL_LEN), FirRing::new(KERNEL_LEN)]`.

- [ ] **Step 5: Update the reload path.** In `process()`, the wavetable-reload block contains:

```rust
                    for state in &mut self.filter_state {
                        if state.len != KERNEL_LEN {
                            *state = FilterState::new(KERNEL_LEN);
                        }
                    }
```

`FirRing` has no public `len` field, and its capacity is fixed at construction (always `KERNEL_LEN` here, which never changes). This guard is dead under `FirRing` — replace the whole `for` loop with a `reset` of each ring (preserving the original intent of clearing convolution history on a wavetable swap):

```rust
                    for state in &mut self.filter_state {
                        state.reset();
                    }
```

- [ ] **Step 6: Rewrite the Raw-mode MAC.** In `process()`'s per-sample loop, the Raw branch (`if filter_mode == FilterMode::Raw { ... }`) currently does:

```rust
                        self.filter_state[state_idx].push(input);

                        // SIMD convolution: forward dot product of the double-buffered
                        // history and time-reversed kernel. No per-element copies needed.
                        const SIMD_LANES: usize = 16;
                        const SIMD_CHUNKS: usize = KERNEL_LEN / SIMD_LANES;
                        let history = self.filter_state[state_idx].history_slice();

                        let filtered: f32 = if self.filter_state[state_idx].is_silent() {
                            0.0
                        } else if self.crossfade_active {
                            let mut acc = f32x16::splat(0.0);
                            let mut acc2 = f32x16::splat(0.0);
                            for chunk_idx in 0..SIMD_CHUNKS {
                                let k = chunk_idx * SIMD_LANES;
                                let h = f32x16::from_slice(&history[k..k + SIMD_LANES]);
                                acc += h * f32x16::from_slice(
                                    &self.synthesized_kernel[k..k + SIMD_LANES],
                                );
                                acc2 += h * f32x16::from_slice(
                                    &self.crossfade_target_kernel[k..k + SIMD_LANES],
                                );
                            }
                            let a = self.crossfade_alpha;
                            hsum(acc) * (1.0 - a) + hsum(acc2) * a
                        } else {
                            let mut acc = f32x16::splat(0.0);
                            for chunk_idx in 0..SIMD_CHUNKS {
                                let k = chunk_idx * SIMD_LANES;
                                let h = f32x16::from_slice(&history[k..k + SIMD_LANES]);
                                acc += h * f32x16::from_slice(
                                    &self.synthesized_kernel[k..k + SIMD_LANES],
                                );
                            }
                            hsum(acc)
                        };
```

Replace that whole block (from `self.filter_state[state_idx].push(input);` through the end of the `let filtered: f32 = ...;` statement) with:

```rust
                        self.filter_state[state_idx].push(input);

                        // SIMD convolution against the time-reversed kernel(s).
                        let filtered: f32 = if self.filter_state[state_idx].is_silent() {
                            // History is all-zero (filter cleared after 100 ms
                            // of silence) — convolution output is zero; skip the
                            // MAC entirely.
                            0.0
                        } else if self.crossfade_active {
                            let a = self.crossfade_alpha;
                            self.filter_state[state_idx].mac(&self.synthesized_kernel)
                                * (1.0 - a)
                                + self.filter_state[state_idx]
                                    .mac(&self.crossfade_target_kernel)
                                    * a
                        } else {
                            self.filter_state[state_idx].mac(&self.synthesized_kernel)
                        };
```

This is bit-identical: `FirRing::mac` performs the same per-chunk `f32x16` multiply-accumulate over the same window and the same `KERNEL_LEN`-long kernel, reduced with `reduce_sum` (the old `hsum` is also `reduce_sum`). The crossfade case did two interleaved accumulators in one loop; two separate `mac` calls accumulate each in the identical chunk order, so each `hsum` result is unchanged. (`synthesized_kernel` and `crossfade_target_kernel` are both `Vec<f32>` of `KERNEL_LEN` — a multiple of 16 — so `mac`'s length contract holds.)

- [ ] **Step 7: Clean up newly-dead code.** After Step 6:
  - `hsum` (the `fn hsum(v: f32x16) -> f32` helper near the top) — check `rg -n 'hsum' wavetable-filter/src/lib.rs`. If it has no remaining callers, delete the `hsum` function. If it still has callers (e.g. in tests or kernel synthesis), leave it.
  - The `use std::simd::f32x16;` / `use std::simd::num::SimdFloat;` imports — check `rg -n 'f32x16|SimdFloat' wavetable-filter/src/lib.rs`. The crossfade-bake block elsewhere in `process()` (the `if self.crossfade_active { ... }` inside the `needs_update` branch) still uses `f32x16` — so `f32x16` is very likely still used; keep its import if so. Remove only genuinely-unused imports. `cargo clippy` in Step 9 is the arbiter.

- [ ] **Step 8: Update the tests.** `wavetable-filter`'s `#[cfg(test)] mod tests` has `FilterState`-specific tests and a `push_sequence(state: &mut FilterState, ...)` helper. With `FilterState` gone, these no longer compile. Remove the `push_sequence` helper and every test that constructs or operates on a `FilterState` directly (their coverage now lives in `tract-dsp`'s `fir` tests). Do NOT remove tests that exercise the plugin as a whole or the Raw/STFT signal path through `process()`-style helpers. If a test is ambiguous, prefer keeping it and adapting it to `FirRing`'s API (`new`/`reset`/`push`/`is_silent`/`mac`) only if that is a trivial rename; otherwise remove it and note it. List every test you remove in your report.

- [ ] **Step 9: Build, test, lint**

- `cargo build -p wavetable-filter` — clean.
- `cargo nextest run -p wavetable-filter` — PASS. Every remaining test green — especially any Raw-convolution signal-path test.
- `cargo clippy -p wavetable-filter -- -D warnings` — clean.
- `cargo fmt -p wavetable-filter`, then `git diff --stat` — only `wavetable-filter/src/lib.rs`.

If a signal-path test fails, STOP and report — a failure means the carve-out changed behaviour.

- [ ] **Step 10: Commit**

```bash
git add wavetable-filter/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(wavetable-filter): carve Raw convolution onto tract-dsp::fir::FirRing

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Carve `wavetable-filter`'s STFT path onto `StftConvolver`

Remove the STFT state fields and the `process_stft_frame` associated fn; the plugin holds `[StftConvolver; 2]`.

**Files:** Modify `wavetable-filter/Cargo.toml`, `wavetable-filter/src/lib.rs`. Work by content. Read the STFT state fields, `process_stft_frame`, `Default`, `initialize`, `reset`, `process()`'s STFT hop loop, and the `run_stft_mono` test helper first.

- [ ] **Step 1: Enable the `stft` feature.** In `wavetable-filter/Cargo.toml`, change `tract-dsp = { path = "../tract-dsp" }` to `tract-dsp = { path = "../tract-dsp", features = ["stft"] }`.

- [ ] **Step 2: Add the import.** Near the top of `wavetable-filter/src/lib.rs`, add `use tract_dsp::stft::StftConvolver;`.

- [ ] **Step 3: Replace the STFT state fields.** `struct WavetableFilter` has a `// ── STFT state ──` block of fields: `stft_fft`, `stft_in: [Vec<f32>; 2]`, `stft_out: [Vec<f32>; 2]`, `stft_magnitudes: Vec<f32>`, `stft_window: Vec<f32>`, `stft_scratch: Vec<f32>`, `stft_in_pos`, `stft_out_pos`. Of these, `stft_magnitudes` (the filter's magnitude spectrum, set by `compute_stft_magnitudes`) MUST stay — it is the per-bin gain handed to the convolver. `stft_window` and `stft_scratch` are also used by the **input-spectrum analyzer** code in `process()` (`rg -n 'stft_window|stft_scratch' wavetable-filter/src/lib.rs` to confirm) — those must stay too. Remove only the convolution state: `stft_fft`, `stft_in`, `stft_out`, `stft_in_pos`, `stft_out_pos`. Add one field in their place:

```rust
    /// Per-channel STFT magnitude convolvers (Phaseless mode).
    stft: [StftConvolver; 2],
```

- [ ] **Step 4: Update `Default`.** Remove the `Default` initialisers for the removed fields (`stft_fft`, `stft_in`, `stft_out`, `stft_in_pos`, `stft_out_pos`). Add `stft: [StftConvolver::new(KERNEL_LEN), StftConvolver::new(KERNEL_LEN)],`. Keep the `stft_magnitudes`, `stft_window`, `stft_scratch` initialisers. The `stft_fft` planner line in `Default` (`let stft_fft = real_planner.plan_fft_forward(KERNEL_LEN);`) becomes unused — remove it.

- [ ] **Step 5: Delete `process_stft_frame`.** Remove the entire `fn process_stft_frame(...)` associated function — `StftConvolver` replaces it.

- [ ] **Step 6: Rewrite the STFT hop loop in `process()`.** The per-sample loop's STFT branch currently has a hop-triggered block calling `Self::process_stft_frame(...)` and an `else` branch that reads from `stft_out`/writes `stft_in`. The whole STFT path — the `if filter_mode != FilterMode::Raw && self.stft_out_pos == 0 { ... }` hop block, AND the `else` branch `self.stft_in[state_idx][self.stft_in_pos] = input; let filtered = self.stft_out[state_idx][self.stft_out_pos];`, AND the `stft_in_pos`/`stft_out_pos` advance at the loop's end — is replaced by a single per-sample `StftConvolver::process` call. In the per-channel branch, the STFT (`else`, i.e. non-`Raw`) arm becomes:

```rust
                    } else {
                        let filtered =
                            self.stft[state_idx].process(input, &self.stft_magnitudes, true);
                        *sample = (input * (1.0 - mix) + filtered * mix * reset_gain) * gain;
                    }
```

Delete the separate hop-trigger block (the `if filter_mode != FilterMode::Raw && self.stft_out_pos == 0 { for ch ... process_stft_frame ... }`) and the `stft_in_pos`/`stft_out_pos` advance block at the end of the loop — `StftConvolver::process` does its own hop framing internally. Be careful: the Raw arm's per-sample crossfade-alpha advance must stay; only the STFT-position bookkeeping goes.

- [ ] **Step 7: Update `initialize` and `reset`.** `initialize()` and `reset()` and the mode-switch and silence-clear paths zero `stft_in`/`stft_out` and reset `stft_in_pos`/`stft_out_pos`. Replace each such block with `for c in &mut self.stft { c.reset(); }`. (`rg -n 'stft_in|stft_out|stft_in_pos|stft_out_pos' wavetable-filter/src/lib.rs` to find every site — there are several: `initialize`, `reset`, the mode-switch block in `process()`, and the 100 ms-silence-clear block in `process()`.) Each becomes a `reset()` of the two convolvers. Do NOT touch `stft_magnitudes` handling.

- [ ] **Step 8: Update the reported STFT latency.** `process()` computes `let stft_latency = if filter_mode == FilterMode::Raw { 0 } else { HOP as u32 };`. `HOP` is `KERNEL_LEN / 2` — equal to `self.stft[0].latency()`. Leave this as-is (the `HOP` constant still exists and equals the convolver latency); it remains correct. (Optionally, for clarity, `self.stft[0].latency() as u32` — but leaving `HOP` is zero-diff and correct.)

- [ ] **Step 9: Update the `run_stft_mono` test helper.** `wavetable-filter`'s `#[cfg(test)] mod tests` has `run_stft_mono(plugin: &mut WavetableFilter, input: &[f32])` which hand-drives the STFT path by calling `WavetableFilter::process_stft_frame(...)` and the `stft_in`/`stft_out` bookkeeping directly. Rewrite it to drive the convolver instead:

```rust
    fn run_stft_mono(plugin: &mut WavetableFilter, input: &[f32]) -> Vec<f32> {
        input
            .iter()
            .map(|&s| plugin.stft[0].process(s, &plugin.stft_magnitudes, true))
            .collect()
    }
```

The STFT tests that use `run_stft_mono` (`test_stft_lowpass_attenuates_highs`, `test_stft_flat_preserves_amplitude`) set `plugin.stft_magnitudes` then call `run_stft_mono` — they keep working unchanged with this rewrite (same magnitudes, same convolution). If any other test referenced `stft_in`/`stft_out`/`process_stft_frame` directly, adapt or remove it and note it in your report.

- [ ] **Step 10: Clean up.** Check `rg -n 'stft_fft|stft_in\b|stft_out\b|stft_in_pos|stft_out_pos|process_stft_frame' wavetable-filter/src/lib.rs` — zero matches outside comments. Remove any import left unused by the FFT-state removal (`cargo clippy` confirms).

- [ ] **Step 11: Build, test, lint**

- `cargo build -p wavetable-filter` — clean.
- `cargo nextest run -p wavetable-filter` — PASS. The STFT tests (`test_stft_lowpass_attenuates_highs`, `test_stft_flat_preserves_amplitude`, `test_stft_magnitudes_match_spectrum`, `test_stft_magnitudes_zero_resonance`, `test_frame_sweep_regression`, the `bench_*` tests) must stay green.
- `cargo clippy -p wavetable-filter -- -D warnings` — clean.
- `cargo fmt -p wavetable-filter`, then `git diff --stat` — only `wavetable-filter/Cargo.toml`, `wavetable-filter/src/lib.rs`, `Cargo.lock`.

If a STFT signal-path test fails, STOP and report — a failure means the carve-out changed behaviour.

- [ ] **Step 12: Commit**

```bash
git add wavetable-filter/Cargo.toml wavetable-filter/src/lib.rs Cargo.lock
git commit -m "$(cat <<'EOF'
refactor(wavetable-filter): carve STFT convolution onto tract-dsp::stft::StftConvolver

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Workspace verification

**Files:** none (verification only).

- [ ] **Step 1:** `cargo build --workspace` — every crate compiles clean.
- [ ] **Step 2:** `cargo nextest run --workspace` — all tests pass. Baseline before this plan is 769; this plan adds `tract-dsp`'s `fir` (4) + `stft` (4) tests and removes the `RawChannel`/`PhaselessChannel` internal tests from `miff` and the `FilterState` tests from `wavetable-filter` (their coverage relocates into `tract-dsp`). The net count will differ from 769 — that is expected; what matters is **zero failures** and that no *signal-path* test was lost. If `concurrent_writer_reader_no_torn_index` fails, re-run once (known flaky stress test).
- [ ] **Step 3:** `cargo clippy --workspace -- -D warnings` — clean (CI lint command).
- [ ] **Step 4:** `cargo fmt --check` — clean. If it reports a diff in a file this plan touched, run `cargo fmt` and amend the relevant commit.
- [ ] **Step 5: Confirm the duplication is gone.**
  - `rg -n 'f32x16' miff/src/convolution.rs` — no matches (the SIMD MAC moved to `tract-dsp::fir`).
  - `rg -n 'process_stft_frame|RealFftPlanner' miff/src/convolution.rs wavetable-filter/src/lib.rs` — no matches in `convolution.rs`; in `wavetable-filter/lib.rs`, `RealFftPlanner` may still appear for kernel synthesis (`compute_base_spectrum_into`/`apply_resonance_and_ifft`) but `process_stft_frame` must be gone.
  - `cargo build -p tract-dsp` (no features) still compiles — confirms `stft` stays optional and `fir` is feature-free.

---

## Self-Review (completed by plan author)

**Spec coverage:** `fir` module → Task 1; `stft` module + feature → Task 2; miff `RawChannel` → Task 3; miff `PhaselessChannel` → Task 4; wavetable-filter Raw carve-out → Task 5; wavetable-filter STFT carve-out → Task 6; verification → Task 7. The `fir`/`stft` split, the `push`/`mac` separation for the crossfade, the `stft` cargo feature, the minimal carve-out (no `lib.rs` reorg) — all covered.

**Placeholder scan:** No `TBD`/`TODO`. New modules have complete code. The carve-out tasks (5, 6) show the exact old block to replace and the exact new code; the "locate by content / `rg` to confirm" instructions are concrete because line numbers shift in a 2933-line file and a few cleanup outcomes (unused imports, whether `hsum` survives) genuinely depend on the rest of the file — clippy is named as the arbiter.

**Type consistency:** `FirRing` (`new`/`reset`/`push`/`is_silent`/`mac`) used identically in Tasks 1, 3, 5. `StftConvolver` (`new`/`reset`/`latency`/`process`) used identically in Tasks 2, 4, 6. `RawChannel`/`PhaselessChannel` keep their existing public signatures so `miff/src/lib.rs` is untouched. `mac` takes `&[f32]`; callers pass `&kernel.rev_taps[..kernel.len]` (miff) and `&self.synthesized_kernel` (wavetable-filter). `process` takes `(sample, mags: &[f32], apply: bool)`; callers pass `(&kernel.mags, !kernel.is_zero)` (miff) and `(&self.stft_magnitudes, true)` (wavetable-filter).
