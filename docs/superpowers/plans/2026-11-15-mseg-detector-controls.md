# Audio detector controls implementation

- [x] Commit reviewed sample-timed triggers and define detector-control behavior.
- [x] Add compatible settings and tested level/filter DSP.
- [x] Publish coherent measured detector level.
- [x] Add modal Detect panel with numeric/drag/reset and undo/focus routing.
- [x] Verify native timing/parity/allocation and popup/source ownership.
- [x] Update manuals and render an actual DSP preview.
- [x] Resolve independent reviews and complete tests plus lint/format checks.

## Validation

- Committed the reviewed sample-timed trigger slice as `68e7a33`.
- All 1,742 focused tests passed with `cargo nextest run --offline -p
  tiny-skia-widgets -p tract-effects -p minitosis -p multosis -p miff -p nap`.
- Workspace Clippy passed with `cargo clippy --workspace --offline -- -D warnings`.
- Formatting and whitespace checks passed with `cargo fmt --check` and
  `git diff --check`.
- Independent read-only reviews covered DSP and editor behavior. Extra DSP
  probes covered five sample rates and malformed/crossed filter settings.
- Fixed a subsequent review finding: the 90%-of-Nyquist clamp shifted valid
  cutoff frequencies. Regression tests first reproduced the error, then verified
  both filter types at 3.9 kHz/8 kHz, 15 and 15.9 kHz/32 kHz, and 19.9 kHz/40 kHz.
  Separate tests cover finite filter limits at and above Nyquist.
- The fix review exposed hidden integrator-state spikes on cutoff edits after
  Nyquist input. A regression reproduced the issue, including below Nyquist.
  Bilinear input/output history and explicit endpoint limits preserve the
  response without that state growth. Tests cover alternating input followed by
  cutoff and sample-rate edits for both filter types.
- Fixed and tested long numeric input overflowing its field; the visible tail
  stays within the panel at 0.5x/1x/2x while the complete value is parsed.
- Rendered and visually inspected `/tmp/mseg-detector.png` using actual detector
  DSP through the `mseg_audio` example's `audio detector` mode.

Native DAW routing and listening checks remain manual.
