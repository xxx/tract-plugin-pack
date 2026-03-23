# tiny-skia-widgets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract shared CPU-rendering widgets from gs-meter into a new `tiny-skia-widgets` crate, add a ParamDial (arc-based rotary knob), and migrate gs-meter + gain-brain to use it.

**Architecture:** New library crate `tiny-skia-widgets/` with no nih-plug dependency. Split the existing monolithic `widgets.rs` into focused modules (primitives, text, controls, param_dial). Migrate consumers to depend on the shared crate instead of their local copies.

**Tech Stack:** Rust, tiny-skia, fontdue

**Spec:** `docs/superpowers/specs/2026-03-22-tiny-skia-widgets-design.md`

---

## File Structure

```
tiny-skia-widgets/
├── Cargo.toml
├── src/
│   ├── lib.rs           — re-exports everything, consumers use `tiny_skia_widgets::*`
│   ├── primitives.rs    — color helpers, draw_rect, draw_rect_outline
│   ├── text.rs          — TextRenderer, GlyphKey, GlyphEntry
│   ├── controls.rs      — draw_button, draw_slider, draw_stepped_selector
│   └── param_dial.rs    — draw_dial (new)
```

Also modified:
- `Cargo.toml` (workspace root) — add member
- `gs-meter/Cargo.toml` — add dependency, remove fontdue
- `gs-meter/src/lib.rs` — remove `pub mod widgets;`
- `gs-meter/src/editor.rs` — change `crate::widgets` to `tiny_skia_widgets`
- `gs-meter/src/widgets.rs` — DELETE
- `gain-brain/Cargo.toml` — add dependency, remove fontdue
- `gain-brain/src/lib.rs` — remove `pub mod widgets;`
- `gain-brain/src/editor.rs` — change `crate::widgets` to `tiny_skia_widgets`
- `gain-brain/src/widgets.rs` — DELETE

---

### Task 1: Create the crate and extract primitives + text + controls

**Files:**
- Create: `tiny-skia-widgets/Cargo.toml`
- Create: `tiny-skia-widgets/src/lib.rs`
- Create: `tiny-skia-widgets/src/primitives.rs`
- Create: `tiny-skia-widgets/src/text.rs`
- Create: `tiny-skia-widgets/src/controls.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create `tiny-skia-widgets/Cargo.toml`**

```toml
[package]
name = "tiny-skia-widgets"
version = "0.1.0"
edition = "2021"

[dependencies]
tiny-skia = "0.11"
fontdue = "0.9"
```

- [ ] **Step 2: Split gs-meter/src/widgets.rs into the three modules**

Read `gs-meter/src/widgets.rs` in full. Copy the contents into the new files:

**`primitives.rs`:** Lines 1-28 (imports, color functions) + lines 172-218 (draw_rect, draw_rect_outline). Add `use tiny_skia::*;` at the top.

**`text.rs`:** Lines 30-170 (TextRenderer struct, GlyphKey, GlyphEntry, impl TextRenderer with new/text_width/draw_text). Add `use tiny_skia::*;` and `use std::collections::HashMap;` at the top.

**`controls.rs`:** Lines 220-346 (draw_button, draw_slider, draw_stepped_selector). Add `use tiny_skia::*;` and `use crate::text::TextRenderer;` and `use crate::primitives::*;` at the top.

- [ ] **Step 3: Create `lib.rs` that re-exports everything**

```rust
mod primitives;
mod text;
mod controls;

pub use primitives::*;
pub use text::*;
pub use controls::*;
```

- [ ] **Step 4: Add to workspace**

In root `Cargo.toml`, add `"tiny-skia-widgets"` to members.

- [ ] **Step 5: Verify it compiles**

Run: `cargo check --package tiny-skia-widgets`

- [ ] **Step 6: Move the tests from gs-meter's widgets.rs**

The existing tests (lines 348-643 in gs-meter's widgets.rs) should move to the appropriate module. Most test `TextRenderer` and drawing functions. Place them in `text.rs` and `controls.rs` as `#[cfg(test)] mod tests { ... }` blocks. The tests that create a `Pixmap` and call `draw_button`/`draw_slider`/`draw_stepped_selector` go in `controls.rs`. Tests for `TextRenderer` and `text_width` go in `text.rs`.

- [ ] **Step 7: Run tests**

Run: `cargo test --package tiny-skia-widgets`
Expected: all existing widget tests pass

- [ ] **Step 8: Commit**

```
feat: create tiny-skia-widgets crate with extracted primitives, text, and controls
```

---

### Task 2: Add ParamDial (draw_dial)

**Files:**
- Create: `tiny-skia-widgets/src/param_dial.rs`
- Modify: `tiny-skia-widgets/src/lib.rs`

- [ ] **Step 1: Write tests first**

Add to `param_dial.rs`:

```rust
//! Arc-based rotary dial widget for parameter display.

use tiny_skia::*;
use crate::text::TextRenderer;
use crate::primitives::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_renderer() -> TextRenderer {
        // Use a minimal font for testing — DejaVuSans is not available here,
        // but we can test with any TTF. Use the fontdue built-in or skip text.
        // For now, create with an empty/minimal font to test arc drawing.
        // Actually, we need a real font for TextRenderer::new.
        // Include a test font or use the fact that draw_dial won't panic
        // even if text rendering produces empty output.
        TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"))
    }

    #[test]
    fn test_draw_dial_zero() {
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let mut tr = test_renderer();
        draw_dial(&mut pixmap, &mut tr, 50.0, 50.0, 30.0, "Gain", "0.0 dB", 0.0);
        // Should not panic; some pixels should be drawn
    }

    #[test]
    fn test_draw_dial_half() {
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let mut tr = test_renderer();
        draw_dial(&mut pixmap, &mut tr, 50.0, 50.0, 30.0, "Gain", "0.0 dB", 0.5);
    }

    #[test]
    fn test_draw_dial_full() {
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let mut tr = test_renderer();
        draw_dial(&mut pixmap, &mut tr, 50.0, 50.0, 30.0, "Gain", "+60 dB", 1.0);
    }

    #[test]
    fn test_draw_dial_clamps_out_of_range() {
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let mut tr = test_renderer();
        // Values outside 0-1 should be clamped, not panic
        draw_dial(&mut pixmap, &mut tr, 50.0, 50.0, 30.0, "Gain", "?", -0.5);
        draw_dial(&mut pixmap, &mut tr, 50.0, 50.0, 30.0, "Gain", "?", 1.5);
    }

    #[test]
    fn test_arc_points_start() {
        let (sx, sy) = arc_point(50.0, 50.0, 30.0, START_ANGLE);
        // At 135 degrees (upper-left quadrant)
        assert!((sx - 50.0).abs() < 30.0);
        assert!((sy - 50.0).abs() < 30.0);
    }

    #[test]
    fn test_arc_points_end() {
        let (ex, ey) = arc_point(50.0, 50.0, 30.0, END_ANGLE);
        // At 405 degrees = 45 degrees (upper-right quadrant)
        assert!((ex - 50.0).abs() < 30.0);
        assert!((ey - 50.0).abs() < 30.0);
    }
}
```

- [ ] **Step 2: Copy a test font file**

Copy `gs-meter/src/fonts/DejaVuSans.ttf` to `tiny-skia-widgets/test_data/DejaVuSans.ttf` for test use only.

Run: `mkdir -p tiny-skia-widgets/test_data && cp gs-meter/src/fonts/DejaVuSans.ttf tiny-skia-widgets/test_data/`

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --package tiny-skia-widgets`
Expected: FAIL — `draw_dial`, `arc_point`, `START_ANGLE`, `END_ANGLE` not found

- [ ] **Step 4: Implement draw_dial**

```rust
use std::f32::consts::PI;

/// Start angle: 225 degrees clockwise from 3-o'clock = 135 degrees in math convention.
const START_ANGLE: f32 = PI * 0.75;
/// End angle: 135 + 270 = 405 degrees in math convention.
const END_ANGLE: f32 = PI * 0.75 + PI * 1.5;

/// Arc stroke colors
const ARC_BG: (u8, u8, u8) = (64, 64, 64);
const ARC_FG: (u8, u8, u8) = (79, 195, 247); // accent blue

/// Compute a point on a circle at a given angle.
fn arc_point(cx: f32, cy: f32, radius: f32, angle: f32) -> (f32, f32) {
    (cx + radius * angle.cos(), cy + radius * angle.sin())
}

/// Map normalized [0,1] to an angle on the 270-degree arc.
fn value_to_angle(normalized: f32) -> f32 {
    START_ANGLE + normalized.clamp(0.0, 1.0) * (END_ANGLE - START_ANGLE)
}

/// Build a stroked arc path using cubic Bezier approximation.
/// Splits the arc into segments of at most 90 degrees.
fn build_arc_path(cx: f32, cy: f32, radius: f32, start: f32, end: f32) -> Path {
    let mut pb = PathBuilder::new();
    let sweep = end - start;
    let num_segments = (sweep.abs() / (PI * 0.5)).ceil() as usize;
    let segment_sweep = sweep / num_segments as f32;

    let (sx, sy) = arc_point(cx, cy, radius, start);
    pb.move_to(sx, sy);

    for i in 0..num_segments {
        let a1 = start + i as f32 * segment_sweep;
        let a2 = a1 + segment_sweep;
        let alpha = 4.0 / 3.0 * (segment_sweep / 4.0).tan();

        let (x1, y1) = arc_point(cx, cy, radius, a1);
        let (x2, y2) = arc_point(cx, cy, radius, a2);

        // Control points
        let cp1x = x1 - alpha * radius * a1.sin();
        let cp1y = y1 + alpha * radius * a1.cos();
        let cp2x = x2 + alpha * radius * a2.sin();
        let cp2y = y2 - alpha * radius * a2.cos();

        pb.cubic_to(cp1x, cp1y, cp2x, cp2y, x2, y2);
    }

    pb.finish().unwrap()
}

/// Draw an arc-based rotary dial.
///
/// * `cx`, `cy` — center of the dial
/// * `radius` — outer radius of the arc
/// * `label` — name shown above the arc
/// * `value_text` — formatted value shown below the arc
/// * `normalized` — 0.0 to 1.0, controls the arc fill amount
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
    let stroke_width = (radius * 0.1).max(2.0);
    let normalized = normalized.clamp(0.0, 1.0);

    // ── Background arc (full 270-degree track) ──
    let bg_path = build_arc_path(cx, cy, radius, START_ANGLE, END_ANGLE);
    let mut bg_paint = Paint::default();
    bg_paint.set_color_rgba8(ARC_BG.0, ARC_BG.1, ARC_BG.2, 255);
    bg_paint.anti_alias = true;
    let bg_stroke = Stroke {
        width: stroke_width,
        line_cap: LineCap::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&bg_path, &bg_paint, &bg_stroke, Transform::identity(), None);

    // ── Value arc (from start to current value) ──
    if normalized > 0.001 {
        let value_angle = value_to_angle(normalized);
        let val_path = build_arc_path(cx, cy, radius, START_ANGLE, value_angle);
        let mut val_paint = Paint::default();
        val_paint.set_color_rgba8(ARC_FG.0, ARC_FG.1, ARC_FG.2, 255);
        val_paint.anti_alias = true;
        let val_stroke = Stroke {
            width: stroke_width,
            line_cap: LineCap::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&val_path, &val_paint, &val_stroke, Transform::identity(), None);

        // ── Indicator dot ──
        let (dot_x, dot_y) = arc_point(cx, cy, radius, value_angle);
        let dot_radius = stroke_width * 1.5;
        let dot_rect = Rect::from_xywh(
            dot_x - dot_radius,
            dot_y - dot_radius,
            dot_radius * 2.0,
            dot_radius * 2.0,
        );
        if let Some(rect) = dot_rect {
            let mut dot_pb = PathBuilder::new();
            dot_pb.push_oval(rect);
            if let Some(dot_path) = dot_pb.finish() {
                pixmap.fill_path(&dot_path, &val_paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    }

    // ── Label (above arc) ──
    let label_size = (radius * 0.35).max(10.0);
    let label_w = text_renderer.text_width(label, label_size);
    let label_x = cx - label_w / 2.0;
    let label_y = cy - radius - stroke_width - 2.0;
    text_renderer.draw_text(pixmap, label_x, label_y, label, label_size, color_muted());

    // ── Value text (below arc) ──
    let val_size = (radius * 0.35).max(10.0);
    let val_w = text_renderer.text_width(value_text, val_size);
    let val_x = cx - val_w / 2.0;
    let val_y = cy + radius + stroke_width + val_size + 2.0;
    text_renderer.draw_text(pixmap, val_x, val_y, value_text, val_size, color_text());
}
```

- [ ] **Step 5: Add `pub mod param_dial;` and re-export in lib.rs**

```rust
mod primitives;
mod text;
mod controls;
mod param_dial;

pub use primitives::*;
pub use text::*;
pub use controls::*;
pub use param_dial::*;
```

- [ ] **Step 6: Run tests**

Run: `cargo test --package tiny-skia-widgets`
Expected: all tests pass

- [ ] **Step 7: Commit**

```
feat: add draw_dial rotary knob widget with arc rendering
```

---

### Task 3: Migrate gs-meter to use tiny-skia-widgets

**Files:**
- Modify: `gs-meter/Cargo.toml`
- Modify: `gs-meter/src/lib.rs`
- Modify: `gs-meter/src/editor.rs`
- Delete: `gs-meter/src/widgets.rs`

- [ ] **Step 1: Add dependency to gs-meter/Cargo.toml**

Add:
```toml
tiny-skia-widgets = { path = "../tiny-skia-widgets" }
```

Remove `fontdue = "0.9"` from dependencies (now provided transitively by tiny-skia-widgets).

- [ ] **Step 2: Update gs-meter/src/lib.rs**

Remove the line `pub mod widgets;`

- [ ] **Step 3: Update gs-meter/src/editor.rs**

Replace all `use crate::widgets;` or `use crate::widgets::*;` with `use tiny_skia_widgets as widgets;` (or use `tiny_skia_widgets` directly). The goal is that `widgets::draw_button(...)`, `widgets::TextRenderer::new(...)`, etc. continue to work with minimal changes.

Also check for any `crate::widgets::` references and update them.

- [ ] **Step 4: Delete gs-meter/src/widgets.rs**

- [ ] **Step 5: Verify gs-meter compiles and tests pass**

Run: `cargo test --package gs-meter`
Expected: all 75 tests pass (the widget tests now live in tiny-skia-widgets)

Run: `cargo clippy --package gs-meter -- -D warnings`

- [ ] **Step 6: Commit**

```
refactor: migrate gs-meter to shared tiny-skia-widgets crate
```

---

### Task 4: Migrate gain-brain to use tiny-skia-widgets

**Files:**
- Modify: `gain-brain/Cargo.toml`
- Modify: `gain-brain/src/lib.rs`
- Modify: `gain-brain/src/editor.rs`
- Delete: `gain-brain/src/widgets.rs`

- [ ] **Step 1: Same migration as gs-meter**

Follow the exact same steps as Task 3 but for gain-brain:
- Add `tiny-skia-widgets = { path = "../tiny-skia-widgets" }` to Cargo.toml
- Remove `fontdue = "0.9"` from Cargo.toml
- Remove `pub mod widgets;` from lib.rs
- Update editor.rs imports
- Delete `gain-brain/src/widgets.rs`

- [ ] **Step 2: Verify gain-brain compiles and tests pass**

Run: `cargo test --package gain-brain`
Expected: all 40 tests pass

Run: `cargo clippy --package gain-brain -- -D warnings`

- [ ] **Step 3: Commit**

```
refactor: migrate gain-brain to shared tiny-skia-widgets crate
```

---

### Task 5: Final verification

- [ ] **Step 1: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all tests pass across all crates

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean

- [ ] **Step 3: Build release bundles**

Run: `cargo nih-plug bundle gs-meter --release && cargo nih-plug bundle gain-brain --release`
Expected: both bundles created successfully

- [ ] **Step 4: Commit any remaining fixes**
