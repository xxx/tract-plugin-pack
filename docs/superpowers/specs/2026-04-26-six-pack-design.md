# Six Pack — Multiband Saturator Design

**Date:** 2026-04-26
**Status:** Design draft

## Overview

Six Pack is a 6-band parallel saturator inspired by Wavesfactory Spectre. It does not work like a conventional multiband distortion: each band's EQ boost is computed against the dry signal as a *difference*, the difference is then run through a per-band saturation algorithm, and all band outputs are summed in parallel with the dry signal. A band at 0 dB gain produces zero difference and contributes silence — gain is *boost-only* and effectively serves as the per-band drive.

Six bands of 1 low-shelf + 4 peaks + 1 high-shelf, six saturation algorithms (Tube, Tape, Diode, Digital, Class B, Wavefold), per-band channel routing (Stereo / Mid / Side), global drive (Easy / Standard / Crush), oversampling (Off / 4× / 8× / 16×), and a global de-emphasis toggle that subtracts the linear EQ boost from the output to leave only the saturation-introduced harmonics. A live input spectrum analyzer sits behind the EQ curve display.

The plugin uses softbuffer + tiny-skia CPU rendering (no GPU), targets many instances per project, and follows the existing tract-plugin-pack conventions for tooling, layout, and DSP discipline.

### Divergences from Spectre (intentional)

Six Pack is inspired by Spectre but is not a clone. Concrete divergences:

- **6 bands instead of 5.** Spectre has 1 low-shelf + 3 peaks + 1 high-shelf; Six Pack adds a second mid-peak slot for finer harmonic placement.
- **Different algorithm set.** Spectre ships 10 distortion algorithms (Solid, Tape, Tube, Warm Tube, Class B, Diode, Bit, Digital, Rectify, Half Rectify) plus a Clean bypass. Six Pack ships 6: Tube, Tape, Diode, Digital, Class B, and Wavefold (signature). We omit Solid, Warm Tube, Bit, Rectify, Half Rectify, and Clean. Wavefold replaces no specific Spectre algorithm — it covers a sonic territory none of Spectre's clipping-family algorithms reach.
- **Per-band channel routing is 3-way (Stereo / Mid / Side), not 5-way.** Spectre adds Left and Right per-band modes; we omit them. Easy to add later if needed.
- **Oversampling has four tiers (Off / 4× / 8× / 16×), not Spectre's three (Normal / Medium / Best = 1× / 4× / 16×).** We add an 8× mid tier for users who want lower-than-16×-CPU but more headroom than 4×.
- **No "Clean" bypass algorithm.** Users wanting a parallel boosting EQ should use a real EQ.

## Background: Spectre's "distort the difference" mechanism

The defining trick: Spectre's per-band processing computes `boost_b = EQ_b(dry) − dry`, then saturates only `boost_b`. The dry signal itself is *not* directly distorted; only the EQ-boosted spectrum is. This is the mechanism we replicate.

Three load-bearing properties follow:

1. **Boost-only gain** is required. A band's gain ≥ 0 dB. The EQ filter has frequency-domain magnitude ≥ 0 dB everywhere (boost-only); the time-domain diff `boost_b = EQ_b(dry) − dry` is sign-changing as a signal but represents only the boosted spectral content. At 0 dB, `EQ_b(dry) == dry` exactly (load-bearing analytical identity, see SVF requirements below), so `boost = 0`, the saturator outputs 0, and the band is silent.
2. **Per-band gain doubles as drive.** Larger boost → larger diff → harder hit on the saturator → more harmonics. The global drive selector (Easy/Standard/Crush) is multiplicative on top.
3. **De-emphasis isolates harmonics.** Output `dry + wet_amp(mix) · Σ saturate(boost_b)` includes both the linear EQ boost and the harmonic content (within the wet path). Subtracting `Σ boost_b` from the wet path removes the linear shape and leaves only the harmonics added by saturation, so the EQ curve stops being audible as an EQ. The cancellation is exact in the trivial-saturation limit (`saturate(x) = x`) — at that limit, with de-emphasis on, `output = dry_amp(mix) · dry`, which equals the original dry signal exactly when `mix ≤ 50%` (where `dry_amp = 1.0`). Above 50% the dry component is being faded out by design, so the residual is simply `dry_amp(mix) · dry`, not silence-vs-dry confusion.

## Signal Flow / DSP Architecture

Per-block processing at the audio rate, with oversampling factor `N` ∈ {1, 4, 8, 16}:

```
input(L,R) ─► input_gain ─► upsample×N ─┬─────────────► dry_OS(L, R) ─────────────────────┐
                                          │                                                  │
                                          ├─► band 1 SVF_L, SVF_R (low-shelf)               │
                                          ├─► band 2 SVF_L, SVF_R (peak)                    │
                                          ├─► band 3 SVF_L, SVF_R (peak)                    │
                                          ├─► band 4 SVF_L, SVF_R (peak)                    │
                                          ├─► band 5 SVF_L, SVF_R (peak)                    │
                                          └─► band 6 SVF_L, SVF_R (high-shelf)              │
                                                                                              │
For each band b ∈ {1..6}:                                                                    │
    diff_L = svf_L_b(dry_OS_L) − dry_OS_L                                                    │
    diff_R = svf_R_b(dry_OS_R) − dry_OS_R                                                    │
    match channel_mode_b:                                                                    │
        Stereo:  sat_L = saturate[algo_b](diff_L · drive_k)                                  │
                 sat_R = saturate[algo_b](diff_R · drive_k)                                  │
                 routed_L_b = sat_L · enable_b                                               │
                 routed_R_b = sat_R · enable_b                                               │
                 routed_boost_L_b = diff_L · enable_b                                        │
                 routed_boost_R_b = diff_R · enable_b                                        │
        Mid:     m_diff = (diff_L + diff_R) / 2                                              │
                 m_sat = saturate[algo_b](m_diff · drive_k)                                  │
                 routed_L_b = routed_R_b = m_sat · enable_b                                  │
                 routed_boost_L_b = routed_boost_R_b = m_diff · enable_b                     │
        Side:    s_diff = (diff_L − diff_R) / 2                                              │
                 s_sat = saturate[algo_b](s_diff · drive_k)                                  │
                 routed_L_b =  s_sat · enable_b                                              │
                 routed_R_b = −s_sat · enable_b                                              │
                 routed_boost_L_b =  s_diff · enable_b                                       │
                 routed_boost_R_b = −s_diff · enable_b                                       │

wet_L = Σ_b routed_L_b                                                                       │
wet_R = Σ_b routed_R_b                                                                       │
                                                                                              │
if de_emph:                                                                                  │
    wet_L −= Σ_b routed_boost_L_b                                                            │
    wet_R −= Σ_b routed_boost_R_b                                                            │
                                                                                              │
output_OS_L = dry_amp(mix) · dry_OS_L + wet_amp(mix) · wet_L                                ◄┘
output_OS_R = dry_amp(mix) · dry_OS_R + wet_amp(mix) · wet_R

output ─► downsample×N ─► output_gain ─► output(L,R)
```

Where:

```
dry_amp(m) = clamp(2·(1−m), 0, 1)   # 1.0 for m∈[0, 0.5], ramps 1.0 → 0.0 over [0.5, 1.0]
wet_amp(m) = clamp(2·m,     0, 1)   # 0.0 → 1.0 over [0, 0.5], 1.0 for m∈[0.5, 1.0]

drive_k ∈ {Easy: 0.6, Standard: 1.0, Crush: 2.0}      # exact values TBD during DSP tuning
```

### M/S routing rationale

The saturator (nonlinear) sees the *post-routing* signal so that "Side" mode actually saturates the side content. SVFs (linear) run on L/R per-channel for clean state continuity across mode changes; because SVFs are linear, `(svf(L) + svf(R))/2 == svf((L+R)/2)` and similarly for differences, so the L/R diffs are first computed and then routed before saturation without loss of fidelity. Mid and Side modes only invoke the saturator function once per band (cheaper than Stereo's two calls).

### Key invariants

- Bands are pure-parallel SVFs. No cascading. Each band has SVF state for L and R independently. Mode changes do not invalidate filter state.
- A band at gain = 0 dB produces `boost = 0` → saturator input is 0 → saturator outputs 0 → no contribution. This is the load-bearing invariant the diff-trick relies on. Requires the SVF formulation to satisfy `H(z) = 1` analytically (not just approximately) at gain = 0 dB. Verified both algebraically (in code review of coefficient formulas) and by integration test.
- Drive is multiplicative on the routed diff before the saturator function. Per-band gain shapes the diff *amplitude and spectral shape*; drive only multiplies the amplitude.
- M/S routing happens *before* saturation but *after* the SVF (which is linear, so this commutes with the route). For "Side" mode this means the saturator actually operates on the side component of the EQ boost — sonically what users expect.
- De-emphasis subtraction uses the same M/S routing as the wet path. Because routing is linear and applied identically to both `sat_b` and `boost_b`, the cancellation `Σ routed_sat_b − Σ routed_boost_b == Σ routed(sat_b − boost_b)` works correctly per channel mode.
- Mix uses Spectre's two-stage piecewise curve. At 50%, dry stays at full level while the saturation harmonics also contribute at full level. At 100%, dry is gone and only saturation harmonics remain — useful for auditioning what the plugin is generating.
- **Headroom is the user's responsibility.** At mix=50% with multiple hot bands, the wet sum can be many dB above dry; combined with a full-level dry, the output can clip the downsampler. The user manages headroom via the Output knob and the Input/Output Link toggle (which automatically compensates Output as Input drives harder). No safety clipper is inserted.
- `input_gain` is applied *before* the upsampler. `output_gain` is applied *after* the downsampler. With Input/Output Link engaged, `output_gain = −input_gain` (in dB) — the host sees a constant overall level as drive changes.
- Latency = whatever the linear-phase polyphase upsampler/downsampler introduces. Reported via `set_latency_samples()` on init and on every Quality change. Note: VST3 hosts vary in their handling of mid-session latency changes; we accept this and document the host caveat in user-facing docs. CLAP is glitch-free.

### Per-band saturation algorithms

| Algorithm | Symmetry | Character | Notes |
|-----------|----------|-----------|-------|
| Tube | symmetric | valve-style soft clip | most versatile; default |
| Tape | asymmetric (slight) | punchier, muffled | bass/kick character; warmer top rolloff |
| Diode | symmetric | soft clip with extra high-freq harmonics | similar to tube but brighter |
| Digital | symmetric | hard clip at ±1 | clean clip-style distortion |
| Class B | symmetric, dead-zone | crossover distortion | percussive/transient material |
| Wavefold | symmetric | west-coast wavefolder | signature algorithm; bouncing peaks generate complex harmonic series; sonically distinct from any clipping shape |

Each is a pure `fn(x: f32, drive: f32) -> f32` with no internal state. Drive scales the input before the shaper.

### Smoothing

- All continuous per-band parameters (freq, gain, Q) use `nih-plug` Smoother with ~10–30 ms ramp. Frequency smoothing is in cents-space (log) to avoid pitch jumps on big drags.
- Discrete params (algorithm, channel mode, drive selector) crossfade over a few ms to avoid clicks. Implementation: maintain two saturator outputs (old and new algo / channel mode / drive value) for the duration of the crossfade ramp; linearly blend. The crossfade applies *only* to the saturator branch — the boost path used for de-emphasis is computed from the (linear) diff and does not depend on drive or algorithm, so no crossfade applies there.
- Per-band enable toggle ramps the band's effective gain → 0 over ~5 ms before disabling its compute path.
- The GUI displays the *target* parameter value on the dot label (so the dot follows the cursor immediately). The audio engine sees the smoothed value, lagging by up to ~30 ms. This is the standard tradeoff; the discrepancy is inaudible in practice.

## Module / File Layout

```
six-pack/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Plugin struct, Params, process(), wire-up
│   ├── main.rs             # Standalone binary entry
│   ├── svf.rs              # SVF biquad + low-shelf/peak/high-shelf coefficient calc
│   ├── saturation.rs       # 6 algorithms as fn(f32, drive) -> f32 + tests
│   ├── oversampling.rs     # Polyphase up/down (factors 4/8/16); ratio-aware
│   ├── spectrum.rs         # Input FFT analyzer: ring buffer + throttled FFT + atomic bins
│   ├── bands.rs            # Per-band state: SVF + sat + smoothing + M/S routing
│   ├── editor.rs           # softbuffer + baseview editor, hit testing, drag, resize
│   ├── editor/
│   │   ├── curve_view.rs   # Frequency-response curve + spectrum overlay + 6 dots
│   │   ├── band_labels.rs  # 6-column label grid (freq/gain/Q/algo/M-S)
│   │   └── bottom_strip.rs # Input/Output/Mix dials, Quality/Drive/De-Emphasis selectors
│   └── fonts/DejaVuSans.ttf
└── (tests inline in #[cfg(test)] modules per existing convention)
```

### Module roles

| File | Role |
|------|------|
| `lib.rs` | Plugin struct, Params, process() loop, parameter wire-up |
| `main.rs` | Standalone binary entry (`nih_export_standalone!`) |
| `svf.rs` | SVF biquad + coefficient computation for low-shelf, peak, high-shelf |
| `saturation.rs` | Six pure-function algorithms + harmonic-content unit tests |
| `oversampling.rs` | Linear-phase polyphase up/down for factors 4 / 8 / 16 |
| `spectrum.rs` | Input FFT analyzer: ring buffer, throttled FFT, atomic bin storage |
| `bands.rs` | Per-band state: SVF coefs + state, M/S routing, smoother handles |
| `editor.rs` | softbuffer + baseview editor lifecycle, hit testing, drag, resize |
| `editor/curve_view.rs` | Frequency-response composite curve + spectrum analyzer overlay + 6 draggable dots |
| `editor/band_labels.rs` | 6-column label grid (freq / gain / Q / algo / channel mode) |
| `editor/bottom_strip.rs` | Input/Output/Mix dials, Quality / Drive / De-Emphasis selectors |

### Reuse from existing crates

- `tiny-skia-widgets`: `draw_rect`, `draw_dial`, `draw_button`, `draw_stepped_selector`, `EditorState`, `SurfaceState`, `DragState`, `TextEditState`, `TextRenderer`. Right-click text entry, cursor tracking, free resize, and click handling are inherited.
- `nih-plug` (workspace pin to fork) for parameter system, editor lifecycle, and host integration.
- `rustfft` (already used by satch and warp-zone) for the spectrum analyzer FFT.

### Why a single crate?

All DSP is plugin-specific. Following existing pattern. If wavefold or saturation primitives prove useful in another plugin later, they can be lifted into a shared crate.

## Data Flow & Shared State

### Audio thread (`process()`)

Allocations are forbidden. Locks are forbidden.

```
For each block:
  1. Read input_gain, output_gain, mix, drive_k, deemph, quality smoothed values
  2. Pre-multiply input(L,R) by input_gain
  3. Upsample input × N (linear-phase polyphase) into pre-allocated scratch
  4. For each oversampled sample s in [0..frames*N]:
        dry_L = upsampled_L[s]
        dry_R = upsampled_R[s]
        wet_L = wet_R = 0
        boost_L = boost_R = 0   # accumulators for de-emph subtraction
        For each band b in 0..6:
            update_svf_coefs_if_dirty(b)    # only if smoothed param changed
            diff_L = svf_L[b].process(dry_L) - dry_L
            diff_R = svf_R[b].process(dry_R) - dry_R
            match channel_mode[b]:
                Stereo: sat_L = saturate[algo_b](diff_L * drive_k)
                        sat_R = saturate[algo_b](diff_R * drive_k)
                        wet_L += sat_L * enable_b
                        wet_R += sat_R * enable_b
                        boost_L += diff_L * enable_b
                        boost_R += diff_R * enable_b
                Mid:    m_diff = (diff_L + diff_R) * 0.5
                        m_sat = saturate[algo_b](m_diff * drive_k)
                        wet_L += m_sat * enable_b
                        wet_R += m_sat * enable_b
                        boost_L += m_diff * enable_b
                        boost_R += m_diff * enable_b
                Side:   s_diff = (diff_L - diff_R) * 0.5
                        s_sat = saturate[algo_b](s_diff * drive_k)
                        wet_L +=  s_sat * enable_b
                        wet_R += -s_sat * enable_b
                        boost_L +=  s_diff * enable_b
                        boost_R += -s_diff * enable_b
        if deemph:
            wet_L -= boost_L
            wet_R -= boost_R
        out_L_s = dry_amp(mix) * dry_L + wet_amp(mix) * wet_L
        out_R_s = dry_amp(mix) * dry_R + wet_amp(mix) * wet_R
  5. Downsample × N into output buffer (matching linear-phase polyphase)
  6. Apply output_gain to output(L,R)
  7. Push original input samples (post-input_gain, pre-OS) into spectrum mono ring;
     if accumulated count ≥ 1024 (offset by per-instance random phase), run FFT
     and write atomic magnitude bins. (No effect on reported latency.)
```

### GUI thread (60 FPS frame paint)

```
On each redraw:
  1. Read latest atomic spectrum bins (lock-free)
  2. Read all param values (Smoother::current() / .value())
  3. Repaint:
       - Background (faded spectrum analyzer fill behind the EQ display)
       - 6-band composite curve (sum of band magnitude responses on a log freq axis)
       - 6 draggable dots (one per band)
       - Per-band label column (freq / gain / Q / algo / channel mode)
       - Bottom strip (Input/Output/Mix dials, Quality / Drive / De-Emphasis labels)
       - Right-click edit caret if active
```

### Shared state between threads

| State | Direction | Mechanism |
|-------|-----------|-----------|
| Parameter values | Host/GUI → Audio | nih-plug Smoother (lock-free internally) |
| Parameter writes | GUI → Host | `GuiContext::{begin,set,end}_set_parameter` |
| Spectrum bins | Audio → GUI | `[AtomicU32; N_BINS]` (f32 stored as bit pattern; same approach as warp-zone's spectral display) |
| Editor size | GUI ↔ persistence | `EditorState` from tiny-skia-widgets |
| Pending host resize | Host → GUI | Packed `AtomicU64` (width:u32, height:u32) |

### Spectrum analyzer

- 2048-point FFT, Hann window. Audio thread maintains a 2048-sample mono ring; every 1024 samples accumulated, runs a single FFT and writes atomic magnitude bins. Mono is derived as `(L + R) · 0.5` per sample at push time — it's a display feed, not a level meter, so a simple sum-and-halve is fine.
- Rate is independent of block size: ~43 Hz at 44.1 kHz, ~188 Hz at 192 kHz.
- 128 magnitude bins (log-spaced) stored as `AtomicU32` (f32 bit pattern). GUI smooths visually with EMA decay (~250 ms).
- `rustfft::FftPlanner` planner is created once in `initialize()`; in-place scratch reused. (Same plumbing as satch's `spectral.rs`.)
- Audio-thread CPU cost per instance: ~0.05% at 44.1k, ~0.2% at 192k.
- **Phase staggering across instances:** the per-1024-sample throttle counter is initialized to a per-instance random offset at `initialize()` time. With many Six Pack instances open in a single session this prevents synchronized FFT spikes (which otherwise all fire on the same audio buffer boundary).
- The spectrum push (audio thread step 7) happens *after* the output is written and does not affect reported plugin latency.

## Error Handling, Edge Cases, and Audio-Thread Safety

### Numerical hazards

- **NaN/Inf guards on saturation outputs.** All six saturator functions are written to be NaN-free for finite inputs. Wavefold uses bounded triangle-wave math (closed-form `mod` arithmetic, not iterative folding). Each algorithm has a unit test feeding `[0, ±ε, ±0.5, ±1.0, ±10, ±100, ±1e9, denormal_min]` and asserting `is_finite(output)`.
- **Wavefold input pre-clip.** At very large drive × diff combinations the f32 modulo loses precision (~30 bits at `x = 1e9`). The wavefold function pre-clips its input to ±64 before folding so the output remains numerically meaningful even at extreme settings. Documented in code; verified by test. Note: at inputs near the ±64 clip the wavefold output is dense aliased content (many fold cycles per sample, well above Nyquist even with 16× OS). This is the algorithm's intentional sonic ceiling, not a bug — the combination of Q=10 + Crush + max gain is meant to be destructive. The pre-clip just keeps it bounded and finite.
- **SVF analytical unity at 0 dB.** The chosen SVF formulation must reduce to `H(z) = 1` exactly when gain = 0 dB (not just within float epsilon). This is the load-bearing identity for the diff-trick. Audio EQ Cookbook peaking/shelf forms satisfy this when the linear gain ratio = 1; TPT SVF requires the same care. Verified algebraically during coefficient implementation, not just by integration test.
- **Transient overshoot at high Q.** SVF impulse response can transiently exceed steady-state magnitude. At Q = 10 with a sharp transient, the diff peak can exceed the steady-state +18 dB nominal. Saturators must remain NaN-free at any input — the existing NaN test covers this with a ±1e9 sweep, plus a dedicated transient-response test (impulse with high-Q peak filter; assert `is_finite` over the full impulse response).
- **Denormal protection** at audio thread entry: set FTZ/DAZ flags via `_MM_SET_FLUSH_ZERO_MODE` (wrapped in `#[cfg(target_arch = "x86_64")]`) once at `initialize()` time. On other architectures (e.g., ARM/Apple Silicon), use the equivalent `FPCR.FZ` setting via the appropriate intrinsic, or rely on the no-denormal claim from nih-plug if it's already handled.
- **SVF coefficient validity.** When freq approaches Nyquist or Q approaches 0, SVF coefficients can blow up. Clamps (these match the parameter ranges; the clamp is defense-in-depth in case a smoother momentarily produces an out-of-range intermediate):
  - `freq ∈ [20 Hz, min(20 kHz, 0.49 × sample_rate × N_OS)]`
  - `Q ∈ [0.1, 10.0]`
  - `gain ∈ [0 dB, +18 dB]` (boost-only enforced by parameter range, not by additional clamp)
- **Parameter ramping bounds.** All Smoothers reset their target on `reset()`. Frequency ramping is done in log-space (cents).

### Structural edge cases

- **All bands at 0 dB:** `boost = 0` everywhere, `wet_L = wet_R = 0`, output = `dry_amp(mix) · dry`. Equals input (modulo input_gain/output_gain) when mix ≤ 50%; fades to silence over [50%, 100%]. Verified by integration test asserting `output == dry_amp(mix) · dry` for sweep of mix.
- **Mono input → stereo output** (host preference): two independent processing paths. With duplicated mono input (L == R), Side mode produces zero (since L − R = 0). Stereo and Mid modes produce equal output on both channels. Verified by integration test.
- **Sample-rate change mid-session.** `reset()` recomputes SVF coefficients, polyphase OS coefficients, and FFT bin spacing. Latency reported via `set_latency_samples()` on every change.
- **Block-size change.** Scratch buffers sized at `initialize()` based on `BufferConfig::max_buffer_size * MAX_OS_FACTOR`. nih-plug guarantees `process()` blocks won't exceed this.
- **Host bypass.** nih-plug's bypass mechanism passes input through unchanged. The plugin still reports its current latency (so the host can compensate for delay-line round-trips when toggling bypass). Per-band enable toggles are a separate user-facing concern: they ramp the band's effective gain → 0 before disabling its compute path.
- **Quality changes mid-session.** `set_latency_samples()` is called when Quality changes. CLAP hosts handle this glitch-free; some VST3 hosts may produce a brief pop during the host's PDC re-resolve. Documented in user-facing docs; not engineered around.

### Allocation discipline (audio thread)

- All `Vec`s pre-sized at `initialize()`; mutated only via index access in `process()`.
- No `String` formatting on the hot path — parameter `value_to_string` runs only on GUI thread.
- `rustfft` planner is created once in `initialize()`; each subsequent FFT call reuses the pre-allocated complex scratch buffer.
- No `try_lock` on the audio thread — all shared state with GUI is via atomics or nih-plug Smoothers.

### What we do NOT validate (per project's "no impossible-scenario validation" rule)

- We don't check that the host-provided sample rate is positive (nih-plug guarantees it).
- We don't check that buffer pointers are non-null in `process()` (host contract).

### NaN escape detection

After every block, the final output samples are checked for `is_finite()` in **debug builds only** via `debug_assert!`. Release builds have zero overhead. If a NaN ever escapes, that's a hard bug — we want it to fail loudly during dev, not be silently masked.

## Testing Strategy

Inline `#[cfg(test)]` modules per existing convention. Run with `cargo nextest run --workspace`.

### Saturation algorithms (`saturation.rs`) — ~30 tests

- Each algorithm: NaN-free for `[0, ±ε, ±0.5, ±1.0, ±10, ±1e9, denormal_min]`.
- Each algorithm: `f(0) == 0` (no DC injection from a silent input).
- Symmetric algos (Tube, Diode, Class B, Wavefold): `f(-x) == -f(x)` within epsilon.
- Asymmetric algos (Tape): bias direction matches manual description.
- Class B: crossover dead zone at low amplitudes (`|f(0.01)| < |f(0.5)|/100`).
- Wavefold: monotonic in fold count as drive increases (sweep drive on `sin`, count zero crossings — should rise).
- Digital: hard ceiling at ±1.0 ± epsilon.
- Each algorithm: gain at quiet input matches expected linear region (`f(0.001) ≈ k · 0.001` for soft-clip family).

### SVF biquads (`svf.rs`) — ~15 tests

- Low-shelf, peak, high-shelf: magnitude at center freq matches gain in dB ± 0.1 dB.
- 0 dB gain on any filter: `output == input` to within float epsilon → `boost = 0` (the load-bearing invariant).
- Filter stability: 1 second of white noise produces bounded output for all freq/Q corners.
- Q sweep: bandwidth matches `BW = freq / Q` at 6 dB peak gain.
- Coefficient recompute idempotency: same params → same coefficients.

### Oversampling (`oversampling.rs`) — ~10 tests

- Round-trip identity: `down(up(x)) == x` to within reconstruction tolerance for sine, square, impulse.
- Passband flatness < 0.1 dB up to 0.45 × Nyquist.
- DC: zero in → zero out.
- All factors {4, 8, 16}: tested independently.

### Bands integration (`bands.rs`) — ~15 tests

- 0 dB band: `diff_L == diff_R == 0` (within epsilon) — validates the diff-trick.
- Stereo mode: `routed_L = sat(diff_L · drive_k)`, `routed_R = sat(diff_R · drive_k)`.
- Mid mode: `routed_L == routed_R == sat((diff_L + diff_R)/2 · drive_k)`.
- Side mode: `routed_L == sat((diff_L − diff_R)/2 · drive_k)`, `routed_R == −routed_L`.
- Drive scaling: `drive_easy < drive_standard < drive_crush` (numeric ordering).
- Algorithm crossfade on change: smooth transition over the configured ramp (no clicks).

### Mix curve & de-emphasis (`lib.rs`) — ~10 tests

- `dry_amp(0%) == 1.0`, `wet_amp(0%) == 0.0`.
- `dry_amp(50%) == 1.0`, `wet_amp(50%) == 1.0` (the "both at max" invariant).
- `dry_amp(100%) == 0.0`, `wet_amp(100%) == 1.0`.
- Continuous monotonicity of each amp curve.
- De-emphasis off, trivial-saturation limit: `output == dry + wet_amp(mix) · Σ boost`.
- De-emphasis on, trivial-saturation limit: `output == dry_amp(mix) · dry` (the boost cancels the wet, leaving only the dry-attenuated path).
  - At mix ≤ 50%: this equals `dry` exactly.
  - At mix > 50%: this is a fade-out of dry, by design — at mix=100% the output is silence at trivial-sat (no harmonics, no dry).
- Per-channel-mode de-emphasis cancellation: confirm that for trivial-saturation, `wet_L − Σ routed_boost_L_b == 0` for each channel mode (Stereo, Mid, Side).
- Side-mode de-emphasis cancellation specifically on R: at trivial-sat, `wet_R = −s_diff` and `boost_R = −s_diff`, so `wet_R − boost_R == 0`. The double-negative ensures the cancellation works on the inverted-side channel.

### Plugin integration (`lib.rs`) — ~10 tests

- All bands at 0 dB, mix ≤ 50%: output equals input (within float tolerance), independent of de-emphasis state. (At mix > 50%, output equals `dry_amp(mix) · input`.)
- Single peak at +9 dB on a sine input at peak freq: output FFT shows expected harmonic structure (2×, 3×, 4× peaks).
- Sample-rate sweep: 44.1k / 48k / 96k / 192k all stable, no NaN, no clipping above ceiling.
- Block-size sweep: 1, 16, 64, 1024, 4096 samples — output RMS error < -90 dB vs. the block-size = 1 reference (smoothing-per-buffer-end and OS scratch-reuse mean we cannot guarantee bit-identical, but we can guarantee inaudibly-equivalent).

### Spectrum analyzer (`spectrum.rs`) — ~5 tests

- 1 kHz sine input → bin nearest 1 kHz peaks; neighbors lower (within window's mainlobe width).
- DC input → bin 0 only.
- Atomic store/load round-trip: f32 → bit pattern → f32 == identity.

**Total target: ~95–105 tests** (sum of categories above; in line with satch=46, gs-meter=62, pope-scope=113).

CI runs `cargo nextest run --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo fmt --check`.

## Defaults and Parameter Ranges

### Per-band defaults

| Slot | Type | Default freq | Default gain | Default Q | Default algo | Default channel |
|------|------|--------------|--------------|-----------|--------------|-----------------|
| 1 | Low-shelf | 60 Hz | 0 dB | 0.71 | Tube | Stereo |
| 2 | Peak | 180 Hz | 0 dB | 0.71 | Tube | Stereo |
| 3 | Peak | 540 Hz | 0 dB | 0.71 | Tube | Stereo |
| 4 | Peak | 1.6 kHz | 0 dB | 0.71 | Tube | Stereo |
| 5 | Peak | 4.8 kHz | 0 dB | 0.71 | Tube | Stereo |
| 6 | High-shelf | 12 kHz | 0 dB | 0.71 | Tube | Stereo |

### Per-band ranges

- **Frequency:** 20 Hz – 20 kHz, log-skewed slider, smoothed in cents-space.
- **Gain:** 0 to +18 dB. Boost-only.
- **Q:** 0.1 to 10.0, log-skewed (so 0.71 sits near center).
- **Algorithm:** enum (Tube / Tape / Diode / Digital / Class B / Wavefold).
- **Channel:** enum (Stereo / Mid / Side).
- **Enable:** bool, default true. Toggled by clicking the filter icon at the top of each band column or by double-clicking the band's dot.

### Global parameters

| Param | Range | Default | Notes |
|-------|-------|---------|-------|
| Input gain | -24 → +24 dB | 0 dB | |
| Output gain | -24 → +24 dB | 0 dB | |
| Input/Output Link | bool | off | When on, output = −input dB |
| Mix | 0% → 100% | 50% | Two-stage curve (Spectre-style) |
| Quality (OS) | enum {Off, 4×, 8×, 16×} | Off | Off keeps default low-CPU |
| Drive | enum {Easy, Standard, Crush} | Standard | Multiplicative on the diff |
| De-Emphasis | bool | **On** | Mirrors Spectre's default; harmonics-only character is the plugin's identity |

### Latency (estimated; refined during implementation)

Linear-phase polyphase up/down filters; latency is approximately one filter group delay each way (filter half-length).

| Quality | Latency (rough estimate) |
|---------|--------------------------|
| Off | 0 samples |
| 4× | ~32 samples (~0.7 ms @ 44.1k) |
| 8× | ~64 samples (~1.5 ms @ 44.1k) |
| 16× | ~128 samples (~2.9 ms @ 44.1k) |

> **Caveat — these are placeholders.** Cascaded half-band designs (the typical implementation) scale roughly with `log₂(N)` rather than linearly with `N`, and depend on tap count and stopband-rejection target. Real-world designs (e.g., r8brain, hiir half-band cascades) commonly land at ~50–100 samples for 4× and ~100–200 for 16×. The numbers above are working estimates; the actual implementation will measure them and update both the spec and the value reported via `set_latency_samples()`.

Reported via `set_latency_samples()` on init and on every Quality change. CLAP hosts handle runtime latency changes glitch-free; some VST3 hosts may produce a brief audible artifact during the host's PDC re-resolve. Documented in user-facing docs; not engineered around.

### GUI / window

- Default size: **720 × 500 px**. Free resize (50% – 200%). Size persisted via `EditorState`.
- Color palette: deep navy + magenta-to-cyan band gradient. Each band has its own static hue along that gradient. Final palette to be chosen during implementation; non-load-bearing.
- Live spectrum overlay: faded fill behind the EQ curve. ~250 ms EMA decay. 128 log-spaced bins.
- Interactions:
  - Drag a band dot for freq + gain.
  - Scroll wheel on a dot adjusts Q.
  - Double-click a dot to enable / disable that band.
  - Right-click any numeric label to type-edit (TextEditState pattern).
  - Modifier-key drag: fine-tune.
  - Modifier-key while clicking a band's algorithm or channel selector: applies the selection to all six bands at once (Spectre parity).

### Preset system

- Use nih-plug's built-in state save/load. Hosts handle preset chunks. No bundled preset library at v1; can be added in a follow-up.

## Build & Install

Mirrors existing plugins:

```bash
# Plugin bundles (VST3 + CLAP)
cargo nih-plug bundle six-pack --release

# Standalone binary
cargo build --bin six-pack --release

# Debug standalone (for GUI testing without DAW)
cargo build --bin six-pack
```

README updated with a "Six Pack" entry under the existing alphabetical plugin list. CLAUDE.md gets a Six Pack section under "Plugins", "Architecture", and "Key Design Decisions".

## Open Questions / Future Work (non-blocking for v1)

- **Drive multipliers.** Numeric values for Easy/Standard/Crush are placeholders (0.6 / 1.0 / 2.0) and will be tuned during DSP implementation against ear tests on representative material.
- **Bundled presets.** Not in v1; a small starter library (vocal air, drum bus, master) can ship in a follow-up.
- **Color palette.** Final palette chosen during implementation. The spec only constrains "deep navy + magenta-to-cyan band gradient".
- **SVF coefficient form.** We will choose between Audio EQ Cookbook biquads, Chamberlin SVF, and Andy Simper's TPT SVF during implementation. The chosen form **must** satisfy `H(z) = 1` analytically at gain = 0 dB (load-bearing for the diff-trick). Cookbook peaking and shelf forms satisfy this when `A = 10^(gain_dB/40) = 1` (the b- and a-coefficients become identical). TPT SVF in its standard "dry + (gain−1)·bandpass" mix form satisfies this *structurally* — at gain=1 the bandpass term is multiplied by zero, so `H(z) = 1` regardless of state. Verified by code review of the coefficient formula, not just by integration test. TPT SVF is currently the most likely choice (warp-free at high Q, modulation-friendly, well-suited to the smoothed-parameter use case).
- **Linear-phase polyphase filter design.** Half-band cascaded design vs. single-stage linear-phase FIR; impulse-response length tuning. We'll choose during implementation, optimizing for transient response and CPU cost. Latency numbers in the table above are estimates and will be replaced with measured values.

## Non-goals

- No conventional EQ behavior (no cuts; this is a saturator that uses EQ as a sculptor for harmonic placement).
- No sidechain input.
- No tempo-synced modulation.
- No GPU rendering. CPU only, via softbuffer + tiny-skia.
- No bundled preset library at v1.
- No "Clean" passthrough algorithm. Users wanting a parallel boosting EQ should use a real EQ.
