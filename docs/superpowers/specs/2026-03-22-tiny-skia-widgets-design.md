# tiny-skia-widgets Design Spec

## Overview

A shared workspace crate extracting CPU-rendering widgets from gs-meter's `widgets.rs` and adding a new `ParamDial` (arc-based rotary knob). Used by gs-meter, gain-brain, and future tiny-skia plugins.

## Crate Structure

```
tiny-skia-widgets/
├── Cargo.toml
├── src/
│   ├── lib.rs           — re-exports, color palette, common types
│   ├── primitives.rs    — draw_rect, draw_rect_outline, color helpers
│   ├── text.rs          — TextRenderer, GlyphKey, GlyphEntry, draw_text, text_width
│   ├── controls.rs      — draw_button, draw_slider, draw_stepped_selector
│   └── param_dial.rs    — draw_dial (arc-based rotary knob)
```

## Dependencies

- `tiny-skia = "0.11"` (drawing)
- `fontdue = "0.9"` (text rendering / glyph rasterization)

No nih-plug dependency. This is a pure rendering library. Plugins handle parameter interaction, hit testing, and drag logic.

## Extracted API (from gs-meter widgets.rs)

These move verbatim from `gs-meter/src/widgets.rs`:

### primitives.rs

```rust
pub fn color_bg() -> tiny_skia::Color;
pub fn color_text() -> tiny_skia::Color;
pub fn color_muted() -> tiny_skia::Color;
pub fn color_accent() -> tiny_skia::Color;
pub fn color_control_bg() -> tiny_skia::Color;
pub fn color_control_fill() -> tiny_skia::Color;
pub fn color_border() -> tiny_skia::Color;

pub fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color);
pub fn draw_rect_outline(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color, width: f32);
```

### text.rs

```rust
pub struct TextRenderer { /* glyph cache, font */ }

impl TextRenderer {
    pub fn new(font_data: &[u8]) -> Self;
    pub fn text_width(&mut self, text: &str, size: f32) -> f32;
    pub fn draw_text(&mut self, pixmap: &mut Pixmap, x: f32, y: f32, text: &str, size: f32, color: Color);
}
```

### controls.rs

```rust
pub fn draw_button(pixmap: &mut Pixmap, tr: &mut TextRenderer, x: f32, y: f32, w: f32, h: f32, label: &str, active: bool, hovered: bool);
pub fn draw_slider(pixmap: &mut Pixmap, tr: &mut TextRenderer, x: f32, y: f32, w: f32, h: f32, label: &str, value_text: &str, normalized: f32);
pub fn draw_stepped_selector(pixmap: &mut Pixmap, tr: &mut TextRenderer, x: f32, y: f32, w: f32, h: f32, labels: &[&str], selected: i32);
```

## New: ParamDial (param_dial.rs)

### API

```rust
pub fn draw_dial(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cx: f32, cy: f32,        // center of the dial
    radius: f32,             // outer radius of the arc
    label: &str,             // name label above the arc
    value_text: &str,        // formatted value below the arc
    normalized: f32,         // 0.0-1.0, drives arc fill amount
);
```

### Visual Design

Matches the vizia ParamDial from `nih-plug-widgets/src/param_dial.rs`:

- **Arc sweep:** 270 degrees (from 135 to 405 degrees in math convention, same as vizia version)
- **Background track:** full 270-degree arc in dark gray (64, 64, 64)
- **Value arc:** from start angle to current value angle, in accent color (79, 195, 247)
- **Indicator dot:** small filled circle at the current value position on the arc
- **Label:** centered above the arc, muted color
- **Value text:** centered below the arc, text color
- **Stroke width:** 3.0 pixels (scales with the radius proportionally)
- **Line caps:** round

### Arc Drawing

tiny-skia doesn't have native arc support like vizia's nanovg. Arcs are approximated using cubic Bezier curves via `tiny_skia::PathBuilder`. A helper function converts (center, radius, start_angle, end_angle) to a series of `cubic_to` calls, using the standard circular arc approximation (split into 90-degree segments, each approximated by a cubic Bezier with control point distance `radius * 4/3 * tan(sweep/4)`).

### No Interaction

`draw_dial` is a pure drawing function. The caller is responsible for:
- Hit testing (check if mouse is within the dial's bounding box)
- Drag handling (vertical drag to change value, shift for fine control)
- Parameter setting (via nih-plug's ParamSetter)

This matches the pattern of `draw_slider` and `draw_button` — the widget crate draws, the editor handles events.

## Migration

### gs-meter

- Delete `gs-meter/src/widgets.rs`
- Add `tiny-skia-widgets = { path = "../tiny-skia-widgets" }` to `gs-meter/Cargo.toml`
- Replace `use crate::widgets;` / `mod widgets;` with `use tiny_skia_widgets as widgets;` (or adjust call sites to use `tiny_skia_widgets::` directly)
- Keep `include_bytes!("fonts/DejaVuSans.ttf")` in the editor — font embedding stays per-plugin

### gain-brain

- Same as gs-meter: delete local `widgets.rs`, depend on `tiny-skia-widgets`

### Workspace

- Add `"tiny-skia-widgets"` to workspace members in root `Cargo.toml`

## Testing

### Unit Tests (param_dial.rs)

- `test_draw_dial_does_not_panic` — draw with various normalized values (0.0, 0.5, 1.0)
- `test_draw_dial_extreme_values` — normalized values outside 0-1 are clamped
- `test_arc_approximation` — verify the Bezier arc helper produces correct start/end points

### Existing Tests

The existing widget tests from gs-meter's `widgets.rs` move to `tiny-skia-widgets` and continue to pass.

## Non-Goals

- No modulation indicator (future enhancement)
- No interaction handling (pure rendering)
- No nih-plug dependency
- No font embedding (plugins provide font data)
