# Multosis Spectral Effects Family — Design

**Date:** 2026-05-24
**Goal:** Add the 14 spectral effects from the Infiltrator manual (pp. 21–24) as
new `EffectKind` variants in multosis, sharing a single FFT engine in
`tract-dsp`.

## Background

Multosis already exposes 22 per-track effects via the `Effect` trait. Several
are FFT-based (`warp_zone`, `satch`, `stretch`, `vocoder`, `pitch_shift`,
`frequency_shift`) and each rolls its own STFT scaffolding on top of the shared
`tract_dsp::stft_analysis::StftAnalyzer`. We want to add 14 more spectral
effects, modelled on Infiltrator's spectral page, with shared scaffolding to
keep the per-effect file short and consistent.

Each effect appears in the per-row effect dropdown alongside the existing 22.
The result is a "spectral family" inside multosis — no new plugin crates.

## Non-Goals

- No new plugin crates. Everything lands in the existing `multosis` and
  `tract-dsp` crates.
- No per-effect Mix or Output Gain controls — multosis already provides
  per-track Mix and the amp MSEG handles level.
- No GUI changes beyond appearing in the existing per-row effect dropdown and
  the standard parameter-dial layout.
- No support for FFT sizes outside {512, 1024, 2048, 4096}.
- No latency compensation tricks — the effect reports its current latency via
  `Effect::latency_samples()`; modulating FFT size produces PDC drift, which
  the user has explicitly accepted as a sound-design trade-off.

## Architecture

### `tract_dsp::spectral_engine::SpectralEngine` (new)

A new feature-gated module in `tract-dsp/src/spectral_engine.rs` providing
audio-thread-safe per-channel STFT analysis/synthesis with switchable FFT size.

```text
SpectralEngine {
    slots: [SpectralSlot; 4],   // FFT sizes 512 / 1024 / 2048 / 4096
    active: usize,              // index into slots
    sample_rate: f32,
}

SpectralSlot {
    fft_size:     usize,
    hop_size:     usize,        // = fft_size / 2 (50% overlap)
    analyzer:     StftAnalyzer, // existing tract-dsp primitive
    ifft:         Arc<dyn Fft<f32>>,
    output_ring:  Vec<f32>,     // overlap-add accumulator (fft_size long)
    output_pos:   usize,
    hop_counter:  usize,
    spectrum:     Vec<Complex<f32>>,
}
```

All four slots are allocated at `new(sample_rate)` — switching FFT sizes is
zero-allocation on the audio thread. Memory cost is ~30 KB/channel for the four
slots combined (4096 × 8 bytes complex + 4096 × 4 bytes float overlap ring +
ring/scratch for the smaller sizes); per multosis the instance-count target is
relaxed so this is acceptable.

**Public API:**

```text
fn new(sample_rate: f32) -> Self
fn set_fft_size(&mut self, fft_size: usize)   // takes effect next hop
fn fft_size(&self) -> usize
fn latency_samples(&self) -> usize             // = active slot's hop_size
fn reset(&mut self)
fn process_sample<T: SpectralTransform>(&mut self, input: f32, t: &mut T) -> f32

trait SpectralTransform {
    fn transform(
        &mut self,
        spectrum: &mut [Complex<f32>],
        fft_size: usize,
        sample_rate: f32,
    );
}
```

`process_sample` does: write input → active slot's analyzer; on hop boundary,
analyze → call `t.transform(...)` → inverse FFT → window → overlap-add into the
output ring; advance hop counter; pull and return one output sample.

`SpectralTransform` is a trait method rather than a closure so per-effect state
(bin delay lines, RNGs, last-frame magnitudes) lives naturally on the effect
struct, not in juggled closure captures.

**FFT-size switching:** `set_fft_size` latches the new active slot. The new
slot starts with cold rings; for ~`hop_size` samples after the switch its
output ring drains the prior content as the new analyzer fills. This produces
the "modulated FFT size sounds bad" effect the user accepted as their
responsibility.

**Hop ratio:** 50% (`hop = fft_size / 2`), matching `tract_dsp::stft::StftConvolver`
and the periodic-Hann analysis window's natural COLA point. The one effect
that genuinely benefits from 75% overlap (SpectralStretch — phase vocoder)
holds its own analyzer at `hop = fft_size / 4` rather than using `SpectralEngine`
for its analysis; everything else uses the engine.

### Multosis integration — per effect

Each new effect lives at `multosis/src/effects/spectral_<name>.rs` with this
shape:

```rust
pub struct SpectralFooEffect {
    sample_rate: f32,
    // Per-effect param cache (linear unit conversions, etc.)
    // Per-effect state (bin delay lines, last-frame magnitudes, RNG, ...).
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralTransform for SpectralFooEffect {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], n: usize, sr: f32) {
        // per-bin math for this effect
    }
}

impl Effect for SpectralFooEffect {
    fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l_out = self.engine_l.process_sample(l, &mut self);
        let r_out = self.engine_r.process_sample(r, &mut self);
        (l_out, r_out)
    }
    fn parameters(&self) -> &'static [ParamSpec] { &Self::PARAMS }
    fn set_param(&mut self, i: usize, v: f32) { ... }
    fn set_sample_rate(&mut self, sr: f32) { ... }
    fn latency_samples(&self) -> usize { self.engine_l.latency_samples() }
    fn reset(&mut self) { self.engine_l.reset(); self.engine_r.reset(); }
}
```

The borrow on `&mut self` for both `engine_*.process_sample(..., &mut self)` is
not legal as written; the actual implementation splits per-channel state so the
trait impl is on a `SpectralFooState` struct (one per channel) rather than the
whole effect, and the engine owns `SpectralFooState` per channel. This split is
small and well-precedented (warp-zone follows the same shape internally).

### FFT-size param

Slot 0 of every spectral effect's `parameters()` array:

```rust
ParamSpec {
    name: "FFT",
    min: 0.0, max: 3.0, default: 2.0,  // default = 2048
    scaling: ParamScaling::Stepped(4),
    format: ParamFormat::Enum(&["512", "1024", "2048", "4096"]),
}
```

`set_param(0, v)` snaps to `[512, 1024, 2048, 4096][v as usize]` and calls
`engine_l.set_fft_size(size)` / `engine_r.set_fft_size(size)`.

### Boilerplate per effect (existing pattern)

Each new effect requires edits to `multosis/src/effects/mod.rs`:

1. Append variant to `EffectKind`.
2. Append variant to `EffectKind::ALL` array.
3. Append arm to `default_params_for_kind`.
4. Append variant to `EffectInstance`.
5. Append dispatch arm in each of the 6 `EffectInstance` methods:
   `set_sample_rate`, `reset`, `parameters`, `process_sample`, `set_param`,
   `latency_samples`.
6. Bump `EffectKind::ALL.len() == 22` test assertion to **36**.

All mechanical and follows the pattern the existing 22 effects already use.

## Per-Effect Catalog

All effects share `params[0] = FFT size`. Subsequent params shown below.

| # | Effect | Params 1..N | DSP outline |
|---|---|---|---|
| 1 | **SpectralShift** | Scale (0.5..2.0), Translate (±100% Nyquist) | Per output bin k, source bin = (k − translate_bins) / scale; linear interp between floor/ceil; mag carried, phase carried. Out-of-range bins zeroed. |
| 2 | **SpectralRotate** | Shift (±100% Nyquist) | Output bin k = input bin ((k − shift_bins) mod (N/2)). Wraps (vs. Shift's zero). |
| 3 | **SpectralTwist** | Freq (20–20k Hz), Twist (±100%), Bandwidth (0.1–4 oct) | Within ±bw/2 oct of Freq: scale each bin's distance-from-center by (1−twist). twist=+1 collapses band onto Freq; twist=−1 doubles its spread. Out-of-band passes. |
| 4 | **SpectralMirror** | Freq, Bandwidth | Within band: bin at +d above Freq swaps with bin at −d below. Magnitude mirrored, phase reflected (conjugate). |
| 5 | **SpectralBandpass** | Freq, Bandwidth | Brickwall: zero all bins outside [Freq·2^(−bw/2), Freq·2^(bw/2)]. No smoothing. |
| 6 | **SpectralStretch** | Speed (0.25–4×), Tempo (1–100% capture-rate), Chaos (0–100%) | Phase vocoder. Speed scales synthesis hop; Tempo throttles new analysis frames; Chaos adds random angle to per-bin phase. Holds own 75%-overlap analyzer. |
| 7 | **SpectralScatter** | Length (10–2000 ms), Feedback (0–95%), Rate (0.1–10 Hz) | Per-bin complex delay line, depth = Length/hop. delay_for_bin[k] = uniform_random(0..Length); resampled at Rate. Output = delayed[k] + Feedback·delayed[k]. |
| 8 | **SpectralCascade** | Length, Feedback, Centre (20–20k Hz) | Per-bin delay = Length · (k − centre_bin) / (N/2). Linear ramp around centre — low centre → upward slide, high centre → downward slide. Negative delays clamp to 0. |
| 9 | **SpectralSmear** | Length (10–2000 ms), Chaos (0–100%) | Per-bin magnitude envelope: instant attack, release tau = Length. mag_out[k] = max(mag_in[k], mag_prev[k]·decay). Chaos randomises phase angle. |
| 10 | **SpectralReverb** | Time (0.1–20 s), Tone (0–100% dark↔bright) | Per-bin feedback. For each bin k: T60_k = Time · tone_curve(k, Tone), where tone_curve linearly interpolates between a HF-decay curve at Tone=0 (low-freq T60 = Time, HF T60 = 0.1·Time) and a LF-decay curve at Tone=1 (LF T60 = 0.1·Time, HF T60 = Time). Tone=0.5 is flat (all bins decay at Time). Per-bin feedback gain g_k = 10^(−3·hop_seconds/T60_k). |
| 11 | **SpectralCompress** | Amount (0–100%), Tone (−100..+100% pink↔white target) | Per-bin compression toward target spectrum. Target = pink (1/f) at −100, flat at 0, white (f) at +100. Ratio[k] = (target[k] / current[k])^Amount. Multiply each bin's magnitude by ratio. |
| 12 | **SpectralCorrupt** | Amount (−100..+100%, 0 = passthrough), Decay (0–100%) | Rank bins by magnitude. Amount>0 zeros the quietest |Amount|% of bins; Amount<0 zeros the loudest. Decay = exponential carry of last-frame gate so cuts feel less abrupt. |
| 13 | **SpectralLofi** | Factor (0–100%), Randomise (0–100%), Slow (1–100 hops) | Bitmask of kept bins, refreshed every `Slow` hop boundaries (hops = analysis frames, not samples). Randomise=0 keeps every Nth bin where N = 1/(1−Factor) (regular decimation); Randomise=100 keeps each bin with independent probability (1−Factor); lerp the two selection rules in between. Zero the rest. |
| 14 | **SpectralSpread** | Amount (0–100%) | Box-blur magnitude across bins with kernel radius r = round(Amount · 16). Amount=0 → r=0 (passthrough). Phase preserved per-bin (magnitude-only blur). Kernel kept small (≤16 bins) so the blur is detail-softening, not spectrum-smashing. |

### Interpretation choices flagged to the user

- **Twist** read as fold-band-into-itself (compression toward Freq). Alternative
  would have been rotate-within-band (circular shift of in-band bins). Picked
  fold because "twist" suggests compression to a point.
- **Cascade** read as linear-bin-delay ramp pivoting at Centre. Alternative
  was exponential. Linear matches the manual's stated direction of slide and
  is simpler.

## Testing Strategy

### Engine (tract-dsp)

`tract-dsp/src/spectral_engine.rs#tests`:

- **Impulse identity:** with an identity `SpectralTransform` (zero-op), feed
  an impulse, assert output (after `latency_samples` of padding) matches input
  within 1/N magnitude error.
- **Sine identity:** sine in → sine out at same amplitude under identity.
- **FFT-size switch:** call `set_fft_size` mid-stream, assert no panic, assert
  output is silent for one hop, then resumes producing.
- **Latency report:** `latency_samples()` matches `active slot's hop_size`
  before/after a switch.
- **Reset:** after `reset()`, all four slots' output rings + input rings are
  zeroed.

### Per-effect (multosis effects)

Each `spectral_<name>.rs#tests` contains at minimum:

- **Silence:** silence in → silence out (after latency).
- **No-op param:** find a param setting that should pass input through (e.g.
  Bandwidth=0 for Mirror, Twist=0 for Twist, Amount=0 for Compress and
  Corrupt, Factor=0 for Lofi). Assert output matches input within 1/N error
  after latency.
- **One DSP shape assertion** per effect (effect-specific): e.g. SpectralBandpass
  with narrow bw kills broadband content outside the passband; SpectralScatter
  with Length=0, Feedback=0 is passthrough; SpectralReverb post-input shows
  non-zero tail energy.

### Workspace-level

- `EffectKind::ALL.len() == 22` assertion in `multosis/src/effects/mod.rs#tests`
  bumps to **36**.
- `multosis::effects` Criterion bench loop auto-picks up the 14 new variants —
  no bench-file edits.

### CI gate after each step

`cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest
run --workspace && cargo xtask native nih-plug bundle multosis --release`.

## Build Sequence

One PR per step is overkill; one commit per logical step is appropriate. The
user authorises each commit per the project's standing rule.

1. **SpectralEngine in tract-dsp** + unit tests. Gate: tests pass, no clippy
   warnings, bundle still builds.
2. **SpectralRotate end-to-end** through multosis — new file + all six
   dispatch arms + default_params entry + ALL.len bump (22→23). Establishes the
   per-effect integration pattern and validates the engine's API under real use
   before duplicating it 13 more times.
3. **Trivial transforms** (single op per bin, no state) — one effect per step:
   SpectralBandpass → SpectralMirror → SpectralShift (Translate-only path) →
   SpectralSpread → SpectralLofi.
4. **Last-frame stateful** (one-frame magnitude carry) — one per step:
   SpectralSmear → SpectralCorrupt → SpectralCompress.
5. **Bin delay lines** (multi-frame complex scratch state) — one per step:
   SpectralCascade → SpectralReverb → SpectralScatter.
6. **Phase-aware** — one per step: SpectralShift (Scale path filled in) →
   SpectralTwist.
7. **Custom analyzer**: SpectralStretch with its own 75%-overlap phase vocoder
   path.
8. **Docs:** update `multosis/CLAUDE.md` and the workspace `CLAUDE.md` Spectral
   section (the multosis effect table); add a brief Spectral subsection to any
   per-plugin manual that lists effect kinds.

After each step the CI gate must pass before moving on. After every step that
adds an effect, `EffectKind::ALL.len() == N` is updated.

## Risks and Mitigations

- **PDC drift on FFT-size modulation.** Accepted per user. No mitigation;
  documented in the FFT-size param's tooltip / doc-comment.
- **Memory cost per instance.** Four pre-allocated FFT slots is ~30 KB/channel.
  Multosis has 16 rows × 2 channels × N spectral effects per row. Worst case
  with every row running a spectral effect: 16 × 60 KB = ~1 MB per multosis
  instance. Acceptable given multosis's relaxed instance-count target.
- **CPU at FFT=4096 across all 16 rows.** Could push the per-block budget.
  Existing `bench-suite` and the per-effect Criterion benches will quantify it
  before the user encounters it in a real session.
- **Borrow split** for two engines + shared per-effect state. Solved by the
  per-channel-state pattern warp_zone already uses; pattern is documented in
  the engine's module doc and in the first effect (SpectralRotate) so
  subsequent effects copy it correctly.

## Scope Estimate

- 1 new tract-dsp module (~250 LOC + tests).
- 14 new multosis effect files (~100–200 LOC each + tests).
- ~250 LOC of dispatch boilerplate in `multosis/src/effects/mod.rs` (14 ×
  ~18 LOC across 7 enum/arm sites).
- Total: ~3500 LOC new, all additive.

This sits at the upper end of single-spec scope. The effects are mutually
independent, so a single spec + single plan with per-effect tasks is
appropriate — no value in splitting the spec.
