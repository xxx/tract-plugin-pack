# Lookahead implementation

Spec: `../specs/2026-11-16-mseg-lookahead-design.md`.

1. Complete: compatible signed timing preferences and a tested preallocated
   audio-frame delay; scheduling/masks handle lookahead paths and lifecycle.
2. Complete: native per-effect/row input, follower/capture and dry-path alignment,
   reported latency and construction/SR-only reserves; default/allocated paths
   verified.
3. Complete: Timing toggle/signed bounds, source ownership, undo and scaled/HiDPI
   cleanup; actual/frozen visual state preserved.
4. Complete: guide/manuals and a real lookahead/signed-shift render example.
5. Complete: focused/full checks and independent DSP/native/UI review. The user
   subsequently requested the host-tail review fix and commit of this slice.

## Verification

- All 2,028 tests pass across tiny-skia-widgets, tract-effects, minitosis,
  multosis, miff and nap with locked/offline nextest.
- Workspace Clippy with `-D warnings`, formatting and whitespace checks pass.
  Cargo reports existing unused `atomic_float` manifest dependencies in Miff and
  Wavetable Filter; no dependency changes were made.
- Permanent tests cover frame alignment/warmup/topology, signed cues and legacy
  wire state, Mini dry/wet/bypass/E+L latency, Multi inactive/None/mute/Solo paths,
  original-versus-series follower/capture input and signed editor ownership/undo.
- A red capture probe exposed stale peaks when toggling lookahead in one waveform
  bin. A separate capture epoch fixes this without changing clocks/trigger counts
  or discarding frozen views. The regression passes. The larger dry reserve also
  required adapting an existing FFT test's expected tap to its actual ring size;
  it still distinguishes live latency from the stale pre-trigger endpoint.
- Independent DSP/native allocation-guarded probes pass 45 Mini signed-shift,
  impulse, dry/wet/bypass and E+L cases; upstream cue compensation, same-total
  topology replacement, SR cue rescaling, empty callbacks, host/grid/peer-MIDI
  partitions, FFT/Listen changes, original Main/aux metadata and the native
  131,072-sample audition reserve. Default-Off timing parity also passes.
- Independent UI probes pass numeric commit/toggle ordering, signed bounds and
  fine drag, keyboard/reset/undo, external ownership changes, small/HiDPI layouts
  and frozen-canvas preservation. Both independent reviews have no remaining
  actionable findings.
- The host-tail review regression first reproduced Normal instead of Tail(192)
  during an inactive/None lookahead impulse's silent callbacks. Multosis now
  reports finite Tail(total declared latency) while its adopted path contains
  lookahead, covering aligned Listen as well. Four allocation-guarded regressions
  pass for one/three windows at every global Mix, default Off, empty/mute/Solo/
  source adoption and Sidechain Listen with FFT latency. Independent review and
  a separate run of all four tests found no further defect. The constant finite
  tail intentionally keeps CLAP processing while lookahead is enabled.
- Rendered and inspected `/tmp/mseg-lookahead.png` using the `mseg_audio` example's
  `stereo 0 audio lookahead` mode. Actual DAW host/PDC routing was not exercised.

Signed shifts compensate the added lookahead windows; existing effect/FFT latency
and process-time MIDI/DAW/free clocks retain their previous semantics. These
limits are stated in the dialog and manuals.
