//! Color helpers and primitive drawing functions.

use tiny_skia::{Color, Paint, Pixmap, PremultipliedColorU8, Rect, Transform};

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
#[inline]
pub fn color_edit_bg() -> Color {
    Color::from_rgba8(0x30, 0x34, 0x40, 0xff)
}

// ---------------------------------------------------------------------------
// Primitive drawing helpers
// ---------------------------------------------------------------------------

/// Fill a rectangle on `pixmap` using source-over compositing.
///
/// When the supplied `color` is fully opaque (alpha = 1.0), this
/// automatically switches to `BlendMode::Source` internally — the
/// result is pixel-identical to source-over for opaque colors but
/// skips the per-pixel blend loop (`source_over_rgba_tail`) that
/// dominates CPU time for callers drawing many small rects. So
/// opaque callers get the fast path for free.
///
/// For bulk hot paths that draw many 1-pixel-wide vertical strips
/// (e.g. the pope-scope waveform renderer), prefer
/// [`fill_column_opaque`], which bypasses the tiny-skia raster
/// pipeline entirely and writes directly into `pixels_mut()`.
pub fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    // Opaque colors take the Source fast path — Source and SourceOver
    // produce identical output when src.a == 255, but Source skips
    // the per-pixel blend arithmetic entirely.
    if color.alpha() >= 1.0 {
        paint.blend_mode = tiny_skia::BlendMode::Source;
    }
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

/// Convert an opaque `tiny_skia::Color` to a premultiplied `u8` RGBA
/// tuple suitable for direct writes into `Pixmap::pixels_mut()`.
/// Assumes opaque input (alpha = 1.0). Saturating conversion for
/// values slightly above 1.0.
#[inline]
pub fn color_to_rgba_u8(color: Color) -> (u8, u8, u8, u8) {
    let r = (color.red() * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    let g = (color.green() * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    let b = (color.blue() * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    let a = (color.alpha() * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    (r, g, b, a)
}

/// Clear an entire pixmap to an opaque color via a direct slice
/// fill. Bypasses `Pixmap::fill`'s per-pixel iterator loop (which
/// profiled at ~24% of the GUI thread under load) in favor of
/// `pixels_mut().fill(color)` — slice `fill` on `Copy` elements is
/// guaranteed by the standard library to compile to a `memset`
/// instruction (or vectorized equivalent) when the element size
/// supports it.
pub fn fill_pixmap_opaque(pixmap: &mut Pixmap, color: Color) {
    let (r, g, b, a) = color_to_rgba_u8(color);
    let Some(px_color) = PremultipliedColorU8::from_rgba(r, g, b, a) else {
        return;
    };
    pixmap.pixels_mut().fill(px_color);
}

/// Fill a 1-pixel-wide vertical strip of a pixmap by writing
/// directly into its backing buffer. Bypasses tiny-skia's raster
/// pipeline entirely — no `RasterPipelineBlitter::new`, no
/// `source_over_rgba_tail`, just a strided write loop.
///
/// Intended for hot paths that fill hundreds of 1-pixel-wide
/// columns per frame (e.g. the pope-scope waveform renderer).
/// The profile of the `draw_rect_opaque` version showed ~13% of GUI
/// time in pipeline construction and another ~15% in `blit_rect`
/// overhead; this primitive replaces both with a plain indexed
/// assignment per pixel.
///
/// `col_x` is the pixel column (rounded from the caller's f32).
/// `y_top`/`y_bot` are the half-open Y range, rounded and clamped
/// to pixmap bounds. `color` must be an opaque tiny-skia color;
/// it is flattened to 8-bit RGBA once, then written into each
/// destination pixel.
pub fn fill_column_opaque(
    pixmap: &mut Pixmap,
    col_x: f32,
    y_top: f32,
    y_bot: f32,
    color: Color,
) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let cx = col_x.round() as i32;
    if cx < 0 || cx >= width {
        return;
    }
    let y_top = (y_top.round() as i32).max(0);
    let y_bot = (y_bot.round() as i32).min(height);
    if y_top >= y_bot {
        return;
    }
    let (r, g, b, a) = color_to_rgba_u8(color);
    let Some(px_color) = PremultipliedColorU8::from_rgba(r, g, b, a) else {
        return;
    };

    let width_us = width as usize;
    let col_x_us = cx as usize;
    let start = y_top as usize * width_us;
    let end = y_bot as usize * width_us;
    let pixels = pixmap.pixels_mut();
    // `chunks_exact_mut(width)` yields one row at a time, and the
    // compiler can hoist the row length bound, eliding per-pixel
    // bounds checks on the inner `row[col_x_us]` write.
    for row in pixels[start..end].chunks_exact_mut(width_us) {
        row[col_x_us] = px_color;
    }
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
