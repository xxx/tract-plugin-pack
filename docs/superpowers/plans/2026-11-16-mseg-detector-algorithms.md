# Detector algorithms implementation

Spec: `../specs/2026-11-16-mseg-detector-algorithms-design.md`.

1. Add tested legacy-compatible response settings and causal Envelope/Peak/RMS
   analysis with cached Detail windows and preserved refractory lifecycle.
2. Extend prepared response telemetry and verify native segmentation, SR handoff,
   bypass/inactive/empty paths and allocation safety.
3. Add modal Algorithm/Detail controls with numeric/dropdown ownership, reset,
   stale-meter protection and independent Listen behavior.
4. Update guide/manuals and render the actual prepared modes offscreen.
5. Run focused/full gates and independent DSP/native/UI reviews, address findings
   with regressions, and leave this new slice uncommitted.

## Completed verification

All five steps are implemented. The six-crate focused suite passes 1,997 tests:
`cargo nextest run --locked --offline -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
All 33 detector-response/audition checks and the final HiDPI small-window check
pass. Workspace Clippy with `-D warnings`, `cargo fmt --check` and
`git diff --check` pass. The `mseg_audio` example's `peak` and `rms` modes render
actual prepared algorithm measurements.

Independent read-only DSP/native and UI reviews are clear. Allocation-guarded
probes verify 450,000 model samples across all three algorithms, five Detail
values and five sample rates; exact default-Envelope parity; Hold/SR/empty/
inactive transitions; bounded inputs; native onset phases, Sidechain, bypass,
host/grid/matching-MIDI-wave partitions and inactive/None prepared telemetry.
Listen token, filter history and audio remain bit exact across response edits.

Regressions cover empty-block sample-rate round trips lengthening Hold and
inactive same-family algorithm changes reinterpreting amplitude history as RMS
power. UI checks cover popup/numeric/fine-drag/undo/reset/Input ownership, stale
meters, unchanged actual frozen capture, and readable enlarge-window prompts
with suspended controls/Listen at 1x and 2x backing. Guide/manuals and offscreen
previews were reviewed. DAW-hosted detection/routing was not exercised.

This slice remains uncommitted. Remaining comparison candidates are trigger
offsets/lookahead/retrigger smoothing and library categories/search/user-preset
management.
