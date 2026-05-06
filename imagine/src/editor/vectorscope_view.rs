//! Vectorscope view: polar-sample dot cloud OR Lissajous trace.
//! Below the scope: correlation bar + balance bar.

use crate::theme;
use crate::vectorscope::VectorConsumer;
use crate::ImagineParams;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tiny_skia::{Color, Paint, Pixmap, PixmapMut, Rect, Transform};
use tiny_skia_widgets::TextRenderer;

const SAMPLE_BUDGET: usize = 4096;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VectorMode {
    Polar = 0,
    Lissajous = 1,
}

impl VectorMode {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => VectorMode::Lissajous,
            _ => VectorMode::Polar,
        }
    }
}

/// Local fill helper using PixmapMut::fill_rect with opaque BlendMode::Source.
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

/// Direct-pixel fill for a single 1×1 dot. Bounds-checked.
fn put_pixel(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, color: Color) {
    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let pixels = pixmap.pixels_mut();
    let idx = (y as usize) * (w as usize) + (x as usize);
    let cu = color.to_color_u8();
    pixels[idx] =
        tiny_skia::PremultipliedColorU8::from_rgba(cu.red(), cu.green(), cu.blue(), cu.alpha())
            .unwrap_or(pixels[idx]);
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    params: &Arc<ImagineParams>,
    vec: &Arc<VectorConsumer>,
    vec_l: &mut Vec<f32>,
    vec_r: &mut Vec<f32>,
    text_renderer: &mut TextRenderer,
) {
    let mode = VectorMode::from_u32(params.vector_mode.load(Ordering::Relaxed));
    let corr = f32::from_bits(params.correlation.load(Ordering::Relaxed));
    let bal = f32::from_bits(params.balance.load(Ordering::Relaxed));

    {
        let mut pm = pixmap.as_mut();
        fill_rect_i(&mut pm, x, y, w, h, theme::panel_bg());
        stroke_rect_i(&mut pm, x, y, w, h, theme::border());

        // Reserve bottom 36 px for correlation + balance bars.
        let scope_h = h - 36;
        let scope_x = x + 6;
        let scope_y = y + 6;
        let scope_w = w - 12;

        // Snapshot up to SAMPLE_BUDGET samples.
        if vec_l.len() < SAMPLE_BUDGET {
            vec_l.resize(SAMPLE_BUDGET, 0.0);
            vec_r.resize(SAMPLE_BUDGET, 0.0);
        }
        let n = vec.snapshot(SAMPLE_BUDGET, vec_l, vec_r);

        let cx = scope_x + scope_w / 2;
        let cy = scope_y + scope_h / 2;
        let r = scope_w.min(scope_h) / 2 - 6;

        // Background grid (a faint cross).
        fill_rect_i(
            &mut pm,
            scope_x,
            scope_y + scope_h / 2,
            scope_w,
            1,
            theme::text_dim(),
        );
        fill_rect_i(
            &mut pm,
            scope_x + scope_w / 2,
            scope_y,
            1,
            scope_h,
            theme::text_dim(),
        );

        match mode {
            VectorMode::Polar => draw_polar(&mut pm, cx, cy, r, &vec_l[..n], &vec_r[..n]),
            VectorMode::Lissajous => draw_lissajous(&mut pm, cx, cy, r, &vec_l[..n], &vec_r[..n]),
        }

        // Correlation bar at the bottom
        let bar_y = y + h - 28;
        let corr_color = theme::cyan_to_pink(if corr > 0.0 { 0.0 } else { 1.0 });
        draw_meter_bar(&mut pm, x + 6, bar_y, w - 12, 8, corr, corr_color);

        // Balance bar
        let bal_y = y + h - 16;
        draw_meter_bar(&mut pm, x + 6, bal_y, w - 12, 8, bal, theme::accent());
    }

    // Text labels.
    let label_size = 10.0_f32;
    let mode_label = match mode {
        VectorMode::Polar => "Polar",
        VectorMode::Lissajous => "Lissajous",
    };
    // Mode toggle area is the bottom-left corner above the meter bars
    // (matches the hit region in the editor: 80×16 px starting at +6 px from
    // the panel's left edge, ending 36 px above the bottom).
    let toggle_y1 = (y + h) as f32 - 36.0;
    let toggle_y0 = toggle_y1 - 16.0;
    text_renderer.draw_text(
        pixmap,
        x as f32 + 6.0,
        toggle_y0 + (toggle_y1 - toggle_y0) * 0.5 + label_size * 0.35,
        mode_label,
        label_size,
        theme::text(),
    );

    // Correlation / Balance captions and values.
    let small_size = 9.0_f32;
    let corr_text = format!("Corr {:+.2}", corr);
    text_renderer.draw_text(
        pixmap,
        x as f32 + 6.0,
        (y + h - 28) as f32 - 2.0,
        &corr_text,
        small_size,
        theme::text_dim(),
    );
    let bal_text = format!("Bal {:+.2}", bal);
    text_renderer.draw_text(
        pixmap,
        x as f32 + 6.0,
        (y + h - 16) as f32 - 2.0,
        &bal_text,
        small_size,
        theme::text_dim(),
    );
}

/// Polar-sample dot cloud, 45°-rotated so mono = vertical axis.
/// Pink for L-leaning, cyan for R-leaning.
fn draw_polar(pixmap: &mut PixmapMut<'_>, cx: i32, cy: i32, radius: i32, l: &[f32], r: &[f32]) {
    let r_f = radius as f32;
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    for i in 0..l.len() {
        let xn = (l[i] - r[i]) * inv_sqrt2;
        let yn = -((l[i] + r[i]) * inv_sqrt2);
        let px = cx + (xn.clamp(-1.0, 1.0) * r_f) as i32;
        let py = cy + (yn.clamp(-1.0, 1.0) * r_f) as i32;
        // Color by L-vs-R dominance.
        let denom = l[i].abs() + r[i].abs() + 1e-9;
        let bias = if l[i].abs() > r[i].abs() {
            ((l[i].abs() - r[i].abs()) / denom).clamp(0.0, 1.0)
        } else {
            -((r[i].abs() - l[i].abs()) / denom).clamp(0.0, 1.0)
        };
        // bias > 0 → pink (L); bias < 0 → cyan (R)
        let t = (bias + 1.0) * 0.5;
        let color = theme::cyan_to_pink(t);
        put_pixel(pixmap, px, py, color);
    }
}

/// Lissajous: L on X, R on Y, no rotation.
fn draw_lissajous(pixmap: &mut PixmapMut<'_>, cx: i32, cy: i32, radius: i32, l: &[f32], r: &[f32]) {
    let r_f = radius as f32;
    for i in 0..l.len() {
        let px = cx + (l[i].clamp(-1.0, 1.0) * r_f) as i32;
        let py = cy - (r[i].clamp(-1.0, 1.0) * r_f) as i32;
        put_pixel(pixmap, px, py, theme::accent());
    }
}

/// Horizontal bar with a marker at `value` ∈ [-1, 1].
fn draw_meter_bar(
    pixmap: &mut PixmapMut<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    value: f32,
    color: Color,
) {
    fill_rect_i(pixmap, x, y, w, h, theme::panel_bg());
    stroke_rect_i(pixmap, x, y, w, h, theme::border());
    let cx = x + w / 2;
    fill_rect_i(pixmap, cx, y + 1, 1, h - 2, theme::text_dim());
    let v = value.clamp(-1.0, 1.0);
    let marker_x = x + ((v + 1.0) * 0.5 * w as f32) as i32;
    fill_rect_i(pixmap, marker_x - 1, y + 1, 3, h - 2, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectorscope::ring_pair;

    #[test]
    fn vector_mode_from_u32() {
        assert_eq!(VectorMode::from_u32(0), VectorMode::Polar);
        assert_eq!(VectorMode::from_u32(1), VectorMode::Lissajous);
        assert_eq!(VectorMode::from_u32(99), VectorMode::Polar);
    }

    #[test]
    fn render_with_empty_ring() {
        let params = Arc::new(ImagineParams::default());
        let (_, cons) = ring_pair();
        let cons = Arc::new(cons);
        let mut pixmap = tiny_skia::Pixmap::new(300, 400).unwrap();
        let mut vl = vec![0.0; SAMPLE_BUDGET];
        let mut vr = vec![0.0; SAMPLE_BUDGET];
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(
            &mut pixmap,
            0,
            0,
            300,
            400,
            &params,
            &cons,
            &mut vl,
            &mut vr,
            &mut tr,
        );
    }

    #[test]
    fn render_with_data() {
        let params = Arc::new(ImagineParams::default());
        let (prod, cons) = ring_pair();
        let cons = Arc::new(cons);
        for i in 0..1000 {
            let phase = (i as f32 * 0.1).sin();
            prod.push(phase * 0.5, phase * 0.6);
        }
        let mut pixmap = tiny_skia::Pixmap::new(300, 400).unwrap();
        let mut vl = vec![0.0; SAMPLE_BUDGET];
        let mut vr = vec![0.0; SAMPLE_BUDGET];
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(
            &mut pixmap,
            0,
            0,
            300,
            400,
            &params,
            &cons,
            &mut vl,
            &mut vr,
            &mut tr,
        );
    }
}
