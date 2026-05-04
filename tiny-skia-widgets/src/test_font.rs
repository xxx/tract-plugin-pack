//! Shared test-only helper that builds a minimal synthetic TrueType font.
//!
//! Real plugins ship a real font via `include_bytes!`; tests only need
//! something fontdue can parse without errors so the rasterisation /
//! compositing code paths can be exercised. This module is `#[cfg(test)]`
//! and is shared by every widget module that needs a `TextRenderer` in
//! its tests.

#![cfg(test)]

/// Build a minimal valid TrueType font with a single .notdef glyph (600
/// units advance width). All characters map to glyph 0, so `text_width`
/// will report the .notdef advance for every character. This is enough
/// for fontdue to parse without errors and for the rasterisation /
/// compositing code paths to be exercised.
pub fn test_font_data() -> Vec<u8> {
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
        while !buf.len().is_multiple_of(4) {
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
    entries.sort_by_key(|e| e.tag);
    for (i, e) in entries.iter().enumerate() {
        let slot = dir_start + i * 16;
        buf[slot..slot + 4].copy_from_slice(&e.tag);
        buf[slot + 4..slot + 8].copy_from_slice(&0u32.to_be_bytes()); // checksum
        buf[slot + 8..slot + 12].copy_from_slice(&e.offset.to_be_bytes());
        buf[slot + 12..slot + 16].copy_from_slice(&e.length.to_be_bytes());
    }

    buf
}
