# MSEG stereo waveform implementation

- [x] Commit reviewed audio backdrop; record stereo-view behavior.
- [x] Add signed stereo bins and bounded atomic transport with regression tests.
- [x] Add Magnitude/Combined/Stereo rendering and display gain with lane clipping.
- [x] Integrate view-only Audio menu choices, frozen history, and focus/resize.
- [x] Add interaction/concurrency/parity coverage and a DSP-driven preview.
- [x] Update manuals and resolve independent review findings.
- [x] Complete focused tests, workspace Clippy, formatting, and whitespace checks.

Verification:

- 1,690 focused tests passed across tiny-skia-widgets, tract-effects, Multosis,
  Minitosis, Miff, and Nap, including both engines' audio-parity/allocation guards.
- Workspace Clippy (`--offline -- -D warnings`), formatting, and whitespace pass.
- The `mseg_audio` example renders magnitude/combined/stereo from actual DSP
  capture; its optional final argument selects -12/0/12 dB display gain.
- Independent capture/DSP and telemetry/UI reviews are clear. A randomized probe
  matched prior magnitude values bit-for-bit across 15,625 groups. Regression
  coverage includes fractional raster dividers/plot edges, tiny visible ranges,
  offscreen-range rejection, and readable menu labels at 0.5x/1x/2x.

Native/DAW listening checks remain manual.
