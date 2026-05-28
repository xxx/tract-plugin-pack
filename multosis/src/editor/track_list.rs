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

/// One of the track-list buttons inside an entry's `track_entry_rect`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackButton {
    /// The M (mute) toggle.
    Mute,
    /// The S (solo) toggle.
    Solo,
}

// Logical-pixel layout of the M/S buttons inside the track entry. The two
// buttons stack vertically in a single column near the right edge -- this
// frees ~16 px of horizontal room compared to a side-by-side layout, which
// the two-line effect name (family on top, suffix on bottom) needs to keep
// e.g. "Spectral Bandpass" readable without the buttons occluding it.
const BUTTON_W: f32 = 14.0;
const BUTTON_H: f32 = 14.0;
const BUTTON_X: f32 = 92.0;
const BUTTON_M_Y_OFF: f32 = 4.0;
const BUTTON_S_Y_OFF: f32 = 22.0;

/// Physical-pixel rect of a `button` inside row `row`'s entry.
pub fn track_button_rect(row: usize, button: TrackButton, scale: f32) -> (f32, f32, f32, f32) {
    let (ex, ey, _ew, _eh) = track_entry_rect(row, scale);
    let by_off = match button {
        TrackButton::Mute => BUTTON_M_Y_OFF,
        TrackButton::Solo => BUTTON_S_Y_OFF,
    };
    let bw = BUTTON_W * scale;
    let bh = BUTTON_H * scale;
    let bx = ex + BUTTON_X * scale;
    let by = ey + by_off * scale;
    (bx, by, bw, bh)
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

/// The M/S button under physical-pixel point `(px, py)` at `scale`, paired
/// with the row it belongs to. Returns `None` when the point is outside
/// every button. Used by the editor to route track-list clicks to mute/solo
/// toggles before falling back to the row-selection path.
pub fn track_button_at(px: f32, py: f32, scale: f32) -> Option<(usize, TrackButton)> {
    if scale <= 0.0 {
        return None;
    }
    for row in 0..ROWS {
        for button in [TrackButton::Mute, TrackButton::Solo] {
            let (bx, by, bw, bh) = track_button_rect(row, button, scale);
            if px >= bx && px < bx + bw && py >= by && py < by + bh {
                return Some((row, button));
            }
        }
    }
    None
}

/// Draw the track listing into `pixmap`. `kinds[r]` is row `r`'s effect kind;
/// bit `r` of `active_mask` lights row `r`'s "sounding" dot (the engine
/// already masks muted/non-soloed rows out of `active_mask`, so muted rows
/// stay dark automatically). `mutes[r]` / `solos[r]` drive the M/S buttons
/// and dim the effect name when effectively bypassed. `selected` is the
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
    mutes: &[bool; ROWS],
    solos: &[bool; ROWS],
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
    // PDC badge stripe: a thin vertical bar on the left edge of any
    // row whose effect kind reports latency AND is currently
    // contributing to the engine's chain-latency sum (i.e. NOT
    // bypassed via mute/solo). Teal stands out against the warm
    // brown/amber palette so the eye can scan a column of badges
    // without competing with the row text colours.
    let pdc_stripe_col = Color::from_rgba8(0x4A, 0xB8, 0xC8, 0xFF);
    let num_col = Color::from_rgba8(0x6A, 0x60, 0x52, 0xFF);
    let name_col = Color::from_rgba8(0x9A, 0x8A, 0x70, 0xFF);
    let sel_col = Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF);
    let dim_num_col = Color::from_rgba8(0x3A, 0x33, 0x2C, 0xFF);
    let dim_name_col = Color::from_rgba8(0x55, 0x4C, 0x3E, 0xFF);
    // Family caption: noticeably present but still subordinate to the
    // unique-suffix line. Sits between `dim_name_col` and `name_col` on the
    // same warm-brown axis so the two lines look like one label, not two
    // unrelated texts.
    let caption_col = Color::from_rgba8(0x78, 0x6C, 0x58, 0xFF);
    let dot_dark = Color::from_rgba8(0x2A, 0x24, 0x1E, 0xFF);
    let dot_live = Color::from_rgba8(0x5F, 0xC9, 0x6A, 0xFF);
    // Mute/solo button colours. Inactive buttons reuse the dim-name swatch
    // (subtle, doesn't fight the row text); active lights up red for Mute
    // and yellow for Solo — the standard DAW convention.
    let btn_off_bg = Color::from_rgba8(0x22, 0x1F, 0x1B, 0xFF);
    let btn_off_text = Color::from_rgba8(0x55, 0x4C, 0x3E, 0xFF);
    let btn_mute_bg = Color::from_rgba8(0xC0, 0x40, 0x3A, 0xFF);
    let btn_solo_bg = Color::from_rgba8(0xE8, 0xC9, 0x4A, 0xFF);
    let btn_active_text = Color::from_rgba8(0x10, 0x10, 0x10, 0xFF);

    // Track entries are CELL (40 px) tall — an 11 px font left them looking
    // tiny in that band. 15 px reads clearly and still leaves the number +
    // name + sounding-dot columns non-overlapping.
    let text_size = 15.0 * scale;
    // Kinds that opt into a family (via `EffectKind::family`) render as a
    // small family caption above the unique suffix; this keeps the part the
    // user is actually scanning ("Bandpass", "Twist") readable instead of
    // being truncated behind "Spectral".
    let caption_size = 10.0 * scale;
    let btn_text_size = 11.0 * scale;
    let any_soloed = solos.iter().any(|&s| s);
    for (row, &kind) in kinds.iter().enumerate() {
        let (x, y, w, h) = track_entry_rect(row, scale);
        let is_sel = selected == Some(row);
        let is_drag_source = drag_source == Some(row);
        let muted = mutes[row];
        let soloed = solos[row];
        // Effectively bypassed iff own mute is on OR some other row is soloed
        // and we aren't. Drives the row-name dim.
        let bypassed = muted || (any_soloed && !soloed);
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
        // PDC badge: drawn only when the effect kind reports latency
        // AND the row is currently contributing to chain latency
        // (matches `AudioEngine::chain_latency_samples`, which skips
        // bypassed rows). Stripe sits to the left of the row number
        // (number text starts at +6 px, so a 3 px-wide stripe at +1
        // leaves a 2 px breathing gap before the digit).
        if kind.reports_latency() && !bypassed {
            let sw = 3.0 * scale;
            let sh = h - 8.0 * scale;
            widgets::draw_rect(pixmap, x + scale, y + 4.0 * scale, sw, sh, pdc_stripe_col);
        }
        // Vertically centred text baseline — same formula `draw_button` uses.
        let ty = y + (h + text_size) * 0.5 - 2.0;
        // While dragging, the source row's text dims so the user can see
        // which entry is "in flight" without needing a floating ghost. A
        // bypassed row also dims, even when selected, so the user gets
        // immediate "this row isn't contributing" feedback.
        let (nc, mc) = if is_drag_source || bypassed {
            (dim_num_col, dim_name_col)
        } else if is_sel {
            (sel_col, sel_col)
        } else {
            (num_col, name_col)
        };
        // Family caption colour: the dedicated mid-warm swatch when the row is
        // active so the caption is legible without competing with the suffix.
        // Dragged / bypassed rows dim to the same swatch as the suffix so the
        // whole label fades together.
        let cc = if is_drag_source || bypassed {
            dim_name_col
        } else {
            caption_col
        };
        tr.draw_text(
            pixmap,
            x + 6.0 * scale,
            ty,
            &format!("{}", row + 1),
            text_size,
            nc,
        );
        // Family kinds render as caption + suffix on stacked lines; everything
        // else uses the original vertically-centred line. `name()` already
        // returns just the suffix for spectral kinds; the family caption is
        // sourced from `EffectKind::family()`.
        let name = kind.track_label();
        let name_x = x + 30.0 * scale;
        if let Some(fam) = kind.family() {
            let caption_ty = y + 14.0 * scale;
            let suffix_ty = y + 32.0 * scale;
            tr.draw_text(pixmap, name_x, caption_ty, fam, caption_size, cc);
            tr.draw_text(pixmap, name_x, suffix_ty, name, text_size, mc);
        } else {
            tr.draw_text(pixmap, name_x, ty, name, text_size, mc);
        }

        // M and S buttons. Hit-test geometry mirrors `track_button_rect`
        // exactly so a click on a glyph lands on the right toggle.
        let (mx, my, mw, mh) = track_button_rect(row, TrackButton::Mute, scale);
        let (sx_, sy_, sw, sh) = track_button_rect(row, TrackButton::Solo, scale);
        let m_bg = if muted { btn_mute_bg } else { btn_off_bg };
        let s_bg = if soloed { btn_solo_bg } else { btn_off_bg };
        let m_fg = if muted { btn_active_text } else { btn_off_text };
        let s_fg = if soloed {
            btn_active_text
        } else {
            btn_off_text
        };
        widgets::draw_rect(pixmap, mx, my, mw, mh, m_bg);
        widgets::draw_rect(pixmap, sx_, sy_, sw, sh, s_bg);
        // Centre the M/S glyphs in their buttons.
        let m_ty = my + (mh + btn_text_size) * 0.5 - 2.0;
        let s_ty = sy_ + (sh + btn_text_size) * 0.5 - 2.0;
        tr.draw_text_centered(pixmap, mx + mw * 0.5, m_ty, "M", btn_text_size, m_fg);
        tr.draw_text_centered(pixmap, sx_ + sw * 0.5, s_ty, "S", btn_text_size, s_fg);

        // "sounding" dot — a small square at the right edge. Moved 8 px to
        // the right of its previous position to clear the new S button.
        let lit = (active_mask >> row) & 1 != 0;
        let d = 6.0 * scale;
        widgets::draw_rect(
            pixmap,
            x + w - 10.0 * scale,
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
    fn track_button_at_returns_mute_for_centre_of_m_button() {
        for row in 0..ROWS {
            let (bx, by, bw, bh) = track_button_rect(row, TrackButton::Mute, 1.5);
            assert_eq!(
                track_button_at(bx + bw / 2.0, by + bh / 2.0, 1.5),
                Some((row, TrackButton::Mute)),
                "row {row} M centre"
            );
        }
    }

    #[test]
    fn track_button_at_returns_solo_for_centre_of_s_button() {
        for row in 0..ROWS {
            let (bx, by, bw, bh) = track_button_rect(row, TrackButton::Solo, 1.5);
            assert_eq!(
                track_button_at(bx + bw / 2.0, by + bh / 2.0, 1.5),
                Some((row, TrackButton::Solo)),
                "row {row} S centre"
            );
        }
    }

    #[test]
    fn track_button_at_misses_outside_both_buttons() {
        // Centre of an entry's name column should not be either button.
        let (ex, ey, _ew, eh) = track_entry_rect(3, 1.0);
        // Name column is at logical x = 24..74 of the entry, well left of
        // the M button at 76. Pick a point at logical x = 50 inside it.
        let px = ex + 50.0;
        let py = ey + eh / 2.0;
        assert_eq!(track_button_at(px, py, 1.0), None);
    }

    #[test]
    fn mute_and_solo_buttons_do_not_overlap_each_other_or_the_entry_edges() {
        // M sits above S in a single vertical column; the two must be
        // disjoint vertically and the column must stay inside the entry
        // (TRACK_PANEL_W minus the dot's right margin).
        for row in 0..ROWS {
            let (ex, ey, ew, eh) = track_entry_rect(row, 1.0);
            let (mx, my, mw, mh) = track_button_rect(row, TrackButton::Mute, 1.0);
            let (sx, sy, sw, sh) = track_button_rect(row, TrackButton::Solo, 1.0);
            assert_eq!(mx, sx, "row {row}: M/S must share an x column");
            assert_eq!(mw, sw, "row {row}: M/S widths differ");
            assert!(my + mh <= sy, "row {row}: M overlaps S vertically");
            assert!(
                sx + sw <= ex + ew,
                "row {row}: column overflows entry right"
            );
            assert!(
                my >= ey && sy + sh <= ey + eh,
                "row {row}: column overflows entry top/bottom"
            );
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
            kind: EffectKind::Svf,
            params: default_params_for_kind(EffectKind::Svf),
            mix: 0.42,
            muted: false,
            soloed: false,
        };
        effects[9] = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: default_params_for_kind(EffectKind::Bitcrush),
            mix: 0.81,
            muted: false,
            soloed: false,
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
        assert_eq!(effects[9].kind, EffectKind::Svf);
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
