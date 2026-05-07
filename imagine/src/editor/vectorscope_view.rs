//! Vectorscope view: half-polar Ozone-style disc, full-square polar dot cloud,
//! or Lissajous trace.
//! Below the scope: correlation bar + balance bar.

use crate::theme;
use crate::vectorscope::VectorConsumer;
use crate::ImagineParams;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tiny_skia::{Color, Paint, Pixmap, PixmapMut, Rect, Transform};
use tiny_skia_widgets::TextRenderer;

const SAMPLE_BUDGET: usize = 4096;

/// Reserved footer height beneath the scope dot cloud for the mode label
/// ("Polar" / "Goniometer" / "Lissajous"). Sits between the scope and the
/// correlation/balance bars (which themselves occupy the bottom 36 px).
const MODE_LABEL_FOOTER_H: i32 = 16;

#[inline]
fn scaled(v: i32, s: f32) -> i32 {
    ((v as f32) * s).round().max(1.0) as i32
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VectorMode {
    /// Ozone-style half-disc polar scope. Mono → straight up,
    /// hard-left/right → bottom corners, anti-phase → baseline. Default.
    HalfPolar = 0,
    /// Full-square 45°-rotated dot cloud, dual-tone (pink = L, cyan = R).
    Polar = 1,
    /// L on X, R on Y (no rotation).
    Lissajous = 2,
}

impl VectorMode {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => VectorMode::Polar,
            2 => VectorMode::Lissajous,
            _ => VectorMode::HalfPolar,
        }
    }

    pub fn as_u32(self) -> u32 {
        self as u32
    }

    pub fn next(self) -> Self {
        match self {
            VectorMode::HalfPolar => VectorMode::Polar,
            VectorMode::Polar => VectorMode::Lissajous,
            VectorMode::Lissajous => VectorMode::HalfPolar,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VectorMode::HalfPolar => "Polar",
            VectorMode::Polar => "Goniometer",
            VectorMode::Lissajous => "Lissajous",
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
    scale_factor: f32,
) {
    let s = scale_factor.max(0.1);
    let mode = VectorMode::from_u32(params.vector_mode.load(Ordering::Relaxed));
    let corr = f32::from_bits(params.correlation.load(Ordering::Relaxed));
    let bal = f32::from_bits(params.balance.load(Ordering::Relaxed));

    let pad = scaled(6, s);
    let bar_h = scaled(8, s);
    let bar_gap = scaled(4, s);
    let bottom_reserve = scaled(36, s);
    let footer_h = scaled(MODE_LABEL_FOOTER_H, s);

    {
        let mut pm = pixmap.as_mut();
        fill_rect_i(&mut pm, x, y, w, h, theme::panel_bg());
        stroke_rect_i(&mut pm, x, y, w, h, theme::border());

        // Reserve bottom area for correlation + balance bars, plus an
        // additional footer above them for the mode label so it
        // doesn't overlap the dot cloud.
        let scope_h = h - bottom_reserve - footer_h;
        let scope_x = x + pad;
        let scope_y = y + pad;
        let scope_w = w - 2 * pad;

        // Snapshot up to SAMPLE_BUDGET samples.
        if vec_l.len() < SAMPLE_BUDGET {
            vec_l.resize(SAMPLE_BUDGET, 0.0);
            vec_r.resize(SAMPLE_BUDGET, 0.0);
        }
        let n = vec.snapshot(SAMPLE_BUDGET, vec_l, vec_r);

        let cx = scope_x + scope_w / 2;
        let cy = scope_y + scope_h / 2;
        let r = scope_w.min(scope_h) / 2 - pad;

        match mode {
            VectorMode::HalfPolar => {
                // Half-disc has its own grid (half-circle outline + spokes +
                // baseline) drawn inside draw_half_polar.
            }
            VectorMode::Polar | VectorMode::Lissajous => {
                // Background grid (a faint cross) for the square modes.
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
            }
        }

        match mode {
            VectorMode::HalfPolar => draw_half_polar(
                &mut pm,
                scope_x,
                scope_y,
                scope_w,
                scope_h,
                &vec_l[..n],
                &vec_r[..n],
                theme::cyan(),
                s,
            ),
            VectorMode::Polar => draw_polar(&mut pm, cx, cy, r, &vec_l[..n], &vec_r[..n]),
            VectorMode::Lissajous => draw_lissajous(&mut pm, cx, cy, r, &vec_l[..n], &vec_r[..n]),
        }

        // Correlation bar at the bottom. Bar layout (from bottom up):
        //   pad-from-bottom (bar_gap), bal bar (bar_h), gap, corr bar (bar_h)
        let bal_y = y + h - bar_h - bar_gap;
        let corr_y = bal_y - bar_h - bar_gap;
        let corr_color = theme::cyan_to_pink(if corr > 0.0 { 0.0 } else { 1.0 });
        draw_meter_bar(
            &mut pm,
            x + pad,
            corr_y,
            w - 2 * pad,
            bar_h,
            corr,
            corr_color,
        );

        // Balance bar
        draw_meter_bar(
            &mut pm,
            x + pad,
            bal_y,
            w - 2 * pad,
            bar_h,
            bal,
            theme::accent(),
        );
    }

    // Text labels.
    let label_size = (10.0_f32 * s).max(6.0);
    let small_size = (9.0_f32 * s).max(6.0);
    let mode_label = mode.label();

    // For HalfPolar, render the L/R corner labels and the +1/0/-1 amplitude
    // markers. The disc geometry must mirror draw_half_polar exactly.
    if mode == VectorMode::HalfPolar {
        let scope_h = h - bottom_reserve - footer_h;
        let scope_x = x + pad;
        let scope_y = y + pad;
        let scope_w = w - 2 * pad;
        let (cx, base_y, disc_radius) = half_disc_geometry(scope_x, scope_y, scope_w, scope_h, s);
        let r_f = disc_radius as f32;
        let cx_f = cx as f32;
        let base_yf = base_y as f32;
        let pad_f = (4.0_f32 * s).max(2.0);

        // L label (bottom-left corner, just outside the disc baseline).
        text_renderer.draw_text(
            pixmap,
            cx_f - r_f - small_size * 0.6 - pad_f,
            base_yf + small_size * 0.4,
            "L",
            small_size,
            theme::text_dim(),
        );
        // R label (bottom-right corner, just outside the disc baseline).
        text_renderer.draw_text(
            pixmap,
            cx_f + r_f + pad_f,
            base_yf + small_size * 0.4,
            "R",
            small_size,
            theme::text_dim(),
        );
        // Amplitude markers along the right edge: +1 at top, 0 at base.
        text_renderer.draw_text(
            pixmap,
            cx_f + r_f + pad_f,
            base_yf - r_f + small_size * 0.4,
            "+1",
            small_size,
            theme::text_dim(),
        );
        text_renderer.draw_text(
            pixmap,
            cx_f + pad_f,
            base_yf - small_size * 0.2,
            "0",
            small_size,
            theme::text_dim(),
        );
    }
    // Mode toggle area is the bottom-left corner above the meter bars
    // (matches the hit region in the editor: 80×16 px starting at +pad from
    // the panel's left edge, ending bottom_reserve px above the bottom).
    let toggle_y1 = (y + h) as f32 - bottom_reserve as f32;
    let toggle_y0 = toggle_y1 - footer_h as f32;
    text_renderer.draw_text(
        pixmap,
        x as f32 + pad as f32,
        toggle_y0 + (toggle_y1 - toggle_y0) * 0.5 + label_size * 0.35,
        mode_label,
        label_size,
        theme::text(),
    );

    // Correlation / Balance captions and values. Recompute the bar positions
    // (mirrors the shape pass above) so captions sit just above each bar.
    let bal_y = y + h - bar_h - bar_gap;
    let corr_y = bal_y - bar_h - bar_gap;
    let corr_text = format!("Corr {:+.2}", corr);
    text_renderer.draw_text(
        pixmap,
        x as f32 + pad as f32,
        corr_y as f32 - 2.0,
        &corr_text,
        small_size,
        theme::text_dim(),
    );
    let bal_text = format!("Bal {:+.2}", bal);
    text_renderer.draw_text(
        pixmap,
        x as f32 + pad as f32,
        bal_y as f32 - 2.0,
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

/// Compute the inscribed half-disc geometry for a given panel rect.
/// Returns `(cx, base_y, disc_radius)`. The half-disc is the upper hemisphere
/// of a circle whose flat baseline sits at `base_y` and whose center is at
/// `cx`. Radius is bounded by panel width / 2 and panel height (with margin).
fn half_disc_geometry(
    panel_x: i32,
    panel_y: i32,
    panel_w: i32,
    panel_h: i32,
    s: f32,
) -> (i32, i32, i32) {
    let margin = scaled(8, s);
    let max_r_x = panel_w / 2 - margin;
    let max_r_y = panel_h - margin * 2;
    let disc_radius = max_r_x.min(max_r_y).max(1);
    let cx = panel_x + panel_w / 2;
    // Anchor the baseline near the bottom of the panel, leaving a small
    // margin so the L/R labels (drawn just below the baseline) have room.
    let base_y = panel_y + panel_h - margin;
    (cx, base_y, disc_radius)
}

/// Ozone-style half-disc polar scope.
/// Origin at bottom-center. Mono → straight up, hard-left → bottom-left,
/// hard-right → bottom-right, anti-phase → baseline.
#[allow(clippy::too_many_arguments)]
fn draw_half_polar(
    pixmap: &mut PixmapMut<'_>,
    panel_x: i32,
    panel_y: i32,
    panel_w: i32,
    panel_h: i32,
    samples_l: &[f32],
    samples_r: &[f32],
    color: Color,
    scale: f32,
) {
    let (cx, base_y, disc_radius) = half_disc_geometry(panel_x, panel_y, panel_w, panel_h, scale);

    // Faint half-circle outline + spokes + baseline.
    draw_half_disc_grid(pixmap, cx, base_y, disc_radius, theme::text_dim());

    // Plot each sample as a 1×1 dot.
    let r_f = disc_radius as f32;
    let n = samples_l.len().min(samples_r.len());
    for i in 0..n {
        let l = samples_l[i].clamp(-1.0, 1.0);
        let rr = samples_r[i].clamp(-1.0, 1.0);
        // Side: hard-left (L=1, R=0) → s = -0.5 → dot to LEFT of center.
        let s_axis = (rr - l) * 0.5;
        // Mid: |M| so anti-phase content stays on the baseline.
        let m_axis = ((l + rr) * 0.5).abs();
        // Clamp to disc: scale (s,m) so radius ≤ 1.
        let mag = (s_axis * s_axis + m_axis * m_axis).sqrt();
        let (s_unit, m_unit) = if mag > 1.0 {
            (s_axis / mag, m_axis / mag)
        } else {
            (s_axis, m_axis)
        };
        let px = cx + (s_unit * r_f) as i32;
        let py = base_y - (m_unit * r_f) as i32;
        put_pixel(pixmap, px, py, color);
    }
}

/// Faint half-circle outline + 7 angular spokes + baseline at the bottom.
/// Uses a midpoint-circle algorithm for the arc (direct pixel writes —
/// matches the existing dot-cloud rendering style).
fn draw_half_disc_grid(
    pixmap: &mut PixmapMut<'_>,
    cx: i32,
    base_y: i32,
    radius: i32,
    color: Color,
) {
    if radius <= 0 {
        return;
    }
    // Baseline (horizontal at base_y).
    fill_rect_i(pixmap, cx - radius, base_y, 2 * radius + 1, 1, color);

    // Half-circle outline (upper half only). Midpoint circle algorithm,
    // emitting only the four octants whose Y is above (or on) the baseline.
    let mut x = radius;
    let mut y = 0;
    let mut err = 1 - x;
    while x >= y {
        // Upper half: use base_y - y and base_y - x (since screen-y grows down).
        // Right half (octants 1,2):
        put_pixel(pixmap, cx + x, base_y - y, color);
        put_pixel(pixmap, cx + y, base_y - x, color);
        // Left half (octants 3,4):
        put_pixel(pixmap, cx - x, base_y - y, color);
        put_pixel(pixmap, cx - y, base_y - x, color);

        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }

    // Angular spokes radiating from origin every π/8 (22.5°). Eight evenly
    // spaced spokes from horizontal-left (π) → horizontal-right (0) gives 7
    // *internal* divisions. We draw the 7 internal spokes (skip the
    // endpoints, which coincide with the baseline).
    let r_f = radius as f32;
    for i in 1..7 {
        let angle = std::f32::consts::PI - std::f32::consts::PI * (i as f32) / 7.0;
        let ex = cx + (r_f * angle.cos()) as i32;
        let ey = base_y - (r_f * angle.sin()) as i32;
        draw_line(pixmap, cx, base_y, ex, ey, color);
    }
}

/// Bresenham line for the spokes. Direct pixel writes, no anti-aliasing —
/// consistent with the rest of this module.
fn draw_line(pixmap: &mut PixmapMut<'_>, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        put_pixel(pixmap, x, y, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
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
        assert_eq!(VectorMode::from_u32(0), VectorMode::HalfPolar);
        assert_eq!(VectorMode::from_u32(1), VectorMode::Polar);
        assert_eq!(VectorMode::from_u32(2), VectorMode::Lissajous);
        assert_eq!(VectorMode::from_u32(99), VectorMode::HalfPolar);
    }

    #[test]
    fn vector_mode_default_is_half_polar() {
        // ImagineParams initializes vector_mode to AtomicU32::new(0) and
        // HalfPolar must be the 0-discriminant so the default is half-polar.
        let params = ImagineParams::default();
        assert_eq!(
            VectorMode::from_u32(params.vector_mode.load(Ordering::Relaxed)),
            VectorMode::HalfPolar,
        );
    }

    #[test]
    fn vector_mode_next_cycles() {
        assert_eq!(VectorMode::HalfPolar.next(), VectorMode::Polar);
        assert_eq!(VectorMode::Polar.next(), VectorMode::Lissajous);
        assert_eq!(VectorMode::Lissajous.next(), VectorMode::HalfPolar);
    }

    #[test]
    fn vector_mode_labels() {
        assert_eq!(VectorMode::HalfPolar.label(), "Polar");
        assert_eq!(VectorMode::Polar.label(), "Goniometer");
        assert_eq!(VectorMode::Lissajous.label(), "Lissajous");
    }

    #[test]
    fn vector_mode_as_u32_roundtrip() {
        for m in [
            VectorMode::HalfPolar,
            VectorMode::Polar,
            VectorMode::Lissajous,
        ] {
            assert_eq!(VectorMode::from_u32(m.as_u32()), m);
        }
    }

    #[test]
    fn render_half_polar_with_data() {
        let params = Arc::new(ImagineParams::default());
        params
            .vector_mode
            .store(VectorMode::HalfPolar.as_u32(), Ordering::Relaxed);
        let (prod, cons) = ring_pair();
        let cons = Arc::new(cons);
        // Push a mix of mono / hard-left / hard-right / anti-phase samples.
        for i in 0..512 {
            let t = i as f32 * 0.05;
            prod.push(t.sin() * 0.5, t.sin() * 0.5); // mono
            prod.push(0.5, 0.0); // hard-left
            prod.push(0.0, 0.5); // hard-right
            prod.push(0.5, -0.5); // anti-phase
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
            1.0,
        );
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
            1.0,
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
            1.0,
        );
    }
}
