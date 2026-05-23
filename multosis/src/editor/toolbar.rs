//! The editor toolbar: parameter controls + Reset, laid out across the upper
//! row of the top strip. Geometry is logical; every draw multiplies by scale.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7.

use crate::editor::grid_view::{MARGIN, TOOLBAR_ROW_H};
use crate::editor::WINDOW_WIDTH;
use crate::MultosisParams;
use nih_plug::prelude::Param;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// One toolbar control in the upper row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarControl {
    /// Cycles the playhead speed.
    Speed,
    /// Drag slider — dry/wet mix.
    Mix,
    /// Drag slider — output gain.
    Output,
    /// Drag slider — wet-bus compressor threshold (dBFS).
    CompThreshold,
    /// Drag slider — wet-bus compressor ratio.
    CompRatio,
    /// Resets the sequence.
    Reset,
}

impl ToolbarControl {
    /// The six controls, left to right.
    pub const ALL: [ToolbarControl; 6] = [
        ToolbarControl::Speed,
        ToolbarControl::Mix,
        ToolbarControl::Output,
        ToolbarControl::CompThreshold,
        ToolbarControl::CompRatio,
        ToolbarControl::Reset,
    ];

    /// Logical `(x, width)` of this control in the 1044-px-wide design row.
    /// Per-button widths are sized to their labels; the row is split into
    /// three concern groups (sequencer / master / compressor) separated by a
    /// wider gap, and the `Reset` action button is right-anchored so the
    /// middle of the row reads as deliberate empty space.
    fn logical_x_w(self) -> (f32, f32) {
        // within-group gap = 4, between-group gap = 16, right pad = 6.
        match self {
            ToolbarControl::Speed => (6.0, 130.0),
            ToolbarControl::Mix => (152.0, 130.0),
            ToolbarControl::Output => (286.0, 130.0),
            ToolbarControl::CompThreshold => (432.0, 130.0),
            ToolbarControl::CompRatio => (566.0, 130.0),
            ToolbarControl::Reset => (958.0, 80.0),
        }
    }
}

/// Vertical inset of a control within its toolbar row, logical px.
const CTRL_INSET: f32 = 4.0;

/// The toolbar layout was authored for a 1056-wide window inset 6 px each
/// side (content span 1044 px). `remap` affinely maps an old logical
/// `(x, width)` onto the current window's content span
/// `[MARGIN, WINDOW_WIDTH - MARGIN]`, preserving every item's relative
/// position and width — so a future window-width change re-fits the toolbar.
fn remap(lx: f32, lw: f32) -> (f32, f32) {
    let span = (WINDOW_WIDTH as f32 - 2.0 * MARGIN) / 1044.0;
    (MARGIN + (lx - 6.0) * span, lw * span)
}

/// The physical-pixel rectangle `(x, y, w, h)` of `ctrl` at `scale`.
pub fn control_rect(ctrl: ToolbarControl, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = ctrl.logical_x_w();
    let (rx, rw) = remap(lx, lw);
    let x = rx * scale;
    let y = CTRL_INSET * scale;
    let w = rw * scale;
    let h = (TOOLBAR_ROW_H - 2.0 * CTRL_INSET) * scale;
    (x, y, w, h)
}

/// The toolbar control under physical-pixel point `(px, py)` at `scale`, or
/// `None` if the point hits no control.
pub fn toolbar_hit(px: f32, py: f32, scale: f32) -> Option<ToolbarControl> {
    ToolbarControl::ALL.into_iter().find(|&ctrl| {
        let (x, y, w, h) = control_rect(ctrl, scale);
        px >= x && px < x + w && py >= y && py < y + h
    })
}

/// One grid-operation button in the lower toolbar row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarOp {
    /// Restore default activations (every cell enabled).
    ReinitCells,
    /// Randomize the enabled flags in the loop region.
    RandomizeActivations,
    /// Copy the loop region to the clipboard.
    Copy,
    /// Paste the clipboard at the loop region.
    Paste,
}

impl ToolbarOp {
    /// The four operations, left to right.
    pub const ALL: [ToolbarOp; 4] = [
        ToolbarOp::ReinitCells,
        ToolbarOp::RandomizeActivations,
        ToolbarOp::Copy,
        ToolbarOp::Paste,
    ];

    /// Logical `(x, width)` of this op button in the 1044-px-wide design row.
    /// Regen actions (Reinit, Randomize) cluster on the left; clipboard
    /// actions (Copy, Paste) cluster right-anchored. The middle is deliberate
    /// empty space.
    fn logical_x_w(self) -> (f32, f32) {
        match self {
            ToolbarOp::ReinitCells => (6.0, 120.0),
            ToolbarOp::RandomizeActivations => (130.0, 100.0),
            ToolbarOp::Copy => (894.0, 70.0),
            ToolbarOp::Paste => (968.0, 70.0),
        }
    }

    /// The button's centred label.
    pub fn label(self) -> &'static str {
        match self {
            ToolbarOp::ReinitCells => "Reinit Cells",
            ToolbarOp::RandomizeActivations => "Rnd Cells",
            ToolbarOp::Copy => "Copy",
            ToolbarOp::Paste => "Paste",
        }
    }
}

/// The physical-pixel rectangle `(x, y, w, h)` of op button `op` at `scale`.
/// The op buttons live in the toolbar's lower row.
pub fn op_rect(op: ToolbarOp, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = op.logical_x_w();
    let (rx, rw) = remap(lx, lw);
    let x = rx * scale;
    let y = (TOOLBAR_ROW_H + CTRL_INSET) * scale;
    let w = rw * scale;
    let h = (TOOLBAR_ROW_H - 2.0 * CTRL_INSET) * scale;
    (x, y, w, h)
}

/// The op button under physical-pixel point `(px, py)` at `scale`, or `None`.
pub fn op_hit(px: f32, py: f32, scale: f32) -> Option<ToolbarOp> {
    ToolbarOp::ALL.into_iter().find(|&op| {
        let (x, y, w, h) = op_rect(op, scale);
        px >= x && px < x + w && py >= y && py < y + h
    })
}

/// Apply a grid-mutating operation in place. `ReinitCells` ignores `seed`;
/// `RandomizeActivations` is deterministic in it. `Copy`/`Paste` are NOT
/// handled here — they need the editor's clipboard, so this is a no-op for
/// them.
pub fn apply_grid_op(grid: &mut crate::grid::Grid, op: ToolbarOp, seed: u32) {
    match op {
        ToolbarOp::ReinitCells => grid.reinit(),
        ToolbarOp::RandomizeActivations => crate::randomize::randomize_activations(grid, seed),
        ToolbarOp::Copy | ToolbarOp::Paste => {}
    }
}

/// Draw the toolbar strip and its six upper-row controls.
pub fn draw_toolbar(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    params: &MultosisParams,
    seq_status: &crate::seq_status::SeqStatusDisplay,
    scale: f32,
    speed_dropdown_open: bool,
) {
    // The whole two-row strip background.
    let strip_h = crate::editor::grid_view::STATUS_H * scale;
    widgets::draw_rect(
        pixmap,
        0.0,
        0.0,
        pixmap.width() as f32,
        strip_h,
        widgets::color_control_bg(),
    );

    for ctrl in ToolbarControl::ALL {
        let (x, y, w, h) = control_rect(ctrl, scale);
        match ctrl {
            ToolbarControl::Speed => {
                let label = format!("Speed: {}", speed_label(params.speed.value()));
                widgets::dropdown::draw_dropdown_trigger(
                    pixmap,
                    tr,
                    (x, y, w, h),
                    &label,
                    speed_dropdown_open,
                );
            }
            ToolbarControl::Mix => {
                let v = params.mix.value();
                let norm = params.mix.unmodulated_normalized_value();
                widgets::draw_slider(
                    pixmap,
                    tr,
                    x,
                    y,
                    w,
                    h,
                    "Mix",
                    &format!("{}%", (v * 100.0).round() as i32),
                    norm,
                    None,
                    false,
                );
            }
            ToolbarControl::Output => {
                let norm = params.output_gain.unmodulated_normalized_value();
                let db = nih_plug::util::gain_to_db(params.output_gain.value());
                widgets::draw_slider(
                    pixmap,
                    tr,
                    x,
                    y,
                    w,
                    h,
                    "Out",
                    &format!("{db:.1} dB"),
                    norm,
                    None,
                    false,
                );
            }
            ToolbarControl::CompThreshold => {
                let norm = params.comp_threshold.unmodulated_normalized_value();
                let db = params.comp_threshold.value();
                widgets::draw_slider(
                    pixmap,
                    tr,
                    x,
                    y,
                    w,
                    h,
                    "Thresh",
                    &format!("{db:.1} dB"),
                    norm,
                    None,
                    false,
                );
            }
            ToolbarControl::CompRatio => {
                let norm = params.comp_ratio.unmodulated_normalized_value();
                let ratio = params.comp_ratio.value();
                widgets::draw_slider(
                    pixmap,
                    tr,
                    x,
                    y,
                    w,
                    h,
                    "Ratio",
                    &format!("{ratio:.1}:1"),
                    norm,
                    None,
                    false,
                );
            }
            ToolbarControl::Reset => {
                widgets::draw_button(pixmap, tr, x, y, w, h, "Reset", false, false);
            }
        }
    }

    // Lower row: the four grid-operation buttons.
    for op in ToolbarOp::ALL {
        let (x, y, w, h) = op_rect(op, scale);
        widgets::draw_button(pixmap, tr, x, y, w, h, op.label(), false, false);
    }

    // Sequence-status readout — anchored in the lower row's middle empty
    // span (between the regen cluster on the left and the clipboard cluster
    // on the right). The relocation matches the row's new "two action
    // clusters with an info readout between" rhythm.
    let step = seq_status.read();
    let status = format!("Step {step}");
    let size = 16.0 * scale;
    let sx = remap(510.0, 0.0).0 * scale;
    let sy = (TOOLBAR_ROW_H + TOOLBAR_ROW_H / 2.0) * scale + size * 0.36;
    tr.draw_text(pixmap, sx, sy, &status, size, widgets::color_text());
}

/// Short label for a `Speed`.
fn speed_label(s: crate::clock::Speed) -> &'static str {
    use crate::clock::Speed;
    match s {
        Speed::Hold => "Hold",
        Speed::Div32 => "1/32",
        Speed::Div16 => "1/16",
        Speed::Div8 => "1/8",
        Speed::Div4 => "1/4",
        Speed::Div2 => "1/2",
        Speed::Div1 => "1/1",
    }
}

/// The Speed dropdown items, in `Speed::ALL` order. Item index `i` maps
/// to `Speed::ALL[i]`.
pub fn speed_items() -> [&'static str; 7] {
    crate::clock::Speed::ALL.map(speed_label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_rects_sit_in_the_upper_toolbar_row() {
        for ctrl in ToolbarControl::ALL {
            let (x, y, w, h) = control_rect(ctrl, 1.0);
            assert!(
                x >= 0.0 && x + w <= crate::editor::WINDOW_WIDTH as f32,
                "{ctrl:?} out of width"
            );
            assert!(y >= 0.0 && y + h <= TOOLBAR_ROW_H, "{ctrl:?} out of row");
        }
    }

    #[test]
    fn control_rects_do_not_overlap() {
        let mut rects: Vec<(f32, f32)> = ToolbarControl::ALL
            .iter()
            .map(|c| {
                let (x, _, w, _) = control_rect(*c, 1.0);
                (x, x + w)
            })
            .collect();
        rects.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for pair in rects.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "controls overlap: {pair:?}");
        }
    }

    #[test]
    fn toolbar_hit_finds_the_control_and_misses_the_grid() {
        let (x, y, w, h) = control_rect(ToolbarControl::Mix, 1.5);
        assert_eq!(
            toolbar_hit(x + w / 2.0, y + h / 2.0, 1.5),
            Some(ToolbarControl::Mix)
        );
        assert_eq!(toolbar_hit(500.0, 400.0, 1.0), None);
    }

    #[test]
    fn op_rects_sit_in_the_lower_toolbar_row() {
        for op in ToolbarOp::ALL {
            let (x, y, w, h) = op_rect(op, 1.0);
            assert!(
                x >= 0.0 && x + w <= crate::editor::WINDOW_WIDTH as f32,
                "{op:?} out of width"
            );
            // Entirely within the lower row [TOOLBAR_ROW_H, 2*TOOLBAR_ROW_H].
            assert!(
                y >= TOOLBAR_ROW_H && y + h <= 2.0 * TOOLBAR_ROW_H,
                "{op:?} out of the lower row"
            );
        }
    }

    #[test]
    fn op_rects_do_not_overlap() {
        let mut rects: Vec<(f32, f32)> = ToolbarOp::ALL
            .iter()
            .map(|o| {
                let (x, _, w, _) = op_rect(*o, 1.0);
                (x, x + w)
            })
            .collect();
        rects.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for pair in rects.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "ops overlap: {pair:?}");
        }
    }

    #[test]
    fn op_hit_finds_an_op_and_misses_elsewhere() {
        let (x, y, w, h) = op_rect(ToolbarOp::Copy, 1.5);
        assert_eq!(op_hit(x + w / 2.0, y + h / 2.0, 1.5), Some(ToolbarOp::Copy));
        // A point in the upper toolbar row is not an op hit.
        assert_eq!(op_hit(20.0, 10.0, 1.0), None);
        // A point in the grid (below the strip) is not an op hit.
        assert_eq!(op_hit(500.0, 400.0, 1.0), None);
    }

    #[test]
    fn apply_grid_op_reinit_cells_restores_activations() {
        use crate::grid::Grid;
        let mut g = Grid::default();
        g.cell_mut(4, 4).enabled = false;
        apply_grid_op(&mut g, ToolbarOp::ReinitCells, 0);
        assert!(g.cell(4, 4).enabled);
    }

    #[test]
    fn apply_grid_op_randomize_is_deterministic_in_seed() {
        use crate::grid::Grid;
        let mut a = Grid::default();
        let mut b = Grid::default();
        apply_grid_op(&mut a, ToolbarOp::RandomizeActivations, 1234);
        apply_grid_op(&mut b, ToolbarOp::RandomizeActivations, 1234);
        assert_eq!(a, b);
    }

    #[test]
    fn apply_grid_op_copy_and_paste_do_not_mutate_the_grid() {
        use crate::grid::Grid;
        let mut g = Grid::default();
        let before = g;
        apply_grid_op(&mut g, ToolbarOp::Copy, 0);
        apply_grid_op(&mut g, ToolbarOp::Paste, 0);
        assert_eq!(
            g, before,
            "Copy/Paste are handled by the editor, not apply_grid_op"
        );
    }

    #[test]
    fn toolbar_rows_lie_within_the_window_margins() {
        let left = crate::editor::grid_view::MARGIN;
        let right = crate::editor::WINDOW_WIDTH as f32 - crate::editor::grid_view::MARGIN;
        for ctrl in ToolbarControl::ALL {
            let (x, _, w, _) = control_rect(ctrl, 1.0);
            assert!(
                x >= left - 0.5 && x + w <= right + 0.5,
                "{ctrl:?} outside margins"
            );
        }
        for op in ToolbarOp::ALL {
            let (x, _, w, _) = op_rect(op, 1.0);
            assert!(
                x >= left - 0.5 && x + w <= right + 0.5,
                "{op:?} outside margins"
            );
        }
    }

    #[test]
    fn toolbar_controls_do_not_overlap() {
        for row in [
            ToolbarControl::ALL
                .iter()
                .map(|&c| {
                    let (x, _, w, _) = control_rect(c, 1.0);
                    (x, x + w)
                })
                .collect::<Vec<_>>(),
            ToolbarOp::ALL
                .iter()
                .map(|&o| {
                    let (x, _, w, _) = op_rect(o, 1.0);
                    (x, x + w)
                })
                .collect::<Vec<_>>(),
        ] {
            let mut spans = row;
            spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            for pair in spans.windows(2) {
                assert!(
                    pair[0].1 <= pair[1].0 + 0.5,
                    "toolbar items overlap: {pair:?}"
                );
            }
        }
    }

    #[test]
    fn toolbar_hit_round_trips_each_item() {
        for ctrl in ToolbarControl::ALL {
            let (x, y, w, h) = control_rect(ctrl, 1.4);
            assert_eq!(toolbar_hit(x + w / 2.0, y + h / 2.0, 1.4), Some(ctrl));
        }
        for op in ToolbarOp::ALL {
            let (x, y, w, h) = op_rect(op, 1.4);
            assert_eq!(op_hit(x + w / 2.0, y + h / 2.0, 1.4), Some(op));
        }
    }

    #[test]
    fn speed_items_match_speed_all_order() {
        let items = speed_items();
        assert_eq!(items.len(), crate::clock::Speed::ALL.len());
        for (i, &s) in crate::clock::Speed::ALL.iter().enumerate() {
            assert_eq!(items[i], speed_label(s));
        }
    }
}
