//! Per-band strip: Width slider (vertical), Stereoize knob, Mode I/II, Solo.
//! 4-up grid.

use crate::theme;
use crate::ImagineParams;
use std::sync::Arc;
use tiny_skia::{Color, Paint, Pixmap, PixmapMut, Rect, Transform};
use tiny_skia_widgets::TextRenderer;

/// Reserved height for the band header row ("B1"–"B4") at the top of each panel.
pub const HEADER_H: i32 = 14;
/// Reserved height for sub-control captions ("Width", "Stz") above each control.
pub const LABEL_H: i32 = 12;
/// Reserved height for value readouts placed beneath their control.
pub const VALUE_H: i32 = 11;

#[inline]
fn scaled(v: i32, s: f32) -> i32 {
    ((v as f32) * s).round().max(1.0) as i32
}

pub struct BandStripLayout {
    pub band_x: [i32; 4],
    pub band_w: i32,
    pub y: i32,
    pub h: i32,
    /// Width slider rect inside each band (relative to band's top-left).
    pub width_rect: (i32, i32, i32, i32),
    /// Stereoize knob center + radius (relative to band's top-left).
    pub stz_center: (i32, i32),
    pub stz_radius: i32,
    /// Stereoize on/off toggle rect (relative to band's top-left). Sits
    /// above the knob, in the row that previously held only the "Stz"
    /// caption text.
    pub stz_on_rect: (i32, i32, i32, i32),
    /// Mode toggle rect (relative to band's top-left).
    pub mode_rect: (i32, i32, i32, i32),
    /// Solo button rect (relative to band's top-left).
    pub solo_rect: (i32, i32, i32, i32),
}

pub fn compute_layout(x: i32, y: i32, w: i32, h: i32, scale_factor: f32) -> BandStripLayout {
    let s = scale_factor.max(0.1);
    let band_w = w / 4;
    let band_x = [x, x + band_w, x + 2 * band_w, x + 3 * band_w];

    // Inside a band:
    //   y = 0..HEADER_H            : band header ("B1"–"B4")
    //   y = HEADER_H..HEADER_H+LABEL_H : "Width" caption above the slider slot
    //   Width slider runs from y=HEADER_H+LABEL_H down to leave VALUE_H at the
    //   bottom for the value readout text.
    let pad = scaled(6, s);
    let header_h = scaled(HEADER_H, s);
    let label_h = scaled(LABEL_H, s);
    let value_h = scaled(VALUE_H, s);
    let slider_w = scaled(32, s);
    let button_w = scaled(32, s);
    let button_h = scaled(14, s);
    let button_right_inset = scaled(38, s);
    let stack_gap = scaled(12, s);
    let knob_x_offset = scaled(12, s);
    let stack_above_pad = scaled(8, s);

    let width_top = header_h + label_h;
    let width_bottom_reserve = value_h + scaled(2, s);
    let width_h = (h - width_top - width_bottom_reserve).max(1);
    let width_rect = (pad, width_top, slider_w, width_h);

    // Stereoize knob: place its center clearly below the band header AND its
    // own "Stz" caption row, so the two labels never visually collide into a
    // single "B1Stz" blob at large scales. The knob center must leave radius
    // worth of clearance above it for the caption + ring top.
    let stz_label_top = header_h + label_h;
    // The Stz on/off toggle replaces the previous static "Stz" caption
    // row. Same width as the Mode/Solo buttons; centred horizontally on
    // the knob's column so it sits directly above the knob.
    let stz_on_w = button_w;
    let stz_on_h = button_h;
    let stz_on_rect_x = (band_w - stz_on_w) / 2 + scaled(12, s);
    let stz_on_rect = (stz_on_rect_x, stz_label_top, stz_on_w, stz_on_h);
    // Reserve room for two stacked button-height bars (Mode + Solo) with a
    // gap between them, then panel padding.
    let buttons_h = button_h + stack_gap + button_h + scaled(4, s);
    // Available vertical room for the knob (between the Stz caption row and
    // the stacked Mode/Solo buttons at the bottom).
    let knob_band_h = (h - stz_label_top - buttons_h - stack_above_pad).max(2);
    // More generous radius than the original /4 formula so the knob occupies
    // more of the panel at large scales. Clamp by the available vertical room
    // so it can't push down into the Mode/Solo stack.
    let stz_radius = ((band_w / 3).min(knob_band_h / 2)).max(scaled(4, s)) - scaled(4, s);
    let stz_radius = stz_radius.max(scaled(3, s));
    // Center the knob within the available vertical band, with its top edge
    // (radius + caption pad) sitting below the Stz label row.
    let stz_center_y = stz_label_top + stz_radius + scaled(4, s);
    let stz_center = (band_w / 2 + knob_x_offset, stz_center_y);

    // Mode toggle / Solo button stack below the knob with stack_gap gaps.
    let stack_top = stz_center_y + stz_radius + stack_above_pad;
    let mode_rect = (band_w - button_right_inset, stack_top, button_w, button_h);
    let solo_rect = (
        band_w - button_right_inset,
        stack_top + button_h + stack_gap,
        button_w,
        button_h,
    );

    BandStripLayout {
        band_x,
        band_w,
        y,
        h,
        width_rect,
        stz_center,
        stz_radius,
        stz_on_rect,
        mode_rect,
        solo_rect,
    }
}

fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    paint.blend_mode = if color.is_opaque() {
        tiny_skia::BlendMode::Source
    } else {
        tiny_skia::BlendMode::SourceOver
    };
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn stroke_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    fill_rect_i(pixmap, x, y, w, 1, color);
    fill_rect_i(pixmap, x, y + h - 1, w, 1, color);
    fill_rect_i(pixmap, x, y, 1, h, color);
    fill_rect_i(pixmap, x + w - 1, y, 1, h, color);
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    params: &Arc<ImagineParams>,
    text_renderer: &mut TextRenderer,
    scale_factor: f32,
) {
    let s = scale_factor.max(0.1);
    let layout = compute_layout(x, y, w, h, s);

    let widths = [
        params.bands[0].width.value(),
        params.bands[1].width.value(),
        params.bands[2].width.value(),
        params.bands[3].width.value(),
    ];
    let stz_ms = [
        params.bands[0].stz_ms.value(),
        params.bands[1].stz_ms.value(),
        params.bands[2].stz_ms.value(),
        params.bands[3].stz_ms.value(),
    ];
    let stz_scales = [
        params.bands[0].stz_scale.value(),
        params.bands[1].stz_scale.value(),
        params.bands[2].stz_scale.value(),
        params.bands[3].stz_scale.value(),
    ];
    let stz_ons = [
        params.bands[0].stz_on.value(),
        params.bands[1].stz_on.value(),
        params.bands[2].stz_on.value(),
        params.bands[3].stz_on.value(),
    ];
    let modes = [
        params.bands[0].mode.value(),
        params.bands[1].mode.value(),
        params.bands[2].mode.value(),
        params.bands[3].mode.value(),
    ];
    let solos = [
        params.bands[0].solo.value(),
        params.bands[1].solo.value(),
        params.bands[2].solo.value(),
        params.bands[3].solo.value(),
    ];

    // Shape pass — borrow PixmapMut just for the duration of the shape draws.
    {
        let mut pm = pixmap.as_mut();
        for i in 0..4 {
            let bx = layout.band_x[i];
            // Panel
            fill_rect_i(
                &mut pm,
                bx,
                layout.y,
                layout.band_w - 4,
                layout.h,
                theme::panel_bg(),
            );
            stroke_rect_i(
                &mut pm,
                bx,
                layout.y,
                layout.band_w - 4,
                layout.h,
                theme::border(),
            );

            // Width slider (vertical bar with marker)
            let (wx, wy, ww, wh) = layout.width_rect;
            let slot_x = bx + wx;
            let slot_y = layout.y + wy;
            fill_rect_i(&mut pm, slot_x, slot_y, ww, wh, theme::spectrum_bg());
            stroke_rect_i(&mut pm, slot_x, slot_y, ww, wh, theme::border());
            // Center line at width=0 — at least 2 px tall, scales with editor.
            let center_h = scaled(2, s);
            let center_y = slot_y + wh / 2 - center_h / 2;
            fill_rect_i(&mut pm, slot_x, center_y, ww, center_h, theme::text());
            // Marker: top = +100, bottom = -100
            let w_norm = (widths[i] + 100.0) / 200.0; // 0..1
            let marker_y = slot_y + wh - (w_norm * wh as f32) as i32;
            fill_rect_i(
                &mut pm,
                slot_x,
                marker_y - 1,
                ww,
                3,
                theme::cyan_to_pink(w_norm),
            );

            // Stereoize knob (filled annular ring + active-mode sector).
            // In Mode I the arc tracks `stz_ms` within [HAAS_MIN_MS,
            // HAAS_MAX_MS]; in Mode II it tracks `stz_scale` within
            // [STZ_SCALE_MIN, STZ_SCALE_MAX]. Dims when `stz_on` is
            // false to show the stereoize stage is bypassed.
            let (cx, cy) = (bx + layout.stz_center.0, layout.y + layout.stz_center.1);
            let radius = layout.stz_radius;
            // Ring background — full circle, brighter than spectrum_bg so the
            // user sees the knob outline even at amount = 0.
            draw_arc_ring(&mut pm, cx, cy, radius, 0.0, 1.0, theme::border());
            let stz_norm = match modes[i] {
                crate::StereoizeModeParam::I => {
                    let range = crate::HAAS_MAX_MS - crate::HAAS_MIN_MS;
                    ((stz_ms[i] - crate::HAAS_MIN_MS) / range).clamp(0.0, 1.0)
                }
                crate::StereoizeModeParam::Ii => {
                    let range = crate::STZ_SCALE_MAX - crate::STZ_SCALE_MIN;
                    ((stz_scales[i] - crate::STZ_SCALE_MIN) / range).clamp(0.0, 1.0)
                }
            };
            if stz_norm > 0.0 {
                let arc_color = if stz_ons[i] {
                    theme::accent()
                } else {
                    theme::text_dim()
                };
                draw_arc_ring(&mut pm, cx, cy, radius, 0.0, stz_norm, arc_color);
            }

            // Mode toggle
            let (mxi, myi, mw, mh) = layout.mode_rect;
            let mode_x = bx + mxi;
            let mode_y = layout.y + myi;
            fill_rect_i(&mut pm, mode_x, mode_y, mw, mh, theme::spectrum_bg());
            stroke_rect_i(&mut pm, mode_x, mode_y, mw, mh, theme::border());
            // Highlight active half (Mode I = left, Mode II = right)
            let half_w = mw / 2;
            let active_x = if matches!(modes[i], crate::StereoizeModeParam::I) {
                mode_x
            } else {
                mode_x + half_w
            };
            fill_rect_i(&mut pm, active_x, mode_y, half_w, mh, theme::accent());

            // Solo button
            let (sxi, syi, sw, sh) = layout.solo_rect;
            let solo_x = bx + sxi;
            let solo_y = layout.y + syi;
            let color = if solos[i] {
                theme::cyan_to_pink(0.5)
            } else {
                theme::spectrum_bg()
            };
            fill_rect_i(&mut pm, solo_x, solo_y, sw, sh, color);
            stroke_rect_i(&mut pm, solo_x, solo_y, sw, sh, theme::border());

            // Stereoize on/off toggle (above the knob). Filled when on.
            let (oxi, oyi, ow, oh) = layout.stz_on_rect;
            let stz_x = bx + oxi;
            let stz_y = layout.y + oyi;
            let stz_color = if stz_ons[i] {
                theme::accent()
            } else {
                theme::spectrum_bg()
            };
            fill_rect_i(&mut pm, stz_x, stz_y, ow, oh, stz_color);
            stroke_rect_i(&mut pm, stz_x, stz_y, ow, oh, theme::border());
        }
    }

    // Text pass — labels for band number, Width value, Stereoize amount,
    // Mode I/II halves, and Solo button.
    let label_size = (11.0_f32 * s).max(6.0);
    let small_size = (9.5_f32 * s).max(6.0);
    for i in 0..4 {
        let bx = layout.band_x[i];

        // "Bn" centered above the panel (just inside the top border).
        let band_label = match i {
            0 => "B1",
            1 => "B2",
            2 => "B3",
            _ => "B4",
        };
        let bw = text_renderer.text_width(band_label, label_size);
        let panel_w = (layout.band_w - 4) as f32;
        let label_x = bx as f32 + (panel_w - bw) * 0.5;
        let label_y = layout.y as f32 + label_size + 1.0;
        text_renderer.draw_text(
            pixmap,
            label_x,
            label_y,
            band_label,
            label_size,
            theme::text(),
        );

        // Width value text (e.g. "+50") below the slider in its reserved row.
        let (wx, wy, ww, wh) = layout.width_rect;
        let slot_x = bx + wx;
        let slot_y = layout.y + wy;
        let width_val = widths[i];
        let width_text = format!("{:+.0}", width_val);
        let wt_w = text_renderer.text_width(&width_text, small_size);
        let wt_x = (slot_x + ww / 2) as f32 - wt_w * 0.5;
        // Baseline sits inside the VALUE_H reserve below the slot.
        let wt_y = (slot_y + wh) as f32 + small_size + 1.0;
        text_renderer.draw_text(
            pixmap,
            wt_x,
            wt_y,
            &width_text,
            small_size,
            theme::text_dim(),
        );

        // "Width" caption above the slider, in the LABEL_H row.
        let caption = "Width";
        let cw = text_renderer.text_width(caption, small_size);
        text_renderer.draw_text(
            pixmap,
            (slot_x + ww / 2) as f32 - cw * 0.5,
            (slot_y - 2) as f32,
            caption,
            small_size,
            theme::text_dim(),
        );

        // Stereoize knob: ms value inside; "Stz" label sits inside the
        // on/off toggle button above (rendered below).
        let (kxr, kyr) = layout.stz_center;
        let kx = bx + kxr;
        let ky = layout.y + kyr;
        let (oxi, oyi, ow, oh) = layout.stz_on_rect;
        let stz_caption = "Stz";
        let scw = text_renderer.text_width(stz_caption, small_size);
        let stz_caption_color = if stz_ons[i] {
            theme::text()
        } else {
            theme::text_dim()
        };
        text_renderer.draw_text(
            pixmap,
            (bx + oxi + ow / 2) as f32 - scw * 0.5,
            (layout.y + oyi + oh / 2) as f32 + small_size * 0.35,
            stz_caption,
            small_size,
            stz_caption_color,
        );
        let stz_text = if stz_ons[i] {
            match modes[i] {
                crate::StereoizeModeParam::I => format!("{:.1}", stz_ms[i]),
                crate::StereoizeModeParam::Ii => format!("{:.2}×", stz_scales[i]),
            }
        } else {
            "off".to_string()
        };
        let stw = text_renderer.text_width(&stz_text, small_size);
        text_renderer.draw_text(
            pixmap,
            kx as f32 - stw * 0.5,
            ky as f32 + small_size * 0.4,
            &stz_text,
            small_size,
            theme::text(),
        );

        // Mode I/II labels inside the toggle halves.
        let (mxi, myi, mw, mh) = layout.mode_rect;
        let mode_x = bx + mxi;
        let mode_y = layout.y + myi;
        let half_w = mw / 2;
        let i_label = "I";
        let ii_label = "II";
        let iw = text_renderer.text_width(i_label, small_size);
        let iiw = text_renderer.text_width(ii_label, small_size);
        let i_active = matches!(modes[i], crate::StereoizeModeParam::I);
        let i_color = if i_active {
            theme::text()
        } else {
            theme::text_dim()
        };
        let ii_color = if i_active {
            theme::text_dim()
        } else {
            theme::text()
        };
        let txt_y = mode_y as f32 + (mh as f32) * 0.5 + small_size * 0.35;
        text_renderer.draw_text(
            pixmap,
            mode_x as f32 + half_w as f32 * 0.5 - iw * 0.5,
            txt_y,
            i_label,
            small_size,
            i_color,
        );
        text_renderer.draw_text(
            pixmap,
            mode_x as f32 + half_w as f32 + half_w as f32 * 0.5 - iiw * 0.5,
            txt_y,
            ii_label,
            small_size,
            ii_color,
        );

        // Solo button label.
        let (sxi, syi, sw, sh) = layout.solo_rect;
        let solo_x = bx + sxi;
        let solo_y = layout.y + syi;
        let solo_label = "Solo";
        let slw = text_renderer.text_width(solo_label, small_size);
        let solo_color = if solos[i] {
            theme::text()
        } else {
            theme::text_dim()
        };
        text_renderer.draw_text(
            pixmap,
            solo_x as f32 + sw as f32 * 0.5 - slw * 0.5,
            solo_y as f32 + sh as f32 * 0.5 + small_size * 0.35,
            solo_label,
            small_size,
            solo_color,
        );
    }
}

/// Draw a partial annular sector (donut slice) from `start_norm` to `end_norm`
/// (both in [0, 1]), where 0 = top of ring (12 o'clock) and progress is
/// clockwise. The ring thickness scales with `radius` so the knob reads as a
/// continuous band at any editor scale.
fn draw_arc_ring(
    pixmap: &mut PixmapMut<'_>,
    cx: i32,
    cy: i32,
    radius: i32,
    start_norm: f32,
    end_norm: f32,
    color: Color,
) {
    if radius <= 1 || end_norm <= start_norm {
        return;
    }
    use tiny_skia::{FillRule, PathBuilder};
    let r_outer = radius as f32;
    // Thickness grows with the radius so the ring stays visible at large
    // scales. Minimum 3 px so it never disappears at scale 1.0.
    let thickness = ((radius as f32 / 4.0).round() as i32).max(3) as f32;
    let r_inner = (r_outer - thickness).max(1.0);
    let cx_f = cx as f32 + 0.5;
    let cy_f = cy as f32 + 0.5;

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    // 12 o'clock origin: subtract π/2 so 0-norm aligns with the top.
    let start = start_norm * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    let end = end_norm * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    // Sample density: ~60 steps for a full turn, scaled to the swept angle.
    let steps = 60_i32.max((((end - start) / 0.05).abs()) as i32);

    let mut pb = PathBuilder::new();
    // Outer edge, start → end.
    let (sx, sy) = (cx_f + r_outer * start.cos(), cy_f + r_outer * start.sin());
    pb.move_to(sx, sy);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let a = start + (end - start) * t;
        pb.line_to(cx_f + r_outer * a.cos(), cy_f + r_outer * a.sin());
    }
    // Inner edge, end → start.
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let a = end - (end - start) * t;
        pb.line_to(cx_f + r_inner * a.cos(), cy_f + r_inner * a.sin());
    }
    pb.close();

    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_4_columns() {
        let layout = compute_layout(0, 0, 400, 200, 1.0);
        assert_eq!(layout.band_x[0], 0);
        assert_eq!(layout.band_x[3], 300);
        assert_eq!(layout.band_w, 100);
    }

    #[test]
    fn render_at_min_size() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(&mut pixmap, 290, 350, 430, 150, &params, &mut tr, 1.0);
    }

    #[test]
    fn arc_ring_no_panic_zero_amount() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(&mut pixmap, 290, 350, 430, 150, &params, &mut tr, 1.0);
    }
}
