//! Per-band strip: Width slider (vertical), Stereoize knob, Mode I/II, Solo.
//! 4-up grid.

use crate::theme;
use crate::ImagineParams;
use std::sync::Arc;
use tiny_skia::{Color, Paint, PixmapMut, Rect, Transform};

pub struct BandStripLayout {
    pub band_x: [i32; 4],
    pub band_w: i32,
    pub y: i32,
    pub h: i32,
    /// Width slider rect inside each band (relative to band's top-left).
    pub width_rect: (i32, i32, i32, i32),
    /// Stereoize knob center + radius (relative to band's top-left).
    pub stz_center: (i32, i32),
    pub stz_radius: i32,
    /// Mode toggle rect (relative to band's top-left).
    pub mode_rect: (i32, i32, i32, i32),
    /// Solo button rect (relative to band's top-left).
    pub solo_rect: (i32, i32, i32, i32),
}

pub fn compute_layout(x: i32, y: i32, w: i32, h: i32) -> BandStripLayout {
    let band_w = w / 4;
    let band_x = [x, x + band_w, x + 2 * band_w, x + 3 * band_w];

    // Inside a band: Width slider on left half (vertical, narrow), knob + buttons on right.
    let pad = 6;
    let width_rect = (pad, pad, 24, h - 2 * pad);
    let stz_center = (band_w / 2 + 12, h / 2 - 16);
    let stz_radius = (band_w / 4).min(h / 4) - 4;
    let mode_rect = (band_w - 38, h / 2 + 8, 32, 14);
    let solo_rect = (band_w - 38, h / 2 + 26, 32, 14);

    BandStripLayout {
        band_x,
        band_w,
        y,
        h,
        width_rect,
        stz_center,
        stz_radius,
        mode_rect,
        solo_rect,
    }
}

fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    paint.blend_mode = if color.is_opaque() {
        tiny_skia::BlendMode::Source
    } else {
        tiny_skia::BlendMode::SourceOver
    };
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn stroke_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    fill_rect_i(pixmap, x, y, w, 1, color);
    fill_rect_i(pixmap, x, y + h - 1, w, 1, color);
    fill_rect_i(pixmap, x, y, 1, h, color);
    fill_rect_i(pixmap, x + w - 1, y, 1, h, color);
}

pub fn draw(
    pixmap: &mut PixmapMut<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    params: &Arc<ImagineParams>,
) {
    let layout = compute_layout(x, y, w, h);

    let widths = [
        params.bands[0].width.value(),
        params.bands[1].width.value(),
        params.bands[2].width.value(),
        params.bands[3].width.value(),
    ];
    let stz_amounts = [
        params.bands[0].stz.value(),
        params.bands[1].stz.value(),
        params.bands[2].stz.value(),
        params.bands[3].stz.value(),
    ];
    let modes = [
        params.bands[0].mode.value(),
        params.bands[1].mode.value(),
        params.bands[2].mode.value(),
        params.bands[3].mode.value(),
    ];
    let solos = [
        params.bands[0].solo.value(),
        params.bands[1].solo.value(),
        params.bands[2].solo.value(),
        params.bands[3].solo.value(),
    ];

    for i in 0..4 {
        let bx = layout.band_x[i];
        // Panel
        fill_rect_i(
            pixmap,
            bx,
            layout.y,
            layout.band_w - 4,
            layout.h,
            theme::panel_bg(),
        );
        stroke_rect_i(
            pixmap,
            bx,
            layout.y,
            layout.band_w - 4,
            layout.h,
            theme::border(),
        );

        // Width slider (vertical bar with marker)
        let (wx, wy, ww, wh) = layout.width_rect;
        let slot_x = bx + wx;
        let slot_y = layout.y + wy;
        fill_rect_i(pixmap, slot_x, slot_y, ww, wh, theme::spectrum_bg());
        stroke_rect_i(pixmap, slot_x, slot_y, ww, wh, theme::border());
        // Center line at width=0
        let center_y = slot_y + wh / 2;
        fill_rect_i(pixmap, slot_x, center_y, ww, 1, theme::text_dim());
        // Marker: top = +100, bottom = -100
        let w_norm = (widths[i] + 100.0) / 200.0; // 0..1
        let marker_y = slot_y + wh - (w_norm * wh as f32) as i32;
        fill_rect_i(
            pixmap,
            slot_x,
            marker_y - 1,
            ww,
            3,
            theme::cyan_to_pink(w_norm),
        );

        // Stereoize knob (filled-arc representation)
        let (cx, cy) = (bx + layout.stz_center.0, layout.y + layout.stz_center.1);
        let radius = layout.stz_radius;
        // Ring background (full circle as series of dots)
        draw_arc_ring(pixmap, cx, cy, radius, 0.0, 1.0, theme::spectrum_bg());
        // Amount arc
        let stz_norm = (stz_amounts[i] / 100.0).clamp(0.0, 1.0);
        if stz_norm > 0.0 {
            draw_arc_ring(pixmap, cx, cy, radius, 0.0, stz_norm, theme::accent());
        }

        // Mode toggle
        let (mxi, myi, mw, mh) = layout.mode_rect;
        let mode_x = bx + mxi;
        let mode_y = layout.y + myi;
        fill_rect_i(pixmap, mode_x, mode_y, mw, mh, theme::spectrum_bg());
        stroke_rect_i(pixmap, mode_x, mode_y, mw, mh, theme::border());
        // Highlight active half (Mode I = left, Mode II = right)
        let half_w = mw / 2;
        let active_x = if matches!(modes[i], crate::StereoizeModeParam::I) {
            mode_x
        } else {
            mode_x + half_w
        };
        fill_rect_i(pixmap, active_x, mode_y, half_w, mh, theme::accent());

        // Solo button
        let (sxi, syi, sw, sh) = layout.solo_rect;
        let solo_x = bx + sxi;
        let solo_y = layout.y + syi;
        let color = if solos[i] {
            theme::cyan_to_pink(0.5)
        } else {
            theme::spectrum_bg()
        };
        fill_rect_i(pixmap, solo_x, solo_y, sw, sh, color);
        stroke_rect_i(pixmap, solo_x, solo_y, sw, sh, theme::border());
    }
}

/// Draw a partial ring from `start_norm` to `end_norm` (both in [0, 1]),
/// where 0 = top of ring (12 o'clock) and progress is clockwise.
/// Implemented as a series of small filled rectangles around the perimeter.
fn draw_arc_ring(
    pixmap: &mut PixmapMut<'_>,
    cx: i32,
    cy: i32,
    radius: i32,
    start_norm: f32,
    end_norm: f32,
    color: Color,
) {
    if radius <= 0 {
        return;
    }
    let r_inner = (radius - 3).max(1) as f32;
    let r_outer = radius as f32;
    let steps = 60;
    let start = start_norm * std::f32::consts::TAU;
    let end = end_norm * std::f32::consts::TAU;
    if end <= start {
        return;
    }
    let r_mid = (r_inner + r_outer) * 0.5;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let angle = start + (end - start) * t;
        // 12 o'clock origin: angle 0 = -π/2
        let real_angle = angle - std::f32::consts::FRAC_PI_2;
        let px = cx + (real_angle.cos() * r_mid) as i32;
        let py = cy + (real_angle.sin() * r_mid) as i32;
        fill_rect_i(pixmap, px - 1, py - 1, 3, 3, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_4_columns() {
        let layout = compute_layout(0, 0, 400, 200);
        assert_eq!(layout.band_x[0], 0);
        assert_eq!(layout.band_x[3], 300);
        assert_eq!(layout.band_w, 100);
    }

    #[test]
    fn render_at_min_size() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let mut pmut = pixmap.as_mut();
        draw(&mut pmut, 290, 350, 430, 150, &params);
    }

    #[test]
    fn arc_ring_no_panic_zero_amount() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let mut pmut = pixmap.as_mut();
        draw(&mut pmut, 290, 350, 430, 150, &params);
    }
}
