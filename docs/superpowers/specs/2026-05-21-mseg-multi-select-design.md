# MSEG Multi-Select Editing — Design

**Date:** 2026-05-21
**Status:** Approved

## Summary

Add node **selection** to the shared MSEG editor widget
(`tiny-skia-widgets/src/mseg/`): click and marquee selection, multi-select
with Ctrl, group move, and group delete. The "add a node" gesture moves from
single-click to double-click to free single-click for selection. The widget
is shared by **miff** and **multosis** — both consume the change.

## Motivation

The MSEG editor today edits one node at a time: single-click empty canvas
inserts a node, drag moves a node, double-click removes a node. For an
envelope with many nodes there is no way to move a cluster together or delete
several at once. Selection — with marquee, Ctrl-extend, group move, and group
delete — makes editing dense envelopes practical.

## Current interaction model (for reference)

`MsegEditState` (in `mseg/editor.rs`) holds transient editor state. Handlers:

- `on_mouse_down` — hit-test: a node → begin a node drag; a tension handle →
  begin a tension drag; empty canvas → **insert a node** and begin dragging
  it; strip → toggle snap / polarity / open a dropdown / randomize.
- `on_mouse_move` — apply the active drag (`DragTarget::Node` calls
  `data.move_node(i, phase, value)` with grid snap).
- `on_mouse_up` — end the drag.
- `on_double_click` — delete the node under the pointer (endpoints excepted).
- `on_right_click` — toggle the `stepped` flag of the segment under the
  pointer.

`MsegData` invariants: `nodes` are strictly ascending in `time`; node 0 is
pinned at `time == 0.0`, the last node at `time == 1.0`; `value` and `time`
are each in `0..1`.

## Design

### 1. Selection state

`MsegEditState` gains transient, non-persisted state:

- A **selection set** — the indices of the currently selected nodes (a
  `Vec<usize>` or a fixed-capacity bitset over `MAX_NODES`; the plan picks).
- **Marquee state** — the press-anchor while a marquee drag is active.
- **Group-drag state** — at drag-start, a **snapshot** of every selected
  node's original `(phase, value)`; this snapshot is the source of truth for
  the whole drag (see §5).

The selection is cleared whenever stored indices could go stale:

- Switching which MSEG is shown (multosis shows one of three MSEGs at a time).
- Any structural edit that changes `node_count` — a node insert, a
  single-node (double-click) delete, or the randomizer. The group-delete
  itself naturally ends with an empty selection.

### 2. Click semantics

The gesture remap (the consuming plugins already distinguish single- vs
double-click and call the matching handler):

| Gesture | New behavior |
|---|---|
| Double-click empty canvas | Insert a node (was single-click). |
| Double-click a node | Delete that node — **unchanged**. |
| Single-click a node | Select it — replaces the selection. |
| Ctrl-click a node | Toggle that node in/out of the selection. |
| Single-click empty canvas (press + release, no drag) | Clear the selection. |
| Press a node, then drag | Group move (§4). If the pressed node was not selected, it first becomes the sole selection — so a lone node still drags as before. |
| Press empty canvas, then drag | Marquee (§3). |

Tension-handle drags, the stepped-draw modifier path, the strip, and
`on_right_click` (segment `stepped` toggle) are all unchanged.

### 3. Marquee selection

Pressing on empty canvas and dragging starts a marquee. While dragging, a
translucent rectangle is drawn from the press anchor to the cursor. On
release, every node whose dot centre falls inside the marquee rectangle is
selected:

- Ctrl held → the enclosed nodes are **added** to the existing selection.
- Ctrl not held → the enclosed nodes **replace** the selection.

A marquee that encloses no nodes and had Ctrl up clears the selection (it is
the drag form of a single-click on empty canvas).

### 4. Group move

Pressing a node and dragging moves the whole current selection. A selection
of one is just an ordinary single-node move.

At drag-start the editor snapshots each selected node's original
`(phase, value)`. Each `on_mouse_move` computes a raw cursor delta
`(d_phase, d_value)` from the dragged node's snapshot position, then:

**Snap.** Only the node directly under the cursor snaps to the grid (when
`data.snap` is on and the fine modifier is up). The group's delta is taken
from *that node's snapped position* minus its snapshot position. Every other
selected node moves by the same delta, unsnapped.

**Horizontal (time).** The group translates rigidly in time. `d_phase` is
clamped, group-wide, so that no selected node crosses an unselected neighbor
and no selected node passes a pinned endpoint:

- For each selected node, the maximum rightward / leftward travel is the gap
  to its nearest unselected neighbor on that side (and to `time 0.0` / `1.0`
  for the endpoints). The group's `d_phase` is clamped to the tightest of
  these limits across the whole selection.
- If the selection includes a pinned endpoint (node 0 or the last node), that
  endpoint cannot move in time, so the group's `d_phase` is clamped to 0.

This keeps node order intact — the group never reorders and never collides.

**Vertical (value).** `d_value` is applied uniformly, then each node's
resulting value is clamped to `0..1` per-node for display: a node overflowing
the top shows at `1.0`, the bottom at `0.0`. The group may visually deform at
the value edges; the logical positions do not (see §5).

### 5. Speculative drag

The snapshot of originals taken at drag-start is the **source of truth** for
the entire drag. Every `on_mouse_move` recomputes each node's position as
`snapshot + delta` — never from the node's current (possibly clamped)
position in the document.

Consequence: a node whose value overflows `0..1` is *shown* clamped at the
edge, but its true logical position (`snapshot + delta`) is unchanged. Drag
the group back and the overflowed node rejoins the group at its exact
relative offset — clamping never corrupts the group's geometry.

The document (`MsegData`) is updated each move to the clamped, invariant-valid
preview so the curve renders through the normal render path; but because the
delta math always reads the snapshot, the document is effectively a render
buffer. The clamped positions become **permanent only on release** — at
`on_mouse_up` the snapshot is discarded and the document's current state is
the committed result.

The horizontal rigid-clamp (§4) guarantees the document stays
invariant-valid (strictly-ascending time, endpoints pinned) at every frame of
the drag, so the renderer and `debug_assert_valid` never see a bad document.

### 6. Multi-delete

A new key handler on the MSEG widget: **Delete** or **Backspace** removes
every selected node at once. Pinned endpoints in the selection are skipped
(they cannot be deleted). After the delete the selection is empty.

`MsegEditState` gains an `on_key` (or `on_delete`) handler returning
`Option<MsegEdit>`. The miff and multosis editors route Delete / Backspace
key events to it when the MSEG pane is the active editing focus.

### 7. Drawing

- Selected node dots draw in a distinct highlight — brighter / accented
  versus the normal dot colour (`render.rs` `draw_nodes`-equivalent path).
- The marquee draws as a translucent rectangle while a marquee drag is active.

### 8. Plugin integration

- `on_mouse_down` gains a `ctrl: bool` parameter alongside the existing
  `fine: bool`. The miff and multosis editors supply it from the cursor
  modifier set.
- Both editors forward Delete / Backspace key events to the new MSEG key
  handler when the MSEG pane is the editing focus.
- Both plugins must continue to build, lint clean, and pass their tests — the
  MSEG widget API change (`ctrl` parameter, new key handler) touches both
  call sites.

### 9. Edge cases

- **Endpoints** (node 0, last node) are selectable and move in value with a
  group; their time is pinned, and selecting one clamps the group's time
  delta to 0. They are skipped by group-delete.
- **Empty selection** — group move and group delete with nothing selected are
  no-ops.
- **Selection of one** — behaves exactly like today's single-node drag.

## Testing

- Single-click a node selects it; Ctrl-click toggles membership; single-click
  empty canvas clears the selection.
- Double-click empty canvas inserts a node; double-click a node deletes it
  (regression — unchanged).
- Marquee selects exactly the enclosed nodes; Ctrl-marquee adds to the
  selection.
- Group move applies a uniform delta to every selected node.
- Only the cursor-anchored node snaps; the rest move by its snapped delta.
- Horizontal clamp: a group cannot be dragged so a selected node crosses an
  unselected neighbor; a group containing an endpoint does not move in time.
- Vertical clamp: a node dragged past the top shows at `1.0`, past the bottom
  at `0.0`.
- **Speculative drag:** drag a group up so a node overflows the top, then drag
  back down; assert the overflowed node returns to its exact original offset
  relative to the group.
- Delete / Backspace removes the whole selection and skips endpoints.
- A structural edit (insert / single-delete) clears the selection.

`cargo build --workspace`, `cargo clippy --workspace -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run` for `tiny-skia-widgets`, `miff`,
and `multosis` all clean.

## Out of scope

- No change to tension handles, the stepped-draw path, the strip, hold
  markers, or `on_right_click`.
- Selection is not persisted — it is transient editor state.
- No copy/paste of selected nodes (a possible future follow-up).
- No marquee-select of tension handles — selection is nodes only.
