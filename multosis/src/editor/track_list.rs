//! The always-visible left-edge track listing — Phase 2 Milestone 2c. One
//! entry per track row: number, effect-kind name, and a "currently sounding"
//! dot. Clicking an entry opens that track's effect editor.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` §2.

use crate::editor::grid_view::{CELL, GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
use crate::effects::EffectKind;
use crate::grid::ROWS;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// The physical-pixel rectangle `(x, y, w, h)` of track-listing entry `row`
/// at `scale`. Entries align vertically with the grid rows.
pub fn track_entry_rect(row: usize, scale: f32) -> (f32, f32, f32, f32) {
    let x = MARGIN * scale;
    let y = (STATUS_H + GUTTER + row as f32 * CELL) * scale;
    (x, y, TRACK_PANEL_W * scale, CELL * scale)
}

/// The track-listing entry under physical-pixel point `(px, py)` at `scale`,
/// or `None` if the point is outside the panel.
pub fn track_at(px: f32, py: f32, scale: f32) -> Option<usize> {
    if scale <= 0.0 {
        return None;
    }
    for row in 0..ROWS {
        let (x, y, w, h) = track_entry_rect(row, scale);
        if px >= x && px < x + w && py >= y && py < y + h {
            return Some(row);
        }
    }
    None
}

/// Draw the track listing into `pixmap`. `kinds[r]` is row `r`'s effect kind;
/// bit `r` of `active_mask` lights row `r`'s "sounding" dot; `selected` is the
/// row highlighted while the effect editor is open (`None` in the grid view).
pub fn draw_track_list(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    kinds: &[EffectKind; ROWS],
    active_mask: u16,
    selected: Option<usize>,
    scale: f32,
) {
    use tiny_skia::Color;
    let panel_bg = Color::from_rgba8(0x15, 0x12, 0x0F, 0xFF);
    let sel_bg = Color::from_rgba8(0x3A, 0x2F, 0x22, 0xFF);
    let border = Color::from_rgba8(0x3A, 0x34, 0x2E, 0xFF);
    let num_col = Color::from_rgba8(0x6A, 0x60, 0x52, 0xFF);
    let name_col = Color::from_rgba8(0x9A, 0x8A, 0x70, 0xFF);
    let sel_col = Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF);
    let dot_dark = Color::from_rgba8(0x2A, 0x24, 0x1E, 0xFF);
    let dot_live = Color::from_rgba8(0x5F, 0xC9, 0x6A, 0xFF);

    // Track entries are CELL (40 px) tall — an 11 px font left them looking
    // tiny in that band. 15 px reads clearly and still leaves the number +
    // name + sounding-dot columns non-overlapping.
    let text_size = 15.0 * scale;
    for (row, &kind) in kinds.iter().enumerate() {
        let (x, y, w, h) = track_entry_rect(row, scale);
        let is_sel = selected == Some(row);
        widgets::draw_rect(pixmap, x, y, w, h, if is_sel { sel_bg } else { panel_bg });
        // bottom hairline
        widgets::draw_rect(pixmap, x, y + h - scale, w, scale, border);
        // Vertically centred text baseline — same formula `draw_button` uses.
        let ty = y + (h + text_size) * 0.5 - 2.0;
        // track number (1-based)
        tr.draw_text(
            pixmap,
            x + 6.0 * scale,
            ty,
            &format!("{}", row + 1),
            text_size,
            if is_sel { sel_col } else { num_col },
        );
        // effect kind name
        tr.draw_text(
            pixmap,
            x + 30.0 * scale,
            ty,
            kind.name(),
            text_size,
            if is_sel { sel_col } else { name_col },
        );
        // "sounding" dot — a small square at the right edge
        let lit = (active_mask >> row) & 1 != 0;
        let d = 8.0 * scale;
        widgets::draw_rect(
            pixmap,
            x + w - 16.0 * scale,
            y + (h - d) / 2.0,
            d,
            d,
            if lit { dot_live } else { dot_dark },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_at_round_trips_each_entry() {
        for row in 0..ROWS {
            let (x, y, w, h) = track_entry_rect(row, 1.5);
            assert_eq!(track_at(x + w / 2.0, y + h / 2.0, 1.5), Some(row));
        }
    }

    #[test]
    fn track_at_misses_outside_the_panel() {
        // Above the first entry (in the toolbar strip) — no hit.
        assert_eq!(track_at(MARGIN + 1.0, 1.0, 1.0), None);
        // Right of the panel (over the grid) — no hit.
        let (x, y, w, h) = track_entry_rect(0, 1.0);
        assert_eq!(track_at(x + w + 50.0, y + h / 2.0, 1.0), None);
    }
}
