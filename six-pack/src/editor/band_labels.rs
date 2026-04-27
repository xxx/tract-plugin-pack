//! 6-column per-band labels grid.
//!
//! Rows: Freq, Gain, Q, Algo, Mode. Each cell shows a value; left-clicking
//! Algo/Mode cycles to the next option, right-clicking Freq/Gain/Q opens
//! the text-edit overlay.

use crate::editor::{band_color, BandLabelField, HitAction, SixPackWindow};
use nih_plug::prelude::Param;
use tiny_skia_widgets as widgets;

const ROWS: [(BandLabelField, &str); 5] = [
    (BandLabelField::Freq, "Freq"),
    (BandLabelField::Gain, "Gain"),
    (BandLabelField::Q, "Q"),
    (BandLabelField::Algo, "Algo"),
    (BandLabelField::Mode, "Mode"),
];

pub(crate) fn draw(win: &mut SixPackWindow, x: f32, y: f32, w: f32, h: f32) {
    let s = win.scale_factor;

    // Background panel
    widgets::draw_rect(
        &mut win.surface.pixmap,
        x,
        y,
        w,
        h,
        widgets::color_control_bg(),
    );
    widgets::draw_rect_outline(
        &mut win.surface.pixmap,
        x,
        y,
        w,
        h,
        widgets::color_border(),
        1.0,
    );

    let label_w = 50.0 * s;
    let cell_pad = 2.0 * s;
    let col_w = (w - label_w - cell_pad * 2.0) / 6.0;
    let row_h = (h - cell_pad * 2.0) / ROWS.len() as f32;

    let label_size = (10.0 * s).max(9.0);
    let value_size = (10.5 * s).max(9.0);

    // Pre-collect cell data so we can avoid borrow conflicts during text draws.
    #[derive(Clone)]
    struct CellData {
        kind: BandLabelField,
        value_text: String,
        normalized: f32,
        editing: Option<String>,
    }
    let mut grid: [[CellData; 6]; 5] = std::array::from_fn(|_| {
        std::array::from_fn(|_| CellData {
            kind: BandLabelField::Freq,
            value_text: String::new(),
            normalized: 0.0,
            editing: None,
        })
    });

    for (row_idx, (field, _)) in ROWS.iter().enumerate() {
        for (band, bp) in win.params.bands.iter().enumerate() {
            let (text, normalized) = match field {
                BandLabelField::Freq => {
                    let f = bp.freq.value();
                    let s = if f >= 1000.0 {
                        format!("{:.1} kHz", f / 1000.0)
                    } else {
                        format!("{:.0} Hz", f)
                    };
                    (s, bp.freq.unmodulated_normalized_value())
                }
                BandLabelField::Gain => (
                    format!("{:+.1}", bp.gain.value()),
                    bp.gain.unmodulated_normalized_value(),
                ),
                BandLabelField::Q => (
                    format!("{:.2}", bp.q.value()),
                    bp.q.unmodulated_normalized_value(),
                ),
                BandLabelField::Algo => (algo_short(bp.algo.value()).to_string(), 0.0),
                BandLabelField::Mode => (mode_short(bp.channel.value()).to_string(), 0.0),
            };
            let editing = win
                .text_edit
                .active_for(&HitAction::BandLabel(band, *field))
                .map(str::to_owned);
            grid[row_idx][band] = CellData {
                kind: *field,
                value_text: text,
                normalized,
                editing,
            };
        }
    }
    let caret = win.text_edit.caret_visible();

    // Row labels (left margin).
    for (row_idx, (_, name)) in ROWS.iter().enumerate() {
        let cy = y + cell_pad + row_idx as f32 * row_h + row_h * 0.5;
        let lw = win.text_renderer.text_width(name, label_size);
        win.text_renderer.draw_text(
            &mut win.surface.pixmap,
            x + label_w - lw - 4.0 * s,
            cy + label_size * 0.4,
            name,
            label_size,
            widgets::color_muted(),
        );
    }

    for (row_idx, row) in grid.iter().enumerate() {
        for (band, cell_ref) in row.iter().enumerate() {
            let cell = cell_ref.clone();
            let cx = x + label_w + cell_pad + band as f32 * col_w;
            let cy = y + cell_pad + row_idx as f32 * row_h;
            let cw = col_w - 2.0 * s;
            let ch = row_h - 2.0 * s;

            // Cell background — subtle band-color tint. Multiplicative dim
            // (not subtractive) so the hue survives even for bright channels
            // close to 0xff (yellow/green/blue/violet would otherwise turn
            // into muddy olive/brown).
            let bg = band_color(band);
            let bg_dim = tiny_skia::Color::from_rgba8(
                (bg.red() * 255.0 * 0.18) as u8,
                (bg.green() * 255.0 * 0.18) as u8,
                (bg.blue() * 255.0 * 0.18) as u8,
                0xff,
            );
            widgets::draw_rect(&mut win.surface.pixmap, cx, cy, cw, ch, bg_dim);
            widgets::draw_rect_outline(
                &mut win.surface.pixmap,
                cx,
                cy,
                cw,
                ch,
                widgets::color_border(),
                1.0,
            );

            // Hit region.
            win.drag
                .push_region(cx, cy, cw, ch, HitAction::BandLabel(band, cell.kind));

            // Edit field overlay or static value text.
            let value_y = cy + ch * 0.5 + value_size * 0.4;
            if let Some(buf) = cell.editing.as_deref() {
                let edit_pad = 2.0 * s;
                widgets::draw_rect(
                    &mut win.surface.pixmap,
                    cx + edit_pad,
                    cy + edit_pad,
                    cw - 2.0 * edit_pad,
                    ch - 2.0 * edit_pad,
                    widgets::color_edit_bg(),
                );
                widgets::draw_rect_outline(
                    &mut win.surface.pixmap,
                    cx + edit_pad,
                    cy + edit_pad,
                    cw - 2.0 * edit_pad,
                    ch - 2.0 * edit_pad,
                    widgets::color_accent(),
                    1.0,
                );
                let bw = win.text_renderer.text_width(buf, value_size);
                let bx = cx + cw * 0.5 - bw * 0.5;
                win.text_renderer.draw_text(
                    &mut win.surface.pixmap,
                    bx,
                    value_y,
                    buf,
                    value_size,
                    widgets::color_text(),
                );
                if caret {
                    widgets::draw_rect(
                        &mut win.surface.pixmap,
                        bx + bw + 1.0,
                        cy + ch * 0.2,
                        1.0,
                        ch * 0.6,
                        widgets::color_text(),
                    );
                }
            } else {
                let vw = win.text_renderer.text_width(&cell.value_text, value_size);
                win.text_renderer.draw_text(
                    &mut win.surface.pixmap,
                    cx + cw * 0.5 - vw * 0.5,
                    value_y,
                    &cell.value_text,
                    value_size,
                    band_color(band),
                );
            }
        }
    }

    // Suppress unused-warning for normalized while we don't yet use it for
    // mini-bar visualization.
    let _ = grid[0][0].normalized;
}

fn algo_short(a: crate::AlgoParam) -> &'static str {
    use crate::AlgoParam::*;
    match a {
        Tube => "Tube",
        Tape => "Tape",
        Diode => "Diode",
        Digital => "Digi",
        ClassB => "ClsB",
        Wavefold => "Fold",
    }
}

fn mode_short(m: crate::ChannelParam) -> &'static str {
    use crate::ChannelParam::*;
    match m {
        Stereo => "Stereo",
        Mid => "Mid",
        Side => "Side",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn algo_short_covers_all_variants() {
        assert_eq!(algo_short(crate::AlgoParam::Tube), "Tube");
        assert_eq!(algo_short(crate::AlgoParam::Tape), "Tape");
        assert_eq!(algo_short(crate::AlgoParam::Wavefold), "Fold");
    }

    #[test]
    fn mode_short_covers_all_variants() {
        assert_eq!(mode_short(crate::ChannelParam::Stereo), "Stereo");
        assert_eq!(mode_short(crate::ChannelParam::Mid), "Mid");
        assert_eq!(mode_short(crate::ChannelParam::Side), "Side");
    }
}
