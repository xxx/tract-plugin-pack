# Imagine — Design Spec

**Date:** 2026-05-05 (revised post-review)
**Status:** Approved (brainstorming) — revised after convention + adversarial review
**Plugin:** Imagine — multiband stereo imager modeled on iZotope Ozone Imager (paid module)

**Revision notes (2026-05-05):**
- Width law switched from naive `1 + width/100` to constant-power equal-power M/S pan law.
- Recover Sides reframed as a perceptual phase-residue control, gated per-band by sign of Width.
- Stereoize Mode II changed from a Hilbert-90 phase rotator to a Schroeder/Gerzon all-pass decorrelator (genuine decorrelation, not phase shift).
- Plugin-defined `bypass` param dropped; host bypass via `BYPASS_BUFFER_COPY`.
- Quality made non-automatable (matches Six Pack's precedent).
- 4-band IIR LR explicitly uses Lipshitz/Vanderkooy delay-matched compensation for true allpass sum.
- Width-spectrum metric replaced with magnitude-squared coherence (renamed "coherence spectrum").
- Single complex `M + jS` FFT replaces two separate FFTs for the spectrum analyzer.
- Coherence computed audio-side and published as a single AtomicU32 per bin (no cross-bin race).
- FIR crossover redesign now uses double-buffered tap arrays + sample-wise crossfade.
- Vectorscope ring buffer SPSC ordering specified explicitly (Release/Acquire on `write_pos`).
- Vectorscope ring sized for max sample rate × max-buffers-per-frame (32k pairs).
- Solo + Recover Sides interaction explicitly resolved (Solo bypasses the Recover path).
- Link Bands clamping rule explicitly defined (delta clamped to least available headroom).
- Pack plumbing items added: `assert_process_allocs`, `ProcessStatus::Tail`, `EnumParam` declarations, manual file.

## 1. Overview & Scope

Imagine is a multiband stereo imager built into the Tract Plugin Pack. It clones the feature set of the paid Ozone Imager (in Ozone 10/11), with the pack's CPU-rendered softbuffer + tiny-skia GUI and a pink/cyan duo-tone palette.

### Use cases
- Mastering: narrow lows for translation, widen highs for "air," recover lost side energy via Recover Sides.
- Mixing: per-source widening of mono synths, drum overheads, reverbs (via Stereoize); selective narrowing of problem frequency ranges.

### In scope (v1)
- 4 fixed bands with draggable crossover splits
- Per-band Width (constant-power M/S law, −100…+100), Stereoize amount + Mode I/II, Solo
- Global Recover Sides (gated, perceptual phase residue), Link Bands, Quality (Linear / IIR — non-automatable)
- Polar-sample vectorscope, Lissajous vectorscope, correlation bar, balance bar, crossover spectrum, width spectrum
- Host-managed bypass via nih-plug's `BYPASS_BUFFER_COPY` (no plugin-defined `bypass` param)

### Non-goals (v1)
- Dynamic 1–4 band count (always 4 bands; defer)
- Frequency-domain (STFT) processing
- Cross-instance group linking (à la Gain Brain)
- Mono-maker / mono-below-X-Hz (subsumed by setting band 1 Width to −100)
- Output gain (constant-power Width law preserves total energy without explicit make-up)
- Plugin-defined `bypass` param (use host bypass)
- Polar-level vectorscope mode
- Correlation trace over time
- A/B compare snapshots
- Automatic band placement ("Learn")
- 1-band CPU-saving preset (candidate for v2 if users want a single-knob mono-maker without 4-band overhead)

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
  │     // Constant-power Width: equal-power M/S pan law, unity at width=0,
  │     // ±3 dB compensation on the unmuted channel at extremes.
  │     θ[i]       = (width_param[i] + 100) / 200 · π            // θ ∈ [0, π]
  │     M_gain[i]  = √2 · cos(θ[i] / 2)                          // [√2, 1, 0]   at [-100, 0, +100]
  │     S_gain[i]  = √2 · sin(θ[i] / 2)                          // [0,  1, √2]  at [-100, 0, +100]
  │     M_scaled[i] = M_band[i] · M_gain[i]
  │     S_scaled[i] = S_band[i] · S_gain[i]
  │
  │     // Recover Sides accumulator: only acts when narrowing (gated by sign of width).
  │     // The unmuted-channel boost from constant-power Width already preserves total energy;
  │     // Recover Sides folds a phase-rotated residue of the lost side into mid for
  │     // *perceptual* width retention, not energy compensation.
  │     S_removed[i] = if width_param[i] < 0 { S_band[i] · (1 − S_gain[i]) } else { 0 }
  │
  │     // Stereoize injection (only if amount[i] > 0):
  │     //   Mode I  : Haas — delayed mid into side. Widens mono sources; combs stereo sources.
  │     //   Mode II : Decorrelator — Schroeder/Gerzon all-pass cascade with prime-spaced delays.
  │     //             Genuinely lowers L/R correlation; mono-compatible.
  │     Mode I  : inject = haas_delay(M_band[i], τ_haas)    · amount[i]
  │     Mode II : inject = decorrelate(M_band[i])            · amount[i]
  │     S_out[i] = S_scaled[i] + inject
  │     M_out[i] = M_scaled[i]
  │
  ▼  Sum bands:    M_sum = Σ M_out[i]    S_sum = Σ S_out[i]    S_removed_total = Σ S_removed[i]
  │
  ▼  Recover Sides (perceptual residue): M_final = M_sum + hilbert_90(S_removed_total) · recover_amount
  │
  ▼  M/S decode:  L = M_final + S_sum,   R = M_final − S_sum
  │
  ▼  Solo override: if any band has Solo on, M_final ← M_out[soloed], S_sum ← S_out[soloed]
  │                  (i.e., decode only the soloed band; Recover Sides path is bypassed)
```

### Latency

- *Quality = Linear*: PDC = max(FIR_crossover_latency, hilbert_90_FIR_latency, decorrelator_FIR_latency). All three FIR designs are length-matched at construction so a single value covers all. Reported via `set_latency_samples` once at `initialize()`.
- *Quality = IIR*: PDC = 0. IIR Linkwitz-Riley + all-pass-cascade Hilbert + Schroeder all-pass decorrelator are all zero-latency (the decorrelator's delays are internal to the all-pass loop, not added latency).
- **Quality is non-automatable** (mirrors Six Pack's Quality precedent). Switching Quality triggers plugin re-init from the GUI; latency reporting is fixed for the lifetime of the instance once `initialize()` runs.

### Load-bearing design choices

- **M/S encode runs once before crossovers.** Crossovers are linear, so per-band M/S encoding is mathematically equivalent to global M/S. Saves duplicated encode/decode work and gives every per-band stage a clean M/S input.
- **Width uses a constant-power M/S law.** At width=0 both gains are unity. As Width moves toward ±100, the *un*muted channel is boosted by √2 (~+3 dB) so total energy `M_gain² + S_gain² = 2` stays constant. This is the equal-power pan law applied to M/S; it produces unity output at width=0 and avoids the +6 dB side-energy spike that a naive `S_gain = 1 + width/100` law produces at +100.
- **Stereoize injects into Side, not into the L/R sum directly.** Width and Stereoize stay in the same domain (Side) and compose cleanly: Width=0 + Stereoize=high → wider stereo from a mono source; Width=−100 + Stereoize=0 → clean mono with +3 dB mid boost.
- **Recover Sides is a perceptual control, gated per-band by the sign of Width.** The Width law already preserves energy; Recover Sides exists to fold a phase-rotated residue of the *lost* side energy into mid, restoring perceptual width when narrowing without adding nominal RMS. Only bands with width<0 contribute to `S_removed_total`. Bands with width≥0 contribute zero, so engaging Recover Sides on a project with no narrowing is a no-op.
- **Stereoize Mode II is a real decorrelator, not a phase rotator.** Schroeder/Gerzon-style 6-stage all-pass cascade with mutually-prime delays. Cross-correlation between input and output drops below ~0.3 on broadband signals, distinct from the original Hilbert-90 design which left correlation near +0.8.
- **`hilbert_90` is used by Recover Sides only.** Mode II uses the Schroeder decorrelator instead. Hilbert runs once per buffer on the aggregated `S_removed_total` signal.
- **Solo is a post-mix gate that decodes only the soloed band's `(M_out, S_out)`.** Recover Sides is bypassed when soloed (the recover residue belongs to the un-soloed bands' Width changes). Keeps the rest of the path unchanged whether Solo is engaged or not.

### Stereoize specifics

- *Mode I (Haas)*: per-band sample-delay line. Default τ ≈ 12 ms (sample-rate adjusted). Delay buffer length sized for the maximum τ (25 ms) at the highest supported sample rate (192 kHz) at startup. τ is an internal constant for v1 (not a user param).
- *Mode II (Schroeder/Gerzon decorrelator)*: 6-stage cascaded all-pass network with mutually-prime delays (e.g. ≈ 41 / 53 / 67 / 79 / 97 / 113 samples at 48 kHz, sample-rate-scaled). Each stage `y[n] = −g·x[n] + x[n−D] + g·y[n−D]`, g=0.7. Zero added latency in IIR mode (group delay is frequency-dependent but the structure has no fixed delay-line tail). FIR variant in Linear-quality mode is matched to the FIR crossover group delay.

## 3. File / Module Layout

```
imagine/
├── Cargo.toml
└── src/
    ├── lib.rs              # Plugin, params, process(), Recover Sides aggregation
    │                       #   (stack-local; no persistent accumulator state),
    │                       #   latency reporting, Solo override
    ├── midside.rs          # M/S encode/decode helpers (with SIMD f32x16 paths)
    ├── crossover.rs        # 4-band split: linear-phase FIR + LR-IIR variants,
    │                       #   selected by Quality. One impl per quality, common trait.
    │                       #   IIR variant uses Lipshitz/Vanderkooy delay-matched cascade
    │                       #   for true allpass-summed 4-band reconstruction.
    │                       #   FIR variant uses double-buffered tap arrays (current + pending)
    │                       #   with crossfade on coefficient swap to eliminate clicks during
    │                       #   crossover-frequency automation.
    ├── hilbert.rs          # 90° phase rotator: FIR (matched group-delay to FIR crossover)
    │                       #   + IIR all-pass cascade. Used by Recover Sides only.
    ├── decorrelator.rs     # Schroeder/Gerzon 6-stage all-pass cascade with prime-spaced
    │                       #   delays. Used by Stereoize Mode II. FIR + IIR variants
    │                       #   parallel to hilbert.rs (same trait shape).
    ├── bands.rs            # Per-band state + process(): constant-power M/S Width gains,
    │                       #   Stereoize Mode I (Haas) / Mode II (decorrelator),
    │                       #   returns (M_out, S_out, S_removed) per sample. No persistent
    │                       #   accumulator — global aggregation lives in lib.rs::process.
    ├── spectrum.rs         # Audio-thread FFT (1024-pt, 1024 hop, throttled).
    │                       #   Single complex FFT of `M + jS` yields |M| and |S| per bin
    │                       #   (no second FFT). Produces:
    │                       #     - input |M| spectrum (atomic bin array, for crossover backdrop)
    │                       #     - magnitude-squared coherence per bin (computed audio-side,
    │                       #       published as a single AtomicU32 per bin to avoid
    │                       #       cross-bin tearing on the GUI side)
    ├── vectorscope.rs      # Lock-free ring buffer of (L,R) samples for GUI polar/Lissajous
    │                       #   render, decimated to ~4–8k points/frame.
    ├── theme.rs            # Pink/cyan duo-tone palette + helpers (mirrors pope-scope/theme.rs).
    ├── editor.rs           # softbuffer + baseview lifecycle, hit testing, drag, resize,
    │                       #   layout B coordination.
    └── editor/
        ├── vectorscope_view.rs   # Polar-sample + Lissajous render, mode toggle.
        ├── spectrum_view.rs      # Crossover spectrum (input |M| spectrum + draggable
        │                         #   splits) + coherence spectrum stacked beneath it.
        ├── band_strip.rs         # Per-band: Width slider, Stereoize knob, Mode I/II,
        │                         #   Solo. Right-click text-entry on continuous controls.
        └── global_strip.rs       # Recover Sides, Link Bands, Quality.
```

### Module boundaries

- `crossover.rs` exposes `Crossover::process(&mut self, ms_in: (f32, f32)) -> [(f32, f32); 4]` returning four `(M_band, S_band)` pairs. FIR vs IIR internals are not visible to callers.
- `hilbert.rs` and `decorrelator.rs` are single-input single-output, both with the same trait shape for FIR/IIR. Quality selects FIR or IIR at construction; no runtime branch on the audio path.
- `bands.rs` knows nothing about M/S encoding; it sees per-band M and S inputs only. `Band::process(...) -> (f32, f32, f32)` returns `(M_out, S_out, S_removed)`. No persistent S_removed accumulator — the global sum lives on the stack in `lib.rs::process`.
- `lib.rs` is the orchestrator: encode → crossover → bands → sum-on-stack → recover → decode → solo gate.
- Everything in `editor/` reads atomics + param state. No DSP lives there.

### Reused from `tiny-skia-widgets`
- `EditorState`, `SurfaceState`, `DragState`, `TextEditState<A>`
- `draw_dial_ex`, `draw_slider`, `draw_stepped_selector`, button/text helpers
- `fill_pixmap_opaque`, `fill_column_opaque`, `draw_rect`/`draw_rect_opaque`

### Workspace plumbing
- Add `imagine` to root `Cargo.toml` workspace members.
- Embedded font (`fonts/DejaVuSans.ttf`) consistent with the rest of the pack.
- `nih_plug` features include `assert_process_allocs` so debug builds panic on audio-thread allocations.
- `Plugin::process()` returns `ProcessStatus::Tail(fir_kernel_len as u32)` in Linear quality, `ProcessStatus::Normal` in IIR.
- Manual at `docs/imagine-manual.md` (markdown; PDF rendered alongside the rest of the pack's docs/).

## 4. UI & Parameter Set

### Layout (Layout B — vectorscope-left)

```
+-------------------------------+-----------------------------------+
| Vectorscope (~40% width)      |  Crossover spectrum + 3 splits    |
|  - polar sample (default)     |    (input |M| spectrum backdrop,  |
|    or Lissajous, toggle below |     draggable vertical lines at   |
|  - square aspect, large       |     120 / 1000 / 8000 Hz default) |
|                               |                                   |
|  [polar | lissajous]          |  +--+--+--+--+                    |
|                               |  |B1|B2|B3|B4|  band strip:       |
|  ─── correlation bar ───      |  |W |W |W |W |    Width slider    |
|  ─── balance bar L─R ───      |  |↕ |↕ |↕ |↕ |    (vertical)     |
|                               |  |Sz|Sz|Sz|Sz|    Stereoize knob  |
|                               |  |I |I |I |I |    Mode I/II       |
|                               |  |s |s |s |s |    Solo button     |
|                               |  +--+--+--+--+                    |
|                               |                                   |
|                               |  Coherence spectrum (per-freq)    |
+-------------------------------+-----------------------------------+
| Recover Sides ◯  ·  Link Bands ☐  ·  Quality [Linear / IIR]         |
+---------------------------------------------------------------------+
```

The band-strip Width slider is **vertical** (top = +100, center = 0, bottom = −100) so the four-up strip fits at the 720×580 minimum window. The "width spectrum" display shows magnitude-squared coherence per frequency, which is the right "decorrelation per frequency" metric (level imbalance / panning is the balance meter's job).

### Parameters (22 total)

**Per-band (×4):**

| Param | Range | Default | Type |
|---|---|---|---|
| `band{i}_width` | −100…+100 (constant-power M/S law) | 0 | `FloatParam`, automatable, smoothed |
| `band{i}_stereoize` | 0…100 | 0 | `FloatParam`, automatable, smoothed |
| `band{i}_stereoize_mode` | {I, II} | I | `EnumParam<StereoizeMode>`, automatable |
| `band{i}_solo` | bool | false | `BoolParam`, automatable |

**Global:**

| Param | Range | Default | Type |
|---|---|---|---|
| `crossover_1` | 20…20000 Hz (log) | 120 | `FloatParam`, automatable, smoothed |
| `crossover_2` | 20…20000 Hz (log) | 1000 | `FloatParam`, automatable, smoothed |
| `crossover_3` | 20…20000 Hz (log) | 8000 | `FloatParam`, automatable, smoothed |
| `recover_sides` | 0…100 | 0 | `FloatParam`, automatable, smoothed |
| `link_bands` | bool | false | `BoolParam`, automatable |
| `quality` | {Linear, IIR} | Linear | `EnumParam<Quality>`, **non-automatable** |

Bypass is host-managed via nih-plug's `BYPASS_BUFFER_COPY` const — no plugin-defined `bypass` param.

### Non-param GUI state (persisted via `EditorState`)
- Window size
- Vectorscope mode (Polar / Lissajous)

### Interaction model

- **Right-click on continuous controls** → text-entry field (via `TextEditState<A>`).
- **Right-click on stepped/toggle controls is a no-op** (Mode I/II, Solo, Quality, Link Bands), matching the pack-wide convention from CLAUDE.md.
- **Right-click priority in the crossover spectrum view**: split-handle hit > band-strip control hit > no-op. Empty regions of the spectrum view are non-interactive.
- Drag splits horizontally on the crossover spectrum to reposition; right-click on a split for text entry.
- Split ordering enforced (split 1 < split 2 < split 3); a drag that would cross is clamped at GUI level. DSP layer also enforces a minimum 50 Hz spacing as defense in depth.
- **`link_bands = true`**: dragging any band's Width slider applies the same delta to *all* bands' Widths, with the delta clamped to the smallest available headroom across all bands (so all bands move equally and the most-saturated band hits the rail first). Same logic for Stereoize amount. Mode I/II and Solo do not link.
- **Solo**: clicking one band's Solo turns off other bands' Solo (radio behavior). Click the active Solo to deactivate (returns to all-bands). Solo gates the output decode to that band's `(M_out, S_out)` only; Recover Sides is bypassed.
- **Mouse-wheel** on continuous controls: nudges by 1% of normalized range (default), 0.1% with shift held. Width's range is [−100, +100] mapped to [0, 1] normalized, so 1% = 2 units and 0.1% = 0.2 units — matches Six Pack's per-param granularity (normalized, not absolute).

### Window behavior
- Minimum size: ~720×580.
- Default size: 960×640.
- Free resize. Scale derived from `physical_width / WINDOW_WIDTH` like the rest of the pack.

### Visual identity
- Pink/cyan duo-tone palette.
- Vectorscope dots and Lissajous trace use pink and cyan to color L-leaning vs R-leaning samples (functional legend at a glance — pink = L-dominant, cyan = R-dominant).
- Coherence spectrum uses a single duo-tone gradient (cyan = coherent / phase-locked, pink = decorrelated / wide) so per-frequency stereo character is readable at a glance. Channel imbalance / panning is shown by the balance meter, not the coherence spectrum.
- `theme.rs` is plugin-local (mirrors pope-scope's pattern); `editor/*` imports from `crate::theme`. Not shared via `tiny-skia-widgets`.

## 5. Data Flow & Threading

### Audio thread (`process()`)

```
1. Read smoothed params (Width × 4, Stereoize × 4, RecoverSides, CrossoverFreq × 3)
   Quality is non-automatable; read once at initialize().
2. Per-block: any crossover freq changed beyond a small threshold?
     - IIR  : recompute LR coefficients in place (cheap arithmetic, no alloc)
     - Linear: redesign 4 FIR lowpass kernels into a pre-allocated *pending* tap array.
                Trigger crossfade: over the next K samples (K = max(buffer_size, 1024)),
                output is a sample-wise lerp from current FIR output to pending FIR output.
                After crossfade, pending becomes current and current is reused as the next
                pending buffer. Eliminates clicks during crossover automation.
                Cost: 4 × N transcendental ops (sinc + window) per redesign, where N is
                the FIR kernel length (1024 or 2048; final value chosen during prototyping).
3. For each sample:
     - encode L/R → M/S
     - crossover.process(M, S) → [(M_band, S_band); 4]
       (during crossfade: also crossfade.process_pending(M, S) → pending bands; lerp outputs)
     - bands.process(...) → [(M_out, S_out, S_removed); 4]
       (constant-power Width, Stereoize Mode I/II, gated S_removed)
     - sum bands on stack → (M_sum, S_sum, S_removed_total)
     - apply Recover Sides: M_final = M_sum + hilbert_90(S_removed_total) · recover_amount
     - solo override (if any band soloed: M_final ← M_out[soloed], S_sum ← S_out[soloed])
     - decode M/S → L/R
4. Per-block update GUI atomics (throttled to one update per ~1024 samples):
     - vectorscope ring buffer: push N (L,R) samples (SPSC, Release on write_pos)
     - input spectrum: complex FFT of `M[t] + j·S[t]` (1024-pt) gives |M| and |S| per bin
                       in one transform. Write |M| as 128 log-binned AtomicU32 array.
     - coherence spectrum: compute magnitude-squared coherence per bin on the audio
                           thread (single bin scalar per frame), publish as AtomicU32 array.
                           No per-bin race because the ratio is computed pre-publish.
     - correlation: Pearson on recent block, single AtomicU32
     - balance: rms(L) vs rms(R) ratio, single AtomicU32
```

### GUI thread (editor render at ~60 fps)

```
1. Tick from baseview frame timer
2. Read pending_resize AtomicU64; if changed, recompute layout
3. Read atomics (Acquire on ring-buffer write_pos; Relaxed on per-bin atomics):
     - input |M| spectrum bins → backdrop of crossover spectrum
     - coherence spectrum bins → duo-tone gradient bar under spectrum
     - vectorscope ring buffer → decimate to ~4–8k points → render polar or Lissajous
     - correlation → bar position
     - balance → bar position
4. Read params for control rendering (band strips, global strip, splits)
5. Render via tiny-skia + softbuffer present
```

### Lock-free primitives

| Channel | Type | Sizing | Ordering |
|---|---|---|---|
| Vectorscope samples | Ring buffer of `(f32, f32)`, SPSC (mirrors `pope-scope/src/ring_buffer.rs`) | 32768 stereo pairs (~340 ms @ 96 kHz; ~170 ms @ 192 kHz) — sized for max-buffers-per-frame at the highest supported sample rate so the audio thread cannot lap the GUI in one frame | writer Release on `write_pos`, reader Acquire |
| Input \|M\| spectrum | `[AtomicU32; 128]` (f32 bits) | 128 log-spaced bins, 20 Hz – 20 kHz | Relaxed (per-bin display, tearing tolerable) |
| Coherence spectrum | `[AtomicU32; 128]` (f32 bits) | same binning, single coherence value per bin (computed audio-side, no cross-bin race) | Relaxed |
| Correlation | `AtomicU32` (f32 bits, range −1…+1) | single value | Relaxed |
| Balance | `AtomicU32` (f32 bits, range −1…+1) | single value | Relaxed |

### No allocations on audio thread
All buffers are pre-allocated in `initialize()` from the host's max buffer size and the highest supported sample rate (192 kHz). This includes:
- FIR crossover tap arrays — *double-buffered* (current + pending) for crossfade on coefficient swap, sized for the chosen FIR length × 4 bands × 2 buffers
- FIR delay lines × 4 bands (one for crossover, one for Hilbert, one for decorrelator FIR)
- IIR LR coefficient arrays + state (8-tap biquad-cascade per band × 4)
- Hilbert delay line (single, shared by Recover Sides path)
- Decorrelator delay lines × 4 bands × 6 stages (max prime-delay sample count at max sample rate)
- Haas delay lines × 4 bands (sized for max τ = 25 ms at 192 kHz = 4800 samples each)
- Spectrum FFT scratch (1024-pt complex FFT input + output buffers)
- Vectorscope ring buffer (32768 stereo pairs)
- FIR redesign scratch (sinc + window arrays, max kernel length × 4 bands)
- Crossfade lerp counter (single `usize`)

`assert_process_allocs` (debug builds) panics on any audio-thread allocation, providing CI-level enforcement.

### Param smoothing
- Width, Stereoize, Recover Sides, Crossover freqs: linear-smoothed at sample rate (nih-plug smoother).
- Mode I/II, Solo, Link: instantaneous (no smoothing).
- Quality: non-automatable, set once at `initialize()`.

### Crossover redesign deferral & crossfade
Dragging a split fires param changes at GUI rate. The smoother lerps to the target. Per-block we check whether the smoothed crossover value has moved beyond a small threshold (e.g. 0.5 Hz); if so, redesign FIR kernels into the *pending* tap array and start a sample-wise crossfade between current and pending FIR outputs over K samples (K = max(buffer_size, 1024)). When the crossfade completes, pending becomes current and the now-stale array is reused as the next pending. This eliminates the impulse-response discontinuity that causes clicks during automation. IIR mode redesigns coefficients in place and applies the new coefs immediately (LR state is small enough that the transient is imperceptible).

Cost: one FIR redesign per detected crossover change is `4 × N` transcendental ops (sinc + window). At N=1024 that's ~4k; at N=2048, ~8k. Crossfade adds 4 × N taps × K samples of additional convolution work for the duration of the crossfade — bounded and predictable.

### Latency reporting
- Reported once via `set_latency_samples` in `initialize()`, based on Quality (FIR length in Linear; 0 in IIR).
- Quality is non-automatable. Changing Quality requires plugin re-init (the host may handle this differently — some DAWs re-instantiate, some pop). Documented as a "set before playback" decision in the manual.

### `ProcessStatus`
- Linear quality: returns `ProcessStatus::Tail(fir_kernel_len as u32)` so the host renders the FIR tail when audio stops.
- IIR quality: returns `ProcessStatus::Normal`.

## 6. Testing Strategy

Inline `#[cfg(test)]` modules per source file, mirroring the pack convention. Target ~50 tests across the plugin.

### `midside.rs` (~6 tests)
- L/R → M/S → L/R round-trip is identity (per-sample and SIMD path)
- Encode/decode of pure mono (S=0), pure side (M=0), and silence
- SIMD `f32x16` path agrees with scalar within machine epsilon

### `crossover.rs` (~12 tests)
- Linear-phase FIR: sum of all 4 bands ≈ delayed identity
- LR-IIR (4-band, Lipshitz/Vanderkooy delay-matched): sum is magnitude-flat ±0.05 dB across [20 Hz, 20 kHz] and phase-rotated allpass-equivalent
- Both: each band's response peaks where expected
- FIR crossfade: rapid crossover automation produces no impulse-response discontinuity (output is C0-continuous within numerical tolerance)
- FIR redesign: in-place coefficient swap into pre-allocated pending buffer; no allocation under `assert_process_allocs`
- IIR coefficient swap: 4-band cascade does not NaN at corner frequencies (20 Hz, sample_rate / 2 − 100 Hz)
- Order constraint: drag clamping at GUI level + 50 Hz minimum spacing at DSP level
- Boundary conditions: split exactly at Nyquist or DC doesn't produce NaN

### `hilbert.rs` (~6 tests)
- FIR variant: magnitude ≈ 1.0 across [50 Hz, 18 kHz]
- FIR variant: phase ≈ ±90° across the same range
- IIR all-pass cascade: magnitude ≈ 1.0 ± 0.01
- IIR phase deviation from 90° within ±5° across [50 Hz, 18 kHz]
- Group delay matches reported latency for FIR variant
- Linearity (stability under input scaling)

### `decorrelator.rs` (~6 tests)
- IIR variant: magnitude ≈ 1.0 ± 0.05 across [20 Hz, 20 kHz] (all-pass cascade is magnitude-flat by construction)
- *True decorrelation*: cross-correlation between input and output on broadband white noise is below 0.3 (the Hilbert-90 design fails this — we explicitly verify the Schroeder cascade does not)
- Output amplitude does not exceed input amplitude (stability of the all-pass coefficients with g=0.7)
- Sample-rate-scaled prime delays produce equivalent decorrelation at 44.1 / 48 / 96 / 192 kHz
- FIR variant: matches IIR variant within window-taper tolerance and has the FIR crossover's group delay
- Resonance check: no spectral peaks above 1.0 in the magnitude response (mutually-prime delays prevent comb resonances)

### `bands.rs` (~12 tests)
- Width=0, Stereoize=0: output equals input (M_gain = S_gain = 1)
- Width=−100: S_gain = 0 → output is mono; M_gain = √2 (mid boosted +3 dB to preserve total power)
- Width=+100: M_gain = 0 → output is sides only; S_gain = √2 (sides boosted +3 dB)
- Constant-power identity: at any width, `M_gain² + S_gain² = 2` (verified across 21 width values)
- Stereoize Mode I: delayed mid appears in side at expected delay tap (sample-accurate)
- Stereoize Mode II: cross-correlation between Stereoize-injected component and source M is below 0.3 on broadband noise (genuine decorrelation, not phase rotation)
- `S_removed[i]` gating: zero when width ≥ 0; equals `S_band · (1 − S_gain)` when width < 0
- `Band::process` returns `(M_out, S_out, S_removed)` with no persistent accumulator state on `Band`
- Solo: when band i is soloed, decoded output equals decode(M_out[i], S_out[i]) only
- Stereoize amount=0: no injection regardless of mode
- Mode I delay length sample-rate-correct at 44.1 / 48 / 96 / 192 kHz
- Decorrelator delay-line state is sample-rate-scaled at construction

### `lib.rs::plugin_tests` (~12 tests)
- *Host bypass equivalence*: with host bypass active (via `BYPASS_BUFFER_COPY`), output equals input bit-for-bit. No plugin-defined `bypass` param.
- *No-op equivalence*: all bands at Width=0 + Stereoize=0 + Recover=0 produces output equal to input within crossover-summing precision.
- *Constant-power preservation*: total RMS with all bands at Width=−100 (full mono) ≈ total RMS of input for symmetric stereo input (the +3 dB mid boost compensates for zeroed sides).
- *Recover Sides gating*: with all bands at Width=+100 and Recover=100, output equals the no-Recover case (gating zeroes S_removed when width≥0; positive widths do not engage the Hilbert path).
- *Recover Sides perceptual residue*: with bands at Width=−100 and Recover=100, the Hilbert-rotated S_removed is folded into M (audible signal energy moves from L−R to L+R; phase relationship verifiable via FFT).
- *Solo gating*: soloing one band with all bands at non-zero Width produces output equal to decode(M_out[soloed], S_out[soloed]) (Recover Sides bypassed for soloed output).
- *Latency reporting*: actual measured impulse delay matches `latency_samples()` for both Quality modes (FIR length in Linear; 0 in IIR).
- *Sample-rate sweep*: 44.1 / 48 / 88.2 / 96 / 176.4 / 192 kHz produce stable, non-NaN output.
- *Crossover drag stress*: rapid crossover automation produces click-free output (FIR crossfade test) and zero allocations under `assert_process_allocs`.
- *Stereoize integration*: Mode I + Mode II selectable per band, both reach output through M/S decode.
- *Link Bands clamping*: with band 0 at Width=+95 and Link active, dragging +10 on band 1 results in all bands moving by +5 (delta clamped to band 0's available headroom).
- *Allocation guard*: 30-second `process()` run with all params automating produces zero allocations (verified by `assert_process_allocs`).

### `spectrum.rs` (~5 tests)
- Sine input at 1 kHz produces peak at the 1 kHz log bin in |M|
- Complex `M + jS` FFT trick: real-part magnitude == |M| FFT and imag-part magnitude == |S| FFT for verified test signals (round-trip identity)
- Coherence = 1.0 for fully phase-locked stereo input (mono summed to L=R)
- Coherence ≈ 0 for fully decorrelated stereo input (independent white noise on L vs R)
- Bin count and range stable across sample rates (44.1 / 48 / 96 / 192 kHz)

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
- *Quality is non-automatable*. Switching requires plugin re-init (set before playback starts).
- *Constant-power Width law boosts the unmuted channel by ~+3 dB at extremes*. At Width=−100, mid is boosted by √2 to preserve total RMS; at Width=+100, side is boosted by √2 likewise. This is by design (matches mastering-imager convention) but may surprise users coming from naive M/S width plugins.
- *Stereoize Mode I creates a comb filter on stereo input*. The Haas-style mid-into-side delay produces audible notches every ~1/(2τ) Hz on signals that already have side content. **Use Mode I primarily for mono sources; Mode II for stereo** (Mode II's Schroeder decorrelator is mono-compatible and doesn't comb).
- *Linear-phase pre-ringing*: visible on transients narrowed to mono. Intrinsic; users who care choose IIR.
- *IIR phase rotation*: identical between L and R (so M/S width is unaffected), but absolute phase between bands shifts. Audible primarily on signals spanning band boundaries.
- *Width + Stereoize + Recover Sides combined at extremes*: can produce inter-channel correlation pathologies. User-managed; no auto-attenuation.

### Init / lifecycle
- `Plugin::initialize`: pre-allocate everything listed in §5 "No allocations on audio thread." Sized for max conceivable buffer × max sample rate (192 kHz).
- Sample rate change: re-derive cutoffs (`cutoff_norm = freq_hz / sample_rate`); IIR coefficients recomputed; FIR kernels redesigned (in-place into pending buffer + crossfade); decorrelator delay-line indices re-derived from sample-rate-scaled primes; latency re-reported.
- Buffer-max-size change: vectorscope ring sized for max conceivable buffer at max sample rate (32k stereo pairs).

### Numerical safety
- Crossover frequencies clamped to `[20 Hz, sample_rate / 2 − 100 Hz]` before coefficient design.
- Crossover ordering invariant enforced both at the GUI layer (drag clamping) AND at the DSP layer (sort + 50 Hz minimum spacing) before filter design.
- IIR LR coefficients verified non-NaN before use; if pathological at corner frequencies, fall back to bypass for that band.
- IIR LR cascade uses Lipshitz/Vanderkooy delay-matched compensation across the 4-band tree so the band sum is true allpass (magnitude-flat to ±0.05 dB across 20 Hz–20 kHz).
- Decorrelator all-pass coefficients (g=0.7) chosen for guaranteed stability; output amplitude bounded above by input amplitude.

### Concurrency invariants
- Vectorscope ring buffer: SPSC, audio writes, GUI reads. `write_pos` is `AtomicUsize` with writer Release / reader Acquire (mirrors `pope-scope/src/ring_buffer.rs`). GUI reads up to `min(write_pos, buffer_size)` samples.
- Spectrum atomics: input |M| bins written every N samples (throttled, Relaxed) — per-bin tearing tolerable for a magnitude display. Coherence bins are computed audio-side (single ratio per bin) and published with Relaxed ordering — no cross-bin race because the ratio is finalized before publish.
- All audio-thread param reads go through nih-plug's smoothed param API.

### Resize
- `pending_resize: AtomicU64` packs `(width_u32, height_u32)`. Editor consumes on next frame; pixmap and softbuffer are rebuilt at new size.
- Layout coordinates computed proportionally each frame; no cached absolute pixel positions.

### Open questions for the implementation phase
1. **FIR kernel length**: 1024 vs 2048 taps. Tradeoff is latency (~10 ms vs ~21 ms at 48 kHz) vs split sharpness. Prototype both and pick by ear + spectrum sharpness.
2. **Decorrelator prime delays**: starting set ≈ {41, 53, 67, 79, 97, 113} samples at 48 kHz. Tune by ear for a "natural spread" without resonance or obvious comb artifacts.
3. **Hilbert IIR cascade order**: 4 stages → ~5° max error; 6 stages → ~1°. CPU cost negligible; pick 6.
4. **Haas delay default τ**: 5–25 ms range. 12 ms is a "wide but not detached" starting point. Validate by ear during prototyping.
5. **Default crossover frequencies**: 120 / 1000 / 8000 Hz are mastering-conventional, but the hi-mid/highs split could go higher (e.g. 12 kHz). Decide in prototyping.
6. **FIR crossfade length K**: max(buffer_size, 1024) is the starting heuristic. May want to bound to a fixed maximum (e.g. 2048 samples) to keep the crossfade audibly imperceptible without dragging the transition out.
