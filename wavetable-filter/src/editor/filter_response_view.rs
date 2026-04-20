//! Filter response curve + input spectrum shadow, using tiny-skia paths.

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use std::sync::Mutex;
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use tiny_skia_widgets::TextRenderer;

use crate::wavetable::Wavetable;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const DB_CEIL: f32 = 0.0;
const DB_FLOOR: f32 = -48.0;
const DB_RANGE: f32 = DB_CEIL - DB_FLOOR;

const PARAM_EPSILON: f32 = 0.001;

pub(crate) struct FftCache {
    planner: RealFftPlanner<f32>,
    frame_buf: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    cached_mags: Vec<f32>,
    cached_frame_pos: f32,
    cached_cutoff: f32,
    cached_resonance: f32,
    freq_table: Vec<f32>,
    freq_table_size: usize,
    cached_response_ys: Vec<f32>,
    cached_input_mags: Vec<f32>,
    cached_input_sr: f32,
}

impl FftCache {
    pub fn new() -> Self {
        Self {
            planner: RealFftPlanner::new(),
            frame_buf: Vec::new(),
            spectrum: Vec::new(),
            cached_mags: Vec::new(),
            cached_frame_pos: -1.0,
            cached_cutoff: -1.0,
            cached_resonance: -1.0,
            freq_table: Vec::new(),
            freq_table_size: 0,
            cached_response_ys: Vec::new(),
            cached_input_mags: Vec::new(),
            cached_input_sr: 0.0,
        }
    }
}

pub(crate) fn refresh_fft_cache(
    cache: &mut FftCache,
    frame_pos: f32,
    cutoff_hz: f32,
    resonance: f32,
    shared_wt: &Mutex<Wavetable>,
) {
    let needs_update = (cache.cached_frame_pos - frame_pos).abs() > 0.01
        || (cache.cached_cutoff - cutoff_hz).abs() > PARAM_EPSILON
        || (cache.cached_resonance - resonance).abs() > PARAM_EPSILON
        || cache.cached_mags.is_empty();
    if !needs_update {
        return;
    }

    let frame_n = match shared_wt.try_lock() {
        Ok(wt) => {
            if wt.frame_count == 0 || wt.frame_size == 0 {
                return;
            }
            let n = wt.frame_size;
            cache.frame_buf.resize(n, 0.0);
            wt.interpolate_frame_into(frame_pos, &mut cache.frame_buf);
            n
        }
        Err(_) => return,
    };

    let fft = cache.planner.plan_fft_forward(frame_n);
    cache
        .spectrum
        .resize(frame_n / 2 + 1, Complex::new(0.0, 0.0));
    for c in cache.spectrum.iter_mut() {
        *c = Complex::new(0.0, 0.0);
    }

    let FftCache {
        frame_buf,
        spectrum,
        cached_mags,
        ..
    } = cache;

    if fft.process(frame_buf, spectrum).is_err() {
        return;
    }
    cached_mags.clear();
    cached_mags.extend(spectrum.iter().map(|c| c.norm()));
    let peak = cached_mags
        .iter()
        .cloned()
        .fold(0.0f32, f32::max)
        .max(1e-10);
    for m in cached_mags.iter_mut() {
        *m /= peak;
    }

    cache.cached_frame_pos = frame_pos;
    cache.cached_cutoff = cutoff_hz;
    cache.cached_resonance = resonance;
    cache.cached_response_ys.clear();
}

pub(crate) fn refresh_input_spectrum(cache: &mut FftCache, shared_in: &Mutex<(f32, Vec<f32>)>) {
    let Ok(data) = shared_in.try_lock() else {
        return;
    };
    let (sr, ref mags) = *data;
    if sr <= 0.0 || mags.is_empty() {
        return;
    }
    cache.cached_input_mags.resize(mags.len(), 0.0);
    cache.cached_input_mags.copy_from_slice(mags);
    cache.cached_input_sr = sr;
}

fn ensure_freq_table(cache: &mut FftCache, num_points: usize) {
    if cache.freq_table_size == num_points {
        return;
    }
    cache.freq_table.resize(num_points + 1, 0.0);
    let log_min = FREQ_MIN.ln();
    let log_range = FREQ_MAX.ln() - log_min;
    for i in 0..=num_points {
        let x_norm = i as f32 / num_points as f32;
        cache.freq_table[i] = (log_min + x_norm * log_range).exp();
    }
    cache.freq_table_size = num_points;
    cache.cached_response_ys.clear();
}

fn ensure_response_ys(cache: &mut FftCache, cutoff_hz: f32, resonance: f32, height: f32, y0: f32) {
    let n = cache.freq_table_size;
    if cache.cached_response_ys.len() == n + 1 || cache.cached_mags.is_empty() {
        return;
    }
    let comb_exp = resonance * 8.0;
    let FftCache {
        cached_mags,
        freq_table,
        cached_response_ys,
        ..
    } = cache;

    let max_src = (cached_mags.len() - 1) as f32;
    cached_response_ys.resize(n + 1, 0.0);
    for i in 0..=n {
        let freq = freq_table[i];
        let src = freq * 24.0 / cutoff_hz;
        let mag = if src >= max_src {
            0.0
        } else if src <= 0.0 {
            cached_mags[0]
        } else {
            let lo = src.floor() as usize;
            let frac = src - lo as f32;
            let interp = cached_mags[lo] * (1.0 - frac) + cached_mags[lo + 1] * frac;
            if comb_exp > 0.01 {
                let dist = frac.min(1.0 - frac);
                let comb = (std::f32::consts::PI * dist).cos().powf(comb_exp);
                interp * comb
            } else {
                interp
            }
        };
        let db = 20.0 * mag.max(1e-6).log10();
        let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
        cached_response_ys[i] = y0 + height - y_norm * height;
    }
}

fn freq_to_x(freq_hz: f32) -> f32 {
    ((freq_hz.max(FREQ_MIN).ln() - FREQ_MIN.ln()) / (FREQ_MAX.ln() - FREQ_MIN.ln())).clamp(0.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_filter_response(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cache: &mut FftCache,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    cutoff_hz: f32,
    resonance: f32,
) {
    // Background
    let mut bg = PathBuilder::new();
    bg.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h).expect("valid rect"));
    if let Some(bg_path) = bg.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(20, 22, 28, 255);
        pixmap.fill_path(
            &bg_path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let mut border = Paint::default();
        border.set_color_rgba8(60, 60, 70, 255);
        border.anti_alias = true;
        pixmap.stroke_path(
            &bg_path,
            &border,
            &Stroke {
                width: 1.0,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }

    let padding = 20.0;
    let width = w - padding * 2.0;
    let height = h - padding * 2.0;
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let x0 = x + padding;
    let y0 = y + padding;

    let num_points = (width.max(1.0) as usize).min(256);
    ensure_freq_table(cache, num_points);
    ensure_response_ys(cache, cutoff_hz, resonance, height, y0);

    // Grid: horizontal dB lines
    for db in [-12.0_f32, -24.0, -36.0, -48.0] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let gy = y0 + height - y_norm * height;
        stroke_line(pixmap, x0, gy, x0 + width, gy, (80, 80, 90, 100), 0.5);
    }
    // 0 dB reference, slightly brighter
    {
        let y_norm = (0.0_f32 - DB_FLOOR) / DB_RANGE;
        let gy = y0 + height - y_norm * height;
        stroke_line(pixmap, x0, gy, x0 + width, gy, (120, 120, 140, 180), 0.5);
    }
    // Vertical decade lines
    for freq in [100.0_f32, 1000.0, 10000.0] {
        let gx = x0 + freq_to_x(freq) * width;
        stroke_line(pixmap, gx, y0, gx, y0 + height, (80, 80, 90, 100), 0.5);
    }

    // Input spectrum shadow
    if cache.cached_input_sr > 0.0 && !cache.cached_input_mags.is_empty() {
        let bin_hz = cache.cached_input_sr / (2.0 * (cache.cached_input_mags.len() - 1) as f32);
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0 + height);
        for i in 0..=num_points {
            let freq = cache.freq_table[i];
            let bin = freq / bin_hz;
            let mag = if bin >= (cache.cached_input_mags.len() - 1) as f32 {
                0.0
            } else if bin <= 0.0 {
                cache.cached_input_mags[0]
            } else {
                let lo = bin.floor() as usize;
                let frac = bin - lo as f32;
                cache.cached_input_mags[lo] * (1.0 - frac) + cache.cached_input_mags[lo + 1] * frac
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

    // Response curve
    if cache.cached_response_ys.len() == num_points + 1 {
        let mut fill_pb = PathBuilder::new();
        let mut stroke_pb = PathBuilder::new();
        for (i, &yy) in cache.cached_response_ys.iter().enumerate() {
            let xx = x0 + (i as f32 / num_points as f32) * width;
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

    // Cutoff marker
    let cutoff_x = x0 + freq_to_x(cutoff_hz) * width;
    stroke_line(
        pixmap,
        cutoff_x,
        y0,
        cutoff_x,
        y0 + height,
        (255, 100, 100, 200),
        2.0,
    );

    // Frequency labels — centered under each tick
    let text_size = 10.0;
    let labels_y = y + h - 5.0;
    for (freq, label) in [
        (50.0_f32, "50"),
        (200.0, "200"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (20000.0, "20k"),
    ] {
        let tw = text_renderer.text_width(label, text_size);
        let tx = x0 + freq_to_x(freq) * width - tw * 0.5;
        text_renderer.draw_text(
            pixmap,
            tx,
            labels_y,
            label,
            text_size,
            tiny_skia::Color::from_rgba8(150, 150, 150, 255),
        );
    }

    // dB labels — right-aligned to x0
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
