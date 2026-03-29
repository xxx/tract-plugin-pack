//! Softbuffer-based editor for gs-meter. CPU rendering via tiny-skia, no GPU required.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tiny_skia_widgets as widgets;

use crate::{GsMeterParams, MeterReadings};

const WINDOW_WIDTH: u32 = 420;
const WINDOW_HEIGHT: u32 = 540;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Window Handler ──────────────────────────────────────────────────────

/// A rectangular hit region with an associated action.
#[derive(Clone)]
struct HitRegion {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    action: HitAction,
}

#[derive(Clone, Copy, PartialEq)]
enum HitAction {
    Slider(ParamId),
    SteppedSegment { param: ParamId, index: i32 },
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq)]
enum ParamId {
    Gain,
    Reference,
    RmsWindow,
    ChannelMode,
    MeterMode,
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonAction {
    Reset,
    GainFromReading(GainSource),
    ScaleDown,
    ScaleUp,
}

#[derive(Clone, Copy, PartialEq)]
enum GainSource {
    PeakMax,
    TruePeak,
    RmsIntegrated,
    RmsMomentary,
    RmsMomentaryMax,
    // LUFS mode sources
    LufsIntegrated,
    LufsShortTerm,
    LufsShortTermMax,
    LufsMomentary,
    LufsMomentaryMax,
    LufsTruePeak,
}

struct GsMeterWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Shared with GsMeterEditor so Editor::size() stays in sync.
    shared_scale: Arc<AtomicCell<f32>>,

    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
    text_renderer: widgets::TextRenderer,

    /// Hit regions rebuilt each frame during draw().
    hit_regions: Vec<HitRegion>,
    /// Currently dragging a slider.
    drag_active: Option<ParamId>,
    /// Mouse position in physical pixels.
    mouse_x: f32,
    mouse_y: f32,
    /// Timestamp of last click for double-click detection.
    last_click_time: std::time::Instant,
    last_click_action: Option<HitAction>,
}

impl GsMeterWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<GsMeterParams>,
        readings: Arc<MeterReadings>,
        should_reset: Arc<AtomicBool>,
        shared_scale: Arc<AtomicCell<f32>>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        // Load embedded font
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            params,
            readings,
            should_reset,
            text_renderer,
            hit_regions: Vec::new(),
            drag_active: None,
            mouse_x: 0.0,
            mouse_y: 0.0,
            last_click_time: std::time::Instant::now(),
            last_click_action: None,
        }
    }

    fn active_gain(&self) -> &FloatParam {
        if self.params.meter_mode.value() == crate::MeterMode::Lufs {
            &self.params.gain_lufs
        } else {
            &self.params.gain
        }
    }

    fn active_reference(&self) -> &FloatParam {
        if self.params.meter_mode.value() == crate::MeterMode::Lufs {
            &self.params.reference_lufs
        } else {
            &self.params.reference_level
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;

        // Clear hit regions and background
        self.hit_regions.clear();
        self.surface.pixmap.fill(widgets::color_bg());

        let pad = 20.0 * s;
        let row_h = 35.0 * s;
        let label_w = 100.0 * s;
        let font_size = 14.0 * s;
        let title_size = 24.0 * s;
        let slider_w = 200.0 * s;
        let slider_h = 26.0 * s;
        let value_w = 120.0 * s;
        let btn_w = 70.0 * s;
        let btn_h = 24.0 * s;
        let gap = 10.0 * s;

        let mut y = 15.0 * s;
        let tr = &mut self.text_renderer;

        // Title row with scale controls on the right
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + title_size,
            "GS Meter",
            title_size,
            widgets::color_text(),
        );

        let scale_btn_size = 24.0 * s;
        let scale_label_w = 48.0 * s;
        let small_font = 11.0 * s;
        let w = self.physical_width as f32;

        // "+" button (rightmost)
        let plus_x = w - pad - scale_btn_size;
        let plus_y = y + 4.0 * s;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            plus_x,
            plus_y,
            scale_btn_size,
            scale_btn_size,
            "+",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: plus_x,
            y: plus_y,
            w: scale_btn_size,
            h: scale_btn_size,
            action: HitAction::Button(ButtonAction::ScaleUp),
        });

        // Scale percentage label
        let pct_text = format!("{}%", (self.scale_factor * 100.0).round() as u32);
        let pct_x = plus_x - scale_label_w;
        let pct_text_w = tr.text_width(&pct_text, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            pct_x + (scale_label_w - pct_text_w) / 2.0,
            plus_y + small_font + 4.0 * s,
            &pct_text,
            small_font,
            widgets::color_muted(),
        );

        // "-" button
        let minus_x = pct_x - scale_btn_size;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            minus_x,
            plus_y,
            scale_btn_size,
            scale_btn_size,
            "-",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: minus_x,
            y: plus_y,
            w: scale_btn_size,
            h: scale_btn_size,
            action: HitAction::Button(ButtonAction::ScaleDown),
        });

        y += row_h;

        // Read meter values
        let peak_db = MeterReadings::load_db(&self.readings.peak_max_db);
        let true_peak_db = MeterReadings::load_db(&self.readings.true_peak_max_db);
        let rms_int_db = MeterReadings::load_db(&self.readings.rms_integrated_db);
        let rms_mom_db = MeterReadings::load_db(&self.readings.rms_momentary_db);
        let rms_max_db = MeterReadings::load_db(&self.readings.rms_momentary_max_db);
        let crest_db = MeterReadings::load_db(&self.readings.crest_factor_db);

        // Channel mode selector
        let mode_idx = match self.params.channel_mode.value() {
            crate::ChannelMode::Stereo => 0,
            crate::ChannelMode::Left => 1,
            crate::ChannelMode::Right => 2,
        };
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + font_size,
            "Channel",
            font_size,
            widgets::color_muted(),
        );
        let sel_x = pad + label_w;
        let sel_y = y + 4.0 * s;
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            tr,
            sel_x,
            sel_y,
            slider_w,
            slider_h,
            &["Stereo", "Left", "Right"],
            mode_idx,
        );
        // Register hit regions for each segment
        let seg_w = slider_w / 3.0;
        for i in 0..3 {
            self.hit_regions.push(HitRegion {
                x: sel_x + i as f32 * seg_w,
                y: sel_y,
                w: seg_w,
                h: slider_h,
                action: HitAction::SteppedSegment {
                    param: ParamId::ChannelMode,
                    index: i,
                },
            });
        }
        y += row_h;

        // Meter mode selector (dB / LUFS)
        let meter_mode = self.params.meter_mode.value();
        let meter_mode_idx = match meter_mode {
            crate::MeterMode::Db => 0,
            crate::MeterMode::Lufs => 1,
        };
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + font_size,
            "Mode",
            font_size,
            widgets::color_muted(),
        );
        let mode_sel_x = pad + label_w;
        let mode_sel_y = y + 4.0 * s;
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            tr,
            mode_sel_x,
            mode_sel_y,
            slider_w,
            slider_h,
            &["dB", "LUFS"],
            meter_mode_idx,
        );
        let mode_seg_w = slider_w / 2.0;
        for i in 0..2 {
            self.hit_regions.push(HitRegion {
                x: mode_sel_x + i as f32 * mode_seg_w,
                y: mode_sel_y,
                w: mode_seg_w,
                h: slider_h,
                action: HitAction::SteppedSegment {
                    param: ParamId::MeterMode,
                    index: i,
                },
            });
        }
        y += row_h;

        // Helper: draw a labeled slider and register its hit region
        macro_rules! slider_row {
            ($label:expr, $param:expr, $param_id:expr, $value_text:expr) => {
                tr.draw_text(
                    &mut self.surface.pixmap,
                    pad,
                    y + font_size,
                    $label,
                    font_size,
                    widgets::color_muted(),
                );
                let sx = pad + label_w;
                let sy = y + 4.0 * s;
                widgets::draw_slider(
                    &mut self.surface.pixmap,
                    tr,
                    sx,
                    sy,
                    slider_w,
                    slider_h,
                    "",
                    $value_text,
                    $param.unmodulated_normalized_value(),
                );
                self.hit_regions.push(HitRegion {
                    x: sx,
                    y: sy,
                    w: slider_w,
                    h: slider_h,
                    action: HitAction::Slider($param_id),
                });
                y += row_h;
            };
        }

        let (active_gain, active_ref) = if meter_mode == crate::MeterMode::Lufs {
            (&self.params.gain_lufs, &self.params.reference_lufs)
        } else {
            (&self.params.gain, &self.params.reference_level)
        };

        let gain_db = nih_plug::util::gain_to_db(active_gain.value());
        let gain_text = if meter_mode == crate::MeterMode::Lufs {
            format!("{:.1} LU", gain_db)
        } else {
            format!("{:.1} dB", gain_db)
        };
        slider_row!("Gain", active_gain, ParamId::Gain, &gain_text);

        let ref_val = active_ref.value();
        let ref_text = if meter_mode == crate::MeterMode::Lufs {
            format!("{:.1} LUFS", ref_val)
        } else {
            format!("{:.1} dB", ref_val)
        };
        slider_row!("Reference", active_ref, ParamId::Reference, &ref_text);

        // RMS Window only in dB mode
        if meter_mode == crate::MeterMode::Db {
            let window_text = format!("{:.0} ms", self.params.rms_window_ms.value());
            slider_row!(
                "RMS Window",
                self.params.rms_window_ms,
                ParamId::RmsWindow,
                &window_text
            );
        }

        // Readings header
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + font_size + 2.0 * s,
            "Readings",
            font_size * 1.1,
            widgets::color_text(),
        );
        y += 30.0 * s;

        if meter_mode == crate::MeterMode::Db {
            // ── dB mode readings ──
            let gain_sources = [
                ("Peak Max", peak_db, GainSource::PeakMax),
                ("True Peak", true_peak_db, GainSource::TruePeak),
                ("RMS (Int)", rms_int_db, GainSource::RmsIntegrated),
                ("RMS (Mom)", rms_mom_db, GainSource::RmsMomentary),
                ("RMS Max", rms_max_db, GainSource::RmsMomentaryMax),
            ];

            for &(label, db, source) in &gain_sources {
                let val = format_db(db);
                tr.draw_text(
                    &mut self.surface.pixmap,
                    pad,
                    y + font_size,
                    label,
                    font_size,
                    widgets::color_muted(),
                );
                tr.draw_text(
                    &mut self.surface.pixmap,
                    pad + label_w + gap,
                    y + font_size,
                    &val,
                    font_size,
                    widgets::color_text(),
                );
                let bx = pad + label_w + gap + value_w + gap;
                let by = y + 2.0 * s;
                widgets::draw_button(
                    &mut self.surface.pixmap,
                    tr,
                    bx,
                    by,
                    btn_w,
                    btn_h,
                    "\u{2192} Gain",
                    false,
                    false,
                );
                self.hit_regions.push(HitRegion {
                    x: bx,
                    y: by,
                    w: btn_w,
                    h: btn_h,
                    action: HitAction::Button(ButtonAction::GainFromReading(source)),
                });
                y += row_h;
            }

            // Crest (no button)
            let crest_val = if crest_db <= -100.0 {
                "-- dB".to_string()
            } else {
                format!("{:.1} dB", crest_db)
            };
            tr.draw_text(
                &mut self.surface.pixmap,
                pad,
                y + font_size,
                "Crest",
                font_size,
                widgets::color_muted(),
            );
            tr.draw_text(
                &mut self.surface.pixmap,
                pad + label_w + gap,
                y + font_size,
                &crest_val,
                font_size,
                widgets::color_text(),
            );
            y += row_h;
        } else {
            // ── LUFS mode readings with gain-match buttons ──
            let lufs_integrated = MeterReadings::load_db(&self.readings.lufs_integrated);
            let lufs_short_term = MeterReadings::load_db(&self.readings.lufs_short_term);
            let lufs_short_term_max = MeterReadings::load_db(&self.readings.lufs_short_term_max);
            let lufs_momentary = MeterReadings::load_db(&self.readings.lufs_momentary);
            let lufs_momentary_max = MeterReadings::load_db(&self.readings.lufs_momentary_max);
            let lufs_true_peak = MeterReadings::load_db(&self.readings.true_peak_max_db);
            let lufs_range = MeterReadings::load_db(&self.readings.lufs_range);

            let lufs_gain_sources = [
                (
                    "Integrated",
                    format_lufs(lufs_integrated),
                    GainSource::LufsIntegrated,
                ),
                (
                    "Short-Term",
                    format_lufs(lufs_short_term),
                    GainSource::LufsShortTerm,
                ),
                (
                    "ST Max",
                    format_lufs(lufs_short_term_max),
                    GainSource::LufsShortTermMax,
                ),
                (
                    "Momentary",
                    format_lufs(lufs_momentary),
                    GainSource::LufsMomentary,
                ),
                (
                    "Mom Max",
                    format_lufs(lufs_momentary_max),
                    GainSource::LufsMomentaryMax,
                ),
                (
                    "True Peak",
                    format_dbtp(lufs_true_peak),
                    GainSource::LufsTruePeak,
                ),
            ];

            for (label, formatted, source) in &lufs_gain_sources {
                tr.draw_text(
                    &mut self.surface.pixmap,
                    pad,
                    y + font_size,
                    label,
                    font_size,
                    widgets::color_muted(),
                );
                tr.draw_text(
                    &mut self.surface.pixmap,
                    pad + label_w + gap,
                    y + font_size,
                    formatted,
                    font_size,
                    widgets::color_text(),
                );
                let bx = pad + label_w + gap + value_w + gap;
                let by = y + 2.0 * s;
                widgets::draw_button(
                    &mut self.surface.pixmap,
                    tr,
                    bx,
                    by,
                    btn_w,
                    btn_h,
                    "\u{2192} Gain",
                    false,
                    false,
                );
                self.hit_regions.push(HitRegion {
                    x: bx,
                    y: by,
                    w: btn_w,
                    h: btn_h,
                    action: HitAction::Button(ButtonAction::GainFromReading(*source)),
                });
                y += row_h;
            }

            // LRA (no gain-match button — it's a range, not an absolute level)
            let lra_val = format_lu(lufs_range);
            tr.draw_text(
                &mut self.surface.pixmap,
                pad,
                y + font_size,
                "LRA",
                font_size,
                widgets::color_muted(),
            );
            tr.draw_text(
                &mut self.surface.pixmap,
                pad + label_w + gap,
                y + font_size,
                &lra_val,
                font_size,
                widgets::color_text(),
            );
            y += row_h;
        }

        // Reset button
        let reset_x = pad;
        let reset_y = y + 2.0 * s;
        let reset_w = 100.0 * s;
        let reset_h = 28.0 * s;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            reset_x,
            reset_y,
            reset_w,
            reset_h,
            "Reset",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: reset_x,
            y: reset_y,
            w: reset_w,
            h: reset_h,
            action: HitAction::Button(ButtonAction::Reset),
        });
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        match id {
            ParamId::Gain => setter.begin_set_parameter(self.active_gain()),
            ParamId::Reference => setter.begin_set_parameter(self.active_reference()),
            ParamId::RmsWindow => setter.begin_set_parameter(&self.params.rms_window_ms),
            ParamId::ChannelMode => setter.begin_set_parameter(&self.params.channel_mode),
            ParamId::MeterMode => setter.begin_set_parameter(&self.params.meter_mode),
        }
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        match id {
            ParamId::Gain => setter.set_parameter_normalized(self.active_gain(), normalized),
            ParamId::Reference => {
                setter.set_parameter_normalized(self.active_reference(), normalized)
            }
            ParamId::RmsWindow => {
                setter.set_parameter_normalized(&self.params.rms_window_ms, normalized)
            }
            ParamId::ChannelMode => {
                setter.set_parameter_normalized(&self.params.channel_mode, normalized)
            }
            ParamId::MeterMode => {
                setter.set_parameter_normalized(&self.params.meter_mode, normalized)
            }
        }
    }

    fn set_param_stepped(&self, setter: &ParamSetter, id: ParamId, index: i32) {
        if id == ParamId::ChannelMode {
            let mode = match index {
                0 => crate::ChannelMode::Stereo,
                1 => crate::ChannelMode::Left,
                _ => crate::ChannelMode::Right,
            };
            setter.set_parameter(&self.params.channel_mode, mode);
        } else if id == ParamId::MeterMode {
            let mode = match index {
                0 => crate::MeterMode::Db,
                _ => crate::MeterMode::Lufs,
            };
            setter.set_parameter(&self.params.meter_mode, mode);
        }
    }

    fn reset_param_to_default(&self, setter: &ParamSetter, id: ParamId) {
        use nih_plug::prelude::Param;
        match id {
            ParamId::Gain => {
                let gain = self.active_gain();
                setter.begin_set_parameter(gain);
                setter.set_parameter_normalized(gain, gain.default_normalized_value());
                setter.end_set_parameter(gain);
            }
            ParamId::Reference => {
                let reference = self.active_reference();
                setter.begin_set_parameter(reference);
                setter.set_parameter_normalized(reference, reference.default_normalized_value());
                setter.end_set_parameter(reference);
            }
            ParamId::RmsWindow => {
                setter.begin_set_parameter(&self.params.rms_window_ms);
                setter.set_parameter_normalized(
                    &self.params.rms_window_ms,
                    self.params.rms_window_ms.default_normalized_value(),
                );
                setter.end_set_parameter(&self.params.rms_window_ms);
            }
            ParamId::ChannelMode => {
                setter.begin_set_parameter(&self.params.channel_mode);
                setter.set_parameter_normalized(
                    &self.params.channel_mode,
                    self.params.channel_mode.default_normalized_value(),
                );
                setter.end_set_parameter(&self.params.channel_mode);
            }
            ParamId::MeterMode => {
                setter.begin_set_parameter(&self.params.meter_mode);
                setter.set_parameter_normalized(
                    &self.params.meter_mode,
                    self.params.meter_mode.default_normalized_value(),
                );
                setter.end_set_parameter(&self.params.meter_mode);
            }
        }
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        match id {
            ParamId::Gain => setter.end_set_parameter(self.active_gain()),
            ParamId::Reference => setter.end_set_parameter(self.active_reference()),
            ParamId::RmsWindow => setter.end_set_parameter(&self.params.rms_window_ms),
            ParamId::ChannelMode => setter.end_set_parameter(&self.params.channel_mode),
            ParamId::MeterMode => setter.end_set_parameter(&self.params.meter_mode),
        }
    }

    fn apply_scale_change(&mut self, delta: f32, window: &mut baseview::Window) {
        let old = self.scale_factor;
        self.scale_factor = (self.scale_factor + delta).clamp(0.75, 3.0);
        if (self.scale_factor - old).abs() > 0.01 {
            // Update shared scale so Editor::size() returns the correct value
            self.shared_scale.store(self.scale_factor);
            let new_w = (WINDOW_WIDTH as f32 * self.scale_factor).round() as u32;
            let new_h = (WINDOW_HEIGHT as f32 * self.scale_factor).round() as u32;
            self.params.editor_state.store_size(new_w, new_h);
            window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            self.gui_context.request_resize();
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_match_reference_minus_reading() {
        // Reference -14 LUFS, reading -20 LUFS -> need +6 dB gain
        assert_eq!(gain_match_db(-14.0, -20.0), Some(6.0));
    }

    #[test]
    fn test_gain_match_negative_gain() {
        // Reference -23 LUFS, reading -14 LUFS -> need -9 dB gain (too loud)
        assert_eq!(gain_match_db(-23.0, -14.0), Some(-9.0));
    }

    #[test]
    fn test_gain_match_zero_when_matched() {
        // Already at target -> 0 dB gain
        assert_eq!(gain_match_db(-14.0, -14.0), Some(0.0));
    }

    #[test]
    fn test_gain_match_invalid_reading_returns_none() {
        // Reading at or below floor -> no valid measurement
        assert_eq!(gain_match_db(-14.0, -100.0), None);
        assert_eq!(gain_match_db(-14.0, -200.0), None);
    }

    #[test]
    fn test_gain_match_just_above_floor() {
        // Reading just above -100 dB floor -> valid measurement
        let result = gain_match_db(-14.0, -99.99);
        assert!(result.is_some());
        let gain = result.unwrap();
        assert!((gain - 85.99).abs() < 0.02);
    }

    #[test]
    fn test_gain_match_works_for_db_mode_too() {
        // dB mode: reference 0 dBFS, peak at -3 dB -> need +3 dB
        assert_eq!(gain_match_db(0.0, -3.0), Some(3.0));
    }

    #[test]
    fn test_gain_match_positive_reading() {
        // Reading above 0 (clipping) -> large negative gain
        assert_eq!(gain_match_db(-14.0, 2.0), Some(-16.0));
    }
}

/// Compute the gain adjustment (in dB) needed to match a meter reading to a reference level.
/// Returns None if the reading is below the -100 dB floor (no valid measurement).
/// Works identically for dB and LUFS modes since both are absolute dB-scale units.
/// Note: the returned value may exceed the gain parameter's range (-40..+40 dB);
/// nih-plug's parameter system will clamp it when applied.
fn gain_match_db(reference: f32, meter_reading: f32) -> Option<f32> {
    if meter_reading <= -100.0 {
        None
    } else {
        Some(reference - meter_reading)
    }
}

fn format_db(db: f32) -> String {
    if db <= -100.0 {
        "-inf dB".to_string()
    } else {
        format!("{:.1} dB", db)
    }
}

fn format_lufs(val: f32) -> String {
    if val <= -100.0 {
        "-- LUFS".to_string()
    } else {
        format!("{:.1} LUFS", val)
    }
}

fn format_dbtp(val: f32) -> String {
    if val <= -100.0 {
        "-inf dBTP".to_string()
    } else {
        format!("{:.1} dBTP", val)
    }
}

fn format_lu(val: f32) -> String {
    if val <= -100.0 {
        "-- LU".to_string()
    } else {
        format!("{:.1} LU", val)
    }
}

impl baseview::WindowHandler for GsMeterWindow {
    fn on_frame(&mut self, _window: &mut baseview::Window) {
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        let _param_setter = ParamSetter::new(self.gui_context.as_ref());

        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;

                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) => {
                self.mouse_x = position.x as f32;
                self.mouse_y = position.y as f32;

                // Handle slider drag
                if let Some(param_id) = self.drag_active {
                    if let Some(region) = self
                        .hit_regions
                        .iter()
                        .find(|r| matches!(&r.action, HitAction::Slider(id) if *id == param_id))
                    {
                        let normalized = ((self.mouse_x - region.x) / region.w).clamp(0.0, 1.0);
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, normalized);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                let mx = self.mouse_x;
                let my = self.mouse_y;

                // Find which hit region was clicked
                let hit = self
                    .hit_regions
                    .iter()
                    .find(|r| mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h)
                    .cloned();

                if let Some(region) = hit {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    let now = std::time::Instant::now();
                    let is_double_click = now.duration_since(self.last_click_time).as_millis()
                        < 400
                        && self.last_click_action.as_ref() == Some(&region.action);
                    self.last_click_time = now;
                    self.last_click_action = Some(region.action);

                    // End any pending drag before processing new click
                    if let Some(prev_id) = self.drag_active.take() {
                        self.end_set_param(&setter, prev_id);
                    }

                    match region.action {
                        HitAction::Slider(param_id) => {
                            if is_double_click {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                self.drag_active = Some(param_id);
                                let normalized = ((mx - region.x) / region.w).clamp(0.0, 1.0);
                                self.begin_set_param(&setter, param_id);
                                self.set_param_normalized(&setter, param_id, normalized);
                            }
                        }
                        HitAction::SteppedSegment { param, index } => {
                            if is_double_click {
                                self.reset_param_to_default(&setter, param);
                            } else {
                                self.begin_set_param(&setter, param);
                                self.set_param_stepped(&setter, param, index);
                                self.end_set_param(&setter, param);
                            }
                        }
                        HitAction::Button(ButtonAction::ScaleDown) => {
                            self.apply_scale_change(-0.25, window);
                        }
                        HitAction::Button(ButtonAction::ScaleUp) => {
                            self.apply_scale_change(0.25, window);
                        }
                        HitAction::Button(ButtonAction::Reset) => {
                            self.should_reset.store(true, Ordering::Relaxed);
                        }
                        HitAction::Button(ButtonAction::GainFromReading(source)) => {
                            let meter_db = match source {
                                GainSource::PeakMax => {
                                    MeterReadings::load_db(&self.readings.peak_max_db)
                                }
                                GainSource::TruePeak => {
                                    MeterReadings::load_db(&self.readings.true_peak_max_db)
                                }
                                GainSource::RmsIntegrated => {
                                    MeterReadings::load_db(&self.readings.rms_integrated_db)
                                }
                                GainSource::RmsMomentary => {
                                    MeterReadings::load_db(&self.readings.rms_momentary_db)
                                }
                                GainSource::RmsMomentaryMax => {
                                    MeterReadings::load_db(&self.readings.rms_momentary_max_db)
                                }
                                GainSource::LufsIntegrated => {
                                    MeterReadings::load_db(&self.readings.lufs_integrated)
                                }
                                GainSource::LufsShortTerm => {
                                    MeterReadings::load_db(&self.readings.lufs_short_term)
                                }
                                GainSource::LufsShortTermMax => {
                                    MeterReadings::load_db(&self.readings.lufs_short_term_max)
                                }
                                GainSource::LufsMomentary => {
                                    MeterReadings::load_db(&self.readings.lufs_momentary)
                                }
                                GainSource::LufsMomentaryMax => {
                                    MeterReadings::load_db(&self.readings.lufs_momentary_max)
                                }
                                GainSource::LufsTruePeak => {
                                    MeterReadings::load_db(&self.readings.true_peak_max_db)
                                }
                            };
                            if let Some(target_gain_db) =
                                gain_match_db(self.active_reference().value(), meter_db)
                            {
                                let target_gain_linear = nih_plug::util::db_to_gain(target_gain_db);
                                let active_gain = self.active_gain();
                                let normalized = active_gain.preview_normalized(target_gain_linear);
                                setter.begin_set_parameter(active_gain);
                                setter.set_parameter_normalized(active_gain, normalized);
                                setter.end_set_parameter(active_gain);
                            }
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(param_id) = self.drag_active.take() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, param_id);
                }
            }
            baseview::Event::Keyboard(kb_event) => {
                use keyboard_types::{Key, KeyState, Modifiers};
                if kb_event.state == KeyState::Down
                    && kb_event.modifiers.contains(Modifiers::CONTROL)
                {
                    match &kb_event.key {
                        Key::Character(c) if c == "=" || c == "+" => {
                            self.apply_scale_change(0.25, window);
                        }
                        Key::Character(c) if c == "-" => {
                            self.apply_scale_change(-0.25, window);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ──────────────────────────────────────────

pub(crate) struct GsMeterEditor {
    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
    /// Shared with GsMeterWindow so Editor::size() reflects runtime changes.
    scaling_factor: Arc<AtomicCell<f32>>,
}

pub(crate) fn create(
    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    // NOTE: persisted state may not be restored yet (host calls create() before set()).
    // Scale factor is derived from persisted size in spawn() instead.
    Some(Box::new(GsMeterEditor {
        params,
        readings,
        should_reset,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
    }))
}

impl Editor for GsMeterEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        // Derive scale factor from persisted size (restored by host before spawn).
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.75, 3.0);
        self.scaling_factor.store(sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let readings = Arc::clone(&self.readings);
        let should_reset = Arc::clone(&self.should_reset);
        let shared_scale = Arc::clone(&self.scaling_factor);

        let scaled_w = persisted_w;
        let scaled_h = persisted_h;

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("GS Meter"),
                size: baseview::Size::new(scaled_w as f64, scaled_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                GsMeterWindow::new(
                    window,
                    gui_context,
                    params,
                    readings,
                    should_reset,
                    shared_scale,
                    sf,
                )
            },
        );

        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }

    fn size(&self) -> (u32, u32) {
        let sf = self.scaling_factor.load();
        let w = (WINDOW_WIDTH as f32 * sf).round() as u32;
        let h = (WINDOW_HEIGHT as f32 * sf).round() as u32;
        (w, h)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(factor);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}
