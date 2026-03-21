use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use realfft::RealFftPlanner;
use std::cell::RefCell;
use std::sync::Arc;

use crate::WavetableFilterParams;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const DB_CEIL: f32 = 0.0;
const DB_FLOOR: f32 = -48.0;
const DB_RANGE: f32 = DB_CEIL - DB_FLOOR; // 48 dB total

/// Epsilon for float comparison to detect parameter changes.
const PARAM_EPSILON: f32 = 0.001;

pub struct FilterResponseView {
    params: Arc<WavetableFilterParams>,
    shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    shared_input_spectrum: Arc<std::sync::Mutex<(f32, Vec<f32>)>>,
    /// Cached FFT magnitude results and the planner/scratch buffers.
    fft_cache: RefCell<FftCache>,
}

struct FftCache {
    planner: RealFftPlanner<f32>,
    frame_buf: Vec<f32>,
    spectrum: Vec<rustfft::num_complex::Complex<f32>>,
    cached_mags: Vec<f32>,
    cached_frame_pos: f32,
    cached_cutoff: f32,
    cached_resonance: f32,
    /// Precomputed log-spaced frequency table (one per pixel column).
    freq_table: Vec<f32>,
    freq_table_size: usize,
    /// Cached response curve Y-coordinates (recomputed only when params change).
    cached_response_ys: Vec<f32>,
}

impl FftCache {
    fn new() -> Self {
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
        }
    }
}

impl FilterResponseView {
    pub fn new<'a>(
        cx: &'a mut Context,
        params: Arc<WavetableFilterParams>,
        shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
        shared_input_spectrum: Arc<std::sync::Mutex<(f32, Vec<f32>)>>,
    ) -> Handle<'a, Self> {
        Self {
            params,
            shared_wavetable,
            shared_input_spectrum,
            fft_cache: RefCell::new(FftCache::new()),
        }
        .build(cx, |_cx| {})
    }

    /// Recompute the FFT magnitudes if parameters changed, otherwise reuse cached data.
    /// Returns false if there is no valid magnitude data available.
    fn update_cached_mags(&self, frame_position: f32, cutoff_hz: f32, resonance: f32) -> bool {
        let mut cache = self.fft_cache.borrow_mut();

        // Use coarser threshold for frame_position to avoid FFT on every draw during sweeps.
        // 0.01 in normalized 0-1 space ≈ 2-3 frames for a 256-frame wavetable.
        let needs_update = (cache.cached_frame_pos - frame_position).abs() > 0.01
            || (cache.cached_cutoff - cutoff_hz).abs() > PARAM_EPSILON
            || (cache.cached_resonance - resonance).abs() > PARAM_EPSILON
            || cache.cached_mags.is_empty();

        if !needs_update {
            return !cache.cached_mags.is_empty();
        }

        // Try to acquire the wavetable lock without blocking the GUI thread
        let frame_n = match self.shared_wavetable.try_lock() {
            Ok(wt) => {
                if wt.frame_count == 0 || wt.frame_size == 0 {
                    return !cache.cached_mags.is_empty();
                }
                let n = wt.frame_size;
                // Resize and interpolate directly into cache buffer (no allocation)
                cache.frame_buf.resize(n, 0.0);
                wt.interpolate_frame_into(frame_position, &mut cache.frame_buf);
                n
            }
            Err(_) => {
                // Lock contended — use stale cached data
                return !cache.cached_mags.is_empty();
            }
        };

        let fft = cache.planner.plan_fft_forward(frame_n);
        cache
            .spectrum
            .resize(frame_n / 2 + 1, rustfft::num_complex::Complex::new(0.0, 0.0));
        for c in cache.spectrum.iter_mut() {
            *c = rustfft::num_complex::Complex::new(0.0, 0.0);
        }

        // Destructure to satisfy the borrow checker: frame_buf and spectrum are
        // disjoint fields, but Rust cannot prove that through &mut cache.
        let FftCache {
            ref mut frame_buf,
            ref mut spectrum,
            ref mut cached_mags,
            ..
        } = *cache;

        if fft.process(frame_buf, spectrum).is_err() {
            return !cached_mags.is_empty();
        }

        cached_mags.clear();
        cached_mags.extend(spectrum.iter().map(|c| c.norm()));

        // Normalize so peak magnitude = 1.0 (0 dB)
        let peak = cached_mags
            .iter()
            .cloned()
            .fold(0.0f32, f32::max)
            .max(1e-10);
        for m in cached_mags.iter_mut() {
            *m /= peak;
        }

        cache.cached_frame_pos = frame_position;
        cache.cached_cutoff = cutoff_hz;
        cache.cached_resonance = resonance;

        true
    }

    /// Precompute the log-frequency table and response curve Y-coordinates.
    /// Called from draw() — only recomputes when display width or parameters change.
    fn update_response_curve(&self, num_points: usize, cutoff_hz: f32, resonance: f32, height: f32, y0: f32) {
        let mut cache = self.fft_cache.borrow_mut();

        // Rebuild frequency table if display width changed
        if cache.freq_table_size != num_points {
            cache.freq_table.resize(num_points + 1, 0.0);
            let log_min = FREQ_MIN.ln();
            let log_range = FREQ_MAX.ln() - log_min;
            for i in 0..=num_points {
                let x_norm = i as f32 / num_points as f32;
                cache.freq_table[i] = (log_min + x_norm * log_range).exp();
            }
            cache.freq_table_size = num_points;
            // Force response recomputation
            cache.cached_response_ys.clear();
        }

        // Recompute response curve Y-coordinates if needed
        if cache.cached_response_ys.len() != num_points + 1 || cache.cached_mags.is_empty() {
            if cache.cached_mags.is_empty() {
                return;
            }
            let comb_exp = resonance * 8.0;

            // Destructure to satisfy borrow checker (disjoint field borrows)
            let FftCache {
                ref cached_mags,
                ref freq_table,
                ref mut cached_response_ys,
                ..
            } = *cache;

            let max_src = (cached_mags.len() - 1) as f32;
            cached_response_ys.resize(num_points + 1, 0.0);
            for i in 0..=num_points {
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

        // --- Compute frequency response (with caching) ---

        // Use modulated values so the response reflects DAW automation/modulation
        let frame_position = self.params.frame_position.modulated_normalized_value();
        let cutoff_hz = self.params.frequency.modulated_plain_value();
        let resonance = self.params.resonance.modulated_plain_value();

        if !self.update_cached_mags(frame_position, cutoff_hz, resonance) {
            return;
        }

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

        // Point count for spectrum and response curves.
        // Cap at 256 — more than enough for visual quality, reduces femtovg tessellation cost.
        let num_points = (width.max(1.0) as usize).min(256);

        // --- Input spectrum shadow ---
        // Copy data out of the lock immediately to avoid holding it during path construction
        let spectrum_snapshot = self.shared_input_spectrum.try_lock().ok().and_then(|data| {
            let (sr, ref mags) = *data;
            if sr > 0.0 && !mags.is_empty() {
                Some((sr, mags.clone()))
            } else {
                None
            }
        });
        // Ensure frequency table is built before using it
        {
            let mut cache = self.fft_cache.borrow_mut();
            if cache.freq_table_size != num_points {
                cache.freq_table.resize(num_points + 1, 0.0);
                let log_min = FREQ_MIN.ln();
                let log_range = FREQ_MAX.ln() - log_min;
                for i in 0..=num_points {
                    let x_norm = i as f32 / num_points as f32;
                    cache.freq_table[i] = (log_min + x_norm * log_range).exp();
                }
                cache.freq_table_size = num_points;
                cache.cached_response_ys.clear(); // force response recomputation
            }
        }

        if let Some((sr, input_mags)) = spectrum_snapshot {
            // Copy freq_table out so we don't hold the borrow across canvas calls
            let freq_table: Vec<f32> = {
                let cache = self.fft_cache.borrow();
                cache.freq_table[..=num_points].to_vec()
            };
            let bin_hz = sr / (2.0 * (input_mags.len() - 1) as f32);

            let mut shadow_path = vg::Path::new();
            shadow_path.move_to(x0, y0 + height);

            for i in 0..=num_points {
                let freq = freq_table[i];
                let bin = freq / bin_hz;

                let mag = if bin >= (input_mags.len() - 1) as f32 {
                    0.0
                } else if bin <= 0.0 {
                    input_mags[0]
                } else {
                    let lo = bin.floor() as usize;
                    let frac = bin - lo as f32;
                    input_mags[lo] * (1.0 - frac) + input_mags[lo + 1] * frac
                };

                let db = 20.0 * mag.max(1e-6).log10();
                let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
                let x = x0 + (i as f32 / num_points as f32) * width;
                let y = y0 + height - y_norm * height;
                shadow_path.line_to(x, y);
            }

            shadow_path.line_to(x0 + width, y0 + height);
            shadow_path.close();

            canvas.fill_path(
                &shadow_path,
                &vg::Paint::color(vg::Color::rgba(255, 200, 100, 25)),
            );
        }

        // --- Frequency response curve (from cached Y-coordinates) ---
        self.update_response_curve(num_points, cutoff_hz, resonance, height, y0);

        let cache = self.fft_cache.borrow();
        let response_ys = &cache.cached_response_ys;

        let mut fill_path = vg::Path::new();
        let mut stroke_path = vg::Path::new();

        if response_ys.len() == num_points + 1 {
            for i in 0..=num_points {
                let x = x0 + (i as f32 / num_points as f32) * width;
                let y = response_ys[i];

                if i == 0 {
                    fill_path.move_to(x, y0 + height);
                    fill_path.line_to(x, y);
                    stroke_path.move_to(x, y);
                } else {
                    fill_path.line_to(x, y);
                    stroke_path.line_to(x, y);
                }
            }
        }

        drop(cache);

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
        for (db, label) in [(0.0_f32, "0"), (-24.0, "-24"), (-48.0, "-48")] {
            let y_norm = (db - DB_FLOOR) / DB_RANGE;
            let y = y0 + height - y_norm * height;
            let _ = canvas.fill_text(x0 - 3.0, y, label, &text_paint);
        }
    }

    fn event(&mut self, _cx: &mut EventContext, _event: &mut Event) {}
}
