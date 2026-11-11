# Shared editor DPI repair

## Confirmed audit

All fourteen editors besides Gated persist physical resize dimensions as logical
window sizes and pass logical pointer coordinates to physical hit regions.
The pinned baseview macOS backend also misses frame/backing-change notifications.
Minitosis and Multosis forward input into the shared effect editor, so normalizing
only the outer pointer cache would leave nested controls broken.

## Implementation

- Introduce `tiny_skia_widgets::EditorWindow`, wrapping each physical renderer.
  Own logical persistence, native geometry polling, host resize units, converted
  pointer events, and cached pointer reprojection in one place.
- A small `PhysicalEditor` hook ends gestures/closes popups and updates backing
  input sensitivity before geometry changes. Rebuild layout before further input.
- Keep all editor drawing and nested effect/MSEG input in physical coordinates.
  Remove legacy pending-resize consumption and physical-size persistence from
  child handlers. Derive physical drawing scale from logical zoom × backing scale.
- Extract Gated's AppKit geometry query into the shared crate, retaining its
  existing Gated integration while eliminating the duplicate native bridge.
- Adjust constant drag distances, double-click radii, and popup scrolling for
  backing DPI. Preserve semantic wheel increments, including Six Pack's Q steps.
- Keep explicit host scaling separate from user zoom. Four editors previously
  accepted a scale callback but opened fixed-scale windows and overwrote that
  factor with user zoom. They now reject that unsupported request consistently
  with the rest of the pack; native macOS backing DPI is handled independently.
- Cover all fourteen siblings: Gain Brain, GS Meter, HD26, Imagine, Miff,
  Minitosis, Multosis, Nap, Pope Scope, Satch, Six Pack, Tinylimit, Warp Zone,
  and Wavetable Filter.

## Validation

- Shared regression tests for 1×/1.5×/2× logical persistence, arbitrary aspect
  ratios, host resize units, pointer conversion/reprojection, and drag sensitivity.
- Focused plugin/editor tests, workspace clippy, formatting, and local native UI.
- Build and run tests on the user's Mac `mini`; inspect Glass's display and
  permission capabilities before claiming a live Retina result. Initial display
  inventory reports an M4 GPU with no attached display.
- Preserve all existing work and do not commit unless requested.

## Results

- Linux: workspace clippy and formatting pass. The initial 2,244-test run and
  small-window native smoke checks missed Wavetable Filter's degenerate plot
  rectangles. Direct rendering regressions now cover 900 × 100, 32 × 32, zero
  plot area, narrow windows, empty/loaded tables, 2D/3D, and 1×/1.5×/2× backing.
  Invalid plots are skipped without changing framebuffer dimensions; normal
  rendering and the 3D cache recover after expansion.
  With these regressions included, all 2,247 workspace tests pass.
- Mac `mini`: all plug-in libraries compile and workspace clippy passes. The
  2,198 library tests outside Minitosis pass. Minitosis aborts in
  `tests::closed_editor_switch_prepares_and_adopts_without_audio_thread_allocation`
  with a 64-byte allocation failure; this reproduces on untouched master
  `4dc15e5a3ecb51bbc6931bdaf9302a0ccadc8691` and is independent of this GUI work.
- Native Mac probe: a click lands correctly at 1×; a programmatic resize from
  420 × 280 to 600 × 418 persists logical dimensions, and reopening retains
  600 × 418 without growth. `mini` has no attached physical display and its
  private Glass session reports backing scale 1. Attempts to expose a temporary
  HiDPI display did not produce a usable screen; Glass also hit capture-query
  timeouts. Test processes/displays were removed. Genuine 2× Mac runtime
  verification remains outstanding; 1.5×/2× cases have regression coverage.

## Reproducing native geometry checks

`cargo run -p tiny-skia-widgets --example dpi_probe -- /tmp/editor-dpi-state.json`
opens a native window using the production wrapper and logs actual dimensions,
backing scale, persisted size, and physical pointer delivery. Its optional test
environment variables are `DPI_PROBE_RESIZE=w,h` (logical target dimensions),
`DPI_PROBE_EXIT_AFTER=n` (exit after n render callbacks), and, on macOS,
`DPI_PROBE_LEFT`/`DPI_PROBE_TOP` (Cocoa screen-point placement). The state file
allows repeated launch/save/reopen checks without a DAW.
