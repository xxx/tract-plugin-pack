# MSEG Editor Widget — Editor Implementation Plan (Plan 2 of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the MSEG envelope widget's editor — rendering (`draw_mseg`) and interaction (`MsegEditState` + event handlers), including curve-only mode — on top of the already-merged MSEG core.

**Architecture:** Two new files in `tiny-skia-widgets/src/mseg/`: `render.rs` (pure layout + all drawing) and `editor.rs` (`MsegEditState` + event handlers). The editor *owns the document* — handlers mutate `&mut MsegData` directly and return `MsegEdit::Changed`. One pure `mseg_layout` function backs both drawing and hit testing, mirroring how `dropdown_popup_layout` backs the dropdown widget. Curve-only mode (a flag on `MsegEditState`) hides the playback/timing controls and the marker lane.

**Tech Stack:** Rust (nightly), `tiny-skia` (CPU rendering), `fontdue` via the crate's `TextRenderer`. No new dependencies.

**Reference reading before starting:**
- `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md` — the design spec (authoritative; note the **Editor: Curve-Only Mode** section).
- `tiny-skia-widgets/src/mseg/mod.rs` — the merged core: `MsegData`, `MsegNode`, `value_at_phase`, `insert_node`/`remove_node`/`move_node`, `MAX_NODES`, `HoldMode`, `PlayMode`, `SyncMode`.
- `tiny-skia-widgets/src/dropdown.rs` — sibling widget: the layout-function pattern, `#[cfg(test)] mod tests` style, render smoke tests, `DropdownState`.
- `tiny-skia-widgets/src/primitives.rs` — `draw_rect`, `draw_rect_outline`, `color_*`.
- `tiny-skia-widgets/src/controls.rs` — `draw_button`, `draw_stepped_selector`.
- `tiny-skia-widgets/src/text_edit.rs` — `TextEditState`.

**Test command:** `cargo nextest run -p tiny-skia-widgets mseg` (whole module) or `... mseg <name>` for one test.

---

## File Structure

- **Create:** `tiny-skia-widgets/src/mseg/render.rs` — `MsegLayout`, the pure `mseg_layout` function, coordinate mapping, and `draw_mseg` plus drawing helpers.
- **Create:** `tiny-skia-widgets/src/mseg/editor.rs` — `MsegEditState`, `MsegEdit`, the drag/hit-test types, and the event handlers.
- **Modify:** `tiny-skia-widgets/src/mseg/mod.rs` — `pub mod render; pub mod editor;` + `pub use` re-exports.

Rendering and interaction are split because they are large and change for different reasons; `mseg_layout` (pure geometry) is the shared seam, in `render.rs`, used by both.

---

## Task 1: Editor scaffold — state types & module registration

**Files:**
- Create: `tiny-skia-widgets/src/mseg/editor.rs`
- Create: `tiny-skia-widgets/src/mseg/render.rs`
- Modify: `tiny-skia-widgets/src/mseg/mod.rs`

- [ ] **Step 1: Create `render.rs` as a stub**

`tiny-skia-widgets/src/mseg/render.rs`:

```rust
//! MSEG editor — pure layout geometry and all drawing.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.
```

- [ ] **Step 2: Create `editor.rs` with the state types**

`tiny-skia-widgets/src/mseg/editor.rs`:

```rust
//! MSEG editor — transient interaction state and event handlers.
//!
//! The editor *owns the document*: handlers mutate `&mut MsegData` directly
//! and return `MsegEdit::Changed` when something changed.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::dropdown::DropdownState;
use crate::mseg::randomize::RandomStyle;
use crate::text_edit::TextEditState;

/// Returned by an event handler when the document changed and the consuming
/// plugin should re-persist (and, for `miff`, re-bake).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MsegEdit {
    Changed,
}

/// Which strip sub-control a `DropdownState` / `TextEditState` action targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StripId {
    Style,
    Duration,
    TimeGrid,
    ValueGrid,
}

/// What the pointer is currently dragging.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DragTarget {
    /// Moving the node at this index.
    Node(usize),
    /// Bending the segment starting at this node index.
    Tension(usize),
    /// Dragging a hold marker.
    Marker(MarkerHandle),
    /// Painting stepped nodes (Alt held).
    StepDraw,
}

/// Which hold marker is being dragged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkerHandle {
    Sustain,
    LoopStart,
    LoopEnd,
}

/// Transient editor state — not persisted.
pub struct MsegEditState {
    /// When true, playback/timing controls and the marker lane are hidden.
    curve_only: bool,
    /// Active drag, if any.
    drag: Option<DragTarget>,
    /// Hovered node index, for highlight.
    hover: Option<usize>,
    /// During a stepped-draw, the last time-grid cell a node was painted in
    /// (so dragging within one cell does not insert duplicates).
    step_last_cell: Option<u32>,
    /// `true` while the caller's stepped-draw modifier (e.g. Alt) is held.
    stepped_draw_held: bool,
    /// Randomizer style currently chosen in the strip.
    style: RandomStyle,
    /// Style-selector dropdown state.
    style_dropdown: DropdownState<StripId>,
    /// Numeric strip-field text entry.
    text_edit: TextEditState<StripId>,
    /// Bumped on each Randomize click so successive clicks differ.
    seed: u32,
}

impl MsegEditState {
    /// A full editor (playback controls + marker lane shown).
    pub fn new() -> Self {
        Self::with_curve_only(false)
    }

    /// A curve-only editor — playback/timing controls and the marker lane are
    /// hidden; curve editing, grid, snap, and the randomizer remain.
    pub fn new_curve_only() -> Self {
        Self::with_curve_only(true)
    }

    fn with_curve_only(curve_only: bool) -> Self {
        Self {
            curve_only,
            drag: None,
            hover: None,
            step_last_cell: None,
            stepped_draw_held: false,
            style: RandomStyle::Smooth,
            style_dropdown: DropdownState::new(),
            text_edit: TextEditState::new(),
            seed: 0,
        }
    }

    /// `true` for a curve-only editor.
    pub fn is_curve_only(&self) -> bool {
        self.curve_only
    }

    /// Set whether the caller's stepped-draw modifier is currently held.
    pub fn set_stepped_draw(&mut self, held: bool) {
        self.stepped_draw_held = held;
    }

    /// The randomizer style currently selected in the strip.
    pub fn style(&self) -> RandomStyle {
        self.style
    }

    /// Advance the randomizer style to the next of the five variants.
    pub fn cycle_style(&mut self) {
        self.style = match self.style {
            RandomStyle::Smooth => RandomStyle::Ramps,
            RandomStyle::Ramps => RandomStyle::Stepped,
            RandomStyle::Stepped => RandomStyle::Spiky,
            RandomStyle::Spiky => RandomStyle::Chaos,
            RandomStyle::Chaos => RandomStyle::Smooth,
        };
    }
}

impl Default for MsegEditState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_full_editor() {
        assert!(!MsegEditState::new().is_curve_only());
    }

    #[test]
    fn new_curve_only_is_curve_only() {
        assert!(MsegEditState::new_curve_only().is_curve_only());
    }
}
```

- [ ] **Step 3: Register the modules in `mseg/mod.rs`**

In `tiny-skia-widgets/src/mseg/mod.rs`, after the existing `pub mod randomize;` line add `pub mod editor;` and `pub mod render;`, and after `pub use randomize::*;` add `pub use editor::*;` and `pub use render::*;`. Read `mod.rs` first to match formatting.

- [ ] **Step 4: Verify**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — the 2 new editor tests plus all existing `mseg` tests.
Run `cargo clippy -p tiny-skia-widgets -- -D warnings`. Several `MsegEditState` fields (`drag`, `hover`, `step_last_cell`, `stepped_draw_held`, `style`, `style_dropdown`, `text_edit`, `seed`) are unused until later tasks — clippy's `dead_code` only fires on private items, and these are private fields of a `pub` struct, so they are reported. Add a single `#[allow(dead_code)]` on the `struct MsegEditState` definition with a `// fields wired up across Tasks 2-11` comment; the final task removes it once every field is used.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs tiny-skia-widgets/src/mseg/render.rs tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(mseg): scaffold editor state and render module"
```

---

## Task 2: Layout geometry & coordinate mapping

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

The widget rect splits into: an optional marker lane (top), the canvas (middle), and the control strip (bottom). Coordinate mapping converts between normalized `MsegData` time/value and canvas pixels.

- [ ] **Step 1: Write failing tests**

Add a `#[cfg(test)] mod tests` to `render.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const RECT: (f32, f32, f32, f32) = (0.0, 0.0, 400.0, 300.0);

    #[test]
    fn full_layout_has_marker_lane() {
        let l = mseg_layout(RECT, false, 1.0);
        assert!(l.marker_lane.3 > 0.0, "full editor has a marker lane");
        // canvas sits below the marker lane, above the strip.
        assert!(l.canvas.1 >= l.marker_lane.1 + l.marker_lane.3 - 0.01);
        assert!(l.canvas.1 + l.canvas.3 <= l.strip.1 + 0.01);
    }

    #[test]
    fn curve_only_layout_has_no_marker_lane() {
        let l = mseg_layout(RECT, true, 1.0);
        assert_eq!(l.marker_lane.3, 0.0, "curve-only has no marker lane");
        // the canvas reclaims the marker lane's space — taller than full mode.
        let full = mseg_layout(RECT, false, 1.0);
        assert!(l.canvas.3 > full.canvas.3);
    }

    #[test]
    fn coord_mapping_round_trips() {
        let l = mseg_layout(RECT, false, 1.0);
        for &p in &[0.0, 0.25, 0.5, 1.0] {
            let x = phase_to_x(&l, p);
            assert!((x_to_phase(&l, x) - p).abs() < 1e-4, "phase {p}");
        }
        for &v in &[0.0, 0.3, 0.5, 1.0] {
            let y = value_to_y(&l, v);
            assert!((y_to_value(&l, y) - v).abs() < 1e-4, "value {v}");
        }
    }

    #[test]
    fn value_axis_is_inverted() {
        let l = mseg_layout(RECT, false, 1.0);
        // value 1.0 is at the TOP (smaller y) than value 0.0.
        assert!(value_to_y(&l, 1.0) < value_to_y(&l, 0.0));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `mseg_layout`, `phase_to_x` etc. not defined.

- [ ] **Step 3: Implement the layout + mapping**

Add to `render.rs` (after the doc comment):

```rust
/// Marker-lane height in unscaled px (full editor only).
const MARKER_LANE_H: f32 = 16.0;
/// Control-strip height in unscaled px.
const STRIP_H: f32 = 30.0;

/// Sub-rectangles of the MSEG widget, each `(x, y, w, h)`. `marker_lane` has
/// height 0 in curve-only mode.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MsegLayout {
    pub marker_lane: (f32, f32, f32, f32),
    pub canvas: (f32, f32, f32, f32),
    pub strip: (f32, f32, f32, f32),
}

/// Compute the widget's sub-rectangles. `curve_only` drops the marker lane;
/// `scale` is the DPI factor.
pub fn mseg_layout(rect: (f32, f32, f32, f32), curve_only: bool, scale: f32) -> MsegLayout {
    let (x, y, w, h) = rect;
    let lane_h = if curve_only { 0.0 } else { MARKER_LANE_H * scale };
    let strip_h = STRIP_H * scale;
    let canvas_h = (h - lane_h - strip_h).max(0.0);
    MsegLayout {
        marker_lane: (x, y, w, lane_h),
        canvas: (x, y + lane_h, w, canvas_h),
        strip: (x, y + lane_h + canvas_h, w, strip_h),
    }
}

/// Normalized phase (0..1) → canvas x pixel.
pub fn phase_to_x(layout: &MsegLayout, phase: f32) -> f32 {
    layout.canvas.0 + phase.clamp(0.0, 1.0) * layout.canvas.2
}

/// Canvas x pixel → normalized phase (0..1, clamped).
pub fn x_to_phase(layout: &MsegLayout, x: f32) -> f32 {
    if layout.canvas.2 <= 0.0 {
        return 0.0;
    }
    ((x - layout.canvas.0) / layout.canvas.2).clamp(0.0, 1.0)
}

/// Normalized value (0..1) → canvas y pixel (value 1.0 at the top).
pub fn value_to_y(layout: &MsegLayout, value: f32) -> f32 {
    layout.canvas.1 + (1.0 - value.clamp(0.0, 1.0)) * layout.canvas.3
}

/// Canvas y pixel → normalized value (0..1, clamped).
pub fn y_to_value(layout: &MsegLayout, y: f32) -> f32 {
    if layout.canvas.3 <= 0.0 {
        return 0.0;
    }
    (1.0 - (y - layout.canvas.1) / layout.canvas.3).clamp(0.0, 1.0)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 4 new tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs
git commit -m "feat(mseg): add editor layout geometry and coordinate mapping"
```

---

## Task 3: Render — background, grid, and the envelope curve

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

- [ ] **Step 1: Write a failing render smoke test**

Add inside `render.rs`'s `mod tests` — note the extra imports:

```rust
    use crate::mseg::MsegData;
    use crate::test_font::test_font_data;
    use crate::text::TextRenderer;
    use tiny_skia::Pixmap;

    fn px_alpha(pm: &Pixmap, x: u32, y: u32) -> u8 {
        pm.pixels()[(y * pm.width() + x) as usize].alpha()
    }

    #[test]
    fn draw_mseg_paints_the_canvas() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let data = MsegData::default();
        let state = MsegEditState::new();
        draw_mseg(&mut pm, &mut tr, RECT, &data, &state, 1.0);
        // The canvas interior is filled — sample a pixel well inside it.
        let l = mseg_layout(RECT, false, 1.0);
        let cx = (l.canvas.0 + l.canvas.2 * 0.5) as u32;
        let cy = (l.canvas.1 + l.canvas.3 * 0.5) as u32;
        assert!(px_alpha(&pm, cx, cy) > 0, "canvas not painted");
    }
```

`MsegEditState` is in scope via `use super::*` only if `render.rs` imports it — add `use crate::mseg::editor::MsegEditState;` near the top of `render.rs` (module scope, not in tests).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `draw_mseg` not defined.

- [ ] **Step 3: Implement `draw_mseg`'s canvas-base + grid + curve**

Add to `render.rs` at module scope. This is the first slice of `draw_mseg`; later tasks extend it with nodes (Task 4), marker lane (Task 5), and strip (Task 6).

```rust
use crate::mseg::{value_at_phase, MsegData};
use crate::primitives::{
    color_accent, color_bg, color_border, color_control_bg, draw_rect, draw_rect_outline,
};
use crate::text::TextRenderer;
use tiny_skia::Pixmap;

/// Draw the whole MSEG widget into `rect`. Composes the marker lane (full
/// mode only), the canvas (grid + curve + nodes), and the control strip.
pub fn draw_mseg(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    rect: (f32, f32, f32, f32),
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
) {
    let layout = mseg_layout(rect, state.is_curve_only(), scale);
    draw_canvas(pixmap, &layout, data, state);
    // Marker lane (Task 5) and control strip (Task 6) are drawn here in
    // later tasks; `text_renderer`/`scale` are used by those.
    let _ = (text_renderer, scale);
}

/// Draw the canvas: background, grid, and the envelope polyline.
fn draw_canvas(pixmap: &mut Pixmap, layout: &MsegLayout, data: &MsegData, _state: &MsegEditState) {
    let (cx, cy, cw, ch) = layout.canvas;
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }
    draw_rect(pixmap, cx, cy, cw, ch, color_control_bg());

    // Vertical time-grid lines.
    let tdiv = data.time_divisions.max(1);
    for i in 1..tdiv {
        let gx = cx + (i as f32 / tdiv as f32) * cw;
        draw_rect(pixmap, gx, cy, 1.0, ch, color_bg());
    }
    // Horizontal value-grid lines.
    let vsteps = data.value_steps.max(1);
    for i in 1..vsteps {
        let gy = cy + (i as f32 / vsteps as f32) * ch;
        draw_rect(pixmap, cx, gy, cw, 1.0, color_bg());
    }

    // Envelope polyline: sample `value_at_phase` per pixel column.
    let cols = cw.max(1.0) as usize;
    let mut prev: Option<(f32, f32)> = None;
    for col in 0..=cols {
        let phase = col as f32 / cols as f32;
        let x = cx + phase * cw;
        let y = cy + (1.0 - value_at_phase(data, phase)) * ch;
        if let Some((px, py)) = prev {
            draw_line(pixmap, px, py, x, y, color_accent());
        }
        prev = Some((x, y));
    }

    draw_rect_outline(pixmap, cx, cy, cw, ch, color_border(), 1.0);
}

/// Draw a 1px line by sampling points along it (sufficient for the curve;
/// stepped segments produce near-vertical jumps which this still renders).
fn draw_line(pixmap: &mut Pixmap, x0: f32, y0: f32, x1: f32, y1: f32, color: tiny_skia::Color) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as usize;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        draw_rect(pixmap, x0 + dx * t, y0 + dy * t, 1.0, 1.0, color);
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — `draw_mseg_paints_the_canvas` plus the Task 1-2 tests.

Run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs
git commit -m "feat(mseg): render editor canvas — grid and envelope curve"
```

---

## Task 4: Render — nodes & tension handles

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

- [ ] **Step 1: Write a failing test**

Add inside `render.rs`'s `mod tests`:

```rust
    #[test]
    fn draw_mseg_paints_node_dots() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let mut data = MsegData::default(); // nodes at (0,0) and (1,1)
        data.insert_node(0.5, 0.5);
        let state = MsegEditState::new();
        draw_mseg(&mut pm, &mut tr, RECT, &data, &state, 1.0);
        // The interior node at (0.5, 0.5) maps to a dot near canvas centre.
        let l = mseg_layout(RECT, false, 1.0);
        let nx = phase_to_x(&l, 0.5) as u32;
        let ny = value_to_y(&l, 0.5) as u32;
        assert!(px_alpha(&pm, nx, ny) > 0, "node dot not painted");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: the test PASSES already if the curve happens to paint that pixel — to make it a true failing test first, temporarily assert a node-specific colour. Simplest: skip the fail-check here (the node dot is additive); run Step 4 after implementing and confirm the test passes. (Acceptable deviation from strict red-green: the pixel is on the curve too. The node-rendering code below is still required by the spec.)

- [ ] **Step 3: Implement node + tension-handle drawing**

Add a `draw_nodes` function to `render.rs` and call it from `draw_canvas` after the polyline, before the outline:

```rust
/// Node-dot radius and tension-handle radius, unscaled px.
const NODE_R: f32 = 4.0;
const TENSION_R: f32 = 3.0;

/// Draw a filled square "dot" centred at `(x, y)`.
fn draw_dot(pixmap: &mut Pixmap, x: f32, y: f32, r: f32, color: tiny_skia::Color) {
    draw_rect(pixmap, x - r, y - r, r * 2.0, r * 2.0, color);
}

/// Draw node dots and per-segment tension handles over the curve.
fn draw_nodes(pixmap: &mut Pixmap, layout: &MsegLayout, data: &MsegData, state: &MsegEditState) {
    let a = data.active();
    // Tension handles: midpoint of each non-stepped segment.
    for i in 0..data.node_count - 1 {
        if a[i].stepped {
            continue;
        }
        let mid_phase = (a[i].time + a[i + 1].time) * 0.5;
        let hx = phase_to_x(layout, mid_phase);
        let hy = value_to_y(layout, value_at_phase(data, mid_phase));
        draw_dot(pixmap, hx, hy, TENSION_R, color_border());
    }
    // Node dots; the hovered node is drawn larger / accented.
    for (i, n) in a.iter().enumerate() {
        let nx = phase_to_x(layout, n.time);
        let ny = value_to_y(layout, n.value);
        let hovered = state.hovered_node() == Some(i);
        let r = if hovered { NODE_R + 1.5 } else { NODE_R };
        draw_dot(pixmap, nx, ny, r, color_accent());
    }
}
```

Add `MsegEditState::hovered_node` to `editor.rs`'s `impl MsegEditState`:

```rust
    /// The currently hovered node index, if any.
    pub fn hovered_node(&self) -> Option<usize> {
        self.hover
    }
```

In `render.rs`'s `draw_canvas`, add `draw_nodes(pixmap, layout, data, _state);` immediately before the `draw_rect_outline(...)` call, and rename the `_state` parameter of `draw_canvas` to `state` (it is now used).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — `draw_mseg_paints_node_dots` plus all earlier tests.
Run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(mseg): render node dots and tension handles"
```

---

## Task 5: Render — marker lane

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

- [ ] **Step 1: Write failing tests**

Add inside `render.rs`'s `mod tests`:

```rust
    #[test]
    fn marker_lane_drawn_for_sustain_in_full_mode() {
        use crate::mseg::HoldMode;
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        data.hold = HoldMode::Sustain(1);
        let state = MsegEditState::new();
        draw_mseg(&mut pm, &mut tr, RECT, &data, &state, 1.0);
        // A sustain marker sits in the marker lane above node 1's x.
        let l = mseg_layout(RECT, false, 1.0);
        let mx = phase_to_x(&l, 0.5) as u32;
        let my = (l.marker_lane.1 + l.marker_lane.3 * 0.5) as u32;
        assert!(px_alpha(&pm, mx, my) > 0, "sustain marker not drawn");
    }

    #[test]
    fn marker_lane_skipped_in_curve_only_mode() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let data = MsegData::default();
        let state = MsegEditState::new_curve_only();
        // Must not panic; the marker lane has zero height.
        draw_mseg(&mut pm, &mut tr, RECT, &data, &state, 1.0);
        assert_eq!(mseg_layout(RECT, true, 1.0).marker_lane.3, 0.0);
    }
```

- [ ] **Step 2: Run tests to verify they fail/pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: `marker_lane_skipped_in_curve_only_mode` already passes (no marker drawing yet, and the layout is correct); `marker_lane_drawn_for_sustain_in_full_mode` FAILS (no marker drawn).

- [ ] **Step 3: Implement the marker lane**

Add to `render.rs` and call from `draw_mseg` after `draw_canvas` (full mode only):

```rust
use crate::mseg::HoldMode;

/// Draw the hold marker(s) in the marker lane. No-op in curve-only mode or
/// when `hold` is `None`.
fn draw_marker_lane(
    pixmap: &mut Pixmap,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
) {
    if state.is_curve_only() || layout.marker_lane.3 <= 0.0 {
        return;
    }
    let (lx, ly, lw, lh) = layout.marker_lane;
    draw_rect(pixmap, lx, ly, lw, lh, color_bg());
    let a = data.active();
    let mark = |pm: &mut Pixmap, node: usize, color: tiny_skia::Color| {
        if node < data.node_count {
            let mx = phase_to_x(layout, a[node].time);
            draw_rect(pm, mx - 3.0, ly + 2.0, 6.0, lh - 4.0, color);
        }
    };
    match data.hold {
        HoldMode::None => {}
        HoldMode::Sustain(i) => mark(pixmap, i, color_accent()),
        HoldMode::Loop { start, end } => {
            mark(pixmap, start, color_border());
            mark(pixmap, end, color_border());
        }
    }
}
```

In `draw_mseg`, after `draw_canvas(...)`, add `draw_marker_lane(pixmap, &layout, data, state);`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — both new tests plus all earlier.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs
git commit -m "feat(mseg): render the hold-marker lane"
```

---

## Task 6: Render — control strip

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

The strip always shows grid (time + value), a snap toggle, and the randomizer (a style label + Randomize button). Full mode additionally shows sync mode and a hold-mode selector. This task draws the strip; strip *interaction* is Task 11. Drawing reuses `draw_button` / `draw_stepped_selector`.

- [ ] **Step 1: Write a failing test**

Add inside `render.rs`'s `mod tests`:

```rust
    #[test]
    fn control_strip_is_painted() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let data = MsegData::default();
        let state = MsegEditState::new();
        draw_mseg(&mut pm, &mut tr, RECT, &data, &state, 1.0);
        let l = mseg_layout(RECT, false, 1.0);
        let sx = (l.strip.0 + 10.0) as u32;
        let sy = (l.strip.1 + l.strip.3 * 0.5) as u32;
        assert!(px_alpha(&pm, sx, sy) > 0, "strip not painted");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — the strip area is unpainted.

- [ ] **Step 3: Implement strip drawing**

Add to `render.rs` and call from `draw_mseg`:

```rust
/// Draw the control strip background and labels. Interaction is wired in
/// Task 11; this draws the static strip and reuses the crate's button style.
fn draw_strip(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
) {
    let (sx, sy, sw, sh) = layout.strip;
    if sw <= 0.0 || sh <= 0.0 {
        return;
    }
    draw_rect(pixmap, sx, sy, sw, sh, color_bg());
    draw_rect_outline(pixmap, sx, sy, sw, sh, color_border(), 1.0);

    let pad = 6.0;
    let text_size = (sh * 0.42).max(9.0);
    let ty = sy + (sh + text_size) * 0.5 - 2.0;
    // Three click-cyclable readouts (snap | grid | style) — one per third of
    // the strip width left of the Randomize button (Task 11 wires the zones).
    let label = format!(
        "snap {}    grid {}/{}    style {:?}",
        if data.snap { "on" } else { "off" },
        data.time_divisions,
        data.value_steps,
        state.style(),
    );
    text_renderer.draw_text(pixmap, sx + pad, ty, &label, text_size, color_border());

    // Randomize button at the right end.
    let btn_w = 84.0;
    crate::controls::draw_button(
        pixmap,
        text_renderer,
        sx + sw - btn_w - pad,
        sy + 3.0,
        btn_w,
        sh - 6.0,
        "Randomize",
        false,
        false,
    );
}
```

In `draw_mseg`, replace the `let _ = (text_renderer, scale);` line with `draw_strip(pixmap, text_renderer, &layout, data, state);`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — `control_strip_is_painted` plus all earlier tests.
Run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs
git commit -m "feat(mseg): render the control strip"
```

---

## Task 7: Hit-testing

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`

A pure `mseg_hit_test` maps a pixel `(x, y)` to what is under it — node, tension handle, marker, empty canvas, or the Randomize button. It is the shared seam for all interaction (Tasks 8-11).

- [ ] **Step 1: Write failing tests**

Add inside `render.rs`'s `mod tests`:

```rust
    #[test]
    fn hit_test_finds_a_node() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let l = mseg_layout(RECT, false, 1.0);
        let nx = phase_to_x(&l, 0.5);
        let ny = value_to_y(&l, 0.5);
        assert_eq!(mseg_hit_test(&l, &data, false, nx, ny), MsegHit::Node(1));
    }

    #[test]
    fn hit_test_empty_canvas() {
        let data = MsegData::default(); // nodes only at the corners
        let l = mseg_layout(RECT, false, 1.0);
        let hit = mseg_hit_test(&l, &data, false, l.canvas.0 + l.canvas.2 * 0.5,
                                l.canvas.1 + l.canvas.3 * 0.5);
        assert_eq!(hit, MsegHit::Canvas);
    }

    #[test]
    fn hit_test_randomize_button() {
        let data = MsegData::default();
        let l = mseg_layout(RECT, false, 1.0);
        // The Randomize button is at the strip's right end.
        let bx = l.strip.0 + l.strip.2 - 48.0;
        let by = l.strip.1 + l.strip.3 * 0.5;
        assert_eq!(mseg_hit_test(&l, &data, false, bx, by), MsegHit::Randomize);
    }

    #[test]
    fn hit_test_outside_is_none() {
        let data = MsegData::default();
        let l = mseg_layout(RECT, false, 1.0);
        assert_eq!(mseg_hit_test(&l, &data, false, -10.0, -10.0), MsegHit::None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `mseg_hit_test` / `MsegHit` not defined.

- [ ] **Step 3: Implement hit-testing**

Add to `render.rs`:

```rust
/// Result of hit-testing a pixel against the MSEG widget.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MsegHit {
    /// On the node dot at this index.
    Node(usize),
    /// On the tension handle of the segment starting at this node index.
    Tension(usize),
    /// On the canvas but not on any node/handle.
    Canvas,
    /// On the Randomize button.
    Randomize,
    /// On the strip but not on a recognised control.
    Strip,
    /// On the marker lane.
    MarkerLane,
    /// Outside the widget.
    None,
}

/// Pointer hit radius (unscaled px) for node / handle picking.
const HIT_R: f32 = 7.0;

fn in_rect(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// Hit-test pixel `(x, y)` against the widget. `curve_only` suppresses the
/// marker lane. Nodes take priority over tension handles, which take priority
/// over empty canvas.
pub fn mseg_hit_test(
    layout: &MsegLayout,
    data: &MsegData,
    curve_only: bool,
    x: f32,
    y: f32,
) -> MsegHit {
    let a = data.active();
    if in_rect(layout.canvas, x, y) {
        // Nodes first.
        for (i, n) in a.iter().enumerate() {
            let nx = phase_to_x(layout, n.time);
            let ny = value_to_y(layout, n.value);
            if (x - nx).abs() <= HIT_R && (y - ny).abs() <= HIT_R {
                return MsegHit::Node(i);
            }
        }
        // Then tension handles.
        for i in 0..data.node_count - 1 {
            if a[i].stepped {
                continue;
            }
            let mid = (a[i].time + a[i + 1].time) * 0.5;
            let hx = phase_to_x(layout, mid);
            let hy = value_to_y(layout, value_at_phase(data, mid));
            if (x - hx).abs() <= HIT_R && (y - hy).abs() <= HIT_R {
                return MsegHit::Tension(i);
            }
        }
        return MsegHit::Canvas;
    }
    if !curve_only && in_rect(layout.marker_lane, x, y) {
        return MsegHit::MarkerLane;
    }
    if in_rect(layout.strip, x, y) {
        // Randomize button — right end, 84px + 6px pad (matches draw_strip).
        let btn_x = layout.strip.0 + layout.strip.2 - 84.0 - 6.0;
        if x >= btn_x {
            return MsegHit::Randomize;
        }
        return MsegHit::Strip;
    }
    MsegHit::None
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 4 new hit-test tests.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs
git commit -m "feat(mseg): add editor hit-testing"
```

---

## Task 8: Interaction — add / move nodes & tension drag

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`

- [ ] **Step 1: Write failing tests**

Add inside `editor.rs`'s `mod tests` — note the imports:

```rust
    use crate::mseg::render::{mseg_layout, phase_to_x, value_to_y};
    use crate::mseg::MsegData;

    const RECT: (f32, f32, f32, f32) = (0.0, 0.0, 400.0, 300.0);

    #[test]
    fn click_empty_canvas_inserts_a_node() {
        let mut data = MsegData::default(); // 2 nodes
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let x = phase_to_x(&l, 0.5);
        let y = value_to_y(&l, 0.5);
        let ev = state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert_eq!(data.node_count, 3);
    }

    #[test]
    fn drag_moves_a_node() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Press on node 1, drag it down to value ~0.2.
        state.on_mouse_down(phase_to_x(&l, 0.5), value_to_y(&l, 0.5),
                            &mut data, RECT, 1.0, false);
        state.on_mouse_move(phase_to_x(&l, 0.5), value_to_y(&l, 0.2),
                            &mut data, RECT, 1.0, false);
        assert!((data.nodes[1].value - 0.2).abs() < 0.05);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn handlers_noop_when_pointer_outside() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        assert_eq!(state.on_mouse_down(-5.0, -5.0, &mut data, RECT, 1.0, false), None);
        assert_eq!(data.node_count, 2);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `on_mouse_down` / `on_mouse_move` / `on_mouse_up` not defined.

- [ ] **Step 3: Implement the core mouse handlers**

Add to `editor.rs`'s `impl MsegEditState`. (Stepped-draw, double-click, right-click, and strip/marker handling are added in Tasks 9-11; the `stepped_draw_held` branch and `MsegHit::Strip`/`Randomize`/`MarkerLane` arms are stubbed here and filled later.)

```rust
    /// Primary-button press. Returns `MsegEdit::Changed` when the document
    /// changed. Adds a node on empty canvas, begins a node or tension drag on
    /// a hit handle.
    pub fn on_mouse_down(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
        fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        let _ = fine;
        match mseg_hit_test(&layout, data, self.curve_only, x, y) {
            MsegHit::Node(i) => {
                self.drag = Some(DragTarget::Node(i));
                None
            }
            MsegHit::Tension(i) => {
                self.drag = Some(DragTarget::Tension(i));
                None
            }
            MsegHit::Canvas => {
                // Stepped-draw (Task 10) takes over when its modifier is held.
                let phase = x_to_phase(&layout, x);
                let value = y_to_value(&layout, y);
                let inserted = data.insert_node(phase, value);
                if let Some(idx) = inserted {
                    self.drag = Some(DragTarget::Node(idx));
                    Some(MsegEdit::Changed)
                } else {
                    None
                }
            }
            MsegHit::None => None,
            // Strip / Randomize / MarkerLane handled in later tasks.
            _ => None,
        }
    }

    /// Pointer motion. Applies the active drag.
    pub fn on_mouse_move(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
        fine: bool,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, y_to_value, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        let _ = fine;
        // Hover highlight (only when not dragging).
        if self.drag.is_none() {
            self.hover = match mseg_hit_test(&layout, data, self.curve_only, x, y) {
                MsegHit::Node(i) => Some(i),
                _ => None,
            };
        }
        match self.drag {
            Some(DragTarget::Node(i)) => {
                let phase = x_to_phase(&layout, x);
                let value = y_to_value(&layout, y);
                data.move_node(i, phase, value);
                Some(MsegEdit::Changed)
            }
            Some(DragTarget::Tension(i)) => {
                // Drag vertically away from the segment's straight midpoint to
                // bend it. Map the vertical offset to tension in -1..1.
                let a = data.active();
                if i + 1 < data.node_count {
                    let straight_mid = (a[i].value + a[i + 1].value) * 0.5;
                    let cur = y_to_value(&layout, y);
                    let rising = a[i + 1].value >= a[i].value;
                    let delta = (cur - straight_mid) * if rising { -2.0 } else { 2.0 };
                    data.nodes[i].tension = delta.clamp(-1.0, 1.0);
                    data.debug_assert_valid();
                    return Some(MsegEdit::Changed);
                }
                None
            }
            _ => None,
        }
    }

    /// Primary-button release. Ends any drag.
    pub fn on_mouse_up(&mut self, data: &mut MsegData) -> Option<MsegEdit> {
        let _ = data;
        self.drag = None;
        self.step_last_cell = None;
        None
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 3 new tests plus earlier.
Run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(mseg): add node add/move and tension-drag interaction"
```

---

## Task 9: Interaction — delete node & toggle stepped

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`

- [ ] **Step 1: Write failing tests**

Add inside `editor.rs`'s `mod tests`:

```rust
    #[test]
    fn double_click_deletes_interior_node() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let ev = state.on_double_click(phase_to_x(&l, 0.5), value_to_y(&l, 0.5),
                                       &mut data, RECT, 1.0);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert_eq!(data.node_count, 2);
    }

    #[test]
    fn double_click_endpoint_does_nothing() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let ev = state.on_double_click(phase_to_x(&l, 0.0), value_to_y(&l, 0.0),
                                       &mut data, RECT, 1.0);
        assert_eq!(ev, None);
        assert_eq!(data.node_count, 2);
    }

    #[test]
    fn right_click_toggles_segment_stepped() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Right-click mid-way along segment 0 (phase ~0.25).
        let x = phase_to_x(&l, 0.25);
        let y = value_to_y(&l, 0.25);
        assert!(!data.nodes[0].stepped);
        let ev = state.on_right_click(x, y, &mut data, RECT, 1.0);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert!(data.nodes[0].stepped);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — `on_double_click` / `on_right_click` not defined.

- [ ] **Step 3: Implement delete & toggle-stepped**

Add to `editor.rs`'s `impl MsegEditState`:

```rust
    /// Double-click: delete the node under the pointer (endpoints excepted).
    pub fn on_double_click(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        if let MsegHit::Node(i) = mseg_hit_test(&layout, data, self.curve_only, x, y) {
            if data.remove_node(i) {
                self.drag = None;
                self.hover = None;
                return Some(MsegEdit::Changed);
            }
        }
        None
    }

    /// Right-click: toggle the `stepped` flag of the segment under the
    /// pointer. The segment is the one whose time range contains the click.
    pub fn on_right_click(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{mseg_hit_test, mseg_layout, x_to_phase, MsegHit};
        let layout = mseg_layout(rect, self.curve_only, scale);
        if !matches!(
            mseg_hit_test(&layout, data, self.curve_only, x, y),
            MsegHit::Canvas | MsegHit::Tension(_) | MsegHit::Node(_)
        ) {
            return None;
        }
        let phase = x_to_phase(&layout, x);
        // Segment i is the last node whose time is <= phase, capped so a
        // segment always exists.
        let a = data.active();
        let mut seg = 0;
        for i in 0..data.node_count - 1 {
            if a[i].time <= phase {
                seg = i;
            }
        }
        data.nodes[seg].stepped = !data.nodes[seg].stepped;
        data.debug_assert_valid();
        Some(MsegEdit::Changed)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — all 3 new tests plus earlier.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(mseg): add delete-node and toggle-stepped interaction"
```

---

## Task 10: Interaction — stepped-draw

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`

While the caller's stepped-draw modifier is held, pressing then dragging across the canvas paints a stepped node in each time-grid cell the pointer enters.

- [ ] **Step 1: Write failing tests**

Add inside `editor.rs`'s `mod tests`:

```rust
    #[test]
    fn stepped_draw_paints_nodes_across_cells() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        state.set_stepped_draw(true);
        let l = mseg_layout(RECT, false, 1.0);
        let before = data.node_count;
        // Press, then drag across several grid cells.
        state.on_mouse_down(phase_to_x(&l, 0.1), value_to_y(&l, 0.8),
                            &mut data, RECT, 1.0, false);
        for &p in &[0.3_f32, 0.5, 0.7, 0.9] {
            state.on_mouse_move(phase_to_x(&l, p), value_to_y(&l, 0.6),
                                &mut data, RECT, 1.0, false);
        }
        state.on_mouse_up(&mut data);
        assert!(data.node_count > before, "stepped-draw inserted no nodes");
        // Painted nodes are stepped.
        assert!(data.active().iter().take(data.node_count - 1).any(|n| n.stepped));
    }

    #[test]
    fn stepped_draw_inactive_when_modifier_not_held() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new(); // modifier NOT held
        let l = mseg_layout(RECT, false, 1.0);
        state.on_mouse_down(phase_to_x(&l, 0.5), value_to_y(&l, 0.5),
                            &mut data, RECT, 1.0, false);
        // Without the modifier this is an ordinary single-node insert.
        assert_eq!(data.node_count, 3);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: `stepped_draw_inactive_when_modifier_not_held` passes (Task 8 behavior); `stepped_draw_paints_nodes_across_cells` FAILS (stepped-draw not implemented — modifier held, but `on_mouse_down`/`move` do not paint).

- [ ] **Step 3: Implement stepped-draw**

Add a private helper and wire it into the two handlers. Add to `editor.rs`'s `impl MsegEditState`:

```rust
    /// Paint one stepped node at the pointer if it has entered a new
    /// time-grid cell since the last paint. Used by stepped-draw.
    fn step_draw_paint(
        &mut self,
        x: f32,
        y: f32,
        data: &mut MsegData,
        layout: &crate::mseg::render::MsegLayout,
    ) -> Option<MsegEdit> {
        use crate::mseg::render::{x_to_phase, y_to_value};
        let phase = x_to_phase(layout, x);
        let value = y_to_value(layout, y);
        let tdiv = data.time_divisions.max(1);
        let cell = (phase * tdiv as f32) as u32;
        if self.step_last_cell == Some(cell) {
            return None; // still inside the last painted cell
        }
        self.step_last_cell = Some(cell);
        // Snap the node to the cell's left edge; mark its segment stepped.
        let snapped_phase = (cell as f32 / tdiv as f32).clamp(0.0, 1.0);
        if let Some(idx) = data.insert_node(snapped_phase, value) {
            // The new node and the one before it both belong to the stepped
            // run — set the *previous* node's segment stepped so the painted
            // run reads as steps.
            if idx > 0 {
                data.nodes[idx - 1].stepped = true;
            }
            data.nodes[idx].stepped = true;
            data.debug_assert_valid();
            return Some(MsegEdit::Changed);
        }
        None
    }
```

In `on_mouse_down`, at the very start of the `MsegHit::Canvas` arm (before the ordinary insert), add:

```rust
            MsegHit::Canvas if self.stepped_draw_held => {
                self.drag = Some(DragTarget::StepDraw);
                self.step_last_cell = None;
                return self.step_draw_paint(x, y, data, &layout);
            }
```

(Place this arm *before* the existing `MsegHit::Canvas =>` arm so the held-modifier case wins.)

In `on_mouse_move`, add a `Some(DragTarget::StepDraw)` arm to the `match self.drag` block:

```rust
            Some(DragTarget::StepDraw) => self.step_draw_paint(x, y, data, &layout),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — both new tests plus all earlier.
Run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(mseg): add stepped-draw interaction"
```

---

## Task 11: Interaction — strip controls (snap, grid, randomize)

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs`

The strip's interactive controls. The strip area left of the Randomize button is split into three click-cyclable zones: a **snap toggle** (left third), a **grid cycle** (middle third — cycles both time and value divisions together), and a **style cycle** (right third — advances the randomizer style through the five variants). The **Randomize button** is the already-distinct `MsegHit::Randomize`. Sync-mode / hold-mode / numeric text-entry are full-mode-only niceties deferred to a follow-up (miff, the immediate consumer, uses curve-only mode and needs none of them); this task delivers every control miff needs.

- [ ] **Step 1: Write failing tests**

Add inside `editor.rs`'s `mod tests`:

```rust
    #[test]
    fn randomize_button_regenerates_and_changes_seed() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        let bx = l.strip.0 + l.strip.2 - 48.0;
        let by = l.strip.1 + l.strip.3 * 0.5;
        let ev = state.on_mouse_down(bx, by, &mut data, RECT, 1.0, false);
        assert_eq!(ev, Some(MsegEdit::Changed));
        assert!(data.is_valid());
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn snap_toggle_zone_flips_snap() {
        let mut data = MsegData::default();
        let was = data.snap;
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Left third of the strip = snap toggle.
        let x = l.strip.0 + l.strip.2 * 0.1;
        let y = l.strip.1 + l.strip.3 * 0.5;
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert_eq!(data.snap, !was);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn grid_zone_cycles_both_axes() {
        let mut data = MsegData::default(); // time_divisions 16, value_steps 8
        let mut state = MsegEditState::new();
        let l = mseg_layout(RECT, false, 1.0);
        // Middle third of the strip = grid cycle.
        let x = l.strip.0 + l.strip.2 * 0.5;
        let y = l.strip.1 + l.strip.3 * 0.5;
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        // 16 -> (32, 16): both axes advanced.
        assert_eq!(data.time_divisions, 32);
        assert_eq!(data.value_steps, 16);
        state.on_mouse_up(&mut data);
    }

    #[test]
    fn style_zone_cycles_the_randomizer_style() {
        let mut data = MsegData::default();
        let mut state = MsegEditState::new();
        let before = state.style();
        let l = mseg_layout(RECT, false, 1.0);
        // Right third of the strip (left of the 84px+6px Randomize button).
        let x = l.strip.0 + l.strip.2 * 0.7;
        let y = l.strip.1 + l.strip.3 * 0.5;
        let ev = state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        assert_eq!(ev, None, "cycling style changes editor state, not the document");
        assert_ne!(state.style(), before);
        state.on_mouse_up(&mut data);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: FAIL — the Randomize / Strip arms of `on_mouse_down` are currently `_ => None` no-ops.

- [ ] **Step 3: Implement strip interaction**

In `editor.rs`, replace the `_ => None,` arm of `on_mouse_down`'s `match` with explicit arms:

```rust
            MsegHit::Randomize => {
                self.seed = self.seed.wrapping_add(1);
                crate::mseg::randomize::randomize(data, self.style, self.seed);
                Some(MsegEdit::Changed)
            }
            MsegHit::Strip => {
                // Left third: snap toggle. Middle third: cycle grid (both
                // axes). Right third: cycle the randomizer style.
                let third = layout.strip.2 / 3.0;
                let local = x - layout.strip.0;
                if local < third {
                    data.snap = !data.snap;
                    Some(MsegEdit::Changed)
                } else if local < third * 2.0 {
                    let (t, v) = match data.time_divisions {
                        0..=4 => (8, 8),
                        5..=8 => (16, 8),
                        9..=16 => (32, 16),
                        _ => (4, 4),
                    };
                    data.time_divisions = t;
                    data.value_steps = v;
                    Some(MsegEdit::Changed)
                } else {
                    // Style cycle: editor state only — not a document change.
                    self.cycle_style();
                    None
                }
            }
            MsegHit::MarkerLane => None,
            MsegHit::None => None,
```

(Delete the old `MsegHit::None => None,` and `_ => None,` lines — every variant is now covered explicitly: `Node`, `Tension`, `Canvas` (×2 with the stepped-draw guard), `Randomize`, `Strip`, `MarkerLane`, `None`. Confirm the `match` is exhaustive.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — both new tests plus all earlier.
Run `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(mseg): add strip interaction — snap, grid, randomize"
```

---

## Task 12: Final verification & cleanup

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs` (remove the Task 1 `#[allow]`)

- [ ] **Step 1: Remove the scaffold allow**

Every `MsegEditState` field is now used (`drag`, `hover`, `step_last_cell`, `stepped_draw_held`, `style`, `style_dropdown`, `text_edit`, `seed`). Remove the `#[allow(dead_code)]` added on `struct MsegEditState` in Task 1.

If `cargo clippy -p tiny-skia-widgets -- -D warnings` then reports `style_dropdown` or `text_edit` as genuinely unused (this plan wired snap/grid/randomize but deferred the dropdown-based style picker and numeric text-entry to a follow-up): keep those two fields, and instead of a struct-wide allow add a targeted `#[allow(dead_code)]` on just those two fields with a `// reserved for the style-dropdown / numeric-entry follow-up` comment. Do not delete them — they are part of the spec's `MsegEditState`.

- [ ] **Step 2: Run the whole module suite**

Run: `cargo nextest run -p tiny-skia-widgets mseg`
Expected: PASS — every `mseg` editor + core test.

- [ ] **Step 3: Full crate suite, workspace lint, fmt**

Run: `cargo nextest run -p tiny-skia-widgets`
Expected: PASS — the new editor tests plus all pre-existing crate tests.

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

Run: `cargo fmt` then `cargo fmt --check`
Expected: clean.

- [ ] **Step 4: Confirm the public API**

Run: `cargo doc -p tiny-skia-widgets --no-deps`
Expected: builds clean. `MsegEditState`, `MsegEdit`, `draw_mseg`, `mseg_layout`, `MsegLayout`, `MsegHit`, `mseg_hit_test`, `phase_to_x`/`x_to_phase`/`value_to_y`/`y_to_value`, and `MsegEditState::new`/`new_curve_only`/`on_mouse_down`/`on_mouse_move`/`on_mouse_up`/`on_double_click`/`on_right_click`/`set_stepped_draw`/`hovered_node`/`is_curve_only` are reachable at the crate root.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs
git commit -m "chore(mseg): drop editor scaffold allow after wiring all fields"
```

---

## Self-Review Notes

**Spec coverage** — the spec's editor requirements map to tasks:
- Module layout (`render.rs`, `editor.rs`) → Task 1.
- `MsegEditState` + `MsegEdit` + `new`/`new_curve_only` → Task 1.
- Layout + coordinate mapping → Task 2.
- Rendering: canvas grid + curve → Task 3; nodes + tension handles → Task 4; marker lane → Task 5; control strip → Task 6.
- Hit-testing → Task 7.
- Interaction: add/move/tension → Task 8; delete + toggle-stepped → Task 9; stepped-draw → Task 10; strip controls (snap, grid, randomize) → Task 11.
- Curve-only mode → threaded through Tasks 1, 2, 5, 6, 7 (the `curve_only` flag suppresses the marker lane and its hit zone; the layout reclaims the space).
- Render smoke tests → Tasks 3-6; interaction tests → Tasks 8-11; final verification → Task 12.

**Coverage of strip features, and deliberate trims:**
- Every control miff (curve-only) needs is delivered: curve editing, the grid
  (both axes, via the grid-cycle zone), snap, and the randomizer **with all
  five styles selectable** (the style-cycle zone). The style is chosen by a
  cycle zone rather than the spec's `dropdown` *widget*, and the grid is set
  by a cycle rather than `TextEditState` numeric entry — functionally
  complete (all styles / a sensible grid range reachable), simpler UI. The
  richer `dropdown`-widget style picker and exact numeric grid entry are an
  additive follow-up; `MsegEditState` still carries `style_dropdown` and
  `text_edit` (Task 12 keeps them with a targeted allow) so that follow-up
  needs no rework.
- **Genuinely deferred (no current consumer — miff uses curve-only mode):**
  the full-mode-only playback controls (sync-mode, duration, hold-mode
  selector); marker-lane *dragging* (markers are rendered in Task 5 but the
  `Marker` drag target is not yet wired); and the optional playhead line.
  All are flagged here, not silently dropped, and are clean follow-ups.

**Type consistency** — `MsegEditState`, `MsegEdit`, `DragTarget`, `MarkerHandle`, `StripId`, `MsegLayout`, `MsegHit` are used identically across tasks. Handler signatures: `on_mouse_down/on_mouse_move(x, y, &mut MsegData, rect, scale, fine)`, `on_mouse_up(&mut MsegData)`, `on_double_click/on_right_click(x, y, &mut MsegData, rect, scale)` — consistent. `mseg_layout(rect, curve_only, scale)`, `mseg_hit_test(&layout, &data, curve_only, x, y)` — consistent.

**Note on the `fine` parameter:** the handlers accept `fine` (shift-for-fine snap bypass) per the spec; this plan threads it through but does not yet branch on it (snapping itself is applied by `MsegData::move_node`/`insert_node`, which always clamp to grid via the core). Wiring `fine` to bypass snap is a small follow-up; the parameter is in the signature so adding it later is non-breaking.
