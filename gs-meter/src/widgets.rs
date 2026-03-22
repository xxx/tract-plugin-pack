//! Minimal widget primitives for rendering a meter plugin GUI using tiny-skia and fontdue.
//!
//! All drawing targets a [`tiny_skia::Pixmap`]. Coordinates are in physical pixels;
//! the caller is responsible for DPI scaling. No event handling lives here — only
//! pure drawing functions.

use std::collections::HashMap;
use tiny_skia::{Color, Paint, Pixmap, PremultipliedColorU8, Rect, Transform};

// ---------------------------------------------------------------------------
// Color constants — dark theme matching the vizia CSS in style.css
// ---------------------------------------------------------------------------

// Color helper and theme constants.
// tiny-skia's Color::from_rgba8 is not const, so we use inline functions.

#[inline]
pub fn color_bg() -> Color { Color::from_rgba8(0x1a, 0x1c, 0x22, 0xff) }
#[inline]
pub fn color_text() -> Color { Color::from_rgba8(0xe0, 0xe0, 0xe0, 0xff) }
#[inline]
pub fn color_accent() -> Color { Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xff) }
#[inline]
pub fn color_muted() -> Color { Color::from_rgba8(0xa0, 0xa0, 0xa0, 0xff) }
#[inline]
pub fn color_control_bg() -> Color { Color::from_rgba8(0x2a, 0x2c, 0x32, 0xff) }
#[inline]
pub fn color_border() -> Color { Color::from_rgba8(0x40, 0x40, 0x40, 0xff) }

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
        self.cache.entry(key).or_insert_with_key(|k| {
            let (metrics, bitmap) = self.font.rasterize(k.ch, size);
            GlyphEntry { bitmap, metrics }
        })
    }
}

// ---------------------------------------------------------------------------
// Primitive drawing helpers
// ---------------------------------------------------------------------------

/// Fill a rectangle on `pixmap`.
pub fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: Color) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
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
    draw_rect(pixmap, x, y + stroke_width, stroke_width, h - 2.0 * stroke_width, color);
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
// Composite widgets
// ---------------------------------------------------------------------------

/// Draw a button with a centred label.
///
/// `hovered` brightens the background slightly; `pressed` darkens it.
pub fn draw_button(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    hovered: bool,
    pressed: bool,
) {
    let bg = if pressed {
        Color::from_rgba8(0x20, 0x22, 0x28, 0xff)
    } else if hovered {
        Color::from_rgba8(0x35, 0x37, 0x3e, 0xff)
    } else {
        color_control_bg()
    };

    draw_rect(pixmap, x, y, w, h, bg);
    draw_rect_outline(pixmap, x, y, w, h, color_border(), 1.0);

    // Centre the label inside the button.
    let text_size = (h * 0.5).max(10.0);
    let tw = text_renderer.text_width(label, text_size);
    let tx = x + (w - tw) * 0.5;
    let ty = y + (h + text_size) * 0.5 - 2.0; // approximate baseline offset
    text_renderer.draw_text(pixmap, tx, ty, label, text_size, color_text());
}

/// Draw a horizontal slider with a fill bar, a left-aligned label, and a
/// right-aligned value string.
///
/// `normalized_value` should be in 0.0..=1.0.
pub fn draw_slider(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    value_text: &str,
    normalized_value: f32,
) {
    let nv = normalized_value.clamp(0.0, 1.0);

    // Track background
    draw_rect(pixmap, x, y, w, h, color_control_bg());
    draw_rect_outline(pixmap, x, y, w, h, color_border(), 1.0);

    // Fill bar
    let fill_w = (w - 2.0) * nv;
    if fill_w > 0.0 {
        draw_rect(pixmap, x + 1.0, y + 1.0, fill_w, h - 2.0, color_accent());
    }

    // Label (left-aligned, vertically centred)
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;
    let pad = 6.0;
    text_renderer.draw_text(pixmap, x + pad, text_y, label, text_size, color_text());

    // Value text (right-aligned)
    let vw = text_renderer.text_width(value_text, text_size);
    text_renderer.draw_text(
        pixmap,
        x + w - vw - pad,
        text_y,
        value_text,
        text_size,
        color_text(),
    );
}

/// Draw a segmented control (stepped selector).
///
/// Each segment is an equal-width button; the one at `active_index` is
/// highlighted with the accent colour.
pub fn draw_stepped_selector(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    options: &[&str],
    active_index: usize,
) {
    if options.is_empty() {
        return;
    }

    let seg_w = w / options.len() as f32;
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;

    for (i, &opt) in options.iter().enumerate() {
        let sx = x + i as f32 * seg_w;
        let is_active = i == active_index;

        let bg = if is_active {
            color_accent()
        } else {
            color_control_bg()
        };
        let fg = if is_active {
            Color::from_rgba8(0x10, 0x10, 0x10, 0xff)
        } else {
            color_text()
        };

        draw_rect(pixmap, sx, y, seg_w, h, bg);
        draw_rect_outline(pixmap, sx, y, seg_w, h, color_border(), 1.0);

        let tw = text_renderer.text_width(opt, text_size);
        let tx = sx + (seg_w - tw) * 0.5;
        text_renderer.draw_text(pixmap, tx, text_y, opt, text_size, fg);
    }
}

/// Draw a meter row: a left label, a value string, and an optional
/// "-> Gain" button.
///
/// This mirrors the `meter_row` helper in the vizia-based editor. Returns the
/// bounding rectangle of the button (if drawn) so the caller can do hit
/// testing.
pub fn draw_meter_row(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    value_text: &str,
    has_button: bool,
    button_hovered: bool,
) -> Option<[f32; 4]> {
    let row_h: f32 = 28.0;
    let text_size: f32 = 13.0;
    let text_y = y + (row_h + text_size) * 0.5 - 2.0;

    let label_w: f32 = 100.0;
    let value_w: f32 = 120.0;
    let gap: f32 = 10.0;

    // Label
    text_renderer.draw_text(pixmap, x, text_y, label, text_size, color_muted());

    // Value
    text_renderer.draw_text(
        pixmap,
        x + label_w + gap,
        text_y,
        value_text,
        text_size,
        color_text(),
    );

    // Optional button
    if has_button {
        let btn_x = x + label_w + gap + value_w + gap;
        let btn_w: f32 = 70.0;
        let btn_h: f32 = 24.0;
        let btn_y = y + (row_h - btn_h) * 0.5;

        draw_button(
            pixmap,
            text_renderer,
            btn_x,
            btn_y,
            btn_w,
            btn_h,
            "\u{2192} Gain", // "-> Gain" using Unicode right arrow
            button_hovered,
            false,
        );

        Some([btn_x, btn_y, btn_w, btn_h])
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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

        macro_rules! push_i16 { ($buf:expr, $v:expr) => { $buf.extend_from_slice(&($v as i16).to_be_bytes()); }; }
        macro_rules! push_u16 { ($buf:expr, $v:expr) => { $buf.extend_from_slice(&($v as u16).to_be_bytes()); }; }
        macro_rules! push_u32 { ($buf:expr, $v:expr) => { $buf.extend_from_slice(&($v as u32).to_be_bytes()); }; }

        // === head (54 bytes) ===
        let off = buf.len() as u32;
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // version 1.0
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // fontRevision
        push_u32!(buf, 0u32); // checksumAdjustment
        push_u32!(buf, 0x5F0F3CF5u32); // magicNumber
        push_u16!(buf, 0x000Bu16); // flags
        push_u16!(buf, 1000u16); // unitsPerEm
        buf.extend_from_slice(&[0u8; 16]); // created + modified
        push_i16!(buf, 0); push_i16!(buf, 0); // xMin, yMin
        push_i16!(buf, 0); push_i16!(buf, 0); // xMax, yMax
        push_u16!(buf, 0u16); // macStyle
        push_u16!(buf, 8u16); // lowestRecPPEM
        push_i16!(buf, 2); // fontDirectionHint
        push_i16!(buf, 0); // indexToLocFormat (short)
        push_i16!(buf, 0); // glyphDataFormat
        entries.push(TableEntry { tag: *b"head", offset: off, length: buf.len() as u32 - off });
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
        entries.push(TableEntry { tag: *b"hhea", offset: off, length: buf.len() as u32 - off });
        pad4(&mut buf);

        // === maxp (32 bytes) ===
        let off = buf.len() as u32;
        buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // version 1.0
        push_u16!(buf, 1u16); // numGlyphs
        buf.extend_from_slice(&[0u8; 26]); // remaining fields
        entries.push(TableEntry { tag: *b"maxp", offset: off, length: buf.len() as u32 - off });
        pad4(&mut buf);

        // === OS/2 (version 1, 86 bytes) ===
        let off = buf.len() as u32;
        push_u16!(buf, 1u16); // version
        push_i16!(buf, 600); // xAvgCharWidth
        push_u16!(buf, 400u16); // usWeightClass
        push_u16!(buf, 5u16); // usWidthClass
        push_u16!(buf, 0u16); // fsType
        buf.extend_from_slice(&[0u8; 20]); // subscript/superscript
        push_i16!(buf, 0); push_i16!(buf, 0); // strikeout
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
        entries.push(TableEntry { tag: *b"OS/2", offset: off, length: buf.len() as u32 - off });
        pad4(&mut buf);

        // === hmtx (4 bytes: one longHorMetric) ===
        let off = buf.len() as u32;
        push_u16!(buf, 600u16); // advanceWidth
        push_i16!(buf, 0); // lsb
        entries.push(TableEntry { tag: *b"hmtx", offset: off, length: buf.len() as u32 - off });
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
        entries.push(TableEntry { tag: *b"cmap", offset: off, length: buf.len() as u32 - off });
        pad4(&mut buf);

        // === loca (4 bytes: 2 offsets for 1 glyph, short format) ===
        let off = buf.len() as u32;
        push_u16!(buf, 0u16); // offset[0]
        push_u16!(buf, 0u16); // offset[1] (glyph has zero length)
        entries.push(TableEntry { tag: *b"loca", offset: off, length: buf.len() as u32 - off });
        pad4(&mut buf);

        // === glyf (0 bytes: empty, .notdef has no outline) ===
        let off = buf.len() as u32;
        entries.push(TableEntry { tag: *b"glyf", offset: off, length: 0 });

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
    fn test_draw_button_states() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(200, 50).unwrap();
        draw_button(&mut pm, &mut renderer, 5.0, 5.0, 80.0, 30.0, "OK", false, false);
        draw_button(&mut pm, &mut renderer, 5.0, 5.0, 80.0, 30.0, "OK", true, false);
        draw_button(&mut pm, &mut renderer, 5.0, 5.0, 80.0, 30.0, "OK", false, true);
    }

    #[test]
    fn test_draw_slider() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_slider(
            &mut pm, &mut renderer,
            5.0, 5.0, 250.0, 28.0,
            "Gain", "-3.0 dB", 0.5,
        );
        // Fill should cover roughly the left half of the slider track.
        let left_px = pixel_at(&pm, 10, 18);
        assert!(left_px.alpha() > 0, "slider fill area should be drawn");
    }

    #[test]
    fn test_draw_slider_clamps_value() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        // Values outside 0..1 should be clamped, not panic.
        draw_slider(&mut pm, &mut renderer, 0.0, 0.0, 200.0, 28.0, "X", "0", -0.5);
        draw_slider(&mut pm, &mut renderer, 0.0, 0.0, 200.0, 28.0, "X", "0", 1.5);
    }

    #[test]
    fn test_draw_stepped_selector() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_stepped_selector(
            &mut pm, &mut renderer,
            5.0, 5.0, 250.0, 28.0,
            &["Stereo", "Left", "Right"], 1,
        );
    }

    #[test]
    fn test_draw_stepped_selector_empty_options() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(100, 50).unwrap();
        draw_stepped_selector(&mut pm, &mut renderer, 0.0, 0.0, 100.0, 28.0, &[], 0);
    }

    #[test]
    fn test_draw_meter_row_with_button() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(400, 50).unwrap();
        let btn = draw_meter_row(
            &mut pm, &mut renderer,
            5.0, 5.0, 380.0,
            "Peak Max", "-1.2 dB", true, false,
        );
        assert!(btn.is_some(), "should return button rect when has_button=true");
        let [_bx, _by, bw, bh] = btn.unwrap();
        assert!(bw > 0.0 && bh > 0.0, "button should have positive dimensions");
    }

    #[test]
    fn test_draw_meter_row_without_button() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(400, 50).unwrap();
        let btn = draw_meter_row(
            &mut pm, &mut renderer,
            5.0, 5.0, 380.0,
            "Crest", "12.3 dB", false, false,
        );
        assert!(btn.is_none(), "should return None when has_button=false");
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
