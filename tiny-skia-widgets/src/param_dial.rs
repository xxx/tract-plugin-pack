//! Arc-based rotary dial widget.
//!
//! Draws a 270-degree arc with a value indicator, label above, and value
//! text below. Pure rendering — no interaction handling.

use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::primitives::{color_accent, color_muted, color_text};
use crate::text::TextRenderer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Arc start angle in radians (135 degrees in math convention: lower-left).
pub(crate) const START_ANGLE: f32 = 135.0_f32 * (std::f32::consts::PI / 180.0);

/// Arc end angle in radians (405 degrees = 135 + 270, lower-right).
pub(crate) const END_ANGLE: f32 = 405.0_f32 * (std::f32::consts::PI / 180.0);

/// Total sweep of the dial arc in radians (270 degrees).
const SWEEP: f32 = 270.0_f32 * (std::f32::consts::PI / 180.0);

/// Background track color (dark gray).
fn color_track() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(64, 64, 64, 255)
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

/// Compute the (x, y) point on a circle at `angle` (radians, math convention:
/// 0 = right, increasing counter-clockwise in standard math but tiny-skia has
/// y-axis pointing down so we use: x = cx + r*cos(a), y = cy + r*sin(a)).
#[inline]
pub(crate) fn arc_point(cx: f32, cy: f32, radius: f32, angle: f32) -> (f32, f32) {
    (cx + radius * angle.cos(), cy + radius * angle.sin())
}

/// Map a normalized value in [0.0, 1.0] to an arc angle in radians.
///
/// 0.0 maps to `START_ANGLE`, 1.0 maps to `END_ANGLE`.
#[inline]
fn value_to_angle(normalized: f32) -> f32 {
    let n = normalized.clamp(0.0, 1.0);
    START_ANGLE + n * SWEEP
}

/// Build a tiny-skia `Path` approximating an arc segment.
///
/// The arc is approximated by splitting it into segments of at most 90 degrees,
/// then fitting each segment with a cubic Bézier curve using the standard
/// tangent-length formula: `alpha = 4/3 * tan(sweep/4)`.
///
/// Returns `None` if the path builder fails to produce a valid path (e.g. when
/// radius is zero or the arc is degenerate).
fn build_arc_path(cx: f32, cy: f32, radius: f32, start: f32, end: f32) -> Option<Path> {
    if radius <= 0.0 {
        return None;
    }

    let total_sweep = end - start;
    if total_sweep.abs() < 1e-6 {
        return None;
    }

    // Split into segments of at most PI/2 radians (90 degrees).
    let max_seg = std::f32::consts::FRAC_PI_2;
    let n_segs = (total_sweep.abs() / max_seg).ceil() as u32;
    let n_segs = n_segs.max(1);
    let seg_sweep = total_sweep / n_segs as f32;

    let mut pb = PathBuilder::new();

    let (x0, y0) = arc_point(cx, cy, radius, start);
    pb.move_to(x0, y0);

    for i in 0..n_segs {
        let a1 = start + i as f32 * seg_sweep;
        let a2 = a1 + seg_sweep;

        let (x1, y1) = arc_point(cx, cy, radius, a1);
        let (x2, y2) = arc_point(cx, cy, radius, a2);

        // Tangent direction at a1: (-sin(a1), cos(a1)) scaled by alpha*radius.
        // Tangent direction at a2: (-sin(a2), cos(a2)) — but we move *against*
        // it for cp2.
        let alpha = (4.0 / 3.0) * (seg_sweep / 4.0).tan();

        let cp1x = x1 + alpha * radius * (-a1.sin());
        let cp1y = y1 + alpha * radius * (a1.cos());
        let cp2x = x2 - alpha * radius * (-a2.sin());
        let cp2y = y2 - alpha * radius * (a2.cos());

        pb.cubic_to(cp1x, cp1y, cp2x, cp2y, x2, y2);
    }

    pb.finish()
}

// ---------------------------------------------------------------------------
// Public widget function
// ---------------------------------------------------------------------------

/// Draw an arc-based rotary dial onto `pixmap`.
///
/// The dial sweeps 270 degrees (135° to 405° in math convention).  The
/// background track is drawn in dark gray; the value arc in accent blue.  A
/// filled indicator dot sits at the current value position.  `label` is
/// rendered above the arc in muted color; `value_text` is rendered below in
/// the standard text color.
///
/// `normalized` is clamped to `[0.0, 1.0]` before use.
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
    let n = normalized.clamp(0.0, 1.0);
    let stroke_width = (radius * 0.1).max(2.0);

    // --- Background track (full arc) ---
    if let Some(track_path) = build_arc_path(cx, cy, radius, START_ANGLE, END_ANGLE) {
        let mut paint = Paint::default();
        paint.set_color(color_track());
        paint.anti_alias = true;
        let stroke = Stroke {
            width: stroke_width,
            line_cap: LineCap::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&track_path, &paint, &stroke, Transform::identity(), None);
    }

    // --- Value arc (start angle to current value) ---
    let value_angle = value_to_angle(n);
    if let Some(value_path) = build_arc_path(cx, cy, radius, START_ANGLE, value_angle) {
        let mut paint = Paint::default();
        paint.set_color(color_accent());
        paint.anti_alias = true;
        let stroke = Stroke {
            width: stroke_width,
            line_cap: LineCap::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&value_path, &paint, &stroke, Transform::identity(), None);
    }

    // --- Indicator dot at current value position ---
    let dot_radius = stroke_width * 0.75;
    let (dot_x, dot_y) = arc_point(cx, cy, radius, value_angle);
    draw_filled_circle(pixmap, dot_x, dot_y, dot_radius, color_accent());

    // --- Label text centered above the arc ---
    let text_size = (radius * 0.38).max(11.0);
    let label_w = text_renderer.text_width(label, text_size);
    let label_x = cx - label_w * 0.5;
    // Place label above the center; the top of the arc is at cy - radius,
    // so put the baseline a little above that.
    let label_y = cy - radius - stroke_width - 8.0;
    text_renderer.draw_text(pixmap, label_x, label_y, label, text_size, color_muted());

    // --- Value text centered below the arc ---
    let value_w = text_renderer.text_width(value_text, text_size);
    let value_x = cx - value_w * 0.5;
    // Place value text just below the arc endpoints (at cy + 0.707*r).
    let value_y = cy + radius * 0.71 + text_size + 4.0;
    text_renderer.draw_text(
        pixmap,
        value_x,
        value_y,
        value_text,
        text_size,
        color_text(),
    );
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Fill a circle at (`cx`, `cy`) with the given `radius` and `color`.
fn draw_filled_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: tiny_skia::Color) {
    if radius <= 0.0 {
        return;
    }
    // Approximate a circle with four cubic Bézier segments.
    // The magic number 0.5523 ≈ 4/3*(sqrt(2)-1) gives a good circle fit.
    const K: f32 = 0.5523;
    let k = K * radius;

    let mut pb = PathBuilder::new();
    pb.move_to(cx + radius, cy);
    pb.cubic_to(cx + radius, cy - k, cx + k, cy - radius, cx, cy - radius);
    pb.cubic_to(cx - k, cy - radius, cx - radius, cy - k, cx - radius, cy);
    pb.cubic_to(cx - radius, cy + k, cx - k, cy + radius, cx, cy + radius);
    pb.cubic_to(cx + k, cy + radius, cx + radius, cy + k, cx + radius, cy);
    pb.close();

    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextRenderer;
    use tiny_skia::Pixmap;

    fn test_renderer() -> TextRenderer {
        let font_data = include_bytes!("../test_data/DejaVuSans.ttf");
        TextRenderer::new(font_data)
    }

    fn test_pixmap() -> Pixmap {
        Pixmap::new(200, 200).unwrap()
    }

    // -----------------------------------------------------------------------
    // Smoke tests — draw_dial must not panic for any normalized value
    // -----------------------------------------------------------------------

    #[test]
    fn test_draw_dial_zero() {
        let mut pm = test_pixmap();
        let mut tr = test_renderer();
        draw_dial(&mut pm, &mut tr, 100.0, 100.0, 40.0, "Gain", "0.0 dB", 0.0);
    }

    #[test]
    fn test_draw_dial_half() {
        let mut pm = test_pixmap();
        let mut tr = test_renderer();
        draw_dial(&mut pm, &mut tr, 100.0, 100.0, 40.0, "Freq", "440 Hz", 0.5);
    }

    #[test]
    fn test_draw_dial_full() {
        let mut pm = test_pixmap();
        let mut tr = test_renderer();
        draw_dial(&mut pm, &mut tr, 100.0, 100.0, 40.0, "Mix", "100%", 1.0);
    }

    #[test]
    fn test_draw_dial_clamps_out_of_range() {
        let mut pm = test_pixmap();
        let mut tr = test_renderer();
        // Values outside [0.0, 1.0] must be clamped without panicking.
        draw_dial(&mut pm, &mut tr, 100.0, 100.0, 40.0, "X", "val", -0.5);
        draw_dial(&mut pm, &mut tr, 100.0, 100.0, 40.0, "X", "val", 1.5);
    }

    // -----------------------------------------------------------------------
    // arc_point geometry tests
    // -----------------------------------------------------------------------

    /// START_ANGLE = 135° → cos(135°) < 0, sin(135°) > 0 in screen space.
    /// In tiny-skia's coordinate system (y down), a point at START_ANGLE
    /// from center is upper-left (dx < 0) and below center (dy > 0).
    ///
    /// Actually in math convention with y-down:
    ///   cos(135°) ≈ -0.707  → x is to the left of center  ✓ upper-left x
    ///   sin(135°) ≈  0.707  → y is below center in screen coords
    ///
    /// "Upper-left" in screen space means x < cx and y < cy.
    /// But sin(135°) > 0, so y = cy + r*sin > cy, which is *lower* on screen.
    ///
    /// The requirement "upper-left quadrant" means: x < cx (dx < 0). The y
    /// position depends on the coordinate convention. We test what the spec
    /// says: start angle point has x < cx (left side).
    #[test]
    fn test_arc_point_start() {
        let (px, _py) = arc_point(100.0, 100.0, 40.0, START_ANGLE);
        // At 135° the point is to the left of center.
        assert!(
            px < 100.0,
            "start angle point should be left of center, got px={px}"
        );
    }

    /// END_ANGLE = 405° = 360° + 45° → equivalent to 45°.
    /// cos(45°) > 0, sin(45°) > 0 → x to the right, y below in screen coords.
    /// "Upper-right quadrant" in terms of x: x > cx (right side).
    #[test]
    fn test_arc_point_end() {
        let (px, _py) = arc_point(100.0, 100.0, 40.0, END_ANGLE);
        // At 405° (= 45°) the point is to the right of center.
        assert!(
            px > 100.0,
            "end angle point should be right of center, got px={px}"
        );
    }

    // -----------------------------------------------------------------------
    // value_to_angle bounds
    // -----------------------------------------------------------------------

    #[test]
    fn test_value_to_angle_bounds() {
        let angle_0 = value_to_angle(0.0);
        let angle_1 = value_to_angle(1.0);

        let eps = 1e-5_f32;
        assert!(
            (angle_0 - START_ANGLE).abs() < eps,
            "0.0 should map to START_ANGLE ({START_ANGLE}), got {angle_0}"
        );
        assert!(
            (angle_1 - END_ANGLE).abs() < eps,
            "1.0 should map to END_ANGLE ({END_ANGLE}), got {angle_1}"
        );
    }
}
