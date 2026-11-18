# Trigger timing implementation

Spec: `../specs/2026-11-16-mseg-trigger-timing-design.md`.

1. Add tested, compatible timing settings, fixed pending-reset ring and source
   blend runtime with normalization, reset/SR/empty lifecycle.
2. Integrate exact audio reset scheduling and continuous delivered blending;
   verify defaults, native boundaries/cadence/Mix/waves and allocation safety.
3. Add Timing menu command/modal with numeric/drag/source ownership and bounded
   scaled layout, preserving undo and actual/frozen visual state.
4. Update guide/manuals and render timing behavior offscreen.
5. Run focused/full gates and independent DSP/native/UI reviews, fix findings
   with regressions, and leave this new slice uncommitted.

## Completed verification

All five steps are implemented. The six-crate focused suite passes 2,011 tests:
`cargo nextest run --locked --offline -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
All 14 timing-focused tests pass. Workspace Clippy with `-D warnings`,
`cargo fmt --check` and `git diff --check` pass. The `mseg_audio` example's
`timing` mode renders actual delayed/blended source measurements and the dialog.

Independent read-only DSP/native and UI reviews are clear. Allocation-guarded
probes verify 140,512 absolute-deadline samples, full ring wrap/saturation,
rounded-zero delays, merges, edits, empty/reset/SR/source transitions, moving
target blend matrices at 1/48 kHz, separate source lag, rearming, enums/Follow/
DAW exclusions, Mix/mapped parameters and fixed setter cadence. Native probes
cover preflight, queued-reset/wave collisions, host/grid/peer-MIDI partitions,
inactive/None rows, SR blend continuation/flush, and Listen/capture/history parity.

Regressions cover stale numeric drafts after same-variant source replacements
and explicit modulation Reset retaining the new phase-zero contract while
default-Off stays unchanged. UI probes verify modal/keyboard/numeric/fine-drag/
rollback/undo/reset ownership, scaled/HiDPI enlargement, source/row/preset cleanup,
and unchanged actual frozen canvas. Guide/manuals and preview were reviewed.
DAW-hosted timing/routing was not exercised.

This slice remains uncommitted. Lookahead/negative shifts are the next timing
slice; library categories/search/user-preset management remains on the comparison
list.
