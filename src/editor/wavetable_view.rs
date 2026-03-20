use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::WavetableFilterParams;

pub struct WavetableView {
    params: Arc<WavetableFilterParams>,
    shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    frame_cache: RefCell<FrameCache>,
    /// When true, show 2D face-on view of the current frame instead of 3D overview.
    show_2d: Cell<bool>,
}

struct FrameCache {
    cached_frames: Vec<Vec<f32>>,
    cached_version: u32,
    cached_frame_count: usize,
    cached_frame_size: usize,
    // 3D: global min/max across all frames (recomputed on wavetable change)
    global_min: f32,
    global_max: f32,
    // 2D: interpolated frame cache (recomputed on frame position change)
    interp_frame: Vec<f32>,
    interp_frame_pos: f32,
    interp_min: f32,
    interp_max: f32,
}

impl FrameCache {
    fn new() -> Self {
        Self {
            cached_frames: Vec::new(),
            cached_version: u32::MAX,
            cached_frame_count: 0,
            cached_frame_size: 0,
            global_min: 0.0,
            global_max: 0.0,
            interp_frame: Vec::new(),
            interp_frame_pos: -1.0,
            interp_min: 0.0,
            interp_max: 0.0,
        }
    }
}

impl WavetableView {
    pub fn new<'a>(
        cx: &'a mut Context,
        params: Arc<WavetableFilterParams>,
        shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
        wavetable_version: Arc<AtomicU32>,
    ) -> Handle<'a, Self> {
        Self {
            params,
            shared_wavetable,
            wavetable_version,
            frame_cache: RefCell::new(FrameCache::new()),
            show_2d: Cell::new(false),
        }
        .build(cx, |_cx| {})
    }
}

impl View for WavetableView {
    fn element(&self) -> Option<&'static str> {
        Some("wavetable-view")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();

        // Draw background
        let mut path = vg::Path::new();
        path.rect(bounds.x, bounds.y, bounds.w, bounds.h);
        canvas.fill_path(&path, &vg::Paint::color(vg::Color::rgb(20, 22, 28)));

        // Draw border
        canvas.stroke_path(
            &path,
            &vg::Paint::color(vg::Color::rgb(60, 60, 70)).with_line_width(1.0),
        );

        // Draw wavetable visualization
        let padding = 20.0;
        let width = bounds.w - padding * 2.0;
        let height = bounds.h - padding * 2.0;

        // Update cached frames only when the wavetable version has changed
        {
            let current_version = self.wavetable_version.load(Ordering::Relaxed);
            let mut cache = self.frame_cache.borrow_mut();

            if current_version != cache.cached_version {
                // Try to acquire the wavetable lock without blocking the GUI thread
                if let Ok(wavetable) = self.shared_wavetable.try_lock() {
                    cache.cached_frames = wavetable.frames.clone();
                    cache.cached_frame_count = wavetable.frame_count;
                    cache.cached_frame_size = wavetable.frame_size;
                    cache.cached_version = current_version;
                    // Recompute global min/max for 3D view
                    let mut gmin = f32::INFINITY;
                    let mut gmax = f32::NEG_INFINITY;
                    for frame in &cache.cached_frames {
                        for &s in frame {
                            gmin = gmin.min(s);
                            gmax = gmax.max(s);
                        }
                    }
                    cache.global_min = gmin;
                    cache.global_max = gmax;
                    // Invalidate interpolated frame cache
                    cache.interp_frame_pos = -1.0;
                }
                // If lock is contended, we keep the stale cached data
            }
        }

        let current_frame_pos = self.params.frame_position.unmodulated_normalized_value();

        // Update 2D interpolation cache if needed (narrow mutable borrow)
        {
            let mut cache = self.frame_cache.borrow_mut();
            let frame_count = cache.cached_frame_count;
            let frame_size = cache.cached_frame_size;
            if frame_count > 0 && frame_size > 0 && self.show_2d.get() {
                let frame_step = if frame_count > 1 {
                    0.5 / (frame_count - 1) as f32
                } else {
                    1.0
                };
                if (current_frame_pos - cache.interp_frame_pos).abs() > frame_step
                    || cache.interp_frame.len() != frame_size
                {
                    let exact_pos = current_frame_pos * (frame_count - 1) as f32;
                    let lo = (exact_pos.floor() as usize).min(frame_count - 1);
                    let hi = (lo + 1).min(frame_count - 1);
                    let frac = exact_pos - lo as f32;

                    cache.interp_frame.resize(frame_size, 0.0);
                    let mut fmin = f32::INFINITY;
                    let mut fmax = f32::NEG_INFINITY;
                    for i in 0..frame_size {
                        let s = cache.cached_frames[lo][i] * (1.0 - frac)
                            + cache.cached_frames[hi][i] * frac;
                        cache.interp_frame[i] = s;
                        fmin = fmin.min(s);
                        fmax = fmax.max(s);
                    }
                    cache.interp_min = fmin;
                    cache.interp_max = fmax;
                    cache.interp_frame_pos = current_frame_pos;
                }
            }
        }

        let cache = self.frame_cache.borrow();
        let frame_count = cache.cached_frame_count;
        let frame_size = cache.cached_frame_size;

        if frame_count == 0 || frame_size == 0 {
            return;
        }

        let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;

        if self.show_2d.get() {
            let range = (cache.interp_max - cache.interp_min).max(0.001);

            // Draw filled waveform
            let x0 = bounds.x + padding;
            let y0 = bounds.y + padding;
            let zero_y = y0 + height / 2.0;

            let mut fill_path = vg::Path::new();
            let mut stroke_path = vg::Path::new();
            fill_path.move_to(x0, zero_y);

            for i in 0..frame_size {
                let s = cache.interp_frame[i];
                let normalized = (s - cache.interp_min) / range;
                let x = x0 + (i as f32 / frame_size as f32) * width;
                let y = y0 + height - normalized * height;

                fill_path.line_to(x, y);
                if i == 0 {
                    stroke_path.move_to(x, y);
                } else {
                    stroke_path.line_to(x, y);
                }
            }

            fill_path.line_to(x0 + width, zero_y);
            fill_path.close();

            canvas.fill_path(
                &fill_path,
                &vg::Paint::color(vg::Color::rgba(79, 195, 247, 30)),
            );
            canvas.stroke_path(
                &stroke_path,
                &vg::Paint::color(vg::Color::rgba(79, 195, 247, 220)).with_line_width(1.5),
            );

            // Zero-crossing line
            let mut zero_path = vg::Path::new();
            zero_path.move_to(x0, zero_y);
            zero_path.line_to(x0 + width, zero_y);
            canvas.stroke_path(
                &zero_path,
                &vg::Paint::color(vg::Color::rgba(80, 80, 90, 100)).with_line_width(0.5),
            );
        } else {
            // === 3D overhead perspective view ===
            let global_min = cache.global_min;
            let global_max = cache.global_max;
            let range = (global_max - global_min).max(0.001);

            // Draw all non-active frames first (back to front)
            for frame_idx in (0..frame_count).rev() {
                if frame_idx == current_frame_idx {
                    continue;
                }

                let frame = &cache.cached_frames[frame_idx];
                let depth = frame_idx as f32 / frame_count.max(1) as f32;
                let perspective_x = depth * 80.0;
                let perspective_y = -depth * 80.0;
                let alpha = 0.3 + (1.0 - depth) * 0.4;

                let mut path = vg::Path::new();
                for (i, &sample) in frame.iter().enumerate() {
                    let normalized = (sample - global_min) / range;
                    let x = bounds.x + padding
                        + (i as f32 / frame_size as f32) * (width * 0.7)
                        + perspective_x;
                    let y = bounds.y + bounds.h - padding * 2.0
                        - (normalized * height * 0.4)
                        + perspective_y;
                    if i == 0 { path.move_to(x, y); } else { path.line_to(x, y); }
                }

                let color = vg::Color::rgba(
                    (50.0 + (1.0 - depth) * 100.0) as u8,
                    (100.0 + (1.0 - depth) * 100.0) as u8,
                    255,
                    (alpha * 255.0) as u8,
                );
                canvas.stroke_path(&path, &vg::Paint::color(color).with_line_width(1.2));
            }

            // Active frame on top
            if current_frame_idx < frame_count {
                let frame = &cache.cached_frames[current_frame_idx];
                let depth = current_frame_idx as f32 / frame_count.max(1) as f32;
                let perspective_x = depth * 80.0;
                let perspective_y = -depth * 80.0;

                let mut path = vg::Path::new();
                for (i, &sample) in frame.iter().enumerate() {
                    let normalized = (sample - global_min) / range;
                    let x = bounds.x + padding
                        + (i as f32 / frame_size as f32) * (width * 0.7)
                        + perspective_x;
                    let y = bounds.y + bounds.h - padding * 2.0
                        - (normalized * height * 0.4)
                        + perspective_y;
                    if i == 0 { path.move_to(x, y); } else { path.line_to(x, y); }
                }

                canvas.stroke_path(
                    &path,
                    &vg::Paint::color(vg::Color::rgba(255, 200, 100, 255)).with_line_width(2.5),
                );
            }

            // Grid: horizontal center line
            let mut grid_path = vg::Path::new();
            grid_path.move_to(bounds.x + padding, bounds.y + padding + height / 2.0);
            grid_path.line_to(bounds.x + padding + width, bounds.y + padding + height / 2.0);
            canvas.stroke_path(
                &grid_path,
                &vg::Paint::color(vg::Color::rgba(80, 80, 90, 100)).with_line_width(0.5),
            );
        }

        drop(cache);
    }

    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| {
            if let WindowEvent::MouseDown(MouseButton::Left) = window_event {
                self.show_2d.set(!self.show_2d.get());
                meta.consume();
            }
        });
    }
}
