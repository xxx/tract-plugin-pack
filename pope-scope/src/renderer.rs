//! Waveform rendering: amplitude mapping, grid, waveform paths, display modes.

use crate::theme;
use std::borrow::Cow;

/// Map a sample value to a Y pixel coordinate using dB scaling.
///
/// - `sample`: audio sample value (typically -1.0 to 1.0)
/// - `min_db`: bottom of visible dB range (e.g. -48.0)
/// - `max_db`: top of visible dB range (e.g. 0.0)
/// - `centre_y`: pixel Y coordinate of the center line (silence)
/// - `half_height`: half the available height in pixels
pub fn sample_to_y(sample: f32, min_db: f32, max_db: f32, centre_y: f32, half_height: f32) -> f32 {
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
    tiny_skia_widgets::draw_rect(pixmap, x, centre_y - 0.5, w, 1.0, grid_bright_c);

    // dB grid lines above and below center
    let db_range = max_db - min_db;
    let mut db = division;
    while db < db_range {
        let normalized = db / db_range;
        let offset = normalized * half_height;

        // Above center
        let y_above = centre_y - offset;
        if y_above > y {
            tiny_skia_widgets::draw_rect(pixmap, x, y_above - 0.5, w, 1.0, grid_c);
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
            tiny_skia_widgets::draw_rect(pixmap, x, y_below - 0.5, w, 1.0, grid_c);
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

/// Draw a waveform as a 1-pixel-per-column envelope.
///
/// In the dense-samples path (`samples.len() > num_cols`), every pixel
/// column gets a single vertical `draw_rect` covering the [min, max]
/// range of the samples that map to it. No paths, no anti-aliased
/// stroking — profiling showed that >40% of GUI time was burned in
/// tiny-skia's `stroke_path` / raster pipeline while the scope was
/// playing audio, and replacing it with column rects turns that into
/// a cheap `memset` per column.
///
/// In the sparse-samples path (`samples.len() <= num_cols`, i.e.
/// extreme zoom) the renderer still builds a tiny connected polyline
/// using `stroke_path`, since there are only a handful of segments
/// and the visible line quality matters there.
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

    if samples.len() <= num_cols {
        // Sparse path: few samples, many pixels → keep the connected
        // line stroke so the visible segments look smooth. This branch
        // is not on the hot profile.
        let step = w / samples.len().max(1) as f32;
        let mut pb = tiny_skia::PathBuilder::new();
        let py0 = sample_to_y(samples[0], min_db, max_db, centre_y, half_height);
        pb.move_to(x, py0);
        for (i, &s) in samples.iter().enumerate().skip(1) {
            let px = x + i as f32 * step;
            let py = sample_to_y(s, min_db, max_db, centre_y, half_height);
            pb.line_to(px, py);
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
        return;
    }

    // Dense path: one 1px-wide vertical rect per column, spanning the
    // (min, max) envelope. Each column's effective top/bot is
    // half-split toward its neighbors — specifically, the top is the
    // minimum (highest on screen) of its own top and the midpoints
    // between itself and each neighbor. This is equivalent to
    // rasterizing the envelope polyline such that each line segment
    // between adjacent columns contributes half its vertical span to
    // each column, which smooths the visible contour without
    // flattening valleys or losing isolated peaks the way a symmetric
    // "reach-out-to-neighbor" bridge would.
    //
    // All fills use `fill_column_opaque`, which writes directly into
    // `Pixmap::pixels_mut()` — no tiny-skia raster pipeline.
    let (mins, maxs) = decimate_to_columns(samples, num_cols);
    let col_top = |i: usize| sample_to_y(maxs[i], min_db, max_db, centre_y, half_height);
    let col_bot = |i: usize| sample_to_y(mins[i], min_db, max_db, centre_y, half_height);
    for i in 0..num_cols {
        let own_top = col_top(i);
        let own_bot = col_bot(i);
        let mid_prev_top = if i > 0 {
            (col_top(i - 1) + own_top) * 0.5
        } else {
            own_top
        };
        let mid_prev_bot = if i > 0 {
            (col_bot(i - 1) + own_bot) * 0.5
        } else {
            own_bot
        };
        let mid_next_top = if i + 1 < num_cols {
            (own_top + col_top(i + 1)) * 0.5
        } else {
            own_top
        };
        let mid_next_bot = if i + 1 < num_cols {
            (own_bot + col_bot(i + 1)) * 0.5
        } else {
            own_bot
        };
        let eff_top = own_top.min(mid_prev_top).min(mid_next_top);
        let eff_bot = own_bot.max(mid_prev_bot).max(mid_next_bot);
        let bot = eff_bot.max(eff_top + 1.0);
        tiny_skia_widgets::fill_column_opaque(pixmap, x + i as f32, eff_top, bot, color);
    }
}

/// Draw a waveform as a filled envelope from the center line.
///
/// `color` must be a fully opaque color — callers should pre-mix any
/// desired transparency with the background via `theme::blend_u32`.
/// All dense-path rects are drawn via `draw_rect_opaque` with
/// `BlendMode::Source`, which skips tiny-skia's per-pixel blend loop
/// and is roughly 4× faster than source-over blending for the
/// hundreds of thin columns a scope emits per frame.
///
/// In the sparse-samples path (extreme zoom) the renderer still
/// builds a closed path and fills it via `fill_path`, because at
/// that density the polygon has few edges and the visible fill
/// quality matters.
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
    fill_color: tiny_skia::Color,
) {
    if samples.is_empty() || w < 2.0 {
        return;
    }

    let centre_y = y + h / 2.0;
    let half_height = h / 2.0;
    let num_cols = w.floor() as usize;

    if samples.len() <= num_cols {
        // Sparse path: few samples, many pixels. Keep the closed
        // polygon fill so the visible shape is smooth. Not on the
        // hot profile.
        let step = w / samples.len().max(1) as f32;
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(x, centre_y);
        for (i, &s) in samples.iter().enumerate() {
            let px = x + i as f32 * step;
            let py = sample_to_y(s, min_db, max_db, centre_y, half_height);
            pb.line_to(px, py);
        }
        pb.line_to(x + (samples.len() - 1) as f32 * step, centre_y);
        pb.close();
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
        return;
    }

    // Dense path: per-column vertical rects from the center to the
    // positive and negative envelope peaks, with each column's
    // effective top/bot half-split toward its neighbors. See
    // `draw_waveform_line` for the rationale; this produces a
    // smoother outline than hard column steps while staying within
    // the direct-pixel-write fast path.
    let (mins, maxs) = decimate_to_columns(samples, num_cols);
    let col_top = |i: usize| sample_to_y(maxs[i], min_db, max_db, centre_y, half_height);
    let col_bot = |i: usize| sample_to_y(mins[i], min_db, max_db, centre_y, half_height);
    for i in 0..num_cols {
        let own_top = col_top(i);
        let own_bot = col_bot(i);
        let mid_prev_top = if i > 0 {
            (col_top(i - 1) + own_top) * 0.5
        } else {
            own_top
        };
        let mid_prev_bot = if i > 0 {
            (col_bot(i - 1) + own_bot) * 0.5
        } else {
            own_bot
        };
        let mid_next_top = if i + 1 < num_cols {
            (own_top + col_top(i + 1)) * 0.5
        } else {
            own_top
        };
        let mid_next_bot = if i + 1 < num_cols {
            (own_bot + col_bot(i + 1)) * 0.5
        } else {
            own_bot
        };
        let top = own_top.min(mid_prev_top).min(mid_next_top);
        let bot = own_bot.max(mid_prev_bot).max(mid_next_bot);
        let px = x + i as f32;
        if top < centre_y {
            tiny_skia_widgets::fill_column_opaque(pixmap, px, top, centre_y, fill_color);
        }
        if bot > centre_y {
            tiny_skia_widgets::fill_column_opaque(pixmap, px, centre_y, bot, fill_color);
        }
    }
}

/// Draw a waveform using the specified draw style.
///
/// The filled variants are drawn with a pre-mixed opaque color
/// (`theme::blend_u32(theme::BG, color, alpha)`) rather than a
/// translucent overlay, so the fast `draw_rect_opaque` path can be
/// used. This is visually identical to a translucent blend against
/// the solid background, provided nothing is drawn between the
/// background fill and the waveform that we still want to show
/// through (the grid is under the waveform and gets hidden inside
/// filled columns — that's acceptable for a scope).
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
    let line_c = theme::to_color(color);
    match draw_style {
        crate::DrawStyle::Line => {
            draw_waveform_line(pixmap, samples, x, y, w, h, min_db, max_db, line_c);
        }
        crate::DrawStyle::Filled => {
            let fill_c = theme::to_color(theme::blend_u32(theme::BG, color, 0.75));
            draw_waveform_filled(pixmap, samples, x, y, w, h, min_db, max_db, fill_c);
        }
        crate::DrawStyle::Both => {
            let fill_c = theme::to_color(theme::blend_u32(theme::BG, color, 0.3));
            draw_waveform_filled(pixmap, samples, x, y, w, h, min_db, max_db, fill_c);
            draw_waveform_line(pixmap, samples, x, y, w, h, min_db, max_db, line_c);
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
            tiny_skia_widgets::draw_rect(pixmap, px - 0.5, y, 1.0, h, theme::to_color(theme::GRID));
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
            text_renderer.draw_text(pixmap, px + 2.0, y + h - 2.0, &label, font_size, label_c);
        }
    }
}

/// Draw a vertical cursor line at the given X position.
pub fn draw_cursor(pixmap: &mut tiny_skia::Pixmap, cursor_x: f32, y: f32, h: f32) {
    tiny_skia_widgets::draw_rect(
        pixmap,
        cursor_x - 0.5,
        y,
        1.0,
        h,
        theme::to_color_alpha(theme::CYAN, 0.8),
    );
}

/// Per-track reading for the cursor tooltip.
///
/// `name` is a `Cow` so callers can either borrow a `&str` directly
/// (e.g. from `snap.track_name`) or provide an owned fallback like
/// `"Slot 3"` when a track has no user-visible name.
pub struct CursorReading<'a> {
    pub name: Cow<'a, str>,
    pub color: u32,
    pub db: f32,
}

/// Format a time in milliseconds the same way the original oscilloscope does:
/// `"123 us"` for values below `1 ms`, `"12.3 ms"` otherwise.
/// Matches JUCE's signed `if (timeMs < 1.0f)` branch so negative millisecond
/// values (e.g. "-0.5 ms" relative to a beat anchor) still round to whole
/// microseconds, not to a decimal millisecond.
pub fn format_time_ms(time_ms: f32) -> String {
    // Collapse -0.0 → 0.0 so we don't emit "-0 us".
    let t = if time_ms == 0.0 { 0.0 } else { time_ms };
    if t < 1.0 {
        format!("{:.0} us", t * 1000.0)
    } else {
        format!("{:.1} ms", t)
    }
}

/// Format a dB value the same way the original oscilloscope does:
/// `"-inf dB"` for silence, `"-12.3 dB"` otherwise.
pub fn format_db(db: f32) -> String {
    if !db.is_finite() || db < -96.0 {
        "-inf dB".to_string()
    } else {
        format!("{:.1} dB", db)
    }
}

/// Compute a tooltip's on-screen rectangle, clamped to a caller-supplied
/// area (normally the waveform draw rect, so the tooltip doesn't overlap
/// control bars or strips).
///
/// Anchors to `(cursor_x + 15, cursor_y - h/2)`. If that spills off the right
/// edge of the area, flips to the other side of the cursor
/// (`cursor_x - w - 15`). Then clamps Y to keep the tooltip fully inside
/// the area.
#[allow(clippy::too_many_arguments)]
pub fn cursor_tooltip_rect(
    cursor_x: f32,
    cursor_y: f32,
    tooltip_w: f32,
    tooltip_h: f32,
    area_x: f32,
    area_y: f32,
    area_w: f32,
    area_h: f32,
    scale: f32,
) -> (f32, f32) {
    let offset = 15.0 * scale;
    let mut tx = cursor_x + offset;
    if tx + tooltip_w > area_x + area_w {
        tx = cursor_x - tooltip_w - offset;
    }
    // If still off the left edge (very narrow area), clamp.
    if tx < area_x {
        tx = area_x;
    }
    let mut ty = cursor_y - tooltip_h / 2.0;
    if ty < area_y {
        ty = area_y;
    }
    if ty + tooltip_h > area_y + area_h {
        ty = (area_y + area_h - tooltip_h).max(area_y);
    }
    (tx, ty)
}

/// Draw the cursor tooltip: a time header plus one row per track reading.
/// Layout matches the original JUCE oscilloscope's drawCursorTooltip.
///
/// `(area_x, area_y, area_w, area_h)` describes the rectangle the tooltip
/// is allowed to occupy — typically the waveform draw area — so the
/// tooltip can't spill over into the control bar or control strips.
#[allow(clippy::too_many_arguments)]
pub fn draw_cursor_tooltip(
    pixmap: &mut tiny_skia::Pixmap,
    text_renderer: &mut tiny_skia_widgets::TextRenderer,
    cursor_x: f32,
    cursor_y: f32,
    time_label: &str,
    readings: &[CursorReading],
    area_x: f32,
    area_y: f32,
    area_w: f32,
    area_h: f32,
    scale: f32,
) {
    if readings.is_empty() {
        return;
    }
    // Tooltip dimensions (mirroring original: 130px wide, 14px line-height,
    // 6px padding, with an extra line for the time header). Width bumped
    // to 140px in the Rust port because DejaVuSans is proportional-width
    // and needs a bit more room than JUCE's monospaced default for longer
    // "-XX.X dB" readings.
    let font_size = 10.0 * scale;
    let line_h = 14.0 * scale;
    let padding = 6.0 * scale;
    let tooltip_w = 140.0 * scale;
    let tooltip_h = padding * 2.0 + line_h * (1.0 + readings.len() as f32);

    let (tx, ty) = cursor_tooltip_rect(
        cursor_x, cursor_y, tooltip_w, tooltip_h, area_x, area_y, area_w, area_h, scale,
    );

    // Background + border. When the tooltip is reporting a single
    // track (Vertical mode, or Overlay/Sum with only one visible
    // track), match the outline to that track's color — this ties the
    // tooltip visually to the lane/trace the cursor is over. With
    // multiple readings there's no unambiguous "the track", so fall
    // back to the amber foreground to match the rest of the theme
    // (and the existing name-hover tooltip, which uses the same color).
    let outline_color = if readings.len() == 1 {
        theme::to_color(readings[0].color)
    } else {
        theme::to_color(theme::FG)
    };
    tiny_skia_widgets::draw_rect(
        pixmap,
        tx,
        ty,
        tooltip_w,
        tooltip_h,
        theme::to_color_alpha(theme::BG, 0.95),
    );
    tiny_skia_widgets::draw_rect_outline(pixmap, tx, ty, tooltip_w, tooltip_h, outline_color, 1.0);

    // Time header (cyan)
    let mut y = ty + padding + font_size;
    text_renderer.draw_text(
        pixmap,
        tx + padding,
        y,
        time_label,
        font_size,
        theme::to_color(theme::CYAN),
    );
    y += line_h;

    // Per-track rows: color swatch + "Name[:8]: XdB"
    let swatch_size = 8.0 * scale;
    let swatch_gap = 4.0 * scale;
    for reading in readings {
        // Color swatch
        tiny_skia_widgets::draw_rect(
            pixmap,
            tx + padding,
            y - font_size + (font_size - swatch_size) / 2.0,
            swatch_size,
            swatch_size,
            theme::to_color(reading.color),
        );
        // Label text: truncated name (<=8 chars) + dB value
        let short_name: String = reading.name.chars().take(8).collect();
        let line = format!("{}: {}", short_name, format_db(reading.db));
        text_renderer.draw_text(
            pixmap,
            tx + padding + swatch_size + swatch_gap,
            y,
            &line,
            font_size,
            theme::to_color(theme::FG),
        );
        y += line_h;
    }
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
        draw_waveform_filled(&mut pm, &samples, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, color);
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_dispatch_line() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        draw_waveform(
            &mut pm,
            &samples,
            0.0,
            0.0,
            200.0,
            100.0,
            -48.0,
            0.0,
            theme::CYAN,
            crate::DrawStyle::Line,
        );
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_dispatch_filled() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        draw_waveform(
            &mut pm,
            &samples,
            0.0,
            0.0,
            200.0,
            100.0,
            -48.0,
            0.0,
            theme::CYAN,
            crate::DrawStyle::Filled,
        );
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_waveform_dispatch_both() {
        let mut pm = make_test_pixmap(200, 100);
        let samples = sine_samples(400);
        draw_waveform(
            &mut pm,
            &samples,
            0.0,
            0.0,
            200.0,
            100.0,
            -48.0,
            0.0,
            theme::CYAN,
            crate::DrawStyle::Both,
        );
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
        draw_beat_grid(
            &mut pm, 0.0, 0.0, 200.0, 100.0, 4, 4.0, &mut tr, 1.0, true, None,
        );
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_beat_grid_zero_beats() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_beat_grid(
            &mut pm, 0.0, 0.0, 200.0, 100.0, 4, 0.0, &mut tr, 1.0, true, None,
        );
        // Should not panic and should not draw
        assert!(!pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_amplitude_grid_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_amplitude_grid(
            &mut pm, 0.0, 0.0, 200.0, 100.0, -48.0, 0.0, &mut tr, 1.0, None,
        );
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_cursor_smoke() {
        let mut pm = make_test_pixmap(200, 100);
        draw_cursor(&mut pm, 100.0, 0.0, 100.0);
        assert!(pixmap_has_nonzero(&pm));
    }

    // ── Cursor tooltip ─────────────────────────────────────────────────

    #[test]
    fn test_format_time_ms_sub_millisecond() {
        assert_eq!(format_time_ms(0.5), "500 us");
        assert_eq!(format_time_ms(0.123), "123 us");
        assert_eq!(format_time_ms(0.0), "0 us");
    }

    #[test]
    fn test_format_time_ms_above_millisecond() {
        assert_eq!(format_time_ms(1.0), "1.0 ms");
        assert_eq!(format_time_ms(12.34), "12.3 ms");
        assert_eq!(format_time_ms(1500.0), "1500.0 ms");
    }

    #[test]
    fn test_format_time_ms_negative_zero_is_zero() {
        // Cosmetic: -0.0 * 1000 = -0 → would render "-0 us" without care.
        assert_eq!(format_time_ms(-0.0), "0 us");
    }

    #[test]
    fn test_format_time_ms_negative_uses_us_branch() {
        // Matches JUCE's signed branch (`if (timeMs < 1.0f)`), which sends
        // ALL negative values to the microsecond format regardless of
        // magnitude. Prevents "-5.0 ms" from disagreeing with "5000 us".
        assert_eq!(format_time_ms(-0.5), "-500 us");
        assert_eq!(format_time_ms(-5.0), "-5000 us");
    }

    #[test]
    fn test_format_db_normal() {
        assert_eq!(format_db(-12.3), "-12.3 dB");
        assert_eq!(format_db(0.0), "0.0 dB");
        assert_eq!(format_db(-96.0), "-96.0 dB");
    }

    #[test]
    fn test_format_db_silence() {
        assert_eq!(format_db(f32::NEG_INFINITY), "-inf dB");
        assert_eq!(format_db(-200.0), "-inf dB");
    }

    #[test]
    fn test_cursor_tooltip_rect_fits_right_of_cursor() {
        // area = (0, 0, 800, 500) with cursor at (100, 200)
        let (tx, ty) = cursor_tooltip_rect(100.0, 200.0, 140.0, 60.0, 0.0, 0.0, 800.0, 500.0, 1.0);
        // Right of cursor + 15px offset
        assert!((tx - 115.0).abs() < 0.001);
        // Vertically centred on cursor
        assert!((ty - (200.0 - 30.0)).abs() < 0.001);
    }

    #[test]
    fn test_cursor_tooltip_rect_flips_off_right_edge() {
        // Cursor near right edge — tooltip won't fit to the right, should flip
        let (tx, _) = cursor_tooltip_rect(780.0, 200.0, 140.0, 60.0, 0.0, 0.0, 800.0, 500.0, 1.0);
        // Flipped: cursor_x - tooltip_w - 15
        assert!((tx - (780.0 - 140.0 - 15.0)).abs() < 0.001);
    }

    #[test]
    fn test_cursor_tooltip_rect_clamps_top() {
        // Verify that the clamp actually triggers: without the clamp, the
        // Y would be cursor_y - tooltip_h / 2.0 = 5 - 30 = -25, which is
        // above the area top. This pins the behavior so nobody can
        // accidentally "pass" this test by tweaking centering math.
        let cursor_y = 5.0;
        let tooltip_h = 60.0;
        let pre_clamp_y = cursor_y - tooltip_h / 2.0;
        assert!(
            pre_clamp_y < 0.0,
            "test setup must put tooltip above the area"
        );

        let (_, ty) = cursor_tooltip_rect(
            100.0, cursor_y, 140.0, tooltip_h, 0.0, 0.0, 800.0, 500.0, 1.0,
        );
        assert_eq!(ty, 0.0);
    }

    #[test]
    fn test_cursor_tooltip_rect_clamps_bottom() {
        // Cursor near bottom — tooltip would spill below area bottom
        let (_, ty) = cursor_tooltip_rect(100.0, 495.0, 140.0, 60.0, 0.0, 0.0, 800.0, 500.0, 1.0);
        assert!((ty - (500.0 - 60.0)).abs() < 0.001);
    }

    #[test]
    fn test_cursor_tooltip_rect_clamps_to_non_origin_area() {
        // Regression: passing a waveform-area rect whose origin is NOT
        // (0, 0) (e.g. in Vertical mode where wave_area_x = strip_w).
        // The tooltip must respect the area's top-left corner and bottom.
        let area_x = 90.0;
        let area_y = 10.0;
        let area_w = 500.0;
        let area_h = 300.0;
        // Cursor near the top of the area: clamp should use area_y, not 0.
        let (_, ty) = cursor_tooltip_rect(
            200.0,
            area_y + 5.0,
            140.0,
            60.0,
            area_x,
            area_y,
            area_w,
            area_h,
            1.0,
        );
        assert_eq!(ty, area_y);
        // Cursor near the area's right edge should flip to the left of
        // the cursor but still not cross the area's left edge.
        let cursor_x = area_x + area_w - 5.0;
        let (tx, _) = cursor_tooltip_rect(
            cursor_x, 150.0, 140.0, 60.0, area_x, area_y, area_w, area_h, 1.0,
        );
        assert!(tx >= area_x);
        assert!(tx + 140.0 <= area_x + area_w + 0.001);
    }

    #[test]
    fn test_draw_cursor_tooltip_smoke() {
        let mut pm = make_test_pixmap(400, 300);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        let readings = [
            CursorReading {
                name: "Kick".into(),
                color: theme::FG,
                db: -6.0,
            },
            CursorReading {
                name: "Snare".into(),
                color: theme::CYAN,
                db: -12.5,
            },
        ];
        draw_cursor_tooltip(
            &mut pm, &mut tr, 200.0, 150.0, "12.3 ms", &readings, 0.0, 0.0, 400.0, 300.0, 1.0,
        );
        assert!(pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_cursor_tooltip_empty_readings_noop() {
        let mut pm = make_test_pixmap(400, 300);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        draw_cursor_tooltip(
            &mut pm,
            &mut tr,
            200.0,
            150.0,
            "0 us",
            &[],
            0.0,
            0.0,
            400.0,
            300.0,
            1.0,
        );
        // No readings → no draw
        assert!(!pixmap_has_nonzero(&pm));
    }

    #[test]
    fn test_draw_cursor_tooltip_truncates_long_name() {
        // Just a smoke test that long names don't panic
        let mut pm = make_test_pixmap(400, 300);
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut tr = tiny_skia_widgets::TextRenderer::new(font_data);
        let readings = [CursorReading {
            name: "A very long track name that exceeds eight characters".into(),
            color: theme::FG,
            db: -3.0,
        }];
        draw_cursor_tooltip(
            &mut pm, &mut tr, 200.0, 150.0, "1.2 ms", &readings, 0.0, 0.0, 400.0, 300.0, 1.0,
        );
        assert!(pixmap_has_nonzero(&pm));
    }
}
