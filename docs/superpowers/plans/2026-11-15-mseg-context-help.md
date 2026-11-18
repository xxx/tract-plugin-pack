# MSEG context-help implementation

- [x] Record authorized context-help and guide behavior.
- [x] Add shared context picking, guide state/content, layout, and input.
- [x] Add toolbar/menu/shortcut entry and final-pass drawing.
- [x] Connect host bounds, focus/gesture priority, and pane/window hint cleanup.
- [x] Cover modal routing, guide clipping/scrolling, and state preservation.
- [x] Update manuals and render offscreen evidence.
- [x] Resolve independent reviews and complete focused tests plus lint/format.

## Validation

- All 1,803 tests passed with `cargo nextest run --offline -p tiny-skia-widgets
  -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  formatting (`cargo fmt --check`), and whitespace (`git diff --check`) passed.
- Independent render and input reviews are clear. Render probes passed 96
  full/static page/scroll cases across 0.5x/1x/2x and narrow bounds. Input probes
  passed 21 modal, gesture, focus, scrolling, and pen/selection preservation cases.
- Resolved review findings: static Grid hints now describe numeric grids;
  direct guide entry takes its scale explicitly; Multosis respects its speed
  popup before help shortcuts and forwards cursor leave to clear hints.
- Resolved follow-up findings: hints have a full-width third footer row; guide
  pages advance by visible body height with overlap; cursor leave clears hints
  before detector-modal and binding-change early returns. Regression probes
  reproduced each failure before the fix and now pass, including actual-font
  footer rendering at Minitosis's default size across 0.5x/1x/2x.
- Independent follow-up layout review passed 40 full/static page combinations
  across 0.5x/0.75x/1x/1.5x/2x, including endpoint, wheel and keyboard scrolling.
  The modal review's public-API probe also confirmed no stale pixels after a
  binding change followed by cursor leave, with no configuration edits.
- Tests cover all entry paths, numeric/menu/gesture priority, unchanged documents,
  selection/viewport/pen repetition, popup layering and mouse priority, bounded
  text and scroll, native bounds, hover cleanup, focus/resize, and Nap ownership.
- Rendered and visually inspected `/tmp/mseg-help.png`,
  `/tmp/mseg-help-static.png`, and `/tmp/mseg-help-hover.png` using the `mseg_help`
  example, including the static guide at half scale.

Native/DAW interaction checks remain manual.
