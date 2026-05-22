# Multosis Undo/Redo — Design

**Date:** 2026-05-22
**Status:** Approved

## Summary

Add plugin-side undo/redo to the multosis editor for the state the host's
own undo cannot reach — the grid, the per-track effects, and the per-track
modulation/MSEGs. A bounded history of full-config snapshots is captured one
entry per editing gesture; **Ctrl+Z** / **Ctrl+Shift+Z** restore it. The
history lives in the editor window and is discarded when the window closes.

The change is confined to `multosis` — a new `undo.rs` module plus capture
and restore wiring in `editor.rs`. No audio-thread code, no new parameters,
no UI layout change.

## Motivation

multosis state splits in two. The five host-automatable parameters
(`speed`, `mix`, `output_gain`, `comp_threshold`, `comp_ratio`) are covered
by the DAW's own undo. But the bulk of an editing session — toggling grid
cells, drawing MSEG nodes, switching effect kinds, retargeting modulation,
randomising — mutates the **persisted, DAW-opaque** state: `grid`,
`track_effects`, `track_modulation`. The host saves these as blobs but cannot
see into them, so it cannot undo any of those edits. A mis-click on the grid
or a fumbled node drag is currently unrecoverable. Plugin-side undo fills
exactly that gap.

## Current model (for reference)

`MultosisParams` (`lib.rs`) holds:

- Host parameters (`#[id = ...]`): `speed`, `mix`, `output_gain`,
  `comp_threshold`, `comp_ratio` — the DAW undoes these.
- Persisted, DAW-opaque state (`#[persist = ...]`, each an `Arc<Mutex<…>>`):
  `grid: Grid`, `track_effects: [TrackEffect; 16]`,
  `track_modulation: [TrackModulation; 16]`, and `editor_state` (window size).

The editor (`MultosisWindow`) mutates the three config structs by `lock()`ing
the mutex, writing, and calling `mark_config_dirty()` — a single
`AtomicBool` the audio engine polls each block to re-bridge the config. Every
persisted-state edit in the editor already funnels through that pattern.

## Design

### 1. The undo system

A new module `multosis/src/undo.rs`:

- **`ConfigSnapshot`** — a plain owned struct holding clones of the three
  DAW-opaque config structs:
  ```rust
  pub struct ConfigSnapshot {
      pub grid: Grid,
      pub effects: [TrackEffect; 16],
      pub modulation: [TrackModulation; 16],
  }
  ```
  `editor_state` (window size) and the host parameters are deliberately not
  part of a snapshot.

- **`UndoHistory`** — owns an undo `Vec<ConfigSnapshot>`, a redo
  `Vec<ConfigSnapshot>`, and a `pending: Option<ConfigSnapshot>` capture slot.
  A depth constant (`UNDO_DEPTH`, 128) bounds the undo stack: pushing a new
  entry past the cap drops the oldest (front) entry.

`UndoHistory` is a field of `MultosisWindow` (the editor). It is created
fresh when the editor window opens and dropped when it closes — so the
history resets on each window open and consumes nothing while the window is
closed. It is **not** persisted with the project.

`Grid`, `TrackEffect`, and `TrackModulation` must be `Clone` (they are
already, being persisted/serialised) and `PartialEq` (derive it where
missing) so a snapshot can be cloned and compared.

### 2. Capturing edits — one entry per gesture

The editor routes every input event through one handler. Undo capture
brackets each input gesture with two `UndoHistory` calls:

- **`begin_capture(current: ConfigSnapshot)`** — if no capture is already
  open, store `current` in `pending`. (Idempotent: a second `begin_capture`
  while one is open is a no-op, so the earliest pre-state is kept.)
- **`commit_capture(current: ConfigSnapshot)`** — if `pending` is set and
  `current` differs from it (`PartialEq` on the snapshot), push `pending`
  onto the undo stack and clear the redo stack; otherwise discard `pending`.
  Either way, `pending` ends empty.

Wiring in the editor's event handler:

- **Mouse:** on any mouse-button **press**, `begin_capture` with a snapshot
  of the current config; on the **release** that ends the gesture,
  `commit_capture`. A click is one gesture; a click-drag (cell paint, MSEG
  node drag, region resize) is one gesture; a right-click that mutates config
  (e.g. toggling an MSEG segment's stepped flag) is likewise one gesture —
  each becomes at most one undo entry.
- **Keyboard and other instantaneous events:** the undo/redo keys are handled
  first (see §4) and never captured. Any other key-down — and any other
  instantaneous input event the editor dispatches that can mutate config,
  such as a scroll-wheel adjustment — is bracketed `begin_capture` before /
  `commit_capture` after its handler, so e.g. a Delete/Backspace MSEG
  multi-delete is one entry.

Because the comparison is over the three config structs only, a gesture that
changes nothing (opening a dropdown, moving the cursor) or that changes only
a host parameter (a knob drag) produces **no** undo entry — those are not the
plugin's to undo. Snapshotting on every gesture costs one ~110 KB clone that
is discarded when nothing changed; this is negligible against the editor's
full CPU re-render per interaction.

### 3. Undo / redo

- **`undo`** — if the undo stack is non-empty: build a `ConfigSnapshot` of
  the current config and push it onto the redo stack; pop the undo stack;
  write the popped snapshot's three structs back into the `params` mutexes;
  call `mark_config_dirty()`; clear the MSEG editor's node selection (node
  indices may not match the restored MSEGs); request a repaint.
- **`redo`** — symmetric: push current onto the undo stack, pop redo, restore.
- A new committed gesture (§2) clears the redo stack — the standard linear
  history rule.
- `selected_track` and `selected_mseg` are editor view state; undo/redo do
  **not** change them (undo must not move the user's view). The dropdowns and
  dials re-read the restored config on the next frame and reflect it.

The audio thread needs no new code: `mark_config_dirty()` makes the engine
re-bridge the restored config on its next process block, exactly as for a
normal edit.

### 4. Trigger

Keyboard only — no toolbar or other UI surface.

- **Ctrl+Z** → undo.
- **Ctrl+Shift+Z** and **Ctrl+Y** → redo.

The editor's keyboard handler tests for these key combinations **before** the
capture bracket of §2, so an undo/redo keystroke is never itself recorded as
an editing gesture. A key combination that hits an empty stack is a no-op.

## Testing

`undo.rs` unit tests:

- push then `undo` returns the prior snapshot; `redo` returns the later one.
- a new push (committed edit) clears the redo stack.
- the depth bound: pushing `UNDO_DEPTH + 1` entries drops the oldest, and the
  history still undoes correctly down to the retained floor.
- `commit_capture` with an unchanged snapshot pushes nothing.
- `begin_capture` while a capture is already open keeps the first pre-state.

Editor-level tests:

- toggling a grid cell then `undo` restores the grid; `redo` re-applies it.
- an effect-kind switch then `undo` restores the kind, its parameters, and
  the track's modulation targets together (one entry).
- a single drag gesture produces exactly one undo entry.
- `undo` clears the MSEG node selection.

`cargo build -p multosis`, `cargo clippy -p multosis -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run -p multosis` all clean. Existing
tests are unaffected — undo is additive editor state.

## Out of scope

- The five host parameters — the DAW's own undo owns those.
- Persisting undo history with the project, or keeping it across a
  window close — the history is per-window-session.
- Any toolbar button or other on-screen undo affordance — keyboard only.
- Per-component (delta) snapshots — full-config snapshots are used; with the
  history window-scoped and multosis run as a handful of instances, their
  memory cost is a non-issue.
- No change to the audio engine, the grid model, effects, or modulation.
