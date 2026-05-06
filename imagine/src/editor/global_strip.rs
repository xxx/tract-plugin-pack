//! Global strip: Recover Sides + Link Bands + Quality.

use crate::theme;
use crate::{ImagineParams, Quality};
use std::sync::Arc;
use tiny_skia::{Color, Paint, PixmapMut, Rect, Transform};

pub struct GlobalLayout {
    pub recover_rect: (i32, i32, i32, i32),
    pub link_rect: (i32, i32, i32, i32),
    pub quality_rect: (i32, i32, i32, i32),
}

pub fn compute_layout(x: i32, y: i32, w: i32, h: i32) -> GlobalLayout {
    // Three sections, equal-ish width.
    let pad = 8;
    let section_w = (w - 4 * pad) / 3;
    GlobalLayout {
        recover_rect: (x + pad, y + 4, section_w, h - 8),
        link_rect: (x + pad * 2 + section_w, y + 4, section_w, h - 8),
        quality_rect: (x + pad * 3 + section_w * 2, y + 4, section_w, h - 8),
    }
}

fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color(color);
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

    // Background
    fill_rect_i(pixmap, x, y, w, h, theme::panel_bg());
    stroke_rect_i(pixmap, x, y, w, h, theme::border());

    // Recover Sides — horizontal bar with marker
    let (rx, ry, rw, rh) = layout.recover_rect;
    fill_rect_i(pixmap, rx, ry, rw, rh, theme::spectrum_bg());
    stroke_rect_i(pixmap, rx, ry, rw, rh, theme::border());
    let recover_norm = params.recover_sides.value() / 100.0;
    let bar_w = (recover_norm * rw as f32) as i32;
    fill_rect_i(pixmap, rx, ry, bar_w, rh, theme::cyan_to_pink(recover_norm));

    // Link Bands — toggle button
    let (lx, ly, lw, lh) = layout.link_rect;
    let link_color = if params.link_bands.value() {
        theme::cyan_to_pink(0.5)
    } else {
        theme::spectrum_bg()
    };
    fill_rect_i(pixmap, lx, ly, lw, lh, link_color);
    stroke_rect_i(pixmap, lx, ly, lw, lh, theme::border());

    // Quality — 2-segment selector (Linear / IIR)
    let (qx, qy, qw, qh) = layout.quality_rect;
    let half_w = qw / 2;
    fill_rect_i(pixmap, qx, qy, qw, qh, theme::spectrum_bg());
    stroke_rect_i(pixmap, qx, qy, qw, qh, theme::border());
    let active_x = match params.quality.value() {
        Quality::Linear => qx,
        Quality::Iir => qx + half_w,
    };
    fill_rect_i(pixmap, active_x, qy, half_w, qh, theme::accent());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_three_sections() {
        let l = compute_layout(0, 0, 600, 36);
        assert!(l.recover_rect.0 < l.link_rect.0);
        assert!(l.link_rect.0 < l.quality_rect.0);
    }

    #[test]
    fn render_at_min_size() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let mut pmut = pixmap.as_mut();
        draw(&mut pmut, 0, 544, 720, 36, &params);
    }
}
