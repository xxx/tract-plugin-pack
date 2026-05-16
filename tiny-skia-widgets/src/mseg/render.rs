//! MSEG editor — pure layout geometry and all drawing.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

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
