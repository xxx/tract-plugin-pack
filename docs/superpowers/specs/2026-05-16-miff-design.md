# miff — Design

**Date:** 2026-05-16
**Status:** Approved
**Crate:** new — `miff/`

## Purpose

miff is a small convolution-filter plugin (VST3 / CLAP / standalone) in the
spirit of Wavetable Filter — but the kernel is not loaded from a wavetable, it
is **hand-drawn with the MSEG editor**. You draw a curve; that curve *is* the
FIR filter's impulse response. Two modes — Raw (direct convolution) and
Phaseless (STFT magnitude-only) — mirror Wavetable Filter.

## Scope

In scope:

- A new `miff/` plugin crate.
- The curve→kernel bake step.
- Raw + Phaseless convolution DSP (copied and adapted from Wavetable Filter).
- Params, persisted document state, and the softbuffer GUI (MSEG editor +
  frequency-response view + control strip).
- Inline unit / integration / render-smoke tests.

Out of scope:

- The MSEG editor widget itself — that is Plan 2 of the MSEG work (see
  Dependencies). miff *uses* it.
- Resonance and frame-scanning (deliberately dropped — see Design Decisions).
- A profiling harness — a recommended follow-up, not part of the initial
  implementation plan.

## Dependencies

- **MSEG core** — already merged into `tiny-skia-widgets` (`mseg` module:
  `MsegData`, `value_at_phase`, the randomizer, compact serde). miff bakes its
  kernel by sampling `value_at_phase`.
- **MSEG editor (Plan 2)** — the interactive editor widget (`draw_mseg`,
  `MsegEditState`) is not yet built. miff's *implementation* depends on it;
  miff's design is being settled first so it can inform Plan 2.

  **Cross-spec requirement miff places on Plan 2:** miff uses the MSEG as a
  *static filter kernel* — there is no playback. The MSEG editor must support
  a **curve-only configuration** in which the playback / timing controls
  (`play_mode`, `sync_mode`, `hold`, duration) are hidden, leaving only
  nodes / segments / tension / stepped / freehand-draw / grid / randomize.
  Plan 2 must add this configuration.

## Architecture

miff is structurally a near-twin of Wavetable Filter: a convolution filter
with Raw + Phaseless modes and a frequency-response view. Its Raw and
Phaseless DSP is **copied and adapted** from Wavetable Filter's proven, tested
implementation — the one substantive change being where the kernel comes
from (a baked MSEG curve, not a wavetable frame).

This copy is a pragmatic "for now," not an endorsement of per-crate DSP
duplication: that duplication across the workspace's plugin crates is
unrefactored debt, and a workspace-wide DSP-sharing refactor is anticipated.
To keep miff refactor-friendly, its convolution DSP lives in a **clean,
self-contained module** (`convolution.rs`) rather than tangled into `lib.rs`,
so a future shared-DSP crate can lift it without untangling.

## Crate Structure & Files

A new `miff/` crate modeled on `wavetable-filter`:

- `miff/Cargo.toml` — same dependency set as `wavetable-filter` (the nih_plug
  fork, baseview, softbuffer, tiny-skia, `tiny-skia-widgets`,
  raw-window-handle), plus a `[[bin]]` for the standalone.
- `miff/src/lib.rs` — plugin struct, params, `process()`, CLAP/VST3 exports.
- `miff/src/main.rs` — the small standalone entry point.
- `miff/src/kernel.rs` — the bake step (curve → normalized FIR taps).
- `miff/src/convolution.rs` — Raw + Phaseless convolution engine, copied and
  adapted from Wavetable Filter; a clean self-contained module.
- `miff/src/editor.rs` (+ `miff/src/editor/response_view.rs`) — softbuffer GUI:
  hosts the MSEG editor widget, the frequency-response view, and the control
  strip.
- `miff/src/fonts/` — embedded font for CPU text rendering.
- Workspace `Cargo.toml` — add `miff` to `members`. `xtask` bundle list and
  `CLAUDE.md` updated for the new plugin.

No test fixtures (miff loads no files).

## The Kernel Pipeline

### Baking

`miff/src/kernel.rs` turns the curve into the FIR kernel. For a kernel of
`len` taps:

```
kernel[i] = 2.0 * value_at_phase(&mseg, i as f32 / (len - 1) as f32) - 1.0
```

then L1-normalize:

```
let l1 = kernel[..len].iter().map(|t| t.abs()).sum();
if l1 > EPS { for t in &mut kernel[..len] { *t /= l1; } }
```

- The **bipolar map** (`2·value − 1`) makes the MSEG midline (value 0.5) a
  silent tap; the curve above/below the midline gives signed taps.
- **L1 normalization** bounds the worst-case convolution gain to unity, so any
  drawn shape keeps the output level sane; the `Gain` param restores level
  deliberately. An all-zero kernel (flat curve at 0.5) is left as-is.
- The bake walks the curve's segments **once** — O(`len`), advancing a segment
  cursor as `i` increases — not O(`len` · node-count).

### Where baking happens

The MSEG editor edits `MsegData` on the GUI thread; `Length` is likewise
editor state. Whenever the curve or `Length` changes, the **GUI thread**
re-bakes the kernel and hands the finished result to the DSP through a
**triple buffer** of a fixed, `Copy` kernel value:

```
struct Kernel { taps: [f32; MAX_KERNEL], len: usize }   // MAX_KERNEL = 4096
```

The audio thread therefore never bakes, never does an O(`len`) burst, and
never allocates — it reads the latest `Kernel` and convolves. This is cleaner
than Wavetable Filter's audio-thread kernel regeneration; miff can afford it
because the kernel source is editor state, not automatable params.

### Shape vs. params

miff's *filter shape* — the `MsegData` curve plus `Length` — is non-automated
document state, persisted with the plugin. The *performance controls* —
`Mode`, `Mix`, `Gain` — are ordinary automatable nih-plug params. `Length` is
deliberately not a nih-plug param (it is document state), which keeps
re-baking purely GUI-thread-triggered. `Length` ranges 64..`MAX_KERNEL`.

## DSP: Raw & Phaseless

`miff/src/convolution.rs`, both modes adapted from Wavetable Filter's engine
and fed the baked, normalized `Kernel`:

- **Raw** — direct time-domain convolution: the audio runs through the
  `len`-tap FIR via a SIMD multiply-accumulate loop (Wavetable Filter's MAC
  loop, generalized to a variable `len` up to `MAX_KERNEL`). Causal — the
  kernel's group delay is part of the filter character, so **no PDC latency is
  reported**. Wavetable Filter's silence fast-path (skip the MAC loop when
  input and filter state are all-zero) is carried over.
- **Phaseless** — STFT magnitude-only: the kernel's magnitude spectrum is
  applied to the audio via windowed FFT → magnitude multiply → IFFT →
  overlap-add (Wavetable Filter's STFT path). No pre-ringing. This **does**
  add latency (the STFT window/hop), reported to the host via
  `set_latency_samples`.

Stereo: one kernel, independent convolution state per channel. The kernel
magnitude FFT computed for Phaseless is also what the response view displays —
computed once, shared. `Mode` switching is click-safe: the inactive path's
state is reset on entry (matching Wavetable Filter).

## Params & GUI

**Automatable nih-plug params** (three): `Mode` (`EnumParam` — Raw |
Phaseless), `Mix` (0..1 dry/wet), `Gain` (output, dB).

**Persisted document state** (not params): the `MsegData` curve and `Length`,
persisted via nih-plug `#[persist]` using `MsegData`'s compact serde; the
editor window size via `EditorState`.

**GUI** — softbuffer + tiny-skia, freely resizable, layout B:

- *Top, full width* — the MSEG editor widget in curve-only mode (its canvas
  plus its own grid / snap / randomize strip; no playback strip). Where the
  kernel is drawn.
- *Middle, wide strip* — the frequency-response view: the kernel's magnitude
  spectrum, with a live input-spectrum shadow behind it. The input FFT is
  computed audio-side and published to atomics; the mechanism is copied from
  Wavetable Filter's `filter_response_view`.
- *Bottom strip* — a Raw/Phaseless stepped selector plus three dials: Mix,
  Gain, Length. Dials use the shared right-click text-entry treatment.

**Editor wiring:** the MSEG editor's event handlers mutate `MsegData`; on
`MsegEdit::Changed` (or a `Length` dial change) miff re-bakes the kernel,
refreshes the response-view curve, and marks state dirty for persistence.
`Mix` / `Gain` / `Mode` go through the normal `ParamSetter` path. The re-bake
and response-view recompute are GUI-thread work triggered by edits — never
per frame.

**Latency:** `Mode` drives reported latency — 0 for Raw, the STFT latency for
Phaseless.

## Testing

Inline `#[cfg(test)]` modules, matching the workspace convention.

- **Kernel bake** (`kernel.rs`) — a flat curve at 0.5 bakes to an all-zero
  kernel; a flat curve at 1.0 → all-`+1` before normalization; a ramp curve →
  ramp taps; L1 normalization holds (`Σ|kernel| == 1` within tolerance,
  skipped for an all-zero kernel); `len` bounds (64, `MAX_KERNEL`); the
  once-walk bake agrees tap-for-tap with a naive `value_at_phase`-per-tap
  reference.
- **Convolution** (`convolution.rs`) — Raw: a unit-impulse kernel
  (`[1, 0, 0, …]`, pre-normalization) passes audio through unchanged; a known
  kernel yields known output; the silence fast-path produces exact zeros.
  Phaseless: magnitude-only behavior, no pre-ringing, correct reported
  latency. Both modes: a zero kernel → silence.
- **Plugin** (`lib.rs`) — `Mix == 0` is dry-equivalent (bypass); a default
  flat document passes signal sanely; `Mode` switching is click-safe.
- **Editor** — render smoke tests: draw the whole editor and the response view
  into a `Pixmap`, asserting non-panic and a sentinel pixel. The MSEG editor
  widget's own interaction tests belong to Plan 2 — miff does not re-test them.

The MSEG core's existing tests cover `value_at_phase` and the curve math;
miff's tests focus on the bake, the convolution, and plugin integration.

**Follow-up (not core scope):** miff's Raw mode at long kernels is
CPU-significant — a profiling harness modeled on the six-pack / imagine /
tinylimit ones is a recommended follow-up.

## Design Decisions

- **The MSEG curve IS the FIR kernel** (not a modulation envelope, not a
  frequency-response paint). You draw the impulse response.
- **Bipolar taps** (`2·value − 1`): the MSEG value range stays 0..1 internally;
  the 0.5 midline maps to a zero tap. This allows highpass / bandpass / comb —
  arbitrary FIR responses — which an all-positive (unipolar) mapping could not.
- **No resonance.** Wavetable Filter's `resonance` is a spectral comb keyed to
  *harmonic wavetable content*; it does not carry that meaning for a
  hand-drawn curve. The drawn curve is the whole filter.
- **No frame-scan.** miff has one curve, not a wavetable of frames.
- **The MSEG is used as a static curve.** A filter kernel has no playback —
  hence the curve-only-mode requirement on Plan 2.
