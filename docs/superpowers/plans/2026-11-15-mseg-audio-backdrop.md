# MSEG audio backdrop implementation

- [x] Commit the reviewed smoothing/trace slice and define next-slice behavior.
- [x] Add bounded waveform measurements and tested display timing/aggregation.
- [x] Capture selected-row input/output in both engines without changing audio.
- [x] Add Audio/Freeze controls and clipped background rendering.
- [x] Cover capture lifetime, source switches, interaction, and allocation edges.
- [x] Update documentation and render an actual DSP offscreen preview.
- [x] Resolve independent reviews and complete focused tests plus lint/format checks.

Verification:

- 1,680 tests passed across tiny-skia-widgets, tract-effects, Multosis, Minitosis,
  Miff, and Nap. Both engines have waveform audio-parity and allocation guards.
- Workspace Clippy (`--offline -- -D warnings`), formatting, and whitespace pass.
- `cargo run --offline -p tract-effects --example mseg_audio -- /tmp/mseg-audio.png`
  rendered an inspected input/output backdrop, source trace, and frozen controls.
- Independent engine/DSP and telemetry/UI reviews are clear. Fixed and covered
  long-cycle rounded phases, late one-shot peaks, and multi-wrap sample steps.
- Made the earlier Smooth reset test seed its click timestamp immediately before
  pressing, avoiding a dependency on how long the preceding test work takes.

Native/DAW listening checks remain manual.
