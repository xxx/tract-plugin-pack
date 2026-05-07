//! Bottom strip: Input dial, link button, Output dial, Mix dial,
//! plus Quality / Drive stepped selectors and a De-Emphasis toggle.

use crate::editor::{GlobalDialId, HitAction, SixPackWindow};
use nih_plug::prelude::Param;
use tiny_skia_widgets as widgets;

pub(crate) fn draw(win: &mut SixPackWindow, x: f32, y: f32, w: f32, h: f32) {
    let s = win.scale_factor;

    // Background panel.
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

    // Layout: 3 dials + link button on the left half, steppers + de-emph on
    // the right. The dial center sits at ~55% of the strip's height so the
    // "Input"/"Output"/"Mix" name labels (drawn above the arc) have room
    // above the dial without crossing the panel's top border.
    let dial_radius = (h * 0.30).clamp(18.0, 32.0 * s);
    let dial_row_cy = y + h * 0.55;

    // Left cluster: Input — link button — Output — Mix.
    let cluster_w = w * 0.55;
    let dial_count = 3usize; // Input, Output, Mix
    let col_spacing = cluster_w / dial_count as f32;

    let dials: [(GlobalDialId, &str); 3] = [
        (GlobalDialId::Input, "Input"),
        (GlobalDialId::Output, "Output"),
        (GlobalDialId::Mix, "Mix"),
    ];

    // Pre-collect because draw_dial_ex needs &mut text_renderer.
    let mut dial_data: [(GlobalDialId, &str, f32, String, Option<String>); 3] = [
        (GlobalDialId::Input, "", 0.0, String::new(), None),
        (GlobalDialId::Output, "", 0.0, String::new(), None),
        (GlobalDialId::Mix, "", 0.0, String::new(), None),
    ];
    // When I/O Link is on, the audio path uses `output_gain = 1 / input_gain`
    // (i.e. `output_dB = -input_dB`) directly and ignores the Output param's
    // stored value. Mirror the Output dial's display to match: same magnitude,
    // opposite sign in dB, which by symmetry of the gain skew is exactly
    // `1 − input_normalized` on the dial.
    let io_linked = win.params.io_link.value();
    let input_norm = win.params.input_gain.unmodulated_normalized_value();
    let input_db = nih_plug::util::gain_to_db(win.params.input_gain.value());

    for (i, &(id, label)) in dials.iter().enumerate() {
        let (text, normalized) = match id {
            GlobalDialId::Input => (format!("{:+.1} dB", input_db), input_norm),
            GlobalDialId::Output => {
                if io_linked {
                    (format!("{:+.1} dB", -input_db), 1.0 - input_norm)
                } else {
                    (
                        format!(
                            "{:+.1} dB",
                            nih_plug::util::gain_to_db(win.params.output_gain.value())
                        ),
                        win.params.output_gain.unmodulated_normalized_value(),
                    )
                }
            }
            GlobalDialId::Mix => (
                format!("{:.0}%", win.params.mix.value() * 100.0),
                win.params.mix.unmodulated_normalized_value(),
            ),
        };
        let editing = win
            .text_edit
            .active_for(&HitAction::GlobalDial(id))
            .map(str::to_owned);
        dial_data[i] = (id, label, normalized, text, editing);
    }

    let caret = win.text_edit.caret_visible();

    // Link button geometry — tucked between Input (i=0) and Output (i=1).
    // Push the hit region BEFORE the dial regions so DragState::hit_test
    // (which returns the first match) picks the button when the cursor is
    // over it. The dial hit regions span the full column width and would
    // otherwise swallow clicks on the button.
    let link_active = win.params.io_link.value();
    let link_w = 24.0 * s;
    let link_h = 16.0 * s;
    let link_x = x + col_spacing - link_w * 0.5;
    let link_y = dial_row_cy - link_h * 0.5;
    win.drag
        .push_region(link_x, link_y, link_w, link_h, HitAction::IoLink);

    let tr = &mut win.text_renderer;

    for (i, &(id, label, normalized, ref text, ref editing)) in dial_data.iter().enumerate() {
        let cx = x + col_spacing * (i as f32 + 0.5);
        widgets::draw_dial_ex(
            &mut win.surface.pixmap,
            tr,
            cx,
            dial_row_cy,
            dial_radius,
            label,
            text,
            normalized,
            None,
            editing.as_deref(),
            caret,
        );
        let hit_w = col_spacing;
        let hit_h = h * 0.85;
        win.drag.push_region(
            cx - hit_w / 2.0,
            dial_row_cy - hit_h / 2.0,
            hit_w,
            hit_h,
            HitAction::GlobalDial(id),
        );
    }

    // Render the link button after the dials so it draws on top. (Hit
    // region was already pushed above.)
    widgets::draw_button(
        &mut win.surface.pixmap,
        tr,
        link_x,
        link_y,
        link_w,
        link_h,
        "L",
        link_active,
        false,
    );

    // Right cluster: Quality + Drive steppers, De-Emphasis toggle.
    let right_x = x + cluster_w + 4.0 * s;
    let right_w = w - cluster_w - 8.0 * s;
    let row_h = (h - 12.0 * s) / 3.0;
    let stepper_h = (row_h - 4.0 * s).clamp(18.0, 32.0 * s);

    // Quality
    let quality_idx = win.params.quality.value() as usize;
    let qy = y + 4.0 * s;
    draw_stepped_with_hit(
        &mut win.surface.pixmap,
        tr,
        &mut win.drag,
        right_x,
        qy,
        right_w,
        stepper_h,
        &["Off", "4x", "8x", "16x"],
        quality_idx,
        HitAction::QualitySeg,
    );

    // Drive
    let drive_idx = win.params.drive.value() as usize;
    let dyl = qy + row_h;
    draw_stepped_with_hit(
        &mut win.surface.pixmap,
        tr,
        &mut win.drag,
        right_x,
        dyl,
        right_w,
        stepper_h,
        &["Carve", "Color", "Crush"],
        drive_idx,
        HitAction::DriveSeg,
    );

    // De-Emphasis toggle
    let de_y = dyl + row_h;
    let de_active = win.params.deemphasis.value();
    widgets::draw_button(
        &mut win.surface.pixmap,
        tr,
        right_x,
        de_y,
        right_w,
        stepper_h,
        "De-Emphasis",
        de_active,
        false,
    );
    win.drag
        .push_region(right_x, de_y, right_w, stepper_h, HitAction::DeEmphasis);
}

#[allow(clippy::too_many_arguments)]
fn draw_stepped_with_hit<F: Fn(usize) -> HitAction>(
    pixmap: &mut tiny_skia::Pixmap,
    tr: &mut widgets::TextRenderer,
    drag: &mut widgets::DragState<HitAction>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    options: &[&str],
    active: usize,
    action_for: F,
) {
    widgets::draw_stepped_selector(pixmap, tr, x, y, w, h, options, active);
    let seg_w = w / options.len() as f32;
    for i in 0..options.len() {
        drag.push_region(x + i as f32 * seg_w, y, seg_w, h, action_for(i));
    }
}
