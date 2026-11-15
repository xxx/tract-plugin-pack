# Shared MSEG curve tools

Multosis, Minitosis, Miff, and Nap share these editing tools. The **Tool**,
**Shapes**, **Edit**, and **View** menus sit beneath the existing curve controls.
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
use one native segment with two endpoints; arc pens use tension interpolation.
Sharp seams between repeated shapes use a very short transition. Previews use
the same boundary weights as the committed shape.
**Escape** cancels the current gesture and returns to Pointer.

### Native shapes and point weights

The Point inspector's **Shape** button cycles **Curve → S-curve → Step**. Curve
uses the existing linear/exponential tension law. S-curve provides flat endpoint
tangents without adding intermediate points; Tension bends its progression.
Step holds its starting level until the following point. The last point's
unused Shape control is disabled.

**Weight** cycles **Hard → Medium → Soft**. Hard keeps the original behavior;
Medium halves the endpoint slope; Soft eases to a flat tangent at that point.
Times and levels remain exact. A Soft point in a rising curve creates a brief
pause; it does not move the point or average its level with its neighbors.
Hard points are filled, Medium points have a center dot, and Soft points are hollow.

Native S-curves already have flat endpoints, so weights do not alter them.
Steps retain their hard jumps. A point's weight may still affect the other
adjacent Curve segment, including the incoming segment at the final point.
All new interpolation stays between its endpoints and uses the same sampler
for drawing, live modulation, Miff tap baking, and Nap sequence generation.

Edit offers **Curve/S-curve/Stepped segments** and **Hard/Medium/Soft points**.
These act on the selected points, or all points when nothing is selected.
Segment commands skip the last point. Inspector style clicks first apply valid
pending numeric input; invalid text blocks the click. Each click/command is
one undo step in Multosis/Minitosis.

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
JSON files. Library operations transfer point positions, levels, tension,
steps, native shapes, and weights. They preserve the destination's length, clock, play mode, grid,
polarity, routing, and modulation depth. Replacing a complete shape clears its
sustain/loop markers. Missing favorites, unavailable storage, invalid files, and
capacity errors appear beside the menus without replacing the curve.

Old projects and version-1 curve files load with Curve/Hard defaults and retain
their original sound. Legacy-only exports stay version 1. Native shapes or
non-Hard weights use version 2, which older builds reject. Use current plugin
builds for projects containing new node metadata; older project readers ignore
the added attributes and cannot reproduce those shapes.

### Copying and pasting a phrase

Select at least two adjacent points, then choose **Copy selection** from Edit.
The clipboard stores that phrase with its time normalized to 0..1. Levels,
tension, steps, native shapes, and interior point weights retain their shape.

Select an interval in the destination and choose **Paste replace selection** to
fit the phrase into that interval. **Paste merge selection** retains existing
interior points; pasted points win when their times coincide. Points outside
the interval and the outgoing segment after its last point are preserved.
Existing boundary points keep their outside-facing weights so adjoining
segments retain their styles. Full-curve replacement transfers every weight.
Both modes select the resulting interval and reject the entire operation if
it would exceed 128 points or create overlapping times.

## Precise points and navigation

Select one point and click **Point**, choose **Inspect point** from Edit, or press
**Ctrl/Cmd+Enter**. The floating inspector provides exact **Time**, **Value**, and
outgoing **Tension** fields. Time uses beats in Beat mode, milliseconds in Time
mode, or percent in Miff/Nap. Bipolar values use -1..1. The shared effect editor
shows the point's routed parameter value (or Amp dB) beneath the fields.

Focusing a field selects its contents for replacement. Enter applies that field;
Tab applies and moves to the next editable field (Shift+Tab moves back). Escape
closes and discards uncommitted text. Clicking outside applies valid input and
closes; invalid input keeps the inspector open with an error. Right-click
dismisses it. Endpoint times, stepped tension, and the last point's unused
tension are read-only. Exact numeric input bypasses the snap grid.

**Wheel** over the canvas zooms time around the pointer; **Shift+wheel** zooms
levels. **Middle-drag** pans. **View → Fit all** restores the full curve;
**Fit selection** frames selected points with a little padding. Zoom is limited
to 64x. Grids and snapping stay in document coordinates. Menus and the inspector
consume wheel input while open. Escape cancels an in-progress pan.
Pending dial/Mix/Output numeric input is finalized before panning starts;
Escape cancels the pan while keeping the committed parameter value.

Navigation does not alter the saved curve or create undo/bake work. Viewports
are retained across resizing in the current editor session. Nap's panes have
independent views, and its pulse field follows the Decay pane's view.

## Live smoothing and trace

Multosis/Minitosis provide **Smooth** and **Trace** at the lower right of the
curve pane. Smooth applies a lag to the selected envelope's output before
Depth and Target mapping. It works on Amp, Mix, continuous effect targets, and
unrouted envelopes. Selector targets show **n/a** and bypass smoothing while
retaining the saved time.

Smooth ranges from **Off** to **1000 ms**. Drag to adjust, Shift-drag for fine
adjustment, double-click to reset to Off, or right-click to enter milliseconds.
The time is the lag's time constant: after a step, roughly 63% of the change is
reached in that time. Settings are saved per envelope and are undoable. Shape
clipboard/files transfer geometry and leave Smooth unchanged. Old projects
default to Off, preserving their existing modulation behavior.

Positive Smooth keeps running during bypass and on inactive rows. Retriggers,
loop wraps, and transport restart retain the lag so they do not reset its level.
Explicit Reset clears it; a DSP flush clears it while preserving phase. Amp and
Mix update each sample; continuous effect targets update on a 1 kHz control
clock. Existing Free Hz and Transient triggers still align to processing blocks.

Trace toggles the white measured source history. It shows actual envelope
output before Depth/Target mapping; colored dial arcs show values delivered to
their destinations, including Mix transitions. Trace follows zoom/pan and
clears when switching envelopes, rows, or effects. It fills as audio is processed
and hides while Minitosis prepares a newly selected effect. Toggling Trace does
not change the sound or project state.

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
| Ctrl/Cmd+Alt+C | Copy selection |
| Ctrl/Cmd+Alt+V | Paste replace selection |
| Ctrl/Cmd+Alt+Shift+V | Paste merge selection |
| Ctrl/Cmd+Enter | Inspect selected point |
| Wheel / Shift+wheel | Zoom time / values |
| Middle-drag | Pan the view |
| Left / Right | Nudge selected interior points by one time-grid cell |
| Shift+Left / Right | Nudge by 1/16 of a cell |
| Delete / Backspace | Delete selected interior points |
| Escape | Close a menu or cancel a curve gesture |

Menus accept arrows, Home/End, Page Up/Down, Enter, Escape, and wheel scrolling.
Active text fields retain keyboard priority. Multosis/Minitosis record each
curve gesture or command as one undo step. Miff rebakes on changes. Nap defers
Efficient-engine IR baking during a gesture and bakes on release.
