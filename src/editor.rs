use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use std::sync::Arc;

use crate::WavetableFilterParams;

// Filter response constants
const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const DB_CEIL: f32 = 0.0;
const DB_FLOOR: f32 = -48.0;
const DB_RANGE: f32 = DB_CEIL - DB_FLOOR;

/// Epsilon for float comparison to detect parameter changes.
const PARAM_EPSILON: f32 = 0.001;

struct EditorState {
    show_2d: bool,
    // FFT cache for filter response
    cached_mags: Vec<f32>,
    cached_frame_pos: f32,
    cached_cutoff: f32,
    cached_resonance: f32,
    fft_planner: RealFftPlanner<f32>,
    fft_frame_buf: Vec<f32>,
    fft_spectrum: Vec<Complex<f32>>,
    /// Precomputed log-spaced frequency table (one entry per pixel column).
    freq_table: Vec<f32>,
    freq_table_size: usize,
    /// Cached response curve Y-coordinates.
    cached_response_ys: Vec<f32>,
}

pub(crate) fn create(
    params: Arc<WavetableFilterParams>,
    wavetable_path: Arc<std::sync::Mutex<String>>,
    should_reload: Arc<std::sync::atomic::AtomicBool>,
    shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    wavetable_version: Arc<std::sync::atomic::AtomicU32>,
    editor_state: Arc<EguiState>,
    shared_input_spectrum: Arc<std::sync::Mutex<(f32, Vec<f32>)>>,
) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        editor_state,
        EditorState {
            show_2d: false,
            cached_mags: Vec::new(),
            cached_frame_pos: -1.0,
            cached_cutoff: -1.0,
            cached_resonance: -1.0,
            fft_planner: RealFftPlanner::new(),
            fft_frame_buf: Vec::new(),
            fft_spectrum: Vec::new(),
            freq_table: Vec::new(),
            freq_table_size: 0,
            cached_response_ys: Vec::new(),
        },
        |egui_ctx, _| {
            egui_ctx.set_visuals(egui::Visuals::dark());
        },
        move |egui_ctx, setter, state| {
            egui::CentralPanel::default().show(egui_ctx, |ui| {
                // Title
                ui.heading("Wavetable Filter");
                ui.add_space(4.0);

                // Wavetable name + Browse button
                ui.horizontal(|ui| {
                    let wt_name = {
                        let path = wavetable_path.lock().unwrap();
                        if path.is_empty() {
                            "No wavetable loaded".to_string()
                        } else {
                            std::path::Path::new(path.as_str())
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| "Unknown".to_string())
                        }
                    };
                    ui.label(format!("Wavetable: {}", wt_name));

                    // Status info: frame count and frame size
                    if let Ok(wt) = shared_wavetable.try_lock() {
                        if wt.frame_count > 0 {
                            ui.label(format!(
                                "{} frames x {} samples",
                                wt.frame_count, wt.frame_size
                            ));
                        }
                    }

                    if ui.button("Browse...").clicked() {
                        let mut dialog = rfd::FileDialog::new()
                            .add_filter("Wavetable files", &["wav", "wt"]);
                        // Set initial directory from current path
                        let current_path = wavetable_path.lock().unwrap().clone();
                        if !current_path.is_empty() {
                            if let Some(dir) = std::path::Path::new(&current_path).parent() {
                                if dir.exists() {
                                    dialog = dialog.set_directory(dir);
                                }
                            }
                        }
                        if let Some(path) = dialog.pick_file() {
                            if let Some(path_str) = path.to_str() {
                                let path_string = path_str.to_string();
                                // Load wavetable
                                match crate::wavetable::Wavetable::from_file(&path_string) {
                                    Ok(new_wavetable) => {
                                        if let Ok(mut shared) = shared_wavetable.lock() {
                                            *shared = new_wavetable;
                                        }
                                        wavetable_version.fetch_add(
                                            1,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        if let Ok(mut wt) = wavetable_path.lock() {
                                            *wt = path_string;
                                        }
                                        should_reload
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                    Err(e) => {
                                        nih_plug::nih_log!("Failed to load wavetable: {}", e);
                                    }
                                }
                            }
                        }
                    }
                });
                ui.add_space(8.0);

                // Mode selector
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    ui.add(
                        nih_plug_egui::widgets::ParamSlider::for_param(&params.mode, setter)
                            .with_width(120.0),
                    );
                });
                ui.add_space(8.0);

                // Parameter row
                ui.horizontal(|ui| {
                    let slider_width = 150.0;

                    ui.vertical(|ui| {
                        ui.label("Frequency");
                        ui.add(
                            nih_plug_egui::widgets::ParamSlider::for_param(
                                &params.frequency,
                                setter,
                            )
                            .with_width(slider_width),
                        );
                    });

                    ui.vertical(|ui| {
                        ui.label("Frame Position");
                        ui.add(
                            nih_plug_egui::widgets::ParamSlider::for_param(
                                &params.frame_position,
                                setter,
                            )
                            .with_width(slider_width),
                        );
                    });

                    ui.vertical(|ui| {
                        ui.label("Resonance");
                        ui.add(
                            nih_plug_egui::widgets::ParamSlider::for_param(
                                &params.resonance,
                                setter,
                            )
                            .with_width(slider_width),
                        );
                    });

                    ui.vertical(|ui| {
                        ui.label("Gain");
                        ui.add(
                            nih_plug_egui::widgets::ParamSlider::for_param(
                                &params.drive,
                                setter,
                            )
                            .with_width(slider_width),
                        );
                    });

                    ui.vertical(|ui| {
                        ui.label("Mix");
                        ui.add(
                            nih_plug_egui::widgets::ParamSlider::for_param(&params.mix, setter)
                                .with_width(slider_width),
                        );
                    });
                });
                ui.add_space(16.0);

                // View areas
                ui.horizontal(|ui| {
                    // Wavetable view
                    ui.vertical(|ui| {
                        draw_wavetable_view(
                            ui,
                            &shared_wavetable,
                            &params,
                            &mut state.show_2d,
                        );
                    });

                    ui.add_space(8.0);

                    // Filter response view
                    ui.vertical(|ui| {
                        draw_filter_response(
                            ui,
                            &params,
                            &shared_wavetable,
                            &shared_input_spectrum,
                            state,
                        );
                    });
                });
            });
        },
    )
}

fn draw_wavetable_view(
    ui: &mut egui::Ui,
    shared_wavetable: &Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    params: &WavetableFilterParams,
    show_2d: &mut bool,
) {
    let size = egui::vec2(500.0, 300.0);
    let (response, painter) = ui.allocate_painter(size, egui::Sense::click());
    let rect = response.rect;

    // Background
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(20, 22, 28));

    // Toggle 2D/3D on click
    if response.clicked() {
        *show_2d = !*show_2d;
    }

    // Try to lock the wavetable; if contended, just show background
    let wt = match shared_wavetable.try_lock() {
        Ok(wt) => wt,
        Err(_) => return,
    };

    if wt.frames.is_empty() {
        return;
    }

    let w = rect.width();
    let h = rect.height();
    let frame_pos = params.frame_position.modulated_normalized_value();
    let frame_size = wt.frame_size;
    let frame_count = wt.frame_count;
    let num_points = (w as usize).min(frame_size).max(1);

    if *show_2d {
        // 2D mode: draw interpolated current frame as a single line
        let frame_data = wt.get_frame_interpolated(frame_pos);
        let points = build_line_points(&frame_data, frame_size, num_points, rect, 0.0, 1.0);
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5, egui::Color32::from_rgba_premultiplied(255, 200, 100, 255)),
        ));
    } else {
        // 3D mode: draw all frames back-to-front with depth-based offset and alpha
        // Active frame index (nearest integer frame)
        let active_frame_float = frame_pos * (frame_count - 1) as f32;
        let active_frame_idx = active_frame_float.round() as usize;

        // Maximum vertical offset for 3D depth effect
        let max_y_offset = h * 0.3;

        // Draw non-active frames back-to-front (last frame = furthest back)
        for i in (0..frame_count).rev() {
            if i == active_frame_idx {
                continue; // draw active frame last (on top)
            }

            let depth = i as f32 / (frame_count - 1).max(1) as f32;
            let y_offset = depth * max_y_offset;
            // Scale down lines farther back
            let y_scale = 1.0 - depth * 0.4;
            // Depth-based alpha: farther back = more transparent
            let alpha = ((1.0 - depth) * 120.0 + 30.0) as u8;

            let points = build_line_points(
                &wt.frames[i],
                frame_size,
                num_points,
                rect,
                -y_offset,
                y_scale,
            );

            let color = egui::Color32::from_rgba_premultiplied(
                (79.0 * (alpha as f32 / 255.0)) as u8,
                (195.0 * (alpha as f32 / 255.0)) as u8,
                (247.0 * (alpha as f32 / 255.0)) as u8,
                alpha,
            );
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.0, color),
            ));
        }

        // Draw active frame on top in orange
        if active_frame_idx < frame_count {
            let depth = active_frame_idx as f32 / (frame_count - 1).max(1) as f32;
            let y_offset = depth * max_y_offset;
            let y_scale = 1.0 - depth * 0.4;

            let points = build_line_points(
                &wt.frames[active_frame_idx],
                frame_size,
                num_points,
                rect,
                -y_offset,
                y_scale,
            );
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(
                    1.5,
                    egui::Color32::from_rgba_premultiplied(255, 200, 100, 255),
                ),
            ));
        }
    }
}

/// Build a polyline of points for one wavetable frame, downsampled to `num_points`.
///
/// `y_offset` shifts the line vertically (for 3D depth effect).
/// `y_scale` scales the waveform amplitude (for perspective shrink).
fn build_line_points(
    frame_data: &[f32],
    frame_size: usize,
    num_points: usize,
    rect: egui::Rect,
    y_offset: f32,
    y_scale: f32,
) -> Vec<egui::Pos2> {
    let w = rect.width();
    let h = rect.height();
    let center_y = rect.center().y + y_offset;
    let amplitude = h * 0.4 * y_scale;

    (0..num_points)
        .map(|i| {
            let t = i as f32 / (num_points - 1).max(1) as f32;
            let sample_idx_f = t * (frame_size - 1) as f32;
            let idx0 = sample_idx_f.floor() as usize;
            let idx1 = (idx0 + 1).min(frame_size - 1);
            let frac = sample_idx_f.fract();
            let sample = frame_data[idx0] + (frame_data[idx1] - frame_data[idx0]) * frac;

            let x = rect.left() + t * w;
            let y = center_y - sample * amplitude;
            egui::pos2(x, y)
        })
        .collect()
}

/// Convert a frequency in Hz to an x position in [0, 1] on a log scale.
fn freq_to_x(freq_hz: f32) -> f32 {
    ((freq_hz.max(FREQ_MIN).ln() - FREQ_MIN.ln()) / (FREQ_MAX.ln() - FREQ_MIN.ln()))
        .clamp(0.0, 1.0)
}

/// Draw the filter frequency response view.
fn draw_filter_response(
    ui: &mut egui::Ui,
    params: &WavetableFilterParams,
    shared_wavetable: &Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    shared_input_spectrum: &Arc<std::sync::Mutex<(f32, Vec<f32>)>>,
    state: &mut EditorState,
) {
    let size = egui::vec2(500.0, 300.0);
    let (response, painter) = ui.allocate_painter(size, egui::Sense::hover());
    let rect = response.rect;

    // Background
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(20, 22, 28));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 70)),
        egui::StrokeKind::Middle,
    );

    let padding = 20.0;
    let width = rect.width() - padding * 2.0;
    let height = rect.height() - padding * 2.0;
    let x0 = rect.left() + padding;
    let y0 = rect.top() + padding;

    // Read modulated parameter values
    let frame_position = params.frame_position.modulated_normalized_value();
    let cutoff_hz = params.frequency.modulated_plain_value();
    let resonance = params.resonance.modulated_plain_value();

    // --- Update cached FFT magnitudes if parameters changed ---
    let has_mags =
        update_cached_mags(state, shared_wavetable, frame_position, cutoff_hz, resonance);

    // --- Grid ---
    let grid_color = egui::Color32::from_rgba_premultiplied(80, 80, 90, 100);
    let grid_stroke = egui::Stroke::new(0.5, grid_color);

    // Horizontal dB lines
    for db in [-12.0_f32, -24.0, -36.0, -48.0] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let y = y0 + height - y_norm * height;
        painter.line_segment(
            [egui::pos2(x0, y), egui::pos2(x0 + width, y)],
            grid_stroke,
        );
    }

    // 0 dB reference line (slightly brighter)
    {
        let y_norm = (0.0_f32 - DB_FLOOR) / DB_RANGE;
        let y = y0 + height - y_norm * height;
        painter.line_segment(
            [egui::pos2(x0, y), egui::pos2(x0 + width, y)],
            egui::Stroke::new(
                0.5,
                egui::Color32::from_rgba_premultiplied(120, 120, 140, 180),
            ),
        );
    }

    // Vertical frequency lines at decade boundaries
    for freq in [100.0_f32, 1000.0, 10000.0] {
        let x = x0 + freq_to_x(freq) * width;
        painter.line_segment(
            [egui::pos2(x, y0), egui::pos2(x, y0 + height)],
            grid_stroke,
        );
    }

    // Cap point count for visual quality vs performance
    let num_points = (width.max(1.0) as usize).min(256);

    // --- Ensure frequency table is up to date ---
    if state.freq_table_size != num_points {
        state.freq_table.resize(num_points + 1, 0.0);
        let log_min = FREQ_MIN.ln();
        let log_range = FREQ_MAX.ln() - log_min;
        for i in 0..=num_points {
            let x_norm = i as f32 / num_points as f32;
            state.freq_table[i] = (log_min + x_norm * log_range).exp();
        }
        state.freq_table_size = num_points;
        state.cached_response_ys.clear();
    }

    // --- Input spectrum shadow ---
    let spectrum_snapshot = shared_input_spectrum.try_lock().ok().and_then(|data| {
        let (sr, ref mags) = *data;
        if sr > 0.0 && !mags.is_empty() {
            Some((sr, mags.clone()))
        } else {
            None
        }
    });

    if let Some((sr, input_mags)) = spectrum_snapshot {
        let bin_hz = sr / (2.0 * (input_mags.len() - 1) as f32);

        let mut fill_points: Vec<egui::Pos2> = Vec::with_capacity(num_points + 3);
        fill_points.push(egui::pos2(x0, y0 + height));

        for i in 0..=num_points {
            let freq = state.freq_table[i];
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
            fill_points.push(egui::pos2(x, y));
        }

        fill_points.push(egui::pos2(x0 + width, y0 + height));

        // Draw as a filled polygon (input spectrum shadow)
        painter.add(egui::Shape::convex_polygon(
            fill_points,
            egui::Color32::from_rgba_premultiplied(255, 200, 100, 25),
            egui::Stroke::NONE,
        ));
    }

    // --- Frequency response curve ---
    if has_mags {
        update_response_curve(state, num_points, cutoff_hz, resonance, height, y0);

        if state.cached_response_ys.len() == num_points + 1 {
            let mut fill_points: Vec<egui::Pos2> = Vec::with_capacity(num_points + 3);
            let mut stroke_points: Vec<egui::Pos2> = Vec::with_capacity(num_points + 1);

            fill_points.push(egui::pos2(x0, y0 + height));

            for i in 0..=num_points {
                let x = x0 + (i as f32 / num_points as f32) * width;
                let y = state.cached_response_ys[i];
                fill_points.push(egui::pos2(x, y));
                stroke_points.push(egui::pos2(x, y));
            }

            fill_points.push(egui::pos2(x0 + width, y0 + height));

            // Filled area under the curve
            painter.add(egui::Shape::convex_polygon(
                fill_points,
                egui::Color32::from_rgba_premultiplied(100, 200, 255, 40),
                egui::Stroke::NONE,
            ));

            // Stroke line on top
            painter.add(egui::Shape::line(
                stroke_points,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 200, 255)),
            ));
        }
    }

    // --- Cutoff marker ---
    let cutoff_x = x0 + freq_to_x(cutoff_hz) * width;
    painter.line_segment(
        [
            egui::pos2(cutoff_x, y0),
            egui::pos2(cutoff_x, y0 + height),
        ],
        egui::Stroke::new(
            2.0,
            egui::Color32::from_rgba_premultiplied(255, 100, 100, 200),
        ),
    );

    // --- Frequency labels ---
    let label_color = egui::Color32::from_rgb(150, 150, 150);
    let font = egui::FontId::proportional(10.0);

    for (freq, label) in [
        (50.0_f32, "50"),
        (200.0, "200"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (20000.0, "20k"),
    ] {
        let x = x0 + freq_to_x(freq) * width;
        painter.text(
            egui::pos2(x, rect.bottom() - 5.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            font.clone(),
            label_color,
        );
    }

    // dB labels on left axis
    for (db, label) in [(0.0_f32, "0"), (-24.0, "-24"), (-48.0, "-48")] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let y = y0 + height - y_norm * height;
        painter.text(
            egui::pos2(x0 - 3.0, y),
            egui::Align2::RIGHT_CENTER,
            label,
            font.clone(),
            label_color,
        );
    }
}

/// Recompute FFT magnitudes from the wavetable frame if parameters changed.
/// Returns true if valid magnitude data is available.
fn update_cached_mags(
    state: &mut EditorState,
    shared_wavetable: &Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    frame_position: f32,
    cutoff_hz: f32,
    resonance: f32,
) -> bool {
    // Use coarser threshold for frame_position to avoid FFT on every draw during sweeps.
    let needs_update = (state.cached_frame_pos - frame_position).abs() > 0.01
        || (state.cached_cutoff - cutoff_hz).abs() > PARAM_EPSILON
        || (state.cached_resonance - resonance).abs() > PARAM_EPSILON
        || state.cached_mags.is_empty();

    if !needs_update {
        return !state.cached_mags.is_empty();
    }

    // Try to acquire the wavetable lock without blocking the GUI thread
    let frame_n = match shared_wavetable.try_lock() {
        Ok(wt) => {
            if wt.frame_count == 0 || wt.frame_size == 0 {
                return !state.cached_mags.is_empty();
            }
            let n = wt.frame_size;
            wt.interpolate_frame_into(frame_position, &mut state.fft_frame_buf);
            n
        }
        Err(_) => {
            // Lock contended -- use stale cached data
            return !state.cached_mags.is_empty();
        }
    };

    let fft = state.fft_planner.plan_fft_forward(frame_n);
    state
        .fft_spectrum
        .resize(frame_n / 2 + 1, Complex::new(0.0, 0.0));
    for c in state.fft_spectrum.iter_mut() {
        *c = Complex::new(0.0, 0.0);
    }

    if fft
        .process(&mut state.fft_frame_buf, &mut state.fft_spectrum)
        .is_err()
    {
        return !state.cached_mags.is_empty();
    }

    state.cached_mags.clear();
    state
        .cached_mags
        .extend(state.fft_spectrum.iter().map(|c| c.norm()));

    // Normalize so peak magnitude = 1.0 (0 dB)
    let peak = state
        .cached_mags
        .iter()
        .cloned()
        .fold(0.0f32, f32::max)
        .max(1e-10);
    for m in state.cached_mags.iter_mut() {
        *m /= peak;
    }

    state.cached_frame_pos = frame_position;
    state.cached_cutoff = cutoff_hz;
    state.cached_resonance = resonance;
    // Force response curve recomputation
    state.cached_response_ys.clear();

    true
}

/// Precompute the response curve Y-coordinates from cached magnitudes.
fn update_response_curve(
    state: &mut EditorState,
    num_points: usize,
    cutoff_hz: f32,
    resonance: f32,
    height: f32,
    y0: f32,
) {
    // Already computed and valid
    if state.cached_response_ys.len() == num_points + 1 || state.cached_mags.is_empty() {
        return;
    }

    let comb_exp = resonance * 8.0;
    let max_src = (state.cached_mags.len() - 1) as f32;

    state.cached_response_ys.resize(num_points + 1, 0.0);
    for i in 0..=num_points {
        let freq = state.freq_table[i];
        let src = freq * 24.0 / cutoff_hz;

        let mag = if src >= max_src {
            0.0
        } else if src <= 0.0 {
            state.cached_mags[0]
        } else {
            let lo = src.floor() as usize;
            let frac = src - lo as f32;
            let interp =
                state.cached_mags[lo] * (1.0 - frac) + state.cached_mags[lo + 1] * frac;
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
        state.cached_response_ys[i] = y0 + height - y_norm * height;
    }
}
