# MSEG Editor Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Coordinate the three MSEGs' visual identity (selector tab, active curve, ghost curves, and param-dial modulation arc) under a single per-slot palette; add a hover-node tooltip that shows the mapped parameter value; add a right-click context menu on selected nodes with four compress/expand transforms.

**Architecture:** A single `mseg_color(slot)` palette lookup in multosis becomes the source of truth, threaded into the existing `draw_mseg` / `draw_mseg_ghost` widget calls, the stepped-selector tab fill, and the param-dial `_ex` modulation-arc parameter. Right-click on a selected node opens a `DropdownState`-backed transform menu; right-click everywhere else keeps its existing segment-stepped behaviour. Compress/expand maths is the closed-form `new = mean + (old - mean) · k` per axis.

**Tech Stack:** Rust (nightly, portable SIMD), nih-plug, tiny-skia, fontdue, softbuffer. CPU rendering throughout; no GPU.

**Spec:** `docs/superpowers/specs/2026-05-23-mseg-editor-improvements-design.md`.

---

## File Structure

| File | Responsibility |
|---|---|
| `multosis/src/editor.rs` | Owns `mseg_color(slot)`. Passes the right colour into both `draw_mseg` (active) and `draw_mseg_ghost` (each ghost). `compute_modulated_norms` returns `[Option<(f32, u8)>; N]`. |
| `multosis/src/editor/effect_editor.rs` | Selector tab uses `mseg_color(active_slot)` via the extended `draw_stepped_selector`. Per-dial loop threads the tagged MSEG slot into `mod_color` on the dial `_ex` calls. Pre-formats the tooltip text for the hovered node and forwards into `draw_mseg`. |
| `tiny-skia-widgets/src/controls.rs` | `draw_stepped_selector` gains `active_color: Option<Color>`. |
| `tiny-skia-widgets/src/mseg/render.rs` | `draw_mseg` gains `value_color: Color` and an `Option<NodeTooltip>` for the hover tooltip. New `draw_node_tooltip` helper. |
| `tiny-skia-widgets/src/mseg/editor.rs` | `MsegHit` gains `SelectedNode(i)`; the hit-tester chooses between `Node` and `SelectedNode` based on selection state. New `StripId::Transform`. Right-click on `SelectedNode` opens the transform menu. Four new methods on `MsegEditState`: `compress_values`, `expand_values`, `compress_times`, `expand_times`. |
| `tiny-skia-widgets/src/param_dial.rs` | `draw_dial_ex` / `draw_dial_dimmed_ex` / `draw_dial_inner` gain `mod_color: Color`. `color_modulation()` / `color_modulation_dot()` become hue-parameterised helpers. |

---

## Task 1: `mseg_color` palette

**Files:**
- Modify: `multosis/src/editor.rs` (add the function near the other module-level helpers, before `MultosisWindow`)
- Test: `multosis/src/editor.rs` (existing `#[cfg(test)] mod tests` at the bottom of the file; if there isn't one, add it)

- [ ] **Step 1: Locate the file's existing test module**

Run: `grep -n "#\[cfg(test)\]\|mod tests" /home/mpd/git-sources/tract-plugin-pack/multosis/src/editor.rs | head`
Expected: at least one `#[cfg(test)]` line. If multiple, prefer the bottom-most. If none, you will create one in Step 3.

- [ ] **Step 2: Write the failing test**

Append to the editor's test module (or create one at the file's end):

```rust
#[test]
fn mseg_color_returns_the_three_slot_hues_and_clamps_oob() {
    use tiny_skia::Color;
    let amp = mseg_color(0);
    let m1 = mseg_color(1);
    let m2 = mseg_color(2);
    // Three distinct colours.
    assert_ne!(rgb8(amp), rgb8(m1));
    assert_ne!(rgb8(amp), rgb8(m2));
    assert_ne!(rgb8(m1), rgb8(m2));
    // Amp matches the existing accent (sky blue 0x4fc3f7).
    assert_eq!(rgb8(amp), (0x4f, 0xc3, 0xf7));
    // MSEG 1 amber.
    assert_eq!(rgb8(m1), (0xff, 0xc8, 0x58));
    // MSEG 2 purple.
    assert_eq!(rgb8(m2), (0xc3, 0x78, 0xff));
    // OOB clamps to MSEG 2 (the highest valid slot).
    assert_eq!(rgb8(mseg_color(99)), rgb8(m2));

    fn rgb8(c: Color) -> (u8, u8, u8) {
        let r = (c.red() * 255.0).round() as u8;
        let g = (c.green() * 255.0).round() as u8;
        let b = (c.blue() * 255.0).round() as u8;
        (r, g, b)
    }
}
```

- [ ] **Step 3: Run the test (expected: fails to compile)**

Run: `cargo nextest run -p multosis mseg_color_returns_the_three_slot_hues_and_clamps_oob 2>&1 | tail -15`
Expected: build error — `mseg_color` is undefined.

- [ ] **Step 4: Implement `mseg_color`**

Add to `multosis/src/editor.rs`, in a logical home near other small helpers (search for an existing `pub fn` near the top of the file — `WINDOW_WIDTH` is a good landmark):

```rust
/// The three MSEG slots each get their own colour: Amp = sky blue (the
/// existing accent), MSEG 1 = amber, MSEG 2 = purple. Used as the value
/// colour for `draw_mseg`/`draw_mseg_ghost`, the active fill of the MSEG
/// selector tab, and the modulation arc of any param dial driven by that
/// MSEG. Slot indices past 2 clamp to 2.
pub fn mseg_color(slot: usize) -> tiny_skia::Color {
    match slot.min(2) {
        0 => tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xff), // Amp — sky blue
        1 => tiny_skia::Color::from_rgba8(0xff, 0xc8, 0x58, 0xff), // MSEG 1 — amber
        _ => tiny_skia::Color::from_rgba8(0xc3, 0x78, 0xff, 0xff), // MSEG 2 — purple
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo nextest run -p multosis mseg_color_returns_the_three_slot_hues_and_clamps_oob 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Workspace lint + format**

Run: `cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: clean exit.

- [ ] **Step 7: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): add mseg_color palette (sky/amber/purple)

The shared lookup for the three MSEG slots' visual identity. Used in
subsequent commits by the selector tab, the active curve, the ghost
curves, and the param-dial modulation arc.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Active-colour override on `draw_stepped_selector`

**Files:**
- Modify: `tiny-skia-widgets/src/controls.rs` (signature + body of `draw_stepped_selector`)
- Modify: every caller of `draw_stepped_selector` in the workspace (pass `None` to preserve existing behaviour)
- Test: `tiny-skia-widgets/src/controls.rs` (new test in the file's `#[cfg(test)]` module)

- [ ] **Step 1: List every existing caller**

Run: `grep -rn "draw_stepped_selector" /home/mpd/git-sources/tract-plugin-pack --include='*.rs' | grep -v controls.rs`
Expected: 4–6 hits across the workspace. Note each file:line — you'll add `None` to each call site in Step 4.

- [ ] **Step 2: Write the failing test**

Append to `tiny-skia-widgets/src/controls.rs`'s test module:

```rust
#[test]
fn stepped_selector_active_colour_override_paints_a_different_active_segment() {
    use tiny_skia::{Color, Pixmap};
    let mut pm_default = Pixmap::new(120, 30).unwrap();
    let mut pm_custom = Pixmap::new(120, 30).unwrap();
    let mut tr = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    // Default (None) renders the active segment in accent blue.
    draw_stepped_selector(&mut pm_default, &mut tr, 0.0, 0.0, 120.0, 30.0, &["A", "B"], 0, None);
    // Magenta override paints a different active fill — probe the centre of
    // segment 0.
    draw_stepped_selector(
        &mut pm_custom,
        &mut tr,
        0.0,
        0.0,
        120.0,
        30.0,
        &["A", "B"],
        0,
        Some(Color::from_rgba8(0xff, 0x00, 0xff, 0xff)),
    );
    let px = |pm: &Pixmap| pm.pixels()[(15 * pm.width() + 30) as usize];
    let d = px(&pm_default);
    let c = px(&pm_custom);
    assert!(
        d.red() != c.red() || d.green() != c.green() || d.blue() != c.blue(),
        "active-colour override must paint a different fill"
    );
    // The override actually paints the requested hue.
    assert!(c.red() > 200 && c.blue() > 200 && c.green() < 100, "magenta probe ({}, {}, {})", c.red(), c.green(), c.blue());
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets stepped_selector_active_colour_override_paints_a_different_active_segment 2>&1 | tail -15`
Expected: build error — `draw_stepped_selector` expects 8 args, not 9.

- [ ] **Step 4: Extend the signature**

Edit `tiny-skia-widgets/src/controls.rs`. Replace the existing `draw_stepped_selector` with:

```rust
/// Draw a segmented control (stepped selector).
///
/// Each segment is an equal-width button; the one at `active_index` is
/// highlighted. When `active_color` is `Some`, that colour overrides the
/// default accent for the active segment's fill — used to coordinate the
/// MSEG selector with the active MSEG's identity colour. `None` preserves
/// the historical accent-blue behaviour.
#[allow(clippy::too_many_arguments)]
pub fn draw_stepped_selector(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    options: &[&str],
    active_index: usize,
    active_color: Option<Color>,
) {
    if options.is_empty() {
        return;
    }

    let seg_w = w / options.len() as f32;
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;

    for (i, &opt) in options.iter().enumerate() {
        let sx = x + i as f32 * seg_w;
        let is_active = i == active_index;

        let bg = if is_active {
            active_color.unwrap_or_else(color_accent)
        } else {
            color_control_bg()
        };
        let fg = if is_active {
            Color::from_rgba8(0x10, 0x10, 0x10, 0xff)
        } else {
            color_text()
        };

        draw_rect(pixmap, sx, y, seg_w, h, bg);
        draw_rect_outline(pixmap, sx, y, seg_w, h, color_border(), 1.0);

        let tw = text_renderer.text_width(opt, text_size);
        let tx = sx + (seg_w - tw) * 0.5;
        text_renderer.draw_text(pixmap, tx, text_y, opt, text_size, fg);
    }
}
```

- [ ] **Step 5: Update every existing caller**

For each call site found in Step 1, append `, None` to the argument list to preserve the existing accent-blue behaviour. Example (for an existing call like `draw_stepped_selector(pixmap, tr, x, y, w, h, &items, idx)`):

```rust
draw_stepped_selector(pixmap, tr, x, y, w, h, &items, idx, None);
```

Repeat for every caller.

- [ ] **Step 6: Run the new test and the full widget suite**

Run: `cargo nextest run -p tiny-skia-widgets 2>&1 | tail -15`
Expected: every test passes including the new one.

- [ ] **Step 7: Run the workspace build + clippy**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings`
Expected: clean exit. Any miss in Step 5 surfaces here as a "this function takes 9 arguments but 8 were supplied" error — fix and re-run.

- [ ] **Step 8: Commit**

```bash
git add tiny-skia-widgets/src/controls.rs
git add $(grep -rl "draw_stepped_selector" /home/mpd/git-sources/tract-plugin-pack --include='*.rs' | grep -v controls.rs)
git commit -m "feat(tiny-skia-widgets): per-call active-colour on draw_stepped_selector

New \`active_color: Option<Color>\` parameter overrides the default
accent fill on the active segment. None preserves historical behaviour;
every existing caller passes None. The MSEG selector tab will pass
\`Some(mseg_color(active_slot))\` so its tab tracks the active MSEG's
identity colour.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `value_color` on `draw_mseg`; thread `mseg_color` everywhere

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs` — `draw_mseg` gains `value_color: Color`; `draw_canvas` and any internal callees that draw the active curve / nodes / selection outline / hold marker switch from `color_accent()` to the threaded colour.
- Modify: `multosis/src/editor.rs` — pass `mseg_color(sel)` into `draw_mseg`; pass `mseg_color(m)` (each `m` ≠ `sel`) into the corresponding `draw_mseg_ghost` calls.
- Modify: `multosis/src/editor/effect_editor.rs` — selector tab's `draw_stepped_selector` call uses `Some(mseg_color(active_idx))`.
- Test: `tiny-skia-widgets/src/mseg/render.rs` and `multosis/src/editor.rs`.

- [ ] **Step 1: Write the failing widget-level test**

Append to `tiny-skia-widgets/src/mseg/render.rs`'s `#[cfg(test)]` module:

```rust
#[test]
fn draw_mseg_paints_value_color_on_the_curve() {
    use tiny_skia::{Color, Pixmap};
    use crate::mseg::{MsegData, MsegEditState};
    let mut pm = Pixmap::new(400, 200).unwrap();
    let mut tr = crate::text::TextRenderer::new(include_bytes!(
        "../../test_data/DejaVuSans.ttf"
    ));
    let mut data = MsegData::default();
    // A rising 0→1 ramp; the line passes through the canvas centre.
    let state = MsegEditState::new();
    // Magenta value colour — probe a known canvas pixel.
    draw_mseg(
        &mut pm,
        &mut tr,
        (0.0, 0.0, 400.0, 200.0),
        &data,
        &state,
        1.0,
        Color::from_rgba8(0xff, 0x00, 0xff, 0xff),
        None,
    );
    // Probe a pixel near the centre of the polyline. For a default 2-node
    // 0→1 ramp the curve passes through the canvas centre — fence a small
    // search window so antialiasing doesn't break the test.
    let layout = mseg_layout((0.0, 0.0, 400.0, 200.0), state.is_curve_only(), 1.0);
    let (cx, cy, cw, ch) = layout.plot;
    let probe_x = (cx + cw * 0.5) as u32;
    let probe_y_range = (cy as u32)..((cy + ch) as u32);
    let mut hit_magenta = false;
    for py in probe_y_range {
        let p = pm.pixels()[(py * pm.width() + probe_x) as usize];
        if p.red() > 180 && p.blue() > 180 && p.green() < 80 {
            hit_magenta = true;
            break;
        }
    }
    assert!(
        hit_magenta,
        "expected at least one magenta pixel on the active curve"
    );
    // Silence the never-used warning while `data` is held for layout.
    let _ = &mut data;
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets draw_mseg_paints_value_color_on_the_curve 2>&1 | tail -15`
Expected: build error — `draw_mseg` doesn't accept those extra args.

- [ ] **Step 3: Extend `draw_mseg`**

Edit `tiny-skia-widgets/src/mseg/render.rs`. Update the signature:

```rust
/// Draw the whole MSEG widget into `rect`. Composes the marker lane (full
/// mode only), the canvas (grid + curve + nodes), the control strip, and any
/// open dropdown popup (drawn last so it overlays everything else).
///
/// `value_color` is used for the curve stroke, node fills, the marquee
/// outline, hold-mode markers, and the selected-node outline. Callers
/// pass the slot's identity colour so the visual matches the MSEG
/// selector tab.
///
/// `node_tooltip`, when `Some`, is the (node_index, formatted_text) pair
/// to render as a small floating tooltip above the indicated node — the
/// caller computes the text since the parameter-mapped value formatting
/// depends on multosis-level state the widget crate doesn't see.
#[allow(clippy::too_many_arguments)]
pub fn draw_mseg(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    rect: (f32, f32, f32, f32),
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
    value_color: tiny_skia::Color,
    node_tooltip: Option<(usize, &str)>,
) {
    let layout = mseg_layout(rect, state.is_curve_only(), scale);
    draw_canvas(pixmap, &layout, data, state, scale, value_color);
    draw_marker_lane(pixmap, &layout, data, state, scale, value_color);
    draw_strip(pixmap, text_renderer, &layout, data, state, scale);

    if let Some((idx, text)) = node_tooltip {
        draw_node_tooltip(pixmap, text_renderer, &layout, data, idx, text, scale);
    }

    let window_size = (rect.0 + rect.2, rect.1 + rect.3);
    if state.dropdown_is_open_for(StripId::TimeGrid) {
        let grid_refs = state.grid_label_refs();
        draw_dropdown_popup(
            pixmap,
            text_renderer,
            state.dropdown_state(),
            &grid_refs,
            window_size,
        );
    } else {
        draw_dropdown_popup(
            pixmap,
            text_renderer,
            state.dropdown_state(),
            style_items(),
            window_size,
        );
    }
}
```

- [ ] **Step 4: Update `draw_canvas`, `draw_marker_lane`, and any internal helpers**

Each function that currently calls `color_accent()` for the polyline/node/selection-outline/hold-marker now takes `value_color: Color` and uses that instead. Concrete touches inside `draw_canvas` (consult the file — the existing line is around `draw_line(pixmap, px, py, x, y, color_accent())` and `let fill = ...0x4f, 0xc3, 0xf7, 0x30...` for the selection rect):

- Polyline stroke: pass `value_color` instead of `color_accent()`.
- Selection outline: pass `value_color`.
- Selection translucent fill: replace the hard-coded `0x4f, 0xc3, 0xf7, 0x30` with `Color::from_rgba(value_color.red(), value_color.green(), value_color.blue(), 0x30 as f32 / 255.0)` (or build the same via `Color::from_rgba8` with `(r,g,b)` from `value_color`'s channels).
- Node fills inside `draw_canvas` (the small circles for each active node): switch from `color_accent()` to `value_color`.
- In `draw_marker_lane`, the existing `mark(pixmap, i, color_accent())` calls for the `HoldMode::Sustain(i)` branch become `mark(pixmap, i, value_color)`. Leave the `color_border()`-coloured Loop start/end markers alone (border-grey is a separate semantic, not "identity").

Add the helper definition near the bottom of the rendering helpers, before the existing `pub fn draw_mseg_ghost`:

```rust
/// Render a small floating tooltip above (or below, when the node is near
/// the top of the canvas) node `idx`, showing `text`. Used by `draw_mseg`
/// when the hover-target is a node.
fn draw_node_tooltip(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    layout: &MsegLayout,
    data: &MsegData,
    idx: usize,
    text: &str,
    scale: f32,
) {
    if idx >= data.node_count {
        return;
    }
    let n = data.nodes[idx];
    let (px_, py_, pw, ph) = layout.plot;
    let cx = px_ + n.time * pw;
    let cy = py_ + (1.0 - n.value) * ph;
    let text_size = (10.0 * scale).max(10.0);
    let tw = text_renderer.text_width(text, text_size);
    let pad_x = 6.0 * scale;
    let pad_y = 3.0 * scale;
    let box_w = tw + 2.0 * pad_x;
    let box_h = text_size + 2.0 * pad_y;
    let gap = 10.0 * scale; // distance from node centre to tooltip edge
    // Default above; flip below when the node is in the top quarter.
    let above = n.value < 0.75;
    let box_y = if above { cy - gap - box_h } else { cy + gap };
    let mut box_x = cx - box_w * 0.5;
    // Keep the tooltip inside the canvas horizontally.
    if box_x < px_ {
        box_x = px_;
    }
    if box_x + box_w > px_ + pw {
        box_x = px_ + pw - box_w;
    }
    draw_rect(pixmap, box_x, box_y, box_w, box_h, color_control_bg());
    draw_rect_outline(pixmap, box_x, box_y, box_w, box_h, color_border(), 1.0);
    text_renderer.draw_text(
        pixmap,
        box_x + pad_x,
        box_y + pad_y + text_size,
        text,
        text_size,
        color_text(),
    );
}
```

If `draw_rect_outline` isn't already imported at the top of the file, add it to the existing `use` block.

- [ ] **Step 5: Update every workspace caller of `draw_mseg`**

Find them:

Run: `grep -rn "widgets::mseg::draw_mseg\b\|mseg::draw_mseg\b" /home/mpd/git-sources/tract-plugin-pack --include='*.rs' | grep -v ghost`
Expected: at least the one call in `multosis/src/editor.rs` near line 777.

For each call, append `, mseg_color(sel), None` (replacing `sel` with whichever variable already names the active MSEG index in that scope — in `multosis/src/editor.rs` it's `sel`). The `None` placeholder is the tooltip; Task 4 will replace it.

Concretely, the multosis call becomes:

```rust
widgets::mseg::draw_mseg(
    &mut self.surface.pixmap,
    &mut self.text_renderer,
    lay.mseg_pane,
    &modu.msegs[sel],
    &self.mseg_edit,
    self.scale_factor,
    mseg_color(sel),
    None,
);
```

- [ ] **Step 6: Update every workspace caller of `draw_mseg_ghost`**

Find them:

Run: `grep -rn "draw_mseg_ghost\b" /home/mpd/git-sources/tract-plugin-pack --include='*.rs' | grep -v render.rs`
Expected: the loop in `multosis/src/editor.rs` (the `for m in 0..3 { if m != sel { ... } }`).

`draw_mseg_ghost` takes `color: u32` (packed RGBA). Build it from the per-ghost MSEG colour with a small inline closure (or update the call site directly):

```rust
for m in 0..3 {
    if m != sel {
        let c = mseg_color(m);
        // Pack the slot colour with ghost alpha (~0x60 — the existing value).
        let r = (c.red() * 255.0).round() as u32;
        let g = (c.green() * 255.0).round() as u32;
        let b = (c.blue() * 255.0).round() as u32;
        let packed = (r << 24) | (g << 16) | (b << 8) | 0x60;
        widgets::mseg::draw_mseg_ghost(
            &mut self.surface.pixmap,
            lay.mseg_pane,
            &modu.msegs[m],
            &self.mseg_edit,
            self.scale_factor,
            packed,
        );
    }
}
```

- [ ] **Step 7: Update the selector tab to use the active MSEG's colour**

Edit `multosis/src/editor/effect_editor.rs::draw_modulation_controls`. Find the existing call:

```rust
widgets::controls::draw_stepped_selector(
    pixmap,
    tr,
    lay.mseg_selector.0,
    lay.mseg_selector.1,
    lay.mseg_selector.2,
    lay.mseg_selector.3,
    &["Amp", "MSEG 1", "MSEG 2"],
    selected_mseg.min(2),
);
```

Replace with the `Some(...)` variant:

```rust
widgets::controls::draw_stepped_selector(
    pixmap,
    tr,
    lay.mseg_selector.0,
    lay.mseg_selector.1,
    lay.mseg_selector.2,
    lay.mseg_selector.3,
    &["Amp", "MSEG 1", "MSEG 2"],
    selected_mseg.min(2),
    Some(crate::editor::mseg_color(selected_mseg.min(2))),
);
```

(The other `draw_stepped_selector` call in that file — the sync-mode selector — already passes `None` per Task 2 step 5; leave it.)

- [ ] **Step 8: Run all tests + clippy + fmt**

Run: `cargo nextest run -p multosis -p tiny-skia-widgets 2>&1 | tail -20`
Expected: all green including `draw_mseg_paints_value_color_on_the_curve`.

Run: `cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs multosis/src/editor.rs multosis/src/editor/effect_editor.rs
git commit -m "feat(multosis): per-MSEG colour identity for selector + curves

draw_mseg now takes a value_color used for the curve stroke, node fills,
selection outline, and the active hold marker. draw_mseg_ghost's color
argument is fed from mseg_color(slot) so each ghost paints in its own
identity hue at the existing ghost alpha. The MSEG selector tab passes
mseg_color(active_slot) as the active-segment override, so the tab
matches the curve.

Tooltip plumbing on draw_mseg is in place but unused — Task 4 fills it
in.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Hover-node tooltip — text formatting + plumbing

**Files:**
- Modify: `multosis/src/editor.rs` — new method `MultosisWindow::node_tooltip_text(&self, sel: usize) -> Option<(usize, String)>` returning `(node_idx, text)` for the currently hovered node, given the selected MSEG. The text is formatted per Amp / assignable / no-target rules. Passes the result into the `draw_mseg` call from Task 3.
- Test: `multosis/src/editor.rs` for the formatting rules (pure data — no rendering involved).

- [ ] **Step 1: Identify the hover-node API on `MsegEditState`**

Run: `grep -n "fn hover\|hover_node\|pub fn hovered" /home/mpd/git-sources/tract-plugin-pack/tiny-skia-widgets/src/mseg/editor.rs | head`
Expected: a getter that returns `Option<usize>` (or directly accessible state). If none exists, add a public getter:

```rust
/// Index of the node currently under the cursor, if any.
pub fn hovered_node(&self) -> Option<usize> {
    match self.hover {
        Some(HoverTarget::Node(i)) => Some(i),
        _ => None,
    }
}
```

(Inspect `HoverTarget` definitions in that file to confirm the variant name; adjust if it's actually `Hover::Node` or similar.)

- [ ] **Step 2: Write the failing test for `node_tooltip_text`**

Append to `multosis/src/editor.rs`'s test module:

```rust
#[test]
fn node_tooltip_text_for_amp_returns_db_readout() {
    use widgets::MsegData;
    // Construct an Amp tooltip text directly via the formatter the editor
    // uses internally — exposed as `mseg_node_tooltip_text`.
    let mut data = MsegData::default();
    data.nodes[1].value = 1.0;
    // Slot 0 = Amp. node 1 has value 1.0 → 0.0 dB.
    let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
    assert_eq!(text, "0.0 dB");

    data.nodes[1].value = 0.5;
    let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
    // 20·log10(0.5) ≈ -6.02 dB.
    assert!(text.starts_with("-6.0"), "expected ~-6.0 dB, got {text}");

    data.nodes[1].value = 0.00001;
    let text = mseg_node_tooltip_text(0, &data, 1, None, 0.0, None);
    assert_eq!(text, "-∞ dB", "below floor should render as -∞ dB");
}

#[test]
fn node_tooltip_text_for_assignable_no_target_returns_raw_level() {
    use widgets::MsegData;
    let mut data = MsegData::default();
    data.nodes[1].value = 0.742;
    let text = mseg_node_tooltip_text(1, &data, 1, None, 0.0, None);
    assert_eq!(text, "0.74");
}
```

(If you also want a coverage point for assignable WITH a target, see Step 6 — wire that test once the helper takes the `ParamSpec` argument.)

- [ ] **Step 3: Run to verify the tests fail**

Run: `cargo nextest run -p multosis node_tooltip_text 2>&1 | tail -15`
Expected: build errors — function doesn't exist.

- [ ] **Step 4: Implement `mseg_node_tooltip_text` as a module-level helper**

Add to `multosis/src/editor.rs`:

```rust
/// Format the tooltip text shown when a node is hovered. Slot `0` is Amp
/// (dB readout); `1` and `2` are assignable MSEGs that map through their
/// target's `ParamSpec` if `spec` is `Some`, or fall back to the raw node
/// level if no target is set.
///
/// `base` and `depth` are the assignable MSEG's current base param value
/// and depth setting — both ignored when `slot == 0` or `spec.is_none()`.
pub fn mseg_node_tooltip_text(
    slot: usize,
    data: &tiny_skia_widgets::MsegData,
    node_idx: usize,
    spec: Option<crate::effects::ParamSpec>,
    base: f32,
    depth_polarity: Option<(f32, tiny_skia_widgets::Polarity)>,
) -> String {
    if node_idx >= data.node_count {
        return String::new();
    }
    let value = data.nodes[node_idx].value;
    if slot == 0 {
        const FLOOR_DB: f32 = -80.0;
        if value <= 1e-4 {
            return "-∞ dB".to_string();
        }
        let db = 20.0 * value.max(1e-4).log10();
        let db = db.max(FLOOR_DB);
        return format!("{db:.1} dB");
    }
    match (spec, depth_polarity) {
        (Some(spec), Some((depth, polarity))) => {
            let v = crate::modulation::assignable_value(value, base, depth, spec, polarity);
            crate::effects::format_value(v, spec.format)
        }
        _ => format!("{value:.2}"),
    }
}
```

If you need `tiny_skia_widgets::Polarity` and `tiny_skia_widgets::MsegData` not already re-exported, double-check via `grep -n "pub use.*Polarity\|pub use.*MsegData" tiny-skia-widgets/src/lib.rs` — they should be.

- [ ] **Step 5: Run the formatter tests, verify pass**

Run: `cargo nextest run -p multosis node_tooltip_text 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Wire the formatter into the editor's `draw_mseg` call**

In `multosis/src/editor.rs`, just before the `draw_mseg` call updated in Task 3, derive the tooltip pair:

```rust
let tooltip: Option<(usize, String)> = self.mseg_edit.hovered_node().map(|idx| {
    let (spec, base, depth_polarity) = if sel == 0 {
        (None, 0.0, None)
    } else {
        let k = sel - 1;
        let target = modu.targets[k];
        let depth = modu.depths[k];
        let polarity = modu.msegs[sel].polarity;
        let spec = target.and_then(|t| {
            let inst = crate::effects::EffectInstance::new(track.kind);
            inst.parameters().get(t).copied()
        });
        let base = target.map(|t| track.params[t]).unwrap_or(0.0);
        (spec, base, Some((depth, polarity)))
    };
    let text = mseg_node_tooltip_text(sel, &modu.msegs[sel], idx, spec, base, depth_polarity);
    (idx, text)
});
// `draw_mseg` wants `Option<(usize, &str)>`; borrow the owned String.
let tooltip_ref = tooltip.as_ref().map(|(i, t)| (*i, t.as_str()));
widgets::mseg::draw_mseg(
    &mut self.surface.pixmap,
    &mut self.text_renderer,
    lay.mseg_pane,
    &modu.msegs[sel],
    &self.mseg_edit,
    self.scale_factor,
    mseg_color(sel),
    tooltip_ref,
);
```

- [ ] **Step 7: Run the full multosis + widget suites and the workspace build**

Run: `cargo nextest run -p multosis -p tiny-skia-widgets 2>&1 | tail -15 && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add multosis/src/editor.rs tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(multosis): hover-node tooltip on MSEG editor

Hovering a node renders a small tooltip above (or below, when near the
top) showing the parameter value that node would map to. Amp shows a dB
readout; assignable MSEGs format through the target's ParamSpec via the
existing assignable_value + format_value plumbing; assignable MSEGs
with no target show the raw 0..1 level.

The formatting is multosis-side (the widget crate doesn't see
ParamSpec); the widget crate exposes draw_node_tooltip and the new
hovered_node accessor on MsegEditState.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `MsegHit::SelectedNode(i)` hit-test variant

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs` — extend `MsegHit` with `SelectedNode(usize)`. Extend the hit-tester to emit `SelectedNode` for selection-membership hits.
- Modify: `tiny-skia-widgets/src/mseg/editor.rs` — `on_mouse_down` and `on_right_click` handle both `Node(i)` and `SelectedNode(i)` for their existing behaviours.
- Test: in `tiny-skia-widgets/src/mseg/editor.rs`.

- [ ] **Step 1: Write the failing test**

Append to the existing test module in `tiny-skia-widgets/src/mseg/editor.rs`:

```rust
#[test]
fn hit_test_returns_selected_node_when_click_lands_on_a_selected_node() {
    use crate::mseg::render::{mseg_hit_test, mseg_layout, MsegHit};
    use crate::mseg::{MsegData, MsegEditState};
    let mut data = MsegData::default();
    // Insert a middle node so we have something selectable that isn't an
    // endpoint.
    let _ = data.insert_node(0.5, 0.5);
    let mut state = MsegEditState::new();
    let rect = (0.0, 0.0, 400.0, 200.0);
    let layout = mseg_layout(rect, false, 1.0);
    // Find the canvas-space pixel of the middle node.
    let (px, py, pw, ph) = layout.plot;
    let n = data.nodes[1];
    let nx = px + n.time * pw;
    let ny = py + (1.0 - n.value) * ph;
    // Before selection: a hit on node 1 reports Node(1).
    let h_before = mseg_hit_test(&layout, &data, false, 1.0, nx, ny);
    assert!(matches!(h_before, MsegHit::Node(1)), "got {:?}", h_before);
    // Select node 1.
    state.select_only(1);
    // Now the same click reports SelectedNode(1).
    let h_after = mseg_hit_test_with_selection(&layout, &data, false, 1.0, nx, ny, &state);
    assert!(
        matches!(h_after, MsegHit::SelectedNode(1)),
        "got {:?}",
        h_after
    );
}
```

You'll add a new top-level `mseg_hit_test_with_selection` function in Step 3 — it wraps the existing hit-tester. We expose it (instead of changing `mseg_hit_test`'s signature) so the simpler API stays available for tests and so the new selection-aware version is opt-in at each call site.

- [ ] **Step 2: Run to verify the test fails**

Run: `cargo nextest run -p tiny-skia-widgets hit_test_returns_selected_node 2>&1 | tail -15`
Expected: build error — `MsegHit::SelectedNode` doesn't exist; `mseg_hit_test_with_selection` doesn't exist.

- [ ] **Step 3: Extend `MsegHit` and add the selection-aware wrapper**

Edit `tiny-skia-widgets/src/mseg/render.rs`. Add the variant:

```rust
pub enum MsegHit {
    /// On the node dot at this index — node is NOT in the active selection.
    Node(usize),
    /// On the node dot at this index — node IS in the active selection.
    /// Right-click on a SelectedNode opens the transform menu; left-click
    /// behaves identically to Node(i).
    SelectedNode(usize),
    Tension(usize),
    Canvas,
    Randomize,
    Strip,
    MarkerLane,
    None,
}
```

Add the wrapper, immediately after `mseg_hit_test`:

```rust
/// Selection-aware wrapper around `mseg_hit_test`. Identical to the bare
/// version except that a node-hit is reported as `SelectedNode(i)` when
/// the node is part of the editor's current selection. Used by the
/// editor's mouse handlers; the bare version is kept for code paths that
/// don't need to discriminate (e.g. plain hover).
pub fn mseg_hit_test_with_selection(
    layout: &MsegLayout,
    data: &MsegData,
    curve_only: bool,
    scale: f32,
    x: f32,
    y: f32,
    state: &crate::mseg::editor::MsegEditState,
) -> MsegHit {
    let h = mseg_hit_test(layout, data, curve_only, scale, x, y);
    match h {
        MsegHit::Node(i) if state.is_node_selected(i) => MsegHit::SelectedNode(i),
        other => other,
    }
}
```

The `crate::mseg::editor::MsegEditState` path requires the module to be a sibling — verify by inspecting `tiny-skia-widgets/src/mseg/mod.rs`'s `pub mod editor;` line.

- [ ] **Step 4: Treat `SelectedNode` identically to `Node` in existing handlers**

In `tiny-skia-widgets/src/mseg/editor.rs`, find every `match` on `MsegHit` (or `matches!(... , MsegHit::...)`) inside `on_mouse_down`, `on_mouse_up`, `on_mouse_move`, and `on_right_click`. For every arm currently handling `MsegHit::Node(i)`, mirror it for `MsegHit::SelectedNode(i)` — the new variant is for routing only; the per-event behaviour for left-clicks/drags/hovers must be byte-for-byte identical.

A clean refactor: collapse the matches with `MsegHit::Node(i) | MsegHit::SelectedNode(i)` where the existing behaviour is preserved verbatim.

In `on_right_click`'s `matches!(... , MsegHit::Canvas | MsegHit::Tension(_) | MsegHit::Node(_))` early-return guard, add `MsegHit::SelectedNode(_)` so the guard continues to admit selected-node clicks (the new behaviour comes in Task 7; this task just opens the routing).

- [ ] **Step 5: Run the new test + the full widget suite**

Run: `cargo nextest run -p tiny-skia-widgets 2>&1 | tail -20`
Expected: every test green, including the new hit-test test and the existing `right_click_toggles_segment_stepped`.

- [ ] **Step 6: Workspace lint + format**

Run: `cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(tiny-skia-widgets): MsegHit::SelectedNode + selection-aware hit-test

A new MsegHit variant distinguishes clicks that land on a node already
in the editor's selection from clicks that land on an unselected node.
The selection-aware wrapper mseg_hit_test_with_selection consults the
editor state to choose between Node and SelectedNode for the same pixel.

All existing handlers (mouse-down/up/move, right-click stepped toggle)
treat the two variants identically — this commit only widens the
routing channel. The transform menu in Task 7 will branch on the
distinction.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Compress / expand transform methods on `MsegEditState`

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs` — four new methods plus a small shared helper.
- Test: same file's test module.

- [ ] **Step 1: Write the failing tests for all four transforms**

Append to the test module:

```rust
#[test]
fn compress_values_pulls_selected_values_25pct_toward_mean() {
    use crate::mseg::{MsegData, MsegEditState};
    let mut data = MsegData::default();
    let i1 = data.insert_node(0.25, 0.0).unwrap();
    let i2 = data.insert_node(0.5, 0.4).unwrap();
    let i3 = data.insert_node(0.75, 1.0).unwrap();
    let mut state = MsegEditState::new();
    state.select_only(i1);
    state.toggle_selected_for_test(i2); // make pub(crate) for tests, or use a public add API
    state.toggle_selected_for_test(i3);
    let mean = (0.0 + 0.4 + 1.0) / 3.0; // ≈ 0.4667
    let exp_i1 = mean + (0.0 - mean) * 0.75;
    let exp_i2 = mean + (0.4 - mean) * 0.75;
    let exp_i3 = mean + (1.0 - mean) * 0.75;
    state.compress_values(&mut data);
    assert!((data.nodes[i1].value - exp_i1).abs() < 1e-5);
    assert!((data.nodes[i2].value - exp_i2).abs() < 1e-5);
    assert!((data.nodes[i3].value - exp_i3).abs() < 1e-5);
}

#[test]
fn expand_values_pushes_selected_values_25pct_away_from_mean_and_clamps() {
    use crate::mseg::{MsegData, MsegEditState};
    let mut data = MsegData::default();
    let i1 = data.insert_node(0.25, 0.1).unwrap();
    let i2 = data.insert_node(0.75, 0.9).unwrap();
    let mut state = MsegEditState::new();
    state.select_only(i1);
    state.toggle_selected_for_test(i2);
    let mean = 0.5;
    // 0.1 → 0.5 + (0.1 - 0.5) * 1.25 = 0.0; 0.9 → 1.0 — both at the
    // clamp boundary, exact.
    state.expand_values(&mut data);
    assert!((data.nodes[i1].value - 0.0).abs() < 1e-5);
    assert!((data.nodes[i2].value - 1.0).abs() < 1e-5);
}

#[test]
fn compress_times_pulls_selected_times_toward_mean_and_leaves_anchors_alone() {
    use crate::mseg::{MsegData, MsegEditState};
    let mut data = MsegData::default();
    let i1 = data.insert_node(0.2, 0.5).unwrap();
    let i2 = data.insert_node(0.6, 0.5).unwrap();
    let mut state = MsegEditState::new();
    // Select the anchor (node 0) AND the two middle nodes. The anchor
    // contributes its value to mean computation? No — for TIME, the
    // anchor's time is locked, so it's excluded from the time mean and
    // not mutated.
    state.select_only(0);
    state.toggle_selected_for_test(i1);
    state.toggle_selected_for_test(i2);
    let t1_0 = data.nodes[i1].time;
    let t2_0 = data.nodes[i2].time;
    let mean = (t1_0 + t2_0) / 2.0;
    state.compress_times(&mut data);
    // Anchor untouched.
    assert_eq!(data.nodes[0].time, 0.0);
    // Middle nodes pulled toward the (non-anchor) time mean.
    assert!((data.nodes[i1].time - (mean + (t1_0 - mean) * 0.75)).abs() < 1e-5);
    assert!((data.nodes[i2].time - (mean + (t2_0 - mean) * 0.75)).abs() < 1e-5);
}

#[test]
fn expand_times_is_a_noop_on_a_single_node_selection() {
    use crate::mseg::{MsegData, MsegEditState};
    let mut data = MsegData::default();
    let i1 = data.insert_node(0.4, 0.5).unwrap();
    let t0 = data.nodes[i1].time;
    let mut state = MsegEditState::new();
    state.select_only(i1);
    state.expand_times(&mut data);
    // Mean of one node == that node's time → delta is zero → no move.
    assert_eq!(data.nodes[i1].time, t0);
}
```

If the existing API doesn't have `toggle_selected_for_test`, add it now as a `#[cfg(test)] pub(crate) fn` on `MsegEditState`:

```rust
#[cfg(test)]
pub(crate) fn toggle_selected_for_test(&mut self, i: usize) {
    self.toggle_selected(i);
}
```

(Or expose `toggle_selected` publicly; either works — pick the smaller diff.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p tiny-skia-widgets compress_values_pulls expand_values_pushes compress_times_pulls expand_times_is 2>&1 | tail -25`
Expected: build errors — methods don't exist.

- [ ] **Step 3: Implement the four transform methods**

Append to the `impl MsegEditState` block in `tiny-skia-widgets/src/mseg/editor.rs` (near the other public mutation methods):

```rust
/// Step multiplier for compress (any axis): pulls toward mean by 25 %.
const COMPRESS_K: f32 = 0.75;
/// Step multiplier for expand (any axis): pushes from mean by 25 %.
const EXPAND_K: f32 = 1.25;

impl MsegEditState {
    /// Pull each selected node's value 25 % toward the selection's mean value.
    pub fn compress_values(&mut self, data: &mut MsegData) -> MsegEdit {
        self.scale_values_around_mean(data, COMPRESS_K)
    }

    /// Push each selected node's value 25 % away from the selection's mean
    /// value, clamping to [0, 1].
    pub fn expand_values(&mut self, data: &mut MsegData) -> MsegEdit {
        self.scale_values_around_mean(data, EXPAND_K)
    }

    /// Pull each selected node's time 25 % toward the selection's mean
    /// (anchor) time. Anchor nodes (index 0 and the last) never move.
    pub fn compress_times(&mut self, data: &mut MsegData) -> MsegEdit {
        self.scale_times_around_mean(data, COMPRESS_K)
    }

    /// Push each selected node's time 25 % away from the selection's mean
    /// time. Anchor nodes never move; times stay strictly within the gap
    /// to their immediate unselected neighbour.
    pub fn expand_times(&mut self, data: &mut MsegData) -> MsegEdit {
        self.scale_times_around_mean(data, EXPAND_K)
    }

    fn scale_values_around_mean(&mut self, data: &mut MsegData, k: f32) -> MsegEdit {
        let n = data.node_count;
        let selected: Vec<usize> = (0..n).filter(|&i| self.is_node_selected(i)).collect();
        if selected.is_empty() {
            return MsegEdit::None;
        }
        let mean: f32 = selected.iter().map(|&i| data.nodes[i].value).sum::<f32>() / selected.len() as f32;
        for &i in &selected {
            let v = data.nodes[i].value;
            let nv = (mean + (v - mean) * k).clamp(0.0, 1.0);
            data.nodes[i].value = nv;
        }
        data.debug_assert_valid();
        MsegEdit::Changed
    }

    fn scale_times_around_mean(&mut self, data: &mut MsegData, k: f32) -> MsegEdit {
        let n = data.node_count;
        let last = n.saturating_sub(1);
        // Anchor nodes (index 0, index last) are never moved on the time
        // axis and never contribute to the time mean.
        let movable: Vec<usize> = (0..n)
            .filter(|&i| self.is_node_selected(i) && i != 0 && i != last)
            .collect();
        if movable.is_empty() {
            return MsegEdit::None;
        }
        let mean: f32 = movable.iter().map(|&i| data.nodes[i].time).sum::<f32>() / movable.len() as f32;
        let gap = MsegData::MIN_NODE_GAP;
        // Apply per-node, capping each result to the gap-respecting bounds
        // of its CURRENT immediate neighbours. Iterate in order so each
        // step sees the just-written previous neighbour.
        for &i in &movable {
            let t = data.nodes[i].time;
            let nt_raw = mean + (t - mean) * k;
            let lo = data.nodes[i - 1].time + gap;
            let hi = data.nodes[i + 1].time - gap;
            data.nodes[i].time = nt_raw.clamp(lo, hi);
        }
        data.debug_assert_valid();
        MsegEdit::Changed
    }
}
```

The `Vec` allocations are GUI-thread work in response to a menu click; not on the audio path, so this is fine.

- [ ] **Step 4: Run the new tests, verify pass**

Run: `cargo nextest run -p tiny-skia-widgets compress_values_pulls expand_values_pushes compress_times_pulls expand_times_is 2>&1 | tail -15`
Expected: all four PASS.

- [ ] **Step 5: Run the whole widget suite + clippy + fmt**

Run: `cargo nextest run -p tiny-skia-widgets 2>&1 | tail -10 && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs
git commit -m "feat(tiny-skia-widgets): MSEG compress/expand transform methods

Four new methods on MsegEditState: compress_values, expand_values,
compress_times, expand_times. Each applies the closed-form
new = mean + (old - mean) · k to the selected nodes' value or time
coordinate, with k = 0.75 for compress and k = 1.25 for expand.

Value transforms clamp to [0, 1] per node. Time transforms exclude
anchor nodes (index 0 and the last) from both the mean and the
mutation, and clamp each result to the gap-respecting bounds of its
immediate neighbours so the time invariant (strictly ascending) holds.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Right-click transform menu — `StripId::Transform` + dispatch

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/editor.rs` — `StripId::Transform`; `on_right_click` branches on `MsegHit::SelectedNode(_)`; selecting an item dispatches to the appropriate transform method.
- Modify: `tiny-skia-widgets/src/mseg/render.rs` — extend the dropdown-popup drawing so it knows the transform menu's items.
- Test: in `tiny-skia-widgets/src/mseg/editor.rs`.

- [ ] **Step 1: Write the failing test**

Append to the test module:

```rust
#[test]
fn right_click_on_selected_node_opens_transform_menu() {
    use crate::mseg::render::{mseg_layout, MsegHit};
    use crate::mseg::{MsegData, MsegEditState, StripId};
    let mut data = MsegData::default();
    let i = data.insert_node(0.5, 0.5).unwrap();
    let mut state = MsegEditState::new();
    state.select_only(i);
    let rect = (0.0, 0.0, 400.0, 200.0);
    let layout = mseg_layout(rect, false, 1.0);
    let (px, py, pw, ph) = layout.plot;
    let n = data.nodes[i];
    let nx = px + n.time * pw;
    let ny = py + (1.0 - n.value) * ph;
    let _ = state.on_right_click(nx, ny, &mut data, rect, 1.0);
    assert!(
        state.dropdown_is_open_for(StripId::Transform),
        "transform menu should open"
    );
    // The segment-stepped flag should NOT have changed.
    assert!(!data.nodes[0].stepped);
}

#[test]
fn right_click_on_unselected_node_still_toggles_segment_stepped() {
    use crate::mseg::{MsegData, MsegEditState, StripId};
    let mut data = MsegData::default();
    let i = data.insert_node(0.5, 0.5).unwrap();
    let mut state = MsegEditState::new();
    // No selection.
    let rect = (0.0, 0.0, 400.0, 200.0);
    let layout = crate::mseg::render::mseg_layout(rect, false, 1.0);
    let (px, py, pw, ph) = layout.plot;
    let n = data.nodes[i];
    let nx = px + n.time * pw;
    let ny = py + (1.0 - n.value) * ph;
    let prev = data.nodes[i].stepped;
    let _ = state.on_right_click(nx, ny, &mut data, rect, 1.0);
    assert!(!state.dropdown_is_open_for(StripId::Transform));
    assert_ne!(data.nodes[i].stepped, prev, "stepped should have toggled");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets right_click_on_selected_node_opens right_click_on_unselected_node 2>&1 | tail -15`
Expected: build error — `StripId::Transform` doesn't exist.

- [ ] **Step 3: Add the `Transform` discriminant**

Edit `tiny-skia-widgets/src/mseg/editor.rs`:

```rust
pub enum StripId {
    TimeGrid,
    Style,
    /// The right-click-on-selected-node transform menu.
    Transform,
}
```

- [ ] **Step 4: Branch `on_right_click` on `SelectedNode` and open the menu**

Replace the body of `MsegEditState::on_right_click` with:

```rust
pub fn on_right_click(
    &mut self,
    x: f32,
    y: f32,
    data: &mut MsegData,
    rect: (f32, f32, f32, f32),
    scale: f32,
) -> Option<MsegEdit> {
    use crate::mseg::render::{mseg_hit_test_with_selection, mseg_layout, x_to_phase, MsegHit};
    let layout = mseg_layout(rect, self.curve_only, scale);
    let hit = mseg_hit_test_with_selection(&layout, data, self.curve_only, scale, x, y, self);
    // Selected-node right-click opens the transform menu instead of the
    // segment-stepped toggle.
    if let MsegHit::SelectedNode(_) = hit {
        let window_size = (rect.0 + rect.2, rect.1 + rect.3);
        self.dropdown.open(
            StripId::Transform,
            // Anchor a 1x1 px rect at the click — `DropdownState::open`
            // uses the anchor to position the popup; a degenerate rect
            // gives "open at this point".
            (x, y, 1.0, 1.0),
            transform_menu_items(),
            0,
            false,
            window_size,
        );
        return None;
    }
    if !matches!(
        hit,
        MsegHit::Canvas | MsegHit::Tension(_) | MsegHit::Node(_)
    ) {
        return None;
    }
    let phase = x_to_phase(&layout, x);
    let seg = {
        let a = data.active();
        let mut seg = 0;
        for (i, n) in a.iter().enumerate().take(data.node_count - 1) {
            if n.time <= phase {
                seg = i;
            } else {
                break;
            }
        }
        seg
    };
    data.nodes[seg].stepped = !data.nodes[seg].stepped;
    data.debug_assert_valid();
    Some(MsegEdit::Changed)
}
```

Add the module-level item list near the other static label arrays:

```rust
pub(crate) fn transform_menu_items() -> &'static [&'static str] {
    &[
        "Compress values",
        "Expand values",
        "Compress times",
        "Expand times",
    ]
}
```

- [ ] **Step 5: Dispatch on dropdown selection**

In `MsegEditState::on_mouse_down`'s existing `DropdownEvent::Selected(StripId::Style, idx)` handler (or wherever `DropdownEvent::Selected` is matched), add a sibling arm for `StripId::Transform`:

```rust
Some(DropdownEvent::Selected(StripId::Transform, idx)) => {
    let edit = match idx {
        0 => self.compress_values(data),
        1 => self.expand_values(data),
        2 => self.compress_times(data),
        _ => self.expand_times(data),
    };
    return Some(edit);
}
```

(The exact match shape will depend on the surrounding code — check existing arms first; the variant name `DropdownEvent::Selected(StripId, usize)` is established by Task 1's grep.)

- [ ] **Step 6: Wire `draw_strip` / `draw_dropdown_popup` to render the transform menu when it's open**

In `tiny-skia-widgets/src/mseg/render.rs::draw_mseg`, the existing if/else over `StripId::TimeGrid` / `else` (Style) needs a third arm for `Transform`:

```rust
if state.dropdown_is_open_for(StripId::TimeGrid) {
    let grid_refs = state.grid_label_refs();
    draw_dropdown_popup(pixmap, text_renderer, state.dropdown_state(), &grid_refs, window_size);
} else if state.dropdown_is_open_for(StripId::Transform) {
    draw_dropdown_popup(
        pixmap,
        text_renderer,
        state.dropdown_state(),
        crate::mseg::editor::transform_menu_items(),
        window_size,
    );
} else {
    draw_dropdown_popup(pixmap, text_renderer, state.dropdown_state(), style_items(), window_size);
}
```

- [ ] **Step 7: Run the new tests + the entire widget suite**

Run: `cargo nextest run -p tiny-skia-widgets 2>&1 | tail -20`
Expected: all green including the new ones; `right_click_toggles_segment_stepped` still green.

- [ ] **Step 8: Workspace lint + format + build**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add tiny-skia-widgets/src/mseg/editor.rs tiny-skia-widgets/src/mseg/render.rs
git commit -m "feat(tiny-skia-widgets): right-click transform menu on selected MSEG nodes

Right-click on a node that is part of the active selection opens a
4-item Compress / Expand × Values / Times menu reusing the existing
DropdownState widget (StripId::Transform). Selecting an item dispatches
to the matching compress_/expand_ method.

Right-click on unselected nodes, segments, and empty canvas continues
to toggle the segment-stepped flag exactly as before.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Modulation arc colour coordination

**Files:**
- Modify: `tiny-skia-widgets/src/param_dial.rs` — `mod_color: Color` parameter on `draw_dial_ex`, `draw_dial_dimmed_ex`, and the shared `draw_dial_inner`. `color_modulation()` / `color_modulation_dot()` become hue-parameterised helpers.
- Modify: `multosis/src/editor.rs` — `compute_modulated_norms()` returns `[Option<(f32, u8)>; MAX_EFFECT_PARAMS]`.
- Modify: `multosis/src/editor/effect_editor.rs` — per-dial loop reads the tag and passes `mseg_color(slot)` as the dial's `mod_color`.
- Test: in `tiny-skia-widgets/src/param_dial.rs`.

- [ ] **Step 1: Write the failing tests in param_dial**

Append to the test module in `tiny-skia-widgets/src/param_dial.rs`:

```rust
#[test]
fn dial_modulation_arc_renders_in_the_supplied_mod_color() {
    use tiny_skia::{Color, Pixmap};
    let mut pm = Pixmap::new(200, 200).unwrap();
    let mut tr = TextRenderer::new(include_bytes!("../test_data/DejaVuSans.ttf"));
    // Two distinct mod colours; probe the arc to confirm each shows.
    let amber = Color::from_rgba8(0xff, 0xc8, 0x58, 0xff);
    let purple = Color::from_rgba8(0xc3, 0x78, 0xff, 0xff);
    // Render with amber.
    draw_dial_ex(
        &mut pm,
        &mut tr,
        100.0,
        100.0,
        40.0,
        "X",
        "v",
        0.3,
        Some(0.7),
        None,
        false,
        amber,
    );
    // Probe a pixel on the arc between unmodulated (0.3) and modulated (0.7)
    // value. At the centre of that arc the colour should be amber-ish.
    let probe_x = 100i32 + ((40.0 as f32 * (1.5_f32).sqrt() * 0.5) as i32);
    let probe_y = 100i32 - 36;
    let _ = (probe_x, probe_y); // sanity-check sites if needed
    // Just assert the pixmap contains at least one amber-dominated pixel.
    let pixels = pm.pixels();
    let amber_hit = pixels.iter().any(|p| p.red() > 200 && p.green() > 130 && p.green() < 220 && p.blue() < 130 && p.alpha() > 100);
    assert!(amber_hit, "expected at least one amber pixel from the mod arc");
    // Now render fresh in purple and assert purple-ish pixels appear.
    let mut pm2 = Pixmap::new(200, 200).unwrap();
    draw_dial_ex(
        &mut pm2,
        &mut tr,
        100.0,
        100.0,
        40.0,
        "X",
        "v",
        0.3,
        Some(0.7),
        None,
        false,
        purple,
    );
    let pixels2 = pm2.pixels();
    let purple_hit = pixels2.iter().any(|p| p.red() > 150 && p.green() < 130 && p.blue() > 200 && p.alpha() > 100);
    assert!(purple_hit, "expected at least one purple pixel from the mod arc");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets dial_modulation_arc_renders 2>&1 | tail -15`
Expected: build error — `draw_dial_ex` has 11 args, the test passes 12.

- [ ] **Step 3: Extend the dial signatures**

Edit `tiny-skia-widgets/src/param_dial.rs`. Update `draw_dial_ex`, `draw_dial_dimmed_ex`, and `draw_dial_inner` to accept `mod_color: tiny_skia::Color` as the final parameter.

Replace `color_modulation()` and `color_modulation_dot()` with hue-parameterised helpers (these become private — no other crate uses them per the grep in the spec):

```rust
/// Build the modulation-arc colour from a base hue, applying the
/// historical α=150 for the arc and α=200 for the indicator dot.
fn modulation_arc_color(hue: tiny_skia::Color) -> tiny_skia::Color {
    let (r, g, b, _) = (
        (hue.red() * 255.0).round() as u8,
        (hue.green() * 255.0).round() as u8,
        (hue.blue() * 255.0).round() as u8,
        0u8,
    );
    tiny_skia::Color::from_rgba8(r, g, b, 150)
}

fn modulation_dot_color(hue: tiny_skia::Color) -> tiny_skia::Color {
    let (r, g, b, _) = (
        (hue.red() * 255.0).round() as u8,
        (hue.green() * 255.0).round() as u8,
        (hue.blue() * 255.0).round() as u8,
        0u8,
    );
    tiny_skia::Color::from_rgba8(r, g, b, 200)
}
```

Delete the existing `color_modulation` / `color_modulation_dot` fns.

In `draw_dial_inner`, replace:

```rust
paint.set_color(color_modulation());
```

with:

```rust
paint.set_color(modulation_arc_color(mod_color));
```

and similarly for the dot:

```rust
draw_filled_circle(pixmap, mod_dot_x, mod_dot_y, mod_dot_radius, modulation_dot_color(mod_color));
```

Update the public wrappers (`draw_dial_ex`, `draw_dial_dimmed_ex`) to forward `mod_color` to `draw_dial_inner`.

For `draw_dial` (no `_ex`), forward a sensible default colour — it's the path with no modulation indicator, so the colour is unused. Use the existing orange for backward compatibility:

```rust
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
        pixmap,
        text_renderer,
        cx,
        cy,
        radius,
        label,
        value_text,
        normalized,
        None,
        None,
        false,
        tiny_skia::Color::from_rgba8(255, 160, 50, 255),
    );
}
```

(Same pattern for `draw_dial_dimmed` — keep its existing forwarding to `draw_dial_dimmed_ex` and append the same orange placeholder.)

Update every other existing test in this file that calls `draw_dial_ex` / `draw_dial_dimmed_ex`: append a placeholder `tiny_skia::Color::from_rgba8(255, 160, 50, 255)` (or whatever colour you want as the test default) to each call.

- [ ] **Step 4: Update `compute_modulated_norms` in multosis**

Edit `multosis/src/editor.rs`. Find the existing definition:

```rust
fn compute_modulated_norms(&self) -> [Option<f32>; crate::effects::MAX_EFFECT_PARAMS] {
```

Replace its body so the return slot carries the MSEG slot (1 or 2) that produced the value. Last-MSEG-wins for the same target stays:

```rust
fn compute_modulated_norms(
    &self,
) -> [Option<(f32, u8)>; crate::effects::MAX_EFFECT_PARAMS] {
    let mut out: [Option<(f32, u8)>; crate::effects::MAX_EFFECT_PARAMS] = [None; crate::effects::MAX_EFFECT_PARAMS];
    let modu = self.selected_track_modulation();
    let track = self.selected_track_effect();
    for k in 0..2 {
        let slot = (k + 1) as u8;
        let Some(target) = modu.targets[k] else { continue };
        if target >= crate::effects::MAX_EFFECT_PARAMS {
            continue;
        }
        let depth = modu.depths[k];
        let polarity = modu.msegs[slot as usize].polarity;
        // Existing per-frame phase-read + value computation, lifted here.
        let phase_atomic = &self.mseg_phases[self.selected_track * 3 + slot as usize];
        let phase = f32::from_bits(phase_atomic.load(std::sync::atomic::Ordering::Relaxed));
        let mseg_value = tiny_skia_widgets::mseg::value_at_phase(&modu.msegs[slot as usize], phase);
        let inst = crate::effects::EffectInstance::new(track.kind);
        let Some(spec) = inst.parameters().get(target).copied() else { continue };
        let base = track.params[target];
        let modulated_value = crate::modulation::assignable_value(
            mseg_value, base, depth, spec, polarity,
        );
        let modulated_norm = crate::effects::value_to_norm(
            modulated_value, spec.min, spec.max, spec.scaling,
        );
        // Last-MSEG-wins: a later iteration overwrites an earlier one.
        out[target] = Some((modulated_norm, slot));
    }
    out
}
```

(Adapt details to whatever the existing function actually does — the goal is to preserve current behaviour, only adding the `slot` tag to the return type.)

- [ ] **Step 5: Update the per-dial loop in effect_editor.rs**

In `multosis/src/editor/effect_editor.rs::draw_effect_section`, find the existing line:

```rust
let mod_arc = modulated_norms.get(i).copied().flatten();
```

Replace with:

```rust
let (mod_arc, mod_color) = match modulated_norms.get(i).copied().flatten() {
    Some((n, slot)) => (Some(n), crate::editor::mseg_color(slot as usize)),
    None => (None, tiny_skia::Color::from_rgba8(255, 160, 50, 255)), // arbitrary; unused when mod_arc is None
};
```

And update the two `draw_dial` callsite branches to pass `mod_color` as the new final argument.

Also update the function's parameter:

```rust
modulated_norms: &[Option<(f32, u8)>; MAX_EFFECT_PARAMS],
```

and the call site in `editor.rs` that builds this array.

- [ ] **Step 6: Run multosis + widgets tests**

Run: `cargo nextest run -p multosis -p tiny-skia-widgets 2>&1 | tail -20`
Expected: all green.

- [ ] **Step 7: Workspace lint + format + bundle build**

Run: `cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: clean.

Run: `cargo xtask native nih-plug bundle multosis --release 2>&1 | tail -8`
Expected: bundle written.

- [ ] **Step 8: Commit**

```bash
git add tiny-skia-widgets/src/param_dial.rs multosis/src/editor.rs multosis/src/editor/effect_editor.rs
git commit -m "feat(multosis): per-MSEG colour on the param-dial modulation arc

compute_modulated_norms now returns the MSEG slot (1 or 2) that
produced each tagged value, so the dial draw code can look up the
slot's identity colour via mseg_color() and pass it to
draw_dial_ex / draw_dial_dimmed_ex as the new mod_color argument.

color_modulation() / color_modulation_dot() are now hue-parameterised
helpers (modulation_arc_color / modulation_dot_color) that take any
base hue and emit the existing α=150 arc / α=200 dot. The plain
draw_dial entry point keeps the original orange for backward
compatibility.

A dial driven by MSEG 1 now paints its arc amber; MSEG 2 paints
purple; last-MSEG-wins when both target the same dial.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification

After Task 8 commits, run the full quality gate:

```bash
cargo nextest run --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
cargo xtask native nih-plug bundle multosis --release
```

Manual verification on the standalone bundle:

1. Add the Delay effect to row 0. Confirm the dropdown's tabs match the palette (Amp sky blue active; MSEG 1/2 tabs muted with their amber/purple swatches showing).
2. Switch to MSEG 1 — selector turns amber; the active curve and nodes turn amber; both ghosts (Amp blue, MSEG 2 purple) are visible behind.
3. Hover a node — tooltip appears with the mapped value (dB on Amp; Hz / % / ms / dB on assignable depending on Target).
4. Marquee-select two middle nodes; right-click on one. Menu opens with 4 items. Each pick visibly changes the selection. Right-click an unselected node still toggles segment-stepped.
5. Set MSEG 1's target to Free; the Free dial's modulation arc turns amber. Switch to MSEG 2 instead; the arc turns purple.

If anything visual reads off (alpha too high, tooltip clipped, menu in the wrong place), the constants are localised — return to the task that introduced them and iterate.
