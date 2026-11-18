# MSEG sidechain implementation

- [x] Commit the reviewed loop-seam slice and define sidechain behavior.
- [x] Add compatible detector routing and borrowed auxiliary-buffer handling.
- [x] Connect both native buses and sample-timed engine paths.
- [x] Add source-aware detector telemetry and waveform capture.
- [x] Expose detector Input and Sidechain waveform controls with undo/focus rules.
- [x] Cover native timing, missing buffers, audio parity, allocation, and UI cases.
- [x] Update manuals, render DSP preview, and resolve independent reviews.
- [x] Complete focused tests, workspace Clippy, formatting, and diff checks.

## Validation

- Committed the reviewed loop-seam slice as `dd31334`.
- All 1,773 tests passed with `cargo nextest run --offline -p tiny-skia-widgets
  -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  formatting (`cargo fmt --check`), and whitespace (`git diff --check`) passed.
- Independent DSP/native and UI/telemetry reviews are clear. Additional native
  probes passed 40 cases across four rates and five host partitions, including
  short auxiliary buffers, peer MIDI/grid splits, and no auxiliary audio leakage.
- Tests cover missing/mono/short input, source/default serialization, coherent
  detector identity, private history reset and SR inheritance, exact trigger
  offsets, inactive/bypass/None paths, main-output parity, allocation guards,
  numeric auto-commit/undo/reset, and source-aware waveform/freeze lifecycle.
- Corrected the SR test to follow existing initialization policy: fresh MSEG
  phases with inherited private detector/filter history. A public-API probe
  confirmed preserved Sidechain routing without a false onset.
- Diagnosed a pre-existing popup pixel-test race by delaying the second draw:
  only the blinking filter caret changed. The layer comparison now excludes
  that caret rectangle while checking all fixed opaque popup pixels.
- Rendered and visually inspected `/tmp/mseg-sidechain.png` using `mseg_audio`
  with `stereo 0 sidechain detector`: independent main tone, raw sidechain
  waveform, measured source trace, and the actual detector level.

Host-specific DAW bus routing and listening checks remain manual.
