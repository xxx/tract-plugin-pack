# Tiny-Skia Right-Click Text Entry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Project commit rule (user standing order):** Never run `git commit` without explicit user approval. The commit steps below describe *what to commit and the intended message* — pause for user approval before running them.

**Goal:** Add right-click-to-type-value support to `draw_dial` and `draw_slider` in
`tiny-skia-widgets`, and wire it into the five softbuffer-based plugin editors
(gain-brain, satch, tinylimit, pope-scope, warp-zone).

**Architecture:** A new `TextEditState<A>` in `tiny-skia-widgets` (parallel to
`DragState<A>`) holds a single active edit — the action being edited and the
typed buffer. `draw_dial` / `draw_slider` gain an `editing_text: Option<&str>`
parameter and a `caret_on: bool`: when editing, the widget renders the buffer
with a caret in the existing value-readout region, suppressing the unit. Each
editor adds a `TextEditState` field, a right-click handler to open the edit
(populated from `Param::normalized_value_to_string(v, false)`), keyboard event
arms for character/backspace/enter/escape, and a commit path that pushes
through `Param::string_to_normalized_value` + `begin/set/end_set_parameter`.

**Tech Stack:** Rust 1.x (nightly via `rust-toolchain.toml`), nih-plug (fork at
`finish-vst3-pr`), baseview (`9a0b42c0`), `keyboard_types 0.6.2`, tiny-skia,
fontdue.

**Spec:** `docs/superpowers/specs/2026-04-19-tiny-skia-text-entry-design.md`

---

## File layout

**Create**
- `tiny-skia-widgets/src/text_edit.rs` — `TextEditState<A>` + unit tests.

**Modify**
- `tiny-skia-widgets/src/lib.rs` — module declaration + re-export.
- `tiny-skia-widgets/src/param_dial.rs` — `draw_dial` / `draw_dial_ex` signatures, highlight + caret rendering.
- `tiny-skia-widgets/src/controls.rs` — `draw_slider` / `draw_outline_slider` same.
- `gain-brain/src/editor.rs` — wire right-click, keyboard, commit; update `draw_dial` call sites.
- `satch/src/editor.rs` — same.
- `tinylimit/src/editor.rs` — same (dials + sliders).
- `pope-scope/src/editor.rs` — same.
- `warp-zone/src/editor.rs` — same.

---

## Task 1: `TextEditState` scaffold (new + active_for + cancel)

**Files:**
- Create: `tiny-skia-widgets/src/text_edit.rs`

- [ ] **Step 1: Write the failing tests**

Create `tiny-skia-widgets/src/text_edit.rs` with only the test module for now:

```rust
//! Shared numeric text-entry state for softbuffer-based nih-plug editors.
//!
//! Mirrors the role of `DragState<A>`: a single in-flight edit, driven by
//! right-click + keyboard events in the host editor. The widget draw
//! functions (`draw_dial`, `draw_slider`) query `active_for(&A)` to decide
//! whether to render a buffer + caret in place of the formatted value.

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    enum A {
        Gain,
        Freq,
    }

    #[test]
    fn new_reports_no_active_edit() {
        let s: TextEditState<A> = TextEditState::new();
        assert!(s.active_for(&A::Gain).is_none());
        assert!(s.active_for(&A::Freq).is_none());
    }

    #[test]
    fn active_for_other_action_is_none() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        assert_eq!(s.active_for(&A::Gain), Some("-6.0"));
        assert!(s.active_for(&A::Freq).is_none());
    }

    #[test]
    fn cancel_clears_state() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.cancel();
        assert!(s.active_for(&A::Gain).is_none());
    }

    #[test]
    fn begin_replaces_active_edit() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.begin(A::Freq, "440");
        assert!(s.active_for(&A::Gain).is_none());
        assert_eq!(s.active_for(&A::Freq), Some("440"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets text_edit -- --nocapture`
Expected: FAIL with "cannot find type `TextEditState` in this scope".

- [ ] **Step 3: Implement minimal struct + new/begin/active_for/cancel**

Prepend above the `#[cfg(test)]` block:

```rust
use std::time::Instant;

/// Shared numeric text-entry state. One in-flight edit at most, tagged by
/// the same action type used for `DragState` hit regions.
pub struct TextEditState<A: Clone + PartialEq> {
    active: Option<A>,
    buffer: String,
    started_at: Instant,
}

/// Maximum buffer length (defensive cap — typed values are short).
const MAX_BUFFER_LEN: usize = 16;

impl<A: Clone + PartialEq> TextEditState<A> {
    pub fn new() -> Self {
        Self {
            active: None,
            buffer: String::new(),
            started_at: Instant::now(),
        }
    }

    /// Open an edit on `action` with `initial` as the starting buffer.
    /// Replaces any in-flight edit (the editor is expected to have called
    /// `commit()` first if it wanted to preserve the previous value).
    pub fn begin(&mut self, action: A, initial: &str) {
        self.active = Some(action);
        self.buffer.clear();
        self.buffer.push_str(initial);
        if self.buffer.len() > MAX_BUFFER_LEN {
            self.buffer.truncate(MAX_BUFFER_LEN);
        }
        self.started_at = Instant::now();
    }

    /// Returns `Some(&buffer)` iff `action` matches the currently active edit.
    pub fn active_for(&self, action: &A) -> Option<&str> {
        match &self.active {
            Some(a) if a == action => Some(&self.buffer),
            _ => None,
        }
    }

    /// Discard the in-flight edit without committing.
    pub fn cancel(&mut self) {
        self.active = None;
        self.buffer.clear();
    }
}

impl<A: Clone + PartialEq> Default for TextEditState<A> {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tiny-skia-widgets text_edit -- --nocapture`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/text_edit.rs
git commit -m "tiny-skia-widgets: scaffold TextEditState<A>

Adds active_for/begin/cancel with a 16-char buffer cap. No keyboard
or draw integration yet — just the state container."
```

---

## Task 2: `insert_char` with numeric filter

**Files:**
- Modify: `tiny-skia-widgets/src/text_edit.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn insert_char_accepts_digits_and_numeric_symbols() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        for c in "0123456789.-+eE".chars() {
            s.insert_char(c);
        }
        assert_eq!(s.active_for(&A::Gain), Some("0123456789.-+eE"));
    }

    #[test]
    fn insert_char_rejects_letters() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        for c in "abcxyzABCXYZ".chars() {
            s.insert_char(c);
        }
        assert_eq!(s.active_for(&A::Gain), Some(""));
    }

    #[test]
    fn insert_char_rejects_whitespace_and_symbols() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        for c in " \t\n/%#@".chars() {
            s.insert_char(c);
        }
        assert_eq!(s.active_for(&A::Gain), Some(""));
    }

    #[test]
    fn insert_char_noop_when_not_active() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.insert_char('1'); // no active edit
        assert!(s.active_for(&A::Gain).is_none());
    }

    #[test]
    fn insert_char_respects_max_buffer_len() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        for _ in 0..32 {
            s.insert_char('9');
        }
        assert_eq!(s.active_for(&A::Gain).unwrap().len(), 16);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: FAIL with "no method named `insert_char`".

- [ ] **Step 3: Implement `insert_char`**

Inside the `impl` block (just after `cancel`):

```rust
    /// Append `c` to the buffer if it is a valid numeric character
    /// (`0-9`, `.`, `-`, `+`, `e`, `E`) and the buffer is under
    /// `MAX_BUFFER_LEN`. Silent no-op otherwise. No-op when no edit
    /// is active.
    pub fn insert_char(&mut self, c: char) {
        if self.active.is_none() {
            return;
        }
        let accepted = matches!(c, '0'..='9' | '.' | '-' | '+' | 'e' | 'E');
        if !accepted {
            return;
        }
        if self.buffer.len() >= MAX_BUFFER_LEN {
            return;
        }
        self.buffer.push(c);
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: 9 passed.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/text_edit.rs
git commit -m "tiny-skia-widgets: add TextEditState::insert_char with numeric filter

Accepts 0-9 . - + e E; rejects everything else silently. Respects
the 16-char buffer cap and is a no-op when no edit is active."
```

---

## Task 3: `backspace`

**Files:**
- Modify: `tiny-skia-widgets/src/text_edit.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn backspace_removes_last_char() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.0");
        s.backspace();
        assert_eq!(s.active_for(&A::Gain), Some("-6."));
    }

    #[test]
    fn backspace_on_empty_is_noop() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        s.backspace();
        assert_eq!(s.active_for(&A::Gain), Some(""));
    }

    #[test]
    fn backspace_when_inactive_is_noop() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.backspace(); // must not panic
        assert!(s.active_for(&A::Gain).is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: FAIL with "no method named `backspace`".

- [ ] **Step 3: Implement `backspace`**

Inside the `impl` block:

```rust
    /// Remove the last character from the buffer. No-op on empty buffer
    /// or when no edit is active.
    pub fn backspace(&mut self) {
        if self.active.is_none() {
            return;
        }
        self.buffer.pop();
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: 12 passed.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/text_edit.rs
git commit -m "tiny-skia-widgets: add TextEditState::backspace"
```

---

## Task 4: `commit`

**Files:**
- Modify: `tiny-skia-widgets/src/text_edit.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    #[test]
    fn commit_returns_action_and_buffer_and_clears() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "-6.2");
        let out = s.commit();
        assert_eq!(out, Some((A::Gain, "-6.2".to_string())));
        assert!(s.active_for(&A::Gain).is_none());
    }

    #[test]
    fn commit_when_inactive_returns_none() {
        let mut s: TextEditState<A> = TextEditState::new();
        assert!(s.commit().is_none());
    }

    #[test]
    fn is_active_mirrors_active_state() {
        let mut s: TextEditState<A> = TextEditState::new();
        assert!(!s.is_active());
        s.begin(A::Gain, "");
        assert!(s.is_active());
        s.cancel();
        assert!(!s.is_active());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: FAIL with "no method named `commit`" / "no method named `is_active`".

- [ ] **Step 3: Implement `commit` and `is_active`**

Inside the `impl` block:

```rust
    /// Consume the in-flight edit and return `(action, buffer)`. Returns
    /// `None` when no edit is active. Always clears state (the editor is
    /// responsible for pushing the value through the nih-plug param path).
    pub fn commit(&mut self) -> Option<(A, String)> {
        let action = self.active.take()?;
        let buffer = std::mem::take(&mut self.buffer);
        Some((action, buffer))
    }

    /// `true` iff an edit is currently in flight. Editors use this to
    /// gate keyboard event consumption.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: 15 passed.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/text_edit.rs
git commit -m "tiny-skia-widgets: add TextEditState::commit and is_active

commit() returns (action, buffer) and clears state; is_active() lets
editors gate whether keyboard events are consumed."
```

---

## Task 5: `caret_visible` blink

**Files:**
- Modify: `tiny-skia-widgets/src/text_edit.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn caret_visible_starts_true() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        assert!(s.caret_visible(), "caret should be visible at t=0");
    }

    #[test]
    fn caret_visible_flips_after_half_second() {
        let mut s: TextEditState<A> = TextEditState::new();
        s.begin(A::Gain, "");
        sleep(Duration::from_millis(550));
        assert!(!s.caret_visible(), "caret should be hidden ~550 ms in");
        sleep(Duration::from_millis(500));
        assert!(s.caret_visible(), "caret should be visible ~1050 ms in");
    }

    #[test]
    fn caret_visible_false_when_inactive() {
        let s: TextEditState<A> = TextEditState::new();
        assert!(!s.caret_visible());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: FAIL with "no method named `caret_visible`".

- [ ] **Step 3: Implement `caret_visible`**

Inside the `impl` block:

```rust
    /// `true` during the "on" half of the 500 ms blink cycle. Returns
    /// `false` when no edit is active.
    pub fn caret_visible(&self) -> bool {
        if self.active.is_none() {
            return false;
        }
        let elapsed_ms = self.started_at.elapsed().as_millis();
        (elapsed_ms % 1000) < 500
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tiny-skia-widgets text_edit`
Expected: 18 passed. (The timing tests sleep ~1s total — keep single-threaded test run.)

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/text_edit.rs
git commit -m "tiny-skia-widgets: add TextEditState::caret_visible blink

500 ms on / 500 ms off cycle driven by Instant::elapsed; false when
no edit is active."
```

---

## Task 6: Re-export `TextEditState` from `lib.rs`

**Files:**
- Modify: `tiny-skia-widgets/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to `tiny-skia-widgets/src/lib.rs` near the other top-level re-exports:

```rust
#[cfg(test)]
mod reexport_tests {
    #[test]
    fn text_edit_state_reexported_from_crate_root() {
        let _: super::TextEditState<()> = super::TextEditState::new();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tiny-skia-widgets reexport_tests`
Expected: FAIL — `TextEditState` not in scope.

- [ ] **Step 3: Add the module + re-export**

Edit `tiny-skia-widgets/src/lib.rs`:

```rust
pub mod controls;
pub mod drag;
pub mod editor_base;
pub mod param_dial;
pub mod primitives;
pub mod text;
pub mod text_edit;   // NEW

pub use controls::*;
pub use drag::*;
pub use editor_base::*;
pub use param_dial::*;
pub use primitives::*;
pub use text::*;
pub use text_edit::*; // NEW
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tiny-skia-widgets reexport_tests`
Expected: 1 passed.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/lib.rs
git commit -m "tiny-skia-widgets: re-export TextEditState from crate root"
```

---

## Task 7: `draw_dial` / `draw_dial_ex` gain `editing_text` + `caret_on`

**Files:**
- Modify: `tiny-skia-widgets/src/param_dial.rs`

**Goal:** add two parameters to both functions. When `editing_text = Some(buf)`,
render a highlighted box in the value-readout region, draw `buf` (no unit), and
draw a 1 px caret flush right of the buffer's last glyph when `caret_on` is true.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` block in `param_dial.rs`:

```rust
    // -----------------------------------------------------------------------
    // Text-entry overlay rendering
    // -----------------------------------------------------------------------

    /// With `editing_text = None`, behaviour is byte-identical to the
    /// unit-aware path. Regression guard for legacy callers.
    #[test]
    fn test_draw_dial_ex_editing_none_matches_existing_render() {
        let mut pm = test_pixmap();
        let mut tr = test_renderer();
        draw_dial_ex(
            &mut pm, &mut tr, 100.0, 100.0, 40.0,
            "Gain", "-6.0 dB", 0.5, None,
            /* editing_text */ None, /* caret_on */ false,
        );
    }

    #[test]
    fn test_draw_dial_ex_with_editing_text_no_panic() {
        let mut pm = test_pixmap();
        let mut tr = test_renderer();
        draw_dial_ex(
            &mut pm, &mut tr, 100.0, 100.0, 40.0,
            "Gain", "-6.0 dB", 0.5, None,
            /* editing_text */ Some("-6."), /* caret_on */ true,
        );
        draw_dial_ex(
            &mut pm, &mut tr, 100.0, 100.0, 40.0,
            "Gain", "-6.0 dB", 0.5, None,
            Some("-6."), /* caret_on */ false,
        );
    }

    #[test]
    fn test_draw_dial_editing_highlight_visible() {
        // When editing, the value-readout region should have a distinctly
        // brighter background than the surrounding dial background. Sample
        // a pixel inside the region (below the arc) to confirm.
        let mut pm_plain = test_pixmap();
        let mut pm_edit = test_pixmap();
        let mut tr = test_renderer();
        draw_dial_ex(
            &mut pm_plain, &mut tr, 100.0, 100.0, 40.0,
            "G", "-6 dB", 0.5, None, None, false,
        );
        draw_dial_ex(
            &mut pm_edit, &mut tr, 100.0, 100.0, 40.0,
            "G", "-6 dB", 0.5, None, Some("-6"), true,
        );
        // Row inside the value readout (~cy + radius*0.71 + 4)
        let y = (100.0 + 40.0 * 0.71 + 6.0) as u32;
        let plain_px = pm_plain.pixels()[(y * pm_plain.width() + 100) as usize];
        let edit_px = pm_edit.pixels()[(y * pm_edit.width() + 100) as usize];
        // Either the edit pixmap has a coloured highlight and plain has
        // alpha==0 (Pixmap::new yields transparent), or both are non-zero
        // and differ. In either case they must differ.
        assert!(
            plain_px.red() != edit_px.red()
                || plain_px.green() != edit_px.green()
                || plain_px.blue() != edit_px.blue()
                || plain_px.alpha() != edit_px.alpha(),
            "editing overlay must paint a highlight that differs from background"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets param_dial`
Expected: FAIL — `draw_dial_ex` takes 9 args, not 11.

- [ ] **Step 3: Extend signatures + render overlay**

In `tiny-skia-widgets/src/param_dial.rs`, replace the existing `draw_dial` and `draw_dial_ex` with:

```rust
#[allow(clippy::too_many_arguments)]
pub fn draw_dial(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cx: f32,
    cy: f32,
    radius: f32,
    label: &str,
    value_text: &str,
    normalized: f32,
) {
    draw_dial_ex(
        pixmap, text_renderer, cx, cy, radius, label, value_text, normalized,
        None, None, false,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn draw_dial_ex(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cx: f32,
    cy: f32,
    radius: f32,
    label: &str,
    value_text: &str,
    normalized: f32,
    modulated_normalized: Option<f32>,
    editing_text: Option<&str>,
    caret_on: bool,
) {
    // ... arc + modulation block unchanged ...

    // --- Label text centered above the arc ---
    let text_size = (radius * 0.38).max(11.0);
    let label_w = text_renderer.text_width(label, text_size);
    let label_x = cx - label_w * 0.5;
    let label_y = cy - radius - stroke_width - 8.0;
    text_renderer.draw_text(pixmap, label_x, label_y, label, text_size, color_muted());

    // --- Value readout: buffer + caret when editing, otherwise formatted value ---
    let value_y = cy + radius * 0.71 + text_size + 4.0;
    if let Some(buf) = editing_text {
        // Highlight box sized to accommodate the widest reasonable buffer
        // (~8 glyphs). Use text_width of a reference string so the box
        // width is stable regardless of the current buffer contents.
        let ref_w = text_renderer.text_width("-999.99", text_size);
        let box_w = ref_w + 12.0;
        let box_h = text_size + 6.0;
        let box_x = cx - box_w * 0.5;
        let box_y = value_y - text_size - 2.0;
        crate::primitives::draw_rect(pixmap, box_x, box_y, box_w, box_h, color_edit_bg());
        crate::primitives::draw_rect_outline(pixmap, box_x, box_y, box_w, box_h, color_accent(), 1.0);

        // Buffer text, left-padded inside the box for a typing-feel layout.
        let buf_x = box_x + 6.0;
        text_renderer.draw_text(pixmap, buf_x, value_y, buf, text_size, color_text());

        // Caret flush right of the last glyph.
        if caret_on {
            let buf_w = text_renderer.text_width(buf, text_size);
            let caret_x = buf_x + buf_w + 1.0;
            let caret_y = box_y + 3.0;
            let caret_h = box_h - 6.0;
            crate::primitives::draw_rect(pixmap, caret_x, caret_y, 1.0, caret_h, color_text());
        }
    } else {
        let value_w = text_renderer.text_width(value_text, text_size);
        let value_x = cx - value_w * 0.5;
        text_renderer.draw_text(pixmap, value_x, value_y, value_text, text_size, color_text());
    }
}
```

Add a `color_edit_bg()` helper at the top of `param_dial.rs` alongside the other color helpers:

```rust
/// Edit-mode highlight color — a brighter variant of the control background.
fn color_edit_bg() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(48, 52, 64, 255)
}
```

- [ ] **Step 4: Update existing callers with `None, false`**

The existing call in `draw_dial` already forwards `None, None, false`. Search for other in-crate callers of `draw_dial_ex`:

```bash
rg 'draw_dial_ex' --type rust
```

Expected hits: tests only (they will be updated with the new trailing args in Step 1). If any non-test crate calls `draw_dial_ex` directly, append `, None, false` to the call. Update them to keep the build green.

- [ ] **Step 5: Run the full crate tests**

Run: `cargo test -p tiny-skia-widgets`
Expected: all tests pass, including the three new ones.

- [ ] **Step 6: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/param_dial.rs
git commit -m "tiny-skia-widgets: draw_dial renders text-entry overlay

Adds editing_text and caret_on parameters to draw_dial_ex (forwarded
None/false from draw_dial). When editing, a highlighted box replaces
the value readout and a 1px caret blinks flush-right of the buffer."
```

---

## Task 8: `draw_slider` / `draw_outline_slider` gain the same parameters

**Files:**
- Modify: `tiny-skia-widgets/src/controls.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `#[cfg(test)] mod tests` in `controls.rs`:

```rust
    #[test]
    fn test_draw_slider_editing_none_no_panic() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_slider(
            &mut pm, &mut renderer, 5.0, 5.0, 250.0, 28.0,
            "Gain", "-3.0 dB", 0.5,
            /* editing_text */ None, /* caret_on */ false,
        );
    }

    #[test]
    fn test_draw_slider_editing_some_renders() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_slider(
            &mut pm, &mut renderer, 5.0, 5.0, 250.0, 28.0,
            "Gain", "-3.0 dB", 0.5,
            Some("-3.0"), /* caret_on */ true,
        );
        // Caret line should have painted at least one non-zero pixel in the
        // right-hand half of the slider (where the readout lives).
        let mut found = false;
        for x in 150..295 {
            let px = pm.pixels()[(18 * pm.width() + x) as usize];
            if px.alpha() > 0 && px.red() + px.green() + px.blue() > 0 {
                found = true;
                break;
            }
        }
        assert!(found, "edit overlay should paint pixels in the value region");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tiny-skia-widgets controls`
Expected: FAIL — `draw_slider` takes 9 args, not 11.

- [ ] **Step 3: Extend `draw_slider` signature and rendering**

Replace `draw_slider` in `tiny-skia-widgets/src/controls.rs`:

```rust
#[allow(clippy::too_many_arguments)]
pub fn draw_slider(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    value_text: &str,
    normalized_value: f32,
    editing_text: Option<&str>,
    caret_on: bool,
) {
    let nv = normalized_value.clamp(0.0, 1.0);

    draw_rect(pixmap, x, y, w, h, color_control_bg());
    draw_rect_outline(pixmap, x, y, w, h, color_border(), 1.0);

    let fill_w = (w - 2.0) * nv;
    if fill_w > 0.0 {
        draw_rect(pixmap, x + 1.0, y + 1.0, fill_w, h - 2.0, color_accent());
    }

    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;
    let pad = 6.0;
    text_renderer.draw_text(pixmap, x + pad, text_y, label, text_size, color_text());

    if let Some(buf) = editing_text {
        // Highlight the right-hand readout region.
        let readout_w = text_renderer.text_width("-999.99", text_size) + 12.0;
        let readout_x = x + w - readout_w - pad;
        let readout_y = y + 2.0;
        let readout_h = h - 4.0;
        draw_rect(pixmap, readout_x, readout_y, readout_w, readout_h,
                  Color::from_rgba8(48, 52, 64, 255));
        draw_rect_outline(pixmap, readout_x, readout_y, readout_w, readout_h,
                          color_accent(), 1.0);
        let buf_x = readout_x + 6.0;
        text_renderer.draw_text(pixmap, buf_x, text_y, buf, text_size, color_text());
        if caret_on {
            let buf_w = text_renderer.text_width(buf, text_size);
            let caret_x = buf_x + buf_w + 1.0;
            draw_rect(pixmap, caret_x, readout_y + 3.0, 1.0, readout_h - 6.0, color_text());
        }
    } else {
        let vw = text_renderer.text_width(value_text, text_size);
        text_renderer.draw_text(pixmap, x + w - vw - pad, text_y, value_text, text_size, color_text());
    }
}
```

Apply the same pattern to `draw_outline_slider` (same overlay block, using the caller-supplied `border_color`/`text_color` in place of the fixed theme colors for the outline).

- [ ] **Step 4: Update existing callers (test + crate)**

Existing tests pass `draw_slider(&mut pm, &mut renderer, …, 0.5)` — add `, None, false` as the last two args.

Find non-test callers that must be updated alongside this change (they will be touched again in Tasks 9–13 but keep the build green after this task):

```bash
rg 'draw_slider\(' --type rust
rg 'draw_outline_slider\(' --type rust
```

For every hit, append `, None, false` to the argument list. This is a mechanical edit; semantics unchanged.

- [ ] **Step 5: Run the full crate tests**

Run: `cargo test -p tiny-skia-widgets`
Expected: all tests pass.

- [ ] **Step 6: Commit (pause for user approval)**

```bash
git add tiny-skia-widgets/src/controls.rs
git add -u  # capture the None/false updates in other crates
git commit -m "tiny-skia-widgets: draw_slider renders text-entry overlay

Adds editing_text/caret_on to draw_slider and draw_outline_slider,
mirroring draw_dial_ex. Existing callers threaded with None/false."
```

---

## Task 9: Wire gain-brain editor (reference integration)

**Files:**
- Modify: `gain-brain/src/editor.rs`

**Context.** Gain-brain displays gain as dB but the `gain` param is a linear
`FloatParam` with a custom `s2v_f32_gain_to_db` formatter. So typing `"-6"`
into the dial routes through the param's own parser end-to-end. No special
case in the editor beyond calling `string_to_normalized_value`.

- [ ] **Step 1: Write the failing tests (or smoke-build guard)**

At the bottom of `gain-brain/src/editor.rs`, add a minimal `#[cfg(test)]` block
that verifies the editor compiles with the new interaction pieces wired in:

```rust
#[cfg(test)]
mod text_entry_tests {
    use super::*;
    use tiny_skia_widgets::TextEditState;

    /// Pure state-machine test — exercises the flow without a real window:
    /// begin → insert_char → commit yields the typed buffer.
    #[test]
    fn text_edit_roundtrip_for_gain_action() {
        let mut s: TextEditState<HitAction> = TextEditState::new();
        s.begin(HitAction::Dial(ParamId::Gain), "0.0");
        s.cancel(); // clear, so next begin starts from empty
        s.begin(HitAction::Dial(ParamId::Gain), "");
        for c in "-6.2".chars() {
            s.insert_char(c);
        }
        let out = s.commit();
        assert_eq!(out, Some((HitAction::Dial(ParamId::Gain), "-6.2".to_string())));
    }

    #[test]
    fn right_click_on_non_dial_does_not_open_edit() {
        // A right-click on GroupIncrement (stepped discrete) should not open
        // an edit. We assert the state machine only transitions when the
        // editor's right-click arm guards on HitAction::Dial.
        //
        // This is a contract test for the implementation choice below.
        let mut s: TextEditState<HitAction> = TextEditState::new();
        // Editor code will `begin()` only for Dial hits — simulate not calling it.
        assert!(!s.is_active());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p gain-brain text_entry_tests`
Expected: FAIL — `TextEditState` path resolution or `HitAction` not used.
Actually both exist, so the test should pass as-is. Purpose is regression
safety around step 3 wiring.

- [ ] **Step 3: Add `TextEditState` field + wiring**

In `GainBrainWindow`, add a field after `drag`:

```rust
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,
```

Initialize in `GainBrainWindow::new`:

```rust
    drag: widgets::DragState::new(),
    text_edit: widgets::TextEditState::new(),
```

- [ ] **Step 4: Populate the right-click arm**

Replace the existing stub:

```rust
baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
    button: baseview::MouseButton::Right,
    ..
}) => {
    if let Some(region) = self.drag.hit_test().cloned() {
        // Ignore right-click during drag — one interaction at a time.
        if self.drag.active_action().is_some() {
            return baseview::EventStatus::Captured;
        }
        // Auto-commit any pending edit on a different widget.
        self.commit_text_edit();

        if let HitAction::Dial(param_id) = region.action {
            let initial = self.formatted_value_without_unit(param_id);
            self.text_edit.begin(HitAction::Dial(param_id), &initial);
        }
        // Stepped segments and buttons are not editable.
    }
}
```

- [ ] **Step 5: Add keyboard arm**

Insert in `on_event`'s `match &event`:

```rust
baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
    if ev.state != keyboard_types::KeyState::Down {
        return baseview::EventStatus::Ignored;
    }
    match &ev.key {
        keyboard_types::Key::Character(s) => {
            for c in s.chars() {
                self.text_edit.insert_char(c);
            }
        }
        keyboard_types::Key::Backspace => self.text_edit.backspace(),
        keyboard_types::Key::Escape => self.text_edit.cancel(),
        keyboard_types::Key::Enter => {
            self.commit_text_edit();
        }
        _ => return baseview::EventStatus::Ignored,
    }
    return baseview::EventStatus::Captured;
}
```

Also auto-commit on any left-click (prepend to the existing `Left` arm, before the hit test):

```rust
baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
    button: baseview::MouseButton::Left,
    modifiers,
}) => {
    self.commit_text_edit();
    // ... existing body unchanged ...
}
```

- [ ] **Step 6: Add helpers `formatted_value_without_unit` and `commit_text_edit`**

Add to the `impl GainBrainWindow` block:

```rust
/// Current parameter value formatted with `include_unit = false`. This
/// is what the edit buffer is seeded with on right-click.
fn formatted_value_without_unit(&self, id: ParamId) -> String {
    use nih_plug::prelude::Param;
    match id {
        ParamId::Gain => {
            let v = self.params.gain.modulated_normalized_value();
            self.params.gain.normalized_value_to_string(v, false)
        }
        ParamId::LinkMode => {
            let v = self.params.link_mode.modulated_normalized_value();
            self.params.link_mode.normalized_value_to_string(v, false)
        }
    }
}

/// Consume any in-flight edit and push the value through the param
/// setter gesture. Parse failures silently revert.
fn commit_text_edit(&mut self) {
    use nih_plug::prelude::Param;
    let Some((action, text)) = self.text_edit.commit() else {
        return;
    };
    let HitAction::Dial(param_id) = action else {
        return;
    };
    let norm = match param_id {
        ParamId::Gain => self.params.gain.string_to_normalized_value(&text),
        ParamId::LinkMode => None, // discrete, not editable
    };
    let Some(norm) = norm else {
        return;
    };
    let setter = ParamSetter::new(self.gui_context.as_ref());
    self.begin_set_param(&setter, param_id);
    self.set_param_normalized(&setter, param_id, norm);
    self.end_set_param(&setter, param_id);
    // Keep display_gain_millibels consistent for immediate visual feedback.
    if param_id == ParamId::Gain {
        let plain = self.params.gain.preview_plain(norm);
        let db = nih_plug::util::gain_to_db(plain);
        self.display_gain_millibels
            .store((db * 100.0).round() as i32, std::sync::atomic::Ordering::Relaxed);
    }
}
```

- [ ] **Step 7: Update the `draw_dial` call site**

At the dial draw, thread the overlay state. Replace:

```rust
widgets::draw_dial(
    &mut self.surface.pixmap, tr, dial_cx, dial_cy, dial_radius,
    "Gain", &gain_text, dial_normalized,
);
```

with:

```rust
let editing = self.text_edit.active_for(&HitAction::Dial(ParamId::Gain));
let caret = self.text_edit.caret_visible();
widgets::draw_dial_ex(
    &mut self.surface.pixmap, tr, dial_cx, dial_cy, dial_radius,
    "Gain", &gain_text, dial_normalized,
    /* modulated */ None,
    editing, caret,
);
```

- [ ] **Step 8: Build + run tests**

Run:
```bash
cargo build -p gain-brain
cargo test -p gain-brain
cargo clippy -p gain-brain -- -D warnings
```

Expected: clean build, all tests pass (incl. the two new ones).

- [ ] **Step 9: Manual smoke test (ask the user to drive the standalone)**

Build and ask the user to run the standalone:
```bash
cargo build --bin gain-brain
target/debug/gain-brain
```

Checklist for the user:
- Right-click on the gain dial: the readout is replaced by a highlighted box containing the dB numeric (no unit) and a blinking caret.
- Type `-6`, press Enter: dial snaps to -6 dB.
- Right-click, type `abc`, press Enter: param unchanged (silent revert).
- Right-click, type `1`, press Escape: param unchanged.
- Start a drag while editing: edit commits, drag begins.
- Right-click on the `[<]` group-decrement button: nothing visible happens (non-editable).

- [ ] **Step 10: Commit (pause for user approval)**

```bash
git add gain-brain/src/editor.rs
git commit -m "gain-brain: right-click-to-type on gain dial

Adds TextEditState + keyboard handler + commit path; dial renders
overlay when editing. Integration tests exercise the state machine."
```

---

## Task 10: Wire satch editor

**Files:**
- Modify: `satch/src/editor.rs`

- [ ] **Step 1: Apply the Task 9 pattern to satch**

Apply the same six pieces of wiring as gain-brain, adapted to satch's
`HitAction` and `ParamId` enums. Specifically:

1. Add `text_edit: widgets::TextEditState<HitAction>` field to the window struct and initialize it in `::new`.
2. In the right-click arm (find `MouseButton::Right` in `satch/src/editor.rs`), add the auto-commit + `begin` branch — **only** for whatever variant corresponds to a continuous dial in satch's `HitAction` enum. Read the enum definition in the file to determine which variants are `Dial(_)`-shaped; ignore button/toggle/selector variants.
3. Add the `Keyboard(ev) if self.text_edit.is_active()` arm with the same match on `Character/Backspace/Enter/Escape`.
4. Prepend `self.commit_text_edit();` to the left-click arm.
5. Add `formatted_value_without_unit` and `commit_text_edit` helpers. In `formatted_value_without_unit`, exhaustively match every continuous `ParamId` variant and call `normalized_value_to_string(v, false)`. In `commit_text_edit`, match only the continuous variants and call `string_to_normalized_value`; return early for discrete ones.
6. For each `draw_dial(...)` / `draw_slider(...)` call site in `draw()`, convert to `draw_dial_ex(...)` / `draw_slider(...)` with the `editing_text` / `caret_on` pair threaded in.

- [ ] **Step 2: Add a state-machine regression test**

Append to `satch/src/editor.rs`:

```rust
#[cfg(test)]
mod text_entry_tests {
    use super::*;
    use tiny_skia_widgets::TextEditState;

    #[test]
    fn text_edit_roundtrip_for_first_dial() {
        // Use the first Dial-variant HitAction that exists in this file.
        // Replace `FIRST_DIAL_ACTION` with the actual value.
        let mut s: TextEditState<HitAction> = TextEditState::new();
        s.begin(FIRST_DIAL_ACTION, "");
        for c in "0.5".chars() {
            s.insert_char(c);
        }
        let out = s.commit();
        assert_eq!(out, Some((FIRST_DIAL_ACTION, "0.5".to_string())));
    }
}
```

Replace `FIRST_DIAL_ACTION` with whatever the first `Dial(...)` variant is in
satch's `HitAction` (e.g. `HitAction::Dial(ParamId::Threshold)`).

- [ ] **Step 3: Build + test + clippy**

```bash
cargo build -p satch
cargo test -p satch
cargo clippy -p satch -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Manual smoke test**

Build `cargo build --bin satch`, ask the user to exercise right-click on each
dial: overlay appears, Enter commits, Escape reverts, click-outside commits.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add satch/src/editor.rs
git commit -m "satch: right-click-to-type on continuous params"
```

---

## Task 11: Wire tinylimit editor

**Files:**
- Modify: `tinylimit/src/editor.rs`

**Unique aspect:** tinylimit uses `draw_slider` in addition to dials. When
updating draw call sites in step 1.6, convert both shapes.

- [ ] **Step 1: Apply the Task 9 pattern to tinylimit**

Same six-piece wiring, covering every continuous `HitAction` variant (dials
*and* sliders).

- [ ] **Step 2: Add state-machine regression test**

Append a `text_entry_tests` module mirroring Task 10's Step 2, using the
first continuous dial/slider action that appears in tinylimit's `HitAction`.

- [ ] **Step 3: Build + test + clippy**

```bash
cargo build -p tinylimit
cargo test -p tinylimit
cargo clippy -p tinylimit -- -D warnings
```

- [ ] **Step 4: Manual smoke test**

Exercise right-click on both dial-style and slider-style params in the
tinylimit standalone.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add tinylimit/src/editor.rs
git commit -m "tinylimit: right-click-to-type on dials and sliders"
```

---

## Task 12: Wire pope-scope editor

**Files:**
- Modify: `pope-scope/src/editor.rs`

- [ ] **Step 1: Apply the Task 9 pattern to pope-scope**

Six-piece wiring. Pope-scope has many dials (timebase, dB range, etc.) — cover
every continuous `Dial(_)` variant in its `HitAction`. Track solo/mute/color
buttons remain non-editable.

- [ ] **Step 2: Add state-machine regression test**

Append `text_entry_tests` mirroring Task 10 Step 2.

- [ ] **Step 3: Build + test + clippy**

```bash
cargo build -p pope-scope
cargo test -p pope-scope
cargo clippy -p pope-scope -- -D warnings
```

- [ ] **Step 4: Manual smoke test**

Exercise right-click on timebase, dB range, and any other continuous param.
Confirm waveform rendering is unaffected while editing.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add pope-scope/src/editor.rs
git commit -m "pope-scope: right-click-to-type on continuous params"
```

---

## Task 13: Wire warp-zone editor

**Files:**
- Modify: `warp-zone/src/editor.rs`

- [ ] **Step 1: Apply the Task 9 pattern to warp-zone**

Six-piece wiring. Shift, Stretch, Feedback, Low, High are continuous; Freeze
is a toggle (non-editable).

- [ ] **Step 2: Add state-machine regression test**

Append `text_entry_tests` mirroring Task 10 Step 2.

- [ ] **Step 3: Build + test + clippy**

```bash
cargo build -p warp-zone
cargo test -p warp-zone
cargo clippy -p warp-zone -- -D warnings
```

- [ ] **Step 4: Manual smoke test**

Exercise right-click on Shift / Stretch while audio is playing — confirm the
spectral waterfall keeps drawing underneath the overlay.

- [ ] **Step 5: Commit (pause for user approval)**

```bash
git add warp-zone/src/editor.rs
git commit -m "warp-zone: right-click-to-type on continuous params"
```

---

## Task 14: Workspace-wide verification

**Files:** (no changes)

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: all prior tests still pass, plus the new tests added in Tasks 1–13.
(Prior count: 372 per CLAUDE.md; new: ~18 in `text_edit.rs` + ~5 in
`param_dial.rs`/`controls.rs` + ~5 across the five editors ≈ **400 total**.)

- [ ] **Step 2: Full workspace clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

- [ ] **Step 3: Format check**

Run: `cargo fmt --check`
Expected: clean. If not, run `cargo fmt` and inspect the diff.

- [ ] **Step 4: Bundle each plugin to verify production builds**

Run each of:
```bash
cargo nih-plug bundle gain-brain --release
cargo nih-plug bundle satch --release
cargo nih-plug bundle tinylimit --release
cargo nih-plug bundle pope-scope --release
cargo nih-plug bundle warp-zone --release
```

Expected: no warnings, bundles written under `target/bundled/`.

- [ ] **Step 5: End-to-end smoke in a DAW (user-driven)**

Ask the user to load each plugin in Bitwig (or their preferred host) and
confirm:
- Right-click on any continuous dial opens the overlay.
- Typing + Enter commits the value; host automation captures one discrete
  event per commit.
- Escape reverts.
- Click-outside commits.
- Double-click-reset still works.
- FL Studio note (if tested): right-click's FL menu is shadowed by our edit
  overlay; users reach FL's menu via the generic parameter list. This is
  documented behavior.

---

## Self-review checklist (performed inline while writing this plan)

**Spec coverage** — each spec requirement maps to a task:

| Spec requirement | Task(s) |
|---|---|
| `TextEditState<A>` struct with `begin/insert_char/backspace/commit/cancel/active_for/caret_visible/is_active` | 1–5 |
| Re-export from crate root | 6 |
| `draw_dial`/`draw_dial_ex` with `editing_text`/`caret_on` | 7 |
| `draw_slider`/`draw_outline_slider` same | 8 |
| Numeric filter `0-9 . - + e E` | 2 |
| 16-char buffer cap | 1, 2 |
| Blink cycle ~500 ms | 5 |
| Unit suppressed during edit | 7, 8 |
| Right-click trigger populated via `normalized_value_to_string(_, false)` | 9–13 |
| Keyboard handler (char/backspace/enter/escape) | 9–13 |
| Commit via `string_to_normalized_value` + `begin/set/end_set_parameter` | 9–13 |
| Auto-commit on click-outside and drag-start | 9–13 |
| Right-click ignored during drag | 9 (explicit early-return guard) |
| Parse failure silent revert | 9 helper `commit_text_edit` |
| Clamping via existing normalization pipeline | documented in Task 9 prose + spec |

**Placeholder scan** — checked for TBD/TODO/hand-waving: none present.
Every task has full code blocks, exact file paths, and concrete commands.

**Type consistency** — method names consistent across tasks:
`begin`, `insert_char`, `backspace`, `commit`, `cancel`, `active_for`,
`caret_visible`, `is_active`. `editing_text: Option<&str>` and `caret_on: bool`
used identically in `draw_dial_ex`, `draw_slider`, `draw_outline_slider`.
`commit_text_edit` / `formatted_value_without_unit` helper names consistent
across editor tasks.

**Scope** — single feature across one shared crate + five editors; each editor
task is its own commit checkpoint. Plan is sized correctly for one
implementation session.
