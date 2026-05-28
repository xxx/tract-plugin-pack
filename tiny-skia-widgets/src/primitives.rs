//! Color helpers and primitive drawing functions.

use tiny_skia::{Color, Paint, Pixmap, PixmapMut, PremultipliedColorU8, Rect, Transform};

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
    // Lifted from 0x404040 -> 0x6e7180 to hit ~3.8:1 against color_bg()
    // and ~3.1:1 against color_control_bg(). The old value sat at ~1.6:1
    // against the page background, low enough that control outlines and
    // section-header divider rules visually dissolved into the panel.
    // The slight blue cast keeps it in family with the palette's cool tint.
    Color::from_rgba8(0x6e, 0x71, 0x80, 0xff)
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
pub fn fill_column_opaque(pixmap: &mut Pixmap, col_x: f32, y_top: f32, y_bot: f32, color: Color) {
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
// Integer-coordinate rect fills on a borrowed PixmapMut
// ---------------------------------------------------------------------------

/// Fill an integer-coordinate rectangle on a borrowed [`PixmapMut`] sub-view.
///
/// The [`Pixmap`]-targeting [`draw_rect`] can't be used when the caller only
/// holds a `PixmapMut` (e.g. a clipped sub-region obtained via
/// `pixmap.as_mut()`), which is common in editors that composite into a slice
/// of the frame. Opaque colors take the `BlendMode::Source` fast path; AA is
/// disabled (axis-aligned integer rects have no fractional edges). No-ops for
/// non-positive width/height.
pub fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    if color.is_opaque() {
        paint.blend_mode = tiny_skia::BlendMode::Source;
    }
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Stroke a 1px integer-coordinate rectangle outline on a borrowed
/// [`PixmapMut`] using [`fill_rect_i`] for each edge.
///
/// The left/right edges are inset 1px vertically so the corner pixels aren't
/// drawn twice. For opaque colors this is pixel-identical to drawing full-height
/// side edges (overwrite is idempotent); for translucent colors it avoids the
/// corners compositing twice and darkening.
pub fn stroke_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    fill_rect_i(pixmap, x, y, w, 1, color);
    fill_rect_i(pixmap, x, y + h - 1, w, 1, color);
    fill_rect_i(pixmap, x, y + 1, 1, (h - 2).max(0), color);
    fill_rect_i(pixmap, x + w - 1, y + 1, 1, (h - 2).max(0), color);
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

    #[test]
    fn test_fill_rect_i_basic() {
        let mut pm = Pixmap::new(100, 100).unwrap();
        fill_rect_i(&mut pm.as_mut(), 10, 10, 20, 20, color_accent());
        let px = pixel_at(&pm, 15, 15);
        assert!(px.alpha() > 0, "rect should have been drawn");
        let outside = pixel_at(&pm, 5, 5);
        assert_eq!(outside.alpha(), 0, "outside the rect should be empty");
    }

    #[test]
    fn test_stroke_rect_i_outline_only() {
        let mut pm = Pixmap::new(100, 100).unwrap();
        stroke_rect_i(&mut pm.as_mut(), 10, 10, 30, 30, color_border());
        let corner = pixel_at(&pm, 10, 10);
        assert!(corner.alpha() > 0, "outline corner should be drawn");
        let centre = pixel_at(&pm, 25, 25);
        assert_eq!(centre.alpha(), 0, "outline interior should be empty");
    }

    #[test]
    fn test_fill_rect_i_zero_size() {
        let mut pm = Pixmap::new(50, 50).unwrap();
        fill_rect_i(&mut pm.as_mut(), 0, 0, 0, 0, color_bg());
        fill_rect_i(&mut pm.as_mut(), 0, 0, -5, 10, color_bg());
        stroke_rect_i(&mut pm.as_mut(), 0, 0, 0, 0, color_border());
    }
}
