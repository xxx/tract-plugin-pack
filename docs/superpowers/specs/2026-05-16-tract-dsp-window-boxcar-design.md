# tract-dsp `window` + `boxcar` Modules — Design

**Status:** approved (brainstorm), ready for implementation planning.

**Goal:** Extract two duplicated DSP primitives into the existing `tract-dsp` crate — the Hann window generator and the O(1) running-sum sliding window — and migrate every consumer. Zero behaviour change: a pure DRY refactor.

**Context:** This is Phase 2a of the multi-pass `tract-dsp` extraction (Pass 1 — `true_peak`/`spsc`/`db` — is merged). Phase 2 was decomposed into four independent sub-projects; this one is the lowest-risk: two small, dependency-free primitives. The `envelope` follower was explicitly scoped *out* — `tinylimit`'s scalar `EnvelopeFilter` and `imagine`'s per-bin spectral smoother are parallel formulations, not copied code, so extracting it is library-building rather than de-duplication.

**Non-goals:** No behaviour change to any plugin. No unification of the two Hann variants (see below). No `envelope` module. No touching the FIR/STFT convolvers or the satch/warp-zone/six-pack/imagine STFT engines (later sub-projects).

---

## Hard constraint: zero behaviour change

Every migrated consumer must produce bit-identical output afterward. The extracted code must be the consumers' existing code, relocated — not "improved." Verification: each plugin's existing test suite stays green, plus `cargo nextest run --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.

---

## Module 1: `tract-dsp/src/window.rs`

### Problem

The Hann window `0.5·(1 − cos(2π·i/D))` is generated inline in **6** places. The denominator `D` differs by call site, in two variants:

- **Periodic** (DFT) Hann, `D = N`: used by the STFT / overlap-add paths — `satch`, `warp-zone`, `miff`, `wavetable-filter`. This is the correct variant for STFT (clean COLA).
- **Symmetric** Hann, `D = N − 1`: used by the one-shot GUI spectrum analyzers — `six-pack`, `imagine`.

The audit flagged the split as "worth a deliberate decision." The decision: **keep both.** Changing a plugin's window denominator shifts its output (sub-bin, but real), which the zero-behaviour-change rule forbids. Unifying them would be a separate, opt-in change — not part of this refactor.

### API

```rust
//! Window functions for spectral analysis.

/// Periodic (DFT) Hann window of `n` samples: `w[i] = 0.5·(1 − cos(2π·i/n))`.
/// The right variant for STFT analysis windows (clean constant-overlap-add).
pub fn hann_periodic(n: usize) -> Vec<f32>

/// Symmetric Hann window of `n` samples: `w[i] = 0.5·(1 − cos(2π·i/(n−1)))`.
/// For one-shot spectral analysis. `n < 2` returns a degenerate window
/// (matches the existing analyzers' behaviour at tiny sizes).
pub fn hann_symmetric(n: usize) -> Vec<f32>
```

Two separately named functions, not a `periodic: bool` parameter — a named call states intent; a bare `true`/`false` does not. `Vec`-returning: every consumer builds its window once at construction (`Default`/`initialize`), never on the audio thread, so a returning allocation is fine.

The exact inline form at each of the 6 sites will be confirmed against the source during planning; each must map to one of these two functions with no numeric change.

### Migrations (6 sites, 6 plugins)

| Consumer | File | Variant |
|----------|------|---------|
| satch | `spectral.rs` | `hann_periodic` |
| warp-zone | `spectral.rs` | `hann_periodic` |
| miff | `convolution.rs` | `hann_periodic` |
| wavetable-filter | `lib.rs` (`stft_window` in `Default`) | `hann_periodic` |
| six-pack | `spectrum.rs` | `hann_symmetric` |
| imagine | `spectrum.rs` | `hann_symmetric` |

Each replaces its inline generation loop with the matching call and adds the `tract-dsp` path dependency (if the crate doesn't already have it). The later FIR/STFT sub-projects inherit the shared `hann` for free.

---

## Module 2: `tract-dsp/src/boxcar.rs`

### Problem

The O(1) running-sum sliding window — evict-oldest / add-newest / keep an `f64` running sum to avoid an O(N) ring rescan — is hand-coded **3 times** in `gs-meter` (and each ring's update logic is itself duplicated across two methods):

- `meter.rs` — `ChannelMeter`'s momentary-RMS ring.
- `lufs.rs` — `LufsMeter`'s momentary (400 ms) ring.
- `lufs.rs` — `LufsMeter`'s short-term (3 s) ring.

### API

```rust
//! Fixed-window running-sum accumulator (boxcar).

/// A sliding window of the last `window` pushed values, maintaining an `f64`
/// running sum so the windowed mean is O(1) per sample and drift-free.
///
/// Generic over the stored element type `T` (`f32` or `f64`); the accumulator
/// is always `f64`. The backing ring is pre-allocated to a fixed maximum
/// capacity at construction — `push`/`set_window`/`reset` never allocate.
pub struct RunningSumWindow<T> { /* ring: Vec<T>, pos, filled, window, sum: f64 */ }

impl<T: Copy + Into<f64>> RunningSumWindow<T> {
    /// Ring pre-allocated to `max_capacity`; logical window set to `window`
    /// (clamped to `[1, max_capacity]`).
    pub fn new(max_capacity: usize, window: usize) -> Self;
    /// Push one value: evict the oldest if the window is full, add the new one.
    pub fn push(&mut self, x: T);
    /// Running sum of the values currently in the window.
    pub fn sum(&self) -> f64;
    /// Number of values currently in the window (`<= window()`).
    pub fn filled(&self) -> usize;
    /// Current logical window size.
    pub fn window(&self) -> usize;
    /// Mean of the values in the window (`0.0` when empty).
    pub fn mean(&self) -> f64;
    /// Change the logical window size without reallocating; resets the window.
    pub fn set_window(&mut self, window: usize);
    /// Clear the window (zero the ring, sum, counters).
    pub fn reset(&mut self);
}
```

### Why generic over `T` — mandatory, not stylistic

`gs-meter` is designed for 100+ instances (CLAUDE.md). `meter.rs`'s ring is `Vec<f32>` — `MAX_WINDOW_SAMPLES` (576 000) × 4 bytes × 2 channels ≈ 4.6 MB/instance. Forcing the ring to `f64` would add ~4.6 MB/instance — ~460 MB across 100 instances. So the element type must stay per-consumer: `meter.rs` uses `RunningSumWindow<f32>`, `lufs.rs` uses `RunningSumWindow<f64>`. The `f64` accumulator is the precision both originals already use, preserved exactly.

`sum()` and `filled()` are part of the public API (not just `mean()`) because `lufs.rs` reads the running sum and fill count directly — it derives 400 ms block energy for the integrated-loudness gating from the momentary ring's sum. `set_window` mirrors `ChannelMeter::set_window_size` (resize without realloc).

### Migrations (3 rings, 1 plugin)

- `gs-meter/src/meter.rs` — `ChannelMeter` replaces its five ring fields (`rms_ring`, `rms_ring_sum`, `rms_window_size`, `rms_ring_pos`, `rms_ring_filled`) with one `RunningSumWindow<f32>`. The duplicated evict/add/wrap logic in both `process_sample` and `process_buffer_channel` collapses to `push`.
- `gs-meter/src/lufs.rs` — `LufsMeter` replaces its momentary ring fields and short-term ring fields with two `RunningSumWindow<f64>`. Call sites that read `momentary_ring_sum`/`momentary_ring_filled` for block energy use `sum()`/`filled()`.

`gs-meter` already depends on `tract-dsp` (Pass 1), so no `Cargo.toml` change is needed there.

---

## Testing

- **`window`:** unit tests for both functions — known values (endpoints, midpoint, symmetry), and a regression check that `hann_periodic(n)` and `hann_symmetric(n)` differ exactly by the denominator. Each migrated plugin's existing tests must stay green.
- **`boxcar`:** unit tests for `RunningSumWindow<f32>` and `<f64>` — fill-up, eviction/wraparound, `set_window` resets, `mean` of a DC sequence, running-sum stability over many cycles (the precision property `gs-meter`'s `test_running_sum_accuracy` already guards), empty-window behaviour, degenerate window of 1.
- **Behaviour preservation:** `gs-meter`'s full `meter.rs` + `lufs.rs` suites must pass unchanged — they already cover RMS, momentary windows, LUFS gating, and running-sum accuracy. Workspace-wide `build` / `nextest` / `clippy -D warnings` / `fmt --check` green.

## Build sequence

1. `window.rs` + tests; declare in `lib.rs`.
2. `boxcar.rs` + tests; declare in `lib.rs`.
3. Migrate the 6 `window` consumers (one commit per plugin, or grouped — each leaves the workspace green).
4. Migrate `gs-meter` `meter.rs` then `lufs.rs` to `boxcar`.
5. Workspace-wide verification.
