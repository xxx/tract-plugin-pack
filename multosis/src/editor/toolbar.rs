//! The editor toolbar: parameter controls + Reset, laid out across the upper
//! row of the top strip. Geometry is logical; every draw multiplies by scale.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7.

use crate::editor::grid_view::TOOLBAR_ROW_H;
use crate::MultosisParams;
use nih_plug::prelude::Param;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// One toolbar control in the upper row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarControl {
    /// Cycles the wavefront speed.
    Speed,
    /// Cycles the throwaway effect bank.
    Bank,
    /// Toggles auto-restart.
    AutoRestart,
    /// Drag slider — dry/wet mix.
    Mix,
    /// Drag slider — output gain.
    Output,
    /// Resets the sequence.
    Reset,
}

impl ToolbarControl {
    /// The six controls, left to right.
    pub const ALL: [ToolbarControl; 6] = [
        ToolbarControl::Speed,
        ToolbarControl::Bank,
        ToolbarControl::AutoRestart,
        ToolbarControl::Mix,
        ToolbarControl::Output,
        ToolbarControl::Reset,
    ];

    /// Logical `(x, width)` of this control. The row is 1056 logical wide.
    fn logical_x_w(self) -> (f32, f32) {
        match self {
            ToolbarControl::Speed => (6.0, 200.0),
            ToolbarControl::Bank => (212.0, 160.0),
            ToolbarControl::AutoRestart => (378.0, 120.0),
            ToolbarControl::Mix => (504.0, 180.0),
            ToolbarControl::Output => (690.0, 180.0),
            ToolbarControl::Reset => (876.0, 174.0),
        }
    }
}

/// Vertical inset of a control within its toolbar row, logical px.
const CTRL_INSET: f32 = 4.0;

/// The physical-pixel rectangle `(x, y, w, h)` of `ctrl` at `scale`.
pub fn control_rect(ctrl: ToolbarControl, scale: f32) -> (f32, f32, f32, f32) {
    let (lx, lw) = ctrl.logical_x_w();
    let x = lx * scale;
    let y = CTRL_INSET * scale;
    let w = lw * scale;
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

/// Draw the toolbar strip and its six upper-row controls.
pub fn draw_toolbar(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    params: &MultosisParams,
    scale: f32,
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
                widgets::draw_button(pixmap, tr, x, y, w, h, &label, false, false);
            }
            ToolbarControl::Bank => {
                let label = format!("Effect: {}", bank_label(params.effect_bank.value()));
                widgets::draw_button(pixmap, tr, x, y, w, h, &label, false, false);
            }
            ToolbarControl::AutoRestart => {
                let on = params.auto_restart.value();
                widgets::draw_button(pixmap, tr, x, y, w, h, "Auto-Restart", on, false);
            }
            ToolbarControl::Mix => {
                let v = params.mix.value();
                widgets::draw_slider(
                    pixmap,
                    tr,
                    x,
                    y,
                    w,
                    h,
                    "Mix",
                    &format!("{}%", (v * 100.0).round() as i32),
                    v,
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
            ToolbarControl::Reset => {
                widgets::draw_button(pixmap, tr, x, y, w, h, "Reset", false, false);
            }
        }
    }
}

/// Short label for a `Speed`.
fn speed_label(s: crate::clock::Speed) -> &'static str {
    use crate::clock::Speed;
    match s {
        Speed::Div32 => "1/32",
        Speed::Div16 => "1/16",
        Speed::Div8 => "1/8",
        Speed::Div4 => "1/4",
        Speed::Div2 => "1/2",
        Speed::Div1 => "1/1",
    }
}

/// Short label for an `EffectBank`.
fn bank_label(b: crate::effects::EffectBank) -> &'static str {
    use crate::effects::EffectBank;
    match b {
        EffectBank::Lowpass => "Lowpass",
        EffectBank::Bitcrush => "Bitcrush",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_rects_sit_in_the_upper_toolbar_row() {
        for ctrl in ToolbarControl::ALL {
            let (x, y, w, h) = control_rect(ctrl, 1.0);
            assert!(x >= 0.0 && x + w <= 1056.0, "{ctrl:?} out of width");
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
}
