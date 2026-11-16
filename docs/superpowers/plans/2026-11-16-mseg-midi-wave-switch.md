# MIDI project-wave switching implementation

- [x] Commit the reviewed project-palette slice and inspect MIDI/runtime seams.
- [x] Record authorized MIDI switching and phase/editing behavior.
- [x] Add validated mapping and fixed-capacity bank snapshot tests/model.
- [x] Add playback overrides and phase-preserving source refresh.
- [x] Integrate both engines/native MIDI ingestion and atomic telemetry.
- [x] Add setup/read-only live display and cover editor/native behavior.
- [x] Update manuals and render offscreen evidence.
- [x] Resolve independent reviews and complete focused/lint/format gates.

## Validation

- All 1,872 focused tests passed with `cargo nextest run --offline
  -p tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy (`cargo clippy --workspace --offline -- -D warnings`),
  formatting (`cargo fmt --check`) and whitespace (`git diff --check`) passed.
- Native/engine tests cover note offsets, unchanged prefix/phase/author data,
  bank contention, reset/configuration/deletion, retrigger ordering and sample-rate
  adoption under allocation guards. Shared tests cover mapping/legacy serde,
  same-offset final selection, lag/Mix continuity and stale telemetry rejection.
- Independent DSP probes pass exact detector-reported Hz/audio-trigger collisions,
  hold-free waveform timing/bins, same-row untouched Mix, 4,096 dense events,
  bank refresh/deletion, inheritance and allocation checks. Review findings fixed:
  inherit the ready bank across a rate-change lock miss; make identical geometry
  an audio no-op; refresh/report modulated FFT latency on the switching sample.
- Independent UI probes pass .5x/315px, 1x/640px and 2x/1260px heights, short-window
  geometry, input priority, mapping limits, metadata/other-curve edits, Help and
  focus/resize behavior. Corrected double-scaled bounds in the new example and
  its new test fixture; all five setup/live-view tests pass with logical bounds.
- Visually inspected actual measured playback and setup renders from the
  `mseg_midi_waves` example: `/tmp/mseg-midi-waves.png`,
  `/tmp/mseg-midi-waves-setup.png` and `/tmp/mseg-midi-waves-setup-half.png`.
- Both implementation reviews are clear. Host-specific DAW MIDI routing remains
  manual; this slice leaves automation switching for the next step.
- Follow-up review reproduced delayed lag response at segment starts. Pending
  wave refreshes now survive both shared-state and Multosis row preparation,
  retaining consumption after precise triggers. Three regressions failed before
  the fix and now pass: shared Amp/continuous-target/Mix smoothing, native
  Minitosis host-buffer partitioning, and Multosis host/grid boundaries. Split
  warm-5/note-0 playback matches a 13-sample buffer with its note at sample 5,
  including first-sample response, phase and allocation guards.
  Independent probes confirm bit-identical Amp/continuous-target/Mix responses
  across host/grid boundaries; precision collisions, identical selections,
  setter counts, latency, waveform clocks and dense MIDI remain green.
