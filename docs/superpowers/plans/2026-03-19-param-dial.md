# ParamDial Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace horizontal parameter sliders with arc-style rotary dials, giving the plugin a more conventional audio-plugin look.

**Architecture:** A new `ParamDial` Vizia `View` with custom NanoVG drawing and mouse event handling. Uses `ParamWidgetBase` for nih-plug parameter binding (same pattern as the existing `ParamSlider`). The editor layout switches from vertical slider rows to a horizontal row of dial columns.

**Tech Stack:** Rust, nih-plug, nih_plug_vizia (Vizia UI framework), NanoVG canvas drawing

---

## File Structure

| File | Role |
|------|------|
| `src/editor/param_dial.rs` | **Create** — ParamDial widget: View impl with draw() and event() |
| `src/editor.rs` | **Modify** — Add `mod param_dial`, replace slider rows with dial row |
| `src/style.css` | **Modify** — Add `param-dial` element styling |

---

### Task 1: Create ParamDial with arc drawing (no interaction)

**Files:**
- Create: `src/editor/param_dial.rs`
- Modify: `src/editor.rs`

Build the visual-only dial first: struct, constructor, and `draw()` method. Wire it up for one parameter (Frequency) to see it on screen.

- [ ] **Step 1: Create the ParamDial struct and constructor**

Create `src/editor/param_dial.rs`:

```rust
use nih_plug::prelude::Param;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;

/// When shift+dragging, one pixel corresponds to this much normalized change.
const GRANULAR_DRAG_MULTIPLIER: f32 = 0.1;

/// Arc sweep in degrees (270° = gap at the bottom).
const ARC_DEGREES: f32 = 270.0;
/// Start angle in radians: 225° measured clockwise from 3-o'clock = 135° in math convention.
const START_ANGLE: f32 = std::f32::consts::PI * 0.75; // 135°
/// End angle in radians: -45° measured clockwise from 3-o'clock = 405° in math convention.
const END_ANGLE: f32 = std::f32::consts::PI * 0.75 + std::f32::consts::PI * 1.5; // 135° + 270° = 405°

#[derive(Lens)]
pub struct ParamDial {
    param_base: ParamWidgetBase,

    drag_active: bool,
    /// Y coordinate where the drag started.
    drag_start_y: f32,
    /// Normalized value when the drag started.
    drag_start_value: f32,
    /// Shift+drag state: if Some, contains the starting Y and value for granular dragging.
    granular_drag_status: Option<GranularDragStatus>,

    scrolled_lines: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct GranularDragStatus {
    pub starting_y_coordinate: f32,
    pub starting_value: f32,
}

impl ParamDial {
    pub fn new<L, Params, P, FMap>(
        cx: &mut Context,
        params: L,
        params_to_param: FMap,
    ) -> Handle<Self>
    where
        L: Lens<Target = Params> + Clone,
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + Copy + 'static,
    {
        Self {
            param_base: ParamWidgetBase::new(cx, params.clone(), params_to_param),

            drag_active: false,
            drag_start_y: 0.0,
            drag_start_value: 0.0,
            granular_drag_status: None,

            scrolled_lines: 0.0,
        }
        .build(
            cx,
            ParamWidgetBase::build_view(params, params_to_param, move |cx, param_data| {
                // Name label above the arc
                Label::new(cx, param_data.param().name())
                    .class("dial-label")
                    .hoverable(false);

                // Value text below the arc
                let value_lens = param_data.make_lens(|param| {
                    param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
                });
                Label::new(cx, value_lens)
                    .class("dial-value")
                    .hoverable(false);
            }),
        )
    }

    /// Map a normalized value [0, 1] to an angle in radians.
    fn value_to_angle(normalized: f32) -> f32 {
        START_ANGLE + normalized.clamp(0.0, 1.0) * (END_ANGLE - START_ANGLE)
    }
}
```

- [ ] **Step 2: Implement the View trait with draw()**

Add to the same file:

```rust
impl View for ParamDial {
    fn element(&self) -> Option<&'static str> {
        Some("param-dial")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();

        // The child Labels (name + value) take up space at the top and bottom.
        // The arc occupies the middle area. Estimate label heights for positioning.
        let label_height = 18.0;
        let arc_size = bounds.w.min(bounds.h - label_height * 2.0);
        let arc_cx = bounds.x + bounds.w / 2.0;
        let arc_cy = bounds.y + label_height + arc_size / 2.0;
        let radius = arc_size / 2.0 - 6.0; // Margin for indicator dot
        let stroke_width = 3.0;

        // --- Draw background arc (full 270° track) ---
        let mut bg_path = vg::Path::new();
        bg_path.arc(arc_cx, arc_cy, radius, START_ANGLE, END_ANGLE, vg::Solidity::Hole);
        let bg_paint = vg::Paint::color(vg::Color::rgb(64, 64, 64)).with_line_width(stroke_width);
        canvas.stroke_path(&bg_path, &bg_paint);

        // --- Draw value arc (from start to current value) ---
        let normalized = self.param_base.unmodulated_normalized_value();
        if normalized > 0.001 {
            let value_angle = Self::value_to_angle(normalized);
            let mut val_path = vg::Path::new();
            val_path.arc(arc_cx, arc_cy, radius, START_ANGLE, value_angle, vg::Solidity::Hole);
            let val_paint = vg::Paint::color(vg::Color::rgb(79, 195, 247)).with_line_width(stroke_width);
            canvas.stroke_path(&val_path, &val_paint);

            // --- Draw indicator dot at the value endpoint ---
            let dot_x = arc_cx + radius * value_angle.cos();
            let dot_y = arc_cy + radius * value_angle.sin();
            let mut dot_path = vg::Path::new();
            dot_path.circle(dot_x, dot_y, 4.0);
            canvas.fill_path(&dot_path, &vg::Paint::color(vg::Color::rgb(79, 195, 247)));
        }
    }
}
```

**Important NanoVG note:** `vg::Path::arc()` takes angles in radians. The direction (clockwise vs counter-clockwise) depends on the `Solidity` parameter. Test with `Solidity::Hole` first — if the arc draws the wrong way, switch to `Solidity::Solid`. The angles may also need adjustment depending on NanoVG's coordinate system (Y-down means angles go clockwise). The key thing is: background arc draws the full 270° track, value arc draws from `START_ANGLE` to `value_to_angle(normalized)`.

- [ ] **Step 3: Wire up one dial in the editor to test visuals**

In `src/editor.rs`, add the module declaration at the top:

```rust
mod param_dial;
```

Add the import:

```rust
use param_dial::ParamDial;
```

Replace the Frequency slider row (lines 177-185):

```rust
// Frequency control
HStack::new(cx, |cx| {
    Label::new(cx, "Frequency")
        .width(Pixels(100.0))
        .height(Pixels(30.0));
    ParamSlider::new(cx, Data::params, |params| &params.frequency);
})
.height(Pixels(40.0))
.col_between(Pixels(10.0));
```

With a test dial:

```rust
// Frequency dial (test)
ParamDial::new(cx, Data::params, |params| &params.frequency)
    .width(Pixels(80.0))
    .height(Pixels(110.0));
```

- [ ] **Step 4: Build and visually verify**

Run: `cargo build --bin wavetable-filter`

Launch the standalone binary and verify the dial appears. Check:
- Background arc visible (270° gray track with gap at bottom)
- Value arc fills to the correct position when the parameter has a non-zero value
- Indicator dot at the endpoint of the value arc
- Label text not required to be perfect yet — just needs to not crash

**Expected:** The dial draws but doesn't respond to mouse input yet.

The arc direction/angles may need tuning — NanoVG uses a Y-down coordinate system where positive angles go clockwise. Adjust `START_ANGLE`, `END_ANGLE`, and `Solidity` as needed until the arc sweeps from bottom-left to bottom-right (clockwise, gap at bottom).

- [ ] **Step 5: Commit**

```bash
git add src/editor/param_dial.rs src/editor.rs
git commit -m "add ParamDial widget with arc drawing, wire up for frequency"
```

---

### Task 2: Add mouse interaction (vertical drag, shift fine-tune, double-click reset, scroll)

**Files:**
- Modify: `src/editor/param_dial.rs`

Add the `event()` method to handle all mouse interactions.

- [ ] **Step 1: Add the event handler**

Add this `event()` method inside the existing `impl View for ParamDial` block:

```rust
fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
    event.map(|window_event, meta| match window_event {
        WindowEvent::MouseDown(MouseButton::Left) => {
            if cx.modifiers().command() {
                // Ctrl/Cmd+Click: reset to default
                self.param_base.begin_set_parameter(cx);
                self.param_base
                    .set_normalized_value(cx, self.param_base.default_normalized_value());
                self.param_base.end_set_parameter(cx);
            } else {
                self.drag_active = true;
                cx.capture();
                cx.focus();
                cx.set_active(true);

                self.param_base.begin_set_parameter(cx);
                self.drag_start_y = cx.mouse().cursory;
                self.drag_start_value = self.param_base.unmodulated_normalized_value();

                if cx.modifiers().shift() {
                    self.granular_drag_status = Some(GranularDragStatus {
                        starting_y_coordinate: cx.mouse().cursory,
                        starting_value: self.drag_start_value,
                    });
                } else {
                    self.granular_drag_status = None;
                }
            }
            meta.consume();
        }
        WindowEvent::MouseDoubleClick(MouseButton::Left)
        | WindowEvent::MouseDown(MouseButton::Right)
        | WindowEvent::MouseDoubleClick(MouseButton::Right) => {
            // Double-click and right-click: reset to default
            self.param_base.begin_set_parameter(cx);
            self.param_base
                .set_normalized_value(cx, self.param_base.default_normalized_value());
            self.param_base.end_set_parameter(cx);
            meta.consume();
        }
        WindowEvent::MouseUp(MouseButton::Left) => {
            if self.drag_active {
                self.drag_active = false;
                cx.release();
                cx.set_active(false);
                self.param_base.end_set_parameter(cx);
                meta.consume();
            }
        }
        WindowEvent::MouseMove(_x, y) => {
            if self.drag_active {
                // Vertical drag: up = increase, down = decrease
                // 200px of travel = full 0→1 range (scaled by DPI)
                let pixels_per_full_range = 200.0 / cx.scale_factor();

                if cx.modifiers().shift() {
                    let status = *self
                        .granular_drag_status
                        .get_or_insert(GranularDragStatus {
                            starting_y_coordinate: *y,
                            starting_value: self.param_base.unmodulated_normalized_value(),
                        });
                    let delta_y = status.starting_y_coordinate - *y;
                    let delta_value = (delta_y / pixels_per_full_range) * GRANULAR_DRAG_MULTIPLIER;
                    let new_value = (status.starting_value + delta_value).clamp(0.0, 1.0);
                    self.param_base.set_normalized_value(cx, new_value);
                } else {
                    self.granular_drag_status = None;
                    let delta_y = self.drag_start_y - *y;
                    let delta_value = delta_y / pixels_per_full_range;
                    let new_value = (self.drag_start_value + delta_value).clamp(0.0, 1.0);
                    self.param_base.set_normalized_value(cx, new_value);
                }
            }
        }
        WindowEvent::KeyUp(_, Some(Key::Shift)) => {
            if self.drag_active && self.granular_drag_status.is_some() {
                // Snap out of granular drag: update start to current position/value
                self.granular_drag_status = None;
                self.drag_start_y = cx.mouse().cursory;
                self.drag_start_value = self.param_base.unmodulated_normalized_value();
            }
        }
        WindowEvent::MouseScroll(_scroll_x, scroll_y) => {
            self.scrolled_lines += scroll_y;
            if self.scrolled_lines.abs() >= 1.0 {
                let use_finer_steps = cx.modifiers().shift();

                if !self.drag_active {
                    self.param_base.begin_set_parameter(cx);
                }

                let mut current_value = self.param_base.unmodulated_normalized_value();
                while self.scrolled_lines >= 1.0 {
                    current_value = self
                        .param_base
                        .next_normalized_step(current_value, use_finer_steps);
                    self.param_base.set_normalized_value(cx, current_value);
                    self.scrolled_lines -= 1.0;
                }
                while self.scrolled_lines <= -1.0 {
                    current_value = self
                        .param_base
                        .previous_normalized_step(current_value, use_finer_steps);
                    self.param_base.set_normalized_value(cx, current_value);
                    self.scrolled_lines += 1.0;
                }

                if !self.drag_active {
                    self.param_base.end_set_parameter(cx);
                }
            }
            meta.consume();
        }
        _ => {}
    });
}
```

- [ ] **Step 2: Build and test interaction**

Run: `cargo build --bin wavetable-filter`

Launch and test:
- Drag up on the dial → value increases, arc fills clockwise
- Drag down → value decreases
- Shift+drag → fine-tune (much slower movement)
- Double-click → resets to default
- Scroll wheel → steps value up/down
- The value text below the dial updates in real-time

If the drag direction feels inverted, flip the delta sign (`*y - self.drag_start_y` instead of `self.drag_start_y - *y`).

- [ ] **Step 3: Commit**

```bash
git add src/editor/param_dial.rs
git commit -m "add vertical drag, shift fine-tune, reset, and scroll to ParamDial"
```

---

### Task 3: Replace all sliders with dials and update layout

**Files:**
- Modify: `src/editor.rs`
- Modify: `src/style.css`

- [ ] **Step 1: Replace all five continuous param sliders with dials**

Replace the five slider HStack rows (Frequency, Frame Position, Resonance, Mix, Drive — lines 177-235) with a single dial row. Keep Mode as a ParamSlider.

The new controls section in `src/editor.rs` (inside the outer VStack, after the wavetable path row):

```rust
// Parameter dials row
HStack::new(cx, |cx| {
    ParamDial::new(cx, Data::params, |params| &params.frequency)
        .width(Pixels(80.0))
        .height(Pixels(110.0));
    ParamDial::new(cx, Data::params, |params| &params.frame_position)
        .width(Pixels(80.0))
        .height(Pixels(110.0));
    ParamDial::new(cx, Data::params, |params| &params.resonance)
        .width(Pixels(80.0))
        .height(Pixels(110.0));
    ParamDial::new(cx, Data::params, |params| &params.mix)
        .width(Pixels(80.0))
        .height(Pixels(110.0));
    ParamDial::new(cx, Data::params, |params| &params.drive)
        .width(Pixels(80.0))
        .height(Pixels(110.0));
})
.col_between(Pixels(20.0))
.height(Pixels(120.0))
.child_left(Stretch(1.0))
.child_right(Stretch(1.0));

// Mode selection row (stays as slider — discrete enum)
HStack::new(cx, |cx| {
    Label::new(cx, "Mode")
        .width(Pixels(100.0))
        .height(Pixels(30.0));
    ParamSlider::new(cx, Data::params, |params| &params.mode);
})
.height(Pixels(40.0))
.col_between(Pixels(10.0));
```

Remove the old individual slider rows for Frequency, Frame Position, Resonance, Mix, and Drive.

- [ ] **Step 2: Add CSS for the dial**

Add to `src/style.css`:

```css
/* Dial styling */
param-dial {
    background-color: transparent;
}

param-dial .dial-label {
    color: #a0a0a0;
    font-size: 11px;
    child-left: 1s;
    child-right: 1s;
    height: 16px;
}

param-dial .dial-value {
    color: #ffffff;
    font-size: 11px;
    child-left: 1s;
    child-right: 1s;
    height: 16px;
}
```

- [ ] **Step 3: Build and visually verify the full layout**

Run: `cargo build --bin wavetable-filter`

Launch and verify:
- Five dials in a horizontal row, evenly spaced and centered
- Each dial shows its label (Frequency, Frame Position, Resonance, Mix, Drive)
- Value text below each dial updates when dragging
- Mode slider still below the dials row
- Visualization area (wavetable + filter response) still below mode
- Overall window isn't too cramped — if it is, adjust `WINDOW_HEIGHT` in `editor.rs`

- [ ] **Step 4: Commit**

```bash
git add src/editor.rs src/editor/param_dial.rs src/style.css
git commit -m "replace parameter sliders with dial row for continuous params"
```

---

### Task 4: Polish and fix issues

**Files:**
- Modify: `src/editor/param_dial.rs`
- Modify: `src/editor.rs`
- Modify: `src/style.css`

This task covers any visual/interaction polish discovered during Tasks 1-3. Common issues to check and fix:

- [ ] **Step 1: Run clippy and fix warnings**

Run: `cargo clippy`

Fix any warnings in the new code.

- [ ] **Step 2: Visual polish pass**

Launch the standalone and check:
- Arc angles look correct (gap at bottom, sweeps clockwise from bottom-left)
- Value text aligns centered below each dial
- Label aligns centered above each dial
- Colors look good against the dark background
- Window dimensions work (adjust `WINDOW_WIDTH`/`WINDOW_HEIGHT` if needed)

Adjust drawing constants (radius, stroke width, colors, font sizes, spacing) as needed.

- [ ] **Step 3: Interaction polish pass**

Test each interaction:
- Vertical drag sensitivity feels right (~200px for full range)
- Shift+drag gives noticeably finer control
- Double-click resets to default value (not zero — the default, e.g. 1 kHz for frequency)
- Scroll wheel steps are reasonable
- Ctrl/Cmd+click also resets

- [ ] **Step 4: Build release bundle**

Run: `cargo nih-plug bundle wavetable-filter --release`

Verify VST3 and CLAP bundles build successfully.

- [ ] **Step 5: Commit**

```bash
git add src/editor/param_dial.rs src/editor.rs src/style.css
git commit -m "polish ParamDial visuals and interaction"
```
