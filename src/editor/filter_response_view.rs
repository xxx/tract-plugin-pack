use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use std::sync::Arc;

use crate::WavetableFilterParams;

pub struct FilterResponseView {
    params: Arc<WavetableFilterParams>,
    shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
}

impl FilterResponseView {
    pub fn new<'a>(
        cx: &'a mut Context,
        params: Arc<WavetableFilterParams>,
        shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    ) -> Handle<'a, Self> {
        Self {
            params,
            shared_wavetable,
        }
        .build(cx, |_cx| {})
    }
}

impl View for FilterResponseView {
    fn element(&self) -> Option<&'static str> {
        Some("filter-response-view")
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

        let padding = 20.0;
        let width = bounds.w - padding * 2.0;
        let height = bounds.h - padding * 2.0;

        // Get current wavetable and frame position
        let frame = {
            let Ok(wavetable) = self.shared_wavetable.lock() else {
                return;
            };
            let frame_pos = self.params.frame_position.unmodulated_normalized_value();
            wavetable.get_frame_interpolated(frame_pos)
        };

        if frame.is_empty() {
            return;
        }

        // Draw frequency response grid
        let mut grid_paint = vg::Paint::color(vg::Color::rgba(80, 80, 90, 100));
        grid_paint.set_line_width(0.5);

        // Horizontal grid lines (dB levels)
        for i in 0..5 {
            let y = bounds.y + padding + (i as f32 / 4.0) * height;
            let mut path = vg::Path::new();
            path.move_to(bounds.x + padding, y);
            path.line_to(bounds.x + padding + width, y);
            canvas.stroke_path(&path, &grid_paint);
        }

        // Vertical grid lines (frequency markers)
        for i in 0..9 {
            let x = bounds.x + padding + (i as f32 / 8.0) * width;
            let mut path = vg::Path::new();
            path.move_to(x, bounds.y + padding);
            path.line_to(x, bounds.y + padding + height);
            canvas.stroke_path(&path, &grid_paint);
        }

        // Draw filter kernel shape (which represents the frequency response)
        let mut response_path = vg::Path::new();

        // Find min and max for proper scaling
        let min_val = frame.iter().copied().fold(f32::INFINITY, f32::min);
        let max_val = frame.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let range = (max_val - min_val).max(0.001);

        for (i, &value) in frame.iter().enumerate() {
            let x = bounds.x + padding + (i as f32 / frame.len() as f32) * width;

            // Normalize the value to 0-1 range
            let normalized = (value - min_val) / range;

            // Map to screen coordinates (flip Y so higher values are at top)
            let y = bounds.y + padding + height - (normalized * height * 0.8) - height * 0.1;

            if i == 0 {
                response_path.move_to(x, y);
            } else {
                response_path.line_to(x, y);
            }
        }

        // Draw the response curve
        canvas.stroke_path(
            &response_path,
            &vg::Paint::color(vg::Color::rgb(100, 200, 255)).with_line_width(2.0),
        );

        // Draw frequency labels
        let mut text_paint = vg::Paint::color(vg::Color::rgb(150, 150, 150));
        text_paint.set_font_size(10.0);
        text_paint.set_text_align(vg::Align::Center);

        let freq_labels = ["20Hz", "100Hz", "500Hz", "1kHz", "5kHz", "10kHz", "20kHz"];
        for (i, label) in freq_labels.iter().enumerate() {
            let x = bounds.x + padding + (i as f32 / 6.0) * width;
            let _ = canvas.fill_text(x, bounds.y + bounds.h - 5.0, label, &text_paint);
        }

        // Draw current frequency marker
        let freq = self.params.frequency.unmodulated_normalized_value();
        let freq_x = bounds.x + padding + freq * width;

        let mut marker_path = vg::Path::new();
        marker_path.move_to(freq_x, bounds.y + padding);
        marker_path.line_to(freq_x, bounds.y + padding + height);

        canvas.stroke_path(
            &marker_path,
            &vg::Paint::color(vg::Color::rgba(255, 100, 100, 200)).with_line_width(2.0),
        );
    }

    fn event(&mut self, _cx: &mut EventContext, _event: &mut Event) {
        // TODO: Handle mouse interaction for frequency adjustment
    }
}
