//! Color helpers and primitive drawing functions.

use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

// ---------------------------------------------------------------------------
// Color constants — dark theme matching the vizia CSS in style.css
// ---------------------------------------------------------------------------

// Color helper and theme constants.
// tiny-skia's Color::from_rgba8 is not const, so we use inline functions.

#[inline]
pub fn color_bg() -> Color {
    Color::from_rgba8(0x1a, 0x1c, 0x22, 0xff)
}
#[inline]
pub fn color_text() -> Color {
    Color::from_rgba8(0xe0, 0xe0, 0xe0, 0xff)
}
#[inline]
pub fn color_accent() -> Color {
    Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xff)
}
#[inline]
pub fn color_muted() -> Color {
    Color::from_rgba8(0xa0, 0xa0, 0xa0, 0xff)
}
#[inline]
pub fn color_control_bg() -> Color {
    Color::from_rgba8(0x2a, 0x2c, 0x32, 0xff)
}
#[inline]
pub fn color_border() -> Color {
    Color::from_rgba8(0x40, 0x40, 0x40, 0xff)
}

// ---------------------------------------------------------------------------
// Primitive drawing helpers
// ---------------------------------------------------------------------------

/// Fill a rectangle on `pixmap` using source-over compositing.
///
/// This is the safe default — respects alpha, composes correctly over
/// existing pixels. If the color is fully opaque and you know you're
/// overwriting the region, prefer [`draw_rect_opaque`] which skips the
/// per-pixel blend entirely.
pub fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Fill a rectangle on `pixmap` using `BlendMode::Source` — the
/// destination pixels are **replaced** with the source color, with no
/// source-over blend. This is significantly faster than [`draw_rect`]
/// because it skips the per-pixel blend loop (`source_over_rgba_tail`
/// in tiny-skia's raster pipeline), which dominates CPU time when
/// drawing many small rects.
///
/// Only use when you're happy to overwrite whatever was under the
/// rect. The color should generally be opaque — with `Source` mode a
/// translucent color produces a visually-translucent *pixel* (alpha
/// stored in the destination), not a blended composite.
pub fn draw_rect_opaque(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    paint.blend_mode = tiny_skia::BlendMode::Source;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Stroke a rectangle outline on `pixmap`.
pub fn draw_rect_outline(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: Color,
    stroke_width: f32,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    // Top edge
    draw_rect(pixmap, x, y, w, stroke_width, color);
    // Bottom edge
    draw_rect(pixmap, x, y + h - stroke_width, w, stroke_width, color);
    // Left edge
    draw_rect(
        pixmap,
        x,
        y + stroke_width,
        stroke_width,
        h - 2.0 * stroke_width,
        color,
    );
    // Right edge
    draw_rect(
        pixmap,
        x + w - stroke_width,
        y + stroke_width,
        stroke_width,
        h - 2.0 * stroke_width,
        color,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Pixmap, PremultipliedColorU8};

    /// Read a single pixel from a pixmap by (x, y) coordinates.
    fn pixel_at(pm: &Pixmap, x: u32, y: u32) -> PremultipliedColorU8 {
        pm.pixels()[(y * pm.width() + x) as usize]
    }

    #[test]
    fn test_draw_rect_basic() {
        let mut pm = Pixmap::new(100, 100).unwrap();
        draw_rect(&mut pm, 10.0, 10.0, 20.0, 20.0, color_accent());
        let px = pixel_at(&pm, 15, 15);
        assert!(px.alpha() > 0, "rectangle should have been drawn");
    }

    #[test]
    fn test_draw_rect_outline_basic() {
        let mut pm = Pixmap::new(100, 100).unwrap();
        draw_rect_outline(&mut pm, 10.0, 10.0, 30.0, 30.0, color_border(), 1.0);
        let corner = pixel_at(&pm, 10, 10);
        assert!(corner.alpha() > 0, "outline corner should be drawn");
        let centre = pixel_at(&pm, 25, 25);
        assert_eq!(centre.alpha(), 0, "outline interior should be empty");
    }

    #[test]
    fn test_draw_rect_zero_size() {
        let mut pm = Pixmap::new(50, 50).unwrap();
        // Should not panic on zero or negative dimensions.
        draw_rect(&mut pm, 0.0, 0.0, 0.0, 0.0, color_bg());
        draw_rect(&mut pm, 0.0, 0.0, -5.0, 10.0, color_bg());
        draw_rect_outline(&mut pm, 0.0, 0.0, 0.0, 0.0, color_border(), 1.0);
    }
}
