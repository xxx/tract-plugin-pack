# Tiny-Skia Widgets: Right-Click Text Entry — Design

**Date:** 2026-04-19
**Status:** Spec, pending implementation plan
**Scope:** `tiny-skia-widgets` crate and the five softbuffer-based plugin editors
(gain-brain, satch, tinylimit, pope-scope, warp-zone).

## Summary

Add right-click-to-type-value support to the continuous widgets in
`tiny-skia-widgets` (dials and sliders). Right-click on a dial or slider opens
an inline numeric edit field in the widget's existing value-readout region.
Enter commits, Escape cancels, click-outside commits, parse failures silently
revert. Double-click-to-reset and all existing drag behavior are unchanged.

## Rationale

Continuous params currently accept only drag input. Typing a precise value
("-6.2" dB, "440" Hz) is a standard affordance in every modern plugin
(Serum, Vital, Pro-Q, Kilohearts, Bitwig/Reaper stock). Right-click is the
least invasive trigger: it adds no keyboard modifier, avoids the triple-click
race with double-click reset, and works uniformly across Bitwig, Reaper,
Ableton, Logic, Studio One, and Cubase because inside a plugin-owned GUI
window the plugin receives all mouse input. FL Studio is the one exception —
it hooks right-click on plugin knobs for its "Link to controller" menu, so
FL users will reach that menu via the generic parameter list instead. This
trade-off is consistent with what other custom-GUI plugins do in FL.

## Non-goals

- Caret movement, text selection, clipboard (future work if needed).
- Right-click context menus with additional actions (MIDI learn, copy value).
- Editing discrete widgets (`draw_button`, `draw_stepped_selector`) — text
  entry doesn't map cleanly to discrete choices; skip them.
- Replacing double-click reset; it stays.

## Interaction model

1. User right-clicks a dial or slider hit region.
2. The widget's value-readout region ("-6.2 dB") is replaced by a highlighted
   edit field showing just the numeric portion ("-6.2"). Unit text is
   suppressed during edit.
3. User types. Only `0-9 . - + e E` are accepted; other characters are silently
   rejected. Buffer is capped at 16 chars.
4. Caret blinks on ~500 ms half-phases, rendered flush right of the current
   buffer.
5. **Enter** commits: buffer → `Param::string_to_normalized_value` →
   `begin/set/end_set_parameter` gesture. Out-of-range values clamp to the
   param's min/max via the existing normalization pipeline. Parse failures
   (non-numeric garbage) silently revert without changing the param.
6. **Escape** cancels without modifying the param.
7. **Click outside the edit region** (any mouse button, any hit) auto-commits
   the active edit, then processes the new click normally.
8. **Starting a drag on any widget** auto-commits the active edit first.
9. **Right-click during a drag** is ignored — one interaction at a time.
10. **Right-click on a different widget while editing** auto-commits the
    first edit and opens a new one on the second widget.

## Architecture

### New module: `tiny-skia-widgets/src/text_edit.rs`

```rust
pub struct TextEditState<A: Clone + PartialEq> {
    active: Option<A>,
    buffer: String,
    started_at: Instant,
}

impl<A: Clone + PartialEq> TextEditState<A> {
    pub fn new() -> Self;

    /// Open edit on `action` with `initial` as the starting buffer.
    /// If already editing a different action, replaces it (the editor is
    /// expected to call `commit()` first — see note below).
    pub fn begin(&mut self, action: A, initial: &str);

    /// Append `c` if it is a valid numeric char (`0-9 . - + e E`) and the
    /// buffer is under the 16-char cap. Silent no-op otherwise.
    pub fn insert_char(&mut self, c: char);

    /// Remove the last char. No-op on empty buffer.
    pub fn backspace(&mut self);

    /// Return `(action, buffer)` and clear state. `None` if nothing active.
    pub fn commit(&mut self) -> Option<(A, String)>;

    /// Clear state without returning anything.
    pub fn cancel(&mut self);

    /// `Some(&buffer)` if editing `action`, else `None`. Widgets use this to
    /// decide whether to render the edit field instead of the value readout.
    pub fn active_for(&self, action: &A) -> Option<&str>;

    /// `true` during visible half of blink cycle.
    pub fn caret_visible(&self) -> bool;
}
```

**Note on auto-commit via `begin`.** Because `begin` cannot return the
pending committable value without an awkward signature, the editor is
expected to call `commit()` first if it detects an edit-context switch
(new right-click on a different widget, drag start, click outside).
`begin` on an already-active state simply replaces the active action
and buffer; no value is lost because the editor committed first.

### Widget API change

`draw_dial` (in `param_dial.rs`) and `draw_slider` (in `controls.rs`) gain
one new parameter:

```rust
editing_text: Option<&str>,
```

When `Some(buf)`:
- Fill the value-readout region with a highlight (brighter variant of
  theme fill + 1px inset border).
- Render `buf` in place of the formatted value; unit suffix is not drawn.
- Draw a 1px caret flush right of the buffer when
  `TextEditState::caret_visible()` is true (editor passes this through, or
  the widget receives a separate `caret_on: bool` — see API detail below).

When `None`: behavior is byte-identical to today's render path. Existing
callers that don't opt in get zero regression.

**API detail.** To keep the widget signature flat, split into two fields:

```rust
editing_text: Option<&str>,
caret_on: bool,   // ignored when editing_text is None
```

This avoids packaging a state reference into the draw call and keeps the
widget pure.

### Editor-side state

Each of the five editors adds a `TextEditState<HitAction>` field alongside
the existing `DragState<HitAction>`. The two states are independent; the
interaction contract between them is:

- **Drag active ⇒ ignore right-click.**
- **Edit active + drag starts ⇒ auto-commit edit first.**
- **Both never active simultaneously.**

### Keyboard event handling

Each editor extends its `on_event` to handle baseview `KeyboardEvent`s:

- `CharacterInput(c)` → `text_edit.insert_char(c)`
- `KeyDown(Backspace)` → `text_edit.backspace()`
- `KeyDown(Enter)` → editor runs the commit path (see below)
- `KeyDown(Escape)` → `text_edit.cancel()`

When `text_edit.active_for(_).is_none()` for every hit action, the editor
returns `EventStatus::Ignored` for keyboard events, so DAW shortcuts
(transport, etc.) pass through unaffected.

### Commit path (shared shape across editors)

```rust
if let Some((action, text)) = self.text_edit.commit() {
    if let Some(param) = self.param_for_action(&action) {
        if let Some(norm) = param.string_to_normalized_value(&text) {
            setter.begin_set_parameter(param);
            setter.set_parameter_normalized(param, norm);
            setter.end_set_parameter(param);
        }
        // parse failure: silent revert
    }
}
```

The `begin/set/end` triple matches existing reset-to-default and drag
gestures — host sees one atomic automation event.

### Value clamping

Handled automatically. `Param::string_to_normalized_value` returns an
`Option<f32>` already in `[0, 1]` because it runs `preview_normalized`
under the hood, applying the param's `FloatRange` / `IntRange` bounds.
`set_parameter_normalized` clamps again in the host path. No additional
clamping logic in `TextEditState`.

**Caveat.** If a plugin uses a custom `value_to_string_fn` /
`string_to_value_fn` pair, the two must round-trip consistently for
non-unit text. None of the current plugins do, but new ones must
respect this contract.

## Affected files

### New
- `tiny-skia-widgets/src/text_edit.rs` — `TextEditState<A>` + unit tests.

### Modified
- `tiny-skia-widgets/src/lib.rs` — re-export `TextEditState`.
- `tiny-skia-widgets/src/param_dial.rs` — add `editing_text`, `caret_on`;
  render highlight + caret when editing.
- `tiny-skia-widgets/src/controls.rs` — same treatment for `draw_slider`.
- `gain-brain/src/editor.rs` — `TextEditState` field, right-click arm,
  keyboard arm, commit wiring, call-site updates for `draw_dial`.
- `satch/src/editor.rs` — same.
- `tinylimit/src/editor.rs` — same; includes `draw_slider` call sites.
- `pope-scope/src/editor.rs` — same.
- `warp-zone/src/editor.rs` — same.

## Test plan

### `TextEditState` unit tests (in `text_edit.rs`)
- `new()` reports no active edit.
- `begin(A, "init")` sets active, buffer == "init".
- `insert_char` accepts each of `0-9 . - + e E`.
- `insert_char` rejects letters (`a`, `z`), whitespace, symbols (`%`, `/`).
- `insert_char` is a no-op once the buffer hits 16 chars.
- `backspace` removes the last char; no-op on empty.
- `commit` on no-active returns `None`.
- `commit` on active returns `(action, buffer)` and clears state.
- `cancel` clears state; next `commit` returns `None`.
- `active_for(&A)` returns `Some(buf)` iff `A` matches the active action.
- `begin` on an already-active state replaces action and buffer.
- `caret_visible()` flips across the 500 ms half-phase boundary.

### Widget render tests (in `param_dial.rs` and `controls.rs`)
- `draw_dial` with `editing_text = None` matches a golden pixel hash of
  the current output (regression guard).
- `draw_dial` with `editing_text = Some("-6.2")`, `caret_on = true` draws
  highlight background, no unit suffix, caret pixel at the right edge of
  the buffer's last glyph.
- Same pair for `draw_slider`.

### Integration (in one editor, pick gain-brain for minimal fixtures)
- Right-click on gain-dial hit region opens edit; `active_for` returns the
  gain action.
- Keyboard Enter with buffer `"-6"` calls `string_to_normalized_value` and
  commits via the param setter gesture.
- Escape reverts — param value unchanged.
- Left-click outside the dial commits the pending edit, then processes
  the new click.
- Right-click during an active drag is ignored.
- Right-click on a second dial while editing the first commits the first
  and opens the second.

Estimated ~30 new tests across the module.

## Risks / open questions

- **Baseview keyboard focus semantics.** Softbuffer/baseview delivers key
  events to the editor window whenever the DAW gives the plugin window
  keyboard focus. On some hosts, focus may not land on the plugin window
  until after the user clicks inside it — the right-click that opens the
  edit should satisfy this on every host we care about, but confirm in
  Bitwig and Reaper at minimum during implementation.
- **FL Studio right-click intercept.** Users expect FL's context menu on
  right-click. We document this trade-off; workaround is the generic
  parameter list. If it becomes a real complaint, a per-host override
  (Alt+right-click in FL?) can be added later — not in scope here.
- **Fontdue caret measurement.** `TextRenderer::measure_text` must return
  reliable widths for partially-typed buffers (including trailing `-`,
  `e`, etc.). Existing text rendering has been stable; no known issues.
