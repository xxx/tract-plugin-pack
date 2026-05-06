//! Spectrum view: input |M| backdrop + 3 draggable splits + coherence bar.

use crate::spectrum::NUM_LOG_BINS;
use crate::theme;
use crate::ImagineParams;
use std::sync::Arc;
use tiny_skia::{BlendMode, Color, Paint, Pixmap, PixmapMut, Rect, Transform};
use tiny_skia_widgets::TextRenderer;

/// Format a frequency in Hz as a short display string.
/// Examples: 80 -> "80 Hz", 1000 -> "1.0 kHz", 8400 -> "8.4 kHz".
pub(crate) fn format_freq(hz: f32) -> String {
    if hz < 1000.0 {
        format!("{:.0} Hz", hz)
    } else {
        format!("{:.1} kHz", hz / 1000.0)
    }
}

const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20_000.0;

/// Map a normalized x ∈ [0, 1] to frequency in Hz (log-scaled).
///
/// Inverse of [`hz_to_x`]. Used by Task 17's hit-testing for split drags.
#[allow(dead_code)]
pub fn x_to_hz(x_norm: f32) -> f32 {
    let log_min = F_MIN.ln();
    let log_max = F_MAX.ln();
    (log_min + (log_max - log_min) * x_norm.clamp(0.0, 1.0)).exp()
}

/// Map frequency in Hz to normalized x ∈ [0, 1].
pub fn hz_to_x(hz: f32) -> f32 {
    let log_min = F_MIN.ln();
    let log_max = F_MAX.ln();
    let log_hz = hz.clamp(F_MIN, F_MAX).ln();
    ((log_hz - log_min) / (log_max - log_min)).clamp(0.0, 1.0)
}

/// Reserved height at the top of the spectrum panel for frequency labels above
/// each split handle. Spectrum bars and split handles are inset below this.
pub const LABEL_H: i32 = 14;

/// Pixel x for a split at given frequency, given the panel's left edge and width.
pub fn split_pixel_x(panel_x: i32, panel_w: i32, hz: f32) -> i32 {
    panel_x + (hz_to_x(hz) * panel_w as f32) as i32
}

// ── Local helpers: integer-rect fills on a PixmapMut ────────────────────

fn fill_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    if color.alpha() >= 1.0 {
        paint.blend_mode = BlendMode::Source;
    }
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn stroke_rect_i(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    fill_rect_i(pixmap, x, y, w, 1, color);
    fill_rect_i(pixmap, x, y + h - 1, w, 1, color);
    fill_rect_i(pixmap, x, y + 1, 1, (h - 2).max(0), color);
    fill_rect_i(pixmap, x + w - 1, y + 1, 1, (h - 2).max(0), color);
}

pub fn draw(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    params: &Arc<ImagineParams>,
    text_renderer: &mut TextRenderer,
) {
    let f1 = params.xover_1.value();
    let f2 = params.xover_2.value();
    let f3 = params.xover_3.value();

    // Spectrum bars, split lines, and split handles are all inset below the
    // top LABEL_H reserve so the frequency labels can sit at y=2..LABEL_H
    // without overlapping the bars.
    let bars_top = y + LABEL_H;
    let bars_h = (h - LABEL_H).max(1);

    {
        let mut pm = pixmap.as_mut();

        // Panel background + border.
        fill_rect_i(&mut pm, x, y, w, h, theme::spectrum_bg());
        stroke_rect_i(&mut pm, x, y, w, h, theme::border());

        // |M| spectrum bars (one per log bin, scaled to bars_h).
        let bar_w = (w as f32 / NUM_LOG_BINS as f32).max(1.0);
        let bar_h_max = (bars_h as f32 - 8.0).max(1.0);
        for i in 0..NUM_LOG_BINS {
            let mag = params.spectrum_display.read_mag_m(i);
            // Convert linear magnitude to pseudo-dB display
            // (-60 dBFS = 0, 0 dBFS = full).
            let db = 20.0_f32 * mag.max(1e-9).log10();
            let h_norm = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let bar_height = (h_norm * bar_h_max) as i32;
            if bar_height <= 0 {
                continue;
            }
            let bar_x = x + (i as f32 * bar_w) as i32;
            let bar_y = y + h - 4 - bar_height;
            fill_rect_i(
                &mut pm,
                bar_x,
                bar_y,
                bar_w as i32,
                bar_height,
                theme::cyan_to_pink(0.5),
            );
        }

        // 3 draggable split lines + handles. Handles sit just below the label
        // strip (y = LABEL_H + 2 from the panel top).
        let handle_y = bars_top + 2;
        let line_y = bars_top;
        let line_h = (y + h - line_y - 2).max(0);
        for hz in [f1, f2, f3] {
            let lx = split_pixel_x(x, w, hz);
            // Vertical line
            fill_rect_i(&mut pm, lx, line_y, 2, line_h, theme::split_line());
            // Handle (small filled square at the top of the bars region)
            fill_rect_i(&mut pm, lx - 4, handle_y, 9, 8, theme::accent());
        }
    }

    // Frequency labels above each split handle, inside the LABEL_H strip.
    let label_size = 10.0_f32;
    for hz in [f1, f2, f3] {
        let lx = split_pixel_x(x, w, hz);
        let txt = format_freq(hz);
        let tw = text_renderer.text_width(&txt, label_size);
        // Center on the handle, just above the panel's top edge.
        let mut tx = lx as f32 - tw * 0.5;
        // Keep label inside the panel horizontally.
        let min_x = x as f32 + 2.0;
        let max_x = (x + w) as f32 - tw - 2.0;
        if tx < min_x {
            tx = min_x;
        }
        if tx > max_x {
            tx = max_x;
        }
        let ty = y as f32 + label_size + 1.0;
        text_renderer.draw_text(pixmap, tx, ty, &txt, label_size, theme::text());
    }
}

pub fn draw_coherence(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    params: &Arc<ImagineParams>,
    text_renderer: &mut TextRenderer,
) {
    {
        let mut pm = pixmap.as_mut();
        fill_rect_i(&mut pm, x, y, w, h, theme::spectrum_bg());
        stroke_rect_i(&mut pm, x, y, w, h, theme::border());

        // Per-bin: read coherence (1 - γ²), use as both height and color t.
        let bar_w = (w as f32 / NUM_LOG_BINS as f32).max(1.0);
        let bar_h_max = (h as f32 - 8.0).max(1.0);
        for i in 0..NUM_LOG_BINS {
            let v = params.spectrum_display.read_coherence(i).clamp(0.0, 1.0);
            let bar_height = (v * bar_h_max) as i32;
            if bar_height <= 0 {
                continue;
            }
            let bar_x = x + (i as f32 * bar_w) as i32;
            let bar_y = y + h - 4 - bar_height;
            fill_rect_i(
                &mut pm,
                bar_x,
                bar_y,
                bar_w as i32,
                bar_height,
                theme::cyan_to_pink(v),
            );
        }
    }

    // Small "Coherence" caption inside the top-left corner.
    let label_size = 10.0_f32;
    let caption = "Coherence";
    text_renderer.draw_text(
        pixmap,
        x as f32 + 4.0,
        y as f32 + label_size + 1.0,
        caption,
        label_size,
        theme::text_dim(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_to_hz_endpoints() {
        let lo = x_to_hz(0.0);
        let hi = x_to_hz(1.0);
        assert!((lo - F_MIN).abs() < 0.01);
        assert!((hi - F_MAX).abs() < 1.0);
    }

    #[test]
    fn hz_to_x_inverse() {
        for hz in [50.0_f32, 200.0, 1000.0, 5000.0, 15000.0] {
            let x = hz_to_x(hz);
            let back = x_to_hz(x);
            assert!((back - hz).abs() < hz * 0.001, "hz={hz}: back={back}");
        }
    }

    #[test]
    fn x_to_hz_clamps() {
        let lo = x_to_hz(-0.5);
        let hi = x_to_hz(1.5);
        assert!((lo - F_MIN).abs() < 0.01);
        assert!((hi - F_MAX).abs() < 1.0);
    }

    #[test]
    fn split_pixel_x_basic() {
        let panel_x = 100;
        let panel_w = 400;
        // log midpoint: sqrt(20 * 20000) ≈ 632.46
        let mid = split_pixel_x(panel_x, panel_w, 632.0);
        assert!((mid - (panel_x + panel_w / 2)).abs() <= 2);
    }

    #[test]
    fn render_at_min_size_no_panic() {
        let params = Arc::new(ImagineParams::default());
        let mut pixmap = tiny_skia::Pixmap::new(720, 580).unwrap();
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        let mut tr = TextRenderer::new(font_data);
        draw(&mut pixmap, 290, 0, 430, 350, &params, &mut tr);
        draw_coherence(&mut pixmap, 290, 480, 430, 100, &params, &mut tr);
    }

    #[test]
    fn format_freq_examples() {
        assert_eq!(format_freq(80.0), "80 Hz");
        assert_eq!(format_freq(120.0), "120 Hz");
        assert_eq!(format_freq(1000.0), "1.0 kHz");
        assert_eq!(format_freq(8400.0), "8.4 kHz");
    }
}
