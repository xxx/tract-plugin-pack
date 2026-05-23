//! The always-visible left-edge track listing — Phase 2 Milestone 2c. One
//! entry per track row: number, effect-kind name, and a "currently sounding"
//! dot. Clicking an entry opens that track's effect editor; pressing and
//! dragging an entry onto another swaps the two tracks (drag-and-drop
//! reorder) — see [`TrackDrag`] and [`swap_rows_pure`].
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` §2.

use crate::editor::grid_view::{CELL, GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
use crate::effects::{EffectKind, TrackEffect};
use crate::grid::{Grid, COLS, ROWS};
use crate::modulation::TrackModulation;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// In-flight drag-and-drop reorder of the track listing.
///
/// `from` is the row the press began on; `current_y` is the most recent
/// cursor y position in physical pixels (updated on each `CursorMoved`).
/// The drop target is computed at release time by hit-testing `current_y`
/// against the track entries (see [`track_at`]).
#[derive(Clone, Copy, Debug)]
pub struct TrackDrag {
    pub from: usize,
    pub current_y: f32,
}

/// Pure track-swap on the editor-owned config: exchanges `a`'s and `b`'s
/// grid cells (all 32 columns), per-track effect config, and per-track
/// modulation config. A no-op when `a == b` or either index is out of range.
///
/// This is the GUI-thread half of the swap. The audio thread runs its own
/// `AudioEngine::swap_tracks` against the same row pair so live DSP state
/// (effect instances, MSEG phases, amplitudes) follows the moved tracks.
pub fn swap_rows_pure(
    grid: &mut Grid,
    effects: &mut [TrackEffect; ROWS],
    modulation: &mut [TrackModulation; ROWS],
    a: usize,
    b: usize,
) {
    if a == b || a >= ROWS || b >= ROWS {
        return;
    }
    for col in 0..COLS {
        let ia = Grid::index(a, col);
        let ib = Grid::index(b, col);
        grid.cells.swap(ia, ib);
    }
    effects.swap(a, b);
    modulation.swap(a, b);
}

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
///
/// `drag_source` dims the row the user is dragging; `drag_target` outlines the
/// row currently under the cursor when it differs from the source. Both are
/// `None` when no drag is in flight.
#[allow(clippy::too_many_arguments)]
pub fn draw_track_list(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    kinds: &[EffectKind; ROWS],
    active_mask: u16,
    selected: Option<usize>,
    drag_source: Option<usize>,
    drag_target: Option<usize>,
    scale: f32,
) {
    use tiny_skia::Color;
    let panel_bg = Color::from_rgba8(0x15, 0x12, 0x0F, 0xFF);
    let sel_bg = Color::from_rgba8(0x3A, 0x2F, 0x22, 0xFF);
    let drag_src_bg = Color::from_rgba8(0x10, 0x0D, 0x0A, 0xFF);
    let drag_target_outline = Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF);
    let border = Color::from_rgba8(0x3A, 0x34, 0x2E, 0xFF);
    let num_col = Color::from_rgba8(0x6A, 0x60, 0x52, 0xFF);
    let name_col = Color::from_rgba8(0x9A, 0x8A, 0x70, 0xFF);
    let sel_col = Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF);
    let dim_num_col = Color::from_rgba8(0x3A, 0x33, 0x2C, 0xFF);
    let dim_name_col = Color::from_rgba8(0x55, 0x4C, 0x3E, 0xFF);
    let dot_dark = Color::from_rgba8(0x2A, 0x24, 0x1E, 0xFF);
    let dot_live = Color::from_rgba8(0x5F, 0xC9, 0x6A, 0xFF);

    // Track entries are CELL (40 px) tall — an 11 px font left them looking
    // tiny in that band. 15 px reads clearly and still leaves the number +
    // name + sounding-dot columns non-overlapping.
    let text_size = 15.0 * scale;
    for (row, &kind) in kinds.iter().enumerate() {
        let (x, y, w, h) = track_entry_rect(row, scale);
        let is_sel = selected == Some(row);
        let is_drag_source = drag_source == Some(row);
        let bg = if is_drag_source {
            drag_src_bg
        } else if is_sel {
            sel_bg
        } else {
            panel_bg
        };
        widgets::draw_rect(pixmap, x, y, w, h, bg);
        // bottom hairline
        widgets::draw_rect(pixmap, x, y + h - scale, w, scale, border);
        // Vertically centred text baseline — same formula `draw_button` uses.
        let ty = y + (h + text_size) * 0.5 - 2.0;
        // While dragging, the source row's text dims so the user can see
        // which entry is "in flight" without needing a floating ghost.
        let (nc, mc) = if is_drag_source {
            (dim_num_col, dim_name_col)
        } else if is_sel {
            (sel_col, sel_col)
        } else {
            (num_col, name_col)
        };
        tr.draw_text(
            pixmap,
            x + 6.0 * scale,
            ty,
            &format!("{}", row + 1),
            text_size,
            nc,
        );
        tr.draw_text(pixmap, x + 30.0 * scale, ty, kind.name(), text_size, mc);
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

    // Draw the drop-target outline last so it overlays the source dim and
    // the selection highlight. Skip when target == source (no movement yet).
    if let Some(t) = drag_target {
        if drag_source != Some(t) {
            let (x, y, w, h) = track_entry_rect(t, scale);
            let t_px = (2.0 * scale).max(1.0);
            widgets::draw_rect(pixmap, x, y, w, t_px, drag_target_outline);
            widgets::draw_rect(pixmap, x, y + h - t_px, w, t_px, drag_target_outline);
            widgets::draw_rect(pixmap, x, y, t_px, h, drag_target_outline);
            widgets::draw_rect(pixmap, x + w - t_px, y, t_px, h, drag_target_outline);
        }
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

    #[test]
    fn swap_rows_pure_exchanges_grid_effects_and_modulation() {
        use crate::effects::{default_params_for_kind, EffectKind, TrackEffect};
        use crate::modulation::{TrackModulation, TriggerSource};
        // Default `Cell` is `enabled: true` everywhere. Blank row 2 entirely
        // while leaving row 9 at its default all-on state so the swap's effect
        // on the grid is observable end-to-end across all 32 columns.
        let mut grid = Grid::default();
        for col in 0..COLS {
            grid.cell_mut(2, col).enabled = false;
        }
        let mut effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        effects[2] = TrackEffect {
            kind: EffectKind::Lowpass,
            params: default_params_for_kind(EffectKind::Lowpass),
            mix: 0.42,
        };
        effects[9] = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: default_params_for_kind(EffectKind::Bitcrush),
            mix: 0.81,
        };
        let mut modu: [TrackModulation; ROWS] =
            std::array::from_fn(TrackModulation::default_for_row);
        modu[2].trigger = TriggerSource::CellLight;
        modu[9].trigger = TriggerSource::CellStep;

        swap_rows_pure(&mut grid, &mut effects, &mut modu, 2, 9);

        // Row 2 picks up row 9's all-on; row 9 picks up row 2's all-off.
        for col in 0..COLS {
            assert!(grid.cell(2, col).enabled, "row 2 col {col} after swap");
            assert!(!grid.cell(9, col).enabled, "row 9 col {col} after swap");
        }
        // Effects swapped.
        assert_eq!(effects[2].kind, EffectKind::Bitcrush);
        assert!((effects[2].mix - 0.81).abs() < 1e-6);
        assert_eq!(effects[9].kind, EffectKind::Lowpass);
        assert!((effects[9].mix - 0.42).abs() < 1e-6);
        // Modulation swapped.
        assert_eq!(modu[2].trigger, TriggerSource::CellStep);
        assert_eq!(modu[9].trigger, TriggerSource::CellLight);
    }

    #[test]
    fn swap_rows_pure_is_a_noop_when_indices_match_or_are_out_of_range() {
        use crate::effects::TrackEffect;
        use crate::modulation::TrackModulation;
        let mut grid = Grid::default();
        grid.cell_mut(3, 5).enabled = true;
        let mut effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        let mut modu: [TrackModulation; ROWS] =
            std::array::from_fn(TrackModulation::default_for_row);

        swap_rows_pure(&mut grid, &mut effects, &mut modu, 3, 3);
        assert!(grid.cell(3, 5).enabled);

        swap_rows_pure(&mut grid, &mut effects, &mut modu, 3, ROWS + 4);
        assert!(grid.cell(3, 5).enabled);

        swap_rows_pure(&mut grid, &mut effects, &mut modu, ROWS + 4, 3);
        assert!(grid.cell(3, 5).enabled);
    }
}
