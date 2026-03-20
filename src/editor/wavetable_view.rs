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
    show_2d: Cell<bool>,
}

struct FrameCache {
    cached_frames: Vec<Vec<f32>>,
    cached_version: u32,
    cached_frame_count: usize,
    cached_frame_size: usize,
}

impl FrameCache {
    fn new() -> Self {
        Self {
            cached_frames: Vec::new(),
            cached_version: u32::MAX, // Forces initial load
            cached_frame_count: 0,
            cached_frame_size: 0,
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
                }
                // If lock is contended, we keep the stale cached data
            }
        }

        let cache = self.frame_cache.borrow();
        let frame_count = cache.cached_frame_count;
        let frame_size = cache.cached_frame_size;
        let frames_data = &cache.cached_frames;

        if frame_count == 0 || frame_size == 0 {
            return;
        }

        // Find global min/max for consistent scaling
        let mut global_min = f32::INFINITY;
        let mut global_max = f32::NEG_INFINITY;
        for frame in frames_data {
            for &sample in frame {
                global_min = global_min.min(sample);
                global_max = global_max.max(sample);
            }
        }
        let range = (global_max - global_min).max(0.001);

        // Use modulated value so the view reflects DAW automation/modulation
        let current_frame_pos = self.params.frame_position.modulated_normalized_value();
        let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;

        if self.show_2d.get() {
            // === 2D face-on view of the current interpolated frame ===
            let exact_pos = current_frame_pos * (frame_count - 1) as f32;
            let lo = (exact_pos.floor() as usize).min(frame_count - 1);
            let hi = (lo + 1).min(frame_count - 1);
            let frac = exact_pos - lo as f32;

            let frame_lo = &frames_data[lo];
            let frame_hi = &frames_data[hi];

            let mut fmin = f32::INFINITY;
            let mut fmax = f32::NEG_INFINITY;
            let num_points = (width as usize).min(frame_size).max(1);
            for pi in 0..num_points {
                let si = ((pi as f32 / num_points as f32) * frame_size as f32) as usize;
                let si = si.min(frame_size - 1);
                let s = frame_lo[si] * (1.0 - frac) + frame_hi[si] * frac;
                fmin = fmin.min(s);
                fmax = fmax.max(s);
            }
            let frange = (fmax - fmin).max(0.001);

            let x0 = bounds.x + padding;
            let y0 = bounds.y + padding;
            let zero_y = y0 + height / 2.0;

            let mut fill_path = vg::Path::new();
            let mut stroke_path = vg::Path::new();
            fill_path.move_to(x0, zero_y);

            for pi in 0..num_points {
                let t = pi as f32 / num_points as f32;
                let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
                let s = frame_lo[si] * (1.0 - frac) + frame_hi[si] * frac;
                let normalized = (s - fmin) / frange;
                let x = x0 + t * width;
                let y = y0 + height - normalized * height;

                fill_path.line_to(x, y);
                if pi == 0 { stroke_path.move_to(x, y); } else { stroke_path.line_to(x, y); }
            }

            fill_path.line_to(x0 + width, zero_y);
            fill_path.close();

            canvas.fill_path(&fill_path, &vg::Paint::color(vg::Color::rgba(79, 195, 247, 30)));
            canvas.stroke_path(
                &stroke_path,
                &vg::Paint::color(vg::Color::rgba(79, 195, 247, 220)).with_line_width(1.5),
            );

            let mut zp = vg::Path::new();
            zp.move_to(x0, zero_y);
            zp.line_to(x0 + width, zero_y);
            canvas.stroke_path(&zp, &vg::Paint::color(vg::Color::rgba(80, 80, 90, 100)).with_line_width(0.5));

            drop(cache);
            return;
        }

        // === 3D overhead perspective view ===
        // Draw all non-active frames first (back to front for proper layering)
        // Overhead view: front frames at lower-left, back frames at upper-right
        for frame_idx in (0..frame_count).rev() {
            if frame_idx == current_frame_idx {
                continue; // Skip active frame, we'll draw it last
            }

            let frame = &frames_data[frame_idx];

            // Calculate depth for overhead perspective
            // depth = 0 (frame 0) at front/lower-left
            // depth = 1 (frame 15) at back/upper-right
            let depth = frame_idx as f32 / frame_count.max(1) as f32;

            // Overhead perspective offsets: move toward upper-right as depth increases
            let perspective_x = depth * 80.0; // Move right
            let perspective_y = -depth * 80.0; // Move up (negative Y)

            let alpha = 0.3 + (1.0 - depth) * 0.4;

            let mut path = vg::Path::new();

            for (i, &sample) in frame.iter().enumerate() {
                // Normalize sample to 0-1 range
                let normalized = (sample - global_min) / range;

                // Overhead view: waveform height represents the "height" of the wave
                // X position: waveform position + perspective offset to the right
                // Y position: waveform amplitude + perspective offset upward
                let x = bounds.x
                    + padding
                    + (i as f32 / frame_size as f32) * (width * 0.7)
                    + perspective_x;

                let y = bounds.y
                    + bounds.h - padding * 2.0  // Start from bottom
                    - (normalized * height * 0.4)  // Wave amplitude
                    + perspective_y; // Move up for depth

                if i == 0 {
                    path.move_to(x, y);
                } else {
                    path.line_to(x, y);
                }
            }

            let color = vg::Color::rgba(
                (50.0 + (1.0 - depth) * 100.0) as u8,
                (100.0 + (1.0 - depth) * 100.0) as u8,
                255,
                (alpha * 255.0) as u8,
            );

            canvas.stroke_path(&path, &vg::Paint::color(color).with_line_width(1.2));
        }

        // Draw the active frame last so it's always on top
        if current_frame_idx < frame_count {
            let frame = &frames_data[current_frame_idx];
            let depth = current_frame_idx as f32 / frame_count.max(1) as f32;
            let perspective_x = depth * 80.0;
            let perspective_y = -depth * 80.0;

            let mut path = vg::Path::new();

            for (i, &sample) in frame.iter().enumerate() {
                let normalized = (sample - global_min) / range;

                let x = bounds.x
                    + padding
                    + (i as f32 / frame_size as f32) * (width * 0.7)
                    + perspective_x;

                let y = bounds.y + bounds.h - padding * 2.0 - (normalized * height * 0.4)
                    + perspective_y;

                if i == 0 {
                    path.move_to(x, y);
                } else {
                    path.line_to(x, y);
                }
            }

            let color = vg::Color::rgba(255, 200, 100, 255); // Bright orange for active
            canvas.stroke_path(&path, &vg::Paint::color(color).with_line_width(2.5));
        }

        // Drop the borrow before drawing grid (doesn't need frame data)
        drop(cache);

        // Draw grid lines
        let mut grid_paint = vg::Paint::color(vg::Color::rgba(80, 80, 90, 100));
        grid_paint.set_line_width(0.5);

        // Horizontal center line
        let mut path = vg::Path::new();
        path.move_to(bounds.x + padding, bounds.y + padding + height / 2.0);
        path.line_to(
            bounds.x + padding + width,
            bounds.y + padding + height / 2.0,
        );
        canvas.stroke_path(&path, &grid_paint);
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
