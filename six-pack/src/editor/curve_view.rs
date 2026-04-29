//! EQ curve view: log-frequency axis, dB amplitude axis, faded spectrum
//! overlay, composite EQ curve, 6 draggable band dots.
//!
//! No allocations on hot redraw paths.

use crate::bands::FilterShape;
use crate::editor::{band_color, band_color_alpha, HitAction, SixPackWindow};
use crate::spectrum::N_BINS;
use crate::BAND_SHAPES;
use tiny_skia_widgets as widgets;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20_000.0;
const DB_MIN: f32 = -3.0;
const DB_MAX: f32 = 21.0;

#[inline]
pub(crate) fn norm_x_to_freq(xnorm: f32) -> f32 {
    let log_min = FREQ_MIN.ln();
    let log_max = FREQ_MAX.ln();
    let t = xnorm.clamp(0.0, 1.0);
    (log_min + t * (log_max - log_min)).exp()
}

#[inline]
fn freq_to_norm_x(freq: f32) -> f32 {
    let log_min = FREQ_MIN.ln();
    let log_max = FREQ_MAX.ln();
    let t = (freq.max(FREQ_MIN).ln() - log_min) / (log_max - log_min);
    t.clamp(0.0, 1.0)
}

#[inline]
fn db_to_norm_y(db: f32) -> f32 {
    ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0)
}

/// Analytic peaking-EQ magnitude (RBJ cookbook style) at frequency `f` for a
/// peak filter centered at `f0` with quality `q` and gain `gain_db`.
///
/// At f=f0 the response equals `10^(gain_db/20)` (linear amplitude that
/// corresponds to `gain_db` decibels).
fn peak_magnitude(f: f32, f0: f32, q: f32, gain_db: f32) -> f32 {
    if gain_db.abs() < 1e-3 || f0 <= 0.0 {
        return 1.0;
    }
    let a = 10f32.powf(gain_db / 20.0);
    let r = f / f0;
    let r2 = r * r;
    // Bell shape in log-magnitude: starts at unity at the band edges, rises
    // to `a` at the center. Standard RBJ peaking-EQ magnitude.
    let denom = (1.0 - r2).powi(2) + (r / q).powi(2);
    let num = (1.0 - r2).powi(2) + (a * r / q).powi(2);
    if denom <= 0.0 {
        return 1.0;
    }
    (num / denom).sqrt()
}

/// Analytic low-shelf magnitude: rises by `gain_db` toward DC.
fn low_shelf_magnitude(f: f32, f0: f32, q: f32, gain_db: f32) -> f32 {
    if gain_db.abs() < 1e-3 || f0 <= 0.0 {
        return 1.0;
    }
    let a = 10f32.powf(gain_db / 40.0);
    // Interpolate magnitude smoothly between 1 (above f0) and a^2 (below f0)
    // using a cosine-shaped transition over 1.5 octaves around f0. This is
    // visual-only, not the exact biquad response, but enough for the GUI.
    let oct = (f / f0).max(1e-9).log2();
    let t = smoothstep(-1.0, 0.5 / q.max(0.1), -oct);
    1.0 * (1.0 - t) + a * a * t
}

/// Analytic high-shelf magnitude: rises by `gain_db` toward Nyquist.
fn high_shelf_magnitude(f: f32, f0: f32, q: f32, gain_db: f32) -> f32 {
    if gain_db.abs() < 1e-3 || f0 <= 0.0 {
        return 1.0;
    }
    let a = 10f32.powf(gain_db / 40.0);
    let oct = (f / f0).max(1e-9).log2();
    let t = smoothstep(-0.5 / q.max(0.1), 1.0, oct);
    1.0 * (1.0 - t) + a * a * t
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Compute the composite EQ magnitude (linear) at frequency `f` summing all
/// 6 bands' contributions. Disabled bands contribute 1.0 (no change).
pub(crate) fn composite_magnitude(win: &SixPackWindow, f: f32) -> f32 {
    let mut total = 1.0_f32;
    for (i, shape) in BAND_SHAPES.iter().enumerate() {
        let bp = &win.params.bands[i];
        if !bp.enable.value() {
            continue;
        }
        let f0 = bp.freq.value();
        let q = bp.q.value().max(0.1);
        let g = bp.gain.value();
        let m = match shape {
            FilterShape::Peak => peak_magnitude(f, f0, q, g),
            FilterShape::LowShelf => low_shelf_magnitude(f, f0, q, g),
            FilterShape::HighShelf => high_shelf_magnitude(f, f0, q, g),
        };
        total *= m;
    }
    total
}

/// Map a spectrum atomic-bin index to its center frequency given the analyzer's
/// log-spaced layout.
#[inline]
fn bin_center_freq(bin_idx: usize, sample_rate: f32) -> f32 {
    // The analyzer maps output bin k to FFT bin range
    // [exp(log_min + k/N * log_max), exp(log_min + (k+1)/N * log_max)] where
    // log_max = ln(FFT_SIZE/2), log_min = 0. Use mid-fraction (k+0.5)/N.
    let fft_size = 2048.0_f32;
    let n_freq_bins = fft_size / 2.0;
    let log_max = n_freq_bins.ln();
    let frac = (bin_idx as f32 + 0.5) / N_BINS as f32;
    let bin_freq_idx = (frac * log_max).exp();
    bin_freq_idx * sample_rate / fft_size
}

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

    let pad_l = 28.0 * s; // left axis labels
    let pad_b = 16.0 * s; // bottom axis labels
    let pad_t = 6.0 * s;
    let pad_r = 6.0 * s;
    let plot_x = x + pad_l;
    let plot_y = y + pad_t;
    let plot_w = (w - pad_l - pad_r).max(40.0);
    let plot_h = (h - pad_t - pad_b).max(40.0);

    // ── Grid: vertical decade lines + 3 dB-step horizontal lines ──
    let grid_color = tiny_skia::Color::from_rgba8(0x40, 0x44, 0x50, 0xff);
    for &freq in &[100.0_f32, 1000.0, 10000.0] {
        let xnorm = freq_to_norm_x(freq);
        let gx = plot_x + xnorm * plot_w;
        widgets::draw_rect(&mut win.surface.pixmap, gx, plot_y, 1.0, plot_h, grid_color);
    }
    for &db in &[0.0_f32, 6.0, 12.0, 18.0] {
        let yn = db_to_norm_y(db);
        let gy = plot_y + plot_h - yn * plot_h;
        widgets::draw_rect(&mut win.surface.pixmap, plot_x, gy, plot_w, 1.0, grid_color);
    }

    // ── Spectrum overlay (faded) ────────────────────────────────────────
    // Use a coarse vertical column scan: for each pixel column, look up the
    // spectrum bin closest to that column's center frequency, then fill from
    // that y down.
    let sample_rate = 48_000.0_f32; // analyzer is sample-rate-agnostic; nominal value.
    let n_cols = plot_w as usize;
    if n_cols >= 4 {
        for col in 0..n_cols {
            let xnorm = (col as f32 + 0.5) / plot_w;
            let f = norm_x_to_freq(xnorm);
            // Find best matching bin by binary search.
            let mut best_idx = 0usize;
            let mut best_diff = f32::INFINITY;
            for b in 0..N_BINS {
                let bf = bin_center_freq(b, sample_rate);
                let d = (bf - f).abs() / f.max(1.0);
                if d < best_diff {
                    best_diff = d;
                    best_idx = b;
                }
            }
            let mag = win.display_bins[best_idx];
            // Map magnitude → dB → pixel, with a quiet floor.
            let db = 20.0 * mag.max(1e-4).log10();
            let snorm = ((db + 60.0) / 70.0).clamp(0.0, 1.0); // -60..+10 dB
            let bar_h = snorm * plot_h * 0.85;
            if bar_h > 0.5 {
                let bar_y = plot_y + plot_h - bar_h;
                let cx = plot_x + col as f32 + 0.5;
                widgets::fill_column_opaque(
                    &mut win.surface.pixmap,
                    cx,
                    bar_y,
                    plot_y + plot_h,
                    tiny_skia::Color::from_rgba8(0x60, 0x70, 0x90, 0xff),
                );
            }
        }

        // ── "Harmonics added" overlay (wet spectrum) ───────────────────
        // Same column-fill pass but reading display_wet_bins and drawn in a
        // bright warm accent so it pops over the cool input fill. The wet
        // path is what Six Pack contributes on top of dry — at low drive
        // it'll be subtle, at high drive it lights up the bands clearly.
        let wet_color = tiny_skia::Color::from_rgba8(0xff, 0xb0, 0x40, 0xff);
        for col in 0..n_cols {
            let xnorm = (col as f32 + 0.5) / plot_w;
            let f = norm_x_to_freq(xnorm);
            let mut best_idx = 0usize;
            let mut best_diff = f32::INFINITY;
            for b in 0..N_BINS {
                let bf = bin_center_freq(b, sample_rate);
                let d = (bf - f).abs() / f.max(1.0);
                if d < best_diff {
                    best_diff = d;
                    best_idx = b;
                }
            }
            let mag = win.display_wet_bins[best_idx];
            let db = 20.0 * mag.max(1e-4).log10();
            let snorm = ((db + 60.0) / 70.0).clamp(0.0, 1.0);
            let bar_h = snorm * plot_h * 0.85;
            if bar_h > 0.5 {
                let bar_y = plot_y + plot_h - bar_h;
                let cx = plot_x + col as f32 + 0.5;
                widgets::fill_column_opaque(
                    &mut win.surface.pixmap,
                    cx,
                    bar_y,
                    plot_y + plot_h,
                    wet_color,
                );
            }
        }
    }

    // ── Composite EQ curve ─────────────────────────────────────────────
    // Plot 1 sample per pixel column, drawing as a thin polyline.
    let n_curve = (plot_w as usize).max(2);
    let mut prev_y = plot_y + plot_h - db_to_norm_y(0.0) * plot_h;
    let curve_color = tiny_skia::Color::from_rgba8(0xc0, 0xe0, 0xff, 0xff);
    for col in 0..=n_curve {
        let xnorm = col as f32 / n_curve as f32;
        let f = norm_x_to_freq(xnorm);
        let mag = composite_magnitude(win, f);
        let db = 20.0 * mag.max(1e-6).log10();
        let yn = db_to_norm_y(db);
        let yy = plot_y + plot_h - yn * plot_h;
        let xx = plot_x + xnorm * plot_w;
        if col > 0 {
            // Step / line approximation: fill a 1px column from min(prev,yy)
            // to max(prev,yy)+1 to make the curve visible without paths.
            let y_lo = prev_y.min(yy);
            let y_hi = prev_y.max(yy);
            widgets::fill_column_opaque(&mut win.surface.pixmap, xx, y_lo, y_hi + 1.0, curve_color);
        }
        prev_y = yy;
    }

    // ── Axis labels ────────────────────────────────────────────────────
    let label_color = widgets::color_muted();
    let label_size = (10.0 * s).max(9.0);
    let tr = &mut win.text_renderer;
    for &(freq, label) in &[(100.0_f32, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let xnorm = freq_to_norm_x(freq);
        let gx = plot_x + xnorm * plot_w;
        let lw = tr.text_width(label, label_size);
        tr.draw_text(
            &mut win.surface.pixmap,
            gx - lw * 0.5,
            y + h - 4.0 * s,
            label,
            label_size,
            label_color,
        );
    }
    for &(db, label) in &[(0.0_f32, "0"), (6.0, "+6"), (12.0, "+12"), (18.0, "+18")] {
        let yn = db_to_norm_y(db);
        let gy = plot_y + plot_h - yn * plot_h;
        tr.draw_text(
            &mut win.surface.pixmap,
            x + 3.0 * s,
            gy + label_size * 0.4,
            label,
            label_size,
            label_color,
        );
    }

    // ── Band dots ──────────────────────────────────────────────────────
    let dot_radius = 7.0 * s;
    for i in 0..6 {
        let bp = &win.params.bands[i];
        let f = bp.freq.value();
        let g = bp.gain.value();
        let xnorm = freq_to_norm_x(f);
        let yn = db_to_norm_y(g);
        let cx = plot_x + xnorm * plot_w;
        let cy = plot_y + plot_h - yn * plot_h;

        // Activity glow (0..1). The audio thread publishes per-band post-
        // saturation RMS in `band_activity_bins`; the editor EMAs it into
        // `display_band_activity`. RMS values are typically small, so we
        // multiply before clamping to make the glow visible at musical
        // levels. Disabled bands don't glow even if state is non-zero from
        // a recent toggle.
        let glow = if bp.enable.value() {
            (win.display_band_activity[i] * 8.0).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Outer halo — translucent ring fading outward, sized by glow.
        if glow > 0.01 {
            let halo_r = dot_radius * (1.5 + 1.4 * glow);
            let halo_alpha = (90.0 * glow) as u8;
            draw_dot(
                &mut win.surface.pixmap,
                cx,
                cy,
                halo_r,
                band_color_alpha(i, halo_alpha),
            );
        }

        let fill = if bp.enable.value() {
            band_color(i)
        } else {
            band_color_alpha(i, 80)
        };
        let hi = band_color_alpha(i, 200);
        // Slightly inflate the dot itself with activity so the visual cue
        // is unmistakable even on a dim halo.
        let active_radius = dot_radius * (1.0 + 0.25 * glow);
        // Filled circle approximated by stacked rect rows for speed.
        draw_dot(&mut win.surface.pixmap, cx, cy, active_radius, fill);
        // Outline.
        draw_dot_outline(&mut win.surface.pixmap, cx, cy, active_radius, hi);

        let hit_r = dot_radius * 1.7;
        win.drag.push_region(
            cx - hit_r,
            cy - hit_r,
            hit_r * 2.0,
            hit_r * 2.0,
            HitAction::BandDot(i),
        );

        // Small numeric label inside the dot.
        let lbl_size = (8.0 * s).max(7.0);
        let label = format!("{}", i + 1);
        let lw = win.text_renderer.text_width(&label, lbl_size);
        win.text_renderer.draw_text(
            &mut win.surface.pixmap,
            cx - lw * 0.5,
            cy + lbl_size * 0.4,
            &label,
            lbl_size,
            tiny_skia::Color::from_rgba8(0x10, 0x10, 0x12, 0xff),
        );
    }

    // ── Cursor tooltip ──────────────────────────────────────────────────
    // When the mouse is inside the plot rect, draw a vertical cursor line
    // and a small readout next to it: frequency at the cursor + the
    // current dry-spectrum and wet-spectrum levels at that frequency.
    // Skipped when the cursor isn't in the window (so the tooltip doesn't
    // latch a phantom hover at the last known position) or while a drag
    // is active (keeps the readout from fighting the drag UI).
    if win.drag.mouse_in_window() && win.drag.active_action().is_none() {
        let (mx, my) = win.drag.mouse_pos();
        if mx >= plot_x && mx < plot_x + plot_w && my >= plot_y && my < plot_y + plot_h {
            draw_cursor_tooltip(win, mx, my, plot_x, plot_y, plot_w, plot_h, s);
        }
    }
}

/// Draws the vertical cursor line + readout box when the mouse is over
/// the plot area. Pulls dry/wet magnitudes from the same display bins
/// the spectrum overlay uses, so the readings line up with what's drawn.
#[allow(clippy::too_many_arguments)]
fn draw_cursor_tooltip(
    win: &mut SixPackWindow,
    mx: f32,
    my: f32,
    plot_x: f32,
    plot_y: f32,
    plot_w: f32,
    plot_h: f32,
    s: f32,
) {
    // Frequency at cursor.
    let xnorm = ((mx - plot_x) / plot_w).clamp(0.0, 1.0);
    let freq = norm_x_to_freq(xnorm);

    // Find the spectrum bin nearest the cursor frequency. The analyzer is
    // sample-rate-agnostic at the bin level; nominal 48 kHz keeps the
    // mapping consistent with the overlay rendering.
    let sample_rate = 48_000.0_f32;
    let mut best_idx = 0usize;
    let mut best_diff = f32::INFINITY;
    for b in 0..N_BINS {
        let bf = bin_center_freq(b, sample_rate);
        let d = (bf - freq).abs() / freq.max(1.0);
        if d < best_diff {
            best_diff = d;
            best_idx = b;
        }
    }
    let dry_mag = win.display_bins[best_idx];
    let wet_mag = win.display_wet_bins[best_idx];

    // Vertical cursor line through the plot at the mouse x.
    widgets::draw_rect(
        &mut win.surface.pixmap,
        mx,
        plot_y,
        1.0,
        plot_h,
        tiny_skia::Color::from_rgba8(0xc0, 0xe0, 0xff, 0x80),
    );

    // Three lines: freq / dry / wet.
    let freq_text = if freq >= 1000.0 {
        format!("{:.2} kHz", freq / 1000.0)
    } else {
        format!("{:.0} Hz", freq)
    };
    let fmt_db = |mag: f32| -> String {
        let db = 20.0 * mag.max(1e-6).log10();
        if db < -90.0 {
            String::from("—")
        } else {
            format!("{:+.1} dB", db)
        }
    };
    let dry_text = format!("Dry: {}", fmt_db(dry_mag));
    let wet_text = format!("Wet: {}", fmt_db(wet_mag));

    // Tooltip box geometry. Width is the longest text line plus padding.
    let font_size = (10.0 * s).max(9.0);
    let line_h = font_size + 4.0 * s;
    let pad = 6.0 * s;
    let tr = &mut win.text_renderer;
    let tw = tr
        .text_width(&freq_text, font_size)
        .max(tr.text_width(&dry_text, font_size))
        .max(tr.text_width(&wet_text, font_size));
    let tooltip_w = tw + pad * 2.0;
    let tooltip_h = pad * 2.0 + line_h * 3.0;

    // Position next to the cursor; flip to the left if it would overflow,
    // and clamp inside the plot rect vertically.
    let offset = 12.0 * s;
    let mut tx = mx + offset;
    if tx + tooltip_w > plot_x + plot_w {
        tx = mx - tooltip_w - offset;
    }
    if tx < plot_x {
        tx = plot_x;
    }
    let mut ty = my - tooltip_h / 2.0;
    if ty < plot_y {
        ty = plot_y;
    }
    if ty + tooltip_h > plot_y + plot_h {
        ty = (plot_y + plot_h - tooltip_h).max(plot_y);
    }

    // Background + border.
    widgets::draw_rect(
        &mut win.surface.pixmap,
        tx,
        ty,
        tooltip_w,
        tooltip_h,
        tiny_skia::Color::from_rgba8(0x10, 0x14, 0x1c, 0xf2),
    );
    widgets::draw_rect_outline(
        &mut win.surface.pixmap,
        tx,
        ty,
        tooltip_w,
        tooltip_h,
        tiny_skia::Color::from_rgba8(0xc0, 0xe0, 0xff, 0xff),
        1.0,
    );

    // Three lines, top to bottom.
    let mut y = ty + pad + font_size;
    tr.draw_text(
        &mut win.surface.pixmap,
        tx + pad,
        y,
        &freq_text,
        font_size,
        tiny_skia::Color::from_rgba8(0xc0, 0xe0, 0xff, 0xff),
    );
    y += line_h;
    tr.draw_text(
        &mut win.surface.pixmap,
        tx + pad,
        y,
        &dry_text,
        font_size,
        tiny_skia::Color::from_rgba8(0x90, 0xa8, 0xc8, 0xff),
    );
    y += line_h;
    tr.draw_text(
        &mut win.surface.pixmap,
        tx + pad,
        y,
        &wet_text,
        font_size,
        tiny_skia::Color::from_rgba8(0xff, 0xb0, 0x40, 0xff),
    );
}

fn draw_dot(pixmap: &mut tiny_skia::Pixmap, cx: f32, cy: f32, r: f32, color: tiny_skia::Color) {
    // Filled disk via stacked rows. r is small (~7 px) so this is cheap.
    let r_i = r.ceil() as i32;
    let cx_i = cx.round() as i32;
    let cy_i = cy.round() as i32;
    for dy in -r_i..=r_i {
        let row_h = (r * r - (dy as f32) * (dy as f32)).max(0.0).sqrt();
        let half = row_h.round() as i32;
        if half <= 0 {
            continue;
        }
        let x0 = (cx_i - half) as f32;
        let y0 = (cy_i + dy) as f32;
        widgets::draw_rect(pixmap, x0, y0, (2 * half) as f32, 1.0, color);
    }
}

fn draw_dot_outline(
    pixmap: &mut tiny_skia::Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    color: tiny_skia::Color,
) {
    // Simple outline: 1-pixel-wide ring sampled along the circle.
    let n = (r * 6.0).ceil() as i32;
    for k in 0..n {
        let theta = k as f32 * std::f32::consts::TAU / n as f32;
        let x = cx + r * theta.cos();
        let y = cy + r * theta.sin();
        widgets::draw_rect(pixmap, x, y, 1.5, 1.5, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_x_to_freq_endpoints() {
        let f0 = norm_x_to_freq(0.0);
        let f1 = norm_x_to_freq(1.0);
        assert!((f0 - FREQ_MIN).abs() < 0.1);
        assert!((f1 - FREQ_MAX).abs() < 1.0);
    }

    #[test]
    fn freq_to_norm_x_endpoints() {
        assert!((freq_to_norm_x(20.0) - 0.0).abs() < 0.001);
        assert!((freq_to_norm_x(20000.0) - 1.0).abs() < 0.001);
    }

    #[test]
    fn freq_x_roundtrip_at_log_center() {
        // log midpoint of 20..20000 is sqrt(20*20000) = 632.4...
        let mid = norm_x_to_freq(0.5);
        let back = freq_to_norm_x(mid);
        assert!((back - 0.5).abs() < 1e-3);
    }

    #[test]
    fn db_to_norm_y_endpoints() {
        assert!((db_to_norm_y(DB_MIN) - 0.0).abs() < 1e-3);
        assert!((db_to_norm_y(DB_MAX) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn peak_magnitude_unit_at_zero_db() {
        let m = peak_magnitude(1000.0, 1000.0, 1.0, 0.0);
        assert!((m - 1.0).abs() < 1e-3);
    }

    #[test]
    fn peak_magnitude_centered_gain() {
        let g = 12.0;
        let m = peak_magnitude(1000.0, 1000.0, 1.0, g);
        let m_db = 20.0 * m.log10();
        // Within ~1 dB of target — analytic formula gives exact peak gain.
        assert!((m_db - g).abs() < 1.0, "got {} dB", m_db);
    }
}
