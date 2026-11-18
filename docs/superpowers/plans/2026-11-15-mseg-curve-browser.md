# Visual MSEG curve browser implementation

- [x] Record the authorized slice and inspect shared library/input ownership.
- [x] Add favorite removal and shared browser behavior tests.
- [x] Implement cached entries, bounded thumbnails and modal input.
- [x] Connect Shapes entry, final popup pass and all four consumers.
- [x] Verify state preservation, storage failures, pane routing and render layout.
- [x] Update manuals and render offscreen examples.
- [x] Resolve independent review findings and run focused/lint/format gates.

## Validation

- All 1,819 focused tests pass with `cargo nextest run --offline
  -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  formatting (`cargo fmt --check`), and whitespace (`git diff --check`) pass.
- Independent state/storage probes passed 96 favorite Load/Fit/metadata/cancel
  cases across full/static editors, all eight slots and 0.5x/1x/2x. The updated
  geometry-aware entry API retains those results and rejects undersized windows
  without opening, reading storage or changing the destination.
- UI review reproduced invisible modal input at Miff-like 640x180 bounds.
  Browser entry now checks layout with the actual pane geometry and reports
  `Enlarge the window to browse curves` when it cannot fit. A regression covers
  normal Shapes menu entry, repeated Enter and an actual-font status render in
  that window and a 200x120 widget without supplied native bounds.
  Independent retesting confirms visible rejection with no curve changes;
  supported-size rendering and ordinary two-press thumbnail loading remain clear
  at 0.5x/1x/2x and 300x260 bounds. No remaining review findings.
- Follow-up review reproduced labels without previews at 640x185. Layout admission
  and drawing now share thumbnail bounds, with minimum visible preview space
  after padding and labels. The exact menu-entry regression and image-based
  checks near minimum window heights failed before the fix and now pass across
  0.5x, 640/880x, 1x and 2x scales; every admitted factory card paints curve pixels.
  Independent public-API rendering checks passed 25 boundary cases: 14 accepted,
  11 rejected, and all 112 accepted thumbnails contain visible previews above
  their labels. Normal 640x185 entry rejects visibly without changing the curve.
- Offscreen factory, favorites and half-scale static favorites previews were
  rendered with the `mseg_browser` example and visually inspected:
  `/tmp/mseg-browser.png`, `/tmp/mseg-browser-favorites.png`, and
  `/tmp/mseg-browser-half.png`.

Native/DAW interaction checks remain manual.
