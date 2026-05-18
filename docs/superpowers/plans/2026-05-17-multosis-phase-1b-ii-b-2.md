# Multosis Phase 1 — Milestone 1b-ii-b-2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the editor's toolbar parameter controls — Speed, Effect bank, Auto-Restart, Mix, Output gain — and a Reset button, all interactive.

**Architecture:** The top strip grows to two logical rows (`STATUS_H` 48→88); this milestone fills the upper row with six controls. A new `editor/toolbar.rs` defines each control's rectangle and a click hit-test. Speed/Bank/Auto-Restart/Reset are cycle/toggle/fire buttons driven by `ParamSetter`; Mix/Output are drag sliders driven by `DragState`. Reset reaches the audio thread through a new `Arc<AtomicBool>` request flag consumed in `process()`. The six grid operations (1b-ii-b-3) and the loop-region drag handles (1b-ii-b-4) come next.

**Tech Stack:** Rust (nightly), nih-plug, baseview + softbuffer + tiny-skia + `tiny-skia-widgets` (`controls`, `drag`), `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §3.1, §7.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (Milestone 1b-ii-b-1, 91 tests green):** The editor renders the grid + wavefront and edits cells on click. `editor.rs`: `MultosisWindow` (fields `surface`, `physical_width`, `physical_height`, `scale_factor`, `pending_resize`, `params`, `wavefront_display`, `grid_handoff`, `mouse_pos`, `text_renderer`; `on_event` handles Resized/CursorMoved/ButtonPressed), `MultosisEditor`, `create(params, wavefront_display, grid_handoff)`, `WINDOW_WIDTH = 1056`, `WINDOW_HEIGHT = 576`. `editor/grid_view.rs`: `STATUS_H = 48.0`, `CELL = 33.0`, `cell_rect`/`cell_at`/`cell_zone`, `draw_grid`/`draw_wavefront`/`draw_status`. `MultosisParams`: `editor_state`, `grid` (`Arc<Mutex<Grid>>`), `speed: EnumParam<Speed>`, `mix: FloatParam`, `output_gain: FloatParam`, `effect_bank: EnumParam<EffectBank>`, `auto_restart: BoolParam`. `Speed` (clock.rs) has `ALL: [Speed;6]`. `EffectBank` (effects.rs): `Lowpass`/`Bitcrush`. The plugin `Multosis` has `engine: AudioEngine` with `engine.reset()`. `tiny_skia_widgets`: `controls::{draw_button, draw_slider}`, `DragState`, `TextRenderer`, `color_*`.

---

### Task 1: Grow the top strip to a two-row toolbar

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/lib.rs`

The grid sits below the strip; growing `STATUS_H` shifts the grid down and grows the window. `cell_rect`/`cell_at`/`cell_zone` all derive from the `STATUS_H` constant, so they need no code change — only the constant and the window height.

- [ ] **Step 1: Update the geometry test**

In `multosis/src/editor/grid_view.rs`, the test `window_size_matches_the_grid` already cross-checks `WINDOW_WIDTH`/`WINDOW_HEIGHT` against `STATUS_H`/`CELL`. It needs no edit — it will fail until the constants below are consistent. Confirm it currently passes (`cargo nextest run -p multosis --lib window_size_matches_the_grid` → PASS), then proceed.

- [ ] **Step 2: Grow `STATUS_H`**

In `multosis/src/editor/grid_view.rs`, change the `STATUS_H` constant:

```rust
/// Logical height of the top toolbar strip (two rows of `TOOLBAR_ROW_H`).
pub const STATUS_H: f32 = 88.0;
/// Logical height of one toolbar row.
pub const TOOLBAR_ROW_H: f32 = 44.0;
```

(Add `TOOLBAR_ROW_H` as a new constant next to `STATUS_H`; `88.0 == 2.0 * 44.0`.)

- [ ] **Step 3: Grow the window height**

In `multosis/src/editor.rs`, change the `WINDOW_HEIGHT` constant:

```rust
pub const WINDOW_HEIGHT: u32 = 616;
```

(`616 == STATUS_H(88) + ROWS(16) * CELL(33)` = 88 + 528.)

- [ ] **Step 4: Update the persisted default editor size**

In `multosis/src/lib.rs`, `impl Default for MultosisParams` initialises `editor_state` with `EditorState::from_size(1056, 576)`. Change the height:

```rust
            editor_state: tiny_skia_widgets::EditorState::from_size(1056, 616),
```

- [ ] **Step 5: Verify**

Run: `cargo nextest run -p multosis`
Expected: PASS — 91 tests; `window_size_matches_the_grid` still passes with the new consistent constants.

Run: `cargo build -p multosis`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs multosis/src/lib.rs
git commit -m "feat(multosis): grow the editor strip to a two-row toolbar"
```

---

### Task 2: Reset-request channel (editor → audio)

**Files:**
- Modify: `multosis/src/lib.rs`

The Reset button lives on the GUI thread; the `Propagator` lives on the audio thread. A shared `Arc<AtomicBool>` carries the request: the editor sets it, `process()` consumes it. Plugin glue — verified by compilation.

- [ ] **Step 1: Add the request flag to the plugin**

In `multosis/src/lib.rs`, add a field to the `Multosis` struct (after `wavefront_display`):

```rust
    /// Set by the editor's Reset button; consumed once per process block.
    reset_request: Arc<std::sync::atomic::AtomicBool>,
```

In `impl Default for Multosis`, add to the returned struct:

```rust
            reset_request: Arc::new(std::sync::atomic::AtomicBool::new(false)),
```

- [ ] **Step 2: Consume the flag in `process()`**

In `impl Plugin for Multosis`, in `process()`, immediately AFTER the `was_playing` update line (`self.was_playing = playing;`) and BEFORE the `grid_handoff.try_read()` line, add:

```rust
        // A Reset request from the editor resets the sequence.
        if self.reset_request.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.engine.reset();
        }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. A `dead_code` warning for `reset_request` being written-but-never-set (the editor wires it in Task 5) is EXPECTED — do NOT suppress it. No errors. Run `cargo nextest run -p multosis` — PASS, 91 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/lib.rs
git commit -m "feat(multosis): add the editor-to-audio reset-request flag"
```

---

### Task 3: Toolbar control geometry

**Files:**
- Create: `multosis/src/editor/toolbar.rs`
- Modify: `multosis/src/editor.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/editor/toolbar.rs`:

```rust
//! The editor toolbar: parameter controls + Reset, laid out across the upper
//! row of the top strip. Geometry is logical; every draw multiplies by scale.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7.

use crate::editor::grid_view::TOOLBAR_ROW_H;
use crate::MultosisParams;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// One toolbar control in the upper row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarControl {
    /// Cycles the wavefront speed.
    Speed,
    /// Cycles the throwaway effect bank.
    Bank,
    /// Toggles auto-restart.
    AutoRestart,
    /// Drag slider — dry/wet mix.
    Mix,
    /// Drag slider — output gain.
    Output,
    /// Resets the sequence.
    Reset,
}

impl ToolbarControl {
    /// The six controls, left to right.
    pub const ALL: [ToolbarControl; 6] = [
        ToolbarControl::Speed,
        ToolbarControl::Bank,
        ToolbarControl::AutoRestart,
        ToolbarControl::Mix,
        ToolbarControl::Output,
        ToolbarControl::Reset,
    ];

    /// Logical `(x, width)` of this control. The row is 1056 logical wide.
    fn logical_x_w(self) -> (f32, f32) {
        match self {
            ToolbarControl::Speed => (6.0, 200.0),
            ToolbarControl::Bank => (212.0, 160.0),
            ToolbarControl::AutoRestart => (378.0, 120.0),
            ToolbarControl::Mix => (504.0, 180.0),
            ToolbarControl::Output => (690.0, 180.0),
            ToolbarControl::Reset => (876.0, 174.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_rects_sit_in_the_upper_toolbar_row() {
        for ctrl in ToolbarControl::ALL {
            let (x, y, w, h) = control_rect(ctrl, 1.0);
            assert!(x >= 0.0 && x + w <= 1056.0, "{ctrl:?} out of width");
            // Entirely within the upper row.
            assert!(y >= 0.0 && y + h <= TOOLBAR_ROW_H, "{ctrl:?} out of row");
        }
    }

    #[test]
    fn control_rects_do_not_overlap() {
        let mut rects: Vec<(f32, f32)> = ToolbarControl::ALL
            .iter()
            .map(|c| {
                let (x, _, w, _) = control_rect(*c, 1.0);
                (x, x + w)
            })
            .collect();
        rects.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for pair in rects.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "controls overlap: {pair:?}");
        }
    }

    #[test]
    fn toolbar_hit_finds_the_control_and_misses_the_grid() {
        let (x, y, w, h) = control_rect(ToolbarControl::Mix, 1.5);
        assert_eq!(
            toolbar_hit(x + w / 2.0, y + h / 2.0, 1.5),
            Some(ToolbarControl::Mix)
        );
        // A point in the grid area (well below the strip) is not a toolbar hit.
        assert_eq!(toolbar_hit(500.0, 400.0, 1.0), None);
    }
}
```

Add `pub mod toolbar;` to `multosis/src/editor.rs` (next to `pub mod grid_view;`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib toolbar`
Expected: build failure — `cannot find function control_rect`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/toolbar.rs`, after the `impl ToolbarControl` block (before the `#[cfg(test)]` module):

```rust
/// Vertical inset of a control within its toolbar row, logical px.
const CTRL_INSET: f32 = 4.0;

/// The physical-pixel rectangle `(x, y, w, h)` of `ctrl` at `scale`.
pub fn control_rect(ctrl: ToolbarControl, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = ctrl.logical_x_w();
    let x = lx * scale;
    let y = CTRL_INSET * scale;
    let w = lw * scale;
    let h = (TOOLBAR_ROW_H - 2.0 * CTRL_INSET) * scale;
    (x, y, w, h)
}

/// The toolbar control under physical-pixel point `(px, py)` at `scale`, or
/// `None` if the point hits no control.
pub fn toolbar_hit(px: f32, py: f32, scale: f32) -> Option<ToolbarControl> {
    ToolbarControl::ALL.into_iter().find(|&ctrl| {
        let (x, y, w, h) = control_rect(ctrl, scale);
        px >= x && px < x + w && py >= y && py < y + h
    })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib toolbar`
Expected: PASS — 3 tests.

Then run `cargo build -p multosis`. Expected: compiles. Unused-import warnings in `toolbar.rs` for `Pixmap`, `widgets`, `MultosisParams` (Task 4 consumes them) are EXPECTED — do NOT remove them.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/toolbar.rs multosis/src/editor.rs
git commit -m "feat(multosis): add toolbar control geometry"
```

---

### Task 4: Render the toolbar

**Files:**
- Modify: `multosis/src/editor/toolbar.rs`
- Modify: `multosis/src/editor.rs`

Rendering — verified by compilation; visual check in Task 8.

- [ ] **Step 1: Add the toolbar renderer**

Add to `multosis/src/editor/toolbar.rs`, after the `toolbar_hit` function (before the `#[cfg(test)]` module):

```rust
/// Draw the toolbar strip and its six upper-row controls.
pub fn draw_toolbar(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    params: &MultosisParams,
    scale: f32,
) {
    // The whole two-row strip background.
    let strip_h = crate::editor::grid_view::STATUS_H * scale;
    widgets::draw_rect(
        pixmap,
        0.0,
        0.0,
        pixmap.width() as f32,
        strip_h,
        widgets::color_control_bg(),
    );

    for ctrl in ToolbarControl::ALL {
        let (x, y, w, h) = control_rect(ctrl, scale);
        match ctrl {
            ToolbarControl::Speed => {
                let label = format!("Speed: {}", speed_label(params.speed.value()));
                widgets::draw_button(pixmap, tr, x, y, w, h, &label, false, false);
            }
            ToolbarControl::Bank => {
                let label = format!("Effect: {}", bank_label(params.effect_bank.value()));
                widgets::draw_button(pixmap, tr, x, y, w, h, &label, false, false);
            }
            ToolbarControl::AutoRestart => {
                let on = params.auto_restart.value();
                widgets::draw_button(pixmap, tr, x, y, w, h, "Auto-Restart", on, false);
            }
            ToolbarControl::Mix => {
                let v = params.mix.value();
                widgets::draw_slider(
                    pixmap, tr, x, y, w, h, "Mix",
                    &format!("{}%", (v * 100.0).round() as i32),
                    v, None, false,
                );
            }
            ToolbarControl::Output => {
                let norm = params.output_gain.unmodulated_normalized_value();
                let db = nih_plug::util::gain_to_db(params.output_gain.value());
                widgets::draw_slider(
                    pixmap, tr, x, y, w, h, "Out",
                    &format!("{db:.1} dB"),
                    norm, None, false,
                );
            }
            ToolbarControl::Reset => {
                widgets::draw_button(pixmap, tr, x, y, w, h, "Reset", false, false);
            }
        }
    }
}

/// Short label for a `Speed`.
fn speed_label(s: crate::clock::Speed) -> &'static str {
    use crate::clock::Speed;
    match s {
        Speed::Div32 => "1/32",
        Speed::Div16 => "1/16",
        Speed::Div8 => "1/8",
        Speed::Div4 => "1/4",
        Speed::Div2 => "1/2",
        Speed::Div1 => "1/1",
    }
}

/// Short label for an `EffectBank`.
fn bank_label(b: crate::effects::EffectBank) -> &'static str {
    use crate::effects::EffectBank;
    match b {
        EffectBank::Lowpass => "Lowpass",
        EffectBank::Bitcrush => "Bitcrush",
    }
}
```

- [ ] **Step 2: Replace the status-strip draw call**

In `multosis/src/editor.rs`, `MultosisWindow::draw` currently calls `grid_view::draw_status(...)`. Replace that call with:

```rust
        toolbar::draw_toolbar(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.params,
            self.scale_factor,
        );
```

(The `draw_grid` and `draw_wavefront` calls stay as they are.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles cleanly — NO warnings (the `toolbar.rs` imports are now all used; `draw_status` in `grid_view.rs` is still used by nothing — see the note below).

> **Note on `draw_status`:** `grid_view::draw_status` is no longer called. Rust does not warn about unused *public* functions, so this does not break the build. Leave `draw_status` in place — removing it is out of scope; a later cleanup may drop it.

Run: `cargo nextest run -p multosis` — PASS, 94 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/toolbar.rs multosis/src/editor.rs
git commit -m "feat(multosis): render the editor toolbar"
```

---

### Task 5: Thread `GuiContext`, the reset flag, and a `DragState` into the editor

**Files:**
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/lib.rs`

Editor wiring — verified by compilation. The toolbar needs a `ParamSetter` (which needs the `GuiContext`), the reset flag, and drag state for the sliders.

- [ ] **Step 1: Extend the editor imports and structs**

In `multosis/src/editor.rs`:

(a) Ensure these imports are present (add any that are missing):

```rust
use crate::editor::toolbar::ToolbarControl;
use std::sync::atomic::AtomicBool;
```

(b) Add three fields to the `MultosisWindow` struct (after `text_renderer`):

```rust
    gui_context: Arc<dyn GuiContext>,
    reset_request: Arc<AtomicBool>,
    toolbar_drag: widgets::DragState<ToolbarControl>,
```

(c) `MultosisWindow::new` — add parameters `gui_context: Arc<dyn GuiContext>` and `reset_request: Arc<AtomicBool>` (after `pending_resize`), store `gui_context` and `reset_request`, and initialise `toolbar_drag: widgets::DragState::new()` in the returned struct.

(d) Add two fields to the `MultosisEditor` struct (after `grid_handoff`):

```rust
    reset_request: Arc<AtomicBool>,
```

(e) Change `create` to take and forward the reset flag:

```rust
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        grid_handoff,
        reset_request,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}
```

(f) In `Editor::spawn`, the signature's `_context` parameter is currently unused — rename it to `context`. Add `let gui_context = Arc::clone(&context);` and `let reset_request = Arc::clone(&self.reset_request);` alongside the other clones, and pass `gui_context` and `reset_request` to `MultosisWindow::new` (in the position matching the new `new` parameters — after the `pending_resize` argument).

- [ ] **Step 2: Update the `editor()` call in `lib.rs`**

In `multosis/src/lib.rs`, the `editor()` method's `editor::create(...)` call passes three arguments. Add a fourth:

```rust
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.wavefront_display.clone(),
            self.grid_handoff.clone(),
            self.reset_request.clone(),
        )
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. `dead_code` warnings for the `MultosisWindow` fields `gui_context`, `reset_request`, and `toolbar_drag` (consumed in Tasks 6–7) are EXPECTED — do NOT suppress them. The plugin-side `reset_request` `dead_code` warning from Task 2 is now resolved (the editor sets it). No errors. Run `cargo nextest run -p multosis` — PASS, 94 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor.rs multosis/src/lib.rs
git commit -m "feat(multosis): thread GuiContext and the reset flag into the editor"
```

---

### Task 6: Handle the toolbar buttons

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation. Wires clicks on the Speed/Bank/Auto-Restart/Reset controls.

- [ ] **Step 1: Add the button handler**

Add a method to the `impl MultosisWindow` block (after `handle_grid_click`):

```rust
    /// Handle a left click on a non-slider toolbar control.
    fn handle_toolbar_button(&mut self, ctrl: ToolbarControl) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Speed => {
                // Cycle to the next speed division.
                let all = crate::clock::Speed::ALL;
                let cur = self.params.speed.value();
                let idx = all.iter().position(|&s| s == cur).unwrap_or(0);
                let next = all[(idx + 1) % all.len()];
                setter.begin_set_parameter(&self.params.speed);
                setter.set_parameter(&self.params.speed, next);
                setter.end_set_parameter(&self.params.speed);
            }
            ToolbarControl::Bank => {
                use crate::effects::EffectBank;
                let next = match self.params.effect_bank.value() {
                    EffectBank::Lowpass => EffectBank::Bitcrush,
                    EffectBank::Bitcrush => EffectBank::Lowpass,
                };
                setter.begin_set_parameter(&self.params.effect_bank);
                setter.set_parameter(&self.params.effect_bank, next);
                setter.end_set_parameter(&self.params.effect_bank);
            }
            ToolbarControl::AutoRestart => {
                let next = !self.params.auto_restart.value();
                setter.begin_set_parameter(&self.params.auto_restart);
                setter.set_parameter(&self.params.auto_restart, next);
                setter.end_set_parameter(&self.params.auto_restart);
            }
            ToolbarControl::Reset => {
                self.reset_request
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // Mix/Output are drag sliders — handled in Task 7, not here.
            ToolbarControl::Mix | ToolbarControl::Output => {}
        }
    }
```

- [ ] **Step 2: Dispatch toolbar clicks in `on_event`**

In `multosis/src/editor.rs`, in `on_event`, the `ButtonPressed { Left, .. }` arm currently calls `self.handle_grid_click(false)`. Replace the body of that arm so a click is routed to the toolbar first, and only falls through to the grid when it misses the toolbar:

```rust
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                let (px, py) = self.mouse_pos;
                if let Some(ctrl) = toolbar::toolbar_hit(px, py, self.scale_factor) {
                    self.handle_toolbar_button(ctrl);
                } else {
                    self.handle_grid_click(false);
                }
            }
```

(The `ButtonPressed { Right, .. }` arm stays as it is — right-click only edits the grid.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. The remaining expected `dead_code` warning is for the `MultosisWindow` field `toolbar_drag` (consumed in Task 7). The `gui_context` and `reset_request` fields are now read. No errors. Run `cargo nextest run -p multosis` — PASS, 94 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): wire the toolbar's Speed/Bank/Auto-Restart/Reset buttons"
```

---

### Task 7: Handle the Mix and Output drag sliders

**Files:**
- Modify: `multosis/src/editor.rs`

Editor wiring — verified by compilation. After this task the build is warning-free.

- [ ] **Step 1: Extend `on_event` for slider drags**

The `toolbar_drag: DragState<ToolbarControl>` field tracks an active slider drag. `DragState` needs the mouse position fed to it on `CursorMoved`, a drag begun on `ButtonPressed`, the drag updated on `CursorMoved`, and ended on `ButtonReleased`.

In `multosis/src/editor.rs`, make these four changes to `on_event`:

(a) In the `CursorMoved` arm, after `self.mouse_pos = (...)`, also feed the drag state and apply an in-progress slider drag:

```rust
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) => {
                let (px, py) = (position.x as f32, position.y as f32);
                self.mouse_pos = (px, py);
                self.toolbar_drag.set_mouse(px, py);
                if let Some(&ctrl) = self.toolbar_drag.active_action() {
                    let current = self.slider_normalized(ctrl);
                    if let Some(norm) = self.toolbar_drag.update_drag(false, current) {
                        self.set_slider(ctrl, norm);
                    }
                }
            }
```

(b) In the `ButtonPressed { Left, .. }` arm, when the click hits a Mix/Output control, begin a drag instead of treating it as a button. Replace that arm's body with:

```rust
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                let (px, py) = self.mouse_pos;
                match toolbar::toolbar_hit(px, py, self.scale_factor) {
                    Some(ctrl @ (ToolbarControl::Mix | ToolbarControl::Output)) => {
                        let current = self.slider_normalized(ctrl);
                        self.toolbar_drag.begin_drag(ctrl, current, false);
                    }
                    Some(ctrl) => self.handle_toolbar_button(ctrl),
                    None => self.handle_grid_click(false),
                }
            }
```

(c) Add a `ButtonReleased { Left, .. }` arm (place it after the `ButtonPressed { Right }` arm, before the `_ => {}` arm):

```rust
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                self.toolbar_drag.end_drag();
            }
```

- [ ] **Step 2: Add the slider helper methods**

Add two methods to the `impl MultosisWindow` block (after `handle_toolbar_button`):

```rust
    /// The current normalized value of a slider control.
    fn slider_normalized(&self, ctrl: ToolbarControl) -> f32 {
        match ctrl {
            ToolbarControl::Mix => self.params.mix.unmodulated_normalized_value(),
            ToolbarControl::Output => self.params.output_gain.unmodulated_normalized_value(),
            _ => 0.0,
        }
    }

    /// Set a slider control to a normalized value via the host.
    fn set_slider(&self, ctrl: ToolbarControl, norm: f32) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match ctrl {
            ToolbarControl::Mix => {
                setter.begin_set_parameter(&self.params.mix);
                setter.set_parameter_normalized(&self.params.mix, norm);
                setter.end_set_parameter(&self.params.mix);
            }
            ToolbarControl::Output => {
                setter.begin_set_parameter(&self.params.output_gain);
                setter.set_parameter_normalized(&self.params.output_gain, norm);
                setter.end_set_parameter(&self.params.output_gain);
            }
            _ => {}
        }
    }
```

- [ ] **Step 3: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings — every `MultosisWindow` field is now consumed. No errors. Run `cargo nextest run -p multosis` — PASS, 94 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): wire the toolbar Mix and Output drag sliders"
```

---

### Task 8: Milestone 1b-ii-b-2 verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests green (94: the 91 from Milestone 1b-ii-b-1, plus the 3 `toolbar` geometry tests).

- [ ] **Step 2: Lint and format**

Run: `cargo clippy -p multosis -- -D warnings`
Expected: no warnings.

Run: `cargo fmt -p multosis -- --check`
Expected: clean (exit 0). If it reports a diff, run `cargo fmt -p multosis` and include the change in the final commit.

- [ ] **Step 3: Release build and bundle**

Run: `cargo build --bin multosis --release`
Expected: the standalone binary builds.

Run: `cargo nih-plug bundle multosis --release`
Expected: a VST3 + CLAP bundle is produced with no errors.

- [ ] **Step 4: Manual smoke test**

Run the standalone binary (`cargo run --bin multosis`). Confirm:
- The top strip is now a taller two-row toolbar; the upper row shows the Speed, Effect, Auto-Restart, Mix, Output, and Reset controls.
- Clicking Speed cycles the division (1/32 → 1/16 → … → 1/1 → 1/32); clicking Effect toggles Lowpass/Bitcrush; clicking Auto-Restart toggles its highlight.
- Dragging the Mix and Output sliders vertically changes the value, audibly.
- Clicking Reset restarts the sequence (the wavefront re-arms from the start cells).
- The grid still edits on click (toolbar clicks do not leak into the grid, and grid clicks do not hit the toolbar).

Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for milestone 1b-ii-b-2"
```

If Step 2 produced no edits, skip this commit.

---

## Milestone 1b-ii-b-2 — definition of done

- The editor has a working toolbar: Speed and Effect cycle on click, Auto-Restart toggles, Reset restarts the sequence, and Mix/Output are drag sliders — all driving the real parameters / engine.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles.
- The six grid operations (1b-ii-b-3) and the loop-region drag handles (1b-ii-b-4) remain.

## Spec coverage check (self-review)

- §7 toolbar — the upper toolbar row carries Speed, Effect bank, Auto-Restart, Mix, Output, and Reset (Tasks 3–4); clicks/drags drive them via `ParamSetter` (Tasks 6–7); Reset reaches the audio thread via the `reset_request` flag (Task 2), consumed in `process()`.
- §3.1 / §5.2 — the manual Reset triggers `engine.reset()` exactly as the transport stopped→playing edge does.
- Out of scope (later milestones): the six grid operations and the loop-region drag handles. The lower toolbar row is intentionally left empty this milestone; 1b-ii-b-3 fills it with the operation buttons.
