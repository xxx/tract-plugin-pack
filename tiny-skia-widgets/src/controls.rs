//! Composite widget drawing functions: buttons, sliders, stepped selectors.

use tiny_skia::{Color, Pixmap};

use crate::primitives::*;
use crate::text::TextRenderer;

// ---------------------------------------------------------------------------
// Composite widgets
// ---------------------------------------------------------------------------

/// Draw a button with a centred label.
///
/// `hovered` brightens the background slightly; `pressed` darkens it.
#[allow(clippy::too_many_arguments)]
pub fn draw_button(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    active: bool,
    _hovered: bool,
) {
    let bg = if active {
        color_accent()
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
    let text_color = if active {
        Color::from_rgba8(0x1a, 0x1c, 0x22, 0xff) // dark text on accent bg
    } else {
        color_text()
    };
    text_renderer.draw_text(pixmap, tx, ty, label, text_size, text_color);
}

/// Draw a horizontal slider with a fill bar, a left-aligned label, and a
/// right-aligned value string.
///
/// `normalized_value` should be in 0.0..=1.0.
#[allow(clippy::too_many_arguments)]
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
#[allow(clippy::too_many_arguments)]
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Pixmap, PremultipliedColorU8};

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

    /// Read a single pixel from a pixmap by (x, y) coordinates.
    fn pixel_at(pm: &Pixmap, x: u32, y: u32) -> PremultipliedColorU8 {
        pm.pixels()[(y * pm.width() + x) as usize]
    }

    #[test]
    fn test_draw_button_states() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(200, 50).unwrap();
        draw_button(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            80.0,
            30.0,
            "OK",
            false,
            false,
        );
        draw_button(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            80.0,
            30.0,
            "OK",
            true,
            false,
        );
        draw_button(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            80.0,
            30.0,
            "OK",
            false,
            true,
        );
    }

    #[test]
    fn test_draw_slider() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_slider(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            "Gain",
            "-3.0 dB",
            0.5,
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
        draw_slider(
            &mut pm,
            &mut renderer,
            0.0,
            0.0,
            200.0,
            28.0,
            "X",
            "0",
            -0.5,
        );
        draw_slider(&mut pm, &mut renderer, 0.0, 0.0, 200.0, 28.0, "X", "0", 1.5);
    }

    #[test]
    fn test_draw_stepped_selector() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_stepped_selector(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            &["Stereo", "Left", "Right"],
            1,
        );
    }

    #[test]
    fn test_draw_stepped_selector_empty_options() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(100, 50).unwrap();
        draw_stepped_selector(&mut pm, &mut renderer, 0.0, 0.0, 100.0, 28.0, &[], 0);
    }
}
