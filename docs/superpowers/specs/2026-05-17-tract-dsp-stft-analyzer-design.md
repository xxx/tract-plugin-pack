# tract-dsp `StftAnalyzer` — Shared STFT Analysis Front-End — Design

**Status:** approved (brainstorm), ready for implementation planning.

**Goal:** Extract the duplicated STFT *analysis* front-end shared by `satch` and `warp-zone` — input ring, hop windowing, forward FFT, and the COLA-derived synthesis window — into a `tract-dsp` module, and migrate both plugins. Zero behaviour change.

**Context:** Phase 2c of the multi-pass `tract-dsp` extraction (Pass 1, Phase 2a, Phase 2b — all merged). The audit flagged `satch`'s `SpectralClipper` and `warp-zone`'s `SpectralShifter` as parallel STFT reimplementations and suggested a shared `StftEngine`. Reading both confirmed: a *full* engine cannot be bit-identical to both — `satch` has **two** synthesis output rings (loud/quiet split, recombined with nonlinear clipping in `process_sample`), `warp-zone` has **one**; and the `1/N` normalisation is applied in the frequency domain by `satch` but the time domain by `warp-zone` (f32 is not associative, so no single engine-level normalisation point is bit-identical to both). What *is* genuinely identical is the **analysis front-end**: the input ring, the windowed extract, the forward FFT, the analysis Hann window, and the verbatim COLA-normalisation block. That is what this sub-project extracts.

**Non-goals:** No behaviour change to `satch` or `warp-zone`. No shared *synthesis* code — the inverse FFT, output ring(s), overlap-add, `1/N` normalisation, and per-bin transform stay in each plugin (that is where they architecturally diverge). No touching the six-pack/imagine spectrum analyzer (the remaining sub-project). No change to `satch`'s loud/quiet detail-preservation or `warp-zone`'s phase vocoder.

---

## Hard constraint: zero behaviour change

`satch` and `warp-zone` must produce bit-identical output. The `StftAnalyzer` reproduces exactly the analysis-side arithmetic both plugins run today; their synthesis halves are not touched. Verification: each plugin's existing test suite stays green (`satch`'s spectral-clipper tests, `warp-zone`'s `spectral` phase-vocoder tests), plus workspace `build` / `nextest` / `clippy -- -D warnings` / `fmt --check`.

---

## Module: `tract-dsp/src/stft_analysis.rs` (feature `stft-analysis`)

### Why a separate feature

`StftAnalyzer` uses `rustfft` (complex transforms). The existing `stft` feature gates `realfft` + `rustfft` for the `stft` module (`StftConvolver`, real transforms). `satch`/`warp-zone` use only `rustfft`. A separate feature keeps them from pulling `realfft`:

```toml
[features]
stft          = ["dep:realfft", "dep:rustfft"]
stft-analysis = ["dep:rustfft"]
```

`tract-dsp/src/lib.rs`: `#[cfg(feature = "stft-analysis")] pub mod stft_analysis;`.

### API

```rust
//! STFT analysis front-end: input ring, hop windowing, forward FFT, COLA window.

/// One analysis frame handed to the caller: the forward-FFT spectrum to
/// transform, plus the COLA-normalised synthesis window for overlap-add.
pub struct StftFrame<'a> {
    /// `fft_size` complex bins — the forward FFT of the latest windowed frame.
    /// The caller transforms this (in place or by reading it) before its IFFT.
    pub spectrum: &'a mut [Complex<f32>],
    /// The COLA-normalised synthesis window (`analysis_window / cola_factor`),
    /// `fft_size` samples — multiply by this during overlap-add.
    pub synthesis_window: &'a [f32],
}

/// Per-channel STFT analysis front-end. Owns the input ring, the periodic-Hann
/// analysis window, the COLA-derived synthesis window, and the forward FFT.
///
/// The caller owns the hop counter and the synthesis half (inverse FFT, output
/// ring(s), overlap-add, normalisation, per-bin transform). It calls `write`
/// each sample, and `analyze` once per hop.
pub struct StftAnalyzer { /* fft_size, fft_forward, scratch, analysis_window,
                             synthesis_window, input_ring, input_pos, fft_buf */ }

impl StftAnalyzer {
    /// `fft_size`-point analysis; `hop_size` is used only to compute the COLA
    /// synthesis window. `fft_size` must be a power of two; `fft_size >= hop_size`.
    pub fn new(fft_size: usize, hop_size: usize) -> Self;
    /// Write one input sample into the ring and advance. Skip this call to
    /// hold the ring frozen (e.g. `warp-zone`'s freeze).
    pub fn write(&mut self, input: f32);
    /// Extract the latest `fft_size` samples (oldest-first, Hann-windowed) and
    /// forward-FFT them; return the spectrum + synthesis window. The caller
    /// invokes this once per hop. Skip it to suppress frame work (e.g.
    /// `satch`'s `skip_fft`).
    pub fn analyze(&mut self) -> StftFrame<'_>;
    /// Zero the input ring and position.
    pub fn reset(&mut self);
    /// Inherent latency in samples (`= fft_size`).
    pub fn latency_samples(&self) -> usize;
}
```

Two methods, no modal flags. The **caller keeps its own hop counter** and decides per sample whether to `write` (skipped for `warp-zone` freeze) and whether to `analyze` (skipped for `satch` `skip_fft`) — the modes fall out of which methods the plugin calls, with no bool parameters. `StftFrame` bundles the spectrum and synthesis window so a single borrow covers a plugin's whole frame block (the spectrum and synthesis window are disjoint `StftAnalyzer` fields).

`analyze`'s windowed extract is `fft_buf[i] = Complex::new(input_ring[(input_pos + i) % fft_size] * analysis_window[i], 0.0)` followed by `fft_forward.process_with_scratch(...)` — character-identical to what both plugins do today. The analysis window is `tract_dsp::window::hann_periodic(fft_size)` (both plugins already use it since Phase 2a). The COLA block (`num_frames = fft_size/hop_size`; per-`hop_size` squared-window accumulation; `cola_factor`; `synthesis_window = analysis_window * (1/cola_factor)`) is the verbatim block both plugins share.

---

## Migration

### `satch` (`satch/src/spectral.rs`)

`SpectralClipper` drops the fields now owned by the analyzer (`fft_size`*, `fft_forward`, `scratch`, `analysis_window`, `synthesis_window`, `input_ring`, `input_pos`) and gains a `StftAnalyzer`. It keeps: `hop_size`, its own `hop_counter`, `mag_buf`, the two output rings (`loud_output_ring`, `quiet_output_ring`), `read_pos`, the inverse FFT plan + its scratch, and `fft_buf`/`loud_buf`/`quiet_buf`.

`process_sample_inner`: `self.stft.write(input)` replaces the input-ring write; the output-ring read/clear, the clipping, and `read_pos`/`hop_counter` advance stay; on a hop, `if !skip_fft { let frame = self.stft.analyze(); … }` — `satch`'s loud/quiet split (now reading `frame.spectrum`, still doing its frequency-domain `*= 1/N`), the two inverse FFTs, and the two overlap-adds (using `frame.synthesis_window`) are unchanged. `skip_fft` simply means `analyze` is not called.

\* `fft_size` may be kept as a `SpectralClipper` field if still referenced (e.g. by `loud_buf` sizing / `process_frame`); the analyzer also exposes `latency_samples()`. The plan settles this against the source.

### `warp-zone` (`warp-zone/src/spectral.rs`)

`SpectralShifter` drops the analyzer-owned fields and gains a `StftAnalyzer`. It keeps: `hop_size`, `hop_counter`, the single `output_ring`, `read_pos`, the inverse FFT plan + scratch, `fft_buf`/`out_buf`, the phase-vocoder state (`last_input_phase`, `accumulated_output_phase`, `last_output_magnitudes`), `freeze`, and `output_magnitudes()`.

`process_sample`: `if !freeze { self.stft.write(input) }`; the output-ring read/clear and `read_pos`/`hop_counter` advance stay; on a hop, `let frame = self.stft.analyze();` then the identity-trim / `remap_bins` per-bin transform (reading `frame.spectrum`), the inverse FFT, and the overlap-add (`frame.synthesis_window`, time-domain `1/N`) are unchanged. Freeze simply means `write` is not called.

`satch` and `warp-zone` already depend on `tract-dsp` (since Phase 2a); each gains `features = ["stft-analysis"]`.

---

## Testing

- **`StftAnalyzer`:** unit tests — `latency_samples` = `fft_size`; `write`/`analyze` over a steady DC input produce the expected windowed spectrum (DC bin dominant); `reset` clears the ring; the synthesis window equals `analysis_window / cola_factor` for a known `fft_size`/`hop_size` (e.g. 2048/512 — Hann 75 % overlap → `cola_factor` 1.5). Tests run under `--features stft-analysis`.
- **Behaviour preservation:** `satch`'s full spectral-clipper suite and `warp-zone`'s full `spectral` suite must pass unchanged — they cover STFT reconstruction, the loud/quiet split, the phase vocoder, identity short-circuit, shift/stretch. Workspace `build` / `nextest` / `clippy --workspace -- -D warnings` / `fmt --check` green.

## Build sequence

1. `stft_analysis.rs` (`StftAnalyzer` + `StftFrame`) + tests; add the `stft-analysis` feature + module declaration.
2. Migrate `satch` — `SpectralClipper`'s analysis front-end → `StftAnalyzer`; add `features = ["stft-analysis"]`.
3. Migrate `warp-zone` — `SpectralShifter`'s analysis front-end → `StftAnalyzer`; add `features = ["stft-analysis"]`.
4. Workspace-wide verification.
