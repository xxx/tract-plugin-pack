//! Softbuffer-based editor for tinylimit. CPU rendering via tiny-skia.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::{MeterReadings, TinylimitParams};
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 500;
const WINDOW_HEIGHT: u32 = 600;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit testing ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum HitAction {
    Dial(ParamId),
    Button(ButtonAction),
    /// The preset dropdown's collapsed trigger.
    PresetDropdown,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ButtonAction {
    ToggleIsp,
    ToggleGainLink,
}

/// Identifies a dropdown for `DropdownState`. tinylimit has exactly one.
#[derive(Clone, Copy, PartialEq, Debug)]
enum DropdownId {
    Preset,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ParamId {
    Input,
    Threshold,
    Ceiling,
    Attack,
    Release,
    Knee,
    StereoLink,
    Transient,
}

// ── Preset data ──────────────────────────────────────────────────────────

struct LimiterPreset {
    name: &'static str,
    attack_ms: f32,
    release_ms: f32,
    knee_db: f32,
    transient_pct: f32,
}

const PRESETS: &[LimiterPreset] = &[
    LimiterPreset {
        name: "Aggressive",
        attack_ms: 0.5,
        release_ms: 50.0,
        knee_db: 0.0,
        transient_pct: 35.0,
    },
    LimiterPreset {
        name: "Loud",
        attack_ms: 0.5,
        release_ms: 40.0,
        knee_db: 0.0,
        transient_pct: 60.0,
    },
    LimiterPreset {
        name: "Punchy",
        attack_ms: 1.0,
        release_ms: 100.0,
        knee_db: 2.0,
        transient_pct: 50.0,
    },
    LimiterPreset {
        name: "Safe",
        attack_ms: 10.0,
        release_ms: 500.0,
        knee_db: 6.0,
        transient_pct: 75.0,
    },
    LimiterPreset {
        name: "Smooth",
        attack_ms: 7.0,
        release_ms: 400.0,
        knee_db: 8.0,
        transient_pct: 70.0,
    },
    LimiterPreset {
        name: "Transparent",
        attack_ms: 5.0,
        release_ms: 300.0,
        knee_db: 4.0,
        transient_pct: 45.0,
    },
    LimiterPreset {
        name: "Vocal",
        attack_ms: 3.0,
        release_ms: 150.0,
        knee_db: 4.0,
        transient_pct: 20.0,
    },
];

/// Preset display names, as a slice for the dropdown widget.
fn preset_names() -> Vec<&'static str> {
    PRESETS.iter().map(|p| p.name).collect()
}

// ── Window Handler ──────────────────────────────────────────────────────

struct TinylimitWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Packed (w << 32 | h) pending host-initiated resize, read on next frame.
    pending_resize: Arc<std::sync::atomic::AtomicU64>,

    params: Arc<TinylimitParams>,
    readings: Arc<MeterReadings>,
    text_renderer: widgets::TextRenderer,

    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,
    dropdown: widgets::DropdownState<DropdownId>,
    /// Currently selected preset index (editor-only state, not persisted).
    current_preset: usize,
}

impl TinylimitWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<TinylimitParams>,
        readings: Arc<MeterReadings>,
        pending_resize: Arc<std::sync::atomic::AtomicU64>,
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
            pending_resize,
            params,
            readings,
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            dropdown: widgets::DropdownState::new(),
            current_preset: 0,
        }
    }

    // ── Param access helpers ────────────────────────────────────────────

    /// Get a reference to the FloatParam for a given ParamId.
    fn float_param(&self, id: ParamId) -> &FloatParam {
        match id {
            ParamId::Input => &self.params.input,
            ParamId::Threshold => &self.params.threshold,
            ParamId::Ceiling => &self.params.ceiling,
            ParamId::Attack => &self.params.attack,
            ParamId::Release => &self.params.release,
            ParamId::Knee => &self.params.knee,
            ParamId::StereoLink => &self.params.stereo_link,
            ParamId::Transient => &self.params.transient_mix,
        }
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.begin_set_parameter(self.float_param(id));
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        setter.set_parameter_normalized(self.float_param(id), normalized);
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.end_set_parameter(self.float_param(id));
    }

    fn reset_param_to_default(&self, setter: &ParamSetter, id: ParamId) {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        setter.begin_set_parameter(p);
        setter.set_parameter_normalized(p, p.default_normalized_value());
        setter.end_set_parameter(p);
    }

    fn formatted_value_without_unit(&self, id: ParamId) -> String {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        let v = p.modulated_normalized_value();
        p.normalized_value_to_string(v, false)
    }

    fn commit_text_edit(&mut self) {
        use nih_plug::prelude::Param;
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let HitAction::Dial(param_id) = action else {
            return;
        };
        let p = self.float_param(param_id);
        let norm = p.string_to_normalized_value(&text);
        let Some(norm) = norm else { return };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        self.begin_set_param(&setter, param_id);
        self.set_param_normalized(&setter, param_id, norm);
        self.end_set_param(&setter, param_id);
    }

    /// Format a parameter's current value for display.
    fn format_value(&self, id: ParamId) -> String {
        self.float_param(id).to_string()
    }

    /// Apply the current preset's values to attack, release, knee, and transient_mix.
    fn apply_preset(&self, setter: &ParamSetter) {
        let preset = &PRESETS[self.current_preset];

        setter.begin_set_parameter(&self.params.attack);
        setter.set_parameter(&self.params.attack, preset.attack_ms);
        setter.end_set_parameter(&self.params.attack);

        setter.begin_set_parameter(&self.params.release);
        setter.set_parameter(&self.params.release, preset.release_ms);
        setter.end_set_parameter(&self.params.release);

        setter.begin_set_parameter(&self.params.knee);
        setter.set_parameter(&self.params.knee, preset.knee_db);
        setter.end_set_parameter(&self.params.knee);

        setter.begin_set_parameter(&self.params.transient_mix);
        setter.set_parameter(&self.params.transient_mix, preset.transient_pct);
        setter.end_set_parameter(&self.params.transient_mix);
    }

    // ── Drawing ─────────────────────────────────────────────────────────

    fn draw(&mut self) {
        use nih_plug::prelude::Param;

        let s = self.scale_factor;

        // Clear hit regions and background
        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let pad = 16.0 * s;
        let title_size = 20.0 * s;
        let font_size = 12.0 * s;
        let small_font = 11.0 * s;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        // Pre-collect all parameter values before entering the text_renderer borrow.
        // Row 1: signal flow (levels + routing)
        // Row 2: limiter character (timing + shape)
        let row1_params: [(ParamId, &str); 4] = [
            (ParamId::Input, "Input"),
            (ParamId::Threshold, "Threshold"),
            (ParamId::Ceiling, "Ceiling"),
            (ParamId::StereoLink, "Link%"),
        ];
        let row2_params: [(ParamId, &str); 4] = [
            (ParamId::Attack, "Attack"),
            (ParamId::Release, "Release"),
            (ParamId::Knee, "Knee"),
            (ParamId::Transient, "Transient"),
        ];
        let all_params: [(ParamId, &str); 8] = [
            row1_params[0],
            row1_params[1],
            row1_params[2],
            row1_params[3],
            row2_params[0],
            row2_params[1],
            row2_params[2],
            row2_params[3],
        ];
        let dial_data: Vec<(ParamId, &str, f32, String)> = all_params
            .iter()
            .map(|&(pid, label)| {
                let normalized = self.float_param(pid).unmodulated_normalized_value();
                let value_text = self.format_value(pid);
                (pid, label, normalized, value_text)
            })
            .collect();

        // Pre-read meter values and toggle states
        let in_peak_l = MeterReadings::load_db(&self.readings.input_peak_l);
        let in_peak_r = MeterReadings::load_db(&self.readings.input_peak_r);
        let out_peak_l = MeterReadings::load_db(&self.readings.output_peak_l);
        let out_peak_r = MeterReadings::load_db(&self.readings.output_peak_r);
        let gr_db = MeterReadings::load_db(&self.readings.gain_reduction);
        let isp_active = self.params.isp.value();
        let gl_active = self.params.gain_link.value();
        // ── Layout zones ──
        let top_margin = 50.0 * s;
        let meter_area_w = 80.0 * s;
        let center_x = meter_area_w + pad;
        let center_w = w - 2.0 * (meter_area_w + pad);
        let meter_top = top_margin + 20.0 * s;
        let meter_bottom = h - 80.0 * s;
        let meter_h = meter_bottom - meter_top;
        let meter_bar_w = 16.0 * s;
        let meter_gap = 4.0 * s;

        // Compute meter positions
        let in_label_x = pad + meter_area_w / 2.0;
        let in_l_x = in_label_x - meter_bar_w - meter_gap / 2.0;
        let in_r_x = in_label_x + meter_gap / 2.0;
        let out_label_x = w - pad - meter_area_w / 2.0;
        let out_l_x = out_label_x - meter_bar_w - meter_gap / 2.0;
        let out_r_x = out_label_x + meter_gap / 2.0;

        // Read threshold and ceiling for meter indicators
        let threshold_db = self.params.threshold.value();
        let ceiling_db = self.params.ceiling.value();

        // Draw meter bars first (no text_renderer needed)
        draw_meter_bar(
            &mut self.surface.pixmap,
            in_l_x,
            meter_top,
            meter_bar_w,
            meter_h,
            in_peak_l,
        );
        draw_meter_bar(
            &mut self.surface.pixmap,
            in_r_x,
            meter_top,
            meter_bar_w,
            meter_h,
            in_peak_r,
        );
        draw_meter_bar(
            &mut self.surface.pixmap,
            out_l_x,
            meter_top,
            meter_bar_w,
            meter_h,
            out_peak_l,
        );
        draw_meter_bar(
            &mut self.surface.pixmap,
            out_r_x,
            meter_top,
            meter_bar_w,
            meter_h,
            out_peak_r,
        );

        // Threshold indicator on input meters (spans both L/R bars)
        let thresh_indicator_x = in_l_x - 2.0 * s;
        let thresh_indicator_w = (in_r_x + meter_bar_w) - in_l_x + 4.0 * s;
        draw_meter_indicator(
            &mut self.surface.pixmap,
            thresh_indicator_x,
            meter_top,
            thresh_indicator_w,
            meter_h,
            threshold_db,
        );

        // Ceiling indicator on output meters (spans both L/R bars)
        let ceil_indicator_x = out_l_x - 2.0 * s;
        let ceil_indicator_w = (out_r_x + meter_bar_w) - out_l_x + 4.0 * s;
        draw_meter_indicator(
            &mut self.surface.pixmap,
            ceil_indicator_x,
            meter_top,
            ceil_indicator_w,
            meter_h,
            ceiling_db,
        );

        // Now borrow text_renderer for all text/widget drawing
        let tr = &mut self.text_renderer;

        // ── Title ──
        let title_y = 12.0 * s;
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            title_y + title_size,
            "tinylimit",
            title_size,
            widgets::color_text(),
        );

        // ── Meter labels ──
        let l_label = "L";
        let r_label = "R";

        // Input meter labels
        let in_label = "IN";
        let in_label_w = tr.text_width(in_label, font_size);
        tr.draw_text(
            &mut self.surface.pixmap,
            in_label_x - in_label_w / 2.0,
            meter_top - 4.0 * s,
            in_label,
            font_size,
            widgets::color_muted(),
        );
        let l_label_w = tr.text_width(l_label, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            in_l_x + (meter_bar_w - l_label_w) / 2.0,
            meter_top - 4.0 * s - font_size - 2.0 * s,
            l_label,
            small_font,
            widgets::color_muted(),
        );
        let r_label_w = tr.text_width(r_label, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            in_r_x + (meter_bar_w - r_label_w) / 2.0,
            meter_top - 4.0 * s - font_size - 2.0 * s,
            r_label,
            small_font,
            widgets::color_muted(),
        );

        // Input peak text below meters
        let in_peak_text_l = format_db(in_peak_l);
        let in_peak_text_r = format_db(in_peak_r);
        let peak_text_y = meter_bottom + font_size + 4.0 * s;
        let peak_lw = tr.text_width(&in_peak_text_l, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            in_l_x + (meter_bar_w - peak_lw) / 2.0,
            peak_text_y,
            &in_peak_text_l,
            small_font,
            widgets::color_muted(),
        );
        let peak_rw = tr.text_width(&in_peak_text_r, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            in_r_x + (meter_bar_w - peak_rw) / 2.0,
            peak_text_y,
            &in_peak_text_r,
            small_font,
            widgets::color_muted(),
        );

        // Output meter labels
        let out_label = "OUT";
        let out_label_w = tr.text_width(out_label, font_size);
        tr.draw_text(
            &mut self.surface.pixmap,
            out_label_x - out_label_w / 2.0,
            meter_top - 4.0 * s,
            out_label,
            font_size,
            widgets::color_muted(),
        );
        let out_l_label_w = tr.text_width(l_label, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            out_l_x + (meter_bar_w - out_l_label_w) / 2.0,
            meter_top - 4.0 * s - font_size - 2.0 * s,
            l_label,
            small_font,
            widgets::color_muted(),
        );
        let out_r_label_w = tr.text_width(r_label, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            out_r_x + (meter_bar_w - out_r_label_w) / 2.0,
            meter_top - 4.0 * s - font_size - 2.0 * s,
            r_label,
            small_font,
            widgets::color_muted(),
        );

        // Output peak text below meters
        let out_peak_text_l = format_db(out_peak_l);
        let out_peak_text_r = format_db(out_peak_r);
        let out_peak_lw = tr.text_width(&out_peak_text_l, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            out_l_x + (meter_bar_w - out_peak_lw) / 2.0,
            peak_text_y,
            &out_peak_text_l,
            small_font,
            widgets::color_muted(),
        );
        let out_peak_rw = tr.text_width(&out_peak_text_r, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            out_r_x + (meter_bar_w - out_peak_rw) / 2.0,
            peak_text_y,
            &out_peak_text_r,
            small_font,
            widgets::color_muted(),
        );

        // ── Center area: 8 dials in 2 rows of 4 ──
        // Row 1: signal flow (Input, Thresh, Ceiling, Link%)
        // Row 2: limiter character (Attack, Release, Knee, Transient)
        let dial_radius = 28.0 * s;
        let dial_col_spacing = center_w / 4.0;
        let dial_row_cy1 = meter_top + meter_h * 0.25;
        let dial_row_cy2 = meter_top + meter_h * 0.60;

        // Row 1: first 4 params
        for (i, &(pid, label, normalized, ref value_text)) in dial_data[..4].iter().enumerate() {
            let cx = center_x + dial_col_spacing * (i as f32 + 0.5);
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(pid))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                cx,
                dial_row_cy1,
                dial_radius,
                label,
                value_text,
                normalized,
                None,
                editing_buf.as_deref(),
                caret,
            );
            let hit_w = dial_col_spacing;
            // Bound the dial's visual extent (circle + value readout). The old
            // `meter_h * 0.35` was far taller than the dial and bled into the
            // preset dropdown row below it, stealing clicks meant for the trigger.
            let hit_h = dial_radius * 3.0;
            self.drag.push_region(
                cx - hit_w / 2.0,
                dial_row_cy1 - hit_h / 2.0,
                hit_w,
                hit_h,
                HitAction::Dial(pid),
            );
        }

        // Row 2: last 4 params
        for (i, &(pid, label, normalized, ref value_text)) in dial_data[4..].iter().enumerate() {
            let cx = center_x + dial_col_spacing * (i as f32 + 0.5);
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(pid))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                cx,
                dial_row_cy2,
                dial_radius,
                label,
                value_text,
                normalized,
                None,
                editing_buf.as_deref(),
                caret,
            );
            let hit_w = dial_col_spacing;
            // Bound the dial's visual extent (circle + value readout). The old
            // `meter_h * 0.35` was far taller than the dial and bled into the
            // preset dropdown row below it, stealing clicks meant for the trigger.
            let hit_h = dial_radius * 3.0;
            self.drag.push_region(
                cx - hit_w / 2.0,
                dial_row_cy2 - hit_h / 2.0,
                hit_w,
                hit_h,
                HitAction::Dial(pid),
            );
        }

        // ── Preset dropdown ──
        let center_mid = center_x + center_w / 2.0;
        let preset_trigger_w = 160.0 * s;
        let preset_trigger_h = 24.0 * s;
        let preset_row_y = dial_row_cy2 + dial_radius + 45.0 * s;
        let preset_trigger_x = center_mid - preset_trigger_w / 2.0;

        widgets::draw_dropdown_trigger(
            &mut self.surface.pixmap,
            tr,
            (
                preset_trigger_x,
                preset_row_y,
                preset_trigger_w,
                preset_trigger_h,
            ),
            PRESETS[self.current_preset].name,
            self.dropdown.is_open_for(DropdownId::Preset),
        );
        self.drag.push_region(
            preset_trigger_x,
            preset_row_y,
            preset_trigger_w,
            preset_trigger_h,
            HitAction::PresetDropdown,
        );

        // ── Toggle buttons: ISP and Gain Link ──
        let btn_w = 80.0 * s;
        let btn_h = 26.0 * s;
        let btn_gap = 12.0 * s;
        let btn_row_y = meter_bottom + 4.0 * s;

        let isp_x = center_mid - btn_w - btn_gap / 2.0;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            isp_x,
            btn_row_y,
            btn_w,
            btn_h,
            "ISP",
            isp_active,
            false,
        );
        self.drag.push_region(
            isp_x,
            btn_row_y,
            btn_w,
            btn_h,
            HitAction::Button(ButtonAction::ToggleIsp),
        );

        let gl_x = center_mid + btn_gap / 2.0;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            gl_x,
            btn_row_y,
            btn_w,
            btn_h,
            "Gain Link",
            gl_active,
            false,
        );
        self.drag.push_region(
            gl_x,
            btn_row_y,
            btn_w,
            btn_h,
            HitAction::Button(ButtonAction::ToggleGainLink),
        );

        // ── GR readout ──
        let gr_text = if gr_db.abs() < 0.05 {
            "GR: 0.0 dB".to_string()
        } else {
            format!("GR: {:.1} dB", gr_db.abs())
        };
        let gr_y = btn_row_y + btn_h + 10.0 * s;
        let gr_w = tr.text_width(&gr_text, font_size);
        tr.draw_text(
            &mut self.surface.pixmap,
            center_mid - gr_w / 2.0,
            gr_y + font_size,
            &gr_text,
            font_size,
            widgets::color_accent(),
        );

        // Preset dropdown popup — drawn last so it overlays every other widget.
        let preset_names = preset_names();
        widgets::draw_dropdown_popup(
            &mut self.surface.pixmap,
            tr,
            &self.dropdown,
            &preset_names,
            (self.physical_width as f32, self.physical_height as f32),
        );
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

/// Draw a single vertical meter bar. dB range: -60 (bottom) to 0 (top).
fn draw_meter_bar(pixmap: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, db: f32) {
    // Background
    widgets::draw_rect(pixmap, x, y, w, h, widgets::color_control_bg());

    // Fill height from dB
    let db_clamped = db.clamp(-60.0, 0.0);
    let fraction = (db_clamped + 60.0) / 60.0; // 0.0 at -60 dB, 1.0 at 0 dB
    let fill_h = h * fraction;
    if fill_h > 0.5 {
        let fill_y = y + h - fill_h;
        let color = if db > -0.1 {
            tiny_skia::Color::from_rgba8(0xff, 0x44, 0x44, 0xff) // red for clipping
        } else {
            widgets::color_accent()
        };
        widgets::draw_rect(pixmap, x, fill_y, w, fill_h, color);
    }

    // Border
    widgets::draw_rect_outline(pixmap, x, y, w, h, widgets::color_border(), 1.0);
}

/// Draw a horizontal indicator line on a meter at a given dB level.
/// Used for threshold (on input meters) and ceiling (on output meters).
fn draw_meter_indicator(pixmap: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, db: f32) {
    let db_clamped = db.clamp(-60.0, 0.0);
    let fraction = (db_clamped + 60.0) / 60.0;
    let line_y = y + h - h * fraction;

    // Draw a 2px horizontal line in a visible color (yellow/orange)
    let indicator_color = tiny_skia::Color::from_rgba8(0xff, 0xc0, 0x40, 0xff);
    widgets::draw_rect(pixmap, x, line_y - 1.0, w, 2.0, indicator_color);
}

/// Format a dB value for meter display.
fn format_db(db: f32) -> String {
    if db <= -100.0 {
        "-inf".to_string()
    } else {
        format!("{:.1}", db)
    }
}

impl baseview::WindowHandler for TinylimitWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        let packed = self.pending_resize.swap(0, Ordering::Relaxed);
        if packed != 0 {
            let new_w = (packed >> 32) as u32;
            let new_h = (packed & 0xFFFF_FFFF) as u32;
            if new_w > 0
                && new_h > 0
                && (new_w != self.physical_width || new_h != self.physical_height)
            {
                window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            }
        }

        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.drag.set_mouse(position.x as f32, position.y as f32);
                if self.dropdown.is_open() {
                    let names = preset_names();
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    self.dropdown
                        .on_mouse_move(position.x as f32, position.y as f32, &names, win);
                }
                if let Some(HitAction::Dial(param_id)) = self.drag.active_action().copied() {
                    let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    let current = self.float_param(param_id).unmodulated_normalized_value();
                    if let Some(norm) = self.drag.update_drag(shift, current) {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, norm);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                // While the preset dropdown is open it owns every click:
                // selecting a row applies the preset, clicking outside closes.
                if self.dropdown.is_open() {
                    let (mx, my) = self.drag.mouse_pos();
                    let names = preset_names();
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    if let Some(widgets::DropdownEvent::Selected(_, idx)) =
                        self.dropdown.on_mouse_down(mx, my, &names, win)
                    {
                        self.current_preset = idx;
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.apply_preset(&setter);
                    }
                    return baseview::EventStatus::Captured;
                }

                // Auto-commit any in-flight edit before starting a drag
                self.commit_text_edit();

                if let Some(region) = self.drag.hit_test().cloned() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());

                    // End any pending drag before processing new click
                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_set_param(&setter, id);
                    }

                    let is_double = self.drag.check_double_click(&region.action);

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                let norm =
                                    self.float_param(param_id).unmodulated_normalized_value();
                                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag.begin_drag(HitAction::Dial(param_id), norm, shift);
                                self.begin_set_param(&setter, param_id);
                            }
                        }
                        HitAction::Button(ButtonAction::ToggleIsp) => {
                            let current = self.params.isp.value();
                            setter.begin_set_parameter(&self.params.isp);
                            setter.set_parameter(&self.params.isp, !current);
                            setter.end_set_parameter(&self.params.isp);
                        }
                        HitAction::Button(ButtonAction::ToggleGainLink) => {
                            let current = self.params.gain_link.value();
                            setter.begin_set_parameter(&self.params.gain_link);
                            setter.set_parameter(&self.params.gain_link, !current);
                            setter.end_set_parameter(&self.params.gain_link);
                        }
                        HitAction::PresetDropdown => {
                            let win = (self.physical_width as f32, self.physical_height as f32);
                            self.dropdown.open(
                                DropdownId::Preset,
                                (region.x, region.y, region.w, region.h),
                                PRESETS.len(),
                                self.current_preset,
                                false, // 7 presets — no typeahead filter
                                win,
                            );
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                self.dropdown.on_mouse_up();
                if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, id);
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                // Ignore right-click during an active drag.
                if self.drag.active_action().is_some() {
                    return baseview::EventStatus::Captured;
                }
                if let Some(region) = self.drag.hit_test().cloned() {
                    // Auto-commit any in-flight edit on a different widget.
                    self.commit_text_edit();
                    if let HitAction::Dial(param_id) = region.action {
                        let initial = self.formatted_value_without_unit(param_id);
                        self.text_edit.begin(HitAction::Dial(param_id), &initial);
                    }
                }
            }
            baseview::Event::Keyboard(ev) if self.dropdown.is_open() => {
                if ev.state != keyboard_types::KeyState::Down {
                    // Swallow key-up so the host doesn't see Enter/Esc releases.
                    return baseview::EventStatus::Captured;
                }
                let dd_key = match &ev.key {
                    keyboard_types::Key::ArrowUp => Some(widgets::DropdownKey::Up),
                    keyboard_types::Key::ArrowDown => Some(widgets::DropdownKey::Down),
                    keyboard_types::Key::Home => Some(widgets::DropdownKey::Home),
                    keyboard_types::Key::End => Some(widgets::DropdownKey::End),
                    keyboard_types::Key::PageUp => Some(widgets::DropdownKey::PageUp),
                    keyboard_types::Key::PageDown => Some(widgets::DropdownKey::PageDown),
                    keyboard_types::Key::Enter => Some(widgets::DropdownKey::Enter),
                    keyboard_types::Key::Escape => Some(widgets::DropdownKey::Esc),
                    _ => None,
                };
                if let Some(dd_key) = dd_key {
                    let names = preset_names();
                    let win = (self.physical_width as f32, self.physical_height as f32);
                    if let Some(widgets::DropdownEvent::Selected(_, idx)) =
                        self.dropdown.on_key(dd_key, &names, win)
                    {
                        self.current_preset = idx;
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.apply_preset(&setter);
                    }
                }
                return baseview::EventStatus::Captured;
            }
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                if ev.state != keyboard_types::KeyState::Down {
                    // Swallow key-up events while editing so the host DAW doesn't
                    // process Enter/Escape releases as its own shortcuts.
                    return baseview::EventStatus::Captured;
                }
                match &ev.key {
                    keyboard_types::Key::Character(s) => {
                        for c in s.chars() {
                            self.text_edit.insert_char(c);
                        }
                    }
                    keyboard_types::Key::Backspace => self.text_edit.backspace(),
                    keyboard_types::Key::Escape => self.text_edit.cancel(),
                    keyboard_types::Key::Enter => {
                        self.commit_text_edit();
                    }
                    _ => return baseview::EventStatus::Ignored,
                }
                return baseview::EventStatus::Captured;
            }
            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ──────────────────────────────────────────

pub(crate) struct TinylimitEditor {
    params: Arc<TinylimitParams>,
    readings: Arc<MeterReadings>,
    /// Packed (w << 32 | h) for host-initiated resize, consumed by window on next frame.
    pending_resize: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) fn create(
    params: Arc<TinylimitParams>,
    readings: Arc<MeterReadings>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(TinylimitEditor {
        params,
        readings,
        pending_resize: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    }))
}

impl Editor for TinylimitEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let readings = Arc::clone(&self.readings);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("tinylimit"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                TinylimitWindow::new(window, gui_context, params, readings, pending_resize, sf)
            },
        );

        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }

    fn size(&self) -> (u32, u32) {
        self.params.editor_state.size()
    }

    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let packed = ((width as u64) << 32) | (height as u64);
        self.pending_resize.store(packed, Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod text_entry_tests {
    use super::*;

    #[test]
    fn text_edit_roundtrip_for_threshold_action() {
        let mut text_edit: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Threshold))
            .is_none());

        text_edit.begin(HitAction::Dial(ParamId::Threshold), "-6.0");
        assert_eq!(
            text_edit.active_for(&HitAction::Dial(ParamId::Threshold)),
            Some("-6.0")
        );

        text_edit.insert_char('0');
        assert_eq!(
            text_edit.active_for(&HitAction::Dial(ParamId::Threshold)),
            Some("-6.00")
        );

        let (action, buffer) = text_edit.commit().unwrap();
        assert_eq!(action, HitAction::Dial(ParamId::Threshold));
        assert_eq!(buffer, "-6.00");
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Threshold))
            .is_none());
    }

    #[test]
    fn state_starts_inactive() {
        let text_edit: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Input))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Threshold))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Ceiling))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Attack))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Release))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Knee))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::StereoLink))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Transient))
            .is_none());
    }
}
