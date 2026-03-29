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
    use tiny_skia::Pixmap;

    /// Build a minimal valid TrueType font with a single .notdef glyph (600
    /// units advance width). All characters map to glyph 0, so `text_width`
    /// will report the .notdef advance for every character. This is enough
    /// for fontdue to parse without errors and for the rasterisation /
    /// compositing code paths to be exercised.
    ///
    /// In production the caller passes real font bytes via `include_bytes!`.
    fn test_font_data() -> Vec<u8> {
        // We build 8 required TrueType tables:
        //   OS/2, cmap, glyf, head, hhea, hmtx, loca, maxp
        // (sorted alphabetically by tag as required by the spec).

        const NUM_TABLES: u16 = 8;
        let mut buf: Vec<u8> = Vec::with_capacity(2048);

        // --- Offset table (12 bytes) ---
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // sfVersion (TrueType)
        buf.extend_from_slice(&NUM_TABLES.to_be_bytes());
        buf.extend_from_slice(&128u16.to_be_bytes()); // searchRange = 8*16
        buf.extend_from_slice(&3u16.to_be_bytes()); // entrySelector = log2(8)
        buf.extend_from_slice(&0u16.to_be_bytes()); // rangeShift = 8*16-128

        // --- Reserve space for table directory (8 * 16 = 128 bytes) ---
        let dir_start = buf.len();
        buf.resize(dir_start + NUM_TABLES as usize * 16, 0);

        struct TableEntry {
            tag: [u8; 4],
            offset: u32,
            length: u32,
        }
        let mut entries: Vec<TableEntry> = Vec::new();

        fn pad4(buf: &mut Vec<u8>) {
            while buf.len() % 4 != 0 {
                buf.push(0);
            }
        }

        macro_rules! push_i16 {
            ($buf:expr, $v:expr) => {
                $buf.extend_from_slice(&($v as i16).to_be_bytes());
            };
        }
        macro_rules! push_u16 {
            ($buf:expr, $v:expr) => {
                $buf.extend_from_slice(&($v as u16).to_be_bytes());
            };
        }
        macro_rules! push_u32 {
            ($buf:expr, $v:expr) => {
                $buf.extend_from_slice(&($v as u32).to_be_bytes());
            };
        }

        // === head (54 bytes) ===
        let off = buf.len() as u32;
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // version 1.0
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // fontRevision
        push_u32!(buf, 0u32); // checksumAdjustment
        push_u32!(buf, 0x5F0F3CF5u32); // magicNumber
        push_u16!(buf, 0x000Bu16); // flags
        push_u16!(buf, 1000u16); // unitsPerEm
        buf.extend_from_slice(&[0u8; 16]); // created + modified
        push_i16!(buf, 0);
        push_i16!(buf, 0); // xMin, yMin
        push_i16!(buf, 0);
        push_i16!(buf, 0); // xMax, yMax
        push_u16!(buf, 0u16); // macStyle
        push_u16!(buf, 8u16); // lowestRecPPEM
        push_i16!(buf, 2); // fontDirectionHint
        push_i16!(buf, 0); // indexToLocFormat (short)
        push_i16!(buf, 0); // glyphDataFormat
        entries.push(TableEntry {
            tag: *b"head",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === hhea (36 bytes) ===
        let off = buf.len() as u32;
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // version
        push_i16!(buf, 800); // ascender
        push_i16!(buf, -200); // descender
        push_i16!(buf, 0); // lineGap
        push_u16!(buf, 600u16); // advanceWidthMax
        push_i16!(buf, 0); // minLeftSideBearing
        push_i16!(buf, 0); // minRightSideBearing
        push_i16!(buf, 0); // xMaxExtent
        push_i16!(buf, 1); // caretSlopeRise
        push_i16!(buf, 0); // caretSlopeRun
        push_i16!(buf, 0); // caretOffset
        buf.extend_from_slice(&[0u8; 8]); // reserved
        push_i16!(buf, 0); // metricDataFormat
        push_u16!(buf, 1u16); // numberOfHMetrics
        entries.push(TableEntry {
            tag: *b"hhea",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === maxp (32 bytes) ===
        let off = buf.len() as u32;
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // version 1.0
        push_u16!(buf, 1u16); // numGlyphs
        buf.extend_from_slice(&[0u8; 26]); // remaining fields
        entries.push(TableEntry {
            tag: *b"maxp",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === OS/2 (version 1, 86 bytes) ===
        let off = buf.len() as u32;
        push_u16!(buf, 1u16); // version
        push_i16!(buf, 600); // xAvgCharWidth
        push_u16!(buf, 400u16); // usWeightClass
        push_u16!(buf, 5u16); // usWidthClass
        push_u16!(buf, 0u16); // fsType
        buf.extend_from_slice(&[0u8; 20]); // subscript/superscript
        push_i16!(buf, 0);
        push_i16!(buf, 0); // strikeout
        push_i16!(buf, 0); // sFamilyClass
        buf.extend_from_slice(&[0u8; 10]); // panose
        buf.extend_from_slice(&[0u8; 16]); // ulUnicodeRange
        buf.extend_from_slice(b"none"); // achVendID
        push_u16!(buf, 0u16); // fsSelection
        push_u16!(buf, 0u16); // usFirstCharIndex
        push_u16!(buf, 0u16); // usLastCharIndex
        push_i16!(buf, 800); // sTypoAscender
        push_i16!(buf, -200); // sTypoDescender
        push_i16!(buf, 0); // sTypoLineGap
        push_u16!(buf, 800u16); // usWinAscent
        push_u16!(buf, 200u16); // usWinDescent
        buf.extend_from_slice(&[0u8; 8]); // ulCodePageRange
        entries.push(TableEntry {
            tag: *b"OS/2",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === hmtx (4 bytes: one longHorMetric) ===
        let off = buf.len() as u32;
        push_u16!(buf, 600u16); // advanceWidth
        push_i16!(buf, 0); // lsb
        entries.push(TableEntry {
            tag: *b"hmtx",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === cmap (274 bytes: format 0 with 256-byte array) ===
        let off = buf.len() as u32;
        push_u16!(buf, 0u16); // version
        push_u16!(buf, 1u16); // numTables
        push_u16!(buf, 1u16); // platformID (Macintosh)
        push_u16!(buf, 0u16); // encodingID (Roman)
        push_u32!(buf, 12u32); // offset to subtable
                               // Format 0 subtable
        push_u16!(buf, 0u16); // format
        push_u16!(buf, 262u16); // length (6 + 256)
        push_u16!(buf, 0u16); // language
        buf.extend_from_slice(&[0u8; 256]); // all chars -> glyph 0
        entries.push(TableEntry {
            tag: *b"cmap",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === loca (4 bytes: 2 offsets for 1 glyph, short format) ===
        let off = buf.len() as u32;
        push_u16!(buf, 0u16); // offset[0]
        push_u16!(buf, 0u16); // offset[1] (glyph has zero length)
        entries.push(TableEntry {
            tag: *b"loca",
            offset: off,
            length: buf.len() as u32 - off,
        });
        pad4(&mut buf);

        // === glyf (0 bytes: empty, .notdef has no outline) ===
        let off = buf.len() as u32;
        entries.push(TableEntry {
            tag: *b"glyf",
            offset: off,
            length: 0,
        });

        // --- Write table directory entries (sorted by tag) ---
        entries.sort_by(|a, b| a.tag.cmp(&b.tag));
        for (i, e) in entries.iter().enumerate() {
            let slot = dir_start + i * 16;
            buf[slot..slot + 4].copy_from_slice(&e.tag);
            buf[slot + 4..slot + 8].copy_from_slice(&0u32.to_be_bytes()); // checksum
            buf[slot + 8..slot + 12].copy_from_slice(&e.offset.to_be_bytes());
            buf[slot + 12..slot + 16].copy_from_slice(&e.length.to_be_bytes());
        }

        buf
    }

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
