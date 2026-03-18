use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use realfft::RealFftPlanner;
use std::sync::Arc;

use crate::WavetableFilterParams;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const DB_CEIL: f32 = 12.0;
const DB_FLOOR: f32 = -48.0;
const DB_RANGE: f32 = DB_CEIL - DB_FLOOR; // 60 dB total

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

/// Convert a frequency in Hz to an x position in [0, 1] on a log scale.
fn freq_to_x(freq_hz: f32) -> f32 {
    ((freq_hz.max(FREQ_MIN).ln() - FREQ_MIN.ln()) / (FREQ_MAX.ln() - FREQ_MIN.ln()))
        .clamp(0.0, 1.0)
}

impl View for FilterResponseView {
    fn element(&self) -> Option<&'static str> {
        Some("filter-response-view")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();

        // Background
        let mut bg_path = vg::Path::new();
        bg_path.rect(bounds.x, bounds.y, bounds.w, bounds.h);
        canvas.fill_path(&bg_path, &vg::Paint::color(vg::Color::rgb(20, 22, 28)));
        canvas.stroke_path(
            &bg_path,
            &vg::Paint::color(vg::Color::rgb(60, 60, 70)).with_line_width(1.0),
        );

        let padding = 20.0;
        let width = bounds.w - padding * 2.0;
        let height = bounds.h - padding * 2.0;
        let x0 = bounds.x + padding;
        let y0 = bounds.y + padding;

        // --- Compute frequency response ---

        let frame = {
            let Ok(wt) = self.shared_wavetable.lock() else {
                return;
            };
            let pos = self.params.frame_position.unmodulated_normalized_value();
            wt.get_frame_interpolated(pos)
        };

        if frame.is_empty() {
            return;
        }

        // FFT the frame to get magnitude spectrum
        let frame_n = frame.len();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(frame_n);
        let mut frame_buf = frame.clone();
        let mut spectrum = vec![rustfft::num_complex::Complex::new(0.0_f32, 0.0); frame_n / 2 + 1];
        if fft.process(&mut frame_buf, &mut spectrum).is_err() {
            return;
        }
        let mut mags: Vec<f32> = spectrum.iter().map(|c| c.norm()).collect();

        // Normalize so peak magnitude = 1.0 (0 dB)
        let peak = mags.iter().cloned().fold(0.0f32, f32::max).max(1e-10);
        for m in &mut mags {
            *m /= peak;
        }

        // Apply resonance — Gaussian multiplicative boost at harmonic 24, same as synthesize_kernel
        let resonance = self.params.resonance.unmodulated_plain_value();
        if resonance > 0.001 {
            let sigma = 3.0_f32;
            let gain = resonance * 3.0;
            for (k, m) in mags.iter_mut().enumerate() {
                let dist = k as f32 - 24.0;
                let gaussian = (-dist * dist / (2.0 * sigma * sigma)).exp();
                *m *= 1.0 + gain * gaussian;
            }
            // No renormalization — peak can exceed 1.0 (shown as positive dB)
        }

        let cutoff_hz = self.params.frequency.unmodulated_plain_value();

        // --- Grid ---

        let mut grid_paint = vg::Paint::color(vg::Color::rgba(80, 80, 90, 100));
        grid_paint.set_line_width(0.5);

        // Horizontal dB lines
        for db in [-12.0_f32, -24.0, -36.0, -48.0] {
            let y_norm = (db - DB_FLOOR) / DB_RANGE;
            let y = y0 + height - y_norm * height;
            let mut path = vg::Path::new();
            path.move_to(x0, y);
            path.line_to(x0 + width, y);
            canvas.stroke_path(&path, &grid_paint);
        }

        // 0 dB reference line — slightly brighter
        {
            let y_norm = (0.0_f32 - DB_FLOOR) / DB_RANGE;
            let y = y0 + height - y_norm * height;
            let mut path = vg::Path::new();
            path.move_to(x0, y);
            path.line_to(x0 + width, y);
            let mut ref_paint = vg::Paint::color(vg::Color::rgba(120, 120, 140, 180));
            ref_paint.set_line_width(0.5);
            canvas.stroke_path(&path, &ref_paint);
        }

        // Vertical frequency lines at decade boundaries
        for freq in [100.0_f32, 1000.0, 10000.0] {
            let x = x0 + freq_to_x(freq) * width;
            let mut path = vg::Path::new();
            path.move_to(x, y0);
            path.line_to(x, y0 + height);
            canvas.stroke_path(&path, &grid_paint);
        }

        // --- Frequency response curve ---
        // For each screen pixel column, compute:
        //   freq = frequency at that x position (log scale)
        //   src_harmonic = freq * 24 / cutoff_hz  (harmonic 24 maps to cutoff)
        //   magnitude = interpolate mags[] at src_harmonic
        //   dB = 20 * log10(magnitude)

        let num_points = width.max(1.0) as usize;
        let max_src = (mags.len() - 1) as f32;

        let mut fill_path = vg::Path::new();
        let mut stroke_path = vg::Path::new();

        for i in 0..=num_points {
            let x_norm = i as f32 / num_points as f32;
            let freq = FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf(x_norm);
            let src = freq * 24.0 / cutoff_hz;

            let mag = if src >= max_src {
                0.0
            } else if src <= 0.0 {
                mags[0]
            } else {
                let lo = src.floor() as usize;
                let frac = src - lo as f32;
                mags[lo] * (1.0 - frac) + mags[lo + 1] * frac
            };

            let db = 20.0 * mag.max(1e-6).log10();
            let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
            let x = x0 + x_norm * width;
            let y = y0 + height - y_norm * height;

            if i == 0 {
                fill_path.move_to(x, y0 + height); // start at bottom-left
                fill_path.line_to(x, y);
                stroke_path.move_to(x, y);
            } else {
                fill_path.line_to(x, y);
                stroke_path.line_to(x, y);
            }
        }

        // Close the fill path along the bottom
        fill_path.line_to(x0 + width, y0 + height);
        fill_path.close();

        canvas.fill_path(
            &fill_path,
            &vg::Paint::color(vg::Color::rgba(100, 200, 255, 40)),
        );
        canvas.stroke_path(
            &stroke_path,
            &vg::Paint::color(vg::Color::rgb(100, 200, 255)).with_line_width(2.0),
        );

        // --- Cutoff marker ---
        let cutoff_x = x0 + freq_to_x(cutoff_hz) * width;
        let mut marker_path = vg::Path::new();
        marker_path.move_to(cutoff_x, y0);
        marker_path.line_to(cutoff_x, y0 + height);
        canvas.stroke_path(
            &marker_path,
            &vg::Paint::color(vg::Color::rgba(255, 100, 100, 200)).with_line_width(2.0),
        );

        // --- Frequency labels ---
        let mut text_paint = vg::Paint::color(vg::Color::rgb(150, 150, 150));
        text_paint.set_font_size(10.0);
        text_paint.set_text_align(vg::Align::Center);

        for (freq, label) in [
            (50.0_f32, "50"),
            (200.0, "200"),
            (1000.0, "1k"),
            (5000.0, "5k"),
            (20000.0, "20k"),
        ] {
            let x = x0 + freq_to_x(freq) * width;
            let _ = canvas.fill_text(x, bounds.y + bounds.h - 5.0, label, &text_paint);
        }

        // dB labels on left axis
        text_paint.set_text_align(vg::Align::Right);
        for (db, label) in [(12.0_f32, "+12"), (0.0, "0"), (-24.0, "-24"), (-48.0, "-48")] {
            let y_norm = (db - DB_FLOOR) / DB_RANGE;
            let y = y0 + height - y_norm * height;
            let _ = canvas.fill_text(x0 - 3.0, y, label, &text_paint);
        }
    }

    fn event(&mut self, _cx: &mut EventContext, _event: &mut Event) {}
}
