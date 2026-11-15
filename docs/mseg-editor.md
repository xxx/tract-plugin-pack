# Shared MSEG curve tools

Multosis, Minitosis, Miff, and Nap share these editing tools. The **Tool**,
**Shapes**, and **Edit** menus sit beneath the existing curve controls.
Multosis/Minitosis provide playback and modulation routing; Miff/Nap use the
same geometry tools for their static kernel/tail curves.

## Shape pens

Choose **Line pen**, **Arc pen**, or **S-curve pen** from Tool. The white preview
shows the next placement. Click to stamp, or drag to set its width, height, and
direction. The pen remembers those dimensions. Clicking again without moving
the pointer places another copy immediately after the previous one. Moving the
pointer chooses a new starting position. Leftward dragging makes repetition
proceed leftward. Repetition stops at the edge of the canvas.

Pens replace points within their span and preserve points outside it. S-curves
use 16 short linear segments; arc pens use the existing tension interpolation.
Sharp seams between repeated shapes use a very short linear transition.
**Escape** cancels the current gesture and returns to Pointer.

## Selecting and transforming

Pointer retains the existing node, tension-handle, marquee, and Ctrl-click
selection gestures. Two or more selected points display a box with eight handles:

- Side handles stretch time; top/bottom handles stretch level; corners do both.
- **Alt** scales around the selection's center.
- **Ctrl** while dragging an edge handle skews the other axis.
- **Shift** bypasses snapping while dragging.

Endpoint times stay pinned to 0 and 1. Other points cannot cross their neighbors.
Dragging back to the initial handle position restores the initial geometry,
including after a boundary was reached. Escape cancels the gesture.

## Reusing curves

Shapes includes eight factory curves: Flat high, Flat center, Ramp up, Ramp down,
Triangle, Sine, Square, and Duck. It also provides eight shared favorite slots:
**Store favorite N** replaces that slot; **Load favorite N** recalls it. Favorites
are saved in the platform's application-data directory and survive closing the
plugin. All four plugins use the same library.

**Copy curve** and **Paste curve** use a dedicated Tract curve clipboard, shared
through that directory between instances, plugin formats, and host processes.
They leave the system's text clipboard alone. Copy always captures the entire
curve. **Paste fitted to height** scales the incoming values to the current
curve's minimum/maximum; a flat incoming curve maps to that range's midpoint.

**Import shape...** and **Export curve...** open native file dialogs for versioned
JSON files. Library operations transfer only point positions, levels, tension,
and steps. They preserve the destination's length, clock, play mode, grid,
polarity, routing, and modulation depth. Replacing a complete shape clears its
sustain/loop markers. Missing favorites, unavailable storage, invalid files, and
capacity errors appear beside the menus without replacing the curve.

## Edit commands

- **Compress/Expand values/times** retain their existing behavior and require a
  selection. Each invocation scales the selected axis by 0.75 or 1.25 around
  its mean.
- **Flip values** inverts selected levels, or the entire curve if nothing is
  selected.
- **Reverse time** reflects a contiguous selection, or the entire curve.
  Curvature reverses with it. Reversed stepped segments use a short linear
  transition to retain the original held level.
- **Nudge points left/right** moves selected interior points one time-grid cell,
  constrained by neighbors. With no selection it moves all interior points.
- **Quantize points** snaps selected points, or all points, to the current
  time/value grids. If times would coincide, the command leaves the curve intact.
- **Repeat 2x/3x** fits two or three copies into the same time span. It requires
  a contiguous selection, or operates on the full curve when nothing is selected.
- **Mutate values** varies existing selected levels, preserving rhythm and point
  count. **Mutate shape** also varies time and curvature while retaining the
  selection's time boundaries. With no selection, either mutates the full curve.
  The existing Randomize button still generates a fresh curve in its chosen style.

Curves remain limited to 128 points. Operations that would exceed that limit
or overlap points are rejected as a whole. The grid menu adds **6 / 8**,
**12 / 8**, and **24 / 16** for triplet subdivisions.

## Keyboard and history

| Shortcut | Action |
|---|---|
| Ctrl/Cmd+A | Select all points |
| Ctrl/Cmd+C | Copy curve |
| Ctrl/Cmd+V | Paste curve |
| Ctrl/Cmd+Shift+V | Paste fitted to height |
| Left / Right | Nudge selected interior points by one time-grid cell |
| Shift+Left / Right | Nudge by 1/16 of a cell |
| Delete / Backspace | Delete selected interior points |
| Escape | Close a menu or cancel a curve gesture |

Menus accept arrows, Home/End, Page Up/Down, Enter, Escape, and wheel scrolling.
Active text fields retain keyboard priority. Multosis/Minitosis record each
curve gesture or command as one undo step. Miff rebakes on changes. Nap defers
Efficient-engine IR baking during a gesture and bakes on release.
