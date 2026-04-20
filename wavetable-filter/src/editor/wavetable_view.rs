//! Wavetable visualization — 2D face-on or 3D overhead stack.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::wavetable::Wavetable;

pub(crate) struct FrameCache {
    pub cached_frames: Vec<Vec<f32>>,
    pub cached_version: u32,
    pub cached_frame_count: usize,
    pub cached_frame_size: usize,
    pub global_min: f32,
    pub global_max: f32,
}

impl FrameCache {
    pub fn new() -> Self {
        Self {
            cached_frames: Vec::new(),
            cached_version: u32::MAX,
            cached_frame_count: 0,
            cached_frame_size: 0,
            global_min: 0.0,
            global_max: 0.0,
        }
    }
}

/// Refresh the cached frames if the wavetable version has advanced.
/// Uses `try_lock` to avoid stalling the GUI thread on contention.
pub(crate) fn refresh_frame_cache(
    cache: &mut FrameCache,
    shared_wt: &Mutex<Wavetable>,
    version: &AtomicU32,
) {
    let current_version = version.load(Ordering::Relaxed);
    if current_version == cache.cached_version {
        return;
    }
    let Ok(wt) = shared_wt.try_lock() else {
        return;
    };
    cache.cached_frames = wt.frames.clone();
    cache.cached_frame_count = wt.frame_count;
    cache.cached_frame_size = wt.frame_size;
    cache.cached_version = current_version;

    let mut gmin = f32::INFINITY;
    let mut gmax = f32::NEG_INFINITY;
    for frame in &cache.cached_frames {
        for &sample in frame {
            gmin = gmin.min(sample);
            gmax = gmax.max(sample);
        }
    }
    cache.global_min = gmin;
    cache.global_max = gmax;
}

/// Draw the wavetable visualization into `pixmap` at the given bounds.
/// `current_frame_pos` is the normalized [0,1] frame index; `show_2d` selects face-on (true)
/// or 3D overhead stack (false).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_wavetable_view(
    pixmap: &mut Pixmap,
    cache: &FrameCache,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    current_frame_pos: f32,
    show_2d: bool,
) {
    // Background
    let mut bg = PathBuilder::new();
    bg.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h).expect("valid rect"));
    if let Some(bg_path) = bg.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(20, 22, 28, 255);
        pixmap.fill_path(&bg_path, &paint, FillRule::Winding, Transform::identity(), None);

        let mut border = Paint::default();
        border.set_color_rgba8(60, 60, 70, 255);
        border.anti_alias = true;
        let stroke = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&bg_path, &border, &stroke, Transform::identity(), None);
    }

    let frame_count = cache.cached_frame_count;
    let frame_size = cache.cached_frame_size;
    if frame_count == 0 || frame_size == 0 {
        return;
    }

    let padding = 20.0;
    let width = w - padding * 2.0;
    let height = h - padding * 2.0;
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let range = (cache.global_max - cache.global_min).max(0.001);
    let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;

    if show_2d {
        draw_2d_face_on(
            pixmap,
            &cache.cached_frames,
            current_frame_pos,
            x + padding,
            y + padding,
            width,
            height,
            frame_count,
            frame_size,
        );
        draw_zero_line(pixmap, x + padding, y + padding + height * 0.5, width);
        return;
    }

    draw_3d_overhead(
        pixmap,
        &cache.cached_frames,
        current_frame_idx,
        cache.global_min,
        range,
        x,
        y,
        w,
        h,
        padding,
        width,
        height,
        frame_count,
        frame_size,
    );

    // Zero line (grid)
    draw_zero_line(pixmap, x + padding, y + padding + height * 0.5, width);
}

#[allow(clippy::too_many_arguments)]
fn draw_2d_face_on(
    pixmap: &mut Pixmap,
    frames: &[Vec<f32>],
    current_frame_pos: f32,
    x0: f32,
    y0: f32,
    width: f32,
    height: f32,
    frame_count: usize,
    frame_size: usize,
) {
    let exact_pos = current_frame_pos * (frame_count - 1) as f32;
    let lo = (exact_pos.floor() as usize).min(frame_count - 1);
    let hi = (lo + 1).min(frame_count - 1);
    let frac = exact_pos - lo as f32;

    let frame_lo = &frames[lo];
    let frame_hi = &frames[hi];

    let num_points = (width as usize).min(frame_size).max(1);

    let mut fmin = f32::INFINITY;
    let mut fmax = f32::NEG_INFINITY;
    for pi in 0..num_points {
        let si = ((pi as f32 / num_points as f32) * frame_size as f32) as usize;
        let si = si.min(frame_size - 1);
        let s = frame_lo[si] * (1.0 - frac) + frame_hi[si] * frac;
        fmin = fmin.min(s);
        fmax = fmax.max(s);
    }
    let frange = (fmax - fmin).max(0.001);
    let zero_y = y0 + height * 0.5;

    let mut fill_pb = PathBuilder::new();
    let mut stroke_pb = PathBuilder::new();
    fill_pb.move_to(x0, zero_y);

    for pi in 0..num_points {
        let t = pi as f32 / num_points as f32;
        let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
        let s = frame_lo[si] * (1.0 - frac) + frame_hi[si] * frac;
        let normalized = (s - fmin) / frange;
        let x = x0 + t * width;
        let y = y0 + height - normalized * height;

        fill_pb.line_to(x, y);
        if pi == 0 {
            stroke_pb.move_to(x, y);
        } else {
            stroke_pb.line_to(x, y);
        }
    }
    fill_pb.line_to(x0 + width, zero_y);
    fill_pb.close();

    if let Some(fill_path) = fill_pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(79, 195, 247, 30);
        paint.anti_alias = true;
        pixmap.fill_path(&fill_path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(stroke_path) = stroke_pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(79, 195, 247, 220);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: 1.5,
            line_cap: LineCap::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&stroke_path, &paint, &stroke, Transform::identity(), None);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_3d_overhead(
    pixmap: &mut Pixmap,
    frames: &[Vec<f32>],
    current_frame_idx: usize,
    global_min: f32,
    range: f32,
    bounds_x: f32,
    bounds_y: f32,
    _bounds_w: f32,
    bounds_h: f32,
    padding: f32,
    width: f32,
    height: f32,
    frame_count: usize,
    frame_size: usize,
) {
    // Non-active frames, back-to-front
    for frame_idx in (0..frame_count).rev() {
        if frame_idx == current_frame_idx {
            continue;
        }
        let frame = &frames[frame_idx];
        let depth = frame_idx as f32 / frame_count.max(1) as f32;
        let perspective_x = depth * 80.0;
        let perspective_y = -depth * 80.0;
        let alpha = 0.3 + (1.0 - depth) * 0.4;

        let draw_w = (width * 0.7) as usize;
        let pts = draw_w.min(frame_size).max(1);

        let mut pb = PathBuilder::new();
        for pi in 0..pts {
            let t = pi as f32 / pts as f32;
            let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
            let normalized = (frame[si] - global_min) / range;
            let x = bounds_x + padding + t * (width * 0.7) + perspective_x;
            let y =
                bounds_y + bounds_h - padding * 2.0 - (normalized * height * 0.4) + perspective_y;
            if pi == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            let r = (50.0 + (1.0 - depth) * 100.0) as u8;
            let g = (100.0 + (1.0 - depth) * 100.0) as u8;
            let a = (alpha * 255.0) as u8;
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, 255, a);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: 1.2,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    // Active frame on top
    if current_frame_idx < frame_count {
        let frame = &frames[current_frame_idx];
        let depth = current_frame_idx as f32 / frame_count.max(1) as f32;
        let perspective_x = depth * 80.0;
        let perspective_y = -depth * 80.0;

        let draw_w = (width * 0.7) as usize;
        let pts = draw_w.min(frame_size).max(1);
        let mut pb = PathBuilder::new();
        for pi in 0..pts {
            let t = pi as f32 / pts as f32;
            let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
            let normalized = (frame[si] - global_min) / range;
            let x = bounds_x + padding + t * (width * 0.7) + perspective_x;
            let y =
                bounds_y + bounds_h - padding * 2.0 - (normalized * height * 0.4) + perspective_y;
            if pi == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 200, 100, 255);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: 2.5,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
}

fn draw_zero_line(pixmap: &mut Pixmap, x: f32, y: f32, w: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + w, y);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(80, 80, 90, 100);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: 0.5,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}
