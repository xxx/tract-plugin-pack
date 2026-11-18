# MSEG smoothing and trace implementation

- [x] Record timing, compatibility, and transition rules.
- [x] Add persisted settings and tested allocation-free smoothing runtime.
- [x] Integrate per-sample advancement and held targets in both engines.
- [x] Add request-aware atomic snapshots and bounded measured trace.
- [x] Add Smooth/Trace controls, actual arcs, and viewport-aware overlay.
- [x] Connect native editor lifetime, pending selection, and reorder invalidation.
- [x] Add integration/allocation coverage, documentation, and offscreen preview.
- [x] Resolve review findings and complete focused tests plus lint/format checks.

Verification completed:

- 1,667 tests passed across tiny-skia-widgets, tract-effects, Multosis, Minitosis,
  Miff, and Nap, including allocation guards and buffer partition comparisons.
- Workspace Clippy (`-D warnings`, offline), formatting, and whitespace checks passed.
- `cargo run --offline -p tract-effects --example mseg_smoothing -- /tmp/mseg-smoothing.png`
  rendered a DSP-driven preview; inspected the source trace, target arc, and controls.
- Independent DSP/engine and telemetry/UI reviews completed. Resolved post-flush
  base restoration, hidden-view recording, replacement epochs, and trace clipping.

Native/DAW interaction and listening checks remain manual.
