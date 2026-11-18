# MIDI pitch implementation

Spec: `../specs/2026-11-16-mseg-midi-pitch-design.md`.

1. Add tested, compatible MIDI-pitch settings/runtime and exact sample-phase
   traversal, preserving lag, follower state and the fixed control cadence.
2. Connect the existing MIDI boundary stream in both engines; verify arbitrary
   host/grid splits, same-offset notes, resets/handoff and allocation safety.
3. Extend actual telemetry and waveform epochs; add Transpose/Channel selectors,
   measured pitch readouts, source ownership and protected saved-length controls.
4. Update guide/manuals and add a real MIDI-driven offscreen example.
5. Run focused/full checks and independent read-only DSP/native/UI reviews;
   resolve findings with regressions and leave the slice uncommitted.

## Completed verification

All five steps are implemented. The six-crate focused suite passes 1,964 tests:
`cargo nextest run --locked --offline -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
The final pitch-focused run passes all 47 selected tests. Workspace Clippy with
`-D warnings`, `cargo fmt --check` and `git diff --check` pass. The
`mseg_midi_pitch` example renders actual MIDI-driven phases, lag and audio;
playing/menu/waiting states were inspected at 0.5x/1x/2x.

Independent read-only DSP/native and UI reviews are clear. Allocation-guarded
probes cover reference phase/lag/follower/Mix/enums/control cadence, arbitrary
host/grid splits, sample-rate handoff, zero-frame calls, 4,096 simultaneous
notes, inactive/bypassed rows, wave priority, and 9,216 waveform traversal cases.
Regression tests cover transport restarts at an identical wrapped phase,
one-shot intro peaks, full extreme-rate readouts, external mode changes during
Length drags, and stale MIDI menus after source replacement. Frozen waveform
retention and exact prepared-readout ownership also passed independent probes.
Legacy empty-buffer MIDI draining remains intact; the no-poll behavior applies
when MIDI pitch is selected. DAW-hosted MIDI routing was not exercised.

This slice remains uncommitted. Remaining comparison candidates are detector
audition/algorithm refinements, trigger offsets/lookahead/retrigger smoothing,
and library categories/search/user-preset management.
