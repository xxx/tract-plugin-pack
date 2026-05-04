//! TextRenderer — fontdue-based glyph rasteriser with a simple cache.

use std::collections::HashMap;
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

// ---------------------------------------------------------------------------
// TextRenderer — fontdue-based glyph rasteriser with a simple cache
// ---------------------------------------------------------------------------

/// Key for the glyph cache: character + quantised pixel size.
#[derive(Hash, Eq, PartialEq, Clone)]
struct GlyphKey {
    ch: char,
    /// Font size multiplied by 10 and truncated, giving 0.1 px resolution while
    /// keeping the key cheap to hash.
    size_tenths: u32,
}

/// Cached rasterisation result for a single glyph.
struct GlyphEntry {
    /// Coverage bitmap (one byte per pixel, row-major).
    bitmap: Vec<u8>,
    metrics: fontdue::Metrics,
}

/// A thin wrapper around [`fontdue::Font`] that caches rasterised glyphs and
/// provides simple text-drawing helpers.
pub struct TextRenderer {
    font: fontdue::Font,
    cache: HashMap<GlyphKey, GlyphEntry>,
}

impl TextRenderer {
    /// Create a new renderer from raw font data (TTF or OTF).
    ///
    /// The caller supplies the bytes — typically via `include_bytes!()` on an
    /// embedded font file.
    pub fn new(font_data: &[u8]) -> Self {
        let settings = fontdue::FontSettings {
            collection_index: 0,
            scale: 40.0,
            load_substitutions: true,
        };
        let font =
            fontdue::Font::from_bytes(font_data, settings).expect("failed to parse font data");
        Self {
            font,
            cache: HashMap::new(),
        }
    }

    /// Measure the width of `text` at the given pixel `size` without drawing.
    pub fn text_width(&mut self, text: &str, size: f32) -> f32 {
        let mut width: f32 = 0.0;
        for ch in text.chars() {
            let entry = self.rasterise(ch, size);
            width += entry.metrics.advance_width;
        }
        width
    }

    /// Draw `text` onto `pixmap` with its baseline at (`x`, `y`).
    ///
    /// `color` is the foreground colour; alpha compositing uses the glyph
    /// coverage as opacity.
    pub fn draw_text(
        &mut self,
        pixmap: &mut Pixmap,
        mut x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: Color,
    ) {
        let pm_width = pixmap.width() as i32;
        let pm_height = pixmap.height() as i32;

        for ch in text.chars() {
            let entry = self.rasterise(ch, size);
            let metrics = entry.metrics;
            let bitmap = &entry.bitmap;

            // Top-left corner in pixel coordinates.
            let gx = (x + metrics.xmin as f32).round() as i32;
            let gy = (y - metrics.ymin as f32 - metrics.height as f32 + 1.0).round() as i32;

            // Blit the coverage bitmap into the pixmap.
            let src_r = (color.red() * 255.0) as u32;
            let src_g = (color.green() * 255.0) as u32;
            let src_b = (color.blue() * 255.0) as u32;

            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let px = gx + col as i32;
                    let py = gy + row as i32;
                    if px < 0 || py < 0 || px >= pm_width || py >= pm_height {
                        continue;
                    }
                    let coverage = bitmap[row * metrics.width + col] as u32;
                    if coverage == 0 {
                        continue;
                    }

                    let idx = (py as u32 * pixmap.width() + px as u32) as usize;
                    let dst = pixmap.pixels_mut()[idx];

                    // Source-over compositing with pre-multiplied alpha.
                    let sa = coverage; // 0..255
                    let inv_sa = 255 - sa;

                    let out_r = ((src_r * sa + dst.red() as u32 * inv_sa) / 255) as u8;
                    let out_g = ((src_g * sa + dst.green() as u32 * inv_sa) / 255) as u8;
                    let out_b = ((src_b * sa + dst.blue() as u32 * inv_sa) / 255) as u8;
                    let out_a = (sa + (dst.alpha() as u32 * inv_sa) / 255) as u8;

                    pixmap.pixels_mut()[idx] =
                        PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a).unwrap();
                }
            }

            x += metrics.advance_width;
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Rasterise (or fetch from cache) a single glyph.
    fn rasterise(&mut self, ch: char, size: f32) -> &GlyphEntry {
        let key = GlyphKey {
            ch,
            size_tenths: (size * 10.0) as u32,
        };
        let font = &self.font;
        self.cache.entry(key).or_insert_with_key(|k| {
            let (metrics, bitmap) = font.rasterize(k.ch, size);
            GlyphEntry { bitmap, metrics }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::color_text;
    use crate::test_font::test_font_data;
    use tiny_skia::Pixmap;

    #[test]
    fn test_text_renderer_creation() {
        let data = test_font_data();
        let _renderer = TextRenderer::new(&data);
    }

    #[test]
    fn test_text_width_empty_string() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let w = renderer.text_width("", 14.0);
        assert_eq!(w, 0.0);
    }

    #[test]
    fn test_text_width_nonempty() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        // With the minimal font every char maps to .notdef (600/1000 em
        // advance). At 14 px the advance is 14 * 600/1000 = 8.4 per char.
        let w = renderer.text_width("AB", 14.0);
        assert!(w > 0.0, "non-empty text should have positive width");
    }

    #[test]
    fn test_draw_text_does_not_panic() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(200, 50).unwrap();
        renderer.draw_text(&mut pm, 5.0, 30.0, "Hello!", 14.0, color_text());
    }

    #[test]
    fn test_glyph_cache_reuse() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        // First call populates the cache.
        let w1 = renderer.text_width("AAA", 16.0);
        // Second call should hit the cache and return the same width.
        let w2 = renderer.text_width("AAA", 16.0);
        assert_eq!(w1, w2, "cached and fresh widths must match");
        assert!(!renderer.cache.is_empty(), "cache should be populated");
    }
}
