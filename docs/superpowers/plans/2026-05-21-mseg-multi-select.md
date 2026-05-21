# MSEG Multi-Select Editing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add node selection — click, marquee, Ctrl-extend, group move, group delete — to the shared MSEG editor widget.

**Architecture:** Selection is a `u128` bitmask in `MsegEditState` (transient editor state; `MAX_NODES` is 128). Click/marquee gestures populate it; a group drag snapshots every node's original `(time, value)` at drag-start and always measures the delta from that snapshot, so boundary clamping never corrupts the group's geometry. "Add a node" moves from single-click to double-click. The widget is shared — miff and multosis both consume the API change.

**Tech Stack:** Rust (nightly), `tiny-skia-widgets` crate (shared), consumed by `miff` and `multosis` nih-plug plugins. `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-05-21-mseg-multi-select-design.md`

**Conventions:**
- Run all `cargo`/`git` from the workspace root `/home/mpd/git-sources/tract-plugin-pack`. Branch: `multosis`.
- Build/test the three affected crates: `cargo build -p tiny-skia-widgets -p miff -p multosis`, `cargo nextest run -p tiny-skia-widgets -p miff -p multosis`, `cargo clippy -p tiny-skia-widgets -p miff -p multosis -- -D warnings`.
- Never use `#[allow(...)]` to silence a warning.
- Commit message trailer MUST be exactly: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Editor diagnostics are often stale — verify with a real build/test.

## File structure

- `tiny-skia-widgets/src/mseg/editor.rs` — `MsegEditState`: selection bitmask, marquee + group-drag state, the gesture handlers. The bulk of the work.
- `tiny-skia-widgets/src/mseg/render.rs` — `draw_nodes` highlights selected nodes; a marquee rectangle is drawn.
- `miff/src/editor.rs` — pass `ctrl` to `on_mouse_down`; pass `rect`/`scale` to `on_mouse_up`; route Delete/Backspace.
- `multosis/src/editor.rs` — same three; plus clear the selection when the active MSEG changes.

`MsegData` (`mseg/mod.rs`) is **not** modified — `move_node`/`insert_node`/`remove_node`/`MIN_NODE_GAP` are reused. `MsegData::MIN_NODE_GAP` is a private associated const; it is reachable from the child module `mseg::editor`. If for any reason it is not, change its declaration to `pub(crate) const MIN_NODE_GAP`.

---

## Task 1: Selection state, `ctrl` parameter, gesture remap

Add the selection bitmask and the click-to-select / double-click-to-add gesture remap. After this task: clicking a node selects it, Ctrl-click toggles, selected nodes draw highlighted, double-click on empty canvas inserts a node. No group move, no marquee, no delete yet.

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`
- Modify: `tiny-skia-widgets/src/mseg/render.rs`
- Modify: `miff/src/editor.rs`, `multosis/src/editor.rs`

- [ ] **Step 1: Add the selection bitmask + accessors to `MsegEditState`**

In `tiny-skia-widgets/src/mseg/editor.rs`, add a field to the `MsegEditState` struct (after `hover`):

```rust
    /// Selected node indices, bit `i` = node `i`. `MAX_NODES` is 128, so a
    /// `u128` covers every node. Transient — never persisted.
    selection: u128,
```

Initialize it in `with_curve_only` — add `selection: 0,` to the struct literal.

Add these methods to the `impl MsegEditState` block (near `hovered_node`):

```rust
    /// Is node `i` currently selected?
    pub fn is_node_selected(&self, i: usize) -> bool {
        i < 128 && self.selection & (1u128 << i) != 0
    }

    /// How many nodes are selected.
    pub fn selection_count(&self) -> u32 {
        self.selection.count_ones()
    }

    /// Clear the selection.
    pub fn clear_selection(&mut self) {
        self.selection = 0;
    }

    /// Make node `i` the sole selection.
    fn select_only(&mut self, i: usize) {
        self.selection = if i < 128 { 1u128 << i } else { 0 };
    }

    /// Toggle node `i`'s membership in the selection.
    fn toggle_selected(&mut self, i: usize) {
        if i < 128 {
            self.selection ^= 1u128 << i;
        }
    }
```

- [ ] **Step 2: Add `ctrl` to `on_mouse_down` and rewrite its node / canvas arms**

In `on_mouse_down`, add a `ctrl: bool` parameter at the end of the signature
(after `fine: bool`). Update its doc comment's first line to mention selection.

Replace the `MsegHit::Node(i)` arm with:

```rust
            MsegHit::Node(i) => {
                // Click selects: Ctrl toggles membership; a plain click on an
                // unselected node makes it the sole selection; a plain click
                // on an already-selected node keeps the selection (so a
                // multi-node group can be dragged). Selection is editor
                // state, not a document change — returns None.
                if ctrl {
                    self.toggle_selected(i);
                } else if !self.is_node_selected(i) {
                    self.select_only(i);
                }
                self.drag = Some(DragTarget::Node(i));
                None
            }
```

Replace the `MsegHit::Canvas =>` arm (the non-stepped-draw one — the
`MsegHit::Canvas if self.stepped_draw_held` arm above it is unchanged) with:

```rust
            MsegHit::Canvas => {
                // A plain click on empty canvas clears the selection; Ctrl
                // preserves it. (Adding a node is now double-click — see
                // `on_double_click`.)
                if !ctrl {
                    self.clear_selection();
                }
                None
            }
```

- [ ] **Step 3: Insert-on-double-click in `on_double_click`**

`on_double_click` currently deletes the node under the pointer. Replace its
body so it also inserts on empty canvas:

```rust
    /// Double-click: delete the node under the pointer (endpoints excepted),
    /// or insert a node when the pointer is on empty canvas. Either edit
    /// clears the selection (node indices would otherwise go stale).
    pub fn on_double_click(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        match mseg_hit_test(&layout, data, self.curve_only, scale, x, y) {
            MsegHit::Node(i) => {
                if data.remove_node(i) {
                    self.drag = None;
                    self.hover = None;
                    self.clear_selection();
                    return Some(MsegEdit::Changed);
                }
            }
            MsegHit::Canvas => {
                let (phase, value) =
                    snap_point(x_to_phase(&layout, x), y_to_value(&layout, y), data, false);
                if data.insert_node(phase, value).is_some() {
                    self.clear_selection();
                    return Some(MsegEdit::Changed);
                }
            }
            _ => {}
        }
        None
    }
```

- [ ] **Step 4: Highlight selected nodes in `draw_nodes`**

In `tiny-skia-widgets/src/mseg/render.rs`, in `draw_nodes`, replace the
node-dot loop (`for (i, n) in a.iter().enumerate() { ... }`) with:

```rust
    // Node dots. The hovered node is drawn larger; a selected node is drawn in
    // a brighter colour so the current selection reads at a glance.
    for (i, n) in a.iter().enumerate() {
        let nx = phase_to_x(layout, n.time);
        let ny = value_to_y(layout, n.value);
        let hovered = state.hovered_node() == Some(i);
        let r = (if hovered { NODE_R + HOVER_BUMP } else { NODE_R }) * scale;
        let color = if state.is_node_selected(i) {
            color_text()
        } else {
            color_accent()
        };
        draw_dot(pixmap, nx, ny, r, color);
    }
```

Ensure `color_text` is in `render.rs`'s imports — it is in the shared palette
(`primitives.rs`); add it to the existing `use` of the colour helpers if
missing.

- [ ] **Step 5: Update the `on_mouse_down` call sites in miff and multosis**

`on_mouse_down` now needs a `ctrl: bool` final argument.

In `miff/src/editor.rs`, the `on_mouse_down` call (around line 720): the
press handler already has the cursor modifiers in scope (it computes the
`fine`/Shift argument). Add `ctrl` computed the same way the `fine` argument
is — `modifiers.contains(keyboard_types::Modifiers::CONTROL)` — and pass it
as the new last argument. Read the surrounding code to match exactly how the
modifier set is named there.

In `multosis/src/editor.rs`, the `on_mouse_down` call (around line 746): same
— add the `ctrl` argument from the modifier set.

- [ ] **Step 6: Write tests**

In `editor.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn click_a_node_selects_it() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Node 1 is the inserted node at phase 0.5.
        state.on_mouse_down(
            phase_to_x(&l, 0.5), value_to_y(&l, 0.5),
            &mut data, RECT, 1.0, false, false,
        );
        assert!(state.is_node_selected(1));
        assert_eq!(state.selection_count(), 1);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn ctrl_click_toggles_nodes_into_the_selection() {
        let mut data = MsegData::default();
        data.insert_node(0.3, 0.5);
        data.insert_node(0.6, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Plain-click node 1, then Ctrl-click node 2 → both selected.
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, false);
        state.on_mouse_up(&mut data);
        state.on_mouse_down(phase_to_x(&l, 0.6), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, true);
        assert!(state.is_node_selected(1));
        assert!(state.is_node_selected(2));
        assert_eq!(state.selection_count(), 2);
        // Ctrl-click node 2 again → toggled out.
        state.on_mouse_up(&mut data);
        state.on_mouse_down(phase_to_x(&l, 0.6), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, true);
        assert!(!state.is_node_selected(2));
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn click_empty_canvas_clears_the_selection() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        state.on_mouse_down(phase_to_x(&l, 0.5), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, false);
        state.on_mouse_up(&mut data);
        assert_eq!(state.selection_count(), 1);
        // Plain click on empty canvas (phase 0.3, value 0.7 — clear of nodes).
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.7), &mut data, RECT, 1.0, false, false);
        assert_eq!(state.selection_count(), 0);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn double_click_empty_canvas_inserts_a_node() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let ev = state.on_double_click(phase_to_x(&l, 0.3), value_to_y(&l, 0.7), &mut data, RECT, 1.0);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert_eq!(data.node_count, 3);
    }

    #[test]
    fn single_click_empty_canvas_no_longer_inserts() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.7), &mut data, RECT, 1.0, false, false);
        assert_eq!(data.node_count, 2, "single click must not insert anymore");
        state.on_mouse_up(&mut data);
    }
```

Then FIX the existing tests that call `on_mouse_down` — every call now needs
the extra `ctrl` argument. The existing tests pass `false` for `fine` in most
cases; append `, false` (ctrl = false) to every `on_mouse_down(...)` call in
the test module. Also: `click_empty_canvas_inserts_a_node` and
`stepped_draw_inactive_when_modifier_not_held` assert single-click-inserts —
`click_empty_canvas_inserts_a_node` is now wrong (single click no longer
inserts); DELETE it (the new `double_click_empty_canvas_inserts_a_node` and
`single_click_empty_canvas_no_longer_inserts` replace it).
`stepped_draw_inactive_when_modifier_not_held` asserts `node_count == 3` after
a single click — that is now wrong; change its body to assert
`node_count == 2` and rename it `single_click_does_not_stepped_draw` (it now
verifies a plain click neither inserts nor stepped-draws).

- [ ] **Step 7: Build, lint, test**

Run: `cargo build -p tiny-skia-widgets -p miff -p multosis && cargo clippy -p tiny-skia-widgets -p miff -p multosis -- -D warnings && cargo nextest run -p tiny-skia-widgets -p miff -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add tiny-skia-widgets/src multosis/src miff/src
git commit -m "$(cat <<'EOF'
feat(mseg): node selection — click selects, double-click adds

MsegEditState gains a u128 selection bitmask. A plain click on a node
selects it; Ctrl-click toggles; a plain click on empty canvas clears the
selection. Adding a node moves from single-click to double-click on
empty canvas. Selected nodes draw in a brighter colour. on_mouse_down
gains a `ctrl` parameter; miff and multosis supply it.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Group move

Dragging a node that belongs to a multi-node selection moves the whole
selection. The group translates rigidly; the move delta is always measured
from a snapshot of originals taken at drag-start (speculative drag).

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`

- [ ] **Step 1: Add the group-drag state**

In `editor.rs`, add a `Group` variant to the `DragTarget` enum:

```rust
    /// Moving the whole selection; `anchor` is the node under the cursor.
    Group { anchor: usize },
```

Add a field to `MsegEditState` (after `selection`):

```rust
    /// Snapshot of every active node's `(time, value)` taken when a group
    /// drag begins — the source of truth for the drag's delta math, so
    /// boundary clamping never corrupts the group's relative geometry.
    group_snapshot: Vec<(f32, f32)>,
```

Initialize it in `with_curve_only`: `group_snapshot: Vec::new(),`.

- [ ] **Step 2: Begin a group drag from `on_mouse_down`**

In `on_mouse_down`'s `MsegHit::Node(i)` arm, replace the
`self.drag = Some(DragTarget::Node(i));` line with:

```rust
                // A drag of a node that is part of a multi-node selection
                // moves the whole group; otherwise it is a single-node drag.
                if self.selection_count() > 1 && self.is_node_selected(i) {
                    self.group_snapshot =
                        data.active().iter().map(|n| (n.time, n.value)).collect();
                    self.drag = Some(DragTarget::Group { anchor: i });
                } else {
                    self.drag = Some(DragTarget::Node(i));
                }
```

- [ ] **Step 3: Add the `apply_group_move` method**

In `editor.rs`, add this method to `impl MsegEditState` (near `step_draw_paint`):

```rust
    /// Translate every selected node rigidly by the delta implied by dragging
    /// `anchor` to the cursor. Only `anchor` snaps; the horizontal delta is
    /// clamped group-wide so no selected node crosses an unselected neighbor
    /// or a pinned endpoint; each value is clamped to 0..1 per-node. The delta
    /// is measured from `group_snapshot` — never from the live (clamped)
    /// positions — so dragging a group back un-clamps every node exactly.
    fn apply_group_move(
        &mut self,
        anchor: usize,
        x: f32,
        y: f32,
        data: &mut MsegData,
        layout: &crate::mseg::render::MsegLayout,
        fine: bool,
    ) {
        use crate::mseg::render::{x_to_phase, y_to_value};
        let n = data.node_count;
        if anchor >= n || self.group_snapshot.len() != n {
            return;
        }
        let gap = MsegData::MIN_NODE_GAP;

        // Anchor's snapped target → raw group delta, measured from the snapshot.
        let (anchor_t0, anchor_v0) = self.group_snapshot[anchor];
        let (snap_t, snap_v) =
            snap_point(x_to_phase(layout, x), y_to_value(layout, y), data, fine);
        let mut d_phase = snap_t - anchor_t0;
        let d_value = snap_v - anchor_v0;

        // Horizontal clamp: the group is rigid. Each selected node's travel is
        // bounded by the gap to its nearest UNSELECTED neighbor; a selected
        // endpoint locks horizontal motion outright.
        let mut max_right = f32::INFINITY;
        let mut max_left = f32::INFINITY;
        for i in 0..n {
            if !self.is_node_selected(i) {
                continue;
            }
            if i == 0 || i + 1 == n {
                max_right = 0.0;
                max_left = 0.0;
                break;
            }
            let t0 = self.group_snapshot[i].0;
            // First unselected node to the right of node i.
            let mut j = i + 1;
            while j < n && self.is_node_selected(j) {
                j += 1;
            }
            let right_limit = if j < n { data.nodes[j].time - gap } else { 1.0 - gap };
            max_right = max_right.min(right_limit - t0);
            // First unselected node to the left of node i.
            let mut k = i;
            while k > 0 && self.is_node_selected(k - 1) {
                k -= 1;
            }
            let left_limit = if k > 0 { data.nodes[k - 1].time + gap } else { gap };
            max_left = max_left.min(t0 - left_limit);
        }
        // Travel limits are >= 0 in a valid document; `.max(0.0)` guards the
        // degenerate case where a node already sits inside the gap.
        d_phase = d_phase.clamp(-max_left.max(0.0), max_right.max(0.0));

        // Write each selected node = snapshot + delta. Endpoints keep their
        // pinned time; every value is clamped to 0..1 per-node.
        for i in 0..n {
            if !self.is_node_selected(i) {
                continue;
            }
            let (t0, v0) = self.group_snapshot[i];
            if i != 0 && i + 1 != n {
                data.nodes[i].time = t0 + d_phase;
            }
            data.nodes[i].value = (v0 + d_value).clamp(0.0, 1.0);
        }
        data.debug_assert_valid();
    }
```

- [ ] **Step 4: Drive the group move from `on_mouse_move`**

In `on_mouse_move`, in the `match self.drag` block, add an arm before the
`_ => None` arm:

```rust
            Some(DragTarget::Group { anchor }) => {
                self.apply_group_move(anchor, x, y, data, &layout, fine);
                Some(MsegEdit::Changed)
            }
```

- [ ] **Step 5: Discard the snapshot on release**

In `on_mouse_up`, after `self.drag = None;`, add:

```rust
        self.group_snapshot.clear();
```

- [ ] **Step 6: Write tests**

In `editor.rs`'s test module, add:

```rust
    /// Select nodes by their indices via Ctrl-clicks at their positions.
    /// Helper for the group-move tests.
    fn select_nodes(state: &mut MsegEditState, data: &mut MsegData, l: &crate::mseg::render::MsegLayout, idxs: &[usize]) {
        for (n, &idx) in idxs.iter().enumerate() {
            let (t, v) = { let a = data.active(); (a[idx].time, a[idx].value) };
            // First node plain-click, the rest Ctrl-click.
            let ctrl = n > 0;
            state.on_mouse_down(phase_to_x(l, t), value_to_y(l, v), data, RECT, 1.0, false, ctrl);
            state.on_mouse_up(data);
        }
    }

    #[test]
    fn group_move_applies_a_uniform_delta() {
        let mut data = MsegData::default();
        data.snap = false;
        data.insert_node(0.3, 0.4); // node 1
        data.insert_node(0.6, 0.7); // node 2
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        select_nodes(&mut state, &mut data, &l, &[1, 2]);
        let (t1, v1) = { let a = data.active(); (a[1].time, a[1].value) };
        let (t2, v2) = { let a = data.active(); (a[2].time, a[2].value) };
        // Press node 1 (anchor), drag it +0.1 phase / -0.1 value.
        state.on_mouse_down(phase_to_x(&l, t1), value_to_y(&l, v1), &mut data, RECT, 1.0, false, false);
        state.on_mouse_move(phase_to_x(&l, t1 + 0.1), value_to_y(&l, v1 - 0.1), &mut data, RECT, 1.0, false);
        // Both selected nodes shifted by the same delta.
        assert!((data.nodes[1].time - (t1 + 0.1)).abs() < 0.02);
        assert!((data.nodes[1].value - (v1 - 0.1)).abs() < 0.02);
        assert!((data.nodes[2].time - (t2 + 0.1)).abs() < 0.02);
        assert!((data.nodes[2].value - (v2 - 0.1)).abs() < 0.02);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn group_move_horizontal_clamp_stops_at_an_unselected_node() {
        let mut data = MsegData::default();
        data.snap = false;
        data.insert_node(0.3, 0.5); // node 1 — will be selected
        data.insert_node(0.6, 0.5); // node 2 — unselected blocker
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Select nodes 1 AND the endpoint won't help — select node 1 only is a
        // single-node drag; select node 1 and node 0 (endpoint) so it is a
        // group, then verify node 1 cannot pass unselected node 2.
        select_nodes(&mut state, &mut data, &l, &[1, 0]);
        // Anchor node 1, drag far right past node 2.
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, false);
        state.on_mouse_move(phase_to_x(&l, 0.95), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false);
        // Node 1 stopped short of node 2 (0.6) by at least MIN_NODE_GAP.
        assert!(data.nodes[1].time < data.nodes[2].time,
            "selected node {} must not cross unselected node {}", data.nodes[1].time, data.nodes[2].time);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn group_move_value_clamps_per_node_at_the_top() {
        let mut data = MsegData::default();
        data.snap = false;
        data.insert_node(0.3, 0.5); // node 1
        data.insert_node(0.6, 0.9); // node 2 — already near the top
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        select_nodes(&mut state, &mut data, &l, &[1, 2]);
        // Anchor node 1, drag up by +0.3 — node 2 (0.9 + 0.3) overflows.
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, false);
        state.on_mouse_move(phase_to_x(&l, 0.3), value_to_y(&l, 0.8), &mut data, RECT, 1.0, false);
        assert!((data.nodes[2].value - 1.0).abs() < 1e-4, "overflowed node clamps to the top");
        assert!(data.nodes[1].value <= 1.0 && data.nodes[1].value > 0.7, "in-range node moved");
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn group_move_speculative_drag_unclamps_on_return() {
        let mut data = MsegData::default();
        data.snap = false;
        data.insert_node(0.3, 0.5); // node 1
        data.insert_node(0.6, 0.9); // node 2
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        select_nodes(&mut state, &mut data, &l, &[1, 2]);
        let v1_before = data.nodes[1].value;
        let v2_before = data.nodes[2].value;
        state.on_mouse_down(phase_to_x(&l, 0.3), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, false);
        // Drag up so node 2 overflows and is clamped at the top...
        state.on_mouse_move(phase_to_x(&l, 0.3), value_to_y(&l, 0.95), &mut data, RECT, 1.0, false);
        assert!((data.nodes[2].value - 1.0).abs() < 1e-4);
        // ...then drag back to the start. Both nodes return to exactly where
        // they were — clamping did not corrupt the group geometry.
        state.on_mouse_move(phase_to_x(&l, 0.3), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false);
        assert!((data.nodes[1].value - v1_before).abs() < 1e-3, "node 1 returned");
        assert!((data.nodes[2].value - v2_before).abs() < 1e-3, "node 2 un-clamped and returned");
        state.on_mouse_up(&mut data);
    }
```

- [ ] **Step 7: Build, lint, test**

Run: `cargo build -p tiny-skia-widgets -p miff -p multosis && cargo clippy -p tiny-skia-widgets -p miff -p multosis -- -D warnings && cargo nextest run -p tiny-skia-widgets -p miff -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add tiny-skia-widgets/src
git commit -m "$(cat <<'EOF'
feat(mseg): group move for multi-node selections

Dragging a node in a multi-node selection translates the whole group.
At drag-start every node's (time, value) is snapshotted; each move
measures the delta from that snapshot, so a node clamped at a value
edge un-clamps exactly when the group is dragged back. The horizontal
delta is clamped group-wide so the group stays rigid and never crosses
an unselected node or a pinned endpoint; values clamp to 0..1 per-node.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Marquee selection

Pressing on empty canvas and dragging draws a marquee rectangle; on release,
the enclosed nodes are selected.

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

- [ ] **Step 1: Add marquee state**

In `editor.rs`, add a `Marquee` variant to `DragTarget`:

```rust
    /// Dragging a selection marquee from the press anchor.
    Marquee,
```

Add fields to `MsegEditState` (after `group_snapshot`):

```rust
    /// While a marquee drag is active: `(anchor_x, anchor_y, cur_x, cur_y)` in
    /// physical pixels. `None` when no marquee is in progress.
    marquee: Option<(f32, f32, f32, f32)>,
    /// Whether Ctrl was held when the marquee began (Ctrl adds to the
    /// selection instead of replacing it).
    marquee_ctrl: bool,
```

Initialize in `with_curve_only`: `marquee: None,` and `marquee_ctrl: false,`.

Add an accessor (near `hovered_node`):

```rust
    /// The active marquee rectangle as `(x, y, w, h)` in physical pixels, if a
    /// marquee drag is in progress. For the renderer.
    pub fn marquee_rect(&self) -> Option<(f32, f32, f32, f32)> {
        self.marquee.map(|(ax, ay, cx, cy)| {
            (ax.min(cx), ay.min(cy), (cx - ax).abs(), (cy - ay).abs())
        })
    }
```

- [ ] **Step 2: Begin the marquee from `on_mouse_down`**

Replace the `MsegHit::Canvas =>` arm (from Task 1) with:

```rust
            MsegHit::Canvas => {
                // Begin a marquee. Whether it selects (on release) or just
                // clears (a no-drag click) is decided in `on_mouse_up`.
                self.drag = Some(DragTarget::Marquee);
                self.marquee = Some((x, y, x, y));
                self.marquee_ctrl = ctrl;
                None
            }
```

- [ ] **Step 3: Grow the marquee in `on_mouse_move`**

In `on_mouse_move`'s `match self.drag` block, add an arm:

```rust
            Some(DragTarget::Marquee) => {
                if let Some((ax, ay, _, _)) = self.marquee {
                    self.marquee = Some((ax, ay, x, y));
                }
                None
            }
```

- [ ] **Step 4: Finalize the marquee in `on_mouse_up`**

`on_mouse_up` needs the layout to convert node positions to pixels — add
`rect: (f32, f32, f32, f32)` and `scale: f32` parameters. Replace
`on_mouse_up` with:

```rust
    /// Primary-button release. Finalizes a marquee selection, ends any drag,
    /// and ends any scrollbar-thumb drag inside an open dropdown.
    pub fn on_mouse_up(
        &mut self,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_layout, phase_to_x, value_to_y};
        self.dropdown.on_mouse_up();
        if matches!(self.drag, Some(DragTarget::Marquee)) {
            if let Some((ax, ay, cx, cy)) = self.marquee {
                let (rx, ry) = (ax.min(cx), ay.min(cy));
                let (rw, rh) = ((cx - ax).abs(), (cy - ay).abs());
                // A marquee that never really moved is a plain click on empty
                // canvas — clear the selection (unless Ctrl preserved it).
                let moved = rw > 2.0 * scale || rh > 2.0 * scale;
                if !moved {
                    if !self.marquee_ctrl {
                        self.clear_selection();
                    }
                } else {
                    let layout = mseg_layout(rect, self.curve_only, scale);
                    if !self.marquee_ctrl {
                        self.clear_selection();
                    }
                    for (i, n) in data.active().iter().enumerate() {
                        let nx = phase_to_x(&layout, n.time);
                        let ny = value_to_y(&layout, n.value);
                        if nx >= rx && nx <= rx + rw && ny >= ry && ny <= ry + rh && i < 128 {
                            self.selection |= 1u128 << i;
                        }
                    }
                }
            }
        }
        self.drag = None;
        self.marquee = None;
        self.group_snapshot.clear();
        self.step_last_cell = None;
        None
    }
```

- [ ] **Step 5: Draw the marquee rectangle**

In `tiny-skia-widgets/src/mseg/render.rs`, in `draw_canvas`, after the call
that draws the nodes (`draw_nodes(...)`) and before the canvas border outline,
add:

```rust
    // Marquee selection rectangle (drawn over the curve and nodes).
    if let Some((mx, my, mw, mh)) = state.marquee_rect() {
        // Translucent fill plus a 1px outline, in the accent colour.
        let fill = tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0x30);
        draw_rect(pixmap, mx, my, mw, mh, fill);
        draw_rect_outline(pixmap, mx, my, mw, mh, color_accent(), 1.0);
    }
```

Confirm `draw_rect_outline` and `color_accent` are in scope in `render.rs`
(they are used elsewhere in the file). If `from_rgba8` is preferred via an
existing helper, match the file's convention.

- [ ] **Step 6: Update `on_mouse_up` call sites in miff and multosis**

`on_mouse_up` now needs `rect` and `scale` arguments.

In `miff/src/editor.rs` (call around line 863) and `multosis/src/editor.rs`
(call around line 1544): the press/move handlers nearby already compute the
MSEG rect (`mseg_rect` in miff; `lay.mseg_pane` in multosis) and the scale
(`s` in miff; `self.scale_factor` in multosis) for the other handler calls.
Pass the same rect and scale to `on_mouse_up`. Read the surrounding code to
use the exact expressions.

- [ ] **Step 7: Write tests**

In `editor.rs`'s test module, add — and FIX every existing `on_mouse_up(...)`
call in the module to pass the new `RECT, 1.0` arguments
(`state.on_mouse_up(&mut data)` → `state.on_mouse_up(&mut data, RECT, 1.0)`):

```rust
    #[test]
    fn marquee_selects_enclosed_nodes() {
        let mut data = MsegData::default();
        data.insert_node(0.25, 0.5); // node 1
        data.insert_node(0.5, 0.5);  // node 2
        data.insert_node(0.75, 0.5); // node 3
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Marquee from (phase 0.2, value 0.7) to (phase 0.55, value 0.3) —
        // encloses nodes 1 and 2, not 3.
        state.on_mouse_down(phase_to_x(&l, 0.2), value_to_y(&l, 0.7), &mut data, RECT, 1.0, false, false);
        state.on_mouse_move(phase_to_x(&l, 0.55), value_to_y(&l, 0.3), &mut data, RECT, 1.0, false);
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(state.is_node_selected(1));
        assert!(state.is_node_selected(2));
        assert!(!state.is_node_selected(3));
    }

    #[test]
    fn ctrl_marquee_adds_to_the_selection() {
        let mut data = MsegData::default();
        data.insert_node(0.25, 0.5); // node 1
        data.insert_node(0.75, 0.5); // node 2
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Select node 1 by click.
        state.on_mouse_down(phase_to_x(&l, 0.25), value_to_y(&l, 0.5), &mut data, RECT, 1.0, false, false);
        state.on_mouse_up(&mut data, RECT, 1.0);
        // Ctrl-marquee around node 2 — node 1 stays selected, node 2 added.
        state.on_mouse_down(phase_to_x(&l, 0.6), value_to_y(&l, 0.7), &mut data, RECT, 1.0, false, true);
        state.on_mouse_move(phase_to_x(&l, 0.9), value_to_y(&l, 0.3), &mut data, RECT, 1.0, false);
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(state.is_node_selected(1));
        assert!(state.is_node_selected(2));
    }
```

- [ ] **Step 8: Build, lint, test**

Run: `cargo build -p tiny-skia-widgets -p miff -p multosis && cargo clippy -p tiny-skia-widgets -p miff -p multosis -- -D warnings && cargo nextest run -p tiny-skia-widgets -p miff -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 9: Commit**

```bash
git add tiny-skia-widgets/src multosis/src miff/src
git commit -m "$(cat <<'EOF'
feat(mseg): marquee selection

Pressing on empty canvas and dragging draws a marquee rectangle; on
release every node whose dot it encloses is selected (Ctrl adds to the
existing selection). A marquee that never moved is a plain click and
clears the selection. on_mouse_up gains rect/scale parameters so it can
map node positions to pixels; miff and multosis supply them.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Multi-delete (Delete / Backspace key)

A key handler on the MSEG widget removes every selected node at once.

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`
- Modify: `miff/src/editor.rs`, `multosis/src/editor.rs`

- [ ] **Step 1: Add the `delete_selection` handler**

In `editor.rs`, add to `impl MsegEditState`:

```rust
    /// Delete every selected node. Pinned endpoints (node 0 and the last
    /// node) are skipped — they cannot be removed. Returns `Changed` if any
    /// node was removed. Clears the selection.
    pub fn delete_selection(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        // Collect selected interior indices, then remove from the highest
        // index down so earlier indices stay valid as the array shifts.
        let mut idxs: Vec<usize> = (0..data.node_count)
            .filter(|&i| self.is_node_selected(i) && i != 0 && i + 1 != data.node_count)
            .collect();
        idxs.sort_unstable();
        let mut removed = false;
        for &i in idxs.iter().rev() {
            if data.remove_node(i) {
                removed = true;
            }
        }
        self.clear_selection();
        self.drag = None;
        self.hover = None;
        if removed {
            Some(MsegEdit::Changed)
        } else {
            None
        }
    }
```

- [ ] **Step 2: Write tests**

In `editor.rs`'s test module, add:

```rust
    #[test]
    fn delete_selection_removes_selected_interior_nodes() {
        let mut data = MsegData::default();
        data.insert_node(0.25, 0.5); // node 1
        data.insert_node(0.5, 0.5);  // node 2
        data.insert_node(0.75, 0.5); // node 3
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        select_nodes(&mut state, &mut data, &l, &[1, 3]);
        let ev = state.delete_selection(&mut data);
        assert_eq!(ev, Some(MsegEdit::Changed));
        // Started with 5 nodes (2 endpoints + 3 inserted); removed 2.
        assert_eq!(data.node_count, 3);
        assert_eq!(state.selection_count(), 0);
    }

    #[test]
    fn delete_selection_skips_endpoints() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5); // node 1
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Select node 0 (endpoint) and node 1 (interior).
        select_nodes(&mut state, &mut data, &l, &[1, 0]);
        let ev = state.delete_selection(&mut data);
        assert_eq!(ev, Some(MsegEdit::Changed));
        // Node 1 removed; the two endpoints survive.
        assert_eq!(data.node_count, 2);
    }

    #[test]
    fn delete_selection_with_nothing_selected_is_a_noop() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let ev = state.delete_selection(&mut data);
        assert_eq!(ev, None);
        assert_eq!(data.node_count, 3);
    }
```

- [ ] **Step 3: Route Delete/Backspace in multosis**

In `multosis/src/editor.rs`, the keyboard handler arm
`baseview::Event::Keyboard(ev) if self.text_edit.is_active()` (around line
1557) handles keys ONLY while a dial text-edit is active. Add a SEPARATE
`baseview::Event::Keyboard(ev)` arm AFTER it (so the text-edit-active arm
still wins when a text edit is open) that handles MSEG deletion when the
effect editor is showing the MSEG pane. It must:
- act only when `self.view == View::Effect` (the MSEG pane is visible) and no
  text edit is active;
- on `keyboard_types::Key::Delete` or `keyboard_types::Key::Backspace`, call
  `self.mseg_edit.delete_selection(...)` on the active MSEG —
  `&mut modu[self.selected_track].msegs[self.selected_mseg.min(2)]` inside a
  `track_modulation` lock — and if it returns `MsegEdit::Changed`, call
  `self.mark_config_dirty()`.

Read how the existing keyboard arm and the MSEG handler calls (around lines
738/746/1311) obtain the `track_modulation` lock and the row / MSEG indices,
and mirror that exactly. Only fire on key-down events, not key-up — match how
the existing handler distinguishes them.

- [ ] **Step 4: Route Delete/Backspace in miff**

In `miff/src/editor.rs`, the keyboard handler arm
`baseview::Event::Keyboard(ev) if self.text_edit.is_active()` (around line
937) handles keys only while a text edit is active. Add a separate
`baseview::Event::Keyboard(ev)` arm after it that, when no text edit is
active, on `Key::Delete` / `Key::Backspace` calls
`self.mseg_state.delete_selection(&mut curve)` on miff's curve document and,
on `MsegEdit::Changed`, triggers miff's re-persist/re-bake path. Read how
miff's other MSEG handlers (around lines 708/720/863/906) obtain the `curve`
document and what they do with a returned `MsegEdit::Changed`, and mirror it
exactly. Fire only on key-down.

- [ ] **Step 5: Build, lint, test**

Run: `cargo build -p tiny-skia-widgets -p miff -p multosis && cargo clippy -p tiny-skia-widgets -p miff -p multosis -- -D warnings && cargo nextest run -p tiny-skia-widgets -p miff -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add tiny-skia-widgets/src multosis/src miff/src
git commit -m "$(cat <<'EOF'
feat(mseg): Delete/Backspace removes the whole selection

MsegEditState::delete_selection removes every selected interior node
(pinned endpoints skipped) and clears the selection. miff and multosis
route Delete/Backspace key-down events to it when the MSEG pane is the
editing focus and no dial text-edit is active.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Clear selection on MSEG switch + final verification

multosis shows one of three MSEGs at a time; switching must clear the
selection so stale node indices can never apply to a different MSEG.

**Files:**
- Modify: `multosis/src/editor.rs`

- [ ] **Step 1: Clear the selection when the active MSEG changes**

In `multosis/src/editor.rs`, find where `self.selected_mseg` is assigned (the
MSEG selector handler — the control that switches between the amplitude and
the two assignable MSEGs). After the assignment that changes which MSEG is
shown, call `self.mseg_edit.clear_selection();`. Guard it so it only clears
when the value actually changed (don't clear if the user re-clicks the
already-active segment), to avoid wiping a selection on a no-op click. Read
the selector handler to place this precisely.

- [ ] **Step 2: Build, lint, test**

Run: `cargo build -p tiny-skia-widgets -p miff -p multosis && cargo clippy -p tiny-skia-widgets -p miff -p multosis -- -D warnings && cargo nextest run -p tiny-skia-widgets -p miff -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 3: Commit**

```bash
git add multosis/src
git commit -m "$(cat <<'EOF'
fix(multosis): clear the MSEG selection when switching MSEGs

The MSEG selection is a set of node indices; switching which of a
track's three MSEGs is shown must clear it so indices from one MSEG
never apply to another.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Full workspace verification**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check && cargo nextest run -p tiny-skia-widgets -p miff -p multosis`
Expected: all PASS, no warnings.

- [ ] **Step 5: Bundle**

Run: `cargo xtask native nih-plug bundle miff --release` and
`cargo xtask native nih-plug bundle multosis --release`.
Expected: bundles created under `target/bundled/`.

- [ ] **Step 6: Manual smoke check**

In both miff and multosis: double-click empty canvas adds a node; single-click
a node selects it (highlighted); Ctrl-click extends; marquee-drag selects a
group; drag a selected node to move the whole group (rigid, no reorder; a node
overflowing the top shows at the edge and un-clamps when dragged back);
Delete/Backspace removes the selection; double-click a node still deletes just
that one.

---

## Self-Review

**Spec coverage:**
- §1 selection state (`u128` bitmask), cleared on MSEG switch / structural edit → Task 1 (bitmask, structural edits clear), Task 5 (MSEG switch). ✓
- §2 gesture remap (double-click adds, click selects, Ctrl toggles, click-empty clears, drag-node group-moves, drag-empty marquees) → Task 1 (click/double-click), Task 2 (drag-node group), Task 3 (drag-empty marquee). ✓
- §3 marquee, Ctrl adds → Task 3. ✓
- §4 group move: snapshot, snap only the anchor, horizontal rigid clamp, vertical per-node clamp → Task 2 `apply_group_move`. ✓
- §5 speculative drag (delta from snapshot, un-clamps on return) → Task 2 (`group_snapshot` is the delta source; test `group_move_speculative_drag_unclamps_on_return`). ✓
- §6 multi-delete via Delete/Backspace, endpoints skipped → Task 4. ✓
- §7 drawing — selected highlight, marquee rect → Task 1 Step 4, Task 3 Step 5. ✓
- §8 plugin integration — `ctrl` param, `on_mouse_up` rect/scale, Delete routing → Task 1 Step 5, Task 3 Step 6, Task 4 Steps 3–4. ✓
- §9 edge cases — endpoints (selectable, lock horizontal, skipped by delete), empty selection no-op, selection-of-one = single drag → Task 2 (`apply_group_move` endpoint branch + the `selection_count() > 1` gate), Task 4 (`delete_selection` endpoint filter + no-op test). ✓

**Placeholder scan:** No TBD/TODO. The plugin-integration steps (Task 1 Step 5, Task 3 Step 6, Task 4 Steps 3–4, Task 5 Step 1) describe the change precisely and say to mirror named, identified existing code rather than leaving a gap — appropriate, since the exact surrounding expressions in the large editor files must be read in place.

**Type consistency:** `MsegEditState` gains `selection: u128`, `group_snapshot: Vec<(f32,f32)>`, `marquee: Option<(f32,f32,f32,f32)>`, `marquee_ctrl: bool`. `DragTarget` gains `Group { anchor: usize }` and `Marquee`. `on_mouse_down` signature: `(x, y, data, rect, scale, fine, ctrl)`. `on_mouse_up` signature: `(data, rect, scale)`. `apply_group_move(anchor, x, y, data, layout, fine)`, `delete_selection(data) -> Option<MsegEdit>`, `marquee_rect() -> Option<(f32,f32,f32,f32)>`, `is_node_selected`/`selection_count`/`clear_selection` — all used consistently across Tasks 1–5 and the tests.
