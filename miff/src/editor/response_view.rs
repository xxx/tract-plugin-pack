//! Frequency-response view: baked kernel magnitude curve + live input-spectrum shadow.
//!
//! Adapted from `wavetable-filter/src/editor/filter_response_view.rs`. The
//! only structural difference is that miff's kernel `mags` are already
//! normalized (peak = 1.0 = 0 dB) and the input spectrum bins map linearly
//! onto the Nyquist range — no cutoff-frequency remapping is needed.

use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use tiny_skia_widgets::TextRenderer;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const DB_CEIL: f32 = 0.0;
const DB_FLOOR: f32 = -48.0;
const DB_RANGE: f32 = DB_CEIL - DB_FLOOR; // 48.0

/// Map a frequency in Hz to a normalised [0, 1] x position on the log axis.
fn freq_to_x_norm(freq_hz: f32) -> f32 {
    ((freq_hz.max(FREQ_MIN).ln() - FREQ_MIN.ln()) / (FREQ_MAX.ln() - FREQ_MIN.ln())).clamp(0.0, 1.0)
}

fn stroke_line(
    pixmap: &mut Pixmap,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: (u8, u8, u8, u8),
    width: f32,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);
    pb.line_to(x1, y1);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        pixmap.stroke_path(
            &path,
            &paint,
            &Stroke {
                width,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// Draw the frequency-response view into `pixmap` within `rect` (x, y, w, h).
///
/// * `kernel_mags` — normalized magnitude spectrum of the baked kernel; peak = 1.0 (0 dB).
///   Length = `kernel::MAG_BINS` = `MAX_KERNEL/2 + 1`. Bin `k` corresponds to
///   frequency `k * sample_rate / MAX_KERNEL`.  Since we don't know the sample
///   rate here we map linearly from bin 0 (DC) to bin `len−1` (Nyquist = 20 kHz
///   limit for display purposes).
/// * `input_mags` — live input-spectrum magnitudes; may be empty (→ no shadow).
/// * `scale` — UI scale factor (scales the logical-constant inner padding).
pub fn draw_response(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    rect: (f32, f32, f32, f32),
    kernel_mags: &[f32],
    input_mags: &[f32],
    scale: f32,
) {
    let (x, y, w, h) = rect;

    // ── Background ──────────────────────────────────────────────────────────
    if let Some(bg_rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        let mut pb = PathBuilder::new();
        pb.push_rect(bg_rect);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(20, 22, 28, 255);
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            let mut border = Paint::default();
            border.set_color_rgba8(60, 60, 70, 255);
            border.anti_alias = true;
            pixmap.stroke_path(
                &path,
                &border,
                &Stroke {
                    width: 1.0,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    let padding = 20.0 * scale;
    let width = w - padding * 2.0;
    let height = h - padding * 2.0;
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let x0 = x + padding;
    let y0 = y + padding;

    let num_points = (width.max(1.0) as usize).min(256);

    // Pre-compute a log-frequency table: freq_table[i] = Hz for x-column i.
    let log_min = FREQ_MIN.ln();
    let log_range = FREQ_MAX.ln() - log_min;
    let freq_table: Vec<f32> = (0..=num_points)
        .map(|i| {
            let x_norm = i as f32 / num_points as f32;
            (log_min + x_norm * log_range).exp()
        })
        .collect();

    // ── Grid: horizontal dB lines ──────────────────────────────────────────
    for db in [-12.0_f32, -24.0, -36.0, -48.0] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let gy = y0 + height - y_norm * height;
        stroke_line(pixmap, x0, gy, x0 + width, gy, (80, 80, 90, 100), 0.5);
    }
    // 0 dB reference (slightly brighter).
    {
        let y_norm = (DB_CEIL - DB_FLOOR) / DB_RANGE;
        let gy = y0 + height - y_norm * height;
        stroke_line(pixmap, x0, gy, x0 + width, gy, (120, 120, 140, 180), 0.5);
    }
    // Vertical decade lines.
    for freq in [100.0_f32, 1000.0, 10000.0] {
        let gx = x0 + freq_to_x_norm(freq) * width;
        stroke_line(pixmap, gx, y0, gx, y0 + height, (80, 80, 90, 100), 0.5);
    }

    // ── Input-spectrum shadow ───────────────────────────────────────────────
    // Bin k maps linearly to frequency k * (FREQ_MAX / (num_input_bins - 1)).
    // The "floor" guard: if no bin exceeds -48 dB the polygon has zero height
    // and tiny-skia prints a warning. Skip the fill in that case.
    let mag_floor = 10f32.powf(DB_FLOOR / 20.0); // amplitude at -48 dB
    let has_audio = !input_mags.is_empty() && input_mags.iter().any(|&m| m > mag_floor);
    if has_audio {
        let num_input_bins = input_mags.len();
        // Treat the last bin as the Nyquist limit (20 kHz for display).
        let bin_to_hz = FREQ_MAX / (num_input_bins - 1).max(1) as f32;
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0 + height);
        for (i, &freq) in freq_table.iter().enumerate() {
            let bin_f = freq / bin_to_hz;
            let mag = if bin_f <= 0.0 {
                input_mags[0]
            } else if bin_f >= (num_input_bins - 1) as f32 {
                0.0
            } else {
                let lo = bin_f.floor() as usize;
                let frac = bin_f - lo as f32;
                input_mags[lo] * (1.0 - frac) + input_mags[lo + 1] * frac
            };
            let db = 20.0 * mag.max(1e-6).log10();
            let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
            let xx = x0 + (i as f32 / num_points as f32) * width;
            pb.line_to(xx, y0 + height - y_norm * height);
        }
        pb.line_to(x0 + width, y0 + height);
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 200, 100, 25);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    // ── Kernel response curve ───────────────────────────────────────────────
    // Map kernel_mags[k] → frequency using: bin k → k * (FREQ_MAX / (nbins-1)).
    // Then look up the closest kernel_mags bin for each log-spaced x column.
    if !kernel_mags.is_empty() {
        let num_kernel_bins = kernel_mags.len();
        let bin_to_hz = FREQ_MAX / (num_kernel_bins - 1).max(1) as f32;

        let mut fill_pb = PathBuilder::new();
        let mut stroke_pb = PathBuilder::new();
        for (i, freq) in freq_table.iter().enumerate() {
            let bin_f = freq / bin_to_hz;
            let mag = if bin_f <= 0.0 {
                kernel_mags[0]
            } else if bin_f >= (num_kernel_bins - 1) as f32 {
                0.0
            } else {
                let lo = bin_f.floor() as usize;
                let frac = bin_f - lo as f32;
                kernel_mags[lo] * (1.0 - frac) + kernel_mags[lo + 1] * frac
            };
            let db = 20.0 * mag.max(1e-6).log10();
            let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
            let xx = x0 + (i as f32 / num_points as f32) * width;
            let yy = y0 + height - y_norm * height;
            if i == 0 {
                fill_pb.move_to(xx, y0 + height);
                fill_pb.line_to(xx, yy);
                stroke_pb.move_to(xx, yy);
            } else {
                fill_pb.line_to(xx, yy);
                stroke_pb.line_to(xx, yy);
            }
        }
        fill_pb.line_to(x0 + width, y0 + height);
        fill_pb.close();
        if let Some(fill_path) = fill_pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(100, 200, 255, 40);
            paint.anti_alias = true;
            pixmap.fill_path(
                &fill_path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(stroke_path) = stroke_pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(100, 200, 255, 255);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: 2.0,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&stroke_path, &paint, &stroke, Transform::identity(), None);
        }
    }

    // ── Text labels ─────────────────────────────────────────────────────────
    // `h` is already a physical pixel dimension, so `h * 0.045` is DPI-proportional
    // on its own — do NOT multiply by `scale` again (that double-scales at HiDPI).
    let text_size = (h * 0.045).clamp(11.0, 24.0);

    // Frequency labels along the bottom edge.
    let labels_y = y + h - text_size * 0.5;
    for (freq, label) in [
        (50.0_f32, "50"),
        (200.0, "200"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (20000.0, "20k"),
    ] {
        let tw = text_renderer.text_width(label, text_size);
        let tx = x0 + freq_to_x_norm(freq) * width - tw * 0.5;
        text_renderer.draw_text(
            pixmap,
            tx,
            labels_y,
            label,
            text_size,
            tiny_skia::Color::from_rgba8(150, 150, 150, 255),
        );
    }

    // dB labels on the left.
    for (db, label) in [(0.0_f32, "0"), (-24.0, "-24"), (-48.0, "-48")] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let yy = y0 + height - y_norm * height;
        let tw = text_renderer.text_width(label, text_size);
        text_renderer.draw_text(
            pixmap,
            x0 - 3.0 - tw,
            yy + text_size * 0.4,
            label,
            text_size,
            tiny_skia::Color::from_rgba8(150, 150, 150, 255),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Pixmap;
    use tiny_skia_widgets::TextRenderer;

    fn make_text_renderer() -> TextRenderer {
        let font_data = include_bytes!("../fonts/DejaVuSans.ttf");
        TextRenderer::new(font_data)
    }

    /// `draw_response` with a non-trivial kernel and empty input must not panic,
    /// and at least one pixel inside `rect` must be painted (non-zero alpha).
    #[test]
    fn draw_response_paints_inside_rect_no_panic() {
        use crate::kernel;
        use tiny_skia_widgets::mseg::MsegData;

        let w = 880_u32;
        let h = 200_u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        let mut tr = make_text_renderer();

        // Bake a ramp curve (MsegData::default() = 0→1 ramp) → non-trivial kernel.
        let k = kernel::bake(&MsegData::default(), 512);
        assert!(!k.is_zero, "ramp curve must produce a non-zero kernel");

        let rect = (0.0_f32, 0.0, w as f32, h as f32);
        draw_response(&mut pm, &mut tr, rect, &k.mags, &[], 1.0);

        // At least one pixel inside the inner content area must have non-zero alpha.
        // We sample at (w/2, h/2) — well inside the rect.
        let sample_x = w / 2;
        let sample_y = h / 2;
        let pixel = pm.pixels()[(sample_y * w + sample_x) as usize];
        assert!(
            pixel.alpha() > 0,
            "expected a painted pixel at ({sample_x}, {sample_y}), got zero alpha"
        );
    }

    /// With all-zero `kernel_mags` the response curve is at the -48 dB floor.
    /// The function must not panic.
    #[test]
    fn draw_response_zero_kernel_no_panic() {
        let mut pm = Pixmap::new(400, 150).unwrap();
        let mut tr = make_text_renderer();
        let zero_mags = vec![0.0_f32; 2049];
        draw_response(
            &mut pm,
            &mut tr,
            (0.0, 0.0, 400.0, 150.0),
            &zero_mags,
            &[],
            1.0,
        );
        // Just verifying no panic.
    }

    /// With non-trivial input spectrum the shadow fill must not panic.
    #[test]
    fn draw_response_with_input_shadow_no_panic() {
        use crate::kernel;
        use tiny_skia_widgets::mseg::MsegData;

        let w = 880_u32;
        let h = 200_u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        let mut tr = make_text_renderer();

        let k = kernel::bake(&MsegData::default(), 512);
        // Fake input spectrum: all bins at 0.5 magnitude.
        let input_mags = vec![0.5_f32; 1025];

        draw_response(
            &mut pm,
            &mut tr,
            (0.0, 0.0, w as f32, h as f32),
            &k.mags,
            &input_mags,
            1.0,
        );

        // Verify a pixel is painted.
        let sample_x = w / 2;
        let sample_y = h / 2;
        let pixel = pm.pixels()[(sample_y * w + sample_x) as usize];
        assert!(pixel.alpha() > 0, "pixel at center should be painted");
    }

    /// Silent input (all bins at or below mag_floor) must NOT draw the shadow
    /// fill — just verify no panic.
    #[test]
    fn draw_response_silent_input_no_shadow_no_panic() {
        use crate::kernel;
        use tiny_skia_widgets::mseg::MsegData;

        let mut pm = Pixmap::new(400, 150).unwrap();
        let mut tr = make_text_renderer();

        let k = kernel::bake(&MsegData::default(), 256);
        // All bins at exactly the floor — should skip the shadow draw.
        let floor = 10f32.powf(-48.0 / 20.0);
        let silent_mags = vec![floor * 0.5; 1025];

        draw_response(
            &mut pm,
            &mut tr,
            (0.0, 0.0, 400.0, 150.0),
            &k.mags,
            &silent_mags,
            1.0,
        );
        // Just verifying no panic.
    }

    /// Empty rect (zero width) must not panic (padding guard).
    #[test]
    fn draw_response_tiny_rect_no_panic() {
        let mut pm = Pixmap::new(100, 100).unwrap();
        let mut tr = make_text_renderer();
        draw_response(&mut pm, &mut tr, (0.0, 0.0, 5.0, 5.0), &[], &[], 1.0);
    }
}
