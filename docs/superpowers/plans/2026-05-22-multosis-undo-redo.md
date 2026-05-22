# Multosis Undo/Redo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add keyboard-driven undo/redo to the multosis editor for the DAW-opaque persisted config — the grid, per-track effects, and per-track modulation/MSEGs.

**Architecture:** A new `undo.rs` module holds a generic bounded undo/redo stack (`UndoHistory<S>`) and a `ConfigSnapshot` of the three persisted structs with `capture`/`restore`. The editor owns an `UndoHistory<ConfigSnapshot>`, brackets each input gesture with `begin_capture`/`commit_capture` (one undo entry per gesture that actually changes config), and routes Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y to undo/redo. History is discarded when the editor window closes.

**Tech Stack:** Rust (nightly), `multosis` crate (a nih-plug plugin) in the `tract-plugin-pack` Cargo workspace. `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-05-22-multosis-undo-redo-design.md`

**Conventions:**
- Run all `cargo`/`git` from the workspace root `/home/mpd/git-sources/tract-plugin-pack`. Branch: `multosis`.
- Build/test/lint just this crate: `cargo build -p multosis`, `cargo nextest run -p multosis`, `cargo clippy -p multosis -- -D warnings`, `cargo fmt --check`.
- Never use `#[allow(...)]` to silence a warning.
- Commit message trailer MUST be exactly: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Editor diagnostics are often stale — verify with a real build/test.

## Background — the current code

`MultosisParams` (`lib.rs`) holds the three DAW-opaque persisted structs, each an `Arc<Mutex<…>>`: `grid: Grid`, `track_effects: [TrackEffect; 16]`, `track_modulation: [TrackModulation; 16]`. `Grid` and `TrackEffect` derive `Clone, Copy, PartialEq`; `TrackModulation` derives `Clone, PartialEq`. `crate::grid::ROWS` is `16`.

The editor (`MultosisWindow` in `editor.rs`) mutates those structs by `lock()`ing the mutex, writing, and calling `mark_config_dirty()` (sets an `AtomicBool` the audio engine polls). `MultosisWindow::on_event` is the single baseview event handler — a `match` on `baseview::Event` with arms for `Window(Resized)`, `Mouse(CursorMoved)`, `Mouse(ButtonPressed{Left})`, `Mouse(ButtonPressed{Right}) if view == Effect`, `Mouse(ButtonReleased{Left})`, `Keyboard(ev) if text_edit.is_active()`, and `Keyboard(ev)`. The editor redraws every frame (the playhead animates), so a config change shows on the next frame with no explicit repaint call.

## File structure

- `multosis/src/undo.rs` — **new.** `UndoHistory<S>` (the generic stack) and `ConfigSnapshot` (+ `capture`/`restore`). Pure logic, fully unit-tested.
- `multosis/src/lib.rs` — add `pub mod undo;`.
- `multosis/src/editor.rs` — `MultosisWindow` gains an `undo` field, `snapshot`/`do_undo`/`do_redo` methods, gesture-capture brackets in four event arms, and Ctrl+Z/Ctrl+Shift+Z/Ctrl+Y routing.

---

## Task 1: The `undo` module

Create `multosis/src/undo.rs` with the generic undo stack and the config snapshot, both fully unit-tested. Purely additive — nothing else uses it yet, the workspace stays green.

**Files:**
- Create: `multosis/src/undo.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Register the module**

In `multosis/src/lib.rs`, the `pub mod` declarations run alphabetically (`clock` … `seq_status`). Add, immediately after `pub mod seq_status;`:

```rust
pub mod undo;
```

- [ ] **Step 2: Write `undo.rs` with the implementation and its tests**

Create `multosis/src/undo.rs` with exactly this content:

```rust
//! Editor-side undo/redo for the DAW-opaque persisted config.
//!
//! The host's own undo covers the automatable parameters; it cannot see into
//! the `grid` / `track_effects` / `track_modulation` persisted blobs. This
//! module captures snapshots of those three structs and restores them.
//! See `docs/superpowers/specs/2026-05-22-multosis-undo-redo-design.md`.

use std::collections::VecDeque;
use std::sync::PoisonError;

use crate::effects::TrackEffect;
use crate::grid::{Grid, ROWS};
use crate::modulation::TrackModulation;
use crate::MultosisParams;

/// Upper bound on the undo stack. Pushing past this drops the oldest entry.
pub const UNDO_DEPTH: usize = 128;

/// A bounded linear undo/redo history of snapshots, plus a one-slot capture
/// window. Generic over the snapshot type so the stack logic is unit-testable
/// without constructing a full [`ConfigSnapshot`].
pub struct UndoHistory<S> {
    undo: VecDeque<S>,
    redo: Vec<S>,
    pending: Option<S>,
}

impl<S: Clone + PartialEq> UndoHistory<S> {
    /// An empty history.
    pub fn new() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            pending: None,
        }
    }

    /// Open a capture window holding the pre-edit snapshot `before`. A no-op
    /// if a capture is already open, so the earliest pre-state is the one
    /// kept.
    pub fn begin_capture(&mut self, before: S) {
        if self.pending.is_none() {
            self.pending = Some(before);
        }
    }

    /// Close the capture window. If a capture is open and `after` differs from
    /// the captured pre-state, push the pre-state as an undo entry and clear
    /// the redo stack. Otherwise the pending capture is discarded.
    pub fn commit_capture(&mut self, after: &S) {
        if let Some(before) = self.pending.take() {
            if before != *after {
                self.push_undo(before);
                self.redo.clear();
            }
        }
    }

    /// Undo: record `current` on the redo stack and return the snapshot to
    /// restore, or `None` if the undo stack is empty.
    pub fn undo(&mut self, current: S) -> Option<S> {
        let snapshot = self.undo.pop_back()?;
        self.redo.push(current);
        Some(snapshot)
    }

    /// Redo: record `current` on the undo stack and return the snapshot to
    /// restore, or `None` if the redo stack is empty.
    pub fn redo(&mut self, current: S) -> Option<S> {
        let snapshot = self.redo.pop()?;
        self.push_undo(current);
        Some(snapshot)
    }

    /// Whether an undo is available.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether a redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Push onto the undo stack, dropping the oldest entry past `UNDO_DEPTH`.
    fn push_undo(&mut self, snapshot: S) {
        if self.undo.len() >= UNDO_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
    }
}

impl<S: Clone + PartialEq> Default for UndoHistory<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// A snapshot of the three DAW-opaque persisted config structs.
#[derive(Clone, PartialEq, Debug)]
pub struct ConfigSnapshot {
    pub grid: Grid,
    pub effects: [TrackEffect; ROWS],
    pub modulation: [TrackModulation; ROWS],
}

impl ConfigSnapshot {
    /// Clone the current `grid` / `track_effects` / `track_modulation` out of
    /// `params`. A poisoned mutex is recovered with `into_inner`, so a
    /// snapshot is always faithful (the editor is the only GUI writer and
    /// holds each lock only briefly).
    pub fn capture(params: &MultosisParams) -> Self {
        let grid = *params.grid.lock().unwrap_or_else(PoisonError::into_inner);
        let effects = *params
            .track_effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let modulation = params
            .track_modulation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Self {
            grid,
            effects,
            modulation,
        }
    }

    /// Write this snapshot's three structs back into `params`.
    pub fn restore(&self, params: &MultosisParams) {
        *params.grid.lock().unwrap_or_else(PoisonError::into_inner) = self.grid;
        *params
            .track_effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.effects;
        *params
            .track_modulation
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = self.modulation.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_with_a_change_pushes_an_undo_entry() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(1);
        h.commit_capture(&2);
        assert!(h.can_undo());
        assert_eq!(h.undo(2), Some(1));
    }

    #[test]
    fn commit_with_no_change_pushes_nothing() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(5);
        h.commit_capture(&5);
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(1);
        h.commit_capture(&2);
        // Current state is 2: undo returns the pre-state 1, stashes 2 to redo.
        assert_eq!(h.undo(2), Some(1));
        assert!(h.can_redo());
        // Current state is now 1: redo returns 2.
        assert_eq!(h.redo(1), Some(2));
        assert!(!h.can_redo());
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(1);
        h.commit_capture(&2);
        assert_eq!(h.undo(2), Some(1));
        assert!(h.can_redo());
        // A fresh committed edit while undone clears the redo stack.
        h.begin_capture(1);
        h.commit_capture(&9);
        assert!(!h.can_redo());
    }

    #[test]
    fn begin_capture_keeps_the_first_pre_state() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        h.begin_capture(10);
        h.begin_capture(20); // ignored — a capture is already open
        h.commit_capture(&30);
        assert_eq!(h.undo(30), Some(10));
    }

    #[test]
    fn undo_and_redo_are_none_on_empty_stacks() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        assert_eq!(h.undo(0), None);
        assert_eq!(h.redo(0), None);
    }

    #[test]
    fn the_depth_bound_drops_the_oldest_entry() {
        let mut h: UndoHistory<i32> = UndoHistory::new();
        // Push UNDO_DEPTH + 1 distinct entries (pre-states 0..=UNDO_DEPTH).
        for i in 0..=UNDO_DEPTH as i32 {
            h.begin_capture(i);
            h.commit_capture(&(i + 1000)); // always a change
        }
        // The stack holds exactly UNDO_DEPTH entries; the oldest (pre-state 0)
        // was dropped, so the deepest retained pre-state is 1.
        let mut count = 0;
        let mut last = None;
        while let Some(s) = h.undo(0) {
            last = Some(s);
            count += 1;
        }
        assert_eq!(count, UNDO_DEPTH);
        assert_eq!(last, Some(1));
    }

    #[test]
    fn config_snapshot_round_trips_a_grid_edit() {
        let params = MultosisParams::default();
        let original = ConfigSnapshot::capture(&params);
        // Flip a grid cell.
        params.grid.lock().unwrap().cell_mut(3, 5).enabled ^= true;
        let mutated = ConfigSnapshot::capture(&params);
        assert_ne!(original, mutated, "the grid edit changed the snapshot");
        // Restoring the original snapshot reverts the edit.
        original.restore(&params);
        assert_eq!(ConfigSnapshot::capture(&params), original);
    }

    #[test]
    fn config_snapshot_round_trips_effect_and_modulation_edits() {
        let params = MultosisParams::default();
        let original = ConfigSnapshot::capture(&params);
        params.track_effects.lock().unwrap()[2].mix = 0.25;
        params.track_modulation.lock().unwrap()[7].depths[0] = -0.5;
        let mutated = ConfigSnapshot::capture(&params);
        assert_ne!(original, mutated);
        original.restore(&params);
        assert_eq!(ConfigSnapshot::capture(&params), original);
    }
}
```

- [ ] **Step 3: Build, lint, format, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: clean, no warnings; the nine new `undo` tests pass alongside the existing suite. (If `cargo fmt --check` reports drift, run `cargo fmt` and re-run.)

- [ ] **Step 4: Commit**

```bash
git add multosis/src/undo.rs multosis/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(multosis): undo history + config snapshot

A new undo module: UndoHistory<S> is a bounded linear undo/redo stack
with a one-slot capture window (begin_capture / commit_capture push an
entry only when the snapshot actually changed). ConfigSnapshot clones
the three DAW-opaque persisted structs — grid, effects, modulation —
and restores them. Pure logic, fully unit-tested; the editor wires it
in next.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Wire undo/redo into the editor

Give `MultosisWindow` an `UndoHistory<ConfigSnapshot>`, bracket each input gesture so one undo entry is captured per config-changing gesture, and route the undo/redo keys. The editor cannot be unit-tested (it owns a window/render surface), so this task is verified by build + lint + the existing regression suite + a manual smoke check; the undo logic itself is fully covered by Task 1.

**Files:**
- Modify: `multosis/src/editor.rs`

- [ ] **Step 1: Import the undo types**

In `multosis/src/editor.rs`, near the existing `use crate::…` lines at the top (e.g. just after `use crate::modulation::TriggerSource;`), add:

```rust
use crate::undo::{ConfigSnapshot, UndoHistory};
```

- [ ] **Step 2: Add the `undo` field to `MultosisWindow`**

In the `struct MultosisWindow { … }` definition, add a field immediately after the `mseg_edit` field:

```rust
    /// Undo/redo history for the DAW-opaque config. Window-scoped — created
    /// fresh on window open, dropped on close.
    undo: UndoHistory<ConfigSnapshot>,
```

- [ ] **Step 3: Initialize `undo` in `MultosisWindow::new`**

In `MultosisWindow::new`, in the struct literal that builds `Self`, add (next to the other simple field initializers such as `selected_track: 0,`):

```rust
            undo: UndoHistory::new(),
```

- [ ] **Step 4: Add the `snapshot` / `do_undo` / `do_redo` methods**

In `impl MultosisWindow`, immediately after the `mark_config_dirty` method, add:

```rust
    /// Snapshot the current DAW-opaque config (grid, effects, modulation).
    fn snapshot(&self) -> ConfigSnapshot {
        ConfigSnapshot::capture(&self.params)
    }

    /// Undo the last captured edit, if any: restore the config, drop the now
    /// stale MSEG node selection, and mark the config dirty for the audio
    /// thread to re-bridge.
    fn do_undo(&mut self) {
        let current = self.snapshot();
        if let Some(snap) = self.undo.undo(current) {
            snap.restore(&self.params);
            self.mseg_edit.clear_selection();
            self.mark_config_dirty();
        }
    }

    /// Redo the last undone edit, if any.
    fn do_redo(&mut self) {
        let current = self.snapshot();
        if let Some(snap) = self.undo.redo(current) {
            snap.restore(&self.params);
            self.mseg_edit.clear_selection();
            self.mark_config_dirty();
        }
    }
```

- [ ] **Step 5: Bracket the left-button gesture**

A left-button gesture spans the `ButtonPressed{Left}` arm through the `ButtonReleased{Left}` arm; `begin_capture` goes at the start of the press arm and `commit_capture` at the end of the release arm.

In the `baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed { button: baseview::MouseButton::Left, modifiers })` arm, insert these two lines as the **first** statements of the arm body, before the existing `let (px, py) = self.mouse_pos;`:

```rust
                let snap = self.snapshot();
                self.undo.begin_capture(snap);
```

In the `baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased { button: baseview::MouseButton::Left, .. })` arm, the body ends with `self.left_gesture = None;`. Append, as the last statements of that arm:

```rust
                let after = self.snapshot();
                self.undo.commit_capture(&after);
```

(The press arm has early `return EventStatus::Captured` paths — that is fine: `commit_capture` lives in the separate release arm, which always runs.)

- [ ] **Step 6: Bracket the right-button gesture**

The right-button `ButtonPressed` arm has no matching release arm and has three early `return baseview::EventStatus::…;` statements, so `begin_capture` and `commit_capture` must both sit in this arm with a single exit. Extract the arm body into a method.

Add a new method to `impl MultosisWindow` (e.g. just after `do_redo`):

```rust
    /// Handle a right-button press in the effect view. Returns the event
    /// status. Extracted from `on_event` so the undo-capture bracket has a
    /// single exit point.
    fn on_right_press(&mut self) -> baseview::EventStatus {
        // <-- moved verbatim: the entire current body of the
        //     `ButtonPressed { button: Right, .. }` arm goes here.
        //     Its three `return baseview::EventStatus::X;` statements stay
        //     as returns.
        baseview::EventStatus::Captured
    }
```

Move the **entire current body** of the right-button `ButtonPressed` arm into `on_right_press` verbatim — it begins with `let (px, py) = self.mouse_pos;`. Keep its three `return baseview::EventStatus::…;` statements unchanged. Append `baseview::EventStatus::Captured` as the method's final expression (the arm previously had no explicit tail and fell through to `on_event`'s trailing `baseview::EventStatus::Captured`).

Then replace the right-button arm body with:

```rust
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) if self.view == View::Effect => {
                let snap = self.snapshot();
                self.undo.begin_capture(snap);
                let status = self.on_right_press();
                let after = self.snapshot();
                self.undo.commit_capture(&after);
                return status;
            }
```

- [ ] **Step 7: Bracket the Enter commit in the text-edit keyboard arm**

In the `baseview::Event::Keyboard(ev) if self.text_edit.is_active()` arm, replace the `keyboard_types::Key::Enter => match self.text_edit.commit() { … },` arm of its inner `match &ev.key` with:

```rust
                    keyboard_types::Key::Enter => {
                        let snap = self.snapshot();
                        self.undo.begin_capture(snap);
                        match self.text_edit.commit() {
                            Some((EffectHit::Dial(i), text)) => {
                                self.commit_dial_text_edit(i, &text)
                            }
                            Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
                            _ => {}
                        }
                        let after = self.snapshot();
                        self.undo.commit_capture(&after);
                    }
```

- [ ] **Step 8: Add undo/redo keys and bracket Delete/Backspace in the keyboard arm**

Replace the entire `baseview::Event::Keyboard(ev) => { … }` arm (the one **without** the `if self.text_edit.is_active()` guard) with:

```rust
            baseview::Event::Keyboard(ev) => {
                if ev.state != keyboard_types::KeyState::Down {
                    return baseview::EventStatus::Ignored;
                }
                // Undo / redo — handled before any capture so the keystroke
                // itself is never recorded as an editing gesture. Active in
                // both views.
                if ev.modifiers.contains(keyboard_types::Modifiers::CONTROL) {
                    let is_z = matches!(
                        &ev.key,
                        keyboard_types::Key::Character(s) if s.eq_ignore_ascii_case("z")
                    );
                    let is_y = matches!(
                        &ev.key,
                        keyboard_types::Key::Character(s) if s.eq_ignore_ascii_case("y")
                    );
                    let shift = ev.modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    if (is_z && shift) || is_y {
                        self.do_redo();
                        return baseview::EventStatus::Captured;
                    }
                    if is_z {
                        self.do_undo();
                        return baseview::EventStatus::Captured;
                    }
                }
                if self.view == View::Effect {
                    match &ev.key {
                        keyboard_types::Key::Delete | keyboard_types::Key::Backspace => {
                            let snap = self.snapshot();
                            self.undo.begin_capture(snap);
                            let sel = self.selected_mseg.min(2);
                            let changed = if let Ok(mut modu) = self.params.track_modulation.lock()
                            {
                                let row = self.selected_track;
                                self.mseg_edit.delete_selection(&mut modu[row].msegs[sel])
                            } else {
                                None
                            };
                            if changed == Some(widgets::mseg::MsegEdit::Changed) {
                                self.mark_config_dirty();
                            }
                            let after = self.snapshot();
                            self.undo.commit_capture(&after);
                            if changed == Some(widgets::mseg::MsegEdit::Changed) {
                                return baseview::EventStatus::Captured;
                            }
                        }
                        _ => {}
                    }
                }
                return baseview::EventStatus::Ignored;
            }
```

This preserves the arm's prior behaviour (Delete/Backspace MSEG multi-delete, `Captured` on a successful delete else `Ignored`) and adds: the undo/redo key handling first, and the capture bracket around the Delete/Backspace handler so a multi-delete is one undo entry. The `return` after a successful delete now sits *after* `commit_capture` so the capture always closes.

- [ ] **Step 9: Build, lint, format, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: clean, no warnings; the full existing suite still passes (this task changes no tested logic — undo capture is additive editor state). (If `cargo fmt --check` reports drift, run `cargo fmt` and re-run.)

If the compiler reports that `ev.modifiers` is not a field, inspect the `keyboard_types` keyboard-event type in scope and use its modifier-set accessor; the rest of the arm is unaffected.

- [ ] **Step 10: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "$(cat <<'EOF'
feat(multosis): editor undo/redo (Ctrl+Z / Ctrl+Shift+Z)

MultosisWindow owns an UndoHistory<ConfigSnapshot>. Each input gesture
is bracketed begin_capture / commit_capture — a left-button gesture
(press through release), a right-button press, the Enter text-edit
commit, and a Delete/Backspace MSEG multi-delete each become at most
one undo entry, recorded only when the config actually changed. Ctrl+Z
undoes, Ctrl+Shift+Z and Ctrl+Y redo; restoring marks the config dirty
and clears the stale MSEG node selection. The right-press arm body is
extracted to `on_right_press` so its capture bracket has a single exit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 11: Manual smoke check**

Bundle and load multosis in a host (`cargo xtask native nih-plug bundle multosis --release`). Verify: toggle a grid cell → Ctrl+Z reverts it → Ctrl+Shift+Z reapplies it; paint a drag of cells → Ctrl+Z reverts the whole drag in one step; drag an MSEG node → one Ctrl+Z reverts the drag; switch an effect kind → Ctrl+Z restores the old kind, params, and modulation targets; randomize → Ctrl+Z reverts it; a new edit after an undo discards the redo; turning a knob (mix/output/comp) is *not* affected by plugin Ctrl+Z (it stays the DAW's).

---

## Self-Review

**Spec coverage:**
- §1 the undo system — `UndoHistory<S>` bounded stack + `ConfigSnapshot` of the three structs, owned by `MultosisWindow`, `UNDO_DEPTH` cap, not persisted, dropped on window close → Task 1 (module), Task 2 Steps 2–3 (field, init). ✓
- §2 capture per gesture — `begin_capture`/`commit_capture`, commit-if-changed; left-gesture press→release bracket, right-press bracket, keyboard handler brackets; param-only changes produce no entry (the snapshot covers only the three config structs) → Task 1 (`begin_capture`/`commit_capture`), Task 2 Steps 5–8. ✓
- §3 undo/redo — restore the three structs, `mark_config_dirty`, clear the MSEG selection, redo cleared on a new edit, view state untouched → Task 1 (`undo`/`redo`, redo-clear in `commit_capture`), Task 2 Step 4 (`do_undo`/`do_redo`). ✓
- §4 trigger — Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y, handled before capture, keyboard only → Task 2 Step 8. ✓
- Testing — `UndoHistory` stack tests + `ConfigSnapshot` round-trip tests → Task 1 Step 2; editor wiring verified by build/lint/regression + manual smoke → Task 2 Steps 9, 11. ✓

**Placeholder scan:** No TBD/TODO. Task 1's `undo.rs` is given in full. Task 2's only "move existing code" instruction (Step 6) names the exact arm and is a verbatim move, not a guess; every other step shows complete code.

**Type consistency:** `UndoHistory<S: Clone + PartialEq>` with `new`, `begin_capture(S)`, `commit_capture(&S)`, `undo(S) -> Option<S>`, `redo(S) -> Option<S>`, `can_undo`/`can_redo`. `ConfigSnapshot { grid: Grid, effects: [TrackEffect; ROWS], modulation: [TrackModulation; ROWS] }` with `capture(&MultosisParams) -> Self` and `restore(&self, &MultosisParams)`. The editor field is `undo: UndoHistory<ConfigSnapshot>`; `snapshot(&self) -> ConfigSnapshot`, `do_undo`/`do_redo`/`on_right_press(&mut self)`. All consistent across Tasks 1–2 and the tests. `Grid`/`TrackEffect`/`TrackModulation` already derive the needed `Clone`/`Copy`/`PartialEq` — no derive changes required.
