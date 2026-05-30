# Nap — Dual Engine (Zero Latency / Efficient FFT) — Design

**Date:** 2026-05-29
**Crate:** `nap` (+ a new reusable primitive in `tract-dsp`)
**Status:** approved 2026-05-29

## Goal

Give Nap a user-selectable convolution engine so users can trade latency for CPU:

- **Zero Latency** — the current time-domain sparse velvet engine (`O(pulse-count)`, no added latency). Default.
- **Efficient** — uniformly-partitioned FFT convolution of the baked impulse response (`O(IR-length)`, density-independent, ~512-sample / ~10.7 ms-at-48 kHz latency).

Efficient is several× cheaper at large/dense settings (where the time-domain engine is at its irreducible `O(pulse-count)` FMA floor, per the perf-investigation commits — `perf(nap): block + SIMD …`, `… fuse the multiply-add`, `… idle fast-path`), and roughly break-even at the default. The choice is a pure CPU↔latency knob.

## Background — the load-bearing invariant

Nap's entire wet path — the sparse signed velvet pulse train, the `Q=6` coloration one-poles (each fed its routed pulses), the post-LP, and the DC blocker — is a single **linear time-invariant** system. Therefore it equals convolution of the input with one dense impulse response `h[n]`. The two engines compute the *same* `h`:

- Zero Latency: the existing sparse direct convolution + filter bank (`ReverbChannel`).
- Efficient: bake `h`, then FFT-convolve.

So **the two modes are sonically identical** (within fp + IR truncation), differing only by Efficient's latency. This is both the feature's appeal and its primary correctness gate (a test asserts the two outputs match, time-aligned).

The perf investigation (5-agent fan-out + `perf stat` + Välimäki 2024) established that the time-domain engine is at its exact, single-threaded, no-FFT ceiling; FFT is the only way to materially cut the active-path cost, and it necessarily adds latency — hence a user choice rather than a silent switch.

## Non-goals (v1)

- **No zero-latency partitioned (Gardner) scheme.** Efficient uses simple uniform partitioning with `P`-sample latency; we are not building the direct-head + growing-FFT-partition cascade. Latency is the accepted tradeoff.
- **No mode-switch crossfade.** Switching engines resets the FFT convolver + dry-delay → a one-time, user-initiated discontinuity. Acceptable.
- **No change to the Zero Latency engine's DSP.** It stays exactly as shipped (including the idle fast-path).

## Components

### `tract_dsp::partitioned_conv::PartitionedConvolver` (new, reusable)
A uniformly-partitioned overlap-save (UPOLS) real convolver, feature-gated behind a `realfft`-pulling feature like the other FFT modules in `tract-dsp`.

- Fixed partition `P = 512`, FFT size `N = 1024` (= 2·P, for linear convolution via overlap-save).
- Holds `K` precomputed IR-partition spectra (`H_0..H_{K-1}`, each `N/2+1` complex bins), a frequency-domain delay line of the last `K` input-block spectra, scratch for the forward/inverse real FFTs, and the time-domain input history. All buffers pre-allocated to a max `K`.
- `set_ir_spectra(&[Complex<f32>], k)` — install the baked partition spectra (audio-thread-safe swap; no realloc).
- `process_block(input: &[f32; P], output: &mut [f32; P])` — one partition's worth; introduces `P`-sample latency. No allocation.
- `reset()` — clear the FDL + history (used on mode switch / `reset`).
- **Correctness gate:** UPOLS output equals direct time-domain convolution of the input stream with the materialized IR (golden test), including block-boundary/overlap behaviour.

### `nap/src/ir.rs` (new) — analytic IR bake (GUI thread)
Builds the dense IR `h[0..L]` from the current pulses + curves in `O(L)`, then its partition spectra:

1. `L = (tail_len + SETTLE_SAMPLES)` (the same `SETTLE_SAMPLES` margin the idle fast-path uses guarantees the truncated IIR tails are ≤ `SILENCE_EPS`), capped at the max (`10 s + settle`).
2. For each filter `q`: scatter `coeff[m]` at `location[m]` for that filter's pulses into a sparse buffer, run that coloration one-pole over `[0..L]`.
3. Sum the `Q` filtered buffers → run the post-LP → run the DC blocker → `h`. (Identical filter coefficients and order to `ReverbChannel`, so `h` is exactly the engine's impulse response.)
4. Partition `h` into `K = ceil(L/P)` blocks, zero-pad each to `N`, forward-FFT → the `K` spectra.

Baked **per channel** (L uses `location`, R uses `location_r` → two IRs). Runs only on the GUI/setup thread.

### `nap/src/handoff.rs` — `IrHandoff` (extend)
Mirror `SequenceHandoff`: `Mutex` + generation counter + non-blocking `try_read_into` that copies the freshly-baked partition spectra (both channels) into the audio thread's pre-allocated convolvers only when the generation changed. No audio-thread allocation.

### `nap/src/lib.rs` — mode + routing
- New `mode: EnumParam<NapMode { ZeroLatency, Efficient }>`, **non-automatable** (latency-affecting, GUI-set — like miff's `Length`).
- Per-channel `PartitionedConvolver` (L/R) + a `P`-sample dry-delay line per channel (for Efficient alignment).
- `process()` reads `mode`:
  - **ZeroLatency:** the existing `ReverbChannel::process_block` path, unchanged. Reported latency 0.
  - **Efficient:** feed each channel's gained dry into its `PartitionedConvolver` (in `P`-sized sub-blocks), delay the dry path by `P` so it aligns with the inherently-`P`-late wet, then the same per-sample pre-delay + dry/wet mix + output gain. Reported latency `P`.
- **Latency reporting:** `process()`/`initialize()` sets `context.set_latency_samples(if Efficient { P } else { 0 })`, updating it when `mode` changes (the switch is infrequent and user-initiated; hosts re-scan).
- **Regeneration (lazy + drag-deferred):** on a design-param/curve edit the editor regenerates the `VelvetSequence` always (sub-ms, drives Zero-Latency audio + the tail visualization live). The IR is baked only when `mode == Efficient` (switching to Efficient bakes it if stale), so Zero-Latency editing never pays the bake cost. The IR-bake cost scales with **Size** (partitions `K = tail_len/P`), not density or curve detail: ~1–2 ms at default Size, ~5–10 ms at max Size. To keep dragging smooth in Efficient mode at large Size, the IR bake is **deferred during a continuous node drag and run on drag-release** (mouse-up); discrete edits (add/delete node, dial change) bake immediately. So during a large-Size drag the curve + tail viz update live while the audio refreshes when the drag ends.
- On mode switch: `reset()` the convolvers + dry-delay (clean start in the new engine).

### `nap/src/editor.rs` — mode selector
A stepped Zero Latency / Efficient selector (shared `tiny_skia_widgets` stepped-selector) in the bottom strip. Selecting Efficient triggers the (possibly-deferred) IR bake.

## Latency & alignment

Efficient's wet is `P` samples late (UPOLS). To keep dry+wet phase-aligned at the output, the dry is delayed by `P` inside the plugin, and the plugin reports `P` latency so the host compensates globally. Pre-delay (a wet-only musical delay) composes on top. Zero Latency reports 0 and delays nothing.

## Testing

- **Mode equivalence (headline):** for a non-trivial sequence + random input, Efficient output (shifted back by `P`) matches Zero Latency output within tolerance. Proves "same sound."
- **IR-bake correctness:** the analytic `h` matches the impulse response obtained by feeding an impulse through a real `ReverbChannel` (small settings, where that's cheap) within tolerance.
- **`PartitionedConvolver`:** UPOLS output == direct convolution against a known IR (golden); impulse-in → IR-out; overlap/boundary correctness across many blocks; `reset` clears state.
- **No-alloc:** both audio paths and the handoff allocate nothing (covered by `assert_process_allocs`); mode-switch resets cleanly.
- **Latency:** Efficient reports `P`; Zero Latency reports 0; the dry-delay aligns dry/wet (verified via the equivalence test's alignment).

## Risks / notes

- **IR-bake cost** scales with Size: ~1–2 ms (default) to ~5–10 ms at max (`L ≈ 488 k`, `K ≈ 954` FFTs), GUI-thread only. Kept smooth by the lazy + drag-deferred regeneration (above): Zero-Latency editing never bakes, and Efficient defers the bake to drag-release at large Size.
- **Memory:** ~8 MB of partition spectra (both channels, max `K`). Fine for a feature-tier plugin.
- **Mode-switch transient:** one-time discontinuity on switch (engines reset). Acceptable; no crossfade in v1.
- **Reusable primitive placement:** `PartitionedConvolver` lives in `tract-dsp` (GUI-free, feature-gated) per the pack's shared-DSP philosophy; it's generically useful (any plugin needing zero-config FFT convolution).
