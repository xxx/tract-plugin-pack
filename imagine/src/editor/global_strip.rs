//! Global strip: Recover Sides + Link Bands + Quality.

use crate::theme;
use crate::{ImagineParams, Quality};
use std::sync::Arc;
use tiny_skia::{Color, Paint, Pixmap, PixmapMut, Rect, Transform};
use tiny_skia_widgets::TextRenderer;

/// Reserved height for the section caption row at the top of the strip
/// ("Recover Sides", "Link", "Quality").
pub const HEADER_H: i32 = 14;

#[inline]
fn scaled(v: i32, s: f32) -> i32 {
    ((v as f32) * s).round().max(1.0) as i32
}

pub struct GlobalLayout {
    pub recover_rect: (i32, i32, i32, i32),
    pub link_rect: (i32, i32, i32, i32),
    pub quality_rect: (i32, i32, i32, i32),
}

pub fn compute_layout(x: i32, y: i32, w: i32, h: i32, scale_factor: f32) -> GlobalLayout {
    let s = scale_factor.max(0.1);
    // Three sections, equal-ish width. The top HEADER_H pixels are reserved
    // for the section captions ("Recover Sides", "Link", "Quality") and the
    // remaining height is the control body.
    let pad = scaled(8, s);
    let header_h = scaled(HEADER_H, s);
    let section_w = (w - 4 * pad) / 3;
    let body_top = header_h + scaled(2, s);
    let body_h = (h - body_top - scaled(4, s)).max(1);
    GlobalLayout {
        recover_rect: (x + pad, y + body_top, section_w, body_h),
        link_rect: (x + pad * 2 + section_w, y + body_top, section_w, body_h),
        quality_rect: (x + pad * 3 + section_w * 2, y + body_top, section_w, body_h),
    }
}

fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut paint = Paint::default();
    paint.set_color(color);
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

    // Shape pass — borrow PixmapMut for rect helpers.
    let recover_norm = params.recover_sides.value() / 100.0;
    let link_on = params.link_bands.value();
    let quality_v = params.quality.value();
    {
        let mut pm = pixmap.as_mut();

        // Background
        fill_rect_i(&mut pm, x, y, w, h, theme::panel_bg());
        stroke_rect_i(&mut pm, x, y, w, h, theme::border());

        // Recover Sides — horizontal bar with marker
        let (rx, ry, rw, rh) = layout.recover_rect;
        fill_rect_i(&mut pm, rx, ry, rw, rh, theme::spectrum_bg());
        stroke_rect_i(&mut pm, rx, ry, rw, rh, theme::border());
        let bar_w = (recover_norm * rw as f32) as i32;
        fill_rect_i(
            &mut pm,
            rx,
            ry,
            bar_w,
            rh,
            theme::cyan_to_pink(recover_norm),
        );

        // Link Bands — toggle button
        let (lx, ly, lw, lh) = layout.link_rect;
        let link_color = if link_on {
            theme::cyan_to_pink(0.5)
        } else {
            theme::spectrum_bg()
        };
        fill_rect_i(&mut pm, lx, ly, lw, lh, link_color);
        stroke_rect_i(&mut pm, lx, ly, lw, lh, theme::border());

        // Quality — 2-segment selector (Linear / IIR)
        let (qx, qy, qw, qh) = layout.quality_rect;
        let half_w = qw / 2;
        fill_rect_i(&mut pm, qx, qy, qw, qh, theme::spectrum_bg());
        stroke_rect_i(&mut pm, qx, qy, qw, qh, theme::border());
        let active_x = match quality_v {
            Quality::Linear => qx,
            Quality::Iir => qx + half_w,
        };
        fill_rect_i(&mut pm, active_x, qy, half_w, qh, theme::accent());
    }

    // Text pass — section captions in the HEADER_H row at the top of the
    // strip, value labels inside their respective bodies.
    let label_size = (11.0_f32 * s).max(6.0);
    let value_size = (10.5_f32 * s).max(6.0);
    // Baseline for header captions: drop label_size into the HEADER_H row.
    let header_y = y as f32 + label_size + 1.0;

    // "Recover Sides" caption above the bar; current value sits inside the bar.
    let (rx, ry, rw, rh) = layout.recover_rect;
    let recover_caption = "Recover Sides";
    let rcap_w = text_renderer.text_width(recover_caption, label_size);
    text_renderer.draw_text(
        pixmap,
        rx as f32 + (rw as f32 - rcap_w) * 0.5,
        header_y,
        recover_caption,
        label_size,
        theme::text(),
    );
    let v = params.recover_sides.value();
    let value_text = format!("{:.0}", v);
    let vw = text_renderer.text_width(&value_text, value_size);
    let val_y = ry as f32 + (rh as f32) * 0.5 + value_size * 0.35;
    text_renderer.draw_text(
        pixmap,
        rx as f32 + rw as f32 - vw - 6.0,
        val_y,
        &value_text,
        value_size,
        theme::text_dim(),
    );

    // "Link" caption above the toggle; the toggle body itself shows on/off.
    let (lx, ly, lw, lh) = layout.link_rect;
    let link_label = "Link";
    let lw_text = text_renderer.text_width(link_label, label_size);
    let link_color = if link_on {
        theme::text()
    } else {
        theme::text_dim()
    };
    text_renderer.draw_text(
        pixmap,
        lx as f32 + lw as f32 * 0.5 - lw_text * 0.5,
        header_y,
        link_label,
        label_size,
        link_color,
    );
    // State indicator inside the toggle ("On" / "Off").
    let link_state = if link_on { "On" } else { "Off" };
    let lsw = text_renderer.text_width(link_state, value_size);
    // Inside the toggle the bg is `cyan_to_pink(0.5)` when on (a warm
    // mid-tone) — use `on_accent()` for contrast there. When off the
    // bg is the dark `spectrum_bg`, so dimmed cream reads fine.
    let link_state_color = if link_on {
        theme::on_accent()
    } else {
        theme::text_dim()
    };
    text_renderer.draw_text(
        pixmap,
        lx as f32 + lw as f32 * 0.5 - lsw * 0.5,
        ly as f32 + lh as f32 * 0.5 + value_size * 0.35,
        link_state,
        value_size,
        link_state_color,
    );

    // "Quality" caption above the selector; halves labelled inside.
    let (qx, qy, qw, qh) = layout.quality_rect;
    let quality_caption = "Quality";
    let qcap_w = text_renderer.text_width(quality_caption, label_size);
    text_renderer.draw_text(
        pixmap,
        qx as f32 + (qw as f32 - qcap_w) * 0.5,
        header_y,
        quality_caption,
        label_size,
        theme::text(),
    );
    let half_w = qw / 2;
    let lin_label = "Linear";
    let iir_label = "IIR";
    let lin_w = text_renderer.text_width(lin_label, value_size);
    let iir_w = text_renderer.text_width(iir_label, value_size);
    let lin_active = matches!(quality_v, Quality::Linear);
    let lin_color = if lin_active {
        theme::on_accent()
    } else {
        theme::text_dim()
    };
    let iir_color = if lin_active {
        theme::text_dim()
    } else {
        theme::on_accent()
    };
    let q_y = qy as f32 + qh as f32 * 0.5 + value_size * 0.35;
    text_renderer.draw_text(
        pixmap,
        qx as f32 + half_w as f32 * 0.5 - lin_w * 0.5,
        q_y,
        lin_label,
        value_size,
        lin_color,
    );
    text_renderer.draw_text(
        pixmap,
        qx as f32 + half_w as f32 + half_w as f32 * 0.5 - iir_w * 0.5,
        q_y,
        iir_label,
        value_size,
        iir_color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_three_sections() {
        let l = compute_layout(0, 0, 600, 52, 1.0);
        assert!(l.recover_rect.0 < l.link_rect.0);
        assert!(l.link_rect.0 < l.quality_rect.0);
    }

    #[test]
    fn layout_reserves_header_row() {
        // Body rects must start strictly below the HEADER_H reserve so the
        // section captions never overlap the control bodies.
        let l = compute_layout(0, 0, 600, 52, 1.0);
        assert!(l.recover_rect.1 >= HEADER_H);
        assert!(l.link_rect.1 >= HEADER_H);
        assert!(l.quality_rect.1 >= HEADER_H);
    }

    #[test]
    fn render_at_min_size() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(&mut pixmap, 0, 528, 720, 52, &params, &mut tr, 1.0);
    }
}
