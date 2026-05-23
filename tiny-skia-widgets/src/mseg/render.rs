//! MSEG editor — pure layout geometry and all drawing.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::dropdown::draw_dropdown_popup;
use crate::dropdown::draw_dropdown_trigger;
use crate::mseg::editor::{style_items, MsegEditState, StripId};
use crate::mseg::{value_at_phase, HoldMode, MsegData};
use crate::primitives::{
    color_bg, color_border, color_control_bg, color_muted, color_text, draw_rect, draw_rect_outline,
};
use crate::text::TextRenderer;
use tiny_skia::Pixmap;

/// Marker-lane height in unscaled px (full editor only).
const MARKER_LANE_H: f32 = 16.0;
/// Control-strip height in unscaled px.
const STRIP_H: f32 = 42.0;

/// Sub-rectangles of the MSEG widget, each `(x, y, w, h)`. `marker_lane` has
/// height 0 in curve-only mode.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MsegLayout {
    pub marker_lane: (f32, f32, f32, f32),
    /// The full canvas panel — background fill and border.
    pub canvas: (f32, f32, f32, f32),
    /// The drawable plot area, inset within `canvas` by the node-dot radius.
    /// Grid lines, the envelope curve, and node/handle positions all map into
    /// `plot`, so a node at a phase/value extreme (0 or 1) keeps its dot fully
    /// inside `canvas` instead of half-clipping the edge.
    pub plot: (f32, f32, f32, f32),
    pub strip: (f32, f32, f32, f32),
}

/// Compute the widget's sub-rectangles. `curve_only` drops the marker lane;
/// `scale` is the DPI factor.
pub fn mseg_layout(rect: (f32, f32, f32, f32), curve_only: bool, scale: f32) -> MsegLayout {
    let (x, y, w, h) = rect;
    let lane_h = if curve_only {
        0.0
    } else {
        MARKER_LANE_H * scale
    };
    let strip_h = STRIP_H * scale;
    let canvas_h = (h - lane_h - strip_h).max(0.0);
    let canvas = (x, y + lane_h, w, canvas_h);
    // Inset the plot by the largest node-dot radius (a hovered node is the
    // biggest) so node dots at the 0/1 extremes stay fully within `canvas`.
    let m = (NODE_R + HOVER_BUMP) * scale;
    let plot = (
        canvas.0 + m,
        canvas.1 + m,
        (canvas.2 - 2.0 * m).max(0.0),
        (canvas.3 - 2.0 * m).max(0.0),
    );
    MsegLayout {
        marker_lane: (x, y, w, lane_h),
        canvas,
        plot,
        strip: (x, y + lane_h + canvas_h, w, strip_h),
    }
}

/// Normalized phase (0..1) → plot x pixel.
pub fn phase_to_x(layout: &MsegLayout, phase: f32) -> f32 {
    // No zero-width guard needed: phase * 0.0 == 0.0 is already correct here.
    layout.plot.0 + phase.clamp(0.0, 1.0) * layout.plot.2
}

/// Plot x pixel → normalized phase (0..1, clamped).
pub fn x_to_phase(layout: &MsegLayout, x: f32) -> f32 {
    if layout.plot.2 <= 0.0 {
        return 0.0;
    }
    ((x - layout.plot.0) / layout.plot.2).clamp(0.0, 1.0)
}

/// Normalized value (0..1) → plot y pixel (value 1.0 at the top).
pub fn value_to_y(layout: &MsegLayout, value: f32) -> f32 {
    layout.plot.1 + (1.0 - value.clamp(0.0, 1.0)) * layout.plot.3
}

/// Plot y pixel → normalized value (0..1, clamped).
pub fn y_to_value(layout: &MsegLayout, y: f32) -> f32 {
    if layout.plot.3 <= 0.0 {
        return 0.0;
    }
    (1.0 - (y - layout.plot.1) / layout.plot.3).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

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
    // The three sub-rects (marker lane / canvas / strip) are non-overlapping,
    // so draw order is immaterial — each paints only its own region.
    draw_canvas(pixmap, &layout, data, state, scale, value_color);
    draw_marker_lane(pixmap, &layout, data, state, scale, value_color);
    draw_strip(pixmap, text_renderer, &layout, data, state, scale);

    if let Some((idx, text)) = node_tooltip {
        draw_node_tooltip(pixmap, text_renderer, &layout, data, idx, text, scale);
    }
}

/// Draw whichever MSEG dropdown popup is currently open in `state`
/// (`TimeGrid`, `Style`, or `Transform`). MUST be called AFTER any
/// layered draws over the same pane — `draw_mseg` doesn't do this
/// inline because multosis paints ghost MSEGs and the playhead on top
/// of the active widget, which would otherwise bury the popup.
///
/// `rect` is the same MSEG widget rect passed to `draw_mseg`. The
/// popup uses `(rect.0 + rect.2, rect.1 + rect.3)` as its bounds
/// hint so it stays inside the widget (and flips upward when near the
/// bottom).
pub fn draw_mseg_dropdown(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    state: &MsegEditState,
    rect: (f32, f32, f32, f32),
) {
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
    } else if state.dropdown_is_open_for(StripId::Transform) {
        draw_dropdown_popup(
            pixmap,
            text_renderer,
            state.dropdown_state(),
            crate::mseg::editor::transform_menu_items(),
            window_size,
        );
    } else {
        // Either the style dropdown is open or nothing is; closed is a no-op.
        draw_dropdown_popup(
            pixmap,
            text_renderer,
            state.dropdown_state(),
            style_items(),
            window_size,
        );
    }
}

/// Draw the hold marker(s) in the marker lane. No-op in curve-only mode or
/// when no hold is configured.
fn draw_marker_lane(
    pixmap: &mut Pixmap,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
    value_color: tiny_skia::Color,
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
            draw_rect(
                pm,
                mx - 3.0 * scale,
                ly + 2.0 * scale,
                6.0 * scale,
                lh - 4.0 * scale,
                color,
            );
        }
    };
    match data.hold {
        HoldMode::None => {}
        HoldMode::Sustain(i) => mark(pixmap, i, value_color),
        HoldMode::Loop { start, end } => {
            mark(pixmap, start, color_border());
            mark(pixmap, end, color_border());
        }
    }
}

/// Draw the canvas: background, grid, and the envelope polyline.
fn draw_canvas(
    pixmap: &mut Pixmap,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
    value_color: tiny_skia::Color,
) {
    let (cx, cy, cw, ch) = layout.canvas;
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }
    draw_rect(pixmap, cx, cy, cw, ch, color_control_bg());

    // Grid lines, the curve, and nodes all live in the inset `plot` area so
    // edge nodes don't clip. Drawn via the plot-based coordinate mapping.
    let (px0, py0, pw, ph) = layout.plot;

    // Vertical time-grid lines.
    // Grid colour sits between the canvas (color_control_bg, ~#2a2c32) and
    // the border (~#404040) — visible but quiet enough not to fight the
    // envelope polyline. `color_bg()` was too close to the canvas to read.
    let grid_color = tiny_skia::Color::from_rgba8(0x44, 0x47, 0x4e, 0xff);
    let tdiv = data.time_divisions.max(1);
    for i in 1..tdiv {
        let gx = phase_to_x(layout, i as f32 / tdiv as f32);
        draw_rect(pixmap, gx, py0, 1.0, ph, grid_color);
    }
    // Horizontal value-grid lines.
    let vsteps = data.value_steps.max(1);
    for i in 1..vsteps {
        let gy = value_to_y(layout, i as f32 / vsteps as f32);
        draw_rect(pixmap, px0, gy, pw, 1.0, grid_color);
    }

    // Midline marker at value = 0.5 — only meaningful in Bipolar view (the
    // zero point for consumers that re-map via `2·value − 1`: miff's kernel
    // taps, multosis's assignable modulation). In Unipolar view the bottom
    // of the plot is "zero" and the midline would be visual noise.
    if matches!(data.polarity, crate::mseg::Polarity::Bipolar) {
        let mid_color = tiny_skia::Color::from_rgba8(0x82, 0x86, 0x90, 0xff);
        let my = value_to_y(layout, 0.5);
        draw_rect(pixmap, px0, my, pw, 1.0, mid_color);
    }

    // Envelope polyline: sample `value_at_phase` per pixel column of the plot.
    let cols = pw.max(1.0) as usize;
    let mut prev: Option<(f32, f32)> = None;
    for col in 0..=cols {
        let phase = col as f32 / cols as f32;
        let x = phase_to_x(layout, phase);
        let y = value_to_y(layout, value_at_phase(data, phase));
        if let Some((px, py)) = prev {
            draw_line(pixmap, px, py, x, y, value_color);
        }
        prev = Some((x, y));
    }

    draw_nodes(pixmap, layout, data, state, scale, value_color);

    // Marquee selection rectangle (drawn over the curve and nodes).
    if let Some((mx, my, mw, mh)) = state.marquee_rect() {
        if mw > 0.0 && mh > 0.0 {
            let fill = tiny_skia::Color::from_rgba(
                value_color.red(),
                value_color.green(),
                value_color.blue(),
                0x30 as f32 / 255.0,
            )
            .unwrap_or(tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0x30));
            draw_rect(pixmap, mx, my, mw, mh, fill);
            draw_rect_outline(pixmap, mx, my, mw, mh, value_color, 1.0);
        }
    }

    draw_rect_outline(pixmap, cx, cy, cw, ch, color_border(), 1.0);
}

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
    let n = data.active()[idx];
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

/// Draw only `data`'s curve polyline — faint, no nodes/markers/strip — inside
/// the MSEG editor plot rect `rect`. For rendering inactive MSEGs as ghost
/// context behind an active `draw_mseg`. `color` is packed `0xRRGGBBAA`.
///
/// Samples the curve per pixel column exactly like `draw_canvas` so the ghost
/// aligns with the active overlay; uses the same `draw_line` stroking routine
/// as the active curve, just in the supplied faint colour.
pub fn draw_mseg_ghost(
    pixmap: &mut Pixmap,
    rect: (f32, f32, f32, f32),
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
    color: u32,
) {
    let layout = mseg_layout(rect, state.is_curve_only(), scale);
    let (_px0, _py0, pw, ph) = layout.plot;
    if pw <= 0.0 || ph <= 0.0 {
        return;
    }
    // Unpack 0xRRGGBBAA into a tiny_skia::Color, matching the convention used
    // by the rest of the widget crate (`Color::from_rgba8`).
    let r = ((color >> 24) & 0xFF) as u8;
    let g = ((color >> 16) & 0xFF) as u8;
    let b = ((color >> 8) & 0xFF) as u8;
    let a = (color & 0xFF) as u8;
    let stroke = tiny_skia::Color::from_rgba8(r, g, b, a);
    // Sample `value_at_phase` per pixel column of the plot and stroke a
    // polyline between consecutive samples — same pattern as `draw_canvas`.
    let cols = pw.max(1.0) as usize;
    let mut prev: Option<(f32, f32)> = None;
    for col in 0..=cols {
        let phase = col as f32 / cols as f32;
        let x = phase_to_x(&layout, phase);
        let y = value_to_y(&layout, value_at_phase(data, phase));
        if let Some((px, py)) = prev {
            draw_line(pixmap, px, py, x, y, stroke);
        }
        prev = Some((x, y));
    }
}

/// Node-dot radius and tension-handle radius, unscaled px.
const NODE_R: f32 = 4.0;
const TENSION_R: f32 = 3.0;
/// Extra dot radius (unscaled px) when a node is hovered.
const HOVER_BUMP: f32 = 1.5;

/// Draw a filled square "dot" centred at `(x, y)`.
fn draw_dot(pixmap: &mut Pixmap, x: f32, y: f32, r: f32, color: tiny_skia::Color) {
    draw_rect(pixmap, x - r, y - r, r * 2.0, r * 2.0, color);
}

/// Draw node dots and per-segment tension handles over the curve.
fn draw_nodes(
    pixmap: &mut Pixmap,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
    value_color: tiny_skia::Color,
) {
    let a = data.active();
    // Tension handles: midpoint of each non-stepped segment.
    for w in a.windows(2) {
        if w[0].stepped {
            continue;
        }
        let mid_phase = (w[0].time + w[1].time) * 0.5;
        let hx = phase_to_x(layout, mid_phase);
        let hy = value_to_y(layout, value_at_phase(data, mid_phase));
        // A mid-gray, visible against the dark canvas but clearly secondary
        // to the bright-accent node dots.
        draw_dot(pixmap, hx, hy, TENSION_R * scale, color_muted());
    }
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
            value_color
        };
        draw_dot(pixmap, nx, ny, r, color);
    }
}

/// The interactive controls of the MSEG control strip, each `(x, y, w, h)`.
/// `snap`, `polarity`, and `play_mode` are toggle buttons, `grid` and `style`
/// are dropdown triggers, and `randomize` triggers the randomizer. Shared by
/// `draw_strip` and hit-testing so the drawn controls and the click zones can
/// never drift apart. When the widget is shown in `curve_only` mode (a static
/// curve consumer — `play_mode` has no meaning), the `play_mode` rect is the
/// zero rect `(0,0,0,0)` and the other four toggles share the original
/// four-segment layout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StripButtons {
    pub snap: (f32, f32, f32, f32),
    pub grid: (f32, f32, f32, f32),
    pub style: (f32, f32, f32, f32),
    pub polarity: (f32, f32, f32, f32),
    pub play_mode: (f32, f32, f32, f32),
    pub randomize: (f32, f32, f32, f32),
}

/// Lay out the MSEG control-strip buttons within `strip` (`scale` is the DPI
/// factor). Per-button widths are fixed (sized to each label's longest text
/// with a little breathing room) and the row is split into three concern
/// groups separated by a wider gap:
///
/// * **gridding** — `snap` toggle + `grid` dropdown, anchored to the left.
/// * **per-MSEG behaviour** — `polarity` toggle, plus `play_mode` toggle when
///   `show_play_mode` is true (animated consumers like multosis). Hidden for
///   curve-only consumers like miff — `play_mode` returns the zero rect.
/// * **randomizer** — `style` dropdown + `randomize` button, anchored to the
///   right edge so the action button stays where the eye expects it.
///
/// Adjacent buttons within a group are separated by `gap`; between groups,
/// `group_gap` is wider. The middle of the strip is intentionally empty.
pub(crate) fn strip_buttons(
    strip: (f32, f32, f32, f32),
    scale: f32,
    show_play_mode: bool,
) -> StripButtons {
    let (sx, sy, sw, sh) = strip;
    let pad = 6.0 * scale;
    let gap = 4.0 * scale;
    let group_gap = 16.0 * scale;
    let by = sy + 3.0 * scale;
    let bh = (sh - 6.0 * scale).max(0.0);

    // Per-button widths (pre-scale). Short toggles get narrow rects; dropdown
    // triggers wider ones to fit their value text plus a chevron.
    let snap_w = 100.0 * scale;
    let grid_w = 130.0 * scale;
    let polarity_w = 100.0 * scale;
    let play_mode_w = 110.0 * scale;
    let style_w = 180.0 * scale;
    let rand_w = 130.0 * scale;

    // Left cluster, anchored at `sx + pad`.
    let snap = (sx + pad, by, snap_w, bh);
    let grid = (snap.0 + snap_w + gap, by, grid_w, bh);
    let polarity = (grid.0 + grid_w + group_gap, by, polarity_w, bh);
    let play_mode = if show_play_mode {
        (polarity.0 + polarity_w + gap, by, play_mode_w, bh)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

    // Right cluster, anchored at `sx + sw - pad`.
    let randomize = (sx + sw - rand_w - pad, by, rand_w, bh);
    let style = (randomize.0 - gap - style_w, by, style_w, bh);

    StripButtons {
        snap,
        grid,
        style,
        polarity,
        play_mode,
        randomize,
    }
}

/// Draw the control strip: `Snap` toggle button, `Grid` and `Style` dropdown
/// triggers, and the `Randomize` button. Interaction is handled in
/// `on_mouse_down`.
fn draw_strip(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
) {
    let (sx, sy, sw, sh) = layout.strip;
    if sw <= 0.0 || sh <= 0.0 {
        return;
    }
    draw_rect(pixmap, sx, sy, sw, sh, color_bg());
    draw_rect_outline(pixmap, sx, sy, sw, sh, color_border(), 1.0);

    let show_play_mode = !state.is_curve_only();
    let b = strip_buttons(layout.strip, scale, show_play_mode);
    // Snap: plain toggle button, Title Case, highlighted when active.
    let snap_label = if data.snap { "Snap On" } else { "Snap Off" };
    use crate::controls::draw_button;
    draw_button(
        pixmap,
        text_renderer,
        b.snap.0,
        b.snap.1,
        b.snap.2,
        b.snap.3,
        snap_label,
        data.snap,
        false,
    );

    // Grid: dropdown trigger showing the current grid setting.
    let grid_label = format!("Grid {}/{}", data.time_divisions, data.value_steps);
    draw_dropdown_trigger(
        pixmap,
        text_renderer,
        b.grid,
        &grid_label,
        state.dropdown_is_open_for(StripId::TimeGrid),
    );

    // Style: dropdown trigger showing the current randomizer style.
    let style_label = format!("Style {}", state.style());
    draw_dropdown_trigger(
        pixmap,
        text_renderer,
        b.style,
        &style_label,
        state.dropdown_is_open_for(StripId::Style),
    );

    // Polarity: cycles between Unipolar (default) and Bipolar view. Neither
    // value is conceptually "on" — the label tells the user which mode is
    // current — so this is drawn as a plain (un-highlighted) button.
    let polarity_label = match data.polarity {
        crate::mseg::Polarity::Bipolar => "Bipolar",
        crate::mseg::Polarity::Unipolar => "Unipolar",
    };
    draw_button(
        pixmap,
        text_renderer,
        b.polarity.0,
        b.polarity.1,
        b.polarity.2,
        b.polarity.3,
        polarity_label,
        false,
        false,
    );

    // Play mode: cycles between Cyclic (default — the envelope loops) and
    // OneShot (runs once per trigger, holds at the end). Same rationale as
    // polarity — neither is "on"/"off", so no highlight. Hidden for
    // `curve_only` consumers (a static curve has no play mode).
    if show_play_mode {
        let play_label = match data.play_mode {
            crate::mseg::PlayMode::OneShot => "One-shot",
            crate::mseg::PlayMode::Cyclic => "Cyclic",
        };
        draw_button(
            pixmap,
            text_renderer,
            b.play_mode.0,
            b.play_mode.1,
            b.play_mode.2,
            b.play_mode.3,
            play_label,
            false,
            false,
        );
    }

    // Randomize: plain button.
    draw_button(
        pixmap,
        text_renderer,
        b.randomize.0,
        b.randomize.1,
        b.randomize.2,
        b.randomize.3,
        "Randomize",
        false,
        false,
    );
}

// ---------------------------------------------------------------------------
// Hit-testing
// ---------------------------------------------------------------------------

/// Result of hit-testing a pixel against the MSEG widget.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MsegHit {
    /// On the node dot at this index — node is NOT in the active selection.
    Node(usize),
    /// On the node dot at this index — node IS in the active selection.
    /// Right-click on a SelectedNode opens the transform menu; left-click
    /// behaves identically to Node(i).
    SelectedNode(usize),
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

pub(crate) fn in_rect((rx, ry, rw, rh): (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= rx && x < rx + rw && y >= ry && y < ry + rh
}

/// Hit-test pixel `(x, y)` against the widget. `curve_only` suppresses the
/// marker lane. `scale` is the DPI factor — the hit radius and the Randomize
/// button bounds are scaled to match the rendering. Nodes take priority over
/// tension handles, which take priority over empty canvas.
pub fn mseg_hit_test(
    layout: &MsegLayout,
    data: &MsegData,
    curve_only: bool,
    scale: f32,
    x: f32,
    y: f32,
) -> MsegHit {
    let a = data.active();
    let hit_r = HIT_R * scale;
    if in_rect(layout.canvas, x, y) {
        // Nodes first.
        for (i, n) in a.iter().enumerate() {
            let nx = phase_to_x(layout, n.time);
            let ny = value_to_y(layout, n.value);
            if (x - nx).abs() <= hit_r && (y - ny).abs() <= hit_r {
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
            if (x - hx).abs() <= hit_r && (y - hy).abs() <= hit_r {
                return MsegHit::Tension(i);
            }
        }
        return MsegHit::Canvas;
    }
    if !curve_only && in_rect(layout.marker_lane, x, y) {
        return MsegHit::MarkerLane;
    }
    if in_rect(layout.strip, x, y) {
        // The Randomize button is exactly the `strip_buttons` randomize rect.
        // Everything else in the strip is `Strip`, resolved to a specific
        // snap/grid/style button by `on_mouse_down` via the same layout.
        if in_rect(
            strip_buttons(layout.strip, scale, !curve_only).randomize,
            x,
            y,
        ) {
            return MsegHit::Randomize;
        }
        return MsegHit::Strip;
    }
    MsegHit::None
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::MsegData;
    use crate::primitives::color_accent;
    use crate::test_font::test_font_data;
    use crate::text::TextRenderer;
    use tiny_skia::Pixmap;

    const RECT: (f32, f32, f32, f32) = (0.0, 0.0, 400.0, 300.0);

    fn px_alpha(pm: &Pixmap, x: u32, y: u32) -> u8 {
        pm.pixels()[(y * pm.width() + x) as usize].alpha()
    }

    #[test]
    fn draw_mseg_paints_the_canvas() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let data = MsegData::default();
        let state = MsegEditState::new();
        draw_mseg(
            &mut pm,
            &mut tr,
            RECT,
            &data,
            &state,
            1.0,
            color_accent(),
            None,
        );
        // The canvas interior is filled — sample a pixel well inside it.
        let l = mseg_layout(RECT, false, 1.0);
        let cx = (l.canvas.0 + l.canvas.2 * 0.5) as u32;
        let cy = (l.canvas.1 + l.canvas.3 * 0.5) as u32;
        assert!(px_alpha(&pm, cx, cy) > 0, "canvas not painted");
    }

    #[test]
    fn full_layout_has_marker_lane() {
        let l = mseg_layout(RECT, false, 1.0);
        assert!(l.marker_lane.3 > 0.0, "full editor has a marker lane");
        // canvas sits below the marker lane, above the strip.
        assert!((l.canvas.1 - (l.marker_lane.1 + l.marker_lane.3)).abs() < 1e-6);
        assert!(l.canvas.1 + l.canvas.3 <= l.strip.1);
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

    #[test]
    fn extreme_node_dots_stay_within_the_canvas() {
        // A node at any phase/value extreme (0 or 1) must have its whole dot
        // — radius NODE_R — inside the canvas panel, never half-clipped.
        for &scale in &[1.0_f32, 2.0] {
            let l = mseg_layout(RECT, false, scale);
            let (cx, cy, cw, ch) = l.canvas;
            let r = NODE_R * scale;
            for &phase in &[0.0_f32, 1.0] {
                for &value in &[0.0_f32, 1.0] {
                    let nx = phase_to_x(&l, phase);
                    let ny = value_to_y(&l, value);
                    assert!(nx - r >= cx - 0.01, "dot clips left at scale {scale}");
                    assert!(nx + r <= cx + cw + 0.01, "dot clips right at scale {scale}");
                    assert!(ny - r >= cy - 0.01, "dot clips top at scale {scale}");
                    assert!(
                        ny + r <= cy + ch + 0.01,
                        "dot clips bottom at scale {scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn draw_mseg_paints_node_dots() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let mut data = MsegData::default(); // nodes at (0,0) and (1,1)
        data.insert_node(0.5, 0.5);
        let state = MsegEditState::new();
        draw_mseg(
            &mut pm,
            &mut tr,
            RECT,
            &data,
            &state,
            1.0,
            color_accent(),
            None,
        );
        // The interior node at (0.5, 0.5) maps to a dot near canvas centre.
        let l = mseg_layout(RECT, false, 1.0);
        let nx = phase_to_x(&l, 0.5) as u32;
        let ny = value_to_y(&l, 0.5) as u32;
        assert!(px_alpha(&pm, nx, ny) > 0, "node dot not painted");
    }

    #[test]
    fn marker_lane_drawn_for_sustain_in_full_mode() {
        use crate::mseg::HoldMode;
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        data.hold = HoldMode::Sustain(1);
        let state = MsegEditState::new();
        draw_mseg(
            &mut pm,
            &mut tr,
            RECT,
            &data,
            &state,
            1.0,
            color_accent(),
            None,
        );
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
        draw_mseg(
            &mut pm,
            &mut tr,
            RECT,
            &data,
            &state,
            1.0,
            color_accent(),
            None,
        );
        assert_eq!(mseg_layout(RECT, true, 1.0).marker_lane.3, 0.0);
    }

    #[test]
    fn control_strip_is_painted() {
        let mut pm = Pixmap::new(400, 300).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let data = MsegData::default();
        let state = MsegEditState::new();
        draw_mseg(
            &mut pm,
            &mut tr,
            RECT,
            &data,
            &state,
            1.0,
            color_accent(),
            None,
        );
        let l = mseg_layout(RECT, false, 1.0);
        let sx = (l.strip.0 + 10.0) as u32;
        let sy = (l.strip.1 + l.strip.3 * 0.5) as u32;
        assert!(px_alpha(&pm, sx, sy) > 0, "strip not painted");
    }

    #[test]
    fn hit_test_finds_a_node() {
        let mut data = MsegData::default();
        data.insert_node(0.5, 0.5);
        let l = mseg_layout(RECT, false, 1.0);
        let nx = phase_to_x(&l, 0.5);
        let ny = value_to_y(&l, 0.5);
        assert_eq!(
            mseg_hit_test(&l, &data, false, 1.0, nx, ny),
            MsegHit::Node(1)
        );
    }

    #[test]
    fn hit_test_empty_canvas() {
        let data = MsegData::default(); // nodes only at the corners
        let l = mseg_layout(RECT, false, 1.0);
        // The default data has a tension handle at (0.5, 0.5) — probe at (0.25, 0.75)
        // which is well away from any node or handle.
        let hit = mseg_hit_test(
            &l,
            &data,
            false,
            1.0,
            l.canvas.0 + l.canvas.2 * 0.25,
            l.canvas.1 + l.canvas.3 * 0.75,
        );
        assert_eq!(hit, MsegHit::Canvas);
    }

    #[test]
    fn hit_test_randomize_button() {
        let data = MsegData::default();
        let l = mseg_layout(RECT, false, 1.0);
        // Centre of the Randomize button: drawn 6px from the strip's right
        // edge, 84px wide → centre is 6 + 84/2 px in from the right.
        let bx = l.strip.0 + l.strip.2 - 6.0 - 84.0 * 0.5;
        let by = l.strip.1 + l.strip.3 * 0.5;
        assert_eq!(
            mseg_hit_test(&l, &data, false, 1.0, bx, by),
            MsegHit::Randomize
        );
    }

    #[test]
    fn hit_test_outside_is_none() {
        let data = MsegData::default();
        let l = mseg_layout(RECT, false, 1.0);
        assert_eq!(
            mseg_hit_test(&l, &data, false, 1.0, -10.0, -10.0),
            MsegHit::None
        );
    }

    #[test]
    fn draw_mseg_paints_value_color_on_the_curve() {
        use tiny_skia::Color;
        let mut pm = Pixmap::new(400, 200).unwrap();
        let mut tr = TextRenderer::new(&test_font_data());
        let data = MsegData::default();
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
        // Probe a pixel on the polyline. For a default 2-node 0→1 ramp the
        // tension handle sits at x=50%; probe at x=25% where only the curve
        // passes. The linear ramp at phase=0.25 has value=0.25, so the curve
        // pixel sits at approximately y = cy + 0.75*ch (inverted axis).
        let layout = mseg_layout((0.0, 0.0, 400.0, 200.0), state.is_curve_only(), 1.0);
        let (cx, cy, cw, ch) = layout.plot;
        let probe_x = (cx + cw * 0.25) as u32;
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
    }

    #[test]
    fn ghost_curve_draws_some_pixels() {
        let mut pm = Pixmap::new(200, 120).unwrap();
        let data = MsegData::default(); // a 0->1 ramp
        let state = MsegEditState::new();
        let rect = (0.0, 0.0, 200.0, 120.0);
        draw_mseg_ghost(&mut pm, rect, &data, &state, 1.0, 0x5A5040FF);
        // The ghost curve strokes a polyline — some pixels are non-transparent.
        let any_drawn = pm.pixels().iter().any(|p| p.alpha() != 0);
        assert!(any_drawn, "the ghost curve should draw some pixels");
    }
}
