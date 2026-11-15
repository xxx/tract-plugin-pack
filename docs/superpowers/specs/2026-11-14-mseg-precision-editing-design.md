# MSEG precision editing

Date: 2026-11-14. Scope approved in conversation after the power-tools milestone.
Continue on `feat/mseg-power-tools`. No Figma mock supplied; reuse existing styling.

## Behavior

1. All four consumers share a Point inspector, View controls, and phrase clipboard
   commands. Existing project and curve JSON formats remain compatible.
2. With exactly one point selected, Point (or Ctrl/Cmd+Enter) opens an inspector containing
   time, value, and outgoing tension. Time is beats in Beat mode, milliseconds in
   Time mode, and percent for static curves. Bipolar values display -1..1. Pinned
   endpoint times and unused/stepped tension fields are read-only.
3. Focusing a field selects its text for replacement. Enter applies one field;
   Tab applies and advances (Shift+Tab reverses). Escape discards uncommitted text
   and closes. Outside-click commits valid input and closes; invalid input stays
   visible with an error. Non-finite, out-of-range, and neighbor-crossing input
   never alters the document. Exact numeric input bypasses snap.
4. The shared effect editor shows the selected point's routed parameter value
   (or Amp dB) in the inspector, updating with host base values/depth. Static
   consumers can provide their own readout. Inspector input stays above underlying
   controls; right-click dismisses without activating those controls.
5. Wheel over the canvas zooms time around the pointer; Shift+wheel zooms values.
   Middle-drag pans both axes. Zoom is bounded to 1x..64x within the curve's 0..1
   domain. View offers Fit all and Fit selection, with padding/minimum spans.
   View changes do not alter curve data, bake kernels, or create undo entries.
6. Drawing, ghosts, playheads, grid, node picking, selection handles, marquee,
   stepped drawing, and pens use the same viewport. Offscreen geometry is clipped
   and cannot generate phantom hits. Snap remains tied to document coordinates.
7. Menus and the inspector consume wheel input. Editing gestures inhibit zoom;
   pending parameter numeric input auto-commits before a pan starts, so Escape
   cancels that pan rather than discarding the parameter edit.
   Resizing ends gestures/overlays and preserves the viewport. Escape cancels pan.
   Nap routes pan/overlay events to their owning pane across pane boundaries.
8. Copy selection captures a contiguous phrase with at least two points, normalizes
   its time span to 0..1, and uses the existing shared curve clipboard. Levels,
   steps, and tension are retained. Whole-curve copy/paste remains available.
9. Paste replace selection fits the clipboard phrase into the selected contiguous
   time interval and replaces its points. Paste merge selection retains existing
   interior points; pasted points win at identical times. Both preserve outside
   points, outgoing metadata after the interval, timing/routing settings, and
   valid hold markers. The resulting interval is selected.
10. Invalid selection, rounding collisions, and capacity overflow reject the
    entire operation. Ctrl/Cmd+Alt+C copies selection, Ctrl/Cmd+Alt+V replaces,
    and Ctrl/Cmd+Alt+Shift+V merges. Existing whole-curve shortcuts are unchanged.
11. Each field commit or paste is one undo entry in Multosis/Minitosis. Miff
    rebakes discrete edits and Nap preserves its gesture-deferred IR baking.

## Technical approach and validation

- Add a GUI-only viewport and state-derived layout; keep default layout mappings
  compatible. Clip rendered segments and skip offscreen hit targets.
- Keep inspector state in the shared widget and render it in the final popup
  pass. Consumers supply mapped value text; keyboard focus remains centralized.
- Add candidate-based phrase capture/paste operations using the version-1 shape
  format and existing transactional interval replacement.
- Test pointer-anchored zoom, pan bounds, fit selection, mapped-coordinate
  round trips, clipping, numeric validation/units, popup ownership, resize,
  selection clipboard scope/capacity, undo, and view-only change reporting.
- Run focused tests across widgets/effects/all four consumers, workspace Clippy,
  format checks, and an offscreen preview. Dispatch a read-only independent
  review per AGENTS.md. Leave changes uncommitted unless separately requested.
