# Envelope follower implementation

Spec: `../specs/2026-11-16-mseg-envelope-follower-design.md`.

1. Add normalized, legacy-compatible config and tested linked stereo AHR runtime,
   with detector filters, gain, threshold, hold/SR/reset transitions.
2. Integrate route combination with the continuous-source path and both native
   input bridges; verify cadence, phase/lag/Mix/Wave independence and allocations.
3. Extend actual telemetry/source identity and add the shared scrollable modal,
   numeric/drag/dropdown ownership, native undo integration and lifecycle cleanup.
4. Update guide/manuals and render a real follower signal/trace at multiple sizes.
5. Run focused/full gates and read-only DSP/native/UI reviews, fixing findings
   with regressions. Leave the slice uncommitted.

## Completed verification

All five steps are implemented. The six-crate focused suite passes 1,946 tests:
`cargo nextest run --locked --offline -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
Workspace Clippy with `-D warnings`, `cargo fmt --check`, and `git diff --check`
pass. The `mseg_follower` example renders actual audio, combined traces and meters;
0.5x/1x/2x panels and short/narrow layouts were inspected.

Independent read-only DSP/native and UI reviews are clear. Allocation-guarded
probes exercise both native engines and host/grid partitioning. Regressions cover
coefficient edits while disabled, zero-frame route changes preserving held
parameters, and inactive-route edits preserving legacy source cadence. Telemetry
tests cover full prepared settings and late measurements after mode/input changes.
UI probes cover numeric commits before drags, Escape rollback, shared reset scope,
stale meters, scrolling and resize/source cleanup. DAW-hosted sidechain routing
was not exercised. This slice remains uncommitted.
