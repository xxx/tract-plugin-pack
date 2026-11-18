# Native MSEG shapes implementation

- [x] Record scope, weighting behavior, and compatibility rules.
- [x] Add tested native interpolation and backward-compatible node metadata.
- [x] Unify optimized bake samplers and verify allocation-free playback.
- [x] Preserve shape/weight semantics through pens and structural operations.
- [x] Add inspector, Edit commands, and point glyphs.
- [x] Add interaction coverage, documentation, and offscreen preview.
- [x] Review, resolve findings, and run focused tests plus lint/format gates.

Validation: 1,649 focused tests passed across widgets, tract-effects, and all four
MSEG consumers. Workspace Clippy (`-D warnings`), formatting, and whitespace
checks passed. Legacy samples are bit-identical; optimized bakes match the shared
sampler; native/weighted playback passes the existing allocation guard.
Independent review is clear after the weighted preview regression was fixed.
The `mseg_shapes` example renders the new controls and shape comparisons offscreen.
DAW/native interaction smoke testing was not performed.
