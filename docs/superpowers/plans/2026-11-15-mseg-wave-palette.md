# Project MSEG wave palette implementation

- [x] Inspect persistence/audio boundaries and record the authorized manual slice.
- [x] Add tested bounded palette model and compatible shape serialization.
- [x] Add Project browser binding, snapshot guard, storage and manual recall.
- [x] Persist and bind the palette in all four plugins without engine changes.
- [x] Test native state round trips, instance/pane sharing and unchanged undo/bakes.
- [x] Update manuals/help and render project slots offscreen.
- [x] Resolve independent reviews and run focused/lint/format gates.

## Validation

- All 1,840 focused tests passed with `cargo nextest run --offline
  -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  formatting (`cargo fmt --check`) and whitespace (`git diff --check`) passed.
- Model and native tests cover validated records, slot transactionality,
  persisted instance isolation, cross-pane sharing and curve-only undo. Full
  Multosis row-array serde runs on an 8 MiB debug test stack; diagnostics
  pinpointed the default test-stack overflow to row-array deserialization.
- Legacy preset loading now inserts an empty bank before deserialization in
  all four plugins, including already populated instances. The hot-load
  regression failed before the migration hook and now passes. Supplied banks
  and unrelated fields are preserved.
- Independent storage probes passed 96 native-metadata Load/Fit cases across
  eight slots, full/static editors and 0.5x/1x/2x, plus snapshot guards, rebinding,
  migration and serialization. No palette accesses enter audio configuration
  or process paths; both reviews are clear.
- Independent UI probes cover modal routing, two ordinary thumbnail presses,
  focus/resize dismissal and cancellation retaining the binding. Compact bounded
  captions fix the reproduced 124x300 tab overlap; pixel regressions pass.
  Prior minimum-preview and 640x185 rejection behavior remain intact.
- Rendered and visually inspected `/tmp/mseg-project-waves.png` and
  `/tmp/mseg-project-waves-half.png` with the extended `mseg_browser` example.

Native/DAW interaction checks remain manual.
