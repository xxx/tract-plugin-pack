# tract-dsp `StftAnalyzer` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the duplicated STFT analysis front-end shared by `satch` and `warp-zone` — input ring, hop windowing, forward FFT, COLA-derived synthesis window — into a `tract-dsp` module and migrate both plugins, with zero behaviour change.

**Architecture:** A new `tract-dsp/src/stft_analysis.rs` module behind a `stft-analysis` cargo feature provides `StftAnalyzer`: it owns the input ring, the periodic-Hann analysis window, the COLA synthesis window, and the forward FFT plan. `analyze()` returns an `StftFrame` bundling the forward-FFT spectrum and the synthesis window (disjoint borrows of two analyzer fields, so a plugin can use both across its whole frame block). Each plugin keeps its own synthesis half — inverse FFT, output ring(s), overlap-add, `1/N` normalisation, per-bin transform — and its own hop counter. The plugin decides per sample whether to `write` (skipped for `warp-zone` freeze) and whether to `analyze` (skipped for `satch` `skip_fft`); the modes fall out of which methods it calls, with no bool parameters.

**Tech Stack:** Rust (nightly, workspace-pinned), `rustfft` 6.2 (complex FFT), `cargo nextest`, `cargo clippy`. The shared module reuses `tract_dsp::window::hann_periodic` (already in the crate since Phase 2a).

---

## Background for the implementer

You are working in the `tract-plugin-pack` Cargo workspace — audio-effect plugins built with `nih-plug`. You are on branch `tract-dsp-stft-analyzer`. The hard constraint for this whole plan is **zero behaviour change**: `satch` and `warp-zone` must produce bit-identical output. The shared `StftAnalyzer` reproduces exactly the analysis-side arithmetic both plugins run today; their synthesis halves are not touched.

Why a *full* shared STFT engine is not possible (and is explicitly out of scope): `satch` has **two** synthesis output rings (a loud/quiet detail split), `warp-zone` has **one**; and `satch` applies the `1/N` normalisation in the frequency domain while `warp-zone` applies it in the time domain. Because f32 arithmetic is not associative, no single engine-level normalisation point is bit-identical to both. What *is* genuinely identical is the analysis front-end — that, and only that, is what this plan extracts.

Key facts:

- **Rust 2021 disjoint field borrows.** Within a single function body, you may hold a borrow of `self.field_a` while separately borrowing `self.field_b`. This is why `StftAnalyzer::analyze` can return a struct borrowing two of its fields at once, and why `satch`'s and `warp-zone`'s `process_frame` can keep the returned `StftFrame` live while mutating other (disjoint) `self` fields. What you may **not** do is call a `&mut self` method while a borrow of `self` (e.g. the `StftFrame`) is live — that is why `warp-zone`'s `remap_bins` becomes an associated function taking explicit slice parameters (Task 3).
- **`rustfft` FFT plans are deterministic** for a given size. `FftPlanner::new().plan_fft_forward(n)` always produces the same algorithm and the same bit-exact output for the same input. The scratch buffer is workspace only — its length does not affect output values.
- **No allocations on the audio thread.** `analyze`, `write`, `reset`, `process_sample`, `process_frame` must not allocate. All buffers are pre-allocated in `new`. Do not introduce `Vec::new()`, `collect()`, `clone()` of collections, etc. in those paths.
- **Build/test commands** (run from the workspace root):
  - `cargo nextest run --workspace` — full test suite (parallel runner).
  - `cargo nextest run -p tract-dsp --features stft-analysis` — `tract-dsp`'s own tests with the new feature on.
  - `cargo clippy --workspace -- -D warnings` — the lint gate CI enforces.
  - `cargo fmt --check` — formatting gate.
  - `cargo build --workspace` — compile check.
- The commit-message trailer for every commit in this plan is:
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`

---

## File Structure

**Created:**

- `tract-dsp/src/stft_analysis.rs` — the `StftAnalyzer` + `StftFrame` types and their unit tests. One responsibility: the STFT analysis front-end.

**Modified:**

- `tract-dsp/Cargo.toml` — add the `stft-analysis` feature (`["dep:rustfft"]`).
- `tract-dsp/src/lib.rs` — declare the feature-gated module; refresh the crate doc comment.
- `satch/Cargo.toml` — add `features = ["stft-analysis"]` to the `tract-dsp` dependency.
- `satch/src/spectral.rs` — `SpectralClipper`'s analysis front-end fields → a `StftAnalyzer`; `new`, `reset`, `process_sample_inner`, `process_frame` updated.
- `warp-zone/Cargo.toml` — add `features = ["stft-analysis"]` to the `tract-dsp` dependency.
- `warp-zone/src/spectral.rs` — `SpectralShifter`'s analysis front-end fields → a `StftAnalyzer`; `new`, `reset`, `process_sample`, `process_frame` updated; `remap_bins` converted from a `&mut self` method to an associated function taking explicit slices.

---

## Task 1: `StftAnalyzer` module + `stft-analysis` feature

**Files:**
- Create: `tract-dsp/src/stft_analysis.rs`
- Modify: `tract-dsp/Cargo.toml`
- Modify: `tract-dsp/src/lib.rs`

- [ ] **Step 1: Add the `stft-analysis` feature to `tract-dsp/Cargo.toml`**

The file currently has a `[features]` section with one entry and an optional-deps `[dependencies]` section. Replace the `[features]` section so it reads:

```toml
[features]
# Gates the FFT-based `stft` module so non-FFT consumers don't pull realfft/rustfft.
stft = ["dep:realfft", "dep:rustfft"]
# Gates the `stft_analysis` module (rustfft only — complex transforms, no realfft).
stft-analysis = ["dep:rustfft"]
```

Leave the `[dependencies]` section unchanged — `rustfft` is already declared `optional = true` there, and `realfft` stays optional.

- [ ] **Step 2: Declare the module in `tract-dsp/src/lib.rs`**

The file currently declares modules and feature-gates `stft`. Update the crate doc comment's feature sentence and add the new module declaration. Change this doc line:

```rust
//! `nih-plug`, `tiny-skia`, `softbuffer`, or editor dependency. By default it
//! pulls no external crates at all — just `std` and `std::simd`. The optional
//! `stft` feature adds `realfft`/`rustfft` and enables the `stft` module.
```

to:

```rust
//! `nih-plug`, `tiny-skia`, `softbuffer`, or editor dependency. By default it
//! pulls no external crates at all — just `std` and `std::simd`. The optional
//! `stft` feature adds `realfft`/`rustfft` and enables the `stft` module; the
//! optional `stft-analysis` feature adds `rustfft` and enables the
//! `stft_analysis` module.
```

And add the module declaration. The module list currently reads:

```rust
pub mod boxcar;
pub mod db;
pub mod fir;
pub mod spsc;
#[cfg(feature = "stft")]
pub mod stft;
pub mod true_peak;
pub mod window;
```

Change it to (insert the `stft_analysis` declaration after `stft`, keeping alphabetical-ish order consistent with the existing file):

```rust
pub mod boxcar;
pub mod db;
pub mod fir;
pub mod spsc;
#[cfg(feature = "stft")]
pub mod stft;
#[cfg(feature = "stft-analysis")]
pub mod stft_analysis;
pub mod true_peak;
pub mod window;
```

- [ ] **Step 3: Write the failing tests for `StftAnalyzer`**

Create `tract-dsp/src/stft_analysis.rs` with the module doc comment, the imports, and **only the test module** for now (so the tests fail to compile against missing types — that is the "red" state):

```rust
//! STFT analysis front-end: input ring, hop windowing, forward FFT, COLA window.
//!
//! [`StftAnalyzer`] owns the input-side STFT scaffolding shared by `satch`'s
//! spectral clipper and `warp-zone`'s phase vocoder: a circular input ring, the
//! periodic-Hann analysis window, the COLA-derived synthesis window, and the
//! forward FFT plan. The caller owns the synthesis half — inverse FFT, output
//! ring(s), overlap-add, `1/N` normalisation, and the per-bin transform — and
//! its own hop counter. It calls [`StftAnalyzer::write`] once per input sample
//! and [`StftAnalyzer::analyze`] once per hop.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

use crate::window::hann_periodic;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::hann_periodic;

    #[test]
    fn latency_is_fft_size() {
        assert_eq!(StftAnalyzer::new(2048, 512).latency_samples(), 2048);
        assert_eq!(StftAnalyzer::new(4096, 1024).latency_samples(), 4096);
    }

    #[test]
    fn synthesis_window_is_analysis_over_cola() {
        // Periodic Hann at 75% overlap (hop = N/4): COLA factor = 1.5, so the
        // synthesis window is the analysis window scaled by 1/1.5.
        let mut a = StftAnalyzer::new(2048, 512);
        let analysis = hann_periodic(2048);
        let frame = a.analyze();
        for i in 0..2048 {
            let expected = analysis[i] / 1.5;
            assert!(
                (frame.synthesis_window[i] - expected).abs() < 1e-6,
                "synthesis[{i}] = {}, expected {expected}",
                frame.synthesis_window[i],
            );
        }
    }

    #[test]
    fn reset_clears_the_input_ring() {
        let mut a = StftAnalyzer::new(64, 16);
        for _ in 0..64 {
            a.write(0.9);
        }
        a.reset();
        // After reset the ring is silent: analysing it yields an all-zero spectrum.
        let frame = a.analyze();
        for bin in frame.spectrum.iter() {
            assert!(bin.norm() < 1e-6, "expected silent spectrum, got {bin}");
        }
    }

    #[test]
    fn dc_input_concentrates_energy_in_bin_zero() {
        // A windowed DC signal has its energy in the DC bin (and the two
        // adjacent bins from the Hann window's transform); every other bin,
        // and bin 1, sits strictly below the DC bin.
        let n = 2048;
        let mut a = StftAnalyzer::new(n, n / 4);
        for _ in 0..n {
            a.write(0.5);
        }
        let frame = a.analyze();
        let dc = frame.spectrum[0].norm();
        assert!(dc > 0.0, "DC bin should be non-zero for DC input");
        for k in 1..n / 2 {
            assert!(
                frame.spectrum[k].norm() < dc,
                "bin {k} ({}) should be below the DC bin ({dc})",
                frame.spectrum[k].norm(),
            );
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo nextest run -p tract-dsp --features stft-analysis`
Expected: FAIL — compile error, `cannot find type StftAnalyzer in this scope` (the test module references types that do not exist yet).

- [ ] **Step 5: Implement `StftFrame` and `StftAnalyzer`**

Insert the following into `tract-dsp/src/stft_analysis.rs` between the `use` lines and the `#[cfg(test)] mod tests` block:

```rust
/// One analysis frame handed to the caller by [`StftAnalyzer::analyze`].
///
/// `spectrum` and `synthesis_window` are disjoint fields of the analyzer, so
/// the caller can hold this whole struct live across its entire frame block —
/// reading and transforming the spectrum, then using the synthesis window for
/// overlap-add — without a borrow conflict.
pub struct StftFrame<'a> {
    /// `fft_size` complex bins — the forward FFT of the latest windowed frame.
    /// Not normalised; the caller applies whatever `1/N` scaling it needs.
    pub spectrum: &'a mut [Complex<f32>],
    /// The COLA-normalised synthesis window (`analysis_window / cola_factor`),
    /// `fft_size` samples — multiply by this during overlap-add.
    pub synthesis_window: &'a [f32],
}

/// Per-channel STFT analysis front-end. Owns the input ring, the periodic-Hann
/// analysis window, the COLA-derived synthesis window, and the forward FFT.
///
/// The caller owns the hop counter and the synthesis half (inverse FFT, output
/// ring(s), overlap-add, normalisation, per-bin transform). It calls
/// [`write`](Self::write) each sample and [`analyze`](Self::analyze) once per
/// hop.
pub struct StftAnalyzer {
    fft_size: usize,
    fft_forward: Arc<dyn Fft<f32>>,
    /// Forward-FFT in-place scratch.
    scratch: Vec<Complex<f32>>,
    analysis_window: Vec<f32>,
    /// Pre-multiplied synthesis window: `analysis_window[i] / cola_factor`.
    synthesis_window: Vec<f32>,
    /// Circular buffer of the most recent `fft_size` input samples.
    input_ring: Vec<f32>,
    /// Write cursor into `input_ring`; also the oldest sample for the next frame.
    input_pos: usize,
    /// Pre-allocated FFT workspace; holds the spectrum returned by `analyze`.
    fft_buf: Vec<Complex<f32>>,
}

impl StftAnalyzer {
    /// Create an `fft_size`-point analyzer. `hop_size` is used only to compute
    /// the COLA synthesis window. `fft_size` must be a power of two and
    /// `fft_size >= hop_size`.
    pub fn new(fft_size: usize, hop_size: usize) -> Self {
        assert!(fft_size > 0 && hop_size > 0 && fft_size >= hop_size);

        let mut planner = FftPlanner::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let scratch_len = fft_forward.get_inplace_scratch_len();

        let analysis_window: Vec<f32> = hann_periodic(fft_size);

        // COLA normalization for a Hann window: the sum of squared window
        // values across the `fft_size / hop_size` overlapping frames is
        // constant. Dividing the synthesis window by that constant makes
        // overlap-add reconstruct unity gain.
        let num_frames = fft_size / hop_size;
        let mut cola_check = vec![0.0_f64; hop_size];
        for frame in 0..num_frames {
            let offset = frame * hop_size;
            for p in 0..hop_size {
                let w = analysis_window[p + offset] as f64;
                cola_check[p] += w * w;
            }
        }
        let cola_factor = cola_check[0] as f32;
        let inv_cola = 1.0 / cola_factor;
        let synthesis_window: Vec<f32> =
            analysis_window.iter().map(|&w| w * inv_cola).collect();

        Self {
            fft_size,
            fft_forward,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            analysis_window,
            synthesis_window,
            input_ring: vec![0.0; fft_size],
            input_pos: 0,
            fft_buf: vec![Complex::new(0.0, 0.0); fft_size],
        }
    }

    /// Write one input sample into the ring and advance. Skip this call to
    /// hold the ring frozen (e.g. `warp-zone`'s freeze).
    pub fn write(&mut self, input: f32) {
        self.input_ring[self.input_pos] = input;
        self.input_pos = (self.input_pos + 1) % self.fft_size;
    }

    /// Extract the latest `fft_size` samples (oldest-first, Hann-windowed) and
    /// forward-FFT them; return the spectrum plus the synthesis window. Call
    /// this once per hop. Skip it to suppress frame work (e.g. `satch`'s
    /// `skip_fft`).
    pub fn analyze(&mut self) -> StftFrame<'_> {
        let n = self.fft_size;
        for i in 0..n {
            let idx = (self.input_pos + i) % n;
            let windowed = self.input_ring[idx] * self.analysis_window[i];
            self.fft_buf[i] = Complex::new(windowed, 0.0);
        }
        self.fft_forward
            .process_with_scratch(&mut self.fft_buf, &mut self.scratch);
        StftFrame {
            spectrum: &mut self.fft_buf,
            synthesis_window: &self.synthesis_window,
        }
    }

    /// Zero the input ring, the position cursor, and the FFT workspace.
    pub fn reset(&mut self) {
        self.input_ring.fill(0.0);
        self.input_pos = 0;
        for bin in self.fft_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
    }

    /// Inherent latency in samples (`= fft_size`).
    pub fn latency_samples(&self) -> usize {
        self.fft_size
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p tract-dsp --features stft-analysis`
Expected: PASS — all four `stft_analysis` tests pass, and the rest of `tract-dsp`'s suite is unaffected.

- [ ] **Step 7: Lint and format**

Run: `cargo clippy -p tract-dsp --features stft-analysis -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --check`
Expected: no diff. (If it reports a diff, run `cargo fmt` and re-check.)

- [ ] **Step 8: Commit**

```bash
git add tract-dsp/Cargo.toml tract-dsp/src/lib.rs tract-dsp/src/stft_analysis.rs
git commit -m "feat(tract-dsp): add StftAnalyzer STFT analysis front-end

New stft_analysis module behind a stft-analysis cargo feature (rustfft
only). StftAnalyzer owns the input ring, periodic-Hann analysis window,
COLA synthesis window, and forward FFT; analyze() returns an StftFrame
bundling the spectrum and synthesis window. Shared front-end for the
satch/warp-zone STFT migrations.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Migrate `satch` onto `StftAnalyzer`

`satch`'s `SpectralClipper` drops the analysis-front-end fields (`fft_forward`, `analysis_window`, `synthesis_window`, `input_ring`, `input_pos`, and `fft_buf` — the latter becomes dead because `process_frame` now reads the analyzer's spectrum directly) and gains a `StftAnalyzer`. It keeps everything on the synthesis side: `fft_size`, `hop_size`, `fft_inverse`, `scratch` (now sized for the inverse FFT only), `mag_buf`, `loud_buf`, `quiet_buf`, `loud_output_ring`, `quiet_output_ring`, `read_pos`, `hop_counter`. The public API (`new`, `reset`, `latency_samples`, `process_sample`, `process_sample_skip_fft`, `process_sample_fast`, `process_sample_skip_fft_fast`, and the free `saturate_td*` functions) is unchanged, so `satch/src/lib.rs` is **not** touched.

**Files:**
- Modify: `satch/Cargo.toml`
- Modify: `satch/src/spectral.rs`
- Test (safety net, unchanged): `satch/src/spectral.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Establish the green baseline**

Run: `cargo nextest run -p satch`
Expected: PASS — note the test count; every one of these must still pass after the migration. This existing suite (STFT reconstruction, the loud/quiet split, flat-top clipping, detail preservation, the clip-aware blend) **is** the behaviour-preservation test for this task. Do not add or remove tests.

- [ ] **Step 2: Add the feature to `satch/Cargo.toml`**

The dependency line currently reads:

```toml
tract-dsp = { path = "../tract-dsp" }
```

Change it to:

```toml
tract-dsp = { path = "../tract-dsp", features = ["stft-analysis"] }
```

- [ ] **Step 3: Add the import to `satch/src/spectral.rs`**

After the existing `use std::sync::Arc;` line, add:

```rust
use tract_dsp::stft_analysis::StftAnalyzer;
```

(Leave `use rustfft::num_complex::Complex;` and `use rustfft::{Fft, FftPlanner};` — `Complex`, `Fft`, and `FftPlanner` are all still used by the kept inverse-FFT path.)

- [ ] **Step 4: Replace the `SpectralClipper` struct fields**

The struct currently declares the fields `fft_size`, `hop_size`, `fft_forward`, `fft_inverse`, `scratch`, `analysis_window`, `synthesis_window`, `mag_buf`, `input_ring`, `loud_output_ring`, `quiet_output_ring`, `input_pos`, `read_pos`, `hop_counter`, `fft_buf`, `loud_buf`, `quiet_buf`. Replace the whole `pub struct SpectralClipper { ... }` block (keep the doc comment above it) with:

```rust
pub struct SpectralClipper {
    fft_size: usize,
    hop_size: usize,

    /// Shared STFT analysis front-end (input ring, analysis window, forward
    /// FFT, COLA synthesis window).
    stft: StftAnalyzer,

    // Inverse FFT plan + its in-place scratch.
    fft_inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,

    // Pre-allocated workspace for per-bin magnitudes (avoids recomputing norm)
    mag_buf: Vec<f32>,

    // Ring buffers
    /// Overlap-add accumulation buffer for loud bins (above threshold). Size = 2 x fft_size.
    loud_output_ring: Vec<f64>,
    /// Overlap-add accumulation buffer for quiet bins (below threshold). Size = 2 x fft_size.
    quiet_output_ring: Vec<f64>,
    /// Current read position in output rings.
    read_pos: usize,
    /// Sample counter within current hop.
    hop_counter: usize,

    // Pre-allocated FFT workspace
    /// Loud bins (above threshold) for separate ISTFT.
    loud_buf: Vec<Complex<f32>>,
    /// Quiet bins (below threshold) for separate ISTFT.
    quiet_buf: Vec<Complex<f32>>,
}
```

- [ ] **Step 5: Rewrite `SpectralClipper::new`**

Replace the body of `new` (keep its doc comment and signature `pub fn new(fft_size: usize, hop_size: usize) -> Self`) with:

```rust
    pub fn new(fft_size: usize, hop_size: usize) -> Self {
        assert!(fft_size > 0 && hop_size > 0 && fft_size >= hop_size);

        let mut planner = FftPlanner::new();
        let fft_inverse = planner.plan_fft_inverse(fft_size);
        let scratch_len = fft_inverse.get_inplace_scratch_len();

        let out_ring_size = 2 * fft_size;

        Self {
            fft_size,
            hop_size,
            stft: StftAnalyzer::new(fft_size, hop_size),
            fft_inverse,
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            mag_buf: vec![0.0; fft_size],
            loud_output_ring: vec![0.0; out_ring_size],
            quiet_output_ring: vec![0.0; out_ring_size],
            read_pos: 0,
            hop_counter: 0,
            loud_buf: vec![Complex::new(0.0, 0.0); fft_size],
            quiet_buf: vec![Complex::new(0.0, 0.0); fft_size],
        }
    }
```

- [ ] **Step 6: Rewrite `SpectralClipper::reset`**

Replace the body of `reset` (keep its doc comment and signature) with:

```rust
    pub fn reset(&mut self) {
        self.stft.reset();
        self.loud_output_ring.fill(0.0);
        self.quiet_output_ring.fill(0.0);
        self.read_pos = 0;
        self.hop_counter = 0;
        for bin in self.loud_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
        for bin in self.quiet_buf.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
    }
```

(`latency_samples` is unchanged — it still returns `self.fft_size`.)

- [ ] **Step 7: Update the input write in `process_sample_inner`**

`process_sample_inner` currently begins:

```rust
        let out_len = self.loud_output_ring.len();

        // Write input into the input ring
        self.input_ring[self.input_pos] = input;
        self.input_pos = (self.input_pos + 1) % self.fft_size;
```

Replace those input-ring lines so it reads:

```rust
        let out_len = self.loud_output_ring.len();

        // Write input into the analyzer's input ring.
        self.stft.write(input);
```

Everything else in `process_sample_inner` (the output-ring read/clear, the clipping, the safety clip, the `read_pos`/`hop_counter` advance, and the `if self.hop_counter >= self.hop_size { ...; if !skip_fft { self.process_frame(); } }` block) is **unchanged**.

- [ ] **Step 8: Rewrite `SpectralClipper::process_frame`**

Replace the whole `fn process_frame(&mut self) { ... }`. Update the doc comment's step 1/2 wording and the body so it uses the analyzer. The new function:

```rust
    /// Loud/quiet split spectral clipper with detail preservation.
    ///
    /// **Step 1+2:** The windowed-frame extract and forward FFT are owned by
    /// the [`StftAnalyzer`]; `analyze()` returns the spectrum (not yet
    /// normalised) and the COLA synthesis window. This frame normalises the
    /// spectrum by `1/N` and caches per-bin magnitudes.
    ///
    /// **Step 3:** Split bins into loud (above threshold) and quiet (below).
    /// Threshold is peak-relative: loud bins are within 6 dB of the frame's
    /// spectral peak; quiet bins are more than 20 dB below the peak.
    ///
    /// **Step 4:** ISTFT both paths separately.
    ///
    /// **Step 5:** Overlap-add each path linearly (with the synthesis window and
    /// COLA normalization) into separate output ring buffers. No nonlinear
    /// processing here — tanh is applied AFTER reconstruction in
    /// `process_sample()` to preserve COLA normalization.
    fn process_frame(&mut self) {
        let n = self.fft_size;
        let out_len = self.loud_output_ring.len();

        // 1+2. Analysis front-end (windowed extract + forward FFT) is owned by
        //      the StftAnalyzer. Normalize the spectrum by 1/N and cache
        //      magnitudes. Using norm_sqr (no sqrt) for threshold comparisons
        //      eliminates ~4096 sqrt calls per frame; sqrt is only computed for
        //      the small number of bins in the transition band.
        let frame = self.stft.analyze();

        let inv_n = 1.0 / n as f32;
        let mut max_mag_sq = 0.0_f32;
        for k in 0..n {
            frame.spectrum[k] *= inv_n;
            let mag_sq = frame.spectrum[k].norm_sqr();
            self.mag_buf[k] = mag_sq;
            if mag_sq > max_mag_sq {
                max_mag_sq = mag_sq;
            }
        }

        // 3. Split bins using peak-relative threshold (squared for comparison).
        //    The threshold adapts to the actual spectral content, ensuring
        //    detail bins go to the quiet path regardless of drive level.
        //    - Above LOUD_RATIO * peak (-6 dB): 100% loud (dominant components)
        //    - Below QUIET_RATIO * peak (-20 dB): 100% quiet (detail)
        //    - Between: smooth crossfade (14 dB transition band)
        let hi_sq = max_mag_sq * (LOUD_RATIO * LOUD_RATIO);
        let lo_sq = max_mag_sq * (QUIET_RATIO * QUIET_RATIO);
        // For the transition band crossfade, we need linear magnitudes.
        // Compute hi/lo from sqrt only once (not per bin).
        let max_mag = max_mag_sq.sqrt();
        let hi = max_mag * LOUD_RATIO;
        let lo = max_mag * QUIET_RATIO;
        let inv_band = if hi > lo { 1.0 / (hi - lo) } else { 1.0 };

        for k in 0..n {
            let mag_sq = self.mag_buf[k];
            if mag_sq >= hi_sq {
                // Clearly loud — 100% to loud path
                self.loud_buf[k] = frame.spectrum[k];
                self.quiet_buf[k] = Complex::new(0.0, 0.0);
            } else if mag_sq <= lo_sq {
                // Clearly quiet — 100% to quiet path (detail preserved)
                self.loud_buf[k] = Complex::new(0.0, 0.0);
                self.quiet_buf[k] = frame.spectrum[k];
            } else {
                // Transition band — smooth crossfade (sqrt only here).
                // quiet = fft - loud avoids a second complex multiply.
                let mag = mag_sq.sqrt();
                let t = (mag - lo) * inv_band; // 0 at lo, 1 at hi
                self.loud_buf[k] = frame.spectrum[k] * t;
                self.quiet_buf[k] = frame.spectrum[k] - self.loud_buf[k];
            }
        }

        // 4. ISTFT both paths.
        // Since we divided by N above, rustfft's unnormalized IFFT
        // produces correctly scaled time-domain signals.
        self.fft_inverse
            .process_with_scratch(&mut self.loud_buf, &mut self.scratch);
        self.fft_inverse
            .process_with_scratch(&mut self.quiet_buf, &mut self.scratch);

        // 5. Overlap-add both paths LINEARLY into their respective output rings.
        //    The synthesis window (= analysis_window / cola_factor) comes from
        //    the analyzer. No nonlinear processing here — tanh is applied
        //    post-reconstruction in process_sample() to preserve COLA
        //    normalization.
        for i in 0..n {
            let out_idx = (self.read_pos + i) % out_len;
            let w = frame.synthesis_window[i];
            self.loud_output_ring[out_idx] += (self.loud_buf[i].re * w) as f64;
            self.quiet_output_ring[out_idx] += (self.quiet_buf[i].re * w) as f64;
        }
    }
```

The `frame` value (borrowing `self.stft`) stays live to the end of the function — it is used again at step 5 for `synthesis_window`. Every other field touched (`mag_buf`, `loud_buf`, `quiet_buf`, `fft_inverse`, `scratch`, `loud_output_ring`, `quiet_output_ring`, `read_pos`) is disjoint from `self.stft`, so the disjoint-field borrow rules permit this.

- [ ] **Step 9: Build, then run the satch suite to verify behaviour is preserved**

Run: `cargo build -p satch`
Expected: compiles with no errors and no dead-code warnings (the old front-end fields are fully removed).

Run: `cargo nextest run -p satch`
Expected: PASS — exactly the same test count as the Step 1 baseline, all passing. If any test fails, the migration changed behaviour — do not adjust the test; find and fix the divergence in the migrated code (the arithmetic must match the original line-for-line).

- [ ] **Step 10: Lint and format**

Run: `cargo clippy -p satch -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --check`
Expected: no diff. (If it reports a diff, run `cargo fmt` and re-check.)

- [ ] **Step 11: Commit**

```bash
git add satch/Cargo.toml satch/src/spectral.rs
git commit -m "refactor(satch): use tract-dsp StftAnalyzer for STFT analysis

SpectralClipper's input ring, analysis window, COLA synthesis window,
and forward FFT move to the shared StftAnalyzer. The loud/quiet split,
the frequency-domain 1/N normalisation, the two inverse FFTs, and the
two overlap-adds are unchanged. Zero behaviour change — satch's full
spectral-clipper suite passes unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Migrate `warp-zone` onto `StftAnalyzer`

`warp-zone`'s `SpectralShifter` drops the analysis-front-end fields (`fft_forward`, `analysis_window`, `synthesis_window`, `input_ring`, `input_pos`, and `fft_buf` — dead once `process_frame` reads the analyzer's spectrum directly) and gains a `StftAnalyzer`. It keeps the synthesis side: `fft_size`, `hop_size`, `fft_inverse`, `scratch` (inverse-FFT-only), `output_ring`, `read_pos`, `hop_counter`, `out_buf`, the phase-vocoder state (`last_input_phase`, `accumulated_output_phase`, `last_output_magnitudes`). The public API (`new`, `latency_samples`, `reset`, `output_magnitudes`, `process_sample`) is unchanged, so `warp-zone/src/lib.rs` is **not** touched.

The one structural change beyond field substitution: `remap_bins` becomes an **associated function** taking explicit slice parameters instead of a `&mut self` method. This is required because `process_frame` must hold the `StftFrame` (a borrow of `self.stft`) live across the call — and you cannot call a `&mut self` method while a borrow of `self` is live. Passing the buffers as explicit slices makes the borrows disjoint and the call legal. This mirrors the existing `StftConvolver::process_frame` associated-function pattern in `tract-dsp/src/stft.rs`. The arithmetic inside `remap_bins` is copied verbatim — only `self.field` becomes a parameter — so it stays bit-identical.

**Files:**
- Modify: `warp-zone/Cargo.toml`
- Modify: `warp-zone/src/spectral.rs`
- Test (safety net, unchanged): `warp-zone/src/spectral.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Establish the green baseline**

Run: `cargo nextest run -p warp-zone`
Expected: PASS — note the test count. This suite (identity passthrough, silence, octave/fractional/extreme shifts, stretch, shift+stretch ordering) **is** the behaviour-preservation test for this task. Do not add or remove tests.

- [ ] **Step 2: Add the feature to `warp-zone/Cargo.toml`**

The dependency line currently reads:

```toml
tract-dsp = { path = "../tract-dsp" }
```

Change it to:

```toml
tract-dsp = { path = "../tract-dsp", features = ["stft-analysis"] }
```

- [ ] **Step 3: Add the import to `warp-zone/src/spectral.rs`**

After the existing `use std::sync::Arc;` line, add:

```rust
use tract_dsp::stft_analysis::StftAnalyzer;
```

(Leave the existing `use rustfft::num_complex::Complex;`, `use rustfft::{Fft, FftPlanner};`, and `use std::f32::consts::{PI, TAU};` — `Complex`, `Fft`, `FftPlanner` are still used by the kept inverse-FFT path, `PI` by `wrap_phase`, `TAU` by `remap_bins`. The `zero()` helper function stays — it is still used by `reset`, `out_buf`, `scratch`, and `remap_bins`.)

- [ ] **Step 4: Replace the `SpectralShifter` struct fields**

Replace the whole `pub struct SpectralShifter { ... }` block with:

```rust
pub struct SpectralShifter {
    fft_size: usize,
    hop_size: usize,

    /// Shared STFT analysis front-end (input ring, analysis window, forward
    /// FFT, COLA synthesis window).
    stft: StftAnalyzer,

    fft_inverse: Arc<dyn Fft<f32>>,
    /// Inverse-FFT in-place scratch.
    scratch: Vec<Complex<f32>>,

    output_ring: Vec<f64>,
    read_pos: usize,
    hop_counter: usize,

    out_buf: Vec<Complex<f32>>,

    last_input_phase: Vec<f32>,
    accumulated_output_phase: Vec<f32>,

    /// Output magnitudes from the most recent frame (for visualization).
    last_output_magnitudes: Vec<f32>,
}
```

- [ ] **Step 5: Rewrite `SpectralShifter::new`**

Replace the body of `new` (keep its signature `pub fn new(fft_size: usize, hop_size: usize) -> Self`) with:

```rust
    pub fn new(fft_size: usize, hop_size: usize) -> Self {
        assert!(fft_size > 0 && hop_size > 0 && fft_size >= hop_size);

        let mut planner = FftPlanner::new();
        let fft_inverse = planner.plan_fft_inverse(fft_size);
        let scratch_len = fft_inverse.get_inplace_scratch_len();

        let out_ring_size = 2 * fft_size;
        let half_plus_one = fft_size / 2 + 1;

        Self {
            fft_size,
            hop_size,
            stft: StftAnalyzer::new(fft_size, hop_size),
            fft_inverse,
            scratch: vec![zero(); scratch_len],
            output_ring: vec![0.0; out_ring_size],
            read_pos: 0,
            hop_counter: 0,
            out_buf: vec![zero(); fft_size],
            last_input_phase: vec![0.0; half_plus_one],
            accumulated_output_phase: vec![0.0; half_plus_one],
            last_output_magnitudes: vec![0.0; half_plus_one],
        }
    }
```

- [ ] **Step 6: Rewrite `SpectralShifter::reset`**

Replace the body of `reset` (keep its signature) with:

```rust
    pub fn reset(&mut self) {
        self.stft.reset();
        self.output_ring.fill(0.0);
        self.read_pos = 0;
        self.hop_counter = 0;
        self.last_input_phase.fill(0.0);
        self.accumulated_output_phase.fill(0.0);
        self.last_output_magnitudes.fill(0.0);
        self.out_buf.fill(zero());
    }
```

(`latency_samples` and `output_magnitudes` are unchanged — `latency_samples` still returns `self.fft_size`, `output_magnitudes` still returns `&self.last_output_magnitudes`.)

- [ ] **Step 7: Update the input write in `process_sample`**

`process_sample` currently begins:

```rust
        let out_len = self.output_ring.len();

        if !freeze {
            self.input_ring[self.input_pos] = input;
            self.input_pos = (self.input_pos + 1) % self.fft_size;
        }
```

Replace the freeze block so it reads:

```rust
        let out_len = self.output_ring.len();

        if !freeze {
            self.stft.write(input);
        }
```

Everything else in `process_sample` (the output-ring read/clear, the `read_pos`/`hop_counter` advance, and the `if self.hop_counter >= self.hop_size { ...; self.process_frame(...); }` block) is **unchanged**.

- [ ] **Step 8: Rewrite `SpectralShifter::process_frame`**

Replace the whole `fn process_frame(&mut self, shift: f32, stretch: f32, low_bin: usize, high_bin: usize) { ... }` with:

```rust
    fn process_frame(&mut self, shift: f32, stretch: f32, low_bin: usize, high_bin: usize) {
        let n = self.fft_size;
        let out_len = self.output_ring.len();

        // Analysis front-end (windowed extract + forward FFT) owned by the
        // StftAnalyzer; `frame` stays live to the overlap-add below, which
        // needs `frame.synthesis_window`.
        let frame = self.stft.analyze();

        // Identity short-circuit: skip phase vocoder when no shift/stretch.
        //
        // Attenuate the identity output by 3 dB to match the RMS level the
        // remap path produces on broadband input. Without this trim,
        // exact-default settings output the input verbatim (full gain),
        // while any non-identity setting loses ~3 dB RMS due to max-wins
        // dropping each target bin's weaker contributor plus the
        // independent-bin phase accumulation breaking the main-lobe
        // vertical phase coherence a windowed sinusoid relies on for its
        // peak sum (the two paths' peaks only differ by ~1.7 dB; RMS
        // differs more because the non-identity output is slightly
        // spikier). Matching RMS rather than peak tracks perceived
        // loudness, which is what makes the moment-to-moment volume feel
        // continuous as the user moves the dial off default. This is a
        // practical calibration against measured program material, not a
        // mathematical derivation.
        const IDENTITY_TRIM: f32 = 0.7079458; // 10^(-3.0 / 20)
        let is_identity = shift.abs() < 1e-6 && (stretch - 1.0).abs() < 1e-6;
        if is_identity {
            for k in 0..n {
                self.out_buf[k] = Complex::new(
                    frame.spectrum[k].re * IDENTITY_TRIM,
                    frame.spectrum[k].im * IDENTITY_TRIM,
                );
            }
        } else {
            Self::remap_bins(
                frame.spectrum,
                &mut self.out_buf,
                &mut self.last_input_phase,
                &mut self.accumulated_output_phase,
                n,
                self.hop_size,
                shift,
                stretch,
                low_bin,
                high_bin,
            );
        }

        // Capture output magnitudes for visualization (before IFFT).
        // Normalize by fft_size/2 so a full-scale sine ≈ 1.0.
        let half = n / 2 + 1;
        let norm_factor = 2.0 / n as f32;
        for k in 0..half {
            self.last_output_magnitudes[k] = self.out_buf[k].norm() * norm_factor;
        }

        // Inverse FFT
        self.fft_inverse
            .process_with_scratch(&mut self.out_buf, &mut self.scratch);

        // Normalize and overlap-add
        let inv_n = 1.0 / n as f32;
        let write_start = self.read_pos;
        for i in 0..n {
            let idx = (write_start + i) % out_len;
            self.output_ring[idx] +=
                (self.out_buf[i].re * inv_n * frame.synthesis_window[i]) as f64;
        }
    }
```

- [ ] **Step 9: Convert `remap_bins` to an associated function**

Replace the whole `fn remap_bins(&mut self, shift: f32, stretch: f32, low_bin: usize, high_bin: usize) { ... }` method. Keep the existing doc comment above it. The new associated function takes the spectrum and the three mutable buffers as explicit slices, plus `fft_size`/`hop_size` as values; the body is the original arithmetic with `self.fft_buf` → `spectrum`, `self.out_buf` → `out_buf`, `self.last_input_phase` → `last_input_phase`, `self.accumulated_output_phase` → `accumulated_output_phase`, `self.fft_size` → `fft_size`, `self.hop_size` → `hop_size`:

```rust
    #[allow(clippy::too_many_arguments)]
    fn remap_bins(
        spectrum: &[Complex<f32>],
        out_buf: &mut [Complex<f32>],
        last_input_phase: &mut [f32],
        accumulated_output_phase: &mut [f32],
        fft_size: usize,
        hop_size: usize,
        shift: f32,
        stretch: f32,
        low_bin: usize,
        high_bin: usize,
    ) {
        let n = fft_size;
        let half_plus_one = n / 2 + 1;

        // Clear output buffer
        for bin in out_buf.iter_mut() {
            *bin = zero();
        }

        let shift_ratio = (shift / 12.0).exp2();
        let lo = low_bin.max(1);
        let hi = high_bin.min(half_plus_one);

        // Pass through bins outside the active range (no shift/stretch)
        for k in 1..lo.min(half_plus_one) {
            out_buf[k] = spectrum[k];
            if k < n / 2 {
                out_buf[n - k] = spectrum[n - k];
            }
            // Keep phase tracking consistent
            last_input_phase[k] = spectrum[k].arg();
            accumulated_output_phase[k] = spectrum[k].arg();
        }
        for k in hi..half_plus_one {
            out_buf[k] = spectrum[k];
            if k < n / 2 {
                out_buf[n - k] = spectrum[n - k];
            }
            last_input_phase[k] = spectrum[k].arg();
            accumulated_output_phase[k] = spectrum[k].arg();
        }

        // Phase 1: Remap bins within the active range.
        // We use out_buf as temporary workspace for in-range bins:
        //   out_buf[k].re = best magnitude so far for target bin k
        //   out_buf[k].im = corresponding phase increment
        let phase_per_bin = TAU * hop_size as f32 / n as f32;

        for k in lo..hi {
            let mag = spectrum[k].norm();
            let phase = spectrum[k].arg();

            // Phase deviation from expected
            let expected_phase_inc = phase_per_bin * k as f32;
            let phase_diff = phase - last_input_phase[k];
            let phase_dev = wrap_phase(phase_diff - expected_phase_inc);

            last_input_phase[k] = phase;

            // Target bin: stretch first, then shift
            let target_f = k as f32 * stretch * shift_ratio;

            // Linear interpolation: distribute magnitude to two adjacent bins
            let target_lo = target_f.floor() as usize;
            let target_hi = target_lo + 1;
            let frac = target_f - target_lo as f32;

            // Phase increment = expected_target + phase_deviation (NOT scaled)
            let phase_inc_lo = phase_per_bin * target_lo as f32 + phase_dev;
            let phase_inc_hi = phase_per_bin * target_hi as f32 + phase_dev;

            // Low bin contribution (max-magnitude-wins)
            if target_lo > 0 && target_lo < half_plus_one {
                let contrib_mag = mag * (1.0 - frac);
                if contrib_mag > out_buf[target_lo].re {
                    out_buf[target_lo] = Complex::new(contrib_mag, phase_inc_lo);
                }
            }

            // High bin contribution (max-magnitude-wins)
            if target_hi > 0 && target_hi < half_plus_one {
                let contrib_mag = mag * frac;
                if contrib_mag > out_buf[target_hi].re {
                    out_buf[target_hi] = Complex::new(contrib_mag, phase_inc_hi);
                }
            }
        }

        // Phase 2: accumulate phases and construct final complex output
        for k in 1..half_plus_one {
            let mag = out_buf[k].re;
            let phase_inc = out_buf[k].im;

            if mag > 0.0 {
                accumulated_output_phase[k] += phase_inc;
                let out_phase = accumulated_output_phase[k];

                let (sin_val, cos_val) = out_phase.sin_cos();
                out_buf[k] = Complex::new(mag * cos_val, mag * sin_val);

                // Mirror for negative frequencies
                if k < n / 2 {
                    out_buf[n - k] = out_buf[k].conj();
                }
            } else {
                out_buf[k] = zero();
                if k < n / 2 {
                    out_buf[n - k] = zero();
                }
            }
        }

        // DC bin: pass through
        out_buf[0] = spectrum[0];
    }
```

- [ ] **Step 10: Build, then run the warp-zone suite to verify behaviour is preserved**

Run: `cargo build -p warp-zone`
Expected: compiles with no errors and no dead-code warnings.

Run: `cargo nextest run -p warp-zone`
Expected: PASS — exactly the same test count as the Step 1 baseline, all passing. If any test fails, the migration changed behaviour — do not adjust the test; find and fix the divergence (the migrated arithmetic must match the original line-for-line).

- [ ] **Step 11: Lint and format**

Run: `cargo clippy -p warp-zone -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --check`
Expected: no diff. (If it reports a diff, run `cargo fmt` and re-check.)

- [ ] **Step 12: Commit**

```bash
git add warp-zone/Cargo.toml warp-zone/src/spectral.rs
git commit -m "refactor(warp-zone): use tract-dsp StftAnalyzer for STFT analysis

SpectralShifter's input ring, analysis window, COLA synthesis window,
and forward FFT move to the shared StftAnalyzer. remap_bins becomes an
associated function taking explicit slices so process_frame can hold
the StftFrame borrow live across the call. The phase vocoder, identity
short-circuit, inverse FFT, and overlap-add are unchanged. Zero
behaviour change — warp-zone's full spectral suite passes unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Workspace-wide verification

No code changes — this task confirms the whole workspace is green with the new feature wired in, and that `tract-dsp` still builds with **no** features (the `stft-analysis` module must be fully gated off).

**Files:** none modified.

- [ ] **Step 1: `tract-dsp` builds with no features**

Run: `cargo build -p tract-dsp`
Expected: PASS — `stft_analysis` is gated out entirely, no `rustfft` pulled, no warnings.

- [ ] **Step 2: `tract-dsp` tests with the feature**

Run: `cargo nextest run -p tract-dsp --features stft-analysis`
Expected: PASS — including the four `stft_analysis` tests.

- [ ] **Step 3: Full workspace build**

Run: `cargo build --workspace`
Expected: PASS — no errors, no warnings.

- [ ] **Step 4: Full workspace test suite**

Run: `cargo nextest run --workspace`
Expected: PASS — every test green. Because `satch` and `warp-zone` enable `stft-analysis` on `tract-dsp`, cargo feature unification builds `tract-dsp` with the feature on for the workspace run, so the `stft_analysis` tests run here too. The count should be the prior workspace total plus the four new `stft_analysis` tests.

- [ ] **Step 5: Lint gate**

Run: `cargo clippy --workspace -- -D warnings`
Expected: PASS — no warnings. (This is the exact gate CI enforces.)

- [ ] **Step 6: Format gate**

Run: `cargo fmt --check`
Expected: no diff.

- [ ] **Step 7: No commit**

This task makes no changes — there is nothing to commit. If any step fails, return to the relevant earlier task and fix it.

---

## Self-Review (completed by plan author)

**Spec coverage** — every section of `docs/superpowers/specs/2026-05-17-tract-dsp-stft-analyzer-design.md` maps to a task:
- "Module: `tract-dsp/src/stft_analysis.rs` (feature `stft-analysis`)", the API (`StftFrame`, `StftAnalyzer`, `new`/`write`/`analyze`/`reset`/`latency_samples`), the `Why a separate feature` rationale → Task 1.
- "Migration → `satch`" → Task 2.
- "Migration → `warp-zone`" → Task 3.
- "Testing → `StftAnalyzer`" (latency, write/analyze DC, reset, synthesis window for 2048/512) → Task 1 Step 3.
- "Testing → Behaviour preservation" (satch/warp-zone suites, workspace gates) → Tasks 2/3 Step 1 baselines + Steps 9–11 / 10–12, and Task 4.
- "Build sequence" 1–4 → Tasks 1–4.

**Spec footnote resolved:** the spec's footnote on whether `satch` keeps a `fft_size` field — `fft_size` **is** kept (used as `n` in `process_frame`, for output-ring sizing, and by `latency_samples`); `latency_samples` continues to return `self.fft_size` rather than delegating, so behaviour is identical. The spec listed `fft_buf` among `satch`'s kept fields; this plan removes it from **both** `satch` and `warp-zone`, because once `process_frame` reads `frame.spectrum` directly the field is dead, and keeping a dead field would draw a clippy warning. This is a refinement of the spec's conservative field list, not a behaviour change.

**Borrow-model decision documented:** the spec's `StftFrame` API is kept as designed. `warp-zone`'s `remap_bins` is converted to an associated function (Task 3 Step 9) so `process_frame` can keep the `StftFrame` borrow live across the call — the spec described `remap_bins` "reading `frame.spectrum`" without specifying this; the conversion is the mechanism that makes it compile, and it mirrors the existing `StftConvolver::process_frame` pattern in `tract-dsp/src/stft.rs`.

**Placeholder scan:** no TBD/TODO/"handle edge cases"/"similar to Task N" — every code step shows complete code.

**Type consistency:** `StftAnalyzer::new(fft_size, hop_size)`, `write(&mut self, f32)`, `analyze(&mut self) -> StftFrame<'_>`, `reset(&mut self)`, `latency_samples(&self) -> usize`, and `StftFrame { spectrum: &mut [Complex<f32>], synthesis_window: &[f32] }` are referenced identically in Tasks 1–3. `Self::remap_bins(spectrum, out_buf, last_input_phase, accumulated_output_phase, fft_size, hop_size, shift, stretch, low_bin, high_bin)` — the call site (Task 3 Step 8) and the definition (Step 9) have matching parameter lists.

---

## Execution Handoff

After all four tasks, dispatch a final code reviewer over the whole branch, then use `superpowers:finishing-a-development-branch` to merge `tract-dsp-stft-analyzer` to `master` locally (the established per-sub-project pattern for this multi-pass refactor).
