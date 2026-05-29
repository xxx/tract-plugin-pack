# Nap — Velvet Reverb — Design

**Date:** 2026-05-29
**Crate:** `nap` (new workspace member; shared reuse from `tiny-skia-widgets` and `tract-dsp`)
**Status:** approved 2026-05-29

## Goal

A new plugin for the pack: **Nap**, an experimental *character reverb* built on
the Extended Dark Velvet Noise (EDVN) engine. Its defining feature — the reason
to build it rather than another algorithmic reverb — is that the user **draws
three curves over a shared tail-position axis (0 → 100 %)** that sculpt how the
reverb tail's **loudness**, **stereo width**, and **tone** each evolve over its
length. The DSP is cheap sparse-velvet convolution (no FFT, zero added latency);
the GUI is rich (three MSEG editors + a live visualization of the actual
generated tail).

Nap fills the pack's single biggest gap — it has **no reverb** — and does it with
a control surface no commercial reverb offers.

## Background

The design follows a deep-research survey
(`docs/research/2026-05-new-audio-dsp-papers.md`) that ranked velvet-noise
reverberation as the strongest buildable, in-window, no-ML candidate. The
algorithm comes from two Aalto Acoustics Lab papers, with the implementable math
extracted and cross-checked against the source PDFs:

- **EDVN** — Fagerström, Schlecht, Välimäki, *"Non-exponential reverberation
  modeling using dark velvet noise,"* JAES 72(6):370–382, 2024
  (arXiv 2403.20090). Introduces the dictionary-filter + probability-matrix
  architecture and the decay/coloration decoupling.
- **BDVN** — Fagerström, Meyer-Kahlen, Schlecht, Välimäki, *"Binaural
  Dark-Velvet-Noise Reverberator,"* DAFx 2024 paper 63. Introduces the
  pulse-location-jitter method for frequency-dependent interaural coherence.

Two facts from these papers are load-bearing for Nap's design:

1. **Decay and coloration are mathematically decoupled.** EDVN factors a target
   into a column-normalized probability matrix `P` (spectral shape, scale-free)
   and a per-frame broadband gain `γ(t)` (level). Because they are independent,
   an arbitrary energy-decay envelope `g(m)` can *replace* `γ`-derived gains
   without touching coloration. → a user-drawable decay curve is essentially free.
2. **Interaural coherence equals the DTFT of the pulse-location jitter PDF**
   (BDVN eq. 20: `S_LR(ω) = σ²·Σ_l p_Δ(l)·e^{−jωl}`). A *single* jitter
   distribution produces the entire frequency-dependent coherence curve; making
   its width time-varying animates the stereo image at **zero added compute**.
   → a user-drawable width curve maps directly to per-pulse max-jitter.

### Existing pack assets Nap reuses

- `tiny_skia_widgets::mseg` — `MsegData` (serde-persistable curve model),
  `MsegEditState::new_curve_only()` (curve-only editor with full mouse
  interaction), `value_at_phase()` for sampling. The three curves are three
  `MsegData` instances.
- `tiny-skia-widgets` — `param_dial`, `controls` (button), `editor_base`
  (`EditorState` size persistence, `SurfaceState`), `drag`, `text`,
  `primitives`.
- The **miff `KernelHandoff` pattern** — `Arc<Mutex<…>>` + `try_lock`
  GUI→audio handoff of a baked artifact, with a single-walk curve sampler that
  reproduces `value_at_phase` without per-tap rescans.
- `tract_dsp::db` for dB↔linear on the trim/mix controls.

> Note: EDVN's coloration uses small **all-pole IIR filters**, which *replace*
> the original-DVN boxcar pulse-width trick. So `tract_dsp::boxcar::RunningSumWindow`
> is **not** used here, despite the original research note suggesting it.

## Non-goals (v1)

- **No impulse-response loading or room matching.** The EDVN
  STFT-analysis → NNLS-fit → LP-filter-design pipeline exists only to *match a
  measured room*; it is the expensive part and a different product. Nap's
  dictionary is hand-designed. (A future "analyze a room" mode could add it.)
- **No global LFO / tempo-synced coherence modulation.** Coherence animation is
  the drawn Width curve (per-tail trajectory), not a free-running LFO.
- **No freeze/infinite-hold, no modulation of the tail by external input.**
- **Not held to the 100+-instance CPU/RAM target.** Nap is a feature plugin
  (like multosis); the DSP is cheap by nature but the GUI is free to be rich.

## Architecture

Two stages split across threads — the **miff bake/handoff pattern**.

```
GUI thread (on design-param edit)        Audio thread (per sample)
─────────────────────────────────        ──────────────────────────
Size, Density, Seed, Width amount,        input → ring buffer
3 MsegData curves                              │
        │ generate                             │ sparse scatter
        ▼                                   ┌───┴── pulse taps → Q filter excitations
  VelvetSequence                            │
  {loc, coeff, filter_idx}_L                Q parallel all-pole IIR filters
  {loc_R} (jittered)                        │ sum
        │ Arc<Mutex<>> publish              post-filter + DC blocker
        ▼ try_lock consume ───────────────► pre-delay + dry/wet mix → out
```

### Stage 1 — `sequence.rs`: velvet sequence generation (GUI thread)

Deterministic, seeded. Inputs: sample rate, Size `L` (samples), Density `ρ`
(pulses/s), Width amount, Seed, and the three `MsegData` curves.

- Grid spacing `Td = fs / ρ`; pulse count `M = L / Td`.
- For pulse `m = 0 … M−1`, with a seeded RNG (so output is reproducible and only
  changes when Seed or a design param changes):
  - **Location** `k(m) = round(m·Td + r₁·(Td − 1))`, `r₁ ~ U[0,1]`.
  - **Tail phase** `φ = k(m) / L ∈ [0,1]` (the shared x-axis for all curves).
  - **Sign** `s(m) = 2·⌊r₂⌋ − 1 ∈ {−1, +1}`.
  - **Decay** → `g(m) = decay.value_at_phase(φ)`, scaled by `√Td`
    (density-energy compensation) then the whole sequence peak-normalized so
    output level is consistent across shapes.
  - **Coefficient** `c(m) = s(m)·g(m)` (folded so playback does one multiply per
    tap; the unit-velvet backbone is structurally multiply-free).
  - **Tone** → brightness scalar `t = tone.value_at_phase(φ)` selects a target
    filter in the ordered dark→bright dictionary; the actual `filter_idx(m)` is
    chosen by EDVN idle-time greedy routing (eqs. 12–14) around that target to
    avoid the same filter clustering on consecutive pulses.
  - **Width** → `J(m) = width.value_at_phase(φ) · J_max` (J_max from Width
    amount). Right-channel jitter `Δ(m)` drawn by inverse-CDF sampling from a
    **Hann-window PDF of half-width `J(m)`**; `k_R(m) = k(m) + Δ(m)` (clamped
    ≥ 0). `J(m)=0` → `k_R = k_L` → mono/coherent at that point in the tail.
- Output `VelvetSequence`: parallel arrays `location[]`, `coeff[]`,
  `filter_idx[]` for the L channel, plus `location_R[]`. Pre-sorted by location
  for cache-friendly playback. Published through `handoff.rs`.

### Stage 2 — `engine.rs`: playback (audio thread)

Per channel (L uses `location/coeff/filter_idx`; R uses `location_R` with the
same `coeff/filter_idx`):

1. Write input sample into a pre-allocated ring buffer (length ≥
   `L + max_predelay + max_jitter`).
2. **Sparse convolution:** for each active pulse tap, add `c(m)·x[n − k(m)]`
   into the excitation accumulator of filter `filter_idx(m)`. Cost is **O(M)
   taps/sample** — far cheaper than dense convolution (`O(L)`), but *not*
   constant; it scales with Size × Density.
3. Run the `Q` parallel all-pole IIR coloration filters on their accumulators.
4. Sum filter outputs → shared **post-filter** (gentle LP) + **DC blocker**.
5. Apply **pre-delay** and **dry/wet mix** against the delay-aligned dry signal.

**Why FIR-not-feedback:** an arbitrary drawn decay shape (gated, reverse,
multi-bump) *requires* a finite-impulse-response structure. A feedback/FDN tail
can only produce exponential decay. Paying O(M) is the deliberate cost of the
draw-your-decay feature; it is the reason Nap is feature-tier, not
100+-instance.

### `coloration.rs`: the all-pole dictionary

A hand-designed, fixed set of `Q ≈ 6` low-order all-pole filters ordered
dark → bright (e.g. resonant-low through open-high). Order is a tuning parameter
(papers use 2nd-order in EDVN, 5th-order in BDVN room matches; start at 2nd,
raise if needed). All filters validated stable (poles strictly inside the unit
circle). No runtime LP fitting — coefficients are baked constants tuned by ear.

### `handoff.rs`: `SequenceHandoff`

Sibling of miff's `KernelHandoff`. `Arc<Mutex<VelvetSequence>>` + a "dirty" /
generation counter. GUI thread generates and publishes; audio thread
`try_lock`s once per block, swaps in the new sequence if present, and otherwise
keeps running the current one (never blocks, never allocates). Sequence buffers
are sized to a max capacity so swap is a move, not a realloc.

## The three curves → DSP mapping

| Curve (`MsegData`, unipolar 0–1) | Drives | Sounds like |
|---|---|---|
| **Decay** | energy gain `g(m)` | exponential, gated, reverse, multi-bump, plateau |
| **Width** | per-pulse max-jitter `J(m)` | image collapses / expands / pulses along the tail (0 = mono) |
| **Tone**  | dictionary routing `filter_idx(m)` | bright→dark air absorption, or any tonal trajectory |

## Parameters

**Automatable, smoothed on the audio thread** (cheap, no regen):

- `mix` — dry/wet, 0–100 %.
- `predelay` — 0–200 ms.
- `output` — output trim, dB.
- `input` — input trim, dB.

**Design-time — non-automatable, trigger regeneration on edit** (like miff's
`Length`, kept off the audio thread):

- `size` — tail length, ~0.1–10 s.
- `density` — pulses/s, ~500–4000.
- `width` — Width-amount; scales the Width curve to physical max-jitter (ms).
- `seed` — `IntParam`, re-rolls the random velvet pattern.

**Persisted GUI state** (`#[persist]`, regenerate on change):

- `decay_curve`, `width_curve`, `tone_curve` — `Arc<Mutex<MsegData>>` each.
- `editor_state` — `Arc<EditorState>`.

Rationale for the automatable/design-time split: jitter and pulse gains are
baked into integer sample positions and per-tap coefficients, so changing them
means regenerating the sequence (an O(M) walk) — correct to keep off the audio
thread. Mix/pre-delay/trims are continuous and stay smooth + automatable. This
matches miff's established philosophy.

## GUI / editor

softbuffer + tiny-skia, CPU-rendered, freely resizable (`EditorState`,
`physical_width / WINDOW_WIDTH` scale, packed-`AtomicU64` host resize), per pack
convention.

Layout, top → bottom:

1. **Header** — title + bypass/format affordances as per pack norm.
2. **Three stacked curve-only MSEG editors** — Decay / Width / Tone, left-labeled,
   sharing one tail-position x-axis. Each is a `MsegEditState::new_curve_only()`
   over its `MsegData`. Right-click text-entry and the usual MSEG gestures come
   for free.
3. **Live tail visualization** — renders the *actual* current `VelvetSequence`
   as a decaying pulse field: stick height = `g(m)`, color = tone/filter index,
   horizontal L/R offset = jitter/width. This is the "see exactly what you drew"
   payoff and the at-a-glance read of the reverb. Sequence snapshot reaches the
   editor via a lightweight clone on handoff (or `tract_dsp::spsc`).
4. **Bottom strip** — Size, Density, Width, Pre-Delay, Mix, Output dials
   (`param_dial`) + a Seed / Regenerate button (`controls`).

## Latency & process status

- **Zero reported latency.** Time-domain causal sparse convolution — no FFT, no
  lookahead. (A genuine selling point vs. satch / warp-zone.) Pre-Delay delays
  only the wet path (the dry signal stays time-aligned), so it is a musical
  control, not algorithmic latency, and is **not** reported to the host.
- `process()` returns `ProcessStatus::Tail` while the velvet tail rings out after
  input goes silent; counts silent input samples like satch.

## Audio-thread safety

- No allocations in `process()`: ring buffers, filter state, and sequence
  buffers are pre-allocated to max capacity; handoff is `try_lock` + move.
  `assert_process_allocs` is already in the dep feature set and will catch
  regressions in tests.
- No `unsafe` beyond the standard baseview/raw-window-handle glue inherited from
  the editor scaffold.

## Crate layout & integration

Add `"nap"` to `Cargo.toml` `members`. Dependencies per the `satch` template
(nih-plug fork, baseview v0.1.1, softbuffer, tiny-skia, tiny-skia-widgets,
tract-dsp, serde, crossbeam, keyboard-types, raw-window-handle 0.5/0.6).

```
nap/
  Cargo.toml
  src/
    lib.rs          # Nap plugin struct, NapParams, process()
    sequence.rs     # EDVN velvet sequence generation (GUI thread)
    engine.rs       # audio-thread sparse convolver + all-pole bank + post/DC
    coloration.rs   # hand-designed dark→bright all-pole dictionary
    handoff.rs      # SequenceHandoff (KernelHandoff sibling)
    editor.rs
    editor/
      tail_view.rs  # live pulse-field visualization
    main.rs
    fonts/DejaVuSans.ttf
  benches/
    dsp.rs          # criterion: engine::process over a Size×Density matrix
```

Generic velvet primitives stay in-crate for v1; promote to `tract-dsp` only if a
second consumer appears (YAGNI).

## Testing strategy (TDD)

Inline `#[cfg(test)]` modules, run under `cargo nextest`.

1. **Decay decoupling** — RMS-over-time of the generated impulse response tracks
   the drawn Decay curve within tolerance, independent of Tone/Width settings.
2. **Coherence ↔ width** — per-tail-segment interaural coherence of the L/R
   sequences tracks the drawn Width curve (validates the DTFT-of-jitter relation);
   `Width=0` → coherence ≈ 1 (mono).
3. **Sparse == dense** — sparse-convolver output equals a reference dense
   convolution against the materialized velvet FIR (golden test, tight epsilon).
4. **Determinism** — identical (seed, params, curves) → byte-identical
   `VelvetSequence`.
5. **Coloration** — dictionary ordering monotonic in spectral centroid
   (dark→bright); every filter stable (poles inside unit circle).
6. **Identity / silence** — `mix = 0` → exact dry passthrough; zero input →
   decays to silence and reports `Tail`.
7. **No-alloc** — process under `assert_process_allocs`; handoff never blocks.
8. **Criterion bench** — `engine::process` over a Size × Density matrix to
   establish the cost ceiling and guard regressions.

## Open risks & mitigations

1. **O(M) cost ceiling** — long Size × high Density is expensive. Mitigation:
   benchmark early; bound the Size × Density product; consider a soft cap or a
   "density too high" indicator in the GUI. Accepted tradeoff for arbitrary
   drawn decay.
2. **All-pole dictionary voicing** — "design by ear" with no room target.
   Mitigation: start simple (≈6 second-order filters), iterate against test
   signals; the spectral-centroid-ordering test keeps the dark→bright invariant
   honest.
3. **Raw velvet brightness / metallic character** — Mitigation: the dictionary +
   Tone curve + post-LP are the coloration; validate by ear with the tail
   visualization as a debugging aid; the `debug_log!` macro + debug bundle for
   any audio-thread diagnostics.
