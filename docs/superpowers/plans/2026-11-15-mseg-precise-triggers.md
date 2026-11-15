# Sample-timed trigger implementation

- [x] Commit reviewed MIDI slice and record precise-trigger behavior.
- [x] Add tested precise runtime and cached detector arithmetic.
- [x] Integrate first/interior sample triggers without changing other rows.
- [x] Add trigger choices/dial binding with persistence and interaction coverage.
- [x] Verify exact offsets, resets, Mix, scope, collisions, and allocation guards.
- [x] Update manuals and render a DSP-driven preview.
- [x] Resolve independent reviews and complete tests plus lint/format checks.

Verification:

- 1,723 focused tests passed across widgets, effects, Multosis, Minitosis, Miff,
  and Nap, including allocation guards and the existing MIDI/legacy regressions.
- Workspace Clippy (`--offline -- -D warnings`), formatting, and whitespace pass.
- Runtime checks cover 44.1/48/96/192 kHz, exact first-fire convention, fractional
  positions, detector equivalence, rate changes, and supra-sample coalescing.
- Native checks cover same-effect SR progress/refractory inheritance, FFT-history
  preservation on first-sample fires, immediate dry-delay latency, bypass/None/
  inactive rows, grid/peer-MIDI boundaries, swaps, DSP flush, and restarts.
- UI checks cover carried Rate/Sens/Hold, undo/JSON, source order, measured popup
  width/priority at 0.5x/1x/2x. Independent core/native and UI/telemetry reviews
  are clear after fixing supra-sample fractional progress with numerator/pending state.
- `cargo run --offline -p tract-effects --example mseg_audio -- /tmp/mseg-audio-exact.png stereo 0 audio`
  rendered actual audio-triggered DSP and measured scopes; preview inspected.
  Use final argument `hz` for the sample-timed Hz demo.

Native/DAW routing and listening checks remain manual.
