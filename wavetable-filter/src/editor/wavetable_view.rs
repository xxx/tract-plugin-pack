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
    /// Pre-rasterized 3D background (all strands + bg fill + border + zero line).
    /// Rebuilt only when the wavetable version changes or the viewport resizes.
    /// Drawing an active strand on top of this is essentially free compared to
    /// rasterizing ~40 anti-aliased 200-point strokes every frame.
    pub bg_pixmap: Option<Pixmap>,
    pub bg_key: (u32, u32, u32, usize),
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
            bg_pixmap: None,
            bg_key: (0, 0, u32::MAX, 0),
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
    cache: &mut FrameCache,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    current_frame_pos: f32,
    show_2d: bool,
) {
    let frame_count = cache.cached_frame_count;
    let frame_size = cache.cached_frame_size;
    let padding = 20.0;
    let width = w - padding * 2.0;
    let height = h - padding * 2.0;

    // 2D mode: cheap (~3 µs), no caching worth doing.
    if show_2d {
        fill_view_bg(pixmap, x, y, w, h);
        if frame_count == 0 || frame_size == 0 || width <= 0.0 || height <= 0.0 {
            return;
        }
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

    // 3D mode: rasterizing all strands every frame is expensive. Cache the full
    // background (bg fill, border, all strands, zero line) into a viewport-sized
    // pixmap, rebuild only when the wavetable or viewport changes, and overlay
    // just the active strand each frame.
    if frame_count == 0 || frame_size == 0 || width <= 0.0 || height <= 0.0 {
        fill_view_bg(pixmap, x, y, w, h);
        return;
    }

    let w_px = w.ceil() as u32;
    let h_px = h.ceil() as u32;
    let key = (w_px, h_px, cache.cached_version, frame_count);
    if cache.bg_pixmap.is_none() || cache.bg_key != key {
        cache.bg_pixmap = build_3d_background_pixmap(
            cache.cached_version,
            &cache.cached_frames,
            cache.global_min,
            cache.global_max,
            w_px,
            h_px,
            padding,
            width,
            height,
            frame_count,
            frame_size,
        );
        cache.bg_key = key;
    }

    if let Some(bg) = cache.bg_pixmap.as_ref() {
        // Raw row-by-row byte copy. tiny-skia's `draw_pixmap` runs the full
        // raster pipeline (gather → transform → seed_shader → store) pixel-by-
        // pixel even with `BlendMode::Source`, which was ~12% of total CPU in
        // the profile. Since both pixmaps are the same format (RGBA8888
        // premultiplied) and the blit is axis-aligned at integer offsets, a
        // straight `copy_from_slice` is byte-identical and much cheaper.
        blit_opaque(pixmap, bg, x.round() as i32, y.round() as i32);
    } else {
        fill_view_bg(pixmap, x, y, w, h);
    }

    let range = (cache.global_max - cache.global_min).max(0.001);
    let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;
    draw_active_strand(
        pixmap,
        &cache.cached_frames,
        current_frame_idx,
        cache.global_min,
        range,
        x,
        y,
        h,
        padding,
        width,
        height,
        frame_count,
        frame_size,
    );
}

/// Fill the full view rect with bg + border. Used by 2D mode and as a fallback.
fn fill_view_bg(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32) {
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
        let stroke = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&bg_path, &border, &stroke, Transform::identity(), None);
    }
}

/// Copy the full `src` pixmap into `dst` at top-left `(dst_x, dst_y)`, clipped
/// to the destination bounds. Both pixmaps must share RGBA8888 premultiplied
/// format (tiny-skia's native). Byte-identical to
/// `dst.draw_pixmap(..., BlendMode::Source, ...)` for opaque content, but
/// bypasses the raster pipeline — one `copy_from_slice` per row.
fn blit_opaque(dst: &mut Pixmap, src: &Pixmap, dst_x: i32, dst_y: i32) {
    let src_w = src.width() as i32;
    let src_h = src.height() as i32;
    let dst_w = dst.width() as i32;
    let dst_h = dst.height() as i32;

    // Intersect src-in-dst with dst bounds; compute src starting offset when
    // negative dst_x/dst_y clip into src.
    let x0 = dst_x.max(0);
    let y0 = dst_y.max(0);
    let sx0 = (-dst_x).max(0);
    let sy0 = (-dst_y).max(0);
    let x1 = (dst_x + src_w).min(dst_w);
    let y1 = (dst_y + src_h).min(dst_h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    let copy_w = (x1 - x0) as usize;
    let copy_h = (y1 - y0) as usize;
    let src_stride = src_w as usize * 4;
    let dst_stride = dst_w as usize * 4;
    let row_bytes = copy_w * 4;

    let src_data = src.data();
    let dst_data = dst.data_mut();

    for row in 0..copy_h {
        let sy = sy0 as usize + row;
        let dy = y0 as usize + row;
        let src_off = sy * src_stride + (sx0 as usize) * 4;
        let dst_off = dy * dst_stride + (x0 as usize) * 4;
        dst_data[dst_off..dst_off + row_bytes]
            .copy_from_slice(&src_data[src_off..src_off + row_bytes]);
    }
}

/// Render the full 3D background (bg fill, border, all stridden strands, zero line)
/// into a fresh `(w_px, h_px)` pixmap. Strand coords are in local viewport space.
#[allow(clippy::too_many_arguments)]
fn build_3d_background_pixmap(
    _version: u32,
    frames: &[Vec<f32>],
    global_min: f32,
    global_max: f32,
    w_px: u32,
    h_px: u32,
    padding: f32,
    width: f32,
    height: f32,
    frame_count: usize,
    frame_size: usize,
) -> Option<Pixmap> {
    let mut bg = Pixmap::new(w_px, h_px)?;
    // Fill + border into the cache pixmap at local (0,0).
    fill_view_bg(&mut bg, 0.0, 0.0, w_px as f32, h_px as f32);

    let range = (global_max - global_min).max(0.001);

    const MAX_STRANDS: usize = 48;
    let stride = frame_count.div_ceil(MAX_STRANDS).max(1);

    // All strands, back-to-front. No active skipping — the active strand is
    // overlaid on the target pixmap separately.
    for frame_idx in (0..frame_count).rev() {
        if frame_idx % stride != 0 {
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
            // Local coords: bounds_x = bounds_y = 0.
            let x = padding + t * (width * 0.7) + perspective_x;
            let y = h_px as f32 - padding * 2.0 - (normalized * height * 0.4) + perspective_y;
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
            bg.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    // Zero line in the cached bg.
    draw_zero_line(&mut bg, padding, padding + height * 0.5, width);
    Some(bg)
}

/// Draw the orange active strand on top of the blitted background.
#[allow(clippy::too_many_arguments)]
fn draw_active_strand(
    pixmap: &mut Pixmap,
    frames: &[Vec<f32>],
    current_frame_idx: usize,
    global_min: f32,
    range: f32,
    bounds_x: f32,
    bounds_y: f32,
    bounds_h: f32,
    padding: f32,
    width: f32,
    height: f32,
    frame_count: usize,
    frame_size: usize,
) {
    if current_frame_idx >= frame_count {
        return;
    }
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
        let y = bounds_y + bounds_h - padding * 2.0 - (normalized * height * 0.4) + perspective_y;
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
