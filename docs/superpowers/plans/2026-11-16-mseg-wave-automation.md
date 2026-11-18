# Project-wave automation implementation

Spec: `../specs/2026-11-16-mseg-wave-automation-design.md`.

1. Add shared failing playback tests for four selectors, MIDI priority, empty
   slots and smoothed boundary refresh. Implement a preallocated automation
   overlay and use effective geometry in every sampler and waveform clock.
2. Append stable host parameter banks to both plugins; bridge per-process values
   and legacy state migration. Test parameter ordering, isolation, native audio
   boundaries, reset, contention and allocation freedom.
3. Bind host values into the shared editor. Extend wave setup with the current
   curve's Wave selector and return host edit requests. Adapt native ParamSetter
   bridges, playback previews, captions and contextual guidance. Test/render
   keyboard, modal and resize routing at multiple sizes.
4. Update manuals and render example, run focused tests/Clippy/fmt/diff checks,
   then request independent read-only DSP/state and UI reviews. Resolve findings
   and repeat only affected checks before the final gate.

## Verification

- All four selectors, host parameter IDs/order/defaults/migration, independent
  effect banks and fixed row lanes are covered. Native Minitosis tests exercise
  parameter changes at sub-block boundaries, empty slots, MIDI priority, bank
  contention and reset under allocation guards. Shared/engine tests cover
  smoothing, handoff, independent sources and unchanged clocks.
- Review reproduced and fixed frozen waveform clearing, intermediate-geometry
  smoothing refreshes during automation activation/release, and unnecessary raw
  refreshes after zero-frame selector changes. Permanent regressions cover
  live/frozen capture retention, exact Amp/parameter/Mix continuity, and Multosis
  pending-row dispatch including release without a note and empty host buffers.
  Independent allocation-guarded probes confirm exact audio/lag/phase for all
  three arbitration corners. Both DSP/native and UI reviews are clear.
- `cargo nextest run --offline -p tiny-skia-widgets -p tract-effects -p minitosis
  -p multosis -p miff -p nap`: **1,892 passed**. Workspace Clippy with `-D warnings`,
  formatting and whitespace checks passed.
- `mseg_midi_waves` now renders `automation` and `automation-setup`. Inspected
  `/tmp/mseg-wave-automation.png`, `/tmp/mseg-wave-automation-setup.png` and
  `/tmp/mseg-wave-automation-half.png`. Independent UI probes also cover 2x and
  short-window bounds, host requests, modal routing and stale telemetry.
- DAW automation recording/playback remains a manual host check. No commit made.
