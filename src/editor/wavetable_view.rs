use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use std::sync::Arc;

use crate::{WavetableFilter, WavetableFilterParams};

pub struct WavetableView {
    params: Arc<WavetableFilterParams>,
    shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    last_frame_count: std::cell::Cell<usize>,
}

impl WavetableView {
    pub fn new(
        cx: &mut Context,
        params: Arc<WavetableFilterParams>,
        shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
        _wavetable_version: Arc<std::sync::atomic::AtomicU32>,
    ) -> Handle<Self> {
        // Get initial frame count
        let initial_frame_count = shared_wavetable
            .lock()
            .map(|wt| wt.frame_count)
            .unwrap_or(0);

        Self {
            params,
            shared_wavetable,
            last_frame_count: std::cell::Cell::new(initial_frame_count),
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

        // Get the current wavetable - read it fresh every frame
        let (frames_data, frame_count, frame_size) = {
            let Ok(wavetable) = self.shared_wavetable.lock() else {
                return;
            };

            // Track if wavetable changed
            let current_count = wavetable.frame_count;
            if current_count != self.last_frame_count.get() {
                self.last_frame_count.set(current_count);
                // Force parent to redraw by requesting layout
                // This is a hack but vizia doesn't give us better options
            }

            (
                wavetable.frames.clone(),
                wavetable.frame_count,
                wavetable.frame_size,
            )
        };

        if frame_count == 0 || frame_size == 0 {
            return;
        }

        // Find global min/max for consistent scaling
        let mut global_min = f32::INFINITY;
        let mut global_max = f32::NEG_INFINITY;
        for frame in &frames_data {
            for &sample in frame {
                global_min = global_min.min(sample);
                global_max = global_max.max(sample);
            }
        }
        let range = (global_max - global_min).max(0.001);

        // Get current frame position
        let current_frame_pos = self.params.frame_position.unmodulated_normalized_value();
        let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;

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

    fn event(&mut self, _cx: &mut EventContext, _event: &mut Event) {
        // TODO: Handle mouse interaction for frame selection
    }
}
