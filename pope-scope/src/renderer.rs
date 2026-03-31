//! Waveform rendering: amplitude mapping, grid, waveform paths, display modes.

use crate::theme;

/// Map a sample value to a Y pixel coordinate using dB scaling.
///
/// - `sample`: audio sample value (typically -1.0 to 1.0)
/// - `min_db`: bottom of visible dB range (e.g. -48.0)
/// - `max_db`: top of visible dB range (e.g. 0.0)
/// - `centre_y`: pixel Y coordinate of the center line (silence)
/// - `half_height`: half the available height in pixels
pub fn sample_to_y(
    sample: f32,
    min_db: f32,
    max_db: f32,
    centre_y: f32,
    half_height: f32,
) -> f32 {
    let sign = if sample >= 0.0 { 1.0 } else { -1.0 };
    let abs_amp = sample.abs().clamp(0.0, 2.0); // reject spikes
    let db = if abs_amp > 0.0 {
        20.0 * abs_amp.log10()
    } else {
        -96.0
    };
    let db_range = max_db - min_db;
    if db_range.abs() < 0.001 {
        return centre_y;
    }
    let normalized = ((db - min_db) / db_range).clamp(0.0, 1.0);
    centre_y - (normalized * half_height * sign)
}

/// Compute dB grid division size for the given dB range.
/// Targets 4-8 grid lines.
pub fn db_grid_division(min_db: f32, max_db: f32) -> f32 {
    let range = max_db - min_db;
    if range > 36.0 {
        12.0
    } else if range > 18.0 {
        6.0
    } else if range > 9.0 {
        3.0
    } else {
        2.0
    }
}

/// Compute time grid divisions for free mode.
/// Returns (division_ms, num_divisions).
pub fn time_grid_divisions(timebase_ms: f32) -> (f32, usize) {
    let targets = [
        1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
    ];
    for &div in &targets {
        let n = (timebase_ms / div).floor() as usize;
        if (4..=10).contains(&n) {
            return (div, n);
        }
    }
    // Fallback
    let div = timebase_ms / 5.0;
    (div, 5)
}

/// Draw amplitude grid lines on a pixmap.
/// Draws horizontal lines at dB divisions within the given area.
/// If `track_color` is provided, grid lines and labels use dimmed
/// versions of that color instead of the default amber theme.
#[allow(clippy::too_many_arguments)]
pub fn draw_amplitude_grid(
    pixmap: &mut tiny_skia::Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    min_db: f32,
    max_db: f32,
    text_renderer: &mut tiny_skia_widgets::TextRenderer,
    scale: f32,
    track_color: Option<u32>,
) {
    let division = db_grid_division(min_db, max_db);
    let centre_y = y + h / 2.0;
    let half_height = h / 2.0;
    let font_size = 8.0 * scale;

    let grid_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.25),
        None => theme::to_color(theme::GRID),
    };
    let grid_bright_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.4),
        None => theme::to_color(theme::GRID_BRIGHT),
    };
    let label_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.6),
        None => theme::to_color(theme::PRIMARY_DIM),
    };

    // Center line (silence)
    tiny_skia_widgets::draw_rect(
        pixmap,
        x,
        centre_y - 0.5,
        w,
        1.0,
        grid_bright_c,
    );

    // dB grid lines above and below center
    let db_range = max_db - min_db;
    let mut db = division;
    while db < db_range {
        let normalized = db / db_range;
        let offset = normalized * half_height;

        // Above center
        let y_above = centre_y - offset;
        if y_above > y {
            tiny_skia_widgets::draw_rect(
                pixmap,
                x,
                y_above - 0.5,
                w,
                1.0,
                grid_c,
            );
            // dB label on right
            let label = format!("{}", (min_db + db) as i32);
            text_renderer.draw_text(
                pixmap,
                x + w - 30.0 * scale,
                y_above - font_size / 2.0,
                &label,
                font_size,
                label_c,
            );
        }

        // Below center (mirror)
        let y_below = centre_y + offset;
        if y_below < y + h {
            tiny_skia_widgets::draw_rect(
                pixmap,
                x,
                y_below - 0.5,
                w,
                1.0,
                grid_c,
            );
        }

        db += division;
    }
}

/// Decimate samples to pixel columns, computing min/max per column.
/// Writes into caller-provided `mins` and `maxs` slices (must be same length = num_columns).
pub fn decimate_to_columns_into(samples: &[f32], mins: &mut [f32], maxs: &mut [f32]) {
    let num_columns = mins.len();
    debug_assert_eq!(mins.len(), maxs.len());
    if samples.is_empty() || num_columns == 0 {
        mins.fill(0.0);
        maxs.fill(0.0);
        return;
    }
    mins.fill(f32::MAX);
    maxs.fill(f32::MIN);
    let samples_per_col = samples.len() as f32 / num_columns as f32;

    for (i, &s) in samples.iter().enumerate() {
        let col = ((i as f32 / samples_per_col) as usize).min(num_columns - 1);
        mins[col] = mins[col].min(s);
        maxs[col] = maxs[col].max(s);
    }

    // Fill columns that got no samples
    for i in 0..num_columns {
        if mins[i] == f32::MAX {
            mins[i] = 0.0;
            maxs[i] = 0.0;
        }
    }
}

/// Decimate samples to pixel columns, computing min/max per column.
/// Returns (min_values, max_values) arrays of length `num_columns`.
pub fn decimate_to_columns(samples: &[f32], num_columns: usize) -> (Vec<f32>, Vec<f32>) {
    let mut mins = vec![0.0f32; num_columns];
    let mut maxs = vec![0.0f32; num_columns];
    decimate_to_columns_into(samples, &mut mins, &mut maxs);
    (mins, maxs)
}

/// Draw a waveform as a connected line stroke using tiny-skia paths.
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
pub fn draw_waveform_line(
    pixmap: &mut tiny_skia::Pixmap,
    samples: &[f32],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    min_db: f32,
    max_db: f32,
    color: tiny_skia::Color,
) {
    if samples.is_empty() || w < 2.0 {
        return;
    }

    let centre_y = y + h / 2.0;
    let half_height = h / 2.0;
    let num_cols = w.floor() as usize;

    let mut pb = tiny_skia::PathBuilder::new();

    if samples.len() <= num_cols {
        // Fewer samples than pixels: connect each sample with a line
        let step = w / samples.len().max(1) as f32;
        let py0 = sample_to_y(samples[0], min_db, max_db, centre_y, half_height);
        pb.move_to(x, py0);
        for (i, &s) in samples.iter().enumerate().skip(1) {
            let px = x + i as f32 * step;
            let py = sample_to_y(s, min_db, max_db, centre_y, half_height);
            pb.line_to(px, py);
        }
    } else {
        // More samples than pixels: draw two connected envelope paths
        // (max envelope and min envelope) with consistent stroke width.
        let (mins, maxs) = decimate_to_columns(samples, num_cols);

        // Max envelope path (positive peaks)
        let y0_top = sample_to_y(maxs[0], min_db, max_db, centre_y, half_height);
        pb.move_to(x, y0_top);
        for i in 1..num_cols {
            let px = x + i as f32;
            let y_top = sample_to_y(maxs[i], min_db, max_db, centre_y, half_height);
            pb.line_to(px, y_top);
        }

        // Min envelope path (negative peaks)
        let mut pb2 = tiny_skia::PathBuilder::new();
        let y0_bot = sample_to_y(mins[0], min_db, max_db, centre_y, half_height);
        pb2.move_to(x, y0_bot);
        for i in 1..num_cols {
            let px = x + i as f32;
            let y_bot = sample_to_y(mins[i], min_db, max_db, centre_y, half_height);
            pb2.line_to(px, y_bot);
        }

        let mut paint = tiny_skia::Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        if let Some(path2) = pb2.finish() {
            pixmap.stroke_path(&path2, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
        // Also draw vertical segments connecting the two envelopes at each column
        // to fill the gap between min and max.
        for i in 0..num_cols {
            let px = x + i as f32;
            let y_top = sample_to_y(maxs[i], min_db, max_db, centre_y, half_height);
            let y_bot = sample_to_y(mins[i], min_db, max_db, centre_y, half_height);
            let seg_h = (y_bot - y_top).max(0.0);
            if seg_h > 1.0 {
                tiny_skia_widgets::draw_rect(pixmap, px, y_top, 1.0, seg_h, color);
            }
        }
    }

    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(
            &path,
            &paint,
            &stroke,
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

/// Draw a waveform as a filled region from center line using tiny-skia paths.
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
pub fn draw_waveform_filled(
    pixmap: &mut tiny_skia::Pixmap,
    samples: &[f32],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    min_db: f32,
    max_db: f32,
    color: tiny_skia::Color,
    alpha: f32,
) {
    let fill_color = tiny_skia::Color::from_rgba(
        color.red(),
        color.green(),
        color.blue(),
        alpha,
    )
    .unwrap_or(color);

    if samples.is_empty() || w < 2.0 {
        return;
    }

    let centre_y = y + h / 2.0;
    let half_height = h / 2.0;
    let num_cols = w.floor() as usize;

    let mut pb = tiny_skia::PathBuilder::new();

    if samples.len() <= num_cols {
        // Forward pass: sample values
        let step = w / samples.len().max(1) as f32;
        pb.move_to(x, centre_y);
        for (i, &s) in samples.iter().enumerate() {
            let px = x + i as f32 * step;
            let py = sample_to_y(s, min_db, max_db, centre_y, half_height);
            pb.line_to(px, py);
        }
        // Close back to center line
        pb.line_to(x + (samples.len() - 1) as f32 * step, centre_y);
        pb.close();
    } else {
        // Envelope fill: max values forward, min values backward
        let (mins, maxs) = decimate_to_columns(samples, num_cols);
        // Forward along max envelope (above center = peaks)
        pb.move_to(x, centre_y);
        for i in 0..num_cols {
            let px = x + i as f32;
            let y_top = sample_to_y(maxs[i], min_db, max_db, centre_y, half_height);
            pb.line_to(px, y_top);
        }
        // Connect to center at far end
        pb.line_to(x + (num_cols - 1) as f32, centre_y);
        pb.close();

        // Second path for negative half (below center)
        let mut pb2 = tiny_skia::PathBuilder::new();
        pb2.move_to(x, centre_y);
        for i in 0..num_cols {
            let px = x + i as f32;
            let y_bot = sample_to_y(mins[i], min_db, max_db, centre_y, half_height);
            pb2.line_to(px, y_bot);
        }
        pb2.line_to(x + (num_cols - 1) as f32, centre_y);
        pb2.close();

        let mut paint = tiny_skia::Paint::default();
        paint.set_color(fill_color);
        paint.anti_alias = true;
        if let Some(path2) = pb2.finish() {
            pixmap.fill_path(
                &path2,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }

    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(fill_color);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

/// Draw a waveform using the specified draw style.
#[allow(clippy::too_many_arguments)]
pub fn draw_waveform(
    pixmap: &mut tiny_skia::Pixmap,
    samples: &[f32],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    min_db: f32,
    max_db: f32,
    color: u32,
    draw_style: crate::DrawStyle,
) {
    let c = theme::to_color(color);
    match draw_style {
        crate::DrawStyle::Line => {
            draw_waveform_line(pixmap, samples, x, y, w, h, min_db, max_db, c);
        }
        crate::DrawStyle::Filled => {
            draw_waveform_filled(pixmap, samples, x, y, w, h, min_db, max_db, c, 0.75);
        }
        crate::DrawStyle::Both => {
            draw_waveform_filled(pixmap, samples, x, y, w, h, min_db, max_db, c, 0.3);
            draw_waveform_line(pixmap, samples, x, y, w, h, min_db, max_db, c);
        }
    }
}

/// Draw time grid lines for free mode.
#[allow(clippy::too_many_arguments)]
pub fn draw_time_grid(
    pixmap: &mut tiny_skia::Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    timebase_ms: f32,
    text_renderer: &mut tiny_skia_widgets::TextRenderer,
    scale: f32,
    show_labels: bool,
) {
    let (div_ms, n) = time_grid_divisions(timebase_ms);
    let font_size = 8.0 * scale;

    for i in 1..=n {
        let frac = (div_ms * i as f32) / timebase_ms;
        let px = x + frac * w;
        if px < x + w {
            tiny_skia_widgets::draw_rect(
                pixmap,
                px - 0.5,
                y,
                1.0,
                h,
                theme::to_color(theme::GRID),
            );
            if show_labels {
                let label = if div_ms * i as f32 >= 1000.0 {
                    format!("{:.1}s", div_ms * i as f32 / 1000.0)
                } else {
                    format!("{}ms", (div_ms * i as f32) as i32)
                };
                text_renderer.draw_text(
                    pixmap,
                    px + 2.0,
                    y + h - 2.0,
                    &label,
                    font_size,
                    theme::to_color(theme::GRID),
                );
            }
        }
    }
}

/// Draw beat grid lines for beat sync mode.
/// Draws three levels: bar lines (brightest, 2px), beat lines (medium, 1px),
/// and quarter-beat subdivision lines (dimmest, 1px).
/// If `track_color` is provided, grid lines and labels use alpha-varied
/// versions of that color instead of the default amber theme.
#[allow(clippy::too_many_arguments)]
pub fn draw_beat_grid(
    pixmap: &mut tiny_skia::Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    beats_per_bar: u32,
    total_beats: f64,
    text_renderer: &mut tiny_skia_widgets::TextRenderer,
    scale: f32,
    show_labels: bool,
    track_color: Option<u32>,
) {
    if total_beats <= 0.0 {
        return;
    }
    let font_size = 8.0 * scale;

    let bar_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.5),
        None => theme::to_color(theme::BAR_LINE),
    };
    let beat_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.3),
        None => theme::to_color(theme::GRID),
    };
    let subdiv_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.15),
        None => theme::to_color(theme::GRID_SUBDIV),
    };
    let label_c = match track_color {
        Some(c) => theme::to_color_alpha(c, 0.6),
        None => theme::to_color(theme::BAR_LINE),
    };

    // Iterate at quarter-beat intervals
    let num_quarters = (total_beats * 4.0).ceil() as usize;

    for i in 0..=num_quarters {
        let frac = i as f32 / (total_beats as f32 * 4.0);
        let px = x + frac * w;
        if px <= x || px >= x + w {
            continue;
        }
        let is_beat = i % 4 == 0;
        let is_bar = is_beat && ((i / 4) as u32).is_multiple_of(beats_per_bar);
        let (color, line_w) = if is_bar {
            (bar_c, 2.0)
        } else if is_beat {
            (beat_c, 1.0)
        } else {
            (subdiv_c, 1.0)
        };
        tiny_skia_widgets::draw_rect(pixmap, px - line_w / 2.0, y, line_w, h, color);

        if show_labels && is_bar {
            let bar = (i / 4) as u32 / beats_per_bar + 1;
            let label = format!("{}", bar);
            text_renderer.draw_text(
                pixmap,
                px + 2.0,
                y + h - 2.0,
                &label,
                font_size,
                label_c,
            );
        }
    }
}

/// Draw a cursor line at the given X position with track info tooltip.
pub fn draw_cursor(
    pixmap: &mut tiny_skia::Pixmap,
    cursor_x: f32,
    y: f32,
    h: f32,
) {
    tiny_skia_widgets::draw_rect(
        pixmap,
        cursor_x - 0.5,
        y,
        1.0,
        h,
        theme::to_color(theme::CYAN),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_to_y_silence() {
        // 0.0 amplitude -> should map to centre
        let y = sample_to_y(0.0, -48.0, 0.0, 100.0, 50.0);
        assert_eq!(y, 100.0);
    }

    #[test]
    fn test_sample_to_y_full_scale() {
        // 1.0 amplitude (0 dB) -> should map to top
        let y = sample_to_y(1.0, -48.0, 0.0, 100.0, 50.0);
        assert!((y - 50.0).abs() < 0.1); // centre - half_height
    }

    #[test]
    fn test_sample_to_y_negative() {
        // -1.0 amplitude -> should map to bottom
        let y = sample_to_y(-1.0, -48.0, 0.0, 100.0, 50.0);
        assert!((y - 150.0).abs() < 0.1); // centre + half_height
    }

    #[test]
    fn test_sample_to_y_half_db() {
        // -24 dB is halfway in the -48 to 0 range
        let amp = 10.0f32.powf(-24.0 / 20.0); // ~0.063
        let y = sample_to_y(amp, -48.0, 0.0, 100.0, 50.0);
        assert!((y - 75.0).abs() < 1.0); // centre - half_height * 0.5
    }

    #[test]
    fn test_sample_to_y_spike_clamped() {
        // Values > 2.0 should be clamped
        let y = sample_to_y(10.0, -48.0, 0.0, 100.0, 50.0);
        let y_clamped = sample_to_y(2.0, -48.0, 0.0, 100.0, 50.0);
        assert_eq!(y, y_clamped);
    }

    #[test]
    fn test_db_grid_division() {
        assert_eq!(db_grid_division(-48.0, 0.0), 12.0); // 48 dB range
        assert_eq!(db_grid_division(-24.0, 0.0), 6.0); // 24 dB range
        assert_eq!(db_grid_division(-12.0, 0.0), 3.0); // 12 dB range
        assert_eq!(db_grid_division(-6.0, 0.0), 2.0); // 6 dB range
    }

    #[test]
    fn test_time_grid_divisions() {
        let (div, n) = time_grid_divisions(1000.0);
        assert!(n >= 4 && n <= 10);
        assert!((div * n as f32 - 1000.0).abs() < div);
    }

    #[test]
    fn test_time_grid_divisions_small() {
        let (_div, n) = time_grid_divisions(10.0);
        assert!(n >= 4 && n <= 10);
    }

    #[test]
    fn test_time_grid_divisions_large() {
        let (_div, n) = time_grid_divisions(10000.0);
        assert!(n >= 4 && n <= 10);
    }

    // ── decimate_to_columns tests ────────────────────────────────────────────

    #[test]
    fn test_decimate_empty() {
        let (mins, maxs) = decimate_to_columns(&[], 4);
        assert_eq!(mins.len(), 4);
        assert_eq!(maxs.len(), 4);
        assert!(mins.iter().all(|&v| v == 0.0));
        assert!(maxs.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_decimate_zero_columns() {
        let (mins, maxs) = decimate_to_columns(&[1.0, 2.0], 0);
        assert!(mins.is_empty());
        assert!(maxs.is_empty());
    }

    #[test]
    fn test_decimate_identity() {
        // 4 samples into 4 columns -> one sample per column
        let samples = [0.1, -0.3, 0.5, -0.2];
        let (mins, maxs) = decimate_to_columns(&samples, 4);
        assert_eq!(mins.len(), 4);
        assert_eq!(maxs.len(), 4);
        for i in 0..4 {
            assert_eq!(mins[i], samples[i]);
            assert_eq!(maxs[i], samples[i]);
        }
    }

    #[test]
    fn test_decimate_reduction() {
        // 8 samples into 2 columns -> 4 samples per column
        let samples = [0.1, -0.3, 0.5, -0.2, 0.8, -0.1, 0.3, -0.7];
        let (mins, maxs) = decimate_to_columns(&samples, 2);
        assert_eq!(mins.len(), 2);
        // Column 0: [0.1, -0.3, 0.5, -0.2] -> min=-0.3, max=0.5
        assert!((mins[0] - (-0.3)).abs() < 0.001);
        assert!((maxs[0] - 0.5).abs() < 0.001);
        // Column 1: [0.8, -0.1, 0.3, -0.7] -> min=-0.7, max=0.8
        assert!((mins[1] - (-0.7)).abs() < 0.001);
        assert!((maxs[1] - 0.8).abs() < 0.001);
    }

    // ── Draw function smoke tests ────────────────────────────────────────────
    // These verify the functions don't panic and produce non-zero output.

    fn make_test_pixmap(w: u32, h: u32) -> tiny_skia::Pixmap {
        tiny_skia::Pixmap::new(w, h).unwrap()
    }

    fn sine_samples(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 / n as f32 * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    fn pixmap_has_nonzero(pm: &tiny_skia::Pixmap) -> bool {
        pm.data().iter().any(|&b| b != 0)
    }

    #[test]
    fn test_draw_waveform_line_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        let color = tiny_skia::Color::from_rgba8(255, 128, 0, 255);
        draw_waveform_line(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, color);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_line_fewer_samples_than_pixels() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(50); // fewer than 200 pixels
        let color = tiny_skia::Color::from_rgba8(255, 128, 0, 255);
        draw_waveform_line(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, color);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_line_empty() {
        let mut pm = make_test_pixmap(200, 100);
        let color = tiny_skia::Color::from_rgba8(255, 128, 0, 255);
        draw_waveform_line(&mut pm, &[], 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, color);
        // Should not panic and should not draw anything
        assert!(!pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_filled_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        let color = tiny_skia::Color::from_rgba8(255, 128, 0, 255);
        draw_waveform_filled(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, color, 0.75);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_dispatch_line() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        draw_waveform(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0,
            theme::CYAN, crate::DrawStyle::Line);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_dispatch_filled() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        draw_waveform(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0,
            theme::CYAN, crate::DrawStyle::Filled);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_dispatch_both() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        draw_waveform(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0,
            theme::CYAN, crate::DrawStyle::Both);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_time_grid_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_time_grid(&mut pm, 0.0, 0.0, 200.0, 100.0, 1000.0, &mut tr, 1.0, true);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_time_grid_no_labels() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_time_grid(&mut pm, 0.0, 0.0, 200.0, 100.0, 1000.0, &mut tr, 1.0, false);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_beat_grid_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_beat_grid(&mut pm, 0.0, 0.0, 200.0, 100.0, 4, 4.0, &mut tr, 1.0, true, None);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_beat_grid_zero_beats() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_beat_grid(&mut pm, 0.0, 0.0, 200.0, 100.0, 4, 0.0, &mut tr, 1.0, true, None);
        // Should not panic and should not draw
        assert!(!pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_amplitude_grid_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_amplitude_grid(&mut pm, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, &mut tr, 1.0, None);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_cursor_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        draw_cursor(&mut pm, 100.0, 0.0, 100.0);
        assert!(pixmap_has_nonzero(&pm));
    }
}
