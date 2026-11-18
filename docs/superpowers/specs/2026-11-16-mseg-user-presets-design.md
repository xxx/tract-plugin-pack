# Named MSEG user presets

Continue `feat/mseg-power-tools` after `4d43f68`. The user authorized the next
library slice: named user presets with save, rename and delete. Extend the
existing CPU browser; no mock supplied. Categories and search follow separately.

## Behavior

1. Multosis, Minitosis, Miff and Nap add a **User** tab after the existing browser
   tabs. It shows shared, persistent named curves, eight thumbnails per page,
   ordered by Unicode lowercase name. Previous/Next and Page Up/Down change pages;
   arrows and Home/End select entries and keep the selection visible. Factory,
   Favorites and Project retain their existing order and behavior.
2. **Save** (S) captures the current authored curve and opens a name field.
   Enter or Save creates a new preset and selects it. Names are trimmed, contain
   1–64 Unicode characters without controls, and are unique under Unicode
   lowercasing (rather than locale-specific collation or full case folding).
   Duplicate/invalid names leave the field open with an error. The library holds
   up to 256 presets; reaching its limit reports an error without replacing data.
3. **Rename** (F2) starts with the selected preset's name. Enter or Rename changes
   that entry's name while retaining its geometry and identity. Text supports
   Unicode, selection-all, caret movement, Backspace and Delete; management and
   curve shortcuts cannot run through the field. Escape cancels the draft before
   closing the browser. Save/Rename never change the current curve or local undo.
   Printable text delivered with Option/Alt, including Shift, enters the name;
   Control/Command shortcuts and nonprintable modifier events remain contained.
4. **Delete** (Delete/Backspace) opens a confirmation showing the selected name.
   Only Enter or the visible Delete button removes it; Escape/Cancel return to
   the browser. Selection clamps after deletion, including an empty last page.
   Confirmation names wrap to the available space; overflow is reachable with
   Up/Down, Home/End, Page Up/Down or wheel, with a visible scrollbar. Instructions
   and errors have their own space above the action buttons.
   Factory, Favorites, Project and the clipboard remain unchanged.
5. **Load** (Enter), **Fit** (Shift+Enter), and ordinary thumbnail double-click
   use the existing geometry-only recall. Destination timing/grid/polarity/
   playback/routing/depth/smoothing stay intact; hold markers and stale point
   selection clear as before. Changed recall is one undo step in Multosis/
   Minitosis and follows normal Miff/Nap bake rules. No-op/error recalls do no
   bake/history work. The existing curve-file format remains usable.
6. Opening User or **Refresh** (R) reads a fresh snapshot, including changes from
   another plugin instance/process. Load/Rename/Delete verify the selected entry
   still matches that snapshot; an externally replaced, renamed or removed entry
   reports the change and refreshes instead of acting on an unseen replacement.
   Concurrent unrelated changes are retained. Saves cannot lose another writer's
   update; a busy library reports a retryable error.
7. A missing library is empty. Corrupt/oversized/unsupported/unreadable storage
   displays an error and disables mutations/recall until a successful refresh;
   it is never silently replaced by an empty library. Failed writes/deletes leave
   existing data intact. Names are display metadata and never filesystem paths.
8. Browsing, naming and confirmation preserve the curve, selection, viewport,
   pen repetition, Project slots and frozen waveform. An outside/right press,
   focus loss, resize, source/preset/pane replacement or library-directory change
   discards pending name/delete interactions with the existing modal cleanup.
   Name entry and confirmation consume all keys, releases, wheel and pointer
   events; hidden controls cannot edit or write. Narrow/scaled/HiDPI layouts keep
   thumbnails and controls usable or show the enlargement message.
9. The shared user library stays outside plugin/DAW state and curve undo, just
   like global Favorites. Browsing uses cached thumbnails; navigation/drawing
   perform no filesystem work. Explicit GUI actions own persistence. No DSP,
   host automation ID, project-bank, latency or audio-thread changes.

## Technical approach

- Add bounded versioned `user-presets.json` alongside existing curve files.
  Records have stable integer IDs, validated names and existing CurveShape
  geometry. Persist a monotonically increasing next ID and sort cached entries.
- Serialize mutations with a nonblocking OS file lock on a stable sibling lock
  file. Re-read and validate under the lock, compare expected records, preserve
  unrelated edits and replace the bank using the existing atomic-write helper.
  Keep lock acquisition and file operations on explicit GUI actions.
- Extend browser state with a User snapshot/error, page offset and a private
  name/delete interaction. User has Load/Fit/Save/Rename/Delete/Refresh actions;
  existing pages keep their five actions. Page controls fit the existing header.
  Bind dialogs to a full authored-document snapshot and validate it before key
  or button dispatch, so external Miff/Nap curve replacement cancels a stale
  captured Save as well as Rename/Delete. Reuse geometry recall, popup priority
  and close/cancel lifecycle.
- Implementation remains local. Independent read-only storage and UI reviews
  follow AGENTS.md; no implementation delegation or commit until requested.

## Validation

- Storage round trips, Unicode/duplicates/limits, legacy files, invalid records,
  atomic failures, lock contention, cross-instance writes and stale selections.
- Real browser Save/Rename/Delete/Load/Fit, pages and empty states, all numeric/
  keyboard/modal cleanup paths, current-curve/undo/bake preservation, cached I/O
  and scaled/HiDPI minimum layouts. Render actual populated and naming states.
- Six-crate nextest, workspace Clippy, fmt/diff and independent reviews. Leave
  the slice uncommitted for user review. Categories/search remain next.
