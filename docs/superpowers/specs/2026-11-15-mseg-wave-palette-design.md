# Project MSEG wave palette

Continue `feat/mseg-power-tools` after browser commit `122d136`. The user
authorized the recommended first palette slice: saved slots and manual recall.
Extend the existing CPU browser; no mock supplied. MIDI and automation switching
follow after this storage/recall foundation.

## Behavior

1. Each Multosis, Minitosis, Miff and Nap instance has eight project wave slots,
   initially empty. They save with plugin presets/DAW projects and remain local
   to that instance. All curves/rows/effects in the instance share that bank.
   Existing global Favorites keep their shared user-library behavior.
2. A **Project** tab follows Factory and Favorites in **Browse shapes...** when
   the embedding host supplies a palette. The existing eight-card layout,
   thumbnail interpolation, minimum preview space and native bounds apply.
   Tab/Shift+Tab cycle the available pages; the opening pane retains input.
   Narrow tabs use bounded Fct/Fav/Prj captions without spilling between buttons.
3. Clicking a card selects it without modifying the current curve. **Store**
   (S) copies the current curve's normalized geometry into the selected slot;
   **Clear** (Delete) empties it. Occupied slots can be overwritten explicitly.
   Store/Clear save project data without changing playback, baking, the current
   curve, selection, viewport, pen repetition or local curve undo history.
4. **Load** (Enter), **Fit** (Shift+Enter), and ordinary thumbnail double-click
   use existing whole-shape recall. Geometry includes tension, native shapes,
   steps and weights; destination timing, grid, polarity, routing, depth and
   smoothing stay intact, and hold markers clear. A changed recall is one local
   undo step in Multosis/Minitosis and follows normal Miff/Nap bake rules.
   Undoing a curve recall leaves the saved project slots intact.
5. Empty slots disable recall. **Refresh** (R) rereads the project bank without
   filesystem access. If the bank changes externally while its page is open,
   an attempted action first refreshes the snapshot and asks the user to choose
   again; it cannot silently load or overwrite an unseen replacement. Entering
   Project always takes a fresh snapshot. Unavailable bindings report an error
   and leave curves/project data untouched.
6. Browsing, dismissal, focus/resize cleanup, numeric-field priority and modal
   routing retain the existing browser contract. Rebinding to another palette
   closes the old browser without disturbing the curve or remembered pen state.
   Drawing and pointer navigation perform no persistence or filesystem work.
7. Project slots use bounded, validated, versioned shape records. Invalid slot
   counts/versions or invalid geometry reject restoration. Older states without
   a bank load an empty bank, including when replacing a preset in an already
   populated instance. Shape clipboard/files transfer
   only geometry and do not carry or overwrite the project palette.
8. Palette storage is GUI/persistence state outside MsegData, TrackModulation
   and audio configuration. Store/Clear require no engine handoff, latency change,
   host automation parameters or audio-thread allocation/locking/destruction.

## Technical approach and validation

- Add a shared WavePalette model with eight optional CurveShapes and validated
  serde. Reuse versioned shape validation without changing curve-file formats.
- Add a separately persisted Arc<Mutex<WavePalette>> to each plugin's params;
  bind it to its shared/static editor at window creation. Keep local curve undo
  snapshots and audio-engine structures unchanged.
- Cache project thumbnails in the browser. Guard project actions against an
  externally replaced snapshot while holding the palette lock; write only the
  explicit Store/Clear action. Rendering uses the cache.
- Test round trips, malformed records, transactional slot edits, instance
  isolation, host serialization/defaults/automation IDs, cross-curve sharing,
  stale bindings, no-op/error behavior, unchanged audio/bakes/curve undo, and
  exact recall metadata. Cover three-page keyboard/mouse routing and rendered
  previews at 0.5x/1x/2x, including minimum window boundaries.
- Update manuals/help, render real project slots offscreen, run six-crate
  focused tests, workspace Clippy, format/whitespace and independent read-only
  reviews. Leave changes uncommitted until requested.
