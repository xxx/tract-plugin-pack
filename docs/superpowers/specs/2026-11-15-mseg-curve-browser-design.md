# Visual MSEG curve browser

Continue `feat/mseg-power-tools` after context-help commit `8d699e8`. The user
authorized the next slice. Extend the existing CPU editor; no mock supplied.
Reference: [ShaperBox Wave Presets](https://downloads.cableguys.com/Cableguys-ShaperBox-3-Manual.pdf#page=35).

## Behavior

1. Multosis, Minitosis, Miff, and Nap offer **Shapes → Browse shapes...** above
   the existing library commands. The browser has Factory and Favorites tabs,
   each showing eight labeled thumbnails. Previews use the actual native curve,
   tension, step and weight interpolation over the full normalized domain.
2. Clicking a thumbnail selects it without altering the destination. Arrows and
   Home/End navigate; Tab switches tabs. **Load** (Enter) applies the selected
   curve; **Fit** (Shift+Enter) fits it to the destination's current value range.
   Double-clicking a populated thumbnail loads it normally. Successful loading
   closes the browser and uses the same geometry replacement as existing shapes.
3. Favorites are the existing eight shared, persistent slots. Empty slots show
   Empty; invalid/unreadable slots show Unavailable and cannot load. The browser
   reads a snapshot on opening; **Refresh** (R) rereads it, including changes from
   another instance. Drawing, hover and ordinary navigation do no filesystem I/O.
4. On Favorites, **Store** (S) saves the destination's current curve into the
   selected slot, including occupied slots. **Clear** (Delete) removes that
   slot. These explicit actions update the browser and report success/failure;
   they do not change the destination or create bake/undo work. Factory entries
   cannot be overwritten or removed. Missing files are already empty when cleared.
5. Loading transfers geometry only, preserving timing, grid, polarity, playback,
   routing, depth and smoothing. Whole-shape replacement clears hold markers and
   stale selection as before. Changed geometry/markers is one undo step in
   Multosis/Minitosis and follows existing Miff/Nap bake rules. Errors and no-op
   loads do not report document changes or create history entries.
6. The browser is modal and drawn in the final popup pass above all editor and
   modulation controls. Mouse buttons, wheel and keyboard stay with its opening
   pane, including across Nap panes. Escape, X, an outside left-click or any
   right-click dismisses it without activating an underlying control. Other
   shortcuts, middle-drag, key-ups and wheel cannot edit the curve beneath it.
7. Opening, browsing and dismissing preserve the curve, selection, viewport,
   tool, remembered pen repeat and display preferences. Existing numeric fields
   finalize through the normal outside-press path before Shapes opens. Existing
   gestures and overlays retain priority over programmatic browser entry.
   Resize, focus loss and view/binding replacement close it through cancellation.
8. The panel and controls fit the native window, including 0.5x/1x/2x and narrow
   bounds. Selected shape/slot status remains readable; thumbnail text cannot
   spill into adjacent cards. Admission reserves visible curve-preview space
   after card padding and label height. Windows too small for the cards show
   **Enlarge the window to browse curves** without opening or capturing input invisibly.
   Static consumers share the same library behavior.
   No project format, host parameter, MIDI, DSP, latency or audio-thread changes.

## Implementation and validation

- Add a shared browser module containing cached entries, modal input, bounded
  layout and thumbnail drawing. Reuse shared overlay bounds and shape application.
- Add bounded favorite removal to CurveLibrary, retaining existing file formats
  and atomic store behavior. Keep filesystem operations on explicit GUI actions.
- Test preview selection/cancellation and pen state, load/fit metadata and no-op
  reporting, native shape rendering, empty/corrupt/failed storage, cross-instance
  refresh, disabled factory actions, all input ownership, layering and pane routing.
- Render actual browser states offscreen; update shared/plugin manuals. Run the
  six-crate focused suite, workspace Clippy, format/whitespace, and independent
  read-only reviews. Leave changes uncommitted until requested.
