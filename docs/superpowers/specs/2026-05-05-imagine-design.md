# Imagine — Design Spec

**Date:** 2026-05-05
**Status:** Approved (brainstorming)
**Plugin:** Imagine — multiband stereo imager modeled on iZotope Ozone Imager (paid module)

## 1. Overview & Scope

Imagine is a multiband stereo imager built into the Tract Plugin Pack. It clones the feature set of the paid Ozone Imager (in Ozone 10/11), with the pack's CPU-rendered softbuffer + tiny-skia GUI and a pink/cyan duo-tone palette.

### Use cases
- Mastering: narrow lows for translation, widen highs for "air," recover lost side energy via Recover Sides.
- Mixing: per-source widening of mono synths, drum overheads, reverbs (via Stereoize); selective narrowing of problem frequency ranges.

### In scope (v1)
- 4 fixed bands with draggable crossover splits
- Per-band Width (−100…+100), Stereoize amount + Mode I/II, Solo
- Global Recover Sides, Link Bands, Quality (Linear / IIR), Bypass
- Polar-sample vectorscope, Lissajous vectorscope, correlation bar, balance bar, crossover spectrum, width spectrum

### Non-goals (v1)
- Dynamic 1–4 band count (always 4 bands; defer)
- Frequency-domain (STFT) processing
- Cross-instance group linking (à la Gain Brain)
- Mono-maker / mono-below-X-Hz (subsumed by setting band 1 Width to −100)
- Output gain (Width preserves energy; Recover Sides keeps narrowing energy-neutral)
- Polar-level vectorscope mode
- Correlation trace over time
- A/B compare snapshots
- Automatic band placement ("Learn")

### Position in the pack
Closest sibling: Six Pack (multiband, draggable per-band controls, Quality switch). Imagine reuses the shared `tiny-skia-widgets` widget kit, the softbuffer + tiny-skia GUI pattern, and the bottom-strip global-controls convention.

## 2. DSP Architecture

### Signal flow

```
L, R input
  │
  ▼  M/S encode:  M = (L+R)/2,  S = (L-R)/2
  │
  ▼  4-band split (one set of crossovers on M, one on S — same filter, so they stay aligned)
  │     Quality = Linear: linear-phase FIR, 4 parallel band-passes summing to identity
  │     Quality = IIR:    cascaded Linkwitz-Riley 24 dB/oct, allpass-summed
  │
  ▼  For each band i ∈ {0..3}:
  │     width_gain[i] = 1 + width_param[i] / 100        // -100 ⇒ mono, 0 ⇒ unity, +100 ⇒ 2×
  │     S_scaled[i]   = S_band[i] * width_gain[i]
  │     S_removed[i]  = S_band[i] - S_scaled[i]         // for Recover Sides
  │
  │     stereoize injection (only if amount[i] > 0):
  │       Mode I  : inject = haas_delay(M_band[i], τ_haas) * amount[i]
  │       Mode II : inject = hilbert_90(M_band[i])       * amount[i]
  │     S_out[i] = S_scaled[i] + inject
  │     M_out[i] = M_band[i]
  │
  ▼  Sum bands:    M_sum = Σ M_out[i]    S_sum = Σ S_out[i]    S_removed_total = Σ S_removed[i]
  │
  ▼  Recover Sides: M_final = M_sum + hilbert_90(S_removed_total) * recover_amount
  │
  ▼  M/S decode:  L = M_final + S_sum,   R = M_final - S_sum
  │
  ▼  Solo override: if any band has Solo on, output only the soloed band's L/R contribution
```

### Latency

- *Quality = Linear*: PDC = max(FIR_crossover_latency, hilbert_90_FIR_latency). FIR designs are length-matched at construction so a single value covers both. Reported via `set_latency_samples`.
- *Quality = IIR*: PDC = 0. IIR Linkwitz-Riley + all-pass-cascade Hilbert are both zero-latency.
- Quality changes during playback re-report PDC dynamically. A brief audio click is expected — the manual recommends choosing Quality before playback starts.

### Load-bearing design choices

- **M/S encode runs once before crossovers.** Crossovers are linear, so per-band M/S encoding is mathematically equivalent to global M/S. Saves duplicated encode/decode work and gives every per-band stage a clean M/S input.
- **Stereoize injects into Side, not into the L/R sum directly.** Width and Stereoize stay in the same domain (Side) and compose cleanly: Width=0 + Stereoize=high → decorrelated content from a mono source; Width=−100 + Stereoize=0 → clean mono.
- **Recover Sides operates on the aggregated removed-side signal, post-band-mix**, not per-band. The Hilbert rotation is on `Σ S_removed[i]`; folding the rotated total into M is what produces the perceptual "energy doesn't leave when you narrow" effect.
- **Hilbert is shared between Stereoize Mode II and Recover Sides** — same impl, called twice with different inputs.
- **Solo is a post-mix gate on the band index**, not a per-band mute earlier in the chain. Keeps the rest of the path unchanged whether Solo is engaged or not.

### Stereoize specifics

- *Mode I (Haas)*: per-band sample-delay line. Default τ ≈ 12 ms (sample-rate adjusted). Delay buffer length sized for the maximum τ at startup. τ is an internal constant for v1 (not a user param).
- *Mode II (Hilbert decorrelator)*: shared `hilbert_90` returns a 90°-shifted copy of the band's M. No additional delay line beyond the Hilbert's own group delay.

## 3. File / Module Layout

```
imagine/
├── Cargo.toml
└── src/
    ├── lib.rs              # Plugin, params, process(), Recover Sides accumulator,
    │                       #   latency reporting, Solo override
    ├── midside.rs          # M/S encode/decode helpers (with SIMD f32x16 paths)
    ├── crossover.rs        # 4-band split: linear-phase FIR + LR-IIR variants,
    │                       #   selected by Quality. One impl per quality, common trait.
    ├── hilbert.rs          # 90° phase rotator: FIR (matched group-delay to FIR crossover)
    │                       #   + IIR all-pass cascade. Shared by Stereoize II and Recover.
    ├── bands.rs            # Per-band state + process(): Width gain, Stereoize Mode I/II,
    │                       #   accumulates S_removed[i] for the global recover path.
    ├── spectrum.rs         # Audio-thread FFT (1024 hop, throttled, atomic bins).
    │                       #   Produces:
    │                       #     - input spectrum (for crossover view backdrop),
    │                       #     - per-band side/(mid+side) ratio for width spectrum.
    ├── vectorscope.rs      # Lock-free ring buffer of (L,R) samples for GUI polar/Lissajous
    │                       #   render, decimated to ~4–8k points/frame.
    ├── theme.rs            # Pink/cyan duo-tone palette + helpers (mirrors pope-scope/theme.rs).
    ├── editor.rs           # softbuffer + baseview lifecycle, hit testing, drag, resize,
    │                       #   layout B coordination.
    └── editor/
        ├── vectorscope_view.rs   # Polar-sample + Lissajous render, mode toggle.
        ├── spectrum_view.rs      # Crossover spectrum (input spectrum + draggable splits)
        │                         #   + width spectrum stacked beneath it.
        ├── band_strip.rs         # Per-band: Width slider, Stereoize knob, Mode I/II,
        │                         #   Solo. Right-click text-entry on continuous controls.
        └── global_strip.rs       # Recover Sides, Link Bands, Quality, Bypass.
```

### Module boundaries

- `crossover.rs` exposes `Crossover::process(&mut self, ms_in: (f32, f32)) -> [(f32, f32); 4]` returning four `(M_band, S_band)` pairs. FIR vs IIR internals are not visible to callers.
- `hilbert.rs` is single-input single-output, with the same trait shape for FIR/IIR; swapping by Quality is a constructor choice, not a runtime branch on the audio path.
- `bands.rs` knows nothing about M/S encoding; it sees per-band M and S inputs only.
- `lib.rs` is the orchestrator: encode → crossover → bands → sum → recover → decode → solo gate.
- Everything in `editor/` reads atomics + param state. No DSP lives there.

### Reused from `tiny-skia-widgets`
- `EditorState`, `SurfaceState`, `DragState`, `TextEditState<A>`
- `draw_dial_ex`, `draw_slider`, `draw_stepped_selector`, button/text helpers
- `fill_pixmap_opaque`, `fill_column_opaque`, `draw_rect`/`draw_rect_opaque`

### Workspace plumbing
- Add `imagine` to root `Cargo.toml` workspace members.
- Embedded font (`fonts/DejaVuSans.ttf`) consistent with the rest of the pack.

## 4. UI & Parameter Set

### Layout (Layout B — vectorscope-left)

```
+-------------------------------+-----------------------------------+
| Vectorscope (~40% width)      |  Crossover spectrum + 3 splits    |
|  - polar sample (default)     |    (input-spectrum backdrop,      |
|    or Lissajous, toggle below |     draggable vertical lines at   |
|  - square aspect, large       |     120 / 1000 / 8000 Hz default) |
|                               |                                   |
|  [polar | lissajous]          |  +--+--+--+--+                    |
|                               |  |B1|B2|B3|B4|  band strip:       |
|  ─── correlation bar ───      |  |W |W |W |W |    Width slider    |
|  ─── balance bar L─R ───      |  |Sz|Sz|Sz|Sz|    Stereoize knob  |
|                               |  |I |I |I |I |    Mode I/II       |
|                               |  |s |s |s |s |    Solo button     |
|                               |  +--+--+--+--+                    |
|                               |                                   |
|                               |  Width spectrum (per-freq width)  |
+-------------------------------+-----------------------------------+
| Recover Sides ◯  ·  Link Bands ☐  ·  Quality [Linear▾]  ·  Bypass ☐ |
+---------------------------------------------------------------------+
```

### Parameters (23 total)

**Per-band (×4):**

| Param | Range | Default | Type |
|---|---|---|---|
| `band{i}_width` | −100…+100 | 0 | continuous, automatable |
| `band{i}_stereoize` | 0…100 | 0 | continuous, automatable |
| `band{i}_stereoize_mode` | {I, II} | I | discrete, automatable |
| `band{i}_solo` | bool | false | discrete, automatable |

**Global:**

| Param | Range | Default | Type |
|---|---|---|---|
| `crossover_1` | 20…20000 Hz (log) | 120 | continuous, automatable |
| `crossover_2` | 20…20000 Hz (log) | 1000 | continuous, automatable |
| `crossover_3` | 20…20000 Hz (log) | 8000 | continuous, automatable |
| `recover_sides` | 0…100 | 0 | continuous, automatable |
| `link_bands` | bool | false | discrete, automatable |
| `quality` | {Linear, IIR} | Linear | discrete, automatable |
| `bypass` | bool | false | discrete, automatable |

### Non-param GUI state (persisted via `EditorState`)
- Window size
- Vectorscope mode (Polar / Lissajous)

### Interaction model

- Right-click on continuous controls → text-entry field (via `TextEditState<A>`).
- Drag splits horizontally on the crossover spectrum to reposition; right-click for text entry.
- Split ordering enforced (split 1 < split 2 < split 3); a drag that would cross is clamped.
- `link_bands = true`: dragging any band's Width slider applies the same delta to all bands' Width. Same for Stereoize amount. Mode I/II and Solo do not link.
- Solo: clicking one band's Solo turns off other bands' Solo (radio behavior). Click the active Solo to deactivate (returns to all-bands).
- Mouse-wheel on dials/sliders: nudge by 1% (default), 0.1% with shift held — mirrors existing pope-scope/six-pack pattern.

### Window behavior
- Minimum size: ~720×580.
- Default size: 960×640.
- Free resize. Scale derived from `physical_width / WINDOW_WIDTH` like the rest of the pack.

### Visual identity
- Pink/cyan duo-tone palette.
- Vectorscope dots and Lissajous trace use pink and cyan to color L-leaning vs R-leaning samples (functional legend at a glance — pink = L-dominant, cyan = R-dominant).
- Width spectrum uses a single duo-tone accent (interpolating pink ↔ cyan as width increases from 0 to 1), so width amount per frequency is readable at a glance. Width is an M/S quantity, so it does not encode L/R balance — that's the balance meter's job.

## 5. Data Flow & Threading

### Audio thread (`process()`)

```
1. Read smoothed params (Width × 4, Stereoize × 4, RecoverSides, CrossoverFreq × 3, Quality, etc.)
2. Per-block: any crossover freq changed?
     - IIR  : recompute LR coefficients in place (cheap arithmetic, no alloc)
     - Linear: redesign 4 FIR lowpass kernels in place into pre-allocated buffers
                (~8k float ops at N=1024; runs once per block at most)
3. For each sample:
     - encode L/R → M/S
     - crossover.process(M, S) → [(M_band, S_band); 4]
     - bands.process(...) → [(M_out, S_out); 4]   (Width, Stereoize, accumulate S_removed)
     - sum bands → (M_sum, S_sum, S_removed_total)
     - apply Recover Sides: M_final = M_sum + hilbert(S_removed_total) * recover
     - decode M/S → L/R
     - solo override (if any band solo'd, replace L/R with that band's contribution only)
4. Per-block update GUI atomics:
     - vectorscope ring buffer: push N (L,R) samples
     - input spectrum FFT (1024-pt, 1024 hop, throttled): write 128 log-spaced bin atomics
     - width spectrum: compute |S_bin| / (|M_bin| + |S_bin|) per bin (bounded [0, 1], non-NaN)
                       → atomic write
     - correlation: Pearson on recent block, single AtomicU32
     - balance: rms(L) vs rms(R) ratio, single AtomicU32
```

### GUI thread (editor render at ~60 fps)

```
1. Tick from baseview frame timer
2. Read pending_resize AtomicU64; if changed, recompute layout
3. Read atomics:
     - input spectrum bins → backdrop of crossover spectrum
     - width spectrum bins → bar plot under spectrum
     - vectorscope ring buffer → decimate to ~4–8k points → render polar or Lissajous
     - correlation → bar position
     - balance → bar position
4. Read params for control rendering (band strips, global strip, splits)
5. Render via tiny-skia + softbuffer present
```

### Lock-free primitives

| Channel | Type | Sizing |
|---|---|---|
| Vectorscope samples | Ring buffer of `(f32, f32)`, single-producer single-consumer | 8192 stereo pairs (~170 ms @ 48 kHz) |
| Spectrum bins (input) | `[AtomicU32; 128]` (f32 bits) | 128 log-spaced bins, range 20 Hz – 20 kHz |
| Width spectrum bins | `[AtomicU32; 128]` | same binning |
| Correlation | `AtomicU32` (f32 bits, range −1…+1) | single value |
| Balance | `AtomicU32` (f32 bits, range −1…+1) | single value |

### No allocations on audio thread
All buffers (FIR taps, FIR delay lines, Hilbert delay lines, Haas delay lines × 4, spectrum FFT scratch, ring buffer storage) are pre-allocated in `initialize()` from the host's max buffer size and sample rate.

### Param smoothing
- Width, Stereoize, Recover Sides, Crossover freqs: linear-smoothed at sample rate (nih-plug smoother).
- Mode I/II, Solo, Link, Quality, Bypass: instantaneous (no smoothing).

### Crossover redesign deferral
Dragging a split fires param changes at GUI rate. The smoother lerps to the target. Per-block we check whether the smoothed value has moved meaningfully; if so, redesign filters. Worst case is one redesign per buffer (~8k ops), comfortably within budget.

### Latency reporting
- `Plugin::initialize()` and on Quality changes, call `set_latency_samples` with the active value (FIR mode) or 0 (IIR mode).
- Quality is automatable but switching modes during playback will cause a click; documented limitation.

## 6. Testing Strategy

Inline `#[cfg(test)]` modules per source file, mirroring the pack convention. Target ~50 tests across the plugin.

### `midside.rs` (~6 tests)
- L/R → M/S → L/R round-trip is identity (per-sample and SIMD path)
- Encode/decode of pure mono (S=0), pure side (M=0), and silence
- SIMD `f32x16` path agrees with scalar within machine epsilon

### `crossover.rs` (~10 tests)
- Linear-phase FIR: sum of all 4 bands ≈ delayed identity
- LR-IIR: sum of all 4 bands is allpass-equivalent to input (magnitude flat, phase rotation only)
- Both: each band's response peaks where expected
- Crossover freq change: redesign in place produces stable output (no allocation, no panic)
- Order constraint: dragging split 2 below split 1 clamps correctly
- Boundary conditions: split exactly at Nyquist or DC doesn't produce NaN

### `hilbert.rs` (~6 tests)
- FIR variant: magnitude ≈ 1.0 across [50 Hz, 18 kHz]
- FIR variant: phase ≈ ±90° across the same range
- IIR all-pass cascade: magnitude ≈ 1.0 ± 0.01
- IIR phase deviation from 90° within ±5° across [50 Hz, 18 kHz]
- Group delay matches reported latency for FIR variant
- Linearity (stability under input scaling)

### `bands.rs` (~10 tests)
- Width=0, Stereoize=0: output equals input
- Width=−100: side channel fully zeroed → output is mono
- Width=+100: side channel doubled
- Stereoize Mode I: delayed mid appears in side at expected delay tap
- Stereoize Mode II: side gains a ~90° rotated copy of mid
- `S_removed[i]` is exactly `S_in - S_scaled` (consistency invariant)
- Solo on a single band: contribution from other bands is zero
- Stereoize amount=0: no injection regardless of mode
- Mode I delay length sample-rate-correct at 44.1 / 48 / 96 / 192 kHz

### `lib.rs::plugin_tests` (~10 tests)
- *Bypass equivalence*: with bypass = true, output equals input bit-for-bit.
- *No-op equivalence*: all bands at Width=0 + Stereoize=0 + Recover=0 produces output equal to input within crossover-summing precision.
- *Energy conservation under narrowing*: total RMS with Width=−100 + Recover=100 ≈ total RMS of input.
- *Solo gating*: soloing one band with all bands at non-zero Width produces output containing only that band's contribution.
- *Latency reporting*: actual measured impulse delay matches `latency_samples()` for both Quality modes.
- *Sample-rate sweep*: 44.1 / 48 / 88.2 / 96 / 176.4 / 192 kHz produce stable, non-NaN output.
- *Quality switch under audio*: switching Quality mid-block doesn't NaN or panic.
- *Crossover drag stress*: rapid crossover automation doesn't allocate, doesn't NaN.
- *Stereoize integration*: Mode I + Mode II selectable per band, both reach output through M/S decode.
- *Link Bands*: enabling link and changing one Width changes all by the same delta.

### `spectrum.rs` (~3 tests)
- Sine input at 1 kHz produces peak at the 1 kHz log bin
- Width spectrum at full mono reads near 0
- Bin count and range stable across sample rates

### `vectorscope.rs` (~3 tests)
- Ring buffer push/pop preserves order
- Decimation produces requested point count
- Wraparound at buffer boundary produces continuous output

### `editor/spectrum_view.rs` (~3 tests)
- Log-frequency mapping x↔Hz round-trips for splits at 20 Hz / 1 kHz / 20 kHz
- Drag handle hit region matches rendered split position
- Render at min and max window size doesn't panic

### `editor/band_strip.rs` (~3 tests)
- Per-band hit regions correct at default and resized layouts
- Solo radio behavior: clicking a second band's solo turns off the first
- Right-click on Width opens text-entry seeded with current value

### Test runner
`cargo nextest run -p imagine`. CI integration follows the existing `.config/nextest.toml`.

### No mocking
All DSP tests use real signals (impulses, sines, noise) and verify against expected mathematical properties.

### Coverage gaps (intentional)
- Visual output (pixel-level rendering correctness) — same as rest of pack.
- Concurrent atomic ordering — `Ordering::Relaxed` semantics; not unit-testable.
- DAW integration — manual smoke test, not a unit test.

## 7. Edge Cases & Caveats

### Documented limitations (not bugs)
- *Switching Quality during playback*: latency changes, brief click expected.
- *Linear-phase pre-ringing*: visible on transients narrowed to mono. Intrinsic; users who care choose IIR.
- *IIR phase rotation*: identical between L and R (so M/S width is unaffected), but absolute phase between bands shifts. Audible primarily on signals spanning band boundaries.
- *Width + Stereoize + Recover Sides combined at extremes*: can produce inter-channel correlation pathologies. User-managed; no auto-attenuation.

### Init / lifecycle
- `Plugin::initialize`: pre-allocate FIR taps, IIR coefficients, FIR delay lines, Hilbert delay lines (FIR), Haas delay lines × 4 (sized for max τ at the highest supported sample rate), spectrum FFT scratch, vectorscope ring buffer.
- Sample rate change: re-derive cutoffs (`cutoff_norm = freq_hz / sample_rate`); IIR coefficients recomputed; latency re-reported.
- Buffer-max-size change: vectorscope ring sized for max conceivable buffer; spectrum FFT input ring sized accordingly.

### Numerical safety
- Crossover frequencies clamped to `[20 Hz, sample_rate / 2 − 100 Hz]` before coefficient design.
- Crossover ordering invariant enforced both at the GUI layer (drag clamping) AND at the DSP layer (sort + minimum-spacing enforcement before filter design).
- IIR LR coefficients verified non-NaN before use; if pathological at corner frequencies, fall back to bypass for that band.

### Concurrency invariants
- Vectorscope ring buffer: SPSC, audio writes, GUI reads. Atomic write index, GUI reads up to `min(write_index, buffer_size)` samples.
- Spectrum atomics: written every N samples (throttled), read at GUI rate. Single-word atomics; non-tearing.
- All audio-thread param reads go through nih-plug's smoothed param API.

### Resize
- `pending_resize: AtomicU64` packs `(width_u32, height_u32)`. Editor consumes on next frame; pixmap and softbuffer are rebuilt at new size.
- Layout coordinates computed proportionally each frame; no cached absolute pixel positions.

### Open questions for the implementation phase
1. **FIR kernel length**: 1024 vs 2048 taps. Tradeoff is latency (~10 ms vs ~21 ms at 48 kHz) vs split sharpness. Prototype both and pick by ear + spectrum sharpness.
2. **Hilbert IIR cascade order**: 4 stages → ~5° max error; 6 stages → ~1°. CPU cost negligible; pick 6.
3. **Haas delay default τ**: 5–25 ms range. 12 ms is a "wide but not detached" starting point. Validate by ear during prototyping.
4. **Default crossover frequencies**: 120 / 1000 / 8000 Hz are mastering-conventional, but the hi-mid/highs split could go higher (e.g. 12 kHz). Decide in prototyping.
