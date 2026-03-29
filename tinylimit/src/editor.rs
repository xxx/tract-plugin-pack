//! Softbuffer-based editor for tinylimit. CPU rendering via tiny-skia.

use baseview::{WindowHandle, WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use serde::{Deserialize, Serialize};
use std::num::{NonZeroIsize, NonZeroU32};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tiny_skia_widgets as widgets;
use crate::{MeterReadings, TinylimitParams};

const WINDOW_WIDTH: u32 = 500;
const WINDOW_HEIGHT: u32 = 600;

// ── Editor State (persisted by the host) ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct TinylimitEditorState {
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    size: AtomicCell<(u32, u32)>,
    #[serde(skip)]
    open: AtomicBool,
}

impl TinylimitEditorState {
    pub fn from_size(width: u32, height: u32) -> Arc<Self> {
        Arc::new(Self {
            size: AtomicCell::new((width, height)),
            open: AtomicBool::new(false),
        })
    }

    pub fn default_state() -> Arc<Self> {
        Self::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
    }

    pub fn size(&self) -> (u32, u32) {
        self.size.load()
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }
}

impl<'a> PersistentField<'a, TinylimitEditorState> for Arc<TinylimitEditorState> {
    fn set(&self, new_value: TinylimitEditorState) {
        self.size.store(new_value.size.load());
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&TinylimitEditorState) -> R,
    {
        f(self)
    }
}

// ── Hit testing ─────────────────────────────────────────────────────────

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
    Dial(ParamId),
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq)]
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

#[derive(Clone, Copy, PartialEq)]
enum ButtonAction {
    ScaleDown,
    ScaleUp,
    ToggleIsp,
    ToggleGainLink,
    PresetPrev,
    PresetNext,
    PresetApply,
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
    LimiterPreset { name: "Aggressive",  attack_ms: 0.5,  release_ms: 50.0,  knee_db: 0.0, transient_pct: 35.0 },
    LimiterPreset { name: "Loud",        attack_ms: 0.5,  release_ms: 40.0,  knee_db: 0.0, transient_pct: 60.0 },
    LimiterPreset { name: "Punchy",      attack_ms: 1.0,  release_ms: 100.0, knee_db: 2.0, transient_pct: 50.0 },
    LimiterPreset { name: "Safe",        attack_ms: 10.0, release_ms: 500.0, knee_db: 6.0, transient_pct: 75.0 },
    LimiterPreset { name: "Smooth",      attack_ms: 7.0,  release_ms: 400.0, knee_db: 8.0, transient_pct: 70.0 },
    LimiterPreset { name: "Transparent", attack_ms: 5.0,  release_ms: 300.0, knee_db: 4.0, transient_pct: 45.0 },
    LimiterPreset { name: "Vocal",       attack_ms: 3.0,  release_ms: 150.0, knee_db: 4.0, transient_pct: 20.0 },
];

// ── Window Handler ──────────────────────────────────────────────────────

struct TinylimitWindow {
    gui_context: Arc<dyn GuiContext>,
    _sb_context: softbuffer::Context<SoftbufferHandleAdapter>,
    sb_surface: softbuffer::Surface<SoftbufferHandleAdapter, SoftbufferHandleAdapter>,
    pixmap: tiny_skia::Pixmap,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Shared with TinylimitEditor so Editor::size() stays in sync.
    shared_scale: Arc<AtomicCell<f32>>,

    params: Arc<TinylimitParams>,
    readings: Arc<MeterReadings>,
    text_renderer: widgets::TextRenderer,

    /// Hit regions rebuilt each frame during draw().
    hit_regions: Vec<HitRegion>,
    /// Currently dragging a dial.
    drag_active: Option<HitAction>,
    /// Y coordinate where the current drag started.
    drag_start_y: f32,
    /// Normalized value (0.0–1.0) when the current drag started.
    drag_start_value: f32,
    /// Shift state from the last mouse event.
    last_shift_state: bool,
    /// When shift is pressed mid-drag, snapshot current Y and normalized value.
    granular_drag_start_y: f32,
    granular_drag_start_value: f32,
    /// Mouse position in physical pixels.
    mouse_x: f32,
    mouse_y: f32,
    /// Timestamp of last click for double-click detection.
    last_click_time: std::time::Instant,
    last_click_action: Option<HitAction>,
    /// Currently selected preset index (editor-only state, not persisted).
    current_preset: usize,
}

impl TinylimitWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<TinylimitParams>,
        readings: Arc<MeterReadings>,
        shared_scale: Arc<AtomicCell<f32>>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let target = baseview_window_to_surface_target(window);
        let sb_context =
            softbuffer::Context::new(target.clone()).expect("could not get softbuffer context");
        let mut sb_surface = softbuffer::Surface::new(&sb_context, target)
            .expect("could not create softbuffer surface");
        sb_surface
            .resize(NonZeroU32::new(pw).unwrap(), NonZeroU32::new(ph).unwrap())
            .unwrap();

        let pixmap = tiny_skia::Pixmap::new(pw, ph).expect("could not create pixmap");

        // Load embedded font
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            _sb_context: sb_context,
            sb_surface,
            pixmap,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            params,
            readings,
            text_renderer,
            hit_regions: Vec::new(),
            drag_active: None,
            drag_start_y: 0.0,
            drag_start_value: 0.0,
            last_shift_state: false,
            granular_drag_start_y: 0.0,
            granular_drag_start_value: 0.0,
            mouse_x: 0.0,
            mouse_y: 0.0,
            last_click_time: std::time::Instant::now(),
            last_click_action: None,
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
        self.hit_regions.clear();
        self.pixmap.fill(widgets::color_bg());

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
            row1_params[0], row1_params[1], row1_params[2], row1_params[3],
            row2_params[0], row2_params[1], row2_params[2], row2_params[3],
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
        let pct_text = format!("{}%", (self.scale_factor * 100.0).round() as u32);

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
        draw_meter_bar(&mut self.pixmap, in_l_x, meter_top, meter_bar_w, meter_h, in_peak_l);
        draw_meter_bar(&mut self.pixmap, in_r_x, meter_top, meter_bar_w, meter_h, in_peak_r);
        draw_meter_bar(&mut self.pixmap, out_l_x, meter_top, meter_bar_w, meter_h, out_peak_l);
        draw_meter_bar(&mut self.pixmap, out_r_x, meter_top, meter_bar_w, meter_h, out_peak_r);

        // Threshold indicator on input meters (spans both L/R bars)
        let thresh_indicator_x = in_l_x - 2.0 * s;
        let thresh_indicator_w = (in_r_x + meter_bar_w) - in_l_x + 4.0 * s;
        draw_meter_indicator(&mut self.pixmap, thresh_indicator_x, meter_top, thresh_indicator_w, meter_h, threshold_db);

        // Ceiling indicator on output meters (spans both L/R bars)
        let ceil_indicator_x = out_l_x - 2.0 * s;
        let ceil_indicator_w = (out_r_x + meter_bar_w) - out_l_x + 4.0 * s;
        draw_meter_indicator(&mut self.pixmap, ceil_indicator_x, meter_top, ceil_indicator_w, meter_h, ceiling_db);

        // Now borrow text_renderer for all text/widget drawing
        let tr = &mut self.text_renderer;

        // ── Title row: "tinylimit"  -  150%  + ──
        let title_y = 12.0 * s;
        tr.draw_text(
            &mut self.pixmap,
            pad,
            title_y + title_size,
            "tinylimit",
            title_size,
            widgets::color_text(),
        );

        let scale_btn_size = 22.0 * s;
        let scale_label_w = 44.0 * s;

        // "+" button (rightmost)
        let plus_x = w - pad - scale_btn_size;
        let plus_y = title_y + 2.0 * s;
        widgets::draw_button(
            &mut self.pixmap,
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
        let pct_x = plus_x - scale_label_w;
        let pct_text_w = tr.text_width(&pct_text, small_font);
        tr.draw_text(
            &mut self.pixmap,
            pct_x + (scale_label_w - pct_text_w) / 2.0,
            plus_y + small_font + 4.0 * s,
            &pct_text,
            small_font,
            widgets::color_muted(),
        );

        // "-" button
        let minus_x = pct_x - scale_btn_size;
        widgets::draw_button(
            &mut self.pixmap,
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

        // ── Meter labels ──
        let l_label = "L";
        let r_label = "R";

        // Input meter labels
        let in_label = "IN";
        let in_label_w = tr.text_width(in_label, font_size);
        tr.draw_text(
            &mut self.pixmap,
            in_label_x - in_label_w / 2.0,
            meter_top - 4.0 * s,
            in_label,
            font_size,
            widgets::color_muted(),
        );
        let l_label_w = tr.text_width(l_label, small_font);
        tr.draw_text(
            &mut self.pixmap,
            in_l_x + (meter_bar_w - l_label_w) / 2.0,
            meter_top - 4.0 * s - font_size - 2.0 * s,
            l_label,
            small_font,
            widgets::color_muted(),
        );
        let r_label_w = tr.text_width(r_label, small_font);
        tr.draw_text(
            &mut self.pixmap,
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
            &mut self.pixmap,
            in_l_x + (meter_bar_w - peak_lw) / 2.0,
            peak_text_y,
            &in_peak_text_l,
            small_font,
            widgets::color_muted(),
        );
        let peak_rw = tr.text_width(&in_peak_text_r, small_font);
        tr.draw_text(
            &mut self.pixmap,
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
            &mut self.pixmap,
            out_label_x - out_label_w / 2.0,
            meter_top - 4.0 * s,
            out_label,
            font_size,
            widgets::color_muted(),
        );
        let out_l_label_w = tr.text_width(l_label, small_font);
        tr.draw_text(
            &mut self.pixmap,
            out_l_x + (meter_bar_w - out_l_label_w) / 2.0,
            meter_top - 4.0 * s - font_size - 2.0 * s,
            l_label,
            small_font,
            widgets::color_muted(),
        );
        let out_r_label_w = tr.text_width(r_label, small_font);
        tr.draw_text(
            &mut self.pixmap,
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
            &mut self.pixmap,
            out_l_x + (meter_bar_w - out_peak_lw) / 2.0,
            peak_text_y,
            &out_peak_text_l,
            small_font,
            widgets::color_muted(),
        );
        let out_peak_rw = tr.text_width(&out_peak_text_r, small_font);
        tr.draw_text(
            &mut self.pixmap,
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
            widgets::draw_dial(
                &mut self.pixmap,
                tr,
                cx,
                dial_row_cy1,
                dial_radius,
                label,
                value_text,
                normalized,
            );
            let hit_w = dial_col_spacing;
            let hit_h = meter_h * 0.35;
            self.hit_regions.push(HitRegion {
                x: cx - hit_w / 2.0,
                y: dial_row_cy1 - hit_h / 2.0,
                w: hit_w,
                h: hit_h,
                action: HitAction::Dial(pid),
            });
        }

        // Row 2: last 4 params
        for (i, &(pid, label, normalized, ref value_text)) in dial_data[4..].iter().enumerate() {
            let cx = center_x + dial_col_spacing * (i as f32 + 0.5);
            widgets::draw_dial(
                &mut self.pixmap,
                tr,
                cx,
                dial_row_cy2,
                dial_radius,
                label,
                value_text,
                normalized,
            );
            let hit_w = dial_col_spacing;
            let hit_h = meter_h * 0.35;
            self.hit_regions.push(HitRegion {
                x: cx - hit_w / 2.0,
                y: dial_row_cy2 - hit_h / 2.0,
                w: hit_w,
                h: hit_h,
                action: HitAction::Dial(pid),
            });
        }

        // ── Preset selector: [<]  Name  [>] ──
        let center_mid = center_x + center_w / 2.0;
        let preset_arrow_w = 28.0 * s;
        let preset_arrow_h = 24.0 * s;
        let preset_label_w = 110.0 * s;
        let preset_total_w = preset_arrow_w * 2.0 + preset_label_w + 4.0 * s * 2.0;
        let preset_row_y = dial_row_cy2 + dial_radius + 45.0 * s;
        let preset_left_x = center_mid - preset_total_w / 2.0;

        // Left arrow "<"
        widgets::draw_button(
            &mut self.pixmap,
            tr,
            preset_left_x,
            preset_row_y,
            preset_arrow_w,
            preset_arrow_h,
            "<",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: preset_left_x,
            y: preset_row_y,
            w: preset_arrow_w,
            h: preset_arrow_h,
            action: HitAction::Button(ButtonAction::PresetPrev),
        });

        // Preset name label (clickable — applies the preset)
        let preset_name_x = preset_left_x + preset_arrow_w + 4.0 * s;
        widgets::draw_button(
            &mut self.pixmap,
            tr,
            preset_name_x,
            preset_row_y,
            preset_label_w,
            preset_arrow_h,
            PRESETS[self.current_preset].name,
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: preset_name_x,
            y: preset_row_y,
            w: preset_label_w,
            h: preset_arrow_h,
            action: HitAction::Button(ButtonAction::PresetApply),
        });
        // Right arrow ">"
        let preset_right_x = preset_name_x + preset_label_w + 4.0 * s;
        widgets::draw_button(
            &mut self.pixmap,
            tr,
            preset_right_x,
            preset_row_y,
            preset_arrow_w,
            preset_arrow_h,
            ">",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: preset_right_x,
            y: preset_row_y,
            w: preset_arrow_w,
            h: preset_arrow_h,
            action: HitAction::Button(ButtonAction::PresetNext),
        });

        // ── Toggle buttons: ISP and Gain Link ──
        let btn_w = 80.0 * s;
        let btn_h = 26.0 * s;
        let btn_gap = 12.0 * s;
        let btn_row_y = meter_bottom + 4.0 * s;

        let isp_x = center_mid - btn_w - btn_gap / 2.0;
        widgets::draw_button(
            &mut self.pixmap,
            tr,
            isp_x,
            btn_row_y,
            btn_w,
            btn_h,
            "ISP",
            isp_active,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: isp_x,
            y: btn_row_y,
            w: btn_w,
            h: btn_h,
            action: HitAction::Button(ButtonAction::ToggleIsp),
        });

        let gl_x = center_mid + btn_gap / 2.0;
        widgets::draw_button(
            &mut self.pixmap,
            tr,
            gl_x,
            btn_row_y,
            btn_w,
            btn_h,
            "Gain Link",
            gl_active,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: gl_x,
            y: btn_row_y,
            w: btn_w,
            h: btn_h,
            action: HitAction::Button(ButtonAction::ToggleGainLink),
        });

        // ── GR readout ──
        let gr_text = if gr_db.abs() < 0.05 {
            "GR: 0.0 dB".to_string()
        } else {
            format!("GR: {:.1} dB", gr_db.abs())
        };
        let gr_y = btn_row_y + btn_h + 10.0 * s;
        let gr_w = tr.text_width(&gr_text, font_size);
        tr.draw_text(
            &mut self.pixmap,
            center_mid - gr_w / 2.0,
            gr_y + font_size,
            &gr_text,
            font_size,
            widgets::color_accent(),
        );
    }

    fn apply_scale_change(&mut self, delta: f32, window: &mut baseview::Window) {
        let old = self.scale_factor;
        self.scale_factor = (self.scale_factor + delta).clamp(0.75, 3.0);
        if (self.scale_factor - old).abs() > 0.01 {
            self.shared_scale.store(self.scale_factor);
            let new_w = (WINDOW_WIDTH as f32 * self.scale_factor).round() as u32;
            let new_h = (WINDOW_HEIGHT as f32 * self.scale_factor).round() as u32;
            self.params.editor_state.size.store((new_w, new_h));
            window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            self.gui_context.request_resize();
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        if let Some(new_pixmap) = tiny_skia::Pixmap::new(pw, ph) {
            self.pixmap = new_pixmap;
        }
        let _ = self.sb_surface.resize(
            NonZeroU32::new(pw).unwrap(),
            NonZeroU32::new(ph).unwrap(),
        );
        self.params.editor_state.size.store((
            pw,
            ph,
        ));
    }

    fn present(&mut self) {
        let mut buffer = self.sb_surface.buffer_mut().unwrap();
        let data = self.pixmap.data();
        // Convert tiny-skia premultiplied RGBA to softbuffer 0x00RRGGBB
        for (dst, src) in buffer.iter_mut().zip(data.chunks_exact(4)) {
            let r = src[0] as u32;
            let g = src[1] as u32;
            let b = src[2] as u32;
            *dst = 0xFF000000 | (r << 16) | (g << 8) | b;
        }
        buffer.present().unwrap();
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
    fn on_frame(&mut self, _window: &mut baseview::Window) {
        self.draw();
        self.present();
    }

    fn on_event(
        &mut self,
        window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, modifiers }) => {
                self.mouse_x = position.x as f32;
                self.mouse_y = position.y as f32;

                if let Some(HitAction::Dial(param_id)) = self.drag_active {
                    let shift_now = modifiers.contains(keyboard_types::Modifiers::SHIFT);

                    // Get the current normalized value for detecting shift transitions
                    let current_norm = self.float_param(param_id).unmodulated_normalized_value();
                    if shift_now && !self.last_shift_state {
                        self.granular_drag_start_y = self.mouse_y;
                        self.granular_drag_start_value = current_norm;
                    } else if !shift_now && self.last_shift_state {
                        self.drag_start_y = self.mouse_y;
                        self.drag_start_value = current_norm;
                    }

                    // Drag sensitivity: 600px = full normalized range (0..1), up = increase
                    let target_norm = if shift_now {
                        let delta_y = self.granular_drag_start_y - self.mouse_y;
                        (self.granular_drag_start_value + delta_y / 600.0 * 0.1)
                            .clamp(0.0, 1.0)
                    } else {
                        let delta_y = self.drag_start_y - self.mouse_y;
                        (self.drag_start_value + delta_y / 600.0)
                            .clamp(0.0, 1.0)
                    };

                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.set_param_normalized(&setter, param_id, target_norm);

                    self.last_shift_state = shift_now;
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
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
                    let is_double_click =
                        now.duration_since(self.last_click_time).as_millis() < 400
                            && self.last_click_action.as_ref() == Some(&region.action);
                    self.last_click_time = now;
                    self.last_click_action = Some(region.action);

                    // End any pending drag before processing new click
                    if let Some(HitAction::Dial(id)) = self.drag_active.take() {
                        self.end_set_param(&setter, id);
                    }

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double_click {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                let norm = self.float_param(param_id).unmodulated_normalized_value();
                                self.drag_start_y = my;
                                self.drag_start_value = norm;
                                self.granular_drag_start_y = my;
                                self.granular_drag_start_value = norm;
                                self.last_shift_state = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag_active = Some(HitAction::Dial(param_id));
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
                        HitAction::Button(ButtonAction::ScaleDown) => {
                            self.apply_scale_change(-0.25, window);
                        }
                        HitAction::Button(ButtonAction::ScaleUp) => {
                            self.apply_scale_change(0.25, window);
                        }
                        HitAction::Button(ButtonAction::PresetPrev) => {
                            self.current_preset = if self.current_preset == 0 {
                                PRESETS.len() - 1
                            } else {
                                self.current_preset - 1
                            };
                        }
                        HitAction::Button(ButtonAction::PresetNext) => {
                            self.current_preset = (self.current_preset + 1) % PRESETS.len();
                        }
                        HitAction::Button(ButtonAction::PresetApply) => {
                            self.apply_preset(&setter);
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(HitAction::Dial(id)) = self.drag_active.take() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, id);
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

pub(crate) struct TinylimitEditor {
    params: Arc<TinylimitParams>,
    readings: Arc<MeterReadings>,
    /// Shared with TinylimitWindow so Editor::size() reflects runtime changes.
    scaling_factor: Arc<AtomicCell<f32>>,
}

pub(crate) fn create(
    params: Arc<TinylimitParams>,
    readings: Arc<MeterReadings>,
) -> Option<Box<dyn Editor>> {
    // NOTE: persisted state may not be restored yet (host calls create() before set()).
    // Scale factor is derived from persisted size in spawn() instead.
    Some(Box::new(TinylimitEditor {
        params,
        readings,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
    }))
}

impl Editor for TinylimitEditor {
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
        let shared_scale = Arc::clone(&self.scaling_factor);

        let scaled_w = persisted_w;
        let scaled_h = persisted_h;

        let window = baseview::Window::open_parented(
            &ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("tinylimit"),
                size: baseview::Size::new(scaled_w as f64, scaled_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                TinylimitWindow::new(window, gui_context, params, readings, shared_scale, sf)
            },
        );

        self.params
            .editor_state
            .open
            .store(true, Ordering::Release);
        Box::new(TinylimitEditorHandle {
            state: self.params.editor_state.clone(),
            window,
        })
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

struct TinylimitEditorHandle {
    state: Arc<TinylimitEditorState>,
    window: WindowHandle,
}

/// # Safety
///
/// The WindowHandle is created by baseview from the host-provided parent window
/// and is only used on the GUI thread. The `Send` bound is required by nih-plug's
/// `Editor::spawn` return type. This is the same pattern used by gain-brain and
/// gs-meter and is safe as long as the handle is not accessed from multiple threads
/// simultaneously, which nih-plug guarantees.
unsafe impl Send for TinylimitEditorHandle {}

impl Drop for TinylimitEditorHandle {
    fn drop(&mut self) {
        self.state.open.store(false, Ordering::Release);
        self.window.close();
    }
}

// ── Raw window handle adapters ──────────────────────────────────────────

struct ParentWindowHandleAdapter(nih_plug::editor::ParentWindowHandle);

unsafe impl HasRawWindowHandle for ParentWindowHandleAdapter {
    fn raw_window_handle(&self) -> RawWindowHandle {
        match self.0 {
            ParentWindowHandle::X11Window(window) => {
                let mut handle = raw_window_handle::XcbWindowHandle::empty();
                handle.window = window;
                RawWindowHandle::Xcb(handle)
            }
            ParentWindowHandle::AppKitNsView(ns_view) => {
                let mut handle = raw_window_handle::AppKitWindowHandle::empty();
                handle.ns_view = ns_view;
                RawWindowHandle::AppKit(handle)
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                let mut handle = raw_window_handle::Win32WindowHandle::empty();
                handle.hwnd = hwnd;
                RawWindowHandle::Win32(handle)
            }
        }
    }
}

#[derive(Clone)]
struct SoftbufferHandleAdapter {
    raw_display_handle: raw_window_handle_06::RawDisplayHandle,
    raw_window_handle: raw_window_handle_06::RawWindowHandle,
}

impl raw_window_handle_06::HasDisplayHandle for SoftbufferHandleAdapter {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle_06::DisplayHandle<'_>, raw_window_handle_06::HandleError> {
        unsafe {
            Ok(raw_window_handle_06::DisplayHandle::borrow_raw(
                self.raw_display_handle,
            ))
        }
    }
}

impl raw_window_handle_06::HasWindowHandle for SoftbufferHandleAdapter {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle_06::WindowHandle<'_>, raw_window_handle_06::HandleError> {
        unsafe {
            Ok(raw_window_handle_06::WindowHandle::borrow_raw(
                self.raw_window_handle,
            ))
        }
    }
}

fn baseview_window_to_surface_target(window: &baseview::Window<'_>) -> SoftbufferHandleAdapter {
    use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};

    let raw_display = window.raw_display_handle();
    let raw_window = window.raw_window_handle();

    SoftbufferHandleAdapter {
        raw_display_handle: match raw_display {
            raw_window_handle::RawDisplayHandle::AppKit(_) => {
                raw_window_handle_06::RawDisplayHandle::AppKit(
                    raw_window_handle_06::AppKitDisplayHandle::new(),
                )
            }
            raw_window_handle::RawDisplayHandle::Xlib(handle) => {
                raw_window_handle_06::RawDisplayHandle::Xlib(
                    raw_window_handle_06::XlibDisplayHandle::new(
                        NonNull::new(handle.display),
                        handle.screen,
                    ),
                )
            }
            raw_window_handle::RawDisplayHandle::Xcb(handle) => {
                raw_window_handle_06::RawDisplayHandle::Xcb(
                    raw_window_handle_06::XcbDisplayHandle::new(
                        NonNull::new(handle.connection),
                        handle.screen,
                    ),
                )
            }
            raw_window_handle::RawDisplayHandle::Windows(_) => {
                raw_window_handle_06::RawDisplayHandle::Windows(
                    raw_window_handle_06::WindowsDisplayHandle::new(),
                )
            }
            _ => todo!("Unsupported display handle"),
        },
        raw_window_handle: match raw_window {
            raw_window_handle::RawWindowHandle::AppKit(handle) => {
                raw_window_handle_06::RawWindowHandle::AppKit(
                    raw_window_handle_06::AppKitWindowHandle::new(
                        NonNull::new(handle.ns_view).unwrap(),
                    ),
                )
            }
            raw_window_handle::RawWindowHandle::Xlib(handle) => {
                raw_window_handle_06::RawWindowHandle::Xlib(
                    raw_window_handle_06::XlibWindowHandle::new(handle.window),
                )
            }
            raw_window_handle::RawWindowHandle::Xcb(handle) => {
                raw_window_handle_06::RawWindowHandle::Xcb(
                    raw_window_handle_06::XcbWindowHandle::new(
                        NonZeroU32::new(handle.window)
                            .expect("XCB window handle is 0 -- host provided invalid parent"),
                    ),
                )
            }
            raw_window_handle::RawWindowHandle::Win32(handle) => {
                let mut raw_handle = raw_window_handle_06::Win32WindowHandle::new(
                    NonZeroIsize::new(handle.hwnd as isize).unwrap(),
                );
                raw_handle.hinstance = NonZeroIsize::new(handle.hinstance as isize);
                raw_window_handle_06::RawWindowHandle::Win32(raw_handle)
            }
            _ => todo!("Unsupported window handle"),
        },
    }
}
