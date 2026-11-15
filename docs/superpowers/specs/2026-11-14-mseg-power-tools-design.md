# MSEG power tools

Date: 2026-11-14. Branch: `feat/mseg-power-tools`.
Scope approved in conversation: shared editing tools from the ShaperBox comparison.
No Figma mock supplied; extend the existing CPU-rendered editor.

## Behavior

1. Multosis, Minitosis, Miff, and Nap expose the same curve tools. Static
   consumers retain their curve-only layout and bake behavior.
2. A second control-strip row exposes Tool, Shapes, and Edit menus without
   replacing the existing snap, grid, polarity, playback, and randomizer controls.
3. Pointer remains the default. Line, Arc, and S-curve pens preview their
   placement. Drag defines the endpoints; clicking stamps the remembered shape,
   and repeated clicks stamp it again. Pens replace points only in their span.
   S-curves use a bounded piecewise-linear approximation, preserving old presets.
4. A selection has edge/corner handles. Drag stretches time and/or level;
   Alt scales around its center, Ctrl on an edge skews the opposite axis, and
   Shift bypasses snap. Geometry is measured from gesture start. Pinned endpoint
   times and unselected neighbors constrain transforms. Escape cancels a gesture.
5. Copy/Paste transfers shape only, across instances and plugins. Timing, grid,
   polarity, playback mode, target, and depth remain those of the destination.
   Paste fitted to height keeps the destination's existing value range.
6. Factory shapes include flat, ramp, triangle, sine, square, and ducking shapes.
   Eight persistent favorite slots are shared across plugins. Shapes can also
   be imported/exported as versioned JSON. Empty favorites and I/O/format errors
   appear in the editor, and leave the current curve intact.
7. Copy uses a Tract curve clipboard in the shared user data directory, rather
   than the system text clipboard; it also works between different host processes.
8. Mutate values preserves times and point count. Mutate shape also varies times
   and curvature, while retaining point count and the selection's time boundaries.
   Both operate on selected points, or all points if nothing is selected.
9. Edit offers vertical/horizontal flip, grid/fine nudges, quantize, and 2x/3x
   repeat. Reverse/repeat require a contiguous selection (or operate on the full
   curve). Reversed steps and discontinuous repeat seams use a short linear
   transition where the current segment representation cannot express the jump.
10. Structural edits are atomic: overflow beyond 128 points, ambiguous overlapping
    points, and invalid imports report an error without partially editing a curve.
    Quantize rejects coincident times instead of silently dropping nodes.
11. Shape replacement clears sustain/loop markers. Interval replacement remaps
    markers by surviving time positions, clearing markers whose points disappear.
    Selection follows structural replacements and cannot retain stale indices.
12. Ctrl/Cmd+A/C/V select/copy/paste; Shift+paste fits height. Arrow keys nudge
    selected points; Shift nudges finely. Delete/Backspace delete selection.
    Menus accept arrows, Home/End, Page Up/Down, Enter, Escape, and wheel scrolling.
    Text fields retain keyboard priority. Shortcuts do not mutate an active drag.
13. Each gesture/menu/keyboard mutation is one undo step in Multosis/Minitosis.
    Miff rebakes on edits; Nap preserves deferred Efficient baking during gestures.
14. Triplet grid choices are available alongside existing straight grids.
15. Open menus draw above editor controls and host overlays. A right-click
    dismisses an open menu without activating or editing a control beneath it.

## Technical approach

- Keep `MsegData` and DSP interpolation unchanged. Add GUI-only shape operations,
  storage, and interaction state under `tiny-skia-widgets/src/mseg/`.
- Use candidate documents and validate before commit. Preserve strict time order,
  pinned times 0/1, finite normalized levels/tension, and fixed capacity.
- Persist only normalized node geometry in bounded, versioned JSON. Use the shared
  application-data directory for clipboard/favorites; filesystem and dialogs run
  only from explicit GUI actions. Writes use temporary files plus rename.
- Route MSEG popup input before plugin controls/pane picking. Centralize keyboard
  and menu commands in the widget so adapters only report changes and bake/undo.
- Review independently per AGENTS.md; implementation stays in this checkout.

## Validation

- Shape tests: metadata preservation, file round trips, invalid/oversized inputs,
  fit-to-height, reverse interpolation/steps, repeat seams/capacity, mutation scope,
  and transactionality on collisions.
- Gesture tests: pen stamping/drag/repetition, handle scaling and return from
  clamping, anchor/neighbor constraints, selection identity, and Escape rollback.
- Integration tests: shared menu/shortcut routing, text priority, Minitosis Ctrl
  shortcuts, and Nap popup ownership. Render an offscreen preview of the tools.
- Run focused nextest for widgets/effects and all four consumers, workspace Clippy,
  and formatting. No commits or publishing without a separate user request.
