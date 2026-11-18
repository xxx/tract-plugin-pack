# MSEG semitone-grid implementation

- [x] Commit reviewed sidechain support and define mapped musical-grid behavior.
- [x] Add compatible grid preference and tested GUI-only snapping helpers.
- [x] Derive and cache pitch/frequency levels in the shared effect editor.
- [x] Connect drawing, gestures, quantization, generation, and grid controls.
- [x] Cover units/routing changes, legacy/static behavior, undo, and UI geometry.
- [x] Update manuals and render the mapped-grid preview.
- [x] Resolve independent reviews and complete focused tests plus lint/format.

## Validation

- Committed the reviewed sidechain slice as `9743f54`.
- All 1,790 tests passed with `cargo nextest run --offline -p tiny-skia-widgets
  -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  formatting (`cargo fmt --check`), and whitespace (`git diff --check`) passed.
- Independent model and GUI reviews are clear. Model probes checked 7,316
  mapped-note cases across the registry, with maximum error 0.0000155 semitones.
  Public-API probes covered 27 gesture/menu cases and six clipping/render cases
  at 0.5x/1x/2x.
- Fixed review findings with regressions: numeric base input now commits before
  the same drawing press refreshes its grid; musical randomization bypasses the
  remembered numeric divisions so Stepped/Spiky patterns are not biased by them.
- Fixed the subsequent pen-repeat review finding: pitch-grid refresh receives
  the selected document and resets placement only for active Semitones with
  Snap On. The regression failed before the fix, then passed 18 pitch/frequency,
  pen-tool, and inactive-snap combinations across redraw and press refreshes.
  A separate test verifies that active musical placement still re-anchors.
  Independent follow-up review passed all 15 targeted grid/pen tests.
- Tests cover routing refresh, units/depth/polarity/clipping, unavailable grids,
  fine editing and gesture geometry, transactional Quantize, undo, project and
  geometry-only file compatibility, and unchanged legacy samples/defaults.
- Rendered and visually inspected `/tmp/mseg-pitch-grid.png` and
  `/tmp/mseg-frequency-grid.png` through the actual Grid and Quantize UI paths.

Native/DAW interaction and listening checks remain manual.
