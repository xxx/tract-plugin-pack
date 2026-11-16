# MSEG loop-seam tools

Continue `feat/mseg-power-tools` after detector controls (`e2b134f`). The user
authorized the loop-seam slice from the original wishlist. Extend the existing
CPU editor; no mock supplied.

## Behavior

1. Edit adds **Match loop end to start**, **Match loop start to end**, and
   **Smooth loop seam (10%)**. These operate on the hold-loop bounds when the
   full editor has a loop region, otherwise the whole curve. Curve-only Miff/Nap
   always use the whole curve. Selection does not change the operation's scope.
2. Matching copies one endpoint's level to the other, preserving point times,
   segment styles, weights, routing, timing, and hold markers. It only matches
   node levels: a final stepped segment may still have a wrap discontinuity.
3. In cyclic playback or a configured hold loop, an amber dashed indicator shows
   a level discontinuity at each visible loop boundary. The status hint identifies
   the seam jump. Read the actual incoming segment's left limit, including steps
   and extremely short segments; equal endpoint nodes alone do not prove a seam
   is continuous. One-shot curves without a loop and static curves do not imply
   a playback wrap or show that automatic indicator. Zoom/pan clip it to the plot.
4. Smooth makes the end level equal the start and eases the start to a flat
   tangent. It inserts a Soft point carrying the original incoming end level,
   followed by a native S-curve return to the start level. The return starts 10%
   of the loop span before the end, or halfway through the final segment if
   existing points already occupy that window. Existing interior points remain.
   The new point eases the incoming curve to meet the return's flat tangent.
5. Smooth edits authored geometry, rather than enabling a hidden playback filter.
   It can change the first/last loop segments and a shared boundary's adjacent
   interpolation. It adds at most one point and preserves the old peak in a
   two-point ramp instead of flattening the entire curve. Stepped held levels
   are used rather than an unused terminal node's value. Already matched, flat
   seams are a no-op; reapplying Smooth does not add another point.
6. All structural edits are transactional. Full capacity or no representable
   insertion position reports an error and leaves data/selection unchanged.
   Hold indices and selected nodes follow the original points across insertion.
7. Each mutation is one local undo entry in Multosis/Minitosis. Miff rebakes;
   Nap retains deferred bake rules. No-op/errors do not bake or create undo.
   Commands remain accessible with mouse, keyboard, and wheel in the existing
   final-pass menu. Numeric focus, modal ownership, DPI, and resize rules remain.
8. The existing curve/project formats and shared sampler carry the resulting
   native segments/weights. Unedited documents sample identically; no new audio
   state, allocation, host parameter, or latency is introduced.

## Implementation and validation

- Share a small geometry-only seam analysis/operations module; use existing
  candidate validation and marker remapping. Match mutates one level; Smooth
  commits a complete validated candidate. Keep old-node selection by insertion
  index and discard stale hover/pen previews after a structural edit.
- Reuse Edit menu routing and clipping primitives. Append menu commands so old
  item indices remain stable; update tests that used End to mean Soft points.
- Test stepped and short-segment limits, full/hold/static scope, endpoint-only
  matching, smooth slopes/bounds/peak preservation, idempotence, capacity and
  rounding rejection, marker/selection identity, serialization/clipboard,
  menu input/rendering at 0.5x/1x/2x, and one-step undo. Verify Miff/Nap sampling
  against resulting geometry and render an offscreen before/after preview.
- Run the six-crate focused suite, workspace Clippy, format/whitespace checks,
  and independent read-only reviews per AGENTS.md. Leave changes uncommitted.
