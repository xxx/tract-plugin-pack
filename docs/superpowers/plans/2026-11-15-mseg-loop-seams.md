# MSEG loop-seam implementation

- [x] Record authorized behavior, playback scope, and preservation rules.
- [x] Add tested seam analysis and transactional geometry operations.
- [x] Connect shared Edit commands and discontinuity rendering.
- [x] Cover selection/markers, undo, popup routing, and static samplers.
- [x] Update manuals and render an offscreen preview.
- [x] Resolve independent reviews and complete focused tests plus lint/format.

## Validation

- All 1,759 tests passed with `cargo nextest run --offline -p tiny-skia-widgets
  -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  `cargo fmt --check`, and `git diff --check` passed.
- Model and UI read-only reviews are clear after correcting the requested 10%
  return position and sizing menus for readable labels at 0.5x/1x/2x.
- Independent probes checked 4,000 randomized curves and 4,004,000 samples for
  bounded output, node/marker preservation, and repeated-command stability.
- Integration coverage verifies one-step undo, selection remapping, saved-state
  round trips, hold-loop matching, and native Miff/Nap sampler agreement.
- The `mseg_loop_seams` example rendered `/tmp/mseg-loop-seams.png`; visual
  inspection confirmed the stepped-tail indicator and native smooth return.

Native/DAW interaction and listening checks remain manual.
