# tract-dsp `fir` + `stft` Convolver Modules — Design

**Status:** approved (brainstorm), ready for implementation planning.

**Goal:** Extract the two duplicated convolution engines — the time-domain SIMD FIR ring and the magnitude-only STFT convolver — into the `tract-dsp` crate, migrate `miff`, and carve the equivalent DSP out of `wavetable-filter`'s `lib.rs`. Zero behaviour change: a pure DRY refactor.

**Context:** Phase 2b of the multi-pass `tract-dsp` extraction (Pass 1 — `true_peak`/`spsc`/`db`; Phase 2a — `window`/`boxcar` — both merged). The audit found `miff/src/convolution.rs` is a self-admitted near-copy of `wavetable-filter`'s convolution DSP: `RawChannel` ≈ `FilterState`, `PhaselessChannel` ≈ `process_stft_frame`. `miff`'s `convolution.rs` is clean and modular; `wavetable-filter`'s equivalent is embedded in a 2933-line `lib.rs`.

**Non-goals:** No behaviour change to any plugin. No reorganisation of `wavetable-filter/src/lib.rs` beyond removing the carved-out DSP (the user chose the minimal carve-out). No extraction of `miff`'s `Kernel` type or the kernel-bake/normalisation (separate concern). No touching the satch/warp-zone STFT engine or the six-pack/imagine spectrum analyzer (later sub-projects).

---

## Hard constraint: zero behaviour change

Every migrated consumer must produce bit-identical output. The shared engines use the exact arithmetic of the code they replace. Verification: each plugin's existing test suite stays green (`miff`'s `RawChannel`/`PhaselessChannel` tests, `wavetable-filter`'s `test_stft_*` / `test_frame_sweep_regression` / Raw-convolution tests), plus workspace `build`/`nextest`/`clippy -D warnings`/`fmt --check`.

---

## Key design decision: ring + MAC, not a high-level `process()`

`wavetable-filter`'s Raw path is not a single-kernel convolution. It crossfades two kernels — `synthesized_kernel` and `crossfade_target_kernel` — MACing the *same* history window against both and blending by `crossfade_alpha`. A high-level `process(sample, kernel)` that pushes the sample internally cannot serve that: it would push twice per sample.

So the shared FIR unit separates **`push`** (advance the history ring by one sample) from **`mac`** (dot-product the current window against a given reversed-tap slice). `miff`'s `RawChannel` composes them once per sample; `wavetable-filter` pushes once and MACs twice. (Alternative considered and rejected: a high-level `process(sample, &[f32])` — it cannot express the crossfade without a separate push.)

---

## Module 1: `tract-dsp/src/fir.rs`

Pure `std` + `std::simd`. No cargo feature — always compiled, zero dependencies.

### Problem

`miff`'s `RawChannel` (`convolution.rs`) and `wavetable-filter`'s `FilterState` (`lib.rs`) are a near-copy: a double-buffered history ring (`2 × cap` so a contiguous `len`-sized window is always readable for zero-modulo SIMD), `next_power_of_two` capacity sizing, an `is_silent` fast-path (set false when `|sample| > 1e-6`, re-armed on `reset`), and an `f32x16` MAC over a pre-reversed kernel. `miff`'s module header explicitly says it was "adapted from wavetable-filter's `FilterState`."

### API

```rust
//! Time-domain FIR convolution: a double-buffered history ring + SIMD MAC.

pub struct FirRing { /* history: Vec<f32> (2*cap), write_pos, mask, is_silent */ }

impl FirRing {
    /// A ring sized for kernels up to `max_len` taps (capacity rounded up to a
    /// power of two).
    pub fn new(max_len: usize) -> Self;
    /// Zero the history; re-arm the silence flag.
    pub fn reset(&mut self);
    /// Push one input sample (double-buffered write). Clears the silence flag
    /// when `sample.abs() > 1e-6`.
    pub fn push(&mut self, sample: f32);
    /// `true` iff only (near-)zero samples have been pushed since the last
    /// `reset` — the MAC output is then guaranteed zero and may be skipped.
    pub fn is_silent(&self) -> bool;
    /// `f32x16` multiply-accumulate of the most-recent `rev_taps.len()` samples
    /// against `rev_taps` (the kernel pre-reversed; length a multiple of 16).
    pub fn mac(&self, rev_taps: &[f32]) -> f32;
}
```

`mac` takes a plain `&[f32]` — **no coupling to `miff`'s `Kernel`**. The tap count is `rev_taps.len()`.

### Migrations

- `miff/src/convolution.rs`: `RawChannel` keeps its name and `process(sample, kernel: &Kernel)` signature but is reimplemented over a `FirRing` — `push`, then the `is_zero` / `is_silent` short-circuits, then `mac(&kernel.rev_taps[..kernel.len])`.
- `wavetable-filter/src/lib.rs`: the `FilterState` struct and its `impl` are removed; `WavetableFilter::filter_state` becomes `[FirRing; 2]`. `process()`'s Raw branch calls `push` once, then `mac` against `synthesized_kernel` (and `crossfade_target_kernel` when crossfading), keeping the existing `is_silent` skip and the blend math in place.

---

## Module 2: `tract-dsp/src/stft.rs`

Behind a `stft` cargo feature (it needs `realfft`/`rustfft`).

### Problem

`miff`'s `PhaselessChannel` (`convolution.rs`) is a self-admitted reproduction of `wavetable-filter`'s `process_stft_frame` + STFT state. Both: a fixed-size Hann-windowed STFT — copy the latest `frame` samples oldest-first with a Hann window, forward real FFT, multiply each bin by a real magnitude gain (phase preserved), inverse real FFT, overlap-add at 50% with `1/frame` normalisation; output delayed by `hop = frame/2`.

### API

```rust
//! Magnitude-only STFT convolution: fixed-frame Hann-windowed overlap-add.

pub struct StftConvolver { /* fft/ifft plans, window, in/out bufs + positions,
                              fft scratch, frame, hop */ }

impl StftConvolver {
    /// A convolver with a fixed `frame`-point transform and `hop = frame/2`
    /// (50% overlap). The analysis window is `window::hann_periodic(frame)`.
    pub fn new(frame: usize) -> Self;
    /// Zero all state.
    pub fn reset(&mut self);
    /// Inherent latency in samples (`= hop`).
    pub fn latency(&self) -> usize;
    /// Process one sample. `mags` is the per-bin magnitude gain
    /// (`frame/2 + 1` bins). `apply == false` skips the per-bin multiply,
    /// giving a delayed dry passthrough (identity). Output is delayed by `hop`.
    pub fn process(&mut self, sample: f32, mags: &[f32], apply: bool) -> f32;
}
```

The analysis window reuses `tract_dsp::window::hann_periodic` (Phase 2a). The `apply` flag covers `miff`'s zero-kernel identity case (`apply = !kernel.is_zero`); `wavetable-filter` always passes `apply = true`. `frame` is a parameter — `miff` uses 4096, `wavetable-filter` uses 2048.

### Cargo feature

`tract-dsp/Cargo.toml`:

```toml
[features]
stft = ["dep:realfft", "dep:rustfft"]

[dependencies]
realfft = { version = "3.3", optional = true }
rustfft = { version = "6.2", optional = true }
```

`tract-dsp` stays zero-dependency by default. `fir` is unconditional. `stft` (and its `realfft`/`rustfft` cost) is opt-in: only `miff` and `wavetable-filter` enable it (`tract-dsp = { path = "../tract-dsp", features = ["stft"] }`). The other consumers (gs-meter, tinylimit, imagine) keep the plain dependency and never pull the FFT crates.

### Migrations

- `miff/src/convolution.rs`: `PhaselessChannel` keeps its name and `process(sample, kernel: &Kernel)` signature, reimplemented over a `StftConvolver` (`apply = !kernel.is_zero`, `mags = &kernel.mags`). `miff`'s `tract-dsp` dependency gains `features = ["stft"]`.
- `wavetable-filter/src/lib.rs`: the STFT state fields and the `process_stft_frame` associated fn are removed; the plugin holds `[StftConvolver; 2]`. `process()`'s STFT hop loop calls `convolver.process(sample, &stft_magnitudes, true)`. The reported STFT latency comes from `StftConvolver::latency()`. `wavetable-filter`'s `tract-dsp` dependency gains `features = ["stft"]`.

---

## Testing

- **`fir`:** unit tests for `FirRing` — impulse/known-kernel MAC results, the silence fast-path, `reset` re-arming silence, double-buffer wraparound. (Port `miff`'s `RawChannel` test cases.)
- **`stft`:** unit tests for `StftConvolver` — zero/identity passthrough (`apply = false`), flat-magnitude energy preservation, fixed `hop` latency. (Port `miff`'s `PhaselessChannel` test cases.) `stft` tests run under `--features stft`.
- **Behaviour preservation:** `miff`'s full suite and `wavetable-filter`'s full suite (Raw convolution, `test_stft_lowpass_attenuates_highs`, `test_stft_flat_preserves_amplitude`, `test_stft_magnitudes_*`, `test_frame_sweep_regression`, the bench tests) must pass unchanged. Workspace `build` / `nextest` / `clippy --workspace -- -D warnings` / `fmt --check` green.

## Build sequence

1. `fir.rs` + tests; declare in `lib.rs`.
2. `stft.rs` + tests behind the `stft` feature; add the feature + optional deps to `Cargo.toml`; declare in `lib.rs`.
3. Migrate `miff` — `RawChannel` then `PhaselessChannel` become wrappers; add `features = ["stft"]`.
4. Carve `wavetable-filter`'s Raw path — `FilterState` → `FirRing`.
5. Carve `wavetable-filter`'s STFT path — STFT state + `process_stft_frame` → `StftConvolver`; add `features = ["stft"]`.
6. Workspace-wide verification.
