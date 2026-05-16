//! MSEG editor — pure layout geometry and all drawing.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::mseg::editor::MsegEditState;
use crate::mseg::{value_at_phase, HoldMode, MsegData};
use crate::primitives::{
    color_accent, color_bg, color_border, color_control_bg, color_muted, draw_rect,
    draw_rect_outline,
};
use crate::text::TextRenderer;
use tiny_skia::Pixmap;

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
    let lane_h = if curve_only {
        0.0
    } else {
        MARKER_LANE_H * scale
    };
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
    // No zero-width guard needed: phase * 0.0 == 0.0 is already correct here.
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

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

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
    // The three sub-rects (marker lane / canvas / strip) are non-overlapping,
    // so draw order is immaterial — each paints only its own region.
    draw_canvas(pixmap, &layout, data, state, scale);
    draw_marker_lane(pixmap, &layout, data, state, scale);
    draw_strip(pixmap, text_renderer, &layout, data, state, scale);
}

/// Draw the hold marker(s) in the marker lane. No-op in curve-only mode or
/// when no hold is configured.
fn draw_marker_lane(
    pixmap: &mut Pixmap,
    layout: &MsegLayout,
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
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
        HoldMode::Sustain(i) => mark(pixmap, i, color_accent()),
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
) {
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
        let x = (cx + phase * cw).min(cx + cw - 1.0);
        let y = (cy + (1.0 - value_at_phase(data, phase)) * ch).min(cy + ch - 1.0);
        if let Some((px, py)) = prev {
            draw_line(pixmap, px, py, x, y, color_accent());
        }
        prev = Some((x, y));
    }

    draw_nodes(pixmap, layout, data, state, scale);
    draw_rect_outline(pixmap, cx, cy, cw, ch, color_border(), 1.0);
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
    // Node dots; the hovered node is drawn larger / accented.
    for (i, n) in a.iter().enumerate() {
        let nx = phase_to_x(layout, n.time);
        let ny = value_to_y(layout, n.value);
        let hovered = state.hovered_node() == Some(i);
        let r = (if hovered { NODE_R + HOVER_BUMP } else { NODE_R }) * scale;
        draw_dot(pixmap, nx, ny, r, color_accent());
    }
}

/// The four interactive buttons of the MSEG control strip, each `(x, y, w, h)`.
/// `snap` / `grid` / `style` are click-to-cycle buttons; `randomize` triggers
/// the randomizer. Shared by `draw_strip` and hit-testing so the drawn buttons
/// and the click zones can never drift apart.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StripButtons {
    pub snap: (f32, f32, f32, f32),
    pub grid: (f32, f32, f32, f32),
    pub style: (f32, f32, f32, f32),
    pub randomize: (f32, f32, f32, f32),
}

/// Lay out the MSEG control-strip buttons within `strip` (`scale` is the DPI
/// factor). `randomize` is a fixed width at the right end; `snap`/`grid`/`style`
/// share the remaining width in three equal segments.
pub(crate) fn strip_buttons(strip: (f32, f32, f32, f32), scale: f32) -> StripButtons {
    let (sx, sy, sw, sh) = strip;
    let pad = 6.0 * scale;
    let gap = 4.0 * scale;
    let by = sy + 3.0 * scale;
    let bh = (sh - 6.0 * scale).max(0.0);
    let rand_w = 84.0 * scale;
    let randomize = (sx + sw - rand_w - pad, by, rand_w, bh);
    let left = sx + pad;
    let avail = (randomize.0 - gap - left).max(0.0);
    let seg_w = ((avail - 2.0 * gap) / 3.0).max(0.0);
    let snap = (left, by, seg_w, bh);
    let grid = (left + seg_w + gap, by, seg_w, bh);
    let style = (left + 2.0 * (seg_w + gap), by, seg_w, bh);
    StripButtons {
        snap,
        grid,
        style,
        randomize,
    }
}

/// Draw the control strip: a row of buttons — `snap` / `grid` / `style`
/// click-to-cycle buttons and the `Randomize` button. Each is a real button
/// (readable centred label on a visible box) so the click zone matches what
/// the user sees. Interaction is handled in `on_mouse_down`.
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

    let b = strip_buttons(layout.strip, scale);
    let snap_label = format!("snap {}", if data.snap { "on" } else { "off" });
    let grid_label = format!("grid {}/{}", data.time_divisions, data.value_steps);
    let style_label = format!("style {}", state.style());
    use crate::controls::draw_button;
    // The snap button is `active`-highlighted when snapping is on.
    draw_button(
        pixmap,
        text_renderer,
        b.snap.0,
        b.snap.1,
        b.snap.2,
        b.snap.3,
        &snap_label,
        data.snap,
        false,
    );
    draw_button(
        pixmap,
        text_renderer,
        b.grid.0,
        b.grid.1,
        b.grid.2,
        b.grid.3,
        &grid_label,
        false,
        false,
    );
    draw_button(
        pixmap,
        text_renderer,
        b.style.0,
        b.style.1,
        b.style.2,
        b.style.3,
        &style_label,
        false,
        false,
    );
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
        if in_rect(strip_buttons(layout.strip, scale).randomize, x, y) {
            return MsegHit::Randomize;
        }
        return MsegHit::Strip;
    }
    MsegHit::None
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
        draw_mseg(&mut pm, &mut tr, RECT, &data, &state, 1.0);
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
}
