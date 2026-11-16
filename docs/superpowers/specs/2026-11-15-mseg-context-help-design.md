# MSEG context help and gesture guide

Continue `feat/mseg-power-tools` after semitone-grid commit `b170686`. The user
authorized the next original-wishlist slice. Extend the existing CPU UI; no mock
supplied. This is shared GUI behavior for Multosis, Minitosis, Miff, and Nap.

## Behavior

1. The existing footer shows concise context hints for MSEG controls, nodes,
   selection handles, pens, and active gestures. Full effect editors also supply
   hints for modulation controls. Explicit operation errors retain priority;
   idle seam/grid/view messages remain available when no contextual hint applies.
   Hints occupy a full-width footer row below the two control rows, including
   at Minitosis's default size and under uniform zoom.
   Moving outside a pane/window clears its hover hint without changing data or
   pen placement. Initial unopened-pointer state has no phantom hover.
2. A **?** button beside Point, **View → Editing guide...**, and **F1 / ?** open
   a four-page guide: Points, Select, Shapes, and View. Text documents actual
   supported gestures and shortcuts. Static consumers omit playback, musical
   routing, and local-undo instructions unavailable in those plugins.
3. The guide fits within supplied native-window bounds, with a bounded fallback
   for standalone widgets. Text wraps, each page scrolls, and long content remains
   accessible at 0.5x/1x/2x and in Nap's first pane. Wheel/up/down/Page Up/Down
   scroll; left/right/Tab change pages; Home/End jump within a page.
   Page Up/Down advance by the visible body height with two lines of overlap;
   pixel offsets clamp to the actual content overflow so no content is skipped.
4. The guide is modal and draws in the final popup pass. All mouse buttons,
   wheel, and keys are kept above underlying curves, effect controls, and other
   panes. Escape/F1/? or X closes it; an outside left or right press dismisses
   it without acting beneath; right-click also dismisses it from inside. Keyboard
   focus and event ownership stay with the opening pane even when the pointer
   crosses into another pane.
5. Numeric editors and existing popups retain keyboard priority. Help shortcuts
   are consumed without opening a guide during any drawing/pan or host dial
   gesture. Clicking help finalizes existing host numeric input through the
   existing outside-press path. Opening/closing guide alone preserves selection,
   viewport, tool, pen placement, and display/freeze preferences.
6. Resize/focus loss/view replacement/window close end transient input and close
   the guide through existing cancellation rules. Cursor leave clears hints;
   cleanup runs before detector-modal and binding-change early returns. Cursor
   leave does not close the guide. Page/scroll/hover state is GUI-only.
7. Help never changes serialized curves, triggers, routing, audio, bake/undo
   state, telemetry requests, latency, or host automation. No new DSP work,
   audio-thread allocations, external dialogs, or documentation network access.

## Implementation and validation

- Keep guide content, state, wrapping, layout, and input in a shared editor help
  module. Reuse the existing control geometry/hit rules for hover hints and add
  the guide to popup ownership. Keep help input separate from geometry mutation.
- Draw the guide opaquely after normal overlays; clip wrapped body text and
  keep header, tabs, close control, and scroll feedback inside its panel.
- Supply native bounds from full/static adapters and clear stale hints on pane
  changes/cursor leave. Reuse Nap's existing modal-pane routing; guide interactions
  must not schedule regeneration or baking.
- Test contextual picking and no phantom hover; button/menu/shortcut entry;
  numeric and gesture priority; all mouse/keyboard/wheel routes; full/static
  content; scroll/page bounds and text/panel clipping; final popup layering;
  cross-pane ownership; focus/resize cleanup; and state/bake/undo preservation.
- Render actual shared guide and hover states offscreen. Update manuals and run
  six-crate focused tests, workspace Clippy, formatting/whitespace checks, and
  independent read-only reviews. Leave changes uncommitted unless requested.
