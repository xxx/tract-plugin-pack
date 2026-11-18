# Native MSEG shapes and point softness

Date: 2026-11-14. Next slice authorized in conversation after precision editing.
Continue on `feat/mseg-power-tools`; reuse the existing UI (no mock supplied).

## Behavior

1. S-curve pens create one native segment between two endpoints rather than
   16 sampled linear segments. Tension remains editable and bends the S-curve.
2. Every point has Hard (default), Medium, or Soft weight. Hard retains the
   original segment behavior; Medium halves the endpoint slope, and Soft eases
   to a flat tangent at that point. Point times and levels remain exact. This
   deliberately creates a pause at a soft point in an otherwise rising curve.
3. Native S-curves already have flat endpoint tangents. Point weights leave
   that segment's shape unchanged. Stepped segments retain their hard jumps;
   weights may still affect an adjacent non-stepped segment.
4. Interpolation is bounded and monotone within each segment. Rendering, live
   modulation, Miff tap baking, and Nap sequence generation use the same rule.
   Sampling allocates nothing and the fixed 128-point capacity remains.
5. Point adds Shape (Curve / S-curve / Step) and Weight (Hard / Medium / Soft)
   cycle controls. The last point's unused shape is disabled. Mode/weight clicks
   first commit valid pending numeric input, as one undoable click; invalid
   numeric text blocks the action. Existing Enter/Tab/Escape and popup ownership
   rules remain intact. Point glyphs distinguish weights.
6. Edit commands apply Curve/S-curve/Step to selected outgoing segments, or all
   when nothing is selected. Hard/Medium/Soft apply to selected points, or all.
   Existing shortcuts and right-click step toggling keep their behavior.
7. Copy, favorites, import/export, repeat, reversal, and local paste retain
   native segment shapes and weights. Reversal transfers the outgoing shape to
   its reversed segment, reverses tension, and mirrors weights with the points.
8. Interval replacement preserves outside-facing boundary weights (when the
   boundary coincides with an existing point) and the final outgoing segment's
   metadata, so edits do not silently change adjoining segment styles. Full-curve
   replacement transfers all weights. Discontinuous seams keep short transitions.
9. Old projects and curve files load as Curve/Hard and sample identically.
   Legacy-only curve files continue to export version 1 without extra fields.
   Files using native shapes/weights export version 2; both versions load in the
   new editor, and older editors reject the new version rather than lose shape
   information. New projects retain the added node metadata on reload.

## Technical approach and validation

- Add serde-defaulted, skip-default `shape`/`weight` enums to MsegNode. Keep the
  existing tension and stepped fields. Check the current 16-byte node footprint.
- Retain the legacy Hard/Curve evaluation path exactly. Otherwise compose the
  existing exponential warp with a monotone cubic Bezier progress function whose
  endpoint slopes are 1 / 0.5 / 0 for Hard / Medium / Soft. Native S forces both
  slopes to zero. No neighbor search or dynamic allocation is needed.
- Expose one pure segment evaluator and use it from the full lookup and both
  optimized forward-only samplers. Preserve their O(samples + nodes) cursor walks.
- Test old serialization and sampled bits, new format round trips, endpoints,
  monotonicity/bounds, weighted slopes, step discontinuities, metadata operations,
  inspector/multi-selection/undo, and bake-versus-display equivalence.
- Exercise native/weighted sampling and the effect path under the existing
  no-allocation guard. Render an offscreen preview and run the six-crate focused
  suite, workspace Clippy, formatting, and independent review.
- Leave changes uncommitted unless separately requested.
