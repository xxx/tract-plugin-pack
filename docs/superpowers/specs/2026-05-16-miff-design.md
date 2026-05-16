# miff — Design

**Date:** 2026-05-16
**Status:** Approved (revised after spec review)
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
- Params, persisted state, and the softbuffer GUI (MSEG editor +
  frequency-response view + control strip).
- Inline unit / integration / render-smoke tests.

Out of scope:

- The MSEG editor widget itself — that is Plan 2 of the MSEG work (see
  Dependencies). miff *uses* it. Plan 2's spec is amended to add the
  curve-only mode miff needs (see C2 below).
- Resonance and frame-scanning (deliberately dropped — see Design Decisions).
- A profiling harness — a recommended follow-up, not part of the initial
  implementation plan.

## Dependencies

- **MSEG core** — already merged into `tiny-skia-widgets` (`mseg` module:
  `MsegData`, `value_at_phase`, the randomizer, compact serde). miff bakes its
  kernel from the curve.
- **MSEG editor (Plan 2)** — the interactive editor widget (`draw_mseg`,
  `MsegEditState`) is not yet built. miff's *implementation* depends on it.

  **Cross-spec requirement (C2):** miff uses the MSEG as a *static filter
  kernel* — there is no playback. The MSEG editor must support a **curve-only
  mode**: `MsegEditState` gains a curve-only flag (constructed via
  `MsegEditState::new_curve_only()`); `draw_mseg` and the event handlers read
  it. In curve-only mode the playback/timing controls (`play_mode`,
  `sync_mode`, `hold`, duration) and the marker lane are not drawn and not
  interactive; the grid / snap / randomize strip and all curve editing
  (nodes, segments, tension, stepped, freehand-draw) remain; the marker lane's
  vertical space is reclaimed by the canvas. The MSEG editor design spec
  (`2026-05-16-mseg-editor-widget-design.md`) has been amended to put this in
  Plan 2's scope. miff embeds the editor in this mode.

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
  raw-window-handle), plus a `[[bin]]` for the standalone. No new external
  crate is needed — the FFT crate Wavetable Filter already uses is reused, and
  the kernel handoff is hand-rolled (see The Kernel Pipeline).
- `miff/src/lib.rs` — plugin struct, params, `process()`, CLAP/VST3 exports.
- `miff/src/main.rs` — the small standalone entry point.
- `miff/src/kernel.rs` — the bake step (curve → normalized FIR taps) and the
  GUI→audio kernel handoff.
- `miff/src/convolution.rs` — Raw + Phaseless convolution engine, copied and
  adapted from Wavetable Filter; a clean self-contained module.
- `miff/src/editor.rs` (+ `miff/src/editor/response_view.rs`) — softbuffer
  GUI: hosts the MSEG editor widget, the frequency-response view, the control
  strip, and all event routing.
- `miff/src/fonts/` — embedded font for CPU text rendering.
- Workspace `Cargo.toml` — add `miff` to `members`. `xtask` bundle list and
  `CLAUDE.md` updated for the new plugin.

No test fixtures (miff loads no files).

## The Kernel Pipeline

### Baking

`miff/src/kernel.rs` turns the curve into the FIR kernel. The bake is a
**distinct, single-walk implementation** — NOT a wrapper over `value_at_phase`
(which rescans from node 0 on every call, so a naive per-tap loop would be
O(`len`·node-count)). The bake walks the curve's segments once, advancing a
segment cursor as the tap index increases — O(`len`) — and must replicate
`value_at_phase`'s stepped-segment hold and exponential tension `warp` exactly.

For a kernel of `len` taps:

```
kernel[i] = 2.0 * curve_value(&mseg, i as f32 / (len - 1) as f32) - 1.0
```

where `curve_value` is the single-walk traversal. The **bipolar map**
(`2·value − 1`) makes the MSEG midline (value 0.5) a silent tap; the curve
above/below the midline gives signed taps. This allows highpass / bandpass /
comb — arbitrary FIR responses.

### Normalization — peak frequency magnitude

After baking the raw taps, the kernel is normalized so its **peak frequency
response magnitude is unity** (0 dB):

```
compute the kernel's magnitude spectrum |H(k)| via FFT
peak = max_k |H(k)|
if peak > EPS:  for t in &mut kernel[..len] { *t /= peak; }
```

This guarantees the filter never boosts any frequency above 0 dB — output
level stays bounded regardless of the drawn shape, and *loudness is
consistent* between different curves (the loudest band is always 0 dB). L1
normalization was rejected in review: it bounds peak-time-domain gain but
leaves a 30–40 dB loudness swing between smooth (sign-cancelling) and
impulse-like curves. The `Gain` param is makeup gain on top.

The FFT used here is the same one Wavetable Filter already depends on; the
bake runs on the GUI thread, so its O(`len` log `len`) cost is free of
audio-thread concerns. The magnitude spectrum computed here is also what the
response view displays (computed once, shared).

### Neutral / default state

A genuinely flat curve at value 0.5 bakes to an **all-zero kernel**. miff
treats an all-zero kernel as **dry passthrough** — the convolution is skipped
and the output equals the input. (A literal zero-kernel convolution would be
silence; that is a useless state, so miff bypasses instead.)

miff's **default document** is exactly this flat 0.5 curve (a two-node
`MsegData`, both nodes value 0.5 — *not* the MSEG core's `default()`, which is
a ramp). A freshly-inserted miff is therefore a clean passthrough; the user
draws to create a filter.

### Where baking happens, and the handoff

The MSEG editor edits `MsegData` on the GUI thread; `Length` changes only via
its GUI dial (it is a non-automatable param — see Params). Whenever the curve
or `Length` changes, the **GUI thread** re-bakes the kernel (sample taps →
normalize) and publishes it to the audio thread through a **hand-rolled
triple buffer**: three pre-allocated kernel slots and an `AtomicUsize` holding
the index of the newest complete slot. This matches the lock-free atomic
patterns already used across the workspace (pope-scope, warp-zone) and adds no
dependency. The kernel slot is a fixed, `Copy` value:

```
struct Kernel { taps: [f32; MAX_KERNEL], len: usize }   // MAX_KERNEL = 4096
```

The audio thread reads the published slot index, reads that `Kernel`, and
convolves — it never bakes, never does an O(`len`) burst, never allocates.

## DSP: Raw & Phaseless

`miff/src/convolution.rs`, both modes adapted from Wavetable Filter's engine
and fed the baked, normalized `Kernel`:

- **Raw** — direct time-domain convolution: the audio runs through the
  `len`-tap FIR via a SIMD multiply-accumulate loop (Wavetable Filter's MAC
  loop). The loop iterates `len / 16` SIMD-wide chunks, so CPU cost scales
  with `Length`, not with `MAX_KERNEL` — `Length` is constrained to multiples
  of 16 so no remainder handling is needed. Causal — the kernel's group delay
  is part of the filter's character, so **no PDC latency is reported**. Note
  this honestly: a hand-drawn kernel can place its energy peak late (e.g. tap
  4000 ≈ 83 ms), which is real, unreported delay presented as filter
  character — the response/curve view shows the kernel so this is visible.
  Wavetable Filter's silence fast-path (skip the MAC loop when input and
  filter state are all-zero) is carried over.
- **Phaseless** — STFT magnitude-only: the kernel's magnitude spectrum is
  applied to the audio via windowed FFT → magnitude multiply → IFFT →
  overlap-add (Wavetable Filter's `process_stft_frame` path). No pre-ringing.
  Phaseless uses a **fixed** STFT frame of `MAX_KERNEL` (4096) points
  regardless of `Length` (a short kernel is zero-padded); the reported latency
  is the constant **hop = frame / 2 = 2048 samples** (~43 ms at 48 kHz).
  Fixing the frame keeps Phaseless latency constant — it never jumps when the
  user changes `Length`.

Stereo: one kernel, independent convolution state per channel. `Mode` switches
between Raw and Phaseless; switching is click-safe — the inactive path's state
is reset on entry (matching Wavetable Filter). The reported latency is
re-sent to the host only when it actually changes (a `last_reported_latency`
guard, as in Wavetable Filter), since `Mode` is automatable and can flip
mid-stream.

## Params & GUI

**nih-plug params:**

- `Mode` — `EnumParam`, Raw | Phaseless. Automatable.
- `Mix` — `FloatParam`, 0..1 dry/wet. Automatable.
- `Gain` — `FloatParam`, output makeup gain in dB. Automatable.
- `Length` — `IntParam`, 64..`MAX_KERNEL`, step 16 (always a multiple of 16),
  default 256. Flagged **`NON_AUTOMATABLE`**. Being a real `Param` means the
  bottom-strip dial and its shared right-click text-entry work normally and
  persistence is automatic; being non-automatable means it can only change via
  the GUI, so re-baking stays GUI-thread-triggered.

**Persisted document state** (not a param): the `MsegData` curve, persisted
via nih-plug `#[persist]` using `MsegData`'s compact serde. The editor window
size is persisted via `EditorState`.

**GUI** — softbuffer + tiny-skia, freely resizable, layout B:

- *Top, full width* — the MSEG editor widget in **curve-only mode** (its
  canvas plus its grid / snap / randomize strip; no playback strip, no marker
  lane). Where the kernel is drawn.
- *Middle, wide strip* — the frequency-response view: the kernel's magnitude
  spectrum (the baked, normalized kernel — its peak sits at 0 dB), with a live
  input-spectrum shadow behind it.
- *Bottom strip* — a Raw/Phaseless stepped selector plus three dials: Mix,
  Gain, Length. Dials use the shared right-click text-entry treatment.

**Editor embedding** (`miff/src/editor.rs`): miff owns a curve-only
`MsegEditState`. The MSEG editor occupies a fixed rect in the top region of
miff's window (computed from the current window size). baseview mouse events
whose position falls in that rect route to `MsegEditState::on_mouse_down /
_move / _up / on_double_click`; events elsewhere go to miff's own dial/strip
hit-testing. Keyboard events route to `MsegEditState::on_key`. miff drives the
MSEG editor's two modifiers: **Alt held** sets `stepped_draw` (freehand
stepped drawing); **Shift held** sets `fine` (snap-bypass for precise
placement). When an MSEG handler returns `MsegEdit::Changed`, miff re-bakes
the kernel, refreshes the response-view curve, and marks plugin state dirty
for persistence. `Mix`/`Gain`/`Mode`/`Length` go through the normal
`ParamSetter` path. The re-bake and response recompute are GUI-thread work
triggered by edits — never per frame.

**Response view input shadow** (`editor/response_view.rs`): the audio thread
computes the input's magnitude spectrum and publishes it to the GUI through a
shared `Arc<Mutex<Vec<f32>>>` (the magnitude bins) updated on a ~30 Hz
throttle counter — the exact mechanism Wavetable Filter's `filter_response_view`
uses. The response view reads the latest bins and draws them as a dim shadow
behind the kernel response curve. The `try_lock` discipline keeps the audio
thread non-blocking.

**Latency:** `Mode` drives reported latency — 0 for Raw, 2048 (the Phaseless
hop) for Phaseless.

## Testing

Inline `#[cfg(test)]` modules, matching the workspace convention.

- **Kernel bake** (`kernel.rs`) — the flat-0.5 default curve bakes to an
  all-zero kernel; a flat curve at 1.0 → all-`+1` taps before normalization; a
  ramp curve → ramp taps; the single-walk bake agrees **tap-for-tap** with a
  naive `value_at_phase`-per-tap reference (this guards the separate
  segment-walk implementation); peak-magnitude normalization holds (peak
  `|H(k)| == 1` within tolerance for any non-zero kernel; skipped for an
  all-zero kernel); `Length` bounds (64, `MAX_KERNEL`) and multiple-of-16.
- **Convolution** (`convolution.rs`) — Raw: a unit-impulse kernel
  (`[1, 0, 0, …]`) passes audio through unchanged; a known kernel yields known
  output; the silence fast-path produces exact zeros. Phaseless:
  magnitude-only behavior, no pre-ringing, the reported latency is the fixed
  2048-sample hop. Both modes: an all-zero kernel → **dry passthrough** (output
  equals input), not silence.
- **Plugin** (`lib.rs`) — the default document (flat 0.5) is a clean
  passthrough; `Mix == 0` is dry-equivalent; `Mode` switching is click-safe;
  latency is re-reported only on change.
- **Editor** — render smoke tests: draw the whole editor and the response view
  into a `Pixmap`, asserting non-panic and a sentinel pixel. The MSEG editor
  widget's own interaction tests belong to Plan 2 — miff does not re-test them.

The MSEG core's existing tests cover the curve math; miff's tests focus on the
bake, the normalization, the convolution, and plugin integration.

**Follow-up (not core scope):** miff's Raw mode at long kernels is
CPU-significant — a profiling harness modeled on the six-pack / imagine /
tinylimit ones is a recommended follow-up. `Length` defaults low (256) partly
for this reason.

## Design Decisions

- **The MSEG curve IS the FIR kernel** — not a modulation envelope, not a
  frequency-response paint. You draw the impulse response.
- **Bipolar taps** (`2·value − 1`): the MSEG value range stays 0..1 internally;
  the 0.5 midline maps to a zero tap, enabling arbitrary (highpass / bandpass /
  comb) FIR responses that an all-positive mapping could not.
- **Peak-magnitude normalization, not L1.** The filter never boosts above
  0 dB and loudness is consistent across drawn shapes; `Gain` is makeup.
- **Flat-0.5 default → all-zero kernel → dry passthrough.** A fresh miff is a
  clean passthrough, not a coloring filter and not silence.
- **No resonance.** Wavetable Filter's `resonance` is a spectral comb keyed to
  *harmonic wavetable content*; it does not carry that meaning for a
  hand-drawn curve. The drawn curve is the whole filter.
- **No frame-scan.** miff has one curve, not a wavetable of frames.
- **The MSEG is used as a static curve** in curve-only mode — a filter kernel
  has no playback.
- **`Length` is a non-automatable param**, not free document state: this makes
  the GUI dial, text entry, and persistence work without bespoke machinery,
  while keeping kernel re-bakes off the audio thread.
