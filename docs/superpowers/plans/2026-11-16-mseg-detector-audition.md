# Detector audition implementation

Spec: `../specs/2026-11-16-mseg-detector-audition-design.md`.

1. Add tested atomic request ownership and a preallocated filtered monitor with
   latency alignment, source changes and bounded fades.
2. Integrate the validated request/output in both native engines, retaining
   normal DSP, detector timing, telemetry and bypass/reset behavior.
3. Add Listen controls, live-source ownership, settings refresh, numeric/modal
   priority and cleanup in both panels; verify scaled rendering.
4. Update guide/manuals and produce an offscreen audition panel preview.
5. Run focused/full gates and independent DSP/native/UI review, resolve findings
   with regressions, and leave the slice uncommitted.

## Completed verification

All five steps are implemented. The six-crate focused suite passes 1,981 tests:
`cargo nextest run --locked --offline -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
All 17 audition-focused tests pass. Workspace Clippy with `-D warnings`,
`cargo fmt --check` and `git diff --check` pass. The `mseg_follower` example's
`audition` mode and `mseg_audio`'s `listen` mode render the panels; previews and
0.5x/1x/2x/narrow layouts were inspected.

Independent read-only DSP/native and UI reviews are clear. Allocation-guarded
native probes cover exact delays 0/1/3/4096/65536, a live MIDI-driven FFT latency
change, conservative Multi latency for inactive rows, detector/follower/phase/
frame/waveform/DSP-tail parity, host/grid/MIDI/empty-block partitioning, mono aux,
disabled routes and pre-threshold sound. A concurrent request probe checked
50,000 single-writer leases and 3,649 coherent snapshots.

Regressions cover rapid same-input fade reversal, filter history advancing
during bypass, all cleanup paths including undo/row-swap trace invalidation,
pending filter handoffs, old-owner release, reset non-revival, and visible
disconnected-panel feedback. Independent UI checks confirm unchanged config and
actual frozen waveform pixels through Listen toggles and focus-loss cleanup.
DAW-hosted audition/routing was not exercised.

This slice remains uncommitted. Further comparison candidates are detector
algorithms/detail, trigger offsets/lookahead/retrigger smoothing, and library
categories/search/user-preset management.
